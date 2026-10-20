#!/usr/bin/env python3
"""Score agent-written Datalog against the reference answers.

Usage:
  score.py expected                 -> recompute expected answers from the
                                       reference queries in questions.tsv
  score.py answers.tsv              -> score a file of `id<TAB>query` rows

  score.py --bank working expected      -> same, for a named bank
  score.py --bank working answers.tsv
  score.py --bank heldout check         -> recompute and compare, write nothing

An answer is correct when the SET of values in the question's answer variables
matches the reference. Extra join columns are fine and extra rows are not:
different correct queries expose different join variables, and scoring on the
whole row would be scoring style rather than truth.

Three banks, per docs/50-self-improvement.md § 5:

  (default)  the frozen M1 30, in this directory, scored against `facts.dl`.
             Re-run only on a signature change; `docs/agent-eval.md` is its
             record. NOTE: `facts.dl` no longer loads — it asserts `exported`
             and `implements`, both of which became RULE heads, which safety
             rule 8 refuses. That is a finding about the frozen bank and not
             something to fix by editing it.
  working    `working/`, 72 questions, scored against `bank.dl`.
  heldout    `heldout/`, 30 questions, scored against `bank.dl`. Never shown
             to an experimenting agent.

**A named bank's reference answers are recomputed on every run.** The
committed `expected.tsv` is a diffable record, never an input: scoring always
uses the freshly computed values and a disagreement is reported as a finding
rather than quietly papered over. `docs/agent-eval.md` has found three
reference-answer defects and none of them by assertion — "a reference answer
nobody recomputes is an assertion nobody checks".
"""
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]

# A named bank: where its questions live, and which fact set answers them.
BANKS = {
    "working": (HERE / "working", HERE / "bank.dl"),
    "heldout": (HERE / "heldout", HERE / "bank.dl"),
}

# Questions whose correct answer genuinely IS "no rows". Everything else must
# have a non-empty reference answer: a question answered by nothing is passed
# by any query that returns nothing, including a broken one, and it scores
# itself correct for free. Ten of the M4 set A were silently in this state.
#
# A conformance question is the one honest exception — "this policy is not
# violated" is the answer you want — so each one is named here with the policy
# it asserts.
KNOWN_EMPTY = {
    # Q49: no module under src/db/ imports anything naming ui. The layering
    # holds, and the query that reports it must return nothing.
    "working": {"49"},
    # Q23: no test file imports the old HTTP client. Same shape.
    "heldout": {"23"},
}


def run(query, facts=None):
    """Return (columns, rows) or (None, message)."""
    argv = ["cargo", "run", "-q", "-p", "codeintel", "--example", "eval", "--"]
    if facts is not None:
        argv += ["--facts", str(facts)]
    out = subprocess.run(argv + [query], cwd=ROOT, capture_output=True, text=True)
    lines = [l for l in out.stdout.splitlines() if l]
    if not lines:
        return None, (out.stderr.strip() or "no output")
    head = lines[0].split("\t")
    if head[0] == "status":
        return None, "\t".join(lines[1].split("\t")[1:])
    columns = head[1:]
    rows = [l.split("\t") for l in lines[1:] if not l.startswith(("rows\t", "truncated\t"))]
    return columns, rows


def project(columns, rows, wanted):
    """The set of tuples over the named columns, as a sorted list."""
    missing = [w for w in wanted if w not in columns]
    if missing:
        return None
    index = [columns.index(w) for w in wanted]
    return sorted({tuple(r[i] for i in index) for r in rows})


def questions(path):
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        qid, answer_vars, text, reference = line.split("\t")
        yield qid, answer_vars.split(","), text, reference


def payload(projected):
    return "|".join(" ".join(t) for t in projected)


def references(qfile, facts):
    """Every reference query, run now. Dies on the two defects that matter:
    a reference that does not run, and a reference that binds the wrong
    variables."""
    out = {}
    for qid, answer_vars, _text, reference in questions(qfile):
        columns, rows = run(reference, facts)
        if columns is None:
            sys.exit(f"question {qid}: reference query failed: {rows}\n  {reference}")
        projected = project(columns, rows, answer_vars)
        if projected is None:
            sys.exit(
                f"question {qid}: reference query does not bind {answer_vars}\n"
                f"  {reference}"
            )
        out[qid] = payload(projected)
    return out


def check_not_empty(bank, computed):
    """Fail loudly if a question that should have an answer has none."""
    allowed = KNOWN_EMPTY.get(bank, set())
    empty = {qid for qid, rows in computed.items() if not rows}
    unexpected = empty - allowed
    if unexpected:
        sys.exit(
            f"questions with an EMPTY reference answer: {sorted(unexpected, key=int)}\n"
            "A question answered by no rows is passed by any query that returns "
            "nothing. Fix the question, or add it to KNOWN_EMPTY with a reason."
        )
    stale = allowed - empty
    if stale:
        sys.exit(
            f"KNOWN_EMPTY lists {sorted(stale, key=int)} but they now return rows — "
            "give them a real assertion and remove them from the list."
        )


def report_moved(efile, computed):
    """Compare the recomputation against the committed record. A difference is
    a finding: the facts or the rules changed what a question's correct answer
    is. It is not an error — the fresh values are what scoring uses — but it is
    never silent."""
    if not efile.exists():
        return
    recorded = {}
    for line in efile.read_text().splitlines():
        if not line.strip():
            continue
        qid, _count, text = line.split("\t", 2)
        recorded[qid] = text
    moved = [q for q in computed if recorded.get(q, computed[q]) != computed[q]]
    if moved:
        print(
            f"WARNING: {len(moved)} reference answer(s) moved since {efile.name}: "
            f"{sorted(moved, key=int)}\n"
            "  The facts or the rules changed what these questions' correct "
            "answers are. Say why, then refresh with `expected`.",
            file=sys.stderr,
        )


def write_expected(qfile, efile, facts, bank):
    computed = references(qfile, facts)
    check_not_empty(bank, computed)
    report_moved(efile, computed)
    lines = [f"{qid}\t{len(v.split('|')) if v else 0}\t{v}" for qid, v in computed.items()]
    efile.write_text("\n".join(lines) + "\n")
    print(f"wrote {len(lines)} expected answers to {efile}")


def score(path, qfile, efile, facts, bank):
    """Score a run. For a named bank the references are recomputed here and
    now; `expected.tsv` is only consulted to report drift."""
    if bank is None:
        expected = {}
        for line in efile.read_text().splitlines():
            qid, _count, text = line.split("\t", 2)
            expected[qid] = text
    else:
        expected = references(qfile, facts)
        check_not_empty(bank, expected)
        report_moved(efile, expected)

    wanted = {qid: vars_ for qid, vars_, _t, _r in questions(qfile)}

    passed, failures = 0, []
    for line in Path(path).read_text().splitlines():
        if not line.strip():
            continue
        qid, query = line.split("\t", 1)
        answer_vars = wanted[qid]
        columns, rows = run(query, facts)
        if columns is None:
            failures.append((qid, query, f"rejected: {rows}"))
            continue
        projected = project(columns, rows, answer_vars)
        if projected is None:
            failures.append((qid, query, f"does not bind {answer_vars}"))
            continue
        got = payload(projected)
        if got == expected[qid]:
            passed += 1
        else:
            failures.append((qid, query, f"got {got or '<empty>'}"))

    total = passed + len(failures)
    print(f"{passed}/{total}")
    for qid, query, why in failures:
        print(f"  q{qid}: {why}\n        {query}")
    return 0 if not failures else 1


def main(argv):
    bank, facts = None, None
    qfile, efile = HERE / "questions.tsv", HERE / "expected.tsv"
    if argv[:1] == ["--bank"]:
        if len(argv) < 2 or argv[1] not in BANKS:
            sys.exit(f"--bank takes one of {sorted(BANKS)}")
        bank = argv[1]
        directory, facts = BANKS[bank]
        qfile, efile = directory / "questions.tsv", directory / "expected.tsv"
        argv = argv[2:]

    if not argv:
        sys.exit(__doc__)
    if argv[0] == "expected":
        write_expected(qfile, efile, facts, bank)
        return 0
    if argv[0] == "check":
        if bank is None:
            sys.exit("`check` needs --bank")
        computed = references(qfile, facts)
        check_not_empty(bank, computed)
        report_moved(efile, computed)
        print(f"{len(computed)} reference answers recomputed")
        return 0
    return score(argv[0], qfile, efile, facts, bank)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
