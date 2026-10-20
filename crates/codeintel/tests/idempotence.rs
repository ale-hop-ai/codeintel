//! Idempotence with tier B in the loop (`CLAUDE.md` § Testing expectations:
//! "index twice -> identical segments").
//!
//! `cli.rs` asserts it on the Rust fixture with `index.scip` deleted. These run
//! every fixture **with** its SCIP index, which is the configuration tier B
//! users have and the one with its own ingest, anchor join and collision
//! handling. The index is compared as bytes: every segment and the dictionary.
//! `manifest.json` records mtimes and is the one file allowed to differ.
//!
//! The other half of the rule, shuffled input order, is not testable from out
//! here: `walk` sorts, so no creation order can reach the extractor. It lives in
//! `crates/extract/tests/common/mod.rs` (`assert_order_independent`), which
//! permutes below the walk.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

const FIXTURES: [&str; 4] = ["rust", "go", "python", "typescript"];

fn reindex(root: &Path, extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["index", ".", "--scip", "index.scip"])
        .args(extra)
        .current_dir(root)
        .env("CODEINTEL_INDEX_DIR", root.join(".codeintel"))
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Every segment and the dictionary, by path, as bytes. Not `manifest.json`
/// (mtimes) and not `lock`.
fn bytes_of(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let store = root.join(".codeintel");
    let mut dirs = vec![store.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let name = path
                .strip_prefix(&store)
                .expect("under the store")
                .to_string_lossy()
                .to_string();
            if name != "manifest.json" && name != "lock" {
                out.insert(name, std::fs::read(&path).expect("readable"));
            }
        }
    }
    out
}

fn assert_same(
    fixture: &str,
    what: &str,
    before: &BTreeMap<String, Vec<u8>>,
    after: &BTreeMap<String, Vec<u8>>,
) {
    let names = |m: &BTreeMap<String, Vec<u8>>| m.keys().cloned().collect::<Vec<_>>().join("\n");
    assert_eq!(
        names(before),
        names(after),
        "{fixture}: {what} changed which files the index holds\n{}",
        common::diff(&names(before), &names(after))
    );
    let moved: Vec<&String> = before
        .iter()
        .filter(|(name, bytes)| after.get(*name) != Some(bytes))
        .map(|(name, _)| name)
        .collect();
    assert!(moved.is_empty(), "{fixture}: {what} rewrote {moved:?}");
}

fn touch(path: &Path) {
    let when = SystemTime::now() + Duration::from_secs(120);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("opens for set_times");
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("sets mtime");
}

fn indexing_twice_with_scip_is_byte_identical(fixture: &str) {
    let dir = common::indexed(fixture);
    let first = bytes_of(dir.path());
    let stderr = reindex(dir.path(), &[]);
    assert!(stderr.contains("0 indexed"), "{fixture}: {stderr}");
    assert_same(fixture, "a second index", &first, &bytes_of(dir.path()));
}

/// A SCIP index whose mtime moved is re-read and every file is re-extracted
/// against it: the whole of tier B runs again, and nothing may differ.
fn re_ingesting_the_scip_index_is_byte_identical(fixture: &str) {
    let dir = common::indexed(fixture);
    let first = bytes_of(dir.path());
    let manifest = std::fs::read(dir.path().join(".codeintel/manifest.json")).expect("manifest");
    touch(&dir.path().join("index.scip"));
    reindex(dir.path(), &[]);
    assert_ne!(
        manifest,
        std::fs::read(dir.path().join(".codeintel/manifest.json")).expect("manifest"),
        "{fixture}: the touched SCIP index was not noticed, so nothing was re-ingested"
    );
    assert_same(
        fixture,
        "re-ingesting the SCIP index",
        &first,
        &bytes_of(dir.path()),
    );
}

fn a_rebuild_is_byte_identical_to_the_first_index(fixture: &str) {
    let dir = common::indexed(fixture);
    let first = bytes_of(dir.path());
    reindex(dir.path(), &["--rebuild"]);
    assert_same(
        fixture,
        "a rebuild from nothing",
        &first,
        &bytes_of(dir.path()),
    );
}

/// One test per fixture and check, so the gate runs the twelve side by side
/// instead of three loops of four. `fixtures_are_all_covered` ties the list
/// below to [`FIXTURES`], so a fixture cannot be added to one and not the other.
macro_rules! per_fixture {
    ($($check:ident),*) => {$(
        mod $check {
            #[test]
            fn rust() { super::$check("rust") }
            #[test]
            fn go() { super::$check("go") }
            #[test]
            fn python() { super::$check("python") }
            #[test]
            fn typescript() { super::$check("typescript") }
        }
    )*};
}

per_fixture!(
    indexing_twice_with_scip_is_byte_identical,
    re_ingesting_the_scip_index_is_byte_identical,
    a_rebuild_is_byte_identical_to_the_first_index
);

#[test]
fn fixtures_are_all_covered() {
    assert_eq!(
        FIXTURES,
        ["rust", "go", "python", "typescript"],
        "per_fixture! names four fixtures; keep it and FIXTURES in step"
    );
}
