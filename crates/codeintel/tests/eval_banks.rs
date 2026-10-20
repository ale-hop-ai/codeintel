//! The working and held-out eval banks, re-derived on every commit:
//! `docs/50-self-improvement.md` § 5.
//!
//! `eval_rot.rs` pins the frozen set A. These two banks were committed with
//! reference queries and a recorded `expected.tsv`, and `score.py --bank <name>
//! check` recomputes them — when a human runs it. Until this test, a rule change
//! could move a reference answer and nothing in CI would say so. "A reference
//! answer nobody recomputes is an assertion nobody checks."
//!
//! Each question's reference query is run against `bank.dl` and the stdlib, the
//! way `examples/eval.rs` runs it, projected onto the question's answer
//! variables, and compared with the committed record. The projection and the
//! `count<TAB>payload` encoding are `score.py`'s, so `score.py expected` writes
//! exactly what this reads.
//!
//! **A failure here is a finding, not a golden to refresh.** A moved reference
//! answer means the facts or the rules changed what a question's correct answer
//! is. Say why in the commit, then regenerate with
//! `score.py --bank <name> expected`. Nothing here rewrites a file, and there is
//! no `UPDATE_GOLDEN`: that switch is the thing this rule exists to keep out.
//!
//! The held-out bank is read and never shown. A failure names its question ids
//! and not their queries, because an experimenting agent sees this output and
//! the bank is worthless the moment one reads a reference query out of it.
//! `docs/50-self-improvement.md` § 5.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use codeintel::{Regexes, STDLIB, load_facts, render_row};
use datalog::{Engine, Limits, Strings};

/// Questions whose correct answer is genuinely "no rows" — a conformance
/// question asserts that a policy is not violated. Every other empty reference
/// answer is a defect: a question answered by nothing is passed by any query
/// that returns nothing, a broken one included. `score.py`'s `KNOWN_EMPTY` is
/// the same list.
const WORKING_EMPTY: &[&str] = &["49"];
const HELDOUT_EMPTY: &[&str] = &["23"];

/// Openers whose subject is the *previous* question's.
///
/// `run.py`'s partition is a fresh shuffle per round, retried until no pair of
/// questions shares a group twice, so a question and its referent are almost
/// never read together and the rule actively pushes them apart. A question that
/// does not carry its own subject is therefore read by the agent as written:
/// without one.
const REFERENTIAL: &[&str] = &["The same", "The other", "Likewise", "Again,"];

/// Questions that open referentially and are kept as they are.
///
/// Not an excuse list — a record of six that a scored run has already been
/// measured against (#88). Rewording one changes what a recorded run was asked,
/// which `docs/agent-eval.md` § Rescored has already withdrawn a score for, so
/// the six are the owner's call and this list is what stops a seventh arriving
/// silently. Each is named with the question it refers to.
const WORKING_REFERENTIAL: &[&str] = &[
    "28", // -> Q27, the call graph on either provenance.
    "32", // -> Q31, the blast radius of `Store#put()`.
    "34", // -> Q33, what `init()` calls directly or transitively.
    "40", // -> Q39, callable definitions called by nothing.
    "43", // -> Q42, exported definitions never referenced outside their file.
    "46", // -> Q45, files referencing something defined in another file.
];
const HELDOUT_REFERENTIAL: &[&str] = &[];

#[test]
fn working_bank_reference_answers_still_hold() {
    holds("working", WORKING_EMPTY, true);
}

#[test]
fn heldout_bank_reference_answers_still_hold() {
    holds("heldout", HELDOUT_EMPTY, false);
}

#[test]
fn working_bank_questions_carry_their_own_subject() {
    carries_its_subject("working", WORKING_REFERENTIAL);
}

#[test]
fn heldout_bank_questions_carry_their_own_subject() {
    carries_its_subject("heldout", HELDOUT_REFERENTIAL);
}

/// No question leans on the previous one's subject, except those recorded as
/// doing so.
///
/// Ids only, never text: this runs over the held-out bank too.
fn carries_its_subject(bank: &str, allowed: &[&str]) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/eval");
    let questions =
        std::fs::read_to_string(dir.join(bank).join("questions.tsv")).expect("a bank file");
    let referential: BTreeSet<&str> = questions
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let mut f = l.split('\t');
            let id = f.next()?;
            let text = f.nth(1)?;
            REFERENTIAL
                .iter()
                .any(|o| text.starts_with(o))
                .then_some(id)
        })
        .collect();
    let allowed: BTreeSet<&str> = allowed.iter().copied().collect();

    let undeclared: Vec<&&str> = referential.difference(&allowed).collect();
    assert!(
        undeclared.is_empty(),
        "{bank}: Q{undeclared:?} open with a back-reference to the previous \
         question. The partition reads them apart, so the agent never sees the \
         referent. Give the question its own subject, or add it to \
         WORKING_REFERENTIAL with the question it refers to."
    );
    let stale: Vec<&&str> = allowed.difference(&referential).collect();
    assert!(
        stale.is_empty(),
        "{bank}: Q{stale:?} are listed as referential but no longer open that \
         way — drop them from the list."
    );
}

struct Question<'a> {
    id: &'a str,
    vars: Vec<&'a str>,
    reference: &'a str,
}

fn parse(line: &str) -> Question<'_> {
    let mut f = line.split('\t');
    let shape = "id<TAB>vars<TAB>text<TAB>query";
    let id = f.next().expect(shape);
    let vars = f.next().expect(shape).split(',').collect();
    let reference = f.nth(1).expect(shape);
    Question {
        id,
        vars,
        reference,
    }
}

fn holds(bank: &str, known_empty: &[&str], show_queries: bool) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/eval");
    let read = |path: std::path::PathBuf| std::fs::read_to_string(path).expect("a bank file");
    let questions = read(dir.join(bank).join("questions.tsv"));
    let recorded = read(dir.join(bank).join("expected.tsv"));
    let facts = read(dir.join("bank.dl"));

    let mut engine = Engine::new(Box::new(Strings::new())).with_regexes(Box::new(Regexes::new()));
    load_facts(&mut engine, &facts).expect("bank.dl loads");
    engine.load_rules(STDLIB).expect("the stdlib loads");

    // id -> (reference query, `count<TAB>payload` as score.py writes it).
    let mut computed: BTreeMap<&str, (&str, String)> = BTreeMap::new();
    let mut broken = String::new();
    for q in questions
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(parse)
    {
        match project(&mut engine, q.reference, &q.vars) {
            Ok(payload) => {
                let count = if payload.is_empty() {
                    0
                } else {
                    payload.split('|').count()
                };
                computed.insert(q.id, (q.reference, format!("{count}\t{payload}")));
            }
            // A diagnostic can quote the query, which the held-out bank may not show.
            Err(why) if show_queries => {
                writeln!(broken, "Q{}: {why}", q.id).expect("writes to a String");
            }
            Err(_) => writeln!(broken, "Q{}: reference query does not run", q.id)
                .expect("writes to a String"),
        }
    }

    let recorded: BTreeMap<&str, &str> = recorded
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_once('\t'))
        .collect();

    // `score.py` compares only ids present on both sides, so a question added
    // without an answer, or an answer left behind by a deleted question, passes
    // there silently. Here both are findings.
    let ids = |pred: &dyn Fn(&str, &(&str, String)) -> bool| -> Vec<&str> {
        computed
            .iter()
            .filter(|(id, v)| pred(id, v))
            .map(|(id, _)| *id)
            .collect()
    };
    let unrecorded = ids(&|id, _| !recorded.contains_key(id));
    let moved = ids(&|id, (_, line)| recorded.get(id).is_some_and(|r| r != line));
    let empty = ids(&|id, (_, line)| line.ends_with('\t') && !known_empty.contains(&id));
    let stale = ids(&|id, (_, line)| known_empty.contains(&id) && !line.ends_with('\t'));
    let orphaned: Vec<&str> = recorded
        .keys()
        .copied()
        .filter(|id| !computed.contains_key(id))
        .collect();

    let mut findings = broken;
    for (what, ids) in [
        ("no recorded answer", &unrecorded),
        ("recorded answer for no question", &orphaned),
        ("reference answer moved", &moved),
        (
            "reference answer is EMPTY and not a known conformance question",
            &empty,
        ),
        ("listed as known-empty but now returns rows", &stale),
    ] {
        if !ids.is_empty() {
            writeln!(findings, "{what}: Q{}", ids.join(", Q")).expect("writes to a String");
        }
    }
    if show_queries {
        for id in &moved {
            if let (Some((reference, line)), Some(was)) = (computed.get(id), recorded.get(id)) {
                writeln!(findings, "Q{id}: {reference}\n  -{was}\n  +{line}")
                    .expect("writes to a String");
            }
        }
    }
    assert!(
        findings.is_empty(),
        "{bank} bank reference answers do not hold — the facts or rules changed what these \
         questions' correct answers are. A finding, not a golden: explain it, then regenerate \
         with `score.py --bank {bank} expected`.\n{findings}"
    );
}

/// `score.py`'s `project` and `payload`: the set of tuples over the answer
/// variables, sorted, each tuple space-joined, tuples joined with `|`.
fn project(engine: &mut Engine, program: &str, wanted: &[&str]) -> Result<String, String> {
    let result = engine
        .query(program, &Limits::default())
        .map_err(|d| format!("reference query failed: {}: {}", d.status, d.message))?;
    let index: Vec<usize> = wanted
        .iter()
        .map(|w| result.columns.iter().position(|c| c == w))
        .collect::<Option<_>>()
        .ok_or_else(|| format!("reference query does not bind {wanted:?}"))?;
    let tuples: BTreeSet<Vec<String>> = result
        .rows
        .iter()
        .map(|row| {
            let line = render_row(engine, row);
            let cells: Vec<&str> = line.split('\t').collect();
            index
                .iter()
                .map(|&i| cells.get(i).map(ToString::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .collect::<Option<_>>()
        .ok_or("a row is narrower than its columns")?;
    Ok(tuples
        .into_iter()
        .map(|t| t.join(" "))
        .collect::<Vec<_>>()
        .join("|"))
}
