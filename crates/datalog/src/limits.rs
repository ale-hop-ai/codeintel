//! Limits, truncation and statistics.
//!
//! Every cap has a documented default and names itself when it fires. A silent
//! cap reads as a complete answer, which is worse than an error
//! (`specs/00-overview.md` invariant 7).

/// Caps on one query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Rows returned before truncation. Default 1,000.
    pub max_result_rows: usize,
    /// Bytes of rendered output before truncation, at a row boundary.
    /// Default 262,144 — rows are not uniformly sized and the consumer is a
    /// context window.
    pub max_result_bytes: usize,
    /// Tuples derived across all strata before the query aborts.
    /// Default 10,000,000.
    pub max_derived_tuples: u64,
    /// Wall-clock budget in milliseconds. Default 5,000.
    pub max_time_ms: u64,
    /// Strata accepted at planning. Default 64.
    ///
    /// The spec said 32. `rules/stdlib.dl` alone stratifies into 37, so that
    /// default rejected every query against the shipped standard library — the
    /// limit was set against an imagined rule set rather than the real one.
    pub max_strata: usize,
    /// Literals in one rule or query body, accepted at planning. Default 32.
    pub max_body_literals: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_result_rows: 1_000,
            max_result_bytes: 262_144,
            max_derived_tuples: 10_000_000,
            max_time_ms: 5_000,
            max_strata: 64,
            max_body_literals: 32,
        }
    }
}

/// The cap that trimmed an answer.
///
/// Two values, because only two limits trim. `max_derived_tuples` and
/// `max_time_ms` abort the query with a diagnostic rather than cut an answer
/// short, so they never appear here — an earlier list of cap *names* included
/// them and told a consumer to switch on values it would never see.
///
/// An enum and not a `&'static str`. The host branches on which cap fired to
/// decide what advice to give — `--limit` moves `max_result_rows` and nothing
/// else, so offering it against a byte cap is advice that changes nothing
/// (#27) — and it did that by matching on a string literal, where a typo
/// falls through to the general arm and silently gives the wrong advice. The
/// wire form is unchanged: [`Cap::as_str`] is what is serialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// `max_result_rows` fired: the engine stopped adding rows.
    Rows,
    /// `max_result_bytes` fired: the rendered answer reached its budget.
    Bytes,
}

impl Cap {
    /// Both of them, for a consumer enumerating what it may be told.
    pub const ALL: [Self; 2] = [Self::Rows, Self::Bytes];

    /// Its wire name — the limit field it corresponds to, spelled exactly as
    /// [`Limits`] spells it, so a consumer can look the number up.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rows => "max_result_rows",
            Self::Bytes => "max_result_bytes",
        }
    }
}

impl std::fmt::Display for Cap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the demand transformation did, and why.
///
/// A closed set of outcomes. It was a `String` built from `&'static str`
/// literals at six sites, which meant a consumer wanting to branch on it —
/// and `tests/counters.rs` pins it, so there is one — had to `starts_with`
/// or `contains` a sentence. The sentences are unchanged: [`Demand::as_str`]
/// returns exactly what was written before, so a pinned golden does not move.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Demand {
    /// The rewrite ran and `Stats::transformed` names what it rewrote.
    Applied,
    /// The caller asked for the program as written.
    #[default]
    Disabled,
    /// No derived literal had a bound argument, so there was nothing to seed.
    /// Not a failure — the common case for an unseeded goal.
    NothingToSeed,
    /// The rewritten program did not pass the safety check.
    Unsafe,
    /// The rewritten program was over the planning limits the plain one met.
    OverLimits,
    /// The rewrite did not stratify and named no cycle to back off from.
    NoCycle,
    /// The rewrite did not stratify and every seeded negation was already
    /// excluded.
    NoBackoff,
    /// Backing off one component at a time ran out of attempts.
    Exhausted,
}

impl Demand {
    /// Its wire form, **verbatim as it has always been written**. These
    /// strings are a pinned golden (`crates/codeintel/tests/counters.rs`) and
    /// are asserted by prefix in the conformance suite; changing one is a
    /// golden change, not a wording change.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Disabled => "disabled",
            Self::NothingToSeed => "nothing to seed: no derived literal has a bound argument",
            Self::Unsafe => "rewrite failed the safety check; the program ran as written",
            Self::OverLimits => "rewrite exceeded the planning limits; the program ran as written",
            Self::NoCycle => {
                "rewrite did not stratify and named no cycle; the program ran as written"
            }
            Self::NoBackoff => {
                "rewrite did not stratify and no seeded negation was left to back off"
            }
            Self::Exhausted => "rewrite backed off too many times; the program ran as written",
        }
    }
}

impl std::fmt::Display for Demand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one evaluation cost. Not part of the answer, so timings here do not
/// break the byte-identical-output invariant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stats {
    /// Tuples derived across every stratum, including duplicates.
    pub derived: u64,
    /// Wall-clock milliseconds.
    pub elapsed_ms: u64,
    /// How many strata ran.
    pub strata: usize,
    /// The literal order chosen per rule, as `head/arity: lit, lit, ...`.
    ///
    /// Bounded. A plan that hit the bound ends with a `(plan truncated at N
    /// rules)` entry rather than simply stopping.
    pub plan: Vec<String>,
    /// Predicates the demand transformation rewrote, so an unexpectedly slow
    /// query can be diagnosed rather than guessed at.
    pub transformed: Vec<String>,
    /// Why `transformed` holds what it holds. An empty `transformed` alone
    /// conflated nothing-to-seed with a rewrite that was tried and dropped,
    /// and the second is the one that shows up as an unexplained timeout.
    pub demand: Demand,
    /// Stdlib rules a query-local rule shadowed. Never silent.
    pub shadowed: Vec<String>,
    /// When the goal derived no rows, the body literal — quoted as the caller
    /// wrote it — that matched nothing first.
    ///
    /// A valid query returning zero rows is indistinguishable from a typo, a
    /// wrong constant, and a base relation the host never populated.
    /// Evaluation already walks the body literal by literal, so the deepest
    /// position it reached is free, and the literal at that position is the one
    /// that stopped the join (`specs/05-surface.md` § Response contract).
    pub empty_at: Option<String>,
    /// The **base** relations the goal's dependency closure reaches, sorted.
    ///
    /// The closure, not the literal syntax of the goal. A caller that must know
    /// whether an answer depends on a relation it could not populate cannot
    /// get that from the query text: a goal over a derived predicate mentions
    /// no base relation at all, and answering `ok` with zero rows would be a
    /// lie it has no way to detect.
    pub depends: Vec<String>,
}
