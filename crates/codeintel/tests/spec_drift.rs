//! The spec and the rules, asserted against each other.
//!
//! `specs/01-facts.md` is the ABI and `rules/stdlib.dl` is the implementation
//! of its § Derived relations. Nothing checked that they agreed, and #71 is
//! what that costs: the spec documented an `impl_name/2` helper the rules had
//! never had, for months. What an agent reads to learn what it may ask for
//! named a relation that answers `invalid-query`.
//!
//! These are guards in the shape `surface.rs` already uses for the schema
//! budget — read the document, read the rules, assert the two agree, in both
//! directions. Both directions matters: a rule the spec omits is as wrong as a
//! relation the spec invents, and only one of the two is visible from the
//! document alone.
//!
//! They are deliberately scoped to `01-facts.md`. `03-datalog.md`'s examples
//! are synthetic by design — `hot/2`, `magic_impact_of/1` — and the base
//! relation sections carry queries a *caller* writes (`lexical_parent/2`,
//! which that section says outright nothing in `stdlib.dl` wants). A guard
//! over every fenced block in `specs/` would be a guard with an exception
//! list, and an exception list is where this kind of drift hides.

use std::collections::{BTreeMap, BTreeSet};

use codeintel::STDLIB;

const FACTS_SPEC: &str = include_str!("../../../specs/01-facts.md");

/// A predicate, as `name/arity`.
type Pred = (String, usize);

/// Every clause head in a Datalog source, as `name/arity`.
///
/// Its own parser rather than `schema::rules`, which reads the `%%` doc lines
/// and not the clauses, and rather than `surface.rs`'s, which does not need
/// the two things this does: several facts on one line (`callable("function").
/// callable("method").`) and commas inside quoted arguments (`about(S, "sig",
/// ...)`). Teaching that parser these cases to share it would make it stricter
/// for a caller that has no use for either.
fn clause_heads(source: &str) -> BTreeSet<Pred> {
    let mut out = BTreeSet::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('%') {
            continue;
        }
        // Only the head: everything left of `:-`, or the whole line for a fact.
        let head = line.split(":-").next().unwrap_or_default();
        // A continuation line of a multi-line body has no head of its own.
        for atom in atoms(head) {
            out.insert(atom);
        }
    }
    out
}

/// The `name(args)` atoms written at the top level of one head.
///
/// Returns nothing for a body continuation line, which is what keeps a literal
/// like `def(S, F, _, N)` from being read as a rule that defines `def`.
fn atoms(head: &str) -> Vec<Pred> {
    let mut out = Vec::new();
    let mut rest = head.trim();
    while let Some(open) = rest.find('(') {
        let (name, tail) = rest.split_at(open);
        let name = name.trim();
        if !is_predicate_name(name) {
            return out;
        }
        let Some((args, after)) = tail.get(1..).and_then(split_args) else {
            return out;
        };
        out.push((name.to_string(), arity(args)));
        // Another fact on the same line follows its `.`; anything else — a
        // body literal, a comparison — means this was not a bare fact list.
        let after = after.trim_start();
        match after.strip_prefix('.') {
            Some(next) => rest = next.trim_start(),
            None => return out,
        }
        if rest.is_empty() {
            return out;
        }
    }
    out
}

fn is_predicate_name(name: &str) -> bool {
    !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The argument text up to the matching `)`, and whatever follows it.
///
/// Quote-aware, because `about(S, "sig", Sig, "", 0)` would otherwise end at
/// the first `)` inside a string the day one appears.
fn split_args(text: &str) -> Option<(&str, &str)> {
    let mut depth = 1usize;
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth -= 1;
                if depth == 0 {
                    return Some((text.get(..i)?, text.get(i + 1..)?));
                }
            }
            _ => {}
        }
    }
    None
}

/// Top-level commas plus one, with commas inside quotes not counted.
fn arity(args: &str) -> usize {
    if args.trim().is_empty() {
        return 0;
    }
    let mut count = 1;
    let mut quoted = false;
    for c in args.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => count += 1,
            _ => {}
        }
    }
    count
}

/// The lines of one `##` section of a markdown document, by heading prefix.
fn section<'a>(doc: &'a str, heading: &str) -> &'a str {
    assert!(
        doc.contains(heading),
        "`{heading}` is not a heading in specs/01-facts.md"
    );
    let start = doc.find(heading).unwrap_or_default();
    let body = doc.get(start..).unwrap_or_default();
    // The next top-level heading ends it. Skip the one we just matched.
    let end = body
        .match_indices("\n## ")
        .map(|(i, _)| i + 1)
        .find(|i| *i > 1)
        .unwrap_or(body.len());
    body.get(..end).unwrap_or_default()
}

/// Every ```` ```prolog ```` block in a slice of markdown, concatenated.
fn prolog_blocks(markdown: &str) -> String {
    let mut out = String::new();
    let mut rest = markdown;
    while let Some(open) = rest.find("```prolog\n") {
        let after = rest.get(open + "```prolog\n".len()..).unwrap_or_default();
        let Some(close) = after.find("```") else {
            break;
        };
        out.push_str(after.get(..close).unwrap_or_default());
        out.push('\n');
        rest = after.get(close + 3..).unwrap_or_default();
    }
    out
}

// ---------------------------------------------------------------- the roster

/// The `_exact` roster in `01-facts.md`, parsed: contaminated rule -> twin.
///
/// `None` is the `about/5` row, which records a deliberate exclusion rather
/// than a twin.
fn exact_roster() -> BTreeMap<Pred, Option<Pred>> {
    let mut out = BTreeMap::new();
    for line in FACTS_SPEC.lines() {
        let line = line.trim();
        let Some(row) = line.strip_prefix("| `") else {
            continue;
        };
        let Some((rule, twin)) = row.split_once(" | ") else {
            continue;
        };
        let Some(rule) = rule.strip_suffix('`').and_then(parse_pred) else {
            continue;
        };
        let twin = twin.trim_end_matches(" |").trim();
        let twin = twin
            .strip_prefix('`')
            .and_then(|t| t.strip_suffix('`'))
            .and_then(parse_pred);
        out.insert(rule, twin);
    }
    assert!(
        out.len() > 5,
        "the `_exact` roster table was not found in specs/01-facts.md — it is \
         the thing this test exists to check, so a parse that finds nothing is \
         a failure, not a pass"
    );
    out
}

fn parse_pred(text: &str) -> Option<Pred> {
    let (name, arity) = text.split_once('/')?;
    if !is_predicate_name(name) {
        return None;
    }
    Some((name.to_string(), arity.parse().ok()?))
}

/// The roster of `_exact` twins is exhaustive in both directions.
///
/// It says so about itself, and twice it was not: an earlier draft claimed
/// every contaminated rule shipped in both forms while only `dead_export` did,
/// and a later one listed `about` while `stdlib.dl` had no `about_exact`. This
/// test found a third — `ref_outside_exact/2`, in the rules and absent from
/// the list — which is the direction prose review never catches, because a
/// reader checks what the list says and not what it leaves out.
#[test]
fn the_exact_roster_matches_the_rules() {
    let roster = exact_roster();
    let rules = clause_heads(STDLIB);

    for (rule, twin) in &roster {
        assert!(
            rules.contains(rule),
            "the roster names `{}/{}` as a contaminated rule, and `rules/stdlib.dl` \
             has no such predicate at that arity",
            rule.0,
            rule.1
        );
        if let Some(twin) = twin {
            assert!(
                rules.contains(twin),
                "the roster promises `{}/{}` as the twin of `{}/{}`, and \
                 `rules/stdlib.dl` has no such predicate at that arity",
                twin.0,
                twin.1,
                rule.0,
                rule.1
            );
        }
    }

    // The `about/5` row records an exclusion. An `about_exact` appearing in the
    // rules would make the spec's reasoning — ten clauses to buy what
    // `calls_exact` already answers — a decision that was quietly reversed.
    for (rule, twin) in &roster {
        if twin.is_some() {
            continue;
        }
        let excluded = format!("{}_exact", rule.0);
        assert!(
            !rules.iter().any(|(name, _)| *name == excluded),
            "`{excluded}` exists in rules/stdlib.dl, but the roster records \
             `{}/{}` as having no twin, deliberately",
            rule.0,
            rule.1
        );
    }

    let twins: BTreeSet<&Pred> = roster.values().flatten().collect();
    for pred in &rules {
        if !pred.0.ends_with("_exact") {
            continue;
        }
        assert!(
            twins.contains(pred),
            "`{}/{}` is an `_exact` rule in rules/stdlib.dl that the roster in \
             specs/01-facts.md does not name. The roster says it is exhaustive, \
             so either add the row or the rule should not exist.",
            pred.0,
            pred.1
        );
    }
}

// ------------------------------------------------- the general form of #71

/// Every rule the spec catalogues exists, and every rule that exists is
/// catalogued.
///
/// This is #71 generalised. `specs/01-facts.md` § Derived relations transcribes
/// `rules/stdlib.dl`, and a transcription nobody diffs is a transcription that
/// drifts: `impl_name/2` sat in it for months naming a relation the engine
/// answers `invalid-query` for.
///
/// Arity is part of the identity. `long_def(S, N)` became `long_def(S)` once
/// already, and three of four agent-eval failures were agents reading the old
/// signature (`docs/agent-eval.md`).
#[test]
fn the_spec_catalogue_and_the_rules_define_the_same_predicates() {
    let catalogued = clause_heads(&prolog_blocks(section(
        FACTS_SPEC,
        "## Derived relations (`rules/stdlib.dl`)",
    )));
    let rules = clause_heads(STDLIB);
    assert!(
        catalogued.len() > 20,
        "§ Derived relations parsed to {} predicates, which means the section \
         or its code fences moved and this test is checking nothing",
        catalogued.len()
    );

    let base: BTreeSet<&str> = facts::RELATIONS.iter().map(|r| r.name).collect();

    let invented: Vec<&Pred> = catalogued
        .iter()
        .filter(|pred| !rules.contains(*pred) && !base.contains(pred.0.as_str()))
        .collect();
    assert!(
        invented.is_empty(),
        "specs/01-facts.md § Derived relations defines {invented:?}, which is \
         neither a rule in rules/stdlib.dl nor a base relation. An agent reading \
         the spec would ask for it and get `invalid-query` (#71)."
    );

    let undocumented: Vec<&Pred> = rules.iter().filter(|p| !catalogued.contains(*p)).collect();
    assert!(
        undocumented.is_empty(),
        "rules/stdlib.dl defines {undocumented:?}, which specs/01-facts.md \
         § Derived relations does not. The spec is the ABI; a rule it omits is \
         a rule nobody can know to ask for."
    );
}

/// Every base relation has a spec section, and every spec section is a base
/// relation.
///
/// The same drift one layer down. The `## Base relations` section carries one
/// level-three heading per relation, written as the relation with its argument
/// names, and that arity is what a reader counts columns from.
///
/// `has_type` is the one heading that is not a relation, and the document says
/// so in the heading itself — DEFERRED, kept so that re-adding it is a decision
/// with a record. Read from the heading rather than listed here, so a second
/// deferred relation needs no change to this test and an undeferred one fails
/// it.
#[test]
fn every_base_relation_has_a_spec_section_of_the_right_arity() {
    let base = section(FACTS_SPEC, "## Base relations");
    let mut documented = BTreeSet::new();

    for line in base.lines() {
        let Some(heading) = line.strip_prefix("### `") else {
            continue;
        };
        let Some((head, rest)) = heading.split_once('`') else {
            continue;
        };
        let heads = atoms(head);
        let [pred] = heads.as_slice() else {
            panic!("`### {head}` is not a readable relation heading");
        };
        if rest.contains("DEFERRED") {
            assert!(
                !facts::RELATIONS.iter().any(|r| r.name == pred.0),
                "`{}` is marked DEFERRED in specs/01-facts.md and is a live base \
                 relation",
                pred.0
            );
            continue;
        }
        let rel = facts::RELATIONS
            .iter()
            .find(|r| r.name == pred.0)
            .unwrap_or_else(|| {
                panic!(
                    "specs/01-facts.md documents `{}` under § Base relations and \
                     `facts::RELATIONS` has no such relation",
                    pred.0
                )
            });
        assert_eq!(
            pred.1, rel.arity,
            "`{}` is written with {} columns in specs/01-facts.md and has {} in \
             facts::RELATIONS",
            pred.0, pred.1, rel.arity
        );
        documented.insert(pred.0.clone());
    }

    for rel in facts::RELATIONS {
        assert!(
            documented.contains(rel.name),
            "`{}` is a base relation with no `### ` section in \
             specs/01-facts.md § Base relations",
            rel.name
        );
    }
}
