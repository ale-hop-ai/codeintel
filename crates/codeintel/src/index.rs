//! Building and refreshing the index.
//!
//! One routine, two callers. `codeintel index` runs it unbounded;
//! `codeintel query`'s auto-refresh runs it with a deadline
//! (`specs/05-surface.md` § `query`). Everything it finishes is committed, so
//! a refresh that runs out of time converges next call instead of abandoning
//! its work and paying the full budget forever.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use extract::scip::Ingest;
use extract::walk::{Candidate, Skips};
use extract::{Anchors, Counts, Extractor, tier_b, walk};
use facts::{NewEntry, ScipInput, Store};

/// The SCIP index `index` reads when `--scip` is not given.
pub const DEFAULT_SCIP: &str = "index.scip";

/// What to do.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Discard and rewrite everything, dictionary included.
    pub rebuild: bool,
    /// Restrict to these languages. Empty means every registered one.
    pub langs: Vec<String>,
    /// Stop starting new files after this instant and report the rest stale.
    pub deadline: Option<Instant>,
    /// SCIP indexes to ingest. Empty means tier A only.
    ///
    /// All-or-nothing by specification: a SCIP index carries cross-file
    /// references, so a changed one invalidates the reference graph globally
    /// and every file is re-extracted (`specs/04-storage.md`
    /// § Incremental reindex).
    pub scip: Vec<PathBuf>,
    /// The repo-relative directory each input's document paths are relative
    /// to. Empty means every input is relative to the repository root; one
    /// entry applies to every input; otherwise one per input, in the order
    /// `scip` gives them. Validated by the caller
    /// (`specs/02-extraction.md` § Document paths, and `--scip-root`).
    pub scip_roots: Vec<String>,
}

impl Plan {
    /// The root for the `i`th SCIP input: none, one for all, or one each.
    fn root_for(&self, i: usize) -> &str {
        let i = if self.scip_roots.len() == 1 { 0 } else { i };
        self.scip_roots.get(i).map_or("", String::as_str)
    }
}

/// What happened.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Files extracted this run.
    pub indexed: usize,
    /// Files the manifest already had, unchanged.
    pub unchanged: usize,
    /// Files that vanished and were dropped.
    pub removed: usize,
    /// Files that are not UTF-8.
    pub binary: usize,
    /// What the walk dropped, by reason.
    pub skips: Skips,
    /// Files a deadline stopped us from refreshing.
    pub stale: Vec<String>,
    /// Facts produced, by shape.
    pub counts: Counts,
    /// Tier-B facts produced, by shape.
    pub scip_counts: tier_b::Counts,
    /// Tier-A definitions that adopted a SCIP identity.
    pub anchored: usize,
    /// Languages found in the tree, so `index` can name the indexer for each.
    pub langs: BTreeSet<String>,
    /// SCIP inputs ingested.
    pub scip: Vec<ScipInput>,
    /// Symbols defined in more than one document across the merged inputs.
    pub scip_collisions: u64,
    /// SCIP documents dropped, with the reason.
    pub scip_skipped: Vec<(String, &'static str)>,
    /// How many ingested documents named a file this index holds. `None` when
    /// nothing was re-ingested and the manifest recorded no count.
    pub scip_matched: Option<u64>,
    /// Ingested documents that named no indexed file, in path order. Only
    /// collected when **none** matched, which is the case worth a report: a
    /// per-document miss is ordinary (a generated file, an excluded dir) and
    /// listing every one of them on a healthy index is noise.
    pub scip_unmatched: Vec<String>,
    /// Repo-relative prefixes that would re-base every unmatched document onto
    /// an indexed file. Reported, never applied (`specs/02-extraction.md`
    /// § Detection suggests; it never re-bases). More than one means the
    /// evidence does not pick a root and the report says so.
    pub scip_root_suggestions: Vec<String>,
    /// Indexed files modified after the newest SCIP input was built. Their
    /// `name_ref` rows are fresh and their `scip_ref` rows are not.
    pub scip_stale: Vec<String>,
    /// Files in the store that parsed with errors and the line of the first
    /// one, over the whole store and not only this run: an unchanged file that
    /// does not parse is still partial.
    pub syntax_errors: Vec<(String, u32)>,
    /// Files in a supported language that were not read, over the whole tree
    /// and not only this run: a file `index` skipped is still unread on the
    /// second `index`, and it is still where the answer might be.
    pub unread: BTreeMap<String, facts::SkipEntry>,
    /// Wall time.
    pub elapsed_ms: u128,
}

impl Report {
    /// True when the index is not a complete picture of the tree right now.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        !self.stale.is_empty()
    }

    /// How many tier-A definitions adopted a SCIP identity, as a percentage.
    ///
    /// The anchor rate is the health metric for the join: a drop means it is
    /// drifting, and `index` prints it so that the drop is visible before a
    /// query built on it is wrong (`specs/02-extraction.md` § Validation).
    #[must_use]
    pub fn anchor_rate(&self) -> Option<f64> {
        (self.counts.defs > 0 && !self.scip.is_empty()).then(|| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a percentage for a human; the counts are in the millions at most"
            )]
            {
                self.anchored as f64 * 100.0 / self.counts.defs as f64
            }
        })
    }
}

/// Walk, extract what changed, drop what vanished, and commit if anything did.
///
/// # Errors
/// I/O failure, or a file that will not extract. A file that is merely absent
/// or unreadable is counted, not fatal.
pub fn refresh(store: &mut Store, plan: &Plan) -> Result<Report> {
    let started = Instant::now();
    let root = store.root().to_path_buf();
    let mut report = Report::default();
    let before = store.manifest().clone();
    // The second this run started looking — what it may record as when it
    // looked (specs/04-storage.md § Manifest).
    let started_at = facts::manifest::now();

    store.sweep().context("sweeping what a crashed run left")?;
    let walked = walk(&root);
    report.skips = walked.skips;
    let visible = walked.visible;

    let mut unread: BTreeMap<String, facts::SkipEntry> = report
        .skips
        .too_large
        .iter()
        .map(|big| {
            (
                big.path.clone(),
                facts::SkipEntry {
                    reason: facts::SkipReason::TooLarge,
                    mtime: big.mtime,
                    size: big.size,
                },
            )
        })
        .collect();
    let mut decided: BTreeSet<String> = BTreeSet::new();

    let wanted: Vec<Candidate> = walked
        .files
        .into_iter()
        .filter(|c| plan.langs.is_empty() || plan.langs.iter().any(|l| l == c.lang.name))
        .collect();

    // A changed extractor invalidates every fact, not just the files a user
    // happens to touch afterwards (specs/04-storage.md § Manifest).
    let fingerprint = extract::fingerprint();
    let extractor_changed = store.manifest().extractor_fingerprint != fingerprint;

    // The SCIP phase: decide whether the index must be re-read, re-read it if
    // so, and record what it saw. Lifted out whole because it is a phase and
    // not a step -- every binding it produces is consumed below, and none of
    // the walk above depends on it.
    let scip = plan_scip(store, plan, &root, &wanted, extractor_changed, &mut report)?;

    let mut extractors: HashMap<&'static str, Extractor> = HashMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for candidate in wanted {
        seen.insert(candidate.path.clone());
        report.langs.insert(candidate.lang.name.to_string());
        if plan.deadline.is_some_and(|d| Instant::now() >= d) {
            report.stale.push(candidate.path);
            continue;
        }
        decided.insert(candidate.path.clone());
        // Two halves, and the split is where the I/O stops being a decision
        // and starts being work: `pending` decides, `extract` does.
        let Some(pending) = pending(
            store,
            plan,
            &scip,
            before.indexed_at,
            &candidate,
            &mut report,
            &mut unread,
        ) else {
            continue;
        };
        extract(
            store,
            &scip,
            &mut extractors,
            &candidate,
            pending,
            &mut report,
        )?;
    }

    // Files only tier B covers keep their segments across a refresh that did
    // not re-read the SCIP index; without this they look vanished and their
    // facts are dropped. Not when the inputs changed without being re-read,
    // which is `index.scip` going away: every fact those files hold came from
    // it, and keeping them would answer `no-scip` beside rows marked `exact`.
    if !scip.reingest && !scip.scip_changed {
        let kept: Vec<String> = store
            .manifest()
            .files
            .iter()
            .filter(|(path, entry)| entry.tiers == ["scip"] && root.join(path).exists())
            .map(|(path, _)| path.clone())
            .collect();
        seen.extend(kept);
    }

    tier_b_only(
        store,
        plan,
        &root,
        &scip.ingest,
        &visible,
        &mut seen,
        &mut report,
    )?;

    forget_vanished(store, plan, &seen, &mut report);

    measure_coverage(
        &scip.ingest,
        &seen,
        scip.reingest,
        store.manifest().scip_matched,
        &mut report,
    );

    // The same rule `query` acts on, read from the manifest this run built
    // (`FileEntry::scip_stale`).
    if let Some(newest) = scip.newest_scip {
        report.scip_stale = store
            .manifest()
            .files
            .iter()
            .filter(|(path, entry)| seen.contains(*path) && entry.scip_stale(newest))
            .map(|(path, _)| path.clone())
            .collect();
    }

    // A file this run did not re-decide keeps what the last run recorded: a
    // deadline stopped it, or `--lang` left its language out. Too large is
    // always re-derived from the walk, which no flag narrows.
    for (path, entry) in &before.skipped {
        if entry.reason != facts::SkipReason::TooLarge
            && visible.contains(path)
            && !decided.contains(path)
        {
            unread.entry(path.clone()).or_insert_with(|| entry.clone());
        }
    }
    store.manifest_mut().skipped.clone_from(&unread);
    report.unread = unread;

    record(store, plan, scip, fingerprint, started_at, &before, &report)?;
    report.syntax_errors = crate::query::syntax_errors(store.manifest())
        .into_iter()
        .map(|(path, line)| (path.to_string(), line))
        .collect();
    report.elapsed_ms = started.elapsed().as_millis();
    Ok(report)
}

/// Write what this run learned onto the manifest, and commit if it moved.
///
/// # Errors
/// I/O failure writing the dictionary or the manifest.
fn record(
    store: &mut Store,
    plan: &Plan,
    scip: ScipPlan,
    fingerprint: String,
    started_at: u64,
    before: &facts::Manifest,
    report: &Report,
) -> Result<()> {
    // One rule, asked twice before and now named once: did this run
    // re-extract everything the fingerprint and the timestamp are claims
    // about? A `--lang` run, or one a deadline stopped, leaves files built by
    // the old extractor — and writing either stamp over them would make the
    // next full run trust them (`specs/04-storage.md` § Manifest).
    let complete = plan.langs.is_empty() && report.stale.is_empty();
    if complete {
        store.manifest_mut().extractor_fingerprint = fingerprint;
    }
    store.manifest_mut().scip = scip.inputs;
    store.manifest_mut().scip_collisions = scip.scip_collisions;
    store.manifest_mut().scip_matched = report.scip_matched;
    // A refresh that changed nothing writes nothing. `query` refreshes before
    // every answer, and committing the same manifest again is an fsync per
    // read. Every write this run made lands in the manifest, so equality is
    // the whole test. A store with no index yet commits regardless, so
    // indexing an empty tree still leaves an index
    // (`specs/04-storage.md` § Incremental reindex).
    if !store.has_index() || *store.manifest() != *before {
        // Inside the commit test, not before it: a read that changed nothing
        // must not start writing a new timestamp.
        if complete {
            store.manifest_mut().indexed_at = started_at;
        }
        store.commit().context("committing the index")?;
    }
    Ok(())
}

/// Throw the index away, keeping only the dictionary generation counter.
///
/// `--rebuild` renumbers every atom, and `query --raw` hands raw atom ids to
/// the caller, so the generation is what stops a saved id from resolving to a
/// *different string* rather than to an error
/// (`specs/04-storage.md` § Manifest).
///
/// # Errors
/// I/O failure removing the contents of the index directory `dir`.
pub fn discard(dir: &Path) -> Result<u64> {
    let generation = facts::Manifest::open(dir)
        .ok()
        .flatten()
        .map_or(0, |m| m.dict_generation);
    if dir.exists() {
        // Everything but the lock. The caller holds it, and a second writer
        // that opened a fresh lock file would not contend with an fd on the
        // unlinked one (`specs/04-storage.md` § Concurrency).
        for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if path.file_name().is_some_and(|n| n == facts::lock::LOCK) {
                continue;
            }
            if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            }
            .with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(generation.saturating_add(1))
}

/// Stat every SCIP input. Cheap, and enough to decide whether to parse them.
///
/// `tool`, `documents` and `ambiguous` are filled in by [`read_scip`]; a
/// missing input is not an error, since `./index.scip` is a default and its
/// absence just means tier A only.
/// Drop the facts of every file the manifest still carries and this run did
/// not see.
fn forget_vanished(store: &mut Store, plan: &Plan, seen: &BTreeSet<String>, report: &mut Report) {
    // A refresh that only added would leave a deleted file's facts answering
    // queries (specs/05-surface.md § `query`). A language `--lang` left out
    // was not walked, so its files were never `seen`; they are carried forward
    // unchanged rather than mistaken for vanished (specs/05-surface.md
    // § `index`). A file only tier B covers is not carried by its language:
    // nothing walks it in any run, and it is `seen` above exactly when its
    // SCIP input still stands.
    let walked = |lang: &str| plan.langs.is_empty() || plan.langs.iter().any(|l| l == lang);
    let vanished: Vec<String> = store
        .manifest()
        .files
        .iter()
        .filter(|(path, entry)| {
            !seen.contains(*path) && (entry.tiers == ["scip"] || walked(&entry.lang))
        })
        .map(|(path, _)| path.clone())
        .collect();
    for path in vanished {
        if store.forget(&path) {
            report.removed += 1;
        }
    }
}

/// What the SCIP phase decided, and what it read if it read anything.
struct ScipPlan {
    /// The inputs differ from the ones the manifest records.
    scip_changed: bool,
    /// Every file must be re-extracted, whatever its mtime says.
    invalidated: bool,
    /// The newest input's mtime, when there is one.
    newest_scip: Option<u64>,
    /// An ingest ran this pass, so there is something that could have seen a
    /// file's current bytes.
    reingest: bool,
    /// What it read. Empty when nothing was read.
    ingest: Ingest,
    /// What the manifest will record as this index's SCIP inputs.
    inputs: Vec<ScipInput>,
    /// Symbols defined in more than one document across the merged inputs.
    scip_collisions: u64,
}

impl ScipPlan {
    /// What this pass's SCIP inputs mean for one file: whether they saw its
    /// current bytes, and the `scip_hash` its manifest entry should carry.
    ///
    /// `known` is the `scip_hash` the manifest already holds for this path.
    ///
    /// Lifted out of the per-file loop because it is the one piece of that
    /// loop with no I/O in it and the one that is hard to read — four inputs
    /// deciding two outputs, where getting either wrong is silent. `scip_saw`
    /// wrong in one direction anchors a file against stale positions and
    /// adopts the wrong identity; wrong in the other drops tier B for a file
    /// the indexer did see. `scip_hash` wrong makes `scip-stale` lie. Pure, so
    /// the table below is a test and not an argument.
    fn view(&self, mtime: u64, hash: &str, known: Option<&str>) -> (bool, Option<String>) {
        // Did the inputs see these bytes? By mtime, or by the hash the
        // manifest recorded at the last ingest of the same inputs — which is
        // what lets a file edited and then restored stop being `scip-stale`
        // without rerunning the indexer. A file they did not see is tier A
        // only: its positions have moved, so anchoring against the old ones
        // would adopt the wrong identity or none, and a tier-B-only `def`
        // beside the tier-A one is two rows for one definition
        // (`specs/02-extraction.md` § The anchor join). With no re-ingest
        // planned nothing was read, and an index that was not read saw
        // nothing.
        let saw = self.reingest
            && (self.newest_scip.is_some_and(|scip| mtime <= scip)
                || (!self.scip_changed && known == Some(hash)));
        // What the inputs last saw, kept across an edit they did not see so a
        // restore can match it; `""` when they never saw any version.
        let scip_hash = if self.inputs.is_empty() {
            None
        } else if saw {
            Some(hash.to_string())
        } else if self.reingest {
            known
                .map(ToString::to_string)
                .filter(|_| !self.scip_changed)
                .or_else(|| Some(String::new()))
        } else {
            known.map(ToString::to_string)
        };
        (saw, scip_hash)
    }
}

/// Index the files only tier B covers: a language no grammar handles, or one
/// the `--lang` filter dropped. They get an ordinary per-file segment rather
/// than the single `_scip.bin` blob in specs/04-storage.md § Layout — see the
/// note there.
///
/// `visible` is the walk's membership list, and a document naming a path that
/// is not in it is refused, not indexed (`specs/02-extraction.md` § Budget).
///
/// # Errors
/// An exhausted dictionary, or a failure to write a segment.
fn tier_b_only(
    store: &mut Store,
    plan: &Plan,
    root: &Path,
    ingest: &Ingest,
    visible: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
    report: &mut Report,
) -> Result<()> {
    for (path, doc) in &ingest.docs {
        if seen.contains(path) || plan.deadline.is_some_and(|d| Instant::now() >= d) {
            continue;
        }
        if !visible.contains(path) {
            // The walk is the only statement about which paths belong to this
            // tree, and it did not reach this one: `.gitignore` excluded it,
            // it is hidden, or it sits behind a symlink the walk does not
            // follow. Indexing it here would re-admit through tier B exactly
            // what tier A was told to skip — on a monorepo ignoring
            // `packages/*/build`, every generated `.d.ts` the indexer covered,
            // one duplicate `def` per already-indexed export (#54,
            // `specs/02-extraction.md` § Budget).
            report.scip_skipped.push((path.clone(), "ignored"));
            continue;
        }
        let Ok(meta) = std::fs::metadata(root.join(path)) else {
            // SCIP names a file that is not here. Emitting facts about it would
            // answer questions about source nobody can open.
            continue;
        };
        seen.insert(path.clone());
        let mut segment = facts::Segment::new();
        let file = store.intern(path).context("the dictionary is full")?;
        let lang = store.intern(&doc.lang).context("the dictionary is full")?;
        segment.push("file", &[file, lang]);
        let scip_counts = tier_b::emit(
            &mut segment,
            ingest,
            path,
            std::fs::read_to_string(root.join(path)).ok().as_deref(),
            &[],
            store.interner_mut(),
        )
        .with_context(|| format!("ingesting SCIP facts for {path}"))?;
        add(&mut report.scip_counts, scip_counts);
        store
            .put(
                path,
                &mut segment,
                NewEntry {
                    mtime: walk::mtime_of(&meta),
                    size: meta.len(),
                    hash: String::new(),
                    lang: doc.lang.clone(),
                    tiers: vec!["scip".to_string()],
                    // Nothing walks it, so nothing edits it as far as this
                    // index can tell: never stale.
                    scip_hash: Some(String::new()),
                    syntax_error: None,
                },
            )
            .with_context(|| format!("writing the segment for {path}"))?;
        report.indexed += 1;
    }
    Ok(())
}

/// Stat the SCIP inputs, decide whether they must be re-read, and re-read them
/// if so, recording what they saw on `report`.
///
/// # Errors
/// Propagates a failure to read or parse an `index.scip`.
fn plan_scip(
    store: &Store,
    plan: &Plan,
    root: &Path,
    wanted: &[Candidate],
    extractor_changed: bool,
    report: &mut Report,
) -> Result<ScipPlan> {
    // Stat the SCIP inputs, which is cheap, and compare before parsing them,
    // which is not. SCIP is all-or-nothing: a reference that *disappeared*
    // from a document leaves no evidence anywhere else, so there is nothing to
    // drive its removal per-document. A changed input re-extracts everything
    // (specs/04-storage.md § Incremental reindex).
    let mut inputs = stat_scip(root, plan);
    let known_scip = &store.manifest().scip;
    let scip_changed = known_scip.len() != inputs.len()
        || !known_scip.iter().zip(&inputs).all(|(a, b)| a.same_bytes(b));
    // Carry forward what only a parse can tell us, so a refresh that does not
    // re-read the index does not blank the manifest's record of it.
    if !scip_changed {
        for (into, known) in inputs.iter_mut().zip(known_scip) {
            into.tool.clone_from(&known.tool);
            into.documents = known.documents;
            into.ambiguous = known.ambiguous;
        }
    }
    // A collision belongs to the merge, not to an input, so it is carried and
    // recorded once (specs/04-storage.md § Manifest).
    let mut scip_collisions = if scip_changed || inputs.is_empty() {
        0
    } else {
        store.manifest().scip_collisions
    };
    let invalidated = extractor_changed || scip_changed;
    let newest_scip = inputs.iter().map(|i| i.mtime).max();

    // Parsing a large `index.scip` on every auto-refresh would put it on the
    // query path for nothing. Parse it only when a file actually needs
    // re-extracting; a refresh that changes nothing reads no protobuf at all.
    // By mtime and size alone, not the racy-clean rule: a tree touched in the
    // indexing second would otherwise decode `index.scip` on every read. A racy
    // file that did change is re-extracted tier A only below, and `scip-stale`.
    let stale_file = wanted.iter().any(|c| {
        !store
            .manifest()
            .files
            .get(&c.path)
            .is_some_and(|e| e.looks_unchanged(c.mtime, c.size))
    });
    let reingest = !inputs.is_empty() && (plan.rebuild || invalidated || stale_file);
    let (ingest, own) = if reingest {
        read_scip(root, plan)?
    } else {
        (Ingest::default(), BTreeMap::new())
    };
    report.scip_skipped.clone_from(&ingest.skipped);
    if reingest {
        scip_collisions = ingest.collisions.len() as u64;
        for input in &mut inputs {
            let own = own.get(&input.path).cloned().unwrap_or_default();
            input.tool = own.tool;
            input.documents = own.documents;
            input.ambiguous = own.ambiguous;
        }
    }
    report.scip.clone_from(&inputs);
    report.scip_collisions = scip_collisions;
    Ok(ScipPlan {
        scip_changed,
        invalidated,
        newest_scip,
        reingest,
        ingest,
        inputs,
        scip_collisions,
    })
}

/// How many ingested documents named a file this index holds, and — when none
/// did — which ones and what root would have landed them.
///
/// Measured once `seen` holds every path this index knows. A document that
/// named no indexed file produced nothing, and an index of which *none* did is
/// a tier-A index wearing a SCIP input: invariant 5's silent degradation, and
/// the whole of #60. Carried forward when nothing was re-ingested, like the
/// collision count, so a refresh that read no protobuf does not blank what the
/// last one measured.
fn measure_coverage(
    ingest: &Ingest,
    seen: &BTreeSet<String>,
    reingest: bool,
    known: Option<u64>,
    report: &mut Report,
) {
    if !reingest {
        report.scip_matched = known;
        return;
    }
    let matched = ingest.docs.keys().filter(|p| seen.contains(*p)).count() as u64;
    report.scip_matched = Some(matched);
    // Carried, not just placed: a document refused before it was placed still
    // named a path, and a wrong root refuses all of them (#123).
    let docs: BTreeSet<String> = ingest
        .docs
        .keys()
        .cloned()
        .chain(ingest.skipped.iter().map(|(path, _)| path.clone()))
        .collect();
    if matched > 0 || docs.is_empty() {
        return;
    }
    report.scip_root_suggestions = extract::scip::suggest_roots(&docs, seen);
    report.scip_unmatched = docs.into_iter().collect();
}

fn stat_scip(root: &Path, plan: &Plan) -> Vec<ScipInput> {
    let mut inputs: Vec<ScipInput> = plan
        .scip
        .iter()
        .enumerate()
        .filter_map(|(i, path)| {
            let meta = std::fs::metadata(absolute(root, path)).ok()?;
            Some(ScipInput {
                path: path.display().to_string().replace('\\', "/"),
                root: plan.root_for(i).to_string(),
                tool: String::new(),
                mtime: walk::mtime_of(&meta),
                size: meta.len(),
                documents: 0,
                ambiguous: 0,
            })
        })
        .collect();
    inputs.sort_by(|a, b| a.path.cmp(&b.path));
    inputs.dedup_by(|a, b| a.path == b.path);
    inputs
}

/// Read every SCIP input and merge them.
///
/// Multiple indexes are ingested independently — symbol strings are globally
/// unique by construction, so there is no merge logic beyond concatenation
/// (`specs/02-extraction.md` § Acquisition).
///
/// Sorted and deduplicated exactly as [`stat_scip`] sorts its inputs. Any
/// symbol two indexes both describe is last-wins, so reading in CLI order made
/// `--scip a --scip b` and `--scip b --scip a` write different manifests from
/// the same files.
///
/// Beside the merge, what each input said about itself, keyed as
/// [`stat_scip`] keys the input. A per-input number cannot be recovered from
/// the merge, and storing the merged one on every input counts it once per
/// input (specs/04-storage.md § Manifest).
fn read_scip(root: &Path, plan: &Plan) -> Result<(Ingest, BTreeMap<String, Own>)> {
    let key = |p: &PathBuf| p.display().to_string().replace('\\', "/");
    // Each input carries its own `--scip-root`, so the sort that makes the
    // merge order-independent has to move the pairs, not the paths.
    let mut paths: Vec<(&PathBuf, &str)> = plan
        .scip
        .iter()
        .enumerate()
        .map(|(i, path)| (path, plan.root_for(i)))
        .collect();
    paths.sort_by_key(|(p, _)| key(p));
    paths.dedup_by_key(|(p, _)| key(p));
    let mut merged = Ingest::default();
    let mut own = BTreeMap::new();
    for (path, under) in paths {
        let absolute = absolute(root, path);
        if !absolute.exists() {
            continue;
        }
        let one = Ingest::read(&absolute, root, under)
            .with_context(|| format!("reading {}", absolute.display()))?;
        own.insert(
            key(path),
            Own {
                tool: one.tool,
                documents: (one.docs.len() + one.skipped.len()) as u64,
                ambiguous: one.ambiguous as u64,
            },
        );
        merged.docs.extend(one.docs);
        merged.symbols.extend(one.symbols);
        merged.externs.extend(one.externs);
        merged.skipped.extend(one.skipped);
    }
    merged.recount();
    Ok((merged, own))
}

/// One SCIP input's own `tool`, `documents` and `ambiguous`, read from it
/// alone.
#[derive(Debug, Clone, Default)]
struct Own {
    tool: String,
    documents: u64,
    ambiguous: u64,
}

fn absolute(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn add(into: &mut tier_b::Counts, counts: tier_b::Counts) {
    into.refs += counts.refs;
    into.resolved += counts.resolved;
    into.only += counts.only;
    into.collided += counts.collided;
}

/// A file that has to be extracted, and what this pass already knows about it.
struct Pending {
    /// Its current contents.
    src: String,
    /// The hash of those bytes.
    hash: String,
    /// Whether this pass's SCIP inputs saw them (`ScipPlan::view`).
    scip_saw: bool,
    /// The `scip_hash` its manifest entry should carry (`ScipPlan::view`).
    scip_hash: Option<String>,
}

/// Decide what this pass owes one walked file, and do the cheap outcomes here.
///
/// `None` means the file is accounted for — unchanged, unreadable, or not
/// UTF-8 — and `report` already says which. `Some` means it has to be
/// extracted, and carries the bytes this function had to read anyway.
///
/// Four of the five outcomes are "no extraction", which is why they live
/// together: the common case for a `query` auto-refresh is that every file
/// takes one of them, and reading the fast paths as one block is what makes
/// the cost of a refresh legible (`specs/04-storage.md` § Incremental
/// reindex).
fn pending(
    store: &mut Store,
    plan: &Plan,
    scip: &ScipPlan,
    indexed_at: u64,
    candidate: &Candidate,
    report: &mut Report,
    unread: &mut BTreeMap<String, facts::SkipEntry>,
) -> Option<Pending> {
    let mut skip = |reason| {
        unread.insert(
            candidate.path.clone(),
            facts::SkipEntry {
                reason,
                mtime: candidate.mtime,
                size: candidate.size,
            },
        );
    };
    let known = store.manifest().files.get(&candidate.path);
    if !plan.rebuild
        && !scip.invalidated
        && known.is_some_and(|e| e.is_clean(candidate.mtime, candidate.size, indexed_at))
    {
        report.unchanged += 1;
        return None;
    }

    let Ok(bytes) = std::fs::read(&candidate.abs) else {
        // Vanished or unreadable between the walk and here. Report it
        // rather than dropping its facts on a transient error.
        report.skips.unreadable.push(candidate.path.clone());
        skip(facts::SkipReason::Unreadable);
        return None;
    };
    let hash = facts::content_hash(&bytes);
    if !plan.rebuild
        && !scip.invalidated
        && let Some(same) = known.filter(|e| e.hash == hash)
    {
        // Touched but not changed: refresh the fast-path fields so the next
        // run does not hash it again.
        let mut entry = same.clone();
        entry.mtime = candidate.mtime;
        entry.size = candidate.size;
        store
            .manifest_mut()
            .files
            .insert(candidate.path.clone(), entry);
        report.unchanged += 1;
        return None;
    }
    let Ok(src) = String::from_utf8(bytes) else {
        report.binary += 1;
        skip(facts::SkipReason::NotUtf8);
        return None;
    };
    let known_scip = known.and_then(|e| e.scip_hash.as_deref());
    let (scip_saw, scip_hash) = scip.view(candidate.mtime, &hash, known_scip);
    Some(Pending {
        src,
        hash,
        scip_saw,
        scip_hash,
    })
}

/// Extract one file, both tiers, and write its segment.
///
/// # Errors
/// A grammar that will not load, an extractor failure, an exhausted
/// dictionary, or a failure to write the segment.
fn extract(
    store: &mut Store,
    scip: &ScipPlan,
    extractors: &mut HashMap<&'static str, Extractor>,
    candidate: &Candidate,
    file: Pending,
    report: &mut Report,
) -> Result<()> {
    let extractor = match extractors.entry(candidate.lang.name) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => e.insert(
            Extractor::new(candidate.lang)
                .with_context(|| format!("preparing the {} extractor", candidate.lang.name))?,
        ),
    };
    // Tier A runs first, entirely, then tier B — the join needs tier A's
    // `def_name` index to anchor against (specs/02-extraction.md).
    let anchors = if file.scip_saw {
        Anchors::of(&scip.ingest, &candidate.path)
    } else {
        Anchors::default()
    };
    let mut extracted = extractor
        .file(&candidate.path, &file.src, store.interner_mut(), &anchors)
        .with_context(|| format!("extracting {}", candidate.path))?;
    report.counts.defs += extracted.counts.defs;
    report.counts.refs += extracted.counts.refs;
    report.counts.imports += extracted.counts.imports;
    report.anchored += extracted.anchored;

    let scip_counts = if file.scip_saw {
        tier_b::emit(
            &mut extracted.segment,
            &scip.ingest,
            &candidate.path,
            Some(&file.src),
            &extracted.defs,
            store.interner_mut(),
        )
        .with_context(|| format!("ingesting SCIP facts for {}", candidate.path))?
    } else {
        tier_b::Counts::default()
    };
    add(&mut report.scip_counts, scip_counts);

    let mut entry = new_entry(candidate, &file.hash);
    entry.scip_hash = file.scip_hash;
    entry.syntax_error = extracted.syntax_error;
    if !anchors.is_empty() || scip_counts.refs > 0 {
        entry.tiers.push("scip".to_string());
    }
    let mut segment = extracted.segment;
    store
        .put(&candidate.path, &mut segment, entry)
        .with_context(|| format!("writing the segment for {}", candidate.path))?;
    report.indexed += 1;
    Ok(())
}

fn new_entry(candidate: &Candidate, hash: &str) -> NewEntry {
    NewEntry {
        mtime: candidate.mtime,
        size: candidate.size,
        hash: hash.to_string(),
        lang: candidate.lang.name.to_string(),
        tiers: vec!["ts".to_string()],
        scip_hash: None,
        syntax_error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan with no inputs read and none recorded: the tier-A-only case.
    fn plan() -> ScipPlan {
        ScipPlan {
            scip_changed: false,
            invalidated: false,
            newest_scip: None,
            reingest: false,
            ingest: Ingest::default(),
            inputs: Vec::new(),
            scip_collisions: 0,
        }
    }

    /// One recorded input. Only `inputs.is_empty()` is read by `view`, so
    /// the fields are placeholders — spelled out rather than defaulted
    /// because `ScipInput` deliberately has no `Default` (#48).
    fn input() -> ScipInput {
        ScipInput {
            path: "index.scip".to_string(),
            tool: String::new(),
            root: String::new(),
            mtime: 0,
            size: 0,
            documents: 0,
            ambiguous: 0,
        }
    }

    /// A plan that read inputs whose newest document is `newest`.
    fn ingested(newest: u64, changed: bool) -> ScipPlan {
        ScipPlan {
            scip_changed: changed,
            newest_scip: Some(newest),
            reingest: true,
            inputs: vec![input()],
            ..plan()
        }
    }

    #[test]
    fn with_no_scip_at_all_nothing_is_seen_and_nothing_is_recorded() {
        assert_eq!(plan().view(10, "h", None), (false, None));
        // Even a file the manifest once had a hash for: with no inputs
        // recorded there is no SCIP index to be stale against.
        assert_eq!(plan().view(10, "h", Some("old")), (false, None));
    }

    #[test]
    fn a_file_older_than_the_index_was_seen() {
        assert_eq!(
            ingested(100, false).view(50, "h", None),
            (true, Some("h".to_string()))
        );
        // Equal counts as seen: the index was built at or after this write.
        assert_eq!(
            ingested(100, false).view(100, "h", None),
            (true, Some("h".to_string()))
        );
    }

    #[test]
    fn a_file_newer_than_the_index_was_not_seen() {
        let (saw, hash) = ingested(100, false).view(101, "h", None);
        assert!(!saw);
        // Empty string, not `None`: the inputs exist and never saw any
        // version of this file, which is what `scip-stale` reads.
        assert_eq!(hash, Some(String::new()));
    }

    #[test]
    fn an_edit_reverted_to_bytes_the_index_saw_is_seen_again() {
        // The whole point of recording the hash: no indexer rerun needed.
        assert_eq!(
            ingested(100, false).view(999, "h", Some("h")),
            (true, Some("h".to_string()))
        );
    }

    #[test]
    fn changed_inputs_make_the_recorded_hash_worthless() {
        // The hash was recorded against *different* inputs, so matching it
        // says nothing about what these ones saw.
        let (saw, hash) = ingested(100, true).view(999, "h", Some("h"));
        assert!(!saw);
        assert_eq!(hash, Some(String::new()));
    }

    #[test]
    fn without_a_reingest_the_recorded_hash_is_carried_forward() {
        // Nothing was read this pass, so nothing was learned: the entry keeps
        // what it had rather than claiming the inputs saw it.
        let carried = ScipPlan {
            inputs: vec![input()],
            ..plan()
        };
        assert_eq!(
            carried.view(999, "h", Some("old")),
            (false, Some("old".to_string()))
        );
    }
}
