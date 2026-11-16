use anyhow::{Result, anyhow};
use micromegas::servers::maintenance::run_update_groups;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

fn nz(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).expect("non-zero")
}

#[derive(Default)]
struct Tracker {
    in_flight: AtomicUsize,
    max: AtomicUsize,
    log: Mutex<Vec<String>>,
}

impl Tracker {
    fn enter(&self, name: String) {
        let n = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(n, Ordering::SeqCst);
        self.log.lock().expect("log").push(name);
    }
    fn exit(&self, name: String) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.log.lock().expect("log").push(name);
    }
}

async fn yield_a_few() {
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn concurrency_one_is_serial_and_ordered() -> Result<()> {
    let tracker = Arc::new(Tracker::default());
    let items: Vec<i32> = (0..5).collect();
    let failures = run_update_groups(
        &items,
        |_| 0,
        nz(1),
        || async { Ok(()) },
        |item, _| {
            let tracker = tracker.clone();
            let item = *item;
            async move {
                tracker.enter(format!("start {item}"));
                yield_a_few().await;
                tracker.exit(format!("end {item}"));
                Ok(())
            }
        },
    )
    .await?;
    assert!(failures.is_empty());
    assert_eq!(tracker.max.load(Ordering::SeqCst), 1);
    let starts: Vec<String> = tracker
        .log
        .lock()
        .expect("log")
        .iter()
        .filter(|l| l.starts_with("start"))
        .cloned()
        .collect();
    let expected: Vec<String> = (0..5).map(|i| format!("start {i}")).collect();
    assert_eq!(starts, expected);
    Ok(())
}

#[tokio::test]
async fn concurrency_is_reached_and_bounded() -> Result<()> {
    let tracker = Arc::new(Tracker::default());
    let started = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Notify::new());
    let items: Vec<i32> = (0..6).collect();
    let fut = run_update_groups(
        &items,
        |_| 0,
        nz(3),
        || async { Ok(()) },
        |item, _| {
            let tracker = tracker.clone();
            let started = started.clone();
            let gate = gate.clone();
            let item = *item;
            async move {
                tracker.enter(format!("start {item}"));
                if started.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
                    gate.notify_waiters();
                } else if started.load(Ordering::SeqCst) < 3 {
                    // The third starter opens the gate; later items pass straight through.
                    gate.notified().await;
                }
                yield_a_few().await;
                tracker.exit(format!("end {item}"));
                Ok(())
            }
        },
    );
    let failures = tokio::time::timeout(Duration::from_secs(10), fut).await??;
    assert!(failures.is_empty());
    assert_eq!(tracker.max.load(Ordering::SeqCst), 3);
    Ok(())
}

#[tokio::test]
async fn group_boundary_is_a_barrier() -> Result<()> {
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let fetches = Arc::new(AtomicUsize::new(0));
    let items: Vec<(i32, i32)> = vec![(0, 0), (0, 1), (0, 2), (1, 3), (1, 4), (2, 5)];
    let failures = run_update_groups(
        &items,
        |(g, _)| *g,
        nz(4),
        || {
            let log = log.clone();
            let fetches = fetches.clone();
            async move {
                let ctx = fetches.fetch_add(1, Ordering::SeqCst);
                log.lock().expect("log").push(format!("fetch {ctx}"));
                Ok(ctx)
            }
        },
        |(g, i), ctx: Arc<usize>| {
            let log = log.clone();
            let (g, i) = (*g, *i);
            async move {
                assert_eq!(*ctx, g as usize);
                log.lock().expect("log").push(format!("start {g} {i}"));
                yield_a_few().await;
                log.lock().expect("log").push(format!("end {g} {i}"));
                Ok(())
            }
        },
    )
    .await?;
    assert!(failures.is_empty());
    assert_eq!(fetches.load(Ordering::SeqCst), 3);
    let log = log.lock().expect("log").clone();
    let pos = |s: &str| log.iter().position(|l| l == s).expect("event present");
    for g in 0..2 {
        let last_end = log
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with(&format!("end {g} ")))
            .map(|(p, _)| p)
            .max()
            .expect("ends");
        assert!(pos(&format!("fetch {}", g + 1)) > last_end);
    }
    assert!(pos("fetch 0") < pos("start 0 0"));
    Ok(())
}

#[tokio::test]
async fn failures_do_not_stop_other_items() -> Result<()> {
    let ran = Arc::new(AtomicUsize::new(0));
    let items: Vec<(i32, i32)> = vec![(0, 0), (0, 1), (0, 2), (1, 3), (1, 4)];
    let failures = run_update_groups(
        &items,
        |(g, _)| *g,
        nz(2),
        || async { Ok(()) },
        |(_, i), _| {
            let ran = ran.clone();
            let i = *i;
            async move {
                ran.fetch_add(1, Ordering::SeqCst);
                if i % 2 == 1 {
                    Err(anyhow!("item {i} failed"))
                } else {
                    Ok(())
                }
            }
        },
    )
    .await?;
    assert_eq!(ran.load(Ordering::SeqCst), 5);
    let idx: Vec<usize> = failures.iter().map(|(i, _)| *i).collect();
    assert_eq!(idx, vec![1, 3]);
    Ok(())
}

#[tokio::test]
async fn failures_are_reported_in_input_order() -> Result<()> {
    let items: Vec<i32> = (0..3).collect();
    let failures = run_update_groups(
        &items,
        |_| 0,
        nz(3),
        || async { Ok(()) },
        |item, _| {
            let item = *item;
            async move {
                // The first item finishes last.
                for _ in 0..(10 - item * 4) {
                    tokio::task::yield_now().await;
                }
                Err(anyhow!("failed {item}"))
            }
        },
    )
    .await?;
    let idx: Vec<usize> = failures.iter().map(|(i, _)| *i).collect();
    assert_eq!(idx, vec![0, 1, 2]);
    Ok(())
}

#[tokio::test]
async fn context_fetch_error_aborts() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let ran = Arc::new(Mutex::new(Vec::<i32>::new()));
    let items: Vec<(i32, i32)> = vec![(0, 0), (0, 1), (1, 2), (1, 3)];
    let res = run_update_groups(
        &items,
        |(g, _)| *g,
        nz(2),
        || {
            let fetches = fetches.clone();
            async move {
                if fetches.fetch_add(1, Ordering::SeqCst) == 1 {
                    Err(anyhow!("fetch failed"))
                } else {
                    Ok(())
                }
            }
        },
        |(_, i), _| {
            let ran = ran.clone();
            let i = *i;
            async move {
                ran.lock().expect("ran").push(i);
                Ok(())
            }
        },
    )
    .await;
    assert!(res.is_err());
    let mut ran = ran.lock().expect("ran").clone();
    ran.sort();
    assert_eq!(ran, vec![0, 1]);
}

#[tokio::test]
async fn empty_input_does_nothing() -> Result<()> {
    let fetches = Arc::new(AtomicUsize::new(0));
    let items: Vec<i32> = vec![];
    let failures = run_update_groups(
        &items,
        |_| 0,
        nz(4),
        || {
            let fetches = fetches.clone();
            async move {
                fetches.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        },
        |_, _| async { Ok(()) },
    )
    .await?;
    assert!(failures.is_empty());
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
    Ok(())
}
