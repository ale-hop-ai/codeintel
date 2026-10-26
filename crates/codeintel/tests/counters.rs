//! Cost goldens: `docs/history.md` M6 § Pulled forward, harness 1.
//!
//! Every query in `tests/fixtures/counters/queries.tsv` runs against its
//! fixture, and what the engine did — `derived`, `demand`, `transformed` — is
//! diffed as text against `expected.tsv`. Those counters are exact, so this
//! is the regression gate CI can run without flaking. Wall clock is
//! `benches/corpus.rs`, on a machine that can be trusted with it.
//!
//! Rows unchanged and `derived` moved: a cost change. Rows moved: a
//! correctness change. Either way, re-read the diff, then
//! `UPDATE_GOLDEN=1 cargo test -p codeintel --test counters`.

use std::fmt::Write as _;
use std::path::Path;

use codeintel::query::{self, Options};

mod common;
use common::{diff, fixtures, indexed};

const QUERIES: &str = include_str!("../../../tests/fixtures/counters/queries.tsv");

const HEADER: &str = "id\tfixture\tstatus\trows\ttruncated\tderived\tdemand\ttransformed\n";

#[test]
fn every_fixed_query_costs_what_the_golden_says() {
    let rust = indexed("rust");
    let go = indexed("go");

    let mut got = String::from(HEADER);
    for line in QUERIES
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let mut fields = line.splitn(3, '\t');
        let shape = "queries.tsv rows are id<TAB>fixture<TAB>query";
        let (id, fixture, goal) = (
            fields.next().expect(shape),
            fields.next().expect(shape),
            fields.next().expect(shape),
        );
        let root = [("rust", &rust), ("go", &go)]
            .into_iter()
            .find(|(name, _)| *name == fixture)
            .map(|(_, dir)| dir.path())
            .expect("queries.tsv names a fixture this test indexes");
        // A counter that varies between two runs of the same query is not a
        // golden, and comparing it would flake rather than say so.
        let first = record(root, goal);
        let second = record(root, goal);
        assert_eq!(
            first, second,
            "query {id} reported different counters on two runs of the same index"
        );
        writeln!(got, "{id}\t{fixture}\t{first}").expect("writes to a String");
    }

    let golden = fixtures().join("counters/expected.tsv");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &got).expect("writes the golden file");
        return;
    }
    let expected = std::fs::read_to_string(&golden).expect("the golden file exists");
    assert!(
        expected == got,
        "query cost changed. Rows unchanged means a cost change, rows moved means a \
         correctness change. Re-read the diff, then \
         `UPDATE_GOLDEN=1 cargo test -p codeintel --test counters`\n{}",
        diff(&expected, &got)
    );
}

/// One query's counters, tab-separated. `elapsed_ms` is left out on purpose:
/// it is the one field that is not exact.
fn record(root: &Path, goal: &str) -> String {
    let options = Options {
        no_refresh: true,
        ..Options::default()
    };
    let answer = query::run(root, goal, &options).expect("the query runs");
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        answer.status.as_str(),
        answer.rows.len(),
        answer.truncated,
        answer.derived,
        answer.demand,
        answer.transformed.join(","),
    )
}
