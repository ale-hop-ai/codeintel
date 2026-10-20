//! The tier-A validation suite, once, for every language that has a fixture.
//!
//! `specs/02-extraction.md` § Validation lists six mechanical guards: golden
//! facts, span exactness, kind fidelity, query liveness, locality, and
//! determinism. Five of the six are the same assertion whatever the language,
//! so they live here and each language's test file supplies its fixture and
//! the claims only that language can make.
//!
//! Each guard catches a failure that is otherwise invisible. A dead query
//! pattern shows up as `status: ok` with zero rows; a cross-file lookup in the
//! extractor shows up as a wrong answer weeks later.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use datalog::atom::is_int;
use extract::lang::{self, KINDS};
use extract::scip::Ingest;
use extract::{Anchors, Extractor, tier_b, walk};
use facts::{Interner, RELATIONS, Segment};
use protobuf::Message as _;
use scip::types::Index;

/// Where a fixture tree lives, relative to this crate.
pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// A whole fixture, extracted file by file into one dictionary.
pub(crate) struct Extracted {
    root: PathBuf,
    interner: Interner,
    segments: BTreeMap<String, Segment>,
    _dir: tempfile::TempDir,
}

/// Extract every file under `root` that has a registered language.
pub(crate) fn extract_tree(root: &Path) -> Extracted {
    extract_in(root, &Order::default())
}

/// How [`extract_in`] orders its inputs. The default is the walk's order and the
/// index as written — the order the indexer itself uses, which sorts.
#[derive(Default)]
pub(crate) struct Order {
    /// Permute the files with this seed before extracting them.
    pub files: Option<u64>,
    /// Join `<root>/index.scip` as tier B, and in which document order.
    pub scip: Option<ScipOrder>,
}

/// The order a SCIP index is read in.
pub(crate) enum ScipOrder {
    /// As the indexer wrote it.
    AsWritten,
    /// Documents, and each document's occurrences and symbols, permuted.
    Shuffled(u64),
}

/// A seeded Fisher-Yates over splitmix64, so a failing seed reproduces, and the
/// suite takes no dependency to get one.
struct Shuffler(u64);

impl Shuffler {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Permute `items`. Returns whether any element moved: a shuffle that left
    /// the order alone tests nothing, and the caller asserts it did not.
    fn shuffle<T>(&mut self, items: &mut [T]) -> bool {
        let mut at: Vec<usize> = (0..items.len()).collect();
        for i in (1..items.len()).rev() {
            let bound = u64::try_from(i + 1).expect("a slice index fits u64");
            let j = usize::try_from(self.next() % bound).expect("below a usize bound");
            items.swap(i, j);
            at.swap(i, j);
        }
        at.iter().enumerate().any(|(i, from)| i != *from)
    }
}

/// [`extract_tree`], with the inputs in the order `order` says — the other half
/// of `CLAUDE.md`'s "shuffle file order -> identical facts". The indexer's walk
/// sorts, so no order a filesystem hands back can reach the extractor; this
/// goes under the walk and permutes what it returns.
pub(crate) fn extract_in(root: &Path, order: &Order) -> Extracted {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut interner = Interner::open(dir.path()).expect("dictionary");
    let mut segments = BTreeMap::new();
    let walked = walk(root);
    let skips = &walked.skips;
    assert!(skips.unreadable.is_empty(), "{:?}", skips.unreadable);
    let mut files = walked.files;
    if let Some(seed) = order.files {
        assert!(
            Shuffler(seed).shuffle(&mut files),
            "seed {seed} left {} files where they were",
            files.len()
        );
    }
    let ingest = order.scip.as_ref().map(|how| {
        let bytes = std::fs::read(root.join("index.scip")).expect("the fixture has an index.scip");
        let mut index = Index::parse_from_bytes(&bytes).expect("a SCIP index");
        if let ScipOrder::Shuffled(seed) = how {
            let mut rng = Shuffler(*seed);
            assert!(
                rng.shuffle(&mut index.documents),
                "seed {seed} left the documents where they were"
            );
            for document in &mut index.documents {
                rng.shuffle(&mut document.occurrences);
                rng.shuffle(&mut document.symbols);
            }
            rng.shuffle(&mut index.external_symbols);
        }
        Ingest::of(&index, root, "")
    });
    for candidate in files {
        let src = std::fs::read_to_string(&candidate.abs).expect("source is UTF-8");
        let mut extractor = Extractor::new(candidate.lang).expect("extractor");
        let saw = ingest
            .as_ref()
            .filter(|i| i.docs.contains_key(&candidate.path));
        let anchors = saw.map_or_else(Anchors::default, |i| Anchors::of(i, &candidate.path));
        let mut out = extractor
            .file(&candidate.path, &src, &mut interner, &anchors)
            .expect("extracts");
        if let Some(ingest) = saw {
            tier_b::emit(
                &mut out.segment,
                ingest,
                &candidate.path,
                Some(&src),
                &out.defs,
                &mut interner,
            )
            .expect("tier B emits");
        }
        segments.insert(candidate.path, out.segment);
    }
    Extracted {
        root: root.to_path_buf(),
        interner,
        segments,
        _dir: dir,
    }
}

impl Extracted {
    /// Every fact as a ground Datalog clause, sorted — the same syntax a fact
    /// file uses, so a golden diff reads as the thing it describes.
    pub(crate) fn render(&self) -> String {
        render_with(&self.interner, &self.segments)
    }

    /// Rows of one relation, as resolved strings, sorted.
    pub(crate) fn rows(&self, relation: &str) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for segment in self.segments.values() {
            let Some(rel) = segment.relation(relation) else {
                continue;
            };
            for row in rel.iter() {
                out.push(row.iter().map(|a| self.show(*a)).collect());
            }
        }
        out.sort();
        out
    }

    /// Rows of one relation in one file.
    pub(crate) fn rows_in(&self, relation: &str, path: &str) -> Vec<Vec<String>> {
        self.rows(relation)
            .into_iter()
            .filter(|r| r.get(1).map(String::as_str) == Some(path))
            .collect()
    }

    /// A relation's first two columns as a map. For `parent` and the like.
    pub(crate) fn pairs(&self, relation: &str) -> BTreeMap<String, String> {
        self.rows(relation)
            .into_iter()
            .filter_map(|r| Some((r.first()?.clone(), r.get(1)?.clone())))
            .collect()
    }

    fn show(&self, atom: u32) -> String {
        if is_int(atom) {
            atom.to_string()
        } else {
            self.interner.resolve(atom).unwrap_or("<?>").to_string()
        }
    }
}

fn render_with(interner: &Interner, segments: &BTreeMap<String, Segment>) -> String {
    let mut lines = BTreeSet::new();
    for segment in segments.values() {
        for rel in RELATIONS {
            let Some(rows) = segment.relation(rel.name) else {
                continue;
            };
            for row in rows.iter() {
                let args: Vec<String> = row
                    .iter()
                    .map(|a| {
                        if is_int(*a) {
                            a.to_string()
                        } else {
                            format!("{:?}", interner.resolve(*a).unwrap_or("<?>"))
                        }
                    })
                    .collect();
                lines.insert(format!("{}({}).", rel.name, args.join(", ")));
            }
        }
    }
    let mut out: Vec<&String> = lines.iter().collect();
    out.sort();
    out.iter()
        .map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Golden facts: the whole extraction, diffed as text.
pub(crate) fn assert_golden(extracted: &Extracted) {
    let rendered = extracted.render();
    let golden = extracted.root.join("expected.facts");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &rendered).expect("writes the golden file");
        return;
    }
    let expected = std::fs::read_to_string(&golden).expect("the golden file exists");
    assert_eq!(
        rendered, expected,
        "extraction changed. Re-read the diff, then `UPDATE_GOLDEN=1 cargo test -p extract`"
    );
}

/// Span exactness, asserted by re-slicing and **re-parsing** the source: every
/// `def_span` is a node boundary and every `def_name` is exactly the identifier.
///
/// The file is parsed again, here, with the language's grammar, and
/// [`span_defect`] reads the spans off that tree. It is the whole file and not
/// the slice `src[start..end]` that is re-parsed: a method, a field, an enum
/// variant or a TypeScript constructor is only a node inside its parent, and
/// standalone it is a parse error or a different kind. On the four fixtures 59
/// of 167 definitions (every member) fail that literal check although their
/// spans are exact, which is why `specs/02-extraction.md` § Validation 2 says
/// "in context".
pub(crate) fn assert_spans_exact(extracted: &Extracted) {
    let spans: BTreeMap<String, (usize, usize)> = extracted
        .rows("def_span")
        .into_iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone(),
                (r.get(3)?.parse().ok()?, r.get(4)?.parse().ok()?),
            ))
        })
        .collect();
    let names: BTreeMap<String, (usize, usize)> = extracted
        .rows("def_name")
        .into_iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone(),
                (r.get(1)?.parse().ok()?, r.get(2)?.parse().ok()?),
            ))
        })
        .collect();
    let langs: BTreeMap<String, &'static lang::Lang> = walk(&extracted.root)
        .files
        .into_iter()
        .map(|c| (c.path, c.lang))
        .collect();

    for def in extracted.rows("def") {
        let (sym, path, name) = (
            def.first().expect("S"),
            def.get(1).expect("F"),
            def.get(3).expect("Name"),
        );
        let src = std::fs::read_to_string(extracted.root.join(path)).expect("source");
        let (start, end) = *spans.get(sym).expect("every def has a span");
        let at = *names.get(sym).expect("every def has a def_name");
        let lang = langs.get(path).expect("a registered language");
        let defect = span_defect(&src, lang, (start, end), name, at);
        assert!(
            defect.is_none(),
            "{sym}: {path}: {}",
            defect.unwrap_or_default()
        );
    }
}

/// What is wrong with a definition's recorded spans, read off a fresh parse of
/// `src`; `None` when they are exact.
///
/// - **The end** is where a node ends. The outermost node inside the span that
///   ends there is *the definition node*, and it is not a parse error.
/// - **The start** is where the definition node starts, or where a contiguous
///   run of attributes, comments and keywords before it starts — the leading
///   siblings `def_span` is documented to reach back over, and nothing else.
///   Containment cannot tell a span that swallows its neighbour from an exact
///   one; this can, because the neighbour is not a leading sibling.
/// - **`def_name`** is the one identifier node whose text is the name, inside
///   the definition node. `starts_with` accepted `get` at `getter`.
///
/// `at` is a 1-based line and a 0-based UTF-8 byte column.
pub(crate) fn span_defect(
    src: &str,
    lang: &lang::Lang,
    (start, end): (usize, usize),
    name: &str,
    at: (usize, usize),
) -> Option<String> {
    use tree_sitter::{Parser, Point};

    let mut parser = Parser::new();
    parser
        .set_language(&(lang.language)())
        .expect("the grammar loads");
    let tree = parser.parse(src, None).expect("parses");
    let root = tree.root_node();

    if src.get(start..end).is_none() || start >= end {
        return Some(format!("{start}..{end} is not a byte range in the file"));
    }
    let Some(mut node) = root.descendant_for_byte_range(end - 1, end) else {
        return Some(format!("no node holds the span's last byte, {}", end - 1));
    };
    if node.end_byte() != end {
        return Some(format!(
            "span end {end} is not where a node ends: the node over its last byte is a {} ending at {}",
            node.kind(),
            node.end_byte()
        ));
    }
    while let Some(parent) = node
        .parent()
        .filter(|p| p.end_byte() == end && p.start_byte() >= start)
    {
        node = parent;
    }
    if node.start_byte() < start {
        return Some(format!(
            "the {} ending at {end} starts at {}, before the span's {start}",
            node.kind(),
            node.start_byte()
        ));
    }
    if node.is_error() || node.has_error() {
        return Some(format!(
            "the {} at {start}..{end} contains a parse error",
            node.kind()
        ));
    }
    let definition = node;

    let mut cursor = definition;
    let mut reached = definition.start_byte();
    while reached > start {
        let Some(leading) = cursor.prev_sibling() else {
            let Some(parent) = cursor.parent().filter(|p| p.start_byte() == reached) else {
                break;
            };
            cursor = parent;
            continue;
        };
        let kind = leading.kind();
        let leads = lang.attribute_kinds.contains(&kind)
            || lang.comment_kinds.contains(&kind)
            || lang.keyword_kinds.contains(&kind);
        if !leads || leading.start_byte() < start {
            break;
        }
        reached = leading.start_byte();
        cursor = leading;
    }
    if reached != start {
        return Some(format!(
            "span start {start} is not a node boundary: the {} starts at {} and the run of \
             attributes, comments and keywords before it reaches back only to {reached}",
            definition.kind(),
            definition.start_byte()
        ));
    }

    let (line, col) = at;
    let Some(row) = line.checked_sub(1) else {
        return Some("def_name line is 0; lines are 1-based".to_string());
    };
    let named = root.descendant_for_point_range(
        Point { row, column: col },
        Point {
            row,
            column: col + name.len(),
        },
    );
    match named {
        Some(n)
            if n.start_position() == (Point { row, column: col })
                && n.utf8_text(src.as_bytes()) == Ok(name)
                && n.start_byte() >= definition.start_byte()
                && n.end_byte() <= definition.end_byte() =>
        {
            None
        }
        Some(n) => Some(format!(
            "def_name {line}:{col} is a {} {:?}, not the identifier {name:?} inside the definition",
            n.kind(),
            n.utf8_text(src.as_bytes()).unwrap_or("?")
        )),
        None => Some(format!("no node at def_name {line}:{col}")),
    }
}

/// Nothing is emitted outside the closed `Kind` vocabulary, and every
/// definition has exactly one `parent`.
pub(crate) fn assert_kinds_closed(extracted: &Extracted) {
    for def in extracted.rows("def") {
        let kind = def.get(2).expect("Kind");
        assert!(KINDS.contains(&kind.as_str()), "{kind} is not a Kind");
    }
    assert_eq!(
        extracted.rows("parent").len(),
        extracted.rows("def").len(),
        "exactly one parent row per def"
    );
}

/// Every `parent` names something this extraction actually defines, or a file.
///
/// A `parent` pointing at a symbol with no `def` row stops `within/2` one hop
/// short without failing the query — a short answer, which the status taxonomy
/// cannot report. `specs/02-extraction.md` § Parent precedence.
pub(crate) fn assert_parents_exist(extracted: &Extracted) {
    let defined: BTreeSet<String> = extracted
        .rows("def")
        .into_iter()
        .filter_map(|r| r.first().cloned())
        .collect();
    let files: BTreeSet<String> = extracted
        .rows("file")
        .into_iter()
        .filter_map(|r| r.first().cloned())
        .collect();
    for (child, owner) in extracted.pairs("parent") {
        assert!(
            defined.contains(&owner) || files.contains(&owner),
            "parent({child}, {owner}) names nothing that exists"
        );
    }
}

/// Query liveness, half one: every kind the language claims to emit was
/// emitted at least once, and references and imports are not empty.
///
/// A dead pattern is the signature of a grammar node rename, and it otherwise
/// surfaces as `status: ok` with zero rows.
pub(crate) fn assert_kinds_present(extracted: &Extracted, kinds: &[&str]) {
    let found: BTreeSet<String> = extracted
        .rows("def")
        .into_iter()
        .filter_map(|r| r.get(2).cloned())
        .collect();
    for kind in kinds {
        assert!(found.contains(*kind), "no {kind} matched in the fixture");
    }
    assert!(!extracted.rows("name_ref").is_empty(), "no call sites");
    assert!(!extracted.rows("import").is_empty(), "no imports");
}

/// Query liveness, half two: no node takes two `@definition.*`/`@scope.*`
/// captures.
///
/// Upstream's Rust query matches a method in a `declaration_list` as both
/// `@definition.method` and `@definition.function` and emits two `def` rows for
/// one method. A query cannot be trusted to stay disjoint by inspection.
pub(crate) fn assert_captures_disjoint(root: &Path) {
    use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

    let found = walk(root).files;
    assert!(!found.is_empty(), "the fixture has no source files");
    for lang in extract::LANGS {
        let language = (lang.language)();
        let query = Query::new(&language, lang.tags).expect("tags.scm compiles");
        let mut parser = Parser::new();
        parser.set_language(&language).expect("the grammar loads");

        for candidate in found.iter().filter(|c| c.lang.name == lang.name) {
            let src = std::fs::read_to_string(&candidate.abs).expect("source");
            let tree = parser.parse(&src, None).expect("parses");
            let mut claimed: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
            let mut cursor = QueryCursor::new();
            let mut matches = cursor.matches(&query, tree.root_node(), src.as_bytes());
            while let Some(m) = matches.next() {
                for capture in m.captures() {
                    let Some(label) = query.capture_names().get(capture.index as usize) else {
                        continue;
                    };
                    if label.starts_with("definition.") || label.starts_with("scope.") {
                        claimed
                            .entry(capture.node.id())
                            .or_default()
                            .insert((*label).to_string());
                    }
                }
            }
            for (node, labels) in claimed {
                assert!(
                    labels.len() == 1,
                    "{}: node {node} is captured as {labels:?}",
                    candidate.path
                );
            }
        }
    }
}

/// Locality: every fact for file X is reproducible by extracting X alone, with
/// nothing else indexed.
///
/// The mechanical guard on incremental soundness. Atom ids differ between the
/// two dictionaries; the resolved facts must not.
pub(crate) fn assert_facts_are_local(extracted: &Extracted) {
    for (path, segment) in &extracted.segments {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut interner = Interner::open(dir.path()).expect("dictionary");
        let src = std::fs::read_to_string(extracted.root.join(path)).expect("source");
        let lang = lang::for_path(path).expect("a language");
        let mut extractor = Extractor::new(lang).expect("extractor");
        let alone = extractor
            .file(path, &src, &mut interner, &Anchors::default())
            .expect("extracts");

        let one = BTreeMap::from([(path.clone(), alone.segment)]);
        let same = BTreeMap::from([(path.clone(), segment.clone())]);
        assert_eq!(
            render_with(&interner, &one),
            render_with(&extracted.interner, &same),
            "{path} is not local"
        );
    }
}

/// Module spelling: an `import` row's `Module` carries no source delimiters.
///
/// The one guard about agreement *between* languages. `specs/01-facts.md`
/// § `import` says the column is the specifier's content, so Go's
/// `import "fmt"` is `fmt` and not `"fmt"`; a language that kept its quotes
/// would make `?- import(F, "fmt", _).` answer zero rows on that language
/// alone, which is the silent-miss class this project exists to prevent (#70).
pub(crate) fn assert_modules_undelimited(extracted: &Extracted) {
    for row in extracted.rows("import") {
        let module = row.get(1).expect("an import row has a Module");
        let delimited = ['"', '\'', '`']
            .iter()
            .any(|d| module.starts_with(*d) || module.ends_with(*d));
        assert!(
            !delimited,
            "import module {module:?} keeps a source delimiter \
             (specs/01-facts.md § `import`)"
        );
    }
}

/// Determinism: a second extraction of the same tree is byte-identical.
pub(crate) fn assert_deterministic(root: &Path) {
    assert_eq!(extract_tree(root).render(), extract_tree(root).render());
}

/// Input order: the facts do not depend on the order files, or the documents of
/// a SCIP index, are read in.
///
/// The same fixture is extracted in the walk's order and then in `SEEDS`
/// permutations of it, tier A alone and then with the fixture's `index.scip`
/// joined as tier B — whose ingest, anchor join and collision handling are where
/// ordering freedom lives — and every rendering must equal the first. The
/// renderings compare as resolved text, because atom ids legitimately follow
/// insertion order.
pub(crate) fn assert_order_independent(root: &Path) {
    const SEEDS: [u64; 4] = [1, 2, 3, 4];
    let sorted = extract_in(root, &Order::default()).render();
    for seed in SEEDS {
        let shuffled = extract_in(
            root,
            &Order {
                files: Some(seed),
                scip: None,
            },
        )
        .render();
        same_facts(
            &sorted,
            &shuffled,
            &format!("tier A, files shuffled, seed {seed}"),
        );
    }

    let joined = extract_in(
        root,
        &Order {
            files: None,
            scip: Some(ScipOrder::AsWritten),
        },
    )
    .render();
    assert_ne!(sorted, joined, "tier B added nothing, so it tests nothing");
    for seed in SEEDS {
        let shuffled = extract_in(
            root,
            &Order {
                files: Some(seed),
                scip: Some(ScipOrder::Shuffled(seed)),
            },
        )
        .render();
        same_facts(
            &joined,
            &shuffled,
            &format!("tier A + B, files and SCIP documents shuffled, seed {seed}"),
        );
    }
}

/// Two renderings of the same facts, or the lines that differ: a whole
/// rendering in a failure message buries the one fact that moved.
fn same_facts(expected: &str, got: &str, what: &str) {
    let (old, new): (BTreeSet<&str>, BTreeSet<&str>) =
        (expected.lines().collect(), got.lines().collect());
    let lost: Vec<&&str> = old.difference(&new).take(5).collect();
    let gained: Vec<&&str> = new.difference(&old).take(5).collect();
    assert!(
        lost.is_empty() && gained.is_empty(),
        "{what}: the facts depend on input order.\nonly in the first ({}): {lost:#?}\nonly in the second ({}): {gained:#?}",
        old.difference(&new).count(),
        new.difference(&old).count()
    );
}
