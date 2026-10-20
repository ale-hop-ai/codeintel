# Agent eval — M1

The load-bearing bet of this project is that an agent can write correct Datalog
over this schema. If that is false, the schema or the surface is wrong and every
milestone after M1 is built on top of the mistake — which is why this runs here,
against hand-written facts, and not at M4 against extractor output.

## Result

**88/90 first-attempt correct. 30/30 questions passed at least two of three
samples.** The gate is ≥ 24/30; the worst single round was 28/30.

| Round | Score |
|---|---:|
| 1 | 30/30 |
| 2 | 28/30 |
| 3 | 30/30 |

## Method

- **Fixture:** `tests/fixtures/eval/facts.dl` — a hand-written fact set for a
  small polyglot repo: 8 files, 23 definitions, Rust with SCIP identity, Go and
  TypeScript with tree-sitter only. Mixed provenance, one ambiguous name, two
  dead exports, an extern package, a trait with two implementors, and tests that
  call the code under test.
- **Rules:** the shipped `rules/stdlib.dl`, unmodified.
- **What the agent saw:** `docs/eval-schema.md` and nothing else — 968 words, a
  hand-written stand-in for `codeintel schema` until that verb exists at M4.
  Each agent was instructed to read that one file and use no other tool, so no
  agent could see the fixture, the expected answers, or this document.
- **Questions:** 30, in `tests/fixtures/eval/questions.tsv`, written before the
  schema page was written. Three samples each, in three rounds, with the
  questions re-partitioned between rounds so no agent context held the same
  grouping twice. Every agent was fresh: 30 agents, 3 questions each.
- **Scoring:** `tests/fixtures/eval/score.py`. An answer is correct when the
  **set of values in the question's answer variables** matches the reference
  query's. Extra join columns are fine; extra or missing rows are not. Scoring
  whole rows would score style rather than truth, because different correct
  queries expose different join variables.
- **Answers as submitted:** `tests/fixtures/eval/runs/round{1,2,3}.tsv`.

## The two failures, verbatim

> **Vocabulary note, schema 2.** The transcripts below are reproduced exactly as
> the agents typed them, and two of them call `callers/2`, which no longer
> exists — it was `calls/2` with the variables renamed and was cut to pay for
> `exported` and `implements` becoming rules. Read `callers(A, S)` as
> `calls(A, S)`. The eval is **not** re-run here: editing a recorded answer to
> match a later vocabulary would make the measurement say something it did not
> say.


**Q5, round 2** — *Who calls the symbol `"scip demo store/Store#get()."`?*

```prolog
?- def(S, _, _, "scip demo store/Store#get()."), callers(A, S).
```

The symbol id was passed in `def`'s **fourth** argument, which is `Name`. The
schema documents `def(S, F, Kind, Name)` and the question quoted a symbol id, so
this is a positional mix-up rather than a missing concept. The same agent's
other two answers were correct, and the other two samples of Q5 were correct —
one of them `?- callers(A, "scip demo store/Store#get().").`, which is the
minimal form.

**Q14, round 2** — *What contains `"scip demo store/Store#get()."`, directly or
transitively, up to the file?*

```prolog
?- within(A, "scip demo store/Store#get().").
```

Arguments reversed. This one is **the schema's fault, not the agent's**: the
page documents

```
within(C, A)                        containment closure, up to the file
```

and never says which position is the child. `parent(Child, Parent)` names its
arguments; `within` does not. An agent reading only that line has a coin flip,
and one of three samples lost it.

## What was changed, and what was not

`within`'s line in the schema now reads `within(Child, Ancestor)`. That is a
copy fix to an ambiguity the transcript identified, and **it is unmeasured** —
this run's 88/90 was scored against the page as it stood. M4 re-runs the eval
against real extracted facts plus 15 held-out questions, which is where the fix
gets tested.

Nothing else was changed. In particular the `def` argument order was left alone:
one sample in ninety mis-positioned an argument that the page spells out and the
other eighty-nine did not, which is an agent slip and not a schema defect. The
plan warns against the confirmation-bias trap of revising the copy after every
failure until the number comes up; two failures out of ninety, one of them with
an identified cause in the copy, is not evidence that the schema is wrong.

## Caveats worth stating

- **Same model family as the author.** Every sample came from the model writing
  this repo. That is the population the tool is built for, and it is also the
  population most likely to share its blind spots.
- **The no-other-tool rule was an instruction, not a sandbox.** Agents had file
  access and were told to read one path. Nothing in the transcripts suggests
  otherwise, but it was not enforced.
- **The fixture is small.** 23 definitions. It exercises the schema, not scale;
  M6 is where scale gets measured.
- **The questions name symbol ids verbatim.** A real agent would have to find
  them first — usually with `innermost_at` or a `def` lookup, which Q21 does
  cover end to end.

---

# Agent eval — M4

The M1 eval ran against hand-written facts and a hand-written schema page. This
one runs against **real extracted facts** and the **generated** `codeintel
schema`, which is the thing M4 built. Two sets, because a re-run alone would
only show that nothing regressed.

## Result

**Set A was re-run on 2026-09-13 and scored 89/90. See § Set A, re-run.** The
table below is kept as it was measured. Its set-A row was withdrawn
(§ Rescored).

| Set | Round 1 | Round 2 | Round 3 | Total | Gate |
|---|---:|---:|---:|---:|---|
| **A** — the M1 30, retargeted to real facts | 29/30 | 28/30 | 28/30 | **85/90** | >= 24/30 ✅ |
| **B** — 15 held-out, on a repo no fixture comes from | 15/15 | 15/15 | 15/15 | **45/45** | >= 12/15 ✅ |

Set A is `tests/fixtures/rust/` with its committed `index.scip`, so SCIP symbol
ids and both provenances are in play. Set B is **this repository pinned at
`8471824`**, tier A only — which doubles as the no-SCIP run: 45/45 with no SCIP
index at all.

**These numbers are a re-run, and the first ones are withdrawn.** An earlier
pass scored 88/90 and 43/45 against a schema that no longer exists: the eval
found `long_def`'s copy defect, the fix changed `long_def`'s signature, and
scoring old transcripts against the new standard library measures nothing about
the agents. Everything below was produced by fresh agents reading the shipped
schema.

## Set A, re-run, 2026-09-13 (#25)

| Round | Score |
|---|---:|
| 1 | 30/30 |
| 2 | 30/30 |
| 3 | 29/30 |
| **Total** | **89/90** |

The gate is >= 24/30 per round, and every round clears it. **This is set A's
score, replacing the withdrawn 85/90.** Set B's 45/45 is unchanged and was not
re-run.

The method is the M4 method below. This run also pins down what the last one
left unrecorded:

- **Tree.** Commit `f3366a9` and `tests/fixtures/rust/` with its committed
  `index.scip`, indexed as `eval_rot.rs::indexed()` does: sources backdated
  and `index.scip` newer. Before any agent ran, `score.py expected A`
  reproduced `expectedA.tsv` byte-identical.
- **Agents.** 30 fresh general-purpose subagents on Claude Opus 5
  (`claude-opus-5`), 10 per round, 3 questions each. Each got `setA.tsv`'s
  question text only, never a reference query. Each read the generated
  `schema` output (5,688 characters), and every one of the 30 made exactly one
  tool call, that read.
- **Partition.** Round 1 took the questions in order. Rounds 2 and 3 were
  drawn with Python's `random.Random(25)`, retried until no pair of questions
  shared a group twice across the three rounds.
- **Prompt**, verbatim apart from the questions and the schema path:

  > You are a participant in an evaluation of a query language. Read exactly
  > one file: `<schema.txt>`. It documents a Datalog query language over
  > facts extracted from a small code repository. Use no other tool after
  > reading it: do not open any other file, list directories, search, or run
  > commands. You cannot execute queries; write your first answer. For each
  > question below write one Datalog query, starting with `?-` and ending
  > with `.`, that binds the variables the question names. Reply with exactly
  > one line per question and nothing else, in the form: `Q<id>: <query>`

- **The scorer was checked before it was trusted.** The withdrawn runs, scored
  against the same index, still fail exactly the questions `eval_rot.rs`
  pinned for them, at 28, 27 and 26. So what moved is the answers, not the
  scorer or the index.
- **Transcription.** Answers were recorded as submitted. The one change was
  decoding `&gt;` back to `>`, an escaping added by the channel that carried
  the replies.

### The failure

**Q26, round 3.** *Which definition names are used by more than one definition
in this repository?*

```prolog
?- named(A), K = count{F: def(S, _, _, A), ref(S, _, _, _, F, _, _)}, K > 1.
```

It counts, per name, the distinct symbols that reference any definition of that
name, so it reads "used by" as "referenced from". The question's wording
supports that reading. `ambiguous(N)`'s line, "N names >1 definition", is the
other reading, and the other two samples took it. The question is not reworded:
rewording it now would change what a recorded run was asked.

### Why 89 is not a paired comparison with 85

Q13 failed in every M4 round and passed in every round here. Q28 failed in two
M4 rounds and passed in all three here. Between the two runs the vocabulary
changed:

- Schema 2 landed.
- `local_def`'s doc line, the line behind Q28, was corrected. All three Q28
  answers here count `def` rows directly.
- `schema` prints `type 1` and `struct 3` side by side in its Kind census, and
  all three Q13 answers here leave Kind unbound.

At three samples a question, the copy change cannot be separated from agent
variance. This run measures the schema as it ships today. It does not measure
whether the edits fixed anything.

### Caveats

- **The schema's static examples quote a fixture path.** Its START HERE and
  EXAMPLES blocks print `innermost_at("src/store.rs", 142, S)` and the
  `innermost_at … impact_of` pattern. Those lines are the same for every
  repository and are not generated from this index. But `src/store.rs` is
  also a fixture file, and Q3 and Q21 have that shape. A grep of the schema
  for the fixture's symbol ids, `src/app.rs`, `crate::db::conn`, `"warm"` and
  `"Store"` found nothing else.
- The M1 caveats still hold: every agent is from the same model family as the
  author, and the one-file rule was an instruction, not a sandbox.

## Rescored, 2026-09-13 — set A withdrawn

`crates/codeintel/tests/eval_rot.rs` (#17) now recomputes set A's reference
answers and rescores the recorded runs on every commit. On its first run, at
`0574a1b`, the table above did not reproduce.

| Set | Round 1 | Round 2 | Round 3 | Total |
|---|---:|---:|---:|---:|
| **A**, recorded runs against current references | 28/30 | 27/30 | 26/30 | 81/90 |
| **B**, recorded runs against `8471824`, rescored with `score.py` | 15/15 | 15/15 | 15/15 | **45/45** |

**Set A's 85/90 is withdrawn, and 81/90 does not replace it.** Four of the new
failures are not agents answering wrongly. Q5 in all three rounds calls
`callers/2`, and round 3's Q10 calls `defines/2`, and schema 2 (`9e5dd5e`)
removed both. This page's rule is that a signature change invalidates a run,
because the agent could not have written the query that now exists. Schema 2 was
that change, and nobody applied the rule when it landed. So 81/90 is what old
transcripts earn against a new vocabulary, not a score. **A fresh set-A run was
owed**, and § Set A, re-run is that run. The other failures are the recorded
ones: Q13 in every round and Q28 in rounds 2 and 3.

**Set B stands.** Its tree is pinned. Its one moved reference answer, Q8 at 16
→ 23 rows, moved together with the recorded queries, and all three rounds still
match it.

**Fourteen set-A reference answers moved.** The reason is the facts, not the
questions:

- **Q5, Q21, Q25 — checked by hand.** `Store::get` no longer calls itself. Its
  body calls `HashMap::get`, and tier A's name matching resolved that to
  `Store::get` until `ad49523` deferred to the compiler. The old reference
  answer recorded a false edge.
- **Q23.** `src/app.rs` no longer "depends on" `tests/store_test.rs`. That is
  the crate-root-module artifact M4 documented and `cookbook.md` § 4 warned
  about.
- **Q10, Q20, Q30.** `crate/` is gone, and three `local src/kinds.rs N`
  definitions appear. This fits `5ca3f68` refusing a symbol that SCIP defines in
  two documents.
- **Q11, Q12, Q22.** Enum variants and trait items are now exported (#13).
- **Q16, Q26, Q27, Q28.** A trait-impl method is qualified by its trait
  (`5ca3f68`), so `fmt` and `f` become distinct definitions and the per-file
  counts move.

Only the first bullet was traced end to end. The others are attributed to the
commit whose stated behaviour matches the diff. The regenerated `expectedA.tsv`
is `score.py expected A`'s output, and `eval_rot.rs` reproduces it independently
through the library. The two agree.


Fresh agent per group, three questions each, questions re-partitioned between
rounds so no context saw the same grouping twice. Each agent read one file — the
generated `schema` output — and was told to use no other tool. Scoring is
`tests/fixtures/eval-m4/score.py`, which runs the real binary with `--raw` and
compares the **set of values in the question's answer variables**. Answers as
submitted are in `runs/`.

**Set B is pinned, and this is not optional.** Its reference answers are facts
about this repository's own source, so *any* commit invalidates them — including
the commit that fixes something the eval found. `setB.commit` records the sha;
reproduce with `git archive <sha> | tar -x -C <dir>`, index that directory, and
score against it. Scoring set B against a live working tree silently measures a
moving target, which is how a 45/45 and a 42/45 can both be "true" on the same
afternoon.

**If you reproduce from a copied tree, force the rebuild.** `rsync -a` and
`cp -a` preserve mtimes, so cargo sees a binary newer than its sources, prints
`Finished in 0.19s`, and you score with whatever build the copy carried. The
symptom is a plausible score from the wrong code. `touch` the sources — or copy
without `-a` — and confirm you saw a real compile before trusting a number.

**Two of set B's questions were replaced before this run.** Q11 and Q13 had
**empty** reference answers — `depends` on a file with no resolvable references,
and a module with no children — so they were passable by any query returning
nothing. They were rewritten against targets that have answers. Every question
in the table above was answered by a fresh agent against the current schema; no
answer in `runs/` was authored or edited by hand. The scorer now refuses to run
if any reference answer is empty outside a named list, so this class of hole
fails loudly instead of being found by accident.

## The failures

**Set A, Q13 — all three rounds.** *Which fields belong to the type named
"Store"?*

```prolog
?- def(T, _, "type", "Store"), parent(A, T), def(A, _, "field", _).
```

`Store` is a `struct`, not a `type`, and `schema` lists both kinds with their
counts from this very index. Three independent agents made the same choice, so
this is worth treating as copy rather than carelessness: `type` is both a Kind
value *and* the English word for what `Store` is. The form that works leaves
Kind unbound — `def(P, _, _, "Store")` — and no rule advertises that as the
robust shape.

**Set A, Q28 — rounds 2 and 3.** *How many definitions does each file hold?*

```prolog
?- file(A, _), B = count{S : local_def(A, S)}.
```

`local_def(F, N)` is `(file, **name**)`, so this counts distinct names rather
than definitions — off by the number of same-named definitions in a file. The
`%%` line reads "S is defined in F. Helper.", which does not say which of the
two arguments is the name.

**`local_def`'s doc line has since been corrected**, and the number above has
not. It read "S is defined in F" for `local_def(F, N)` — naming a variable
absent from its own signature and calling a name a symbol — which is a plainly
false line and was fixed as a correctness matter. That makes this page carry a
milder version of the property it withdrew the first pass for: the copy an agent
would read today is not quite the copy these agents read, and the difference is
the line behind Q28.

The score stands as measured. Whether it would now be 87/90 is **untested, on
purpose**: re-running after every copy change is how a score stops measuring the
tool and starts measuring how many times you re-rolled. The rule for this page
is that a *signature* change invalidates a run — the agent could not have
written the query that now exists — while a corrected doc line does not, because
every recorded query is still legal and still scores identically. `long_def`
crossed that line and forced a re-run; `local_def` did not.

**Both failures are a signature reading as something it is not**, which is the
same shape as the `long_def` defect this eval found on its first pass. That one
was fixed — `def_lines(S, N)` now states a length with no threshold, and all
three rounds used it correctly for the question that used to fail. The score
went *down* from 88/90 to 85/90 because the questions got harder to pass by
accident, not because the tool got worse.

### Q14's reference answer was wrong, and the agents were right

Set A's Q14 — "what contains `store/impl#[Store]get().`, directly or
transitively, **up to the file**?" — recorded `local src/store.rs Store#` as the
answer. That symbol is the `impl` block's stale tier-A id, which after anchoring
has no `def` row, and the recorded answer never reached the file the question
explicitly asks for. The reference answer was wrong when it was written; the
scorer compared a wrong answer against a wrong expectation and called it a pass.

All three set-A rounds wrote the identical, correct query. So fixing the
extractor (`specs/02-extraction.md` § Parent precedence, rule 3's existence
check) changed only what that correct query *returns*: now `src/store.rs`.
**85/90 and 45/45 both stand** — recomputing every set-A reference answer from
its query moved exactly one line, Q14's, and rescoring all six recorded rounds
reproduced both totals unchanged. Set B is untouched because it runs tier A only,
where the `impl` block and its type share a symbol and the parent resolves.

This is the third reference-answer defect this eval has found in itself, after
the ten empty answers and the moving set-B target. The pattern is consistent: the
eval is better at finding bugs in the eval than the eval is at finding bugs in
the tool, and every one of them was invisible until something forced a
recomputation. A reference answer nobody recomputes is an assertion nobody
checks.

## What building the eval found

Two things, before a single agent ran.

**Ten of thirty expected answers came back empty**, because the retargeted
questions quoted symbol ids I had written by hand and got wrong — the real one is
`store/impl#[Store]get().`, not `store/Store#get().`. A question whose correct
answer is "no rows" is passed by any query that returns nothing, so a third of
set A was scoring itself as correct for free. Now two are empty, both the known
`implements`/`extern` coverage gap.

**A real defect, in tier B rather than M4.** `parent(S)` for a method inside an
`impl` block points at `local src/store.rs Store#`, which has **no `def` row**.
Every other parent edge in the index carries a resolved SCIP id. So `within/2`
and `about(S, "parent", ...)` join to nothing for any method in an `impl`, and
`tests/stdlib.rs`'s containment test sailed past it because it used module
nesting. Recorded in [plan.md](plan.md); not fixed inside M4, because parent
precedence is tier-B ingest and has blast radius.

---

# The bank, split three ways — 2026-10-20

[docs/50-self-improvement.md](50-self-improvement.md) § 5 splits this eval into
three banks, for the reason this page keeps arriving at from the other
direction: a score that is re-run after every copy change stops measuring the
tool and starts measuring how many times you re-rolled. The rule is kept by
splitting the bank rather than by weakening it.

| Bank | Where | Size | Facts | Re-run |
|---|---|---:|---|---|
| **Frozen** | `tests/fixtures/eval/`, `tests/fixtures/eval-m4/` | 30 + 45 | `facts.dl`, the rust fixture, this repo at `8471824` | only on a signature change |
| **Working** | `tests/fixtures/eval/working/` | 75 | `tests/fixtures/eval/bank.dl` | freely, every Tier 3 |
| **Held-out** | `tests/fixtures/eval/heldout/` | 30 | `tests/fixtures/eval/bank.dl` | merge only |

**No measurement is recorded here yet.** The two new banks are apparatus: 105
questions, their reference queries, and a scorer. The first scored run is a
separate piece of work and it gets its own section, with its tree, its agents,
its partition and its prompt, the way § Set A, re-run does.

**An experimenting agent never sees the held-out bank.** If it did, the set
would measure memorization.

## No stored reference answers

The working and held-out banks' reference answers are **recomputed from their
reference queries on every run**. `score.py --bank working <answers>` computes
them first and scores against what it just computed; `expected.tsv` in each
bank directory is a diffable record that the scorer compares against and warns
about, never an input it trusts.

That is this eval's own hardest-won lesson, and it has now been learned three
times — ten empty set-A answers, set B's moving target, and Q14's wrong answer
that the scorer confirmed as a pass. Every one was invisible until something
forced a recomputation. *A reference answer nobody recomputes is an assertion
nobody checks.* These two banks are built so there is nothing to forget to
recompute.

The scorer also refuses to run a bank where any reference answer is empty,
unless that question is named in `KNOWN_EMPTY` with the reason. A question
answered by no rows is passed by any query that returns nothing, including a
broken one. Two questions are named there, one per bank, and both for the same
honest reason: a conformance question asserts a policy *holds*, so "no rows" is
the answer you want.

## What the frozen 30 do not ask

`rules/stdlib.dl` defines **41 rule heads**. The frozen 30 name **17** of them.
The 24 they never name are not a random tail — they are, almost exactly, the
precision apparatus:

```
at  callable  calls_at  calls_exact  dead_export_exact  def_lines
depends_exact  entrypoint_exact  file_of  impl_sym  local_def  named
reach_of_exact  reaches  reaches_exact  recursive  recursive_exact  ref
ref_outside  ref_outside_exact  scip_claim  tighter_at  trait_sym
uses_package_exact
```

Nine of the ten `_exact` twins are in that list. The frozen bank asks
`impact_of_exact` once and nothing else, so the one property
[50-self-improvement.md](50-self-improvement.md) § 2 says the whole scoreboard
rests on — that an agent picks the right tier, and that the two tiers *differ*
— has never been in an eval question. The working and held-out banks name
**35** of the 41 heads between them. The five still unnamed — `impl_sym`,
`ref_outside_exact`, `scip_claim`, `tighter_at`, `trait_sym` — are internal
helpers, reached through the rules that name them.

## The fixture is new, and `facts.dl` is left broken

The two new banks run against `tests/fixtures/eval/bank.dl`, not against
`facts.dl`.

`facts.dl` **no longer loads at all**. It asserts `exported` and `implements`
as facts, and schema 2 and 3 made both of them rule heads, which safety rule 8
refuses outright:

```
stdlib: invalid-query: `exported` is a base relation supplied by the fact store
and cannot also be defined by a rule (safety rule 8); pick a different head name
```

So every question in the frozen M1 bank currently fails to run, and
`expected.tsv`'s Q18 still records an answer for `long_def/2`, a rule deleted
at schema 3. **None of that is repaired here.** The frozen bank is the record
of what M1 measured, and editing its fixture would change the world a recorded
run was scored in without changing the record — the same move this page
withdrew a score for. It is written down instead.

`bank.dl` is a fresh 16-file polyglot fixture written against the vocabulary
that ships today, and it is deliberately shaped to make the `_exact` twins
*disagree*, which `facts.dl` never did:

- `src/legacy.rs` is Rust the indexer never reached. The only reference to
  `Cache::warm` anywhere is a name-tier row in it, so `entrypoint_exact` calls
  `warm` an entrypoint and `entrypoint` does not — the INVERSION its doc line
  warns about, as a row rather than as a hedge. `dead_export` and
  `dead_export_exact` split on the same symbol.
- 22 `calls` rows, 15 of them `calls_exact`. Three definitions sit on a cycle;
  one of those cycles survives into `recursive_exact`.
- `visibility` and `name_export` instead of asserted `exported`, so all three
  `exported` clauses fire, including the recursive `inherited` one.
- `scip_impl` at method granularity and `name_impl` at type granularity, plus
  two `name_impl` rows that `implements` correctly refuses — one for the
  `trait_sym` guard, one for `S != T`.
- `module` definitions, so `calls_at`'s `FromKind != "module"` guard is
  exercised; a tier-B opaque placeholder, so `scip_claim`'s "emitted but not
  resolved" case is real; and a `test` role, so the fifth `is_test` clause has
  a file.

## The count{} trap, as a question

`count{X : goal}` groups by every variable bound **outside** the aggregate, not
only by the ones the author meant ([specs/03-datalog.md](../specs/03-datalog.md)
safety rule 4). So

```prolog
?- def(S, _, _, _), A = count{ S : def(S, _, _, _) }.
```

returns `A = 1`, once per definition, with `status: "ok"`. There is no
truncation flag, no provenance hedge and no diagnostic — it is the cheapest way
to get a confidently wrong number out of this tool, and nothing in the surface
warns about it. Working Q59 and held-out Q26 ask for exactly that total, and
the form above scores wrong against them.

## Running them

```sh
python3 tests/fixtures/eval/score.py --bank working check        # recompute only
python3 tests/fixtures/eval/score.py --bank working expected     # refresh the record
python3 tests/fixtures/eval/score.py --bank working answers.tsv  # score a run
python3 tests/fixtures/eval/score.py --bank heldout answers.tsv
```

`score.py` with no `--bank` still means the frozen M1 bank and still reads
`expected.tsv`, so every command recorded above means what it meant.
`cargo run -p codeintel --example eval` gained a `--facts <path>` argument for
the second fixture and defaults to the embedded `facts.dl`.

---

# Tier 3 runner — 2026-10-20

`tests/fixtures/eval/run.py` is [docs/50-self-improvement.md](50-self-improvement.md)
§ 4's Tier 3, automated. Everything above was run by hand; this is the same
method with the manual steps written down and the "instruction, not a sandbox"
caveat removed.

```sh
python3 tests/fixtures/eval/run.py --bank working --dry-run   # the plan and the cost bound
python3 tests/fixtures/eval/run.py probe                      # prove the isolation (one agent, ~$0.01)
python3 tests/fixtures/eval/run.py --bank working             # 75 questions x 3 rounds
python3 tests/fixtures/eval/run.py --bank working --questions 6 --rounds 3 --per-agent 2   # a small sample
python3 tests/fixtures/eval/run.py --bank heldout --merge     # the merge gate; aggregates only
```

Sample size is `--questions` (seeded subset, 0 = whole bank), `--rounds`
(samples per question) and `--per-agent`; agents = rounds x ceil(questions /
per-agent). `--max-usd` stops *launching* agents past a total and says so in the
report — a truncated run is labelled, never presented as a full one.

## Why headless, and not tally

Both were evaluated. **Headless `claude -p`, one process per agent.**

| | headless `claude -p` | `tally spawn` |
|---|---|---|
| Isolation | `--tools ""` removes every tool; `--safe-mode` removes CLAUDE.md, hooks, skills, MCP, memory; fresh empty cwd | a pad is a full interactive session in a worktree: this repo's CLAUDE.md, hooks and memory load, and the agent has file tools. Isolation falls back to an instruction — the caveat this runner exists to remove |
| Collecting answers | one JSON object on stdout | scrape a TUI |
| Cost accounting | `total_cost_usd` and token usage per agent | none |
| Footprint | a process | a pad. The grid is 5 x 8 and ~16 pads were running when this was built; a 75-agent run would not fit, and it is a shared surface the user presses to jump between sessions |

Tally is the right home for the *experimenter* running `run.py`, and for nothing
the eval agents do. The runner never touches the grid.

## What an agent sees

The system prompt is replaced, not appended to, and says one sentence. The user
message is the instructions of the M4 prompt, the **page inline** (the original
had agents read it with a tool — with no tools the same text arrives in the
prompt), and its questions as `Q<id>: <text>`. The page is
`cargo run --example eval -- --facts bank.dl --schema`: the real `Schema`
renderer over a census of the bank's own facts. `codeintel schema` itself cannot
be used, because the bank is a fact file with no tree to index, and the no-index
page ends "THERE IS NO INDEX HERE", which would be a different measurement. The
page is the one this tree's rules produce, so a change to a rule signature or a
doc line changes what the next run reads. Never the reference query, the answer
variables, the facts, or any other group.

**Isolation is measured, not asserted.** `run.py probe` puts a random canary in
the agent's cwd, asks it to read it and describe its situation, and passes only
if the canary is absent, it took one turn, and its input was <= 600 tokens (a
CLAUDE.md, memory or tool schema is thousands). Findings from building it:

- With the **default system prompt** and `--tools ""` the model is still told it
  has tools, writes a `Read` call as text, and invents the contents of the file.
  The canary read back as a different random string. Replacing the system
  prompt fixed it, which is why `--system-prompt` is there.
- The agent still receives the account email, the date and a budget note as a
  context note (~250 tokens, constant across agents). `--safe-mode` does not
  drop them. They cannot carry the answer; they are a residual, not a leak.
- `--bare` was the obvious flag and is unusable here: it ignores OAuth and the
  keychain and needs `ANTHROPIC_API_KEY`, which this host does not set.

Agents run as separate processes, so they cannot see each other. Questions are
re-partitioned each round from a seeded shuffle, retried until no two questions
share a group twice; when that is arithmetically impossible `repeated pairs` in
the report says by how much. The seed fixes the partition and the subset only.

## What it reports

Per-round score, total, the **pass-count histogram** (how many questions passed
3, 2, 1, 0 rounds), and agreement as "questions that passed in a majority of
rounds" — the 2-of-3 figure when rounds is 3. Plus agents run, tokens, cost,
cost per agent, wall time split into agents and scoring, and an extrapolated
cost for the full bank, labelled an estimate. Artifacts, in
`target/eval/<bank>/<utc>/`: `report.txt`, `agents.jsonl` (raw replies and
usage), `outcomes.json`, `schema.txt` and `answers.tsv`. Any agent that took
more than one turn, hit a permission denial or errored is flagged `ANOMALY`.

An unparseable or missing answer is a failure, not a skip. A transport failure
(error, timeout, no JSON) is retried up to twice and the retry count recorded; a
*wrong* answer is never retried.

**Held-out.** `--bank heldout` needs `--merge`. It prints aggregates only — no
question ids, queries or failure reasons — and keeps neither `answers.tsv` nor
the raw replies, because a per-question breakdown handed to an experimenting
agent is the held-out set leaking.

## What it does not do

- **It does not make the number stable.** It is an LLM measurement: the same
  tree, seed and model give a different score on a different day. Compare
  distributions across runs, not totals. Re-running to move a score is still the
  failure this page opened with.
- **Model is a parameter** (`--model`, default `claude-opus-5`, the M4 runs'
  model). Scores across models do not compare.
- **Scoring is slow.** `score.py` recomputes every reference query in the bank
  with one `cargo run` each, ~2.5 minutes for the working bank regardless of how
  many answers it scores. Fixed overhead per run, not per agent.
- **Same-family blind spots** and **the small fixture** (43 definitions) are the
  M1 caveats, unchanged.

## Validated sample, not a score

Built and checked on 2026-10-20 at `0291ac8` + this change: working bank,
`--questions 4 --per-agent 2 --rounds 3`, seed 1 — 6 agents, 12 answers, on
`claude-opus-5`.

| Measured | |
|---|---|
| Isolation probe | canary absent, 1 turn, 372 input tokens |
| Agents | 6/6 ran, 0 anomalies, 0 retries, every answer parsed |
| Cost | $0.206 total, **$0.034 per agent**, 17,430 tokens in, 531 out |
| Partition | 3 rounds, 0 repeated pairs (4 questions in pairs: the 3 perfect matchings) |
| Scoring check | a deliberately wrong query and a malformed one were both caught as failures |
| Extrapolation | the full working bank is 75 agents, about **$2.6** and ~25 minutes at 3 in flight |

The sample's answers scored 12/12. **That is not a measurement of the schema.**
Four questions out of 75 were chosen by seed, not for being representative, and
no claim is made from them; they exist to show every stage of the pipeline ran.
The first full-bank number is the user's call and gets its own section, with its
tree, seed and report, the way § Set A, re-run does.

## Tier 3, first automated run — 2026-10-20

The first measurement taken by `tests/fixtures/eval/run.py` rather than by hand.
Run because the schema and the rendering both moved (`schema_bytes` 15,407 →
5,356; one notation per row), which `docs/50-self-improvement.md` § 4 names as
Tier 3's trigger.

**210/225 first-attempt correct (93%). 70/75 questions passed a majority of
three rounds.** Per round 71/75, 71/75, 68/75. Histogram 3x:67 2x:3 1x:3 0x:2.
Tree `95d032f`, model `claude-opus-5`, seed 1, schema page sha `f00beda11363`,
$2.836 at $0.038/agent, 96 s of agents at 4 in flight.

This is the **working** bank. It is not comparable to the frozen bank's 85/90,
which is a different question set against a different fixture.

### The isolation is not quite what the page claims

`run.py probe` reports the agent sees no tools and no working directory and
cannot read a canary — but it also reports a system-reminder carrying the
operator's email address and today's date. "Sees the schema page and nothing
else" is therefore not literally true. Nothing in that reminder is the fixture,
the bank or the expected answers, so the measurement stands; the sentence does
not, and is corrected here rather than in a footnote to a score.

### Q20's reference answer is wrong, and the agents were right

> *Which definitions state a bounded visibility, or state none where the
> grammar offers the slot?*

The reference is `?- visibility(A, "restricted").`, which answers only the first
half. "States none where the grammar offers the slot" is `inherited`. All three
rounds returned the larger, correct set — identically — and all three scored as
failures.

**This is the fourth reference-answer defect this eval has found in itself**,
and the second where the agents were right. Recorded, not silently repaired:
the 210/225 above was measured against the wrong reference and is reported as
measured. The question and its query are reconciled separately, and whatever
the next run reports is the next run's number.

### The one systematic failure: `count` must be bound first

Four of the six aggregate attempts, across Q26 and Q60, were rejected with

```
`count` may only appear as `X = count{ T : goal }`
```

because the agent wrote the aggregate straight into a comparison:

```prolog
?- file(A, _), count{S : def(S, A, _, _)} > 3.          % rejected
?- file(A, _), B = count{S : def(S, A, _, _)}, B > 3.   % accepted, and scored
```

The schema's `BUILTINS` line teaches `count{X:goal}` — the aggregate, with no
indication that it cannot stand in comparison position. The agents did not
misunderstand aggregation; they could not see the binding requirement, because
the one line that teaches the builtin omits it.

The engine's error message is good and names the legal form. It does not help
here: this eval allows one attempt, which is the right measure of whether a
schema teaches, and a retry budget would measure the error message instead.

**Next hypothesis, and it is cheap:** teach the builtin as `N = count{X:goal}`.
Expected to recover up to four samples. It changes the schema page, so it is
measured by re-running this bank and reporting both numbers — which is an
experiment, not a re-roll, as long as the before number stays on this page.

### The `count` fix, measured — 2026-10-20

Both changes the run above called for, and the re-run that priced them.

| | before | after |
|---|---|---|
| first-attempt | 210/225 (93%) | **216/225 (96%)** |
| majority of 3 | 70/75 | **72/75** |
| never passed (0x) | 2 | **0** |
| rounds | 71, 71, 68 | 71, 70, **75** |
| `schema_bytes` (fd) | 5,356 | 5,495 |
| cost | $2.836 | $2.882 |

**The aggregate failures are gone.** Q60 does not appear in the re-run's failure
list at all, and Q26 recovered to 2/3 — its one remaining miss is an engine
error, not the binding form. That was the whole prediction: four samples lost to
a line of documentation that taught the builtin without teaching that it binds.

**The price was 139 bytes of `schema_bytes`**, declared in advance under § 6's
*A declared price*. Charged once per session; it bought back a class of question
an agent could not previously write. No other cost metric moved —
`answer_bytes_total` is unchanged at 38,985 and 93,905 — and no confirmation
rate moved, which is the half of the gate that is not spendable.

**Q20 is reconciled, not refreshed into agreement.** Its reference query is now
`?- visibility(A, V), V != "public".`, which answers the question as worded
rather than its first half. The bank guard caught the moved answer and refused
to accept it silently, which is the first time that guard has fired on something
real.

Three of the four surviving failures are empty answers on Q14, Q19, Q34 and
Q68 — the same shape as the old frozen bank's two failures: a signature read as
something it is not. Q19 asks for `name_export`, a relation with three rows in
the whole corpus. Whether a schema can teach a relation that rare is the next
question, and it is not obviously yes.

**This run is not comparable to the 85/90 frozen bank**, and the 93% above is
kept rather than replaced: the before number stays on the page, which is the
condition under which re-running is an experiment and not a re-roll.

---

# The four survivors, read — 2026-10-20 (#85)

Q14, Q19, Q34 and Q68 of the working bank, read from the two Tier 3 runs'
`answers.tsv` and `outcomes.json` (the run that scored 210/225 and the one that
scored 216/225), and re-checked against `tests/fixtures/eval/bank.dl` through
`cargo run -p codeintel --example eval`. **No agent was run for this.** Nothing
below is measured; § Predictions is written before any re-run, so that a re-run
can be held to it.

## Defective-reference check first

All four references recompute and hold. Re-derived by hand, not through the
scorer:

- **Q14** — `within(A, "scip demo store/")` returns 7. `parent` gives three
  direct children (`Backend#`, `Store#`, `open()`) and four below them
  (`Backend#get()`, `Store#get/put/evict()`). Seven.
- **Q19** — one row, `scip ts util/retry().`; it is the third `exported` clause's
  body verbatim (`rules/stdlib.dl:39`). **One latent weakness, not a defect:**
  the question says "*rather than* by their own declaration" and the reference
  has no such filter. It returns the same row only because the fixture has no
  declaration that is both `public` and named in an export clause. Adding
  `!visibility(A, "public")` moves no row here; it is left alone, because a
  reference edit is the owner's call.
- **Q34** — `calls_exact` from `init` is `connect`, `open` (2); transitively 4.
  The reference is the transitive reading. See below: the question does not say
  so.
- **Q68** — `calls_at` to `put` is `connect` and `test_put`, both `exact`.

**Question defect found, not a reference defect.** Q34 begins "The same, for …".
Its referent is Q33. The partition put Q33 and Q34 in different agent groups in
all three rounds of both runs, so no agent ever saw what "the same" meant. Six
bank questions open this way (28, 32, 34, 40, 43, 46). Q34 survives because the
words "each callee" were enough for 5 of 6 samples to guess `reach_of_exact`.

## The four

### Q14 — "contained … in the module `"scip demo store/"`"

- **Wrote** (3 of 6 samples, both runs):
  `?- def(M, _, "module", "scip demo store/"), within(A, M).` Run 2 round 2 the
  same with `P`. Empty: `def`'s fourth column is `Name`, which is `store`.
- **Read the signature as:** the quoted string is the module's name, because the
  question says "the module". `def (S, F, Kind, Name)` is documented; nothing
  says what a quoted `"scip …"` string is.
- **Would have had to see:** that it is a SymId and goes in an `S` position
  directly. The page prints no SymId anywhere — NOTES says only that columns
  print `Name path:line` "not the SymId". The agent that passed wrote
  `within(A, "scip demo store/")`, which is what Q13 (no kind word) gets from
  every sample.
- **Bucket: teachable gap.** Same shape as the M1 bank's Q5 (symbol id in
  `def`'s `Name` slot), which that page called an agent slip at 1 in 90. At 3 in
  6 on one question it is not a slip.

### Q19 — "exported by a TypeScript export clause rather than by their own declaration"

- **Wrote:** all six samples found `name_export` and joined it to `def` — so the
  issue's worry, a relation too rare to teach, is **refuted**: rarity cost
  nothing. Then each added a filter for "rather than":
  `!exported(A)` (4 of 6, both runs); `visibility(A, "inherited")` (1);
  `visibility(A, V), V != "public"` (1, the only pass).
- **Read the signature as:** `exported(S)` — "S is visible outside its crate" —
  as the declaration's own `pub`, so `!exported` means "no `pub` of its own". It
  is not: its third clause is `name_export`, so `!exported(A)` after a
  `name_export` join is a contradiction and always empty. `inherited` was read
  as "states nothing"; in TypeScript a bare declaration reads `restricted`
  (`crates/extract/tests/typescript.rs:141`), and `inherited` is the
  variant/trait-item case.
- **Would have had to see:** that `exported` is the union of `public`, parent
  inheritance and `name_export`.
- **Bucket: teachable gap.** The page states `name_export` and `exported` and
  never connects them.

### Q34 — "The same, for `"scip demo lib/init()."`, … compiler-resolved"

- **Wrote** (1 of 6 fails): run 2 round 2,
  `def(S, F, _, "init"), contains(F, "lib/"), reach_of_exact(S, A)`. It took the
  quoted id apart into a name and a directory. `init` lives in `src/lib.rs`, so
  `"lib/"` matches nothing. The same agent's identical shape in run 1 used
  `"lib"` and passed — by accident of one slash.
- **Read the signature as:** the same SymId-is-a-description misreading as Q14.
- **Would have had to see:** the Q14 line.
- **Bucket: teachable gap, at the noise floor** — one sample in six. Plus the
  dangling "The same" above, which is a question defect and is not charged to the
  schema.

### Q68 — "using the orientation relation only, who calls …"

- **Wrote** (1 of 6 fails): `about(…, "caller", C, _, _), def(C, A, _, B).` It
  took `about`'s `A` to be the caller's symbol and joined `def` for file and name.
  Empty: for `caller`, `A` already is the file and `B` the name
  (`rules/stdlib.dl:317`).
- **Read the signature as:** `about(S, Rel, A, B, Line)` — the page never says
  what `A`, `B` and `Line` hold for any `Rel`. They differ by `Rel`: `sig`/`doc`
  put the text in `A`; `extern` puts package and version there.
- **Would have had to see:** one line saying `A,B,Line` are file, name, line of
  the related symbol. The question's own "bind the caller's file to A" was
  available and was passed over.
- **Bucket: teachable gap, at the noise floor.** The other five samples read
  `A`/`B` correctly without the line.

## Predictions, in advance

Three edits, each a doc line, priced by rendering the page with and without
(`eval --facts bank.dl --schema`: 5,299 → 5,505 bytes, **+206, +3.9%**):

| Fixes | Edit | Bytes |
|---|---|---:|
| Q19 | `exported(S)  S is visible outside its crate: public, inherited from parent, or in name_export.` (`rules/stdlib.dl`) | **+50** |
| Q14, Q34 | NOTES: `a quoted "scip ..." or "local ..." string IS a SymId: pass it as S, never as Name.` (`crates/codeintel/src/schema.rs`) | **+85** |
| Q68 | `about(...)`: append `A,B,Line = file, name, line of the RELATED symbol (sig/doc: A = text).` (`rules/stdlib.dl`) | **+71** |

Cheapest first, by failing samples per byte: Q19 (4 of 6 samples removed by the
`exported` gloss, 50 B), Q14 + Q34 (4 of 12, 85 B), Q68 (1 of 6, 71 B).

Predicted recovery against run 2's 216/225, one re-run at the same seed:

| | run 2 | predicted | would falsify |
|---|---:|---:|---|
| Q19 | 1/3 | **2–3/3** | still ≤ 1/3: the filter confusion is about `visibility` values, not `exported`; the next edit glosses `restricted` |
| Q14 | 1/3 | **3/3** | still failing: the cause is the question's word "module", not the page |
| Q34 | 2/3 | 3/3 — **indistinguishable from noise** | — |
| Q68 | 2/3 | 3/3 — **indistinguishable from noise** | — |
| **Total** | 216 | **221–223/225**, majority-passes 74–75/75 | — |

Q34 and Q68 each fail 1 sample in 6 across both runs; one re-run of 3 cannot
separate a fixed gap from variance, and this page should not claim it did. The
Q19 and Q14 rows are the testable ones. No confirmation rate is expected to move:
every edit is a doc line, none touches a rule.

**What is not recommended.** Teaching `name_export` harder: the evidence is that
it was found 6 of 6. Rewording Q14 before the re-run: that would change what a
recorded run was asked; if Q14 survives the edit, reword it and say so.

**One run settles Q14's cause** (page vs wording) and nothing cheaper does; that
is a ~$2.88 call and is left to the owner.

## Side finding

A two-line `%%` doc in `stdlib.dl` is joined into one page line with the
continuation's leading spaces kept, and the page's own `about` line is already
cut mid-list (`caller| callee`). Single-line `%%` docs were used for the pricing
above to avoid it.
