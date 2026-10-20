# codeintel — rules for implementing agents

Read [specs/00-overview.md](specs/00-overview.md) before doing anything. It is
binding. Then [docs/plan.md](docs/plan.md) for where you are in the sequence.

## The one thing

This project exists because `~/projects/dex` solved the same problem in 84,887
lines of Go, and the excess is not accident — it is what happens when every new
question becomes a new endpoint. Read [docs/research.md](docs/research.md) §1
before you add anything. **You are here to keep it small.**

The mechanism that keeps it small is the query language. A new structural
question is a Datalog rule, not a function, not a CLI verb, not an MCP tool.
If you find yourself adding a `fn exported_symbols_by_dir(...)`, stop — that is
literally one of dex's 34 store methods and it is three lines of Datalog.

## Working rules

**Spec first.** Non-trivial change → update the spec, validate it against the
code, then write code. Spec and code disagree → surface it and ask. Do not
reconcile silently in either direction.

**Stay in scope.** No refactors, dependencies, or modernization unless ordered.
A new dependency is checked against `rust-quality/docs/STACK.md` **first**, then
recorded in [docs/research.md](docs/research.md) §6 with a reason. STACK.md
outranks §6 on crate choice; a deliberate deviation gets its trigger written
down. The bar is high — that table is the entire budget.

**Quality gate.** This repo consumes
[rust-quality](https://github.com/alehatsman/rust-quality) via
[provision](https://github.com/alehatsman/provision); `provision`'s own `tasks/`
is the reference wiring. Read `rust-quality/docs/RUST.md` before writing Rust.

```sh
provision apply tasks/ci-fast.yml   # pre-commit: lockfile, fmt, clippy, staged ai-lint
provision apply tasks/ci.yml        # pre-push: the full gate
provision apply tasks/findings.yml  # -> .gate/findings.jsonl, machine-readable
```

Read `.gate/findings.jsonl` rather than scraping terminal output. The lint block
lives in the root `Cargo.toml` and is drift-checked — do not hand-edit it; change
it upstream in rust-quality and bump the pin in `tasks/tools.yml`.

**Investigation is read-only.** Cite `path:line`.

**Classify failures before fixing.** caused-by-change / pre-existing /
environment / dependency / unclear. Then fix.

**Judgment call with blast radius → ask, don't pick.**

**Git.** Worktrees at `~/worktrees/codeintel/<branch>`, outside the repo.
Conventional branches and commits. Never auto-push `main`. Never add Claude
attribution to commits or PRs.

**Close with:** changes, files, validation, branch/commit, queued friction,
commit message.

## Invariants you may not break

From [specs/00-overview.md](specs/00-overview.md), repeated because these are
the ones that get broken by a well-meaning commit:

1. **Facts are extracted, never inferred.** Every tuple traces to a byte range
   or a SCIP field.
2. **Provenance is never averaged.** `exact` and `name`. Two values. No
   confidence floats, no blending, no third tier.
3. **Derivation lives in Datalog.** If it can be a rule, it is a rule. `calls`
   is a rule. Resist moving rules into Rust "for speed" — measure first, and
   [docs/plan.md](docs/plan.md) M6 is where measuring happens.
4. **No scores.** No PageRank, betweenness, communities, importance, or smell
   rankings. This is not an oversight; [docs/research.md](docs/research.md) §1c
   has the measurement showing why such a score over a mixed-provenance graph
   lies.
5. **Truncation and degradation are reported.** Never a silent cap, never a
   silent empty. An `ok` with zero rows and a `no-index` must be distinguishable
   without reading prose.
6. **`crates/datalog` knows nothing about code.** No dependency on `facts` or
   `extract`. A test enforces this.

## Before adding anything, answer these

1. Can it be a Datalog rule in `rules/stdlib.dl`? → Then it is one.
2. Does it add a CLI verb or MCP tool? → The budget is 5 and 1. What are you
   deleting?
3. Does it need a new dependency? → Justify it in research.md §6.
4. Does it compute a number that ranks code? → No. See invariant 4.
5. Does it make an extractor guess? → No. Emit nothing and let `Prov` tell the
   truth ([specs/02-extraction.md](specs/02-extraction.md) § Name matching).

## Language support

Four out of the box — Rust, Go, Python, TypeScript/TSX — in two tiers
([docs/plan.md](docs/plan.md) M5). Five `lang.rs` rows, since TSX is its own
grammar. This read "nine", which is the number M5 narrowed to four and
[specs/00-overview.md](specs/00-overview.md) § Layout gives the reason for: the
extractor, not the engine, was the under-estimate (#101). Adding one
is a grammar crate, two **authored** `.scm` files, and a `lang.rs` row — **if
you find yourself writing per-language Rust, the extractor is wrong.** Fix the
extractor.

**The one exception, and its bar.** `Export` in `lang.rs` is a closed
vocabulary of visibility *mechanisms* — a vocabulary the way `Kind` and `Role`
are. A language picks a variant in its row; it does not get code. A language
whose visibility none of the four describe needs a fifth variant, and that is
the only Rust a new language may add. The bar, all four:

1. It cannot be a `tags.scm` capture. A tree-sitter predicate filters a match,
   it cannot label one, so the query form needs each pattern written twice and
   double-matches the node — which emits two `def` rows.
2. It is parameterized by data on the row, not by the language's name. A
   `match lang.name` anywhere in `crates/extract` is the thing this rule
   forbids, and there is none today.
3. It is usable by a language other than the one that motivated it.
4. [specs/02-extraction.md](specs/02-extraction.md) § `Export` gains its row in
   the same commit.

Nothing else is exempt. `@scope.impl` is a reserved capture suffix, not Rust
leaking in — any language with implementation blocks uses the same three
captures (`@scope.impl`, `@trait`, `@target`) and the same language-neutral
code path.

## Testing expectations

Non-negotiable, per milestone in [docs/plan.md](docs/plan.md):

- **Engine:** differential testing against the naive evaluator. A semi-naive bug
  drops rows silently; this is the only practical defence.
- **Extraction:** golden fact files, diffed as text. Span exactness asserted by
  re-parsing the **file** and checking every `def_span` against node boundaries
  in context ([specs/02-extraction.md](specs/02-extraction.md) § Validation 2).
  This read "re-slicing and re-parsing", which names a check that is not run and
  cannot be: a method, a field, an enum variant and a TypeScript constructor are
  nodes only inside their parent, so 59 of 167 fixture definitions fail the
  slice-alone parse with exact spans (#98).
- **Determinism:** every query byte-identical across repeated runs, with the
  count justified where it is paid. The engine's conformance suite runs 100
  over four synthetic programs (`crates/datalog/src/conformance.rs:342`); the
  real-index suite runs `RUNS = 10` over the documented corpus
  (`crates/codeintel/tests/documented_queries.rs:332`), where one pass is
  ~3.5 s in the test profile and 100 would be ~6 minutes inside a gate that
  takes about one. Ten runs is nine comparisons against the first, so a
  hash-seed-dependent order that flips half the time is missed with
  probability ~2^-9. This read "across 100 runs", which was true of the cheap
  enforcer and unaffordable for the one where it bites (#99).
- **Idempotence:** index twice → identical segments. Shuffle file order →
  identical facts.
- **Surface budget:** asserted in CI.
- **Cost:** `tests/counters.rs` pins `derived`, `demand` and `transformed` for a
  fixed query set over the fixtures. `tests/corpus_counters.rs` pins `status`,
  `truncated`, `rows`, `derived`, `demand` and a digest of the rows for the
  pinned corpora, and is in the gate too (#94) — the corpus baselines used to
  be compared only by a local wall-clock run, which is how fd's `derived` moved
  and sat for weeks (#88). `UPDATE_GOLDEN=1` / `UPDATE_COUNTERS=1` only once
  you can say which changed and why:

  - `rows` moved → the answer set changed size. Correctness.
  - `rows` same, `digest` moved → different rows, same number of them.
    Correctness, and the one a count alone cannot see (#88).
  - rows and digest same, `derived` moved → same answer, different work. Cost.

  The fixture test has no digest, so "rows unchanged means a cost change" holds
  only there, and only until it does not.
- **Performance claims** cite `provision apply tasks/bench.yml`
  (`target/bench/<name>.json`) against the pinned corpora. A hand-run table on
  an unpinned tree is not evidence.
- **Eval:** `tests/eval_rot.rs` recomputes the agent eval's set A. A moved
  reference answer is a finding, not a golden to refresh.

## Friction

The user's tools are `provision` (alehatsman/provision) and `moongit`
(alehatsman/moongit). Friction with them — crashes, missing capability, unclear
errors, broken idempotency — gets captured with `gh issue create`, never
patched, never routed around. Queue it and raise it at close. Do not derail the
current task.

## Voice

Clone trooper. Short, tactical, no corporate filler.
