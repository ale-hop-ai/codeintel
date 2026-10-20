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
fn the_scip_stale_hints_are_valid_shell(fixture: &str) {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let dir = tempfile::tempdir().expect("tempdir");
    let copied = Command::new("cp")
        .arg("-R")
        .arg(fixtures.join(fixture).join("."))
        .arg(dir.path())
        .status()
        .expect("cp runs");
    assert!(copied.success(), "{fixture}");
    // `.codeintel/` is gitignored, so a developer who ran `codeintel index`
    // inside a fixture leaves one behind and `cp -R` drags it along. The
    // store below then points at that index instead of a fresh one, and the
    // test fails with `stale` — schema 2 against a build speaking 3 — on
    // that machine only. `cli.rs:45` already skips the name when it copies;
    // this does the same, after the fact, because `cp` cannot exclude.
    let store = dir.path().join(".codeintel");
    if store.exists() {
        std::fs::remove_dir_all(&store).expect("drop a copied-in store");
    }
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
    // Asserted, not discarded. This call used to throw its status away, so a
    // failed setup surfaced three assertions later as `no run: in <output>` --
    // which names the hint and says nothing about the index that was never
    // built. #110 has three sightings and no usable message, and this is one
    // reason why: the first thing that can fail was the one thing not checked.
    //
    // Only the setup is asserted. The calls below are `query` and `status` on
    // a deliberately degraded index, where a non-zero exit is the answer being
    // measured rather than a failure.
    let indexed = run(&["index", ".", "--scip", "index.scip"]);
    assert!(
        indexed.contains("indexed") && indexed.contains("scip:"),
        "{fixture}: indexing the copy did not ingest a SCIP index: {indexed}"
    );
    // Bytes the SCIP index did not see, in a file the index holds.
    let source = first_source(dir.path());
    assert!(source.is_some(), "{fixture} has a source file");
    let source = source.expect("checked");
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
            .find_map(|l| l.split_once("run: ").map(|(_, rest)| rest));
        assert!(line.is_some(), "{fixture} {producer}: no run: in {out}");
        assert_parses(
            &format!("{fixture} {producer} scip-stale hint"),
            line.expect("checked"),
        );
    }
}

/// One test per language, so the four run side by side.
#[test]
fn the_scip_stale_hints_are_valid_shell_for_rust() {
    the_scip_stale_hints_are_valid_shell("rust");
}

#[test]
fn the_scip_stale_hints_are_valid_shell_for_go() {
    the_scip_stale_hints_are_valid_shell("go");
}

#[test]
fn the_scip_stale_hints_are_valid_shell_for_python() {
    the_scip_stale_hints_are_valid_shell("python");
}

#[test]
fn the_scip_stale_hints_are_valid_shell_for_typescript() {
    the_scip_stale_hints_are_valid_shell("typescript");
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

/// A tree whose chain ends in `scip-python index .` — the shape #104 reported —
/// with a real call in the Rust so one query returns rows and composes nothing.
fn two_language_tree() -> tempfile::TempDir {
    let tree = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tree.path().join("f.rs"),
        "fn callee() {}\nfn caller() { callee(); }\n",
    )
    .expect("write");
    std::fs::write(tree.path().join("f.py"), "def f():\n    pass\n").expect("write");
    tree
}

/// The `run: ` tail of the hint `goal` produces, under one hint shape.
fn chain_under(tree: &Path, shape: &str, goal: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["query", goal])
        .current_dir(tree)
        .env("XDG_CACHE_HOME", tree.join(".cache-for-test"))
        .env("CODEINTEL_HINT_SHAPE", shape)
        .env_remove("CODEINTEL_INDEX_DIR")
        .output()
        .expect("the binary runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // `expect` and not `panic!`: a helper in tests/ is production code to the
    // panic-surface lints (clippy.toml). Both queries are `no-scip` on this
    // tree, so both hints carry a chain.
    let (_, after) = text
        .split_once("run: ")
        .expect("a no-scip hint carries a runnable chain");
    after.lines().next().expect("a line").to_string()
}

/// Composing a second clause onto a hint must leave its runnable chain alone.
///
/// Not a syntax check — the broken output *is* valid shell, which is why
/// `sh -n` cannot see it (#104). When the chain's last command ends in `.`,
/// the `. also:` opening the next clause is read as two more arguments to it:
/// `scip-python index .. also: …`, and `..` is a valid path, so the command
/// runs against the parent directory.
///
/// So this asserts the chain's **boundary**: two queries against one tree in
/// one condition, one returning rows and composing nothing, one returning none
/// and composing the emptiness. The `run: ` tail must be identical.
#[test]
fn composing_a_hint_leaves_its_runnable_chain_untouched() {
    let tree = two_language_tree();
    assert!(codeintel(tree.path(), &["index", "."]).status.success());
    // Both are `no-scip`, so both carry the same chain. Only the second is
    // empty, and emptiness is what composes.
    let (rows, empty) = (
        "?- calls(A, B).",
        "?- def(S, F, K, N), scip_ref(S, F, _, _, _, _).",
    );

    for shape in ["chain-last", "own-line"] {
        assert_eq!(
            chain_under(tree.path(), shape, rows),
            chain_under(tree.path(), shape, empty),
            "{shape}: composition appended to the runnable chain's line. Everything \
             after the last command is read as more arguments to it, so an agent \
             that pastes the advertised fix runs it against the wrong input (#104)"
        );
    }

    // What ships, recorded so this branch states the difference rather than
    // implying it. When a shape is adopted, this is the assertion that flips.
    assert_ne!(
        chain_under(tree.path(), "glued", rows),
        chain_under(tree.path(), "glued", empty),
        "the default no longer glues: #104 is fixed, so adopt a shape and \
         delete this assertion"
    );
}
