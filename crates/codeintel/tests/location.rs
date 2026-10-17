//! Where the index lives (`specs/04-storage.md` § Location).
//!
//! The other suites pin `CODEINTEL_INDEX_DIR` inside their temp tree so they can
//! read the store back. These do not: they drive the real default.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

/// A repository with a `.gitignore` and one source file, and a private cache.
struct World {
    tree: tempfile::TempDir,
    cache: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let tree = tempfile::tempdir().expect("tempdir");
        std::fs::write(tree.path().join("a.rs"), "pub fn alpha() {}\n").expect("write");
        std::fs::write(tree.path().join(".gitignore"), "/target\n").expect("write");
        Self {
            tree,
            cache: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(binary());
        command
            .current_dir(self.tree.path())
            .env("XDG_CACHE_HOME", self.cache.path())
            .env("HOME", self.cache.path())
            .env_remove("CODEINTEL_INDEX_DIR");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("the binary runs")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn gitignore(&self) -> String {
        std::fs::read_to_string(self.tree.path().join(".gitignore")).expect("read")
    }

    /// Every entry directly under the cache's `codeintel/`.
    fn cached(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.cache.path().join("codeintel")) else {
            return Vec::new();
        };
        entries.map(|e| e.expect("entry").path()).collect()
    }

    fn status_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.ok(&["status", "--format", "json"])).expect("JSON")
    }
}

#[test]
fn the_default_index_is_in_the_cache_and_the_tree_is_untouched() {
    let world = World::new();
    world.ok(&["index", "."]);

    assert!(!world.tree.path().join(".codeintel").exists());
    assert_eq!(
        world.gitignore(),
        "/target\n",
        ".gitignore must not be touched"
    );
    let stderr = String::from_utf8_lossy(&world.run(&["index", "."]).stderr).to_string();
    assert!(!stderr.contains("gitignore"), "{stderr}");

    let cached = world.cached();
    assert_eq!(cached.len(), 1, "{cached:?}");
    let key = cached[0].file_name().expect("name").to_string_lossy();
    assert_eq!(key.len(), 16, "{key}");
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()), "{key}");
    assert!(cached[0].join("manifest.json").exists());

    let reported = world.status_json()["index_dir"]
        .as_str()
        .expect("index_dir")
        .to_string();
    assert_eq!(
        PathBuf::from(reported).canonicalize().expect("exists"),
        cached[0].canonicalize().expect("exists")
    );
    assert!(
        world
            .ok(&["query", "?- def(S, B, C, D)."])
            .contains("alpha")
    );
}

#[test]
fn two_roots_get_two_indexes() {
    let world = World::new();
    world.ok(&["index", "."]);
    let other = tempfile::tempdir().expect("tempdir");
    std::fs::write(other.path().join("b.rs"), "pub fn beta() {}\n").expect("write");
    let out = world
        .command()
        .args(["index", "."])
        .current_dir(other.path())
        .output()
        .expect("runs");
    assert!(out.status.success());
    assert_eq!(world.cached().len(), 2);
}

#[test]
fn querying_without_an_index_creates_nothing() {
    let world = World::new();
    let status = world.ok(&["status"]);
    assert!(status.contains("no-index"), "{status}");
    let out = world.run(&["query", "?- def(_, _, _, _)."]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("no-index"));
    assert!(world.cached().is_empty());
    assert!(!world.tree.path().join(".codeintel").exists());
}

#[test]
fn the_flag_beats_the_env_var_beats_the_default() {
    let world = World::new();
    let by_env = world.cache.path().join("by-env");
    let by_flag = world.cache.path().join("by-flag");

    let out = world
        .command()
        .env("CODEINTEL_INDEX_DIR", &by_env)
        .args(["index", ".", "--index-dir"])
        .arg(&by_flag)
        .output()
        .expect("runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(by_flag.join("manifest.json").exists());
    assert!(!by_env.exists(), "the flag wins");

    let out = world
        .command()
        .env("CODEINTEL_INDEX_DIR", &by_env)
        .args(["index", "."])
        .output()
        .expect("runs");
    assert!(out.status.success());
    assert!(by_env.join("manifest.json").exists());
    assert!(world.cached().is_empty(), "the default is not used");
    assert!(!world.tree.path().join(".codeintel").exists());
}

#[test]
fn a_relative_override_is_relative_to_the_working_directory() {
    let world = World::new();
    world.ok(&["index", ".", "--index-dir", "elsewhere/idx"]);
    assert!(
        world
            .tree
            .path()
            .join("elsewhere/idx/manifest.json")
            .exists()
    );
}

#[test]
fn an_existing_in_tree_index_keeps_being_used_and_nothing_else_is_written() {
    let world = World::new();
    let legacy = world.tree.path().join(".codeintel");
    world.ok(&["index", ".", "--index-dir", ".codeintel"]);
    assert!(legacy.join("manifest.json").exists());

    // No flag, no env: the legacy directory is found, not shadowed by a fresh
    // index in the cache.
    std::fs::write(world.tree.path().join("b.rs"), "pub fn beta() {}\n").expect("write");
    world.ok(&["index", "."]);
    assert!(world.cached().is_empty(), "{:?}", world.cached());
    let reported = world.status_json()["index_dir"]
        .as_str()
        .expect("index_dir")
        .to_string();
    assert_eq!(
        PathBuf::from(reported).canonicalize().expect("exists"),
        legacy.canonicalize().expect("exists")
    );
    assert!(world.ok(&["query", "?- def(S, B, C, D)."]).contains("beta"));
    assert_eq!(world.gitignore(), "/target\n");
}

#[test]
fn an_explicit_override_beats_an_existing_in_tree_index() {
    let world = World::new();
    world.ok(&["index", ".", "--index-dir", ".codeintel"]);
    let target = world.cache.path().join("explicit");
    world.ok(&["index", ".", "--index-dir", target.to_str().expect("utf8")]);
    assert!(target.join("manifest.json").exists());
}

#[test]
fn an_index_builds_in_a_read_only_tree() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    let tree = world.tree.path();
    std::fs::set_permissions(tree, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    let out = world.run(&["index", "."]);
    std::fs::set_permissions(tree, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(world.cached().len(), 1);
}

#[test]
fn no_cache_location_is_an_error_that_names_the_override() {
    let world = World::new();
    let out = world
        .command()
        .env_remove("XDG_CACHE_HOME")
        .env_remove("HOME")
        .args(["index", "."])
        .output()
        .expect("runs");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--index-dir"), "{stderr}");
    assert!(!world.tree.path().join(".codeintel").exists());
}

#[test]
fn mcp_honours_the_override() {
    use std::io::Write as _;
    let world = World::new();
    let dir = world.cache.path().join("mcp-idx");
    world.ok(&["index", ".", "--index-dir", dir.to_str().expect("utf8")]);

    let mut child = world
        .command()
        .args(["mcp", "--index-dir"])
        .arg(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("starts");
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "code_query", "arguments": {"query": "?- def(_, _, _, _).", "format": "json"}}
    });
    writeln!(child.stdin.as_mut().expect("stdin"), "{request}").expect("write");
    let out = child.wait_with_output().expect("exits");
    let line = out.stdout.split(|b| *b == b'\n').next().expect("a line");
    let response: serde_json::Value = serde_json::from_slice(line).expect("JSON-RPC");
    assert_eq!(
        response["result"]["structuredContent"]["status"], "ok",
        "{response}"
    );
    assert!(world.cached().is_empty());
}

#[test]
fn the_walk_does_not_index_an_index_kept_inside_the_tree() {
    let world = World::new();
    world.ok(&["index", ".", "--index-dir", ".codeintel"]);
    assert_eq!(world.status_json()["files"], 1);
}
