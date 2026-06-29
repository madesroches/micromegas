# Blender Action Capture — Close the Operator-History Overflow Gap Plan

## Overview
The Blender add-on captures semantic user actions by draining
`bpy.context.window_manager.operators` (a small, shared ring buffer) on a fixed
1 s timer. During rapid editing the ring rotates fully between polls, so
operators that appear *and* scroll out within one interval are lost; the drain
only emits a `WARN` gap marker. This plan makes the drain **event-driven** —
piggy-backing on the persistent modal recorder so the ring is drained on every
discrete input event, right when operators are registered — keeps the timer as a
backstop, and replaces the boolean gap signal with queryable metrics so loss is
measurable. Net effect: under normal interactive use the ring no longer
overflows between drains, and any residual loss is quantified instead of merely
flagged.

## Current State

### The drain
`blender/micromegas_blender/actions.py` owns the operator-history drain:

- `_POLL_INTERVAL_S = 1.0` (`actions.py:31`) drives `_poll_timer` →
  `on_poll()` → `_poll_operators()` + `_poll_transitions()`.
- `_poll_operators()` (`actions.py:124`) snapshots `wm.operators` bl_idnames,
  diffs against `_prev_op_idnames` via `_appended_start()`, logs the appended
  suffix to `blender.action` (TRACE), and on a detected gap logs a single
  `WARN` (`actions.py:132-137`).
- `_appended_start()` (`actions.py:63-97`) aligns `current` against a retained
  suffix of `prev`. Full turnover (no suffix aligns) → `(0, True)`; ambiguous
  boundary patterns → also flagged. The function is correct and well-tested
  (`tests/test_actions.py`); the gap is a *cadence* problem, not a diff bug.

The drain is stateful through the module global `_prev_op_idnames`, and
`_poll_operators()` is idempotent: calling it more often is safe — each call
emits only what is new since the last call, from whichever caller.

### The recorder (the unused high-frequency hook)
`blender/micromegas_blender/recorder.py` runs a persistent modal operator
(`MICROMEGAS_OT_recorder`) whose `modal()` (`recorder.py:81-121`) receives
**every** input event with `PASS_THROUGH`, skips motion/timer noise
(`_SKIP_TYPES`, `recorder.py:43-53`), and logs discrete key/mouse presses to
`blender.input`. This callback already fires at human-input frequency — far
tighter than 1 s — and is the natural place to drain operators close to when
they are registered. It is currently decoupled from `actions.py`.

### Wiring
`__init__.py register()` (`actions.py`/`recorder.py` are sibling modules) calls
`set_context(lib, handle)` then `register()` on each sub-module
(`__init__.py:274-284`). There is no cross-module coupling between `recorder`
and `actions` today.

### Metrics plumbing
`binding.py` already exposes `metric_i` / `metric_f` (`binding.py:136-152`);
`handlers.py` uses local `_metric_i` / `_metric_f` helpers (`handlers.py:41-43`).
`actions.py` currently has only `_log` — no metric helper.

### Docs
- `mkdocs/docs/blender/index.md:113-122` describes the ~1 s poller and the
  `possible gap` marker.
- `blender/README.md:175-178` describes the operator-history poller.

## Design

Three complementary changes, in priority order.

### 1. Event-driven draining (primary fix)
Drain the ring from the recorder's modal loop, on every discrete event, in
addition to the timer. Because operators are registered synchronously in
response to user input, draining on each input event captures them within one
event of registration instead of waiting up to 1 s.

Latency detail: the recorder modal is a `PASS_THROUGH` handler, so it observes
an event *before* that event's keymap handler invokes the operator. The operator
this event triggers is therefore captured on the **next** event's drain. That is
still per-keystroke cadence (a few events/second), so the ring no longer fills
between drains under normal use. The final operator before an idle period is
caught by the timer backstop (change #2).

Keep modules decoupled via callback injection (open/closed; no `recorder →
actions` import coupling, and recorder stays unit-testable without `actions`):

- `actions.py`: add a public `drain_operators()` that simply calls
  `_poll_operators()` (the transitions poll stays timer-only — mode/workspace/
  tool changes do not need per-event cadence).
- `recorder.py`: add a module-global `_on_event` callback plus a
  `set_event_callback(cb)` setter. In `modal()`, after the existing
  `_registered`/generation guards and once `_lib`/`_handle` are present, invoke
  `_on_event()` for any event **not** in `_SKIP_TYPES` (i.e. discrete key/mouse/
  scroll — the same events already considered interesting), guarded by
  try/except so a drain failure never breaks event pass-through. Draining on
  both PRESS and RELEASE is fine and cheap (a `list()` + comprehension + compare
  at human frequency).
- `__init__.py register()`: after wiring contexts, call
  `recorder.set_event_callback(actions.drain_operators)` before
  `recorder.register()`. `unregister()` clears it (`set_event_callback(None)`).

Both the timer and the modal call the same `_poll_operators()` on the main
thread; calls are serialized and idempotent, so there is no double-logging and
no locking needed.

Known residual gap (documented, not fixed here): while a full-screen sub-modal
operator (knife, grab, …) runs, Blender suspends the recorder modal, so no
event-driven drains occur during it. But such a sub-modal registers only **one**
operator (itself) on completion, captured on the next resumed event — so it does
not overflow the ring. Overflow now requires many operators to register between
two consecutive recorder events (e.g. a script/macro burst), which the timer
backstop and quantification (below) cover.

### 2. Timer as backstop
Keep the `_poll_timer` registration and `on_poll()` unchanged so the timer still
drains operators (and is the sole driver of transitions) during periods when the
modal is suspended or receiving only motion events. `_POLL_INTERVAL_S` stays a
tunable; recommend leaving it at **1.0 s** since event-driven draining is now the
primary path and a shorter interval only adds idle polling overhead. (Document
that lowering it is a safe mitigation lever.)

### 3. Quantify loss instead of a boolean gap
Exact dropped count is **not recoverable** from Blender's API: `wm.operators`
exposes no monotonic id or sequence number, so there is no way to count entries
that appeared and vanished between two snapshots. Provide honest, queryable
signals instead of a bare log line:

- Track `_ring_capacity` = max `len(idnames)` ever observed across polls (the
  ring's effective capacity, discovered at runtime).
- Add a local `_metric_i` helper to `actions.py` (mirroring `handlers.py`).
- On a detected gap, emit `_metric_i("blender.action_gap", "count", 1)` so gap
  frequency becomes a time series, and enrich the existing WARN message with the
  observed capacity:
  `f"possible gap: operator history overflowed between polls (ring_capacity={cap})"`.
- Emit `_metric_i("blender.action_captured", "count", n)` each poll where `n` is
  the number of actions emitted this poll, so capture rate is queryable and the
  effectiveness of the event-driven fix is measurable from telemetry.

Metric names are a fixed, bounded set (cardinality discipline preserved).

### 4. Ring-length configurability (investigation outcome)
The issue asks whether the ring length is configurable. Finding from the code
and Blender's Python API surface: `wm.operators` is a read-only collection with
no exposed capacity setting, and there is no alternative public API that yields
operator history without the same small bound. **Verify during implementation**
(quick check in a running Blender — see Testing) and, assuming confirmed,
document it as not feasible rather than pursuing it. This is why the fix targets
drain *cadence* rather than ring size.

### Flow (after)
```
input event ─► recorder.modal() ─(non-motion)─► actions.drain_operators()
                                                      │  (also: 1 s timer ─► on_poll())
                                                      ▼
                                          _poll_operators()  [idempotent, main thread]
                                          ├─ emit appended actions  → blender.action (TRACE)
                                          ├─ blender.action_captured = n (metric)
                                          └─ on gap: WARN(+ring_capacity) + blender.action_gap=1
```

## Implementation Steps

1. **`actions.py` — public drain + metrics + capacity.**
   - Add `drain_operators()` → `_poll_operators()`.
   - Add module global `_ring_capacity` (init 0) and reset it in
     `unregister()` alongside the other state resets. Add `_ring_capacity` to
     the `global` declarations in both `_poll_operators()` (`actions.py:125`)
     and `unregister()` (`actions.py:227`) so the assignments mutate the module
     global rather than creating throwaway locals.
   - Add `_metric_i(name, unit, value)` helper (guarded by `_lib`/`_handle`).
   - In `_poll_operators()`: update `_ring_capacity = max(_ring_capacity,
     len(idnames))`; include `ring_capacity` in the gap WARN; emit
     `blender.action_gap` on gap and `blender.action_captured` with the count of
     emitted actions.
2. **`recorder.py` — event callback.**
   - Add `_on_event` global + `set_event_callback(cb)`.
   - In `modal()`, for events not in `_SKIP_TYPES`, call `_on_event()` inside a
     try/except before/after the existing input log (order irrelevant).
   - Clear `_on_event` in `unregister()` (or leave to setter; reset to None).
3. **`__init__.py` — wire it.**
   - In `register()`: `recorder.set_event_callback(actions.drain_operators)`
     (after `actions.set_context`, before `recorder.register()`).
   - In `unregister()`: `recorder.set_event_callback(None)`.
4. **Tests** (see Testing Strategy).
5. **Docs** — update `mkdocs/docs/blender/index.md` and `blender/README.md`, and
   the `actions.py`/`recorder.py` module docstrings, to describe event-driven
   draining + the new metrics.

## Files to Modify
- `blender/micromegas_blender/actions.py`
- `blender/micromegas_blender/recorder.py`
- `blender/micromegas_blender/__init__.py`
- `blender/micromegas_blender/tests/test_actions.py`
- `blender/micromegas_blender/tests/test_recorder.py` (new)
- `mkdocs/docs/blender/index.md`
- `blender/README.md`

## Trade-offs
- **Event-driven via recorder vs. a dedicated operator-post handler.** Blender
  exposes no `operator_post` handler, so the modal recorder is the only
  high-frequency, no-extra-machinery hook available. Reusing it (rather than
  adding a second modal) keeps one event source and avoids duplicate event
  overhead.
- **Callback injection vs. direct import.** Injection keeps `recorder`
  independent of `actions` (no import cycle risk, recorder testable in
  isolation) at the cost of one wiring line in `__init__.py` — consistent with
  the existing `set_context` wiring pattern.
- **Keep the timer vs. event-only.** The timer is cheap and covers the
  modal-suspended / motion-only windows the event hook cannot. Removing it would
  reopen a gap during long sub-modals.
- **Quantify vs. exact count.** Blender gives no operator sequence id, so exact
  loss is unknowable; gap-rate + capture-rate metrics are the honest, queryable
  substitute. Stated explicitly rather than fabricating a count.
- **Leave `_POLL_INTERVAL_S` at 1 s.** Shortening is pure mitigation and adds
  idle overhead; the event hook addresses the root cadence problem instead.

## Documentation
- `mkdocs/docs/blender/index.md` (`:113-122`): rewrite the poller paragraph to
  describe event-driven draining with the timer as backstop; mention
  `blender.action_gap` / `blender.action_captured` metrics; keep the
  sub-modal-suspension note (now the main residual case).
- `blender/README.md` (`:175-178`): same update, briefer.
- `actions.py` / `recorder.py` module docstrings: reflect the dual drive
  (event + timer) and the cross-wiring.

## Testing Strategy
- **Unit (`test_actions.py`, extend):**
  - `drain_operators()` emits the same actions as `_poll_operators()` (delegation).
  - Overflow now emits the `blender.action_gap` metric and the WARN message
    includes `ring_capacity`; capacity reflects the max length seen.
  - `blender.action_captured` metric equals the number of actions emitted.
  - Existing `_appended_start` / drain tests remain green (behavior unchanged).
- **Unit (`test_recorder.py`, new):** using the fake `bpy` + a `SimpleNamespace`
  event, assert the injected event callback fires for discrete events
  (e.g. `type="A", value="PRESS"`) and does **not** fire for `_SKIP_TYPES`
  events (`MOUSEMOVE`, `TIMER`); assert a raising callback does not break
  pass-through.
- **Integration sanity:** a test that drives `drain_operators()` repeatedly
  (simulating per-event draining) over an op sequence that would overflow a 1 s
  poll, and confirms no gap is reported and all ops are captured.
- **Manual in Blender:** reproduce the issue's burst (editmode toggle → knife →
  extrude → select, rapidly), confirm `blender.action` captures the sequence
  with no/markedly fewer `blender.action_gap` events vs. before. While there,
  verify there is no Python API to resize `wm.operators` (closes investigation
  item #4).
- Run `poetry run pytest` (or the add-on's test runner) and `poetry run black`
  on changed Python files before commit.

## Open Questions
1. **Drain on every non-motion event, or only on PRESS/CLICK?** Plan drains on
   all non-`_SKIP_TYPES` events (includes RELEASE) for maximum coverage; the
   cost is negligible. Confirm there's no objection to the slightly higher drain
   frequency.
2. **Keep `blender.action_captured` per-poll metric?** It makes the fix's effect
   measurable but adds a steady low-rate metric stream. Drop it if the gap
   counter alone is considered sufficient.
3. **Ring capacity verification** (item #4) requires a running Blender to
   confirm no resize API exists — fine to do during implementation, but flagging
   it as the one fact not verifiable from the repo alone.
