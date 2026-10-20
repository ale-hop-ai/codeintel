//! `codeintel schema` — the text an agent reads to learn this system.
//!
//! The highest-leverage output in the project, and the only one with a hard
//! size budget: **~1500 tokens** (`specs/05-surface.md` § `schema`). If it does
//! not fit, the standard library is too big — cut rules, not the catalogue.
//!
//! Nothing here is declared twice. The rule list comes from `rules/stdlib.dl`,
//! the kinds and roles from `extract::lang`, the relations from
//! `facts::RELATIONS`, and the counts from the index in front of us. A
//! catalogue that restates any of those is a catalogue that can disagree with
//! them, and `schema` disagreeing with the index is invariant 5 broken by the
//! onboarding text itself.

use std::fmt;

use crate::census::Census;

/// One advertised rule: its head as written, and its doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// `innermost_at(F, Line, S)` — the head, argument names included, because
    /// argument *order* is what an agent gets wrong.
    pub head: String,
    /// The `%%` lines above it, joined.
    pub doc: String,
}

/// The advertised rules, parsed out of `stdlib.dl`.
///
/// The convention is a `%%` block above a predicate's first clause. The first
/// line carries the signature; any further `%%` lines continue the doc:
///
/// ```text
/// %% about(S, Rel, A, B, L)  all of S in one trip. Rel = sig|doc|defined|
/// %%   caller|callee|implements|implementor|test|extern
/// about(S, "sig", Sig, "", 0) :- def_sig(S, Sig).
/// ```
///
/// Continuation lines are **joined, not dropped**. A parser that demanded a
/// signature on every `%%` line silently truncated `about`'s value list at the
/// wrap, so the one relation designed to answer everything in one round trip
/// advertised half of what it answers.
///
/// The signature is written out rather than lifted from the clause because
/// heads carry constants — `ref(..., "exact")`, `about(S, "sig", ...)` — and
/// the argument names are what an agent needs.
/// `the_rule_list_matches_the_rules` pins the two together.
#[must_use]
pub fn rules(stdlib: &str) -> Vec<Rule> {
    let mut out: Vec<Rule> = Vec::new();
    for line in stdlib.lines() {
        let Some(text) = line.strip_prefix("%%") else {
            continue;
        };
        let text = text.trim();
        if let Some(close) = text.find(')') {
            let (head, doc) = text.split_at(close + 1);
            out.push(Rule {
                head: head.to_string(),
                doc: doc.trim().to_string(),
            });
        } else if let Some(rule) = out.last_mut() {
            // No signature on this line: a continuation of the block above it.
            rule.doc.push(' ');
            rule.doc.push_str(text);
        }
    }
    out
}

/// The predicate name and arity of an advertised head, for the drift test.
#[must_use]
pub fn split_head(head: &str) -> Option<(&str, usize)> {
    let (name, args) = head.split_once('(')?;
    let args = args.strip_suffix(')')?;
    Some((name, args.split(',').count()))
}

/// The schema for one index.
///
/// A `Display` rather than a function returning `String`: the whole output is
/// one formatting operation, every `write!` carries its own error, and a caller
/// that wants a `String` asks for `.to_string()`.
#[derive(Debug, Clone, Copy)]
pub struct Schema<'a> {
    /// The counts to print against the catalogue.
    pub census: &'a Census,
}

impl fmt::Display for Schema<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let census = self.census;
        f.write_str(
            "START HERE -- you have a location, you need a symbol\n  \
             ?- innermost_at(\"src/store.rs\", 142, S).           from ripgrep / git diff / a stack trace\n  \
             ?- def(S, F, _, N), contains(N, \"auth\").           from a word\n  \
             ?- def(S, _, _, \"Store\"), about(S, Rel, A, B, L).  everything about S, one trip\n\n\
             RELATIONS  base facts, extracted. Counts from THIS index.\n",
        )?;
        for rel in facts::RELATIONS {
            let (args, note) = signature(rel.name);
            let gap = if note.is_empty() { "" } else { "  " };
            writeln!(
                f,
                "  {:<11} {args} {}{gap}{note}",
                rel.name,
                census.rows(rel.name)
            )?;
        }

        f.write_str("\nVALUES  closed sets, counted here. A value at 0 returns no rows.\n  Kind ")?;
        for kind in extract::lang::KINDS {
            write!(f, " {kind} {}", census.kinds.get(*kind).unwrap_or(&0))?;
        }
        f.write_str("\n  Role ")?;
        for role in extract::lang::ROLES {
            write!(f, " {role} {}", census.roles.get(*role).unwrap_or(&0))?;
        }
        write!(
            f,
            "\n  Prov  exact {} (scip_ref rows)  name {} (name_ref rows)\n  Lang ",
            census.rows("scip_ref"),
            census.rows("name_ref")
        )?;
        if census.langs.is_empty() {
            f.write_str(" none indexed")?;
        }
        for (lang, counts) in &census.langs {
            write!(f, " {lang} {} files", counts.files)?;
        }

        f.write_str("\n\nRULES  derived. A new question is a rule, not a verb.\n")?;
        for rule in rules(crate::STDLIB) {
            writeln!(f, "  {}  {}", rule.head, rule.doc)?;
        }

        f.write_str(
            "\nBUILTINS  = != < <= > >= + - * / between/3 match/2 prefix/2 suffix/2\n            \
             contains/2 N = count{X:goal}\n\n\
             NOTES\n  \
             lines 1-based, columns 0-based UTF-8 bytes. < and > are integers only.\n  \
             an integer and its string differ: Line = \"42\" never matches. line 0 = n/a.\n  \
             count binds, it does not compare: `N = count{..}, N > 3`, never\n    \
             `count{..} > 3`. It groups by every variable bound OUTSIDE it.\n  \
             a symbol column prints `Name path:line`, not the SymId; --raw prints it.\n  \
             Prov \"name\" is text matching: method calls x.f() are mostly MISSING.\n    \
             For precision use the _exact rules; `codeintel status` says if SCIP\n    \
             is present and fresh.\n\n\
             EXAMPLES  more in docs/cookbook.md\n  \
             what does this diff hunk affect?\n  \
             ?- innermost_at(\"src/store.rs\", 142, S), impact_of(S, C),\n     \
             def(C, F, _, N), !is_test(F).\n\n  \
             no module under ui/ may import from db/ -- needs no SCIP index\n  \
             ?- import(F, M, _), prefix(F, \"src/ui/\"), contains(M, \"db\").\n",
        )?;

        if !census.indexed {
            f.write_str(
                "\nTHERE IS NO INDEX HERE. Every count above is 0 because nothing has been\n\
                 indexed, not because your code has none of these. run: codeintel index .\n",
            )?;
        }
        Ok(())
    }
}

/// The argument names for a base relation and its note, from
/// [`facts::RELATIONS`].
///
/// This was a second hand-written table, keyed by name, living in a different
/// crate from the arities it had to agree with (#48). It is now a lookup: one
/// row per relation, in the crate that owns the schema, where a new relation
/// cannot be added without filling both fields in. An unknown name still
/// answers `("", "")` — callers pass relation names from that same table, so
/// the arm is unreachable in practice and present so this is total.
#[must_use]
pub fn signature(name: &str) -> (&'static str, &'static str) {
    facts::schema::by_name(name).map_or(("", ""), |rel| (rel.args, rel.note))
}

/// What a base relation's column accepts, for diagnosing a constant that
/// matched nothing (`specs/05-surface.md` § `query`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// Integers. A quoted constant here can never match: an integer and its
    /// string form are different atoms.
    Int,
    /// A closed set of strings, listed with this index's counts.
    Vocab(&'static [&'static str]),
    /// A repo-relative file path. The most common constant an agent writes,
    /// because it arrives from `ripgrep`, `git diff` and stack traces — which
    /// are exactly the sources that produce absolute and `./`-prefixed forms.
    Path,
    /// Anything — a symbol id, a name. Nothing useful to say.
    Free,
}

/// Provenance is two values and nothing else, so it is written here rather than
/// reached for through a tier module that has no reason to expose it.
const PROVS: &[&str] = &["exact", "name"];

/// The column types of `relation`, positionally.
///
/// Derived from the argument *names* rather than declared as a fourth table —
/// `every_typed_column_is_reachable` asserts every classified name actually
/// occurs. Classification is by name because the names are already consistent
/// and meaningful: a column called `StartLine` holds a line number in every
/// relation that has one.
#[must_use]
pub fn columns(relation: &str) -> Vec<Column> {
    let (args, _) = signature(relation);
    args.trim_matches(['(', ')'])
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(|arg| match arg {
            "Line" | "Col" | "StartLine" | "EndLine" | "StartByte" | "EndByte" => Column::Int,
            "F" | "G" => Column::Path,
            "Kind" => Column::Vocab(extract::lang::KINDS),
            "Role" => Column::Vocab(extract::lang::ROLES),
            "Prov" => Column::Vocab(PROVS),
            _ => Column::Free,
        })
        .collect()
}
