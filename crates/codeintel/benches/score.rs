//! The scoreboard over the pinned corpora: `docs/50-self-improvement.md` § 3.
//!
//! ```sh
//! provision apply --stream tasks/score.yml                      # checks out and indexes every corpus first
//! cargo bench -p codeintel --bench score -- <name> <dir> <scip> # or one corpus directly
//! UPDATE_BASELINE=1 cargo bench -p codeintel --bench score -- <name> <dir> <scip>
//! ```
//!
//! `<name>` picks `benches/corpora/<name>/`, which holds the `queries.tsv` the
//! wall-clock bench already uses and this bench's own `score-baseline.json`.
//! The result is `target/score/<name>.json`.
//!
//! **No clock.** `benches/corpus.rs` measures the clock and this one must not:
//! the host is a 10-core M5 under an EDR tax and the committed timings were
//! taken elsewhere (`docs/50-self-improvement.md` § 1). Every number here is a
//! byte count or a set cardinality — exactly deterministic, machine-
//! independent, and therefore safe to gate on anywhere.
//!
//! **The corpus is read, never copied.** `scip-stale` is decided by mtime
//! (`index.rs`, `ScipPlan::view`), and a copied tree's mtimes are all *now*, so
//! every file reads as edited after the indexer ran and tier B is dropped
//! wholesale — measured: `0 refs, 0 resolved`, `anchored 0/629`, and a
//! scoreboard of zeroes that looks like a result. The index directory is the
//! only thing this bench writes.
//!
//! **A corpus whose SCIP status is not `ok` is refused.** A stale index
//! inflates the name-only side of every pair and makes precision lie, so the
//! file is not written at all. `specs/00-overview.md` invariant 5 binds the
//! measurement apparatus exactly as it binds the tool.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use codeintel::census::{Census, Report};
use codeintel::index::{self, Plan};
use codeintel::overlay::Overlay;
use codeintel::status::Status;
use codeintel::{Regexes, STDLIB};
use datalog::{Atom, Engine, Limits, Relation};
use facts::Store;
use serde_json::{Value, json};

/// One stdlib rule and the `_exact` twin that is its oracle.
///
/// Six pairs ship, and the roster is `rules/stdlib.dl`'s own
/// (`docs/50-self-improvement.md` § 1). The name twin is a text-matching
/// *prediction* of the twin the compiler answered, so the pair yields
/// precision and recall with no human and no clock.
struct Pair {
    /// The rule's name, and the key it is reported under.
    rule: &'static str,
    /// `positive` -- the `_exact` twin is a filter, so `exact_only` is 0 by
    /// construction. `negated` -- negation inverts the containment, so
    /// `exact_only` is meaningful and means the opposite of a recall miss
    /// (`docs/50-self-improvement.md` section 2).
    shape: &'static str,
    /// The goal that asks the name twin.
    name_goal: &'static str,
    /// The goal that asks the oracle.
    exact_goal: &'static str,
    /// Leading columns of `name_goal` that make a row's identity. The two
    /// goals must agree on them, column for column.
    key: usize,
    /// The column of `name_goal` carrying `Prov`, when the rule has one.
    ///
    /// Only `ref` does. The other five name twins hand the consumer a tuple
    /// with no provenance in it at all, which is what the calibration metric
    /// found out (§ 2).
    prov: Option<usize>,
    /// For those five, the goal over `tier/4` that hands the provenance back
    /// (`specs/01-facts.md` § Row provenance). Its rows, projected onto the
    /// first `key` columns, must be exactly `name_goal`'s: a `tier` that drops
    /// or invents a row is an error here, not a better calibration. Its last
    /// column is `Prov`. The oracle (`exact_goal`) is untouched and does not
    /// read `tier`, so the measurement is not circular.
    tier_goal: Option<&'static str>,
}

/// The six pairs, in `docs/50-self-improvement.md` § 3's order.
///
/// `ref` is the odd one: its provenance is a column rather than a separate
/// predicate, so the oracle is the same rule with `Prov` bound and the
/// prediction is the whole relation projected onto the six columns before it.
const PAIRS: &[Pair] = &[
    Pair {
        rule: "calls",
        shape: "positive",
        name_goal: "?- calls(A, B).",
        exact_goal: "?- calls_exact(A, B).",
        key: 2,
        prov: None,
        tier_goal: Some("?- tier(\"calls\", A, B, Prov)."),
    },
    Pair {
        rule: "dead_export",
        shape: "negated",
        name_goal: "?- dead_export(S).",
        exact_goal: "?- dead_export_exact(S).",
        key: 1,
        prov: None,
        tier_goal: Some("?- tier(\"dead_export\", S, B, Prov)."),
    },
    Pair {
        rule: "entrypoint",
        shape: "negated",
        name_goal: "?- entrypoint(S).",
        exact_goal: "?- entrypoint_exact(S).",
        key: 1,
        prov: None,
        tier_goal: Some("?- tier(\"entrypoint\", S, B, Prov)."),
    },
    Pair {
        rule: "depends",
        shape: "positive",
        name_goal: "?- depends(F, G).",
        exact_goal: "?- depends_exact(F, G).",
        key: 2,
        prov: None,
        tier_goal: Some("?- tier(\"depends\", F, G, Prov)."),
    },
    Pair {
        rule: "uses_package",
        shape: "positive",
        name_goal: "?- uses_package(F, P).",
        exact_goal: "?- uses_package_exact(F, P).",
        key: 2,
        prov: None,
        tier_goal: Some("?- tier(\"uses_package\", F, P, Prov)."),
    },
    Pair {
        rule: "ref",
        shape: "positive",
        name_goal: "?- ref(S, F, L, C, From, Role, Prov).",
        exact_goal: "?- ref(S, F, L, C, From, Role, \"exact\").",
        key: 6,
        prov: Some(6),
        tier_goal: None,
    },
];

/// Every scoreboard number that is judged, and which way is better.
///
/// Counts — `confirmed`, `unconfirmed`, `exact_only`, `files`, `defs` — are
/// reported and not judged: they move whenever the rules move, and reading the
/// diff is the point. The ratios above them are what a hypothesis targets.
const JUDGED: &[(&str, Direction)] = &[
    ("/scip/anchor_rate", Direction::Higher),
    ("/cost/schema_bytes", Direction::Lower),
    ("/cost/answer_bytes_p50", Direction::Lower),
    ("/cost/answer_bytes_p95", Direction::Lower),
    ("/cost/answer_bytes_total", Direction::Lower),
    ("/cost/bytes_per_row", Direction::Lower),
    ("/coverage/span_coverage", Direction::Higher),
    ("/coverage/role_known", Direction::Higher),
    ("/coverage/scip_impl_rows", Direction::Higher),
];

/// Which way a metric improves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Fewer bytes is better.
    Lower,
    /// A higher ratio is better.
    Higher,
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("score: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<bool> {
    // `cargo bench` passes `--bench` to a harness-less target; flags are not
    // ours.
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let [name, corpus, scip] = args.as_slice() else {
        bail!("usage: cargo bench -p codeintel --bench score -- <name> <corpus-dir> <scip-file>");
    };
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec = manifest_dir.join("benches/corpora").join(name);
    let set = query_set(&spec)?;
    let corpus = PathBuf::from(corpus)
        .canonicalize()
        .context("the corpus directory")?;
    let scip = PathBuf::from(scip)
        .canonicalize()
        .context("the SCIP index")?;
    let rev = git_head(&corpus);

    // Outside both trees: `specs/04-storage.md` § Location, and the corpus
    // checkout stays byte-identical to its pin.
    let tmp = tempfile::tempdir().context("tempdir")?;
    let index_dir = tmp.path().join("index");
    println!("corpus {} at {rev}", corpus.display());
    let (files, defs) = build(&corpus, &index_dir, &scip)?;
    println!("indexed {files} files, {defs} defs");

    let census = read_census(&corpus, &index_dir)?;
    let Some(anchor_rate) = gate(&corpus, &index_dir, &census)? else {
        return Ok(false);
    };

    let cost = cost(&corpus, &index_dir, &set)?;
    println!(
        "schema {} bytes; answers p50 {} p95 {} total {} over {} rows",
        cost.schema_bytes, cost.p50, cost.p95, cost.total, cost.rows
    );

    let confirmation = confirmation(&corpus, &index_dir)?;
    let coverage = coverage(&census);

    let current = json!({
        "corpus": { "name": name, "rev": rev, "files": files, "defs": defs },
        // Which build measured this. `corpus.rev` pins the tree that was
        // measured; this pins the code that did the measuring, and without it
        // a scoreboard cannot be quoted as any particular build's (#78).
        "tool": tool_rev(),
        // `index` is the oracle's identity, and it is load-bearing for the
        // same reason `corpus.rev` is. `rust-analyzer scip` on one pinned
        // checkout is **not** a function of that checkout: run cold it
        // resolves less than run against a warm `target/`. Measured on fd at
        // its pin — 928 KB cold, 949 KB warm, `ref` tp 8288 against 8590, and
        // every precision moved. Two indexes, one tree, two answers, so a
        // baseline taken against one of them says nothing about the other.
        "scip": {
            "status": Status::Ok.as_str(),
            "anchor_rate": anchor_rate,
            "index": facts::content_hash(&std::fs::read(&scip)?),
        },
        "cost": {
            "schema_bytes": cost.schema_bytes,
            "answer_bytes_p50": cost.p50,
            "answer_bytes_p95": cost.p95,
            "answer_bytes_total": cost.total,
            "bytes_per_row": ratio(cost.total, cost.rows),
        },
        "confirmation": confirmation,
        "coverage": coverage,
    });

    let out = manifest_dir
        .join("../../target/score")
        .join(format!("{name}.json"));
    write_json(&out, &current)?;
    println!("wrote {}", out.display());

    let baseline = spec.join("score-baseline.json");
    if std::env::var_os("UPDATE_BASELINE").is_some() {
        write_json(&baseline, &current)?;
        println!("wrote {}", baseline.display());
        return Ok(true);
    }
    let Ok(text) = std::fs::read_to_string(&baseline) else {
        println!(
            "no baseline at {}: nothing compared. UPDATE_BASELINE=1 writes one",
            baseline.display()
        );
        return Ok(false);
    };
    let base: Value = serde_json::from_str(&text).context("score-baseline.json")?;
    Ok(compare(&base, &current))
}

/// Index `root` with `scip` into `index_dir`, and say what it found.
fn build(root: &Path, index_dir: &Path, scip: &Path) -> Result<(usize, usize)> {
    index::discard(index_dir)?;
    let mut store = Store::open(root, index_dir.to_path_buf(), &extract::fingerprint())?;
    let plan = Plan {
        scip: vec![scip.to_path_buf()],
        ..Plan::default()
    };
    let report = index::refresh(&mut store, &plan)?;
    // A corpus that indexes nothing is a broken harness, not a clean one, and
    // its scoreboard is all zeroes and entirely plausible
    // (`specs/00-overview.md` invariant 5 — never a silent empty).
    if report.indexed == 0 || report.counts.defs == 0 {
        bail!(
            "the index found {} file(s) and {} def(s) in {}. A pinned corpus is never empty, so \
             the harness is wrong, not the tree",
            report.indexed,
            report.counts.defs,
            root.display()
        );
    }
    Ok((report.indexed, report.counts.defs))
}

/// The census of the index just written, with the tree walked so `changed` is
/// filled — a `stale` corpus has to be visible to the gate below.
fn read_census(root: &Path, index_dir: &Path) -> Result<Census> {
    let store = Store::open(root, index_dir.to_path_buf(), &extract::fingerprint())?;
    let census = Census::of(&store)?;
    Ok(census.with_tree(root, store.manifest()))
}

/// The anchor rate, or `None` when this corpus may not be scored.
///
/// Three ways to fail, each printed and none silent: a status that is not
/// `ok`, a SCIP index that was ingested and resolved nothing, and an anchor
/// rate the census cannot compute. Every one of them makes the name side of a
/// pair look better than it is, which is the one way this measurement can lie
/// in the tool's favour.
fn gate(root: &Path, index_dir: &Path, census: &Census) -> Result<Option<f64>> {
    let store = Store::open(root, index_dir.to_path_buf(), &extract::fingerprint())?;
    let report = Report {
        census,
        manifest: store.manifest(),
        index_dir,
        pinned: Some(index_dir),
    };
    let status = report.status();
    if status != Status::Ok {
        println!(
            "REFUSED: the corpus SCIP status is `{}`, not `ok`, so no scoreboard was written. {}\n\
             A stale index inflates every name-only row and makes precision lie.",
            status.as_str(),
            report.hint().unwrap_or_default()
        );
        return Ok(None);
    }
    let (refs, resolved) = (census.rows("scip_ref"), census.rows("resolved"));
    if refs == 0 || resolved == 0 {
        println!(
            "REFUSED: the index holds {refs} scip_ref and {resolved} resolved row(s). The oracle \
             is empty, so every `_exact` twin answers nothing and every precision would read \
             0.0 — a result shaped exactly like a tool that is always wrong."
        );
        return Ok(None);
    }
    let Some(rate) = census.anchor_rate() else {
        println!(
            "REFUSED: the census cannot compute an anchor rate for this index, so `scip` would \
             carry a number that was never measured."
        );
        return Ok(None);
    };
    // The census reports a percentage; `docs/50-self-improvement.md` § 3
    // writes the fraction.
    Ok(Some(round(rate / 100.0)))
}

/// What one MCP session spent answering the corpus's query set.
struct Cost {
    /// The `{"schema":true}` result.
    schema_bytes: usize,
    /// Per-query results.
    p50: usize,
    p95: usize,
    total: usize,
    /// Rows those results carried.
    rows: usize,
}

/// Drive one `codeintel mcp` session and weigh what it sent back.
///
/// Called the way an agent calls it: `initialize` as Claude Code does, then the
/// schema, then every query at the default limit. The initialize is part of the
/// measurement because it decides which body the server sends
/// (`specs/05-surface.md` § The consumer): a session that declared nothing
/// would be weighed on the text body and read back through the JSON one. **Bytes of the tool result**, not of the JSON-RPC envelope
/// around it — the envelope's `id` is the client's own bytes echoed back
/// (`specs/05-surface.md` § Symbol rendering).
fn cost(root: &Path, index_dir: &Path, set: &[(String, String)]) -> Result<Cost> {
    let mut input = String::new();
    writeln!(
        input,
        "{}",
        json!({"jsonrpc": "2.0", "id": -1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "score", "version": "0"}}})
    )?;
    writeln!(
        input,
        "{}",
        json!({"jsonrpc": "2.0", "id": 0, "method": "tools/call",
               "params": {"name": codeintel::mcp::TOOL, "arguments": {"schema": true}}})
    )?;
    for (i, (_, goal)) in set.iter().enumerate() {
        writeln!(
            input,
            "{}",
            json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call",
                   "params": {"name": codeintel::mcp::TOOL, "arguments": {"query": goal}}})
        )?;
    }

    let mut out: Vec<u8> = Vec::new();
    codeintel::mcp::serve(root, Some(index_dir), input.as_bytes(), &mut out)?;
    let text = String::from_utf8(out).context("the server's output is UTF-8")?;
    let mut sizes = Vec::new();
    let mut rows = 0;
    let mut schema_bytes = 0;
    for (i, line) in text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .skip(1)
        .enumerate()
    {
        let weighed = weigh(line)?;
        let (bytes, got) = (weighed.bytes, weighed.rows);
        if weighed.failed {
            bail!("mcp call {i} answered with an error; that measures nothing: {line}");
        }
        // Every call refreshes, and a refresh that fired would have
        // re-extracted tier A only and dropped the oracle out from under the
        // precision measured next (`specs/05-surface.md` § `query`). It cannot
        // happen on a pinned checkout, and if it ever does the scoreboard must
        // not be the thing that finds out quietly.
        if weighed.refreshed != 0 || !matches!(weighed.status.as_str(), "ok" | "truncated") {
            bail!(
                "mcp call {i} answered `{}` after refreshing {} file(s). The corpus moved while \
                 it was being scored",
                weighed.status,
                weighed.refreshed
            );
        }
        if i == 0 {
            schema_bytes = bytes;
        } else {
            sizes.push(bytes);
            rows += got;
        }
    }
    if sizes.len() != set.len() {
        bail!("mcp: {} answers to {} queries", sizes.len(), set.len());
    }
    Ok(Cost {
        schema_bytes,
        p50: percentile(&sizes, 0.50),
        p95: percentile(&sizes, 0.95),
        total: sizes.iter().sum(),
        rows,
    })
}

/// What one MCP response weighed, and what it said about itself.
struct Weighed {
    /// Bytes of the tool result.
    bytes: usize,
    /// Rows it carried.
    rows: usize,
    /// `isError`.
    failed: bool,
    /// The answer's status, so a degraded index cannot be weighed in silence.
    status: String,
    /// Files auto-refresh re-extracted before answering.
    refreshed: u64,
}

fn weigh(line: &str) -> Result<Weighed> {
    let value: Value = serde_json::from_str(line).context("a response is JSON")?;
    let result = value.get("result").context("a response carries a result")?;
    Ok(Weighed {
        bytes: result.to_string().len(),
        rows: result
            .pointer("/structuredContent/rows")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        failed: result.get("isError").and_then(Value::as_bool) == Some(true),
        status: result
            .pointer("/structuredContent/status")
            .and_then(Value::as_str)
            .unwrap_or("(none)")
            .to_string(),
        refreshed: result
            .pointer("/structuredContent/stats/refreshed")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

/// Per-rule confirmation against the `_exact` twin.
///
/// Not precision: the twin is a filter on `Prov`, not an independent oracle,
/// so an unconfirmed row is one SCIP did not resolve and **not** one judged
/// wrong (`docs/50-self-improvement.md` § 2, Confirmation).
///
/// Never averaged across rules: `docs/50-self-improvement.md` § 8, which is
/// invariant 2 one level up.
fn confirmation(root: &Path, index_dir: &Path) -> Result<Value> {
    let mut oracle = Oracle::open(root, index_dir)?;
    let mut out = serde_json::Map::new();
    for pair in PAIRS {
        let mut predicted = oracle.keys(pair.name_goal, pair.key, pair.prov)?;
        if let Some(goal) = pair.tier_goal {
            // Every tier goal binds `(A, B, Prov)`, so `Prov` is column 2 for
            // unary and binary rules alike.
            let tiered = oracle.keys(goal, pair.key, Some(2))?;
            if !tiered.keys().eq(predicted.keys()) {
                bail!(
                    "`{goal}` is not exactly the rows of `{}`: {} against {}. `tier` must tag, \
                     never filter or invent",
                    pair.name_goal,
                    tiered.len(),
                    predicted.len()
                );
            }
            predicted = tiered;
        }
        let exact = oracle.keys(pair.exact_goal, pair.key, None)?;
        let truth: BTreeSet<&Vec<Atom>> = exact.keys().collect();
        let (mut tp, mut fp) = (0_usize, 0_usize);
        for key in predicted.keys() {
            if truth.contains(key) {
                tp += 1;
            } else {
                fp += 1;
            }
        }
        let missed = truth.len() - tp;
        println!(
            "{:<13} {:<8} confirmed {tp:>7}  unconfirmed {fp:>7}  exact_only {missed:>7}  rate {:>6}",
            pair.rule,
            pair.shape,
            show(&ratio(tp, tp + fp)),
        );
        out.insert(
            pair.rule.to_string(),
            json!({
                "shape": pair.shape,
                "confirmed": tp,
                "unconfirmed": fp,
                "exact_only": missed,
                "confirmation_rate": ratio(tp, tp + fp),
            }),
        );
    }
    Ok(Value::Object(out))
}

/// What the extractor saw, straight off the census.
///
/// No Datalog: these are row counts of base relations, and the census is where
/// `status` reads the same ones from.
fn coverage(census: &Census) -> Value {
    let roles: usize = census
        .roles
        .iter()
        .filter(|(role, _)| role.as_str() != "unknown")
        .map(|(_, n)| *n)
        .sum();
    json!({
        "span_coverage": ratio(census.rows("def_span"), census.rows("def")),
        "role_known": ratio(roles, census.rows("scip_ref")),
        "scip_impl_rows": census.rows("scip_impl"),
    })
}

/// An engine over the corpus's index, holding the standard library.
///
/// Not `query::run`: that host caps an answer at `max_result_bytes` for a
/// context window's sake, and a precision computed over a truncated prefix is
/// a precision of something else. The oracle needs every row, so it asks the
/// engine directly and compares interned atoms — no rendering, no caps, and no
/// chance of two notations of one tuple disagreeing.
struct Oracle {
    engine: Engine,
    /// Interned constants belong to the engine; the handle keeps them alive.
    _literals: codeintel::overlay::Literals,
}

impl Oracle {
    fn open(root: &Path, index_dir: &Path) -> Result<Self> {
        let store = Store::open(root, index_dir.to_path_buf(), &extract::fingerprint())?;
        let relations = store.load()?;
        let (interner, _) = store.into_parts();
        let (overlay, literals) = Overlay::new(interner);
        let mut engine = Engine::new(Box::new(overlay)).with_regexes(Box::new(Regexes::new()));
        // Every base relation, present or not: an undeclared one is a query
        // error, and "no rows" is a different answer.
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
            .load_rules(STDLIB)
            .map_err(|d| anyhow::anyhow!("rules/stdlib.dl does not load: {}", d.message))?;
        literals.seal();
        Ok(Self {
            engine,
            _literals: literals,
        })
    }

    /// `goal`'s rows projected onto their first `key` columns, each mapped to
    /// whether the row said it was a name-tier guess.
    ///
    /// A key seen twice is flagged if any of its rows was — which for `ref`
    /// means a position carrying both provenances is not counted as unflagged
    /// on the strength of its name row.
    fn keys(
        &mut self,
        goal: &str,
        key: usize,
        prov: Option<usize>,
    ) -> Result<BTreeMap<Vec<Atom>, bool>> {
        let mut limits = Limits::default();
        // Every row, or the measurement is of a prefix.
        limits.max_result_rows = usize::MAX;
        limits.max_result_bytes = usize::MAX;
        limits.max_derived_tuples = u64::MAX;
        limits.max_time_ms = u64::MAX;
        let result = self
            .engine
            .query(goal, &limits)
            .map_err(|d| anyhow::anyhow!("`{goal}`: {}", d.message))?;
        if result.truncated {
            bail!("`{goal}` was truncated; the scoreboard would describe a prefix of the answer");
        }
        let name = self.engine.intern("name");
        let mut out: BTreeMap<Vec<Atom>, bool> = BTreeMap::new();
        for row in &result.rows {
            let Some(head) = row.get(..key) else {
                bail!("`{goal}` answered {} columns, not {key}", row.len());
            };
            let flagged = prov.is_some_and(|c| row.get(c).copied() == name);
            *out.entry(head.to_vec()).or_insert(false) |= flagged;
        }
        Ok(out)
    }
}

/// `a / b` rounded, or `null` when `b` is zero.
///
/// Not `0.0`. A rule with no rows on either side has an undefined precision,
/// and writing zero there is a tool that is never right wearing the same
/// number as a tool that was never asked (`specs/00-overview.md` invariant 5).
fn ratio(a: usize, b: usize) -> Value {
    #[expect(
        clippy::cast_precision_loss,
        reason = "row counts far below 2^53; the ratio is reported to four places"
    )]
    if b == 0 {
        Value::Null
    } else {
        json!(round(a as f64 / b as f64))
    }
}

/// Four places. Finer digits are noise and rewrite every line of a baseline.
fn round(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// A ratio for the terminal: four places, or `n/a`.
fn show(v: &Value) -> String {
    v.as_f64()
        .map_or_else(|| "n/a".to_string(), |f| format!("{f:.4}"))
}

/// Report every difference and say whether anything regressed.
///
/// Nothing here is a clock, so there is no "inconclusive": the numbers compare
/// on any machine or the baseline is from another corpus.
fn compare(base: &Value, current: &Value) -> bool {
    let (base_rev, rev) = (base.pointer("/corpus/rev"), current.pointer("/corpus/rev"));
    if base_rev != rev {
        println!(
            "REFUSED: the baseline measured corpus {base_rev:?}, this is {rev:?}. A baseline \
             against a different tree measures nothing"
        );
        return false;
    }
    // The oracle has an identity of its own, and it is not implied by the
    // corpus rev (see `scip.index` where it is written). Every precision,
    // every recall, the anchor rate and the coverage ratios are all measured
    // against it, so a different index is a different experiment — reported
    // as a refusal, never absorbed as a regression.
    let (base_scip, scip) = (base.pointer("/scip/index"), current.pointer("/scip/index"));
    if base_scip != scip {
        println!(
            "REFUSED: the baseline's oracle was SCIP index {base_scip:?}, this is {scip:?}. The \
             same checkout indexed twice is not the same oracle, so nothing here is comparable. \
             Re-use that index, or UPDATE_BASELINE=1 against this one"
        );
        return false;
    }
    let mut regressed = Vec::new();
    for (pointer, direction) in JUDGED {
        judge(base, current, pointer, *direction, &mut regressed);
    }
    for pair in PAIRS {
        for metric in ["precision", "recall"] {
            let pointer = format!("/precision/{}/{metric}", pair.rule);
            judge(base, current, &pointer, Direction::Higher, &mut regressed);
        }
    }
    // Reported, never judged: they move with the rules and the diff is the
    // thing to read (`docs/50-self-improvement.md` § 4).
    for pair in PAIRS {
        for metric in ["tp", "fp", "fn"] {
            let pointer = format!("/precision/{}/{metric}", pair.rule);
            if base.pointer(&pointer) != current.pointer(&pointer) {
                println!(
                    "moved {pointer}: {:?} -> {:?}",
                    base.pointer(&pointer),
                    current.pointer(&pointer)
                );
            }
        }
    }
    for f in &regressed {
        println!("REGRESSED {f}");
    }
    regressed.is_empty()
}

/// One metric against its baseline.
fn judge(
    base: &Value,
    current: &Value,
    pointer: &str,
    direction: Direction,
    out: &mut Vec<String>,
) {
    let (old, new) = (base.pointer(pointer), current.pointer(pointer));
    if old == new {
        return;
    }
    let (Some(a), Some(b)) = (old.and_then(Value::as_f64), new.and_then(Value::as_f64)) else {
        // One side is absent or `null`: a metric that went from measured to
        // undefined, or arrived in a baseline that predates it. Never silent.
        out.push(format!(
            "{pointer}: {old:?} -> {new:?}, which is not a comparison"
        ));
        return;
    };
    let better = match direction {
        Direction::Lower => b < a,
        Direction::Higher => b > a,
    };
    if better {
        println!("improved {pointer}: {a} -> {b}");
    } else {
        out.push(format!("{pointer}: {a} -> {b}"));
    }
}

/// The corpus's `queries.tsv`: `id<TAB>query`, `#` lines skipped. Shared with
/// `benches/corpus.rs`, so cost is weighed over the set the clock already runs.
fn query_set(spec: &Path) -> Result<Vec<(String, String)>> {
    let path = spec.join("queries.tsv");
    let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let set = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            l.split_once('\t')
                .map(|(id, goal)| (id.to_string(), goal.to_string()))
                .with_context(|| format!("{}: `{l}`", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    if set.is_empty() {
        bail!("{}: no queries", path.display());
    }
    Ok(set)
}

/// Nearest rank. `values` need not be sorted.
fn percentile(values: &[usize], p: f64) -> usize {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "p is in 0..=1 and the product is at most the sample count"
    )]
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted.get(rank.saturating_sub(1)).copied().unwrap_or(0)
}

/// The build that measured: the short rev, plus `-dirty` when it cannot be
/// checked out again.
///
/// The baselines this bench writes do not count as dirt. `tasks/score.yml`
/// scores fd and then tokio, so with `UPDATE_BASELINE=1` the first step's
/// output would make the second step's build read as unbuildable — a `-dirty`
/// that describes the measurement, not the code.
fn tool_rev() -> String {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rev =
        git(here, &["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "norev".to_string());
    let dirty = git(here, &["status", "--porcelain"]).is_some_and(|s| {
        s.lines()
            .any(|l| !l.trim_end().ends_with("score-baseline.json"))
    });
    if dirty { format!("{rev}-dirty") } else { rev }
}

fn git_head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string())
}

/// A git command's trimmed stdout, or `None` when it fails.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(value)?;
    std::fs::write(path, text + "\n").with_context(|| path.display().to_string())
}
