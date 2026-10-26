//! What an answer looks like on each wire it is sent on, and what it costs.
//!
//! `max_result_bytes` bounds output in bytes because the consumer is a context
//! window (`specs/00-overview.md` invariant 9). The printed text is not what
//! every surface sends: the CLI's JSON carries both notations of every row, and
//! an MCP result escapes its body into a JSON string. So the cap is measured by
//! encoding the answer the way it is sent, **with the same functions that send
//! it** — a size estimate kept beside a separate encoder is a second copy of
//! the format, and the two drift (`specs/05-surface.md` § Response contract).

use crate::query::Answer;

/// Where an answer goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Wire {
    /// `codeintel query`: rows on stdout, the status on stderr.
    #[default]
    Text,
    /// `codeintel query --format json`: one JSON document on stdout.
    Json,
    /// An MCP `tools/call` result for a client that reads `content`: the text
    /// body, and nothing in `structuredContent`.
    McpText,
    /// An MCP `tools/call` result for a client that reads `structuredContent`:
    /// the JSON body, and nothing in `content`.
    McpJson,
}

impl Wire {
    /// The bytes `answer` costs sent this way.
    ///
    /// For MCP this is the tool result object — everything except the
    /// JSON-RPC envelope, whose `id` is whatever the client chose to send.
    #[must_use]
    pub fn bytes(self, answer: &Answer, raw: bool) -> usize {
        match self {
            Self::Text => body(answer, raw).len(),
            Self::Json => json(answer, raw).to_string().len() + 1,
            Self::McpText | Self::McpJson => tool_result(answer, self == Self::McpJson, raw)
                .to_string()
                .len(),
        }
    }
}

/// The full JSON answer, in the shape `specs/05-surface.md` § Response contract
/// specifies: `status`, `columns`, `rows`, `truncated`, `cap`, `hint`, `stats`.
/// `rows` is one notation, chosen by `raw`: the expanded form by default, the
/// raw atoms under `raw` (`specs/05-surface.md` § The consumer).
#[must_use]
pub fn json(answer: &Answer, raw: bool) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("status".into(), answer.status.as_str().into());
    map.insert("columns".into(), serde_json::json!(answer.columns));
    // Values are carried structurally rather than tab-joined and split back
    // apart: a doc comment containing a tab would otherwise arrive as more
    // values than there are columns.
    let rows: Vec<&Vec<String>> = answer
        .rows
        .iter()
        .map(|r| if raw { &r.raw } else { &r.display })
        .collect();
    map.insert("rows".into(), serde_json::json!(rows));
    map.insert("truncated".into(), answer.truncated.into());
    // The wire form of the cap is its limit's name, unchanged by `Cap`
    // becoming an enum: a consumer reads it to look the number up in
    // `Limits`.
    map.insert(
        "cap".into(),
        serde_json::json!(answer.cap.map(datalog::Cap::as_str)),
    );
    map.insert("hint".into(), serde_json::json!(answer.hint));
    // Always present, `null` for "the root you named". An absent key reads as
    // "not measured"; `null` reads as "nothing moved"
    // (`specs/05-surface.md` § Response contract).
    map.insert("root".into(), serde_json::json!(answer.root));
    map.insert(
        "stats".into(),
        // No `elapsed_ms`. It is the one field that cannot be a function of the
        // repo state and the query, and invariant 8 says the same repo state and
        // the same query produce byte-identical output. The CLI prints it on
        // stderr, beside `status=`, where a human debugging a slow query wants
        // it and no consumer is diffing.
        {
            let mut stats = serde_json::Map::new();
            stats.insert("derived".into(), answer.derived.into());
            stats.insert("refreshed".into(), answer.refreshed.into());
            stats.insert("transformed".into(), serde_json::json!(answer.transformed));
            stats.insert("demand".into(), answer.demand.as_str().into());
            stats.insert("depends".into(), serde_json::json!(answer.depends));
            stats.insert("shadowed".into(), serde_json::json!(answer.shadowed));
            stats.insert("widened".into(), serde_json::json!(answer.widened));
            // What the index could not see, without prose. Omitted at zero so
            // the common answer pays nothing (`specs/05-surface.md` § `query`,
            // #130, #132).
            if answer.unread > 0 {
                stats.insert("unread".into(), answer.unread.into());
            }
            if answer.scip_uncovered > 0 {
                stats.insert("scip_uncovered".into(), answer.scip_uncovered.into());
            }
            serde_json::Value::Object(stats)
        },
    );
    serde_json::Value::Object(map)
}

/// Rows as text, with the status and hint appended.
///
/// The status is never omitted: `ok` with zero rows and a missing index must be
/// distinguishable without reading prose (`specs/00-overview.md` invariant 6),
/// and in an MCP result there is no stderr to put it on.
#[must_use]
pub fn text(answer: &Answer, raw: bool) -> String {
    let mut out = body(answer, raw);
    verdict(
        &mut out,
        answer.status,
        answer.root.as_deref(),
        answer.hint.as_deref(),
    );
    out
}

/// The `status=`, `root=` and `hint:` lines that end a text result. `query` and
/// `schema` both end in them, through here, so the two cannot drift.
pub fn verdict(
    out: &mut String,
    status: crate::status::Status,
    root: Option<&str>,
    hint: Option<&str>,
) {
    out.push_str("status=");
    out.push_str(status.as_str());
    out.push('\n');
    // Only on a redirect. A line saying the root is the root on every answer
    // is noise, and no redirect is what the caller already assumed
    // (`specs/05-surface.md` § Response contract).
    if let Some(root) = root {
        out.push_str("root=");
        out.push_str(root);
        out.push('\n');
    }
    if let Some(hint) = hint {
        out.push_str("hint: ");
        out.push_str(hint);
        out.push('\n');
    }
}

/// The rows as stdout carries them: one per line, or `true` for a ground goal
/// that holds.
///
/// A ground goal has no columns: truth is one empty row and falsehood is none
/// (`specs/03-datalog.md` § Evaluation). Printed literally that is a bare
/// newline versus nothing — an answer no one can see. The CLI and an MCP text
/// result both print through here, so they print the same answer
/// (`specs/05-surface.md` § MCP).
#[must_use]
pub fn body(answer: &Answer, raw: bool) -> String {
    if answer.columns.is_empty() && !answer.rows.is_empty() {
        return "true\n".to_string();
    }
    let mut out = String::new();
    for row in &answer.rows {
        out.push_str(&row.line(raw));
        out.push('\n');
    }
    out
}

/// The catalog as one JSON document: what `codeintel schema --format json`
/// prints and what an MCP `schema: true` call with `format: "json"` carries.
///
/// The counts go in structurally as well as inside `text`. The whole argument
/// for the text form is that a value at 0 is a value not to query — and a
/// consumer that has to regex prose to learn that is the same defect `query`
/// has when `display` is the only thing on offer.
#[must_use]
pub fn schema_json(
    catalog: &crate::schema::Schema<'_>,
    census: &crate::census::Census,
    root: &std::path::Path,
    index_dir: Option<&std::path::Path>,
    refreshed: &(crate::status::Status, Option<String>),
) -> serde_json::Value {
    let (status, hint) = schema_status(census, root, index_dir, refreshed);
    serde_json::json!({
        "status": status.as_str(),
        "hint": hint,
        "text": catalog.to_string(),
        "indexed": census.indexed,
        "rules": crate::schema::rules(crate::STDLIB)
            .iter()
            .map(|r| serde_json::json!({ "head": r.head, "doc": r.doc }))
            .collect::<Vec<_>>(),
        "relations": facts::RELATIONS
            .iter()
            .map(|rel| serde_json::json!({
                "name": rel.name,
                "args": crate::schema::signature(rel.name).0,
                "rows": census.rows(rel.name),
            }))
            .collect::<Vec<_>>(),
        "kinds": counted(extract::lang::KINDS, &census.kinds),
        "roles": counted(extract::lang::ROLES, &census.roles),
        "roles_counted_over": "scip_ref",
    })
}

/// The status a catalog carries: the refresh's own (`ok`, or `stale` when it
/// ran out of budget or could not take the lock, which is what `query` says
/// of the same index), or `no-index` with the hint `query` gives when there is
/// nothing to count (`specs/05-surface.md` § `schema`).
#[must_use]
pub fn schema_status(
    census: &crate::census::Census,
    root: &std::path::Path,
    index_dir: Option<&std::path::Path>,
    refreshed: &(crate::status::Status, Option<String>),
) -> (crate::status::Status, Option<String>) {
    // Counted as none but answered for: a pinned index of another root says
    // whose it is rather than "index this" (#141).
    if census.indexed || refreshed.1.is_some() {
        refreshed.clone()
    } else {
        // `schema` reports on the directory it was given and never adopts
        // another (`specs/05-surface.md` § The nearest index), but its hint is
        // the one `query` gives: the complete chain, and whatever is nearby.
        (
            crate::status::Status::NoIndex,
            Some(crate::discover::no_index_hint(
                root,
                &crate::discover::around(root, index_dir),
            )),
        )
    }
}

/// A closed vocabulary with this index's count against each value, zeros
/// included — a value at 0 is a value not to query.
fn counted(
    vocabulary: &[&str],
    counts: &std::collections::BTreeMap<String, usize>,
) -> serde_json::Value {
    serde_json::Value::Object(
        vocabulary
            .iter()
            .map(|name| {
                let n = counts.get(*name).copied().unwrap_or(0);
                ((*name).to_string(), serde_json::json!(n))
            })
            .collect(),
    )
}

/// One `code_query` result: **one body**, in the field the client reads
/// (`specs/05-surface.md` § The consumer).
///
/// `structured` is what the client declared at `initialize`. A client that
/// takes `structuredContent` gets the JSON document there and an empty
/// `content` (the field is required by the result schema, so it is present and
/// carries nothing); any other client gets the text in `content` and no
/// `structuredContent`. Sending both put every row on the wire two or three
/// times.
#[must_use]
pub fn tool_result(answer: &Answer, structured: bool, raw: bool) -> serde_json::Value {
    // "The query never ran", as the CLI's exit 2 (#135).
    let is_error = !answer.ran();
    if structured {
        // An error also carries its text in `content`: a client that renders
        // an `isError` result from `content` showed "Unknown error" for every
        // invalid query (#144). An error has no rows, so the second copy is a
        // status and a hint, not the answer twice.
        let content = if is_error {
            serde_json::json!([{ "type": "text", "text": text(answer, raw) }])
        } else {
            serde_json::json!([])
        };
        serde_json::json!({
            "content": content,
            "structuredContent": json(answer, raw),
            "isError": is_error,
        })
    } else {
        serde_json::json!({
            "content": [{ "type": "text", "text": text(answer, raw) }],
            "isError": is_error,
        })
    }
}
