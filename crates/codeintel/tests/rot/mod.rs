//! What a rot guard says about a set of recomputed reference answers.
//!
//! Shared by `eval_banks.rs` and `eval_rot.rs`, which compute the answers in
//! different ways (a bare engine over `bank.dl`, an indexed fixture through
//! `query::run`) and must judge them the same way. Both used to carry their own
//! judgement, and the second one treated a reference that does not run as an
//! empty answer (#96).
//!
//! A question's answer is `count<TAB>payload`, as `score.py` writes it, or the
//! reason it could not be computed.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A question's reference query and what running it gave.
pub(crate) type Computed<'a> = BTreeMap<&'a str, (&'a str, Result<String, String>)>;

/// `count<TAB>payload` for a payload of `|`-joined tuples.
pub(crate) fn line(payload: &str) -> String {
    let count = if payload.is_empty() {
        0
    } else {
        payload.split('|').count()
    };
    format!("{count}\t{payload}")
}

/// Every way the recomputed answers disagree with the record. Empty means they
/// hold.
///
/// - a reference that does not run is reported, with its reason when
///   `show_queries` allows it, and never read as an empty answer;
/// - `score.py` compares only ids present on both sides, so a question with no
///   recorded answer, and an answer for no question, are findings here;
/// - an empty reference answer is a finding unless the question is in
///   `known_empty`: a question answered by nothing is passed by any query that
///   returns nothing, a broken one included.
pub(crate) fn findings(
    computed: &Computed<'_>,
    recorded: &BTreeMap<&str, &str>,
    known_empty: &[&str],
    show_queries: bool,
) -> String {
    let mut out = String::new();
    for (id, (_, answer)) in computed {
        let Err(why) = answer else { continue };
        // A diagnostic can quote the query, which the held-out bank may not show.
        if show_queries {
            writeln!(out, "Q{id}: reference query did not run: {why}")
        } else {
            writeln!(out, "Q{id}: reference query did not run")
        }
        .expect("writes to a String");
    }

    let rows = |pred: &dyn Fn(&str, &str) -> bool| -> Vec<&str> {
        computed
            .iter()
            .filter_map(|(id, (_, answer))| Some((*id, answer.as_ref().ok()?)))
            .filter(|(id, line)| pred(id, line))
            .map(|(id, _)| id)
            .collect()
    };
    let unrecorded = rows(&|id, _| !recorded.contains_key(id));
    let moved = rows(&|id, line| recorded.get(id).is_some_and(|r| *r != line));
    let empty = rows(&|id, line| line.ends_with('\t') && !known_empty.contains(&id));
    let stale = rows(&|id, line| known_empty.contains(&id) && !line.ends_with('\t'));
    let orphaned: Vec<&str> = recorded
        .keys()
        .copied()
        .filter(|id| !computed.contains_key(id))
        .collect();

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
            writeln!(out, "{what}: Q{}", ids.join(", Q")).expect("writes to a String");
        }
    }
    if show_queries {
        for id in &moved {
            if let (Some((reference, Ok(line))), Some(was)) = (computed.get(id), recorded.get(id)) {
                writeln!(out, "Q{id}: {reference}\n  -{was}\n  +{line}")
                    .expect("writes to a String");
            }
        }
    }
    out
}
