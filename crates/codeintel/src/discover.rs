//! Finding an index that is not where the caller is standing.
//!
//! The store moved out of the tree (`specs/04-storage.md` § Location), so
//! "is there an index here" stopped being a directory listing and became a
//! blake3 of the canonical root path plus one `stat`. That makes asking the
//! question of a *set* of candidate roots cheap, and finding an index near a
//! root a bounded probe rather than a scan
//! (`specs/04-storage.md` § Discovery).
//!
//! The mapping only runs one way — `<label>-<hash>` cannot be read back into a
//! path — so nothing here enumerates the cache. Answering a question about one
//! directory by parsing every manifest on the machine would cost every
//! unrelated repository a JSON parse.

use std::path::{Path, PathBuf};

/// Ancestors probed before the search gives up, with or without a `.git`.
const MAX_UP: usize = 8;

/// Levels below the root the downward probe descends.
const MAX_DOWN: usize = 3;

/// Directories the downward probe visits before the cap is reported.
const MAX_DIRS: usize = 4_096;

/// Indexes named in a hint before the rest become a count.
const MAX_NAMED: usize = 3;

/// What a probe around a root with no index of its own found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Nearby {
    /// Roots above, nearest first.
    pub above: Vec<PathBuf>,
    /// Roots below, shallowest first and lexicographic within a depth.
    pub below: Vec<PathBuf>,
    /// True when `MAX_DOWN` or `MAX_DIRS` stopped the downward probe before it
    /// ran out of directories. Reported, never silent (invariant 5).
    pub bounded: bool,
}

impl Nearby {
    /// Nothing was looked for. What an explicit `--index-dir` gets: every
    /// candidate root would resolve to the one pinned directory, so the probe
    /// would answer "yes" for all of them.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// True when the probe found nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.above.is_empty() && self.below.is_empty()
    }

    /// The root to answer from, or `None` when the choice is not this tool's
    /// to make (`specs/05-surface.md` § The nearest index).
    ///
    /// An ancestor wins: its index covers every file under the requested root
    /// and more. A descendant is adopted only when it is the *only* one —
    /// picking between two siblings would be a guess, and this tool does not
    /// guess (invariant 1).
    #[must_use]
    pub fn adopt(&self) -> Option<&Path> {
        self.above.first().map_or_else(
            || match self.below.as_slice() {
                [only] => Some(only.as_path()),
                _ => None,
            },
            |nearest| Some(nearest.as_path()),
        )
    }

    /// The found roots as one clause, or `None` when there are none.
    ///
    /// The **root** is named, not its store directory: `--path` is what the
    /// caller types, and under `specs/04-storage.md` § Location the store
    /// directory is not somewhere they ever do.
    #[must_use]
    pub fn found(&self) -> Option<String> {
        let all: Vec<String> = self
            .above
            .iter()
            .chain(self.below.iter())
            .map(|p| p.display().to_string())
            .collect();
        let (named, rest) = all.split_at(all.len().min(MAX_NAMED));
        let lead = if named.len() == 1 {
            "found an index at"
        } else {
            "found indexes at"
        };
        match (named, rest.len()) {
            ([], _) => None,
            (_, 0) => Some(format!("{lead} {}", named.join(", "))),
            (_, more) => Some(format!("{lead} {} and {more} more", named.join(", "))),
        }
    }

    /// What bounded the downward probe, when something did.
    #[must_use]
    pub fn truncation(&self) -> Option<String> {
        self.bounded
            .then(|| format!("searched {MAX_DOWN} level(s) below, at most {MAX_DIRS} directories"))
    }
}

/// Probe for an index near `root`, which has none of its own.
///
/// `index_dir` is the caller's explicit store directory; when there is one,
/// nothing is probed (`specs/04-storage.md` § Discovery).
#[must_use]
pub fn around(root: &Path, index_dir: Option<&Path>) -> Nearby {
    if index_dir.is_some_and(|d| !d.as_os_str().is_empty()) {
        return Nearby::none();
    }
    let (below, bounded) = below(root);
    Nearby {
        above: above(root),
        below,
        bounded,
    }
}

/// True when `dir` has an index of its own.
///
/// One hash and one `stat`. `locate` creates nothing, so probing a directory
/// that will never be indexed leaves nothing behind.
fn indexed(dir: &Path) -> bool {
    facts::store::locate(dir, None)
        .is_ok_and(|store| store.join(facts::manifest::MANIFEST).is_file())
}

/// Indexed ancestors, nearest first.
///
/// The ascent stops at this tree's own repository root — the first directory
/// from `root` upward holding a `.git` entry, a file as well as a directory so
/// that a git worktree counts. That root is itself a candidate; nothing above
/// it is, and `MAX_UP` bounds the climb when there is no `.git` at all.
/// Unbounded, the ascent reaches `$HOME`, and one stray `codeintel index ~`
/// would then answer for every repository beneath it.
fn above(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut at = root;
    for _ in 0..MAX_UP {
        if at.join(".git").exists() {
            break;
        }
        let Some(parent) = at.parent() else { break };
        if indexed(parent) {
            found.push(parent.to_path_buf());
        }
        at = parent;
    }
    found
}

/// Indexed descendants, shallowest first and lexicographic within a depth,
/// and whether a bound stopped the search short.
fn below(root: &Path) -> (Vec<PathBuf>, bool) {
    let (candidates, bounded) = extract::walk::dirs(root, MAX_DOWN, MAX_DIRS);
    let found = candidates
        .into_iter()
        .filter(|(_, dir)| indexed(dir))
        .map(|(_, dir)| dir)
        .collect();
    (found, bounded)
}

/// The `no-index` hint: one complete line that leaves nothing to look up
/// (`specs/05-surface.md` § The no-index hint).
///
/// One command that builds both tiers: an agent told to build only the first
/// came back for `no-scip` on its next question, and that second round trip
/// is where agents were measured giving up (#59).
#[must_use]
pub fn no_index_hint(root: &Path, nearby: &Nearby) -> String {
    let mut parts = Vec::new();
    parts.push(format!("no index for {}", root.display()));
    if let Some(found) = nearby.found() {
        parts.push(found);
    } else if let Some(bound) = nearby.truncation() {
        // Only when nothing was found: a cap that changed no answer is noise,
        // a cap that may have hidden the answer is invariant 5.
        parts.push(bound);
    }
    format!("{}. {}", parts.join("; "), index_chain(root))
}

/// The hint an adopted answer carries: what moved, and how to stop it moving.
///
/// Prose beside the structural report in `root`, never instead of it: an agent
/// branches on the field, a human reads this.
#[must_use]
pub fn adopted_hint(requested: &Path, adopted: &Path) -> String {
    format!(
        "no index for {}; answered from the nearest index, at {} — every row is scoped to there, \
         not here. to answer for {} itself, {}",
        requested.display(),
        adopted.display(),
        requested.display(),
        index_chain(requested)
    )
}

/// `run: codeintel index <root>`: one command, both tiers, since `index` runs
/// the SCIP indexers itself (`specs/05-surface.md` § The no-index hint).
fn index_chain(root: &Path) -> String {
    format!(
        "run: codeintel index {}",
        shell_quote(&root.to_string_lossy())
    )
}

/// `word` as one POSIX shell word. Left bare when every byte is safe, so the
/// common line stays the line the spec shows; single-quoted otherwise, with an
/// embedded `'` written as `'\''`.
pub(crate) fn shell_quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+=:@%,".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_index_dir_disables_the_probe() {
        let pinned = PathBuf::from("/tmp/pinned");
        assert_eq!(
            around(Path::new("/tmp/whatever"), Some(&pinned)),
            Nearby::none()
        );
    }

    #[test]
    fn an_ancestor_beats_a_descendant() {
        let nearby = Nearby {
            above: vec![PathBuf::from("/repo")],
            below: vec![PathBuf::from("/repo/sub/daemon")],
            bounded: false,
        };
        assert_eq!(nearby.adopt(), Some(Path::new("/repo")));
    }

    #[test]
    fn one_descendant_is_adopted_and_two_are_not() {
        let one = Nearby {
            below: vec![PathBuf::from("/repo/daemon")],
            ..Nearby::default()
        };
        assert_eq!(one.adopt(), Some(Path::new("/repo/daemon")));

        let two = Nearby {
            below: vec![PathBuf::from("/repo/a"), PathBuf::from("/repo/b")],
            ..Nearby::default()
        };
        assert_eq!(two.adopt(), None, "two siblings are a choice, not a guess");
    }

    #[test]
    fn the_found_clause_names_roots_and_counts_the_rest() {
        let many = Nearby {
            below: (0..5).map(|i| PathBuf::from(format!("/r/{i}"))).collect(),
            ..Nearby::default()
        };
        let found = many.found().expect("five indexes were found");
        assert!(found.contains("/r/0"), "{found}");
        assert!(found.ends_with("and 2 more"), "{found}");
    }

    #[test]
    fn nothing_found_is_no_clause() {
        assert_eq!(Nearby::none().found(), None);
    }
}
