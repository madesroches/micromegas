# Test-Quality Fixes for Timing/Sleep-Based Tests Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1252

## Overview

A tech-debt survey of the timing/sleep-based tests found that most are *not*
actually flaky — their correctness comes from `flush_thread_buffer()` /
`drop(runtime)` / `tokio::sync::Notify` / condition-polling, not from wall-clock
timing. This task makes three concrete, scoped test-quality improvements:

1. Remove a genuine race-to-fail in `cron_loop_drains` by awaiting a
   task-started signal instead of guessing with a fixed 50 ms sleep.
2. Replace the pointless random sleeps in `async_span_tests.rs` with a small
   fixed sleep, which also drops the `rand` test dependency from `analytics`.
3. Correct the docstrings of `thread_park_test` (and the `TracingRuntimeExt`
   trait doc) so they describe what the code actually verifies — flushing on
   thread **stop**, not on thread **park**.

This is deliberately narrow: it is a test-quality pass, **not** a broad
"fix flaky tests" sweep and **not** a behavior change to production code.

## Current State

### 1. `cron_loop_drains` — genuine race
`rust/public/tests/graceful_shutdown_tests.rs:142-194`. The test spawns
`run_tasks_forever` with a single `SlowTask` (sleeps 300 ms, then sets an
`AtomicBool`), sleeps a fixed **50 ms** "to give the task time to start"
(`:186`), triggers shutdown via `Notify`, then asserts `finished == true`.

If 50 ms is not enough for `run_tasks_forever` to schedule and enter the
`SlowTask` before the shutdown `Notify` is observed, the drain path finds an
empty (or not-yet-started) task set and the callback never runs, so `finished`
stays `false` and the assertion fails. Low-probability, but a real flaky
**failure** mode.

The correct pattern already exists in the same file: `axum_drain_completes`
(`:22-64`) uses a `handler_started: Arc<Notify>` that the handler fires on
entry, and the test `await`s it before triggering shutdown (`:56-57`).

Relevant production code (no changes needed, but informs the fix):
- `run_tasks_forever` (`rust/public/src/servers/maintenance.rs:203-263`):
  with `max_parallelism == 1`, after `task_set.spawn(task.spawn().await)` the
  set length reaches the cap and the loop enters a `select!` awaiting either
  `task_set.join_next()` or `shutdown`; on shutdown it calls `drain_task_set`,
  which awaits every in-flight task to completion before returning. So once the
  callback has *started*, draining is guaranteed to await it — awaiting a
  started-signal fully closes the race.
- `CronTask` / `TaskCallback` (`rust/public/src/servers/cron_task.rs:9-14`):
  `run(&self, task_scheduled_time)` is the hook where the stub signals "started".

### 2. Random sleeps in `async_span_tests.rs`
`rust/analytics/tests/async_span_tests.rs`:
- `manual_inner` (`:11-15`), `macro_inner` (`:24-29`) sleep
  `rand::rng().random_range(0..=1000)` ms.
- `named_inner_work` (`:148-152`) sleeps `rand::rng().random_range(0..=500)` ms.

Every assertion in this file is an **exact event count** (`== 6`, `== 16`)
evaluated *after* `flush_thread_buffer()` + `drop(runtime)`. The sleep is only
an `.await` yield point; its duration has zero effect on recorded event counts.
The random sleeps add up to ~1 s of pure wait per test and pull in a `rand`
dependency and nondeterministic runtime for no coverage benefit.

`rand` is declared only as an `analytics` **dev-dependency**
(`rust/analytics/Cargo.toml:50`, under `[dev-dependencies]` at `:47`) and its
**only** real use in the crate is these three call sites (verified: no
`use rand` / `rand::` in `analytics/src` or any other `analytics/tests` file —
other grep hits are the substring inside `cpu_brand`, `strand`, "random" in
comments). So removing the sleeps lets us drop the dependency.

### 3. `thread_park_test` verifies the wrong thing — and there is no park callback
`rust/tracing/tests/thread_park_test.rs`. The test name (`test_thread_park_flush`)
and docs (`:8-10`, `:15`, `:21-23`) claim it validates the `on_thread_park`
flush callback. Its only assertion is `total_events >= 8` (`:75-79`), evaluated
*after* `drop(runtime)`.

**Key finding:** there is no `on_thread_park` callback registered anywhere.
`TracingRuntimeExt` (`rust/tracing/src/runtime.rs:76-100`) wires only
`on_thread_start` (init stream) and `on_thread_stop` (flush + unregister). A
repo-wide search for `.on_thread_park(` returns nothing; the only mentions of
`on_thread_park` in the codebase are inaccurate doc comments — the test's, and
the trait doc at `runtime.rs:38` ("`on_thread_park`: Flushes event buffer when
thread becomes idle").

Consequences:
- The `>= 8` assertion is satisfied purely by the flush-on-thread-**stop** that
  `drop(runtime)` triggers. It would pass even if a park callback existed and
  were broken — the test constrains nothing about park behavior.
- **Asserting a park-specific invariant is not possible**: park-flushing is not
  implemented, so events are not flushed until thread stop. An assertion like
  "events observed before `drop(runtime)`" would *fail*, not strengthen the test.

Therefore the issue's Option A ("assert a park-specific invariant") is
infeasible without adding a production feature (out of scope — see Trade-offs),
and Option B (correct the docs) is the right resolution.

### 4. Wall-clock upper bounds (low priority, no code change)
- `graceful_shutdown_tests.rs:103` — `elapsed < Duration::from_secs(2)` with
  grace = 200 ms (~10× margin).
- `http_event_sink_transport_tests.rs:366` — `elapsed < Duration::from_secs(3)`
  with `request_timeout` = 200 ms (~5× margin).

The issue marks these low priority and conditional ("if either ever flakes in
CI"). There is no acceptance criterion for them.

## Design

### Fix 1 — `cron_loop_drains` awaits a started-signal
Mirror `axum_drain_completes`:
- Add a `started: Arc<Notify>` field to the test-local `SlowTask`.
- In `SlowTask::run`, call `self.started.notify_one()` **before** the 300 ms
  sleep (i.e., first line of the callback).
- In the test body, clone the `Notify`, and after spawning the runner replace
  the fixed `sleep(50 ms)` with `started.notified().await` before
  `notify.notify_one()`.

`tokio::sync::Notify` stores a single permit, so even if the callback fires
`notify_one()` before the test reaches `notified().await`, the await returns
immediately — no lost wakeup. Keep the 300 ms work sleep so the callback is
still genuinely in-flight when shutdown fires, exercising the drain path.

### Fix 2 — fixed sleeps + drop `rand`
- Replace each random-sleep body with a small fixed `sleep`:
  ```rust
  async fn manual_inner() {
      sleep(Duration::from_millis(1)).await;
  }
  ```
  Same for `macro_inner`. Keep a 1 ms `sleep` (not full removal) so the `.await`
  suspension point that the test exercises is preserved, and `Duration` /
  `tokio::time::sleep` imports stay meaningful.
- `named_inner_work(operation: &'static str)` — replace the random sleep with a
  fixed 1 ms sleep and keep an `eprintln!("doing {operation}");` so the
  `operation` parameter stays used (avoids an unused-variable warning) without
  reprinting a now-meaningless duration.
- Remove `use rand::Rng;` (`:6`).
- Remove `rand.workspace = true` from `rust/analytics/Cargo.toml`
  `[dev-dependencies]` (`:50`).
- Leave `instrumented_sync_function`'s fixed `std::thread::sleep(100 ms)` (`:40`)
  as-is — it is neither random nor flagged.

### Fix 3 — correct the docs (thread_park + trait)
`rust/tracing/tests/thread_park_test.rs`:
- Rename `test_thread_park_flush` → `test_worker_thread_span_flush` (accurate:
  it verifies worker-thread spans are recorded and flushed on runtime shutdown).
- Rewrite the function docstring (`:21-23`) and the `park_inducing_function`
  docstring/comment (`:8-10`, `:15`) to describe reality: instrumented async
  work runs across `.await` points on tokio worker threads, and the resulting
  span events are flushed when the worker threads stop on `drop(runtime)`. Drop
  the `on_thread_park` claims.
- Optionally rename the helper `park_inducing_function` →
  `instrumented_async_work` for accuracy (the sleep is a yield point, not the
  thing under test). Keep the `>= 8` assertion (it remains a valid lower bound:
  4 tasks × 2 span events).
- The file may keep its name (`thread_park_test.rs`) to avoid churn, or be
  renamed to `thread_span_flush_test.rs`. Prefer keeping the name; renaming is
  optional and noted for the reviewer.

`rust/tracing/src/runtime.rs`:
- Fix the `TracingRuntimeExt` doc comment (`:38`, and the trait-level list at
  `:13-16`, `:36-40`) to stop claiming an `on_thread_park` callback. Describe
  the two callbacks that actually exist: `on_thread_start` (init stream) and
  `on_thread_stop` (flush + unregister). This removes the misleading source of
  the test's original premise.

### Fix 4 — no change
Leave both upper bounds as-is. Document the fallback (widen the bound, or gate
behind a serialized / `#[ignore]` perf test) so a future CI flake has a clear,
pre-agreed remedy without re-litigating.

## Implementation Steps

1. **`cron_loop_drains`** (`rust/public/tests/graceful_shutdown_tests.rs`):
   add `started: Arc<Notify>` to `SlowTask`, notify at the top of `run`, clone
   into the task, and replace the `sleep(50 ms)` with `started.notified().await`.
2. **`async_span_tests.rs`** (`rust/analytics/tests/async_span_tests.rs`):
   fixed 1 ms sleeps in `manual_inner`, `macro_inner`, `named_inner_work`;
   keep `eprintln!("doing {operation}")`; remove `use rand::Rng;`.
3. **`analytics/Cargo.toml`**: remove `rand.workspace = true` from
   `[dev-dependencies]`.
4. **`thread_park_test.rs`**: rename the test fn, rewrite docstrings/comments to
   describe thread-stop flushing; keep the `>= 8` assertion.
5. **`runtime.rs`**: correct the `TracingRuntimeExt` doc comments to drop the
   `on_thread_park` claim and describe the real start/stop callbacks.
6. Run `cargo fmt`, `cargo clippy --workspace -- -D warnings`, and the affected
   tests (see Testing Strategy).

## Files to Modify
- `rust/public/tests/graceful_shutdown_tests.rs` — Fix 1
- `rust/analytics/tests/async_span_tests.rs` — Fix 2
- `rust/analytics/Cargo.toml` — Fix 2 (drop `rand` dev-dep)
- `rust/tracing/tests/thread_park_test.rs` — Fix 3
- `rust/tracing/src/runtime.rs` — Fix 3 (trait doc)

## Trade-offs
- **Fixed 1 ms sleep vs. removing the sleep entirely.** A 1 ms sleep keeps a
  real `.await` suspension point, which is what the async-instrumentation tests
  are meant to exercise; full removal would make the inner fns non-suspending.
  Event counts are identical either way, so 1 ms is the lower-risk choice.
- **Correcting docs vs. implementing `on_thread_park`.** Implementing a real
  park-flush callback would let the test assert a park-specific invariant, but
  that is a production behavior change (flushing on *every* park could add lock
  contention on the hot idle path) and is out of scope for a test-quality issue
  whose stated non-goals exclude broad changes. If park-flushing is desired, it
  should be its own issue with its own perf justification; this plan only aligns
  the docs with current behavior.
- **Keeping the 300 ms work sleep in `cron_loop_drains`.** With the started
  signal the drain is deterministic regardless of duration, but keeping a
  non-trivial work sleep ensures the callback is genuinely mid-flight at
  shutdown, so the test still exercises draining rather than a
  race-free-but-already-finished task.

## Documentation
No user-facing docs (mkdocs) are affected. The only documentation touched is
in-source: the `TracingRuntimeExt` doc comments in `runtime.rs` and the test
docstrings in `thread_park_test.rs`, both corrected as part of Fix 3.

## Testing Strategy
- `cargo test -p micromegas --test graceful_shutdown_tests` — `cron_loop_drains`
  and neighbors still pass; confirm no fixed pre-shutdown sleep remains.
- `cargo test -p micromegas-analytics --test async_span_tests` — the three
  count assertions (`== 6`, `== 6`, `== 16`) still pass; note the measurably
  lower wall-clock (was up to ~1 s/test of random sleep, now ~1 ms).
- `cargo test -p micromegas-tracing --test thread_park_test` — renamed test
  still passes its `>= 8` assertion.
- `cargo build -p micromegas-analytics --tests` — compiles with `rand` removed
  (proves no other analytics test depended on it).
- `cargo fmt` and `cargo clippy --workspace -- -D warnings` clean (in
  particular, no unused-import/variable warnings from the edits).

## Open Questions
None that block implementation. Two low-stakes cosmetic choices are left to the
implementer/reviewer and default as noted above: (a) whether to rename the
`thread_park_test.rs` **file** (default: keep the name); (b) whether to rename
the `park_inducing_function` helper (default: rename to `instrumented_async_work`
for accuracy).
