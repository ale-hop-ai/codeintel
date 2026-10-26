---
id: onboarding
status: part implemented; F1 and F2 closed (80e3b59), F3-F9 open; D1, D2 and D3 decided 2026-10-19
binding: the decisions in §7 are binding; the rest is research
---
# Onboarding — install once, run one command, usable

Research for the request: *the user must not have to edit `.gitignore`, must not
have to place indexer output where the tool can find it, the store belongs
outside the tree, worktrees and branches must work, and SCIP must stop being a
manual chore.*

No code was changed. Every claim below cites `path:line`. The open decisions are
at the end; three of them have blast radius and are not mine to pick.

---

## 1. The vision, in one paragraph

> Install the binary. Run `codeintel index` in a repository. Everything else —
> where the store lives, whether this is a worktree, which SCIP indexer the
> languages need, running it, finding its output — is the tool's problem. The
> repository is never written to. The agent registration is done once, globally,
> not per repository.

Two properties fall out of that and both are worth stating as rules, because
they are what stops this from growing into a daemon:

- **The repository is read-only to `codeintel`.** Today it is not (§2).
- **Nothing runs a build behind a query.** Indexers run on `index`, never on the
  auto-refresh that `query` and `mcp` perform. A structural question must never
  block on `cargo`/`tsc`/`go build`.

---

## 2. What friction exists today, measured against that

| # | Friction | Evidence |
|---|---|---|
| F1 | The store lives in the tree: `.codeintel/` under the repo root | **CLOSED** by `80e3b59` (#63). Was `store.rs:23,53`. Now `crates/facts/src/store.rs:25` (`DIR`, "never created") and `:41-73` (`locate`: flag, legacy in-tree dir if present, else the cache dir) |
| F2 | `codeintel index` **edits a tracked file** — appends `.codeintel/` to `.gitignore` | **CLOSED** by `80e3b59` (#63), which deleted `ignore_the_store()` (introduced `9e72fb0`). Nothing in `crates/*/src` writes `.gitignore`; `crates/extract/src/walk.rs:3,83` only reads it. Guarded by `crates/codeintel/tests/location.rs:78-80` ("`.gitignore` must not be touched") |
| F3 | SCIP output is expected at `./index.scip` in the repo root, so it must be ignored too | **STILL TRUE.** `crates/codeintel/src/index.rs:20` (`DEFAULT_SCIP`), `crates/codeintel/src/main.rs:412-422` (default input when the file exists). The indexer writes wherever the user ran it; this repository's own `.gitignore:10` still carries `/index.scip` |
| F4 | Indexers are never run — the tool prints the command and stops | **STILL TRUE.** `crates/codeintel/src/main.rs:527-545` (`report_scip` prints one command per language), `crates/extract/src/lang.rs:221` (`Lang.indexer`). No `std::process::Command` anywhere in `crates/*/src`; `--run-indexers` (D2) is decided, not built |
| F5 | Indexers are never installed, and installing them is per-language and undocumented | **STILL TRUE.** `tasks/` holds no indexer install (no `tasks/indexers.yml`); `tasks/score.yml:19-78` *invokes* `rust-analyzer scip` for its corpora and assumes it is present. README § "You want the SCIP tier" names the commands, not how to get them. #109: the drift check in `tests/fixtures/README.md` § Indexer pins needs these installed; only `rust-analyzer` and `scip-go` are on the dev host |
| F6 | SCIP re-runs are manual; drift is *detected* (`scip-stale`) but nobody acts on it | **STILL TRUE.** `specs/05-surface.md:32` (the status), `crates/codeintel/src/query.rs:1286-1300` (hint names the commands), `crates/codeintel/src/main.rs:595-600` (`index` summary), `crates/codeintel/src/query.rs:1404-1409` (auto-refresh is tier-A only in effect, by design) |
| F7 | First query on a fresh clone is `no-index`, not an answer | `crates/codeintel/src/query.rs:269-272` (`Status::NoIndex`, `discover::no_index_hint`) |
| F8 | MCP registration is per repository (`--path /your/repo`) | `README.md` § Install, `crates/codeintel/src/main.rs:117-123` (`Mcp { path }`, default `.`) |
| F9 | Installing `codeintel` itself needs a Rust toolchain and a checkout of this repo | `README.md` § Install, `docs/plan.md:128` |

**Status, re-verified 2026-10-20 against the tree.** F1 and F2 are closed by
`80e3b59` — the store moved out of the tree (§7 D1) and `ignore_the_store()`
was deleted with it. F3 is **not** closed: the earlier note here said it was,
but `./index.scip` is still the default input and nothing moves indexer output
into the store (that waits on `--run-indexers`, §4.2). F4 is decided, not
built: §7 D2 makes `--run-indexers` opt-in, and today the command is only
printed. F5 and F6 still hold. F7 through F9 are outside this pass and are
unchecked. **2026-10-26:** D2 was reopened and built — `index` runs the
indexers by default into `<store>/scip/`, which closes F3 for every tree
without a hand-built `./index.scip`, closes F4, and closes F6 for the inputs
`index` produced (it re-runs a stale indexer itself). The table is left as
verified on 2026-10-20. The §2 table cites the lines read on 2026-10-20, re-pointed on 2026-10-21 (#126) where `query.rs` and the README had moved under them; the
paragraphs below it are left as written because they are the evidence the
decisions were made against.

F2 is the sharpest one and is worth spelling out: `.gitignore` is a **tracked**
file. `codeintel index` therefore dirties `git status`, can be committed by
accident, and produces a rebase conflict in any repo that already manages its
ignore list. In a git worktree it dirties *that branch's* tree. A tool whose
entire storage story is "the index is 100% derived" has no business writing to
version control. This repository's own `.gitignore` carries two lines
(`.codeintel/` and `/index.scip`) that exist only because of F1 and F3.

---

## 3. Where the store goes

### 3.1 Layout

> **Superseded by §7 D1.** The shipped layout is `$XDG_CACHE_HOME/codeintel/
> <hash>/`, resolved by `crates/facts/src/store.rs:25-73`, not `~/.codeintel`.
> The env var is `CODEINTEL_INDEX_DIR`, not `CODEINTEL_HOME`. What survives
> from this section unchanged is §3.2 (the key is the working tree path, never
> the branch), §3.3 (reverse root check, report-never-delete orphans) and the
> deferred per-repository segment pool.

```
$CODEINTEL_HOME                 default: ~/.codeintel     (env override, clap already has `env`)
  projects/
    <slug>-<hash12>/            one directory per working tree
      manifest.json
      dict.bin  dict.idx
      seg/<blake3>.bin
      lock
      scip/<lang>.scip          indexer output, §4
      scip/<lang>.log           last indexer run's stderr
```

- `hash12` = first 12 hex of `blake3(canonicalized root path)`. This is the
  identity. It is what makes two repos both called `api` distinct.
- `slug` = the last path component, sanitized. It is **decoration for humans
  reading `ls`** and carries no meaning. For a git worktree it renders
  `<repo>@<worktree-name>` so the listing is legible.
- No new dependency. `std::env::var("CODEINTEL_HOME")`, else
  `std::env::home_dir()` — un-deprecated since 1.86 and the toolchain is pinned
  at 1.98.1 (`rust-toolchain.toml`). `blake3` is already in the budget
  (`docs/research.md:379`).

Not `$XDG_CACHE_HOME` / `~/Library/Caches` by default. The store is a cache by
nature but a *slow, expensive, user-visible* one, and the request named
`~/.codeintel`. `CODEINTEL_HOME` is the escape hatch for anyone who disagrees,
and it is the same hatch CI needs.

### 3.2 The key is the working tree path. Never the branch.

This is the whole worktree/branch answer and it is smaller than it looks.

**Branches need no state at all.** The index describes *the bytes on disk*,
which include uncommitted edits. A branch-keyed index would be wrong the instant
someone edits a file. `git switch` is just a large incremental refresh — every
changed file's mtime moves, the existing machinery re-extracts exactly those
(`specs/04-storage.md` § Incremental reindex). That already works. Do not build
per-branch anything.

**Worktrees need nothing either**, because a worktree is a distinct path and
therefore a distinct key, automatically. `~/worktrees/codeintel/feat-x` and the
main checkout get separate stores, separate locks, and no interference. This is
the correct semantics: two working trees hold different bytes at the same
relative paths, so they cannot share one manifest.

What they *could* share is the expensive part, and this is worth recording as
deferred rather than built:

> **Deferred: a per-repository dictionary + segment pool.** Segments are
> content-addressed (`seg/<blake3-of-bytes>.bin`, `specs/04-storage.md`
> § Layout), so two worktrees of one repo that differ in 40 files would produce
> ~identical segment sets. Keying the *store directory* on the git common dir
> (readable without invoking git: a worktree's `.git` is a file containing
> `gitdir: …/.git/worktrees/<name>`) while keeping **one manifest per working
> tree** would make the second worktree nearly free in time and disk.
> **Do not build it until someone measures the cost of N worktrees.** The
> hazards are real: one writer lock shared across all worktrees of a repo,
> `--rebuild` renumbering atoms under manifests it does not own
> (`dict_generation` exists for exactly this, `specs/04-storage.md` § Manifest),
> and a repo-scoped `rm -rf` blowing away four trees' indexes.
> **Trigger: a user keeps ≥3 live worktrees and index time or disk is measured
> as a problem.**

### 3.3 Collisions, moves, and rot

- **Reverse check on open.** `manifest.roots[0]` already records the root
  (`specs/04-storage.md` § Manifest). If it does not equal the requested root,
  the directory belongs to a different tree — a deleted path reused, or the
  1-in-2^48 collision. Treat as `corrupt` with the store path in the hint, or
  rebuild silently. Cheap: one string compare on a field already loaded.
- **A moved or renamed repo** gets a new key and re-indexes from scratch. That
  is correct and costs one index run; the old directory becomes an orphan.
- **Orphan reclaim.** `~/.codeintel/projects/` grows as trees are deleted.
  Policy: `codeintel status` reports total store size and any project directory
  whose recorded root no longer exists, with the `rm -rf` line. **Report, never
  delete** — the same rule the tool already follows for `.gitignore` is the one
  it should follow for its own files, and a tool that deletes directories under
  `$HOME` on a heuristic is a tool nobody runs twice. No new verb: `status` is
  the reporting verb (`specs/05-surface.md` § `status`).

### 3.4 What this deletes

`ignore_the_store()` and its caller and its test go away
(`crates/codeintel/src/index.rs:484-503`, `main.rs:307-309`,
`crates/codeintel/tests/cli.rs:402-426`). Net negative Rust. That is the shape a
change to this repo is supposed to have.

### 3.5 Spec ripples, if this is approved

Not optional and not small — every one of these is a spec edit *before* code:

- `specs/00-overview.md` invariant 5 says "`rm -rf .codeintel` is always a valid
  repair". Becomes path-agnostic; the *hint* prints the resolved directory.
- `specs/04-storage.md` § Layout, § Concurrency, and the `.codeintel/`-excluded-
  from-file-sync note all name the in-tree path.
- `specs/05-surface.md:29,37` — the `no-index` and `corrupt` hints.
- Every literal `rm -rf .codeintel` in code and tests
  (`crates/codeintel/src/census.rs:276`, `crates/facts/tests/intern.rs:150`).
- `status` gains one line: **where the store is.** Non-negotiable. Moving the
  store out of sight without telling anyone where it went trades one friction
  for a worse one.
- `crates/extract/src/walk.rs:81` keeps skipping `.codeintel` — a legacy
  directory left in a tree must not be indexed as source.
- README § Install, and the two `.gitignore` lines in this repo.

**Migration: none.** The store is derived and pre-1.0. `codeintel index` that
sees a legacy in-tree `.codeintel/` prints one line saying it is no longer used
and to `rm -rf` it. It does not read it and does not delete it.

---

## 4. SCIP

### 4.1 The actual problem

Tier B needs three things to happen and today the user owns all three: install an
indexer, run it, re-run it when the tree moves. The tool owns only the fourth
(ingest). The cost of the gap is not cosmetic — without tier B every reference is
provenance `name`, and `impact_of`, the README's headline query, is `no-scip`.

### 4.2 Layer 1 — run the indexer (`--run-indexers`, trigger now met)

`docs/plan.md:1254` already records `--run-indexers` as deferred with the
trigger *"users report that printing the command is not enough"*. That is this
request. Design:

- **Detection reuses what exists.** `Lang` already carries the canonical command
  (`crates/extract/src/lang.rs:217`) and the refresh already knows which
  languages the tree holds (`crates/codeintel/src/query.rs:1004-1013`). No
  project-marker sniffing needed for v1: index tier A first, then run an indexer
  for each language actually found.
- **Output goes to the store**, `<store>/scip/<lang>.scip`, not the repo root.
  F3 dies. `./index.scip` stays supported as an input for CI-produced indexes.
- **Re-run policy is free.** Run an indexer only when that language has ≥1
  `scip-stale` file, which the manifest already computes via `scip_hash`
  (`specs/04-storage.md` § Manifest). No new state, no timestamp heuristics.
- **Never on the query path.** `index` only. `query`/`mcp` auto-refresh must not
  spawn a compiler.
- **A failing indexer is a degradation, not an error.** Exit code and the last
  lines of stderr go in the `index` summary, full log to
  `<store>/scip/<lang>.log`, tier A answers stand, status stays `no-scip`. Same
  for a wall-clock cap (indexers take minutes; the cap must be reported, per
  invariant 7, never silent).
- **Zero new dependencies** — `std::process::Command`.
- **Honest note for the spec:** auto-running makes index *content* depend on
  which indexer version is installed. That is already true today for anyone who
  runs the command by hand; it just becomes the tool's statement rather than the
  user's. Determinism (invariant 8) is per-repo-state-and-query, unaffected.

Default on or off is a decision, not a detail — see §7.

### 4.3 Layer 2 — installing indexers is `provision`'s job, not Rust's

`tasks/indexers.yml`, alongside `tasks/install.yml`. Declarative, idempotent,
the user's own tool, zero Rust, zero dependencies, zero surface. Covers
`scip-go`, `scip-python`, `scip-typescript`, `rust-analyzer`.

`codeintel` must **never** download a binary. That is network access, a platform
matrix, and a supply-chain surface, inside a tool whose pitch is that it needs no
build and no network. The missing-indexer hint gains one line naming the task.

### 4.4 Layer 3 — nothing

There is no third layer. Tier A plus honest `name` provenance is the product;
SCIP is the precision upgrade. Do not invest past making it one command.

---

## 5. The last mile: install, register, first answer

- **F9, installing the binary.** `provision apply tasks/install.yml` requires a
  checkout. Add `cargo install --git <url> codeintel` to the README as the
  user-facing path (`docs/plan.md:153` rejected `cargo install` for the *dev*
  loop, for a reason that does not apply to users), and consider pre-built
  release binaries later. Docs change, not a code change.
- **F8, MCP registration.** With the store keyed on the tree, one **global**
  registration works everywhere, because `--path` already defaults to `.` and
  agent clients launch MCP servers with cwd set to the project
  (`crates/codeintel/src/main.rs:84`). Recommended line for the README:
  `claude mcp add --scope user codeintel -- codeintel mcp`. Optionally
  `tasks/mcp-register.yml`. No sixth verb — `docs/plan.md:1263` deferred
  "shipping an MCP registration step" and a provision task honours that.
- **F7, the first query.** See §7, decision 3.

---

## 6. Proposed sequencing

Each phase is independently shippable and independently revertible.

| Phase | Content | Surface cost |
|---|---|---|
| **P0** | Specs first: `04-storage` § Layout, `00-overview` invariant 5, `05-surface` hints. Nothing compiles until these read correctly. | — |
| **P1** | Store resolution → `$CODEINTEL_HOME/projects/<slug>-<hash12>`; `Store::open` takes a resolved dir; delete `ignore_the_store`; `status` prints the store path | 0 verbs, 0 deps, net −LOC |
| **P2** | Orphan/size reporting in `status`; reverse root check on open | 0 verbs |
| **P3** | `tasks/indexers.yml` + README install/registration lines | 0 code |
| **P4** | indexers run by `index` (default on, `--no-scip` off — D2 reopened 2026-10-26), output into the store, stale-driven re-run, failure reporting | 1 flag, 0 verbs, 0 deps |
| **P5** | *(decision 3)* first-query bootstrap | see §7 |

P1+P3 alone deliver most of the request: nothing is written to the repo, and one
`provision` command makes tier B reachable.

---

## 7. Decisions

**D1 — default store root. DECIDED and SHIPPED, against this document's
recommendation.** The store is not `~/.codeintel`. It resolves flag →
`CODEINTEL_INDEX_DIR` → `$XDG_CACHE_HOME`/`$HOME` cache → an existing
`<root>/.codeintel/` for trees that already have one
(`crates/facts/src/store.rs:25-73`). The cache dir won because the store is
derived, reproducible and expendable, which is what a cache dir is for, and
because every backup and sync tool already knows to skip it. `codeintel status`
prints the resolved path, which was the non-negotiable part and is honoured.
F1 and F2 are closed by this: nothing is written to the tree, and
`ignore_the_store()` is gone.

**D2 — does `codeintel index` run indexers by default? DECIDED: yes
(reopened by the owner 2026-10-26; first decided "no, opt-in
`--run-indexers`").** The first decision's reason was cost and invisibility:
on-by-default turns a two-second command into a multi-minute one on first run,
spawns a build tool the user did not ask for, and an agent waiting four minutes
cannot tell a slow indexer from a hung one. The reopening keeps the cost and
answers the rest: a progress line and log path before each program starts, a
reported 600 s timeout, and `--no-scip` as the way out. What it buys is the
second round trip of #59 gone for good — one command builds both tiers — and
the whole #102 chain with it. Binding, as built
([specs/02-extraction.md](../specs/02-extraction.md) § Acquisition):

- runs only the SCIP indexer in the static `Lang` table, never a build, test,
  task runner or install — a narrow carve-out to the non-goal in
  [specs/00-overview.md](../specs/00-overview.md);
- skipped when `--scip` is given or `./index.scip` exists;
- output to `<store>/scip/<program>.scip`, never the repository;
- re-runs only a program whose output is missing or older than a file of its
  languages (mtime, the cheap direction — not `scip_hash`, which needs every
  file hashed before the decision);
- **never on the query path** — `index` only, never the auto-refresh in
  `query`/`mcp`;
- a failing indexer is a degradation, not an error: outcome and log path in the
  `index` summary, recorded in the manifest, tier A answers stand;
- a wall-clock cap is reported, never silent (invariant 5);
- zero new dependencies, `std::process::Command`;
- `codeintel` never downloads a binary. A missing one is reported; installing
  indexers is the host's job (`provision`, per §4.3).

**D3 — does the first query bootstrap an index? DECIDED: no (option a), with
the cheaper half of option (b) shipped.** Recorded 2026-10-20; the decision was
taken on #59 (closed 2026-10-19), and this page had not caught up. The `no-index`
hint is one complete chained line (`da19139`), and a bootstrap on the query path
is declined, not deferred: it breaks §1's rule that nothing runs a build behind a
query, and it risks an agent answering from a half-built index, which is a
confidently wrong answer rather than a missing one. #61 also made the commonest
real `no-index` miss — an index in a subdirectory — resolve by itself. No issue
is needed; there is nothing left to decide.
