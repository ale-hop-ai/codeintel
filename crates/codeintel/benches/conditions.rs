//! What the machine was doing while a bench ran, and whether that makes the
//! numbers worth keeping.
//!
//! `CLAUDE.md`: a hand-run table on an unpinned tree is not evidence.
//! `tasks/bench.yml` already refuses to compare a baseline against a different
//! corpus rev for that reason. Load is the same kind of fact — a number
//! measured on a busy machine describes the machine, not the code — and
//! nothing stopped a loaded host from writing an authoritative-looking
//! baseline until this.
//!
//! **Why load average, and why the one-minute figure.** It counts threads
//! runnable but not running, which is exactly the thing that perturbs a
//! wall-clock measurement; CPU utilization does not distinguish "busy with us"
//! from "busy with someone else". Its known weakness is lag: it is an
//! exponentially-weighted average, so it keeps reading high for a while after
//! the machine actually frees up. For a gate protecting a committed artifact
//! that lag is a feature — it errs toward refusing to write, and a refusal
//! costs a re-run while a bad baseline silently weakens the gate for everyone
//! afterwards.
//!
//! **Why measured before the run, not after.** The bench's own work raises
//! load — the walk alone runs on four threads — so an end-of-run reading gates
//! on our own contribution. The start reading is what says whether the machine
//! was free to begin with. Both are recorded; only the start one decides.
//!
//! **Why 0.5 per core.** Measured on the host these corpora are pinned
//! against, during the #28 profiling, which recorded 12 cores in
//! `baseline.json`:
//!
//! | load average | per core | what the phase breakdown did |
//! |---:|---:|---|
//! | 3.95 - 4.19 | 0.33 - 0.35 | stable; p95 within 2% of p50 |
//! | 14 - 25 | 1.2 - 2.1 | p95 ran 2.4x to 6x p50 |
//!
//! At the high readings one run produced an impossible breakdown — a negative
//! derived phase — because the host moved underneath it. The line sits above
//! the known-good readings and well below the known-bad ones. It is
//! deliberately not "one per core": a fully-subscribed machine is already
//! queueing, and the bench wants about five cores of its own.
//!
//! **The denominator is approximate, in the safe direction.** Load average is
//! a system-wide queue depth, so the honest divisor is the machine's CPU
//! count; this uses [`std::thread::available_parallelism`], which is what
//! *this process* may use. The two can differ — the same call that wrote
//! `cpus: 12` into `baseline.json` returns 10 on the same machine now — and
//! when it reads low the gate gets stricter, never looser. A gate that
//! occasionally asks for a quieter machine than strictly necessary is the
//! error worth having.

// Three targets include this with `#[path]` — both benches and the test that
// exercises it — and each uses a different subset. Dead-code warnings here are
// an artefact of that sharing, not an unused item.
#![allow(
    dead_code,
    reason = "three targets include this with #[path] and each uses a different subset"
)]

use std::process::Command;

use serde_json::{Value, json};

/// Load average per core at or below which a wall-clock number is trusted.
/// The argument for this figure is in the module docs; it is measured, not
/// chosen by taste.
pub(crate) const MAX_LOAD_PER_CORE: f64 = 0.5;

/// What the machine was doing at one moment.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Conditions {
    /// One-minute load average, or `None` where the platform has none.
    pub(crate) load1: Option<f64>,
    /// Cores the process can actually use.
    pub(crate) cores: usize,
}

impl Conditions {
    /// Read them now.
    ///
    /// `CODEINTEL_BENCH_LOAD` raises the reading, and can only ever raise it:
    /// it exists so the refusal path can be exercised without making the
    /// machine genuinely busy, and taking the maximum means it cannot be used
    /// the other way, to talk a loaded host into writing a baseline. A test
    /// hook that can only tighten a gate is safe to ship.
    #[must_use]
    pub(crate) fn now() -> Self {
        let measured = load_average().first().copied();
        let forced = std::env::var("CODEINTEL_BENCH_LOAD")
            .ok()
            .and_then(|v| v.parse::<f64>().ok());
        Self {
            load1: match (measured, forced) {
                (Some(m), Some(f)) => Some(m.max(f)),
                (m, f) => m.or(f),
            },
            cores: std::thread::available_parallelism().map_or(0, usize::from),
        }
    }

    /// Load average per core, or `None` when either figure is unavailable.
    #[must_use]
    pub(crate) fn per_core(self) -> Option<f64> {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a core count, which is nowhere near 2^53"
        )]
        self.load1
            .filter(|_| self.cores > 0)
            .map(|load| load / self.cores as f64)
    }

    /// Whether a wall-clock number taken under these is worth committing.
    ///
    /// An unreadable load average counts as trustworthy rather than blocking
    /// the harness on a platform this does not know — but the caller says so
    /// out loud, because a silent assumption is the thing invariant 5 forbids.
    #[must_use]
    pub(crate) fn trustworthy(self) -> bool {
        self.per_core().is_none_or(|per| per <= MAX_LOAD_PER_CORE)
    }

    /// One line a human can act on: what was read, and what it means.
    #[must_use]
    pub(crate) fn describe(self) -> String {
        match (self.load1, self.per_core()) {
            (Some(load), Some(per)) => format!(
                "load average {load:.2} over {} core(s) = {per:.2} per core, against a limit of \
                 {MAX_LOAD_PER_CORE:.2}",
                self.cores
            ),
            _ => "load average unavailable on this platform; conditions unknown".to_string(),
        }
    }
}

/// What a run concluded. Three values, not two: a wall-clock comparison made
/// on a loaded host has not passed and has not failed, and reporting either
/// would put a claim the run did not earn into an exit code
/// (`specs/00-overview.md` invariant 5 — degradation is reported, never
/// silent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Pass,
    Fail,
    /// Counters agreed; the host was too busy for the clock to mean anything.
    Inconclusive,
}

impl Verdict {
    /// 0 passed, 1 failed, 2 is a harness error, 3 is "ask again when the
    /// machine is quiet". A distinct code so a script can tell an inconclusive
    /// run from a passing one without parsing prose.
    pub(crate) fn code(self) -> std::process::ExitCode {
        match self {
            Self::Pass => std::process::ExitCode::SUCCESS,
            Self::Fail => std::process::ExitCode::from(1),
            Self::Inconclusive => std::process::ExitCode::from(3),
        }
    }

    /// The precedence: counters are exact and load does not move them, so a
    /// counter difference fails on any host. Only once they agree does the
    /// host's load get to decide whether the clock is worth believing.
    pub(crate) fn decide(counters_agreed: bool, under: Conditions) -> Self {
        if !counters_agreed {
            Self::Fail
        } else if under.trustworthy() {
            Self::Pass
        } else {
            Self::Inconclusive
        }
    }
}

/// Start and end conditions as they are recorded beside a result.
///
/// Deliberately **not** part of `host()`: that value is compared for equality
/// to decide whether two runs are on the same machine, and load differs on
/// every run, so folding it in there would quietly stop wall clock from ever
/// being compared again.
#[must_use]
pub(crate) fn report(start: Conditions, end: Conditions) -> Value {
    json!({
        "load_average_start": start.load1,
        "load_average_end": end.load1,
        "load_per_core_start": start.per_core(),
        "cores": start.cores,
        "max_load_per_core": MAX_LOAD_PER_CORE,
        "trustworthy": start.trustworthy(),
        "dirty_tree": dirty_tree(),
    })
}

/// Whether the repository had uncommitted changes when this ran. A baseline
/// written from a dirty tree describes a build nobody can check out again.
#[must_use]
pub(crate) fn dirty_tree() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| !o.stdout.is_empty())
}

/// The 1, 5 and 15 minute load averages, or empty where there are none.
///
/// Read without a dependency: `sysctl` on the BSDs and macOS, `/proc/loadavg`
/// on Linux. A crate for three floats would not earn its line in
/// `docs/research.md` § 6.
fn load_average() -> Vec<f64> {
    if let Ok(text) = std::fs::read_to_string("/proc/loadavg") {
        return numbers(&text);
    }
    Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| numbers(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// The first three floats in `text`, ignoring the `{ }` that `sysctl` wraps
/// them in and the `1/234 5678` that `/proc/loadavg` trails them with.
///
/// `filter_map`, not `map_while`: stopping at the first unparsable token
/// returned nothing at all for `sysctl`, whose output opens with `{`.
pub(crate) fn numbers(text: &str) -> Vec<f64> {
    text.split_whitespace()
        .filter_map(|t| t.parse::<f64>().ok())
        .take(3)
        .collect()
}
