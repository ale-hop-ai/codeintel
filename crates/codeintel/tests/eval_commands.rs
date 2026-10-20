//! The eval's fact fixtures and documented commands, executed: #87.
//!
//! `9e5dd5e` made `exported` a stdlib rule, `tests/fixtures/eval/facts.dl` still
//! asserted it as a fact, and the stdlib stopped loading against that fixture.
//! Nothing noticed for over a month: `eval_rot.rs` and `eval_banks.rs` read
//! `bank.dl` and the rust fixture, and `examples/eval.rs` claimed in prose that
//! every recorded command still meant what it meant. Prose is not checked.
//!
//! Two tests, both of which RUN the example binary rather than reading anything:
//!
//! - every `*.dl` fact fixture under `tests/fixtures` loads against the shipped
//!   stdlib and renders its schema page;
//! - every `cargo run ... --example eval -- <args>` line in the example's own
//!   module doc runs, with the literal documented arguments, and exits 0.
//!
//! A fixture that is known dead is listed in [`KNOWN_DEAD`] with the issue that
//! owns the decision. It must still fail, with the fixture-load exit code, so a
//! listed fixture cannot hide a different breakage, and the moment it is
//! repaired the entry is a failure to delete. The list is not a place to park
//! things: it is the decision, recorded where CI reads it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// Fixtures that do not load, and the open decision for each.
const KNOWN_DEAD: &[(&str, &str)] = &[("tests/fixtures/eval/facts.dl", "#87")];

/// `examples/eval.rs` exits with this when the fixture or stdlib will not load.
const LOAD_FAILED: i32 = 3;

/// The fixture the example embeds when `--facts` is absent.
const DEFAULT_FIXTURE: &str = "tests/fixtures/eval/facts.dl";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

/// Built here, not assumed: `cargo test --test eval_commands` does not build
/// examples, and a plain `cargo test` that does is not a thing to depend on.
///
/// **Under a file lock, because `OnceLock` shares nothing across processes.**
/// `nextest` runs every test in its own process, so each one reaches this and
/// each one shells out to `cargo build`. Concurrently: one process's `cargo`
/// returned success while the binary was not yet at the path the other
/// computed, and the test failed with exit 127 — `No such file or directory` —
/// four tests into the gate (#110, reproduced under load after six green runs
/// on an idle host). The same misreading the gate-cost audit found in
/// `documented_queries.rs`, where a per-process `OnceLock` was rebuilding the
/// repository index once per test.
///
/// So the first process builds and the rest wait, and the path is checked
/// rather than assumed: a future failure should say "built, but not where I
/// looked" instead of 127.
fn example() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("eval-commands");
        std::fs::create_dir_all(&home).expect("a scratch directory");
        let lock = std::fs::File::create(home.join("build.lock")).expect("a lock file");
        lock.lock().expect("takes the lock");
        let built = Command::new(env!("CARGO"))
            .args(["build", "-q", "-p", "codeintel", "--example", "eval"])
            .current_dir(root())
            .status()
            .expect("cargo runs");
        assert!(built.success(), "cargo build --example eval failed");
        let exe = std::env::current_exe().expect("test binary path");
        let path = exe
            .parent()
            .and_then(Path::parent)
            .expect("target/<profile>/deps")
            .join("examples")
            .join(format!("eval{}", std::env::consts::EXE_SUFFIX));
        assert!(
            path.exists(),
            "cargo reported success but {} is not there; the computed path or \
             the build's output directory has moved",
            path.display()
        );
        path
    })
}

fn dead(fixture: &str) -> Option<&'static str> {
    KNOWN_DEAD
        .iter()
        .find(|(path, _)| *path == fixture)
        .map(|(_, issue)| *issue)
}

fn run(args: &str) -> Output {
    Command::new("sh")
        .arg("-c")
        .arg(format!("{} {args}", example().display()))
        .current_dir(root())
        .output()
        .expect("sh runs")
}

/// `Ok` when `args` behaved as `fixture`'s standing says it must.
fn check(fixture: &str, args: &str) -> Result<(), String> {
    let out = run(args);
    let code = out.status.code();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    match (dead(fixture), code) {
        (None, Some(0)) | (Some(_), Some(LOAD_FAILED)) => Ok(()),
        (None, _) => Err(format!(
            "`eval {args}` exited {code:?}\nstdout: {stdout}\nstderr: {stderr}"
        )),
        (Some(issue), Some(0)) => Err(format!(
            "`eval {args}` now runs, but {fixture} is listed in KNOWN_DEAD for {issue}: \
             delete the entry"
        )),
        (Some(issue), _) => Err(format!(
            "`eval {args}` exited {code:?}, not {LOAD_FAILED}: {fixture} is listed dead for \
             {issue} but is failing some other way\nstderr: {stderr}"
        )),
    }
}

fn fixtures(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("fixture dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            fixtures(&path, out);
        } else if path.extension().is_some_and(|e| e == "dl") {
            out.push(path);
        }
    }
}

#[test]
fn every_fact_fixture_loads_against_the_stdlib() {
    let mut all = Vec::new();
    fixtures(&root().join("tests/fixtures"), &mut all);
    assert!(!all.is_empty(), "found no .dl fixtures; the walk is broken");

    let failures: Vec<String> = all
        .iter()
        .filter_map(|path| {
            let rel = path.strip_prefix(root()).expect("under the repo root");
            let rel = rel.to_string_lossy().replace('\\', "/");
            check(&rel, &format!("--facts {rel} --schema")).err()
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_documented_eval_command_runs() {
    let source = std::fs::read_to_string(root().join("crates/codeintel/examples/eval.rs"))
        .expect("examples/eval.rs");
    let documented: Vec<&str> = source
        .lines()
        .filter_map(|l| l.strip_prefix("//! cargo run -p codeintel --example eval -- "))
        .collect();
    assert!(
        documented.len() >= 3,
        "found {} documented commands in examples/eval.rs; the module doc changed shape and \
         this test no longer reads it",
        documented.len()
    );

    let failures: Vec<String> = documented
        .iter()
        .filter_map(|args| {
            let fixture = args
                .split_once("--facts ")
                .map_or(DEFAULT_FIXTURE, |(_, rest)| {
                    rest.split_whitespace().next().unwrap_or(DEFAULT_FIXTURE)
                });
            check(fixture, args).err()
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
