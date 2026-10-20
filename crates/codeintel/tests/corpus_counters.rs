//! Counter goldens over the pinned corpora, with no clock in them (#94).
//!
//! `benches/corpus.rs` pins `status`, `rows`, `derived` and `demand` per query
//! in each corpus's `baseline.json`, and compares them *exactly* — but only
//! when a human runs the wall-clock bench, on a quiet machine, against a
//! host-matched baseline. Nothing in the gate reads them, so fd's query 8 moved
//! `derived` 4989 -> 4981 and sat there for weeks (#88), while the commit that
//! moved it asserted no pinned set had.
//!
//! Counters need none of that. They are exact on any host under any load, so
//! this runs them in the gate: index each corpus into a temp directory, run its
//! query set, compare. Wall clock stays in `benches/corpus.rs`, host-gated,
//! local-only.
//!
//! **Contents, not just counts.** #88's trap was that `rows` is a *count*: 231
//! before and 231 after proved nothing, and only reading the rows settled it.
//! Each query therefore also pins a blake3 digest over its `--raw` rows — the
//! tuple identity, so a pure rendering change does not read as a moved answer.
//! Three signals, and they say different things:
//!
//! - `rows` moved -> the answer set changed size. Correctness.
//! - `rows` same, `digest` moved -> different rows, same number of them.
//!   Correctness, and this is the one #88 could not see.
//! - rows and digest same, `derived` moved -> same answer, different work.
//!   Cost.
//!
//! A corpus that is not checked out is **reported, never silently skipped**
//! (`specs/00-overview.md` invariant 5). `tasks/ci.yml` runs this one test with
//! `--nocapture` so that report reaches the gate's output on a pass too.
//!
//! ```sh
//! provision apply tasks/bench.yml   # checks the corpora out
//! UPDATE_COUNTERS=1 cargo test -p codeintel --test corpus_counters -- --nocapture
//! ```
//!
//! A failure here is a finding, not a golden to refresh: read which of the
//! three signals moved, say which in the commit message, and only then record.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// The corpora the gate checks by default.
///
/// Measured on this tree, one cold index plus the query set, 10-core M5:
/// **fd 1.3s, tokio 21.4s, deno 218s.** The gate itself is ~60s, so fd and
/// tokio cost it a third and deno would cost it four times over — a check
/// nobody can afford to run is the defect this test exists to fix, not a
/// stricter version of it.
///
/// The price is stated rather than hidden: **deno is the only TypeScript
/// corpus**, so a TypeScript-only extraction change moves no counter the gate
/// reads. `CODEINTEL_CORPUS_COUNTERS=all` checks everything, and
/// `tasks/bench.yml` covers deno whenever the wall clock is run.
const GATE: &[&str] = &["fd", "tokio"];

/// Where `tasks/bench.yml` checks the corpora out.
fn corpus_root(name: &str) -> PathBuf {
    std::env::var_os("CODEINTEL_CORPUS_DIR")
        .map_or_else(
            || {
                PathBuf::from(std::env::var_os("HOME").expect("HOME"))
                    .join(".cache/codeintel/corpus")
            },
            PathBuf::from,
        )
        .join(name)
}

fn corpora_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/corpora")
}

/// Why a corpus was not compared. Every variant is printed; none is silent.
enum Skipped {
    NotCheckedOut,
    NotAGitCheckout,
    Moved { want: String, got: String },
    NotRecorded,
    OutsideBudget,
}

impl Skipped {
    fn describe(&self, name: &str) -> String {
        match self {
            Self::NotCheckedOut => format!(
                "{name}: SKIPPED — not checked out. `provision apply tasks/bench.yml` fetches it"
            ),
            Self::NotAGitCheckout => {
                format!("{name}: SKIPPED — present but not a git checkout, so its rev is unknown")
            }
            Self::Moved { want, got } => format!(
                "{name}: SKIPPED — checked out at {got}, counters were recorded at {want}. \
                 Nothing is compared across corpus revs"
            ),
            Self::NotRecorded => format!(
                "{name}: SKIPPED — no counters.json. \
                 `UPDATE_COUNTERS=1 cargo test -p codeintel --test corpus_counters`"
            ),
            Self::OutsideBudget => format!(
                "{name}: SKIPPED — outside the gate's cost budget. \
                 `CODEINTEL_CORPUS_COUNTERS=all` checks it, and so does tasks/bench.yml"
            ),
        }
    }
}

/// `#[ignore]` because `tasks/ci.yml` runs it as its own step, with
/// `--nocapture`.
///
/// Not to keep it out of the gate — the gate is the whole point — but because
/// a plain `cargo test` captures a passing test's output, and a corpus this
/// skipped would then be invisible exactly when nothing else reports it
/// (invariant 5). Running it in both places instead would index every corpus
/// twice. The two files name each other; moving one without the other is the
/// way this rots.
#[test]
#[ignore = "tasks/ci.yml runs this with --nocapture, so the skip report is visible"]
fn every_pinned_corpus_costs_what_its_counters_say() {
    let update = std::env::var_os("UPDATE_COUNTERS").is_some();
    let mut names: Vec<String> = std::fs::read_dir(corpora_dir())
        .expect("benches/corpora exists")
        .filter_map(|e| {
            let e = e.ok()?;
            e.file_type().ok()?.is_dir().then(|| e.file_name())
        })
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    names.sort();

    let mut failures = String::new();
    let (mut checked, mut skipped) = (Vec::new(), Vec::new());
    for name in &names {
        match check(name, update, &mut failures) {
            Ok(()) => checked.push(name.clone()),
            Err(why) => {
                println!("{}", why.describe(name));
                skipped.push(name.clone());
            }
        }
    }

    println!(
        "corpus counters: {} checked ({}), {} skipped ({})",
        checked.len(),
        if checked.is_empty() {
            "none".to_string()
        } else {
            checked.join(" ")
        },
        skipped.len(),
        if skipped.is_empty() {
            "none".to_string()
        } else {
            skipped.join(" ")
        }
    );
    assert!(failures.is_empty(), "{failures}");
}

/// Compare one corpus, or say why it could not be.
fn check(name: &str, update: bool, failures: &mut String) -> Result<(), Skipped> {
    let all = std::env::var_os("CODEINTEL_CORPUS_COUNTERS").is_some_and(|v| v == "all");
    if !all && !update && !GATE.contains(&name) {
        return Err(Skipped::OutsideBudget);
    }
    let root = corpus_root(name);
    if !root.is_dir() {
        return Err(Skipped::NotCheckedOut);
    }
    let got = head_rev(&root).ok_or(Skipped::NotAGitCheckout)?;
    let path = corpora_dir().join(name).join("counters.json");
    let recorded = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    if recorded.is_none() && !update {
        return Err(Skipped::NotRecorded);
    }
    if let Some(want) = recorded
        .as_ref()
        .and_then(|r| r.pointer("/corpus/rev"))
        .and_then(Value::as_str)
        && want != got
        && !update
    {
        return Err(Skipped::Moved {
            want: want.to_string(),
            got,
        });
    }

    let index = tempfile::tempdir().expect("tempdir");
    index_corpus(&root, index.path());
    let measured: Vec<Value> = queries(name)
        .into_iter()
        .map(|(id, goal)| measure(&root, index.path(), &id, &goal))
        .collect();
    let current = json!({ "corpus": { "name": name, "rev": got }, "queries": measured });

    if update {
        let mut text = serde_json::to_string_pretty(&current).expect("serializes");
        text.push('\n');
        std::fs::write(&path, text).expect("writes counters.json");
        return Ok(());
    }

    let recorded = recorded.expect("checked above");
    let by_id = |v: &Value| -> Vec<(String, Value)> {
        v.get("queries")
            .and_then(Value::as_array)
            .map(|qs| {
                qs.iter()
                    .map(|q| {
                        (
                            q.get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            q.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (old, new) = (by_id(&recorded), by_id(&current));
    if old
        .iter()
        .map(|(id, _)| id)
        .ne(new.iter().map(|(id, _)| id))
    {
        writeln!(
            failures,
            "{name}: the query set differs from the one the counters were recorded for"
        )
        .expect("writes to a String");
        return Ok(());
    }
    for ((id, a), (_, b)) in old.iter().zip(&new) {
        for key in ["status", "truncated", "rows", "derived", "demand", "digest"] {
            if a.get(key) != b.get(key) {
                writeln!(
                    failures,
                    "{name} query {id} {key}: {} -> {}\n  {}",
                    a.get(key).unwrap_or(&Value::Null),
                    b.get(key).unwrap_or(&Value::Null),
                    reading(key)
                )
                .expect("writes to a String");
            }
        }
    }
    Ok(())
}

/// What a moved counter means, so the diff is read before it is recorded.
fn reading(key: &str) -> &'static str {
    match key {
        "rows" => "the answer set changed size: a correctness change",
        "digest" => {
            "same number of rows, different rows: a correctness change, and the one a \
             row count cannot see (#88)"
        }
        "derived" => "same answer, different work: a cost change",
        "truncated" => {
            "the row cap started or stopped firing, so the digest covers a different prefix"
        }
        _ => "the query no longer resolves the way it did",
    }
}

fn head_rev(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `id<TAB>query`, `#` comments, the set `benches/corpus.rs` runs.
fn queries(name: &str) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(corpora_dir().join(name).join("queries.tsv"))
        .expect("a corpus has a query set");
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t'))
        .map(|(id, goal)| (id.to_string(), goal.to_string()))
        .collect()
}

fn index_corpus(root: &Path, index: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["index"])
        .arg(root)
        .arg("--index-dir")
        .arg(index)
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "indexing {}: {}",
        root.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// One query's counters, plus a digest of the rows themselves.
fn measure(root: &Path, index: &Path, id: &str, goal: &str) -> Value {
    let run = |raw: bool| -> Value {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_codeintel"));
        cmd.args(["query", goal, "--format", "json", "--no-refresh", "--path"])
            .arg(root)
            .arg("--index-dir")
            .arg(index);
        if raw {
            cmd.arg("--raw");
        }
        let out = cmd.output().expect("the binary runs");
        assert!(
            out.status.code() != Some(2),
            "query {id}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("the query prints JSON")
    };

    let answer = run(false);
    // The digest is over `--raw` rows: atoms as themselves, which is the tuple's
    // identity. Digesting the display form would report a rendering change as a
    // moved answer.
    let rows = run(true);
    let rows = rows.get("rows").and_then(Value::as_array);
    let mut hasher = blake3::Hasher::new();
    for row in rows.into_iter().flatten() {
        for cell in row.as_array().into_iter().flatten() {
            hasher.update(cell.as_str().unwrap_or_default().as_bytes());
            hasher.update(&[0]);
        }
        hasher.update(&[1]);
    }
    json!({
        "id": id,
        "status": answer.get("status"),
        // Recorded because tokio's and deno's larger queries hit the row cap,
        // and a digest over a capped answer covers the prefix only. Without
        // this, a cap that started or stopped firing would move no field here.
        "truncated": answer.get("truncated"),
        "rows": answer.get("rows").and_then(Value::as_array).map(Vec::len),
        "derived": answer.pointer("/stats/derived"),
        "demand": answer.pointer("/stats/demand"),
        "digest": hasher.finalize().to_hex().to_string(),
    })
}
