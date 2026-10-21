//! Answering one query.
//!
//! Auto-refresh, then load, then evaluate. An agent's loop is edit → ask, and
//! answering that from a stale index produces a confidently wrong answer — the
//! worst failure this tool has, because it looks like a right one
//! (`specs/05-surface.md` § `query`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use datalog::ast::Literal;
use datalog::atom::is_int;
use datalog::{EmptyAt, Engine, Limits, Relation, Term};
use facts::{Lock, Store};

use crate::discover::{self, Nearby};
use crate::index::{self, Plan};
use crate::overlay::{Literals, Overlay};
use crate::render::{Row, Sites};
use crate::status::Status;
use crate::wire::Wire;
use crate::{Regexes, render, schema};

/// How long auto-refresh may take before the answer is `stale`.
pub const MAX_REFRESH_MS: u64 = 2_000;

/// True when only tier B can populate the base relation `name`.
///
/// A goal whose dependency **closure** reaches one of these, against an index
/// with no SCIP, gets `no-scip` rather than `ok` with zero rows. The closure,
/// not the literal syntax: `?- impact_of(S, C).` mentions none of them, and the
/// README's own headline query would otherwise return `ok` and nothing on a
/// fresh install — the exact failure invariant 6 exists to prevent
/// (`docs/plan.md` M3).
///
/// `implements` is deliberately absent. Since schema 2 it is a rule with a
/// tier-A `name` branch over `name_impl`, so it answers without SCIP, and
/// `no-scip` would disclaim rows the index really has. `scip_impl`, the base
/// relation beneath it, stays: a query naming that one directly is asking for
/// the compiler's answer specifically.
///
/// **Read off `facts::RELATIONS`, not kept beside it.** This was a hand-written
/// list of four names in this file, which a relation added to the schema could
/// fall out of without a compiler or a test noticing — and the symptom was
/// `ok` with zero rows, the one failure invariant 6 exists to prevent (#48).
/// `Rel::scip_only` has no default, so the question now has to be answered
/// where the relation is declared.
#[must_use]
pub fn scip_backed(name: &str) -> bool {
    facts::schema::by_name(name).is_some_and(|rel| rel.scip_only)
}

/// How much larger the engine's raw-atom byte budget is than the printed one.
///
/// The engine caps the bytes *it* materialises, measured over raw atoms; this
/// host prints a strictly shorter form. The multiplier keeps the engine's cap
/// from being the one that fires — which would truncate rows that fit the
/// printed budget — while still bounding materialisation, since a caller may
/// raise `--limit` without bound.
const RAW_BYTE_HEADROOM: usize = 8;

/// What the caller asked for.
#[derive(Debug, Clone)]
pub struct Options {
    /// Rows before truncation.
    pub limit: usize,
    /// Skip auto-refresh, for benchmarking or a deliberately pinned index.
    pub no_refresh: bool,
    /// Print the underlying atoms instead of `Name path:line`.
    ///
    /// What round-trips: the output of one query pasted into the next query's
    /// literal. The rendered form is for reading, not for feeding back.
    pub raw: bool,
    /// Extra rule files, loaded after `stdlib.dl` and before the goal.
    ///
    /// This is where a repository keeps its own conformance rules — layering,
    /// banned dependencies, allowed directions — so they live in the repository
    /// under review rather than in this binary.
    ///
    /// **Clauses here are additive, not replacements.** A predicate is the
    /// union of its clauses, so a file defining `is_test` *widens* it rather
    /// than redefining it. Only a rule written in the query program itself
    /// shadows a loaded one, and that shadowing is reported in
    /// [`Answer::shadowed`]. A repository that means to replace a stdlib rule
    /// puts it in the program, not in a rule file.
    pub rules: Vec<PathBuf>,
    /// The index directory, if the caller named one (`--index-dir`, or
    /// `CODEINTEL_INDEX_DIR`). `None` takes the default
    /// (`specs/04-storage.md` § Location).
    pub index_dir: Option<PathBuf>,
    /// Where the answer is sent. `max_result_bytes` is measured on it
    /// (`specs/05-surface.md` § Symbol rendering).
    pub wire: Wire,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            limit: Limits::default().max_result_rows,
            no_refresh: false,
            raw: false,
            rules: Vec::new(),
            index_dir: None,
            wire: Wire::Text,
        }
    }
}

/// One response. Mirrors the MCP contract in `specs/05-surface.md`.
#[derive(Debug, Clone)]
pub struct Answer {
    /// What happened.
    pub status: Status,
    /// Variable names from the goal, in order of first appearance.
    pub columns: Vec<String>,
    /// The rows, in both notations, ordered once by the display text. The two
    /// notations of a row describe the same tuple at the same index.
    pub rows: Vec<Row>,
    /// True when a cap dropped rows.
    pub truncated: bool,
    /// Which cap fired.
    pub cap: Option<datalog::Cap>,
    /// Non-null whenever `status != ok`, and always actionable.
    pub hint: Option<String>,
    /// The root that actually answered, when it is **not** the one the caller
    /// asked about.
    ///
    /// `None` means "the root you named". A path means the requested root had
    /// no index, the nearest one was adopted, and the scope of every row moved
    /// with it (`specs/05-surface.md` § The nearest index). A consumer
    /// branching on this learns the scope moved without reading English, which
    /// is what keeps adoption from being a silent degradation (invariant 6).
    pub root: Option<String>,
    /// Tuples derived.
    pub derived: u64,
    /// Milliseconds spent evaluating.
    pub elapsed_ms: u64,
    /// Files re-extracted before the query ran.
    pub refreshed: usize,
    /// Predicates demand transformation rewrote. Empty means it did not apply,
    /// which on a seeded recursive rule is the difference between a seeded
    /// traversal and all-pairs reachability.
    pub transformed: Vec<String>,
    /// Why `transformed` holds what it holds (`datalog::Stats::demand`).
    pub demand: datalog::Demand,
    /// Base relations the goal's dependency closure reaches.
    pub depends: Vec<String>,
    /// Stdlib rules the query itself shadowed. Never silent: a rule quietly
    /// replacing `is_test` would change every answer that reads it.
    pub shadowed: Vec<String>,
    /// Stdlib predicates a `--rules` file added clauses to, as `name/arity`.
    ///
    /// Separate from [`Self::shadowed`] because the mechanism is the opposite
    /// one and only one of them was ever reported. A `--rules` file is
    /// **additive** — a predicate is the union of its clauses — so a file
    /// naming `callable/1` does not replace the stdlib's definition, it widens
    /// it, and every rule reading `callable` changes with it. Measured on
    /// `tests/fixtures/rust`: a rules file adding three `callable` clauses took
    /// `?- calls(A, B).` from 3 edges to 15 and `?- entrypoint(S).` from 16 to
    /// 22, with `shadowed: []` and `status: ok`. Widening is the one a
    /// repository actually does, since `--rules` is where its conformance rules
    /// live, and it was the silent one.
    pub widened: Vec<String>,
    /// True when the engine evaluated the goal. A `stale` answer for a schema
    /// mismatch carries `status.answered()` and zero rows without one having
    /// run, so the status alone cannot say whether empty means "not true of
    /// your code" (`specs/05-surface.md` § `query`, `--expect-empty`).
    pub evaluated: bool,
}

impl Answer {
    fn of(status: Status, hint: impl Into<String>) -> Self {
        Self {
            status,
            columns: Vec::new(),
            rows: Vec::new(),
            truncated: false,
            cap: None,
            hint: Some(hint.into()),
            root: None,
            derived: 0,
            elapsed_ms: 0,
            refreshed: 0,
            transformed: Vec::new(),
            demand: datalog::Demand::default(),
            depends: Vec::new(),
            shadowed: Vec::new(),
            widened: Vec::new(),
            evaluated: false,
        }
    }
}

/// Evaluate `program` against the index under `root`.
///
/// # Errors
/// I/O failure that is not expressible as a status. A query that is wrong, an
/// index that is missing, and a lock someone else holds are all answers.
pub fn run(root: &Path, program: &str, options: &Options) -> Result<Answer> {
    Warm::default().run(root, program, options)
}

/// Answers queries against one repository, keeping the loaded index between
/// calls for as long as its manifest does not move.
///
/// `codeintel mcp` holds one for the life of the process. The CLI builds one
/// per invocation through [`run`], so both answer through the same code
/// (`specs/05-surface.md` § MCP).
#[derive(Default)]
pub struct Warm {
    loaded: Option<Loaded>,
}

impl std::fmt::Debug for Warm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Warm")
            .field("loaded", &self.loaded.is_some())
            .finish()
    }
}

impl Warm {
    /// Refresh, then evaluate `program` — against the loaded index if the
    /// manifest has not moved since it was loaded, against a fresh load if it
    /// has.
    ///
    /// # Errors
    /// As [`run`].
    pub fn run(&mut self, root: &Path, program: &str, options: &Options) -> Result<Answer> {
        let mut answer = self.answer(root, program, options, true)?;
        // Every answer, a diagnostic and `no-index` included: the budget bounds
        // what is sent, not only its rows (`specs/05-surface.md` § Symbol
        // rendering).
        fit_hint(&mut answer, Limits::default().max_result_bytes, |a| {
            sent(a, options)
        });
        Ok(answer)
    }

    /// `adopt` is false on the one recursive call this makes, so the nearest
    /// index is looked for once and never from the root that was adopted.
    fn answer(
        &mut self,
        root: &Path,
        program: &str,
        options: &Options,
        adopt: bool,
    ) -> Result<Answer> {
        let dir = facts::store::locate(root, options.index_dir.as_deref())
            .context("locating the index")?;
        let mut store = match open(root, &dir)? {
            Ok(store) => store,
            Err(answer) => {
                self.loaded = None;
                return Ok(answer);
            }
        };
        if !store.has_index() {
            self.loaded = None;
            // The caller does not choose the root — `codeintel mcp` is launched
            // with whatever directory the session opened in — so "wrong
            // directory" is not an answer they can act on (#61).
            let here = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            let nearby = if adopt {
                discover::around(&here, options.index_dir.as_deref())
            } else {
                Nearby::none()
            };
            if let Some(adopted) = nearby.adopt() {
                let adopted = adopted.to_path_buf();
                return self.adopted(&here, &adopted, program, options);
            }
            return Ok(Answer::of(
                Status::NoIndex,
                discover::no_index_hint(&here, &nearby),
            ));
        }
        if let Some(hint) = stale_schema(store.manifest(), root) {
            self.loaded = None;
            return Ok(Answer::of(Status::Stale, hint));
        }

        // Every call refreshes. Warmth skips the load, never the refresh that
        // keeps the answer current.
        let (status, hint, refreshed) = refresh(&mut store, root, options)?;
        // A manifest that turned corrupt between the two reads: there is
        // nothing to evaluate against.
        if !status.answered() {
            self.loaded = None;
            return Ok(Answer::of(status, hint.unwrap_or_default()));
        }
        let current = self
            .loaded
            .take()
            .filter(|l| l.manifest == *store.manifest() && l.rules == options.rules);
        let mut loaded = if let Some(loaded) = current {
            loaded
        } else {
            let (store, relations) = match load(store, root)? {
                Ok(loaded) => loaded,
                Err(answer) => return Ok(answer),
            };
            match Loaded::of(store, relations, &options.rules)? {
                Ok(loaded) => loaded,
                Err(answer) => return Ok(answer),
            }
        };
        let answer = evaluate(&mut loaded, program, options, status, hint, refreshed);
        // The literals this call interned go with it, so the next call is given
        // the ids a freshly loaded engine would give it.
        loaded.literals.discard();
        self.loaded = Some(loaded);
        Ok(answer)
    }

    /// Answer from `adopted` instead of `requested`, and say so.
    ///
    /// The status stays the adopted index's own condition — it can be `stale`,
    /// `no-scip` or `scip-stale`, and those are facts nowhere else in the
    /// response. The redirect rides in `root`, structurally, for the same
    /// reason truncation composes rather than overwrites
    /// (`specs/05-surface.md` § The nearest index).
    fn adopted(
        &mut self,
        requested: &Path,
        adopted: &Path,
        program: &str,
        options: &Options,
    ) -> Result<Answer> {
        let mut answer = self.answer(adopted, program, options, false)?;
        answer.root = Some(adopted.display().to_string());
        let note = discover::adopted_hint(requested, adopted);
        answer.hint = Some(match answer.hint.take() {
            // The note ends in a chain and `existing` follows it, so this is
            // the third #104 site and the only one with no stop at all.
            Some(existing) => compose(&note, &existing, " also: "),
            None => note,
        });
        Ok(answer)
    }
}

/// An index loaded into an engine, and what answering needs beside it.
struct Loaded {
    /// The manifest this was built from. A refresh that moves it retires this.
    manifest: facts::Manifest,
    /// The rule files loaded after the standard library.
    rules: Vec<PathBuf>,
    /// Stdlib predicates those files added clauses to (`Answer::widened`).
    /// Computed at load, not per query, because it is a property of the rule
    /// set and the warm engine reuses it.
    widened: Vec<String>,
    engine: Engine,
    literals: Literals,
    relations: BTreeMap<&'static str, Relation>,
    sites: Sites,
    scip: ScipState,
    langs: Vec<&'static str>,
}

impl Loaded {
    /// Load `store` into a fresh engine with the standard library and `rules`.
    ///
    /// The inner `Err` is an answer — a rule file that does not load — and
    /// keeps nothing warm.
    fn of(store: Store, relations: Relations, rules: &[PathBuf]) -> Result<Result<Self, Answer>> {
        let scip = ScipState::of(store.manifest());
        let langs = indexers(store.manifest());

        let sites = Sites::of(&relations);
        let (interner, manifest) = store.into_parts();
        let (overlay, literals) = Overlay::new(interner);
        let mut engine = Engine::new(Box::new(overlay)).with_regexes(Box::new(Regexes::new()));
        // Declare every base relation, present or not. A relation with no rows is
        // "this is not true of your code"; an undeclared one would be a query
        // error, and the two are not the same answer.
        for rel in facts::RELATIONS {
            engine.insert_relation(
                rel.name,
                relations
                    .get(rel.name)
                    .cloned()
                    .unwrap_or_else(|| Relation::new(rel.arity)),
            );
        }
        engine
            .load_rules(crate::STDLIB)
            .map_err(|d| anyhow::anyhow!("rules/stdlib.dl does not load: {}", d.message))?;
        // A repository's own conformance rules. Additive: a predicate is the
        // union of its clauses, so a file defining `is_test` widens it. Only a
        // rule in the query program shadows a loaded one.
        //
        // Additive is the right semantics and it was the unreported one. A file
        // that names a stdlib predicate changes every rule reading it, and
        // nothing said so, so the heads are snapshotted here and compared after
        // each file (`Answer::widened`).
        let mut defined: BTreeSet<String> = engine.loaded().iter().flatten().cloned().collect();
        let mut widened: Vec<String> = Vec::new();
        for path in rules {
            let src = match std::fs::read_to_string(path) {
                Ok(src) => src,
                Err(e) => {
                    return Ok(Err(Answer::of(
                        Status::InvalidQuery,
                        format!("{}: {e}", path.display()),
                    )));
                }
            };
            if let Err(diagnostic) = engine.load_rules(&src) {
                return Ok(Err(Answer::of(
                    Status::of(diagnostic.status),
                    format!(
                        "{}: {}{}",
                        path.display(),
                        diagnostic.message,
                        host_hint(&diagnostic)
                    ),
                )));
            }
            // What this file defined, against everything defined before it.
            // `defined` grows as files load, so a second rule file widening the
            // first is reported too, not only a collision with the stdlib.
            //
            // The file's heads are collected BEFORE any of them joins
            // `defined`. Testing and inserting in one pass made a predicate's
            // own second clause collide with its first, so every new
            // multi-clause rule — which is what a layering policy is — was
            // reported as widening a stdlib rule that did not exist (#76).
            // A set also collapses three clauses of one predicate into the one
            // widening they are.
            let heads: BTreeSet<String> = engine
                .loaded()
                .last()
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            for head in &heads {
                let entry = format!("{head} ({})", path.display());
                if defined.contains(head) && !widened.contains(&entry) {
                    widened.push(entry);
                }
            }
            defined.extend(heads);
        }
        // The rules' constants belong to the engine, not to any one call.
        literals.seal();

        Ok(Ok(Self {
            manifest,
            rules: rules.to_vec(),
            widened,
            engine,
            literals,
            relations,
            sites,
            scip,
            langs,
        }))
    }
}

/// The hint for an index written by a build that speaks another schema, or
/// `None`. `status` asks the same question (`specs/05-surface.md` § `status`).
#[must_use]
pub fn stale_schema(manifest: &facts::Manifest, root: &Path) -> Option<String> {
    manifest.is_stale_schema().then(|| {
        format!(
            "this index was written for schema_version {}, this build speaks {}. run: codeintel \
             index {} --rebuild",
            manifest.schema_version,
            facts::SCHEMA_VERSION,
            root.display()
        )
    })
}

/// Base relations by name, as a store loads them.
type Relations = BTreeMap<&'static str, Relation>;

/// Open the store for `root` held in `dir`, or the answer a refused one
/// deserves.
fn open(root: &Path, dir: &Path) -> Result<Result<Store, Answer>> {
    match Store::open(root, dir, &extract::fingerprint()) {
        Ok(store) => Ok(Ok(store)),
        Err(error) => refused(error, "opening the index").map(Err),
    }
}

/// What `crates/facts` refused, as the answer it deserves; an I/O failure, as
/// the error it is. Only a refusal is `corrupt` or `stale`: a permission error
/// answered `corrupt` would have an agent delete a healthy index
/// (`specs/04-storage.md` § Segment format).
fn refused(error: std::io::Error, doing: &str) -> Result<Answer> {
    match facts::fault(&error) {
        Some(fault) => Ok(Answer::of(Status::of_fault(fault), error.to_string())),
        None => Err(anyhow::Error::new(error).context(doing.to_string())),
    }
}

/// Read the segments `store`'s manifest names.
///
/// The writer lock is released by now, so a refresh in another process may
/// have committed and unlinked a segment this manifest still names. A missing
/// segment re-reads the manifest: moved, load the new one, once; moved again,
/// `stale`; never moved, `corrupt`, because nothing replaced it
/// (`specs/04-storage.md` § Concurrency).
fn load(mut store: Store, root: &Path) -> Result<Result<(Store, Relations), Answer>> {
    let mut retried = false;
    loop {
        let error = match store.load() {
            Ok(relations) => return Ok(Ok((store, relations))),
            Err(error) => error,
        };
        if error.kind() != std::io::ErrorKind::NotFound {
            return refused(error, "loading the index").map(Err);
        }
        let reopened = match open(root, store.dir())? {
            Ok(reopened) => reopened,
            Err(answer) => return Ok(Err(answer)),
        };
        if reopened.manifest() == store.manifest() {
            return Ok(Err(Answer::of(Status::Corrupt, error.to_string())));
        }
        if retried {
            return Ok(Err(Answer::of(
                Status::Stale,
                format!(
                    "the index moved twice while this answer was loading ({error}). ask again, \
                     or run: codeintel index {}",
                    root.display()
                ),
            )));
        }
        retried = true;
        store = reopened;
    }
}

/// Evaluate `program` against a loaded index and shape the answer.
fn evaluate(
    loaded: &mut Loaded,
    program: &str,
    options: &Options,
    mut status: Status,
    mut hint: Option<String>,
    refreshed: usize,
) -> Answer {
    let budget = Limits::default().max_result_bytes;
    let mut limits = Limits::default();
    limits.max_result_rows = options.limit;
    // The engine measures its cap over **raw** atoms, and this host prints a
    // shorter form: a ~68-character `SymId` renders as `name path:line`. If the
    // engine held the printed budget it would always be the cap that fires, and
    // it would fire on rows that would have fit — the user would silently get a
    // fraction of the answer the budget allows. So the engine's cap becomes a
    // memory guard with headroom, and the printed budget is enforced where the
    // printed bytes exist (`specs/05-surface.md` § Symbol rendering).
    limits.max_result_bytes = budget.saturating_mul(RAW_BYTE_HEADROOM);
    let result = match loaded.engine.query(program, &limits) {
        Ok(result) => result,
        Err(diagnostic) => {
            let mut answer = Answer::of(
                Status::of(diagnostic.status),
                format!("{}{}", diagnostic.message, host_hint(&diagnostic)),
            );
            // An aborted evaluation reports what it measured, not the defaults
            // of an answer that never ran (invariant 5). `None` is a failure
            // before evaluation began, where the defaults are the truth.
            if let Some(stats) = diagnostic.stats {
                answer.derived = stats.derived;
                answer.elapsed_ms = stats.elapsed_ms;
                answer.transformed = stats.transformed;
                answer.demand = stats.demand;
                answer.depends = stats.depends;
                answer.shadowed = stats.shadowed;
                answer.widened.clone_from(&loaded.widened);
            }
            return answer;
        }
    };

    // One render, one order. Rendering each notation separately and sorting
    // both would give two arrays whose `i`th rows are different tuples. The
    // render cuts at `budget` bytes of printed text, which every wire costs at
    // least; the wire is measured below.
    let printed = render(&loaded.engine, &result, &loaded.sites, options.raw, budget);

    // The index's condition first. A refresh that did not finish outranks the
    // SCIP verdict, which is why `verdict` is only asked on an `ok` refresh.
    if status == Status::Ok
        && let Some((scip_status, scip_hint)) =
            loaded.scip.verdict(&result.stats.depends, &loaded.langs)
    {
        status = scip_status;
        hint = Some(scip_hint);
    }

    // The answer for one set of kept rows. `byte_cut` is whether
    // `max_result_bytes` dropped any.
    let shape = |rows: Vec<Row>, byte_cut: bool| -> Answer {
        let truncated = result.truncated || byte_cut;
        // When both caps fire, the byte budget decided which rows are missing,
        // so it is the one reported. Naming the engine's row cap there advised
        // `--limit`, which returns the same rows every time (#27).
        let cap = if byte_cut {
            Some(datalog::Cap::Bytes)
        } else {
            result.cap
        };
        let mut status = status;
        let mut hint = hint.clone();
        if truncated {
            // `--limit` moves `max_result_rows` and nothing else, so offering
            // it against a byte cap is advice that changes nothing.
            // Exhaustive over `Cap`, so a third cap could not be added
            // upstream and land silently in the general arm with the wrong
            // advice attached (#48).
            let cut = match cap {
                Some(datalog::Cap::Rows) => format!(
                    "{} fired at {}; raise it with --limit or narrow the query",
                    datalog::Cap::Rows,
                    options.limit
                ),
                // The engine's cap and the host's are different numbers, and
                // quoting the wrong one is invariant 5 with extra steps.
                Some(fired @ datalog::Cap::Bytes) => {
                    let at = if byte_cut {
                        budget
                    } else {
                        limits.max_result_bytes
                    };
                    format!("{fired} fired at {at} bytes; narrow the query")
                }
                None => "a cap fired; narrow the query".to_string(),
            };
            // `truncated` and `cap` carry the cut whatever the status says, so
            // a `stale` or `no-scip` already on the answer keeps the status: it
            // is the one fact nowhere else in the response
            // (`specs/05-surface.md` § Status taxonomy).
            hint = Some(match hint {
                Some(condition) if status != Status::Ok => compose(&condition, &cut, ". also: "),
                _ => {
                    status = Status::Truncated;
                    cut
                }
            });
        }
        // `hint` is non-null whenever the status is not `ok` **or** the result
        // is empty. Zero rows from a valid query is indistinguishable from a
        // typo, a wrong constant and a path that was never indexed, and that is
        // the one case the status taxonomy cannot separate on its own
        // (`specs/05-surface.md` § Response contract).
        //
        // A condition already in the slot does not displace it: the condition
        // says what the index could not see and the emptiness says which
        // literal stopped the join, and a degraded index is precisely when the
        // caller most needs both. Composed the way truncation composes above,
        // and the status stays the condition for the same reason.
        if rows.is_empty() && !truncated {
            let mut empty = empty_hint(&result, &loaded.relations, &loaded.engine);
            if let Some(note) = syntax_error_note(&syntax_errors(&loaded.manifest)) {
                empty = compose(&empty, &note, ". also: ");
            }
            hint = Some(match hint {
                Some(condition) => compose(&condition, &empty, ". also: "),
                None => empty,
            });
        }
        Answer {
            status,
            columns: result.columns.clone(),
            rows,
            truncated,
            cap,
            hint,
            // Stamped by the caller that adopted a root, not here: this
            // function does not know which root it was handed.
            root: None,
            derived: result.stats.derived,
            elapsed_ms: result.stats.elapsed_ms,
            refreshed,
            transformed: result.stats.transformed.clone(),
            demand: result.stats.demand,
            depends: result.stats.depends.clone(),
            shadowed: result.stats.shadowed.clone(),
            widened: loaded.widened.clone(),
            evaluated: true,
        }
    };

    let mut answer = shape(printed.rows, printed.truncated);
    if sent(&mut answer, options) > budget && !answer.rows.is_empty() {
        // Not every row fits, and a cut only lengthens the rest of the answer
        // (a status, a hint), so the answer is some shorter prefix. The size
        // grows with the prefix: bisect for the longest one that fits
        // (`specs/05-surface.md` § Symbol rendering).
        let mut rows = std::mem::take(&mut answer.rows);
        let (mut lo, mut hi) = (0, rows.len().saturating_sub(1));
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if sent(
                &mut shape(rows.iter().take(mid).cloned().collect(), true),
                options,
            ) <= budget
            {
                lo = mid;
            } else {
                hi = mid.saturating_sub(1);
            }
        }
        rows.truncate(lo);
        answer = shape(rows, true);
    }
    answer
}

/// The bytes `answer` costs on the wire `options` names.
///
/// This used to measure with `elapsed_ms` pinned at `u64::MAX`, so that where
/// a cut landed did not depend on how fast the run was. That guarded the
/// second-order effect of a non-deterministic field while the field itself was
/// still serialized into the answer, so the cut point was stable and the bytes
/// around it were not. `elapsed_ms` no longer travels in the response
/// (`wire::json`), and the workaround goes with it.
fn sent(answer: &mut Answer, options: &Options) -> usize {
    options.wire.bytes(answer, options.raw)
}

/// How a composed hint separates its clauses from the runnable chain one of
/// them may end in (#104, #102).
///
/// **Adopted: [`Shape::ChainLast`].** Built as an experiment with both
/// outcomes side by side (#102's comparison), then taken on the argument that
/// A keeps `specs/05-surface.md`'s "one complete line" promise verbatim, costs
/// nothing on any wire, and makes the defect impossible rather than fixed.
/// `CODEINTEL_HINT_SHAPE` still selects any of the three, so `own-line` stays
/// one variable away if #102 wants somewhere to put a polyglot block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// What shipped until #104: `condition. also: emptiness`, with the chain
    /// left wherever the
    /// condition put it — which is mid-hint, so `. also: ` glues onto the last
    /// command and `scip-python index .` becomes `scip-python index ..`
    /// (#104). `..` is a valid path, so it runs, against the parent directory.
    Glued,
    /// **Adopted.** The chain moves to the end of the composed hint, still one
    /// line.
    /// Nothing can follow it, so the defect becomes impossible rather than
    /// fixed. `specs/05-surface.md`'s "one complete line" survives verbatim.
    ChainLast,
    /// B: the chain gets its own line. Also terminal, and it gives #102
    /// somewhere to put a polyglot block — at the cost of a hint that is no
    /// longer one line, which several readers assume.
    OwnLine,
}

impl Shape {
    /// `CODEINTEL_HINT_SHAPE`, defaulting to the adopted shape.
    ///
    /// `glued` is kept selectable because it is what every recorded hint in
    /// `docs/` and the specs was captured from; reproducing one needs the
    /// shape it was written under, not an explanation of why it changed.
    #[must_use]
    pub fn current() -> Self {
        match std::env::var("CODEINTEL_HINT_SHAPE").as_deref() {
            Ok("glued") => Self::Glued,
            Ok("own-line") => Self::OwnLine,
            _ => Self::ChainLast,
        }
    }
}

/// Where a hint's runnable tail starts.
///
/// Every producer of one writes it last — `discover::index_chain`, `census`'s
/// stale and scip-stale lines, the `no-scip` indexer list — so the **last**
/// `run: ` opens it and everything before it is prose. Composing under
/// [`Shape::ChainLast`] or [`Shape::OwnLine`] keeps that true, so the split
/// stays idempotent however many clauses are added.
const RUN: &str = "run: ";

fn split_run(hint: &str) -> (&str, Option<&str>) {
    match hint.rfind(RUN) {
        Some(at) => (hint.get(..at).unwrap_or(hint).trim_end(), hint.get(at..)),
        None => (hint, None),
    }
}

/// Add `extra` to `hint` as a second clause, per [`Shape`].
///
/// `separator` is what the caller would have written between the two, because
/// the three composition sites do not agree: two write `. also: ` and the
/// adopted note writes ` also: ` with no full stop at all — which is how it
/// produces `codeintel index . also:`, where `.` is the shell's source builtin
/// taking `also:` as its argument.
fn compose(hint: &str, extra: &str, separator: &str) -> String {
    let (prose, run) = split_run(hint);
    let (Some(run), shape @ (Shape::ChainLast | Shape::OwnLine)) = (run, Shape::current()) else {
        // `Glued` is what ships, byte for byte: this branch is the thing the
        // other two are being compared against, so it is not re-derived.
        return format!("{hint}{separator}{extra}");
    };
    let clause = sentence(prose, &format!("also: {extra}"));
    match shape {
        Shape::OwnLine => format!("{clause}\n{run}"),
        _ => sentence(&clause, run),
    }
}

/// Join two clauses with exactly one full stop between them.
///
/// Moving the chain means re-punctuating: the prose the chain was lifted off
/// already ends in `.` where the old separator supplied its own, and the
/// clause it now follows may end in none at all. Without this the two shapes
/// read `can see.. also:` and `matched run:`, which would be a worse hint
/// arguing for a better one.
fn sentence(left: &str, right: &str) -> String {
    let left = left.trim_end();
    if left.is_empty() {
        return right.to_string();
    }
    if left.ends_with(['.', ';', ',', ':']) {
        format!("{left} {right}")
    } else {
        format!("{left}. {right}")
    }
}

/// What a hint cut to fit `max_result_bytes` ends with.
const HINT_CUT: &str = " ... [hint cut to fit max_result_bytes]";

/// What a hint whose runnable chain would not fit ends with instead.
///
/// The chain is dropped whole rather than cut, so the marker has to say the
/// actionable half is missing — a hint that quietly loses the command is a
/// silent degradation (invariant 5), and the caller cannot tell a hint that
/// never had one from a hint that lost it.
const HINT_CUT_NO_CHAIN: &str = " ... [hint cut to fit max_result_bytes; recovery command dropped]";

/// Cut the hint to the longest prefix, on a character boundary, with which
/// `answer` fits `budget` as `measure` counts it — **never cutting into, or
/// appending to, a runnable chain** (#108).
///
/// Rows are cut first, in [`evaluate`]; this is for an answer whose envelope
/// is over budget on its own. Only the hint is cut: `status`, `truncated` and
/// `cap` are what a consumer branches on, `columns` echoes the caller's own
/// goal, and `stats` is bounded by the rule set (`specs/05-surface.md`
/// § Symbol rendering).
///
/// A plain prefix cut lands wherever the budget runs out, and the two places
/// that can be are both wrong. Inside a command it leaves half of one. Just
/// *after* one it is worse, because the marker's own words become arguments —
/// `rust-analyzer scip . ... [hint cut to fit max_result_bytes]` runs, against
/// whatever `...` and `[hint` glob to. That is #104's failure reached through
/// the cut rather than through composition, and `specs/05-surface.md`
/// § Response contract already forbids it: nothing may be appended to the line
/// a runnable chain ends, and a marker is an append.
///
/// So the chain is **atomic**: kept whole with the prose cut around it, or
/// dropped whole and reported. Keeping it is tried first because the chain is
/// the actionable half — the prose says what is wrong and the chain says what
/// to type, and under pressure the second is worth more. The budget is still
/// the budget: when the chain alone will not fit, it goes.
fn fit_hint(answer: &mut Answer, budget: usize, mut measure: impl FnMut(&mut Answer) -> usize) {
    if measure(answer) <= budget {
        return;
    }
    let Some(full) = answer.hint.take() else {
        return;
    };
    // The tail runs to the end of the chain's own line. Under a composition
    // that glued prose onto it that line is the rest of the hint, which is
    // exactly the boundary #104 lost — so the whole remainder is treated as
    // the chain. Conservative in the only safe direction: this never cuts
    // something that might be a command, at the price of sometimes dropping
    // prose that followed one.
    let tail = full.rfind(RUN).map(|at| {
        let end = full
            .get(at..)
            .and_then(|rest| rest.find('\n'))
            .map_or(full.len(), |n| at + n);
        let prose = full.get(..at).unwrap_or_default().trim_end();
        let separator = full.get(prose.len()..at).unwrap_or_default();
        (prose, separator, full.get(at..end).unwrap_or_default())
    });

    // Longest prefix of `prose` that fits once `assemble` has put it back
    // together. `None` when not even the empty prose fits.
    let mut longest = |prose: &str, assemble: &dyn Fn(&str) -> String, answer: &mut Answer| {
        let at = |n: usize| {
            let mut n = n.min(prose.len());
            while !prose.is_char_boundary(n) {
                n = n.saturating_sub(1);
            }
            assemble(prose.get(..n).unwrap_or_default())
        };
        answer.hint = Some(at(0));
        if measure(answer) > budget {
            return None;
        }
        // The size grows with the prefix: bisect for the longest that fits.
        let (mut lo, mut hi) = (0, prose.len());
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            answer.hint = Some(at(mid));
            if measure(answer) <= budget {
                lo = mid;
            } else {
                hi = mid.saturating_sub(1);
            }
        }
        Some(at(lo))
    };

    if let Some((prose, separator, chain)) = tail {
        let keep = |cut: &str| {
            if cut.is_empty() {
                chain.to_string()
            } else {
                format!("{cut}{HINT_CUT}{separator}{chain}")
            }
        };
        if let Some(hint) = longest(prose, &keep, answer) {
            answer.hint = Some(hint);
            return;
        }
        // The chain alone is over budget. It goes, and the marker says so.
        let drop = |cut: &str| format!("{cut}{HINT_CUT_NO_CHAIN}");
        answer.hint = Some(longest(prose, &drop, answer).unwrap_or_else(|| drop("")));
        return;
    }

    let cut = |p: &str| format!("{p}{HINT_CUT}");
    answer.hint = Some(longest(&full, &cut, answer).unwrap_or_else(|| cut("")));
}

/// What this host adds to an engine diagnostic: the verb or the rule that
/// fixes it here. The engine names the failure in its own terms and knows no
/// verb and no rule, which is invariant 6; the sentence that says `codeintel
/// schema` or `impact_of` belongs to the crate that has them.
fn host_hint(diagnostic: &datalog::Diagnostic) -> &'static str {
    match diagnostic.status {
        datalog::Status::InvalidQuery if diagnostic.message.starts_with("no relation or rule") => {
            ". check the spelling against `codeintel schema`"
        }
        datalog::Status::BudgetExceeded => {
            ". a seeded form such as `impact_of(Seed, C)` explores the reachable subgraph"
        }
        _ => "",
    }
}

/// Why an empty result is empty, in one actionable line.
///
/// The engine reports the body literal its join never got past
/// ([`datalog::Stats::empty_at`]); this adds what the index holds for that
/// literal's relation, which is what separates "wrong constant" from "that
/// relation is empty — you need a SCIP index" from "this is not true of your
/// code".
fn empty_hint(
    result: &datalog::QueryResult,
    relations: &BTreeMap<&str, Relation>,
    engine: &Engine,
) -> String {
    let Some(at) = &result.stats.empty_at else {
        return "the goal derived no rows: this is not true of your code as indexed".to_string();
    };
    let text = &at.text;
    let name = relation_of(&at.literal).unwrap_or_default();
    let held = relations.get(name).map_or(0, Relation::len);
    if let Literal::Neg(pred) = &at.literal {
        // Matching is what emptied the join, so "matched 0 rows" would say the
        // opposite of the truth (#124).
        let total = if facts::schema::by_name(name).is_some() {
            format!(" (the index holds {held} `{name}` row(s))")
        } else {
            String::new()
        };
        let next = if pred.args.contains(&Term::Wildcard) {
            "a `_` inside `!` means no such row anywhere; bind the variable first"
        } else {
            "drop the `!` to see the rows it excluded"
        };
        return format!(
            "`!{text}` excluded every row: each binding that reached it has a \
             matching `{name}` row{total}. {next}"
        );
    }
    if facts::schema::by_name(name).is_some() {
        // A constant in a typed column is the agent's most likely error, and
        // the relation total says nothing about it
        // (`specs/05-surface.md` § Response contract).
        if let Some(column) = column_hint(at, name, relations, engine) {
            return format!("`{text}` matched 0 rows; {column}");
        }
        return format!(
            "`{text}` matched 0 rows; the index holds {held} `{name}` row(s){}{}",
            preceding(at, ". "),
            integer_hint(at, engine)
        );
    }
    format!(
        "`{text}` matched 0 rows{}{}",
        preceding(at, "; "),
        integer_hint(at, engine)
    )
}

/// `everything before it in the join matched` behind `separator`, unless
/// nothing came before it: said of the first literal it is false (#140).
fn preceding(at: &EmptyAt, separator: &str) -> String {
    if at.position == 0 {
        String::new()
    } else {
        format!("{separator}everything before it in the join matched")
    }
}

/// The relation a literal names, or `None` for a builtin that names none.
///
/// A comparison, an assignment, `between` and the string tests have no
/// relation and no column schema, so they fall through to the hint that only
/// reports what the join reached.
fn relation_of(literal: &Literal) -> Option<&str> {
    match literal {
        Literal::Pos(pred) | Literal::Neg(pred) => Some(&pred.name),
        _ => None,
    }
}

/// The string constants of the literal that stopped the join, each with the
/// argument position it sits in.
///
/// Read from the parsed literal the engine evaluated, never from its text. The
/// text path split arguments on unquoted commas and counted an escaped `\"` as
/// closing the string, so `def(S, F, "x\"y, function", "nosuchkind")` split
/// into five arguments: every later column shifted by one, the hint diagnosed
/// a fabricated value against the wrong column, and the constant that actually
/// matched nothing was never examined (#81).
///
/// Integer constants are skipped. They are the correct form for an integer
/// column, and in a vocabulary or path column there is nothing an index can
/// say about them that the relation total does not already say.
fn string_constants(at: &EmptyAt, engine: &Engine) -> Vec<(usize, String)> {
    let (Literal::Pos(pred) | Literal::Neg(pred)) = &at.literal else {
        return Vec::new();
    };
    pred.args
        .iter()
        .enumerate()
        .filter_map(|(i, term)| match term {
            Term::Const(atom) if !is_int(*atom) => {
                engine.resolve(*atom).map(|value| (i, value.to_string()))
            }
            _ => None,
        })
        .collect()
}

/// The int-vs-string trap, for a literal whose columns carry no type.
///
/// `at(S, F, "42")` is a derived rule, so `column_hint` has no schema to say
/// its third column is a line number. A quoted constant that parses as an
/// integer is a different atom from the integer and can never match it; that
/// is the one case where naming the replacement is a statement about the atom
/// space, not a guess about the code.
fn integer_hint(at: &EmptyAt, engine: &Engine) -> String {
    for (_, bare) in string_constants(at, engine) {
        if bare.parse::<i64>().is_ok() {
            return format!(
                ". did you mean {bare}? {bare:?} is a string, and a string never \
                 matches an integer"
            );
        }
    }
    String::new()
}

/// Diagnose the first constant sitting in a typed column of `relation`.
///
/// Returns `None` when every constant is in a `Free` column, which is the
/// common case and leaves the relation-total hint alone. Nothing here suggests
/// a *replacement* value: listing what the column holds is a fact about the
/// index, proposing what the agent meant is a guess (invariant 1).
fn column_hint(
    at: &EmptyAt,
    relation: &str,
    relations: &BTreeMap<&str, Relation>,
    engine: &Engine,
) -> Option<String> {
    let columns = schema::columns(relation);
    for (i, bare) in string_constants(at, engine) {
        match columns.get(i) {
            Some(schema::Column::Int) => {
                return Some(format!(
                    "that column holds integers and {bare:?} is a string — they are \
                     different atoms and can never match. write {bare} unquoted"
                ));
            }
            Some(schema::Column::Path) => {
                if let Some(said) = path_hint(&bare, relations, engine) {
                    return Some(said);
                }
            }
            Some(schema::Column::Vocab(values)) => {
                let counts = vocabulary(relation, i, values, relations, engine);
                let listed = counts
                    .iter()
                    .map(|(value, n)| format!("{value} {n}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                // The count for *this* value decides what is true. Saying "0
                // rows" while the list beside it shows a non-zero count would
                // be the hint contradicting its own evidence, and a literal
                // constrains more than one column: a value with rows of its own
                // means the emptiness came from somewhere else in the literal.
                let lead = match counts.iter().find(|(value, _)| *value == bare) {
                    None => format!("{bare:?} is not one of this column's values"),
                    Some((_, 0)) => format!("{bare:?} has 0 rows in this index"),
                    Some((_, n)) => format!(
                        "{bare:?} has {n} row(s) here, so another constant in this \
                         literal is what matched nothing"
                    ),
                };
                return Some(format!("{lead}. the column holds: {listed}"));
            }
            _ => {}
        }
    }
    None
}

/// Why a path constant matched nothing, when the answer is knowable.
///
/// Paths are the constant an agent is most likely to get wrong, because they
/// arrive from `ripgrep`, `git diff` and stack traces — all of which emit
/// absolute or `./`-prefixed forms, while the index keys on repo-relative ones.
/// Every branch below reports a **fact about the index**: that a normalised
/// form is indexed, or that a file with this basename is indexed elsewhere.
/// Neither proposes what the agent meant (invariant 1); they state what exists
/// and let the agent re-aim.
fn path_hint(
    wanted: &str,
    relations: &BTreeMap<&str, Relation>,
    engine: &Engine,
) -> Option<String> {
    let indexed: BTreeSet<&str> = relations
        .get("file")
        .into_iter()
        .flat_map(Relation::iter)
        .filter_map(|row| row.first().copied().and_then(|a| engine.resolve(a)))
        .collect();
    if indexed.contains(wanted) {
        return None; // Indexed: the emptiness is elsewhere in the literal.
    }

    // The shapes the tools an agent pipes from actually emit: a leading `./`,
    // or an absolute path.
    //
    // The absolute case is matched by *suffix at a path boundary*, not by
    // stripping the root. Stripping cannot be made reliable: `root` as passed
    // is commonly `.`, so it must be canonicalised, and canonicalising resolves
    // symlinks — on macOS a path under `/var` canonicalises to `/private/var`,
    // which is no longer a prefix of what the agent wrote. A suffix match does
    // not care how the caller reached the directory. The longest match wins, so
    // an index holding both `a/b.rs` and `x/a/b.rs` reports the more specific.
    let relative = wanted.strip_prefix("./").unwrap_or_else(|| {
        indexed
            .iter()
            .filter(|p| wanted.len() > p.len() && wanted.ends_with(*p))
            .filter(|p| {
                wanted
                    .as_bytes()
                    .get(wanted.len() - p.len() - 1)
                    .is_some_and(|b| *b == b'/')
            })
            .max_by_key(|p| p.len())
            .copied()
            .unwrap_or(wanted)
    });
    if relative != wanted && indexed.contains(relative) {
        return Some(format!(
            "paths are repo-relative, and `{relative}` is indexed. \
             the leading path was not matched literally"
        ));
    }

    // Same file name somewhere else is a fact worth stating: it separates "you
    // mistyped the directory" from "this file is not indexed at all".
    let base = wanted.rsplit('/').next().unwrap_or(wanted);
    let same: Vec<&str> = indexed
        .iter()
        .copied()
        .filter(|p| p.rsplit('/').next() == Some(base))
        .take(3)
        .collect();
    if !same.is_empty() {
        return Some(format!(
            "no indexed file is `{wanted}`; `{base}` is indexed at: {}",
            same.join(" ")
        ));
    }
    Some(format!(
        "no indexed file is `{wanted}`, and none is named `{base}`. \
         the index holds {} file(s); `codeintel status` says what was skipped",
        indexed.len()
    ))
}

/// This index's count for every value of a closed vocabulary column, in the
/// vocabulary's own order so the output is stable and a zero is visible in
/// place rather than absent.
fn vocabulary(
    relation: &str,
    column: usize,
    values: &[&'static str],
    relations: &BTreeMap<&str, Relation>,
    engine: &Engine,
) -> Vec<(&'static str, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    if let Some(rows) = relations.get(relation) {
        for row in rows.iter() {
            if let Some(value) = row.get(column).copied().and_then(|a| engine.resolve(a)) {
                *counts.entry(value).or_default() += 1;
            }
        }
    }
    values
        .iter()
        .map(|v| (*v, counts.get(v).copied().unwrap_or(0)))
        .collect()
}

/// What the index knows about its SCIP inputs, and what that means for an
/// answer.
#[derive(Debug, Default)]
pub struct ScipState {
    /// True when no SCIP index was ingested.
    pub absent: bool,
    /// How many documents the ingested inputs carried, and how many named a
    /// file this index holds. `None` when no input was ingested, or when the
    /// manifest predates the count.
    ///
    /// Zero matched out of some is the same emptiness as `absent` with a
    /// different repair, and it used to answer `ok` with zero rows — an index
    /// built from a subdirectory matches nothing and says nothing (#60,
    /// `specs/05-surface.md` § Status taxonomy).
    pub matched: Option<(u64, u64)>,
    /// Indexed files whose bytes the SCIP inputs did not see
    /// ([`facts::FileEntry::scip_stale`]).
    pub stale: Vec<String>,
}

impl ScipState {
    /// Read the SCIP situation out of a manifest.
    ///
    /// Public so `status` reports the *same* staleness `query` acts on. They
    /// disagreed once — `query` answered `scip-stale` naming two files while
    /// `status`, the artifact you are told to paste into a bug report, printed
    /// `ok` and said nothing.
    #[must_use]
    pub fn of(manifest: &facts::Manifest) -> Self {
        let Some(newest) = manifest.scip.iter().map(|i| i.mtime).max() else {
            return Self {
                absent: true,
                matched: None,
                stale: Vec::new(),
            };
        };
        Self {
            absent: false,
            matched: manifest
                .scip_matched
                .map(|matched| (matched, manifest.scip.iter().map(|i| i.documents).sum())),
            stale: manifest
                .files
                .iter()
                .filter(|(_, entry)| entry.scip_stale(newest))
                .map(|(path, _)| path.clone())
                .collect(),
        }
    }

    /// The status this answer deserves, or `None` when SCIP has nothing to say
    /// about it.
    fn verdict(&self, depends: &[String], langs: &[&'static str]) -> Option<(Status, String)> {
        if !depends.iter().any(|d| scip_backed(d)) {
            return None;
        }
        let commands = if langs.is_empty() {
            "build a SCIP index for this repository".to_string()
        } else {
            format!("run: {}", langs.join(" && "))
        };
        let wanted = depends
            .iter()
            .filter(|d| scip_backed(d))
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if self.absent {
            return Some((
                Status::NoScip,
                format!(
                    "this query depends on {wanted}, which only a SCIP index populates; the \
                     answer above is what tier A alone can see. {commands}"
                ),
            ));
        }
        // Ingested and landed nowhere. The relations are as empty as they are
        // with no index at all, so the status is the same one; only the repair
        // differs, and the hint is where a repair belongs.
        if let Some((0, documents)) = self.matched
            && documents > 0
        {
            return Some((
                Status::NoScip,
                format!(
                    "this query depends on {wanted}, and the SCIP index that was ingested \
                     populated none of it: 0 of {documents} document(s) matched an indexed \
                     file. its paths are relative to some other directory — `codeintel index` \
                     names the one to pass as --scip-root"
                ),
            ));
        }
        if self.stale.is_empty() {
            return None;
        }
        Some((
            Status::ScipStale,
            format!(
                "{} file(s) changed after the SCIP index was built, so their `name_ref` rows are \
                 fresh and their `scip_ref` rows are not: {}. {commands}",
                self.stale.len(),
                name_list(&self.stale)
            ),
        ))
    }
}

/// The first five of `names`, comma-joined, with `...` when there are more.
/// A hint names files so the reader can act, not so it can list them all.
pub(crate) fn name_list<'a>(names: impl IntoIterator<Item = &'a String>) -> String {
    let mut names = names.into_iter();
    let mut out: Vec<&str> = names.by_ref().take(5).map(String::as_str).collect();
    if names.next().is_some() {
        out.push("...");
    }
    out.join(", ")
}

/// Files that parsed with errors, in path order, with the line of the first
/// error (`specs/02-extraction.md` § A file that does not parse is partial).
pub(crate) fn syntax_errors(manifest: &facts::Manifest) -> Vec<(&str, u32)> {
    manifest
        .files
        .iter()
        .filter_map(|(path, entry)| Some((path.as_str(), entry.syntax_error?)))
        .collect()
}

/// `N file(s) parsed with errors, so a definition after the error may be
/// missing: a.go:3, b.py:9, ...` — five names, the total always shown. `None`
/// when every file parsed.
pub fn syntax_error_note(errors: &[(&str, u32)]) -> Option<String> {
    if errors.is_empty() {
        return None;
    }
    let named: Vec<String> = errors
        .iter()
        .map(|(path, line)| format!("{path}:{line}"))
        .collect();
    Some(format!(
        "{} file(s) parsed with errors, so a definition after the error may be missing: {}",
        errors.len(),
        name_list(&named)
    ))
}

/// The indexer command for every language the index holds files in.
pub(crate) fn indexers(manifest: &facts::Manifest) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = manifest
        .files
        .values()
        .filter_map(|entry| extract::lang::by_name(&entry.lang).map(|l| l.indexer))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Refresh `store` as `query` would, so a catalog counted from it describes the
/// database `query` is about to answer from. No index, or one written for
/// another schema, is left alone: `query` answers those before it refreshes.
///
/// # Errors
/// An I/O failure that is not a refusal; refusals are statuses.
pub fn refresh_store(store: &mut Store, root: &Path) -> std::io::Result<(Status, Option<String>)> {
    if !store.has_index() || stale_schema(store.manifest(), root).is_some() {
        return Ok((Status::Ok, None));
    }
    refresh(store, root, &Options::default())
        .map(|(status, hint, _)| (status, hint))
        .map_err(std::io::Error::other)
}

/// Bring the index up to date, within the refresh budget.
///
/// A reader that cannot take the writer lock does not fail: it reads the
/// current manifest and reports `stale`. `locked` is for a second writer, and
/// telling a reader "locked" is not actionable
/// (`specs/04-storage.md` § Concurrency).
fn refresh(
    store: &mut Store,
    root: &Path,
    options: &Options,
) -> Result<(Status, Option<String>, usize)> {
    if options.no_refresh {
        return Ok((Status::Ok, None, 0));
    }
    // A read-only checkout cannot create the lock file. That is a reader that
    // cannot take the lock, and it gets the same answer
    // (`specs/04-storage.md` § Concurrency).
    let mut lock = match Lock::open(store.dir()) {
        Ok(lock) => lock,
        Err(e) => {
            return Ok((
                Status::Stale,
                Some(format!(
                    "could not open the writer lock ({e}); this answer is from the index as it \
                     stands. run: codeintel index {}",
                    root.display()
                )),
                0,
            ));
        }
    };
    let held = lock.try_hold().context("taking the writer lock")?;
    let Ok(held) = held else {
        return Ok((
            Status::Stale,
            Some(format!(
                "another codeintel run holds the lock; this answer is from the index as it \
                 stands. run: codeintel index {}",
                root.display()
            )),
            0,
        ));
    };

    // The manifest was read before the lock. A writer that committed in
    // between named segments this copy does not, and a refresh built on this
    // copy would commit a manifest that orphans them. Re-read now, and reopen
    // only if it moved: a reopen rebuilds the dictionary map, and the JSON
    // compare is cheap.
    let current = match facts::Manifest::open(store.dir()) {
        Ok(current) => current,
        Err(e) => return refused(e, "re-reading the manifest").map(|a| (a.status, a.hint, 0)),
    };
    if current.as_ref() != Some(store.manifest()) {
        *store = match open(root, store.dir())? {
            Ok(reopened) => reopened,
            Err(answer) => return Ok((answer.status, answer.hint, 0)),
        };
    }

    // The SCIP inputs the index was built with, carried forward. Without them
    // an auto-refresh looks like "the SCIP index disappeared", re-extracts the
    // tree tier-A-only, and silently deletes every tier-B fact — the index
    // would degrade a little more with each query.
    //
    // Auto-refresh is still tier-A-only in effect: a file edited since the SCIP
    // index was built is re-extracted against the *old* anchors, so it keeps
    // whatever resolved identity still matches and reports `scip-stale`
    // (`docs/plan.md` M3). A full `codeintel index` is what reconciles it.
    // The `--scip-root` of each input is carried with it. It is not a
    // property of the file, so dropping it here would re-ingest every
    // document at its un-based path, match nothing, and delete every tier-B
    // fact — the same silent degradation this block exists to prevent, one
    // layer down (`specs/02-extraction.md` § Document paths, and
    // `--scip-root`).
    let plan = Plan {
        deadline: Some(Instant::now() + Duration::from_millis(MAX_REFRESH_MS)),
        scip: store
            .manifest()
            .scip
            .iter()
            .map(|input| PathBuf::from(&input.path))
            .collect(),
        scip_roots: store
            .manifest()
            .scip
            .iter()
            .map(|input| input.root.clone())
            .collect(),
        ..Plan::default()
    };
    let report = index::refresh(store, &plan)?;
    drop(held);

    if report.is_stale() {
        return Ok((
            Status::Stale,
            Some(format!(
                "auto-refresh ran out of its {MAX_REFRESH_MS} ms budget with {} file(s) left: \
                 {}. run: codeintel index {}",
                report.stale.len(),
                name_list(&report.stale),
                root.display()
            )),
            report.indexed,
        ));
    }
    Ok((Status::Ok, None, report.indexed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes a composed hint can take, as #102/#104 are weighing them.
    ///
    /// Strings and not a mode, because this must hold for whichever of them is
    /// adopted — and for today's, which is `glued`. A fix that protected only
    /// the shape its author had in mind would pass on one of these and fail on
    /// the other two.
    fn composed_hints() -> Vec<(&'static str, String)> {
        let prose = "this query depends on scip_ref, which only a SCIP index populates; \
                     the answer above is what tier A alone can see. also: \
                     `scip_ref(S,F,_,_,_,_)` matched 0 rows; the index holds 0 rows";
        let chain = "run: cd /repo && codeintel index . && rust-analyzer scip . && \
                     scip-go && scip-python index . && codeintel index .";
        let (head, tail) = prose.split_once("also:").expect("a composed hint");
        vec![
            ("glued", format!("{head}{chain} also:{tail}")),
            ("chain-last", format!("{prose}. {chain}")),
            ("own-line", format!("{prose}\n{chain}")),
        ]
    }

    /// A cut never leaves a runnable chain with words after it, at any budget.
    ///
    /// The sweep is what found #108 — a plain prefix cut lands wherever the
    /// budget runs out, and just after a complete command it hands the marker's
    /// own words to it as arguments. So the assertion is the one
    /// `specs/05-surface.md` § Response contract states: whatever survives the
    /// cut either carries no chain at all, or carries one that is byte-identical
    /// to the chain it started with and is the last thing on its line.
    #[test]
    fn a_cut_never_appends_to_a_runnable_chain_whatever_the_shape() {
        // The smallest hint this can produce, envelope included.
        let floor = Wire::McpJson.bytes(&Answer::of(Status::NoScip, HINT_CUT_NO_CHAIN), false);
        for (name, hint) in composed_hints() {
            let original = hint
                .get(hint.rfind("run: ").expect("a chain")..)
                .and_then(|tail| tail.split('\n').next())
                .expect("a chain line");
            for budget in (120..=620).step_by(4) {
                let mut answer = Answer::of(Status::NoScip, hint.clone());
                fit_hint(&mut answer, budget, |a| Wire::McpJson.bytes(a, false));
                let got = answer.hint.clone().unwrap_or_default();

                // Below the floor nothing complies: the envelope plus the
                // shortest marker this can emit is already over, and that is
                // true of any cut, including the one before #108. Above it,
                // keeping a chain whole is not a licence to overrun.
                let bytes = Wire::McpJson.bytes(&answer, false);
                assert!(
                    bytes <= budget.max(floor),
                    "{name} at {budget}: {bytes} bytes, floor {floor}"
                );

                let Some(at) = got.rfind("run: ") else {
                    // Dropped whole, and said so: that is the other legal
                    // outcome, never a silent loss.
                    assert!(
                        got.ends_with(HINT_CUT_NO_CHAIN),
                        "{name} at {budget}: the chain is gone and the hint does \
                         not say so: {got}"
                    );
                    continue;
                };
                let kept = got
                    .get(at..)
                    .and_then(|tail| tail.split('\n').next())
                    .expect("a chain line");
                assert_eq!(
                    kept, original,
                    "{name} at {budget}: the chain's line changed. Everything after \
                     the last command is read as more arguments to it, and the cut \
                     marker is no exception (#108)"
                );
            }
        }
    }

    #[test]
    fn a_hint_over_the_budget_is_cut_on_a_char_boundary_and_the_status_kept() {
        // No rows to drop: the hint is the only thing that can shrink. Every
        // character is two bytes, so a cut that ignored boundaries would panic
        // or split one.
        let hint = "é".repeat(2_000);
        let mut answer = Answer::of(Status::NoIndex, hint.clone());
        let budget = 1_000;
        let measure = |a: &mut Answer| Wire::McpJson.bytes(a, false);
        assert!(measure(&mut answer) > budget);

        fit_hint(&mut answer, budget, measure);

        let bytes = measure(&mut answer);
        assert!(bytes <= budget, "{bytes} bytes against {budget}");
        assert_eq!(answer.status, Status::NoIndex);
        assert!(!answer.truncated, "a hint cut drops no rows");
        let cut = answer.hint.as_deref().expect("a hint");
        let kept = cut.strip_suffix(HINT_CUT).expect("the marker");
        assert!(!kept.is_empty() && hint.starts_with(kept), "{cut}");
    }

    #[test]
    fn a_hint_that_fits_is_left_alone() {
        let mut answer = Answer::of(Status::NoIndex, "run: codeintel index .");
        fit_hint(&mut answer, 262_144, |a| Wire::McpJson.bytes(a, false));
        assert_eq!(answer.hint.as_deref(), Some("run: codeintel index ."));
    }

    fn opened(root: &Path) -> Store {
        Store::open(root, root.join(facts::store::DIR), &extract::fingerprint()).expect("opens")
    }

    fn indexed(source: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), source).expect("write");
        index::refresh(&mut opened(dir.path()), &Plan::default()).expect("indexes");
        dir
    }

    #[test]
    fn a_segment_a_concurrent_commit_unlinked_is_loaded_from_the_new_manifest() {
        let dir = indexed("pub fn before() {}\n");
        // This reader has read the manifest and released the lock.
        let reader = opened(dir.path());
        // Another process's refresh commits and unlinks the segment it names.
        std::fs::write(dir.path().join("a.rs"), "pub fn after_the_edit() {}\n").expect("write");
        index::refresh(&mut opened(dir.path()), &Plan::default()).expect("refreshes");

        let (store, relations) = load(reader, dir.path())
            .expect("no I/O error")
            .expect("loaded after one retry");
        assert!(relations.contains_key("def"));
        assert_eq!(store.manifest(), opened(dir.path()).manifest());
    }

    #[test]
    fn a_segment_missing_under_an_unmoved_manifest_is_corrupt() {
        let dir = indexed("pub fn only() {}\n");
        let reader = opened(dir.path());
        let seg = reader.manifest().files["a.rs"].seg.to_string();
        std::fs::remove_file(dir.path().join(".codeintel/seg").join(seg)).expect("removes");

        let answer = load(reader, dir.path())
            .expect("no I/O error")
            .map(|_| ())
            .expect_err("an answer, not a load");
        assert_eq!(answer.status, Status::Corrupt, "{:?}", answer.hint);
    }
}
