//! The bench harness's load gate, tested where tests actually run.
//!
//! `benches/conditions.rs` is `#[path]`-included by the bench targets, and
//! those are `harness = false` — `cargo bench` calls their `main` and no
//! libtest ever collects a `#[test]` inside them. Pulling the module in here
//! puts its unit tests on a target that does run them.

#[path = "../benches/conditions.rs"]
mod conditions;

use conditions::{Conditions, MAX_LOAD_PER_CORE, Verdict, numbers};

#[test]
fn the_threshold_is_the_one_the_argument_justifies() {
    // The module docs argue from measurements at 0.33 and at 1.2+ per core.
    // If this figure moves, that argument has to move with it.
    assert!(
        (MAX_LOAD_PER_CORE - 0.5).abs() < f64::EPSILON,
        "the threshold changed; update the argument in benches/conditions.rs"
    );
}

#[test]
fn a_busy_host_is_refused_and_an_idle_one_is_not() {
    let busy = Conditions {
        load1: Some(24.0),
        cores: 12,
    };
    let idle = Conditions {
        load1: Some(3.0),
        cores: 12,
    };
    assert!(!busy.trustworthy(), "{}", busy.describe());
    assert!(idle.trustworthy(), "{}", idle.describe());
    // The message has to carry the numbers, or a refusal is not actionable.
    assert!(busy.describe().contains("24"), "{}", busy.describe());
    assert!(busy.describe().contains("12"), "{}", busy.describe());
}

#[test]
fn a_counter_difference_fails_however_busy_the_host_is() {
    // Counters are exact and load does not move them. If a loaded host could
    // downgrade a counter failure to "inconclusive", the gate would lose the
    // one check it can make anywhere.
    let busy = Conditions {
        load1: Some(40.0),
        cores: 12,
    };
    assert_eq!(Verdict::decide(false, busy), Verdict::Fail);
}

#[test]
fn agreeing_counters_on_a_busy_host_are_inconclusive_not_a_pass() {
    let busy = Conditions {
        load1: Some(40.0),
        cores: 12,
    };
    let idle = Conditions {
        load1: Some(1.0),
        cores: 12,
    };
    assert_eq!(Verdict::decide(true, busy), Verdict::Inconclusive);
    assert_eq!(Verdict::decide(true, idle), Verdict::Pass);
}

#[test]
fn the_real_host_reads_without_panicking() {
    // Not an assertion about this machine's load — that would be a test whose
    // result depends on what else is running. Only that the read works and
    // reports something a human can act on.
    let now = Conditions::now();
    assert!(now.cores > 0, "available_parallelism found no cores");
    let described = now.describe();
    assert!(
        described.contains("per core") || described.contains("unavailable"),
        "{described}"
    );
}

#[test]
fn sysctl_and_proc_spellings_both_parse() {
    // macOS: `sysctl -n vm.loadavg` wraps them in braces.
    assert_eq!(numbers("{ 1.25 2.50 3.75 }"), vec![1.25, 2.50, 3.75]);
    // Linux: `/proc/loadavg` trails them with a run count and a last pid.
    assert_eq!(numbers("1.25 2.50 3.75 1/234 5678"), vec![1.25, 2.50, 3.75]);
    assert!(numbers("nothing here").is_empty());
}

#[test]
fn the_limit_is_per_core_not_absolute() {
    // The same load average is fine on a big machine and not on a small one.
    // A bare number would have said otherwise.
    let busy = Conditions {
        load1: Some(6.0),
        cores: 4,
    };
    let idle = Conditions {
        load1: Some(6.0),
        cores: 32,
    };
    assert!(!busy.trustworthy(), "{}", busy.describe());
    assert!(idle.trustworthy(), "{}", idle.describe());
}

#[test]
fn an_unreadable_load_average_does_not_block_the_harness() {
    // A platform this cannot read should not make the bench unusable — but
    // the caller says so out loud rather than assuming the host was idle.
    let unknown = Conditions {
        load1: None,
        cores: 8,
    };
    assert!(unknown.trustworthy());
    assert_eq!(unknown.per_core(), None);
    assert!(unknown.describe().contains("unavailable"));
}

#[test]
fn an_unreadable_load_average_still_refuses_to_commit_a_baseline() {
    // The asymmetry that keeps a parser bug from becoming a committed number:
    // unknown conditions are good enough to report numbers under and not good
    // enough to write a baseline from. This harness shipped with a load parser
    // that returned nothing on macOS, and nothing read as unknown, and unknown
    // read as fine — so every baseline would have been approved by a gate that
    // looked like it was working.
    let unknown = Conditions {
        load1: None,
        cores: 8,
    };
    assert!(unknown.trustworthy(), "reporting is still allowed");
    assert!(!unknown.safe_to_commit(), "writing a baseline is not");

    // And the two agree whenever the reading is real, in both directions, so
    // the asymmetry is confined to the unknown case.
    let idle = Conditions {
        load1: Some(1.0),
        cores: 8,
    };
    let busy = Conditions {
        load1: Some(12.0),
        cores: 8,
    };
    assert!(idle.trustworthy() && idle.safe_to_commit());
    assert!(!busy.trustworthy() && !busy.safe_to_commit());
}

#[test]
fn the_boundary_is_inclusive() {
    let exactly = Conditions {
        load1: Some(MAX_LOAD_PER_CORE * 8.0),
        cores: 8,
    };
    assert!(exactly.trustworthy(), "at the limit is still allowed");
}
