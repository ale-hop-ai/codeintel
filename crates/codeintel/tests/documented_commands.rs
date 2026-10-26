//! Shell commands the repo documents, executed where that is cheap: #93.
//!
//! `eval_commands.rs` (#87) runs the example's commands and `documented_queries.rs`
//! (#89) runs every documented query. This is the third of the family, for the
//! shell lines a person copies out of the repository.
//!
//! What is *not* here is as deliberate as what is. `tasks/*.yml` are run by
//! `provision`, so a test would be the gate running the gate: `ci.yml` is the
//! gate itself, `ci-fast.yml` and `lints-check.yml` repeat steps it takes, and
//! `build.yml`/`install.yml` are a 50 s release build. Those were run by hand
//! on 2026-10-20 and all passed; nothing here pretends to guard them. What can
//! be asserted for nothing is below.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

/// The no-index hint for a tree holding `files`, with the `run: ` prefix gone.
fn hint_for(dir: &Path, files: &[(&str, &str)]) -> String {
    for (name, body) in files {
        std::fs::write(dir.join(name), body).expect("writes a source file");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["query", "?- file(F, L)."])
        .current_dir(dir)
        .env("CODEINTEL_INDEX_DIR", dir.join("nowhere"))
        .output()
        .expect("the binary runs");
    let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains(" run: "),
        "no `run:` in the no-index answer:\n{text}"
    );
    let run = text.split(" run: ").nth(1).expect("checked above");
    run.lines().next().expect("a line").to_owned()
}

fn parses(command: &str) -> bool {
    Command::new("sh")
        .args(["-n", "-c", command])
        .status()
        .expect("sh runs")
        .success()
}

#[test]
fn a_rust_only_no_index_hint_is_valid_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let hint = hint_for(dir.path(), &[("a.rs", "fn a() {}\n")]);
    assert!(hint.starts_with("codeintel index "), "{hint}");
    assert!(parses(&hint), "not valid shell: {hint}");
}

/// Was a known failure pinned to `false` until #93 landed; the `--project-name
/// <name>` placeholder made `<` a redirect, so any tree holding Python turned
/// the whole chain into a syntax error. Now a plain assertion, which is what
/// the constant it replaced was there to become.
#[test]
fn a_polyglot_no_index_hint_is_valid_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let hint = hint_for(
        dir.path(),
        &[("a.rs", "fn a() {}\n"), ("c.py", "def c(): pass\n")],
    );
    assert!(parses(&hint), "not valid shell: {hint}");
}

/// Was a known failure until #93 quoted the path: `cd /tmp/sp ace && ...`
/// walks into the wrong directory, or none. There is no `cd` now; the root is
/// `index`'s argument, and must reach it as one word.
#[test]
fn a_root_with_a_space_in_it_is_walked_into_by_the_hint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spaced = dir.path().join("with space");
    std::fs::create_dir(&spaced).expect("mkdir");
    let hint = hint_for(&spaced, &[("a.rs", "fn a() {}\n")]);
    let lands = Command::new("sh")
        .args([
            "-c",
            &format!("set -- {hint}; test \"$3\" -ef '{}'", spaced.display()),
        ])
        .status()
        .expect("sh runs")
        .success();
    assert!(
        lands,
        "the hint did not name the spaced root as one word. hint: {hint}"
    );
}

/// A task named in a document that says what exists now must be a file in
/// `tasks/`. `docs/onboarding.md` is a proposal and names tasks that do not
/// exist yet, on purpose, so it is not read.
#[test]
fn every_task_the_docs_name_exists() {
    let mut sources = vec![
        root().join("README.md"),
        root().join("CLAUDE.md"),
        root().join("docs/plan.md"),
    ];
    for dir in ["specs", "tasks"] {
        for entry in std::fs::read_dir(root().join(dir)).expect("readable") {
            sources.push(entry.expect("entry").path());
        }
    }
    let mut missing = Vec::new();
    for path in sources.iter().filter(|p| p.is_file()) {
        let text = std::fs::read_to_string(path).expect("text");
        for name in task_names(&text) {
            if !root().join("tasks").join(&name).exists() {
                missing.push(format!("{}: tasks/{name}", path.display()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "named but absent:\n{}",
        missing.join("\n")
    );
    assert!(
        sources.iter().any(|p| p.ends_with("README.md")),
        "the sources list lost README.md"
    );
}

/// Every `tasks/<name>.yml` in `text`, as `<name>.yml`.
fn task_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("tasks/") {
        let tail = rest.get(at + "tasks/".len()..).expect("a char boundary");
        let name: String = tail
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.'))
            .collect();
        let name = name.trim_end_matches('.');
        if name
            .strip_suffix(".yml")
            .is_some_and(|stem| !stem.is_empty())
        {
            names.push(name.to_owned());
        }
        rest = tail;
    }
    names
}

#[test]
fn the_task_scanner_finds_what_it_should() {
    assert_eq!(
        task_names("run `tasks/ci-fast.yml`, not tasks/ or tasks/x.txt; tasks/a1.yml."),
        ["ci-fast.yml", "a1.yml"]
    );
}
