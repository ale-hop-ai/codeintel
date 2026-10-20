//! `codeintel schema` and `codeintel status`.
//!
//! These are `docs/plan.md` M4's done-when list for the two read-only verbs.
//! What they guard is a single failure mode: a catalogue that *declares* what
//! the index holds instead of *counting* it. A static list advertising sixteen
//! kinds when the index holds five is invariant 5 broken by the onboarding
//! text itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use codeintel::census::{Census, LangCount};
use codeintel::schema;

/// An answer that is JSON. A process that printed nothing fails `from_slice`
/// with "EOF while parsing", which says nothing about why it printed nothing.
#[track_caller]
fn stdout_json(out: &Output) -> serde_json::Value {
    let why = format!(
        "exited {:?} and printed {} byte(s) of stdout, not JSON. stderr: {}",
        out.status.code(),
        out.stdout.len(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect(&why)
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust")
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
    for extra in ["expected.facts", "index.scip", ".gitignore"] {
        drop(std::fs::remove_file(dir.path().join(extra)));
    }
    dir
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        if name == "target" || name == ".codeintel" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().expect("file type").is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(root)
        .env("CODEINTEL_INDEX_DIR", root.join(".codeintel"))
        .output()
        .expect("the binary runs")
}

fn stdout(root: &Path, args: &[&str]) -> String {
    let out = run(root, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The `schema` size ceiling, in **characters**, because characters are what
/// can be measured here.
///
/// `specs/05-surface.md` states the budget in tokens (~1500), and no tokenizer
/// is available offline — adding one is a dependency for a number that gates
/// copy length. Estimates of this text span **~1,400 tokens** (4 chars/token,
/// prose-like) to **~1,750** (a BPE-shaped count that charges per punctuation
/// mark and per identifier fragment, which is what dense tabular output really
/// looks like).
///
/// This is now the *only* budget the standard library spends against — the rule
/// count cap was dropped (`specs/00-overview.md` § Surface budget), because a
/// rule's cost to an agent is the bytes it must read, and a count cap pushed
/// back against the one growth path the design actually wants.
///
/// **Measured against [`worst_case`], not against a fixture tree** (#42). It was
/// measured against `tree()` — twelve files of one language — while `schema`
/// embeds live per-kind and per-language counts, so its length scales with the
/// index it reports on and the fixture is the *smallest* input there is. The
/// ceiling read green at 5,700 while this repository rendered 5,788, and a
/// ceiling that only binds on the smallest possible input is not a ceiling. The
/// loose 10,000 that replaced it caught a doubling and nothing subtler.
///
/// 6,000 against the worst case is a ceiling with roughly 10% of headroom over
/// a census no real index exceeds, and it binds on every index. A change that
/// pushes past it is a change that has to argue for itself: the rule list must
/// stay complete, so the copy is what gives — or the standard library is too
/// big and a rule goes.
const MAX_SCHEMA_CHARS: usize = 6_000;

/// The largest census `schema` can be asked to render.
///
/// Every count at seven digits, every `Kind` and `Role` occupied, and every
/// language `extract::lang` ships present — the three axes along which this
/// output grows. Built rather than indexed, because a tree exercising all of
/// them is a tree nobody has and the numbers would still be whatever that tree
/// happened to hold.
///
/// Derived from `facts::RELATIONS`, `KINDS`, `ROLES` and `LANGS` rather than
/// listed, so a new relation or a tenth language raises the measured worst case
/// instead of silently leaving it behind.
///
/// `MAX_ROWS` is the one declared number: a ceiling measured against unbounded
/// counts cannot exist. Ten million rows in a single base relation is roughly a
/// ten-million-line repository's `name_ref`, and an index past it renders one
/// extra character per relation per digit.
fn worst_case() -> Census {
    const MAX_ROWS: usize = 9_999_999;
    let mut census = Census {
        indexed: true,
        ..Default::default()
    };
    for rel in facts::RELATIONS {
        census.relations.insert(rel.name, MAX_ROWS);
    }
    for kind in extract::lang::KINDS {
        census.kinds.insert((*kind).to_string(), MAX_ROWS);
    }
    for role in extract::lang::ROLES {
        census.roles.insert((*role).to_string(), MAX_ROWS);
    }
    for lang in extract::lang::LANGS {
        census.langs.insert(
            lang.name.to_string(),
            LangCount {
                files: MAX_ROWS,
                ..Default::default()
            },
        );
    }
    census
}

#[test]
fn the_schema_fits_its_size_budget_with_the_rule_list_complete() {
    let census = worst_case();
    let text = schema::Schema { census: &census }.to_string();

    assert!(
        text.len() <= MAX_SCHEMA_CHARS,
        "schema is {} chars on the worst-case census, ceiling is {MAX_SCHEMA_CHARS}. The \
         rule list must stay complete, so the copy is what gives — or the standard \
         library is too big and a rule goes.\n{text}",
        text.len()
    );

    for rule in schema::rules(codeintel::STDLIB) {
        assert!(text.contains(&rule.head), "{} is not advertised", rule.head);
    }
}

/// The ceiling binds the binary, not just a `Display` a test can construct.
///
/// `worst_case()` renders through `schema::Schema`; so does `codeintel schema`,
/// and this is the assertion that says so. A real index must come in under the
/// worst case, and the fixture is the one index CI always has.
#[test]
fn the_rendered_schema_is_what_the_binary_prints() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["schema"]);
    assert!(
        text.len() <= MAX_SCHEMA_CHARS,
        "the fixture index renders {} chars, over the worst case",
        text.len()
    );
    assert!(text.starts_with("START HERE"), "{text}");
}

#[test]
fn the_rule_list_matches_the_rules() {
    // The `%%` signature is written out, so it can drift from the clause below
    // it. Every advertised head must name a real predicate at its real arity,
    // and every predicate in the file must be advertised — otherwise `schema`
    // is a catalogue of a stdlib that no longer exists.
    let advertised: Vec<(String, usize)> = schema::rules(codeintel::STDLIB)
        .iter()
        .map(|r| {
            let (name, arity) =
                schema::split_head(&r.head).unwrap_or_else(|| panic!("`{}` is not a head", r.head));
            (name.to_string(), arity)
        })
        .collect();

    let mut defined: Vec<(String, usize)> = Vec::new();
    for line in codeintel::STDLIB.lines() {
        let head = line.split_once(":-").map_or(line, |(h, _)| h);
        let Some((name, args)) = head.split_once('(') else {
            continue;
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            continue;
        }
        let Some((args, _)) = args.rsplit_once(')') else {
            continue;
        };
        let row = (name.to_string(), args.split(',').count());
        if !defined.contains(&row) {
            defined.push(row);
        }
    }

    for rule in &advertised {
        assert!(
            defined.contains(rule),
            "`{}/{}` is advertised but not defined at that arity",
            rule.0,
            rule.1
        );
    }
    for rule in &defined {
        assert!(
            advertised.contains(rule),
            "`{}/{}` is defined but has no `%%` line, so `schema` hides it",
            rule.0,
            rule.1
        );
    }
    // 39 -> 44, each a deliberate rule that says which one:
    //   `scip_claim`    (#56) "tier B resolved this position" stopped meaning
    //                   "a `scip_ref` row exists here", and a base-relation
    //                   literal cannot express "...and its target is a symbol
    //                   something describes".
    //   `trait_sym`     (#40) the trait side of an `impl` must resolve to a
    //                   trait-like `Kind`, and that is three clauses, not one.
    //   `reaches_exact`, `recursive_exact`, `reach_of_exact` (#53) the three
    //                   traversals over `calls` that shipped without the
    //                   `_exact` sibling this file's own header promises.
    //
    //   44 -> 41 (#42) three rules deleted to buy back schema characters, each
    //                   one join an agent can write itself:
    //                   `callers_by_name(C, N)` = `calls(C, S), def(S, _, _, N)`,
    //                   `impact_by_name` the same over `impact_of`, and
    //                   `long_def(S)` = `def_lines(S, L), L > 80` — whose own
    //                   doc line already said to use `def_lines`.
    //
    //   41 -> 42 (exp/prov-rows) `tier(Rel, A, B, Prov)`: five rules shipped
    //                   with no `Prov` in their tuples, so a wrong row from
    //                   them could not say it was a guess. One relation hands
    //                   it back; no existing head changes
    //                   (specs/01-facts.md § Row provenance).
    //
    // This is the DRIFT tripwire, not the budget: the rule-count cap was
    // dropped in favour of the byte ceiling asserted by
    // `the_schema_fits_its_size_budget_with_the_rule_list_complete`.
    assert_eq!(advertised.len(), 42, "the rule count moved: {advertised:?}");
}

#[test]
fn schema_counts_come_from_this_index() {
    // The whole point: a kind at 0 is a kind the agent must not query, and it
    // is only knowable by counting.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["schema"]);

    // The fixture is Rust, so it has functions and no classes.
    assert!(text.contains("class 0"), "{text}");
    assert!(!text.contains("function 0"), "{text}");
    // Tier A only: every reference is provenance `name`.
    assert!(text.contains("exact 0"), "{text}");
    assert!(text.contains("Lang  rust"), "{text}");
}

#[test]
fn schema_with_no_index_says_so_rather_than_reading_as_an_inventory() {
    // Zero everywhere is indistinguishable from "your code has none of these"
    // unless the output says which it is.
    let dir = tempfile::tempdir().expect("tempdir");
    let text = stdout(dir.path(), &["schema"]);
    assert!(text.contains("THERE IS NO INDEX HERE"), "{text}");
    assert!(text.contains("codeintel index ."), "{text}");
    // The catalogue is still there: an agent can learn the system before
    // indexing anything.
    assert!(text.contains("innermost_at"), "{text}");

    // And a consumer can branch on it without reading the text (#52): the
    // status on stderr, exit 0 because the catalog is the complete answer
    // (`specs/05-surface.md` § `schema`).
    let out = run(dir.path(), &["schema"]);
    assert!(out.status.success(), "{:?}", out.status);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("status=no-index"), "{stderr}");
    // The complete chain, not a fragment (`specs/05-surface.md` § The no-index
    // hint, #59). `schema` reports on the directory it was given and does not
    // adopt a nearby index, but it gives `query`'s hint.
    assert!(stderr.contains("hint: no index for"), "{stderr}");
    assert!(stderr.contains("codeintel index ."), "{stderr}");

    let out = run(dir.path(), &["schema", "--format", "json"]);
    assert!(out.status.success(), "{:?}", out.status);
    let json: serde_json::Value = stdout_json(&out);
    assert_eq!(json["status"], "no-index", "{json}");
    assert!(json["hint"].is_string(), "{json}");
}

#[test]
fn status_reports_per_language_counts_and_the_fingerprint() {
    // "python: 1,204 files, 11 defs" is visibly absurd to a human in one
    // second; `status: ok` is not (`docs/plan.md` M4).
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["status"]);
    assert!(text.starts_with("status: ok"), "{text}");
    assert!(text.contains("extractor: blake3:"), "{text}");
    assert!(text.contains("rust "), "{text}");
    assert!(text.contains("defs,"), "{text}");
    // Provenance is never blended into one number.
    assert!(text.contains("exact refs"), "{text}");
    assert!(text.contains("name refs"), "{text}");
    assert!(text.contains("changed since index: none"), "{text}");
    assert!(text.contains("scip: none"), "{text}");
}

#[test]
fn status_json_is_the_bug_report_artifact() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = stdout(dir.path(), &["status", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");

    assert_eq!(json["status"], "ok");
    assert!(
        json["extractor_fingerprint"]
            .as_str()
            .is_some_and(|f| f.starts_with("blake3:")),
        "{json}"
    );
    assert!(json["languages"]["rust"]["defs"].as_u64().unwrap_or(0) > 0);
    assert!(json["languages"]["rust"]["refs_name"].as_u64().unwrap_or(0) > 0);
    assert_eq!(
        json["languages"]["rust"]["refs_exact"], 0,
        "tier A only here"
    );
    // Every relation, including the ones at zero — an absent key would read as
    // "not measured" rather than "empty".
    for rel in facts::RELATIONS {
        assert!(
            json["relations"][rel.name].is_number(),
            "{} is missing from status json",
            rel.name
        );
    }
    assert!(json["scip"].as_array().is_some_and(Vec::is_empty));
}

#[test]
fn status_notices_a_file_that_changed_after_the_index() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    std::fs::write(dir.path().join("src/store.rs"), "pub fn added() {}\n").expect("write");

    let text = stdout(dir.path(), &["status"]);
    assert!(text.starts_with("status: stale"), "{text}");
    assert!(text.contains("src/store.rs"), "{text}");
}

#[test]
fn status_with_no_index_is_an_answer_not_a_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(dir.path(), &["status"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("status: no-index"), "{text}");
    // The root the caller named. This asserted a literal `codeintel index .`,
    // which is the wrong directory whenever `status` was given a path — it
    // sent the reader to index wherever they happened to be standing.
    let root = dir.path().canonicalize().expect("canonical");
    assert!(
        text.contains(&format!("no index for {}", root.display())),
        "{text}"
    );
    assert!(
        text.contains(&format!("run: cd {} && codeintel index .", root.display())),
        "{text}"
    );
}

#[test]
fn a_wrapped_rule_doc_is_joined_not_dropped() {
    // `about` is the one relation designed to answer everything in one round
    // trip, and its `Rel` vocabulary does not fit on one line. A parser that
    // demanded a signature on every `%%` line dropped the continuation, so an
    // agent learned five of the ten values and would never ask for the rest.
    let about = schema::rules(codeintel::STDLIB)
        .into_iter()
        .find(|r| r.head.starts_with("about("))
        .expect("about is advertised");
    for value in [
        "sig",
        "doc",
        "defined",
        "parent",
        "caller",
        "callee",
        "implements",
        "implementor",
        "test",
        "extern",
    ] {
        assert!(about.doc.contains(value), "`{value}` is not advertised");
    }
    assert!(
        !about.doc.ends_with('|'),
        "truncated at a wrap: {}",
        about.doc
    );

    let dir = tree();
    run(dir.path(), &["index", "."]);
    assert!(stdout(dir.path(), &["schema"]).contains("implementor"));
}

#[test]
fn every_relation_has_a_signature_of_the_right_arity() {
    // `signature()` now reads `facts::RELATIONS`, so the two cannot disagree
    // about *which* relations exist (#48). What this still checks is the
    // rendered head: `schema` prints `name(args)` and `split_head` has to
    // parse that back, so a signature that is malformed rather than merely
    // absent would print a broken row in the project's most-read output.
    for rel in facts::RELATIONS {
        let (args, _) = schema::signature(rel.name);
        assert!(!args.is_empty(), "{} has no signature", rel.name);
        let head = format!("{}{args}", rel.name);
        let (name, arity) = schema::split_head(&head)
            .unwrap_or_else(|| panic!("{} has a malformed signature {args}", rel.name));
        assert_eq!(name, rel.name);
        assert_eq!(arity, rel.arity, "{} signature is {args}", rel.name);
    }
}

#[test]
fn the_value_vocabularies_are_the_extractors_own() {
    // Re-declaring them here would let `schema` advertise a kind the extractor
    // cannot emit, or hide one it does.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["schema"]);
    for kind in extract::lang::KINDS {
        assert!(text.contains(&format!(" {kind} ")), "{kind} is not listed");
    }
    for role in extract::lang::ROLES {
        assert!(text.contains(&format!(" {role} ")), "{role} is not listed");
    }
}

#[test]
fn status_with_no_index_does_not_walk_the_tree() {
    // The walk is a full gitignore-aware traversal, and with no index there is
    // nothing to report an unindexed extension against.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("notes.md"), "# hello\n").expect("write");
    let out = stdout(dir.path(), &["status", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(json["status"], "no-index");
    assert!(
        json["unsupported"]
            .as_object()
            .is_some_and(serde_json::Map::is_empty),
        "{json}"
    );
}

#[test]
fn every_status_verdict_carries_its_remedy_on_both_formats() {
    // `specs/00-overview.md` invariant 6: a degradation is a status "with an
    // actionable hint", and `docs/plan.md` M4 states it as `hint` non-null
    // whenever `status != "ok"`. `status --format json` carried no `hint` key
    // at all — and it is the format M4 calls "the bug-report artifact for a
    // tool with no telemetry", so the consumer that cannot read prose was the
    // one left with a verdict and no remedy. The sibling assertion for
    // `schema --format json` existed; this one did not.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = stdout(dir.path(), &["status", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(json["status"], "no-index");
    let hint = json["hint"].as_str().expect("a non-ok status has a hint");
    assert!(hint.contains("codeintel index"), "{hint}");
    // The root the caller named, not the directory they are standing in.
    let root = dir.path().canonicalize().expect("canonical");
    assert!(hint.contains(&root.display().to_string()), "{hint}");

    // The text form says the same thing.
    let text = stdout(dir.path(), &["status"]);
    assert!(text.contains("status: no-index"), "{text}");
    assert!(text.contains(&root.display().to_string()), "{text}");

    // `ok` is the only verdict with no hint.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = stdout(dir.path(), &["status", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(json["status"], "ok", "{json}");
    assert!(json["hint"].is_null(), "{json}");
}

/// One JSON-RPC response per stdout line. A server that died says why on stderr.
#[track_caller]
fn responses(out: &Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| {
            let why = format!(
                "`codeintel mcp` exited {:?}; line {l:?} is not a JSON-RPC response. stderr: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            );
            serde_json::from_str(l).expect(&why)
        })
        .collect()
}

/// One JSON-RPC exchange per line, in; the responses, out.
fn rpc(root: &Path, requests: &[serde_json::Value]) -> Vec<serde_json::Value> {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = Command::new(binary())
        .args(["mcp"])
        .current_dir(root)
        .env("CODEINTEL_INDEX_DIR", root.join(".codeintel"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the server starts");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write");
        }
    }
    let out = child.wait_with_output().expect("the server exits");
    assert!(
        out.status.success(),
        "`codeintel mcp` exited {:?} after {} byte(s) of stdout. stderr: {}",
        out.status.code(),
        out.stdout.len(),
        String::from_utf8_lossy(&out.stderr)
    );
    responses(&out)
}

#[test]
fn the_surface_budget_holds() {
    // `specs/00-overview.md` § Surface budget, asserted in CI. Three of these
    // cannot grow; the fourth is the one that does, which is why asserting
    // only the other three would make the founding thesis unfalsifiable.
    //
    // The verb count is asserted in `main.rs`, where the clap type lives.
    let dir = tempfile::tempdir().expect("tempdir");
    let tools = rpc(
        dir.path(),
        &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let tools = tools[0]["result"]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 1, "the MCP tool budget is 1: {tools:?}");
    assert_eq!(tools[0]["name"], codeintel::mcp::TOOL);

    assert!(
        facts::RELATIONS.len() <= 16,
        "base relations: {}",
        facts::RELATIONS.len()
    );

    // There is deliberately no cap on the rule count; see
    // `specs/00-overview.md` § Surface budget. The budget rules are spent
    // against is `schema` output size, asserted by
    // `the_schema_fits_its_size_budget_with_the_rule_list_complete`, because a
    // rule's real cost is the bytes an agent reads before it can work. A count
    // cap pushed back against the one growth path the design wants — a new
    // question is meant to become a rule, not a verb or a Rust helper.
}

#[test]
fn the_mcp_tool_takes_a_program_not_positional_arguments() {
    // `rule` + `args` carried the defect that deleted the `rules` verb: an
    // integer and its string form are different atoms, so `args: ["142"]`
    // bound the string and returned `ok` with zero rows.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let properties = out[0]["result"]["tools"][0]["inputSchema"]["properties"]
        .as_object()
        .expect("properties");
    assert!(!properties.contains_key("rule"), "{properties:?}");
    assert!(!properties.contains_key("args"), "{properties:?}");
    assert!(properties.contains_key("query"));
    assert!(properties.contains_key("schema"));
}

#[test]
fn a_notification_gets_no_reply_and_a_request_does() {
    // A JSON-RPC message with no `id` is a notification. Answering one is a
    // protocol violation, and `notifications/initialized` is the one every
    // client sends immediately after `initialize`.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
        ],
    );
    assert_eq!(out.len(), 2, "a notification was answered: {out:?}");
    assert_eq!(out[0]["id"], 1);
    assert_eq!(out[0]["result"]["serverInfo"]["name"], "codeintel");
    assert_eq!(out[1]["id"], 2);
}

#[test]
fn the_mcp_answer_carries_the_status_taxonomy() {
    // There is no stderr here to put the status on, and `ok` with zero rows
    // must stay distinguishable from every failure (invariant 6).
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = rpc_structured(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- def(S, F, \"function\", N)."}}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- file(F, \"python\")."}}}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- def(S, F,"}}}),
            serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- nosuchrelation(X)."}}}),
        ],
    );

    let rows = &out[0]["result"];
    assert_eq!(rows["isError"], false);
    assert_eq!(rows["structuredContent"]["status"], "ok");
    // One body: a client that takes `structuredContent` gets nothing in
    // `content`.
    assert_eq!(rows["content"], serde_json::json!([]), "{rows}");

    // `ok` and empty, with a hint saying which literal matched nothing.
    let empty = &out[1]["result"];
    assert_eq!(empty["structuredContent"]["status"], "ok");
    assert!(
        empty["structuredContent"]["hint"]
            .as_str()
            .is_some_and(|h| h.contains("matched 0 rows")),
        "{empty}"
    );

    // A broken query is a tool result with a status, not a protocol error.
    let bad = &out[2]["result"];
    assert_eq!(bad["isError"], true);
    assert_eq!(bad["structuredContent"]["status"], "invalid-query");
    assert!(bad["structuredContent"]["hint"].is_string(), "{bad}");

    // A relation that does not exist is `invalid-query`, not `ok` with zero
    // rows. A typo and "this is not true of your code" are different answers,
    // and returning the second for the first is the failure invariant 6 names.
    let unknown = &out[3]["result"];
    assert_eq!(unknown["structuredContent"]["status"], "invalid-query");
    assert!(
        unknown["structuredContent"]["hint"]
            .as_str()
            .is_some_and(|h| h.contains("nosuchrelation")),
        "the offending name must appear: {unknown}"
    );
}

#[test]
fn the_mcp_schema_call_works_with_no_index() {
    // `{"schema":true}` is the first call a client makes, and it must answer
    // before anything has been indexed.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"code_query","arguments":{"schema":true}}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"code_query","arguments":{"schema":true,"query":"?- file(F, L)."}}}),
        ],
    );
    let text = out[0]["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert!(text.contains("innermost_at"), "{text}");
    assert!(text.contains("THERE IS NO INDEX HERE"), "{text}");
    // The call is still no error: the catalog was delivered. And it was
    // delivered in `content` only — a client that declared nothing gets the
    // field every client reads.
    let result = &out[0]["result"];
    assert_eq!(result["isError"], false, "{result}");
    assert!(result.get("structuredContent").is_none(), "{result}");

    // The status is structural for a client that takes `structuredContent`
    // (#52), and the text is not sent a second time beside it.
    let structured = rpc_structured(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"code_query","arguments":{"schema":true}}}),
        ],
    );
    let result = &structured[0]["result"];
    assert_eq!(result["content"], serde_json::json!([]), "{result}");
    assert_eq!(
        result["structuredContent"]["status"], "no-index",
        "{result}"
    );
    assert!(
        result["structuredContent"]["hint"]
            .as_str()
            .is_some_and(|h| h.starts_with("no index for") && h.contains("codeintel index .")),
        "{result}"
    );
    assert_eq!(
        result["structuredContent"]["text"].as_str(),
        Some(text),
        "{result}"
    );
    assert_eq!(result["isError"], false, "{result}");

    // Both modes at once names the two rather than silently picking one.
    assert!(out[1]["error"]["message"].is_string(), "{:?}", out[1]);
}

/// The `initialize` a client sends, declaring the revision it speaks.
fn initialize(version: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":0,"method":"initialize",
        "params":{"protocolVersion":version,"capabilities":{},
                  "clientInfo":{"name":"test","version":"0"}}})
}

/// [`rpc`] as a client that takes `structuredContent` (Claude Code): the
/// `initialize` is sent and its response dropped, so `out[i]` answers
/// `requests[i]`.
fn rpc_structured(root: &Path, requests: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut all = vec![initialize("2025-06-18")];
    all.extend(requests.iter().cloned());
    let mut out = rpc(root, &all);
    out.remove(0);
    out
}

/// Raw bytes in, the responses out — for lines no `serde_json::Value` can be.
fn rpc_bytes(root: &Path, input: &[u8]) -> Vec<serde_json::Value> {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = Command::new(binary())
        .args(["mcp"])
        .current_dir(root)
        .env("CODEINTEL_INDEX_DIR", root.join(".codeintel"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the server starts");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input)
        .expect("write");
    let out = child.wait_with_output().expect("the server exits");
    assert!(
        out.status.success(),
        "`codeintel mcp` exited {:?} after {} byte(s) of stdout. stderr: {}",
        out.status.code(),
        out.stdout.len(),
        String::from_utf8_lossy(&out.stderr)
    );
    responses(&out)
}

#[test]
fn a_malformed_line_is_answered_and_the_session_survives_it() {
    // Silence on a line the client sent leaves it waiting forever, and ending
    // the session on one bad line takes every later call with it (#36). Each
    // bad line gets an `id: null` error, and the `ping` after them still works.
    let dir = tempfile::tempdir().expect("tempdir");
    let ping = serde_json::json!({"jsonrpc":"2.0","id":9,"method":"ping"});
    let mut input = Vec::new();
    input.extend_from_slice(b"\xff\xfe not utf-8\n");
    input.extend_from_slice(b"{not json\n");
    input.extend_from_slice(b"[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]\n");
    input.extend_from_slice(b"42\n");
    input.extend(std::iter::repeat_n(
        b'x',
        codeintel::mcp::MAX_LINE_BYTES + 10,
    ));
    input.push(b'\n');
    input.extend_from_slice(format!("{ping}\n").as_bytes());

    let out = rpc_bytes(dir.path(), &input);
    let codes: Vec<i64> = out
        .iter()
        .take(5)
        .map(|r| r["error"]["code"].as_i64().unwrap_or_default())
        .collect();
    assert_eq!(codes, [-32700, -32700, -32600, -32600, -32600], "{out:?}");
    for response in out.iter().take(5) {
        assert!(response["id"].is_null(), "{response}");
    }
    assert_eq!(out.len(), 6, "{out:?}");
    assert_eq!(out[5]["id"], 9, "the session did not survive: {out:?}");
}

#[test]
fn a_tools_call_under_another_name_is_refused() {
    // The one tool has one name. Anything else ran `code_query` regardless.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"rules","arguments":{"query":"?- def(S, F, K, N)."}}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "arguments":{"query":"?- def(S, F, K, N)."}}}),
        ],
    );
    assert_eq!(out.len(), 2, "{out:?}");
    for response in &out {
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains(codeintel::mcp::TOOL)),
            "{response}"
        );
    }
}

#[test]
fn a_request_with_no_method_is_answered_rather_than_ignored() {
    // A caller blocked forever on a silent id is the worst outcome this server
    // has. Anything carrying an `id` gets a reply, including one that cannot be
    // routed.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":9}),
            serde_json::json!({"jsonrpc":"2.0","id":10,"method":"ping"}),
        ],
    );
    assert_eq!(out.len(), 2, "a request went unanswered: {out:?}");
    assert_eq!(out[0]["id"], 9);
    assert_eq!(out[0]["error"]["code"], -32600);
    assert_eq!(out[1]["id"], 10);
}

#[test]
fn an_mcp_error_result_still_carries_a_status() {
    // The promise is that a consumer never parses prose to find the status.
    // The error paths are exactly where it would otherwise have to.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc_structured(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"code_query","arguments":{"query":"?- def(S, F, K, N)."}}}),
        ],
    );
    let result = &out[0]["result"];
    assert_eq!(result["structuredContent"]["status"], "no-index");
    assert!(result["structuredContent"]["hint"].is_string(), "{result}");
}

#[test]
fn schema_json_carries_the_counts_structurally() {
    // The whole argument for the text form is that a value at 0 is a value not
    // to query. A consumer that has to regex prose to learn that is in the same
    // position `query` puts one in when `display` is the only thing on offer.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = stdout(dir.path(), &["schema", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");

    assert!(
        json["text"]
            .as_str()
            .is_some_and(|t| t.contains("RELATIONS"))
    );
    assert_eq!(json["indexed"], true);
    assert_eq!(json["status"], "ok", "{json}");
    assert!(json["hint"].is_null(), "{json}");

    // Every relation, with its argument names and its row count.
    let relations = json["relations"].as_array().expect("relations");
    assert_eq!(relations.len(), facts::RELATIONS.len());
    for rel in relations {
        assert!(rel["args"].as_str().is_some_and(|a| a.starts_with('(')));
        assert!(rel["rows"].is_number());
    }

    // Closed vocabularies, zeros included.
    for kind in extract::lang::KINDS {
        assert!(json["kinds"][*kind].is_number(), "{kind} missing");
    }
    for role in extract::lang::ROLES {
        assert!(json["roles"][*role].is_number(), "{role} missing");
    }
    // This fixture is Rust with no SCIP: functions yes, classes no.
    assert!(json["kinds"]["function"].as_u64().unwrap_or(0) > 0);
    assert_eq!(json["kinds"]["class"], 0);

    // Rules come through with their signatures, not just names.
    let rules = json["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), schema::rules(codeintel::STDLIB).len());
    assert!(
        rules
            .iter()
            .all(|r| r["head"].as_str().is_some_and(|h| h.contains('(')))
    );
}

/// A tree of `rows` functions whose doc comments are long and made of the
/// characters JSON escapes: quote, backslash, tab. Each costs two bytes in a
/// JSON string, and four once that string is escaped again into an MCP result.
fn escaping_tree(rows: usize) -> tempfile::TempDir {
    use std::fmt::Write as _;
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    let doc = "\"\\\t".repeat(60);
    let mut source = String::new();
    for i in 0..rows {
        writeln!(source, "/// {doc}\npub fn f{i:04}() {{}}").expect("writes to a String");
    }
    std::fs::write(dir.path().join("src/lib.rs"), source).expect("write");
    dir
}

/// Invariant 9 on the MCP wire: the result object, not the printed text, is
/// what `max_result_bytes` bounds. Measured on the text alone, a capped answer
/// went out several times over the budget (#49). The cap holds for both bodies:
/// the text in `content` for a client that declared nothing, the JSON in
/// `structuredContent` for one that takes it.
#[test]
fn an_mcp_result_fits_max_result_bytes_as_sent() {
    let dir = escaping_tree(2500);
    run(dir.path(), &["index", "."]);
    let budget = datalog::Limits::default().max_result_bytes;
    let call = |id: u64| {
        serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{
            "name":"code_query",
            "arguments":{"query":"?- def_doc(S, D).","limit":5000}}})
    };
    let text_out = rpc(dir.path(), &[call(1)]);
    let json_out = rpc_structured(dir.path(), &[call(1)]);

    for response in text_out.iter().chain(&json_out) {
        let sent = response["result"].to_string().len();
        assert!(sent <= budget, "{sent} bytes sent against {budget}");
    }

    let structured = &json_out[0]["result"]["structuredContent"];
    assert_eq!(structured["status"], "truncated", "{structured}");
    assert_eq!(structured["truncated"], true);
    assert_eq!(structured["cap"], "max_result_bytes");
    // A cut keeps a prefix, not nothing, and one notation of it.
    let kept = structured["rows"].as_array().map_or(0, Vec::len);
    assert!((10..2500).contains(&kept), "{kept} rows");
    assert!(structured.get("display").is_none(), "{structured}");
    assert!(
        json_out[0]["result"]["content"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    let result = &text_out[0]["result"];
    assert!(result.get("structuredContent").is_none(), "{result}");
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(text.lines().count() > 10, "{text}");
    assert!(text.contains("status=truncated"), "{text}");
}

/// One answer, one text: a ground goal that holds prints `true` in an MCP
/// result exactly as on the CLI, not a blank line nobody can see (#51).
#[test]
fn an_mcp_ground_goal_prints_true_like_the_cli() {
    let dir = escaping_tree(3);
    run(dir.path(), &["index", "."]);
    let call = |id: u64, goal: &str| {
        serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{
            "name":"code_query","arguments":{"query":goal}}})
    };
    let out = rpc(
        dir.path(),
        &[
            call(1, "?- def(_, _, _, _)."),
            call(2, r#"?- def(_, _, _, "no_such_name_anywhere")."#),
        ],
    );
    let text = |i: usize| {
        out[i]["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .to_string()
    };
    assert!(text(0).starts_with("true\nstatus=ok\n"), "{}", text(0));
    assert!(text(1).starts_with("status=ok\n"), "{}", text(1));
}

/// The catalog reaches a client in the one field it reads, and only that one.
/// `content` for a client that declared nothing; `structuredContent` for one
/// that takes it, carrying the catalog text once rather than a second time as
/// rules, relations and counts (`specs/05-surface.md` § The consumer).
#[test]
fn the_mcp_schema_reaches_the_field_the_client_reads_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let call = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"code_query","arguments":{"schema":true}}});
    let plain = rpc(dir.path(), std::slice::from_ref(&call));
    let structured = rpc_structured(dir.path(), std::slice::from_ref(&call));

    let result = &plain[0]["result"];
    assert!(result.get("structuredContent").is_none(), "{result}");
    let text = result["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("innermost_at"), "{text}");

    let result = &structured[0]["result"];
    assert_eq!(result["content"], serde_json::json!([]), "{result}");
    assert_eq!(result["structuredContent"]["status"], "no-index");
    assert_eq!(result["structuredContent"]["text"].as_str(), Some(text));
    assert!(
        result["structuredContent"].get("relations").is_none(),
        "the catalog was sent twice: {result}"
    );
}

/// Which body a client gets is decided at `initialize`, and a client that
/// declared nothing, or something older than `structuredContent`, gets
/// `content`.
#[test]
fn the_body_follows_what_the_client_declared_at_initialize() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let call = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"code_query","arguments":{"query":"?- def(S, F, \"function\", N)."}}});
    let bare_init = serde_json::json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}});
    let non_date = initialize("latest");
    let sessions = [
        ("no initialize", vec![call.clone()], false),
        ("empty params", vec![bare_init, call.clone()], false),
        (
            "2024-11-05",
            vec![initialize("2024-11-05"), call.clone()],
            false,
        ),
        (
            "2025-03-26",
            vec![initialize("2025-03-26"), call.clone()],
            false,
        ),
        ("not a date", vec![non_date, call.clone()], false),
        (
            "2025-06-18",
            vec![initialize("2025-06-18"), call.clone()],
            true,
        ),
        (
            "2025-11-25",
            vec![initialize("2025-11-25"), call.clone()],
            true,
        ),
    ];
    for (name, requests, structured) in sessions {
        let out = rpc(dir.path(), &requests);
        let result = &out[out.len() - 1]["result"];
        assert_eq!(
            result.get("structuredContent").is_some(),
            structured,
            "{name}: {result}"
        );
        let content = result["content"].as_array().expect("content is required");
        assert_eq!(content.is_empty(), structured, "{name}: {result}");
        if requests.len() == 2 {
            let negotiated = out[0]["result"]["protocolVersion"].as_str();
            let expected = if structured {
                "2025-06-18"
            } else {
                "2024-11-05"
            };
            assert_eq!(negotiated, Some(expected), "{name}");
        }
    }
}

/// `raw` selects the one notation of `rows`, in the text body and the JSON one,
/// and `format` is no longer an argument: the client decided.
#[test]
fn raw_selects_the_one_notation_and_format_is_gone() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let call = |raw: bool| {
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"code_query",
            "arguments":{"query":"?- def(S, F, \"function\", N).","raw":raw}}})
    };
    let shown = rpc_structured(dir.path(), &[call(false)]);
    let raw = rpc_structured(dir.path(), &[call(true)]);
    let (shown, raw) = (
        &shown[0]["result"]["structuredContent"],
        &raw[0]["result"]["structuredContent"],
    );
    assert_ne!(shown["rows"], raw["rows"], "raw changed nothing");
    assert_eq!(
        shown["rows"].as_array().map(Vec::len),
        raw["rows"].as_array().map(Vec::len)
    );

    let refused = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"code_query","arguments":{"query":"?- file(F, L).","format":"json"}}}),
        ],
    );
    let message = refused[0]["error"]["message"].as_str().expect("an error");
    assert!(message.contains("no argument `format`"), "{message}");
}

/// The same on the CLI's JSON: both notations of every row, escaped, are what
/// stdout carries, so they are what the cap measures.
#[test]
fn query_json_fits_max_result_bytes_with_escaping_counted() {
    let dir = escaping_tree(2500);
    run(dir.path(), &["index", "."]);
    let budget = datalog::Limits::default().max_result_bytes;
    let out = run(
        dir.path(),
        &[
            "query",
            "?- def_doc(S, D).",
            "--limit",
            "5000",
            "--format",
            "json",
        ],
    );
    assert!(
        out.stdout.len() <= budget,
        "{} bytes on stdout against {budget}",
        out.stdout.len()
    );
    let body: serde_json::Value = stdout_json(&out);
    assert_eq!(body["truncated"], true, "{body}");
    assert_eq!(body["cap"], "max_result_bytes");
    let kept = body["rows"].as_array().map_or(0, Vec::len);
    assert!((10..2500).contains(&kept), "{kept} rows");
}

/// An I/O failure is not `corrupt`: that status's repair deletes a healthy
/// index. MCP answers it as a JSON-RPC error and the CLI exits 2
/// (`specs/05-surface.md` § MCP).
#[cfg(unix)]
#[test]
fn an_unreadable_segment_is_an_io_error_not_corrupt() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tree();
    run(dir.path(), &["index", "."]);
    let seg = std::fs::read_dir(dir.path().join(".codeintel/seg"))
        .expect("segments")
        .next()
        .expect("one segment")
        .expect("entry")
        .path();
    std::fs::set_permissions(&seg, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    if std::fs::read(&seg).is_ok() {
        // Root reads through mode 000; there is no I/O failure to provoke.
        return;
    }

    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"code_query","arguments":{"query":"?- file(F, _)."}}}),
        ],
    );
    assert_eq!(out[0]["error"]["code"], -32603, "{:?}", out[0]);

    let cli = run(dir.path(), &["query", "?- file(F, _)."]);
    let stderr = String::from_utf8_lossy(&cli.stderr);
    assert_eq!(cli.status.code(), Some(2), "{stderr}");
    assert!(!stderr.contains("status=corrupt"), "{stderr}");
}

#[test]
fn status_notices_a_file_added_after_the_index() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    std::fs::write(dir.path().join("src/added.rs"), "pub fn added() {}\n").expect("write");

    let text = stdout(dir.path(), &["status"]);
    assert!(text.starts_with("status: stale"), "{text}");
    assert!(text.contains("src/added.rs"), "{text}");
}

fn write_at(path: &Path, text: &str, mtime: std::time::SystemTime) {
    std::fs::write(path, text).expect("write");
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_modified(mtime)
        .expect("set mtime");
}

/// An edit in the second a refresh read the file, at the same length, leaves
/// mtime and size as recorded. The racy-clean rule hashes it anyway
/// (`specs/04-storage.md` § Manifest).
#[test]
fn a_same_second_same_length_edit_is_seen() {
    let dir = tree();
    let file = dir.path().join("src/racy.rs");
    // A future mtime stands in for "the second the refresh read it": it is at
    // or past `indexed_at`, which is what makes the file suspect.
    let second = std::time::SystemTime::now() + std::time::Duration::from_secs(3_600);
    write_at(&file, "pub fn alpha() {}\n", second);
    run(dir.path(), &["index", "."]);
    write_at(&file, "pub fn omega() {}\n", second);

    let text = stdout(dir.path(), &["status"]);
    assert!(text.starts_with("status: stale"), "{text}");
    assert!(text.contains("src/racy.rs"), "{text}");

    let rows = stdout(dir.path(), &["query", r#"?- def(_, "src/racy.rs", _, N)."#]);
    assert!(rows.contains("omega"), "{rows}");
    assert!(!rows.contains("alpha"), "{rows}");
}

#[test]
fn status_reports_another_schema_as_stale_without_counts() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let path = dir.path().join(".codeintel/manifest.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("reads")).expect("JSON");
    json["schema_version"] = 999.into();
    std::fs::write(&path, json.to_string()).expect("writes");

    let out = run(dir.path(), &["status"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.starts_with("status: stale"), "{text}");
    assert!(text.contains("schema_version 999"), "{text}");
}

#[test]
fn status_reports_a_store_that_will_not_load_as_corrupt() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let seg = std::fs::read_dir(dir.path().join(".codeintel/seg"))
        .expect("segments")
        .next()
        .expect("one segment")
        .expect("entry")
        .path();
    let mut bytes = std::fs::read(&seg).expect("reads");
    if let Some(byte) = bytes.first_mut() {
        *byte ^= 0xff;
    }
    std::fs::write(&seg, bytes).expect("writes");

    let out = run(dir.path(), &["status", "--format", "json"]);
    assert_eq!(out.status.code(), Some(2));
    let json: serde_json::Value = stdout_json(&out);
    assert_eq!(json["status"], "corrupt", "{json}");
}

#[test]
fn an_argument_the_tool_does_not_have_is_refused() {
    // `path` was accepted and ignored. An MCP server started in one repository
    // and asked for `path: "<another repository>"` answered from its own tree
    // with `status: "ok"` — a well-formed, confident answer about the wrong
    // codebase. The CLI takes `--path`, the tool description never said the
    // repository is fixed, so it is the first argument an agent invents.
    let dir = tree();
    run(dir.path(), &["index", "."]);

    let elsewhere = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- def(S, F, \"function\", N).",
                             "path": elsewhere.path().to_string_lossy()}}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- def(S, F, \"function\", N).", "bogus": 1}}}),
            // The arguments it does have still work, so this is a guard and not
            // a wall.
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- def(S, F, \"function\", N).",
                             "limit": 2, "raw": true}}}),
        ],
    );

    let message = out[0]["error"]["message"]
        .as_str()
        .expect("an unknown argument is an error, not an answer");
    assert!(message.contains("no argument `path`"), "{message}");
    assert!(message.contains("working directory"), "{message}");
    assert!(out[0]["result"].is_null(), "{}", out[0]);

    let message = out[1]["error"]["message"].as_str().expect("error");
    assert!(message.contains("no argument `bogus`"), "{message}");

    assert!(out[2]["error"].is_null(), "{}", out[2]);
    assert!(
        out[2]["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("status=truncated")),
        "{}",
        out[2]
    );

    // And the declaration a validating client reads says the same thing.
    let tools = rpc(
        dir.path(),
        &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    assert_eq!(
        tools[0]["result"]["tools"][0]["inputSchema"]["additionalProperties"],
        serde_json::Value::Bool(false)
    );
}

/// The advertised doc for `uses_package` must name the trap. Regression for #58.
///
/// `schema` is what an agent reads before writing a query, and "F references
/// external package P" sent it to `?- uses_package(F, "react").` — zero rows
/// on a TypeScript repository where every file imports `react`, because
/// scip-typescript resolves those symbols into `@types/react`. The relation is
/// truthful about `extern`; the advertisement was not truthful about which
/// name `P` holds. The fix for #58 is documentation, so the documentation is
/// what this test holds in place.
#[test]
fn uses_package_advertises_whose_package_name_it_reports() {
    for name in ["uses_package", "uses_package_exact"] {
        let rule = schema::rules(codeintel::STDLIB)
            .into_iter()
            .find(|r| schema::split_head(&r.head).is_some_and(|(n, _)| n == name))
            .unwrap_or_else(|| panic!("`{name}` is not advertised"));
        assert!(
            rule.doc.contains("INDEXER") || rule.doc.contains("indexer"),
            "`{name}`'s doc must say the package name is the indexer's, not \
             the import's (#58): {:?}",
            rule.doc
        );
    }

    let uses_package = schema::rules(codeintel::STDLIB)
        .into_iter()
        .find(|r| schema::split_head(&r.head).is_some_and(|(n, _)| n == "uses_package"))
        .expect("advertised");
    assert!(
        uses_package.doc.contains("import"),
        "`uses_package`'s doc must point at the relation that answers what a \
         file declares (#58): {:?}",
        uses_package.doc
    );
}

/// `schema` counts and `query` counts for one relation, on the same tree.
fn schema_and_query_rows(root: &Path, relation: &str, goal: &str) -> (u64, u64) {
    let schema = stdout(root, &["schema", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&schema).expect("JSON");
    let in_schema = json
        .get("relations")
        .and_then(serde_json::Value::as_array)
        .expect("relations")
        .iter()
        .find(|r| r.get("name").is_some_and(|n| n == relation))
        .and_then(|r| r.get("rows")?.as_u64())
        .expect("the relation is in the catalog");
    let counted = stdout(root, &["query", &format!("?- N = count{{{goal}}}.")]);
    let in_query = counted
        .lines()
        .find_map(|line| line.trim().parse::<u64>().ok())
        .expect("a count");
    (in_schema, in_query)
}

#[test]
fn schema_counts_equal_what_query_sees_on_a_tree_edited_since_indexing() {
    // The catalog says "Counts from THIS index. A value at 0 returns no rows"
    // and an agent decides what to ask from it. It must describe the database
    // `query` answers from, which refreshes on every call.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let before = schema_and_query_rows(dir.path(), "def", "S: def(S, _, _, _)");

    std::fs::write(
        dir.path().join("src/added.rs"),
        "pub fn one() {}\npub fn two() {}\npub fn three() {}\n",
    )
    .expect("write");
    let (in_schema, in_query) = schema_and_query_rows(dir.path(), "def", "S: def(S, _, _, _)");
    assert_eq!(in_schema, in_query, "schema and query disagree on `def`");
    assert!(in_schema >= before.0 + 3, "{in_schema} vs {}", before.0);

    let files = |root: &Path| schema_and_query_rows(root, "file", "F: file(F, _)");
    std::fs::remove_file(dir.path().join("src/added.rs")).expect("remove");
    let (in_schema, in_query) = files(dir.path());
    assert_eq!(in_schema, in_query, "schema and query disagree on `file`");
}

#[test]
fn schema_says_stale_when_it_could_not_refresh() {
    // Invariant 5: a catalog counted from an index it could not bring up to
    // date says so, the way `query` does.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let mut lock = facts::Lock::open(&dir.path().join(".codeintel")).expect("opens");
    let held = lock.try_hold().expect("no io error").expect("uncontended");

    let out = stdout(dir.path(), &["schema", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(json["status"], "stale", "{json}");
    assert!(
        json["hint"]
            .as_str()
            .is_some_and(|h| h.contains("codeintel index")),
        "{json}"
    );
    drop(held);
}
