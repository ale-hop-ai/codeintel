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
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Load average per core at or below which a wall-clock number is trusted.
/// The argument for this figure is in the module docs; it is measured, not
/// chosen by taste.
pub(crate) const MAX_LOAD_PER_CORE: f64 = 0.5;

/// Load per core at the **end** of a run above which something other than the
/// bench must have arrived.
///
/// The start reading cannot catch a machine that was idle when the run began
/// and busy by the time it finished, and that is not hypothetical: a deno run
/// started at 0.39 per core and ended at 1.99, because Defender and an
/// unrelated TypeScript build woke up inside it. Every number it produced was
/// measured across that ramp, and the start-only check called it trustworthy.
///
/// Judged looser than the start, because the bench's own work legitimately
/// raises load — the walk alone runs on four threads, so it can add roughly
/// half a runnable thread per core on a ten-core box. Twice the start limit
/// leaves room for that and still catches an external arrival, which is what
/// this is looking for.
pub(crate) const MAX_END_LOAD_PER_CORE: f64 = MAX_LOAD_PER_CORE * 2.0;

/// How long [`settle`] waits for the machine to go quiet, in seconds, before
/// it gives up and says so.
///
/// Five minutes is longer than the scanner took to work through a copy of the
/// largest corpus and far shorter than the run it protects — deno's is close
/// to an hour, so waiting is cheap next to measuring across a scan and
/// discarding the result.
pub(crate) const SETTLE_TIMEOUT_S: u64 = 300;

/// How often [`settle`] re-reads the load average. The one-minute average
/// cannot move faster than this is worth polling.
const SETTLE_POLL_S: u64 = 5;

/// The logical CPUs the **operating system** reports, which is the stable
/// number, or `None` where it cannot be read.
///
/// Deliberately not [`std::thread::available_parallelism`]. That is documented
/// as the parallelism available to *this process* — it honours affinity masks,
/// cgroup quotas and, on macOS, the scheduler's current disposition — so it is
/// a property of the process and not of the host. It read 12 when the
/// committed baselines were written and 10 the next time anyone looked, on the
/// same machine, which silently disabled every wall-clock comparison (#83).
///
/// For deciding "are these two timings from the same machine", stability beats
/// precision: a number that drifts on one machine skips the comparison
/// silently, while a number that is merely coarse at worst permits a
/// comparison that is noisy and visible.
#[must_use]
pub(crate) fn cores() -> Option<usize> {
    // macOS and the BSDs. `hw.logicalcpu` counts what the OS schedules onto,
    // unaffected by what this process is allowed to use.
    if let Some(n) = sysctl_usize("hw.logicalcpu") {
        return Some(n);
    }
    // Linux. `/proc/cpuinfo` lists one `processor` line per logical CPU;
    // `nproc` is not used because it honours affinity, the same mistake again.
    if let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") {
        let n = text.lines().filter(|l| l.starts_with("processor")).count();
        if n > 0 {
            return Some(n);
        }
    }
    None
}

/// The machine, as a human-readable string, or `None`.
///
/// Recorded beside a result and deliberately **not** part of host identity
/// (see `corpus::host`). Its job is diagnosis: when a comparison is refused
/// because the core count differs, this is what says whether that is the same
/// machine reporting differently or genuinely another one. It answered that
/// question the first time it was asked — the committed baselines record 12
/// cores and this machine is a 10-core Mac17,2, so they came from other
/// hardware and were never going to be comparable.
#[must_use]
pub(crate) fn machine() -> Option<String> {
    let model = sysctl_string("hw.model");
    let cpu = sysctl_string("machdep.cpu.brand_string");
    match (model, cpu) {
        (Some(m), Some(c)) => Some(format!("{m} ({c})")),
        (m, c) => m.or(c),
    }
}

/// A `sysctl` string, or `None`.
fn sysctl_string(name: &str) -> Option<String> {
    let out = Command::new("sysctl").args(["-n", name]).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A `sysctl` integer, or `None`.
fn sysctl_usize(name: &str) -> Option<usize> {
    let out = Command::new("sysctl").args(["-n", name]).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .flatten()
}

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
            // The host's count, so that a system-wide load average is divided
            // by the number of CPUs that load is spread over. Falling back to
            // the process's view is better than dividing by nothing.
            cores: cores()
                .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
                .unwrap_or(0),
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

    /// Whether a reading is good enough to **commit** a baseline.
    ///
    /// Stricter than [`Self::trustworthy`] in exactly one case: an unreadable
    /// load average refuses here and permits there. Writing a baseline is the
    /// one action a re-run does not undo — the committed file then gates every
    /// later comparison — so the unknown case has to fall the other way.
    ///
    /// The reason is not hypothetical. This harness shipped with a load parser
    /// that stopped at `sysctl`'s leading brace and returned nothing, and
    /// "nothing" read as unknown, and unknown read as fine: on macOS the gate
    /// would have approved every baseline while looking like it worked. A
    /// platform whose load cannot be read is a guess about the future; a
    /// parser that returns nothing is a thing that already happened.
    #[must_use]
    pub(crate) fn safe_to_commit(self) -> bool {
        self.per_core().is_some_and(|per| per <= MAX_LOAD_PER_CORE)
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

    /// The precedence: counters are exact and neither load nor hardware moves
    /// them, so a counter difference fails anywhere. Only once they agree does
    /// it matter whether the clock was actually judged — and if it was not,
    /// for any reason, the answer is `Inconclusive` and never `Pass`. A run
    /// that compared nothing has not passed.
    pub(crate) fn decide(counters_agreed: bool, clock_judged: bool) -> Self {
        match (counters_agreed, clock_judged) {
            (false, _) => Self::Fail,
            (true, true) => Self::Pass,
            (true, false) => Self::Inconclusive,
        }
    }
}

/// What waiting for a quiet machine achieved.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Settled {
    /// The reading the wait ended on. This is the run's start reading.
    pub(crate) conditions: Conditions,
    /// How long it waited, in seconds.
    pub(crate) waited_s: f64,
    /// Whether it gave up rather than seeing a quiet machine.
    pub(crate) timed_out: bool,
}

/// The configured wait, in seconds. `CODEINTEL_BENCH_SETTLE_SECS` overrides
/// it; `0` skips the wait entirely, which is what the tests and an
/// already-quiet CI-style host want.
fn settle_timeout() -> Duration {
    let secs = std::env::var("CODEINTEL_BENCH_SETTLE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(SETTLE_TIMEOUT_S);
    Duration::from_secs(secs)
}

/// Wait for the machine to go quiet, bounded.
///
/// Called **after** the corpus copy and before any measurement, because the
/// copy is what disturbs the host: on macOS the endpoint scanner reads every
/// file the bench has just written, and with the runs sequential each corpus
/// was measured under the previous one's scan as well as its own (#84). A
/// reading taken before the copy describes a machine that no longer exists by
/// the time the first metric is timed.
///
/// Waiting rather than sleeping a fixed amount: the harness already has the
/// number that says whether the host is quiet, so it can watch that number
/// instead of guessing at a duration. Giving up is reported, never silent —
/// the run then proceeds exactly as any other run on a busy host, warning and
/// refusing to write a baseline.
#[must_use]
pub(crate) fn settle() -> Settled {
    settle_with(
        settle_timeout(),
        Duration::from_secs(SETTLE_POLL_S),
        Conditions::now,
    )
}

/// [`settle`] with its clock and its source of readings supplied, so the
/// waiting can be exercised in a test without a busy machine or a real wait.
#[must_use]
pub(crate) fn settle_with(
    timeout: Duration,
    poll: Duration,
    mut read: impl FnMut() -> Conditions,
) -> Settled {
    let started = Instant::now();
    loop {
        let now = read();
        let waited = started.elapsed();
        if now.trustworthy() || waited >= timeout {
            return Settled {
                conditions: now,
                waited_s: waited.as_secs_f64(),
                timed_out: !now.trustworthy(),
            };
        }
        // Saturating: `waited < timeout` here, but a clamp is cheaper than
        // trusting that across an edit.
        std::thread::sleep(poll.min(timeout.saturating_sub(waited)));
    }
}

/// Whether the machine stayed quiet for the whole run.
///
/// A run is only worth committing if it began quiet **and** was still quiet at
/// the end. See [`MAX_END_LOAD_PER_CORE`].
#[must_use]
pub(crate) fn held_steady(start: Conditions, end: Conditions) -> bool {
    start.safe_to_commit()
        && end
            .per_core()
            .is_none_or(|per| per <= MAX_END_LOAD_PER_CORE)
}

/// Start and end conditions as they are recorded beside a result.
///
/// Deliberately **not** part of `host()`: that value is compared for equality
/// to decide whether two runs are on the same machine, and load differs on
/// every run, so folding it in there would quietly stop wall clock from ever
/// being compared again.
#[must_use]
pub(crate) fn report(settled: Settled, end: Conditions) -> Value {
    let start = settled.conditions;
    json!({
        // What the wait for a quiet machine achieved, before the first metric
        // was timed. A wait that gave up is a reported degradation.
        "settle_waited_s": settled.waited_s,
        "settle_timed_out": settled.timed_out,
        "load_average_start": start.load1,
        "load_average_end": end.load1,
        "load_per_core_start": start.per_core(),
        "cores": start.cores,
        "max_load_per_core": MAX_LOAD_PER_CORE,
        "trustworthy": start.trustworthy(),
        "held_steady": held_steady(start, end),
        "max_end_load_per_core": MAX_END_LOAD_PER_CORE,
        "dirty_tree": dirty_tree(),
        "machine": machine(),
        "run_id": run_id(),
    })
}

/// What produced a result, as a filename fragment.
///
/// A run's identity is the **code** that produced it, because that is what the
/// numbers describe. Clean tree: the short rev. Dirty tree: the rev is a lie on
/// its own, so a fingerprint of the actual working state goes with it —
/// `<rev>-dirty-<8 hex>` — and two different mid-change builds land in
/// different files. Mid-change profiling is exactly when `docs/plan.md` § #28
/// went wrong, so it is the case that most needs distinguishing.
///
/// Two runs of **identical** code still overwrite each other, deliberately:
/// they describe the same thing, so the newer one supersedes, and stamping a
/// timestamp on every run would fill `target/` with files that answer no
/// question the first one did not. Keep a result you care about by copying it.
#[must_use]
pub(crate) fn run_id() -> String {
    let rev = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "norev".to_string());
    if !dirty_tree() {
        return rev;
    }
    // Tracked edits and the list of untracked paths both change what was
    // built, so both feed the fingerprint. `git diff HEAD` covers staged and
    // unstaged tracked changes; the porcelain list catches a new file that no
    // diff mentions.
    let mut state = git(&["diff", "HEAD"]).unwrap_or_default();
    state.push_str(&git(&["status", "--porcelain"]).unwrap_or_default());
    let hash = facts::content_hash(state.as_bytes());
    let short: String = hash.trim_start_matches("blake3:").chars().take(8).collect();
    format!("{rev}-dirty-{short}")
}

/// A git command's trimmed stdout, or `None` when it fails.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether the repository had uncommitted changes when this ran. A baseline
/// written from a dirty tree describes a build nobody can check out again.
#[must_use]
pub(crate) fn dirty_tree() -> bool {
    git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty())
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
