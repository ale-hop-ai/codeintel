---
id: storage
status: proposed
binding: no
---
# 04 — Storage

The fact store is **100% derived**. Deleting the index directory is always a
valid repair, nothing user-authored is ever written there, and there is no
migration path — a schema bump means reindex.

That property is what buys the simplicity below. We are not a database.

## Location

The index lives **outside the tree it describes.** A tool that answers
questions about a tree does not write into it: no new directory in the
checkout, no `.gitignore` edit, and an index can be built where the tree is
read-only.

**One index per checkout, not per repository.** The key is the canonical root
path, so a worktree and the main checkout index separately. That is the
correct granularity: they hold different source, and one shared index would be
wrong for whichever was indexed second.

The index directory (`<index>` below) is the first of:

1. `--index-dir <PATH>` on `index`, `query`, `schema`, `status` and `mcp`.
2. `CODEINTEL_INDEX_DIR`, when set and non-empty.
3. `<root>/.codeintel/`, **if that directory already exists** — an index a
   previous version wrote is kept in use, not silently re-indexed elsewhere.
4. `<cache>/codeintel/<key>/`, where `<cache>` is `$XDG_CACHE_HOME` when it is
   set to an absolute path, else `$HOME/.cache`.

`<key>` is `<label>-<hash>`. `<hash>` is the first 16 hex digits of the blake3
of the canonical root path's bytes, and is the whole of what keeps two roots
apart. `<label>` is the canonical path's **last two components**, each folded
to ASCII alphanumerics, `.` and `_` with every other run becoming one `-`,
each cut to 32 characters and the whole cut to 64 — so
`~/worktrees/codeintel/fix-qc-sweep` keys as
`codeintel-fix-qc-sweep-<hash>`. Two components and not one because a worktree
directory is named for its branch, and `fix-qc-sweep` alone does not say which
repository it belongs to. A path with no usable component keys as the hash
alone.

**The label is legibility, never identity.** Nothing reads it back; a user
pruning the cache by hand should be able to see which checkout an entry
describes without opening its manifest. Two roots that fold to one label still
get two directories, because the hash differs.

With none of `XDG_CACHE_HOME`, `HOME` or an override available, resolution
fails with an error that names `--index-dir`; it never falls back into the tree.

- `--index-dir` wins over the env var because it is the more specific, more
  deliberate statement. Both beat the legacy directory: an explicit override is
  an instruction, a leftover directory is not. A relative path is taken
  relative to the working directory, and is made absolute without requiring it
  to exist.
- Rules 1–2 name **one repository's** index. Pointing two different roots at
  one directory makes each overwrite the other's facts; the default key exists
  so that never happens by accident. `CODEINTEL_INDEX_DIR` is for a single
  repository (CI, a read-only checkout), not a global setting.
- Resolution **never creates anything.** `query` and `status` on a root with no
  index report `no-index` and leave no directory behind. The directory is
  created by the writer lock (`index`, or a `query` auto-refresh of an existing
  index).
- Nothing is ever written into the tree, and `.gitignore` is never read for
  this purpose nor modified. The walk still skips a directory named
  `.codeintel`, so a legacy index is not indexed as source.
- `status` reports the resolved directory (`index:` in text, `index_dir` in
  JSON), because the repair for a `corrupt` index is "delete it" and the
  reader must be able to find it.
- Pruning cache entries whose tree has gone is **not** part of this change. The
  manifest already records the root (`roots`), so a later `status`-side check is
  possible without a format change.

### Discovery

The store moved out of the tree, so **"is there an index for this directory"
stopped being a directory listing.** There is no `.codeintel/` to see. An index
exists for a root exactly when `locate(root)` resolves to a directory holding a
`manifest.json` — a blake3 of the canonical path, then one `stat`. That is the
only test, and it is the test this section is built on.

That has a useful consequence and an awkward one.

Useful: asking *"does root R have an index"* costs one hash and one `stat`, for
any R, with no filesystem search. So asking it of a **set** of candidate roots
is cheap, and finding an index near a root is a bounded probe rather than a
scan.

Awkward: the mapping is one-way. `<label>-<hash>` cannot be read back into a
path, and the only record of which tree an entry describes is `roots` inside
its manifest. Enumerating `<cache>/codeintel/` and parsing every manifest would
find an index at any depth, but it costs a full JSON parse of every index on
the machine — including every unrelated repository — to answer a question about
one directory. **Discovery therefore probes candidate roots; it never
enumerates the cache.**

The candidate set for a root `R`, when `R` itself has no index:

- **Up.** Each ancestor of `R`, nearest first, stopping at `R`'s own
  repository root — the first directory from `R` upward holding a `.git`
  entry, a file as well as a directory so that a git worktree counts. That
  root is itself a candidate; nothing above it is, and `MAX_UP` bounds the
  climb when the tree is not in a checkout at all. Bounded, because an
  unbounded ascent reaches `$HOME`, and one stray `codeintel index ~` would
  then answer for every repository beneath it.
- **Down.** Each directory under `R`, to `MAX_DOWN` levels, by the same
  `.gitignore`-aware walk the indexer uses, so `target/` and `node_modules/`
  are not candidates. Bounded by a directory count as well as a depth, and
  **both bounds are reported when they fire** (invariant 5). A monorepo deeper
  than `MAX_DOWN` gets "searched 3 levels", never a silent "nothing here".

`MAX_UP` is 8 and `MAX_DOWN` is 3. Both are caps on a guess, not semantics: the
numbers cover the shapes that occur — a subpackage inside a repo, a repo inside
a worktree directory, a `daemon/` beside the root — and anything past them is a
root the caller should name.

An explicit index directory (`--index-dir`, `CODEINTEL_INDEX_DIR`) **disables
discovery entirely.** Rules 1–2 name one repository's index, so every candidate
root would resolve to the same pinned directory and the probe would answer
"yes" for all of them. A caller who pinned the store has already said where it
is.

What is done with what is found is a surface question, not a storage one:
[05-surface.md](05-surface.md) § The nearest index.

## Layout

```
<index>/
  manifest.json          index metadata + per-file segment table
  dict.bin               string interner, append-only
  dict.idx               offset table into dict.bin
  seg/<blake3-of-bytes>.bin  one segment per source file, named by its content
```

**There is no `seg/_scip.bin`.** An earlier draft put facts from SCIP documents
with no tier-A counterpart into one blob. They get an ordinary per-file segment
instead, because the blob buys nothing and costs three things: a second load
path, a manifest field to name it, and the loss of per-file forgetting — a
tier-B-only file that is deleted can no longer be dropped on its own. Every
argument for per-file segments applies to them unchanged.

## Interner

Maps `&str` ↔ `Atom(u32)`. Append-only, so an id is stable for the life of the
index and segments never need rewriting when the dictionary grows.

**Id space** ([01-facts.md](01-facts.md) § Integers):

```
0x0000_0000 ..= 0x0FFF_FFFF   small non-negative integers, encoded as themselves
0x1000_0000 ..= 0xFFFF_FFFF   deduplicated strings; 0x1000_0000 is EMPTY ("")
```

`dict.bin` is concatenated UTF-8 with no separators. `dict.idx` is a `Vec<u64>`
of start offsets; entry `i` spans `idx[i]..idx[i+1]`. **u64, not u32** — u32
offsets silently cap `dict.bin` at 4 GiB with no stated limit and wrap to
garbage strings on breach. The extra four bytes per entry costs ~10 MB on a
2.5M-string dictionary. Both are read into memory with `fs::read`;
resolving an atom to a string is two loads and a slice.

**Read, not mapped.** An earlier design mapped them. The mapping bought nothing
measurable: `validate` scans every byte before `open` returns, and `by_text`
copies every string to the heap, so both of an mmap's advantages — paying only
for the pages touched, and sharing them between processes — were already
forfeited by the time the first lookup happened. Segment data, which is larger
and hotter, was read with `fs::read` throughout. Dropping it removed the one
`unsafe` block in `crates/facts` and one dependency (#44).

In-memory, the indexer additionally holds a `HashMap<String, Atom>` for dedup
during a run. The query path never needs it — queries compare
atoms, and only materialize strings when formatting output.

**Every string goes through the dedup map, including doc comments.** An earlier
draft bypassed it for strings over 256 bytes. Measured, hashing ~120 MB of doc
comments with blake3 costs ~0.12 s against a 30 s index budget — 0.4% — in
exchange for a third id partition, a safety rule, and an `invalid-query` class
an agent cannot guess. Not worth it, and it violated this document's own rule at
§ Loading: do not build it until a benchmark says so.

**Growth.** Atoms are never reused and never renumbered. A long-lived index on a
churning repo accumulates dead strings. Reclaim is `codeintel index --rebuild`,
which rewrites the dictionary and every segment from scratch. There is no
incremental GC and there will not be one — a dictionary big enough to matter is
a repo big enough that a 30-second rebuild is acceptable.

## Segment format

One file per indexed source file, holding every fact that file produced.

```
magic       "CIF1"                          4 bytes
schema_ver  u32 le                          4
n_relations u32 le                          4
--- per relation ---
rel_id      u32 le                          4   index into the relation table
arity       u32 le                          4
n_rows      u32 le                          4
rows        [u32 le; arity * n_rows]        4 * arity * n_rows
--- trailer ---
checksum    blake3 of every byte above       32
```

The trailer is what § Concurrency's "every segment carries a body checksum"
means concretely. It sits at the end rather than in the header so the writer
streams the body once and stamps it, and so the layout above stays the layout a
reader walks front-to-back.

Rows are sorted by column order and deduplicated **at write time**, so loading
is concatenate + merge, never sort-from-scratch.

The relation table (id → name, arity) lives in `crates/facts` as a compile-time
constant, not in the file. Schema changes bump `schema_ver`; a mismatch on open
is `status: "stale"` with the reindex command, never a silent read.

**A refusal is typed, and an I/O failure is not a refusal.** `crates/facts`
marks what it refuses: a segment whose `schema_ver` this build does not speak is
*stale*; a failed checksum, magic, relation table, dictionary or manifest is
*corrupt*. The host maps those two to `stale` and `corrupt`, and nothing else
maps to either: a full disk or a permission error is an error the taxonomy does
not express, never `corrupt`, whose repair hint would have an agent delete a
healthy index.

Endianness is fixed little-endian and the loader validates alignment. An index
is not portable across architectures with different `u32` alignment
requirements — it is a cache, so this is stated, not solved.

## Loading

```
read every segment
for each relation: concatenate its rows across segments, merge-sort, dedup
```

No parsing. The on-disk bytes are the in-memory representation. For a 1M-symbol
repo (~200 MB, [01-facts.md](01-facts.md) § Scale) the merge is the only real
cost and it is linear in row count.

Optimization, only if measurement demands it: cache the merged per-relation
arrays in `<index>/merged/` keyed by the manifest hash, so an unchanged index
loads with zero merging. **Do not build this until a benchmark shows load time
above 200 ms.** It is listed here so a future agent knows it was considered and
deliberately deferred.

## Manifest

```json
{
  "schema_version": 3,
  "created_at": "2026-09-12T10:00:00Z",
  "roots": ["/Users/x/proj"],
  "writer_version": "codeintel 0.1.0",
  "extractor_fingerprint": "blake3:7d1a...",
  "dict_bin_len": 40213884,
  "dict_idx_len": 2011240,
  "dict_generation": 3,
  "indexed_at": 1757000000,
  "scip": [
    { "path": "index.scip", "tool": "scip-typescript 0.4.0",
      "mtime": 1757000000, "size": 40218811, "documents": 812,
      "ambiguous": 0 }
  ],
  "scip_collisions": 0,
  "skipped": {
    "src/huge.rs": { "reason": "too-large", "mtime": 1757000000, "size": 2800020 },
    "src/bin.rs":  { "reason": "not-utf8",  "mtime": 1757000000, "size": 22 }
  },
  "files": {
    "src/store.rs": {
      "seg": "a3f1...bin", "mtime": 1757000000, "size": 4021,
      "hash": "blake3:9c2e...", "lang": "rust", "tiers": ["ts", "scip"],
      "scip_hash": "blake3:9c2e..."
    },
    "src/broken.go": {
      "seg": "b41c...bin", "mtime": 1757000000, "size": 118,
      "hash": "blake3:51aa...", "lang": "go", "tiers": ["ts"],
      "scip_hash": null, "syntax_error": 3
    }
  }
}
```

**`syntax_error` is the line of the first parse error, and its absence means the
file parsed whole.** It is the 1-based line of the first `ERROR` or `MISSING`
node tree-sitter produced ([02-extraction.md](02-extraction.md) § A file that
does not parse is partial). It is a function of the file's own bytes like every
other per-file fact (invariant 3b), so it is re-derived whenever the file is
re-extracted and is carried with the entry. A manifest written before the field
existed reads absent; the extractor fingerprint changes with the field, which
forces the full re-extract that fills it.

**`skipped` is the files the walk reached in a supported language and the
indexer did not read, and absence means there were none.** `reason` is one of
`too-large` (over the 2 MiB cap), `not-utf8` and `unreadable`; `mtime` and
`size` are what the walk saw. A file with no grammar is not here — `status`
counts those by extension (05-surface.md § `status`). It is a fact the indexer
already holds, not an inference (invariant 1), and it is whole-tree, not
per-run: every refresh re-derives it from the walk, carrying forward only the
`not-utf8` and `unreadable` entries a `--lang` or deadline-stopped run did not
re-decide. Its `mtime`+`size` are what let `status` tell an unchanged refused
file (accounted for) from an edited one (a change). A manifest written before
the field reads absent, and the next refresh fills it; no schema bump, as with
`syntax_error`.

`hash` is content, `mtime`+`size` is the fast path. A file is unchanged without
hashing iff mtime and size both match **and its mtime is before `indexed_at`**;
otherwise hash before deciding to re-extract.

**`indexed_at` is the second the last refresh that observed every file
started** — git's racy-clean rule. mtime has one-second resolution, so an edit
landing in the same second the refresh read the file, at the same length, leaves
mtime and size exactly as recorded. The fast path alone would never re-read it,
and that is the agent's own edit-then-ask loop. A file with `mtime >=
indexed_at` may have changed after it was read, so it is hashed. It is the
refresh's *start*, not its commit: a file modified between the read and the
commit has an mtime at or past the start and stays suspect, where a commit
timestamp could fall in the next second and clear it. Only a run that walked
every language and finished records it, like the fingerprint: a `--lang` run or
one `max_refresh_ms` stopped did not look at every file. A manifest written
before the field existed reads `0`, and clears nothing until the next commit.

**A racy file costs a hash per refresh, never a write.** The suspects are the
files touched in or after the second the last committing refresh started —
normally the file just edited. A refresh that hashes them and finds them
unchanged writes nothing, so a read stays a read (§ Incremental reindex), and
`indexed_at` moves on at the next commit. Nor do they force a SCIP re-ingest:
that decision stays on mtime and size, because a tree checked out and indexed
within one second would otherwise decode `index.scip` on every read. A racy
file that did change is re-extracted tier A only and reported `scip-stale` —
with no re-ingest, the index was not read, and it saw nothing.

**`scip_hash` is the content the SCIP inputs last saw.** At ingest it becomes
`hash` when the file was not newer than the newest input, or when `hash`
equals what the previous ingest of the same inputs recorded; otherwise the
recorded value is kept, or `""` when the inputs never saw any version. A file
is `scip-stale` iff `scip_hash` differs from `hash`, so a file that is touched,
or edited and restored, clears itself without rerunning the indexer.
Staleness by mtime alone could not: the touched-but-unchanged path refreshes
`mtime`, and a restored file stayed `scip-stale` until the indexer reran. An
index written before the field existed has none and falls back to mtime until
its next ingest.

**A SCIP input carries `mtime` and `size` for the same reason, and `tool`,
`documents` and `ambiguous` are *not* compared.** Those are only
known after the index is parsed, and a refresh that changes nothing must not parse it — a large
`index.scip` on the query path would put a protobuf decode in front of every
answer. Comparing them would make every refresh look like a changed SCIP input
and re-extract the whole tree. `ambiguous` counts the occurrences placed
nowhere because the index declares no position encoding and the text before
the column is not ASCII ([02-extraction.md](02-extraction.md) § Position
normalization). All three are that input's own, read from it alone, never the
merge: every input carrying a merged number reports the total once per input,
and a reader that sums counts it once per input.

**`scip_collisions` is top-level because it has no per-input value.** It counts
symbols defined in more than one document across the merge of every input
([02-extraction.md](02-extraction.md) § The anchor join), and a collision can
span two inputs. It is carried forward on a refresh that does not parse, like
the per-input counts, and is `0` with no inputs. A manifest written before the
field existed reads `0` until its next ingest.

**`extractor_fingerprint` is blake3 over the binary version, the source of
`crates/extract/src`, the locked versions of the grammar and SCIP crates, every
vendored and authored `.scm` byte, every field of every `lang.rs` row, and the
kind-mapping table. A mismatch forces a full re-extract.** The binary version
alone is not enough: it has been `0.1.0` through every extractor fix, and a
`cargo update` moves a caret-pinned grammar without touching it. Hashing the
source over-invalidates on a comment edit, and that is the cheap direction. The
field list is destructured, so a new `Lang` field does not compile until it is
hashed. Without it, incremental indexing means an
extractor fix reaches only the files a user happens to edit afterwards: half the
index is built by the old query and half by the new one, `status` says `ok`, and
nothing will ever reconcile them. For the same reason the fingerprint is
recorded only by a run that walked every language and finished: a `--lang`
run, or one `max_refresh_ms` stopped, leaves the old value in place, so the
next full run still re-extracts what it skipped. It also makes every bug report irreproducible,
because the reporter's first move is deleting the index directory, which destroys the
evidence. Fifteen lines.

**`roots` is an array from day one** even though v1 writes one element.
Retrofitting it after segment paths are committed is an invasive change; doing
it now is one character.

**`seg` is exactly what the writer names it: 64 lowercase hex digits and
`.bin`, and nothing else parses.** It is joined onto `seg/` to load a segment
and to unlink a retired one, and the manifest is a file in a directory a cloned
repository can ship. An unchecked `"/home/x/.ssh/id_ed25519"` or `"../../src/main.rs"`
survives `Path::join` as a path outside the index, and the first refresh that
retires that entry — a file the tree no longer has is enough — deletes it. A
manifest naming any other shape is `corrupt`, never read and never followed.
Only `Store::put` names a segment, from the bytes it writes; a caller hands it
every other field (`NewEntry`), so no placeholder name exists to be written and
`SegName` has no value that fails its own check.

**`dict_bin_len` / `dict_idx_len` bound what a reader may trust.** The manifest
names segments but does not name dictionary extents, so a reader that loads a
new manifest and reads a dictionary mid-append can read an offset past the end
of the bytes the manifest vouched for. Recovery from a torn append is truncation to the recorded
length: a reader bounds its view to the recorded extents, and a writer
truncates both files to them before it appends.

The rule runs in both directions:

- **Longer than recorded is a torn append; shorter or missing is corrupt.**
  Non-zero extents with `dict.bin` or `dict.idx` absent, or either file shorter
  than its extent, is `corrupt`. Reading that as "no dictionary yet" starts the
  id space over at `""`, and every segment atom past the surviving prefix is
  then handed to a new string: old rows resolve to the wrong names, and the
  next commit makes it permanent. Extents of zero still mean no dictionary,
  whatever is on disk.
- **A commit records the extents the interner vouches for, not the file
  sizes.** A refresh that interns nothing appends nothing, so it leaves a
  crashed run's torn tail where it is — past the extents, where no reader looks
  and the next append cuts it — and records the extents it read through.
  Recording `stat` sizes would stamp those torn bytes as trusted, and every
  later open would fail validation until `rm -rf`.
- **A flush that fails leaves the interner as it was.** The strings it did not
  write are still pending, the view is still the one it had, and a retry cuts
  back to where the failed append began.

**`dict_generation` increments on `--rebuild`.** `--rebuild` renumbers every
atom, and `query --raw` hands raw atom ids to the caller. Without a generation
stamp, a saved raw id resolves after a rebuild to a *different string* rather
than to an error.

## Incremental reindex

```
walk the tree (ignore crate, gitignore-aware)
  new file          -> extract, write segment, add manifest entry
  changed file      -> extract, overwrite segment, update entry
  unchanged file    -> skip
  vanished file     -> delete segment, drop entry
if any SCIP input changed (mtime or size):
  re-extract every file with the new ingest -> the anchor join runs inline
```

The anchor join is not a pass over written facts. Tier A consults what SCIP
knows about a file *while it emits*, so a definition adopts its SCIP symbol
before any row exists and nothing is rewritten twice
([02-extraction.md](02-extraction.md) § The anchor join). That is why a changed
SCIP input re-extracts rather than rewrites: there is nothing to rewrite.

Single-file change: one parse, one segment write, one manifest write. Target
< 1 s, dominated by the tree walk.

**A refresh that changes nothing writes nothing.** No dictionary flush, no
manifest write, no `fsync`. `query` refreshes before every answer, and
committing an unchanged manifest cost 8 ms of a ~40 ms MCP call on a 799-file
tree (#18), which is a durable write per read. "Changed" means the manifest
after the run differs from the manifest before it. Every segment written, every
file forgotten, every touched file's refreshed `mtime` and every dictionary
extent lands in the manifest, so equality is the whole test and no separate
dirty flag can disagree with it. The one exception: **a store with no index yet
always commits**, so `codeintel index` on a tree with nothing to extract leaves
an empty index rather than `no-index`.

**SCIP is all-or-nothing.** A SCIP index carries cross-file references, so a
changed `index.scip` invalidates the reference graph globally. Attempting
per-document SCIP incrementality is a correctness trap: a reference that
*disappeared* from a document leaves no evidence in any other document, so
there is nothing to drive its removal. Re-ingest the whole thing.

Corollary worth surfacing to the user: after editing source, the file is
re-extracted **tier A only** — tier B emits nothing for a file the inputs did
not see ([02-extraction.md](02-extraction.md) § The anchor join) — so its
definitions are `local` and its `scip_ref` rows are gone until the indexer
reruns, while every other file keeps its exact rows. `status` reports the
files. A query whose answer depends on `"exact"` provenance is then answering
about the past for those files, and the agent must be able to see that.

## Concurrency

**One writer, many readers, no locking protocol.**

- `codeintel index` writes each new or changed segment to `seg/<hash>.bin.tmp`,
  **`fsync`s it**, and renames it into place; then writes `manifest.json.tmp`,
  `fsync`s it, and renames it **last**. `<hash>` is blake3 of the segment's
  own bytes, not of the source path: a changed file gets a *new* segment file,
  the one the old manifest names is untouched until the new manifest is in
  place, and it is unlinked only after. A vanished file's segment is likewise
  unlinked after the commit that drops its entry. Every rename is followed by
  an `fsync` of the directory, without which a crash can keep the file and
  lose its name. The `fsync` is not optional: without it
  a power loss can land the manifest rename while segment data is still in page
  cache, and the result is a manifest naming a **zero-filled** segment. Atom `0`
  is the *integer* zero ([01-facts.md](01-facts.md) § Integers), so those bytes
  decode as well-formed facts asserting that every symbol lives at line 0,
  column 0 — no error, no status, a silently wrong answer. The manifest is the only thing that names segments, so a reader sees
  either the old index or the new one, never a mix. A crash before the
  manifest rename leaves the old manifest naming files that all still exist,
  plus orphans — `.tmp` files and segments no manifest names — which the next
  writer removes under the lock before it writes anything.
- Readers read the segments named by the manifest they loaded, after
  releasing the lock. A concurrent refresh may commit and unlink those segments
  in between. **A named segment that is not there is a race until shown
  otherwise:** the reader re-reads the manifest and, if it moved, loads the new
  one — once. If it moved again the answer is `stale`. If it never moved, the
  manifest names a segment nothing replaced, and that is `corrupt`.
- Two concurrent writers are undefined behaviour. Take an advisory lock on
  `<index>/lock` (`flock`, `LOCK_EX | LOCK_NB`) and fail rather than racing.
  **`query`'s auto-refresh is a writer and takes the same lock** — an MCP server
  serving two concurrent queries is two writers, and `dict.bin` is append-only
  with no tmp+rename, so interleaved appends put `dict.idx` offsets out of step
  with `dict.bin` bytes and every atom past that point resolves to the wrong
  string, with no checksum to notice.
- `--rebuild` takes the writer lock first and discards under it, and
  `<index>/lock` survives the discard: a second writer opening a fresh lock
  file would not contend with the first. A refresh re-reads the manifest after
  taking the lock — one read to decide whether to refresh, one under the lock
  to build on — so a refresh that lost a race never commits a manifest built
  from a copy another writer has already replaced.
- A **reader** that cannot take the lock does not fail. It reads the current
  manifest and returns `status: "stale"`. `locked` is for a second writer;
  telling a reader "locked" is not actionable. A reader that cannot open the
  lock file at all — a read-only checkout — is the same case: the answer comes
  from the index as it stands, `stale`, with the reason.
- Every segment carries a body checksum, validated on open, and `n_rows` is
  bounds-checked against the file length. A truncated or zero-filled segment
  returns `status: "corrupt"` with hint `codeintel index . --rebuild`
  — never a panic and never silent rows of atom 0.

Windows has no unlink-open-file semantics and is out of scope for v1. Say so in
the README rather than half-supporting it.

## What is not here, and why

- **No SQLite.** A C dependency, schema migrations, and a parse/load step, for a
  store that is derived, single-writer, and rebuildable. dex paid all three
  costs; [research.md](../docs/research.md) §5.
- **No WAL, no transactions.** Crash recovery is `fsync` + rename ordering +
  per-segment checksums + dictionary extents in the manifest, above. A crashed
  index leaves the previous manifest pointing at valid, checksummed segments,
  plus orphans — `.tmp` files and unnamed segments — that the next run removes.
- **The index directory must be excluded from file sync.** A Dropbox/iCloud
  partial write or an SMB/NFS truncation gives a torn or short read. Now that
  nothing is mapped this surfaces as a `corrupt` status rather than the
  `SIGBUS` an mmap raised, which no status code could report — but a synced
  index directory is still a broken one. One line in the README.
- **No compression.** `u32` columns of interned ids are already dense. Adding
  compression would put a decode step on the load path to save disk nobody is
  short of.
- **No network format, no export.** `codeintel query --format json` is the
  export. If someone needs the raw facts, they query for them.
