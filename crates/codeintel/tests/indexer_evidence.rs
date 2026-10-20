//! Every command string codeintel emits for an indexer it does not own has a
//! row in `specs/02-extraction.md` § Indexer evidence (#122).
//!
//! `hint_runs.rs` proves the chain *parses*; nothing proved any indexer in it
//! was ever run. #93 shipped a flag nobody had run, and #109 found scip-go
//! exiting 0 over an empty index. The honest place for "has this been run" is a
//! table, and a table is only honest while it cannot go stale — so this reads
//! it, in both directions, against `Lang.indexer`.

use std::collections::BTreeSet;

use regex::Regex;

const SPEC: &str = include_str!("../../../specs/02-extraction.md");

/// The table's data rows, split into cells. Empty when the section is absent,
/// so the tests below fail on content rather than on a panic in the reader.
fn table_rows() -> Vec<Vec<String>> {
    let Some((_, after)) = SPEC.split_once("\n### Indexer evidence") else {
        return Vec::new();
    };
    let section = after.split("\n#").next().unwrap_or_default();
    section
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            l.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect()
}

fn commands() -> BTreeSet<String> {
    table_rows()
        .into_iter()
        .filter_map(|cells| cells.first().map(|c| c.trim_matches('`').to_string()))
        .collect()
}

fn emitted() -> BTreeSet<String> {
    extract::lang::LANGS
        .iter()
        .map(|l| l.indexer.to_string())
        .collect()
}

#[test]
fn every_emitted_indexer_command_declares_its_evidence() {
    let missing: Vec<_> = emitted().difference(&commands()).cloned().collect();
    assert!(
        missing.is_empty(),
        "Lang.indexer strings with no row in § Indexer evidence: {missing:?}"
    );
}

#[test]
fn no_evidence_row_describes_a_command_nothing_emits() {
    let stale: Vec<_> = commands().difference(&emitted()).cloned().collect();
    assert!(
        stale.is_empty(),
        "evidence rows for commands no Lang emits: {stale:?}"
    );
}

#[test]
fn every_row_has_three_cells_and_a_two_word_vocabulary() {
    let version = Regex::new(r"\d+\.\d+").expect("regex");
    let date = Regex::new(r"\d{4}-\d{2}-\d{2}").expect("regex");
    for cells in table_rows() {
        assert_eq!(cells.len(), 3, "a row must have 3 cells: {cells:?}");
        let (command, class, evidence) = (&cells[0], &cells[1], &cells[2]);
        assert!(
            class == "executed" || class == "unverified",
            "{command}: class is `{class}`; the vocabulary is `executed` and `unverified`, \
             with no gradient between"
        );
        if class == "executed" {
            assert!(
                version.is_match(evidence) && date.is_match(evidence),
                "{command} is `executed` without a tool version and an ISO date: {evidence}"
            );
        }
    }
}
