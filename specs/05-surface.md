---
id: surface
status: proposed
binding: yes
---
# 05 — Surface

The surface budget from [00-overview.md](00-overview.md) is binding: **5 CLI
verbs, 1 MCP tool, <= 16 base relations, and `schema` output <= 10,000
characters with the rule list complete.** Growth requires deletion.

There is **no cap on the rule count**. There was, twice, and both numbers were
set rather than derived; 00-overview.md § Surface budget records why they were
dropped. Rules are the growth path the design wants — a new structural question
is meant to become a rule, not a verb or a Rust helper — so the thing to bound
is what they cost an agent, which is `schema` bytes. `tests/surface.rs` asserts
all four.

## Status taxonomy

Every response on every surface carries a `status`. Adopted from dex, which got
this right: a consumer branches on status instead of catching failures or
guessing what an empty result means.

| status | Meaning | Hint returned |
|---|---|---|
| `ok` | query ran, results are complete | — , unless `root` differs from the root asked about (§ The nearest index) |
| `truncated` | query ran, a cap fired | which cap (`max_result_rows` / `max_result_bytes`), and its value |
| `no-index` | no index for this root, and none was adopted | one line that indexes it, tier B included (§ The no-index hint); plus any index found nearby (§ The nearest index) |
| `stale` | sources changed and auto-refresh was skipped, exceeded `max_refresh_ms`, or `schema_version` mismatched | `run: codeintel index .` — names the N files |
| `no-scip` | the goal's **relation-dependency closure** reaches a relation only SCIP populates, and none was ingested — **or** an index was ingested and not one of its documents matched an indexed file, which leaves those relations just as empty | which relations, and the indexer command for the languages present. When documents were ingested and none matched, the hint says so, with the count and the `--scip-root` that would fix it |
| `scip-stale` | an indexed file holds bytes `index.scip` did not see ([04-storage.md](04-storage.md) § Manifest, `scip_hash`) | which files, and the indexer command |
| `invalid-query` | parse or safety failure | the rule, the variable, the violated rule |
| `unstratified` | negation cycle | the cycle |
| `timeout` / `budget-exceeded` | a limit aborted evaluation | the limit and its value |
| `locked` | another index run holds the lock | its pid |
| `corrupt` | a segment or the dictionary would not load | `run: codeintel index . --rebuild` |

**There was an `unsupported-language` row here, and nothing ever emitted it.**
Deciding it per query costs a full tree walk to find files no grammar covers,
which is `codeintel status`'s job and is why the walk lives there. A status the
taxonomy advertises and the code cannot produce is the same defect as a status
the code produces and the taxonomy omits — `the_status_taxonomy_matches_the_spec`
now fails on either. `status` reports unsupported extensions, biggest first.

**`ok` with zero rows means "this is not true of your code."** That must be
distinguishable from every failure above without reading prose. This is
invariant 6 and the most common way a tool like this lies to an agent.

`no-scip` is that invariant at its sharpest, and it fires on the **closure, not
the syntax**. `?- impact_of(S, C).` mentions no base relation at all; its
closure reaches `scip_ref`. Without the closure test the README's own headline
query returns `ok` with zero rows on a fresh install.

**`stale`, `truncated`, `no-scip` and `scip-stale` all carry rows and all exit
0.** They are answers about a known-imperfect index, not failures: the caller
gets what there is and is told exactly what is wrong with it. Only `no-index`,
`invalid-query`, `unstratified`, `timeout`, `budget-exceeded`, `locked` and
`corrupt` exit 2 — and so does an error no status expresses (an unreadable
program on stdin, an I/O failure), because 2 means "the query never ran" and 1
is `--expect-empty`'s "it ran and found a violation" (§ `query`). `schema` is
the one exception: its answer is the catalog, complete without an index, so it
reports `no-index` and exits 0 (§ `schema`).

**When two of those apply, the index's condition wins the `status` and the cap
rides along.** A truncated answer from a stale index is `stale`, not
`truncated`; likewise `no-scip` and `scip-stale`. Truncation is already carried
structurally — `truncated: true` and `cap` are set whatever `status` says — so
a status of `truncated` would add nothing a consumer cannot read, while
overwriting `stale` would erase the one fact that is nowhere else in the
response. The hint names both: the index's problem first, then the cap. Between
the index conditions the order is `stale`, then `no-scip`, then `scip-stale`: a
refresh that did not finish makes the SCIP verdict moot.

---

## CLI

```
codeintel index  [PATH] [--scip FILE]... [--scip-root DIR]... [--rebuild]
                 [--lang L]... [--index-dir DIR]
codeintel query  <PROGRAM|-> [--path DIR] [--format text|json] [--limit N]
                 [--rules FILE]... [--raw] [--no-refresh] [--expect-empty]
                 [--index-dir DIR]
codeintel schema [--path DIR] [--format text|json] [--index-dir DIR]
codeintel status [PATH] [--format text|json] [--index-dir DIR]
codeintel mcp    [--path DIR] [--index-dir DIR]
```

**Five verbs.** `PATH` defaults to `.`.

`--index-dir` (env `CODEINTEL_INDEX_DIR`) says where the index lives; the
default is outside the tree. Precedence and edge cases:
[04-storage.md](04-storage.md) § Location. It is a flag, not a verb, so the
surface budget is unchanged.

`rules` is gone. It translated `rules NAME A B` into `?- NAME(A, B, Vars...)`
with args "bound positionally as atoms, no type coercion" — and integers and
their string forms are different atoms ([01-facts.md](01-facts.md) § Integers),
so `codeintel rules innermost_at src/store.rs 142` bound the *string* `"142"`,
matched nothing, and returned `status: ok` with zero rows. Broken for every rule
taking a line number, including the headline one, and lying while doing it. The
`*_by_name` rules it needed stay in `stdlib.dl`; they are useful on their own.

`--format tsv` is gone: text is already TSV-ish and there were three formats for
two consumers.

`--run-indexers` is not built — see [02-extraction.md](02-extraction.md)
§ Acquisition. `index` prints the exact command.

### `index`

Builds or updates the store. `--scip` may repeat; defaults to `./index.scip` if
present. `--rebuild` discards and rewrites, including the dictionary. `--lang`
restricts grammars: files of every other language are neither refreshed nor
dropped, they are carried forward unchanged.

`--scip-root` declares the directory a SCIP input's document paths are relative
to, for an indexer that had to be run from a subdirectory
([02-extraction.md](02-extraction.md) § Document paths, and `--scip-root`). Zero
means the repository root, one applies to every `--scip`, N pairs positionally
with N; any other count is an error. Like `--index-dir`, it is a flag and not a
verb, so the surface budget is unchanged.

`index` prints the exact indexer command for every language it found.
Copy-pasteable, not a description of one. It never runs one: `--run-indexers`
is not built ([docs/plan.md](../docs/plan.md) M3, § Explicitly deferred).

Prints a summary to stderr: files indexed/skipped/unchanged, facts produced,
SCIP coverage, anchor rate, SCIP occurrences skipped because their column is
ambiguous ([02-extraction.md](02-extraction.md) § Position normalization),
elapsed. The facts line reads `tier-A defs`, the same words as the anchor rate
below and the same quantity: distinct symbols tree-sitter found in this run,
not `def` rows and not the store's total. `refs` and `imports` beside it are
row counts, which is why the one that is not says so. Skipped files are
summarized **by reason**
— "3 too large, 88 unsupported (`.scala`)" — because a silently unindexed
subtree is the single most confusing failure this tool can have.

SCIP documents the walk does not reach are listed there too, with the reason
`ignored`, per [02-extraction.md](02-extraction.md) § Budget. That count is
affordable where the walk's is not: the documents are an enumerated set, already
in memory, and testing each against the walked paths costs no extra traversal.

**SCIP coverage is a match count, not a document count.** The summary prints
`N of M documents matched an indexed file`, and when that number is zero with
`M > 0` it is a reported degradation, not a `0.0%` anchor rate the reader has to
diagnose: `0 of 2523 SCIP documents matched an indexed file`, the first few
unmatched paths, and — when exactly one prefix re-bases every one of them onto
an indexed file — `paths look relative to a subdirectory; re-run with
--scip-root daemon`. When no single prefix does, or more than one does, it says
that instead and suggests nothing. Detection never re-bases on its own
([02-extraction.md](02-extraction.md) § Detection suggests; it never re-bases).

This is invariant 5 at the only place it can be caught. "SCIP present, nothing
matched" and "no SCIP at all" are different repairs, and a silent `anchored:
0/2523 defs (0.0%)` names neither (#60).

**The anchor rate counts tier-A definitions, and says so in the line.** It is
`anchored: N/M tier-A defs (P%)`: of the definitions tree-sitter found, how
many adopted a SCIP identity — the figure
[02-extraction.md](02-extraction.md) § Validation 3 sets a floor under, and the
one that goes to zero when document paths do not match. The denominator is
**not** `def` rows: those include the definitions only SCIP knows, each of
which carries `resolved` by construction, so dividing by them measures how much
of the store came from SCIP rather than how well the join worked. Both counts
are per distinct symbol, not per declaration, so an overload set or a
getter/setter pair counts once on each side (§ One symbol, several declarations
in [01-facts.md](01-facts.md)). `status` prints the same line for the same
store, which it did not before: it divided `resolved` rows by `def` rows and
read several points high under the same label (#77).

**That count is not `def`'s row count, and it is not meant to be.** This
paragraph used to finish "matching the single `def` row it produces", which was
wrong twice over and both ways made the number look like a row count it never
was (#106). `def` is distinct over `(S, F, Kind, Name)`, so a symbol whose
declarations differ in `Kind` is one symbol and **two** rows; and `def` also
holds the tier-B-only definitions this counter excludes by construction. On
deno the counter reads 84,068 against 84,091 `def` rows, and the 23 are real
symbols, named on #106 — TypeScript declaration merging (`class` + `interface`:
`EventEmitter`, `URL`, `Duplex`, eleven more) and, against the issue's own
premise, seven in **Rust**: `#[cfg]`-gated `struct` + `typealias`
(`MaybeDashMap`, `UnixStreamResource`), `enum` + `typealias`, and a `macro`
sharing a name with a `function`.

The two quantities are both wanted, so neither moves: the rate needs symbols
because anchoring is per symbol — a denominator in rows would make the
percentage wrong wherever a symbol carries two kinds — and `def`'s row count is
what a query reads. Having one number serve both is what this says it does not
do.

**There is no `ignored` count.** For the *walk*: the `ignore` crate does not
report the entries its matchers drop, so the number costs a second full
traversal with the matchers off — which on a repo with a populated `target/` is
the most expensive thing in the walk, spent on a figure that changes no
decision. An earlier draft of this
line read "412 ignored, 3 too large, ...".

### `query`

```sh
codeintel query '?- calls(C, S), def(S, _, _, "get").'
codeintel query - < investigation.dl
```

**Auto-refresh, on by default.** Before evaluating, `query` stats the files in
the manifest and re-extracts any that changed. An agent's normal loop is edit →
ask, and answering that from a stale index produces a confidently wrong answer —
the worst failure this tool has, because it looks like a right one.

- Tier A only. SCIP cannot be refreshed incrementally
  ([04-storage.md](04-storage.md) § Incremental reindex), so a file edited since
  the SCIP index was built is re-extracted with tier A alone: fresh `name_ref`,
  no `scip_ref`, `local` definitions
  ([02-extraction.md](02-extraction.md) § The anchor join). That is reported as
  `scip-stale`, which auto-refresh makes **more** likely, not less — so the
  status matters more now, not less.
- **Auto-refresh carries the manifest's SCIP inputs forward.** What it must
  never do is refresh with no SCIP at all: that looks like "the index
  disappeared", re-extracts the tree tier-A-only, and deletes every tier-B fact
  — the store would degrade a little more with each query, silently. Rerunning
  the indexer and `codeintel index` is what reconciles a `scip-stale` index.
- Bounded by `max_refresh_ms` (default 2,000). If the refresh would exceed it,
  it is abandoned, the query runs against the index as it stands, and the
  response carries `status: "stale"` naming the files it could not refresh.
  Never a silent slow path and never a silent stale answer.
- `stats.refreshed` **counts** the files re-extracted before the answer, as an
  integer. It was specified as a list, and a list has no bound: one
  `git checkout` can re-extract hundreds of files, and every response would
  carry every path (#19). A refresh that runs out of time already names the
  files it left behind, five at most, in the `stale` hint. Zero is the common
  case. It
  costs a gitignore-aware walk of the tree, one `stat` per file (which is how
  new files are found), and **no write**
  ([04-storage.md](04-storage.md) § Incremental reindex). ~5 ms for 799 files.
- `--no-refresh` skips it, for benchmarking or for querying a deliberately
  pinned index.
- New files are picked up; **deleted files are pruned.** A refresh that only
  added would leave a deleted file's facts answering queries.

**`--rules FILE` is where a repository keeps its own conformance rules** —
layering, banned dependencies, allowed directions — so they live in the
repository under review rather than in this binary. Repeatable, loaded after
`stdlib.dl`.

**Clauses in a rule file are additive, not replacements.** A predicate is the
union of its clauses, so a file defining `is_test` *widens* it. Only a rule
written in the query program itself shadows a loaded one, and that shadowing is
reported on stderr and in `stats.shadowed` — a repository rule quietly replacing
`is_test` would change every answer that reads it. A repository that means to
replace a stdlib rule puts it in the program, not in a rule file.

**Widening is reported too, in `stats.widened`.** Only shadowing was, and
widening is the one a repository actually does: `--rules` is where its
conformance rules live, so a collision there is an accident of naming rather
than an intent to replace. It is not a small effect. On
`tests/fixtures/rust`, a rules file whose three clauses happen to name
`callable/1` takes `?- calls(A, B).` from 3 edges to 15 and `?- entrypoint(S).`
from 16 to 22 — every rule downstream of the collided predicate moves — and
this reported `shadowed: []` with `status: "ok"`. The two mechanisms stay
separate fields because they are opposite operations, and a reader has to know
which one happened: `shadowed` replaced a definition, `widened` added to one.
Each entry names the predicate and the file, deduplicated per file, and both go
to stderr as well.

**A widening is a collision with something already defined** — the stdlib, or a
rule file loaded earlier. A predicate a file introduces is not one, however many
clauses it introduces it with: a layering policy is one clause per forbidden
edge (`docs/cookbook.md` §1), and reporting its second clause as widening its
first tells the user to go looking for a stdlib conflict that does not exist
(#76).

**`--expect-empty` exits 1 if any row comes back.** A conformance check states
the violation it looks for, so finding none is the passing case. The exit code
is 1 and not 2 because "your code violates this" and "I could not tell you" are
different results in CI: 2 stays reserved for a query that never ran.

MCP always refreshes; the flag is not exposed there. An agent editing files is
the assumed case, not the exception.

Text format is TSV-ish, one row per line, aligned, designed to be read by a
human and grepped by an agent. JSON is
`{ status, columns, rows, truncated, cap, stats }`.

**Row order is lexicographic over the rendered row, then over the raw row**,
applied here rather than in the engine. The engine's own order is by atom, which
is dictionary order — insertion order ([03-datalog.md](03-datalog.md)
§ Determinism). That is total and deterministic for one index, but a cold index
and an incrementally-updated one assign different atom ids to the same string,
so the same repo state would print the same rows in a different order depending
on how the index was built. Sorting the rendered text costs one sort per query
and makes invariant 8 hold across index paths, which is the property a consumer
actually relies on.

**The raw row breaks ties.** Two different tuples can render identically — two
symbols of one name defined on one line, or a symbol beside a string with the
same text. Ordered by the rendered row alone they kept atom order, which is the
index-dependent order this sort exists to remove, and the byte cap could keep a
different one of them. The raw row is the tuple itself, so the key is total.

The sort runs **before** the byte cap, so the rows a truncated answer keeps are
a function of the rendered text alone. `max_result_rows` still fires inside the
engine, in atom order, so that cap's surviving *set* can depend on how the
index was built — a known gap, named so it is not mistaken for closed.

**Symbol rendering.** A raw `SymId` is a ~68-character SCIP string — 17 tokens,
unreadable, and impossible for an agent to retype correctly. In text format a
column holding a symbol atom renders as `Name path:line`, with the name and the
location joined by a **space**, so the column stays one printed field:

```
?- calls(C, S).
start src/app.rs:7      open src/db/conn.rs:3
warm src/store.rs:38    get src/store.rs:19
```

**One printed field per column, always.** An earlier draft of this line read
"where the row also carries a file and a line they render adjacent" — which, on
the reading where a symbol expands into two tab-separated fields, makes the text
form wider than `columns` says it is and hands a consumer that splits on tabs a
ragged table. The space join costs nothing and keeps the TSV honest.

Symbol-ness is **extracted, not guessed**: an atom is a symbol exactly when
`def` has a row for it, and the location comes from `def_span`. A symbol with no
`def_span` prints `Name path`; an atom with no `def` row prints as itself.
Nothing here infers from the shape of a string.

Because the renderer resolves the location itself, `?- calls(C, S).` is already
a useful answer — a `def(S, F, _, N), at(S, F, L)` tail is for *filtering* on
those values or binding them, not for seeing where something is. `schema` says
so, because every join an agent does not have to write is a join it cannot get
wrong.

`--raw` prints the underlying `SymId` instead. Use it when piping one query's
output into another query's literal. This is presentation only — the tuple is
unchanged, and `--raw` is what round-trips.

**One notation per answer, chosen by `raw`.** JSON carries `rows` only: the
rendered form by default, the raw atoms under `raw: true`. An earlier version
carried both at once — `rows` raw and `display` expanded — so that "a
programmatic consumer never parses the pretty form."

That consumer does not exist. **The sole consumer of this surface is a language
model** (§ The consumer), and for it the second notation is not a convenience,
it is a duplicate of every row it already has. Measured on a seven-row answer:
`rows` 1163 bytes, `display` 1131 bytes, carrying the same seven tuples twice.

The round-trip the two-notation rule existed to protect is preserved exactly,
because `raw` already selects the notation that round-trips, and the tie-break
rule below is unaffected: the sort key stays (rendered row, raw row) whichever
notation is sent, so ordering does not depend on the flag.

**The byte cap is measured on the bytes sent**, not on the engine's estimate
over raw atoms. Symbol expansion is what the consumer's context window actually
pays for, and `crates/datalog` cannot account for it without learning what a
symbol is (invariant 6). It is applied after the sort, so a truncated answer is
a stable prefix of the ordered rows.

"The bytes sent" is per surface, and each is measured by encoding the answer
with the same function that sends it:

| Surface | Measured |
|---|---|
| CLI `query`, text | the rows on stdout (the status goes to stderr) |
| CLI `query --format json` | the whole document: one notation of every row, JSON escaping included |
| MCP `code_query` | the tool result object, one body (§ The consumer) |

An earlier version measured the printed text everywhere. That bounded the text
surface and nothing else: the JSON document carries two notations per row, and
an MCP result carried the text *and* both notations again in
`structuredContent`, several times the budget on a capped answer while
reporting `max_result_bytes` as honoured. Rendering still cuts at the printed
text first, which every surface costs at least; the host then keeps the longest
prefix whose encoding fits, and a cut there sets `truncated` and
`cap: "max_result_bytes"` like any other. The JSON-RPC envelope around an MCP
result is not counted — its `id` is the client's own bytes echoed back. The cut
carries no timing, so which rows survive does not depend on how fast the run
was (invariant 8).

**The budget bounds the whole answer, not only its rows.** Dropping rows cannot
shrink an answer whose envelope alone is over budget, and the one part of an
envelope that grows without the caller's help is the `hint`. So after the row
cut, every answer on every surface — a diagnostic and a `no-index` included —
is measured again, and if it is still over, the hint is cut to the longest
prefix that fits, on a character boundary, ending in
` ... [hint cut to fit max_result_bytes]`. `status`, `truncated` and `cap` are
never cut: a consumer branches on them and an answer without them is not an
answer (invariant 6). Nothing else is cut either. `columns` echoes the goal the
caller wrote and `stats` is bounded by the rule set, so an answer still over
budget after its hint is cut is one whose own query text paid for it. A hint
cut does not set `truncated`, which means rows were dropped; the marker in the
hint says what was cut. On the CLI text surface the hint goes to stderr and is
not counted, so it is never cut there.

**A runnable chain is atomic to the cut**, because the cut marker is an append
and § Response contract forbids appending to the line a chain ends. A prefix
cut lands wherever the budget runs out, and both places it can land are wrong:
inside a command it leaves half of one, and immediately *after* one it is worse
— `rust-analyzer scip . ... [hint cut to fit max_result_bytes]` is a command
whose arguments are the marker's own words, which is #104's failure reached
through the cut rather than through composition (#108).

So the chain is kept whole with the prose cut around it, or dropped whole. It
is **kept** by preference: the prose says what is wrong and the chain says what
to type, and under pressure the second is worth more. When the chain alone will
not fit the budget, the budget wins — the chain is dropped and the marker says
so, ` ... [hint cut to fit max_result_bytes; recovery command dropped]`, because
a hint that quietly loses its command cannot be told from one that never had
one (invariant 5).

Below the floor — a budget smaller than the envelope plus the shorter marker —
nothing complies, and no cut can change that. That floor is a property of the
budget, not of the hint.

For that to be true the engine's own cap must not be the one that fires. The
rendered form is **shorter** than the raw one — a ~68-character `SymId` becomes
`name path:line` — so an engine holding the printed budget would truncate rows
that fit it, and the caller would silently receive a fraction of the answer the
budget allows. The host therefore gives the engine the same budget with
headroom, where it serves as a bound on materialisation rather than as the
answer's size limit.

**Both notations of a row are produced together and ordered once**, by the
display text. Rendering each separately and sorting each gives two arrays whose
`i`th entries are different tuples, so a consumer that shows the display form
and feeds the raw form into its next query binds the wrong symbol. One
consequence worth stating: `--raw` prints the same rows in the same order as the
default, because it is the same answer in a different notation.

**A goal with no variables prints `true` or nothing.** Truth is one empty row
and falsehood is none ([03-datalog.md](03-datalog.md) § Evaluation), so printed
literally the two differ by one invisible newline while both carry `status: ok`.
The word is the answer. JSON is unchanged — one empty row versus none, which is
already unambiguous there.

**A symbol whose `Name` is empty renders as its location alone.** SCIP gives a
crate-root module an empty `display_name`; holding the column open prefixes the
location with a space and reads as a glitch.

Values are carried structurally — one string per column — until the moment of
printing. Tab-joining and splitting back apart loses the column boundaries of
any value containing a tab, which doc comments do.

**At the moment of printing, a value's own control characters are escaped:**
newline as `\n`, carriage return as `\r`, tab as `\t`, backslash as `\\`. Text
output promises one row per line and `columns.len()` tab-separated fields; a
raw newline in a `def_doc` breaks both, and it breaks them *silently* — the
consumer sees more rows than the query matched, with ragged field counts, and
nothing in the status or the row count says so. One multi-line doc comment
turns a single row into seven lines of which six are not rows. That is a
malformed answer wearing `status: ok`, which is invariant 5, and no amount of
"read the notes" fixes a format that lies about its own shape.

Backslash is escaped too, so the transformation is reversible: without it, a
literal `\n` in source text and an escaped newline are the same two characters
and a consumer cannot tell them apart. JSON is unaffected — it has always
escaped these — which is why the defect was invisible to anything that checked
the structured format.

**A truncation hint names a knob that exists.** `--limit` moves
`max_result_rows` and nothing else, so it is offered only when that is the cap
that fired; a byte cap says "narrow the query".

**When both caps fire, `cap` names the one that shaped the output.** The
engine can stop at `max_result_rows` and the printed budget can then cut
further: on deno, `--limit 5000` kept 5,000 rows and the byte budget printed
2,757 (#27). The rows missing from the answer were dropped by the byte cap,
so that is the cap reported, with its advice. Naming the row cap there told
the consumer to raise `--limit`, which returns the same 2,757 rows every time.
A consumer that follows the hint loops, and invariant 7 is not satisfied by a
cap that fired but did not decide the answer.

### The nearest index

An agent does not choose the root. `codeintel mcp` is registered once and
launched with the client's working directory, so the root is whatever directory
the session was opened in. An index built one level down — which is where a
SCIP indexer has to run when the build lives in a subdirectory — answered
`codeintel query` run from that subdirectory and `no-index` to every MCP call,
forever, with no way for the agent to say otherwise (#61). A tool whose only
answer is "wrong directory" and whose caller cannot change directory is a tool
the agent stops using.

So **when the requested root has no index, `query` looks for the nearest one**,
by the bounded probe in [04-storage.md](04-storage.md) § Discovery, and adopts
it when the choice is unambiguous:

1. **Nearest ancestor wins.** An index at an ancestor of `R` covers every file
   under `R` and more — a superset, the monorepo-root case, and the common one.
2. **Otherwise one descendant, and only if there is exactly one.** "One
   unambiguous child" is the whole rule. Two sibling indexes are a choice this
   tool is not entitled to make on the caller's behalf, and picking the
   lexicographically first would be a guess — invariant 1.
3. **Otherwise nothing is adopted**, and the answer is `no-index` with every
   index found named in the hint.

**Adoption is never silent, and it is reported structurally, not in prose.**
The response carries `root`: the root that actually answered. It is `null` when
that is the root the caller asked about, and the path otherwise. An agent
branching on `root != null` learns the scope moved without reading English,
which is invariant 6 — and it matters most in the descendant case, where
`status: ok` with zero rows means "not true of `daemon/`" rather than "not true
of your repository".

**`root` carries the redirect; `status` keeps carrying the index's own
condition.** The adopted index can be `stale`, `no-scip` or `scip-stale`, and
those are facts about the answer that are nowhere else in the response. A
dedicated `redirected` status would have overwritten exactly one of them, for
the same reason truncation composes rather than overwrites (§ Status taxonomy).
`ok` therefore carries a hint in this one case, naming the adopted root and the
command that indexes the requested one instead.

**`index`, `status` and `schema` do not adopt.** They are statements *about a
named directory*; `query` is a question *about some code*. A `status` that
quietly reported on a different directory than the one on its command line
would be useless for the thing `status` exists for. They do report what the
probe found, in their `no-index` hint, so the reader is one copy-paste from the
right command either way.

Discovery runs **only** on `no-index` — never when there is an index to
answer from, so it costs nothing on the hot path — and not at all when
`--index-dir` or `CODEINTEL_INDEX_DIR` pinned the store
([04-storage.md](04-storage.md) § Discovery).

### The no-index hint

`run: codeintel index <root>` was the whole hint, and it was not enough (#59).
An agent that gets it runs the one command, asks again, and is told `no-scip`
on the next question that needs tier B — a second round trip, and the one place
measured where agents gave up and fell back to `grep`.

**The `no-index` hint is one complete line that leaves nothing to look up.** It
chains, in order: the tier-A index, the SCIP indexer for every language
actually present in the tree, and the re-index that ingests their output.

```
no index for /repo. run: cd /repo && codeintel index . && rust-analyzer scip . && codeintel index .
```

- The languages come from the same `.gitignore`-aware walk `index` performs and
  the same `Lang.indexer` string `index` prints, so the hint and the summary
  can never disagree about which indexer a tree needs. No project-marker
  sniffing, no guessing: a language is in the chain when a file of it is in the
  tree (invariant 1).
- A tree whose languages have no SCIP indexer gets the first command alone. The
  chain never names a command that does not exist.
- The trailing `codeintel index .` is not redundant. `index` reads
  `./index.scip` when it is there and the indexer has only just written it, so
  without the second run the SCIP output is produced and never ingested.
- `cd <root> &&` leads, because every SCIP indexer resolves its own project
  relative to the working directory. The root is a shell word: bare when every
  byte is safe, single-quoted otherwise, so a root with a space or a `'` still
  lands where it says.
- **The line is paste-ready, so it holds no placeholder.** An `Lang.indexer`
  string is run verbatim; `<name>` is a redirect in `sh` and makes the whole
  chain a syntax error. A command that needs an argument the tree does not
  supply is written without it. `crates/codeintel/tests/hint_runs.rs` runs the
  hint, and the `build one:` list `index` prints, through `sh -n` over a tree
  holding every language in the table, at a root with a space and a quote.
- **The chain is `&&` on purpose.** If one indexer is missing or exits
  non-zero, the chain stops there and the final `codeintel index .` does not
  run: nothing is ingested, and the index stays tier A, which is what it already
  was. A partial SCIP set ingested without anyone being told is the worse
  outcome. The shell reports which command failed. **This guards only an
  indexer that fails.** `scip-go` run where `go.mod` is not the working
  directory exits 0 over an empty index, so the chain continues and `index`
  ingests `0 documents` (#109); what catches that is `index`'s own `scip
  coverage` line, not the `&&`. § Indexer evidence in
  [02-extraction.md](02-extraction.md) records which commands have been run.
- When discovery found indexes that were not adopted, the hint names them
  first: `no index for /repo; found one at /repo/daemon`. The found path is the
  **root**, not its store directory — that is what the caller passes to
  `--path`, and under [04-storage.md](04-storage.md) § Location the store
  directory is not somewhere the caller ever types.
- The hint is cut to fit `max_result_bytes` like any other (§ Symbol
  rendering), so a chain over a polyglot tree degrades by truncation with its
  marker, never by a silent cap.

**The server still does not index.** The rule stands: **no indexer ever runs on
the query path.** `index` builds; `query` and `mcp` refresh tier A from files
already in the manifest and nothing else (§ `query`). A structural question
must never block on `cargo`, `tsc` or `go build`, and an answer served from a
half-built index is the confidently-wrong answer this whole taxonomy exists to
prevent.

So #59's Option A — the MCP server building the tier-A index itself on
`no-index` and answering — is **not adopted here.** It would put a build behind
a query, hand out rows from an index that is still filling, and make the first
answer of a session depend on how far the walk got. That is a decision with
blast radius and it is the user's to make, not this spec's. Option B is what is
built: the hint above, complete in one line.

### `schema`

Prints the relation catalog, atom vocabularies, and stdlib rule signatures.
**Must fit in ~1500 tokens** — this is the text an agent reads to learn the
system, and it is the highest-leverage output in the project.

**The rule list is generated from `rules/stdlib.dl`, not written here.** One
`%%` line above a predicate's first clause carries its signature and its
one-line doc:

```
%% within(Child, Ancestor)  transitive containment. Child is FIRST.
within(C, P) :- parent(C, P).
```

The signature is written out rather than lifted from the clause because heads
carry constants — `ref(..., "exact")`, `about(S, "sig", ...)` — and the
*argument names* are what an agent needs. A test asserts both directions: every
advertised head names a real predicate at its real arity, and every predicate in
the file is advertised. A rule cannot ship unlisted, and a renamed one cannot
leave a stale catalogue behind.

**The 1500 is a band, not a number, and the output sits at the top of it.**
No tokenizer is available offline and adding one is a dependency for a figure
that gates copy length. Estimates of this text span **~1,400 tokens** at four
characters per token — prose-like, and what an earlier draft of this paragraph
claimed — to **~1,750** under a BPE-shaped count that charges per punctuation
mark and per identifier fragment, which is what dense tabular output actually
looks like. Today: **5,256 characters, 41 rules** on this repository, and the
honest statement is that `schema` is *at* its budget rather than inside it.

That matters because it is now the *only* constraint the standard library
spends against. The RULES section is over half the output, so every rule added
spends copy that is genuinely not there, and it spends it visibly. The CI
assertion is therefore on characters, which can be measured — **6,000, against
a worst-case census rather than a fixture tree** ([00-overview.md](00-overview.md)
§ Surface budget, #42). `schema` embeds live per-kind and per-language counts,
so a ceiling measured against the smallest possible index binds on nothing; the
worst case is every relation, kind, role and shipped language at seven-digit
counts, and it renders 5,490. If the assertion fires, the standard library is
too big — cut rules, not the catalogue.

Two things pay for it, and only one is allowed to. Fixed-width padding is free
to cut: the 44-wide argument column existed for `def_span` alone and the other
fifteen relation rows paid for it, as the 30-wide head column did over forty
rules. A rule is the real currency, and three went for #42 —
`callers_by_name`, `impact_by_name` and `long_def`, each one join or one
comparison a caller writes itself. Buying room by writing worse docs is the
failure mode here, and unlike a count it is visible in the diff.

```
START HERE — you have a location, you need a symbol
  ?- innermost_at("src/store.rs", 142, S).     from ripgrep / git diff /
                                                a stack trace / a compiler error
  ?- def(S, F, _, N), contains(N, "auth").      from a word
  ?- def(S, _, _, "Store"), about(S, Rel, A, B, L).   everything about S, one trip

RELATIONS
  file(F, Lang)
  def(S, F, Kind, Name)
  def_span(S, StartLine, EndLine, StartByte, EndByte)
  scip_ref(S, F, Line, Col, From, Role)     compiler-resolved occurrence
  name_ref(Name, F, Line, Col, From)        unresolved identifier (tier A)
  ...
VALUES   closed sets, with counts from THIS index. A kind at 0 is a kind you
         will get no rows for -- do not query it.                        <gen>
  Kind   function 4,201  class 812  module 96  interface 41
         method 0  struct 0  enum 0  trait 0  constant 0  field 0
         constructor 0  variable 0  macro 0  typealias 0  type 0  unknown 1,340
  Role   def 5,150  read 0  write 0  import 402  test 0
  Prov   name 18,904  exact 0
Generated from the manifest, not printed as a constant. A static list
advertising sixteen kinds when the index holds four is this document's own
invariant 5 broken by its own onboarding text.
RULES
  innermost_at(F, Line, S)       location -> tightest enclosing symbol
  about(S, Rel, A, B, L)         sig|doc|defined|caller|callee|implements|test
  ref(S,F,L,C,From,Role,Prov)    resolved occurrence, either tier
  calls(Caller, Callee)          callable reference, either provenance
  calls_exact(Caller, Callee)    SCIP-resolved only
  impact_of(S, Caller)           transitive callers of S -- SEED THE 1st ARG
  reach_of(S, Callee)            transitive callees of S -- SEED THE 1st ARG
  is_test(F)                     test file, by path or SCIP role
  ...
BUILTINS = != < <= > >= + - * / between/3 match/2 prefix/2 suffix/2 contains/2
           count{X:g}
NOTES
  lines are 1-based; columns are 0-based UTF-8 bytes
  integers and their string forms are different atoms: Line = "42" never matches
  < and > are integers only
  Prov "name" = tree-sitter text matching: method calls x.f() are mostly
    MISSING. For precision use calls_exact / impact_of_exact, and check
    `codeintel status` for whether a SCIP index is present and fresh.
EXAMPLES
  what does this diff hunk affect?
  ?- innermost_at("src/store.rs", 142, S), impact_of(S, C),
     def(C, F, _, N), at(C, F, L), !is_test(F).

  blast radius of a rename, precise only
  ?- def(S, _, _, "get"), impact_of_exact(S, C), def(C, F, _, N), at(C, F, L).

  exported and unreferenced outside its own file
  ?- dead_export(S), def(S, F, _, _), match(F, "^src/").
```

`--format json` carries the same catalogue **structurally**: `relations` with
argument names and row counts, `kinds` and `roles` as the closed vocabulary with
this index's count against every value including the zeros, `rules` with their
signatures, and `text` for the rendered form. A consumer must never regex the
prose to learn that a kind is at zero — that is the same defect as handing
`query` a `display` column and no raw one.

**`schema` carries a `status` like every response, and exits 0 without an
index.** With no index the status is `no-index` and the hint is the
usual `run: codeintel index <root>`: `status=` and `hint:` on stderr for text,
`status` and `hint` fields for `--format json`, and over MCP in whichever field the client reads (§ The consumer).
It used to say `ok` in every form and leave the difference to a sentence inside
the text — invariant 6, which a consumer must be able to branch on without
reading prose. The status goes to stderr in text mode so stdout stays exactly
the catalog the size budget measures.

The catalog itself is still returned in full, and the command **exits 0**,
unlike a `no-index` query. The vocabulary is what an agent reads before anything
is indexed, so the catalog is the answer and it is complete; only the counts
are absent, and the status says so. Exit 2 means "never ran", and failing
`codeintel schema > vocab.txt` on a fresh checkout for printing exactly what was
asked would be that code lying in the other direction. A store that exists but
will not load is still `corrupt` and exits 2.

**`schema` refreshes the way `query` does, and says so when it cannot.** Its
counts are what an agent uses to decide what is worth asking, so they must be
the counts `query` will answer from. It refreshes the index first, within the
same `max_refresh_ms`; a refresh that runs out of budget or cannot take the
writer lock leaves the catalog counted from the index as it stands, with status
`stale` and `query`'s hint. With no index, or one written for another schema,
it does not refresh. `tests/surface.rs` asserts the counts equal what a query
sees on a tree edited since indexing, and that a lock-blocked refresh reports
`stale`. (Before this, `schema` reported `ok` over a tree with 45 files changed,
`def` over 108 files while the next query saw 115.)

### `status`

Index freshness, per-tier. Reports the counts a user needs to trust or distrust
an answer: files indexed, files changed since index, SCIP tool/coverage/
staleness/ambiguous columns, anchor rate, unsupported languages, fact counts per relation.

SCIP coverage is reported the same way `index` reports it: documents matched
against indexed files, with the zero-match line and its `--scip-root`
suggestion, because the state a bug report has to carry is the one `index`
printed an hour ago and nobody kept. The anchor rate is the same line too,
counted the same way (§ `index`). The figures still differ in *scope* — every
count `index` prints is the run it just did, every count `status` prints is the
whole store — so after an incremental reindex the two describe different sets
of files. They never describe the same set two ways.

The verdict uses the checks `query` uses, in this order: `no-index` when there
is nothing here; `stale` when the index was written for another
`schema_version`; `corrupt` when the store will not load; `stale` when a file
was added, removed or changed since the index — changed by the racy-clean rule
of [04-storage.md](04-storage.md) § Manifest, so a same-second, same-length edit
is hashed and seen; `scip-stale` when an indexed file holds bytes the SCIP
inputs did not see; `ok` otherwise. `no-index`, `stale` and `scip-stale` **exit
0**: "there is no index in this directory" is an answer, not a failure of the
command that reported it. `corrupt` exits 2, as it does for `query`. A schema
mismatch or a store that will not load prints the status and its hint and no
counts, because there are none it could read.

Both formats name the resolved index directory (`index:` in text, `index_dir`
in JSON) — where [04-storage.md](04-storage.md) § Location put it, which a
reader repairing a `corrupt` index cannot otherwise work out.

`status` **never adopts a nearby index** (§ The nearest index) — it reports on
the directory named on its command line and nothing else — but its `no-index`
hint is the same one `query` gives: the complete index chain, and any index
discovery found above or below, named by root.

**`status` does not refresh, on purpose.** It is the diagnostic for the index
*as it stands*: a refresh would repair the very staleness it exists to report,
and make the orientation command write to disk. It already reports the
condition — `stale` with the changed files named — so unlike `schema` it was
never silent.

The unsupported-extension count costs a full walk, which is why it lives here
and not on every query. It is printed biggest-first, because the number that
matters is the one that turns out to be a whole unindexed subtree.

`--format json` is the bug-report artifact for a tool with no telemetry. It
carries the extractor fingerprint, the dictionary generation, the SCIP inputs,
per-relation row counts for **every** relation including the zeros — an absent
key would read as "not measured" rather than "empty" — and per-language
`{files, defs, refs, imports}`. "python: 1,204 files, 11 defs" is visibly absurd
to a human in one second; `status: ok` is not.

Per-language attribution goes through the relations that carry a file column
(`def`, `scip_ref`, `name_ref`, `import`) and the manifest's file→language
table. `exported`, `resolved` and the rest have no file column and are reported
as totals only, rather than joined through `def` to invent a language for them.

---

## MCP

**One tool.** Named `code_query`.

```json
{
  "name": "code_query",
  "description": "Query the structural code graph in Datalog. Facts: definitions, spans, containment, imports, references, implements. Call with {\"schema\":true} first to get the relation catalog and examples.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query":  { "type": "string", "description": "Datalog program ending in a ?- goal." },
      "schema": { "type": "boolean", "description": "Return the relation catalog and exit." },
      "limit":  { "type": "integer", "default": 200 },
      "raw":    { "type": "boolean", "description": "SymIds instead of `Name path:line`." }
    }
  }
}
```

Exactly one of `query` or `schema` per call; both is an error naming the two.

**`rule` and `args` are not in this schema.** They were, and they carried the
defect that deleted the `rules` CLI verb: `args` is an array of strings, and an
integer and its string form are different atoms ([01-facts.md](01-facts.md)
§ Integers), so `{"rule": "innermost_at", "args": ["src/store.rs", "142"]}` bound
the *string* `"142"`, matched nothing, and returned `ok` with zero rows. Coercing
digit-only arguments would be the extractor guessing, which invariant 1 forbids,
and would make a symbol genuinely named `142` unaddressable. An agent writing
`?- innermost_at("src/store.rs", 142, S).` gets the integer right because it is
writing Datalog rather than filling positional slots.

**The server stays warm between calls.** `codeintel mcp` is one long-lived
process. Reloading the index on every call cost ~18 ms of a ~40 ms answer on a
799-file tree (#18). The server keeps the loaded engine (relations, the standard
library, symbol sites) and reuses it for as long as the manifest *after refresh*
equals the manifest the engine was built from. A refresh that commits anything,
or another process's `codeintel index`, moves the manifest, and the next call
rebuilds. **Every call still refreshes first.** Warmth skips the load, never the
refresh that keeps an answer current. A call that ends in `corrupt`, `no-index`
or a schema mismatch keeps nothing warm.

**Query literals do not outlive their call.** A query interns constants the
corpus does not contain. A long-lived engine would keep them, so atom ids would
depend on what was asked earlier — and so would which rows a truncated answer's
stable prefix holds ([03-datalog.md](03-datalog.md) § Determinism). The host
therefore gives the engine an **overlay dictionary**: a corpus string resolves to
its stored atom, any other string gets a scratch atom above the dictionary, and
the scratch range is emptied after every call. The CLI builds the same engine
through the same code, once. A warm server and the CLI give byte-identical
answers to the same query on the same index. `crates/datalog` is unchanged: the
overlay is a `Symbols` implementation in this crate.

**Transport is newline-delimited JSON-RPC 2.0 on stdin and stdout, hand-written.**
A message with no `id` is a notification and gets no reply. There is no MCP SDK
dependency: the protocol surface this server needs is `initialize`, `tools/list`,
`tools/call` and `ping`, `serde_json` is already present for the response
contract, and the whole transport is ~150 lines. Recorded in
[research.md](../docs/research.md) §6.

**A bad line is answered, and the session survives it.** A client that sent a
request and gets silence waits forever, so every line that is not a valid
request gets a JSON-RPC error with `id: null`, and the server reads the next
line:

| Line | Error |
|---|---|
| not UTF-8, or not JSON | `-32700` parse error |
| JSON, but not an object — including a batch array | `-32600` invalid request |
| longer than 1 MiB | `-32600` invalid request; the rest of the line is skipped unread into memory |

**Batches are rejected, not served.** The server speaks one request per line;
MCP dropped batching in a later revision, and supporting it would be protocol
surface no client of this tool needs. **`tools/call` names its tool.** A `name`
other than `code_query` — or none — is `-32602` naming the one tool that exists,
rather than running `code_query` under whatever name was sent.

**A result carries one body: `content` or `structuredContent`, by what the
client declared at `initialize`.** The tool result's `content` is the answer as
text — the rows, a `status=` line, any `root=` and `hint:` line — and
`structuredContent` is the same answer as the JSON document of § Response
contract. Exactly one carries it.

*Which one.* MCP has no capability that says "I read `structuredContent`";
the field arrived with revision 2025-06-18, and the declaration a client makes
is the `protocolVersion` it asks for. A client that asks for 2025-06-18 or a
later date is answered with 2025-06-18 and gets `structuredContent`, with an
empty `content` (`[]`: the result schema requires the field, and an empty list
is not the answer sent twice). Every other client — an older revision, a
`protocolVersion` that is not a date, an `initialize` with no params, no
`initialize` at all — is answered with 2024-11-05 as before and gets `content`
only, because `content` is the field every client reads. The choice is made per
connection at `initialize` and is not an argument.

*Why not both.* This was the design — `structuredContent` self-sufficient and
`content` the same answer beside it — and it was reasoned from a measured
client: Claude Code shows the model `structuredContent` and **drops `content`
entirely** whenever `structuredContent` is present (`anthropics/claude-code#55677`,
`#64316`), so a server that kept the answer only in `content` was invisible to
it. The fix for that was to fill both, and filling both put every row on the
wire three times (rows, display, and the text copy of each): a 202-row answer
cost 51,044 bytes against 7,539 on the CLI, and a zero-row answer cost 926
against 0. The visibility failure is cured by sending the field the client
reads, not by sending both. `format` is gone from the tool's arguments for
the same reason: with the body fixed by the client, it was inert for every
client that prefers `structuredContent` and advertised anyway. `raw` stays and
is live in both bodies: it chooses the notation of the rows.

*A finding, not a decision.* On the 202-row answer the text body is 8,009
bytes and the JSON body 9,070 (`tests/fixtures/cost/expected.tsv`). The JSON
body is the larger of the two for the same rows, and the only reason to send it
is that a client reads nothing else. If a client reads `content` when it is the
only field present — which is what the issues above describe — then the
declaration at `initialize` buys a client nothing and costs this server a
state machine. That has not been tested against a live Claude Code and is the
experiment that would settle it.

**An argument the tool does not have is refused.** `code_query` takes `query`,
`schema`, `limit` and `raw`, and nothing else; the input schema says
`additionalProperties: false` and the handler enforces it, because a hand-rolled
transport cannot rely on the client validating. Unknown arguments used to be
ignored, and the one a caller reaches for first is `path` — the CLI takes
`--path`, nothing in the tool description says the repository is fixed, so an
agent generalises. A server started in one repository and asked for
`path: "<another repository>"` answered from its own tree with `status: "ok"`:
a well-formed, confident answer about the wrong codebase, which is exactly the
failure invariant 6 exists to prevent. The repository is the server's working
directory, one server per repository, and the refusal says so.

**The budget bounds the whole result, whichever field carries it.** The cap
is measured on the tool result object as actually serialized
(`crate::wire::Wire::bytes`), through the same `tool_result` that sends it, so
the envelope — `isError`, the empty `content`, and `stats` in the JSON body —
leaves less headroom for rows before a cut (§ Truncation, above).

**The catalog travels the same way.** `schema: true` puts the catalog text in
`content` for a client that reads it. For a client that takes `structuredContent`
it is `{status, hint, text}` there — the same catalog text once, not again as
`rules`, `relations`, `kinds` and `roles`, which are `codeintel schema --format
json`'s restatement of that text for a parser and which no consumer of this tool
is. `isError` is false whenever the catalog was delivered, including `no-index`
(§ `schema`).

**Both surfaces print one answer.** A ground goal that holds prints `true` in
an MCP text result exactly as on the CLI; one function writes the rows for
both, so the two cannot drift again. The MCP result printed a blank line — the
answer nobody can see, which the CLI had already stopped printing.

**An error no status expresses is a JSON-RPC error.** An I/O failure — a full
disk, a permission error — is `-32603` internal error carrying the message: the
MCP counterpart of the CLI's exit 2. It is not a tool result saying `corrupt`,
whose hint is `--rebuild` and would have an agent delete a healthy index.
`corrupt` and `stale` come only from what `crates/facts` refused
([04-storage.md](04-storage.md) § Segment format).

### Response contract

```json
{
  "status": "ok",
  "columns": ["C", "F", "L"],
  "rows": [["handle_read", "src/api/handler.rs", 42]],
  "truncated": false,
  "hint": null,
  "root": null,
  "stats": { "derived": 18422, "refreshed": 0, "transformed": ["impact_of"],
             "demand": "applied", "depends": ["def", "name_ref", "scip_ref"],
             "shadowed": [], "widened": [] }
}
```

- **`stats` on `timeout` and `budget-exceeded` is what the run measured**, not
  defaults: `derived` and `elapsed_ms` at the abort, and the `depends`, `demand`
  and `transformed` that were decided before evaluation began — which is exactly
  when an unexpectedly slow query needs them. A status that fails before
  evaluation (`invalid-query`, `unstratified`) reports the zero values, because
  nothing ran and zero is true.

- **`root` is the root that answered**, and it is `null` exactly when that is
  the root the caller asked about. A path there means the nearest index was
  adopted and the scope of every row moved with it (§ The nearest index). The
  key is always present and `null` is written out, because an absent key reads
  as "not measured" where `null` reads as "the root you named" — the same
  reason `status` reports relations at zero. In text format the line
  `root=<path>` is printed **only** when it is non-null: a redundant line on
  every answer is noise, and the absence of a redirect is what the caller
  already assumed.

- **`hint` is non-null whenever `status != "ok"` OR the result is empty.** On an
  empty result it names the first body literal that matched nothing:
  `"def/4 with Kind=\"method\" matched 0 rows; this index contains 0 method
  definitions"`. As originally specified, `hint` was guaranteed null in exactly
  the case the taxonomy exists to disambiguate — a valid query returning zero
  rows, which is indistinguishable from a typo, a wrong `Kind`, a path that is
  not indexed, and a missing SCIP index. The engine already evaluates
  literal-by-literal, so the first-zero literal is free.
- **When a condition and an empty result coincide, the hint carries both**, as
  `condition. also: emptiness`, and the **status stays the condition**. The two
  are different facts: the condition says what the index could not see, the
  emptiness says which literal stopped the join, and neither implies the other.
  An earlier implementation filled the slot with the condition and skipped the
  emptiness, which inverted the guarantee this contract exists for — a
  `scip-stale` or `no-scip` index is exactly when a caller most needs to tell
  "no" from "I cannot see it", and it was the only state in which the tool
  refused to say. The status stays the condition because the condition is the
  one fact nowhere else in the response, the same reason truncation composes
  rather than overwrites (§ Status taxonomy).

  **Nothing may be appended to the line a runnable chain ends.** A hint's
  condition half often ends in one (§ The no-index hint), and a shell reads
  everything after the last command as more arguments to it. Composing
  `. also: …` onto `scip-python index .` yields `scip-python index .. also: …`,
  and `..` is a valid path: the chain parses, runs, and indexes the parent
  directory. `sh -n` cannot see this — the output is valid shell, which is the
  whole problem — and no in-line separator fixes it, because every one of them
  is just another argument. Only a newline, or nothing following the chain at
  all, terminates a command.

  **The chain therefore comes last** (#104). Composition moves it to the end of
  the composed hint rather than leaving it where the condition put it, so
  nothing can follow it and the defect is impossible rather than fixed. The
  hint stays one line, so the promise above survives verbatim. Both outcomes
  were built and compared before this was taken — the alternative was to give
  the chain its own line, which would have given #102 somewhere to put a
  polyglot block at the cost of a hint that is no longer one line, and of a
  fourth line in a stderr block whose other three are `key=value`.
  `CODEINTEL_HINT_SHAPE` still selects `own-line` or the original `glued`, the
  latter because every hint recorded in `docs/` and in these specs was captured
  under it.

  Testable without a shell, which matters because the broken form is valid
  shell: compose a hint, and assert its `run: ` tail is the tail it carried
  uncomposed. **Cutting is a composition too** — `max_result_bytes` trimming a
  hint can land just after a complete command and hand the truncation marker's
  own words to it as arguments, which is the same defect through a narrower
  door (#108).
- **A constant in a typed column is diagnosed against that column, not against
  the relation.** "the index holds 1448 `def` rows" is true and useless: the
  agent's error was the *value*, and the relation total says nothing about it.
  Two column types carry enough information to say more, and both are the
  subject of a recorded eval failure:
  - **Closed vocabulary** (`Kind`, `Role`, `Prov`, `Lang`) — report that
    column's values with this index's counts. An agent that writes
    `Kind="type"` for a Rust struct needs to see `struct 79` beside `type 1`;
    an agent that writes `Kind="klass"` needs to see that no such value exists.
    Listing the vocabulary is a fact about the index, not a guess about intent,
    so it stays inside invariant 1 — **do not suggest a replacement value.**
  - **Path** (`F`, `G`) — the constant an agent gets wrong most often, because
    paths arrive from `ripgrep`, `git diff` and stack traces, and those emit
    absolute and `./`-prefixed forms while the index keys on repo-relative
    ones. Report, in order: that stripping the repo root or a leading `./`
    yields a path that *is* indexed; else that a file with the same basename is
    indexed elsewhere; else that neither the path nor the basename is known,
    with the file count and a pointer to `status` for what was skipped. All
    three are facts about the index. The basename case in particular separates
    "you named the wrong directory" from "this file is not indexed at all",
    which are different problems with different fixes.
  - **Integer** (`Line`, `Col`, `StartLine`, `EndLine`, `StartByte`, `EndByte`)
    — a quoted constant in one of these can *never* match, because an integer
    and its string form are different atoms. Say so and show the unquoted form.
    This is the footgun the `NOTES` block already warns about, which is
    evidence a warning in the preamble is not where an agent reads it.

  **The host reads the literal the engine parsed, never its text.** `Stats`
  carries the failing literal in both forms — the source slice, which is the
  only form worth showing the caller, and the parsed literal, whose constants
  are interned atoms. Recovering the arguments by splitting the source on
  unquoted commas is parsing Datalog outside the parser, and it is wrong on
  real input: an escaped `\"` reads as closing the string, so one argument
  splits in two, every column after it shifts, and the hint diagnoses a value
  the caller never wrote against a column it never occupied — while the
  constant that actually matched nothing goes unexamined (#81). Naming the
  wrong cause is worse than naming none, which is the whole reason this slot
  exists.

  This stays a `hint` on an `ok` result rather than becoming `invalid-query`.
  The query is well-formed and the answer — no rows — is true. Promoting it to
  an error would make the taxonomy claim a malformed program where there is
  only an empty one, and the taxonomy's whole job is keeping those apart.
- Everything else carries a command the agent can run. `no-index` returns the
  complete chain of § The no-index hint, not "index not found" and not a
  fragment that needs a second lookup.
- On `invalid-query`, `hint` contains the corrected shape where it can be
  inferred — an arity mismatch reports the expected arity. An agent should be
  able to self-correct in one retry.
- Text format wins by default. It is ~4× denser than JSON for tabular results
  and every consumer is a language model.

### The consumer

**There is exactly one, and it is a language model.** No human reads this
output except while debugging the tool itself, and no programmatic consumer
exists or is planned. Where a rule in this spec trades density for the
convenience of a consumer that parses structurally, density wins, because the
context window is the budget that is actually scarce.

Three rules follow, and they supersede the earlier drafts named at each site:

1. **One notation per answer** (§ `query`, Symbol rendering). `rows`, chosen by
   `raw`. Not `rows` *and* `display`.
2. **One body per MCP tool result.** A result carries `content` or
   `structuredContent`, never the same answer in both. Which one is chosen by
   what the client declared at `initialize` (§ MCP); a client that takes
   `structuredContent` gets only that, and one that does not — or declared
   nothing — gets `content`. Sending both made `format` and `raw` inert for
   every client that prefers structured content — the parameters were
   advertised and did nothing — and cost 3717 bytes to deliver a 1131-byte
   answer.
3. **The schema is a budget, not an inventory.** It is read once per session
   before any question is asked, so its bytes are charged to every session
   including the ones that ask one question. It is currently 18,304 bytes.

**These are decisions, not measurements.** The magnitude each one recovers is
the subject of a standing hypothesis in
[docs/50-self-improvement.md](../docs/50-self-improvement.md) § 7, measured by
that document's scoreboard against a pinned corpus. The decision stands on the
consumer being a language model; the number is what the experiment reports.

The byte cap is unaffected in principle and tightened in practice: it is still
measured on the bytes actually sent (§ `query`, the byte cap), which is now a
smaller number for the same rows.

### Why one tool

dex's `internal/mcp` is 17,763 lines and it needed a lint to stop its tool
descriptions from arguing with each other ([research.md](../docs/research.md)
§1a). The failure mode is structural: N tools means N descriptions competing for
the router's attention, and each new question is tempted to become tool N+1.
One tool plus a query language means new questions cost zero surface.

The cost is real and should be stated: the agent must learn Datalog. That is
what `schema: true` is for, and the ≥ 8/10 first-attempt success criterion in
[00-overview.md](00-overview.md) is the test of whether that cost was paid down.

---

## Non-surface

No HTTP server. No daemon. No watch mode in v1 — `index` on a single changed
file is under a second, so the harness can call it from a hook and a file
watcher is a process-lifecycle problem we do not need to own.

No `--explain`, no query plan output beyond `stats`. No interactive REPL. No
output templating. Each of these is a plausible half-day of work that permanently
widens the surface.
