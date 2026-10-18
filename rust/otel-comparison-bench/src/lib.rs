//! Shared harness for the `micromegas` and `opentelemetry` bench binaries.
//!
//! Both binaries use the same group and case ids so `report.py` can pair the libraries by the
//! ids criterion records in `benchmark.json`.

use criterion::{BenchmarkId, Criterion, Throughput};
use std::{
    sync::{
        Barrier,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// One recorded operation; the argument is a persistent counter so high-cardinality ids stay new.
pub type Case = fn(usize);

/// Called once per case after its last sample with the case id (`group/case/lib`, or
/// `contention/case/lib/threads`) and the number of operations it emitted. Panics if the library
/// dropped any of them, except for OpenTelemetry metrics, which report instead.
pub type AfterCase = fn(id: &str, emitted: u64);

pub const THREAD_COUNTS: [u64; 3] = [1, 4, 8];

/// Blocks until the library's background queue is empty.
pub type Drain = fn();

/// Operations emitted between two drains, across all threads. The OpenTelemetry batch queue holds
/// 2048 records by default and drops on full, so a chunk this size can never overflow it.
pub const BATCH: usize = 2048;

/// Times only the emits: after each chunk of at most `BATCH` operations, `drain` runs outside
/// the timed region so the background thread always starts a chunk with an empty queue.
pub fn bench_cases(
    c: &mut Criterion,
    group_name: &str,
    lib: &str,
    cases: &[(&str, Case)],
    drain: Drain,
    after_case: Option<AfterCase>,
) {
    let mut group = c.benchmark_group(group_name);
    for (name, case) in cases {
        // Persists across warm-up and sample rounds so ids keep being new to the library.
        let mut i = 0usize;
        group.bench_function(BenchmarkId::new(*name, lib), |b| {
            b.iter_custom(|iters| {
                let mut elapsed = Duration::ZERO;
                let mut remaining = iters as usize;
                while remaining > 0 {
                    let n = remaining.min(BATCH);
                    let start = Instant::now();
                    for _ in 0..n {
                        case(i);
                        i = i.wrapping_add(1);
                    }
                    elapsed += start.elapsed();
                    drain();
                    remaining -= n;
                }
                elapsed
            })
        });
        if let Some(after_case) = after_case {
            after_case(&format!("{group_name}/{name}/{lib}"), i as u64);
        }
    }
    group.finish();
}

/// Runs each case on 1, 4 and 8 threads that start together and use disjoint counter ranges.
/// The reported throughput is the aggregate of all threads. Threads emit `BATCH / threads`
/// operations per chunk and meet at a barrier; the main thread drains between chunks. A chunk
/// costs the slowest thread's emit time, measured inside the thread so barrier wake-up latency
/// stays out of it.
pub fn bench_contention(
    c: &mut Criterion,
    lib: &str,
    cases: &[(&str, Case)],
    per_thread_init: fn(),
    per_thread_exit: fn(),
    drain: Drain,
    after_case: Option<AfterCase>,
) {
    let mut group = c.benchmark_group("contention");
    for (name, case) in cases {
        let case = *case;
        for threads in THREAD_COUNTS {
            group.throughput(Throughput::Elements(threads));
            // Advanced after every round so ids stay unique across threads and rounds; it ends
            // as the total emitted by all threads.
            let mut base = 0u64;
            group.bench_with_input(
                BenchmarkId::new(format!("{name}/{lib}"), threads),
                &threads,
                |b, &threads| {
                    b.iter_custom(|iters| {
                        let per_chunk = (BATCH as u64 / threads).max(1);
                        let nb_chunks = iters.div_ceil(per_chunk);
                        let start_barrier = Barrier::new(threads as usize + 1);
                        let end_barrier = Barrier::new(threads as usize + 1);
                        let slowest = AtomicU64::new(0);
                        let mut elapsed = Duration::ZERO;
                        std::thread::scope(|s| {
                            for t in 0..threads {
                                let (start_barrier, end_barrier, slowest) =
                                    (&start_barrier, &end_barrier, &slowest);
                                let first = base + t * iters;
                                s.spawn(move || {
                                    per_thread_init();
                                    let mut next = first;
                                    let end = first + iters;
                                    for _ in 0..nb_chunks {
                                        let n = per_chunk.min(end - next);
                                        start_barrier.wait();
                                        let start = Instant::now();
                                        for i in next..next + n {
                                            case(i as usize);
                                        }
                                        let took = start.elapsed().as_nanos() as u64;
                                        slowest.fetch_max(took, Ordering::Relaxed);
                                        next += n;
                                        end_barrier.wait();
                                    }
                                    per_thread_exit();
                                });
                            }
                            for _ in 0..nb_chunks {
                                start_barrier.wait();
                                end_barrier.wait();
                                elapsed += Duration::from_nanos(slowest.swap(0, Ordering::Relaxed));
                                drain();
                            }
                        });
                        drain();
                        base += threads * iters;
                        elapsed
                    })
                },
            );
            if let Some(after_case) = after_case {
                after_case(&format!("contention/{name}/{lib}/{threads}"), base);
            }
        }
    }
    group.finish();
}

/// One line per encoded sample, parsed by `report.py` (criterion has no custom-metric output).
pub fn print_bytes(id: &str, nb_events: usize, raw_bytes: usize, wire_bytes: usize) {
    println!("bytes: {id} events={nb_events} raw={raw_bytes} wire={wire_bytes}");
}

/// One line per OpenTelemetry metric case, parsed by `report.py`: samples recorded and the
/// individual data points that reached the exporter.
pub fn print_metric_drops(id: &str, kept: u64, emitted: u64) {
    println!("metric-drops: {id} kept={kept} samples={emitted}");
}
