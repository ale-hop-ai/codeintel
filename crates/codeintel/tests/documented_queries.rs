//! Every `?- ... .` the docs show, executed: #89.
//!
//! `docs/cookbook.md:146` is the dex-parity row for `GraphQualifiedNameAt`. It
//! left the file and line free, so `innermost_at` had nothing to seed and
//! enumerated every line of every definition: 5 s and a timeout on this
//! repository, in the one table that is the project's parity evidence. Nothing
//! ran it. `eval_commands.rs` (#87) did the same for the example's module doc;
//! this does it for the three documents that teach queries.
//!
//! The ground is this repository's own tree, not a fixture. Cost scales with
//! the index, and that row answers in 40 ms over the twelve-file rust fixture
//! (`derived` 1380, `status` `ok`), so a fixture run passes the defect.
//! `docs/00-overview.md` learned the same lesson about the `schema` budget (#42):
//! a bound measured only on the smallest input is declared, not measured.
//!
//! A query that is legitimately empty here is `ok` with zero rows, which is a
//! real answer. What the docs may not do is time out, or fail to parse or
//! resolve: those mean the documentation is wrong, not the index.
//!
//! The same corpus is the determinism corpus (#99): `CLAUDE.md` promises
//! byte-identical answers across runs, and the conformance suite's four
//! synthetic queries over hand-written facts say little about ordering over a
//! real store. Each documented query is run [`RUNS`] times in-process, then
//! the query *shapes* the docs happen not to show are run the same way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use codeintel::query::{self, Answer, Options};
use codeintel::wire;

const DOCS: &[&str] = &["docs/cookbook.md", "README.md", "specs/05-surface.md"];

/// Spans the extractor finds that are not queries, with the reason. Each must
/// still be found, so a deleted example does not leave a dead entry behind.
const NOT_RUNNABLE: &[(&str, &str)] = &[
    (
        "?- ui_imports_db(F, M).",
        "needs the repository's own conformance.dl, which is not shipped",
    ),
    (
        "?- entrypoint_exact(S), def(S, F, _, N), !is_test(F), !in_tests(S).",
        "needs the in_tests rule shown above it; `in_tests_excludes_tests_and_test_and_misses_every_other_name` runs the program",
    ),
    (
        "?- innermost_at(\"src/store.rs\", 23, S), calls(C, S), in_tests(C), def(C, F, _, N).",
        "needs the in_tests rule shown above it; `about_test_is_blind_to_in_file_tests_and_in_tests_widens_it` runs the program",
    ),
    (
        "?- NAME(A, B, Vars...)",
        "a template in the history of the removed `rules` verb",
    ),
    (
        "?- goal.\" }, \"schema\": { \"type\": \"boolean\", \"description\": \"Return the relation catalog and exit.\" }, \"limit\": { \"type\": \"integer\", \"default\": 200 }, \"raw\": { \"type\": \"boolean\", \"description\": \"SymIds instead of",
        "the prose inside the MCP tool's JSON schema",
    ),
];

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

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

/// An index of this repository, shared by every test here and every process
/// running them.
///
/// `nextest` runs each test in its own process, so a `OnceLock` shares nothing
/// between tests: the first version of this built the 129-file index once per
/// test, three times in the gate. The index lives in `CARGO_TARGET_TMPDIR`
/// instead, and every process runs `index .` against it under a file lock. The
/// first builds it, the rest are the incremental no-op (0 files indexed), and
/// because that run refreshes against the live tree the index is never stale.
/// Readers take no lock: a commit is an atomic rename, and these only query.
fn repo_index() -> &'static Path {
    static INDEX: OnceLock<PathBuf> = OnceLock::new();
    INDEX.get_or_init(|| {
        let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("documented-queries");
        std::fs::create_dir_all(&home).expect("a scratch directory");
        let lock = std::fs::File::create(home.join("lock")).expect("a lock file");
        lock.lock().expect("takes the lock");
        let index = home.join("index");
        let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
            .args(["index", "."])
            .current_dir(root())
            .env("CODEINTEL_INDEX_DIR", &index)
            .output()
            .expect("the binary runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        index
    })
}

/// `text` over `range`, which every caller derives from a `char_indices` offset.
fn tail(text: &str, range: impl std::slice::SliceIndex<str, Output = str>) -> &str {
    text.get(range).expect("a char boundary")
}

/// Every `?-` goal in `text`, whitespace-normalised, with its 1-based line.
///
/// A goal ends at the first `.` outside a string literal that is followed by
/// whitespace, a closing backtick or a closing single quote, or at a backtick.
/// That covers a fenced block, an inline code span, a table cell and a
/// `codeintel query '...'` command line alike.
fn goals(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = tail(text, from..).find("?-") {
        let start = from + at;
        let mut end = text.len();
        let mut quoted = false;
        let mut chars = tail(text, start + 2..).char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let here = start + 2 + i;
            match c {
                '"' => quoted = !quoted,
                '`' if !quoted => {
                    end = here;
                    break;
                }
                '.' if !quoted
                    && chars
                        .peek()
                        .is_none_or(|&(_, n)| n.is_whitespace() || n == '`' || n == '\'') =>
                {
                    end = here + 1;
                    break;
                }
                _ => {}
            }
        }
        let line = tail(text, ..start).matches('\n').count() + 1;
        let goal = tail(text, start..end)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        found.push((line, goal));
        from = end.max(start + 2);
    }
    found
}

/// goal -> where it appears, over every document.
fn documented() -> BTreeMap<String, Vec<String>> {
    let mut all: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for doc in DOCS {
        let text = std::fs::read_to_string(root().join(doc)).expect("a document");
        for (line, goal) in goals(&text) {
            all.entry(goal).or_default().push(format!("{doc}:{line}"));
        }
    }
    all
}

#[test]
fn the_extractor_reads_the_shapes_the_docs_use() {
    let text = "inline `?- a(X, \"x.y\").` and\n```dl\n?- b(S),\n   c(S).   % note\n```\n\
                | row | `?- d(F).` |\ncodeintel query '?- e(F).' --flag";
    let got: Vec<String> = goals(text).into_iter().map(|(_, g)| g).collect();
    assert_eq!(
        got,
        [
            "?- a(X, \"x.y\").",
            "?- b(S), c(S).",
            "?- d(F).",
            "?- e(F)."
        ]
    );
}

#[test]
fn every_documented_query_runs_against_this_repository() {
    let documented = documented();
    assert!(
        documented.len() >= 40,
        "extracted {} distinct goals; the docs changed shape or the extractor broke",
        documented.len()
    );
    for (goal, reason) in NOT_RUNNABLE {
        assert!(
            documented.contains_key(*goal),
            "NOT_RUNNABLE entry no longer matches any document ({reason}): {goal}"
        );
    }

    let index = repo_index();
    let binary = env!("CARGO_BIN_EXE_codeintel");

    let mut wrong = Vec::new();
    for (goal, places) in &documented {
        if NOT_RUNNABLE.iter().any(|(skip, _)| skip == goal) {
            continue;
        }
        let out = Command::new(binary)
            .args(["query", "--format", "json", "--no-refresh", goal])
            .current_dir(root())
            .env("CODEINTEL_INDEX_DIR", index)
            .output()
            .expect("the binary runs");
        let response: serde_json::Value = stdout_json(&out);
        let status = response["status"].as_str().unwrap_or("?");
        if matches!(status, "timeout" | "invalid-query") {
            wrong.push(format!(
                "{} — {goal}\n    status {status}: {}",
                places.join(", "),
                response["hint"].as_str().unwrap_or("")
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "documented queries that do not run:\n{}",
        wrong.join("\n")
    );
}

const IN_TESTS: &str = r#"in_tests(S) :- within(S, A), def(A, _, "module", "tests").
in_tests(S) :- within(S, A), def(A, _, "module", "test").
"#;

/// A crate with no SCIP index, written so each class of function is one row.
///
/// `entrypoint_exact` is the cookbook's example but it requires `resolved`, so
/// over any tree without an index it is empty and a comparison over it passes
/// whatever the filter does. `entrypoint` has rows here, and `in_tests` is a
/// conjunct that does not care which twin it is composed with.
const CRATE: &str = r"fn live() { helper(); }
fn helper() {}
fn dead() {}
#[cfg(test)]
mod tests {
    #[test]
    fn in_a_tests_module() {}
}
#[cfg(test)]
mod vocabulary {
    fn in_an_unguessed_module() {}
}
";

fn names(root: &Path, program: &str) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["query", "--no-refresh", program])
        .current_dir(root)
        .env("CODEINTEL_INDEX_DIR", root.join(".codeintel"))
        .output()
        .expect("the binary runs");
    // `no-scip` is the status of tier A alone, which is what this tree is for.
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("status=no-scip"),
        "{program}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    names.sort();
    names
}

/// The cookbook's `in_tests` program (#111), which the goal extractor cannot
/// run because its rule lives beside the goal rather than in it. Pins both
/// halves of the claim: it excludes `mod tests`, and it misses a module named
/// anything else, which is the ~7.5% the cookbook states.
#[test]
fn in_tests_excludes_tests_and_test_and_misses_every_other_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("lib.rs"), CRATE).expect("write");
    let index = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["index", "."])
        .current_dir(dir.path())
        .env("CODEINTEL_INDEX_DIR", dir.path().join(".codeintel"))
        .output()
        .expect("the binary runs");
    assert!(index.status.success());

    let all = names(
        dir.path(),
        "?- entrypoint(S), def(S, F, _, N), !is_test(F).",
    );
    for want in [
        "dead",
        "in_a_tests_module",
        "in_an_unguessed_module",
        "live",
    ] {
        assert!(all.iter().any(|n| n == want), "{want} must list: {all:?}");
    }
    let kept = names(
        dir.path(),
        &format!("{IN_TESTS}?- entrypoint(S), def(S, F, _, N), !is_test(F), !in_tests(S)."),
    );
    assert!(!kept.iter().any(|n| n == "in_a_tests_module"), "{kept:?}");
    assert!(
        kept.iter().any(|n| n == "in_an_unguessed_module"),
        "`vocabulary` is the documented miss; if it is now excluded the cookbook is stale: {kept:?}"
    );
    assert!(kept.iter().any(|n| n == "dead"), "{kept:?}");
}

/// #121: `about(S, "test")` is path-only, so a symbol called only from an
/// in-file `mod tests` has no test row, and the cookbook's `in_tests` finds it.
#[test]
fn about_test_is_blind_to_in_file_tests_and_in_tests_widens_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "fn covered() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { super::covered(); }\n}\n",
    )
    .expect("write");
    let index = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["index", "."])
        .current_dir(dir.path())
        .env("CODEINTEL_INDEX_DIR", dir.path().join(".codeintel"))
        .output()
        .expect("the binary runs");
    assert!(index.status.success());

    let floor = names(
        dir.path(),
        "?- def(S, _, _, \"covered\"), about(S, \"test\", F, N, L).",
    );
    assert!(floor.is_empty(), "about must be blind here: {floor:?}");
    let widened = names(
        dir.path(),
        &format!(
            "{IN_TESTS}?- def(S, _, _, \"covered\"), calls(C, S), in_tests(C), def(C, F, _, N)."
        ),
    );
    assert_eq!(widened, ["covered"], "{widened:?}");
}

// ── determinism (#99) ───────────────────────────────────────────────────────

/// Runs of each query. 100 is `CLAUDE.md`'s number and the engine's own
/// conformance test uses it over four tiny programs; over a real index it is
/// not affordable per query in a gate, so this is lower on purpose. What the
/// count buys is the chance of catching an order that depends on a per-run
/// hash seed: each run builds a fresh engine, so every `HashMap` in it draws a
/// new seed, and an unordered iteration that reaches the output differs
/// between two runs with probability well above one half once a relation holds
/// more than a handful of tuples. Ten runs of a query whose output depends on
/// one such iteration miss it with probability ~2^-9, and the corpus has
/// dozens of queries sharing the engine paths that matter. Measured on this
/// repository, one pass over the documented corpus is ~3.5 s in the test
/// profile, so 100 runs would be ~6 minutes inside a gate that takes one.
const RUNS: usize = 10;

/// The answer as both wire forms put it on the page, or the diff-able reason
/// it was not an answer.
fn rendered(root: &Path, index: &Path, program: &str, options: &Options) -> (String, Answer) {
    let options = Options {
        no_refresh: true,
        index_dir: Some(index.to_path_buf()),
        ..options.clone()
    };
    let answer = query::run(root, program, &options).expect("the query runs");
    let text = format!(
        "{}\n{}",
        wire::json(&answer, options.raw),
        wire::body(&answer, options.raw)
    );
    (text, answer)
}

fn assert_stable(
    label: &str,
    root: &Path,
    index: &Path,
    program: &str,
    options: &Options,
) -> Answer {
    let (first, answer) = rendered(root, index, program, options);
    assert!(
        !matches!(
            answer.status.to_string().as_str(),
            "timeout" | "invalid-query"
        ),
        "{label}: {program}: status {}: {:?}",
        answer.status,
        answer.hint
    );
    assert!(
        !answer.rows.is_empty() || !label.contains("shape:"),
        "{label}: {program} is empty here, so its determinism is vacuous"
    );
    for run in 1..RUNS {
        let (again, _) = rendered(root, index, program, options);
        assert_eq!(
            first, again,
            "{label}: run {run} of {program:?} differs from run 0"
        );
    }
    answer
}

/// The documented queries are split over [`CHUNKS`] tests so the gate runs them
/// side by side: one test looping over all of them was the longest in the gate
/// and set its wall time. Goal `i` belongs to chunk `i % CHUNKS`, so the chunks
/// partition the corpus and drop nothing.
const CHUNKS: usize = 4;

fn documented_stable(chunk: usize) {
    let index = repo_index();
    let root = root();
    let runnable: Vec<_> = documented()
        .into_iter()
        .filter(|(goal, _)| !NOT_RUNNABLE.iter().any(|(skip, _)| skip == goal))
        .collect();
    assert!(runnable.len() >= CHUNKS * 4, "{} goals", runnable.len());
    for (goal, places) in runnable.iter().skip(chunk).step_by(CHUNKS) {
        assert_stable(&places.join(", "), &root, index, goal, &Options::default());
    }
}

#[test]
fn every_documented_query_is_byte_identical_across_runs_chunk_0() {
    documented_stable(0);
}

#[test]
fn every_documented_query_is_byte_identical_across_runs_chunk_1() {
    documented_stable(1);
}

#[test]
fn every_documented_query_is_byte_identical_across_runs_chunk_2() {
    documented_stable(2);
}

#[test]
fn every_documented_query_is_byte_identical_across_runs_chunk_3() {
    documented_stable(3);
}

/// The shapes the engine can produce that the documented corpus does not pin,
/// and the conformance suite's four synthetic queries do not either: seeded
/// recursion through the demand rewrite, an aggregate, negation over a
/// derived relation, both truncation caps, a regex builtin, arithmetic, a
/// ground query, a raw-atom rendering.
///
/// `transformed` is asserted where the label claims the demand rewrite ran, so
/// a shape cannot quietly stop being the shape it names.
fn shapes_stable(chunk: usize) {
    let index = repo_index();
    let root = root();
    let seeded = r#"?- def(S, _, "function", "locate"), impact_of(S, C)."#;
    let shapes: &[(&str, &str, Options, fn(&Answer))] = &[
        ("seeded recursion", seeded, Options::default(), |a| {
            assert!(
                !a.transformed.is_empty(),
                "the demand rewrite did not apply"
            );
        }),
        (
            "seeded reach",
            r#"?- def(S, _, _, "run"), reach_of(S, C)."#,
            Options::default(),
            |_| {},
        ),
        (
            "unseeded recursion",
            "?- recursive(S).",
            Options::default(),
            |_| {},
        ),
        (
            "aggregate, grouped",
            "?- def(S, F, _, _), N = count{X: within(X, S)}, N > 2.",
            Options::default(),
            |_| {},
        ),
        (
            "negation",
            "?- def(S, F, \"function\", _), !calls(_, S).",
            Options::default(),
            |_| {},
        ),
        (
            "regex builtin",
            "?- def(S, F, _, N), match(N, \"^run\").",
            Options::default(),
            |_| {},
        ),
        (
            "arithmetic",
            "?- def_span(S, A, B, _, _), W = B - A, W > 40.",
            Options::default(),
            |_| {},
        ),
        (
            "ground query",
            r#"?- file("crates/facts/src/store.rs", "rust")."#,
            Options::default(),
            |_| {},
        ),
        (
            "row cap",
            "?- def(S, F, K, N).",
            Options {
                limit: 7,
                ..Options::default()
            },
            |a| assert_eq!(a.cap, Some(datalog::Cap::Rows)),
        ),
        (
            "byte cap",
            "?- def(S, F, K, N), def_sig(S, G), def_doc(S, D).",
            Options::default(),
            |a| assert_eq!(a.cap, Some(datalog::Cap::Bytes)),
        ),
        (
            "raw atoms",
            "?- def(S, F, K, N).",
            Options {
                raw: true,
                limit: 50,
                ..Options::default()
            },
            |_| {},
        ),
    ];
    assert!(shapes.len() >= CHUNKS, "{} shapes", shapes.len());
    for (label, program, options, check) in shapes.iter().skip(chunk).step_by(CHUNKS) {
        let answer = assert_stable(&format!("shape: {label}"), &root, index, program, options);
        check(&answer);
    }
}

#[test]
fn every_query_shape_is_byte_identical_across_runs_chunk_0() {
    shapes_stable(0);
}

#[test]
fn every_query_shape_is_byte_identical_across_runs_chunk_1() {
    shapes_stable(1);
}

#[test]
fn every_query_shape_is_byte_identical_across_runs_chunk_2() {
    shapes_stable(2);
}

#[test]
fn every_query_shape_is_byte_identical_across_runs_chunk_3() {
    shapes_stable(3);
}

/// Both provenance tiers in one store. This repository is indexed without
/// SCIP, so everything above is `name`; the rust fixture ships a real
/// `index.scip`, and the rules that choose between `exact` and `name` (`ref`,
/// `calls`, the `_exact` family, `tier`) are where a tier-dependent order
/// would live.
#[test]
fn answers_over_both_provenance_tiers_are_byte_identical_across_runs() {
    let fixture = root().join("tests/fixtures/rust");
    let index = tempfile::tempdir().expect("tempdir");
    let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
        .args(["index", ".", "--scip", "index.scip"])
        .current_dir(&fixture)
        .env("CODEINTEL_INDEX_DIR", index.path())
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for relation in [
        "scip_ref(S, F, L, C, From, R)",
        "name_ref(N, F, L, C, From)",
    ] {
        let tier = assert_stable(
            "tiers",
            &fixture,
            index.path(),
            &format!("?- {relation}."),
            &Options::default(),
        );
        assert!(
            !tier.rows.is_empty(),
            "the fixture index holds no {relation} rows, so it is not two-tier"
        );
    }
    for goal in [
        "?- ref(S, F, L, C, From, R, P).",
        "?- calls(A, B).",
        "?- calls_exact(A, B).",
        "?- tier(\"calls\", A, B, P).",
        "?- def(S, _, \"function\", \"put\"), impact_of(S, C).",
        "?- def(S, _, \"function\", \"put\"), impact_of_exact(S, C).",
        "?- dead_export(S).",
        "?- implements(S, T, P).",
    ] {
        assert_stable("tiers", &fixture, index.path(), goal, &Options::default());
    }
}
