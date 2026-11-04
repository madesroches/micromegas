//! Fair per-identity admission scheduler for FlightSQL queries.
//!
//! The scheduler never rejects: every identity gets its own FIFO queue, a per-process budget caps
//! the cost in flight, and each time capacity frees up the next query comes from the identity
//! with the least cost in flight. It is generic over cost and identity and knows nothing about
//! DataFusion.

use anyhow::{Result, bail};
use micromegas_tracing::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

const MIB: u64 = 1 << 20;
const GIB: f64 = (1u64 << 30) as f64;

pub const ADMISSION_BUDGET_ENV: &str = "MICROMEGAS_ADMISSION_BUDGET_GB";
pub const ADMISSION_MIN_COST_ENV: &str = "MICROMEGAS_ADMISSION_MIN_COST_MB";
pub const ADMISSION_MAX_PER_IDENTITY_ENV: &str = "MICROMEGAS_ADMISSION_MAX_PER_IDENTITY_GB";
pub const ADMISSION_OPAQUE_LEAF_COST_ENV: &str = "MICROMEGAS_ADMISSION_OPAQUE_LEAF_COST_MB";

const DEFAULT_MIN_COST_MB: u64 = 8;
const DEFAULT_OPAQUE_LEAF_COST_MB: u64 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionConfig {
    /// Per-process cost units allowed in flight.
    pub budget: u64,
    /// Floor per query, so 0-byte queries still occupy a slot.
    pub min_cost: u64,
    /// The most one identity may hold in flight (<= budget).
    pub identity_cap: u64,
}

impl AdmissionConfig {
    /// Fails on combinations that would deadlock or panic: `clamp(estimate, min_cost,
    /// identity_cap)` panics when `min_cost > identity_cap`.
    pub fn validate(&self) -> Result<()> {
        if self.budget == 0 {
            bail!("admission budget must be greater than zero");
        }
        if self.identity_cap > self.budget {
            bail!(
                "admission per-identity maximum ({}) exceeds the budget ({})",
                self.identity_cap,
                self.budget
            );
        }
        if self.min_cost > self.identity_cap {
            bail!(
                "admission minimum cost ({}) exceeds the per-identity maximum ({})",
                self.min_cost,
                self.identity_cap
            );
        }
        Ok(())
    }

    /// Builds the validated config from parsed values. An unset maximum defaults to 80% of the
    /// budget.
    pub fn resolve(budget: u64, min_cost: u64, max_per_identity: Option<u64>) -> Result<Self> {
        let config = Self {
            budget,
            min_cost,
            identity_cap: max_per_identity.unwrap_or((budget as u128 * 4 / 5) as u64),
        };
        config.validate()?;
        Ok(config)
    }
}

/// Converts a decimal GB value to bytes, rejecting anything that is not a finite non-negative
/// number that fits in a `u64`.
pub fn gb_to_bytes(gb: f64) -> Result<u64> {
    if !gb.is_finite() || gb < 0.0 {
        bail!("expected a finite non-negative number of GB, got {gb}");
    }
    let bytes = (gb * GIB).round();
    if bytes > u64::MAX as f64 {
        bail!("{gb} GB does not fit in 64 bits of bytes");
    }
    Ok(bytes as u64)
}

/// Parses a decimal-GB env knob: unset is `None`, anything unparseable is an error so a typo
/// such as `4GB` cannot silently leave the knob off. Takes the `std::env::var` result so it can
/// be tested without touching the process environment.
pub fn parse_gb_env(
    name: &str,
    var: std::result::Result<String, std::env::VarError>,
) -> Result<Option<u64>> {
    match var {
        Ok(raw) => {
            let gb: f64 = raw
                .trim()
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid {name} value '{raw}': {e}"))?;
            gb_to_bytes(gb)
                .map(Some)
                .map_err(|e| anyhow::anyhow!("invalid {name} value '{raw}': {e}"))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("invalid {name}: {e}")),
    }
}

fn mb_env_to_bytes(name: &str, default_mb: u64) -> u64 {
    crate::settings::resolve_u64_env(name, default_mb).saturating_mul(MIB)
}

/// Flat cost charged for a plan leaf whose work cannot be seen at plan time.
pub fn opaque_leaf_cost_from_env() -> u64 {
    mb_env_to_bytes(ADMISSION_OPAQUE_LEAF_COST_ENV, DEFAULT_OPAQUE_LEAF_COST_MB)
}

/// Builds the scheduler from the environment. `None` (budget unset) means admission is
/// disabled. The min-cost and per-identity knobs are not read then, so they cannot fail startup.
pub fn admission_scheduler_from_env() -> Result<Option<Arc<AdmissionScheduler>>> {
    let Some(budget) = parse_gb_env(ADMISSION_BUDGET_ENV, std::env::var(ADMISSION_BUDGET_ENV))?
    else {
        return Ok(None);
    };
    let min_cost = mb_env_to_bytes(ADMISSION_MIN_COST_ENV, DEFAULT_MIN_COST_MB);
    let max_per_identity = parse_gb_env(
        ADMISSION_MAX_PER_IDENTITY_ENV,
        std::env::var(ADMISSION_MAX_PER_IDENTITY_ENV),
    )?;
    let config = AdmissionConfig::resolve(budget, min_cost, max_per_identity)?;
    info!(
        "query admission enabled: budget={} identity_cap={} min_cost={}",
        config.budget, config.identity_cap, config.min_cost
    );
    Ok(Some(Arc::new(AdmissionScheduler::new(config))))
}

struct Waiter {
    cost: u64,
    arrival: u64,
    grant: oneshot::Sender<()>,
}

#[derive(Default)]
struct IdentityState {
    in_flight: u64,
    queue: VecDeque<Waiter>,
}

impl IdentityState {
    fn is_idle(&self) -> bool {
        self.in_flight == 0 && self.queue.is_empty()
    }
}

#[derive(Default)]
struct State {
    in_flight_total: u64,
    queued_total: u64,
    /// Set by dispatch when the chosen head is waiting on the budget. Stops a small query from
    /// an idle identity from passing it on the fast path.
    head_blocked: bool,
    next_arrival: u64,
    per_identity: HashMap<Arc<str>, IdentityState>,
}

/// Gauges sampled under the lock and published after releasing it.
struct Sample {
    in_flight: u64,
    queued: u64,
}

impl State {
    fn sample(&self) -> Sample {
        Sample {
            in_flight: self.in_flight_total,
            queued: self.queued_total,
        }
    }

    fn remove_if_idle(&mut self, identity: &Arc<str>) {
        if self
            .per_identity
            .get(identity)
            .is_some_and(IdentityState::is_idle)
        {
            self.per_identity.remove(identity);
        }
    }

    fn release(&mut self, identity: &Arc<str>, cost: u64) {
        if let Some(state) = self.per_identity.get_mut(identity) {
            state.in_flight -= cost;
        }
        self.in_flight_total -= cost;
        self.remove_if_idle(identity);
    }

    /// Grants queued waiters in least-in-flight order until the next one does not fit.
    fn dispatch(&mut self, config: &AdmissionConfig) {
        loop {
            let pick = self
                .per_identity
                .iter()
                .filter_map(|(id, st)| {
                    let head = st.queue.front()?;
                    (st.in_flight + head.cost <= config.identity_cap)
                        .then(|| (st.in_flight, head.arrival, id.clone(), head.cost))
                })
                .min_by_key(|(in_flight, arrival, _, _)| (*in_flight, *arrival));
            let Some((_, _, identity, cost)) = pick else {
                self.head_blocked = false;
                return;
            };
            if self.in_flight_total + cost > config.budget {
                // Nobody with more in flight may pass this head, so a large query cannot be
                // starved by a steady stream of small ones; its identity's own in-flight only
                // drains, so it eventually fits.
                self.head_blocked = true;
                return;
            }
            let state = self
                .per_identity
                .get_mut(&identity)
                .expect("picked identity exists");
            let waiter = state.queue.pop_front().expect("picked head exists");
            self.queued_total -= 1;
            state.in_flight += cost;
            self.in_flight_total += cost;
            if waiter.grant.send(()).is_err() {
                // The receiver is gone; its drop guard cannot see this waiter anymore.
                self.release(&identity, cost);
            }
        }
    }
}

pub struct AdmissionScheduler {
    state: Mutex<State>,
    config: AdmissionConfig,
}

impl AdmissionScheduler {
    pub fn new(config: AdmissionConfig) -> Self {
        Self {
            state: Mutex::new(State::default()),
            config,
        }
    }

    pub fn config(&self) -> &AdmissionConfig {
        &self.config
    }

    /// Waiters across all identities; backs the `admission_queued_queries` gauge.
    pub fn queued_count(&self) -> u64 {
        self.lock().queued_total
    }

    /// The cost `admit` would charge for an estimate.
    pub fn charged_cost(&self, estimate: u64) -> u64 {
        // Clamping to the cap lets a query larger than the whole budget run instead of waiting
        // forever, which is what never rejecting requires.
        estimate.clamp(self.config.min_cost, self.config.identity_cap)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish(sample: Sample) {
        imetric!("admission_in_flight_cost", "bytes", sample.in_flight);
        imetric!("admission_queued_queries", "count", sample.queued);
    }

    /// Waits for this identity's turn and returns a permit holding `cost` until dropped.
    pub async fn admit(self: &Arc<Self>, identity: Arc<str>, cost: u64) -> AdmissionPermit {
        let charged = self.charged_cost(cost);
        let (arrival, rx, sample) = {
            let mut state = self.lock();
            let id_state = state.per_identity.entry(identity.clone()).or_default();
            let identity_free = id_state.queue.is_empty()
                && id_state.in_flight + charged <= self.config.identity_cap;
            let fast = identity_free
                && state.in_flight_total + charged <= self.config.budget
                && !state.head_blocked;
            if fast {
                state
                    .per_identity
                    .get_mut(&identity)
                    .expect("entry created above")
                    .in_flight += charged;
                state.in_flight_total += charged;
                let sample = state.sample();
                drop(state);
                Self::publish(sample);
                return AdmissionPermit::new(self.clone(), identity, charged);
            }
            let arrival = state.next_arrival;
            state.next_arrival += 1;
            let (tx, rx) = oneshot::channel();
            state
                .per_identity
                .get_mut(&identity)
                .expect("entry created above")
                .queue
                .push_back(Waiter {
                    cost: charged,
                    arrival,
                    grant: tx,
                });
            state.queued_total += 1;
            state.dispatch(&self.config);
            (arrival, rx, state.sample())
        };
        imetric!("admission_queued", "count", 1_u64);
        Self::publish(sample);
        let mut guard = WaitGuard {
            scheduler: self.clone(),
            identity: identity.clone(),
            arrival,
            charged,
            rx: Some(rx),
        };
        let granted = guard.rx.as_mut().expect("receiver present").await;
        guard.rx = None;
        debug_assert!(
            granted.is_ok(),
            "a waiter's sender is only dropped after a grant"
        );
        AdmissionPermit::new(self.clone(), identity, charged)
    }

    fn release(&self, identity: &Arc<str>, cost: u64) {
        let sample = {
            let mut state = self.lock();
            state.release(identity, cost);
            state.dispatch(&self.config);
            state.sample()
        };
        Self::publish(sample);
    }
}

/// Removes a cancelled waiter, or gives back the cost of a grant nobody took.
struct WaitGuard {
    scheduler: Arc<AdmissionScheduler>,
    identity: Arc<str>,
    arrival: u64,
    charged: u64,
    rx: Option<oneshot::Receiver<()>>,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if self.rx.is_none() {
            return;
        }
        let sample = {
            let mut state = self.scheduler.lock();
            let removed = state
                .per_identity
                .get_mut(&self.identity)
                .and_then(|st| {
                    let pos = st.queue.iter().position(|w| w.arrival == self.arrival)?;
                    st.queue.remove(pos)
                })
                .is_some();
            if removed {
                state.queued_total -= 1;
                state.remove_if_idle(&self.identity);
            } else {
                // Not queued anymore, so a grant raced the cancellation.
                state.release(&self.identity, self.charged);
            }
            state.dispatch(&self.scheduler.config);
            state.sample()
        };
        AdmissionScheduler::publish(sample);
    }
}

/// Holds an identity's share of the budget until dropped.
pub struct AdmissionPermit {
    scheduler: Arc<AdmissionScheduler>,
    identity: Arc<str>,
    cost: u64,
}

impl AdmissionPermit {
    fn new(scheduler: Arc<AdmissionScheduler>, identity: Arc<str>, cost: u64) -> Self {
        Self {
            scheduler,
            identity,
            cost,
        }
    }

    /// The cost this permit holds against the budget.
    pub fn cost(&self) -> u64 {
        self.cost
    }
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        self.scheduler.release(&self.identity, self.cost);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler(budget: u64, min_cost: u64, cap: u64) -> Arc<AdmissionScheduler> {
        Arc::new(AdmissionScheduler::new(AdmissionConfig {
            budget,
            min_cost,
            identity_cap: cap,
        }))
    }

    #[tokio::test]
    async fn idle_identities_are_removed() {
        let s = scheduler(100, 1, 100);
        let a: Arc<str> = Arc::from("a");
        let p1 = s.admit(a.clone(), 10).await;
        let p2 = s.admit(Arc::from("b"), 10).await;
        assert_eq!(s.lock().per_identity.len(), 2);
        assert_eq!(s.lock().per_identity[&a].in_flight, 10);
        drop(p1);
        assert_eq!(s.lock().per_identity.len(), 1);
        drop(p2);
        assert!(s.lock().per_identity.is_empty());
        assert_eq!(s.lock().in_flight_total, 0);
    }

    #[tokio::test]
    async fn cancelled_waiter_leaves_no_state() {
        let s = scheduler(10, 10, 10);
        let held = s.admit(Arc::from("a"), 10).await;
        let waiting = tokio::spawn({
            let s = s.clone();
            async move { s.admit(Arc::from("b"), 10).await }
        });
        while s.queued_count() == 0 {
            tokio::task::yield_now().await;
        }
        waiting.abort();
        let _ = waiting.await;
        assert_eq!(s.queued_count(), 0);
        assert_eq!(s.lock().per_identity.len(), 1);
        drop(held);
        assert!(s.lock().per_identity.is_empty());
    }
}
