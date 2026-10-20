//! Tier A against `tests/fixtures/rust/`.
//!
//! The six guards of `specs/02-extraction.md` § Validation are in
//! [`common`], shared with every other language. What is here is what only
//! Rust can claim: that upstream's four collapsed kinds stay four, and that an
//! `impl` block owns its methods without being a definition itself.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::{Extracted, extract_tree, fixture};
use extract::LANGS;

/// Every kind `queries/rust/tags.scm` claims to emit.
const KINDS: &[&str] = &[
    "struct",
    "enum",
    "type",
    "typealias",
    "trait",
    "module",
    "macro",
    "function",
    "method",
    "constant",
    "variable",
    "field",
];

fn root() -> PathBuf {
    fixture("rust")
}

fn extracted() -> Extracted {
    extract_tree(&root())
}

#[test]
fn golden_facts() {
    common::assert_golden(&extracted());
}

#[test]
fn spans_are_exact() {
    common::assert_spans_exact(&extracted());
}

#[test]
fn kinds_stay_inside_the_vocabulary() {
    common::assert_kinds_closed(&extracted());
}

#[test]
fn every_parent_names_something_that_exists() {
    common::assert_parents_exist(&extracted());
}

#[test]
fn every_pattern_matches_at_least_once() {
    let extracted = extracted();
    common::assert_kinds_present(&extracted, KINDS);
    assert!(
        extracted
            .rows("import")
            .iter()
            .any(|r| r.get(2).is_some_and(|alias| !alias.is_empty())),
        "the aliased-import pattern never matched"
    );
}

#[test]
fn no_node_takes_two_definition_captures() {
    common::assert_captures_disjoint(&root());
}

#[test]
fn facts_are_a_function_of_their_own_file() {
    common::assert_facts_are_local(&extracted());
}

#[test]
fn an_import_module_has_no_quotes() {
    common::assert_modules_undelimited(&extracted());
}

#[test]
fn a_second_extraction_is_byte_identical() {
    common::assert_deterministic(&root());
}

#[test]
fn input_order_does_not_change_the_facts() {
    common::assert_order_independent(&root());
}

#[test]
fn kinds_are_faithful() {
    // The test upstream's query fails: struct, enum, union, type alias, trait,
    // const and impl must not collapse into one kind.
    let extracted = extracted();
    let by_name: BTreeMap<String, String> = extracted
        .rows_in("def", "src/kinds.rs")
        .into_iter()
        .filter_map(|r| Some((r.get(3)?.clone(), r.get(2)?.clone())))
        .collect();

    for (name, kind) in [
        ("Config", "struct"),
        ("Mode", "enum"),
        ("Word", "type"),
        ("Key", "typealias"),
        ("Handler", "trait"),
        ("LIMIT", "constant"),
        ("BANNER", "variable"),
        ("handle", "method"),
        ("limit", "field"),
        ("Fast", "constant"),
        ("shout", "macro"),
    ] {
        assert_eq!(
            by_name.get(name).map(String::as_str),
            Some(kind),
            "{name} should be a {kind}"
        );
    }
}

#[test]
fn an_impl_block_owns_its_methods() {
    // Upstream captures `impl_item` as a reference, which parents every method
    // to the file. Ours makes it a scope, so `parent` is the type.
    let extracted = extracted();
    let parents = extracted.pairs("parent");
    assert_eq!(
        parents
            .get("local src/store.rs Store#get().")
            .map(String::as_str),
        Some("local src/store.rs Store#"),
    );
    assert_eq!(
        parents
            .get("local src/store.rs detail/helper().")
            .map(String::as_str),
        Some("local src/store.rs detail/"),
    );
    assert_eq!(
        parents.get("local src/store.rs Store#").map(String::as_str),
        Some("src/store.rs"),
    );
    // And the impl block itself defined nothing.
    assert!(
        !extracted
            .rows("def")
            .iter()
            .any(|r| r.get(3).map(String::as_str) == Some("Store")
                && r.get(2).map(String::as_str) == Some("class")),
        "an impl block is not a definition"
    );
}

#[test]
fn an_inner_attribute_stays_out_of_the_first_definition_span() {
    // `#![allow(dead_code)]` belongs to the crate, not to the struct written
    // under it. Absorbing it made `innermost_at(F, 1, S)` answer `A`, and an
    // edit built on that span deleted the crate attribute.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "#![allow(dead_code)]\npub struct A;\n",
    )
    .expect("writes");
    let extracted = extract_tree(dir.path());

    let span = extracted
        .rows("def_span")
        .into_iter()
        .find(|r| r.first().is_some_and(|s| s.ends_with("A#")))
        .expect("the struct has a span");
    assert_eq!(span.get(1).map(String::as_str), Some("2"), "{span:?}");
    assert_eq!(span.get(3).map(String::as_str), Some("21"), "{span:?}");
}

#[test]
fn a_brace_list_is_one_import_row_with_no_alias() {
    // `01-facts.md` § `import`: one row per statement, the specifier as
    // written, `Alias` only for a top-level `as`. Splitting the list would
    // make the extractor decide what a path means, and an inner `as` is not
    // the statement's binding.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "use a::{b, c as d};\nuse e::f as g;\n",
    )
    .expect("writes");
    let extracted = extract_tree(dir.path());

    let imports: Vec<(String, String)> = extracted
        .rows("import")
        .into_iter()
        .map(|r| (r[1].clone(), r[2].clone()))
        .collect();
    assert_eq!(
        imports,
        [
            ("a::{b, c as d}".to_string(), String::new()),
            ("e::f".to_string(), "g".to_string()),
        ]
    );
}

#[test]
fn two_trait_impls_for_one_type_give_their_methods_two_symbols() {
    // `impl Display for A` and `impl Debug for A` both define `fmt`. One
    // symbol for both merged their callers and gave `def_span` two rows for
    // one S; the trait now qualifies the member (01-facts.md § Symbol
    // identity).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub struct A;\n\
         impl std::fmt::Display for A { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) } }\n\
         impl std::fmt::Debug   for A { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) } }\n\
         pub trait T { fn go(&self); }\n\
         impl T for A { fn go(&self) {} }\n\
         impl T for &A { fn go(&self) {} }\n",
    )
    .expect("writes");
    let extracted = extract_tree(dir.path());

    let mut fmts: Vec<Vec<String>> = extracted
        .rows("def")
        .into_iter()
        .filter(|r| r.get(3).map(String::as_str) == Some("fmt"))
        .collect();
    fmts.sort();
    assert_eq!(
        fmts.iter().map(|r| r[0].as_str()).collect::<Vec<_>>(),
        [
            "local lib.rs A#[Debug]fmt().",
            "local lib.rs A#[Display]fmt().",
        ]
    );
    assert!(fmts.iter().all(|r| r[2] == "method"), "{fmts:?}");
    let parents = extracted.pairs("parent");
    for row in &fmts {
        assert_eq!(
            parents.get(&row[0]).map(String::as_str),
            Some("local lib.rs A#")
        );
    }

    let mut gos: Vec<String> = extracted
        .rows("def")
        .into_iter()
        .filter(|r| r.get(3).map(String::as_str) == Some("go"))
        .map(|r| r[0].clone())
        .collect();
    gos.sort();
    assert_eq!(
        gos,
        [
            "local lib.rs A#[T][`&A`]go().",
            "local lib.rs A#[T]go().",
            "local lib.rs T#go().",
        ]
    );
}

// `assert_spans_exact` has to fail on the spans it exists to catch. The old
// check was containment: the span holds the name and the line at `def_name`
// starts with it. Each case below passes that and is wrong, so the re-parse is
// shown to see what containment could not (`specs/02-extraction.md`
// § Validation 2).

const SPAN_SRC: &str =
    "/// Gets one.\npub fn get() -> u8 {\n    1\n}\n\npub fn getter() -> u8 {\n    2\n}\n";

fn rust_lang() -> &'static extract::lang::Lang {
    LANGS
        .iter()
        .find(|l| l.name == "rust")
        .expect("rust is registered")
}

/// What `assert_spans_exact` checked before: the span holds the name, and the
/// line at `def_name` starts with it.
fn containment(
    src: &str,
    (start, end): (usize, usize),
    name: &str,
    (line, col): (usize, usize),
) -> bool {
    src.get(start..end).is_some_and(|t| t.contains(name))
        && src
            .lines()
            .nth(line - 1)
            .and_then(|l| l.get(col..))
            .is_some_and(|t| t.starts_with(name))
}

const GET: (usize, usize) = (0, 42);
const GET_NAME: (usize, usize) = (2, 7);

#[test]
fn the_recorded_span_of_get_is_exact() {
    assert_eq!(
        SPAN_SRC.get(GET.0..GET.1),
        Some("/// Gets one.\npub fn get() -> u8 {\n    1\n}")
    );
    assert_eq!(
        common::span_defect(SPAN_SRC, rust_lang(), GET, "get", GET_NAME),
        None
    );
}

/// Each: (what is wrong, the span, the name's position, the name).
#[test]
fn spans_that_pass_containment_fail_the_reparse() {
    let swallowed_neighbour = (0, 75);
    let ends_mid_item = (0, 36);
    let starts_mid_item = (6, 42);
    let cases = [
        (
            "swallows the next item",
            swallowed_neighbour,
            "get",
            GET_NAME,
        ),
        ("ends inside the body", ends_mid_item, "get", GET_NAME),
        (
            "starts inside the doc comment",
            starts_mid_item,
            "get",
            GET_NAME,
        ),
        // `getter` starts with `get`, and `starts_with` cannot tell.
        ("names a longer identifier", (44, 75), "get", (6, 7)),
    ];
    for (what, span, name, at) in cases {
        assert!(
            containment(SPAN_SRC, span, name, at),
            "{what}: the case must pass the old check or it shows nothing"
        );
        assert!(
            common::span_defect(SPAN_SRC, rust_lang(), span, name, at).is_some(),
            "{what}: the re-parse accepted a span containment also accepts"
        );
    }
}
