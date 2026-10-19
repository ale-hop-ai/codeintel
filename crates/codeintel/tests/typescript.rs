//! `codeintel` end to end over a TypeScript tree, TSX included.
//!
//! `docs/plan.md` M5's done-when for a language, in the part that needs the
//! whole binary rather than the extractor: its `imports.scm` produces usable
//! `import/3` rows, a conformance rule over them passes and fails correctly,
//! and every definition anchors against its SCIP index except the ones that
//! structurally cannot, which are named (`specs/02-extraction.md`
//! § Validation — a set rather than a rate, #79).
//!
//! The fixture carries a real `index.scip` produced by `scip-typescript` 0.4.0,
//! committed so that these run without the indexer installed. Regenerate it
//! with `npx @sourcegraph/scip-typescript@0.4.0 index` inside
//! `tests/fixtures/typescript/`. The `tier_a` tests below delete it first:
//! conformance is the capability that works on a first run with no SCIP index.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/typescript")
}

/// A throwaway copy of the TypeScript fixture, `index.scip` included.
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
        if name == ".codeintel" || name == "node_modules" {
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

/// Column `n` of every row, as a sorted set.
fn column(rows: &[String], n: usize) -> Vec<&str> {
    rows.iter()
        .filter_map(|r| r.split('\t').nth(n))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[test]
fn a_typescript_tree_indexes_and_names_its_indexer() {
    let dir = tier_a_tree();
    let summary = index(dir.path());
    // Eight `.ts` (a `.d.ts` among them) and two `.tsx`.
    assert!(summary.contains("10 indexed"), "{summary}");
    // Not "no SCIP index found" — the literal command
    // (`specs/02-extraction.md` § Acquisition).
    assert!(summary.contains("scip-typescript"), "{summary}");

    let (rows, stderr) = query(dir.path(), "?- file(F, L).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&rows, 1), ["typescript", "typescriptreact"]);
}

#[test]
fn an_import_is_the_specifier_without_its_quotes() {
    // Never resolved to a path (`specs/01-facts.md` § `import`). `import * as
    // net` binds a local name for the module; `import { Row }` binds a member
    // and has no alias.
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- import("ui/panel.tsx", M, A)."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(rows, ["../db/conn\t", "../net/conn\tnet", "./row\t"]);
}

#[test]
fn a_conformance_rule_fires_and_then_stops_firing() {
    // The headline capability, on a tree with no SCIP index at all: no module
    // under ui/ may import from db/. It has to find the violation, and it has
    // to return zero rows with `status: ok` once the violation is gone.
    let dir = tier_a_tree();
    index(dir.path());

    let rules = dir.path().join("conformance.dl");
    std::fs::write(
        &rules,
        "%% ui_imports_db(F, M)  no module under ui/ may import from db/\n\
         ui_imports_db(F, M) :- import(F, M, _), prefix(F, \"ui/\"), contains(M, \"db/\").\n",
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
        String::from_utf8_lossy(&violating.stdout).contains("ui/panel.tsx"),
        "the violation names the file"
    );

    // Remove the offending import and the same check passes. The `net` import
    // stays, so this is the rule getting narrower rather than the file getting
    // emptier.
    let panel = dir.path().join("ui/panel.tsx");
    let source = std::fs::read_to_string(&panel).expect("read");
    let cleaned: Vec<&str> = source
        .lines()
        .filter(|l| !l.contains(r#"from "../db/conn""#))
        .collect();
    std::fs::write(&panel, cleaned.join("\n")).expect("write");

    let clean = run(dir.path(), &args);
    let stderr = String::from_utf8_lossy(&clean.stderr);
    assert_eq!(clean.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("status=ok"), "{stderr}");
}

#[test]
fn a_method_is_reachable_through_its_class() {
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(T, _, "class", "Store"), within(S, T), def(S, _, "method", N)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&rows, 2), ["#evict", "get", "handle", "put"]);
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
    assert!(summary.contains("scip-typescript"), "{summary}");
    // Reported, never inferred: the rate still has to reach the summary.
    assert!(summary.contains("anchored: "), "{summary}");
    assert_eq!(
        unanchored(dir.path()),
        [
            // The non-ASCII line (#21): `scip-typescript` counts that column
            // in UTF-16 and declares no position encoding, so the occurrence
            // is ambiguous and skipped. A limit of the indexer.
            "kinds.ts:AFTER_WIDE",
            // `sig.ts` is a file the committed `index.scip` has no document
            // for at all — its own file precisely so that growing an indexed
            // one does not move `calls` rows (#75). The indexer never saw
            // either definition.
            "sig.ts:clamp",
            "sig.ts:listen",
        ],
        "a definition stopped anchoring, or one was added that the committed \
         index.scip predates. Added one: list it here with the reason. Did \
         not: the anchor join has drifted, and regenerating index.scip to make \
         this pass would bury it (#79)."
    );
}

#[test]
fn the_type_checker_answers_what_tier_a_would_not_guess() {
    // `open` is exported by both `db/conn` and `net/conn`, so tier A's
    // `!ambiguous(N)` guard refuses the edge. The type checker gives each
    // caller its one target, JSX tags included.
    //
    // Six rows are ABSENT that a naive reading of `ref` would expect, all
    // with a FILE as the would-be caller. `scip-typescript` 0.4.0 records the
    // binding an import makes as an ordinary reference at module scope, with
    // an empty role bitset, as `scip-python` does; a decorator is referenced
    // at module scope, which is also where it runs; and so is the name in
    // `export default outer`. `calls_at`'s own contract is "attributed to its
    // enclosing def" — a module-scope reference has no enclosing def, so it
    // cannot be represented here regardless of what runs when (#39).
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), "?- calls(A, B).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(
        rows,
        [
            "Draw ui/panel.tsx:18\tPanel ui/panel.tsx:10",
            "Panel ui/panel.tsx:10\tRow ui/row.tsx:1",
            "Panel ui/panel.tsx:10\topen db/conn.ts:1",
            "Panel ui/panel.tsx:10\topen net/conn.ts:1",
            "describe kinds.ts:12\tdescribe kinds.ts:55",
            "handle store/store.ts:33\tget store/store.ts:20",
            "hidden kinds.ts:96\touter kinds.ts:86",
            "outer kinds.ts:86\tinner kinds.ts:88",
            "put store/store.ts:29\tconstructor store/store.ts:7",
            "start app/app.ts:3\topen db/conn.ts:1",
            "testWarm store/store.test.ts:3\twarm store/store.ts:42",
            "warm store/store.ts:42\tget store/store.ts:20",
        ]
    );
    let (exact, _) = query(dir.path(), "?- calls_exact(A, B).");
    assert_eq!(
        exact.len(),
        rows.len(),
        "every TypeScript edge here is exact"
    );

    // The absence, stated directly: no file is ever `calls`'s first argument.
    let (file_callers, _) = query(dir.path(), "?- calls(C, S), file(C, _).");
    assert_eq!(
        file_callers,
        Vec::<String>::new(),
        "a file was reported as a caller: {file_callers:?}"
    );
}

#[test]
fn implements_has_rows_because_the_indexer_states_them() {
    // `implements/3` is empty on Rust, whose indexer emits no relationships
    // (docs/plan.md M4). `scip-typescript` writes `is_implementation` for a
    // class that `implements` an interface, and for each method that satisfies
    // one, across files.
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(dir.path(), r#"?- implements(S, T, "exact")."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(
        rows,
        [
            "Config kinds.ts:33\tHandler kinds.ts:19",
            "Store store/store.ts:16\tHandler kinds.ts:19",
            "handle kinds.ts:51\thandle kinds.ts:21",
            "handle store/store.ts:33\thandle kinds.ts:21",
        ]
    );
}

#[test]
fn tier_b_owns_a_method_by_its_class() {
    let dir = tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, "store/store.ts", "method", N), parent(S, P), def(P, _, PK, PN)."#,
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
            ("#evict", "Store"),
            ("describe", "Entry"),
            ("get", "Store"),
            ("handle", "Store"),
            ("put", "Store"),
        ]
    );
}

#[test]
fn an_overload_set_is_one_symbol_and_an_accessor_pair_is_two() {
    // `specs/01-facts.md` § `def`. The compiler names two overload signatures
    // and their implementation one symbol, and tier A agrees: one `def`, a
    // span per declaration. It names a getter and its setter apart
    // (`<get>size`, `<set>size`), and once anchored they are two definitions.
    let dir = tree();
    index(dir.path());
    let (parse, stderr) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, "parse"), def_span(S, A, B, _, _)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&parse, 0), ["parse kinds.ts:76"]);
    assert_eq!(column(&parse, 1), ["76", "77", "78"]);

    let (size, _) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, "size"), def_sig(S, G)."#,
    );
    assert_eq!(
        size,
        [
            "size kinds.ts:59\tget size(): number",
            "size kinds.ts:63\tset size(value: number)",
        ]
    );
}

#[test]
fn an_undeclared_column_after_a_non_ascii_character_is_skipped_not_misplaced() {
    // `scip-typescript` 0.4.0 declares no position encoding and counts UTF-16
    // (#21). On `export const WIDE = "é"; export const AFTER_WIDE = legacy;`
    // the definition of `AFTER_WIDE` and the reference to `legacy` sit after
    // the `é`, so both are skipped and counted. `WIDE` still anchors.
    let dir = tree();
    let summary = index(dir.path());
    assert!(
        summary.contains("scip ambiguous: 2 occurrence(s) skipped"),
        "{summary}"
    );
    let (rows, _) = query(dir.path(), r#"?- def(S, "kinds.ts", _, "AFTER_WIDE")."#);
    assert_eq!(rows.len(), 1, "one definition, not two: {rows:?}");
    let (resolved, _) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, N), resolved(S), match(N, "WIDE")."#,
    );
    assert_eq!(column(&resolved, 1), ["WIDE"]);
}

#[test]
fn an_export_clause_exports_a_declaration_that_stays_restricted() {
    // `export { retries, sealed as seal }` and `export default outer` in
    // kinds.ts (#22). `visibility` is what the declaration states, so all
    // three stay `restricted`; `exported` reaches them through `name_export`
    // (`specs/01-facts.md` § `name_export`).
    let dir = tier_a_tree();
    index(dir.path());
    let (rows, stderr) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, N), visibility(S, "restricted"), exported(S)."#,
    );
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&rows, 1), ["outer", "retries", "sealed"]);
    // A restricted declaration no clause names is still not exported.
    let (rows, _) = query(
        dir.path(),
        r#"?- def(S, "kinds.ts", _, "hidden"), exported(S)."#,
    );
    assert!(rows.is_empty(), "{rows:?}");
}

/// `specs/01-facts.md` § `def_doc`, with a SCIP index present.
///
/// `scip-typescript` 0.4.0 writes no `signature_documentation` and renders the
/// hover signature into `documentation[0]` instead, for every symbol. Taking
/// that as the doc comment made `def_doc` hold for almost everything, so
/// `undocumented_export` — the whole of which is `exported(S), !def_doc(S, _)`
/// — went silent the moment the index was ingested (#55). Tier A alone had it
/// right, which is what makes the two halves of this test comparable.
#[test]
fn a_hover_signature_is_not_a_doc_comment() {
    let dir = tree();
    index(dir.path());

    // No `def_doc` is a rendered signature. Every one of these was a row.
    let (rows, stderr) = query(dir.path(), "?- def_doc(S, D).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert!(!rows.is_empty(), "the fixture is documented");
    for row in &rows {
        assert!(!row.contains("```"), "a fenced hover block survived: {row}");
    }

    // `#evict` and `describe` are undocumented methods tier A never anchored,
    // so tier B is the only thing that could have given them a `def_doc`.
    // A `SymId` renders as `Name path:line`, so a name is a prefix of it.
    let documented = column(&rows, 0);
    let names = |set: &[&str], want: &str| set.iter().any(|s| s.starts_with(want));
    assert!(
        !names(&documented, "#evict store/store.ts"),
        "{documented:?}"
    );
    assert!(
        !names(&documented, "describe store/store.ts"),
        "{documented:?}"
    );
    // A real JSDoc still arrives, un-prefixed by the fence it followed.
    let (get, _) = query(
        dir.path(),
        r#"?- def(S, "store/store.ts", "method", "get"), def_doc(S, D)."#,
    );
    assert!(get.iter().any(|r| r.contains("Read one entry.")), "{get:?}");

    // And the relation that depends on the contract answers again.
    let (undocumented, stderr) = query(dir.path(), "?- undocumented_export(S).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    let undocumented = column(&undocumented, 0);
    assert!(names(&undocumented, "Panel "), "{undocumented:?}");
    assert!(names(&undocumented, "Draw "), "{undocumented:?}");
}

/// `uses_package` names the package the INDEXER resolved to, and that is not
/// the specifier the source wrote. Regression for #58.
///
/// The reported case is a `DefinitelyTyped` stub: `import merge from 'lodash.merge'`
/// makes `import(F, "lodash.merge", "merge")` while scip-typescript resolves
/// the symbol into the stub, so `extern` says `@types/lodash.merge` and
/// `?- uses_package(F, "lodash.merge").` is zero rows — on a repository where
/// most packages are typed that way, which is most `TypeScript` repositories.
///
/// This fixture depends on nothing, and shows the same split anyway: `Map` and
/// `String` come from `lib.es5.d.ts`, which scip-typescript attributes to the
/// `typescript` package. No file imports `typescript`. So the two columns are
/// disjoint here, which is the whole shape of the bug in one assertion, and it
/// needs no `@types/` dependency committed to a fixture to say so.
///
/// The fix for #58 was documentation (`docs/cookbook.md` § 4,
/// `specs/01-facts.md` § `extern`): the relations stay as they are, because
/// `import`'s `Module` is a per-language specifier and not a package name, so
/// neither side converts into the other without the guess
/// `specs/02-extraction.md` § Name matching forbids. This test pins the
/// behaviour those documents now describe, so a later union is a failure here
/// rather than a silent change of what the `P` column means.
#[test]
fn uses_package_names_the_indexers_package_not_the_imports() {
    let dir = tree();
    index(dir.path());

    let (packages, stderr) = query(dir.path(), "?- uses_package(F, P).");
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(
        column(&packages, 1),
        ["typescript"],
        "the fixture declares no dependency; every extern is the stdlib \
         `.d.ts`, which scip-typescript attributes to `typescript`"
    );

    let (imports, _) = query(dir.path(), "?- import(F, M, _).");
    let specifiers = column(&imports, 1);
    assert!(!specifiers.is_empty(), "the fixture has imports");
    assert!(
        !specifiers.contains(&"typescript"),
        "no file imports `typescript`, yet it is the only package \
         `uses_package` reports: {specifiers:?}"
    );

    // The trap, stated as the query an agent writes. `ok` with zero rows is
    // indistinguishable from "this file does not use it" without the hint.
    let (none, stderr) = query(dir.path(), r#"?- uses_package(F, "../db/conn")."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(none, Vec::<String>::new());

    // And the recipe `docs/cookbook.md` § 4 now sends that question to.
    let (declared, stderr) = query(dir.path(), r#"?- import(F, "../db/conn", _)."#);
    assert!(stderr.contains("status=ok"), "{stderr}");
    assert_eq!(column(&declared, 0), ["app/app.ts", "ui/panel.tsx"]);
}

/// #75. `calls` must not move because the source outgrew the SCIP index.
///
/// `scip-typescript` gives every document a module symbol whose enclosing
/// range runs to one line past the last line it saw. In the fixture as
/// committed that end line cannot be placed in the source, so the range is
/// dropped and module-scope references fall through to the file — which
/// `calls_at` already refuses. Append two lines and the same range becomes
/// placeable, the module owns every module-scope reference, and because the
/// module is also a `def` the old guard let it through: `export default
/// outer` arrived as a `calls` row.
///
/// The trigger was "the file is longer than the index", an ordinary stale
/// state that says nothing about the code. The answer must be identical
/// either way, and it is the clean answer that is right: a module is a scope,
/// not a body.
#[test]
fn a_module_is_not_a_caller_however_long_the_file_is() {
    let goal = "?- calls(C, S), def(C, \"kinds.ts\", _, _).";

    let clean = tree();
    index(clean.path());
    let (before, status) = query(clean.path(), goal);
    assert!(status.contains("status=ok"), "{status}");
    assert!(!before.is_empty(), "the fixture lost its calls");

    let grown = tree();
    let kinds = grown.path().join("kinds.ts");
    let src = std::fs::read_to_string(&kinds).expect("the fixture has kinds.ts")
        + "\nexport function appended(): void {}\n";
    std::fs::write(&kinds, src).expect("writes");
    // Back onto the pinned clock, so `index` still reads the SCIP inputs as
    // having seen this file. That is the state the issue describes: a longer
    // source against an index nobody regenerated.
    set_mtime(&kinds, SOURCE_MTIME);
    index(grown.path());

    let (after, status) = query(grown.path(), goal);
    assert!(status.contains("status=ok"), "{status}");
    assert_eq!(
        after, before,
        "growing the file moved `calls`; the module symbol is a caller again"
    );
    assert!(
        !after.iter().any(|row| row.starts_with("kinds.ts\t")),
        "a module-scope reference became a call: {after:?}"
    );
}

/// The guard above can only see the module if the module is typed one. The
/// document's own symbol has a `SymbolInformation` with no `kind` in it, so
/// the kind comes from the mandatory descriptor grammar instead
/// (`specs/02-extraction.md` § A namespace descriptor is a `module`).
#[test]
fn a_documents_own_symbol_is_a_module_not_an_unknown() {
    let dir = tree();
    index(dir.path());
    let (rows, status) = query(
        dir.path(),
        "?- def(S, \"kinds.ts\", K, \"kinds.ts\"), K = \"module\".",
    );
    assert!(status.contains("status=ok"), "{status}");
    assert_eq!(
        rows.len(),
        1,
        "the document symbol is not a module: {rows:?}"
    );
}
