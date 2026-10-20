//! Byte goldens: `docs/50-self-improvement.md` § 2 (Cost), Tier 0.
//!
//! `counters.rs`'s twin. Where that pins what the engine *did* — `derived`,
//! `demand`, `transformed` — this pins what the answer *costs*: the bytes of
//! the answer as each surface actually serializes it, for every query in
//! `tests/fixtures/cost/queries.tsv`, diffed as text against `expected.tsv`.
//!
//! Bytes, not tokens. Bytes are exact and tokenizer-independent, so this is a
//! cost gate CI can run with no clock and no tolerance.
//!
//! The measurement is [`Wire::bytes`] — the same function the byte cap is
//! enforced with, which encodes through the same `wire::body` / `wire::json` /
//! `wire::tool_result` that stdout and an MCP result are built from. A byte
//! count that is not what was sent is not a cost metric
//! (`specs/05-surface.md` § `query`, the byte cap).
//!
//! Each query is measured once per wire, because the wire is an input: the
//! byte cap is enforced against the wire's own encoding, so the surface can
//! decide which rows survive.
//!
//! Reading a diff. `rows` unchanged and `bytes` moved: a cost change — the
//! same answer got cheaper or dearer to send. `rows` moved: a correctness
//! change, and the bytes moved because the answer did. `truncated` moved: a
//! cap started or stopped firing, which is both. Say which one it was, then
//! `UPDATE_GOLDEN=1 cargo test -p codeintel --test cost`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use codeintel::Wire;
use codeintel::query::{self, Options};

const QUERIES: &str = include_str!("../../../tests/fixtures/cost/queries.tsv");

const HEADER: &str = "id\tfixture\twire\tstatus\trows\ttruncated\tbytes\n";

/// The four surfaces an answer is sent on, with the name the golden calls each.
/// Exhaustive over [`Wire`]: a fifth surface must cost a golden column, not
/// land unmeasured.
const WIRES: [(Wire, &str); 4] = [
    (Wire::Text, "text"),
    (Wire::Json, "json"),
    (Wire::McpText, "mcp-text"),
    (Wire::McpJson, "mcp-json"),
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

#[test]
fn every_fixed_query_costs_the_bytes_the_golden_says() {
    let rust = indexed("rust");
    let go = indexed("go");

    let mut got = String::from(HEADER);
    for line in QUERIES
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let mut fields = line.splitn(3, '\t');
        let shape = "queries.tsv rows are id<TAB>fixture<TAB>query";
        let (id, fixture, goal) = (
            fields.next().expect(shape),
            fields.next().expect(shape),
            fields.next().expect(shape),
        );
        let root = [("rust", &rust), ("go", &go)]
            .into_iter()
            .find(|(name, _)| *name == fixture)
            .map(|(_, dir)| dir.path())
            .expect("queries.tsv names a fixture this test indexes");
        for (wire, name) in WIRES {
            // Invariant 8 says the same tree and the same query produce
            // byte-identical output. A byte count that moves between two runs
            // of one index is that invariant broken, and comparing it against
            // a golden would flake rather than say so.
            let first = record(root, goal, wire);
            let second = record(root, goal, wire);
            assert_eq!(
                first, second,
                "query {id} on the {name} wire cost different bytes on two runs \
                 of the same index — that is non-determinism in the answer, not \
                 a golden to refresh"
            );
            writeln!(got, "{id}\t{fixture}\t{name}\t{first}").expect("writes to a String");
        }
    }

    let golden = fixtures().join("cost/expected.tsv");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &got).expect("writes the golden file");
        return;
    }
    let expected = std::fs::read_to_string(&golden).expect("the golden file exists");
    assert!(
        expected == got,
        "answer bytes changed. Rows unchanged means a cost change, rows moved \
         means a correctness change. Re-read the diff, then \
         `UPDATE_GOLDEN=1 cargo test -p codeintel --test cost`\n{}",
        diff(&expected, &got)
    );
}

/// One query's cost on one wire, tab-separated.
///
/// `rows` and `truncated` travel beside `bytes` so the diff separates the two
/// kinds of change: bytes alone is a number that cannot say whether the answer
/// got cheaper or merely smaller.
fn record(root: &Path, goal: &str, wire: Wire) -> String {
    let options = Options {
        no_refresh: true,
        wire,
        ..Options::default()
    };
    let answer = query::run(root, goal, &options).expect("the query runs");
    format!(
        "{}\t{}\t{}\t{}",
        answer.status.as_str(),
        answer.rows.len(),
        answer.truncated,
        wire.bytes(&answer, options.raw),
    )
}

/// The lines that differ, as `-expected` / `+got`, keyed by position. The
/// query set is ordered by id and each id by wire, so a position is one
/// query on one surface.
fn diff(expected: &str, got: &str) -> String {
    let (old, new): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), got.lines().collect());
    let mut out = String::new();
    for i in 0..old.len().max(new.len()) {
        let (a, b) = (old.get(i).copied(), new.get(i).copied());
        if a != b {
            if let Some(a) = a {
                writeln!(out, "-{a}").expect("writes to a String");
            }
            if let Some(b) = b {
                writeln!(out, "+{b}").expect("writes to a String");
            }
        }
    }
    out
}

/// A copy of a fixture, indexed with both tiers.
///
/// The clock is pinned the way `tests/scip.rs` and `tests/counters.rs` pin it:
/// every source at a fixed past instant and `index.scip` a minute later. A copy
/// that crosses a second boundary would otherwise leave some sources newer than
/// the SCIP index, they would index tier A only, and every answer reading them
/// would lose rows — and so bytes.
fn indexed(fixture: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixtures().join(fixture), dir.path());
    drop(std::fs::remove_file(dir.path().join("expected.facts")));
    backdate(dir.path(), SOURCE_MTIME);
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);
    let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["index", ".", "--scip", "index.scip"])
        .current_dir(dir.path())
        .env("CODEINTEL_INDEX_DIR", dir.path().join(".codeintel"))
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir
}

const SOURCE_MTIME: u64 = 1_700_000_000;

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        if name == "target" || name == ".codeintel" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().expect("file type").is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn backdate(dir: &Path, secs: u64) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            backdate(&path, secs);
        } else {
            set_mtime(&path, secs);
        }
    }
}

fn set_mtime(path: &Path, secs: u64) {
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("opens for set_times");
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("sets mtime");
}
