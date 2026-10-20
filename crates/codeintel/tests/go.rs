//! `codeintel` end to end over a Go tree.
//!
//! `docs/plan.md` M5's done-when for a language, in the part that needs the
//! whole binary rather than the extractor: its `imports.scm` produces usable
//! `import/3` rows, a conformance rule over them passes and fails correctly,
//! and every definition anchors against its SCIP index except the ones that
//! structurally cannot, which are named (`specs/02-extraction.md`
//! § Validation — a set rather than a rate, #79).
//!
//! The fixture carries a real `index.scip` produced by `scip-go`, committed so
//! that these run without the indexer installed. Regenerate it by running
//! `scip-go` inside `tests/fixtures/go/`. The `tier_a` tests below delete it
//! first: conformance is the capability that works on a first run with no SCIP
//! index, so it is the one that has to work the day a language lands.

use std::path::{Path, PathBuf};
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

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/go")
}

/// A throwaway copy of the Go fixture, `index.scip` included.
///
/// On a pinned clock, as `scip.rs`'s `tree` explains: `index` trusts the SCIP
/// inputs to have seen a file only when its mtime is not newer than theirs,
/// and a checkout's mtimes are whatever the checkout wrote. macOS copies carry
/// them over, so a fresh worktree read the fixture as `scip-stale` (#50).
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
    // The golden file is not source; the fixture is a tree to index.
    drop(std::fs::remove_file(dir.path().join("expected.facts")));
    backdate(dir.path(), SOURCE_MTIME);
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);
    dir
}

/// Seconds since the epoch every copied source is stamped with.
const SOURCE_MTIME: u64 = 1_700_000_000;

/// Stamp every file in the tree, recursively, with `secs`.
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
    let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("opens for set_times");
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("sets mtime");
}

/// The same tree with no SCIP index, so tier A answers alone.
fn tier_a_tree() -> tempfile::TempDir {
    let dir = tree();
    std::fs::remove_file(dir.path().join("index.scip")).expect("the fixture has one");
    dir
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        if name == ".codeintel" {
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

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(root)
        .env("CODEINTEL_INDEX_DIR", root.join(".codeintel"))
        .output()
        .expect("the binary runs")
}

fn index(root: &Path) -> String {
    let out = run(root, &["index", "."]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn query(root: &Path, program: &str) -> (Vec<String>, String) {
    let out = run(root, &["query", program]);
    assert!(
        matches!(out.status.code(), Some(code) if code != 2),
        "`query {program}` exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Definitions with no `resolved` row, as `file:Name`, sorted.
///
/// The anchor gate asserts on this set rather than on the rate `index`
/// prints. At fixture size a rate is arithmetic about the fixture: one miss
/// in 33 definitions is three points, so the gate sat at zero headroom and
/// failed on the next definition added whatever the join had done (#79).
fn unanchored(root: &Path) -> Vec<String> {
    let (rows, stderr) = query(root, "?- def(S, F, _, N), !resolved(S).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    // `?- def(S, F, _, N)` projects three columns, not four: a wildcard is not
    // a column. S, then F, then N.
    let mut out: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            let columns: Vec<&str> = r.split('\t').collect();
            Some(format!("{}:{}", columns.get(1)?, columns.get(2)?))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_go_tree_indexes_and_names_its_indexer() {
    let dir = tier_a_tree();
    let summary = index(dir.path());
    assert!(summary.contains("7 indexed"), "{summary}");
    // Not "no SCIP index found" — the literal command, because the gap between
    // tier A and tier B is the difference between a symbol map and a call
    // graph (`specs/02-extraction.md` § Acquisition).
    assert!(summary.contains("scip-go"), "{summary}");
    // `go.mod` has a registered-looking extension and no grammar. It is
    // skipped, and the skip is reported rather than silently swallowed.
    assert!(summary.contains("skipped"), "{summary}");
}

/// `status` names the indexer that would reconcile a `scip-stale` Go file.
/// It printed `rust-analyzer scip .` for every language.
#[test]
fn a_scip_stale_go_file_names_scip_go_in_status() {
    let dir = tree();
    index(dir.path());
    let path = dir.path().join("kinds.go");
    let mut src = std::fs::read_to_string(&path).expect("source");
    src.push_str("\n// edited after the SCIP index was built\n");
    std::fs::write(&path, src).expect("writes");
    index(dir.path());

    let report = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert!(report.contains("scip stale:"), "{report}");
    assert!(report.contains("kinds.go"), "{report}");
    assert!(report.contains("run: scip-go"), "{report}");
    assert!(!report.contains("rust-analyzer"), "{report}");
}

#[test]
fn an_import_is_the_specifier_without_its_quotes() {
    // The import path literal's *content*, never resolved to a path
    // (`specs/01-facts.md` § `import`) and never carrying Go's quotes — the
    // spelling every other language uses, so `?- import(F, "fmt", _).` reads
    // the same here (#70). The alias column carries Go's `_` verbatim:
    // "imported only for its side effects" is a thing a conformance rule wants
    // to ask about.
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- import("ui/panel.go", M, A)."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    // The bound file is a constant, so only `M` and `A` are columns.
    assert_eq!(
        rows,
        ["example.com/fixture/db\t", "example.com/fixture/net\t_"]
    );
}

#[test]
fn a_conformance_rule_fires_and_then_stops_firing() {
    // The headline capability, on a tree with no SCIP index at all: no module
    // under ui/ may import from db/. It has to find the violation, and it has
    // to return zero rows with `status: ok` — not an error, not a silence —
    // once the violation is gone.
    let dir = tier_a_tree();
    index(dir.path());

    let rules = dir.path().join("conformance.dl");
    std::fs::write(
        &rules,
        "%% ui_imports_db(F, M)  no package under ui/ may import from db/\n\
         ui_imports_db(F, M) :- import(F, M, _), prefix(F, \"ui/\"), contains(M, \"/db\").\n",
    )
    .expect("write");
    let rules = rules.to_string_lossy().to_string();
    let args = [
        "query",
        "?- ui_imports_db(F, M).",
        "--rules",
        &rules,
        "--expect-empty",
    ];

    let violating = run(dir.path(), &args);
    assert_eq!(
        violating.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&violating.stderr)
    );
    assert!(
        String::from_utf8_lossy(&violating.stdout).contains("ui/panel.go"),
        "the violation names the file"
    );

    // Remove the offending import and the same check passes. The `net` import
    // stays, so this is the rule getting narrower rather than the file getting
    // emptier.
    let panel = dir.path().join("ui/panel.go");
    let source = std::fs::read_to_string(&panel).expect("read");
    let cleaned: Vec<&str> = source
        .lines()
        .filter(|l| !l.contains("fixture/db") && !l.contains("db.Open"))
        .collect();
    std::fs::write(&panel, cleaned.join("\n")).expect("write");

    let clean = run(dir.path(), &args);
    let stderr = String::from_utf8_lossy(&clean.stderr);
    assert_eq!(clean.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

/// #111: `entrypoint_exact` never consults `scip_impl`. With scip-go's rows
/// present the implementing method still lists, and the dependency closure the
/// response reports does not name the relation, so no indexer can fix it.
#[test]
fn entrypoint_exact_ignores_scip_impl_even_when_the_indexer_fills_it() {
    let dir = tree();
    index(dir.path());
    let (impls, stderr) = query(dir.path(), "?- scip_impl(S, T).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(!impls.is_empty(), "scip-go writes is_implementation rows");

    let (listed, _) = query(
        dir.path(),
        r"?- entrypoint_exact(S), scip_impl(S, T), def(S, F, _, N).",
    );
    assert!(
        !listed.is_empty(),
        "an implementing method lists as uncalled"
    );

    let out = run(
        dir.path(),
        &["query", "?- entrypoint_exact(S).", "--format", "json"],
    );
    let body: serde_json::Value = stdout_json(&out);
    let depends: Vec<&str> = body["stats"]["depends"]
        .as_array()
        .expect("depends")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(
        !depends.contains(&"scip_impl") && !depends.contains(&"implements"),
        "{depends:?}"
    );
}

#[test]
fn a_method_is_reachable_through_its_type() {
    // `within/2` recurses through `parent`, so a method parented to its file
    // would answer this query with itself and stop. The Go case for it is
    // `docs/plan.md` M5's last done-when line.
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(T, _, "struct", "Store"), within(S, T), def(S, _, K, N)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.split('\t').nth(3))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(names, ["Get", "Put", "entries", "evict"]);
}

#[test]
fn every_language_in_one_tree_is_one_index() {
    // The polyglot half of `docs/plan.md` M5's done-when: Rust, Go, Python
    // and TypeScript (with TSX) in one tree. Adding a language extended the
    // fixture; it did not need a different assertion.
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), &dir.path().join("svc"));
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust"),
        &dir.path().join("cli"),
    );
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/python"),
        &dir.path().join("py"),
    );
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/typescript"),
        &dir.path().join("ts"),
    );
    for stale in [
        "svc/expected.facts",
        "svc/index.scip",
        "cli/expected.facts",
        "py/expected.facts",
        "py/index.scip",
        "ts/expected.facts",
        "ts/index.scip",
    ] {
        drop(std::fs::remove_file(dir.path().join(stale)));
    }
    // `cli/` carries a `rust-analyzer` index built at the repository root, and
    // its documents name `src/…`, not `cli/src/…`. Tier A alone is the subject
    // here: that one walk, one dictionary and one store hold five grammars.
    drop(std::fs::remove_file(dir.path().join("cli/index.scip")));

    let summary = index(dir.path());
    // Every indexer command, because every language is present and no gap
    // should need looking for (`specs/02-extraction.md` § Acquisition).
    assert!(summary.contains("scip-go"), "{summary}");
    assert!(summary.contains("rust-analyzer scip ."), "{summary}");
    assert!(summary.contains("scip-python"), "{summary}");
    assert!(summary.contains("scip-typescript"), "{summary}");

    let (rows, stderr) = query(dir.path(), "?- file(F, Lang).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    let langs: std::collections::BTreeSet<&str> =
        rows.iter().filter_map(|r| r.split('\t').nth(1)).collect();
    assert_eq!(
        langs.into_iter().collect::<Vec<_>>(),
        ["go", "python", "rust", "typescript", "typescriptreact"]
    );

    // A question asked once, answered across every grammar: each definition
    // named `Open`/`open`, whatever language declared it.
    let (opens, stderr) = query(dir.path(), r#"?- def(S, F, _, N), match(N, "^[Oo]pen$")."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    let files: std::collections::BTreeSet<&str> =
        opens.iter().filter_map(|r| r.split('\t').nth(1)).collect();
    assert_eq!(
        files.into_iter().collect::<Vec<_>>(),
        [
            "cli/src/db/conn.rs",
            "cli/src/net/conn.rs",
            "py/db/conn.py",
            "py/net/conn.py",
            "svc/db/conn.go",
            "svc/net/conn.go",
            "ts/db/conn.ts",
            "ts/net/conn.ts",
        ]
    );
}

#[test]
fn only_the_definitions_that_cannot_anchor_fail_to() {
    // `docs/plan.md` M5 states this gate as a rate, >= 95%, which is the right
    // shape for a real tree and the wrong one here. At fixture size one miss
    // is several points, so the gate sat at zero headroom and the next
    // definition anyone added failed it whatever the anchor join had done
    // (#79). The failure then pointed at regenerating a committed SCIP index,
    // which needs an indexer installed to add a fixture — backwards.
    //
    // So the set is named instead. Every definition below cannot anchor for a
    // stated reason that is not the join's doing; every other definition must.
    // `index` still computes and prints the rate — that is M5's metric for a
    // real tree and `specs/05-surface.md` pins its reporting — this test just
    // stops asserting a threshold a corpus this size cannot support.
    let dir = tree();
    let summary = index(dir.path());
    assert!(summary.contains("scip-go"), "{summary}");
    // Reported, never inferred: the rate still has to reach the summary.
    assert!(summary.contains("anchored: "), "{summary}");
    assert_eq!(
        unanchored(dir.path()),
        [
            // Appended to `kinds.go` after `index.scip` was built (#43), so
            // the committed index has no occurrence for it. Nothing about the
            // join: the indexer never saw this definition.
            "kinds.go:Drain",
        ],
        "a definition stopped anchoring, or one was added that the committed \
         index.scip predates. Added one: list it here with the reason. Did \
         not: the anchor join has drifted, and regenerating index.scip to make \
         this pass would bury it (#79)."
    );
}

#[test]
fn a_document_outside_the_repository_is_skipped_and_named() {
    // `scip-go` emits the generated test-main for a package with tests out of
    // the Go build cache, so the document arrives as
    // `../../../../../../Library/Caches/go-build/…`. Ingesting it put a `file`
    // row outside the tree and gave `calls` an edge whose caller was a path no
    // reader could open. Skipped, and **named** — invariant 5.
    let dir = tree();
    let summary = index(dir.path());
    assert!(summary.contains("scip skipped 1 document(s)"), "{summary}");
    assert!(
        summary.contains("path escapes the repository root"),
        "{summary}"
    );
    let (rows, stderr) = query(dir.path(), r#"?- file(F, _), contains(F, "..")."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(rows.is_empty(), "no file row leaves the tree: {rows:?}");
}

#[test]
fn the_compiler_answers_what_tier_a_would_not_guess() {
    // `Open` is exported by both `db` and `net`, so tier A's `!ambiguous(N)`
    // guard refuses the edge. Two callers, one correct target each, and the
    // provenance says which tier answered.
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), "?- calls(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(
        rows,
        [
            "Draw ui/panel.go:11\tOpen db/conn.go:4",
            "Start app/app.go:8\tOpen db/conn.go:4",
            "TestWarm store/store_test.go:6\tWarm store/store.go:48",
            "Warm store/store.go:48\tGet store/store.go:24",
        ]
    );
    let (named, _) = query(dir.path(), r"?- calls_exact(A, B).");
    assert_eq!(named.len(), rows.len(), "every Go edge here is exact");
}

#[test]
fn tier_b_owns_a_method_by_its_package_qualified_type() {
    // Tier A resolved the receiver within one file; the compiler knows the
    // package. Both must land on the type, never on the file
    // (`specs/02-extraction.md` § Parent precedence).
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, "store/store.go", "method", N), parent(S, P), def(P, _, PK, PN)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    let owners: Vec<(&str, &str)> = rows
        .iter()
        // Columns are the free variables in order: S, N, P, PK, PN.
        .filter_map(|r| {
            let c: Vec<&str> = r.split('\t').collect();
            Some((*c.get(1)?, *c.get(4)?))
        })
        .collect();
    assert_eq!(
        owners,
        [
            ("Describe", "Entry"),
            ("Get", "Store"),
            ("Put", "Store"),
            ("evict", "Store"),
        ]
    );
}

/// A `--lang` run re-extracts one language. Recording the new extractor
/// fingerprint would make the next full run trust the other language's
/// segments, built by the old extractor (`specs/04-storage.md` § Manifest).
#[test]
fn a_lang_restricted_refresh_does_not_record_the_extractor_fingerprint() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), &dir.path().join("svc"));
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust"),
        &dir.path().join("cli"),
    );
    for stale in [
        "svc/expected.facts",
        "svc/index.scip",
        "cli/expected.facts",
        "cli/index.scip",
    ] {
        drop(std::fs::remove_file(dir.path().join(stale)));
    }
    index(dir.path());
    let manifest = dir.path().join(".codeintel/manifest.json");
    let text = std::fs::read_to_string(&manifest).expect("manifest");
    let current =
        serde_json::from_str::<serde_json::Value>(&text).expect("json")["extractor_fingerprint"]
            .as_str()
            .expect("recorded")
            .to_string();

    // Pretend the index was built by an older extractor.
    std::fs::write(&manifest, text.replace(&current, "blake3:older-extractor")).expect("writes");

    let out = run(dir.path(), &["index", ".", "--lang", "go"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = std::fs::read_to_string(&manifest).expect("manifest");
    assert!(
        after.contains("blake3:older-extractor"),
        "a partial run recorded the fingerprint"
    );

    // The full run notices and re-extracts what the partial one skipped.
    let summary = index(dir.path());
    assert!(!summary.contains("0 indexed"), "{summary}");
    let after = std::fs::read_to_string(&manifest).expect("manifest");
    assert!(after.contains(&current), "{after}");
}

#[test]
fn a_lang_restricted_refresh_carries_the_other_language_forward() {
    // `--lang go` does not walk the Rust files, so they are never seen. They
    // are not vanished either: an unwalked language is carried forward
    // unchanged, not dropped (`specs/05-surface.md` § `index`). The manifest
    // is read directly because `query` refreshes with every language first.
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), &dir.path().join("svc"));
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust"),
        &dir.path().join("cli"),
    );
    for stale in [
        "svc/expected.facts",
        "svc/index.scip",
        "cli/expected.facts",
        "cli/index.scip",
    ] {
        drop(std::fs::remove_file(dir.path().join(stale)));
    }
    let indexed = |root: &Path| -> Vec<String> {
        let store = facts::Store::open(root, root.join(".codeintel"), "").expect("opens");
        store.manifest().files.keys().cloned().collect()
    };
    index(dir.path());
    let before = indexed(dir.path());
    assert!(
        before.iter().any(|f| f == "cli/src/ui/panel.rs"),
        "{before:?}"
    );

    let out = run(dir.path(), &["index", ".", "--lang", "go"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(summary.contains("0 removed"), "{summary}");
    assert_eq!(
        indexed(dir.path()),
        before,
        "the Rust files survived a Go-only refresh"
    );

    // The restriction still applies to what it names: a Go file that vanishes
    // is dropped, a Rust file that vanishes is not noticed until it is walked.
    std::fs::remove_file(dir.path().join("svc/ui/panel.go")).expect("removes");
    std::fs::remove_file(dir.path().join("cli/src/ui/panel.rs")).expect("removes");
    let out = run(dir.path(), &["index", ".", "--lang", "go"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let files = indexed(dir.path());
    assert!(!files.iter().any(|f| f == "svc/ui/panel.go"), "{files:?}");
    assert!(
        files.iter().any(|f| f == "cli/src/ui/panel.rs"),
        "{files:?}"
    );
    index(dir.path());
    let files = indexed(dir.path());
    assert!(
        !files.iter().any(|f| f == "cli/src/ui/panel.rs"),
        "{files:?}"
    );
}
