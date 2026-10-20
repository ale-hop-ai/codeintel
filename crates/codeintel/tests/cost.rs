//! Byte goldens: `docs/50-self-improvement.md` § 2 (Cost), Tier 0.
//!
//! `counters.rs`'s twin. Where that pins what the engine *did* — `derived`,
//! `demand`, `transformed` — this pins what the answer *costs*: the bytes of
//! the answer as each surface actually serializes it, for every query in
//! `tests/fixtures/cost/queries.tsv`, diffed as text against `expected.tsv`.
//!
//! Bytes, not tokens. Bytes are exact and tokenizer-independent, so this is a
//! cost gate CI can run with no clock and no tolerance.
//!
//! The measurement is [`Wire::bytes`] — the same function the byte cap is
//! enforced with, which encodes through the same `wire::body` / `wire::json` /
//! `wire::tool_result` that stdout and an MCP result are built from. A byte
//! count that is not what was sent is not a cost metric
//! (`specs/05-surface.md` § `query`, the byte cap).
//!
//! A row of `queries.tsv` may carry a fourth column, `limit`: the row cap passed
//! as `--limit`. Without it the default applies, and `max_result_rows` would
//! fire long before `max_result_bytes` can, which is why the widest fixture
//! answer sat at 51 KB against a 262 KB budget and the byte cut was never
//! measured. A bigger fixture would have re-pinned every golden and
//! `counters.rs` with it; a query that is wide by construction costs one row.
//!
//! Each query is measured once per wire, because the wire is an input: the
//! byte cap is enforced against the wire's own encoding, so the surface can
//! decide which rows survive.
//!
//! Reading a diff. `rows` unchanged and `bytes` moved: a cost change — the
//! same answer got cheaper or dearer to send. `rows` moved: a correctness
//! change, and the bytes moved because the answer did. `truncated` moved: a
//! cap started or stopped firing, which is both. Say which one it was, then
//! `UPDATE_GOLDEN=1 cargo test -p codeintel --test cost`.

use std::fmt::Write as _;
use std::path::Path;

use codeintel::query::{self, Options};
use codeintel::{Row, Wire};

mod common;
use common::{diff, fixtures, indexed};

const QUERIES: &str = include_str!("../../../tests/fixtures/cost/queries.tsv");

const HEADER: &str = "id\tfixture\twire\tstatus\trows\ttruncated\tbytes\n";

/// The four surfaces an answer is sent on, with the name the golden calls each.
/// Exhaustive over [`Wire`]: a fifth surface must cost a golden column, not
/// land unmeasured.
const WIRES: [(Wire, &str); 4] = [
    (Wire::Text, "text"),
    (Wire::Json, "json"),
    (Wire::McpText, "mcp-text"),
    (Wire::McpJson, "mcp-json"),
];

#[test]
fn every_fixed_query_costs_the_bytes_the_golden_says() {
    let rust = indexed("rust");
    let go = indexed("go");

    let mut got = String::from(HEADER);
    for line in QUERIES
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let mut fields = line.splitn(4, '\t');
        let shape = "queries.tsv rows are id<TAB>fixture<TAB>query[<TAB>limit]";
        let (id, fixture, goal) = (
            fields.next().expect(shape),
            fields.next().expect(shape),
            fields.next().expect(shape),
        );
        let limit = fields
            .next()
            .map(|n| n.parse::<usize>().expect("the limit column is a row count"));
        let root = [("rust", &rust), ("go", &go)]
            .into_iter()
            .find(|(name, _)| *name == fixture)
            .map(|(_, dir)| dir.path())
            .expect("queries.tsv names a fixture this test indexes");
        for (wire, name) in WIRES {
            // Invariant 8 says the same tree and the same query produce
            // byte-identical output. A byte count that moves between two runs
            // of one index is that invariant broken, and comparing it against
            // a golden would flake rather than say so.
            let first = record(root, goal, limit, wire);
            let second = record(root, goal, limit, wire);
            assert_eq!(
                first, second,
                "query {id} on the {name} wire cost different bytes on two runs \
                 of the same index — that is non-determinism in the answer, not \
                 a golden to refresh"
            );
            writeln!(got, "{id}\t{fixture}\t{name}\t{first}").expect("writes to a String");
        }
    }

    let golden = fixtures().join("cost/expected.tsv");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &got).expect("writes the golden file");
        return;
    }
    let expected = std::fs::read_to_string(&golden).expect("the golden file exists");
    assert!(
        expected == got,
        "answer bytes changed. Rows unchanged means a cost change, rows moved \
         means a correctness change. Re-read the diff, then \
         `UPDATE_GOLDEN=1 cargo test -p codeintel --test cost`\n{}",
        diff(&expected, &got)
    );
}

/// The product of two `def` rows: 77 x 77 rows on the rust fixture, each eight
/// columns wide. Wide enough that every wire's encoding of the whole answer is
/// over `max_result_bytes`, so the cut — the one place the surface decides
/// which rows survive — fires on all four. It is a row of `queries.tsv`, so
/// the golden pins what each wire keeps.
const WIDE: &str = "?- def(A, F, K, N), def(B, G, L, M).";

/// Rows, so that the engine's `max_result_rows` is never the cap that fires.
const WIDE_LIMIT: usize = 100_000;

/// `specs/05-surface.md` § Symbol rendering: the sort runs before the byte cap,
/// "so a truncated answer is a stable prefix of the ordered rows".
///
/// The golden pins how many rows survive; this pins *which*. The oracle is
/// built without the cut: `WIDE` is re-asked one `A` at a time, each slice far
/// under the budget, and the union is sorted by the spec's key — the rendered
/// row, then the raw row. A truncated answer must be exactly the first `n` of
/// that, and `n` must be the longest prefix that fits: the next row would not.
#[test]
fn a_truncated_answer_is_a_stable_prefix_of_the_ordered_rows() {
    assert!(
        QUERIES.contains(&format!("{WIDE}\t{WIDE_LIMIT}\n")),
        "queries.tsv must pin WIDE at WIDE_LIMIT, or the golden measures a different query"
    );
    let rust = indexed("rust");
    let root = rust.path();
    let budget = datalog::Limits::default().max_result_bytes;

    let symbols = query::run(
        root,
        "?- def(A, _, _, _).",
        &Options {
            no_refresh: true,
            raw: true,
            ..Options::default()
        },
    )
    .expect("the query runs");
    let mut ordered: Vec<Row> = Vec::new();
    for row in &symbols.rows {
        let symbol = &row.raw[0];
        assert!(
            !symbol.contains(['"', '\\']),
            "{symbol} would need escaping in a literal"
        );
        let slice = query::run(
            root,
            &format!("?- A = \"{symbol}\", def(A, F, K, N), def(B, G, L, M)."),
            &Options {
                no_refresh: true,
                limit: WIDE_LIMIT,
                ..Options::default()
            },
        )
        .expect("the query runs");
        assert!(!slice.truncated, "a slice must fit, or it is not an oracle");
        ordered.extend(slice.rows);
    }
    ordered.sort_by(|a, b| a.display.cmp(&b.display).then_with(|| a.raw.cmp(&b.raw)));
    ordered.dedup_by(|a, b| a.display == b.display && a.raw == b.raw);

    let mut kept_by_wire = Vec::new();
    for (wire, name) in WIRES {
        let options = Options {
            no_refresh: true,
            wire,
            limit: WIDE_LIMIT,
            ..Options::default()
        };
        let answer = query::run(root, WIDE, &options).expect("the query runs");
        let n = answer.rows.len();
        assert!(answer.truncated, "{name}: WIDE no longer reaches the cap");
        assert_eq!(answer.cap, Some(datalog::Cap::Bytes), "{name}");
        assert!(
            0 < n && n < ordered.len(),
            "{name}: kept {n} of {} rows",
            ordered.len()
        );
        for (i, (got, want)) in answer.rows.iter().zip(&ordered).enumerate() {
            assert!(
                got.display == want.display && got.raw == want.raw,
                "{name}: row {i} is not the {i}th of the ordered rows — a truncated \
                 answer must be a stable prefix, not a selection\n got  {:?}\n want {:?}",
                got.display,
                want.display
            );
        }
        let fits = wire.bytes(&answer, options.raw);
        assert!(fits <= budget, "{name}: {fits} bytes against {budget}");
        // The longest prefix that fits: one more row would not.
        let mut longer = answer.clone();
        longer.rows.push(ordered[n].clone());
        let over = wire.bytes(&longer, options.raw);
        assert!(
            over > budget,
            "{name}: row {n} also fits ({over} <= {budget} bytes), so the cut dropped more than it had to"
        );
        kept_by_wire.push(n);
    }
    // A wire that encodes more per row keeps fewer: the cap is on the bytes
    // sent, so the surface, not the engine, chose the survivors.
    assert!(
        kept_by_wire.iter().any(|&n| n != kept_by_wire[0]),
        "every wire kept the same rows {kept_by_wire:?}, so the cap is not measuring the wire"
    );
}

/// One query's cost on one wire, tab-separated.
///
/// `rows` and `truncated` travel beside `bytes` so the diff separates the two
/// kinds of change: bytes alone is a number that cannot say whether the answer
/// got cheaper or merely smaller.
fn record(root: &Path, goal: &str, limit: Option<usize>, wire: Wire) -> String {
    let options = Options {
        no_refresh: true,
        wire,
        limit: limit.unwrap_or(Options::default().limit),
        ..Options::default()
    };
    let answer = query::run(root, goal, &options).expect("the query runs");
    format!(
        "{}\t{}\t{}\t{}",
        answer.status.as_str(),
        answer.rows.len(),
        answer.truncated,
        wire.bytes(&answer, options.raw),
    )
}
