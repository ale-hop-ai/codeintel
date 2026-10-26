//! `index` runs the SCIP indexers itself (`specs/02-extraction.md`
//! § Acquisition), one test per rule.
//!
//! No real indexer runs here. `PATH` holds a stub directory and the system
//! one, and the stub `rust-analyzer` copies the fixture's committed
//! `index.scip` to the output it is given and logs the call — so these test
//! what `index` does around an indexer, on any host, in milliseconds. That an
//! indexer's real flags work is § Indexer evidence's job, not this file's.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust")
}

/// Every copied source is stamped with this, so an output written now is
/// newer than all of them.
const SOURCE_MTIME: u64 = 1_700_000_000;

struct Setup {
    tree: tempfile::TempDir,
    bin: tempfile::TempDir,
    store: tempfile::TempDir,
}

impl Setup {
    /// A copy of the Rust fixture with no `index.scip`, and a stub
    /// `rust-analyzer` that exits `code` after copying the fixture's index to
    /// its output (only when `code` is 0).
    fn new(code: i32) -> Self {
        let tree = tempfile::tempdir().expect("tempdir");
        let copied = Command::new("cp")
            .arg("-R")
            .arg(fixture().join("."))
            .arg(tree.path())
            .status()
            .expect("cp runs");
        assert!(copied.success());
        for name in ["index.scip", "expected.facts"] {
            std::fs::remove_file(tree.path().join(name)).expect("in the fixture");
        }
        drop(std::fs::remove_dir_all(tree.path().join(".codeintel")));
        backdate(tree.path());
        let setup = Self {
            tree,
            bin: tempfile::tempdir().expect("tempdir"),
            store: tempfile::tempdir().expect("tempdir"),
        };
        setup.stub(code);
        setup
    }

    /// (Re)write the stub to exit `code`.
    fn stub(&self, code: i32) {
        let script = format!(
            "#!/bin/sh\necho \"$PWD $*\" >> '{calls}'\nfor a; do out=$a; done\n{copy}exit {code}\n",
            calls = self.calls_file().display(),
            copy = if code == 0 {
                format!("cp '{}' \"$out\"\n", fixture().join("index.scip").display())
            } else {
                String::new()
            },
        );
        let path = self.bin.path().join("rust-analyzer");
        std::fs::write(&path, script).expect("write stub");
        let chmod = Command::new("chmod")
            .arg("+x")
            .arg(&path)
            .status()
            .expect("chmod runs");
        assert!(chmod.success());
    }

    fn calls_file(&self) -> PathBuf {
        self.bin.path().join("calls")
    }

    /// How many times the stub ran, and the lines it logged.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.calls_file())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_codeintel"))
            .args(args)
            .current_dir(self.tree.path())
            .env("CODEINTEL_INDEX_DIR", self.store.path())
            .env(
                "PATH",
                format!("{}:/bin:/usr/bin", self.bin.path().display()),
            )
            .output()
            .expect("the binary runs")
    }

    fn json(&self, program: &str) -> serde_json::Value {
        let out = self.run(&["query", program, "--format", "json"]);
        let why = format!(
            "not JSON: exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect(&why)
    }

    fn output(&self) -> PathBuf {
        self.store.path().join("scip").join("rust-analyzer.scip")
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn backdate(dir: &Path) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            backdate(&path);
        } else {
            set_mtime(
                &path,
                SystemTime::UNIX_EPOCH + Duration::from_secs(SOURCE_MTIME),
            );
        }
    }
}

fn set_mtime(path: &Path, when: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(when))
        .expect("set mtime");
}

/// New bytes, and an mtime past the output's: staleness is by content
/// (`scip_hash`), re-running by mtime.
fn edit(path: &Path) {
    let mut text = std::fs::read_to_string(path).expect("read");
    text.push('\n');
    std::fs::write(path, text).expect("write");
    set_mtime(path, SystemTime::now() + Duration::from_secs(120));
}

/// Every file under `dir`, relative, sorted.
fn listing(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("readable") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .expect("under root")
                        .display()
                        .to_string(),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn index_runs_the_indexer_into_the_store_and_ingests_it() {
    let setup = Setup::new(0);
    let before = listing(setup.tree.path());
    let out = setup.run(&["index", "."]);
    let err = stderr(&out);
    assert!(out.status.success(), "{err}");

    // Announced before it starts, with the command and the log.
    assert!(
        err.contains("codeintel: running rust-analyzer scip . --output ") && err.contains("(log: "),
        "{err}"
    );
    assert!(err.contains("rust-analyzer: ok in"), "{err}");
    let calls = setup.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    // From the root, with the output in the store.
    let root = setup.tree.path().canonicalize().expect("canonical");
    assert!(
        calls[0].starts_with(&format!("{} scip . --output ", root.display())),
        "{calls:?}"
    );
    assert!(setup.output().is_file());
    assert!(setup.store.path().join("scip/rust-analyzer.log").is_file());

    // Tier B landed: an exact call graph, and no `no-scip`.
    let answer = setup.json("?- calls_exact(A, B).");
    assert_eq!(answer["status"], "ok", "{answer}");
    assert!(
        !answer["rows"].as_array().expect("rows").is_empty(),
        "{answer}"
    );

    // codeintel wrote nothing into the tree.
    assert_eq!(listing(setup.tree.path()), before);
}

#[test]
fn an_up_to_date_output_is_not_rebuilt_and_a_newer_source_rebuilds_it() {
    let setup = Setup::new(0);
    assert!(setup.run(&["index", "."]).status.success());
    let again = setup.run(&["index", "."]);
    assert!(
        stderr(&again).contains("rust-analyzer: up to date"),
        "{}",
        stderr(&again)
    );
    assert_eq!(setup.calls().len(), 1);

    // A source newer than the output: the indexer is stale.
    let future = SystemTime::now() + Duration::from_secs(120);
    set_mtime(&setup.tree.path().join("src/lib.rs"), future);
    let stale = setup.run(&["index", "."]);
    assert!(
        stderr(&stale).contains("rust-analyzer: ok in"),
        "{}",
        stderr(&stale)
    );
    assert_eq!(setup.calls().len(), 2);
}

#[test]
fn rebuild_reruns_the_indexer_and_keeps_its_output() {
    let setup = Setup::new(0);
    assert!(setup.run(&["index", "."]).status.success());
    let out = setup.run(&["index", ".", "--rebuild"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(setup.calls().len(), 2);
    // The discard ran after the indexer, and kept what it wrote.
    assert!(setup.output().is_file());
    assert_eq!(setup.json("?- calls_exact(A, B).")["status"], "ok");
}

#[test]
fn no_scip_runs_nothing_and_reads_nothing() {
    let setup = Setup::new(0);
    let out = setup.run(&["index", ".", "--no-scip"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(setup.calls().is_empty());
    assert!(stderr(&out).contains("no SCIP index"), "{}", stderr(&out));
    let answer = setup.json("?- calls_exact(A, B).");
    assert_eq!(answer["status"], "no-scip", "{answer}");
    // The hint is the command that builds tier B.
    let hint = answer["hint"].as_str().expect("hint");
    assert!(hint.contains("run: codeintel index "), "{hint}");

    // And it conflicts with naming an input.
    let both = setup.run(&["index", ".", "--no-scip", "--scip", "x.scip"]);
    assert_eq!(both.status.code(), Some(2), "{}", stderr(&both));
}

#[test]
fn a_hand_built_index_is_read_and_nothing_is_run() {
    let setup = Setup::new(0);
    std::fs::copy(
        fixture().join("index.scip"),
        setup.tree.path().join("index.scip"),
    )
    .expect("copy");
    let out = setup.run(&["index", "."]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(setup.calls().is_empty());
    assert!(
        stderr(&out).contains("scip: index.scip"),
        "{}",
        stderr(&out)
    );

    let named = Setup::new(0);
    let input = named.bin.path().join("given.scip");
    std::fs::copy(fixture().join("index.scip"), &input).expect("copy");
    let input = input.display().to_string();
    let out = named.run(&["index", ".", "--scip", &input]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(named.calls().is_empty());
}

#[test]
fn a_failing_indexer_is_reported_and_tier_a_stands() {
    let setup = Setup::new(3);
    let out = setup.run(&["index", "."]);
    let err = stderr(&out);
    assert!(
        out.status.success(),
        "a failing indexer is not fatal: {err}"
    );
    assert!(err.contains("rust-analyzer: exit 3; see "), "{err}");
    assert!(!setup.output().exists(), "a failed run leaves no output");

    let answer = setup.json("?- calls_exact(A, B).");
    assert_eq!(answer["status"], "no-scip", "{answer}");
    let hint = answer["hint"].as_str().expect("hint");
    assert!(hint.contains("rust-analyzer: exit 3, see "), "{hint}");
    // Tier A answers.
    assert_eq!(setup.json("?- def(S, F, K, N).")["status"], "ok");

    // `status` says why too.
    let status = setup.run(&["status"]);
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(
        text.contains("indexer: rust-analyzer scip . — exit 3"),
        "{text}"
    );
}

#[test]
fn a_failing_rerun_keeps_the_previous_output() {
    let setup = Setup::new(0);
    assert!(setup.run(&["index", "."]).status.success());
    setup.stub(3);
    edit(&setup.tree.path().join("src/lib.rs"));
    let out = setup.run(&["index", "."]);
    let err = stderr(&out);
    assert!(err.contains("rust-analyzer: exit 3"), "{err}");
    // The old output is still read: the edited file is `scip-stale`, and the
    // hint sends the reader back to `index`, which produced the input.
    assert!(err.contains("scip: "), "{err}");
    let answer = setup.json("?- scip_ref(S, F, L, C, From, R).");
    assert_eq!(answer["status"], "scip-stale", "{answer}");
    let hint = answer["hint"].as_str().expect("hint");
    assert!(hint.contains("run: codeintel index "), "{hint}");
    assert!(!hint.contains("re-run the indexer that wrote"), "{hint}");
}

#[test]
fn a_missing_program_is_skipped_with_what_to_do() {
    let setup = Setup::new(0);
    std::fs::remove_file(setup.bin.path().join("rust-analyzer")).expect("rm stub");
    let out = setup.run(&["index", "."]);
    let err = stderr(&out);
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains("rust-analyzer: skipped — not on PATH; install it"),
        "{err}"
    );
    let hint = setup.json("?- calls_exact(A, B).")["hint"]
        .as_str()
        .expect("hint")
        .to_string();
    assert!(
        hint.contains("rust-analyzer is not on PATH; install it"),
        "{hint}"
    );
}

#[test]
fn a_project_file_not_at_the_root_skips_the_indexer() {
    let setup = Setup::new(0);
    std::fs::remove_file(setup.tree.path().join("Cargo.toml")).expect("rm");
    let out = setup.run(&["index", "."]);
    let err = stderr(&out);
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains("rust-analyzer: skipped — no Cargo.toml at the root"),
        "{err}"
    );
    assert!(setup.calls().is_empty());
    // Re-running `index` would skip it again, so the hint does not say to.
    let hint = setup.json("?- calls_exact(A, B).")["hint"]
        .as_str()
        .expect("hint")
        .to_string();
    assert!(hint.contains("no Cargo.toml at the root"), "{hint}");
    assert!(hint.contains("--scip-root"), "{hint}");
}

#[test]
fn the_query_path_never_runs_an_indexer() {
    let setup = Setup::new(0);
    assert!(setup.run(&["index", "."]).status.success());
    // Edited after the index: the query refreshes tier A, and runs nothing.
    edit(&setup.tree.path().join("src/lib.rs"));
    let answer = setup.json("?- scip_ref(S, F, L, C, From, R).");
    assert_eq!(answer["status"], "scip-stale", "{answer}");
    assert_eq!(setup.calls().len(), 1);
    // Nor does `status`.
    assert!(setup.run(&["status"]).status.success());
    assert_eq!(setup.calls().len(), 1);
}

#[test]
fn lang_restricts_which_indexers_run() {
    let setup = Setup::new(0);
    // A Rust tree indexed for Go only: the walk finds no Go, so `index`
    // refuses the zero-file tree (#140) — and runs no indexer on the way.
    setup.run(&["index", ".", "--lang", "go"]);
    assert!(setup.calls().is_empty(), "{:?}", setup.calls());
}
