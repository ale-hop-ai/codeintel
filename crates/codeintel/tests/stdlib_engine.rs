//! The shipped rule library through the engine's own gates: it loads as
//! written, the naive twin accepts it, hand-written facts get hand-verified
//! answers, the seeded traversals agree with naive evaluation, and demand
//! transformation applies to them.
//!
//! These lived in `crates/datalog`'s conformance tests and read
//! `rules/stdlib.dl` from there, so any stdlib edit broke the code-agnostic
//! crate's unit tests. They belong to the crate that owns the library
//! (`specs/00-overview.md` invariant 6). The engine's differential twins are
//! public and doc-hidden for exactly this file.

use codeintel::Regexes;
use datalog::{Engine, Limits, QueryResult, Strings};

/// Rows rendered as text, in result order: the comparable form.
fn render(engine: &Engine, result: &QueryResult) -> Vec<String> {
    result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|a| {
                    engine
                        .resolve(*a)
                        .map_or_else(|| a.to_string(), |s| format!("\"{s}\""))
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Run a query both ways and assert they agree; return the fast answer and
/// its result. The fast side is semi-naive over the demand-rewritten program,
/// the slow side naive over the program as written
/// (`specs/03-datalog.md` § Validation 5).
fn both_in(make: impl Fn() -> Engine, query: &str) -> (Vec<String>, QueryResult) {
    let mut fast = make();
    let quick = fast
        .query(query, &Limits::default())
        .expect("semi-naive answers");
    let mut slow = make();
    let plain = slow
        .query_naive(query, &Limits::default())
        .expect("naive answers");

    let a = render(&fast, &quick);
    let b = render(&slow, &plain);
    assert_eq!(a, b, "semi-naive and naive disagree on {query:?}");
    assert_eq!(quick.columns, plain.columns);
    (a, quick)
}

// ── the standard library ────────────────────────────────────────────────────

/// The 15 base relations of `specs/01-facts.md`, empty.
const BASE: [(&str, usize); 16] = [
    ("file", 2),
    ("def", 4),
    ("def_span", 5),
    ("def_name", 3),
    ("def_sig", 2),
    ("def_doc", 2),
    ("parent", 2),
    ("visibility", 2),
    ("resolved", 1),
    ("import", 3),
    ("scip_ref", 6),
    ("name_ref", 5),
    ("scip_impl", 2),
    ("name_impl", 4),
    ("extern", 4),
    ("name_export", 2),
];

fn stdlib_engine() -> Engine {
    let mut e = Engine::new(Box::new(Strings::new())).with_regexes(Box::new(Regexes::new()));
    for (name, arity) in BASE {
        e.insert_relation(name, datalog::Relation::new(arity));
    }
    e
}

/// The engine must accept its own standard library, **as written**, with no
/// transformation applied. A rule that is legal only after demand
/// transformation would leave the differential test with nothing to compare
/// against — which is the failure this one line guards.
#[test]
fn the_engine_loads_its_own_standard_library() {
    let mut e = stdlib_engine();
    e.load_rules(codeintel::query::STDLIB)
        .expect("stdlib loads");
    assert!(e.rule_count() > 30, "loaded {} rules", e.rule_count());
}

#[test]
fn the_naive_evaluator_accepts_the_standard_library_too() {
    let mut e = stdlib_engine();
    e.load_rules(codeintel::query::STDLIB)
        .expect("stdlib loads");
    let result = e
        .query_naive(r#"?- def(S, F, "function", N)."#, &Limits::default())
        .expect("naive evaluation runs the stdlib as written");
    assert!(result.rows.is_empty(), "no facts were loaded");
}

/// Install a base relation from text rows. A token that parses as a
/// non-negative integer becomes an integer atom, everything else a string —
/// the same split `specs/01-facts.md` § Integers describes.
fn install(e: &mut Engine, name: &str, rows: &[&[&str]]) {
    let arity = rows.first().map_or(1, |r| r.len());
    let mut rel = datalog::Relation::new(arity);
    for row in rows {
        let atoms: Vec<datalog::Atom> = row
            .iter()
            .map(|token| {
                token
                    .parse::<u32>()
                    .ok()
                    .filter(|n| datalog::atom::is_int(*n))
                    .or_else(|| e.intern(token))
                    .unwrap_or(0)
            })
            .collect();
        assert!(
            rel.push(&atoms),
            "row {row:?} has the wrong width for {name}"
        );
    }
    e.insert_relation(name, rel);
}

/// One struct with two methods, one of which calls the other by name. Small
/// enough to hand-verify, shaped enough to exercise the location bridge,
/// containment, name resolution and the seeded traversals.
fn store_fixture() -> Engine {
    let mut e = stdlib_engine();
    install(&mut e, "file", &[&["src/store.rs", "rust"]]);
    install(
        &mut e,
        "def",
        &[
            &["S#", "src/store.rs", "struct", "Store"],
            &["S#get().", "src/store.rs", "method", "get"],
            &["S#put().", "src/store.rs", "method", "put"],
        ],
    );
    install(
        &mut e,
        "def_span",
        &[
            &["S#", "1", "50", "0", "1000"],
            &["S#get().", "10", "20", "100", "400"],
            &["S#put().", "30", "40", "500", "900"],
        ],
    );
    install(
        &mut e,
        "parent",
        &[
            &["S#", "src/store.rs"],
            &["S#get().", "S#"],
            &["S#put().", "S#"],
        ],
    );
    // `put` calls `get` by name, in the same file, so rule 1 of `ref` resolves
    // it without SCIP and the call graph has one edge.
    install(
        &mut e,
        "name_ref",
        &[&["get", "src/store.rs", "35", "8", "S#put()."]],
    );
    e.load_rules(codeintel::query::STDLIB)
        .expect("stdlib loads");
    e
}

/// `store_fixture`, plus one `scip_ref` row shaped like a `pub use store::get;`
/// re-export: rust-analyzer sets no role bits on that occurrence (role
/// `"unknown"`, `scip.rs`'s `role_of`), and its enclosing symbol is the file
/// itself, not a def — `From` is `"src/lib.rs"` (#39).
fn re_export_fixture() -> Engine {
    let mut e = store_fixture();
    install(
        &mut e,
        "file",
        &[&["src/store.rs", "rust"], &["src/lib.rs", "rust"]],
    );
    install(
        &mut e,
        "scip_ref",
        &[&["S#get().", "src/lib.rs", "3", "12", "src/lib.rs", "unknown"]],
    );
    e
}

#[test]
fn a_re_export_is_not_attributed_to_the_file_as_a_caller() {
    // `Role != "def"` alone never filters this: `scip_ref` never carries a
    // `def` row, so an import/re-export occurrence passed straight through
    // with `From` bound to the file. `def(From, _, _, _)` is the guard that
    // does the work.
    let (rows, _) = both_in(re_export_fixture, r#"?- calls("src/lib.rs", "S#get().")."#);
    assert_eq!(
        rows,
        vec![] as Vec<String>,
        "the re-export was attributed to the file as a caller: {rows:?}"
    );

    // The real call site survives: `put` still calls `get` by name.
    let (rows, _) = both_in(re_export_fixture, r#"?- calls("S#put().", "S#get().")."#);
    assert_eq!(rows, vec![String::new()], "{rows:?}");
}

/// `store_fixture`, plus the three shapes a `scip_ref` target can take at a
/// position tier A can also answer (#56).
///
/// `put` calls `get` at `35:8`. Three more call sites of the same name are
/// added at `36:8`, `37:8` and `38:8`, each covered by a `scip_ref` whose
/// target is a different kind of symbol:
///
/// * `36:8` — a real definition in this index. Tier B resolved it.
/// * `37:8` — a third-party package symbol with an `extern` row and no `def`,
///   the `Map#get()` shape. Tier B resolved it; the target just lives
///   elsewhere.
/// * `38:8` — `local src/store.rs 1`, the opaque placeholder scip-typescript
///   writes when it cannot bind a name. No `def`, no `extern`, nothing in the
///   index describes it.
fn dangling_fixture() -> Engine {
    let mut e = store_fixture();
    install(
        &mut e,
        "name_ref",
        &[
            &["get", "src/store.rs", "35", "8", "S#put()."],
            &["get", "src/store.rs", "36", "8", "S#put()."],
            &["get", "src/store.rs", "37", "8", "S#put()."],
            &["get", "src/store.rs", "38", "8", "S#put()."],
        ],
    );
    install(
        &mut e,
        "scip_ref",
        &[
            &["S#get().", "src/store.rs", "36", "8", "S#put().", "read"],
            &[
                "npm map Map#get().",
                "src/store.rs",
                "37",
                "8",
                "S#put().",
                "read",
            ],
            &[
                "local src/store.rs 1",
                "src/store.rs",
                "38",
                "8",
                "S#put().",
                "read",
            ],
        ],
    );
    install(
        &mut e,
        "extern",
        &[&["npm map Map#get().", "npm", "map", "1.0.0"]],
    );
    e.load_rules(codeintel::query::STDLIB)
        .expect("stdlib loads");
    e
}

/// A `scip_ref` whose target nothing in the index can describe is not a claim
/// on the position, so tier A still answers there (#56).
///
/// Before this, the guard read `!scip_ref(_, F, L, C, _, _)` — *some* row at
/// the position, whatever it pointed at. scip-typescript writes an opaque
/// `local <file> <N>` placeholder wherever it cannot bind a name, so a
/// perfectly ordinary cross-package call was claimed by a symbol with no
/// definition anywhere, and `calls_at`'s `def(S, _, K, _)` then dropped it:
/// the position resolved to **nothing** and the callee reported zero callers.
/// 5.6% of `scip_ref` rows on the reporting corpus had this shape.
#[test]
fn a_dangling_scip_ref_does_not_suppress_the_name_tier() {
    // The placeholder's own position. Tier A resolves `get` here, by name.
    let (rows, _) = both_in(
        dangling_fixture,
        r#"?- ref(S, "src/store.rs", 38, 8, From, Role, Prov)."#,
    );
    assert_eq!(
        rows,
        vec!["\"S#get().\" \"S#put().\" \"read\" \"name\""],
        "the placeholder blocked tier A and answered nothing itself: {rows:?}"
    );

    // ...and the `exact` rule drops it too, so the tiers still partition the
    // position rather than both claiming it. One row, not two.
    let (rows, _) = both_in(
        dangling_fixture,
        r#"?- ref(S, "src/store.rs", 38, 8, From, Role, "exact")."#,
    );
    assert!(
        rows.is_empty(),
        "an occurrence naming an undescribable symbol is not a resolution: {rows:?}"
    );
}

/// The other half of the same guard: a target that *is* described still
/// suppresses tier A, whether it is described here or in a package (#56).
///
/// Gating on `def` alone — the obvious reading of the issue — frees every
/// reference into a dependency, because a third-party symbol has no `def`
/// either. That is the worst place to free: `Map#get()` is exactly the
/// `this.entries.get(key)` shape whose name guess the guard was introduced to
/// kill, and on this project's own TypeScript fixture 14 of the 18 def-less
/// `scip_ref` rows are that shape.
#[test]
fn a_resolved_scip_ref_still_suppresses_the_name_tier() {
    for line in ["36", "37"] {
        let (rows, _) = both_in(
            dangling_fixture,
            &format!(r#"?- ref(S, "src/store.rs", {line}, 8, From, Role, "name")."#),
        );
        assert!(
            rows.is_empty(),
            "tier A guessed at a position tier B had resolved, line {line}: {rows:?}"
        );
        let (rows, _) = both_in(
            dangling_fixture,
            &format!(r#"?- ref(S, "src/store.rs", {line}, 8, From, Role, "exact")."#),
        );
        assert_eq!(rows.len(), 1, "line {line} lost its exact row: {rows:?}");
    }

    // The net effect an agent sees: `get` has a caller again, and `put` is not
    // an entrypoint-by-silence.
    let (rows, _) = both_in(dangling_fixture, r#"?- calls(C, "S#get().")."#);
    assert_eq!(rows, vec!["\"S#put().\""], "{rows:?}");
}

#[test]
fn the_standard_library_answers_over_hand_written_facts() {
    // The location bridge: a file:line from a stack trace becomes a symbol.
    let (rows, _) = both_in(store_fixture, r#"?- innermost_at("src/store.rs", 15, S)."#);
    assert_eq!(rows, vec!["\"S#get().\""], "the tightest span wins");

    // ...and the line that only the struct covers still resolves to the struct.
    let (rows, _) = both_in(store_fixture, r#"?- innermost_at("src/store.rs", 5, S)."#);
    assert_eq!(rows, vec!["\"S#\""]);

    // Containment closure. Ordering is by atom — dictionary order, which is
    // the order the facts above interned their strings in, not lexicographic
    // order.
    let (rows, _) = both_in(store_fixture, r#"?- within("S#get().", A)."#);
    assert_eq!(rows, vec!["\"src/store.rs\"", "\"S#\""]);

    // Orientation, one round trip.
    let (rows, _) = both_in(store_fixture, r#"?- about("S#get().", Rel, A, B, L)."#);
    assert!(
        rows.iter().any(|row| row.contains("\"parent\"")),
        "about reports the parent: {rows:?}"
    );

    // A span question, with arithmetic.
    let (rows, _) = both_in(store_fixture, "?- long_def(S).");
    assert!(rows.is_empty(), "no definition here is over 80 lines");
}

/// The combinations `specs/03-datalog.md` § Validation 5 promises and the
/// two-rule programs above cannot reach: a seeded ground negation, and the
/// negation-inside-recursion back-off that `impact_of` takes. Each is run
/// against naive evaluation of the library as written, so a rewrite that drops
/// a row is a disagreement, not a fast wrong answer.
#[test]
fn the_seeded_stdlib_traversals_agree_with_naive_evaluation() {
    // Seeded ground negation: `!tighter_at(F, L, S)` with every column bound.
    let (rows, result) = both_in(store_fixture, r#"?- innermost_at("src/store.rs", 15, S)."#);
    assert_eq!(rows, vec!["\"S#get().\""]);
    assert!(
        result
            .stats
            .transformed
            .iter()
            .any(|n| n == "tighter_at@bbb"),
        "{:?}",
        result.stats.transformed
    );

    // Negation + demand + recursion: seeding `!ambiguous` makes the rewrite
    // unstratifiable, the engine backs it off, and the answer must survive.
    let (rows, result) = both_in(store_fixture, r#"?- def(S, _, _, "get"), impact_of(S, C)."#);
    assert_eq!(rows, vec!["\"S#get().\" \"S#put().\""], "put calls get");
    assert!(
        result.stats.transformed.iter().any(|n| n == "impact_of@bf"),
        "{:?}",
        result.stats.transformed
    );
    assert!(
        !result
            .stats
            .transformed
            .iter()
            .any(|n| n.starts_with("ambiguous@")),
        "{:?}",
        result.stats.transformed
    );

    // Both seeds in one query, which is where the back-off has to be per
    // predicate to keep either.
    let (rows, _) = both_in(
        store_fixture,
        r#"?- innermost_at("src/store.rs", 35, S), impact_of(S, C)."#,
    );
    assert_eq!(rows, vec![] as Vec<String>, "nothing calls put");
    let (rows, _) = both_in(store_fixture, r#"?- reach_of("S#put().", C)."#);
    assert_eq!(rows, vec!["\"S#get().\""]);
}

/// Demand transformation must apply **against the shipped rule library**, not
/// only against a two-rule test program.
///
/// This is the gate `docs/plan.md` M1 asked for, widened by what M2 found: the
/// transformation is validated after rewriting and falls back to the plain
/// program when the rewrite does not stratify, so a rewrite that stops applying
/// is silent — the query still answers, it just answers by computing all-pairs
/// reachability first. On a real repository that is a timeout rather than a
/// wrong answer, which is why nothing caught it until there was a real
/// repository.
#[test]
fn demand_transformation_applies_over_the_standard_library() {
    let mut e = stdlib_engine();
    e.load_rules(codeintel::query::STDLIB)
        .expect("stdlib loads");

    for (query, wanted) in [
        (
            r#"?- innermost_at("src/store.rs", 15, S)."#,
            "innermost_at@bbf",
        ),
        (
            r#"?- def(S, _, _, "get"), impact_of(S, C)."#,
            "impact_of@bf",
        ),
        (
            r#"?- innermost_at("src/store.rs", 15, S), impact_of(S, C)."#,
            "impact_of@bf",
        ),
        // Both at once. Seeding `!tighter_at` is what breaks `impact_of`'s
        // stratification, so this is the query that proves the back-off is per
        // predicate and not per program: before it was, this pair fell all the
        // way back to the plain program and computed all-pairs reachability.
        (
            r#"?- innermost_at("src/store.rs", 15, S), impact_of(S, C)."#,
            "tighter_at@bbb",
        ),
        (r#"?- reach_of("S#get().", C)."#, "reach_of@bf"),
    ] {
        let result = e.query(query, &Limits::default()).expect("answers");
        assert!(
            result.stats.transformed.iter().any(|n| n == wanted),
            "{query}\n  wanted {wanted} among {:?}",
            result.stats.transformed
        );
    }
}

/// Ground-negation seeding is what makes `innermost_at` cheap; backing it off
/// for `ambiguous` is what keeps `impact_of` stratifiable. Assert both, so
/// losing either is a test failure rather than a slow query.
#[test]
fn the_negation_backoff_keeps_what_it_can() {
    let mut e = stdlib_engine();
    e.load_rules(codeintel::query::STDLIB)
        .expect("stdlib loads");

    let seeded = e
        .query(
            r#"?- innermost_at("src/store.rs", 15, S)."#,
            &Limits::default(),
        )
        .expect("answers");
    assert!(
        seeded
            .stats
            .transformed
            .iter()
            .any(|n| n == "tighter_at@bbb"),
        "the negated literal was not seeded: {:?}",
        seeded.stats.transformed
    );

    let fell_back = e
        .query(
            r#"?- def(S, _, _, "get"), impact_of(S, C)."#,
            &Limits::default(),
        )
        .expect("answers");
    assert!(
        !fell_back
            .stats
            .transformed
            .iter()
            .any(|n| n.starts_with("ambiguous@")),
        "seeding a negated literal here makes the program unstratifiable: {:?}",
        fell_back.stats.transformed
    );
}

/// A repository's conformance rules do not spend the stratum budget.
///
/// `--rules` is the documented home for them, and each independent rule used to
/// cost one stratum on top of the standard library's own. The library spent 39
/// of `max_strata`'s 64, so **26 conformance rules made every query fail at
/// planning** — including queries touching none of them. The limit had already
/// been raised 32 -> 64 once for the same reason.
#[test]
fn a_hundred_conformance_rules_do_not_exhaust_max_strata() {
    let mut engine = store_fixture();
    let mut extra = String::new();
    for i in 0..100 {
        use std::fmt::Write as _;
        writeln!(
            extra,
            "%% conf{i}(S) -- an independent conformance rule.\n\
             conf{i}(S) :- def(S, F, \"function\", N)."
        )
        .expect("writing to a String cannot fail");
    }
    engine.load_rules(&extra).expect("the rules load");
    engine
        .query("?- conf0(S).", &Limits::default())
        .expect("a query touching one of them still plans");
}
