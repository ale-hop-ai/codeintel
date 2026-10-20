//! Shared by the golden tests — `cost`, `counters`, `eval_rot`, `eval_banks`.
//!
//! One indexed copy of a fixture, with the clock pinned, and the text diff that
//! reports a moved golden. Every item here is used by every test that declares
//! `mod common;`, so none of it is dead in any of them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

pub(crate) fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// A copy of a fixture, indexed with both tiers.
///
/// The clock is pinned the way `tests/scip.rs` pins it: every source at a fixed
/// past instant and `index.scip` a minute later. A copy that crosses a second
/// boundary would otherwise leave some sources newer than the SCIP index, they
/// would index tier A only, and every answer reading them would flake.
pub(crate) fn indexed(fixture: &str) -> tempfile::TempDir {
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

/// The lines that differ, as `-expected` / `+got`, keyed by position. A golden
/// is ordered, so a position is one query.
pub(crate) fn diff(expected: &str, got: &str) -> String {
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
