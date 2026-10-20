//! The commands `codeintel` prints for a user to paste are shell, so they are
//! run through a shell's parser (`specs/05-surface.md` § The no-index hint).
//!
//! `sh -n` parses and executes nothing. Two defects shipped because the hint
//! was asserted to be copy-pasteable and never pasted (#93).

use std::path::Path;
use std::process::{Command, Output};

fn codeintel(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(args)
        .current_dir(root)
        .env("XDG_CACHE_HOME", root.join(".cache-for-test"))
        .env_remove("CODEINTEL_INDEX_DIR")
        .output()
        .expect("the binary runs")
}

fn sh(args: &[&str]) -> Output {
    Command::new("sh").args(args).output().expect("sh runs")
}

/// A tree holding a source file of every language the indexer table covers, at
/// a root a naive `cd` cannot survive: a space and a single quote.
fn polyglot_tree() -> tempfile::TempDir {
    let tree = tempfile::Builder::new()
        .prefix("it's a tree ")
        .tempdir()
        .expect("tempdir");
    for lang in extract::lang::LANGS {
        for ext in lang.extensions {
            std::fs::write(tree.path().join(format!("f.{ext}")), "").expect("write");
        }
    }
    tree
}

fn assert_parses(what: &str, line: &str) {
    let out = sh(&["-n", "-c", line]);
    assert!(
        out.status.success(),
        "{what} is not valid sh: {}\n{line}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_no_index_hint_is_valid_shell_over_every_language() {
    let tree = polyglot_tree();
    let out = codeintel(tree.path(), &["query", "?- def(S,F,K,N)."]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let (_, line) = text
        .split_once("run: ")
        .unwrap_or_else(|| panic!("no hint in: {text}"));
    let line = line.lines().next().expect("a line");

    assert_parses("the no-index chain", line);
    for lang in extract::lang::LANGS {
        let command = lang.indexer;
        assert!(line.contains(command), "{command} missing from: {line}");
    }

    // `cd` must land in the root, not in the first word of it.
    let (cd, _) = line.split_once(" && ").expect("a chain");
    let landed = sh(&["-c", &format!("{cd} && pwd -P")]);
    assert!(landed.status.success(), "{cd}");
    assert_eq!(
        String::from_utf8_lossy(&landed.stdout).trim_end(),
        tree.path()
            .canonicalize()
            .expect("canonical")
            .to_string_lossy()
    );
}

#[test]
fn every_indexer_command_is_valid_shell_on_its_own() {
    for lang in extract::lang::LANGS {
        assert_parses(lang.name, lang.indexer);
    }
}

#[test]
fn the_build_one_list_that_index_prints_is_valid_shell() {
    let tree = polyglot_tree();
    let out = codeintel(tree.path(), &["index", "."]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let (_, list) = stderr
        .split_once("build one:")
        .unwrap_or_else(|| panic!("no build-one list in: {stderr}"));
    let commands: Vec<&str> = list
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with("    "))
        .map(str::trim)
        .collect();
    assert!(!commands.is_empty(), "{stderr}");
    for command in commands {
        assert_parses("a build-one line", command);
    }
}

/// The `scip-stale` hint rides on every answer until someone re-runs the
/// indexers, so it is the paste-ready line an agent is shown most. It has no
/// `cd` and builds its own chain, so it is checked as its own producer, for
/// each language, rather than inferred from the `no-index` one.
#[test]
fn the_scip_stale_hints_are_valid_shell_for_each_language() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for fixture in ["rust", "go", "python", "typescript"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let copied = Command::new("cp")
            .arg("-R")
            .arg(fixtures.join(fixture).join("."))
            .arg(dir.path())
            .status()
            .expect("cp runs");
        assert!(copied.success(), "{fixture}");
        let store = dir.path().join(".codeintel");
        let run = |args: &[&str]| {
            let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
                .args(args)
                .current_dir(dir.path())
                .env("CODEINTEL_INDEX_DIR", &store)
                .output()
                .expect("the binary runs");
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        };
        run(&["index", ".", "--scip", "index.scip"]);
        // Bytes the SCIP index did not see, in a file the index holds.
        let source =
            first_source(dir.path()).unwrap_or_else(|| panic!("{fixture} has a source file"));
        let mut text = std::fs::read_to_string(&source).expect("read");
        text.push_str("\n\n");
        std::fs::write(&source, text).expect("write");
        // Refresh keys on mtime at one-second grain; the copy and this edit
        // can share a second, so move the edit past it.
        std::fs::File::options()
            .write(true)
            .open(&source)
            .and_then(|f| {
                f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            })
            .expect("set mtime");

        // `query` first: it refreshes the index, which is what turns bytes the
        // SCIP index did not see into `scip-stale`. `status` only reads.
        for (producer, args) in [
            ("query", &["query", "?- scip_ref(S,F,L,C,From,R)."][..]),
            ("status", &["status"][..]),
        ] {
            let out = run(args);
            assert!(
                out.contains("scip-stale") || out.contains("scip stale"),
                "{fixture} {producer}: {out}"
            );
            let line = out
                .lines()
                .find_map(|l| l.split_once("run: ").map(|(_, rest)| rest))
                .unwrap_or_else(|| panic!("{fixture} {producer}: no run: in {out}"));
            assert_parses(&format!("{fixture} {producer} scip-stale hint"), line);
        }
    }
}

fn first_source(dir: &Path) -> Option<std::path::PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .map(|e| e.expect("entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == ".codeintel") {
                continue;
            }
            if let Some(found) = first_source(&path) {
                return Some(found);
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["rs", "go", "py", "ts"].contains(&e))
        {
            return Some(path);
        }
    }
    None
}
