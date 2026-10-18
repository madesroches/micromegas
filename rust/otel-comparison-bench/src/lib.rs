//! Shared harness for the `micromegas` and `opentelemetry` bench binaries.
//!
//! Both binaries use the same group and case ids so `report.py` can pair the libraries by the
//! ids criterion records in `benchmark.json`.

use criterion::{BenchmarkId, Criterion, Throughput};
use std::{sync::Barrier, time::Instant};

/// One recorded operation; the argument is a persistent counter so high-cardinality ids stay new.
pub type Case = fn(usize);

/// Called once per case after its last sample with the case id (`group/case/lib`, or
/// `contention/case/lib/threads`) and the number of operations it emitted.
pub type AfterCase = fn(id: &str, emitted: u64);

pub const THREAD_COUNTS: [u64; 3] = [1, 4, 8];

pub fn bench_cases(
    c: &mut Criterion,
    group_name: &str,
    lib: &str,
    cases: &[(&str, Case)],
    after_case: Option<AfterCase>,
) {
    let mut group = c.benchmark_group(group_name);
    for (name, case) in cases {
        // Persists across warm-up and sample rounds so ids keep being new to the library.
        let mut i = 0usize;
        group.bench_function(BenchmarkId::new(*name, lib), |b| {
            b.iter(|| {
                case(i);
                i = i.wrapping_add(1);
            })
        });
        if let Some(after_case) = after_case {
            after_case(&format!("{group_name}/{name}/{lib}"), i as u64);
        }
    }
    group.finish();
}

/// Runs each case on 1, 4 and 8 threads that start together and use disjoint counter ranges.
/// The reported throughput is the aggregate of all threads.
pub fn bench_contention(
    c: &mut Criterion,
    lib: &str,
    cases: &[(&str, Case)],
    per_thread_init: fn(),
    per_thread_exit: fn(),
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
                        let barrier = Barrier::new(threads as usize + 1);
                        let elapsed = std::thread::scope(|s| {
                            for t in 0..threads {
                                let barrier = &barrier;
                                let start = base + t * iters;
                                s.spawn(move || {
                                    per_thread_init();
                                    barrier.wait();
                                    for i in start..start + iters {
                                        case(i as usize);
                                    }
                                    per_thread_exit();
                                });
                            }
                            barrier.wait();
                            Instant::now()
                        })
                        .elapsed();
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

/// One line per case, parsed by `report.py`.
pub fn print_drops(id: &str, dropped: u64, emitted: u64) {
    let pct = if emitted == 0 {
        0.0
    } else {
        100.0 * dropped as f64 / emitted as f64
    };
    println!("{id}: dropped {dropped} of {emitted} ({pct:.2}%)");
}
