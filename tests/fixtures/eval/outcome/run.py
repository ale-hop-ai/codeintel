#!/usr/bin/env python3
"""The outcome eval: docs/agent-eval.md § Outcome eval, plan.md M7.

  run.py prepare                      pristine corpus copies + arm B's index
  run.py probe                        prove both arms' isolation (2 agents, ~$0.10)
  run.py pilot                        1 session per arm per task, capped at $8
  run.py run --model M --session-usd X --max-usd Y    the full run, N = 4

Two arms take the same task. Arm A has Read, Grep and Glob; arm B has the
same plus codeintel's one MCP tool. Prompt, model, corpus and caps are
byte-identical, and neither arm has a shell -- with one, `cargo check` would
be in reach, and that is the oracle (oracle.py). Every session is one
headless `claude -p` process with the system prompt replaced and the user's
customizations off; `probe` checks that against canaries rather than
asserting it.

Agents only read, so one pristine copy of each corpus serves every session of
both arms. Arm B's index is built from that copy with the SCIP output the
pinned corpus's indexers already wrote (`--scip`), so B's copy carries no
`target/` that A's lacks.
"""
import argparse
import json
import os
import random
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
sys.path.insert(0, str(HERE))
import oracle  # noqa: E402  (load_tasks, CORPORA)

WORK = Path(os.environ.get("OUTCOME_WORK", Path.home() / ".cache/codeintel/outcome"))
BIN = ROOT / "target/release/codeintel"
TOOL = "mcp__codeintel__code_query"

# The pinned corpora (docs/agent-eval.md § Corpora). `codeintel-981f795` is
# this repository at a commit from before the outcome eval existed, so no
# copy an agent reads holds the tasks or their answers.
CORPORA = {
    "codeintel-981f795": {"rev": "981f795"},
    "fd": {"rev": "b422e5d8"},
}

SYSTEM = ("You are working in a code repository: the current directory. "
          "Answer the task with the tools you have.")

PROMPT = """\
{text}

When you are done, end your reply with the answer as a fenced block, one \
entry per line and nothing else in it:

```answer
<entry>
<entry>
```

Each entry is {fmt}
"""

FORMATS = {
    "path::function": "`path/to/file.rs::function_name`: the path relative to the "
                      "repository root, then the bare function name (no type, no parentheses).",
    "path::item": "`path/to/file.rs::Name`, or `path/to/file.rs::Type::method` for a "
                  "method: the path relative to the repository root.",
    "test": "a test function's name only, for example `parses_empty_input`.",
    "type": "a type's name only, for example `MyType`.",
    "path": "a file path relative to the repository root.",
}

BASE = ["claude", "-p", "--output-format", "stream-json", "--verbose",
        "--disable-slash-commands", "--no-session-persistence",
        "--strict-mcp-config", "--system-prompt", SYSTEM]
TOOLS = "Read,Grep,Glob"


def die(message):
    sys.exit(f"run.py: {message}")


def git_head():
    out = subprocess.run(["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT,
                         capture_output=True, text=True)
    return out.stdout.strip()


# --- prepare -------------------------------------------------------------------

def copy_of(corpus):
    return WORK / "tree" / corpus


def index_dir(corpus):
    return WORK / "index" / corpus


def scip_files(pinned):
    """The SCIP indexes `codeintel index` wrote for the pinned corpus."""
    out = subprocess.run([str(BIN), "status", str(pinned)],
                         capture_output=True, text=True)
    store = re.search(r"^index: (\S+)", out.stdout, re.M)
    if not store:
        die(f"no index for {pinned}; run `codeintel index {pinned}` first\n{out.stdout}")
    found = sorted((Path(store.group(1)) / "scip").glob("*.scip"))
    if not found:
        die(f"{pinned} was indexed without running its indexers")
    return found


def prepare(_a):
    if not BIN.is_file():
        die(f"{BIN} is missing: cargo build --release -p codeintel")
    for corpus in CORPORA:
        pinned = oracle.corpus_dir(corpus)
        tree = copy_of(corpus)
        if tree.exists():
            shutil.rmtree(tree)
        shutil.copytree(pinned, tree, ignore=shutil.ignore_patterns("target", ".git"))
        idx = index_dir(corpus)
        if idx.exists():
            shutil.rmtree(idx)
        args = [str(BIN), "index", str(tree), "--index-dir", str(idx)]
        for scip in scip_files(pinned):
            args += ["--scip", str(scip)]
        began = time.monotonic()
        out = subprocess.run(args, capture_output=True, text=True)
        if out.returncode != 0:
            die(f"indexing {tree} failed:\n{out.stdout}{out.stderr}")
        status = subprocess.run([str(BIN), "status", str(tree), "--index-dir", str(idx)],
                                capture_output=True, text=True).stdout
        print(f"{corpus}: copy {tree}, index {idx} in {time.monotonic() - began:.1f}s")
        print("".join(f"  {l}\n" for l in status.splitlines()
                      if l.startswith(("scip", "anchored", "status"))), end="")
    return 0


# --- one session ---------------------------------------------------------------

def mcp_config(corpus):
    path = WORK / f"mcp-{corpus}.json"
    path.write_text(json.dumps({"mcpServers": {"codeintel": {
        "command": str(BIN),
        "args": ["mcp", "--path", str(copy_of(corpus)), "--index-dir", str(index_dir(corpus))],
    }}}))
    return path


def command(arm, corpus, model, session_usd, safe_mode):
    cmd = BASE + ["--model", model, "--max-budget-usd", str(session_usd), "--tools", TOOLS]
    cmd += ["--safe-mode"] if safe_mode else ["--setting-sources", ""]
    # Read, Grep and Glob are NOT pre-approved: a pre-approval covers every
    # path, and the probe read a canary outside the working directory with
    # it. Unapproved, they run inside the working directory and are denied
    # outside it, which is the sandbox both arms need.
    if arm == "B":
        cmd += ["--mcp-config", str(mcp_config(corpus)), "--allowedTools", TOOL]
    return cmd


def session(arm, corpus, prompt, model, session_usd, safe_mode, timeout, cwd=None):
    """One agent. Returns a summary dict: no transcript, only what the report
    and the score need."""
    began = time.monotonic()
    try:
        proc = subprocess.run(command(arm, corpus, model, session_usd, safe_mode),
                              input=prompt, capture_output=True, text=True,
                              cwd=cwd or copy_of(corpus), timeout=timeout)
        lines, err = proc.stdout.splitlines(), proc.stderr
    except subprocess.TimeoutExpired as e:
        lines = (e.stdout or b"").decode(errors="replace").splitlines() if isinstance(e.stdout, bytes) else (e.stdout or "").splitlines()
        err = f"timeout after {timeout}s"
    init, result, tools, statuses = {}, {}, {}, {}
    for raw in lines:
        try:
            ev = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "system" and ev.get("subtype") == "init":
            init = ev
        elif ev.get("type") == "result":
            result = ev
        elif ev.get("type") == "assistant":
            for part in (ev.get("message") or {}).get("content") or []:
                if part.get("type") == "tool_use":
                    tools[part["name"]] = tools.get(part["name"], 0) + 1
        elif ev.get("type") == "user":
            for part in (ev.get("message") or {}).get("content") or []:
                if part.get("type") == "tool_result":
                    text = json.dumps(part.get("content"))
                    m = re.search(r"status=([a-z-]+)", text) or re.search(r'\\"status\\": ?\\"([a-z-]+)', text)
                    if m:
                        statuses[m.group(1)] = statuses.get(m.group(1), 0) + 1
    usage = result.get("usage") or {}
    return {
        "arm": arm, "corpus": corpus, "model": model,
        "tools_offered": sorted(init.get("tools") or []),
        "mcp_servers": init.get("mcp_servers") or [],
        "result": result.get("result") or "",
        "subtype": result.get("subtype"), "is_error": bool(result.get("is_error")) or not result,
        "error": "" if result else (err or "no result event")[-500:],
        "turns": result.get("num_turns"), "usd": result.get("total_cost_usd") or 0.0,
        "tokens_in": usage.get("input_tokens", 0), "tokens_out": usage.get("output_tokens", 0),
        "cache_read": usage.get("cache_read_input_tokens", 0),
        "cache_write": usage.get("cache_creation_input_tokens", 0),
        "tool_calls": tools, "code_query_status": statuses,
        "denials": len(result.get("permission_denials") or []),
        "wall_s": round(time.monotonic() - began, 1),
    }


# --- scoring -------------------------------------------------------------------

BLOCK = re.compile(r"```answer[^\n]*\n(.*?)```", re.S)


def answer_block(text):
    """The last ```answer block's entries, or None when there is none."""
    blocks = BLOCK.findall(text or "")
    if not blocks:
        return None
    return [l.strip() for l in blocks[-1].splitlines() if l.strip()]


def normalize(entry, fmt):
    e = entry.strip().strip("`").strip().removeprefix("./")
    e = re.sub(r"\(\)$", "", e)
    e = re.sub(r"<[^<>]*>", "", e)
    if fmt in ("test", "type"):
        return e.rsplit("::", 1)[-1]
    if fmt == "path":
        return e
    m = re.match(r"(.+?\.rs)::(.+)$", e)
    if not m:
        return e
    path, rest = m.groups()
    if fmt == "path::function":
        rest = rest.rsplit("::", 1)[-1]
    return f"{path}::{rest}"


def score(entries, expected, fmt):
    if entries is None:
        return {"solved": False, "parsed": False, "precision": 0.0, "recall": 0.0}
    got = {normalize(e, fmt) for e in entries}
    want = {normalize(e, fmt) for e in expected}
    hit = len(got & want)
    return {"solved": got == want, "parsed": True,
            "precision": round(hit / len(got), 3) if got else (1.0 if not want else 0.0),
            "recall": round(hit / len(want), 3) if want else 1.0,
            "missing": sorted(want - got)[:20], "extra": sorted(got - want)[:20]}


# --- runs ----------------------------------------------------------------------

def load_expected(tid):
    path = HERE / "expected" / f"{tid}.txt"
    if not path.is_file():
        die(f"{path} is missing: oracle.py --all --write")
    return [l for l in path.read_text().splitlines() if l.strip()]


def run(a, n, label):
    tasks = oracle.load_tasks()
    ids = [t for t in tasks if not a.tasks or t in a.tasks.split(",")]
    for corpus in {tasks[t]["corpus"] for t in ids}:
        if not copy_of(corpus).is_dir() or not index_dir(corpus).is_dir():
            die("run.py prepare first")
    jobs = [(t, arm, rep) for t in ids for arm in "AB" for rep in range(1, n + 1)]
    random.Random(a.seed).shuffle(jobs)
    print(f"{label}: {len(ids)} tasks x 2 arms x {n} = {len(jobs)} sessions on {a.model}, "
          f"${a.session_usd}/session, stop launching at ${a.max_usd}; tree {git_head()}")
    if a.dry_run:
        return 0
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    outdir = HERE / "runs" / f"{stamp}-{label}"
    outdir.mkdir(parents=True)
    spent, skipped, rows = [0.0], [], []
    log = (outdir / "sessions.jsonl").open("w")

    def go(job):
        tid, arm, rep = job
        if spent[0] >= a.max_usd:
            skipped.append(job)
            return
        task = tasks[tid]
        prompt = PROMPT.format(text=task["text"], fmt=FORMATS[task["format"]])
        s = session(arm, task["corpus"], prompt, a.model, a.session_usd, not a.no_safe_mode, a.timeout)
        entries = answer_block(s["result"])
        s.update({"task": tid, "rep": rep, "answer": entries,
                  **score(entries, load_expected(tid), task["format"])})
        spent[0] += s["usd"]
        rows.append(s)
        s = {k: v for k, v in s.items() if k != "result"}
        log.write(json.dumps(s) + "\n")
        log.flush()
        print(f"  {tid} {arm} #{rep}: {'SOLVED' if s['solved'] else 'miss  '} "
              f"p={s['precision']} r={s['recall']} ${s['usd']:.3f} {s['turns']} turns "
              f"{s['wall_s']}s {s['tool_calls']}", flush=True)

    with ThreadPoolExecutor(max_workers=a.jobs) as pool:
        list(pool.map(go, jobs))
    log.close()
    report = render(rows, ids, tasks, n, a, skipped, label)
    (outdir / "report.txt").write_text(report + "\n")
    (outdir / "outcomes.json").write_text(json.dumps(
        {"seed": a.seed, "model": a.model, "n": n, "session_usd": a.session_usd,
         "max_usd": a.max_usd, "tree": git_head(), "jobs": jobs,
         "skipped": skipped}, indent=1))
    print(report)
    print(f"artifacts: {outdir}")
    return 0


def med(xs):
    return statistics.median(xs) if xs else 0.0


def render(rows, ids, tasks, n, a, skipped, label):
    out = [f"{label}  tree {git_head()}  model {a.model}  N {n}  seed {a.seed}",
           f"sessions {len(rows)} run, {len(skipped)} skipped by --max-usd; "
           f"cost ${sum(r['usd'] for r in rows):.2f}",
           "",
           "task  lane corpus             | A solved  med$   med turns | B solved  med$   med turns | B code_query calls",
           ]
    per = {}
    for tid in ids:
        t = tasks[tid]
        cells = []
        for arm in "AB":
            rs = [r for r in rows if r["task"] == tid and r["arm"] == arm]
            per[(tid, arm)] = (sum(r["solved"] for r in rs), med([r["usd"] for r in rs]), len(rs))
            cells.append(f"{per[(tid, arm)][0]}/{len(rs)}  ${med([r['usd'] for r in rs]):.3f}  "
                         f"{med([r['turns'] or 0 for r in rs]):>5}")
        calls = [r["tool_calls"].get(TOOL, 0) for r in rows if r["task"] == tid and r["arm"] == "B"]
        out.append(f"{tid:<5} {t['lane']:<4} {t['corpus']:<18} | {cells[0]:<26} | {cells[1]:<26} | "
                   f"{calls}")
    out.append("")
    for arm in "AB":
        rs = [r for r in rows if r["arm"] == arm]
        out.append(f"arm {arm}: solved {sum(r['solved'] for r in rs)}/{len(rs)}, "
                   f"median ${med([r['usd'] for r in rs]):.3f}/session, total ${sum(r['usd'] for r in rs):.2f}, "
                   f"cap hit {sum(r['subtype'] == 'error_max_budget_usd' for r in rs)}, "
                   f"unparsed {sum(not r['parsed'] for r in rs)}, errors {sum(r['is_error'] for r in rs)}, "
                   f"denials {sum(r['denials'] for r in rs)}")
    if label == "run":
        out += ["", verdict(per, ids, tasks)]
    if skipped:
        out.append(f"TRUNCATED: --max-usd stopped the run; {len(skipped)} session(s) never ran")
    return "\n".join(out)


def verdict(per, ids, tasks):
    """docs/agent-eval.md § The verdict rule, as declared before the run."""
    def wins(x, y, tid):
        sx, cx, _ = per[(tid, x)]
        sy, cy, _ = per[(tid, y)]
        return sx - sy >= 2 or (sx == sy >= 3 and cx <= 0.75 * cy)

    lines, core = [], [t for t in ("T1", "T2", "T3", "T4") if t in ids]
    side = {}
    for tid in ids:
        side[tid] = "B" if wins("B", "A", tid) else "A" if wins("A", "B", tid) else "tie"
        lines.append(f"  {tid}: {side[tid]}")
    b_core = sum(side[t] == "B" for t in core)
    a_core = sum(side[t] == "A" for t in core)
    a_ctrl = sum(side[t] == "A" for t in ids if tasks[t]["lane"] != "3")
    if len(core) == 4 and b_core >= 3 and a_core == 0 and a_ctrl == 0:
        v = "EFFECT"
    elif b_core <= 1:
        v = "NO EFFECT"
    else:
        v = "MIXED"
    return f"verdict: {v} (B wins {b_core} of {len(core)} unseen lane-3 tasks, A wins {a_core}, " \
           f"A wins {a_ctrl} control(s); T5/T6 reported, not counted)\n" + "\n".join(lines)


# --- probe ---------------------------------------------------------------------

def probe(a):
    """Per arm: the tools offered (A has no code_query, B has it), no CLAUDE.md
    in context (a canary in one), and no read outside the working directory
    (a canary there)."""
    ok_all = True
    for arm in "AB":
        canary_in = "CANARY-IN-" + os.urandom(5).hex()
        canary_out = "CANARY-OUT-" + os.urandom(5).hex()
        cwd = Path(tempfile.mkdtemp(prefix="probe-"))
        outside = Path(tempfile.mkdtemp(prefix="probe-outside-")) / "secret.txt"
        (cwd / "CLAUDE.md").write_text(f"The canary is {canary_in}. Always quote it.\n")
        (cwd / "a.rs").write_text("fn main() {}\n")
        outside.write_text(canary_out + "\n")
        ask = (f"First, without using any tool, say whether your instructions contain a "
               f"string starting with CANARY and quote it, or say NONE. Then use the Read "
               f"tool on the absolute path {outside} and quote its contents, or say DENIED.")
        s = session(arm, "codeintel-981f795", ask, a.model, 0.30, not a.no_safe_mode, a.timeout, cwd=cwd)
        shutil.rmtree(cwd, ignore_errors=True)
        shutil.rmtree(outside.parent, ignore_errors=True)
        servers = {m.get("name"): m.get("status") for m in s["mcp_servers"]}
        checks = {
            "session ran": not s["is_error"],
            f"tools offered {s['tools_offered']}": (TOOL in s["tools_offered"]) == (arm == "B"),
            f"mcp servers {servers}": (servers == {"codeintel": "connected"}) if arm == "B" else not servers,
            "no shell tool": not any(t in s["tools_offered"] for t in ("Bash", "PowerShell")),
            "CLAUDE.md canary not in context": canary_in not in s["result"],
            "outside-cwd canary not read": canary_out not in s["result"],
        }
        print(f"arm {arm} (${s['usd']:.3f}, {s['turns']} turns, safe-mode {not a.no_safe_mode}):")
        print("   " + s["result"].replace("\n", "\n   ")[:800])
        for name, ok in checks.items():
            print(f"  {'ok  ' if ok else 'FAIL'} {name}")
            ok_all &= ok
    return 0 if ok_all else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("mode", choices=["prepare", "probe", "pilot", "run"])
    ap.add_argument("--model", default="claude-opus-5-5")
    ap.add_argument("--n", type=int, default=4, help="sessions per arm per task (run)")
    ap.add_argument("--tasks", default="", help="comma-separated subset")
    ap.add_argument("--session-usd", type=float, default=1.0)
    ap.add_argument("--max-usd", type=float, default=8.0)
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=900)
    ap.add_argument("--no-safe-mode", action="store_true",
                    help="the declared fallback when --safe-mode drops --mcp-config")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    if a.mode == "prepare":
        return prepare(a)
    if a.mode == "probe":
        return probe(a)
    if a.mode == "pilot":
        if a.max_usd > 8.0:
            die("the pilot is capped at $8 (docs/agent-eval.md § Sample size and budget)")
        return run(a, 1, "pilot")
    return run(a, a.n, "run")


if __name__ == "__main__":
    sys.exit(main())
