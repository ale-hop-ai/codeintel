//! The index is not always where the caller is standing (#61), and a miss has
//! to be one copy-paste from fixed (#59).
//!
//! These tests deliberately do **not** set `CODEINTEL_INDEX_DIR`: pinning the
//! store directory is what disables discovery (`specs/04-storage.md`
//! § Discovery), so a test that pins it tests the wrong thing. They set
//! `XDG_CACHE_HOME` and `HOME` into a throwaway directory instead, which is
//! both where the default resolution puts the store and what keeps the probe
//! from ever seeing a real index on the machine running them.

use std::path::Path;
use std::process::{Command, Output};

/// An answer that is JSON. A process that printed nothing fails `from_slice`
/// with "EOF while parsing", which says nothing about why it printed nothing.
#[track_caller]
fn stdout_json(out: &Output) -> serde_json::Value {
    let why = format!(
        "exited {:?} and printed {} byte(s) of stdout, not JSON. stderr: {}",
        out.status.code(),
        out.stdout.len(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect(&why)
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

/// A tree with its own isolated cache. The cache is a sibling of the tree, not
/// a child, so the downward probe cannot find it by walking.
struct Sandbox {
    tree: tempfile::TempDir,
    cache: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            tree: tempfile::tempdir().expect("tempdir"),
            cache: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn root(&self) -> &Path {
        self.tree.path()
    }

    /// The root as every path in an answer spells it. On macOS a tempdir is
    /// `/var/...` and its canonical form is `/private/var/...`.
    fn canonical(&self) -> std::path::PathBuf {
        self.root().canonicalize().expect("canonical")
    }

    /// Write one Rust file at `rel`, creating its directory.
    fn file(&self, rel: &str, body: &str) -> &Self {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, body).expect("writable");
        self
    }

    fn run(&self, at: &Path, args: &[&str]) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(at)
            .env("XDG_CACHE_HOME", self.cache.path())
            .env("HOME", self.cache.path())
            .output()
            .expect("the binary runs")
    }

    fn index(&self, rel: &str) {
        let at = self.root().join(rel);
        let out = self.run(&at, &["index", "."]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `codeintel query --format json` run from `rel`, as a document.
    fn query(&self, rel: &str) -> serde_json::Value {
        let at = self.root().join(rel);
        let out = self.run(&at, &["query", "?- def(S, F, K, N).", "--format", "json"]);
        serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
            // stderr carries the status line and the hint; without it a parse
            // failure here says nothing about why.
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            assert!(out.stdout.is_empty(), "no JSON on stdout: {stderr}");
            serde_json::Value::Null
        })
    }
}

const SOURCE: &str = "pub fn marker() {}\n";

#[test]
fn an_index_one_level_down_answers_the_root() {
    // #61 exactly: SCIP has to run where the build is, so the index lands in
    // `daemon/`, and the server is rooted at the repository.
    let sandbox = Sandbox::new();
    sandbox.file("daemon/main.rs", SOURCE);
    sandbox.index("daemon");

    let answer = sandbox.query(".");
    assert_eq!(answer["status"], "ok", "{answer}");
    assert!(
        !answer["rows"].as_array().expect("rows").is_empty(),
        "the nearest index holds rows and they are the answer: {answer}"
    );
    // Structural, not prose: an agent learns the scope moved by branching on
    // this (`specs/05-surface.md` § The nearest index).
    assert_eq!(
        answer["root"],
        sandbox.canonical().join("daemon").display().to_string(),
        "{answer}"
    );
    let hint = answer["hint"].as_str().expect("a hint");
    assert!(hint.contains("answered from the nearest index"), "{hint}");
}

#[test]
fn an_index_at_the_root_answers_a_subdirectory() {
    let sandbox = Sandbox::new();
    sandbox.file("daemon/main.rs", SOURCE);
    sandbox.index(".");

    let answer = sandbox.query("daemon");
    assert_eq!(answer["status"], "ok", "{answer}");
    assert_eq!(
        answer["root"],
        sandbox.canonical().display().to_string(),
        "an ancestor's index covers every file below it: {answer}"
    );
}

#[test]
fn the_ascent_stops_at_the_repository_root() {
    // A checkout is a boundary. Past it is someone else's tree, and `$HOME` is
    // a few levels further (`specs/04-storage.md` § Discovery).
    let sandbox = Sandbox::new();
    sandbox.file("outer.rs", SOURCE);
    sandbox.file("inner/main.rs", SOURCE);
    sandbox.file("inner/.git", "gitdir: nowhere\n");
    sandbox.index(".");

    let answer = sandbox.query("inner");
    assert_eq!(
        answer["status"], "no-index",
        "the enclosing index is outside this checkout: {answer}"
    );
    assert!(answer["root"].is_null(), "{answer}");
    let hint = answer["hint"].as_str().expect("a hint");
    assert!(!hint.contains("found"), "{hint}");
}

#[test]
fn an_answer_from_the_root_asked_for_reports_no_redirect() {
    let sandbox = Sandbox::new();
    sandbox.file("main.rs", SOURCE);
    sandbox.index(".");

    let answer = sandbox.query(".");
    assert_eq!(answer["status"], "ok", "{answer}");
    // Present and null, never absent: an absent key reads as "not measured"
    // (`specs/05-surface.md` § Response contract).
    assert!(
        answer.get("root").is_some() && answer["root"].is_null(),
        "{answer}"
    );
}

#[test]
fn two_sibling_indexes_are_a_choice_this_tool_does_not_make() {
    let sandbox = Sandbox::new();
    sandbox.file("a/main.rs", SOURCE);
    sandbox.file("b/main.rs", SOURCE);
    sandbox.index("a");
    sandbox.index("b");

    let answer = sandbox.query(".");
    assert_eq!(answer["status"], "no-index", "{answer}");
    assert!(answer["root"].is_null(), "{answer}");
    let hint = answer["hint"].as_str().expect("a hint");
    // Both named, so the caller picks — the tool does not (invariant 1).
    assert!(hint.contains("found indexes at"), "{hint}");
    assert!(hint.contains("/a") && hint.contains("/b"), "{hint}");
}

#[test]
fn a_pinned_index_dir_turns_discovery_off() {
    let sandbox = Sandbox::new();
    sandbox.file("daemon/main.rs", SOURCE);
    sandbox.index("daemon");

    // Every candidate root resolves to this one directory, so a probe would
    // answer "yes" for all of them (`specs/04-storage.md` § Discovery).
    let pinned = sandbox.cache.path().join("pinned");
    let out = Command::new(binary())
        .args(["query", "?- def(S, F, K, N).", "--format", "json"])
        .current_dir(sandbox.root())
        .env("XDG_CACHE_HOME", sandbox.cache.path())
        .env("HOME", sandbox.cache.path())
        .env("CODEINTEL_INDEX_DIR", &pinned)
        .output()
        .expect("the binary runs");
    let answer: serde_json::Value = stdout_json(&out);
    assert_eq!(answer["status"], "no-index", "{answer}");
    assert!(answer["root"].is_null(), "{answer}");
    let hint = answer["hint"].as_str().expect("a hint");
    assert!(!hint.contains("found"), "nothing was probed for: {hint}");
}

#[test]
fn the_no_index_hint_is_one_complete_line() {
    // #59 Option B. The agent that gets this runs one line and is done:
    // `index` builds tier A and runs the indexers for tier B.
    let sandbox = Sandbox::new();
    sandbox.file("main.rs", SOURCE);

    let answer = sandbox.query(".");
    assert_eq!(answer["status"], "no-index", "{answer}");
    let hint = answer["hint"].as_str().expect("a hint");
    let root = sandbox.canonical();
    assert!(
        hint.starts_with(&format!("no index for {}", root.display())),
        "{hint}"
    );
    // `index` runs the indexer itself, so one command is the whole remedy.
    assert!(
        hint.ends_with(&format!("run: codeintel index {}", root.display())),
        "{hint}"
    );
    assert_eq!(hint.matches("codeintel index").count(), 1, "{hint}");
}

#[test]
fn a_tree_with_no_scip_indexer_is_not_told_to_run_one() {
    // Never name a command that does not exist for this tree (invariant 1).
    let sandbox = Sandbox::new();
    sandbox.file("notes.md", "# nothing a grammar covers\n");

    let answer = sandbox.query(".");
    assert_eq!(answer["status"], "no-index", "{answer}");
    let hint = answer["hint"].as_str().expect("a hint");
    assert!(!hint.contains("scip"), "{hint}");
    assert_eq!(hint.matches("codeintel index").count(), 1, "{hint}");
}

#[test]
fn status_reports_the_neighbour_but_never_answers_for_it() {
    let sandbox = Sandbox::new();
    sandbox.file("daemon/main.rs", SOURCE);
    sandbox.index("daemon");

    let out = sandbox.run(sandbox.root(), &["status", "--format", "json"]);
    assert!(out.status.success(), "{:?}", out.status);
    let json: serde_json::Value = stdout_json(&out);
    // `status` is a statement about the directory on its command line
    // (`specs/05-surface.md` § The nearest index).
    assert_eq!(json["status"], "no-index", "{json}");
    let hint = json["hint"].as_str().expect("a hint");
    assert!(hint.contains("found an index at"), "{hint}");
    assert!(hint.contains("daemon"), "{hint}");
}

#[test]
fn the_mcp_server_rooted_at_the_repository_answers_from_the_subdirectory() {
    // The whole of #61: the agent cannot pass `--path`, so the server finding
    // the index is the only place this can be fixed.
    use std::io::Write as _;
    use std::process::Stdio;

    let sandbox = Sandbox::new();
    sandbox.file("daemon/main.rs", SOURCE);
    sandbox.index("daemon");

    let mut child = Command::new(binary())
        .args(["mcp"])
        .current_dir(sandbox.root())
        .env("XDG_CACHE_HOME", sandbox.cache.path())
        .env("HOME", sandbox.cache.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the server starts");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        let init = serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18" }
        });
        let call = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "code_query", "arguments": {
                "query": "?- def(S, F, K, N)."
            }}
        });
        writeln!(stdin, "{init}").expect("write");
        writeln!(stdin, "{call}").expect("write");
    }
    let out = child.wait_with_output().expect("the server exits");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let why = format!(
        "`codeintel mcp` exited {:?} and printed {stdout:?}. stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let response: serde_json::Value =
        serde_json::from_str(stdout.lines().nth(1).expect(&why)).expect(&why);

    let structured = &response["result"]["structuredContent"];
    assert_eq!(structured["status"], "ok", "{response}");
    assert_eq!(
        structured["root"],
        sandbox.canonical().join("daemon").display().to_string(),
        "{response}"
    );
    assert!(
        !structured["rows"].as_array().expect("rows").is_empty(),
        "{response}"
    );
}
