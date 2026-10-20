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
    assert_eq!(Verdict::decide(false, busy.trustworthy()), Verdict::Fail);
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
    assert_eq!(
        Verdict::decide(true, busy.trustworthy()),
        Verdict::Inconclusive
    );
    assert_eq!(Verdict::decide(true, idle.trustworthy()), Verdict::Pass);
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

#[test]
fn a_run_id_names_the_code_that_produced_it() {
    // #78: two builds writing one path is how a parallel-walk result got
    // quoted as the serial walk's. The id has to change when the code does.
    let id = conditions::run_id();
    assert!(!id.is_empty(), "a run always has an identity");
    assert!(
        !id.contains('/') && !id.contains(' '),
        "a run id becomes a filename: {id}"
    );
    // This tree is dirty while the change is being written and clean after it
    // lands, so assert the shape of whichever case is live rather than
    // pinning one and failing in the other.
    if conditions::dirty_tree() {
        assert!(
            id.contains("-dirty-"),
            "a dirty tree's rev alone does not identify what was built: {id}"
        );
    } else {
        assert!(
            !id.contains("-dirty-"),
            "a clean tree is identified by its rev alone: {id}"
        );
    }
}

#[test]
fn a_run_that_started_quiet_and_ended_busy_is_not_committable() {
    // The case the start-only check missed: a deno run began at 0.39 per core
    // and ended at 1.99 because Defender and an unrelated build woke up inside
    // it. Every number came from across that ramp.
    let quiet = Conditions {
        load1: Some(3.85),
        cores: 10,
    };
    let busy = Conditions {
        load1: Some(19.89),
        cores: 10,
    };
    assert!(quiet.safe_to_commit(), "the start was genuinely fine");
    assert!(
        !conditions::held_steady(quiet, busy),
        "a run interrupted mid-flight is not a baseline"
    );
    assert!(conditions::held_steady(quiet, quiet));
}

#[test]
fn the_end_bound_leaves_room_for_the_benchs_own_threads() {
    // The walk runs on four threads, so the bench raises load by roughly half
    // a runnable thread per core. The end bound has to tolerate that or the
    // biggest corpus could never be baselined at all.
    let start = Conditions {
        load1: Some(3.0),
        cores: 10,
    };
    let ours = Conditions {
        load1: Some(8.0),
        cores: 10,
    };
    assert!(
        conditions::held_steady(start, ours),
        "0.8 per core at the end is the bench working, not an interruption"
    );
}

#[test]
fn a_comparison_that_did_not_happen_is_never_a_pass() {
    // #83: the baseline came from another machine, so the clock was not
    // judged. Counters still agreeing does not make that a pass — a 4x
    // regression went by unreported in exactly this state.
    assert_eq!(Verdict::decide(true, false), Verdict::Inconclusive);
    // And a counter difference still fails, whatever the clock did.
    assert_eq!(Verdict::decide(false, false), Verdict::Fail);
    assert_eq!(Verdict::decide(false, true), Verdict::Fail);
}

#[test]
fn settling_waits_for_a_quiet_machine_and_reports_giving_up() {
    // #84: the corpus copy is what makes the host busy, so the harness waits
    // after it. A machine that goes quiet ends the wait; one that never does
    // says so rather than pretending the reading was fine.
    let busy = Conditions {
        load1: Some(20.0),
        cores: 10,
    };
    let quiet = Conditions {
        load1: Some(2.0),
        cores: 10,
    };

    let mut reads = 0;
    let settled = conditions::settle_with(
        std::time::Duration::from_secs(10),
        std::time::Duration::from_millis(1),
        || {
            reads += 1;
            if reads < 3 { busy } else { quiet }
        },
    );
    assert!(!settled.timed_out, "the machine went quiet");
    assert!(settled.conditions.safe_to_commit());
    assert_eq!(reads, 3, "it kept looking until the reading was good");

    let gave_up = conditions::settle_with(
        std::time::Duration::from_millis(5),
        std::time::Duration::from_millis(1),
        || busy,
    );
    assert!(gave_up.timed_out, "a host that stays busy is reported");
    assert!(!gave_up.conditions.safe_to_commit());
}

#[test]
fn settling_costs_nothing_on_a_host_that_is_already_quiet() {
    // The wait must not become a tax on the common case: one reading, no
    // sleep, and a disabled timeout still returns the reading it took.
    let quiet = Conditions {
        load1: Some(1.0),
        cores: 10,
    };
    let settled = conditions::settle_with(
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(60),
        || quiet,
    );
    assert!(!settled.timed_out);
    assert!(settled.waited_s < 1.0, "{} s", settled.waited_s);
}

#[test]
fn the_host_core_count_is_the_os_view_not_this_process() {
    // The whole of #83: available_parallelism answers for the process. If the
    // OS can be asked, its answer is the one that identifies the machine, and
    // it must be sane.
    let Some(os_cores) = conditions::cores() else {
        return; // a platform this does not know; the fallback is tested above
    };
    assert!(os_cores > 0, "the OS reported {os_cores} logical CPUs");
    let process = std::thread::available_parallelism().map_or(0, usize::from);
    assert!(
        os_cores >= process,
        "the host cannot have fewer CPUs than this process may use: {os_cores} < {process}"
    );
}
