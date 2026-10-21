# stdlib row audit, 2026-10-21

The question #111 and #114 each answered for one rule, asked of every head in
`rules/stdlib.dl`: is a row actionable, and can the rule see what its gloss says
it sees. Run on this repository at 36ea917 after `rust-analyzer scip .` and
`codeintel index .`:

```
scip coverage: 89 of 89 document(s) matched an indexed file
anchored: 2302/2796 tier-A defs (82.3%)       scip collisions: 67 symbol(s) not adopted
```

Rust only. The other three languages were not run against a fresh indexer.

Classes: **A** actionable; **U** unactionable by construction (#114: the row is
right and no edit answers it); **B** blind (#111: the rule cannot see what its
gloss implies). `stats.depends` is a dependency *closure*, so it proves what a
rule cannot consult, never what it uses (#117: `uses_package` lists `name_ref`
and can never use it).

| rule | class | evidence (rows, `status ok` unless noted) | issue |
|---|---|---|---|
| `exported` | B | 940 rows; `pub` in a private module counts (`tests/fixtures/rust/src/store.rs:44`; probe crate for the `mod x;` form) | #115 |
| `undocumented_export` | B | 41 non-test rows, 41 are `pub mod x;` declarations whose file carries `//!` | #115 |
| `dead_export` | B | name form: 41 non-test rows with no anchor, all module declarations | #115 |
| `dead_export_exact` | U | 98 rows, 35 `inherited`, 63 `public` | #114 |
| `entrypoint_exact` | B, U | 288 non-test rows; none of the 5 `main` fns list (no `resolved`) | #111 |
| `entrypoint` | B | 147 name-only rows; trait impls, in-file tests | #111 |
| `calls`, `calls_exact`, `calls_at` | B | trait dispatch: `Interning::intern` has an outgoing edge and no incoming | #116 |
| `impact_of_exact`, `reach_of_exact`, `impact_of`, `reach_of` | B | 0 vs 233 rows for impl method vs trait method | #116 |
| `reaches`, `reaches_exact`, `recursive`, `recursive_exact` | A, floor | `recursive_exact` 26 rows, all direct recursion (read in source); `recursive` 51 | #116 |
| `about` | B | `caller` 0 rows for the impl above; `test` 124 of 154 in-file-covered symbols empty | #116, #121 |
| `is_test` | B | path-only; clause 5 (`Role = "test"`) 0 rows | #121 |
| `implements` | B | 7 rows of 42 impl blocks; 35 external traits; 0 method rows | #120 |
| `uses_package`, `uses_package_exact`, `tier("uses_package", ...)` | B | identical, 399 = 399; `tier(..., "name")` 0 rows | #117 |
| `uses_package*` rows | B | 251 std, 58 first-party, 90 third-party | #118 |
| `ref` | B | `Role = "read"` on 1319 name rows, schema says `read 0` | #119 |
| `tier` (other relations) | A | `depends` name 6, `calls` name 714, `dead_export` name 224, `entrypoint` name 147 | |
| `depends`, `depends_exact` | A | 392 / 386 | |
| `ref_outside`, `ref_outside_exact` | A | 767 = 767: the name tier adds nothing on a repo with fresh SCIP | |
| `symbol_at`, `innermost_at`, `tighter_at` | A | 0 non-callable innermost over every line of every function in `query.rs`; 12 tie rows | |
| `def_lines` | A | 282 one-line defs report 1, none report 0 | |
| `file_of`, `at`, `within`, `callable`, `local_def`, `named`, `ambiguous`, `scip_claim`, `trait_sym`, `impl_sym` | A, helpers | navigation; `ambiguous` 829 of 2524 names, so name resolution is blocked for a third of them | |

Checked and clean: `Role`'s zero counts are in `schema`'s VALUES line, so the
#44 resolution holds there; the complaint in #119 is that `ref` disagrees with it.

## Not reached

- Go, Python and TypeScript with fresh SCIP. Every row above is Rust.
- `impl_sym`'s two "unique exported def repo-wide" clauses were not isolated.
- `exported` on `mod x;` has no instance in this repository outside fixtures;
  it was shown on a five-file probe crate, which proves the rule's reading and
  not how often it bites.
- 1,460 `scip_ref` rows (446 outside test paths) target a symbol with neither
  `def` nor `extern` and are dropped from `ref`; that is #1 and #56, not re-filed.
