# status / hint / degradation path audit, 2026-10-21

Invariant 5, read literally: truncation and degradation are reported; never a
silent cap, never a silent empty; an `ok` with zero rows and a `no-index` must be
distinguishable without reading prose. Asked of every status, hint, error and
degraded-index path the CLI and MCP tool can produce. Run on `580c613`, release
build, against scratch trees built for each path (not this repository: every
path below was **triggered**, not read from source). Thirteen issues, #130-#142
(#133 and #141 each carry several doors of one shape).

Two questions per row. **J**: could a caller tell this from success with the
JSON alone? **N**: does the message name the next step? Classes: **HONEST**,
**SILENT** (degradation not reported: an invariant-5 violation), **MISLEADING**
(something is said and it points the wrong way; #124 is the worked example).

| path | trigger | J | N | class | issue |
|---|---|---|---|---|---|
| `no-index`, own root | query a tree never indexed | yes: `status`, exit 2 | yes: `run: cd <root> && codeintel index . && ...` | HONEST | |
| `no-index`, siblings | parent of two indexed repos | yes | yes, names both | HONEST | |
| nearest index adopted | query from a subdirectory | yes: `root` set | yes | HONEST | |
| `no-index`, missing directory | `query --path /nonexistent` | status yes | `cd` into a directory that does not exist; `status`/`schema` say "does not exist" | MISLEADING | #141 |
| `stale`, lock held | another process holds the writer lock | yes | yes | HONEST | |
| `stale`, refresh deadline | 3,000 files all edited, 2 s budget | yes; hint names count and first five | yes | HONEST | |
| `stale`, schema mismatch | `schema_version` rewritten to 2 | status yes, but = the answered kind | yes | MISLEADING (exit 0, no evaluation, `schema` says `ok`) | #135 |
| `corrupt`, segment | truncate a segment to 0 bytes | yes, exit 2 | yes: `index . --rebuild` | HONEST | |
| `corrupt`, manifest | `{` in manifest.json | yes, exit 2 | yes | HONEST | |
| I/O failure | `chmod 000 manifest.json` | no JSON at all (stderr text, exit 2); documented | no | HONEST (by spec) | |
| `locked`, `index` | second writer | stderr only, exit 2 (`index` has no JSON) | yes: pid | HONEST | |
| `no-scip`, none ingested | Rust tree, no SCIP | yes | yes: `run: rust-analyzer scip .` | HONEST | |
| `no-scip`, `!is_test(F)` | same tree | yes | the repair changes nothing | MISLEADING | #139 |
| `no-scip`, 0 of N matched | SCIP built from `a/`, ingested at root | yes | names `codeintel index`, not the root the spec promises | MISLEADING | #142 |
| `scip-stale` | edit a covered file after the SCIP | yes | yes | HONEST | |
| partial SCIP coverage | SCIP covers `a/`, not `b/` | **no**: `ok`, empty | n/a | SILENT | #132 |
| SCIP collisions / ambiguous / skipped docs | index-time only | **no** at query | n/a | SILENT | #132 |
| `--scip <missing>` | `index . --scip nope.scip` | **no** | prints the "you passed nothing" line | SILENT | #133 |
| 0-byte `.scip` | `: > empty.scip` | **no**: `ok` | n/a | SILENT | #133 |
| garbage `.scip` | `echo garbage > bad.scip` | exit 2, named | yes | HONEST | |
| skipped: too large | 2.7 MB `.rs` | **no** (`index` only) | pointer to `status`, which is silent too | SILENT | #130 |
| skipped: not UTF-8 / unreadable | binary, `chmod 000` | **no** at query; `status` `stale` forever | repair never clears it | SILENT + MISLEADING | #130 |
| skipped: unsupported language | `.scala`, `.zig` | `status` only | yes | HONEST (status) | |
| zero files indexed | `.scala`/`.md` only, or empty repo | **no**: `ok` | hint contradicts itself | SILENT | #140 |
| syntax error in a source file | `func B( {` before `func C() {}` | **no**: `C` is missing, `ok` | n/a | SILENT | #131 |
| `truncated`, rows | `--limit 3` | yes: `truncated`, `cap`, status | yes | HONEST | |
| `truncated`, bytes | 90,000-row cross product | yes | yes | HONEST | |
| `truncated` + degraded index | `--limit 3` on `impact_of`, no SCIP | yes: status keeps `no-scip`, `cap` set | yes, composed | HONEST | |
| exact-limit boundary | `--limit 300` on 300 rows | `ok`, not truncated (correct) | | HONEST | |
| `--limit 0` | | `truncated`, 0 rows | yes | HONEST | |
| hint over the byte budget | 300 KB constant in the query | status kept; hint ends `[hint cut ...]` | n/a | HONEST | |
| `invalid-query` family | 11 shapes (empty, no `?-`, unknown relation, arity, unsafe negation, bad regex, unterminated string, `+` on string, bad rules file, missing rules file) | yes | yes, names rule and variable | HONEST | |
| `unstratified` | `p :- !q. q :- !p.` | yes | names the cycle | HONEST | |
| `budget-exceeded` | 4-way `def` cross product | yes | seeding advice for a query with no recursion | MISLEADING | #138 |
| `timeout` | `reaches` on a 2,500-node cycle | yes | "raise the limit": none exists | MISLEADING | #138 |
| `ok`, empty, typed constant | wrong Kind / path / int-as-string | `ok`, hint names column and counts | yes | HONEST | |
| `ok`, empty, negated literal | `!def(S,_,_,_)` | `ok` | reads correctly (#124 landed) | HONEST | |
| `--rules` widened / shadowed | `is_test` in a rules file / the program | yes: `stats.widened` / `shadowed`, stderr | yes | HONEST | |
| `--no-refresh` over a changed tree | edit after index | **no**: `ok`, `refreshed: 0` | n/a | SILENT by contract (the caller opted in); not filed | |
| `--expect-empty` on a degraded answer | `no-scip`, `scip-stale`, stale-schema | exit 0 | n/a | SILENT in the exit code | #134 |
| `index <file>`, `--lang klingon` | | exit 0 | none | SILENT | #141 |
| pinned index, other `--path` | `CODEINTEL_INDEX_DIR` + `--path emptydir` | `ok`; `refreshed: 0` for an index it emptied | | SILENT (documented hazard, spec 04 § Location) | #141 |
| MCP malformed line / batch / unknown method / unknown arg | | JSON-RPC error, session continues | yes | HONEST | |
| MCP wrong-typed `limit` / `raw` | `"limit":"2"`, `"raw":"true"` | **no**: coerced to default | `query: 5` gets the missing-argument error | SILENT + MISLEADING | #136 |
| MCP `schema`, text wire | no index | **no** status field; prose only | yes, in prose | SILENT | #137 |
| `schema` / `status`, no index | | `no-index`, exit 0 (documented) | yes | HONEST | |

Not triggered: a read-only checkout's lock failure (the code path is
`query.rs:1378`; running as the owner cannot make `Lock::open` fail), and a
`scip-python`/`scip-typescript` index (not installed on this host, #127 § 4).
`index` over an index whose SCIP input was dropped by a plain `index .` prints
`no SCIP index: tier A only`; it does not say the previous index had one.
