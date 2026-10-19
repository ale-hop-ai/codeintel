//! Finding the files to index.
//!
//! `.gitignore`-aware via the `ignore` crate, and everything it drops is
//! summarized **by reason** (`specs/05-surface.md` § `index`): a silently
//! unindexed subtree is the single most confusing failure this tool can have.
//!
//! Skipped files produce **zero** facts — not even a `file` row — so
//! `!file(F, _)` means "not indexed", which is different from "indexed and
//! empty" (`specs/02-extraction.md` § Budget).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::lang::{self, Lang};

/// Largest file tier A will parse.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// A file worth extracting.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Repo-relative, `/`-separated. This is `F` in every relation.
    pub path: String,
    /// Where it actually is.
    pub abs: PathBuf,
    /// Its language.
    pub lang: &'static Lang,
    /// Its length in bytes.
    pub size: u64,
    /// Its mtime in seconds since the epoch.
    pub mtime: u64,
}

/// What the walk dropped, by reason.
///
/// There is no `ignored` count. The `ignore` crate does not report the entries
/// its matchers drop, and recovering the number means a second traversal with
/// the matchers off — which on a repo with a populated `target/` is the most
/// expensive thing in the walk, spent on a cosmetic figure.
/// `specs/05-surface.md` § `index` records the deviation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skips {
    /// Over [`MAX_BYTES`].
    pub too_large: usize,
    /// Extension with no registered grammar, counted per extension so the
    /// summary can say `88 unsupported (.scala)`.
    pub unsupported: BTreeMap<String, usize>,
    /// Paths the walker itself could not read.
    pub unreadable: Vec<String>,
}

impl Skips {
    /// How many files were skipped in total.
    #[must_use]
    pub fn total(&self) -> usize {
        self.too_large + self.unsupported.values().sum::<usize>() + self.unreadable.len()
    }
}

/// Everything one walk of the tree found.
#[derive(Debug, Clone, Default)]
pub struct Walked {
    /// The files worth extracting, sorted by path.
    pub files: Vec<Candidate>,
    /// Every file the walk reached, extractable or not — the repo-relative
    /// path of each entry the matchers let through, including the ones
    /// [`Skips`] then dropped for size or for having no grammar.
    ///
    /// This is the tree's membership list, and it is what tier B is checked
    /// against: a SCIP document naming a path that is not in here names a path
    /// `.gitignore` excluded, and indexing it would re-admit through tier B
    /// exactly what tier A was told to skip (`specs/02-extraction.md`
    /// § Budget).
    pub visible: BTreeSet<String>,
    /// What the walk dropped, by reason.
    pub skips: Skips,
}

/// Walk `root` and return the indexable files, sorted by path.
///
/// Sorted because `docs/plan.md` M2 requires that shuffling the file order
/// changes nothing, and the cheapest way to hold that is to not have an order
/// that depends on the filesystem in the first place.
#[must_use]
pub fn walk(root: &Path) -> Walked {
    let mut out = Vec::new();
    let mut visible = BTreeSet::new();
    let mut skips = Skips::default();
    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        // Honour `.gitignore` in a tree that is not a git checkout — a worktree,
        // an export, a vendored copy. The default refuses to, and the effect is
        // that `target/` silently becomes 40,000 indexed files.
        .require_git(false);
    // The store is derived and lives under the root; indexing it would be
    // indexing ourselves.
    walker.filter_entry(|e| e.file_name() != std::ffi::OsStr::new(".codeintel"));

    for entry in walker.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                skips.unreadable.push(e.to_string());
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(path) = relative(root, entry.path()) else {
            continue;
        };
        visible.insert(path.clone());
        let Some(lang) = lang::for_path(&path) else {
            let ext = path
                .rsplit_once('.')
                .map_or_else(|| "(none)".to_string(), |(_, ext)| format!(".{ext}"));
            *skips.unsupported.entry(ext).or_default() += 1;
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            skips.unreadable.push(path);
            continue;
        };
        if meta.len() > MAX_BYTES {
            skips.too_large += 1;
            continue;
        }
        out.push(Candidate {
            path,
            abs: entry.path().to_path_buf(),
            lang,
            size: meta.len(),
            mtime: mtime_of(&meta),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Walked {
        files: out,
        visible,
        skips,
    }
}

/// A path under `root`, `/`-separated.
#[must_use]
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for part in rel.components() {
        let part = part.as_os_str().to_str()?;
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    (!out.is_empty()).then_some(out)
}

/// An mtime in whole seconds since the epoch, or zero if the platform has none.
#[must_use]
pub fn mtime_of(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::create_dir_all(root.join("target")).expect("mkdir");
        std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").expect("write");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("README.md"), "# hi\n").expect("write");
        std::fs::write(root.join("notes.scala"), "object A\n").expect("write");
        std::fs::write(root.join("target/big.rs"), "fn ignored() {}\n").expect("write");
        std::fs::write(root.join(".gitignore"), "target/\n").expect("write");
        dir
    }

    #[test]
    fn the_walk_finds_source_and_counts_what_it_dropped() {
        let dir = fixture();
        let walked = walk(dir.path());
        let (found, skips) = (&walked.files, &walked.skips);
        let paths: Vec<&str> = found.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["src/lib.rs", "src/main.rs"]);
        assert_eq!(skips.unsupported.get(".md"), Some(&1));
        assert_eq!(skips.unsupported.get(".scala"), Some(&1));
        assert_eq!(skips.unsupported.get(".rs"), None, "target/ is gitignored");
    }

    #[test]
    fn a_file_over_the_cap_is_reported_not_parsed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let big = vec![b'\n'; usize::try_from(MAX_BYTES).unwrap_or(0) + 1];
        std::fs::write(dir.path().join("big.rs"), big).expect("write");
        let walked = walk(dir.path());
        assert!(walked.files.is_empty());
        assert_eq!(walked.skips.too_large, 1);
        assert_eq!(walked.skips.total(), 1);
        assert!(
            walked.visible.contains("big.rs"),
            "too large is still part of the tree"
        );
    }

    #[test]
    fn the_store_directory_is_not_indexed() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".codeintel/seg")).expect("mkdir");
        std::fs::write(dir.path().join(".codeintel/oops.rs"), "fn a() {}").expect("write");
        std::fs::write(dir.path().join("real.rs"), "fn b() {}").expect("write");
        let walked = walk(dir.path());
        assert_eq!(walked.files.len(), 1);
        assert_eq!(
            walked.files.first().map(|c| c.path.as_str()),
            Some("real.rs")
        );
        assert_eq!(
            walked.visible,
            ["real.rs".to_string()].into_iter().collect()
        );
    }

    /// #54, the tier-A half: the `ignore` crate is handed the tree root and
    /// discovers `.gitignore` at every level, so a wildcard directory segment,
    /// a nested file and a negation all behave as `git check-ignore` does.
    ///
    /// The leak in #54 was never here — it was tier B ingesting SCIP documents
    /// for paths this walk had already refused. This pins the half that was
    /// right so a future change cannot quietly make the reporter correct.
    #[test]
    fn gitignore_globs_are_matched_as_git_matches_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        for d in [
            "packages/ui/src",
            "packages/ui/build",
            "packages/ui/build/nested",
            "packages/api/build",
            "deep/a/b/build",
            "vendor",
        ] {
            std::fs::create_dir_all(root.join(d)).expect("mkdir");
        }
        std::fs::write(root.join(".gitignore"), "packages/*/build\nvendor/*.rs\n").expect("write");
        // Nested, and a negation that re-admits one file the parent excluded.
        // `vendor/*.rs`, not `vendor/`: git cannot re-include a file whose
        // parent *directory* is excluded, because it never descends to read
        // the nested rules.
        std::fs::write(root.join("vendor/.gitignore"), "!keep.rs\n").expect("write");
        for f in [
            "packages/ui/src/lib.rs",
            "packages/ui/build/out.rs",
            "packages/ui/build/nested/deeper.rs",
            "packages/api/build/out.rs",
            // `*` spans one segment, so `packages/*/build` does not reach this.
            "deep/a/b/build/kept.rs",
            "vendor/keep.rs",
            "vendor/drop.rs",
        ] {
            std::fs::write(root.join(f), "fn a() {}\n").expect("write");
        }

        let walked = walk(root);
        let paths: Vec<&str> = walked.files.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "deep/a/b/build/kept.rs",
                "packages/ui/src/lib.rs",
                "vendor/keep.rs",
            ]
        );
        assert_eq!(
            walked.visible,
            paths.iter().map(|p| (*p).to_string()).collect(),
            "an excluded path is not in the tree for tier B either"
        );
    }

    #[test]
    fn paths_are_slash_separated_and_relative() {
        let root = Path::new("/a/b");
        assert_eq!(
            relative(root, Path::new("/a/b/c/d.rs")).as_deref(),
            Some("c/d.rs")
        );
        assert_eq!(relative(root, Path::new("/a/b")), None);
        assert_eq!(relative(root, Path::new("/x/y.rs")), None);
    }
}
