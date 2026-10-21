# Cookbook

Every question below is Datalog over the relations in
[01-facts.md](../specs/01-facts.md). None of them is a CLI verb, an MCP tool, or
a function in this binary — that is the whole design, and this file is the
evidence for it.

Read [`codeintel schema`](../specs/05-surface.md) first; it is generated from
your index and tells you which values actually occur in it.

---

## 1. Conformance — start here

**This section needs no SCIP index.** `import/3` comes from
`queries/<lang>/imports.scm`, which is ours, so it works on the first run after
`codeintel index .` with no language indexer installed. Everything in §3 that
touches `calls` or `ref` does not.

The shape is always the same: **write the rule that describes the violation,
then assert that it finds nothing.**

```dl
%% ui_imports_db(F, M)  no module under ui/ may import from db/
ui_imports_db(F, M) :- import(F, M, _), prefix(F, "src/ui/"), contains(M, "db").
```

```sh
codeintel query '?- ui_imports_db(F, M).' --rules conformance.dl --expect-empty
```

`--expect-empty` exits **1** when a row comes back and 0 when none does. It is 1
and not 2 because 2 means the query never ran: "your code violates this" and "I
could not tell you" are different results in CI.

It is also 2 on an index written by another schema, where the query never ran
(`status=stale`). A repository with no SCIP index is a different case: see #134.

### Layering

One rule per forbidden direction. Naming each one separately is what makes the
CI failure legible.

```dl
%% layer_violation(F, M)  the whole layering policy, one clause per edge
layer_violation(F, M) :- import(F, M, _), prefix(F, "src/ui/"),    contains(M, "db").
layer_violation(F, M) :- import(F, M, _), prefix(F, "src/db/"),    contains(M, "ui").
layer_violation(F, M) :- import(F, M, _), prefix(F, "src/domain/"), contains(M, "infra").
```

### A banned dependency

```dl
%% banned(F, M)  nothing may reach for the old client
banned(F, M) :- import(F, M, _), contains(M, "legacy_http").
```

### An allowed-direction rule (the inverse shape)

Sometimes the policy is a whitelist. State it as "an import that is not one of
the allowed kinds".

```dl
%% allowed(F, M)  what src/core/ is permitted to import
allowed(F, M) :- import(F, M, _), prefix(M, "std").
allowed(F, M) :- import(F, M, _), prefix(M, "crate::core").

%% core_reaches_out(F, M)  src/core/ importing anything else
core_reaches_out(F, M) :- import(F, M, _), prefix(F, "src/core/"), !allowed(F, M).
```

### Layer-crossing at file granularity, once `calls` is available

```dl
%% crosses(F, G)  a file depending on one it should not
crosses(F, G) :- depends(F, G), prefix(F, "src/ui/"), prefix(G, "src/db/").
```

`depends` reads `ref`, so it degrades with provenance — see §4.

---

## 2. Orientation

You have a location and need a symbol. This is how anything gets *into* the
graph.

```dl
?- innermost_at("src/storex.rs", 23, S).          % from a stack trace, a diff
?- def(S, F, _, N), contains(N, "auth").         % from a word
?- about(S, Rel, A, B, L).                       % everything about S, one trip
```

`about` is ten questions in one relation, discriminated by `Rel`:
`sig doc defined parent caller callee implements implementor test extern`.

The impact of a change, excluding tests:

```dl
?- innermost_at("src/store.rs", 23, S), impact_of(S, C),
   def(C, F, _, N), !is_test(F).
```

**Seed `impact_of` and `reach_of` with a constant.** Unseeded they compute
all-pairs reachability and hit the budget. `reaches/2` and `recursive/1` are the
deliberately unseeded pair; use them only on a small tree.

**Every traversal has an `_exact` form.** `reach_of_exact`, `reaches_exact`,
`recursive_exact` alongside `impact_of_exact`. A closure compounds what name
matching invents — one invented edge anywhere on a path invents the whole path,
and that is how `?- recursive(S).` comes back with `__init__` and `run` on a
repo where neither is recursive. Ask the `_exact` form of any question whose
answer you intend to act on.

---

## 3. The dex 34, as rules

`~/projects/dex`'s `internal/store/store_graph.go` exposes **34 methods**.
[research.md](research.md) §1a names 17 of them and elides the rest; the counts
below were taken from the file itself, because a claim about 34 methods that
only ever shows 17 is not checkable.

**Of the 34: 17 are not questions at all, 13 are Datalog rules, 4 are refused.**

The 17 that are not questions are the write path (`GraphUpsertNodes`,
`GraphUpsertEdges`, `GraphPruneUnseen`, `SetNodeVecs`, `GraphSetCentrality`),
epoch and maintenance bookkeeping (`GraphMaxEpoch`, `GraphSeenTime`,
`GraphStats`, `GraphScale`), bulk dump (`GraphAllNodes`, `GraphAllEdges`),
embeddings (`NodeKNN`, `NodeVecCount`, `NodesNeedingEmbed`), chunk retrieval
(`ChunksByPaths`) and edit spans (`SymbolEditSpanByID`,
`SymbolEditSpansByName`). Here the write path is `codeintel index` plus the
manifest, the bookkeeping is `codeintel status`, and embeddings and chunk
retrieval are a different product — `NodeKNN` takes a query vector.
**Out of scope is a third category, distinct from refused.**

Several of those are one-liners here anyway, which is worth saying because it
makes the table shorter rather than longer: `SymbolEditSpansByName` is
`?- def(S, _, _, "get"), def_span(S, L1, L2, B1, B2).`, `GraphAllNodes` is
`?- def(S, F, K, N).`, and `GraphStats` is `codeintel status`.

The 17 that *are* questions:

| dex method | here |
|---|---|
| `SymbolsByFile` | `?- file_of(S, "src/store.rs"), def(S, _, K, N).` |
| `SelectSymbols` | `?- def(S, F, K, N), contains(N, "auth").` |
| `ExportedSymbolsByDir` | `?- exported(S), def(S, F, _, N), prefix(F, "src/api/").` |
| `GraphQualifiedNameAt` | `?- innermost_at("src/store.rs", 23, S), within(S, A), def(A, _, _, N).` |
| `ImportsForFile` | `?- import("src/store.rs", M, Alias).` |
| `ImportsForDir` | `?- import(F, M, _), prefix(F, "src/db/").` |
| `InternalPackageImports` | `?- import(F, M, _), prefix(M, "crate::").` |
| `ExternalImports` | `?- uses_package(F, P).` — indexer's package names, § 4 |
| `UsedByPackages` | `?- import(F, M, _), prefix(M, "tokio").` — § 4 |
| `MainEntrypoints` | `?- entrypoint(S), def(S, F, _, N).` |
| `CallerFiles` | `?- def(S, _, _, "get"), calls(C, S), def(C, F, _, _).` |
| `UnresolvedInboundForFile` | `?- name_ref(N, "src/store.rs", L, _, _), !def(_, _, _, N).` |
| `Smells.dead_exports` | `?- dead_export(S), def(S, F, _, N).` |
| `Smells.long_functions` | `?- def_lines(S, Lines), Lines > 60, def(S, F, _, N).` |
| `Smells.undocumented` | `?- undocumented_export(S), def(S, F, _, N).` |
| `TopCentralByDir` | **refused** — see below |
| `PackageCentrality` | **refused** |
| `FileCentrality` | **refused** |
| `GraphCommunities` | **refused** |
| `Smells` *as a ranked list* | **refused** as a ranking; the three above are the facts under it |

Twenty rows, thirteen distinct dex methods — `Smells` is one method that appears
four times, once per thing it bundles. Every answered row is one to five lines
and the longest is three literals. Variation by directory, by package or by file
is a `prefix/2` on a column, not a new method, which is why dex's
`ImportsForDir` and `ImportsForFile` are one row each here rather than two
implementations.

### What is refused, and why

**No centrality, no communities, no importance score, no smell ranking.** This
is invariant 4 in [00-overview.md](../specs/00-overview.md), and it is not
squeamishness: [research.md](research.md) §1c measured what such a score does
over a mixed-provenance graph. A `name`-provenance edge set is missing most
method calls, so "centrality" computed over it ranks the code whose *identifiers
happen to be unique*, not the code that matters. The number would look
authoritative and be wrong, and nothing in the output would say so.

What you can have instead is the fact under the question. `entrypoint/1` and
`dead_export/1` are the two that people usually want centrality *for*, and both
are exact statements about the graph rather than positions in a ranking.

---

## 4. Provenance — read this before trusting §3

Every rule that reads `calls` or `ref` is contaminated by resolution quality,
which is why each ships in two forms.

**Where SCIP covers the code, the two forms agree.** On all four committed
fixtures — `tests/fixtures/{rust,go,python,typescript}/`, each with its
indexer's `index.scip` — `calls` and `calls_exact` return the same edges, and
every row of `?- calls_at(From, S, F, L, P).` reads `P = "exact"`. That is by
construction rather than by luck: the `name` rules carry `!scip_claim(...)`, so
tier A cannot invent an edge at a position tier B already answered
([02-extraction.md](../specs/02-extraction.md) § Validation 4). A fully covered
tree is the case where the distinction costs you nothing.

**Where SCIP does not reach, only the contaminated form has rows.** This
repository carries no SCIP index, so:

```
?- calls(C, S).          every edge, all of it provenance `name`
?- calls_exact(C, S).    0 rows, and status: no-scip
```

That zero is not silent. The response carries `status: no-scip` and the exact
indexer command for every language in the tree, so "nothing here can answer
you" never reads as "the answer is none".

What tier A invents is visible one rule further on. `?- recursive(S).` on this
repository reports `get` and `settle`, among others, and neither recurses:

- `Db::get` (`crates/datalog/src/db.rs:25`) has the body `self.rels.get(name)`,
  a `BTreeMap::get`. Tier A matched the identifier `get` to the enclosing
  definition of that name and invented a self-call.
- `Segment::settle` (`crates/facts/src/segment.rs:87`) calls `rows.settle()`,
  which is `Relation::settle`. Same shape.

One invented edge becomes a whole invented cycle, which is why §2 says to ask
the `_exact` form of anything you intend to act on. On this repository with a
SCIP index present, M3 measured tier A over-reporting **252 call edges, 17% of
what it claims** ([plan.md](plan.md) M3); that figure is the entire argument
for tier B and for keeping the two provenances apart rather than averaging
them.

- Use `calls_exact`, `impact_of_exact`, `dead_export_exact`, `entrypoint_exact`,
  `depends_exact` and `uses_package_exact` when precision matters.
- Check `codeintel status` for whether a SCIP index is present and fresh. A
  query whose dependency closure reaches a SCIP-only relation returns
  `status: no-scip` when there is none — it never silently returns zero rows.
- `dead_export` **over**-reports on `name` provenance: an unresolved call is an
  invisible call, so a symbol can look unreferenced when it is not.
- A `dead_export` row means the *export* is unnecessary, and for an `inherited`
  definition (a Rust enum variant, trait item or trait-impl method; a TypeScript
  interface or enum member) there is no slot to narrow, so the row has no edit
  (#114). On this repository 35 of 98 `dead_export_exact` rows are those. The
  filter is `!visibility(S, "inherited")`; `visibility(S, "public")` is wrong,
  it also drops a TypeScript `export { x }`, which is narrowable:
  `?- dead_export_exact(S), def(S, F, _, N), !is_test(F), !visibility(S, "inherited").`
- `entrypoint` **inverts**: a callable looks like an entrypoint precisely when
  its callers were not resolved.
- `entrypoint_exact` is a **candidate list, not a dead-code list**, and `_exact`
  does not fix that (#111). It never consults `scip_impl`/`implements`, so a
  trait or interface method called through the trait lists its implementations
  as uncalled; `is_test` is path-only, so Rust's in-file `#[cfg(test)]`
  functions list; a definition the anchor join did not reach (`codeintel status`
  prints `anchored: n/m`) is in neither answer. Read every row before acting on
  it. The one filter the schema supports for the first class is free functions
  only, which a trait cannot dispatch to:
  `?- entrypoint_exact(S), parent(S, P), def(P, _, "module", _), def(S, F, _, N).`
- `impact_of*`, `reach_of*`, `reaches*`, `recursive*` and `about`'s `caller`,
  `callee` and `test` are **blind to trait dispatch**, and answer `ok` (#116). A
  call through a trait names the trait's method, so the implementing method has
  no callers: on this repository `impact_of_exact` of `Interning::intern` is 0
  rows and of the trait's `Symbols::intern` 233. `_exact` does not fix it (no
  `calls` closure reads `scip_impl` or `implements`). Before concluding that
  nothing depends on a method, ask what trait it implements and seed that method
  too: `?- def(T, _, "trait", _), parent(M, T), def(M, _, "method", "intern"), impact_of_exact(M, C).`
  A `reach_of` from a trait method is empty because it has no body; seed the
  impl. `recursive*` cannot see a cycle through a trait: a floor, never wrong.

### Rust's in-file `#[cfg(test)]` modules: a naming guess, stated as one

`is_test(F)` is a path rule, which is right for Go (`_test.go`), Python
(`test_*.py`) and TypeScript (`*.test.ts`) and wrong for Rust, whose unit tests
live in the source file under `#[cfg(test)] mod tests`. Every `#[test]` function
in one is a callable nothing calls, so `entrypoint_exact` lists it (#111). The
rule below excludes them. **It guesses the module's name, and says so**:

```
in_tests(S) :- within(S, A), def(A, _, "module", "tests").
in_tests(S) :- within(S, A), def(A, _, "module", "test").

?- entrypoint_exact(S), def(S, F, _, N), !is_test(F), !in_tests(S).
```

**The guess is wrong about 1 module in 13.** Across the 2,748 `#[cfg(test)]`
modules in the 26,386 `.rs` files of `~/.cargo/registry/src` (a literal text
scan, 2026-10-21), 70% are named `tests`, 22% `test` and 7.5% something else
(`conformance`, `spec_tests`, `vocabulary`). A module outside the two names is
not excluded, so the composed answer is **narrower than the truth, in the
direction that lists test code as dead**, the direction that gets code deleted.
It is a filter, not a proof: read the rows.

**`about(S, "test", ...)` has the same hole, and is a floor** (#121). It joins
`is_test(F)`, so a call from an in-file `mod tests` is not a test row, and the
answer is `ok`. Measured on this repository (2026-10-21, fresh SCIP, anchored
2330/2824): 154 symbols are called from a `tests`/`test` module outside a test
file; `about` names a test for 30 and none for **124**. An empty `test` row is
not "uncovered" in Rust. Excluded: any call from an in-file test module; calls
through a trait (#116); a module outside the two names (the 154 is already
that guess, so the gap is at least that). Widen it with the same guess, stated
as one:

```
?- innermost_at("src/store.rs", 23, S), calls(C, S), in_tests(C), def(C, F, _, N).
```

Rows are callers named by their module's name, not by an attribute. No
percentage is offered: a count of tests is not coverage.

Why the guess is the only thing available: `#[cfg(test)]` leaves nothing in
any fact. `attribute_kinds` puts the attribute in `def_span`'s byte range and
out of `def_sig`, and a rule cannot read source (invariant 3b). Nothing in the
index distinguishes that module from any other, so no rule can ask for it.

Why this is not a stdlib rule: a shipped `test_scope` would assert "this is test
code", and an `ok` answer would then read as exact where it is a naming
convention. That is the failure #44 deleted a vocabulary for — a thing that
advertises more certainty than it has. And a fact that is exact needs a base
relation (16 of 16 are spent) for a mechanism only Rust has, which fails the
`Export` bar's third item in `CLAUDE.md` (usable by a language other than the
one that motivated it): Go, Python and TypeScript all mark test scope by path.

### `uses_package` names the indexer's package, not the import's

`uses_package(F, P)`'s `P` is `extern`'s `Pkg` — the package the *indexer*
attributed the symbol to. That is not always the package the source imports,
and the gap is not a provenance tier, so no `_exact` form closes it.

On TypeScript it bites hardest. `import merge from 'lodash.merge'` gives:

```
?- import(F, "lodash.merge", A).        the row, with A = "" (a default import binds a member, not the module)
?- uses_package(F, "lodash.merge").     0 rows
?- uses_package(F, "@types/lodash.merge").  the row
```

scip-typescript resolves the symbol to the `.d.ts` inside `@types/lodash.merge`,
so `extern` reports the DefinitelyTyped stub and never the runtime name. Most of
a typical TypeScript repository is typed that way — `react`, `react-dom`,
`lodash*` — so the packages an agent is most likely to ask about are exactly the
ones that answer zero (#58). It is not DefinitelyTyped-specific either: on
`tests/fixtures/typescript/`, which depends on nothing, every `extern` row reads
`npm typescript 5.9.3`, because `Map` and `String` come from `lib.es5.d.ts`.

**So pick by the question, not by the word "package":**

| question | relation |
|---|---|
| does this file declare a dependency on `react`? | `?- import(F, "react", _).` |
| …in Rust, where the specifier is a path? | `?- import(F, M, _), prefix(M, "tokio").` |
| which dependency did this *symbol* come from? | `?- extern(S, _, P, V).` |
| which files touch any third-party symbol? | `?- uses_package(F, P).` |

`uses_package` is truthful about what it states; it is the wrong question for
"does F use X", and a zero there means "no `extern` row says so", never "no".
There is no union of the two relations, because `import`'s `Module` is a
per-language specifier and not a package name: `"std::collections::HashMap"` in
Rust, `"fmt"` in Go, `"../db/conn"` in TypeScript. Rewriting either name
into the other is the extractor guess
[02-extraction.md](../specs/02-extraction.md) § Name matching forbids.

### Two artifacts this fixture shows, and what they mean

`?- depends(F, G).` on `tests/fixtures/rust/` reports `src/lib.rs` depending on
`src/app.rs`, `src/kinds.rs`, `src/store.rs` and every other module file. No
code in `lib.rs` uses any of them: the file is six `pub mod` lines under a
doc comment and nothing else. Each `mod` statement is a reference to a module symbol SCIP places
in the module's *own* file rather than where the statement is
([plan.md](plan.md) M3 records the same effect on the anchor rate), and
`depends` is file-granular, so it inherits that placement. Read a `depends` row
involving a module definition as "these two files are in one module graph", not
as "this file uses that file".

The same placement is why the nine `mod` definitions are the nine unanchored
defs on this fixture — tier A puts them at the `mod x;` line and SCIP puts them
at line 1 of the other file, so the join cannot match them. They are exempt by
count, not by allowlist
([02-extraction.md](../specs/02-extraction.md) § Validation 3).

`?- ambiguous(N).` lists `self`, `key` and `store` alongside real collisions.
SCIP emits a `local` definition for every parameter and binding, the collapse
table has no word for `Parameter`, and they land in `def` as `unknown`. Filter
with `def(S, _, K, _), K != "unknown"` when that matters.

---

## 5. Writing your own

The rule is the unit of extension. If you are about to ask for a feature, write
the rule first — it is usually shorter than the feature request.

```dl
%% my_question(A, B)  what this is for, one line
my_question(A, B) :- ...
```

The `%%` line is not decoration: `codeintel schema` generates its rule list from
it, and a test asserts every rule has one and that the signature matches the
clause. A rule without a `%%` line is a rule an agent will never find.

Load it with `--rules`. **Clauses are additive** — a file defining `is_test`
*widens* it, because a predicate is the union of its clauses. To *replace* a
stdlib rule, put your version in the query program itself; that shadows the
loaded one, and the shadowing is reported rather than silent.
