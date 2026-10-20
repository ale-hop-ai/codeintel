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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const DOCS: &[&str] = &["docs/cookbook.md", "README.md", "specs/05-surface.md"];

/// Spans the extractor finds that are not queries, with the reason. Each must
/// still be found, so a deleted example does not leave a dead entry behind.
const NOT_RUNNABLE: &[(&str, &str)] = &[
    (
        "?- ui_imports_db(F, M).",
        "needs the repository's own conformance.dl, which is not shipped",
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

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
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

    let index = tempfile::tempdir().expect("tempdir");
    let binary = env!("CARGO_BIN_EXE_codeintel");
    let indexed = Command::new(binary)
        .args(["index", "."])
        .current_dir(root())
        .env("CODEINTEL_INDEX_DIR", index.path())
        .output()
        .expect("the binary runs");
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let mut wrong = Vec::new();
    for (goal, places) in &documented {
        if NOT_RUNNABLE.iter().any(|(skip, _)| skip == goal) {
            continue;
        }
        let out = Command::new(binary)
            .args(["query", "--format", "json", "--no-refresh", goal])
            .current_dir(root())
            .env("CODEINTEL_INDEX_DIR", index.path())
            .output()
            .expect("the binary runs");
        let response: serde_json::Value =
            serde_json::from_slice(&out.stdout).expect("--format json prints JSON");
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
