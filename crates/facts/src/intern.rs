//! The string interner: `&str` ↔ `Atom(u32)`, append-only.
//!
//! An id is stable for the life of an index, so a segment never needs
//! rewriting when the dictionary grows. Reclaiming dead strings is
//! `codeintel index --rebuild` and nothing else; there is no incremental GC
//! (`specs/04-storage.md` § Interner).
//!
//! Id space, from `specs/01-facts.md` § Integers:
//!
//! ```text
//! 0x0000_0000 ..= 0x0FFF_FFFF   integers, encoded as themselves
//! 0x1000_0000 ..= 0xFFFF_FFFF   deduplicated strings; 0x1000_0000 is ""
//! ```

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Error, Result, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use datalog::atom::{Atom, STR_MIN};
use datalog::symbols::Symbols;

/// File holding the concatenated UTF-8, with no separators.
const BIN: &str = "dict.bin";

/// File holding `n + 1` little-endian `u64` start offsets into [`BIN`].
///
/// `u64`, not `u32`: `u32` offsets silently cap the dictionary at 4 GiB and
/// wrap to garbage strings on breach. Four extra bytes per entry is ~10 MB on a
/// 2.5M-string dictionary, and that is the whole cost.
const IDX: &str = "dict.idx";

/// A dictionary on disk, read into memory.
///
/// Read rather than mapped. The mapping bought nothing measurable: `validate`
/// scans every byte before `open` returns, and `by_text` (eagerly, then) copied
/// every string to the heap, so both of an mmap's advantages — paying only for
/// what is touched, and sharing pages — were forfeited before the first lookup.
/// `by_text` is lazy now (#28); `validate` still reads every byte, on purpose.
/// `Store::load` already reads the larger and hotter segment data with
/// `fs::read`; the dictionary was the small path, and the only thing holding
/// an `unsafe` block and a dependency (#44).
#[derive(Debug)]
pub struct Dict {
    bin: Vec<u8>,
    /// Bytes of `bin` a reader may trust. The file can run past this after a
    /// torn append; nothing past it is ever read.
    bin_len: usize,
    /// Start offsets, `len() + 1` of them; entry `i` spans `starts[i]..starts[i + 1]`.
    starts: Vec<u64>,
}

impl Dict {
    /// Open the dictionary in `dir`, or `None` when there is not one yet.
    ///
    /// # Errors
    /// I/O failure, or a dictionary whose index is malformed — a truncated
    /// offset table, an offset past the end of the data, or a span that is not
    /// UTF-8. Each is reported rather than read through.
    pub fn open(dir: &Path) -> Result<Option<Self>> {
        Self::open_within(dir, None)
    }

    /// Open the dictionary trusting only the first `bin_len` bytes of the data
    /// and `idx_len` bytes of the offset table — the extents the manifest
    /// recorded at its last commit.
    ///
    /// A crash mid-append leaves both files longer than any manifest says.
    /// Bounding the view to the recorded extents is the recovery
    /// (`specs/04-storage.md` § Manifest); the writer truncates the files to
    /// the same extents before it appends. Extents of zero mean no dictionary
    /// worth reading, whatever is on disk.
    ///
    /// # Errors
    /// As [`Self::open`]. A file *shorter* than its extent, or missing while the
    /// extents are not zero, is corrupt: reading either as no dictionary would
    /// start the id space over and hand every segment's atoms to new strings.
    pub fn open_to(dir: &Path, bin_len: u64, idx_len: u64) -> Result<Option<Self>> {
        if idx_len == 0 {
            return Ok(None);
        }
        Self::open_within(dir, Some((bin_len, idx_len)))
    }

    /// Open against the recorded `(bin, idx)` extents, or against whatever is
    /// on disk when there are none.
    fn open_within(dir: &Path, extents: Option<(u64, u64)>) -> Result<Option<Self>> {
        let (bin_path, idx_path) = (dir.join(BIN), dir.join(IDX));
        for (path, name) in [(&bin_path, BIN), (&idx_path, IDX)] {
            if !path.exists() {
                return match extents {
                    None => Ok(None),
                    Some((bin_len, idx_len)) => Err(corrupt(format!(
                        "{name} is missing, but the manifest records {bin_len} bytes of {BIN} \
                         and {idx_len} of {IDX}"
                    ))),
                };
            }
        }
        let bin = std::fs::read(&bin_path)?;
        let idx = std::fs::read(&idx_path)?;
        let (bin_len, idx_len) = match extents {
            None => (bin.len(), idx.len()),
            Some((bin_len, idx_len)) => (
                recorded(bin_len, bin.len(), BIN)?,
                recorded(idx_len, idx.len(), IDX)?,
            ),
        };
        if idx_len % 8 != 0 {
            return Err(corrupt(format!(
                "{IDX} is {idx_len} bytes, which is not a whole number of u64 offsets"
            )));
        }
        let (offsets, _) = idx.get(..idx_len).unwrap_or_default().as_chunks::<8>();
        let starts: Vec<u64> = offsets.iter().copied().map(u64::from_le_bytes).collect();
        let dict = Self {
            bin,
            bin_len,
            starts,
        };
        dict.validate()?;
        Ok(Some(dict))
    }

    /// Every span is inside the data, ascending, and UTF-8.
    ///
    /// Checked once at open rather than per lookup: the index is a cache, and a
    /// cache that has been truncated should say so on the first query instead
    /// of handing back a different string than it stored.
    fn validate(&self) -> Result<()> {
        let total = self.bin_len as u64;
        let mut previous = 0u64;
        for (i, start) in self.starts.iter().enumerate() {
            if *start < previous {
                return Err(corrupt(format!("{IDX} offset {i} goes backwards")));
            }
            if *start > total {
                return Err(corrupt(format!(
                    "{IDX} offset {i} is {start}, past the {total} bytes of {BIN}"
                )));
            }
            previous = *start;
        }
        if self.starts.last().copied().unwrap_or(0) != total {
            return Err(corrupt(format!(
                "{IDX} ends at {} but {BIN} holds {total} bytes",
                self.starts.last().copied().unwrap_or(0)
            )));
        }
        for i in 0..self.len() {
            if self.resolve_index(i).is_none() {
                return Err(corrupt(format!("entry {i} of {BIN} is not UTF-8")));
            }
        }
        Ok(())
    }

    /// How many strings it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    /// True when it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes of string data a reader may trust.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bin_len
    }

    /// The `(dict.bin, dict.idx)` byte extents this view reads: what a
    /// manifest records so that the next reader trusts exactly this much.
    #[must_use]
    pub fn extents(&self) -> (u64, u64) {
        (self.bin_len as u64, self.starts.len() as u64 * 8)
    }

    /// The string behind a string atom.
    #[must_use]
    pub fn resolve(&self, atom: Atom) -> Option<&str> {
        self.resolve_index(usize::try_from(atom.checked_sub(STR_MIN)?).ok()?)
    }

    fn resolve_index(&self, i: usize) -> Option<&str> {
        let start = usize::try_from(*self.starts.get(i)?).ok()?;
        let end = usize::try_from(*self.starts.get(i + 1)?).ok()?;
        core::str::from_utf8(self.bin.get(..self.bin_len)?.get(start..end)?).ok()
    }

    /// Every string, in id order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &str> {
        (0..self.len()).map(|i| self.resolve_index(i).unwrap_or_default())
    }
}

/// A dictionary being built: what is on disk, plus what this run has added.
///
/// Interning a string only ever appends. [`Self::flush`] is the one thing that
/// touches disk, so a query that mentions a name the corpus does not have costs
/// an in-memory entry and nothing more.
#[derive(Debug)]
pub struct Interner {
    dir: PathBuf,
    dict: Option<Dict>,
    added: Vec<String>,
    /// Every string, on disk and pending, to its atom: the `intern`/`lookup`
    /// direction. Built on first use, not at open (#28): a warm query call
    /// opens a store, finds its manifest unchanged, and drops it, and building
    /// this map was 23.8 of the 27.7 ms that open cost on deno with nothing
    /// ever reading it. `OnceLock`, so `Interner` stays `Sync`.
    by_text: OnceLock<HashMap<String, Atom>>,
    /// Extents the files are cut back to before the next append, when this
    /// interner was opened against a manifest's record of them.
    trusted: Option<(u64, u64)>,
}

impl Interner {
    /// Open the dictionary in `dir`, creating an empty one in memory if there
    /// is none. The empty string is always atom `STR_MIN`.
    ///
    /// # Errors
    /// I/O failure, or a malformed dictionary; see [`Dict::open`].
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        Self::open_with(dir.into(), None)
    }

    /// Open the dictionary trusting only the recorded extents; see
    /// [`Dict::open_to`]. A flush first truncates both files to them.
    ///
    /// # Errors
    /// As [`Self::open`].
    pub fn open_to(dir: impl Into<PathBuf>, bin_len: u64, idx_len: u64) -> Result<Self> {
        Self::open_with(dir.into(), Some((bin_len, idx_len)))
    }

    fn open_with(dir: PathBuf, trusted: Option<(u64, u64)>) -> Result<Self> {
        let dict = match trusted {
            Some((bin_len, idx_len)) => Dict::open_to(&dir, bin_len, idx_len)?,
            None => Dict::open(&dir)?,
        };
        // Checked here, where it can still be an error: the map that would
        // have found it is built later, on a path that cannot fail.
        if let Some(d) = &dict
            && d.len()
                .checked_sub(1)
                .is_some_and(|last| atom_at(last).is_none())
        {
            return Err(corrupt("the dictionary is larger than the atom space"));
        }
        let mut out = Self {
            dir,
            dict,
            added: Vec::new(),
            by_text: OnceLock::new(),
            trusted,
        };
        // `01-facts.md`: the empty string is the missing-value atom, and it is
        // STR_MIN. Reserving it here means no caller has to remember to.
        if out.is_empty() {
            out.intern("");
        }
        Ok(out)
    }

    /// How many strings it holds, on disk and in memory together.
    #[must_use]
    pub fn len(&self) -> usize {
        self.dict.as_ref().map_or(0, Dict::len) + self.added.len()
    }

    /// True when it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many strings are not yet on disk.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.added.len()
    }

    /// The dictionary extents this interner vouches for: the recorded ones it
    /// was opened against, or what its last flush wrote — never the size of a
    /// file a crashed run left longer. `(0, 0)` while nothing is on disk.
    ///
    /// This, not a `stat`, is what [`crate::Store::commit`] records: a commit
    /// that interned nothing leaves a torn tail where it is, and its bytes are
    /// not trusted just because a later manifest was written
    /// (`specs/04-storage.md` § Manifest).
    #[must_use]
    pub fn extents(&self) -> (u64, u64) {
        self.dict.as_ref().map_or((0, 0), Dict::extents)
    }

    /// The atom for `s`, appending it if the dictionary does not have it.
    ///
    /// Returns `None` only when the atom space is exhausted — 3.8 billion
    /// distinct strings.
    pub fn intern(&mut self, s: &str) -> Option<Atom> {
        if let Some(atom) = self.lookup(s) {
            return Some(atom);
        }
        let atom = atom_at(self.len())?;
        self.added.push(s.to_string());
        // `lookup` above built the map, so this always finds it.
        if let Some(map) = self.by_text.get_mut() {
            map.insert(s.to_string(), atom);
        }
        Some(atom)
    }

    /// The atom for `s` if the dictionary already has it, without appending.
    #[must_use]
    pub fn lookup(&self, s: &str) -> Option<Atom> {
        self.map().get(s).copied()
    }

    /// The string-to-atom map, built from every string held on first use.
    fn map(&self) -> &HashMap<String, Atom> {
        self.by_text.get_or_init(|| {
            let on_disk = self.dict.iter().flat_map(Dict::iter);
            on_disk
                .chain(self.added.iter().map(String::as_str))
                .enumerate()
                // `open_with` checked that every index has an atom.
                .filter_map(|(i, text)| Some((text.to_string(), atom_at(i)?)))
                .collect()
        })
    }

    /// True once the string-to-atom map exists. For the test that pins it
    /// staying unbuilt on an open nothing asks of.
    #[cfg(test)]
    fn map_built(&self) -> bool {
        self.by_text.get().is_some()
    }

    /// The string behind a string atom.
    #[must_use]
    pub fn resolve(&self, atom: Atom) -> Option<&str> {
        if let Some(text) = self.dict.as_ref().and_then(|d| d.resolve(atom)) {
            return Some(text);
        }
        let index = usize::try_from(atom.checked_sub(STR_MIN)?).ok()?;
        let on_disk = self.dict.as_ref().map_or(0, Dict::len);
        self.added
            .get(index.checked_sub(on_disk)?)
            .map(String::as_str)
    }

    /// Append everything interned since the last flush, then reopen the map.
    ///
    /// Append, not rewrite: ids are stable for the life of the index, so the
    /// bytes already on disk are exactly the bytes that belong there.
    ///
    /// # Errors
    /// I/O failure while creating the directory, appending, or remapping.
    pub fn flush(&mut self) -> Result<()> {
        if self.added.is_empty() && self.dict.is_some() {
            return Ok(());
        }
        // The old view is held until the append succeeds, so a flush that
        // fails leaves this interner answering exactly as before: `len()`
        // still counts the strings on disk, and the new ones are still
        // pending. Dropping it first made every id after a failure collide
        // with one on disk. The cut in `append` never goes below what the view
        // reads, and Windows, where a mapped file cannot be cut, is out of
        // scope (`specs/04-storage.md` § Concurrency).
        let previous = self.dict.take();
        if let Err(e) = self.append() {
            self.dict = previous;
            return Err(e);
        }
        self.added.clear();
        self.trusted = None;
        self.dict = Dict::open(&self.dir)?;
        Ok(())
    }

    /// Cut both files back to the trusted extents, then append every pending
    /// string and `fsync`.
    fn append(&mut self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut bin = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(BIN))?;
        let mut idx = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(IDX))?;
        // A torn append from a crashed run is cut off here, so the new
        // strings start exactly where the manifest says the old ones end
        // (`specs/04-storage.md` § Manifest).
        if let Some((bin_len, idx_len)) = self.trusted {
            bin.set_len(bin_len)?;
            idx.set_len(idx_len)?;
        }
        let mut offset = bin.metadata()?.len();
        let fresh = idx.metadata()?.len() == 0;
        // Where a retry cuts back to if anything below fails, whether or not a
        // manifest ever recorded it: appending after this append's torn bytes
        // would put every later offset out of step.
        self.trusted = Some((offset, idx.metadata()?.len()));
        if fresh {
            // Entry `i` spans `starts[i]..starts[i + 1]`, so a dictionary of `n`
            // strings has `n + 1` offsets and the first one opens the file.
            idx.write_all(&offset.to_le_bytes())?;
        }
        // No truncation on the append path: the table's last entry is already
        // the end of the last string, which is exactly where the next one
        // starts. Rewriting it would stretch the previous string over the new
        // one — `alpha` and `beta` resolving to `alphabeta`, which is how this
        // was found.
        for text in &self.added {
            bin.write_all(text.as_bytes())?;
            offset += text.len() as u64;
            idx.write_all(&offset.to_le_bytes())?;
        }
        bin.flush()?;
        idx.flush()?;
        bin.sync_all()?;
        idx.sync_all()?;
        Ok(())
    }

    /// Where this dictionary lives.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Symbols for Interner {
    fn intern(&mut self, s: &str) -> Option<Atom> {
        Self::intern(self, s)
    }

    fn resolve(&self, a: Atom) -> Option<&str> {
        Self::resolve(self, a)
    }
}

/// The atom for the `i`th string, or `None` past the end of the atom space.
fn atom_at(i: usize) -> Option<Atom> {
    u32::try_from(i).ok()?.checked_add(STR_MIN)
}

/// A recorded extent as a length into a file of `len` bytes. A file longer
/// than its extent holds a torn append, which the view leaves out; a shorter
/// one has lost bytes the manifest vouched for.
fn recorded(extent: u64, len: usize, name: &str) -> Result<usize> {
    usize::try_from(extent)
        .ok()
        .filter(|n| *n <= len)
        .ok_or_else(|| {
            corrupt(format!(
                "the manifest records {extent} bytes of {name}, which holds {len}"
            ))
        })
}

fn corrupt(message: impl AsRef<str>) -> Error {
    crate::corrupt(&format!(
        "the string dictionary is corrupt: {}",
        message.as_ref()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dictionary on disk holding `words`, flushed, and its extents.
    fn on_disk(words: &[&str]) -> (tempfile::TempDir, (u64, u64)) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut interner = Interner::open(dir.path()).expect("opens");
        for w in words {
            interner.intern(w).expect("interns");
        }
        interner.flush().expect("flushes");
        let extents = interner.extents();
        (dir, extents)
    }

    /// #28: opening against a manifest builds no map. The warm no-change call
    /// opens, compares manifests and drops the store; the map was most of that
    /// open's cost and nothing read it.
    #[test]
    fn an_open_that_is_never_asked_builds_no_map() {
        let (dir, (bin, idx)) = on_disk(&["alpha", "beta"]);
        let interner = Interner::open_to(dir.path(), bin, idx).expect("opens");
        assert!(!interner.map_built());
        // Resolving goes the other way and does not need it either.
        assert_eq!(interner.resolve(STR_MIN + 1), Some("alpha"));
        assert!(!interner.map_built());
    }

    /// The lazy map answers exactly what the eager one did: every string on
    /// disk to its own atom, a pending string to the atom `intern` gave it,
    /// an unknown string to nothing — before and after a flush.
    #[test]
    fn the_lazy_map_agrees_with_the_dictionary() {
        let words = ["", "alpha", "beta", "gamma"];
        let (dir, (bin, idx)) = on_disk(&words[1..]);
        let mut interner = Interner::open_to(dir.path(), bin, idx).expect("opens");
        for (i, w) in words.iter().enumerate() {
            let atom = STR_MIN + u32::try_from(i).expect("small");
            assert_eq!(interner.lookup(w), Some(atom), "{w:?}");
            assert_eq!(interner.resolve(atom), Some(*w));
        }
        assert_eq!(interner.lookup("delta"), None);
        let delta = interner.intern("delta").expect("interns");
        assert_eq!(interner.intern("delta"), Some(delta), "interned once");
        assert_eq!(interner.lookup("delta"), Some(delta));
        interner.flush().expect("flushes");
        assert_eq!(interner.lookup("delta"), Some(delta), "after a flush");
        assert_eq!(interner.lookup("alpha"), Some(STR_MIN + 1));
    }
}
