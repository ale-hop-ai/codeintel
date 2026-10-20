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
//! ```
//!
//! `--facts` exists because the eval has more than one fact set: `facts.dl` is
//! frozen with the M1 bank, and `bank.dl` carries the working and held-out
//! ones. The default is still the embedded M1 fixture, so every command
//! recorded in `docs/agent-eval.md` still means what it meant.

use std::process::ExitCode;

use codeintel::{Regexes, STDLIB, load_facts, render_row};
use datalog::{Engine, Limits, Strings};

const FACTS: &str = include_str!("../../../tests/fixtures/eval/facts.dl");

const USAGE: &str = "usage: eval [--facts <path>] '<datalog query>'";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut facts_path = None;
    let mut query = None;
    while let Some(arg) = args.next() {
        if arg == "--facts" {
            let Some(path) = args.next() else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            facts_path = Some(path);
        } else if query.is_none() {
            query = Some(arg);
        } else {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    let Some(query) = query else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

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
