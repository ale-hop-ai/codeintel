#!/usr/bin/env python3
"""The outcome eval's oracle: docs/agent-eval.md § Outcome eval.

  oracle.py T1              print T1's expected answer
  oracle.py --all --write   regenerate expected/*.txt

It never uses codeintel. Each recipe edits a scratch copy of the corpus and
lets the compiler or the test runner say what moved:

  callers     `#[deprecated]` on the target; every use warns, in every crate
              and target, and the build still succeeds, so one crate's error
              cannot hide another crate's uses. Each warning's line maps to
              its enclosing `fn`.
  tests       a marker write at the target's entry; every test runs alone,
              and the ones that leave the marker executed it.
  unused_pub  `#[deprecated]` on every `pub` item of a crate at once; an item
              no warning outside that crate's src/ names is used nowhere else.
  impls       `#[deprecated]` on the trait; the warnings on `impl` lines name
              the implementing types.
  grep        `rg -F -l`.
  named       the answer is the target itself.

Lints are capped to warnings (`--cap-lints=warn`), so a `deny` in the
corpus's lint table cannot turn a deprecation into a stopped build.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
CORPORA = Path(os.environ.get("CODEINTEL_CORPORA",
                              Path.home() / ".cache/codeintel/corpus"))
TARGETS = Path.home() / ".cache/codeintel/oracle-target"


def die(message):
    sys.exit(f"oracle.py: {message}")


def load_tasks():
    tasks = {}
    lines = (HERE / "tasks.tsv").read_text().splitlines()
    head = lines[0].split("\t")
    for line in lines[1:]:
        if line.strip() and not line.startswith("#"):
            row = dict(zip(head, line.split("\t")))
            tasks[row["id"]] = row
    return tasks


def corpus_dir(name):
    path = CORPORA / name
    if not path.is_dir():
        die(f"{path} is not checked out; `run.py corpora` prepares it")
    return path


def scratch_copy(src):
    """A copy without `target/`: the edit must not touch the pinned corpus.

    Every file is stamped *now* (`shutil.copy`, not `copy2`). The copies share
    one cargo target directory, and cargo shares artifacts between copies of
    one workspace: a copy that kept the corpus's old mtimes looked older than
    the previous recipe's edited build, so cargo reused it, and a baseline
    test run carried the last task's `panic!()`."""
    dst = Path(tempfile.mkdtemp(prefix="oracle-")) / "tree"
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns("target", ".git"),
                    copy_function=shutil.copy)
    return dst


def cargo(tree, corpus, args):
    env = dict(os.environ, RUSTFLAGS="--cap-lints=warn",
               CARGO_TARGET_DIR=str(TARGETS / corpus), CARGO_TERM_COLOR="never")
    return subprocess.run(["cargo", *args], cwd=tree, env=env,
                          capture_output=True, text=True)


# --- Rust source, read just well enough to find `fn` bodies -------------------

def blank_noise(text):
    """`text` with comments, strings and char literals replaced by spaces,
    newlines kept, so brace matching and line numbers stay exact."""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        elif (m := re.match(r'(b?r)(#*)"', text[i:])) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            close = '"' + m.group(2)
            j = text.find(close, i + len(m.group(0)))
            j = n if j < 0 else j + len(close)
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            j += 1
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        elif c == "'" and (m := re.match(r"'(\\.[^']*|[^\\'\n])'", text[i:])):
            out.append(" " * len(m.group(0)))
            i += len(m.group(0))
        else:
            out.append(c)
            i += 1
    return "".join(out)


def items(path):
    """[(name, first_line, last_line, is_test)] for every `fn` with a body,
    1-based inclusive. `is_test`: inside a `#[cfg(test)]` module or carrying
    `#[test]`."""
    text = blank_noise(Path(path).read_text())
    line_at = [0]
    for ch in text:
        line_at.append(line_at[-1] + (ch == "\n"))

    def line(pos):
        return text.count("\n", 0, pos) + 1

    def body_end(open_pos):
        depth = 0
        for j in range(open_pos, len(text)):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    return j
        return len(text) - 1

    test_spans = []
    for m in re.finditer(r"#\[cfg\(test\)\]\s*(pub(\([^)]*\))?\s+)?mod\s+\w+\s*\{", text):
        test_spans.append((line(m.start()), line(body_end(m.end() - 1))))
    found = []
    for m in re.finditer(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)", text):
        j = m.end()
        while j < len(text) and text[j] not in "{;":
            j += 1
        if j >= len(text) or text[j] == ";":
            continue
        first, last = line(m.start()), line(body_end(j))
        before = text[max(0, m.start() - 300):m.start()]
        is_test = "#[test]" in before.split("fn ")[-1] or any(a <= first <= b for a, b in test_spans)
        found.append((m.group(1), first, last, is_test))
    return found


def enclosing_fn(path, line_no):
    """The innermost `fn` whose body covers `line_no`, or None."""
    best = None
    for name, first, last, is_test in items(path):
        if first <= line_no <= last and (best is None or first >= best[1]):
            best = (name, first, last, is_test)
    return best


def def_line(tree, rel, pattern, at=None):
    """The 0-based index of the one line matching `pattern`; with `at`, the
    1-based line it must be on (for a name defined twice in one file)."""
    lines = (tree / rel).read_text().splitlines()
    hits = [i for i, l in enumerate(lines) if re.search(pattern, l)]
    if at is not None:
        hits = [i for i in hits if i + 1 == at]
    if len(hits) != 1:
        die(f"{rel}: expected one definition matching {pattern!r}, found {len(hits)}")
    return hits[0]


def insert_before(tree, rel, index, text):
    path = tree / rel
    lines = path.read_text().splitlines(keepends=True)
    indent = re.match(r"\s*", lines[index]).group(0)
    lines.insert(index, indent + text + "\n")
    path.write_text("".join(lines))


def deprecated_uses(tree, corpus, notes=False):
    """{(rel_path, line)} of every `deprecated` warning, all targets; with
    `notes`, {(rel_path, line, note)}, the note being the `OE<n>` tag."""
    out = cargo(tree, corpus, ["check", "--workspace", "--all-targets",
                               "--message-format=json", "--quiet"])
    uses = set()
    for raw in out.stdout.splitlines():
        try:
            msg = json.loads(raw)
        except json.JSONDecodeError:
            continue
        m = msg.get("message") or {}
        if msg.get("reason") != "compiler-message" or (m.get("code") or {}).get("code") != "deprecated":
            continue
        for span in m.get("spans", []):
            if span.get("is_primary"):
                p = Path(span["file_name"])
                rel = p.relative_to(tree) if p.is_absolute() else p
                # A deprecated struct's fields inherit its deprecation, so
                # `value.field` warns with the struct's note while naming
                # nothing (T3 asks for names and calls).
                if re.match(r"use of deprecated field", m.get("message", "")):
                    continue
                if notes:
                    tag = re.search(r": (OE\d+)$", m.get("message", ""))
                    uses.add((str(rel), span["line_start"], tag.group(1) if tag else ""))
                else:
                    uses.add((str(rel), span["line_start"]))
    if out.returncode != 0:
        die(f"cargo check failed in the edited copy:\n{out.stderr[-3000:]}")
    return uses


# --- recipes -----------------------------------------------------------------

def split_target(target):
    """`path::name` or `path::name@line` -> (path, name, line or None)."""
    rel, name = target.rsplit("::", 1)
    name, _, at = name.partition("@")
    return rel, name, int(at) if at else None


def fn_pattern(name):
    return rf"^\s*(pub(\([^)]*\))?\s+)?((const|async|unsafe)\s+)*fn\s+{re.escape(name)}\b"


def callers(task):
    """`path::fn` of every non-test fn under `src/` that uses the target."""
    rel, name, at = split_target(task["target"])
    tree = scratch_copy(corpus_dir(task["corpus"]))
    insert_before(tree, rel, def_line(tree, rel, fn_pattern(name), at), "#[deprecated]")
    found = set()
    for f, line_no in deprecated_uses(tree, task["corpus"]):
        if not re.search(r"(^|/)src/", f):
            continue
        hit = enclosing_fn(tree / f, line_no)
        if hit and not hit[3]:
            found.add(f"{f}::{hit[0]}")
    shutil.rmtree(tree.parent)
    return sorted(found)


def test_binaries(tree, corpus):
    """[(executable, package_dir)] for every test target `cargo test --tests`
    runs."""
    out = cargo(tree, corpus, ["test", "--workspace", "--tests", "--no-run",
                               "--message-format=json", "--quiet"])
    if out.returncode != 0:
        die(f"cargo test --no-run failed in the edited copy:\n{out.stderr[-3000:]}")
    found = []
    for raw in out.stdout.splitlines():
        try:
            msg = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if msg.get("reason") == "compiler-artifact" and msg.get("executable") \
                and (msg.get("profile") or {}).get("test"):
            found.append((msg["executable"], str(Path(msg["manifest_path"]).parent)))
    return sorted(set(found))


def listed(exe, cwd, ignored):
    args = [exe, "--list", "--format", "terse"] + (["--ignored"] if ignored else [])
    out = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    return {l[: -len(": test")] for l in out.stdout.splitlines() if l.endswith(": test")}


def tests(task):
    """Names of the tests during which the target ran, counted directly.

    The target's entry writes a marker file named by `ORACLE_TRACE`, and every
    test `cargo test --tests` runs (ignored ones are not) runs on its own with
    its own marker path. The environment reaches a binary a test spawns, so
    a test that runs the CLI counts if the CLI ran the target. Pass or fail
    does not matter. The first design injected `panic!()` and counted the
    tests that newly failed, and missed every test that asserts a failure:
    a panicking binary still fails, so the test still passes (fd's
    `assert_failure`, found by the pilot)."""
    rel, name, at = split_target(task["target"])
    corpus = task["corpus"]
    tree = scratch_copy(corpus_dir(corpus))
    path = tree / rel
    lines = path.read_text().splitlines(keepends=True)
    i = def_line(tree, rel, fn_pattern(name), at)
    while "{" not in blank_noise(lines[i]):
        i += 1
    lines.insert(i + 1, 'if let Some(p) = std::env::var_os("ORACLE_TRACE") '
                        '{ let _ = std::fs::write(p, b"x"); }\n')
    path.write_text("".join(lines))
    traces = Path(tempfile.mkdtemp(prefix="oracle-trace-"))
    jobs = []
    for exe, cwd in test_binaries(tree, corpus):
        for t in sorted(listed(exe, cwd, False) - listed(exe, cwd, True)):
            jobs.append((exe, cwd, t))

    def one(job):
        exe, cwd, t = job
        marker = traces / f"{len(t)}-{abs(hash((exe, t)))}"
        try:
            subprocess.run([exe, "--exact", t, "--test-threads=1", "-q"], cwd=cwd,
                           env=dict(os.environ, ORACLE_TRACE=str(marker)),
                           capture_output=True, timeout=600)
        except subprocess.TimeoutExpired:
            print(f"note: {t} timed out; counted only if it reached the target",
                  file=sys.stderr)
        return t.rsplit("::", 1)[-1] if marker.exists() else None

    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=8) as pool:
        hits = {h for h in pool.map(one, jobs) if h}
    print(f"note: {len(jobs)} test(s) run one by one, {len(hits)} reached the target",
          file=sys.stderr)
    shutil.rmtree(tree.parent)
    shutil.rmtree(traces)
    return sorted(hits)


PUB_ITEM = re.compile(r"^(\s*)pub\s+((const|async|unsafe)\s+)*(fn|struct|enum|trait|type|const|static)\s+([A-Za-z_][A-Za-z0-9_]*)")
INHERENT_IMPL = re.compile(r"\bimpl\s*(?:<[^{>]*>)?\s*([A-Za-z_]\w*)(?:<[^{]*>)?\s*(?:where[^{]*)?\{")


def blocks(text, pattern):
    """[(match, first_line, last_line)] for every brace block `pattern` opens.
    `text` is already blanked."""
    found = []
    for m in re.finditer(pattern, text):
        depth = 0
        for j in range(m.end() - 1, len(text)):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    break
        found.append((m, text.count("\n", 0, m.start()) + 1, text.count("\n", 0, j) + 1))
    return found


def unused_pub(task):
    """`path::Name` (`path::Type::method` inside an inherent impl) of every
    `pub` fn, struct, enum, trait, type, const or static under the crate's
    src/, outside `#[cfg(test)]` modules, that no code outside that src/ uses.
    Not modules, fields or re-exports. Each candidate gets its own
    `#[deprecated(note = "OE<n>")]`, and the note in a warning says which."""
    src = task["target"].rstrip("/")
    tree = scratch_copy(corpus_dir(task["corpus"]))
    candidates = {}
    for path in sorted((tree / src).rglob("*.rs")):
        rel = str(path.relative_to(tree))
        text = blank_noise(path.read_text())
        test_mods = [(a, b) for _m, a, b in blocks(
            text, r"#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")]
        impl_blocks = [(m.group(1), a, b) for m, a, b in blocks(text, INHERENT_IMPL)
                       if " for " not in m.group(0)]
        lines = path.read_text().splitlines(keepends=True)
        marks = []
        for i, l in enumerate(text.splitlines()):
            m = PUB_ITEM.match(l)
            if not m or any(a <= i + 1 <= b for a, b in test_mods):
                continue
            owner = [t for t, a, b in impl_blocks if a < i + 1 <= b]
            name = f"{owner[-1]}::{m.group(5)}" if owner else m.group(5)
            tag = f"OE{len(candidates)}"
            candidates[tag] = f"{rel}::{name}"
            marks.append((i, tag))
        for i, tag in reversed(marks):
            indent = re.match(r"\s*", lines[i]).group(0)
            lines.insert(i, f'{indent}#[deprecated(note = "{tag}")]\n')
        path.write_text("".join(lines))
    used = {}
    for f, line_no, note in sorted(deprecated_uses(tree, task["corpus"], notes=True)):
        if not f.startswith(src + "/") and note in candidates:
            used.setdefault(note, f"{f}:{line_no}")
    if os.environ.get("ORACLE_SITES"):
        for tag, site in sorted(used.items(), key=lambda kv: candidates[kv[0]]):
            print(f"used: {candidates[tag]} at {site}", file=sys.stderr)
    shutil.rmtree(tree.parent)
    return sorted(v for k, v in candidates.items() if k not in used)


def impls(task):
    """Type names on every `impl ... for T` line that names the target trait."""
    rel, name = task["target"].rsplit("::", 1)
    tree = scratch_copy(corpus_dir(task["corpus"]))
    insert_before(tree, rel, def_line(tree, rel, rf"^\s*(pub(\([^)]*\))?\s+)?trait\s+{re.escape(name)}\b"),
                  "#[deprecated]")
    found = set()
    for f, line_no in deprecated_uses(tree, task["corpus"]):
        text = (tree / f).read_text().splitlines()[line_no - 1]
        m = re.search(rf"\bimpl\b.*\b{re.escape(name)}\b.*\bfor\s+&?(?:'\w+\s+)?(?:mut\s+)?([A-Za-z_][\w:]*)", text)
        if m:
            found.add(m.group(1).rsplit("::", 1)[-1])
    shutil.rmtree(tree.parent)
    return sorted(found)


def grep(task):
    # `target/` is the build output the indexer left behind; the agents' copy
    # has none, so the oracle searches what they search.
    out = subprocess.run(["rg", "-F", "-l", "-g", "!/target", "--", task["target"], "."],
                         cwd=corpus_dir(task["corpus"]), capture_output=True, text=True)
    return sorted(p.removeprefix("./") for p in out.stdout.splitlines())


def named(task):
    return [task["target"]]


RECIPES = {"callers": callers, "tests": tests, "unused_pub": unused_pub,
           "impls": impls, "grep": grep, "named": named}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("ids", nargs="*")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--write", action="store_true", help="write expected/<id>.txt")
    a = ap.parse_args()
    tasks = load_tasks()
    ids = sorted(tasks) if a.all else a.ids
    if not ids:
        die("name a task, or --all")
    for tid in ids:
        if tid not in tasks:
            die(f"no task {tid} in tasks.tsv")
        answer = RECIPES[tasks[tid]["kind"]](tasks[tid])
        text = "".join(f"{x}\n" for x in answer)
        if a.write:
            (HERE / "expected").mkdir(exist_ok=True)
            (HERE / "expected" / f"{tid}.txt").write_text(text)
            print(f"{tid}: {len(answer)} line(s)")
        else:
            sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
