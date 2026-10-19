//! The CLI and the single-tool MCP server, over the engine and the fact store.
//!
//! Thin by construction: five verbs, one MCP tool, and a hard surface budget
//! asserted in CI. See `specs/05-surface.md`.
//!
//! This crate is also the *host*: it supplies the engine's regex builtin and
//! its dictionary, which is what keeps `crates/datalog` at zero dependencies.
//!
//! `index` and `query` land at M2; `schema` and `status` at M4. `mcp` is the
//! fifth and last verb.

/// The shipped rule library, compiled in.
///
/// One `include_str!`, not two. It was declared in both `query` (which loads
/// it into the engine) and `schema` (which reads its `%%` docs for the
/// catalog), so the file was embedded in the binary twice and the two paths
/// could have been pointed at different files without a word from the
/// compiler (#48). It belongs to neither module — it is a fact about this
/// build — so it lives here.
pub const STDLIB: &str = include_str!("../../../rules/stdlib.dl");

pub mod census;
pub mod discover;
pub mod index;
pub mod load;
pub mod mcp;
pub mod overlay;
pub mod query;
pub mod regexes;
pub mod render;
pub mod schema;
pub mod status;
pub mod wire;

pub use discover::Nearby;
pub use index::{Plan, Report};
pub use load::load_facts;
pub use query::{Answer, Options, Warm};
pub use regexes::Regexes;
pub use render::{Rendered, Row, Sites, render, render_row};
pub use status::Status;
pub use wire::Wire;
