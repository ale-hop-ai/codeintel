#!/usr/bin/env python3
"""Tier 3 of docs/50-self-improvement.md: the agent eval, automated.

  run.py --bank working                    every question, 3 samples each
  run.py --bank working --questions 6 --rounds 2 --per-agent 3
  run.py --bank working --dry-run          print the plan and cost bound, spawn nothing
  run.py probe                             prove the isolation, spawn one agent
  run.py --bank heldout --merge            the merge gate; aggregates only

What it does: render `codeintel schema` for the bank's facts, partition the
questions into groups, hand each group to a FRESH headless `claude -p` agent
whose entire world is that page plus its questions, collect `Q<id>: ?- ... .`
answers, and score them with score.py. docs/agent-eval.md § Tier 3 runner has
the argument for every choice below.

The isolation is the measurement. Each agent is one `claude -p` process with
  * no tools at all (`--tools ""`) — it cannot read a fixture, a bank or this
    repo, because it has no way to read anything. This is stricter than the
    original runs, where "read one file" was an instruction and not a sandbox;
  * the default system prompt replaced (`--system-prompt`), so it is not even
    told tools exist (with the default prompt the model hallucinates a Read
    call and then invents the file's contents);
  * `--safe-mode`: no CLAUDE.md, hooks, skills, plugins, MCP, memory;
  * a fresh empty temp directory as cwd, no session persisted, one turn.
`probe` checks this against a canary instead of asserting it.

Agents never see each other: separate processes, separate contexts. Questions
are re-partitioned between rounds, seeded, so no agent context holds the same
grouping twice when that is arithmetically possible.

It is an LLM measurement. The result is a distribution: per-round scores, the
per-question pass histogram, agreement across rounds, and what it cost.
Re-rolling to move the number is how a score stops meaning anything
(docs/agent-eval.md) — a run's artifacts are kept so a number has a provenance.
"""
import argparse
import hashlib
import itertools
import json
import math
import random
import re
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SCORE = HERE / "score.py"
BANK_FACTS = HERE / "bank.dl"
BANKS = {"working": HERE / "working", "heldout": HERE / "heldout"}

SYSTEM = "You are a participant in an evaluation of a query language."

PROMPT = """\
Below is the complete documentation of a Datalog query language over facts \
extracted from a small code repository. It is all you have: you cannot \
execute queries and you have no tools. For each question after it, write one \
Datalog query, starting with `?-` and ending with `.`, that binds the \
variables the question names. Write your first answer. Reply with exactly \
one line per question and nothing else, in the form: Q<id>: <query>

=== DOCUMENTATION ===
{schema}
=== END DOCUMENTATION ===

Questions:
{questions}
"""

ANSWER = re.compile(r"^Q(\d+):\s*(\?-.*\.)\s*$")

# `claude -p` with everything off. Order is irrelevant; each flag is listed in
# the module docstring for the property it buys.
CLAUDE = [
    "claude", "-p", "--output-format", "json", "--safe-mode", "--tools", "",
    "--strict-mcp-config", "--disable-slash-commands", "--no-session-persistence",
    "--system-prompt", SYSTEM,
]


def die(message):
    sys.exit(f"run.py: {message}")


def git_head():
    out = subprocess.run(["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT,
                         capture_output=True, text=True)
    dirty = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"],
                           cwd=ROOT, capture_output=True, text=True).stdout.strip()
    return out.stdout.strip() + ("+dirty" if dirty else "")


def schema_page():
    """The page an agent reads, generated from THIS tree's schema and rules."""
    out = subprocess.run(
        ["cargo", "run", "-q", "-p", "codeintel", "--example", "eval", "--",
         "--facts", str(BANK_FACTS), "--schema"],
        cwd=ROOT, capture_output=True, text=True)
    if out.returncode != 0 or not out.stdout.strip():
        die(f"could not render the schema page:\n{out.stderr}")
    return out.stdout


def load_questions(bank):
    """id -> question text. The reference query and answer variables stay
    behind: only score.py reads them."""
    found = {}
    for line in (BANKS[bank] / "questions.tsv").read_text().splitlines():
        if line.strip():
            qid, _vars, text, _reference = line.split("\t")
            found[qid] = text
    return found


# Openers whose subject is the previous question's. Kept in step with
# REFERENTIAL in crates/codeintel/tests/eval_banks.rs, which guards the banks.
REFERENTIAL = ("The same", "The other", "Likewise", "Again,")


def chains_of(ids, texts):
    """(referent, question) pairs: a question that opens referentially, paired
    with the question numbered just before it. Only pairs whose referent is in
    `ids` — a sampled subset can drop one."""
    have = set(ids)
    return [(str(int(q) - 1), q) for q in ids
            if texts[q].startswith(REFERENTIAL) and str(int(q) - 1) in have]


def keep_chains(groups, chains, seen):
    """Put each (referent, question) pair in one group, moving as little as
    possible: the question swaps places with a groupmate of its referent, so
    exactly two questions change group per repaired pair and nothing else in
    the round moves. The groupmate must be free of chains itself and must not
    create a pair `seen` already holds. Returns (groups, pairs still split)."""
    linked = {q for pair in chains for q in pair}
    where = {q: gi for gi, g in enumerate(groups) for q in g}
    split = []
    for ref, q in chains:
        if where[ref] == where[q]:
            continue
        for mover, anchor in ((q, ref), (ref, q)):
            home, target = groups[where[mover]], groups[where[anchor]]
            fits = [m for m in target if m not in linked
                    and not any(frozenset((mover, o)) in seen for o in target if o != m)
                    and not any(frozenset((m, o)) in seen for o in home if o != mover)]
            if fits:
                m = fits[0]
                gm, ga = where[mover], where[anchor]
                home[home.index(mover)], target[target.index(m)] = m, mover
                where[mover], where[m] = ga, gm
                break
        else:
            split.append((ref, q))
    for g in groups:
        for ref, q in chains:
            if ref in g and q in g:
                g.remove(q)
                g.insert(g.index(ref) + 1, q)
    return groups, split


def partition(ids, per_agent, rounds, rng, chains=()):
    """`rounds` partitions of `ids` into groups of `per_agent`. Each round is a
    fresh shuffle, retried until no pair of questions shares a group twice
    across rounds — the original's rule. Returns (rounds, repeated_pairs,
    split); repeated_pairs > 0 means it was arithmetically out of reach.

    `chains` (off by default, so the recorded partition is reproducible) keeps
    each referent and the question that refers back to it in one group, every
    round. That is a deliberate exception to the rule above: a chain's pair
    co-occurs in all rounds by construction, and is excluded from the repeat
    count rather than hidden in it. `split` lists chains no swap could join."""
    linked = {frozenset(c) for c in chains}
    # `drawn` steers the shuffle exactly as without chains, so the unrepaired
    # round is the recorded one; `kept` is what the repaired rounds hold, and is
    # what repairs and the repeat count are checked against. Repairing inside
    # the retry loop instead re-rolls every later round: a round is accepted
    # after ~50 rejected shuffles, and any change to `seen` moves which one.
    drawn, kept, plan, repeats, split = set(), set(), [], 0, []
    for rnd in range(rounds):
        best = None
        for _attempt in range(500):
            order = list(ids)
            rng.shuffle(order)
            groups = [order[i:i + per_agent] for i in range(0, len(order), per_agent)]
            pairs = {frozenset(p) for g in groups for p in itertools.combinations(g, 2)}
            clash = len(pairs & drawn)
            if best is None or clash < best[0]:
                best = (clash, groups, pairs)
            if clash == 0:
                break
        clash, groups, pairs = best
        drawn |= pairs
        left = []
        if chains:
            groups, left = keep_chains(groups, chains, kept)
        held = {frozenset(p) for g in groups for p in itertools.combinations(g, 2)} - linked
        repeats += len(held & kept)
        split += [(rnd + 1, *c) for c in left]
        kept |= held
        plan.append(groups)
    return plan, repeats, split


def run_agent(model, prompt, max_usd, timeout):
    """One fresh agent. Returns the parsed `claude -p` JSON, or an error dict.
    Retries only when no answer came back at all — a transport failure — and
    never to improve a bad answer."""
    last = None
    for attempt in range(3):
        cwd = tempfile.mkdtemp(prefix="agent-")
        try:
            proc = subprocess.run(
                CLAUDE + ["--model", model, "--max-budget-usd", str(max_usd)],
                input=prompt, capture_output=True, text=True, cwd=cwd, timeout=timeout)
            try:
                data = json.loads(proc.stdout)
            except json.JSONDecodeError:
                data = {"is_error": True, "result": proc.stdout or proc.stderr}
        except subprocess.TimeoutExpired:
            data = {"is_error": True, "result": f"timeout after {timeout}s"}
        finally:
            shutil.rmtree(cwd, ignore_errors=True)
        data["attempts"] = attempt + 1
        if not data.get("is_error"):
            return data
        last = data
    return last


def tokens(data):
    u = data.get("usage") or {}
    return (u.get("input_tokens", 0) + u.get("cache_creation_input_tokens", 0)
            + u.get("cache_read_input_tokens", 0), u.get("output_tokens", 0))


def parse_answers(text):
    """id -> first query. One line per answer, exactly as the prompt asked;
    anything else the agent wrote is ignored, and a question with no such line
    is unanswered and scores as a failure."""
    found = {}
    for line in text.splitlines():
        m = ANSWER.match(line.strip())
        if m and m.group(1) not in found:
            found[m.group(1)] = m.group(2)
    return found


def score(bank, rows, workdir):
    """Score `rows` — (qid, query) pairs — with score.py, once. Returns
    (failed {(qid, query): reason}, scorer stderr). References are recomputed
    by score.py on this call, as always."""
    path = workdir / "answers.tsv"
    path.write_text("".join(f"{q}\t{a}\n" for q, a in rows))
    proc = subprocess.run([sys.executable, str(SCORE), "--bank", bank, str(path)],
                          cwd=ROOT, capture_output=True, text=True)
    if proc.returncode not in (0, 1):
        die(f"score.py failed:\n{proc.stdout}{proc.stderr}")
    failed, lines = {}, proc.stdout.splitlines()
    for i, line in enumerate(lines):
        m = re.match(r"^  q(\d+): (.*)$", line)
        if m and i + 1 < len(lines):
            failed[(m.group(1), lines[i + 1].strip())] = m.group(2)
    return failed, proc.stderr.strip()


def pct(n, d):
    return f"{100 * n / d:.0f}%" if d else "n/a"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("mode", nargs="?", choices=["run", "probe"], default="run")
    ap.add_argument("--bank", choices=sorted(BANKS), default="working")
    ap.add_argument("--rounds", type=int, default=3, help="samples per question")
    ap.add_argument("--per-agent", type=int, default=3, help="questions per agent")
    ap.add_argument("--questions", type=int, default=0,
                    help="sample this many questions, seeded; 0 = the whole bank")
    ap.add_argument("--seed", type=int, default=1, help="partition and subset only")
    ap.add_argument("--model", default="claude-opus-5")
    ap.add_argument("--jobs", type=int, default=4, help="agents in flight")
    ap.add_argument("--max-usd", type=float, default=3.0,
                    help="stop launching agents past this total spend")
    ap.add_argument("--agent-usd", type=float, default=0.50, help="per-agent ceiling")
    ap.add_argument("--timeout", type=int, default=300)
    ap.add_argument("--keep-chains", action="store_true",
                    help="keep a back-referring question in its referent's group (#91)")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--merge", action="store_true", help="required for --bank heldout")
    # NOT under `target/`. These are the evidence a number on
    # docs/agent-eval.md rests on, and `cargo clean` removed four runs' worth
    # on 2026-10-20 -- including the run the page's most recent figure cites.
    # A build command must not be able to delete the record (#112). All of a
    # run's artifacts are committed, agents.jsonl included -- see .gitignore.
    ap.add_argument("--out", type=Path, default=HERE / "artifacts")
    a = ap.parse_args()

    if a.mode == "probe":
        return probe(a)
    heldout = a.bank == "heldout"
    if heldout and not a.merge:
        die("the held-out bank is the merge gate. --merge, or use --bank working.")

    page = schema_page()
    texts = load_questions(a.bank)
    ids = sorted(texts, key=int)
    if a.questions and a.questions < len(ids):
        ids = sorted(random.Random(a.seed).sample(ids, a.questions), key=int)
    rng = random.Random(a.seed)
    chains = chains_of(ids, texts) if a.keep_chains else []
    plan, repeats, split = partition(ids, a.per_agent, a.rounds, rng, chains)
    jobs = [(r, g) for r, groups in enumerate(plan, 1) for g in groups]

    def prompt_for(group):
        qs = "\n".join(f"Q{q}: {texts[q]}" for q in group)
        return PROMPT.format(schema=page.rstrip("\n"), questions=qs)

    print(f"bank {a.bank}: {len(ids)} questions x {a.rounds} rounds, "
          f"{a.per_agent}/agent -> {len(jobs)} agents on {a.model}")
    print(f"schema page {len(page)} chars, sha256 {hashlib.sha256(page.encode()).hexdigest()[:12]}"
          f"; tree {git_head()}; seed {a.seed}; repeated pairs {repeats}")
    if a.keep_chains:
        print(f"chains kept {len(chains)} x {a.rounds} rounds, outside the repeated-pair "
              f"rule: {' '.join(f'Q{r}+Q{q}' for r, q in chains)}; "
              f"left split {split or 'none'}")
    if a.dry_run:
        for r, g in jobs:
            print(f"  round {r}: {' '.join('Q' + q for q in g)}")
        print(f"cost bound: at most ${len(jobs) * a.agent_usd:.2f} "
              f"({len(jobs)} x --agent-usd), stops at --max-usd ${a.max_usd:.2f}")
        return 0

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    outdir = a.out / a.bank / stamp
    outdir.mkdir(parents=True)
    t0 = time.monotonic()
    spent = [0.0]
    skipped = []

    def go(job):
        r, group = job
        if spent[0] >= a.max_usd:
            skipped.append(job)
            return None
        began = time.monotonic()
        data = run_agent(a.model, prompt_for(group), a.agent_usd, a.timeout)
        data["wall_s"] = round(time.monotonic() - began, 1)
        spent[0] += data.get("total_cost_usd") or 0.0
        return data

    with ThreadPoolExecutor(max_workers=a.jobs) as pool:
        results = list(pool.map(go, jobs))

    # Assemble rows and per-agent facts.
    rows, asked, agents = [], [], []
    for (r, group), data in zip(jobs, results):
        if data is None:
            continue
        got = parse_answers(data.get("result") or "") if not data.get("is_error") else {}
        for q in group:
            asked.append((r, q))
            if q in got:
                rows.append((r, q, got[q]))
        tin, tout = tokens(data)
        agents.append({
            "round": r, "questions": group, "answered": len(got), "turns": data.get("num_turns"),
            "error": bool(data.get("is_error")), "attempts": data.get("attempts"),
            "usd": data.get("total_cost_usd") or 0.0, "wall_s": data.get("wall_s"), "tokens_in": tin, "tokens_out": tout,
            "denials": len(data.get("permission_denials") or []),
            "result": data.get("result"),
        })

    agents_wall = time.monotonic() - t0
    failed, warning = score(a.bank, [(q, query) for _r, q, query in rows], outdir)
    reasons = {}
    outcome = {}
    answered = {(r, q): query for r, q, query in rows}
    for r, q in asked:
        query = answered.get((r, q))
        if query is None:
            outcome[(r, q)] = "unanswered"
        elif (q, query) in failed:
            outcome[(r, q)] = "fail"
            reasons[(r, q)] = failed[(q, query)]
        else:
            outcome[(r, q)] = "pass"

    rounds_done = sorted({r for r, _ in asked})
    per_round = {r: sum(outcome[(r, q)] == "pass" for rr, q in asked if rr == r) for r in rounds_done}
    per_round_n = {r: sum(1 for rr, _ in asked if rr == r) for r in rounds_done}
    hist = {}
    for q in ids:
        k = sum(outcome.get((r, q)) == "pass" for r in rounds_done)
        hist[q] = k
    majority = sum(1 for k in hist.values() if k * 2 > len(rounds_done))
    unanswered = sum(v == "unanswered" for v in outcome.values())
    usd = sum(x["usd"] for x in agents)
    wall = time.monotonic() - t0
    full_agents = 3 * math.ceil(len(load_questions(a.bank)) / 3)
    mean_usd = usd / len(agents) if agents else 0.0
    anomalies = [x for x in agents if x["turns"] != 1 or x["denials"] or x["error"]]

    lines = [
        f"{a.bank} {stamp}  tree {git_head()}  model {a.model}  seed {a.seed}",
        f"agents {len(agents)}/{len(jobs)} run, {len(skipped)} skipped by --max-usd; "
        f"questions {len(ids)}; rounds {len(rounds_done)}; repeated pairs {repeats}",
        "per round: " + "  ".join(f"r{r} {per_round[r]}/{per_round_n[r]}" for r in rounds_done),
        f"total {sum(per_round.values())}/{len(asked)} first-attempt correct "
        f"({pct(sum(per_round.values()), len(asked))}); unanswered {unanswered}",
        f"agreement: {majority}/{len(ids)} questions passed in a majority of {len(rounds_done)} rounds"
        f"; pass-count histogram "
        + " ".join(f"{k}x:{sum(1 for v in hist.values() if v == k)}"
                   for k in range(len(rounds_done), -1, -1)),
        f"cost ${usd:.3f} (${mean_usd:.3f}/agent, {sum(x['tokens_in'] for x in agents)} tokens in, "
        f"{sum(x['tokens_out'] for x in agents)} out), wall {agents_wall:.0f}s for agents at {a.jobs} in flight "
        f"+ {wall - agents_wall:.0f}s scoring",
        f"a full run of this bank ({full_agents} agents) would cost about "
        f"${mean_usd * full_agents:.2f} at this model's measured mean — an estimate, not a bound",
    ]
    if anomalies:
        lines.append(f"ANOMALY: {len(anomalies)} agent(s) with turns != 1, a denial, or an error "
                     "— isolation or transport is suspect, read agents.jsonl")
    if skipped:
        lines.append(f"TRUNCATED: --max-usd stopped the run; {len(skipped)} agent(s) never ran, "
                     "the score above covers fewer samples than asked for")
    if warning:
        lines.append(warning)
    if not heldout:
        for q in ids:
            if hist[q] < len(rounds_done):
                fails = [f"r{r}:{outcome[(r, q)]}" + (f" ({reasons[(r, q)]})" if (r, q) in reasons else "")
                         for r in rounds_done if outcome.get((r, q)) != "pass"]
                lines.append(f"  q{q}: {hist[q]}/{len(rounds_done)}  " + "; ".join(fails))
    report = "\n".join(lines)
    print(report)

    if heldout:
        # An experimenting agent never sees the held-out set: aggregates only,
        # and nothing that names a question, a query or a failure is kept.
        (outdir / "report.txt").write_text(report + "\n")
        (outdir / "answers.tsv").unlink(missing_ok=True)
        return 0
    (outdir / "report.txt").write_text(report + "\n")
    (outdir / "agents.jsonl").write_text("".join(json.dumps(x) + "\n" for x in agents))
    (outdir / "outcomes.json").write_text(json.dumps(
        {"schema_sha256": hashlib.sha256(page.encode()).hexdigest(), "seed": a.seed,
         "plan": plan, "outcome": {f"{r}/{q}": v for (r, q), v in sorted(outcome.items())}},
        indent=1))
    (outdir / "schema.txt").write_text(page)
    print(f"artifacts: {outdir}")
    return 0


def probe(a):
    """Isolation, measured. One agent, a canary file in its cwd and another in
    the repo path, asked to read both and describe its situation. Passes only
    if neither canary's content appears, no turn beyond the first was taken,
    and its input token count is what the prompt alone accounts for."""
    canary = "CANARY-" + hashlib.sha256(str(time.time()).encode()).hexdigest()[:10]
    cwd = Path(tempfile.mkdtemp(prefix="agent-"))
    (cwd / "canary.txt").write_text(canary + "\n")
    ask = ("Read the file ./canary.txt and quote its contents exactly. Then say, in one line each: "
           "your working directory; the tools you can call; whether you were given any instructions "
           "or context besides this message. Say NONE where none. If you cannot read the file, say "
           "CANNOT READ.")
    try:
        proc = subprocess.run(CLAUDE + ["--model", a.model, "--max-budget-usd", str(a.agent_usd)],
                              input=ask, capture_output=True, text=True, cwd=cwd, timeout=a.timeout)
    finally:
        shutil.rmtree(cwd, ignore_errors=True)
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError:
        die(f"probe: no JSON from claude:\n{proc.stdout}{proc.stderr}")
    tin, tout = tokens(data)
    text = data.get("result") or ""
    checks = {
        "canary content not in the reply": canary not in text,
        "one turn, no tool use": data.get("num_turns") == 1 and not data.get("permission_denials"),
        # The message is ~80 tokens. A CLAUDE.md, memory, skill list or tool
        # schema is thousands. 600 leaves room for the account-context note
        # (~250, measured) and fails on any real leak.
        f"input tokens {tin} <= 600": tin <= 600,
    }
    print(text)
    print(f"cost ${data.get('total_cost_usd', 0):.4f}, {tin} in / {tout} out")
    for name, ok in checks.items():
        print(f"{'ok  ' if ok else 'FAIL'} {name}")
    return 0 if all(checks.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
