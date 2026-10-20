//! Every stdlib head, both evaluators: #97.
//!
//! `CLAUDE.md`: "A semi-naive bug drops rows silently; this is the only
//! practical defence." `stdlib_engine.rs` runs the differential over a handful
//! of hand-picked queries and names 9 of the stdlib's 42 heads; twenty were
//! never evaluated under both evaluators at all, even transitively, and those
//! were the `_exact` twins and the whole-graph rules.
//!
//! The goal set is generated, not listed. Heads come from `rules/stdlib.dl`.
//! For each, over `tests/fixtures/eval/bank.dl` (built to make the `_exact`
//! twins disagree): the unseeded goal; every non-empty subset of columns bound
//! to the values they take in the first answer row, which is every adornment a
//! constant can give the head; and the head joined to itself on each ordered
//! pair of columns, which is how a literal earlier in a body binds one. The
//! rewrite builds a seed in a different place for each, and the naive twin is
//! the oracle for it.
//!
//! The shared `Solver` is not under test here: both evaluators run it, so a bug
//! in a join or a comparison is the same in both. This finds what the driver
//! and the demand rewrite do to rows. Planted-bug results are in the commit.
//!
//! A head that answers no rows over the fixture cannot be seeded, and is
//! skipped only by being named in `NO_ROWS`, with the reason. The list is
//! asserted equal to what is observed, both ways, so a skip cannot be silent
//! and a head that gains rows cannot stay listed.

use std::collections::BTreeSet;
use std::path::Path;

use codeintel::{Regexes, STDLIB, load_facts, render_row};
use datalog::{Engine, Limits, Strings};

/// Heads with no rows over `bank.dl`, so only the unseeded goal is run. Each
/// needs a fact shape the bank does not have; none is a head nobody asks.
const NO_ROWS: &[&str] = &[];

fn bank() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/eval/bank.dl"),
    )
    .expect("bank.dl")
}

fn engine(facts: &str) -> Engine {
    let mut e = Engine::new(Box::new(Strings::new())).with_regexes(Box::new(Regexes::new()));
    load_facts(&mut e, facts).expect("bank.dl loads");
    e.load_rules(STDLIB).expect("the stdlib loads");
    e
}

/// `(name, arity)` for every rule head in the stdlib, in file order, once.
fn heads() -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for line in STDLIB.lines() {
        if !line.starts_with(|c: char| c.is_ascii_lowercase()) {
            continue;
        }
        let head = line.split_once(":-").map_or(line, |(h, _)| h);
        let Some((name, rest)) = head.split_once('(') else {
            continue;
        };
        let mut depth = 0;
        let mut quoted = false;
        let mut arity = 1;
        for c in rest.chars() {
            match c {
                '"' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted && depth == 0 => break,
                ')' if !quoted => depth -= 1,
                ',' if !quoted && depth == 0 => arity += 1,
                _ => {}
            }
        }
        if !out.iter().any(|(n, _)| n == name) {
            out.push((name.to_string(), arity));
        }
    }
    out
}

/// A value as a goal literal: a string quoted, an integer bare.
fn literal(e: &Engine, atom: datalog::Atom) -> String {
    e.resolve(atom).map_or_else(
        || atom.to_string(),
        |s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
    )
}

/// Run `goal` both ways and return the rows, or why they disagree.
fn both(facts: &str, goal: &str) -> Result<Vec<Vec<datalog::Atom>>, String> {
    let (mut fast, mut slow) = (engine(facts), engine(facts));
    let quick = fast
        .query(goal, &Limits::default())
        .map_err(|d| format!("semi-naive: {}: {}", d.status, d.message))?;
    let plain = slow
        .query_naive(goal, &Limits::default())
        .map_err(|d| format!("naive: {}: {}", d.status, d.message))?;
    if quick.truncated || plain.truncated {
        return Err("truncated, so the comparison is of two prefixes".to_string());
    }
    let render = |e: &Engine, rows: &[Vec<datalog::Atom>]| -> Vec<String> {
        rows.iter().map(|r| render_row(e, r)).collect()
    };
    let (a, b) = (render(&fast, &quick.rows), render(&slow, &plain.rows));
    if a != b {
        let only =
            |x: &[String], y: &[String]| -> usize { x.iter().filter(|r| !y.contains(r)).count() };
        return Err(format!(
            "semi-naive gave {} rows, naive {}; {} only semi-naive, {} only naive",
            a.len(),
            b.len(),
            only(&a, &b),
            only(&b, &a)
        ));
    }
    Ok(quick.rows)
}

/// The head joined to itself, column `i` of the first literal shared with
/// column `j` of the second, for every ordered pair `i != j`.
///
/// A seed from a constant in the query is one way to bind a column; the other
/// is a literal earlier in the body, which is how rules call each other and how
/// `def(S, ...), impact_of(S, C)` is asked. The demand rewrite handles them in
/// different places, and only this shape reaches the second.
fn joined(name: &str, arity: usize) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..arity {
        for j in 0..arity {
            if i == j {
                continue;
            }
            let first: Vec<String> = (0..arity).map(|k| format!("A{k}")).collect();
            let second: Vec<String> = (0..arity)
                .map(|k| {
                    if k == j {
                        format!("A{i}")
                    } else {
                        format!("B{k}")
                    }
                })
                .collect();
            out.push(format!(
                "?- {name}({}), {name}({}).",
                first.join(", "),
                second.join(", ")
            ));
        }
    }
    out
}

fn goal(name: &str, args: &[String]) -> String {
    format!("?- {name}({}).", args.join(", "))
}

#[test]
fn every_stdlib_head_agrees_under_both_evaluators() {
    let facts = bank();
    let probe = engine(&facts);
    let heads = heads();
    assert!(
        heads.len() >= 40,
        "found {} heads; the scan broke",
        heads.len()
    );

    let mut disagree = Vec::new();
    let mut rowless = BTreeSet::new();
    let mut ran = 0;
    for (name, arity) in &heads {
        let vars: Vec<String> = (0..*arity).map(|i| format!("V{i}")).collect();
        let first = match both(&facts, &goal(name, &vars)) {
            Ok(rows) => rows,
            Err(why) => {
                disagree.push(format!("{name}/{arity} unseeded: {why}"));
                continue;
            }
        };
        ran += 1;
        let Some(row) = first.first() else {
            rowless.insert(name.as_str());
            continue;
        };
        let values: Vec<String> = row.iter().map(|a| literal(&probe, *a)).collect();
        // Every non-empty subset of columns bound to the first row's values:
        // each demand adornment the head can be asked in by constants. A
        // head's arity is at most seven, so this is at most 127 goals.
        assert!(*arity <= 8, "{name}/{arity}: bind-subsets would explode");
        let mut goals: Vec<String> = (1_u32..1 << arity)
            .map(|mask| {
                let args: Vec<String> = (0..*arity)
                    .map(|i| {
                        if mask >> i & 1 == 1 {
                            values[i].clone()
                        } else {
                            vars[i].clone()
                        }
                    })
                    .collect();
                goal(name, &args)
            })
            .collect();
        goals.extend(joined(name, *arity));
        for text in goals {
            ran += 1;
            if let Err(why) = both(&facts, &text) {
                disagree.push(format!("{text}: {why}"));
            }
        }
    }

    assert!(
        disagree.is_empty(),
        "the evaluators disagree, or a goal did not run:\n{}",
        disagree.join("\n")
    );
    let listed: BTreeSet<&str> = NO_ROWS.iter().copied().collect();
    assert_eq!(
        rowless, listed,
        "heads with no rows over bank.dl, seeded goals not run; this must equal NO_ROWS"
    );
    assert!(
        ran > heads.len(),
        "ran {ran} goals over {} heads",
        heads.len()
    );
}
