//! No-DB tests for the admission scheduler and its config parsing.

use futures::FutureExt;
use micromegas::servers::query_admission::{
    AdmissionConfig, AdmissionPermit, AdmissionScheduler, gb_to_bytes, parse_gb_env,
};
use std::env::VarError;
use std::sync::Arc;
use tokio::task::JoinHandle;

fn scheduler(budget: u64, min_cost: u64, identity_cap: u64) -> Arc<AdmissionScheduler> {
    Arc::new(AdmissionScheduler::new(AdmissionConfig {
        budget,
        min_cost,
        identity_cap,
    }))
}

fn id(name: &str) -> Arc<str> {
    Arc::from(name)
}

fn spawn_admit(
    s: &Arc<AdmissionScheduler>,
    identity: &str,
    cost: u64,
) -> JoinHandle<AdmissionPermit> {
    let s = s.clone();
    let identity = id(identity);
    tokio::spawn(async move { s.admit(identity, cost).await })
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

async fn wait_queued(s: &AdmissionScheduler, n: u64) {
    for _ in 0..1000 {
        if s.queued_count() == n {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("queued_count never reached {n}, is {}", s.queued_count());
}

fn admit_now(s: &Arc<AdmissionScheduler>, identity: &str, cost: u64) -> Option<AdmissionPermit> {
    s.admit(id(identity), cost).now_or_never()
}

#[tokio::test]
async fn fast_path_has_no_wait_under_budget() {
    let s = scheduler(100, 1, 100);
    let a = admit_now(&s, "a", 30).expect("admitted immediately");
    let b = admit_now(&s, "b", 30).expect("admitted immediately");
    assert_eq!(s.queued_count(), 0);
    drop((a, b));
}

#[tokio::test]
async fn over_budget_waiter_is_admitted_when_a_permit_drops() {
    let s = scheduler(10, 10, 10);
    let held = admit_now(&s, "a", 10).unwrap();
    let waiting = spawn_admit(&s, "b", 10);
    wait_queued(&s, 1).await;
    assert!(!waiting.is_finished());
    drop(held);
    let permit = waiting.await.unwrap();
    assert_eq!(s.queued_count(), 0);
    drop(permit);
}

#[tokio::test]
async fn newcomer_goes_ahead_of_a_long_queue_from_another_identity() {
    let s = scheduler(20, 10, 20);
    let a1 = admit_now(&s, "a", 10).unwrap();
    let a2 = admit_now(&s, "a", 10).unwrap();
    let mut a_waiters = vec![];
    for _ in 0..10 {
        a_waiters.push(spawn_admit(&s, "a", 10));
        wait_queued(&s, a_waiters.len() as u64).await;
    }
    let b = spawn_admit(&s, "b", 10);
    wait_queued(&s, 11).await;
    drop(a1);
    settle().await;
    assert!(b.is_finished(), "b has nothing in flight and must go first");
    assert!(a_waiters.iter().all(|w| !w.is_finished()));
    drop((a2, b.await.unwrap()));
}

#[tokio::test]
async fn least_in_flight_identity_is_admitted_first() {
    let s = scheduler(30, 10, 30);
    let a1 = admit_now(&s, "a", 10).unwrap();
    let a2 = admit_now(&s, "a", 10).unwrap();
    let b1 = admit_now(&s, "b", 10).unwrap();
    let wa = spawn_admit(&s, "a", 10);
    wait_queued(&s, 1).await;
    let wb = spawn_admit(&s, "b", 10);
    wait_queued(&s, 2).await;
    let wc = spawn_admit(&s, "c", 10);
    wait_queued(&s, 3).await;

    drop(a1);
    settle().await;
    assert!(wc.is_finished() && !wa.is_finished() && !wb.is_finished());
    let pc = wc.await.unwrap();

    drop(a2);
    settle().await;
    assert!(wa.is_finished() && !wb.is_finished());
    let pa = wa.await.unwrap();

    drop(b1);
    settle().await;
    assert!(wb.is_finished());
    drop((pc, pa, wb.await.unwrap()));
}

#[tokio::test]
async fn blocked_large_head_holds_back_smaller_queries_from_others() {
    let s = scheduler(20, 5, 20);
    let x_big = admit_now(&s, "x", 15).unwrap();
    let x_small = admit_now(&s, "x", 5).unwrap();
    let big = spawn_admit(&s, "b", 10);
    wait_queued(&s, 1).await;
    let small = spawn_admit(&s, "c", 5);
    wait_queued(&s, 2).await;

    // The small query would fit now, but it must not pass the blocked head.
    drop(x_small);
    settle().await;
    assert!(!big.is_finished() && !small.is_finished());

    drop(x_big);
    settle().await;
    assert!(big.is_finished() && small.is_finished());
}

#[tokio::test]
async fn fresh_identity_waits_behind_a_blocked_head_on_the_fast_path() {
    let s = scheduler(20, 5, 20);
    let held = admit_now(&s, "x", 15).unwrap();
    let big = spawn_admit(&s, "b", 10);
    wait_queued(&s, 1).await;
    // 15 + 5 fits the budget, so only the blocked head can be what keeps it out.
    assert!(admit_now(&s, "c", 5).is_none());
    let small = spawn_admit(&s, "c", 5);
    wait_queued(&s, 2).await;
    drop(held);
    settle().await;
    assert!(big.is_finished() && small.is_finished());
}

#[tokio::test]
async fn cost_above_the_budget_is_clamped_and_runs() {
    let s = scheduler(100, 1, 80);
    let big = admit_now(&s, "a", 1_000).expect("an oversized query still runs");
    assert_eq!(big.cost(), 80);
    let other = admit_now(&s, "b", 20).expect("the rest of the budget stays free");
    assert_eq!(other.cost(), 20);
}

#[tokio::test]
async fn min_cost_floor_is_applied() {
    let s = scheduler(100, 10, 100);
    assert_eq!(admit_now(&s, "a", 1).unwrap().cost(), 10);
    assert_eq!(admit_now(&s, "a", 0).unwrap().cost(), 10);
}

#[tokio::test]
async fn lone_identity_never_exceeds_its_cap_and_leaves_room_for_others() {
    let s = scheduler(100, 10, 50);
    let a1 = admit_now(&s, "a", 25).unwrap();
    let a2 = admit_now(&s, "a", 25).unwrap();
    let a3 = spawn_admit(&s, "a", 25);
    wait_queued(&s, 1).await;
    // The cap-blocked waiter is not a blocked head, so it does not hold anyone back.
    let b = admit_now(&s, "b", 50).expect("fits in budget - identity_cap");
    settle().await;
    assert!(!a3.is_finished());
    drop(a1);
    settle().await;
    assert!(a3.is_finished(), "its own completion frees its cap");
    drop((a2, b, a3.await.unwrap()));
}

#[tokio::test]
async fn cap_equal_to_budget_lets_a_lone_identity_fill_it() {
    let s = scheduler(100, 10, 100);
    let permits: Vec<_> = (0..4).map(|_| admit_now(&s, "a", 25).unwrap()).collect();
    assert_eq!(s.queued_count(), 0);
    assert!(admit_now(&s, "a", 25).is_none());
    drop(permits);
}

#[tokio::test]
async fn cancelled_waiter_is_removed_and_queues_behind_it_advance() {
    let s = scheduler(10, 5, 10);
    let held = admit_now(&s, "x", 5).unwrap();
    let blocked = spawn_admit(&s, "a", 10);
    wait_queued(&s, 1).await;
    // Held back only by the blocked head ahead of it.
    let behind = spawn_admit(&s, "b", 5);
    wait_queued(&s, 2).await;
    settle().await;
    assert!(!behind.is_finished());

    blocked.abort();
    let _ = blocked.await;
    settle().await;
    assert!(
        behind.is_finished(),
        "the stale blocked head must be cleared"
    );
    assert_eq!(s.queued_count(), 0);
    drop((held, behind.await.unwrap()));
}

#[tokio::test]
async fn cancellation_racing_a_grant_releases_the_granted_cost() {
    let s = scheduler(10, 10, 10);
    let held = admit_now(&s, "a", 10).unwrap();
    let mut waiting = Box::pin(s.admit(id("b"), 10));
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    assert_eq!(s.queued_count(), 1);

    // The grant fires here, but the future is dropped without taking the permit.
    drop(held);
    drop(waiting);
    assert_eq!(s.queued_count(), 0);
    admit_now(&s, "c", 10).expect("the budget did not leak");
}

#[test]
fn config_validation() {
    let ok = |budget, min_cost, max| AdmissionConfig::resolve(budget, min_cost, max);
    assert!(ok(0, 0, None).is_err());
    assert!(ok(100, 1, Some(101)).is_err());
    assert!(ok(100, 51, Some(50)).is_err());
    assert!(ok(100, 50, Some(50)).is_ok());
    assert_eq!(ok(100, 1, None).unwrap().identity_cap, 80);
    assert_eq!(ok(100, 1, Some(100)).unwrap().identity_cap, 100);
    assert!(ok(100, 90, None).is_err());
}

#[test]
fn budget_parsing() {
    let parse = |v: Result<String, VarError>| parse_gb_env("TEST_VAR", v);
    assert_eq!(parse(Err(VarError::NotPresent)).unwrap(), None);
    assert_eq!(parse(Ok("0.5".into())).unwrap(), Some(512 * 1024 * 1024));
    assert_eq!(parse(Ok("2".into())).unwrap(), Some(2 << 30));
    for bad in ["4GB", "", "-1", "NaN", "inf", "-inf", "1e30"] {
        assert!(parse(Ok(bad.into())).is_err(), "{bad:?} should be rejected");
    }
    assert!(gb_to_bytes(f64::MAX).is_err());
}
