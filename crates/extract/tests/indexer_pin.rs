//! Every committed `index.scip` names the indexer that wrote it, and
//! `tests/fixtures/indexers.pin` says which one that was meant to be.
//!
//! The tier-B tests read committed indexes so they run without an indexer
//! installed (`.gitignore`). The price is that the gate pins one indexer's
//! output shape forever and a regenerated fixture from a different producer
//! passes everything that does not happen to look at the difference. This is
//! the one check that does, and it fails closed: a fixture with no pin, a pin
//! with no fixture and an empty fixture set are all failures, not skips.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use extract::scip::Ingest;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn parse_pins(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t'))
        .map(|(dir, tool)| (dir.trim().to_string(), tool.trim().to_string()))
        .collect()
}

/// Problems, one per line; empty means the pins hold. `found` is what the
/// indexes say, `pins` what is recorded.
fn problems(found: &BTreeMap<String, String>, pins: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    if found.is_empty() {
        out.push("no tests/fixtures/*/index.scip found: nothing was checked".to_string());
    }
    for (dir, tool) in found {
        match pins.get(dir) {
            None => out.push(format!(
                "{dir}: index.scip says `{tool}` and indexers.pin has no line for it"
            )),
            Some(want) if want != tool => out.push(format!(
                "{dir}: index.scip says `{tool}`, indexers.pin expects `{want}`"
            )),
            Some(_) => {}
        }
    }
    for dir in pins.keys().filter(|d| !found.contains_key(*d)) {
        out.push(format!(
            "{dir}: pinned, but tests/fixtures/{dir}/index.scip is missing"
        ));
    }
    out
}

#[test]
fn every_committed_index_was_written_by_the_pinned_indexer() {
    let root = fixtures();
    let pin_path = root.join("indexers.pin");
    let why = format!("read {}", pin_path.display());
    let text = std::fs::read_to_string(&pin_path).expect(&why);

    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(&root).expect("read tests/fixtures") {
        let dir = entry.expect("dir entry").path();
        let index = dir.join("index.scip");
        if !index.is_file() {
            continue;
        }
        let name = dir
            .file_name()
            .expect("fixture name")
            .to_string_lossy()
            .into_owned();
        let ingest = Ingest::read(&index, &dir, "").expect("a committed index.scip parses");
        found.insert(name, ingest.tool);
    }

    let bad = problems(&found, &parse_pins(&text));
    assert!(bad.is_empty(), "indexer pin drifted:\n{}", bad.join("\n"));
}

fn map(rows: &[(&str, &str)]) -> BTreeMap<String, String> {
    rows.iter()
        .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
        .collect()
}

#[test]
fn a_matching_pin_is_clean() {
    let m = map(&[("go", "scip-go 0.2.7")]);
    assert!(problems(&m, &m).is_empty());
}

#[test]
fn a_changed_version_fails() {
    let bad = problems(
        &map(&[("go", "scip-go 0.3.0")]),
        &map(&[("go", "scip-go 0.2.7")]),
    );
    assert_eq!(bad.len(), 1, "{bad:?}");
}

#[test]
fn a_fixture_with_no_pin_fails_rather_than_skips() {
    let bad = problems(&map(&[("go", "scip-go 0.2.7")]), &map(&[]));
    assert_eq!(bad.len(), 1, "{bad:?}");
}

#[test]
fn an_index_with_no_tool_info_fails_against_any_pin() {
    let bad = problems(&map(&[("go", "")]), &map(&[("go", "scip-go 0.2.7")]));
    assert_eq!(bad.len(), 1, "{bad:?}");
}

#[test]
fn a_pin_with_no_fixture_fails() {
    let bad = problems(
        &map(&[("go", "scip-go 0.2.7")]),
        &map(&[("go", "scip-go 0.2.7"), ("c", "x 1")]),
    );
    assert_eq!(bad.len(), 1, "{bad:?}");
}

#[test]
fn finding_nothing_fails() {
    assert_eq!(problems(&map(&[]), &map(&[])).len(), 1);
}
