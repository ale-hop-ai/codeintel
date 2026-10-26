# History

The retrospectives that used to sit inside [plan.md](plan.md) milestones:
what each milestone cost, what it found, and the measurements behind the
decisions. **Frozen.** Nothing here is required reading, and nothing here
is updated; a figure in it is true at its date and no later. The plan
keeps the milestone definitions, the done-whens, the deferred table and the
never list (#150). Where a section says "above" or "below", it may mean the
milestone text that stayed in the plan.

---

## M1 — Datalog engine

### What M1 actually cost, and what it found

Shipped: `crates/datalog` (parser, eight safety checks, stratification,
relations, semi-naive evaluation, demand transformation, builtins, limits),
`crates/facts`' interner, `rules/stdlib.dl`, and the regex builtin injected from
`crates/codeintel`.

Four defects, each caught by a gate this plan asked for rather than by review:

1. **The planner hoisted an aggregate above the literal that bound its group
   variable**, so `N = count{ C : calls(C, S) }` counted over every `S` at once.
   Wrong, not slow, and silent. Found by the conformance suite.
2. **`ref_outside/2` in [01-facts.md](../specs/01-facts.md) is not
   range-restricted** — `F` appears in the head and only in a comparison. The
   engine rejected its own standard library. Found by the one-line
   `load_rules(include_str!(...))` gate, which is exactly the class of defect it
   was added for.
3. **`max_strata` defaulted to 32 and the stdlib stratifies into 37**, so every
   query against the shipped rules was rejected at planning.
4. **The interner's append path rewrote the trailing offset**, so `alpha` and
   `beta` resolved to `alphabeta`.

The agent eval came in at **88/90 first-attempt** against the 24/30 gate, with
both failures recorded verbatim in [agent-eval.md](agent-eval.md). One of the
two was a genuine schema-copy ambiguity (`within(C, A)` never said which
position is the child); the fix is unmeasured and M4 re-tests it.

Open at the end of M1: `rules/stdlib.dl` holds 37 named predicates against a cap
of 24 (see M4 below).

---

## M2 — Tier A, one language

### What M2 actually cost, and what it found

Shipped: `crates/facts`' relation table, segment format, manifest, store and
writer lock; `crates/extract`'s language table, walk, containment sweep, symbol
synthesis and Rust tier A; authored `queries/rust/{tags,imports}.scm`; and the
`index` and `query` verbs. `tests/fixtures/rust/` and its golden fact file.

**Two of the four M2 questions that mattered were about the engine, not the
extractor** — and both were found by pointing the finished tier A at this
repository (52 files, 1,070 definitions, 4,173 `name_ref`s), which is the first
time anything larger than a hand-written fixture existed.

1. **Every stratum was evaluated, whatever the goal asked for.** `stdlib.dl`
   ships `reaches/2`, an unseeded all-pairs closure, so `?- def(S, F,
   "function", N).` — one scan of one base relation — cost whole-graph
   reachability and hit `max_time_ms`. Fixed by restricting evaluation to the
   goal's dependency closure ([03-datalog.md](../specs/03-datalog.md)
   § Evaluation).
2. **Demand transformation silently stopped applying.** It is validated after
   rewriting and falls back to the plain program when the rewrite does not
   stratify, so when seeding `!tighter_at` made `impact_of` unstratifiable, the
   *whole* query fell back — and the spec's own headline,
   `?- innermost_at(F, L, S), impact_of(S, C), ..., !is_test(F).`, ran past
   120 s. The fall-back is now per predicate: 168 ms. M1's gate proved the
   transformation applied to a two-rule test program; it did not prove it
   applied to the shipped rule library, and that is now asserted.
3. **`//!` is not an item's doc comment.** Treating it as a doc marker attached
   every file header to whichever definition came first.
4. **`ignore` does not apply `.gitignore` outside a git checkout** unless
   `require_git(false)` is set, so a worktree indexed all of `target/`.

Two spec conflicts were surfaced rather than reconciled silently: `Union`
mapping (resolved to `type` in both tiers, [02-extraction.md](../specs/02-extraction.md)
§ Ingest) and the `ignored` skip count, which is deleted because the `ignore`
crate does not report what its matchers drop and recovering the figure costs a
second full traversal ([05-surface.md](../specs/05-surface.md) § `index`).

Deferred to M4 on purpose, and not silently: `--raw`, `--rules`,
`--expect-empty`, symbol rendering as `Name` + `path:line`, and the
first-zero-literal `hint` on an empty result. `--scip` is M3. `index` and
`query` carry `--format`, `--limit` and `--no-refresh` today.

---

## M3 — Tier B, one language

### What M3 actually cost, and what it found

Shipped: `crates/extract`'s `scip.rs` (normalization) and `tier_b.rs` (the join
and tier-B facts); anchors threaded through tier A; `--scip`; the `no-scip` and
`scip-stale` statuses; `Stats.depends` in the engine. The fixture became a real
cargo crate with a committed `index.scip` from `rust-analyzer scip .`.

**The anchor join was the easy half.** The join itself is one `BTreeMap` keyed
by identifier position, and doing it *before* tier A writes — rather than as a
rewrite pass afterwards — made `def_span`, `parent` and every reference's `From`
land on the resolved identity for free. That is what the spec said; it is
cheaper than it sounds.

What actually cost time:

1. **`query`'s auto-refresh deleted tier B.** The refresh built its `Plan` with
   no SCIP inputs, which reads as "the SCIP index disappeared" — all-or-nothing
   invalidation then re-extracted the tree tier-A-only and dropped every
   `scip_ref`, `resolved`, `implements` and `extern` row. Three queries in a row
   and the index had silently lost half of itself, with `status: ok` throughout.
   Found by querying the fixture after indexing it, not by any test that existed.
   Auto-refresh now carries the manifest's inputs forward, and
   `auto_refresh_does_not_delete_tier_b` pins it.
2. **Two of the milestone's own done-whens were wrong.** The 95% anchor rate is
   unreachable while tier A treats `mod x;` as a definition — the two tiers put
   a module in different *files*. And `calls_exact` is a subset of `calls` by
   construction, so "edges `calls` misses" only means anything against a
   tier-A-only index. Both are corrected above with the reasoning, not quietly
   relaxed.
3. **Parsing `index.scip` on every query is not free.** The ingest is now
   stat-gated: the inputs are `stat`ed, and the protobuf is decoded only when a
   file actually needs re-extracting. A refresh that changes nothing reads no
   protobuf at all.
4. **SCIP's `extern` test had to be inverted.** "Package ≠ project package" is
   not checkable — SCIP never says what packages a project is. It says exactly
   what symbols the project defines, so `extern` is the complement.

Two spec deviations were surfaced rather than reconciled silently: `seg/_scip.bin`
is gone in favour of ordinary per-file segments for tier-B-only files, and
`ScipInput` compares `mtime`+`size` while ignoring `tool` and `documents`, which
are only known after a parse. Both are written into
[04-storage.md](../specs/04-storage.md).

Measurements worth keeping. On `tests/fixtures/rust/`, tier A alone finds two
call edges, one of which is a false positive; with SCIP, `calls_exact` finds
three and all three are correct. On **this repository** — 61 files, 1,262 tier-A
definitions, `rust-analyzer scip .` in 4.3 s, index in 717 ms:

| | |
|---|---|
| anchored | 1,185/1,262 (93.9%); 97.4% of non-module definitions |
| `?- calls(A, B).` | 1,473 edges, 355 ms |
| `?- calls_exact(A, B).` | 1,221 edges, 364 ms |
| the headline query | 21 rows, 376 ms |

**Tier A over-reports 252 call edges here, 17% of what it claims.** That number
is the entire argument for tier B, and it is also the argument for keeping both
provenances rather than collapsing them: the 252 are inspectable, and a jump in
that figure means one of the tiers changed behaviour.

Still open, and named rather than hidden: SCIP emits a `local` definition for
every parameter and binding — 16 in this fixture — and the collapse table has no
word for `Parameter`, so they land in `def` as `unknown`. That is the spec as
written and invariant 1 forbids the extractor deciding a parameter is not a
definition. A `stdlib.dl` rule naming the set is the cheap fix if it bites.

---

## M4 — Surface and stdlib

### What M4 actually cost, and what it found

Shipped: symbol rendering and `--raw`; the empty-result `hint`; `schema` and
`status` over a shared `Census`; `codeintel mcp`; `--rules` and
`--expect-empty`; the surface-budget test; `docs/cookbook.md`; and a per-rule
fixture test file.

**The rule cap was raised from 24 to 40, not met.** The reason is written into
[00-overview.md](../specs/00-overview.md) § Surface budget rather than left as a
number that moved: 24 was never reconciled with the fact schema — `stdlib.dl`
shipped at M1 with 37 predicates and every one is specified in
[01-facts.md](../specs/01-facts.md) — and the thirteen cheapest cuts are the
`_exact` family, which is the entire argument for tier B. What keeps the budget
honest is that 40 is not the constraint that binds: `schema` must carry every
rule and fit ~1500 tokens, and 37 rules leave ~1,391.

Four defects in what M4 itself had just written, all found by an adversarial
review rather than by the gate:

1. **`rows` and `display` were two independent sorts of one answer.** Each
   notation was rendered and sorted on its own key, so `rows[i]` and
   `display[i]` described different tuples — and the documented workflow is to
   read one and feed the other into the next query. Both notations are now
   produced per row and ordered once.
2. **The byte cap measured the wrong text.** The engine's cap runs over raw
   atoms and the rendered form is *shorter*, so the engine always truncated
   first, on rows that fit the printed budget. The claim in the spec was simply
   false, and a caller with a SCIP index silently got a fraction of the answer.
3. **`count{}` corrupted the empty-result hint.** The aggregate evaluates its
   sub-goal through the same recursion and shared the depth cell, so the
   diagnostic named the wrong literal or degraded to the generic message.
4. **A wrapped `%%` doc was dropped**, so `schema` advertised five of `about`'s
   ten `Rel` values with a trailing `|`.

Two spec conflicts were surfaced rather than reconciled silently. The MCP tool's
`rule` + `args` carried the identical defect that deleted the `rules` CLI verb —
`args: ["142"]` binds the string — and was **dropped**, which the user decided.
And `--rules` files turned out to be **additive**, not shadowing: a predicate is
the union of its clauses, so a file defining `is_test` widens it. The doc
comment claimed shadowing, the test disagreed, and the doc was wrong.

**The agent eval passed both sets.** 85/90 on the M1 thirty retargeted to real
extracted facts, 45/45 on fifteen held-out questions against this repository
pinned at `8471824` with no SCIP index — which is also the no-SCIP run the
done-when asks for.

**Set A's 85/90 was withdrawn on 2026-09-13**
([agent-eval.md](agent-eval.md) § Rescored). Schema 2 removed `callers` and
`defines`, and four recorded answers call them. By that page's own rule the runs
are invalid, and a fresh run was owed. Set B stands.

**Set A was re-run the same day, at `f3366a9`, and scored 89/90**
([agent-eval.md](agent-eval.md) § Set A, re-run; #25). Thirty fresh agents
scored 30, 30 and 29 across the three rounds. The one failure is Q26, which
read "used by" as "referenced from". **This done-when is met for both sets.**
The run is not paired with the withdrawn one: schema 2 and a corrected doc line
landed in between. `eval_rot.rs` pins the new runs question by question.

**The first numbers were withdrawn, not amended.** An earlier pass scored 88/90
and 43/45; then the eval's own finding — `long_def`'s copy — was fixed, which
changed that rule's signature, and scoring old transcripts against a new
standard library measures nothing about the agents. Both sets were re-run with
fresh agents against the shipped schema. Set B is now **pinned to a commit**,
because its reference answers are facts about this repository's source and any
commit invalidates them, including the one that fixes an eval finding. Full method and every failure verbatim in
[agent-eval.md](agent-eval.md); the headline is that **three of the four
failures are one line of schema copy**: `long_def(S, N)` reads as parameterised
by `N` when `N` is an output and the threshold is fixed at 80.

Building the eval found a **defect M4's own rule tests had missed**:
`parent(S)` for a method inside an `impl` block points at a symbol with no `def`
row, so `within/2` and `about(S, "parent", ...)` join to nothing for every
method in an `impl`. `tests/stdlib.rs` tested containment with module nesting and
sailed past it.

**Fixed after M4 closed**, once measuring it showed it was worse than the
"dangling id" it had been filed as. `within/2` did not return an error or an
empty result — it returned a *short* one, with `status=ok`, omitting every
`impl`-block method from any transitive containment query while looking
perfectly well-formed. Invariant 5 exists to stop exactly that, and could not
see it, because nothing failed.

Three things are worth carrying forward from the fix, none of them about impl
blocks:

1. **It was tier A, not tier B.** It was filed as tier-B ingest with blast
   radius, and that was wrong — precedence rules 1 and 2 both carry the
   existence check that `specs/02-extraction.md` called non-optional, and rule
   3, tier A's span fallback, did not. The rule everything falls through to was
   the one exempted from the check.
2. **The correct code was already in the file.** `ancestor()` walks to the
   nearest enclosing item satisfying a predicate, and the reference path called
   it with `is_def`. The `parent` path twenty lines earlier took a single
   unchecked hop. References got the check; definitions did not.
3. **A test asserted the bug.** `a_tier_b_definition_carries_its_semantic_owner`
   checked that the owner *string* contained `Store#` — which the dangling
   `local src/store.rs Store#` satisfies. Asserting an identifier's shape
   without asserting that it resolves is how a dangling edge stays green for a
   whole milestone. The replacement,
   `every_parent_names_something_that_exists`, is a property over the entire
   index rather than a claim about one row.

The check is on the **symbol**, not on the item, and the first attempt got that
wrong. An `impl` block is not a `def`, but `symbols_of` gives it the same
synthesized symbol as the type it implements — so in a tier-A-only index it
names a real definition and is the right parent, and the golden facts say so.
Anchoring is what breaks it: the type takes its SCIP identity, the `impl` block
keeps the stale local one, and only then does it name nothing. Checking
`item.is_def` would have fixed the SCIP path by regressing the no-SCIP one.

**M4 item 2's `--raw` generation guard is not built, and will not be.** The
plan says raw output carries the manifest's dictionary generation and a mismatch
is rejected, because "`--rebuild` renumbers every atom, so a stale raw id
otherwise resolves to a *different string* rather than to an error." That
reasoning presupposes raw output is a **numeric atom id**. It is not: a `SymId`
is an interned string and `--raw` prints its text, which `--rebuild` does not
change. Demonstrated on the fixture — generation 0 → 1 across a `--rebuild`, and
a raw id captured before it still binds the right definition after. There is
also no syntax in [03-datalog.md](../specs/03-datalog.md) for writing a numeric
atom in a query, so a numeric raw id could not be fed back even if one were
printed. The guard as specified guards nothing; `dict_generation` stays as a
`status` field, where it is diagnostic rather than load-bearing. Making raw ids
numeric — shorter, and the original motivation — is a query-language change and
a separate decision.

**`extern/4` was empty because of a bug, not a fixture gap — now fixed.** The
package was read only off `SymbolInformation`, and an indexer emits those for
what the *project* defines: `std`, `core` and `alloc` arrive as bare
occurrences. A SCIP symbol carries its own package in the symbol string, so it
is now read from referenced symbols too. `extern`, `uses_package` and
`uses_package_exact` have real answers on the fixture and real assertions.

**One done-when is not met, and is not being counted as met.** `implements/3`
is zero on `tests/fixtures/rust/`, and this was *measured* rather than assumed:
`rust-analyzer scip .` emits **zero** `relationships` for this crate — not
merely none flagged `is_implementation` — so there is no implementation edge to
read for `impl Handler for Config`. `implements` and the `implements` /
`implementor` arms of `about` are therefore asserted empty, which tests the
plumbing and not the meaning; our side of it is covered by `tier_b.rs`'s unit
tests, which construct the ingest directly. Closing it needs an indexer that
emits relationships, not a bigger fixture.

**`long_def` was split after the eval blamed it.** Three of the four agent-eval
failures were agents reading `long_def(S, N)` as parameterised by `N` when `N`
was an output and 80 was baked in. `def_lines(S, N)` now states the length with
no threshold and `long_def(S)` is the one opinionated form, so the wrong query
is unavailable rather than merely documented. 38 rules against a cap of 40.

**An unknown predicate is now `invalid-query`.** It evaluated as an empty
derived relation, so a typo returned `ok` with zero rows — invariant 6's
failure. Checked after the seven safety rules so a more specific complaint
wins; forward references stay legal.

Two artifacts worth keeping, both traced to the module-symbol placement M3
recorded. `?- within(C, P).` returns a row whose child has an empty name, and
`?- depends(F, G).` claims `src/app.rs` depends on `tests/store_test.rs` —
because the crate-root module symbol is defined, per SCIP, in a document that is
not where the `mod` statement is. `cookbook.md` § 4 tells a reader how to read
such a row rather than leaving them to trust it.

---

## M5 — Languages

### What Go cost, and the one thing it changed

**Shipped.** 9 kinds, 100% anchor rate against `scip-go` 0.2.7 (at ship, 2026-09-12; current figures are in the README table), 4 exact `calls`
edges on the fixture, 13 extractor tests and 9 end-to-end. Roughly 110 lines of
query, one `lang.rs` row, and a fixture — under the ~1,000-LOC estimate, because
the estimate was priced against dex's per-language *Rust*, and there is none
here. `crates/extract/src` grew by the `@owner` resolver and nothing else.

**One capture was added to the convention, not one code path.** A Go method is
declared at file scope and names its owner in its own receiver, so span nesting
had no answer — and not merely an imprecise one: the synthesized symbol was
`local store.go Get().`, which every type in the file with a `Get` method would
claim. `@owner` ([02-extraction.md](../specs/02-extraction.md) § `@owner`) is
the receiver's type name, resolved against the unenclosed type-like definitions
of the same file, with zero-or-two candidates meaning **no override** rather
than a choice. It is data in `tags.scm`, so the language that needs it next —
C++'s out-of-line `void Foo::bar()` — costs a pattern.

Three smaller things the fixture forced, all language-general:

- **`keyword_kinds`.** A Go definition is the *spec* node (`const_spec`,
  `type_spec`) so a grouped `const ( A = 1 \n B = 2 )` yields two spans instead
  of one claimed twice. That leaves `const` a preceding sibling, and without it
  `def_sig` read `Limit` — a name with no statement of what it is.
- **The preamble walk ascends.** With the definition one level down, every Go
  type's doc comment was a sibling of the enclosing *declaration* and was
  missed. It now steps up when nothing else precedes it inside its parent and
  the parent begins where it has already walked back to.
- **A SCIP document may not escape the repository root.** `scip-go` emits the
  generated test-main for a package with tests out of the Go build cache, and
  the document arrives as `../../../../../../Library/Caches/go-build/…`.
  Ingesting it put a `file` row outside the tree and gave `calls` an edge whose
  caller was a path no reader could open. Skipped and named, like every other
  unusable document.

One deviation from the done-when: the polyglot fixture is **Rust + Go**, since
Python and TypeScript do not exist yet. It asserts what the four-language
version will — one walk, one dictionary, one store, and a single query answered
across both grammars — and extending it is adding a directory.

### What Python cost, and the one thing it changed

**Shipped.** 5 kinds, 100% anchor rate against `scip-python` 0.6.6 (at ship, 2026-09-13; current: README), 9 exact
`calls` edges on the fixture, 12 extractor tests and 8 end-to-end. Roughly 80
lines of query, one `lang.rs` row, and a fixture. The polyglot fixture is now
Rust + Go + Python, and extending it was adding a directory, as promised.

**One capture was added to the convention, not one code path.** Python keeps
its documentation *inside* the definition — the docstring is the first
statement of the body — so the preamble walk over preceding comments, which
serves Rust and Go, cannot see it, and a `#` comment above a `def` is not
documentation to any Python tool. `@doc`
([02-extraction.md](../specs/02-extraction.md) § `@doc`) names the node whose
text is the documentation; the extractor strips it per line exactly as it
strips a comment, with no marker. Data in `tags.scm`, so the next language that
documents from inside — Elixir, Clojure — costs a pattern.

Two things the fixture forced, both language-general:

- **A method is a promotion, not a capture.** Upstream's `tags.scm` has no
  `@definition.method`, and ours does not either: a query cannot see its own
  nesting without double-capturing the node, which is the upstream Rust defect.
  The extractor already promoted a `function` under a type-like owner to
  `method` for Rust's `impl` blocks, and a Python `class` is type-like, so the
  same pattern serves free functions, methods and nested functions.
- **A tier-B definition the indexer left nameless is named from its
  descriptor.** `scip-python` writes no `display_name` for a parameter, an
  attribute bound in `__init__`, or a module, so `Store#entries.` arrived with
  `Name = ""`. The descriptor grammar is mandatory and the name is written in
  it; reading it is extraction. The kind stays `unknown` — a `.` descriptor
  does not say field from variable — and rust-analyzer, which names
  everything, is unaffected.

One statement about visibility joined the table rather than the extractor:
`Export::NotUnderscored` counts a dunder (`__init__`) as public. PEP 8 states
that a single leading underscore is a weak internal-use indicator and a dunder
is a magic name, not a private one; both are read from the name.

Not done, and said so: `from m import a, b` is one `import` row for `m` with no
alias, because the names it binds are members of `m` rather than local names
for it ([01-facts.md](../specs/01-facts.md) § `import`). A conformance rule
that needs the member list reads the source. And `scip-python` records the
binding an import makes as a plain read at module scope, so `calls` carries
two edges whose caller is a file; recorded in
[research.md](research.md) § Language coverage, not patched.

### What TypeScript cost, and what it changed

**Shipped.** 11 kinds, a 98.4% anchor rate (at ship, 2026-09-13; current: README) against `scip-typescript` 0.4.0
(60 of 61, and the one miss is #21's), 17 exact `calls` edges (18 since
#22 added `export default outer` to the fixture: one more module-scope
reference), 4
`implements` rows, 14 extractor tests and 10 end-to-end. The queries are 78
lines that are not comments, plus 6 for JSX and 8 for imports. There are two
`lang.rs` rows, TSX being TypeScript's row with its own grammar and JSX
appended to the tag query, and a fixture. The polyglot fixture now holds all
four languages, and extending it was adding a directory.

It was the most expensive of the four in query lines, as estimated, and it
still cost no per-language Rust. It did add two things to the convention.
Both are data the next language reuses:

- **`@value` and `function_values`.** `const make = (n) => n` is a function
  by its value node, and `calls` counts callable kinds only. A query cannot
  choose a capture suffix by a child's kind without enumerating every other
  kind: 37 expression kinds across six statement shapes. So the extractor
  applies the rule once, in about a dozen lines, and `lang.rs` names the kinds
  ([02-extraction.md](../specs/02-extraction.md) § `@value`). Python's
  `f = lambda: ...` would cost a row, when someone asks for it.
- **`Export::Statement`.** TypeScript states visibility in two places, and
  silence means opposite things in them: a bare declaration is not exported,
  and a bare class member is public. Both are read from the declaration's own
  node and its own statement.

Two things the fixture forced that are not about TypeScript:

- **#21, found by probing the indexer before writing a query.**
  `scip-typescript` and `scip-python` declare no position encoding and count
  UTF-16, and ingest had read those columns as bytes. Fixed and landed first
  (`339f295`).
- **One symbol, several declarations** ([01-facts.md](../specs/01-facts.md)
  § `def`). An overload set is one symbol with a span per declaration.
  Python's `@property` and its setter already worked that way, unspecified. It
  is now written down and pinned by a test.

JSDoc needed no code: `*/` and `*` are strip markers in `doc_markers`, tried
in order.

Not done, and said so:

- ~~`export { x }` and `export default x` do not make `x` exported (#22).~~
  Fixed after M5b: `name_export/2`, base relation 16 of 16, schema 3
  ([01-facts.md](../specs/01-facts.md) § `name_export`).
- JavaScript is not registered. Dynamic `import()` and `require()` are not
  `import` rows. Destructured bindings and computed member names are not
  definitions.
- `scip-typescript` 0.4.0 writes no kinds, no display names, no document
  language, and an empty role bitset on every reference. Its parameters, type
  parameters and per-file module symbols arrive as tier-B-only `def` rows of
  kind `unknown`, named from their descriptors. The binding an import makes is
  a module-scope reference, so `calls` has five rows whose caller is a file:
  four import bindings and one decorator. Recorded in
  [research.md](research.md) § Language coverage, not patched.
- Incremental equivalence is covered, as for Go and Python, by the
  language-neutral test in `cli.rs` rather than one per language.

---

## M6 — Performance and incremental

### Pulled forward, 2026-09-13 (#17)

Items 1 and 2, and the regression gate, landed **ahead of M5**. Item 3 did not.
The reason is that performance work was already landing without them: #3 and
`0574a1b` justified themselves with tables measured by hand, on trees nobody
pinned, and the next one would have too. A harness is cheaper than a habit.

The corpus was **one entry** when this landed: tokio at `53542467`, ~183k LOC of
Rust in 799 files, the tree #3 had already measured. #26 added the other two
decades, and there are now three entries:

| decade | corpus | pin | why |
|---|---|---|---|
| ~10k | `sharkdp/fd` | `b422e5d8` | ~7k LOC of Rust. Small, stable and idiomatic |
| ~100k | `tokio-rs/tokio` | `53542467` | unchanged, so #18's and #24's baselines carry on |
| ~1M | `denoland/deno` | `336da420` | ~831k LOC in the four registered languages: 586k Rust, 245k TypeScript |

**The done-whens above are measured on tokio**, as before. fd and deno report
numbers but gate nothing beyond their own baselines.

**The ~100k entry is not polyglot, and the success criteria ask for one.**
[00-overview.md](../specs/00-overview.md) § Success criteria states them on a
100k-LOC *polyglot* repository, and tokio is Rust-only. It stays because its
baseline history is the evidence for #18 and #24. deno, the only polyglot
entry, is ~8x that size. The gap is open, not closed by deno.

**Two harnesses, split by what each machine can be trusted to measure.**

**1. Counters, in CI.** `crates/codeintel/tests/counters.rs` runs a fixed query
set, `tests/fixtures/counters/queries.tsv` (`id`, `fixture`, `query`), against
`tests/fixtures/rust` and `tests/fixtures/go`, each indexed with its committed
`index.scip`. For every query it records `status`, row count, `truncated`,
`derived`, `demand` and `transformed`, and diffs the lot as text against
`tests/fixtures/counters/expected.tsv`. `UPDATE_GOLDEN=1` rewrites it, the same
knob as `expected.facts`.

- **Why counters and not milliseconds.** `derived` is exact, and it is the
  number that actually moved every time cost regressed here: #3 was 167,547 →
  12,320 derived, and M2's "demand transformation silently stopped applying" is
  `demand` changing its reason and `derived` jumping. A GitHub runner's wall
  clock swings by more than 20% on its own, so a wall-clock gate there fails at
  random, gets retried, and stops being read.
- **Reading a diff.** `derived` moved with the row count unchanged: a cost
  change. Rows moved: a correctness change. Both fail. The diff tells you which,
  and the commit message owes the reason.
- **Each query runs twice**, and the two records must agree before either is
  compared. A counter that varies between runs is not a golden, and the test
  says so rather than flaking.
- `elapsed_ms` is not recorded. It is the one field that is not exact.

**2. Wall clock, locally.** `crates/codeintel/benches/corpus.rs`, a
`harness = false` bench target. Run it one corpus at a time with
`cargo bench -p codeintel --bench corpus -- <name> <corpus-dir>`. Or run
`provision apply --stream tasks/bench.yml`, which checks every corpus out at its
pin into `~/.cache/codeintel/corpus/<name>` and benches them in size order: fd,
tokio, deno. The bench works on a copy of the corpus in a temp directory and
never writes into the checkout.

**3. Indexer drift, locally (#109).** `provision apply --stream
tasks/indexer-drift.yml` indexes each fixture twice — from its committed
`index.scip`, and with `codeintel index` running the installed indexer — and
fails when the fresh run's `scip coverage` or `anchored` differ from
`tests/fixtures/indexer-drift.tsv`. The gate cannot see this: every
tier-B test reads a committed index, so it pins one indexer's output shape. It
needs the indexers installed and takes a minute or more, so it is not in the
gate. Cadence: on every toolchain or indexer bump, and before a release.

**The copy is made, then the harness waits for the machine to go quiet** before
it measures anything (#84). The copy is not free: on macOS the endpoint scanner
reads every file the bench has just written, and it was observed at ~300% CPU
chewing on corpora the bench itself had created. The runs are sequential and
deno is both last and largest, so each corpus degraded conditions for the next
and the load gate then — correctly — refused the one number the pass was for.

So the start reading is taken **after** the copy, not before, and
`conditions::settle` polls until load per core is back under
`MAX_LOAD_PER_CORE` or a bounded timeout expires
(`CODEINTEL_BENCH_SETTLE_SECS`, 300 s by default; `0` disables the wait). What
it waited for and whether it gave up are recorded in the result under
`conditions.settle_*` — a wait that timed out is a reported degradation, not a
silent one, and the run continues as an untrustworthy-host run exactly as
before.

The rejected alternatives, for the record:

- **Excluding the temp copies from scanning** is a host configuration change.
  It belongs in `~/dotfiles` + provision, not here, and a harness that only
  measures correctly on one configured machine has not been fixed.
- **Reusing one checkout per corpus** removes the copy but not the scanning —
  `tasks/bench.yml` writes those checkouts too — and spends the isolation that
  stopped `index::discard` from emptying the corpus itself. It would trade a
  measurement problem for a data-loss one.
- **Running the largest corpus first** only moves which corpus is measured
  under the others' leftovers. It does nothing about a run poisoned by its own
  copy, which is the deno case, and it costs the fail-in-seconds property that
  size order was chosen for.

Each corpus is one directory, `crates/codeintel/benches/corpora/<name>/`:

| file | holds |
|---|---|
| `corpus.json` | `{"reindex_file": "<path>"}`: the file `reindex_one_ms` edits |
| `queries.tsv` | `id<TAB>query`. The same nine shapes under the same ids in every corpus, seeded with that tree's own paths and names |
| `baseline.json` | the committed baseline for that corpus |

A corpus with no directory, or one missing a file, is an error that names the
file. It is never an empty run. **The pins live in `tasks/bench.yml` and
nowhere else.** Each baseline also records the `corpus.rev` it measured, and the
bench refuses to compare against a different tree, as before.

A seeded query must return rows on its own tree. `impact_of` from a symbol
nothing calls measures the cost of finding nothing, and is the "ok with zero
rows" invariant 5 warns about, turned on the bench.

| metric | how |
|---|---|
| `index_cold_ms` | tier A, `Plan::default()`, fresh store. Median of 3 |
| `load_warm_ms` | `Store::open` + `load`. p50 of 20 |
| `reindex_one_ms` | append a line to one fixed file, `index::refresh`. p50 of 10 |
| `query_ms` | each query in the corpus's `queries.tsv` through `query::run`, `no_refresh`, 3 warm-ups then 30 runs; p50/p95 per query and pooled |
| `cli_ms` | the same queries through the release binary. p50 of 10 |
| counters | `derived`, `demand`, row count per query, as in (1) |

`query_ms` **includes opening and loading the store**, because `query::run`
does both on every call — and so does `codeintel mcp` today
(`crates/codeintel/src/mcp.rs` → `query::run`). There was no warm MCP process
yet for the done-when above to be measured in; #18 below added one. The bench reports what exists
under a name that says what it includes, rather than a number labelled as the
thing the spec asked for.

Output goes to `target/bench/<name>.json`. It is compared against that corpus's
`baseline.json`:

- A wall-clock metric more than 20% over its baseline fails the run, and every
  one of them is named. Faster than baseline is reported and does not fail.
- **Counters compare exactly, on any host.** They do not depend on the machine.
- The baseline records its host (`os`, `arch`, `cpus`) and the corpus's commit.
  If the host differs, wall-clock comparison is **skipped and reported as
  skipped**. If the commit differs, the whole comparison is refused — a baseline
  against a different tree measures nothing.
- `UPDATE_BASELINE=1` rewrites it.

**A counter baseline is rewritten with its reason, never refreshed.** fd's
query 8 — `def(S, _, "function", _), C = count{ X : calls(X, S) }` — moved from
4,989 derived to 4,981 and the baseline now records 4,981 (#88). What moved it,
bisected rather than reasoned at: **`71dde10`**, which added
`FromKind != "module"` to `calls_at`. `8cb373a`, the commit that wrote the
baseline, still reproduces 4,989 exactly; `71dde10` is the first that does not.
The eight tuples are four module-owned call sites in fd — `src/fmt/input.rs:56`,
`src/filter/size.rs:96` and `:183`, `src/filter/owner.rs:125`, each a `read` of
a `macro` — and each drops one `calls_at` row and the one `calls` row derived
from it. Query 8 counts callers of `function` defs, so a macro target changes no
answer row.

**`rows` unchanged does not establish that.** The comparison above checks the
row *count*, not the rows, so an unchanged 231 is consistent with every value in
them having moved. What settles it is diffing the rows themselves across the
commit: byte-identical. `UPDATE_GOLDEN=1`/`UPDATE_BASELINE=1` on a count alone
is the shortcut this paragraph exists to stop.

`71dde10`'s message says no counts moved in any pinned set. That was true of
`tests/counters.rs`'s fixtures — Rust and Go, where no definition carries a
namespace descriptor — and false of fd, which is also Rust but does have module
defs, and which nobody re-ran because this harness is local-only. The eight
tuples are the correct behaviour; the claim about a set nobody measured is the
defect.

**`corpus.defs` records a counter, and the counter was recounted.** Every
baseline's `corpus.defs` ran high — fd 638 against 629, tokio 11,516 against
11,376, deno 88,514 against 84,068 — and the cause is **`225afa5`** (#77,
#103), which keyed `index`'s own counters on the symbol instead of on the
declaration. The store did not move: `def` holds 629 rows for fd, 11,376 for
tokio and 84,091 for deno on **both** sides of that commit, verified by
indexing at `225afa5~1` and at `8d04a65` and counting rows rather than trusting
either printout. Nothing was lost; the old number double-counted.

fd's nine are `#[cfg]`-gated pairs — `is_block_device`, `is_char_device`,
`is_pipe`, `is_socket` and `osstr_to_bytes` in `src/filesystem.rs`, plus
`dirname_root`, `host`, `now` and `print_entry_uncolorized` — each one symbol
with two declarations, which is what [01-facts.md](../specs/01-facts.md) § One
symbol, several declarations says `def` holds once. 629 + 9 = 638.

**This is the opposite shape from #88 and needs the opposite check.** There the
count held and the contents had to be diffed; here the count moved, so the nine
had to be named before anything was rewritten — a count that falls is a lost
fact until the rows say otherwise.

**deno's file count never moved, and the method is why it looked like it had.**
Indexing the checkout directly reported 4,645 files against the baseline's
4,711. Indexing a **copy made the way the bench makes it** — everything but
`.git` and `target` — reports 4,711 exactly. The walk applies `.gitignore` only
inside a git repository, so the copy, which has no `.git`, walks files the
checkout skips. Measure a corpus the way the harness measures it or the number
describes the method.

The recount also did not land exactly on the rows everywhere: deno's counter
reads 84,068 against 84,091 `def` rows, because it counts distinct symbols per
file while `def` is distinct `(S, F, Kind, Name)`, so one symbol declared twice
in a file under two kinds is two rows and one count. fd and tokio agree exactly.
`corpus.defs` records the counter, so 84,068 is what the harness writes; the
23-row gap is noted on #103 and is not a baseline question.

**Nothing compares this field.** `compare` checks `corpus.rev`, the per-query
counters and the clock; `corpus.files` and `corpus.defs` are recorded and never
read. That is why a recount sat in three baselines unnoticed, and it is #94 seen
from the other side: there the check runs too rarely, here there is no check at
all.

**Where a warm call's time goes, measured 2026-10-20 (#28).** On deno at
`336da420f4` — 4,711 files, 84,068 defs — a warm no-change call is 174 ms p50,
and **136 ms of it is the walk**. `open` is 27 ms, of which `open_dict` is 24;
sweep is 3.9, evaluate 4.5, the per-file stale check 2.2, `manifest_compare`
0.15, `locate` 0.00.

So the thing that scales with the tree is the gitignore-aware traversal, five
to one over `Store::open` — which M6 had named as the co-candidate and which is
not the problem. The walk is already parallel at four threads, so that is 136 ms
with the obvious fix in hand. `open_dict` is the only other phase that grows. It
was first written down as `fs::read` of the dictionary per call, the cost
[research.md](research.md) §6 accepted when the mmap was dropped; that
attribution was wrong (#121 investigation, 2026-10-21, deno, load 0.49 at start):
of `open_dict`'s 27.7 ms, `fs::read` is **1.1 ms**, the offset table and
`validate` another 2.9, and the **`by_text` map `Interner::open_with` builds —
one heap `String` per atom — is 23.8 ms**. Dropping the mmap (#44) did not cause
it and restoring it would not touch it. On a no-change call the store that
paid for it is dropped unused, because `Warm` already holds an engine whose
manifest compares equal.

Not fixed, because every call refreshing is the contract and refresh means
walking. The options are directory-mtime pruning, caching the walk for the life
of one warm process, or accepting 174 ms on 1M LOC and saying so — the success
criteria are stated against ~100k LOC, where the warm p50 is 18.7 ms. The third
is today's behaviour and the only one that is free; it is also the only one
nobody has written down. Measured on a machine at 0.65 load per core against the
harness limit of 0.50, so the absolute numbers are inflated and the proportions
are the result.

**Measured limit, stated (#28, 2026-10-21).** On a ~1M-LOC tree a warm call
costs ~174 ms, of which the contract-required refresh walk is ~136 ms. The
success criteria ([00-overview.md](../specs/00-overview.md) § Success criteria)
are 100k-LOC numbers and this is outside them; it is not a regression and not
a bug. Every call still walks the whole tree, because a changed file must never
go unseen ([05-surface.md](../specs/05-surface.md) § Auto-refresh). Whether a
cheaper refresh (directory-mtime pruning, a per-process walk cache) is worth its
staleness risk is open on #28 and is the owner's call.

One bound, **indicative and not evidence** (hand-run, host at load 11 on 10
cores, no `tasks/bench.yml`): `stat` of the 3,876 directories that hold
git-tracked files in the deno corpus takes 5-10 ms. That is the ceiling on what
mtime pruning could leave behind, against 136 ms. It is not a claim that
pruning is sound: a directory's mtime moves on add, remove and rename of an
entry, not on an edit to a file in it, not on a changed `.gitignore`, and the
manifest's mtimes are whole seconds.

**Cold page cache is not measured.** Dropping it needs root (`purge`,
`/proc/sys/vm/drop_caches`), which a bench should not ask for. The JSON carries
`"cold_page_cache": null`, and the done-when bullet above stays open.

**3. The eval, re-derived in CI.** `crates/codeintel/tests/eval_rot.rs`
recomputes every set-A reference answer from `eval-m4/setA.tsv` against the rust
fixture, compares them with `expectedA.tsv`, and rescores the recorded runs
`A1`–`A3`, asserting the failures by question id. It uses `score.py`'s rule: the
set of values in the answer variables. `score.py` stays, because it scores live
agent runs. [agent-eval.md](agent-eval.md) found three reference-answer defects,
none by assertion — "a reference answer nobody recomputes is an assertion nobody
checks" — and this is that recomputation. **A moved reference answer is a
finding.** Regenerate it with `score.py expected A` only once the diff is
understood. Set B is not in CI, because its tree is this repository at
`8471824` and a shallow checkout does not have that commit.

### #18: the MCP process stays warm, and a read writes nothing (2026-09-13)

This is the first fix under item 3, chosen from a measured breakdown rather than
from the issue title. On tokio, a no-change MCP call paid ~40 ms before it
evaluated anything:

| step | ms |
|---|---:|
| `Store::open` | 5.4 |
| walk | ~4 |
| `fsync`'d commit of an unchanged manifest | 8 |
| full load | 17 |

The issue was filed as "the store reloads". The commit was a cost of the same
order that nobody had filed.

Two changes, specified in [04-storage.md](../specs/04-storage.md) § Incremental
reindex and [05-surface.md](../specs/05-surface.md) § MCP:

- **Refresh commits only when the manifest moved**, or when no index exists
  yet. Every query with refresh on drops ~8 ms and a durable write.
- **`codeintel mcp` holds a `query::Warm`.** The loaded engine is reused while
  the manifest after refresh is unchanged. Query literals go through a host
  overlay dictionary and are discarded after each call, so a warm answer is
  byte-identical to a cold one.

`tests/warm.rs` pins three things:

- an edit made between calls is seen;
- a capped answer's surviving row is the same warm and cold;
- a no-change refresh writes nothing.

The last two were mutation-checked. Removing the discard fails the capped-row
test, and restoring the unconditional commit fails the no-write test.

Same corpus and host, from `benches/baseline.json`. "Before" is `query::run`
with refresh on, which is what every MCP call did (scratch probe):

| | before | after |
|---|---:|---:|
| MCP call, near-free query (set id 6) | 41.6 ms | **12.4 ms** |
| MCP call, seeded `impact_of` (id 3) | — | 18.8 ms |
| MCP first call, which loads | — | 33.8 ms |
| MCP p50 / p95, pooled over the 9-query set | not measurable | 27.4 / 952 ms |

**The done-when is still unmet, and the reason has moved.** A warm call's
overhead is now ~12 ms, which is under the 20 ms p50 on its own. What holds the
pooled p50 at 27 ms and the p95 at 952 ms is evaluation: three whole-graph
queries in the fixed set (`dead_export`, `count{calls}` and `depends`, ~930 ms
each) and a seeded `reach_of` at ~67 ms. The next M6 fix is in the engine or in
those rules, not in loading. #18's option C, which skips `Store::open` and the
sweep on an unchanged tree for another ~6 ms, was measured and not taken.

`query_ms` and `cli_ms` did not move. Both run with `--no-refresh` and a fresh
`Warm` per call, so they measure a cold load, by design.

### #24: a constant is not a join (2026-09-13)

The second fix under item 3, taken from the residue #18 named. The three
~930 ms queries turned out to be one cost: `exported/1`, which every query
reaching `ref`'s name fallback pays. `?- exported(S).` alone took 868 ms for
4,866 rows. On tokio its recursion is one level deep, and the time went to a
cross product. `|R| >> k` counts a constant exactly as it counts a join
variable, so in the seeded body `visibility(S, "inherited")` tied
`parent(S, P)` at `11380 >> 1`. Written order broke the tie toward the
product, and 1,788 seeds were paired with 2,126 candidates.

The fix is **connected first** ([03-datalog.md](../specs/03-datalog.md)
§ Evaluation). Once anything is bound, a literal that shares no bound variable
ranks after every literal that does. The planner and the adornment walk both
apply it.

Same corpus and host, from `benches/baseline.json` before and after:

| | before | after |
|---|---:|---:|
| `?- exported(S).`, scratch probe, not in the set | 868 ms | 2 ms |
| query 7, `dead_export`, p50 | 929.6 ms | **44.7 ms** |
| query 8, `count{calls}`, p50 | 932.3 ms | **47.0 ms** |
| query 9, `depends`, p50 | 960.8 ms | **43.0 ms** |
| query 4, seeded `reach_of`, p50 | 79.7 ms | 35.9 ms |
| query p95, pooled | 965.2 ms | 47.2 ms |
| MCP p50 / p95, pooled | 27.4 / 952 ms | 19.7 / 33.5 ms |

**Rows and `derived` are identical for every query, and `tests/counters.rs`
did not move.** That is the finding to keep. `derived` counts derivations,
not rows visited, and reordering a body changes only the latter, so the CI
cost gate cannot see a cross product. Two plan-string tests in
`conformance.rs` pin it instead. Both were mutation-checked: disabling the
rule fails both.

**The MCP done-when reads as met on this run, and is not counted as met.**
The p50 is 19.7 ms against 20, measured at load average ~4 with other
sessions on the host, and a run of the same code an hour earlier read
21.9 ms. The p95 is a third of its budget and stands. `index_cold_ms` was
recorded at 10.4 s against 9.2 s. The index path runs no Datalog, and this
note first called the difference host load. **That was not shown, and it did
not hold up.** #26 re-measured tokio on a quiet host and got 10.8 s, so the move
is real and predates #24. Its cause is not established. TypeScript
(`8001010`) landed between the 9.2 s baseline and this one, which makes it the
first suspect, but that is not a finding. Cold page cache is still unmeasured.

### #26: three corpora, measured on a quiet host (2026-09-13)

fd and deno joined tokio (§ Pulled forward). All three baselines were
re-measured with the machine otherwise idle, at load average 1.3–1.6 from this
session and the OS. Then `tasks/bench.yml` ran end to end against them: 6 steps
ok, every counter equal, and every compared metric within 6% of its baseline.

| from `benches/corpora/<name>/baseline.json` as #26 wrote it | fd ~10k | tokio ~100k | deno ~1M |
|---|---:|---:|---:|
| files / defs (`defs` since recounted to 629 / 11,376 / 84,068) | 24 / 638 | 799 / 11,516 | 4,711 / 88,514 |
| `index_cold_ms` | 373 | 10,818 | 58,243 |
| `load_warm_ms` | 1.5 | 21.7 | 137.6 |
| `reindex_one_ms` | 31.9 | 47.9 | 179.1 |
| query p50 / p95, pooled | 2.8 / 3.5 ms | 32.6 / 44.5 ms | 218.5 / 325.5 ms |
| MCP p50 / p95, pooled | 2.5 / 3.1 ms | 18.7 / 31.7 ms | 202.5 / 322.7 ms |

- **The MCP query done-when is met on tokio.** p50 was 18.7 and 18.8 ms in
  the baseline run and the end-to-end run, and p95 was 31.7 and 31.6 ms. The
  margin on p50 is 1.2 ms, which is thin. #24's 19.7 ms was the loaded
  reading.
- **At ~1M LOC, the MCP process is warm and still costs ~200 ms a call.** A
  warm call reuses the loaded engine (#18), so something in each call scales
  with the tree. On tokio, #18 measured `Store::open` plus the refresh walk at
  ~9 ms. At deno's size that is the first place to look. **Not profiled
  here**, and it is the next item-3 measurement.
- deno's warm load, 137.6 ms, is under the 200 ms that would trigger the
  merged-array cache ([04-storage.md](../specs/04-storage.md) § Loading). The
  done-whens are stated on tokio, so this triggers nothing, but at ~1M LOC it
  is the closest of the thresholds.
- deno's cold index, 58 s, is twice the < 30 s criterion. That criterion is a
  100k-LOC number, and tokio meets it.

### #28: the warm call scales with the walk, not with the store (2026-10-19)

M6 item 3, measured at last. `benches/warm_phases.rs` breaks a warm no-change
call into its phases on a corpus that has not changed since it was indexed,
which is the case the contract puts every call in. p50 / p95 in ms:

> **Two columns of this table are not what they are labelled, and it is left
> standing with the defect named rather than quietly deleted.** Found
> 2026-10-19 while re-baselining.
>
> * The **deno** column was written at 22:08 by a binary that *already had the
>   parallel walk*, so its `walk`, `refresh` and `warm_total` rows are the
>   parallel figures presented as the serial baseline. The serial walk measured
>   on a quiet host is 368.11 / 407.27, not 160.965 / 586.559.
> * The **tokio** column came from a 21:35 run of a harness version whose phase
>   decomposition was wrong — sequential blocks, `open` double-counted — taken
>   at load 18.
>
> The cause is mechanical and is being fixed rather than remembered:
> `target/bench/<name>.json` is one path per corpus that every run overwrites,
> and nothing in it recorded which build or which host conditions produced it.
> Provenance — rev, dirty flag, load average, core count — is being stamped into
> every result, and `UPDATE_BASELINE` refused on a host too loaded to trust.
>
> **The conclusion below survives both defects**, which is why the table stays:
> `walk` dominates `refresh` under either set of deno numbers — 161 of 168 ms
> parallel, 368 of a larger total serial — and the ordering of phases is
> unchanged. The *magnitudes* in the deno and tokio columns are not evidence
> and must not be quoted. Re-measure on a quiet host before this table is cited
> for anything.

| phase | fd (~7k) | tokio (~183k) | deno (~831k) |
|---|---|---|---|
| `locate` | 0.001 / 0.002 | 0.000 / 0.000 | 0.002 / 0.003 |
| `open` | 0.343 / 0.695 | 5.448 / 6.773 | 28.826 / 50.616 |
| `manifest_reread` | 0.027 / 0.051 | 0.291 / 0.587 | 1.890 / 2.815 |
| `manifest_compare` | 0.001 / 0.004 | 0.011 / 0.012 | 0.159 / 0.234 |
| `sweep` | 0.074 / 0.110 | 0.754 / 1.414 | 3.859 / 9.471 |
| **`walk`** | **0.889 / 1.284** | **7.334 / 9.637** | **160.965 / 586.559** |
| `refresh` | 1.034 / 1.742 | 14.268 / 21.757 | 167.589 / 407.595 |
| `warm_total` | 2.102 / 3.384 | 36.531 / 57.894 | 218.539 / 456.704 |
| `mcp_p50` anchor | 2.917 | — | 324.211 |

**It is the walk.** 161 of deno's 168 ms of `refresh` is
`extract::walk` — the gitignore-aware traversal — and `refresh` is 168 of the
219 ms a warm call spends. The per-file stale check that the walk exists to
feed costs 2.8 ms. Evaluation costs 22.1. `Store::open` was the plan's leading
candidate and it is 28.8 ms, real but second; `locate`, `manifest_compare` and
`sweep` are noise at every size.

The scaling is the tree, not the index: deno's walk is 22x tokio's for 4.5x the
LOC, because the walk's cost is directories and ignore-matching, not the files
that survive it. deno's `node_modules`-shaped tree is the worst case for
exactly that.

**No fix is proposed, and this is the honest outcome rather than a deferral.**
Item 3 says fix what the numbers say — and what they say is that the dominant
cost is the one phase the contract requires. Every call refreshes
([05-surface.md](../specs/05-surface.md) § `query`), refreshing means knowing
which files changed, and knowing that means walking the tree. The candidates
that would move it all break something:

| idea | what it costs |
|---|---|
| Cache the walk between calls | A file created since the last call is invisible. That is the confidently-wrong answer the auto-refresh contract exists to prevent. |
| Watch the tree (fsevents/inotify) | A daemon, a new dependency, and a platform matrix — and `docs/plan.md` § Explicitly deferred already holds watch mode behind a different trigger. |
| Walk only the directories the manifest knows | Same hole as caching: a new directory is never looked in. |
| Trust directory mtimes | Not portable, and not true on every filesystem the tool runs on. |

So the finding stands as a measurement and the contract stands unchanged.
**The 20 ms p50 done-when is stated on tokio, where `warm_total` is 36.5 ms in
this harness and the MCP p50 was 18.7 ms in #26** — the difference is that this
bench measures `Warm::run` on a cold-ish process rather than the warm MCP loop,
so it is a profile, not a gate, and it compares nothing. deno gates nothing
either. Re-open this if a corpus in the 100k decade ever shows `walk`
dominating, which is the size the criteria are written against.

Issue #28 stays open, retitled to the contract question rather than the
profiling one: the profiling is done.

**Addendum, 2026-10-20: the bench had been measuring an empty tree.** The
re-baseline this section asks for did not happen, and the reason is worth more
than the numbers would have been.

`index::discard` empties whatever directory it is given. It used to take the
repository root and join `.codeintel` itself; 80e3b59 (#63) changed it to take
the index directory and updated `main.rs`, but not `benches/corpus.rs`. So from
2026-10-17 the corpus bench deleted its own corpus copy on the first of three
cold-index runs, and everything after that measured nothing: fd reported a
20 ms cold index over **0 files and 0 defs**, all nine queries answered zero
rows, and `reindex_one` died looking for a file it had erased. Fixed in
b8c93c9, with a guard — a pinned corpus is never empty, so a cold index that
finds no files now fails loudly instead of reporting plausible zeros.

The committed baselines predate the bug (written 2026-09-13), so they are stale
rather than corrupt. The hazard was the next `UPDATE_BASELINE`: it compares
against nothing, so it would have written measurements of an empty tree over
the only real reference numbers and nothing would have objected. **The load
gate added the day before guards against a busy host, not a broken harness.**

What was established on a verified-quiet host (0.28 load per core, start to
end, recorded in `target/bench/<name>-<rev>.json`):

- **tokio still meets the done-when.** MCP p50 **18.8 ms**, p95 **30.3 ms**,
  against 18.7 / 31.7 in the baseline. The parallel walk did not cost it.
- **`load_warm_ms` on tokio went 21.7 -> 84.5, and the walk cannot explain it**
  — loading does not walk. It is a per-file-open cost in the environment:
  +63 ms over 799 segments is ~79 us each. That figure predicted deno's
  `load_warm` at ~510 ms before it was run; deno measured **493.6**, within 3%.
  Corporate endpoint security hooks every file operation on this machine, and
  `find` over the settled deno tree takes 0.55 s for ~14.8k files, roughly 10x
  what the hardware should do.
- **deno could not be measured.** Its run began at 0.39 load per core and ended
  at 1.99 — Defender and an unrelated TypeScript build woke up inside it — so
  every deno figure from 2026-10-20 is across a ramp and is not evidence. Its
  `mcp_p50` read 344.5 and its p95 1104.1; **do not quote those.**

**No baseline was refreshed, deliberately.** The counter deltas are all
explained and all intentional — `defs` moved 638 -> 629 on fd because 225afa5
keys the counter on the symbol rather than the declaration (the `def` relation
itself diffs identical, 629 rows either way); query 8's `derived` moved because
c6d807b and 71dde10 require an enclosing def before a reference is a call, so
fd's `calls` went 495 -> 491 with query 8's answers unchanged; and its status
moved `truncated` -> `no-scip` because 95a3f51 stopped a cap hiding a
condition. Those belong in a refreshed baseline. The wall clock does not: it
carries an environmental tax this machine applies and a clean machine would
not, and committing it would set the +20% gate above the real cost, where it
passes everything. `UPDATE_BASELINE` writes counters and clock together, so it
is both or neither — **which is the argument for splitting them.**

Two defects in the harness this turned up, both now fixed:

- The load gate read conditions only at the **start**. deno started quiet and
  ended loaded and was still marked trustworthy. A run is now committable only
  if it also ends quiet, judged against twice the start limit so the bench's
  own four-thread walk does not trip it.
- `host()` identifies a machine by `available_parallelism()`, which reads 10
  today and 12 when the baselines were written, so **wall clock was not
  compared at all** on any corpus this run — tokio's 4x `load_warm` regression
  passed in silence. The counter check caught every real problem today. That is
  the argument for counters being the CI gate and the clock staying local, and
  against `available_parallelism` as machine identity. Not fixed here.

**Correction, 2026-10-20, later: the baselines are from different hardware.**
The entry above reads the wall-clock gap between the committed baselines and
today as an environmental tax. That was half right and the other half was an
assumption nobody had checked.

`host()` identified a machine by `available_parallelism()`, which answers for
the *process*. Replacing it with the OS's own count (#83) and asking the
machine directly gives **Mac17,2, Apple M5, 10 logical CPUs**. The committed
baselines record **12**. `available_parallelism` can never exceed the hardware,
so the machine that wrote them had at least twelve: those baselines were
measured on **different hardware**, and no amount of quiet would have made
today's numbers comparable to them.

So the host check was right to refuse the comparison all along. Its only fault
was doing it in silence, which is what #83 fixes: an uncomparable host now
names the field that differs and returns the inconclusive verdict rather than
passing.

What survives of the tax finding, and what does not:

- **Survives.** The ~79 us per file open is self-consistent *on this machine*:
  derived from tokio's 799 segments, it predicted deno's `load_warm` at ~510 ms
  before deno ran, against 493.6 measured. `find` over the settled deno tree
  takes 0.55 s for ~14.8k files. Both are measurements of this host against
  itself and neither depends on the baseline.
- **Does not.** Attributing tokio's `load_warm` 21.7 -> 84.5 ms *to the tax*
  compared two different machines and cannot carry that weight. An M5 should
  beat a twelve-core predecessor at the single-threaded work `load_warm` does,
  which points the same way, but "points the same way" is not a measurement.
  The honest statement is that the two numbers are not comparable at all.

This also re-frames the refresh. Updating these baselines is not freshening
stale numbers for the same machine; it is measuring **this** machine for the
first time. That makes refreshing more clearly correct once the host stays
quiet long enough, and it makes the old numbers worth keeping in history rather
than treating as a target this hardware failed to hit.

**Addendum, same day: there was a fix inside the contract after all.** The
table above says the walk dominates; it does not say the walk was
single-threaded, and it was. `ignore` ships a parallel walker, and using it
changes nothing about what the walk decides — same policy, same matchers, same
outputs — so none of the four rejected ideas above is involved. Measured over 31
runs per corpus, p50 / p95 in ms, walk alone:

| corpus | serial | parallel (4 threads) |
|---|---|---|
| fd (~7k) | 0.90 / 1.20 | 1.51 / 4.10 |
| tokio (~183k) | 7.16 / 15.45 | 4.80 / 6.86 |
| deno (~831k) | 389.58 / 1117.24 | 165.36 / 269.22 |

deno halves and its tail improves 4x; tokio's p95 improves 2.3x. **fd regresses
by 0.6 ms p50**, which is thread-pool startup on a tree of 24 files and is
stated rather than buried — it is 0.6 ms against a 20 ms p50 criterion, and the
tail it buys back at the other two decades is worth it. Thread count is four,
not one per core: the walk is bound by directory reads, and 2, 3, 6 and 8 were
all worse than 4 on these corpora.

**These numbers were measured on a host that was not quiet** — two dozen agent
sessions were live, several busy — which is exactly what this repo's own rule
says not to cite. They are reported because the effect is far larger than the
noise and the direction is consistent across every corpus and thread count, but
the authority is `provision apply tasks/bench.yml` on a quiet host, and the
baselines there should be re-measured before any of these figures is quoted as
settled.
