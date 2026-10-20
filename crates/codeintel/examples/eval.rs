//! Run one Datalog query against the agent-eval fixture and print the rows.
//!
//! The harness behind `docs/agent-eval.md`. It stands in for `codeintel query`
//! at M1, when there is a fact store but no CLI yet — and it is an example
//! rather than a verb, because the surface budget is five verbs and this is
//! not one of them.
//!
//! ```sh
//! cargo run -p codeintel --example eval -- '?- def(S, F, "function", N).'
//! cargo run -p codeintel --example eval -- --facts tests/fixtures/eval/bank.dl '?- file(F, L).'
//! cargo run -p codeintel --example eval -- --facts tests/fixtures/eval/bank.dl --schema
//! ```
//!
//! `--schema` prints the page `codeintel schema` would print for an index
//! holding exactly these facts, counts and all, through the real renderer. It
//! is what `tests/fixtures/eval/run.py` hands an agent: the bank has no source
//! tree to index, and the no-index page says "THERE IS NO INDEX HERE", which
//! would be a different measurement.
//!
//! `--facts` exists because the eval has more than one fact set: `facts.dl` is
//! frozen with the M1 bank, and `bank.dl` carries the working and held-out
//! ones. The default is still the embedded M1 fixture, but that fixture has not
//! loaded since schema 2 made `exported` a rule (#87), so the default-fixture
//! commands recorded in `docs/agent-eval.md` do NOT run. `tests/eval_commands.rs`
//! executes every command above and every `.dl` fixture, and holds the dead one
//! to its recorded failure until #87 is decided.

use std::process::ExitCode;

use codeintel::census::Census;
use codeintel::schema::Schema;
use codeintel::{Regexes, STDLIB, load_facts, render_row};
use datalog::{Engine, Limits, Strings};

const FACTS: &str = include_str!("../../../tests/fixtures/eval/facts.dl");

const USAGE: &str = "usage: eval [--facts <path>] ('<datalog query>' | --schema)";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut facts_path = None;
    let mut query = None;
    let mut want_schema = false;
    while let Some(arg) = args.next() {
        if arg == "--facts" {
            let Some(path) = args.next() else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            facts_path = Some(path);
        } else if arg == "--schema" {
            want_schema = true;
        } else if query.is_none() {
            query = Some(arg);
        } else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    if want_schema == query.is_some() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let read;
    let facts = match facts_path {
        None => FACTS,
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(text) => {
                read = text;
                read.as_str()
            }
            Err(error) => {
                eprintln!("fixture {path}: {error}");
                return ExitCode::from(3);
            }
        },
    };

    let mut engine = Engine::new(Box::new(Strings::new())).with_regexes(Box::new(Regexes::new()));
    if let Err(diagnostic) = load_facts(&mut engine, facts) {
        eprintln!("fixture: {diagnostic}");
        return ExitCode::from(3);
    }
    if let Err(diagnostic) = engine.load_rules(STDLIB) {
        eprintln!("stdlib: {diagnostic}");
        return ExitCode::from(3);
    }

    let Some(query) = query else {
        return print_schema(&mut engine);
    };

    match engine.query(&query, &Limits::default()) {
        Ok(result) => {
            println!("columns\t{}", result.columns.join("\t"));
            // Raw rows: the eval's fact files are hand-written, so there is no
            // index to resolve a symbol's location against.
            let mut rows: Vec<String> =
                result.rows.iter().map(|r| render_row(&engine, r)).collect();
            rows.sort();
            for row in rows {
                println!("{row}");
            }
            println!("rows\t{}", result.rows.len());
            if result.truncated {
                println!(
                    "truncated\t{}",
                    result.cap.map_or("?", datalog::Cap::as_str)
                );
            }
            ExitCode::SUCCESS
        }
        Err(diagnostic) => {
            println!("status\t{}", diagnostic.status);
            println!("message\t{}", diagnostic.message);
            ExitCode::from(1)
        }
    }
}

/// The `schema` page for an index holding exactly the loaded facts.
fn print_schema(engine: &mut Engine) -> ExitCode {
    let mut census = Census {
        indexed: true,
        ..Census::default()
    };
    for rel in facts::RELATIONS {
        let goals = (0..rel.arity)
            .map(|i| format!("V{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let Some(rows) = engine_rows(engine, &format!("?- {}({goals}).", rel.name)) else {
            return ExitCode::from(3);
        };
        census.relations.insert(rel.name, rows.len());
        match rel.name {
            "file" => {
                for lang in column(&rows, 1) {
                    census.langs.entry(lang.clone()).or_default().files += 1;
                }
            }
            "def" => {
                for kind in column(&rows, 2) {
                    *census.kinds.entry(kind.clone()).or_default() += 1;
                }
            }
            "scip_ref" => {
                for role in column(&rows, 5) {
                    *census.roles.entry(role.clone()).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    print!("{}", Schema { census: &census });
    ExitCode::SUCCESS
}

/// Every row of a query, raw, split into columns. `None` after reporting why.
fn engine_rows(engine: &mut Engine, query: &str) -> Option<Vec<Vec<String>>> {
    match engine.query(query, &Limits::default()) {
        Ok(result) if !result.truncated => Some(
            result
                .rows
                .iter()
                .map(|r| {
                    render_row(engine, r)
                        .split('\t')
                        .map(str::to_owned)
                        .collect()
                })
                .collect(),
        ),
        Ok(_) => {
            eprintln!("schema: `{query}` was truncated; a census cannot be a lower bound");
            None
        }
        // A base relation the fact file never mentions is not loaded at all,
        // and the engine reports that as unknown. In a census that is a zero.
        Err(diagnostic) if diagnostic.message.starts_with("no relation or rule named") => {
            Some(Vec::new())
        }
        Err(diagnostic) => {
            eprintln!("schema: {query}: {}", diagnostic.message);
            None
        }
    }
}

fn column(rows: &[Vec<String>], index: usize) -> impl Iterator<Item = &String> {
    rows.iter().filter_map(move |row| row.get(index))
}
