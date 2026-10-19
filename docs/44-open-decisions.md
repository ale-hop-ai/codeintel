# #44 parts 2 and 3 — evidence and recommendation

Working memo for the two decisions left open on
[#44](https://github.com/alehatsman/codeintel/issues/44) after part 1 landed
(d02f143). **Investigation only — no behaviour changed by the branch that
carries this file.** Delete it once both are decided; the decisions belong in
the specs, not in `docs/`.

Line citations are against `2addffd`.

---

## Part 2 — `scip_ref`'s `def` role

### What the code does

`crates/extract/src/scip.rs:341` branches on `Definition`: a definition
occurrence goes to `doc.defs`, everything else to `doc.refs` with
`role_of(...)`. So the `(SymbolRole::Definition, "def")` row in `ROLES`
(`scip.rs:85`) is unreachable, `?- scip_ref(S, F, L, C, _, "def").` is `ok`
with zero rows, and `specs/01-facts.md:379` — "Definition sites appear here too
with `Role = "def"`" — is wrong.

### Who would consume them: nobody, today

Every `scip_ref` consumer in the repo:

| site | uses the role? |
|---|---|
| `scip_claim` — `rules/stdlib.dl:131-132` | no, joins `def`/`extern` |
| `ref` — `stdlib.dl:140-143` | passes it through, does not read it |
| `calls_at` — `stdlib.dl:211-214` | `Role != "def"` |
| `is_test` — `stdlib.dl:295` | `"test"` only |

`docs/cookbook.md` does not mention `scip_ref`. Neither does any file in
`queries/`. There is no consumer waiting on this.

### They would duplicate what `def`/`def_name`/`at` already answer

A def-role row would be `(S, F, Line, Col, From, "def")` at the identifier
token. That is `def(S, F, _, _)` joined with `def_name(S, Line, Col)`
(`01-facts.md:188` — "position of the identifier token"), and `at(S, F, L)`
(`stdlib.dl:277`) answers the file-and-line form directly. The only new column
is `From`, and at a definition's own name token `From` is that definition: the
sweep attributes an occurrence to its innermost enclosing definition, and a
definition's name token is inside its own span.

So the stated motive in `01-facts.md:379` — "'every place this symbol occurs'
is one relation, not two" — is already served by two base relations that are
joined in one line.

### What they would cost: the shape, not a corpus number

**Not measured on the pinned corpora.** No SCIP index exists for `fd`, `tokio`
or `deno`, and generating one was cancelled (see the note under part 3).
Measured instead on the four committed fixtures, which carry real indexes from
four different indexers. The def-role count is the `resolved` figure `index`
prints — one per ingested definition occurrence:

| fixture | indexer | `scip_ref` rows | def-role rows added | growth |
|---|---|---|---|---|
| rust | rust-analyzer 1.98.1 | 117 | 68 | +58% |
| python | scip-python 0.6.6 | 96 | 60 | +63% |
| go | scip-go 0.2.7 | 63 | 49 | +78% |
| typescript | scip-typescript 0.4.0 | 101 | 94 | +93% |

Fixtures are definition-dense — a handful of call sites per definition where
real code has many — so a corpus ratio would be lower. Read it as a lower
bound: `scip_ref` grows by tens of percent, and `01-facts.md:998` already makes
`scip_ref` the largest relation in the scale table (5M rows, 120 MB).

### `Role != "def"` is not dead code — it is the guard option (a) needs

`stdlib.dl:204-210` says the filter never filters anything. True today. Under
option (a) it becomes load-bearing: every def-role row arrives with
`From == S`, so without it `calls(S, S)` fires for every callable.
`recursive` becomes universal, `entrypoint` empties, `impact_of` gains a
self-loop everywhere.

Three other consumers are safe by accident. Worth writing down so nobody
re-derives it:

- `ref_outside` (`stdlib.dl:331`) and `depends` (`316`) compare `G != F`, and a
  def-role occurrence is always in the symbol's own defining file. A symbol
  defined in two documents would break that — and is refused as a collision
  (`02-extraction.md` § The anchor join).
- `uses_package` (`360`) joins `extern`, and a symbol some document defines is
  by construction not `extern`.
- `scip_claim` would gain the definition positions and suppress nothing: tier A
  emits `name_ref` only from `@reference` captures (`tier_a.rs:413-433`, with
  `assert_captures_disjoint` enforcing the partition), so no tier-A row sits at
  a definition identifier.

### Recommendation: **(b)**

Nothing consumes them; they duplicate `def` + `def_name` + `at`; they grow the
largest relation by tens of percent; and the single rule that would need a
guard shows what option (a) costs in thinking, not just in bytes.

Concretely, (b) is three edits and a test:

1. `01-facts.md:379` — definition sites live in `def`/`def_name`, not
   `scip_ref`. State it, and state why (the `From` column is vacuous there).
2. Delete the unreachable `(SymbolRole::Definition, "def")` row at
   `scip.rs:85`.
3. Drop `Role != "def"` from `calls_at`, and the comment block explaining it.

Plus a test pinning that no `scip_ref` row ever carries `"def"`. Enforce the
invariant where the rows are created, not with a downstream guard that reads as
dead. The counterfactual guard is recoverable from this memo and from the spec
line in step 1.

---

## Part 3 — the dictionary mmap

### Is the SAFETY comment false? Premise yes, conclusion no.

`intern.rs:425-428` claims "the invariant is that no other process truncates
these files while an index is open". The writer truncates, **in this process**,
at `intern.rs:366-367` — and does it with a mapping live: `flush` takes the
`Dict` out of `self` (`339`) but holds it in `previous` across the `append`
call. A mapping over a file cut below the mapped region is exactly the SIGBUS
the comment is supposed to rule out.

The conclusion still holds, for a reason the comment does not give and `flush`
does (`intern.rs:331-336`): **the cut never goes below what the open view
reads.**

- Opened via `open_to`: `Dict::open_to` bounds the view to exactly
  `(bin_len, idx_len)` and `trusted` is the same pair. Cut == view.
- Opened via `open`: `trusted` is `None`, so `append` calls no `set_len` at all.
- After a successful flush, `trusted` is reset to `None` (`343`), so the next
  append truncates nothing.
- The only other `Some` is set mid-append (`374`) to the file length *before*
  this append, which is at least any view's `bin_len`; a retry after a failed
  append cuts back to there.

And `resolve_index` reads only `bin.get(..self.bin_len)` (`184`), never past
the trusted extent.

So the comment is wrong on its face, the code is sound, and the argument that
makes it sound lives three method bodies away. That is the exact state in which
an `unsafe` invariant gets broken by a well-meaning commit.

### Does the mmap buy anything? On the code, no.

Both classic benefits are already forfeited before `open` returns:

1. **Lazy paging — no.** `validate` (`122-141`) calls `resolve_index` for every
   entry, and `resolve_index` ends in `core::str::from_utf8` over that entry's
   bytes (`184`). `from_utf8` scans every byte, so every page of `dict.bin` is
   faulted in. Not optional: `open_within` calls `validate` unconditionally
   (`113`).
2. **Avoiding a heap copy — no.** `Interner::open_with` (`238-240`) inserts
   `text.to_string()` into `by_text` for every entry. The whole dictionary is
   duplicated onto the heap, plus `HashMap` overhead, before the interner is
   returned.

What remains is that `Dict::resolve` hands out `&str` borrowed from the
mapping. `fs::read` gives an owned `Vec<u8>` with identical borrow semantics
and the same resident bytes.

### The argument that settles it without a clock

`Store::load` already reads every segment with `std::fs::read`
(`store.rs:384`), and segments are the larger, hotter data — ~200 MB at 1M
symbols (`01-facts.md` § Scale) against a dictionary of ~40 MB in the manifest
example and 7.7 MB on the largest real index on this host. **The project
already decided `fs::read` is adequate for the big path.** The dictionary is
the small one, and it is the sole reason the workspace carries `memmap2`, one
`unsafe` block, and one `#[expect(unsafe_code)]`.

### The one number nobody has

**UNMEASURED: what dropping the mmap actually costs at load.** The metric is
`load_warm_ms` (`Store::open` + `Store::load`, `benches/corpus.rs:184-198`).
Pinned baselines: fd 1.543 ms, tokio 21.735 ms, deno 137.562 ms.

The code says the cost should be about zero and may be negative — `validate`
already forces the full read, and one sequential `read()` should beat N page
faults — but that is an expectation, not a measurement.

No A/B was run: no perf work on this host while other sessions compete for it.
Observed load averages while this memo was written, 23:12–23:25: **36.21
one-minute, 49.65 five-minute**, on a 12-CPU host. Nothing timed under that
would have been evidence. One bench run on a quiet host, both arms, settles it.

The counter-argument a measurement would also settle: an mmap is clean,
page-cache-backed, evictable memory; a `Vec<u8>` is anonymous and counts
against RSS. At 7.7 MB that is noise. It is the only axis on which (a) wins.

### Recommendation: **(b)**, with one cheap gate

Drop the mmap, take `fs::read`, delete the crate's only `unsafe` and the
`memmap2` dependency. The gate: one `load_warm_ms` A/B on a quiet host. If deno
moves by less than the bench's own 20% tolerance — which the code says it will
— take (b). If it moves more, take (a) and write the real SAFETY argument from
the four bullets above.

This is not hedging; it is one measurement with a stated threshold and a stated
default. Option (a) is also strictly cheaper to abandon later than (b) is to
reverse.

### Spec ripples if (b) lands

| file:line | now | after |
|---|---|---|
| `04-storage.md:162` | "Both are `mmap`-ed; resolving an atom is two loads and a slice" | read into memory; still two loads and a slice |
| `04-storage.md:164-166` | "a `HashMap<&str, Atom>` borrowing from the mmap" | **already false** — it owns `String`s (`intern.rs:239`). Correct either way |
| `04-storage.md:226` § Loading | "mmap every segment" | **already false** — `store.rs:384` reads them |
| `04-storage.md:499` | index dir excluded from file sync, justified by mmap | rule stands, rationale changes: concurrent writers, not mmap |
| `research.md:358` §5 | "Loading is `mmap` + concatenate" | `fs::read` + concatenate |
| `research.md:360` §5 | "`memmap2` 0.9.11 is the one dependency this requires" | delete |
| `research.md:378` §6 table | `memmap2 \| segment loading \| deviation` | delete the row |
| `research.md:417-420` §6 | the "`memmap2` has no STACK entry" deviation note | delete; record in §6 that the deviation was retired |

Net: one fewer dependency against a budget `research.md` §6 calls "the entire
budget", and `unsafe_code = "warn"` becomes vacuous in `crates/facts`.

Three of those lines are stale **today**, independent of this decision. Worth
fixing whichever option wins.
