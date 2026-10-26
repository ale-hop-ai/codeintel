//! Where a warm MCP call's time goes: `docs/plan.md` M6 item 3, issue #28.
//!
//! ```sh
//! cargo bench -p codeintel --bench warm_phases -- <name> <corpus-dir>
//! ```
//!
//! The question is not "how long does a warm call take" — `benches/corpus.rs`
//! answers that — but "which phase of it scales with the tree". So this walks
//! the phases of `query::Warm::answer` one at a time, on a corpus that has not
//! changed since it was indexed, which is the case a warm no-change call is in:
//!
//! | phase | what it is |
//! |---|---|
//! | `locate` | `facts::store::locate` |
//! | `open` | `facts::Store::open` — manifest parse + dictionary map |
//! | `manifest_reread` | refresh's second `Manifest::open` |
//! | `open_dict` | the dictionary half of `open`: `Interner::open_to` |
//! | `dict_read` | inside `open_dict`: `fs::read` of `dict.bin` and `dict.idx` alone |
//! | `dict_open` | inside `open_dict`: `Dict::open_to`, i.e. read + offsets + `validate` |
//! | `manifest_compare` | the `!=` that decides whether to reopen |
//! | `sweep` | `Store::sweep` — `read_dir` over the segment directory |
//! | `walk` | `extract::walk` — the gitignore-aware traversal |
//! | `refresh` | `index::refresh` alone, sweep and walk included, open excluded |
//! | `stale_check` | `refresh - sweep - walk`, the per-file clean test |
//! | `warm_total` | `query::Warm::run` of the cheapest query in the set |
//! | `mcp_p50` | anchor: the same call through `mcp::serve`, flush to flush |
//!
//! `warm_total - refresh - open - locate` is what evaluation and rendering
//! cost. The query used is the cheapest one in `corpora/<name>/queries.tsv` by
//! derived count, so that the residual is as close to zero as the set allows
//! and the per-call floor is what is left.
//!
//! Local only, like `corpus.rs`. It writes
//! `target/bench/<name>-phases-<run-id>.json`
//! and compares nothing: this is a profile, not a gate.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use codeintel::index::{self, Plan};
use codeintel::query::{Options, Warm};
use facts::Store;
use serde_json::{Value, json};

#[path = "conditions.rs"]
mod conditions;

use conditions::Conditions;

/// Every phase, in the order a round runs them.
const PHASES: [&str; 11] = [
    "locate",
    "open",
    "manifest_reread",
    "open_dict",
    "dict_read",
    "dict_open",
    "manifest_compare",
    "sweep",
    "walk",
    "refresh",
    "warm_total",
];

/// Rounds kept. Each round is a full warm call's worth of work per phase.
const RUNS: usize = 20;
/// Discarded before measuring, so the page cache is the same across phases.
const WARMUPS: usize = 3;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("warm_phases: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let [name, corpus] = args.as_slice() else {
        bail!("usage: cargo bench -p codeintel --bench warm_phases -- <name> <corpus-dir>");
    };
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let goal = cheapest_query(&manifest.join("benches/corpora").join(name))?;
    let corpus = PathBuf::from(corpus)
        .canonicalize()
        .context("the corpus directory")?;

    println!(
        "host conditions before the copy: {}",
        Conditions::now().describe()
    );

    let tmp = tempfile::tempdir().context("tempdir")?;
    let root = tmp.path().canonicalize().context("tempdir")?;
    copy(&corpus, &root)?;
    let dir = root.join(".codeintel");
    println!("corpus {} -> {}", corpus.display(), root.display());

    // After the copy, which is what disturbs the host, and before our own
    // measured work raises it (#84). A profile is a wall-clock number too.
    let settled = conditions::settle();
    let started_under = settled.conditions;
    println!(
        "host conditions after {:.0}s settling: {}{}",
        settled.waited_s,
        started_under.describe(),
        if settled.timed_out {
            " (gave up waiting)"
        } else {
            ""
        }
    );
    if !started_under.trustworthy() {
        println!(
            "WARNING: these numbers describe a busy machine as much as they describe the code"
        );
    }

    // Index once. Everything below is a no-change call against this.
    let started = Instant::now();
    let mut store = Store::open(&root, &dir, &extract::fingerprint())?;
    let report = index::refresh(&mut store, &Plan::default())?;
    println!(
        "indexed {} file(s), {} defs, in {:.0} ms",
        report.indexed,
        report.counts.defs,
        ms(started)
    );
    drop(store);

    let options = Options {
        index_dir: Some(dir.clone()),
        ..Options::default()
    };
    let fingerprint = extract::fingerprint();

    // State the cheap phases need, built once. Timing a `Store::open` inside
    // the `manifest_compare` sample would measure the open, not the compare.
    let compare_store = Store::open(&root, &dir, &fingerprint)?;
    let compare_manifest = facts::Manifest::open(&dir)?.context("the index exists")?;
    let sweep_store = Store::open(&root, &dir, &fingerprint)?;
    let (bin_len, idx_len) = (
        compare_store.manifest().dict_bin_len,
        compare_store.manifest().dict_idx_len,
    );
    let mut warm = Warm::default();

    // One round runs every phase once, in order. Measuring each phase in its
    // own block instead put them in different host conditions — on deno the
    // walk was sampled while the OS was still scanning the fresh copy, and came
    // out at 797 ms against a 458 ms `refresh` that contains it, for a
    // `stale_check` of -343 ms. Interleaving spreads whatever the host is doing
    // across every phase equally, which is what makes the shares comparable.
    let mut phases: Vec<(&str, Vec<f64>)> = PHASES.iter().map(|p| (*p, Vec::new())).collect();
    for round in 0..WARMUPS + RUNS {
        let mut round_ms: Vec<(&str, f64)> = Vec::with_capacity(PHASES.len());

        let t = Instant::now();
        facts::store::locate(&root, Some(dir.as_path()))?;
        round_ms.push(("locate", ms(t)));

        let t = Instant::now();
        drop(Store::open(&root, &dir, &fingerprint)?);
        round_ms.push(("open", ms(t)));

        let t = Instant::now();
        drop(facts::Manifest::open(&dir)?);
        round_ms.push(("manifest_reread", ms(t)));

        let t = Instant::now();
        drop(facts::intern::Interner::open_to(&dir, bin_len, idx_len)?);
        round_ms.push(("open_dict", ms(t)));

        // `open_dict` less `dict_open` is the `by_text` map; `dict_open` less
        // `dict_read` is the offset table and `validate`.
        let t = Instant::now();
        std::hint::black_box((
            std::fs::read(dir.join("dict.bin"))?,
            std::fs::read(dir.join("dict.idx"))?,
        ));
        round_ms.push(("dict_read", ms(t)));

        let t = Instant::now();
        drop(facts::intern::Dict::open_to(&dir, bin_len, idx_len)?);
        round_ms.push(("dict_open", ms(t)));

        let t = Instant::now();
        std::hint::black_box(compare_manifest == *compare_store.manifest());
        round_ms.push(("manifest_compare", ms(t)));

        let t = Instant::now();
        sweep_store.sweep()?;
        round_ms.push(("sweep", ms(t)));

        let t = Instant::now();
        std::hint::black_box(extract::walk(&root));
        round_ms.push(("walk", ms(t)));

        // The open is outside the timer. `refresh` is the walk and the clean
        // test, and `warm_total` pays the open separately — timing it in both
        // made `evaluate` come out at -17.8 ms on deno, which is how this was
        // found.
        let mut store = Store::open(&root, &dir, &fingerprint)?;
        let t = Instant::now();
        let report = index::refresh(&mut store, &Plan::default())?;
        round_ms.push(("refresh", ms(t)));
        if report.indexed != 0 {
            bail!(
                "a no-change refresh re-extracted {} file(s)",
                report.indexed
            );
        }
        drop(store);

        let t = Instant::now();
        let answer = warm.run(&root, &goal, &options)?;
        round_ms.push(("warm_total", ms(t)));
        if !answer.status.answered() {
            bail!("warm call answered {}", answer.status.as_str());
        }

        if round >= WARMUPS {
            for (label, value) in round_ms {
                let slot = phases
                    .iter_mut()
                    .find(|(l, _)| *l == label)
                    .context("every sampled phase is in PHASES")?;
                slot.1.push(value);
            }
        }
    }

    // An anchor, not a phase. `warm_total` is `Warm::run` in this process; the
    // baseline's `mcp_p50_ms` is flush-to-flush through `mcp::serve`. Measuring
    // both here says whether this profile's absolute numbers are comparable to
    // `baseline.json`, or whether the host differs from the one that wrote it.
    let mcp_p50 = measure_mcp(&root, &dir, &goal)?;
    println!("\nanchor: mcp p50 {mcp_p50:.2} ms, flush to flush, same query");

    report_phases(name, &goal, &phases, mcp_p50, settled)
}

/// p50 and p95 of each phase, printed and written as JSON.
fn report_phases(
    name: &str,
    goal: &str,
    phases: &[(&str, Vec<f64>)],
    mcp_p50: f64,
    settled: conditions::Settled,
) -> Result<()> {
    let p50 = |label: &str| {
        phases
            .iter()
            .find(|(l, _)| *l == label)
            .map_or(f64::NAN, |(_, r)| percentile(r, 0.50))
    };
    // Derived, not measured: `index::refresh` contains the sweep and the walk,
    // and what is left is the per-file clean test and the commit decision.
    let stale_check = p50("refresh") - p50("sweep") - p50("walk");
    // What `Warm::run` spends outside the refresh and the open: evaluation and
    // rendering of the cheapest query in the set.
    let evaluate = p50("warm_total") - p50("refresh") - p50("open") - p50("locate");

    println!("\nquery: {goal}");
    println!("{:<18} {:>10} {:>10}", "phase", "p50 ms", "p95 ms");
    for (label, runs) in phases {
        println!(
            "{label:<18} {:>10.2} {:>10.2}",
            percentile(runs, 0.50),
            percentile(runs, 0.95)
        );
    }
    println!("{:<18} {stale_check:>10.2}      derived", "stale_check");
    println!("{:<18} {evaluate:>10.2}      derived", "evaluate");

    // A derived phase is a difference between two measured ones, so a negative
    // value means the host moved underneath the round rather than that the
    // phase was free. Refuse the profile instead of writing the number down:
    // an impossible breakdown that got committed is worse than no breakdown
    // (`CLAUDE.md`: a hand-run table on an unpinned tree is not evidence).
    for (label, value) in [("stale_check", stale_check), ("evaluate", evaluate)] {
        if value < 0.0 {
            bail!(
                "{label} came out at {value:.2} ms, which is impossible: it is a difference \
                 between measured phases. Either a phase is double-counted in the breakdown or \
                 the host moved underneath the round. Check p50 against p95 to tell them apart \
                 — a tight pair means the host was fine and the model is wrong. Nothing written."
            );
        }
    }

    let mut measured = serde_json::Map::new();
    for (label, runs) in phases {
        measured.insert(
            (*label).to_string(),
            json!({ "p50_ms": percentile(runs, 0.50), "p95_ms": percentile(runs, 0.95) }),
        );
    }
    let out = json!({
        "corpus": name,
        "query": goal,
        "runs": RUNS,
        // Which build produced this, and what else the machine was doing.
        // Without it a profile is just numbers in a file: `target/bench/` holds
        // one path per corpus and every run overwrites it, so a table quoted
        // from the wrong run reads exactly like a table quoted from the right
        // one. That happened to deno in `docs/history.md` § #28 — a parallel-walk
        // JSON was read as the serial walk's.
        "provenance": conditions::report(settled, Conditions::now()),
        "phases": measured,
        "derived": { "stale_check_ms": stale_check, "evaluate_ms": evaluate },
        "anchor": { "mcp_p50_ms": mcp_p50 },
    });
    // Stamped with what produced it (#78): the deno row of `docs/history.md`
    // § #28 was written by a parallel-walk binary and read as the serial
    // walk's, because both wrote the same path.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/bench")
        .join(format!("{name}-phases-{}.json", conditions::run_id()));
    std::fs::create_dir_all(path.parent().context("a parent")?)?;
    std::fs::write(&path, serde_json::to_string_pretty(&out)? + "\n")?;
    println!("\nwrote {}", path.display());
    Ok(())
}

/// The query in the set with the fewest derived rows, so that `warm_total`
/// minus the refresh is as close to the per-call floor as the set allows.
///
/// Read from `baseline.json`, which records `derived` per query, falling back
/// to the first line of `queries.tsv` when there is no baseline yet.
fn cheapest_query(spec: &Path) -> Result<String> {
    let tsv = std::fs::read_to_string(spec.join("queries.tsv")).context("queries.tsv")?;
    let first = tsv
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split_once('\t').map_or(l, |(_, goal)| goal).to_string())
        .next()
        .context("queries.tsv has a query")?;
    let Ok(text) = std::fs::read_to_string(spec.join("baseline.json")) else {
        return Ok(first);
    };
    let baseline: Value = serde_json::from_str(&text).context("baseline.json")?;
    let cheapest = baseline
        .get("queries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|q| {
            Some((
                q.get("derived")?.as_u64()?,
                q.get("query")?.as_str()?.to_string(),
            ))
        })
        .min_by_key(|(derived, _)| *derived)
        .map(|(_, goal)| goal);
    Ok(cheapest.unwrap_or(first))
}

/// p50 of one warm `mcp::serve` call answering `goal`, flush to flush.
///
/// The same metric `benches/corpus.rs` reports as `mcp_p50_ms`, so the number
/// can be read against `corpora/<name>/baseline.json` directly.
fn measure_mcp(root: &Path, dir: &Path, goal: &str) -> Result<f64> {
    let calls = WARMUPS + RUNS;
    let mut input = String::new();
    for id in 1..=calls {
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": codeintel::mcp::TOOL, "arguments": { "query": goal } },
        });
        writeln!(input, "{request}")?;
    }
    let mut stamps = Stamps::default();
    let started = Instant::now();
    codeintel::mcp::serve(root, Some(dir), input.as_bytes(), &mut stamps)?;
    if stamps.at.len() != calls {
        bail!("mcp: {} responses to {calls} calls", stamps.at.len());
    }
    if String::from_utf8_lossy(&stamps.bytes).contains("\"isError\":true") {
        bail!("mcp: a call answered with an error; that measures nothing");
    }
    let mut gaps = Vec::with_capacity(calls);
    let mut last = started;
    for &at in &stamps.at {
        gaps.push((at.duration_since(last).as_secs_f64() * 1_000_000.0).round() / 1000.0);
        last = at;
    }
    Ok(percentile(gaps.get(WARMUPS..).unwrap_or_default(), 0.50))
}

/// A writer that keeps what the server wrote and when it flushed.
#[derive(Default)]
struct Stamps {
    bytes: Vec<u8>,
    at: Vec<Instant>,
}

impl std::io::Write for Stamps {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.at.push(Instant::now());
        Ok(())
    }
}

/// A corpus copy without `.git`, `target` or a previous index.
fn copy(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" || name == "target" || name == ".codeintel" {
            continue;
        }
        let target = to.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "p is in 0..=1 and the product is at most the sample count"
    )]
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted
        .get(rank.saturating_sub(1))
        .copied()
        .unwrap_or(f64::NAN)
}

/// Milliseconds since `started`, rounded to the microsecond.
fn ms(started: Instant) -> f64 {
    (started.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0
}
