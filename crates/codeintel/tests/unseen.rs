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
