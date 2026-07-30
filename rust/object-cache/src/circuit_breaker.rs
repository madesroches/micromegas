//! A small, domain-agnostic circuit breaker: closed/open/probing state driven
//! by consecutive-failure counting and a fixed cooldown. Deliberately free of
//! any cache-specific naming so it stays reusable (and so `imetric!`'s
//! literal-name requirement doesn't leak into it) — state transitions are
//! *returned* to the caller, which owns the metrics and logs.
//!
//! See `rust/tasks/1360_cache_client_circuit_breaker_plan.md` ("The breaker")
//! for the full design rationale.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Tunables for a `CircuitBreaker`.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Consecutive unresponsive requests that trip the breaker. `0` disables it.
    pub failure_threshold: u32,
    /// How long the circuit stays open before one probe is admitted. Fixed,
    /// not backed off — see "Why the cooldown is fixed" in the plan. The
    /// client passes its `stall_timeout`, reusing it rather than adding a
    /// second knob; an occasional probe overlapping the next admitted request
    /// is harmless, since there is no doubling for it to corrupt.
    pub cooldown: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            cooldown: Duration::from_secs(3),
        }
    }
}

/// What a caller may do with the guarded resource right now.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Admission {
    /// Closed: use it normally.
    Allow,
    /// Open, cooldown elapsed: this one request probes for recovery.
    Probe,
    /// Open: skip it entirely — no connection, no timeout cost.
    Bypass,
}

/// A state change worth reporting, returned so the caller emits its own
/// metrics/logs and the breaker stays domain-agnostic.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[must_use]
pub enum Transition {
    None,
    Opened { cooldown: Duration },
    Closed,
}

#[derive(Debug)]
struct State {
    /// Consecutive unresponsive requests while closed; any response resets it.
    consecutive: u32,
    /// `Some(t)` => open: bypass until `t`, then admit one probe.
    open_until: Option<Instant>,
}

/// Gates access to a flaky resource: trips after `failure_threshold`
/// consecutive unresponsive reports, stays open for a fixed `cooldown`, then
/// admits exactly one probe request to test recovery.
///
/// Each clock-dependent method has an `_at(now: Instant)` form (the real
/// logic) plus a wrapper that passes `Instant::now()`, so the state machine
/// is unit-testable with a synthetic clock and no sleeps.
#[derive(Debug)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Mutex<State>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Mutex::new(State {
                consecutive: 0,
                open_until: None,
            }),
        }
    }

    pub fn admit(&self) -> Admission {
        self.admit_at(Instant::now())
    }

    pub fn admit_at(&self, now: Instant) -> Admission {
        if self.config.failure_threshold == 0 {
            return Admission::Allow;
        }
        let mut state = self.state.lock().expect("circuit breaker mutex poisoned");
        match state.open_until {
            None => Admission::Allow,
            Some(t) if now < t => Admission::Bypass,
            Some(_) => {
                // Re-arm before probing: a failed probe therefore needs no
                // extra bookkeeping, and a cancelled/dropped probe future
                // can't leave the circuit permanently stuck open.
                state.open_until = Some(now + self.config.cooldown);
                Admission::Probe
            }
        }
    }

    /// Same read of state as `admit`, but never returns `Probe` and never
    /// mutates `open_until` — for a caller (`prefetch`) that must not be able
    /// to consume the single per-cooldown probe slot a demand read would
    /// otherwise receive.
    pub fn admit_bypass_only(&self) -> Admission {
        if self.config.failure_threshold == 0 {
            return Admission::Allow;
        }
        let state = self.state.lock().expect("circuit breaker mutex poisoned");
        match state.open_until {
            None => Admission::Allow,
            Some(_) => Admission::Bypass,
        }
    }

    /// The resource completed an operation (any HTTP status counts — it's alive).
    pub fn record_responsive(&self) -> Transition {
        self.record_responsive_at(Instant::now())
    }

    pub fn record_responsive_at(&self, _now: Instant) -> Transition {
        let mut state = self.state.lock().expect("circuit breaker mutex poisoned");
        let was_open = state.open_until.take().is_some();
        state.consecutive = 0;
        if was_open {
            Transition::Closed
        } else {
            Transition::None
        }
    }

    /// The resource lost: an abandon-budget expiry, a stall, a connect
    /// failure, or a transport error (see "Abandon vs. unresponsive").
    pub fn record_unresponsive(&self) -> Transition {
        self.record_unresponsive_at(Instant::now())
    }

    pub fn record_unresponsive_at(&self, now: Instant) -> Transition {
        let mut state = self.state.lock().expect("circuit breaker mutex poisoned");
        if state.open_until.is_some() {
            // Already open: a failed probe needs no action (admit_at re-armed
            // the window when it handed the probe out), and a stale
            // pre-trip report is a pure no-op.
            return Transition::None;
        }
        if self.config.failure_threshold == 0 {
            // Breaker disabled; never accumulate.
            return Transition::None;
        }
        state.consecutive = state.consecutive.saturating_add(1);
        if state.consecutive >= self.config.failure_threshold {
            let cooldown = self.config.cooldown;
            state.open_until = Some(now + cooldown);
            Transition::Opened { cooldown }
        } else {
            Transition::None
        }
    }
}
