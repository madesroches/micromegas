//! Synthetic-clock unit tests for `CircuitBreaker` — fully deterministic,
//! zero sleeps, driven entirely through the `_at(now)` API.

use std::time::{Duration, Instant};

use micromegas_object_cache::circuit_breaker::{
    Admission, CircuitBreaker, CircuitBreakerConfig, Transition,
};

fn breaker(failure_threshold: u32, cooldown: Duration) -> CircuitBreaker {
    CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold,
        cooldown,
    })
}

#[test]
fn below_threshold_stays_allow_and_success_resets_consecutive() {
    let b = breaker(5, Duration::from_secs(3));
    let base = Instant::now();

    for _ in 0..4 {
        assert_eq!(b.record_unresponsive_at(base), Transition::None);
    }
    assert_eq!(b.admit_at(base), Admission::Allow);

    // A success resets the counter.
    assert_eq!(b.record_responsive_at(base), Transition::None);

    for _ in 0..4 {
        assert_eq!(b.record_unresponsive_at(base), Transition::None);
    }
    assert_eq!(
        b.admit_at(base),
        Admission::Allow,
        "4 failures after a reset must not trip a threshold-5 breaker"
    );
}

#[test]
fn trips_at_exactly_failure_threshold() {
    let b = breaker(5, Duration::from_secs(3));
    let base = Instant::now();

    for _ in 0..4 {
        assert_eq!(b.record_unresponsive_at(base), Transition::None);
    }
    let cooldown = Duration::from_secs(3);
    assert_eq!(
        b.record_unresponsive_at(base),
        Transition::Opened { cooldown }
    );
    // Reported exactly once — a further failure while open is a no-op, not
    // another `Opened`.
    assert_eq!(b.record_unresponsive_at(base), Transition::None);
}

#[test]
fn bypass_for_whole_cooldown_then_exactly_one_probe() {
    let cooldown = Duration::from_secs(3);
    let b = breaker(5, cooldown);
    let base = Instant::now();
    for _ in 0..5 {
        let _ = b.record_unresponsive_at(base);
    }

    assert_eq!(b.admit_at(base), Admission::Bypass);
    assert_eq!(
        b.admit_at(base + Duration::from_millis(2999)),
        Admission::Bypass
    );
    let probe_at = base + cooldown;
    assert_eq!(b.admit_at(probe_at), Admission::Probe);
    // Immediately after handing out the probe, the window is re-armed: the
    // very next admit at the same instant bypasses.
    assert_eq!(b.admit_at(probe_at), Admission::Bypass);
}

#[test]
fn failed_probe_reprobes_exactly_one_cooldown_later_not_sooner_not_doubled() {
    let cooldown = Duration::from_secs(3);
    let b = breaker(5, cooldown);
    let base = Instant::now();
    for _ in 0..5 {
        let _ = b.record_unresponsive_at(base);
    }

    let probe1_at = base + cooldown;
    assert_eq!(b.admit_at(probe1_at), Admission::Probe);
    // The failed probe reports unresponsive.
    assert_eq!(b.record_unresponsive_at(probe1_at), Transition::None);

    // Not sooner than another full cooldown from the probe.
    assert_eq!(
        b.admit_at(probe1_at + cooldown - Duration::from_millis(1)),
        Admission::Bypass
    );
    // Exactly one more cooldown later, fixed cadence (no doubling).
    let probe2_at = probe1_at + cooldown;
    assert_eq!(b.admit_at(probe2_at), Admission::Probe);
}

#[test]
fn stale_unresponsive_reports_while_open_are_noops_and_dont_move_open_until() {
    let cooldown = Duration::from_secs(3);
    let b = breaker(5, cooldown);
    let base = Instant::now();
    for _ in 0..5 {
        let _ = b.record_unresponsive_at(base);
    }

    // A burst of stale in-flight failure reports draining after the trip.
    for _ in 0..10 {
        assert_eq!(b.record_unresponsive_at(base), Transition::None);
    }

    // `open_until` hasn't moved: the probe still arrives at exactly `base + cooldown`.
    assert_eq!(
        b.admit_at(base + cooldown - Duration::from_millis(1)),
        Admission::Bypass
    );
    assert_eq!(b.admit_at(base + cooldown), Admission::Probe);
}

#[test]
fn successful_probe_closes_and_retripping_reopens_with_same_cooldown() {
    let cooldown = Duration::from_secs(3);
    let b = breaker(5, cooldown);
    let base = Instant::now();
    for _ in 0..5 {
        let _ = b.record_unresponsive_at(base);
    }

    let probe_at = base + cooldown;
    assert_eq!(b.admit_at(probe_at), Admission::Probe);
    assert_eq!(b.record_responsive_at(probe_at), Transition::Closed);
    assert_eq!(b.admit_at(probe_at), Admission::Allow);

    // Re-tripping opens with the same cooldown as the first trip.
    for _ in 0..4 {
        assert_eq!(b.record_unresponsive_at(probe_at), Transition::None);
    }
    assert_eq!(
        b.record_unresponsive_at(probe_at),
        Transition::Opened { cooldown }
    );
}

#[test]
fn failure_threshold_zero_always_allows_never_opens() {
    let b = breaker(0, Duration::from_secs(3));
    let base = Instant::now();
    for _ in 0..100 {
        assert_eq!(b.record_unresponsive_at(base), Transition::None);
        assert_eq!(b.admit_at(base), Admission::Allow);
        assert_eq!(b.admit_bypass_only(), Admission::Allow);
    }
}

#[test]
fn cancelled_probe_does_not_permanently_stick_the_circuit_open() {
    let cooldown = Duration::from_secs(3);
    let b = breaker(5, cooldown);
    let base = Instant::now();
    for _ in 0..5 {
        let _ = b.record_unresponsive_at(base);
    }

    let probe1_at = base + cooldown;
    // Admit the probe, then never report anything for it (a cancelled query).
    assert_eq!(b.admit_at(probe1_at), Admission::Probe);

    // Bypass throughout the extended window...
    assert_eq!(
        b.admit_at(probe1_at + cooldown - Duration::from_millis(1)),
        Admission::Bypass
    );
    // ...and a fresh probe is admitted once it elapses: not permanently stuck.
    assert_eq!(b.admit_at(probe1_at + cooldown), Admission::Probe);
}

#[test]
fn admit_bypass_only_never_probes_and_never_mutates_state() {
    let cooldown = Duration::from_secs(3);
    let b = breaker(5, cooldown);
    let base = Instant::now();
    for _ in 0..5 {
        let _ = b.record_unresponsive_at(base);
    }

    let probe_at = base + cooldown;
    // A burst of `admit_bypass_only` calls past the cooldown instant must
    // never return `Probe` and must never consume/delay the demand path's
    // single probe slot.
    for i in 0..10u64 {
        assert_eq!(
            b.admit_bypass_only(),
            Admission::Bypass,
            "iteration {i} at/after probe_at"
        );
    }

    // The very next `admit_at` call still returns `Probe`, proving
    // `admit_bypass_only` never re-armed `open_until`.
    assert_eq!(b.admit_at(probe_at), Admission::Probe);
}
