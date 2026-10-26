//! A file that parses with errors is partial, and every surface says so
//! (`specs/02-extraction.md` § A file that does not parse is partial, #131).
//!
//! Each fixture is built so **real content is lost**: a valid declaration
//! follows the broken one and tree-sitter's recovery swallows it. A test that
//! only asserted "an error was reported" would pass while the bug — a
//! confidently wrong "that function does not exist" — stayed.

use std::path::Path;
use std::process::{Command, Output};

/// (file, source, valid definition the error swallows, line of first error)
const LOSSY: &[(&str, &str, &str, u32)] = &[
    (
        "bad.go",
        "package main\nfunc A() {}\nfunc B( {\nfunc C() {}\n",
        "C",
        3,
    ),
    (
        "bad.py",
        "def a():\n    pass\nx = (1,\ndef c():\n    pass\n",
        "c",
        3,
    ),
    (
        "bad.ts",
        "function a() {}\nfunction b( {\nfunction c() {}\n",
        "c",
        2,
    ),
];

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(args)
        .current_dir(root)
        .env("CODEINTEL_INDEX_DIR", root.join(".codeintel"))
        // No indexer on this host reaches the child: a tree with no
        // `index.scip` is indexed tier A, as it was before `index` ran them
        // (`specs/02-extraction.md` § Acquisition). `tests/indexers.rs` tests
        // the runner with a stub.
        .env_remove("PATH")
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

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn query_json(root: &Path, program: &str) -> serde_json::Value {
    let out = run(root, &["query", program, "--format", "json"]);
    let why = format!("not JSON: {}", String::from_utf8_lossy(&out.stdout));
    serde_json::from_slice(&out.stdout).expect(&why)
}

#[test]
fn the_fixtures_really_lose_the_valid_definition() {
    for (file, src, lost, _) in LOSSY {
        let dir = tree(&[(file, src)]);
        assert!(run(dir.path(), &["index", "."]).status.success());
        let answer = query_json(dir.path(), &format!("?- def(S,\"{file}\",_,\"{lost}\")."));
        assert_eq!(
            answer["rows"].as_array().map(Vec::len),
            Some(0),
            "{file}: `{lost}` is a valid declaration and the grammar's recovery is expected to \
             swallow it; if it no longer does, the fixture no longer shows the bug"
        );
    }
}

#[test]
fn index_status_and_an_empty_query_each_name_the_file_and_line() {
    for (file, src, lost, line) in LOSSY {
        let dir = tree(&[(file, src)]);
        let indexed = run(dir.path(), &["index", "."]);
        let here = format!("{file}:{line}");
        assert!(
            stderr(&indexed).contains("syntax errors: 1 file(s) parsed with errors")
                && stderr(&indexed).contains(&here),
            "index: {}",
            stderr(&indexed)
        );

        let status = run(dir.path(), &["status", ".", "--format", "json"]);
        let json: serde_json::Value = serde_json::from_slice(&status.stdout).expect("json");
        assert_eq!(json["syntax_errors"][file], *line, "status json");
        let text = String::from_utf8_lossy(&run(dir.path(), &["status", "."]).stdout).into_owned();
        assert!(text.contains(&here), "status text: {text}");

        let answer = query_json(dir.path(), &format!("?- def(S,\"{file}\",_,\"{lost}\")."));
        assert_eq!(answer["status"], "ok");
        let hint = answer["hint"].as_str().expect("an empty result has a hint");
        assert!(
            hint.contains("parsed with errors") && hint.contains(&here),
            "{hint}"
        );
    }
}

#[test]
fn an_unchanged_broken_file_is_still_reported_on_the_next_index() {
    let (file, src, _, line) = LOSSY[0];
    let dir = tree(&[(file, src)]);
    run(dir.path(), &["index", "."]);
    let again = stderr(&run(dir.path(), &["index", "."]));
    assert!(again.contains("1 unchanged"), "{again}");
    assert!(again.contains(&format!("{file}:{line}")), "{again}");
}

#[test]
fn a_tree_that_parses_whole_says_nothing() {
    let dir = tree(&[("ok.go", "package main\nfunc A() {}\nfunc C() {}\n")]);
    let indexed = run(dir.path(), &["index", "."]);
    assert!(
        !stderr(&indexed).contains("syntax errors"),
        "{}",
        stderr(&indexed)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&run(dir.path(), &["status", ".", "--format", "json"]).stdout)
            .expect("json");
    assert_eq!(json["syntax_errors"], serde_json::json!({}));
    let answer = query_json(dir.path(), "?- def(S,\"ok.go\",_,\"Nope\").");
    let hint = answer["hint"].as_str().expect("empty result has a hint");
    assert!(!hint.contains("parsed with errors"), "{hint}");
}

#[test]
fn a_non_empty_answer_carries_no_syntax_hint() {
    let dir = tree(&[("bad.go", LOSSY[0].1)]);
    run(dir.path(), &["index", "."]);
    let answer = query_json(dir.path(), "?- def(S,\"bad.go\",_,\"A\").");
    assert!(answer["hint"].is_null(), "{}", answer["hint"]);
}
