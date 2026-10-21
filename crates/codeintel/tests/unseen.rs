//! An index that saw less than the caller thinks says so (#130, #140).
//!
//! Each fixture holds real content the indexer did not read, so a test that
//! only asserted "something was reported" cannot pass while the silent
//! "not found" stays.

use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(args)
        .current_dir(root)
        .env("CODEINTEL_INDEX_DIR", root.join(".codeintel"))
        .output()
        .expect("the binary runs")
}

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, src) in files {
        std::fs::write(dir.path().join(name), src).expect("write");
    }
    dir
}

fn query_json(root: &Path, program: &str) -> serde_json::Value {
    let out = run(root, &["query", program, "--format", "json"]);
    let why = format!("not JSON: {}", String::from_utf8_lossy(&out.stdout));
    serde_json::from_slice(&out.stdout).expect(&why)
}

#[test]
fn the_first_literal_is_not_said_to_follow_a_match() {
    let dir = tree(&[("A.scala", "class A {}\n")]);
    assert!(run(dir.path(), &["index", "."]).status.success());
    let answer = query_json(dir.path(), "?- def(S,F,K,N).");
    let hint = answer["hint"].as_str().expect("an empty answer has a hint");
    assert!(hint.contains("the index holds 0 `def` row(s)"), "{hint}");
    assert!(!hint.contains("everything before it"), "{hint}");
}

#[test]
fn a_later_literal_still_says_what_matched_before_it() {
    let dir = tree(&[("a.rs", "pub fn f() {}\n")]);
    assert!(run(dir.path(), &["index", "."]).status.success());
    let answer = query_json(dir.path(), "?- def(S,F,K,\"f\"), def(S,F,K,\"nosuch\").");
    let hint = answer["hint"].as_str().expect("hint");
    assert!(
        hint.contains("everything before it in the join matched"),
        "{hint}"
    );
}

/// (file, bytes, definition inside it, reason the index gives)
fn unread_tree() -> tempfile::TempDir {
    let dir = tree(&[("lib.rs", "pub fn good() {}\n")]);
    let huge = format!("pub fn in_huge() {{}}\n{}", "// pad\n".repeat(400_000));
    std::fs::write(dir.path().join("huge.rs"), huge).expect("write");
    std::fs::write(dir.path().join("bin.rs"), b"pub fn in_bin() {}\n\xff\xfe\n").expect("write");
    std::fs::write(dir.path().join("unread.rs"), "pub fn unread() {}\n").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join("unread.rs"),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("chmod");
    }
    dir
}

/// Running as root reads a mode-0 file, so the fixture would not lose it.
fn unreadable_is_enforced(dir: &Path) -> bool {
    std::fs::read(dir.join("unread.rs")).is_err()
}

#[test]
fn a_query_for_a_symbol_in_an_unread_file_says_the_file_was_not_read() {
    let dir = unread_tree();
    let indexed = run(dir.path(), &["index", "."]);
    assert!(indexed.status.success());
    let summary = String::from_utf8_lossy(&indexed.stderr).into_owned();
    assert!(summary.contains("not read:"), "{summary}");

    let mut lost = vec![
        ("in_huge", "huge.rs", "too large"),
        ("in_bin", "bin.rs", "not UTF-8"),
    ];
    if unreadable_is_enforced(dir.path()) {
        lost.push(("unread", "unread.rs", "unreadable"));
    }
    for (symbol, file, reason) in lost {
        let answer = query_json(dir.path(), &format!("?- def(S,F,K,\"{symbol}\")."));
        assert_eq!(answer["rows"].as_array().map(Vec::len), Some(0), "{symbol}");
        let hint = answer["hint"].as_str().expect("hint");
        assert!(
            hint.contains(&format!("{file} ({reason})")),
            "{symbol}: {hint}"
        );
    }
}

#[test]
fn status_lists_what_was_not_read_and_does_not_call_it_changed() {
    let dir = unread_tree();
    for _ in 0..2 {
        assert!(run(dir.path(), &["index", "."]).status.success());
        let out = run(dir.path(), &["status", ".", "--format", "json"]);
        let status: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
        assert_eq!(status["skipped"]["huge.rs"]["reason"], "too-large");
        assert_eq!(status["skipped"]["bin.rs"]["reason"], "not-utf8");
        assert_eq!(status["changed"], serde_json::json!([]), "{status}");
        assert_eq!(
            status["status"], "ok",
            "re-indexing cannot clear a `stale` it caused"
        );
    }
}

#[test]
fn a_skipped_file_that_becomes_readable_is_indexed() {
    let dir = tree(&[("bin.rs", "pub fn was_bin() {}\n")]);
    std::fs::write(dir.path().join("bin.rs"), b"pub fn was_bin() {}\n\xff\n").expect("write");
    assert!(run(dir.path(), &["index", "."]).status.success());
    std::fs::write(dir.path().join("bin.rs"), "pub fn was_bin() {}\n// fixed\n").expect("write");
    let out = run(dir.path(), &["status", ".", "--format", "json"]);
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(
        status["status"], "stale",
        "an edited skipped file is a change"
    );
    assert!(run(dir.path(), &["index", "."]).status.success());
    let answer = query_json(dir.path(), "?- def(S,F,K,\"was_bin\").");
    assert_eq!(answer["rows"].as_array().map(Vec::len), Some(1));
    let out = run(dir.path(), &["status", ".", "--format", "json"]);
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(status["skipped"], serde_json::json!({}));
}

#[test]
fn an_index_of_zero_files_says_so_in_an_empty_answer_and_in_status() {
    let dir = tree(&[("A.scala", "class A {}\n"), ("README.md", "# hi\n")]);
    assert!(run(dir.path(), &["index", "."]).status.success());
    for program in ["?- def(S,F,K,N).", "?- impact_of(S,C)."] {
        let answer = query_json(dir.path(), program);
        let hint = answer["hint"].as_str().expect("hint");
        assert!(
            hint.contains("the index holds 0 files"),
            "{program}: {hint}"
        );
    }
    let out = run(dir.path(), &["status", ".", "--format", "json"]);
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(status["files"], 0);
}
