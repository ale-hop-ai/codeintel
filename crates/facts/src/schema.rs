//! The relation table: id, name, and arity for every base relation.
//!
//! It lives here as a compile-time constant rather than in the segment file
//! (`specs/04-storage.md` § Segment format), so a segment carries relation
//! *ids* and nothing else. A schema change bumps [`SCHEMA_VERSION`] and a
//! mismatch on open is `stale` with the reindex command — never a silent read
//! of rows against the wrong column meanings.
//!
//! Fifteen relations against a ceiling of sixteen
//! (`specs/00-overview.md` § Surface budget). `has_type` is deferred and
//! deliberately absent (`specs/01-facts.md`).
//!
//! Note what is **not** here: `exported` and `implements`. Both are derived in
//! `rules/stdlib.dl` over the narrower facts below — `visibility` and
//! `scip_impl`/`name_impl` — because whether a symbol is visible outside its
//! crate, or implements a trait named in another file, is a join and not a
//! token. That is invariant 3, and it is the same move `ref` made in v1.

/// Bumped whenever a relation is added, removed, or changes arity.
pub const SCHEMA_VERSION: u32 = 3;

/// One base relation.
///
/// Everything a consumer needs to *describe* this relation lives here, in one
/// row. It used to live in three places — arity here, argument names and a
/// note in `codeintel::schema`, SCIP-only-ness in a fourth hand-kept list in
/// `codeintel::query` — and parallel tables drift in one direction: toward the
/// one nobody remembered to edit. A SCIP-only relation left out of that list
/// answered `ok` with zero rows instead of `no-scip`, which is invariant 6
/// broken by an omission no compiler could see (#48).
///
/// Now it is a struct literal with no `..`, so a relation added without
/// deciding each of these does not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rel {
    /// Its id, as written into a segment. Stable for a given [`SCHEMA_VERSION`].
    pub id: u32,
    /// Its name, as written in Datalog.
    pub name: &'static str,
    /// How many columns it has.
    pub arity: usize,
    /// Its argument names, parenthesised, as `specs/01-facts.md` writes them.
    ///
    /// The names carry the meaning that [`Self::arity`] cannot: a column
    /// called `StartLine` holds a line number, and that is what lets a host
    /// diagnose a quoted integer. `arity_matches_the_argument_names` asserts
    /// the two agree.
    pub args: &'static str,
    /// One clause of agent-facing copy, or empty. Printed beside the signature
    /// in `schema`, which is the most-read output this project has.
    pub note: &'static str,
    /// True when **only** a SCIP index populates this relation.
    ///
    /// A goal whose dependency closure reaches one of these, against an index
    /// with no SCIP, is `no-scip` and not `ok` with zero rows — the difference
    /// between "your code does not do this" and "this index cannot see it"
    /// (`specs/05-surface.md` § Status taxonomy).
    pub scip_only: bool,
}

/// Every base relation, in id order. The index into this array *is* the id.
///
/// Order is the order of `specs/01-facts.md` § Base relations, which is also
/// roughly the order the extractor emits them in.
pub const RELATIONS: &[Rel] = &[
    Rel {
        id: 0,
        name: "file",
        arity: 2,
        args: "(F, Lang)",
        note: "",
        scip_only: false,
    },
    Rel {
        id: 1,
        name: "def",
        arity: 4,
        args: "(S, F, Kind, Name)",
        note: "",
        scip_only: false,
    },
    Rel {
        id: 2,
        name: "def_span",
        arity: 5,
        args: "(S, StartLine, EndLine, StartByte, EndByte)",
        note: "",
        scip_only: false,
    },
    Rel {
        id: 3,
        name: "def_name",
        arity: 3,
        args: "(S, Line, Col)",
        note: "the identifier token",
        scip_only: false,
    },
    Rel {
        id: 4,
        name: "def_sig",
        arity: 2,
        args: "(S, Sig)",
        note: "",
        scip_only: false,
    },
    Rel {
        id: 5,
        name: "def_doc",
        arity: 2,
        args: "(S, Doc)",
        note: "",
        scip_only: false,
    },
    Rel {
        id: 6,
        name: "parent",
        arity: 2,
        args: "(Child, Parent)",
        note: "one level; see within/2",
        scip_only: false,
    },
    Rel {
        id: 7,
        name: "visibility",
        arity: 2,
        args: "(S, Vis)",
        note: "public|restricted|inherited",
        scip_only: false,
    },
    Rel {
        id: 8,
        name: "resolved",
        arity: 1,
        args: "(S)",
        note: "a SCIP symbol anchored to this def",
        scip_only: true,
    },
    Rel {
        id: 9,
        name: "import",
        arity: 3,
        args: "(F, Module, Alias)",
        note: "",
        scip_only: false,
    },
    Rel {
        id: 10,
        name: "scip_ref",
        arity: 6,
        args: "(S, F, Line, Col, From, Role)",
        note: "compiler-resolved",
        scip_only: true,
    },
    Rel {
        id: 11,
        name: "name_ref",
        arity: 5,
        args: "(Name, F, Line, Col, From)",
        note: "unresolved, tier A",
        scip_only: false,
    },
    Rel {
        id: 12,
        name: "scip_impl",
        arity: 2,
        args: "(S, T)",
        note: "compiler-resolved",
        scip_only: true,
    },
    Rel {
        id: 13,
        name: "name_impl",
        arity: 4,
        args: "(F, Type, Trait, Line)",
        note: "unresolved, tier A",
        scip_only: false,
    },
    Rel {
        id: 14,
        name: "extern",
        arity: 4,
        args: "(S, Manager, Package, Version)",
        note: "",
        scip_only: true,
    },
    Rel {
        id: 15,
        name: "name_export",
        arity: 2,
        args: "(F, Name)",
        note: "export { x }, tier A",
        scip_only: false,
    },
];

/// The relation with this id.
#[must_use]
pub fn by_id(id: u32) -> Option<&'static Rel> {
    RELATIONS.get(usize::try_from(id).ok()?)
}

/// The relation with this name.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static Rel> {
    RELATIONS.iter().find(|r| r.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_indices() {
        for (i, rel) in RELATIONS.iter().enumerate() {
            assert_eq!(u32::try_from(i), Ok(rel.id), "{} is out of order", rel.name);
        }
    }

    #[test]
    fn names_are_unique_and_resolvable() {
        for rel in RELATIONS {
            assert_eq!(by_name(rel.name), Some(rel));
            assert_eq!(by_id(rel.id), Some(rel));
        }
        let mut names: Vec<&str> = RELATIONS.iter().map(|r| r.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate relation name");
    }

    #[test]
    fn the_fact_spec_declares_the_version_this_code_writes() {
        // `specs/01-facts.md` is the ABI, and its front matter carries the
        // schema version. It sat at 1 for the whole of v2 — the § Changelog
        // table in that same file already described v2, so the document
        // disagreed with itself and with this constant, and nothing noticed.
        // A version declared in prose and asserted nowhere is one that drifts.
        let spec = include_str!("../../../specs/01-facts.md");
        let declared = spec
            .lines()
            .find_map(|l| l.strip_prefix("schema_version:"))
            .map(str::trim)
            .expect("specs/01-facts.md front matter declares schema_version");
        assert_eq!(
            declared,
            SCHEMA_VERSION.to_string(),
            "specs/01-facts.md front matter says schema_version: {declared}, \
             facts::schema::SCHEMA_VERSION is {SCHEMA_VERSION}"
        );
    }

    #[test]
    fn arity_matches_the_argument_names() {
        // The two halves of one fact. `arity` is what a segment is decoded
        // against; `args` is what a reader is shown. A relation whose
        // signature says four columns and whose arity says five prints a
        // table that does not match the rows under it.
        for rel in RELATIONS {
            let args = rel
                .args
                .strip_prefix('(')
                .and_then(|a| a.strip_suffix(')'))
                .unwrap_or_else(|| panic!("{} args are not parenthesised: {}", rel.name, rel.args));
            let named = args.split(',').filter(|a| !a.trim().is_empty()).count();
            assert_eq!(named, rel.arity, "{} is {}", rel.name, rel.args);
        }
    }

    #[test]
    fn only_the_tier_b_relations_are_scip_only() {
        // Named rather than counted: this is the list a `no-scip` verdict is
        // read off, and it was once a hand-kept copy in another crate that
        // could silently fall behind this table (#48). Changing it is a
        // deliberate edit here, with `specs/05-surface.md` § Status taxonomy
        // to match.
        let scip_only: Vec<&str> = RELATIONS
            .iter()
            .filter(|r| r.scip_only)
            .map(|r| r.name)
            .collect();
        assert_eq!(scip_only, ["resolved", "scip_ref", "scip_impl", "extern"]);
    }

    #[test]
    fn the_base_relation_budget_holds() {
        // `specs/00-overview.md` § Surface budget: <= 16, currently 16. This is
        // one of the four numbers CI asserts; the budget is spent (#22), so the
        // next relation deletes one, and that is a spec change.
        assert!(RELATIONS.len() <= 16, "base relations: {}", RELATIONS.len());
        assert_eq!(RELATIONS.len(), 16);
    }
}
