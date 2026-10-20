//! Tier B end to end: `docs/plan.md` M3's done-when list, one test each.
//!
//! The fixture carries a real `index.scip` produced by `rust-analyzer scip .`,
//! committed so that these run without the indexer installed. Regenerate it
//! with `cargo build && rust-analyzer scip .` inside `tests/fixtures/rust/`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust")
}

/// A throwaway copy of the fixture tree, `index.scip` included.
///
/// The copy is given a *fixed* clock: every source sits at `SOURCE_MTIME` and
/// `index.scip` one minute after it — the order a real repository is in once
/// the indexer has run.
///
/// `std::fs::copy` does not carry mtimes over; it stamps every file with the
/// same `now`. Manifest mtimes are whole seconds, and `index` decides whether
/// the SCIP inputs saw a file with `candidate.mtime <= scip`. So a test that
/// edits a file within the same second as the copy is told the inputs saw the
/// new bytes, and tier B anchors against positions that have moved.
///
/// That is not hypothetical. It is why
/// `an_edited_file_is_tier_a_only_until_the_indexer_reruns` read two `def`
/// rows for one definition on CI — where the whole test finishes in 150ms —
/// and one on a developer machine slow enough to cross a second boundary.
/// Pinning the clock removes the race instead of outrunning it.
fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
    drop(std::fs::remove_file(dir.path().join("expected.facts")));
    backdate(dir.path(), SOURCE_MTIME);
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);
    dir
}

/// Seconds since the epoch every copied source is stamped with. Any fixed past
/// instant does; this one is readable as 2023-11-14.
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
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("opens for set_times");
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("sets mtime");
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        // Build output is not fixture data, and copying it is slow.
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
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    stderr
}

/// Always `--raw`: this file asserts what a symbol *is* — the SCIP string the
/// anchor join settled on — not how it prints. The rendered `Name path:line`
/// form is `tests/cli.rs`'s subject.
fn query(root: &Path, program: &str) -> (Vec<String>, String) {
    let out = run(root, &["query", program, "--raw"]);
    assert!(
        matches!(out.status.code(), Some(code) if code != 2),
        "`query {program}` exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let rows = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    (rows, String::from_utf8_lossy(&out.stderr).to_string())
}

fn rows(root: &Path, program: &str) -> Vec<String> {
    let (rows, stderr) = query(root, program);
    assert!(
        stderr.contains("status=ok")
            || stderr.contains("status=no-scip")
            || stderr.contains("status=scip-stale"),
        "{program}: {stderr}"
    );
    rows
}

#[test]
fn the_anchor_join_resolves_everything_it_structurally_can() {
    let dir = tree();
    index(dir.path());

    // A tier-A definition is one with a `def_name` — tier B emits none, because
    // SCIP's range is not our `def_span`.
    let all = rows(dir.path(), "?- def(S, _, K, _), def_name(S, _, _).");
    let unresolved = rows(
        dir.path(),
        "?- def(S, _, K, _), def_name(S, _, _), !resolved(S).",
    );

    // Modules are exempt, and the exemption is pinned so it cannot quietly
    // grow. Tier A puts module `conn` at `mod conn;` in `src/db/mod.rs`;
    // rust-analyzer puts it at line 1 of `src/db/conn.rs`. Different files, so
    // an identifier-position join structurally cannot match them — this is a
    // property of the two models, not a defect in the join.
    let modules = |rows: &[String]| rows.iter().filter(|r| r.contains("\tmodule")).count();
    assert_eq!(
        modules(&unresolved),
        unresolved.len(),
        "a non-module definition failed to anchor: {unresolved:?}"
    );

    let anchorable = all.len() - modules(&all);
    let anchored = anchorable - (unresolved.len() - modules(&unresolved));
    assert!(anchorable > 20, "the fixture got smaller: {anchorable}");
    let rate = anchored * 100 / anchorable;
    assert!(
        rate >= 95,
        "anchor rate {rate}% ({anchored}/{anchorable}); a drop means the join is drifting"
    );
}

/// rust-analyzer keys symbols by package, not cargo target, so `crate/` is
/// defined in `src/lib.rs` and again in `tests/store_test.rs`. Adopting it
/// gave one `SymId` two `def` rows in two files. The join now refuses a symbol
/// defined in more than one document, and `status` says how many.
#[test]
fn a_symbol_defined_in_two_documents_is_not_adopted() {
    let dir = tree();
    let summary = index(dir.path());
    assert!(
        summary.contains("scip collisions: 1 symbol(s)"),
        "{summary}"
    );

    let twice = rows(dir.path(), "?- def(S, F, _, _), def(S, G, _, _), F != G.");
    assert!(twice.is_empty(), "one SymId, two files: {twice:?}");
    assert!(
        rows(dir.path(), r#"?- def(S, _, _, _), suffix(S, " crate/")."#).is_empty(),
        "the colliding symbol was emitted anyway"
    );
    // Its references are facts and stay; they just join to no definition.
    assert!(
        !rows(
            dir.path(),
            r#"?- scip_ref(S, _, _, _, _, _), suffix(S, " crate/")."#
        )
        .is_empty()
    );

    let report = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert!(report.contains("scip collisions: 1"), "{report}");
}

fn index_with(root: &Path, scip: &[&str]) -> String {
    let mut args = vec!["index", "."];
    for path in scip {
        args.extend(["--scip", path]);
    }
    let out = run(root, &args);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    stderr
}

fn status_json(root: &Path) -> serde_json::Value {
    let out = run(root, &["status", "--format", "json"]);
    stdout_json(&out)
}

/// Drop a second SCIP input into the tree, stamped like `index.scip`.
fn add_input(dir: &Path, from: &Path, name: &str) {
    std::fs::copy(from, dir.join(name)).expect("copy");
    set_mtime(&dir.join(name), SOURCE_MTIME + 60);
}

/// A collision belongs to the merge, not to an input. Each input used to carry
/// the merged count and `index` and `status` summed them, so two inputs
/// reported every collision twice (#23).
#[test]
fn two_scip_inputs_count_a_collision_once() {
    let dir = tree();
    add_input(dir.path(), &dir.path().join("index.scip"), "again.scip");

    let summary = index_with(dir.path(), &["index.scip", "again.scip"]);
    assert!(
        summary.contains("scip collisions: 1 symbol(s)"),
        "{summary}"
    );
    let report = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert!(report.contains("scip collisions: 1 symbol(s)"), "{report}");

    let json = status_json(dir.path());
    assert_eq!(json["scip_collisions"], 1, "{json}");
    let inputs = json["scip"].as_array().expect("scip is an array");
    assert_eq!(inputs.len(), 2, "{json}");
    assert!(
        inputs.iter().all(|i| i.get("collisions").is_none()),
        "a per-input collision count reads as that input's own: {json}"
    );
}

/// `tool` and `documents` are each input's own. The merge stamped its document
/// total and the last-read tool on every input (#23).
#[test]
fn each_scip_input_reports_its_own_tool_and_documents() {
    let dir = tree();
    index(dir.path());
    let alone = status_json(dir.path());
    let rust = &alone["scip"][0];

    add_input(dir.path(), &fixture().join("../go/index.scip"), "go.scip");
    index_with(dir.path(), &["index.scip", "go.scip"]);
    let both = status_json(dir.path());
    let inputs = both["scip"].as_array().expect("scip is an array");
    assert_eq!(inputs.len(), 2, "{both}");
    // Sorted by path.
    let (go, rust_again) = (&inputs[0], &inputs[1]);
    assert_eq!(go["path"], "go.scip", "{both}");

    assert_eq!(rust_again["tool"], rust["tool"], "{both}");
    assert_eq!(rust_again["documents"], rust["documents"], "{both}");
    assert_ne!(go["tool"], rust["tool"], "{both}");
    assert_ne!(go["documents"], rust["documents"], "{both}");
}

#[test]
fn tier_a_references_are_bounded_by_what_tier_b_confirms() {
    let dir = tree();
    index(dir.path());

    // Tier A's false-positive rate against a SCIP-covered tree is now **zero by
    // construction**, not zero by allowlist: name matching carries
    // `!scip_ref(...)`, so it cannot resolve a name at a position tier B
    // already answered (`specs/01-facts.md` § Tier precedence).
    //
    // This used to hold a `KNOWN_IMPRECISE` allowlist with one entry —
    // `src/store.rs:24:26`, `self.entries.get(key)` inside `Store::get`, where
    // the same-file rule binds `get` to the enclosing method and SCIP knows it
    // is `HashMap::get`. That entry documented the defect as accepted
    // behaviour. The guard removes the row, so the allowlist is gone rather
    // than merely unused: an exception list nothing populates reads as evidence
    // of rigour while asserting nothing.
    let named = rows(dir.path(), r#"?- ref(_, F, L, C, _, _, "name")."#);
    assert!(
        named.is_empty(),
        "SCIP covers this fixture, so no occurrence is left for tier A to \
         resolve: {named:?}"
    );
}

#[test]
fn exact_calls_find_edges_tier_a_alone_cannot_see() {
    let dir = tree();

    // Tier A only, first. `open` is exported twice repo-wide — `db::conn` and
    // `net::conn` — so tier A's `!ambiguous(N)` guard refuses to pick one and
    // emits no edge at all. That is the extractor declining to guess, which is
    // correct, and it is exactly the hole tier B fills.
    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());
    let name_only: BTreeSet<String> = rows(dir.path(), "?- calls(A, B).").into_iter().collect();
    assert!(
        !name_only.iter().any(|e| e.contains("open()")),
        "tier A should not have resolved an ambiguous name: {name_only:?}"
    );

    // Now with SCIP.
    std::fs::copy(fixture().join("index.scip"), dir.path().join("index.scip")).expect("restores");
    // Back on the pinned clock: a copy can carry the checkout's mtime, older
    // than the sources `tree` stamped, and the inputs would then see none of
    // them (#50).
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);
    index(dir.path());
    let exact: BTreeSet<String> = rows(dir.path(), "?- calls_exact(A, B).")
        .into_iter()
        .collect();

    // Hand-audited, both directions. Three calls, and the fixture has exactly
    // three: `start` and `draw` each call `db::conn::open` (never `net`), and
    // `warm` calls `Store::get`.
    let mut audited: Vec<&str> = exact.iter().map(String::as_str).collect();
    audited.sort_unstable();
    assert_eq!(audited.len(), 3, "{audited:?}");
    for (caller, callee) in [
        ("app/start().", "db/conn/open()."),
        ("store/warm().", "impl#[Store]get()."),
        ("ui/panel/draw().", "db/conn/open()."),
    ] {
        assert!(
            exact
                .iter()
                .any(|e| e.contains(caller) && e.ends_with(callee)),
            "{caller} -> {callee} missing from {audited:?}"
        );
    }
    // The other direction: nothing calls `net::conn::open`, and tier B says so
    // rather than splitting the difference between two same-named functions.
    assert!(!exact.iter().any(|e| e.contains("net/conn/open()")));

    // Two edges tier A alone could not see at all.
    let gained = exact.len() - name_only.len();
    assert_eq!(gained, 1, "{exact:?} vs {name_only:?}");
    assert!(
        exact.iter().filter(|e| e.contains("open()")).count() == 2,
        "{exact:?}"
    );

    // `calls_exact` is a subset of `calls` by construction — the same rule with
    // `Prov` pinned — so the plan's "edges `calls` misses" is only ever true of
    // *tier-A-only* `calls`, which is how this test reads it.
    //
    // Against a full index the two are now **equal**, which is the point of
    // tier precedence (`specs/01-facts.md` § Tier precedence): name matching
    // fires only where tier B never resolved the occurrence, so on a fixture
    // SCIP covers completely there is nothing left for it to guess at. This
    // assertion used to read the other way — that `calls` carried exactly one
    // tier-A self-edge for `get` that no compiler agrees with — which pinned
    // the defect as though it were the design.
    let both: BTreeSet<String> = rows(dir.path(), "?- calls(A, B).").into_iter().collect();
    assert!(exact.is_subset(&both));
    let over_reported: Vec<&String> = both.difference(&exact).collect();
    assert!(
        over_reported.is_empty(),
        "SCIP covers this fixture, so name matching must add no edge: {over_reported:?}"
    );
}

/// Tier precedence as a property rather than a count
/// (`specs/01-facts.md` § Tier precedence): where tier B resolved an
/// occurrence, tier A does not also guess at it.
///
/// Without this the tiers made independent claims about the same byte position
/// and `ref` was their union, so a guess the compiler directly contradicts
/// survived into `calls`, `depends`, `impact_of`, `dead_export` and
/// `entrypoint`. Measured on this project's own source: 1,448 of 1,629
/// name-provenance rows sat at a position `scip_ref` covered and named a
/// different symbol.
#[test]
fn name_matching_does_not_guess_where_the_compiler_answered() {
    let dir = tree();
    index(dir.path());

    let overlapping = rows(
        dir.path(),
        r#"?- ref(S, F, L, C, From, Role, "name"), scip_ref(_, F, L, C, _, _)."#,
    );
    assert!(
        overlapping.is_empty(),
        "a name guess survived where tier B had resolved the occurrence: {overlapping:?}"
    );
}

#[test]
fn removing_the_scip_index_degrades_to_name_provenance() {
    let dir = tree();
    index(dir.path());
    assert!(!rows(dir.path(), "?- resolved(S).").is_empty());

    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());

    // Clean degradation: tier A still answers, and nothing claims to be exact.
    assert!(rows(dir.path(), "?- resolved(S).").is_empty());
    assert!(rows(dir.path(), r#"?- ref(S, F, L, C, X, R, "exact")."#).is_empty());
    assert!(!rows(dir.path(), "?- def(S, F, K, N).").is_empty());
}

#[test]
fn no_scip_fires_on_the_dependency_closure_not_the_syntax() {
    let dir = tree();
    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());

    // `impact_of` mentions no base relation at all. Its closure reaches
    // `scip_ref`, so the answer must say so — otherwise the README's own
    // headline query returns `ok` with zero rows on a fresh install, which is
    // the exact failure invariant 6 exists to prevent.
    for program in [
        "?- calls_exact(A, B).",
        "?- impact_of(S, C).",
        "?- resolved(S).",
        "?- scip_ref(S, F, L, C, X, R).",
    ] {
        let (_, stderr) = query(dir.path(), program);
        assert!(
            stderr.contains("status=no-scip"),
            "{program} did not report no-scip: {stderr}"
        );
        assert!(stderr.contains("rust-analyzer scip ."), "{stderr}");
    }

    // A query that needs nothing from tier B is plain `ok`. `no-scip` on
    // everything would be as useless as `no-scip` on nothing.
    for program in ["?- def(S, F, K, N).", "?- import(F, M, A)."] {
        let (_, stderr) = query(dir.path(), program);
        assert!(stderr.contains("status=ok"), "{program}: {stderr}");
    }
}

#[test]
fn a_truncated_answer_that_needs_scip_still_says_no_scip() {
    // `ref` carries name-grade rows without SCIP, so it both truncates and
    // needs tier B. The cap used to win and hide that half the answer's
    // provenance could never be there (#35).
    let dir = tree();
    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());
    let out = run(
        dir.path(),
        &[
            "query",
            "?- ref(S, F, L, C, X, R, P).",
            "--limit",
            "1",
            "--format",
            "json",
        ],
    );
    let body: serde_json::Value = stdout_json(&out);
    assert_eq!(body["status"], "no-scip", "{body}");
    assert_eq!(body["truncated"], true, "{body}");
    assert_eq!(body["cap"], "max_result_rows", "{body}");
    let hint = body["hint"].as_str().unwrap_or_default();
    assert!(hint.contains("max_result_rows"), "{hint}");
}

#[test]
fn a_present_scip_index_answers_ok() {
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(!rows.is_empty());
}

#[test]
fn index_prints_the_indexer_command_for_every_language_it_found() {
    let dir = tree();
    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    let stderr = index(dir.path());
    // The literal, copy-pasteable line — not a description of one.
    assert!(stderr.contains("rust-analyzer scip ."), "{stderr}");
    assert!(stderr.contains("no SCIP index"), "{stderr}");
}

#[test]
fn index_reports_what_it_ingested_and_how_well_it_joined() {
    let dir = tree();
    let stderr = index(dir.path());
    assert!(stderr.contains("scip: index.scip"), "{stderr}");
    assert!(stderr.contains("rust-analyzer"), "{stderr}");
    assert!(stderr.contains("anchored:"), "{stderr}");
}

/// `index` and `status` print the same anchor rate, because it is one
/// statistic and they used to disagree about which one it was.
///
/// `status` divided `resolved` rows by `def` rows, which counts the
/// tier-B-only definitions on both sides — each `resolved` by construction —
/// and read several points above `index`'s figure for the same store. A reader
/// who indexed and then asked `status` saw the number move with nothing having
/// changed, and had no way to tell which one the docs meant (#77).
///
/// Asserted as string equality on the whole line, not on a parsed percentage:
/// the counts have to match too, and a rounded percentage can agree while the
/// counts do not.
#[test]
fn index_and_status_agree_on_the_anchor_rate() {
    let dir = tree();
    let indexed = index(dir.path());
    let line = |text: &str| -> String {
        text.lines()
            .map(str::trim)
            .find(|l| l.starts_with("anchored: "))
            .unwrap_or_else(|| panic!("no anchor line in {text}"))
            .to_string()
    };
    let reported = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert_eq!(line(&indexed), line(&reported));

    // And the line says which definitions it counts, because "defs" alone was
    // the word both denominators claimed.
    assert!(line(&indexed).contains("tier-A defs"), "{indexed}");
}

/// The summary's definition count is **symbols**, and `def`'s row count is
/// larger whenever one symbol is declared under two kinds.
///
/// #77 said the two "agree exactly" and four fixtures agreed, because every
/// one of them declares each symbol under a single `Kind` — so the claim was
/// checked against the only inputs that cannot falsify it, which is the shape
/// `71dde10` had too (#88, #106). On deno they are 84,068 against 84,091, and
/// the 23 are real: TypeScript declaration merging, and `#[cfg]`-gated Rust
/// alternatives that are not the same kind.
///
/// Built here rather than added to `tests/fixtures/typescript`: that fixture
/// carries a committed `index.scip`, and new source in it is source the SCIP
/// input has not seen, which reads as `scip-stale` and strands the anchor
/// tests until someone can run `scip-typescript` again.
#[test]
fn the_summary_counts_symbols_and_def_counts_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    // One symbol, two kinds, twice — once the way TypeScript reaches it and
    // once the way Rust does.
    std::fs::write(
        dir.path().join("merged.ts"),
        "export interface Widget {\n  id: string;\n}\nexport class Widget {\n  constructor(public \
         id: string) {}\n}\n",
    )
    .expect("writes");
    std::fs::write(
        dir.path().join("cfg.rs"),
        "#[cfg(unix)]\npub struct Handle(i32);\n#[cfg(not(unix))]\npub type Handle = u64;\n",
    )
    .expect("writes");

    let summary = index(dir.path());
    let defs: usize = summary
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("facts: "))
        .and_then(|l| l.split(" tier-A defs").next()?.parse().ok())
        .unwrap_or_else(|| panic!("no facts line in {summary}"));

    let rows = status_json(dir.path())
        .pointer("/relations/def")
        .and_then(serde_json::Value::as_u64)
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
        .expect("status reports every relation's rows");

    // Four symbols — Widget, Handle, the constructor and the field — and six
    // rows, because Widget and Handle each carry two kinds.
    assert_eq!(defs, 4, "{summary}");
    assert_eq!(rows, 6, "{summary}");
    assert!(
        defs < rows,
        "the summary counts symbols and `def` counts rows; {defs} vs {rows}"
    );
}

#[test]
fn removing_the_scip_index_drops_the_files_only_it_covered() {
    // A file tier A will not parse — over the size cap here, an unsupported
    // language in the field — has a segment made of SCIP facts and nothing
    // else. When `index.scip` goes, so must it: the alternative is `no-scip`
    // beside rows still marked `exact`.
    //
    // Over the cap, not gitignored: an ignored path is outside the tree for
    // *both* tiers (`specs/02-extraction.md` § Budget), so it has no segment
    // to drop. Padding is appended, so every SCIP position still lands on the
    // byte it named.
    let dir = tree();
    let app = dir.path().join("src/app.rs");
    let mut src = std::fs::read_to_string(&app).expect("reads");
    src.push_str(&"\n".repeat(usize::try_from(extract::walk::MAX_BYTES).unwrap_or(0) + 1));
    std::fs::write(&app, src).expect("writes");
    set_mtime(&app, SOURCE_MTIME);
    index(dir.path());
    assert!(
        !rows(
            dir.path(),
            r#"?- ref(S, "src/app.rs", L, C, X, R, "exact")."#
        )
        .is_empty(),
        "the ignored file is covered by SCIP alone"
    );
    assert!(rows(dir.path(), r#"?- name_ref(N, "src/app.rs", L, C, X)."#).is_empty());

    std::fs::remove_file(dir.path().join("index.scip")).expect("removes");
    index(dir.path());

    assert!(rows(dir.path(), r#"?- file("src/app.rs", L)."#).is_empty());
    assert!(rows(dir.path(), r#"?- ref(S, F, L, C, X, R, "exact")."#).is_empty());
    assert!(rows(dir.path(), "?- resolved(S).").is_empty());
}

#[test]
fn a_file_edited_after_the_scip_index_reports_scip_stale() {
    let dir = tree();
    index(dir.path());
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");

    // Touch a source file so it is newer than `index.scip`.
    let path = dir.path().join("src/store.rs");
    let mut src = std::fs::read_to_string(&path).expect("source");
    src.push_str("\npub fn added() -> u32 {\n    7\n}\n");
    std::fs::write(&path, src).expect("writes");

    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=scip-stale"), "{stderr}");
    assert!(stderr.contains("src/store.rs"), "{stderr}");

    // A query that needs nothing exact is unaffected: its answer is fresh.
    let (_, stderr) = query(dir.path(), "?- def(S, F, K, N).");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

/// Staleness is by content, not by mtime: a file edited and put back holds
/// the bytes the indexer saw, and says so without the indexer rerunning.
#[test]
fn a_restored_file_is_not_scip_stale() {
    let dir = tree();
    index(dir.path());
    let path = dir.path().join("src/store.rs");
    let original = std::fs::read(&path).expect("source");

    let mut edited = original.clone();
    edited.extend_from_slice(b"\npub fn added() -> u32 {\n    7\n}\n");
    std::fs::write(&path, &edited).expect("writes");
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=scip-stale"), "{stderr}");

    std::fs::write(&path, &original).expect("restores");
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(
        stderr.contains("status=ok"),
        "the second look agrees: {stderr}"
    );

    // A touch alone — same bytes, new mtime — is not an edit either.
    std::fs::write(&path, &original).expect("touches");
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

/// An edited file is extracted with tier A alone until the indexer reruns:
/// one `def` per definition, all of them `local`, no `scip_ref` in that file,
/// and every other file untouched. Before this, tier B emitted a second `def`
/// for each symbol at the old positions.
#[test]
fn an_edited_file_is_tier_a_only_until_the_indexer_reruns() {
    let dir = tree();
    index(dir.path());
    let get = r#"?- def(S, "src/store.rs", "method", "get")."#;
    assert_eq!(rows(dir.path(), get).len(), 1);
    assert!(
        !rows(
            dir.path(),
            r#"?- def(S, "src/store.rs", "method", "get"), resolved(S)."#
        )
        .is_empty()
    );

    // Shift every position in the file.
    let path = dir.path().join("src/store.rs");
    let mut src = std::fs::read_to_string(&path).expect("source");
    src.insert_str(0, "// a line that moves everything below it\n\n");
    std::fs::write(&path, src).expect("writes");
    index(dir.path());

    let defs = rows(dir.path(), get);
    assert_eq!(defs.len(), 1, "one definition, one row: {defs:?}");
    assert!(defs[0].starts_with("local "), "tier A alone: {defs:?}");
    assert!(rows(dir.path(), r#"?- scip_ref(S, "src/store.rs", L, C, X, R)."#).is_empty());
    assert!(!rows(dir.path(), r#"?- scip_ref(S, "src/app.rs", L, C, X, R)."#).is_empty());

    let report = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert!(report.contains("scip stale:"), "{report}");
    assert!(report.contains("src/store.rs"), "{report}");
}

#[test]
fn auto_refresh_does_not_delete_tier_b() {
    let dir = tree();
    index(dir.path());
    let before = rows(dir.path(), "?- resolved(S).").len();
    assert!(before > 0);

    // Three queries in a row. Auto-refresh is a writer; if it forgot the SCIP
    // inputs it would re-extract tier-A-only and the index would lose tier B a
    // little more each time, silently.
    for _ in 0..3 {
        drop(query(dir.path(), "?- def(S, F, K, N)."));
    }
    assert_eq!(rows(dir.path(), "?- resolved(S).").len(), before);
}

#[test]
fn local_symbols_are_not_cross_linked_between_files() {
    let dir = tree();
    index(dir.path());

    // SCIP's `local N` is document-scoped. Without the rewrite every file's
    // `local 0` is one symbol, and the failure is silent and repo-wide: a
    // parameter in one file would answer questions about a parameter in
    // another.
    let locals = rows(dir.path(), r#"?- def(S, F, K, N), match(S, "^local ")."#);
    assert!(locals.len() > 5, "{locals:?}");
    let files: BTreeSet<&str> = locals
        .iter()
        .filter_map(|row| row.split('\t').nth(1))
        .collect();
    assert!(files.len() > 1, "{locals:?}");
    for row in &locals {
        let symbol = row.split('\t').next().unwrap_or_default();
        let file = row.split('\t').nth(1).unwrap_or_default();
        assert!(
            symbol.starts_with(&format!("local {file} ")),
            "a local symbol does not carry its own document: {row}"
        );
    }
}

#[test]
fn a_tier_b_definition_carries_its_semantic_owner() {
    let dir = tree();
    index(dir.path());
    // Rule 1 of parent precedence: the descriptor prefix. A field truncates to
    // the type that declares it, and that type is indexed, so the existence
    // check passes and the field is owned by the type, not by the file.
    let owners = rows(
        dir.path(),
        r#"?- def(S, _, "field", "entries"), parent(S, P)."#,
    );
    assert_eq!(owners.len(), 1, "{owners:?}");
    let owner = owners[0].split('\t').nth(1).unwrap_or_default();
    assert!(owner.ends_with("Store#"), "{owners:?}");

    // This test used to use `Store::get` and assert only that the owner
    // *string* contained `Store#`. It passed against
    // `local src/store.rs Store#` — tier A's symbol for the `impl` block, which
    // after anchoring names nothing at all. Asserting the shape of an
    // identifier without asserting that the identifier resolves is how a
    // dangling edge stays green for a milestone;
    // `every_parent_names_something_that_exists` is the assertion that catches
    // it.
}

/// Precedence rule 3's existence check, as a property over the whole index:
/// every `parent` target is a definition or a file. Nothing else is a thing
/// that can own a definition.
///
/// This guards a *silent* defect. `within/2` recurses through `parent`, so a
/// target that is neither ends the recursion early and the query returns a
/// short answer with `status=ok` — no truncation, no degradation, nothing
/// invariant 5 can report. Before the fix this found four rows: every method of
/// a Rust `impl` block in the fixture.
#[test]
fn every_parent_names_something_that_exists() {
    let dir = tree();
    index(dir.path());
    let dangling = rows(
        dir.path(),
        "?- parent(C, P), !def(P, _, _, _), !file(P, _).",
    );
    assert!(dangling.is_empty(), "parent names nothing: {dangling:?}");
}

/// The consequence, as the query an agent actually asks: a method inside an
/// `impl` block is transitively within the file that holds it.
///
/// Rust methods are the case that broke, because `rust-analyzer` names them
/// `store/impl#[Store]get().` and the descriptor prefix `store/impl#[Store]` is
/// not a definition — so rules 1 and 2 both decline and rule 3 decides.
#[test]
fn an_impl_block_method_is_within_its_file() {
    let dir = tree();
    index(dir.path());
    for method in ["get", "put", "evict"] {
        let found = rows(
            dir.path(),
            &format!(r#"?- def(S, _, "method", "{method}"), within(S, "src/store.rs")."#),
        );
        assert_eq!(found.len(), 1, "`{method}` is not within its own file");
    }
}

/// `status` reports the same SCIP staleness `query` acts on.
///
/// These disagreed: a `query` touching `scip_ref` answered `scip-stale` and
/// named the files, while `status` — the artifact with no telemetry behind it,
/// the one a bug report is supposed to carry — printed its verdict and
/// mentioned nothing. A user pasting `status` into an issue would have omitted
/// the single fact that explains the wrong answer. Both now read the same
/// `ScipState`, so they cannot drift apart again.
#[test]
fn status_reports_the_scip_staleness_that_query_acts_on() {
    let dir = tree();
    index(dir.path());

    // Edit a source file so it is newer than the committed `index.scip`, then
    // re-index so the manifest carries the new mtime.
    let touched = dir.path().join("src/app.rs");
    let mut text = std::fs::read_to_string(&touched).expect("read");
    text.push_str("\npub fn added_after_scip() {}\n");
    std::fs::write(&touched, text).expect("write");
    index(dir.path());

    let report = String::from_utf8_lossy(&run(dir.path(), &["status"]).stdout).to_string();
    assert!(report.contains("scip stale:"), "{report}");
    assert!(report.contains("src/app.rs"), "{report}");
    assert!(report.contains("rust-analyzer scip ."), "{report}");

    // And the query verb agrees, naming the same file.
    let (_, stderr) = query(dir.path(), "?- calls_exact(A, B).");
    assert!(stderr.contains("status=scip-stale"), "{stderr}");
    assert!(stderr.contains("src/app.rs"), "{stderr}");
}

/// A better resolution tier must not return fewer rows.
///
/// `exported` reaches an inherited-visibility method's visibility through its
/// owner. In a tier-A-only index a trait-impl method's parent is the
/// implementing type, because `symbols_of` gives the `impl` block the same
/// symbol as the type. Anchoring broke exactly that: the type took its SCIP
/// identity, the `impl` block kept the stale local one, the existence check on
/// precedence rule 3 then rejected it, and the parent fell through to the
/// **file** — which has no `visibility` row, so the recursion died.
///
/// Adding a SCIP index therefore *removed* answers, with `status: ok` and
/// nothing to distinguish it from a correct empty. `docs/plan.md` M4 records
/// the same shape of defect for `within/2` and calls it "a short answer no
/// status code can report".
#[test]
fn anchoring_does_not_shrink_exported() {
    let dir = tree();
    index(dir.path());
    let scip = rows(
        dir.path(),
        r#"?- def(S, "src/kinds.rs", _, "handle"), exported(S)."#,
    );

    // The same tree with no SCIP index at all.
    let bare = tree();
    std::fs::remove_file(bare.path().join("index.scip")).expect("remove index.scip");
    index(bare.path());
    let tier_a = rows(
        bare.path(),
        r#"?- def(S, "src/kinds.rs", _, "handle"), exported(S)."#,
    );

    assert_eq!(
        scip.len(),
        tier_a.len(),
        "SCIP returned {} exported `handle` methods, tier A alone returned {}. A better \
         resolution tier must not lose rows.\n  scip:  {scip:?}\n  tierA: {tier_a:?}",
        scip.len(),
        tier_a.len()
    );
    assert_eq!(scip.len(), 3, "{scip:?}");

    // And the reason: every one of those parents is a definition, not a file.
    let orphans = rows(
        dir.path(),
        r#"?- def(S, "src/kinds.rs", _, "handle"), parent(S, P), !def(P, _, _, _)."#,
    );
    assert!(
        orphans.is_empty(),
        "a trait-impl method's parent is its type, not the file: {orphans:?}"
    );
}

/// #54: `.gitignore` bounds the whole index, not just tier A.
///
/// `scip-typescript` indexes a monorepo's generated `packages/*/build/**.d.ts`
/// even though `.gitignore` excludes them, and tier B used to ingest every
/// document that merely existed on disk — so each generated declaration came
/// back as a second `def` for an export already indexed from its source. The
/// walk is the only statement about which paths belong to the tree; tier B is
/// held to it, and says which documents it refused.
#[test]
fn a_gitignored_path_is_not_re_admitted_through_tier_b() {
    let dir = tree();
    index(dir.path());
    assert!(
        !rows(dir.path(), r#"?- file("src/app.rs", L)."#).is_empty(),
        "the fixture indexes src/app.rs before it is ignored"
    );

    // A wildcard directory segment, exactly as `packages/*/build` is one.
    let ignore = dir.path().join(".gitignore");
    let mut rules = std::fs::read_to_string(&ignore).unwrap_or_default();
    rules.push_str("\n*/app.rs\n");
    std::fs::write(&ignore, rules).expect("writes");
    // `--rebuild`: the refusal is reported by the run that reads the SCIP
    // index, and an incremental run over an unchanged `index.scip` does not
    // read one. Both runs must drop the file; only this one can name why.
    let out = run(dir.path(), &["index", ".", "--rebuild"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");

    assert!(
        rows(dir.path(), r#"?- file("src/app.rs", L)."#).is_empty(),
        "an ignored path produces zero facts, not even a file row"
    );
    assert!(rows(dir.path(), r#"?- def(S, "src/app.rs", K, N)."#).is_empty());
    assert!(
        rows(dir.path(), r#"?- scip_ref(S, "src/app.rs", L, C, X, R)."#).is_empty(),
        "tier B emits nothing for a path outside the tree"
    );
    assert!(
        stderr.contains("src/app.rs: ignored"),
        "the refusal is named, never silent: {stderr}"
    );
}

/// The fixture moved into `<root>/<where>/`, with `index.scip` left at the top
/// and its document paths therefore relative to the subdirectory — exactly
/// what `rust-analyzer scip .` run from a crate in a monorepo produces (#60).
fn subtree(wheres: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for sub in wheres {
        let into = dir.path().join(sub);
        copy(&fixture(), &into);
        drop(std::fs::remove_file(into.join("expected.facts")));
        std::fs::remove_file(into.join("index.scip")).expect("the index moves to the top");
    }
    std::fs::copy(fixture().join("index.scip"), dir.path().join("index.scip")).expect("copy");
    backdate(dir.path(), SOURCE_MTIME);
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);
    dir
}

/// Invariant 5. The silent half of #60: every document missed every file, the
/// summary said `anchored: 0/2523 defs (0.0%)`, and nothing said why.
#[test]
fn a_scip_index_that_matches_nothing_says_so_and_names_the_root() {
    let dir = subtree(&["daemon"]);
    let stderr = index(dir.path());

    assert!(
        stderr.contains("0 of") && stderr.contains("document(s) matched an indexed file"),
        "the zero-match line is missing: {stderr}"
    );
    assert!(
        stderr.contains("--scip-root daemon"),
        "the suggestion is missing or wrong: {stderr}"
    );

    // And a query over a SCIP-only relation must not answer `ok` with zero
    // rows: the relations are as empty as they are with no index at all.
    let (rows, status) = query(dir.path(), "?- scip_ref(S, F, L, C, From, R).");
    assert!(rows.is_empty(), "{rows:?}");
    assert!(status.contains("status=no-scip"), "{status}");
    assert!(status.contains("0 of"), "the hint must say why: {status}");
}

/// The explicit half. `--scip-root` is a declaration, so the re-based path is
/// still a SCIP field plus something the user said — never a guess
/// (`specs/02-extraction.md` § Document paths, and `--scip-root`).
#[test]
fn scip_root_rebases_and_the_facts_land() {
    let dir = subtree(&["daemon"]);
    let out = run(dir.path(), &["index", ".", "--scip-root", "daemon"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");

    assert!(
        !stderr.contains("0 of"),
        "documents should have matched: {stderr}"
    );
    let resolved = rows(dir.path(), "?- resolved(S).");
    assert!(resolved.len() > 20, "nothing anchored: {}", resolved.len());
    let refs = rows(dir.path(), "?- scip_ref(S, F, L, C, From, R).");
    assert!(!refs.is_empty(), "no tier-B references");
    assert!(
        refs.iter().all(|row| row.contains("daemon/")),
        "a fact escaped the re-based root: {refs:?}"
    );
}

/// #123. `scip-go` declares no position encoding, so every document needs its
/// source to be transcoded; under a wrong root the source is not at the path and
/// every document is refused *before* it is counted. The count read `0 of 0`,
/// so no summary line, no `status`, and no query answer reported a degradation.
#[test]
fn a_go_index_from_a_subdirectory_reports_the_documents_it_carried() {
    let go = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/go");
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&go, &dir.path().join("svc"));
    drop(std::fs::remove_file(dir.path().join("svc/expected.facts")));
    std::fs::rename(
        dir.path().join("svc/index.scip"),
        dir.path().join("index.scip"),
    )
    .expect("the index moves to the top");
    backdate(dir.path(), SOURCE_MTIME);
    set_mtime(&dir.path().join("index.scip"), SOURCE_MTIME + 60);

    let stderr = index(dir.path());
    assert!(
        stderr.contains("0 of 8 document(s) matched an indexed file"),
        "the carried count is missing: {stderr}"
    );
    // The committed index carries one stray document under the Go build cache,
    // which no root can re-base, so detection correctly suggests nothing.
    assert!(!stderr.contains("re-run with"), "{stderr}");

    let out = run(dir.path(), &["status", "."]);
    let status = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(status.contains("0 of 8 document(s)"), "{status}");

    let out = run(
        dir.path(),
        &["query", "?- calls_exact(A, B).", "--format", "json"],
    );
    let body = stdout_json(&out);
    assert_eq!(body["status"], "no-scip", "{body}");
    assert!(
        body["hint"].as_str().is_some_and(|h| h.contains("0 of 8")),
        "{body}"
    );

    // Declared, the root lands the documents it carried.
    let out = run(dir.path(), &["index", ".", "--scip-root", "svc"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("0 of 8"), "{stderr}");
}

/// Two subtrees re-base every document equally well, so the evidence picks
/// nothing and the report must say that rather than choose
/// (`specs/02-extraction.md` § Detection suggests; it never re-bases).
#[test]
fn two_candidate_roots_suggest_neither() {
    let dir = subtree(&["daemon", "agent"]);
    let stderr = index(dir.path());

    assert!(stderr.contains("0 of"), "{stderr}");
    assert!(
        stderr.contains("2 directories would re-base all of them"),
        "ambiguity must be reported, not resolved: {stderr}"
    );
    assert!(
        !stderr.contains("re-run with"),
        "nothing may be suggested: {stderr}"
    );
}

/// A root is part of the identity of an ingest and `stat` cannot see it.
#[test]
fn changing_scip_root_reingests() {
    let dir = subtree(&["daemon"]);
    index(dir.path());
    let out = run(dir.path(), &["index", ".", "--scip-root", "daemon"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    assert!(
        !stderr.contains("0 of"),
        "the re-base was not re-ingested: {stderr}"
    );
}

/// A mis-paired root would re-base a whole index onto the wrong subtree and
/// still report a plausible anchor rate.
#[test]
fn a_mispaired_scip_root_is_an_error() {
    let dir = subtree(&["daemon"]);
    let out = run(
        dir.path(),
        &[
            "index",
            ".",
            "--scip",
            "index.scip",
            "--scip-root",
            "daemon",
            "--scip-root",
            "agent",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("--scip-root given 2"), "{stderr}");
}

/// The Rust fixture at the root and the Go fixture under `go/`, each with the
/// SCIP index its own indexer wrote, stamped after the sources (#102).
fn polyglot() -> tempfile::TempDir {
    let dir = tree();
    let go = dir.path().join("go");
    copy(&fixture().join("../go"), &go);
    drop(std::fs::remove_file(go.join("expected.facts")));
    backdate(&go, SOURCE_MTIME);
    set_mtime(&go.join("index.scip"), SOURCE_MTIME + 60);
    dir
}

/// Every indexer in the advertised chain writes `./index.scip`, so ingesting
/// one per language is the only way a polyglot tree reaches tier B for all of
/// them. A second input must add its rows, and `index` must name every file it
/// read — a silent drop of the other language is invariant 5 (#102).
#[test]
fn a_second_scip_input_for_a_second_language_adds_its_rows() {
    let dir = polyglot();
    let out = run(
        dir.path(),
        &[
            "index",
            ".",
            "--scip",
            "index.scip",
            "--scip",
            "go/index.scip",
            "--scip-root",
            ".",
            "--scip-root",
            "go",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("scip: index.scip ("), "{stderr}");
    assert!(stderr.contains("scip: go/index.scip ("), "{stderr}");

    let go = rows(
        dir.path(),
        r#"?- scip_ref(_, F, _, _, _, _), suffix(F, ".go")."#,
    );
    let rust = rows(
        dir.path(),
        r#"?- scip_ref(_, F, _, _, _, _), suffix(F, ".rs")."#,
    );
    assert!(!go.is_empty(), "no Go scip_ref rows: {stderr}");
    assert!(!rust.is_empty(), "no Rust scip_ref rows: {stderr}");
}
