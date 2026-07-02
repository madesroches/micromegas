# Blender Action Capture — Identity-Based Operator Draining Plan

## Overview
The Blender add-on drains `bpy.context.window_manager.operators` (a 32-slot ring)
and decides which entries are *new* by **positionally aligning `bl_idname`
strings** between polls (`_appended_start`). That heuristic is fundamentally
ambiguous whenever the recent operator history contains a repeating pattern — and
by design it responds to the ambiguity by (a) logging a `possible gap` WARN and
(b) **re-emitting** the ambiguous tail on *every* poll. In production this turns
into a storm: a stable-but-periodic operator history, polled ~9×/second by the
backstop timer while a full-screen modal suspends the event-driven path, produced
**485 false gap warnings and ~2100 duplicate action logs in 12 minutes** — with
**zero operators actually lost**. This plan replaces string-position diffing with
**per-entry object identity** (`op.as_pointer()`), which makes "what is new"
exact, kills the false-gap/duplicate storm, and reduces gap detection to the one
condition that genuinely means loss. This is the "solid solution" — the previous
fix (#1182, event-driven draining) treated cadence but left the unsound diff in
place.

## Root Cause (evidence)
Telemetry for prod process `a76fa16f-7e23-4d1f-a513-dafdb710efff` (Blender 4.5),
around `2026-06-30 14:26 UTC`:

- **485** `possible gap: operator history overflowed between polls
  (ring_capacity=32)` WARNs over ~12 min, firing in tight ~108 ms clusters (=
  `_POLL_INTERVAL_S`).
- **No `blender.input` events** during the storm → the recorder modal was
  suspended (a full-screen modal, `SCT_OT_vc_paint`, was running), so the
  event-driven drain path was dormant and only the 0.1 s backstop timer fired.
- Three consecutive polls (108 ms apart) emitted the **identical** four
  operators with **byte-identical parameters** (same `TRANSFORM_OT_translate`
  `Vector((-48.328…))`, doubled `OBJECT_OT_editmode_toggle`, `VIEW3D_OT_select`).
  The buffer was **completely stable** — nothing was being lost.
- Aggregate duplicates over the window: `OBJECT_OT_editmode_toggle` ×962,
  `VIEW3D_OT_select` ×711, `TRANSFORM_OT_translate` ×459.

### Why the heuristic storms
`_appended_start(current, prev)` (`actions.py:78-112`) collects every drop count
`d` for which a suffix of `prev` prefixes `current`, then — on disagreement —
flags a gap and picks the alignment reporting the **most** new entries. When the
history tail is **periodic** with period `p` (e.g. repeated `select → move →
toggle` cycles give a tail like `…T,E,E,S,T,E,E,S`), the buffer aligns with
itself at shifts `0, p, 2p, …`. Multiple `d` values are valid and disagree, so
the function returns `gap=True` and `start` pointing one period back. Result:
**every poll re-logs the last period and logs a WARN**, indefinitely, even though
the buffer never changed. The existing tests `test_repeated_identical_operators_
not_dropped` and `test_repeated_boundary_pattern_not_dropped` (`test_actions.py:
59-78`) codify exactly this behavior as intended — it is the defect.

## Current State
`blender/micromegas_blender/actions.py`:
- `_prev_op_idnames: list[str] | None` (`:46`) — previous poll's `bl_idname`
  snapshot.
- `_poll_operators()` (`:139-164`) — snapshots `[op.bl_idname for op in ops]`,
  calls `_appended_start()` to get `(start, gap)`, logs the gap WARN +
  `blender.action_gap` metric on `gap`, emits `ops[start:]` at TRACE, emits
  `blender.action_captured` when `n > 0`, stores `_prev_op_idnames = idnames`.
- `_appended_start()` (`:78-112`) — the unsound positional string diff (to be
  removed).
- `_format_op()` (`:115-136`), `drain_operators()` (`:167-169`), the timer
  (`:233-249`), and transition polling (`:177-225`) are **unaffected**.

Draining is dual-driven and idempotent: the recorder modal calls
`drain_operators()` per discrete event (`recorder.py:125-129`; wired in
`__init__.py:283`) and the 0.1 s timer calls `on_poll()`. Both run on the main
thread and funnel into `_poll_operators()`. **This wiring stays exactly as is** —
only the internals of "which entries are new / is this a gap" change.

`_ring_capacity = 32` (`actions.py:49`) is a documented hard cap. From the prior
plan's Blender-source investigation (`tasks/completed/1181_..._plan.md:277-288`):
`wm.operators` is a `ListBase` of separately heap-allocated `wmOperator` nodes,
capped by `#define MAX_OP_REGISTERED 32` and managed FIFO via
`BLI_addtail`/`BLI_remlink`/`WM_operator_free` in `wm_operator_register()`
(`source/blender/windowmanager/intern/wm.cc`). **Each history entry is a distinct
heap node whose address is stable for the entry's lifetime** — this is what makes
identity-based tracking sound.

## Design

### Core idea: track entries by pointer identity, not string position
Every `bpy_struct` exposes `as_pointer()` → the address of the underlying C data
as a Python `int`. For a `wm.operators` entry that address is the `wmOperator*`
node, which is allocated once and freed only when FIFO-dropped. It is therefore a
**stable, unique identifier** for that history entry across polls. Replace the
positional diff with a pointer set-difference.

**Locked design decision — the pointer *set*.** The load-bearing invariant is a
single rule: *never emit an entry whose pointer was present on the previous
poll.* Everything we need follows from it by construction — a stable/periodic
buffer emits nothing (the storm is structurally impossible, not tuned away), and
a gap is *detected* (full ring **and** disjoint pointer sets), never guessed. Do
**not** substitute a "remember only the newest pointer" high-water anchor: it
saves a few bytes of state but assumes strict append-only ordering and is
fragile to pointer reuse at the split point (a reused address matched mid-buffer
splits the tail wrong), trading a one-sentence-provable invariant for a fragile
one. The set is the simplest formulation whose correctness can be proven, and it
is final for this plan.

```
def _poll_operators():
    ops = list(wm.operators)                 # oldest -> newest (unchanged)
    cur_ptrs = [op.as_pointer() for op in ops]
    prev = _prev_op_ptrs                      # set[int] | None

    # New entries = those whose pointer was not present last poll, in buffer order.
    if prev is None:                          # first poll after (re)start / clear
        new_ops = []                          # nothing was "missed"; establish baseline
    else:
        prev_set = prev
        new_ops = [op for op, p in zip(ops, cur_ptrs) if p not in prev_set]

    # Genuine loss (gap) — the ONLY real overflow condition:
    #   ring is full AND none of last poll's entries survive → entries were
    #   FIFO-dropped before we ever saw them. Partial overlap proves we saw
    #   everything appended since the newest retained entry, so it is NOT a gap.
    gap = (
        prev is not None
        and len(ops) >= _ring_capacity
        and prev                              # non-empty
        and prev.isdisjoint(cur_ptrs)
    )

    if gap:
        _log(WARN, "blender.action", f"...(ring_capacity={_ring_capacity})")
        _metric_i("blender.action_gap", "count", 1)

    n = 0
    for op in new_ops:
        try:                                  # per-op guard retained (as today):
            _log(TRACE, "blender.action", _format_op(op)); n += 1
        except Exception:
            pass                              # _format_op can raise on a stored entry
    if n > 0:
        _metric_i("blender.action_captured", "count", n)

    _prev_op_ptrs = set(cur_ptrs)             # must run unconditionally
```

Why this is correct for every observed mutation mode of `wm.operators`:
- **First poll** (`prev is None`, after register/re-register): baseline only,
  **no emissions**. This is an intentional, observable behavior change: today
  `_poll_operators` calls `_appended_start(idnames, _prev_op_idnames or [])`,
  which returns `(0, False)` for an empty prev, so the first poll emits every
  entry already in the ring — re-emitting pre-capture history, and pure
  duplicates on a mid-session re-register. Establish-baseline-and-emit-nothing
  is the correct "what is new" semantics; the affected test
  (`test_poll_emits_new_actions_with_params`) is realigned in step 3.
- **Stable buffer** (the storm case): all pointers already in `prev_set` →
  `new_ops == []`, `gap` false (partial/total overlap) → **zero emissions, zero
  WARNs**. Storm eliminated.
- **Periodic tail**: each `select`/`move`/`toggle` entry is a distinct node with
  a distinct pointer → no ambiguity, no re-emission.
- **Repeated identical user actions** (e.g. 40 rapid `VIEW3D_OT_select` clicks):
  40 distinct allocations → 40 distinct pointers → all 40 captured (the string
  heuristic could not distinguish these).
- **Normal append + FIFO drop**: retained entries keep their pointers (skipped),
  appended entries have new pointers (emitted). Overlap is non-empty → no false
  gap.
- **History cleared** (file load / undo-to-empty / mode changes that clear):
  `ops == []` → `new_ops == []`, `gap` false (empty ring is not full). Matches
  the existing "clear is not a gap" contract.
- **True overflow** (>32 register-ops between two polls, e.g. a `bpy.ops` script
  burst with the modal suspended): full ring, zero pointer overlap → `gap=True`,
  emitted once. Exact and rare.

### State change
- Replace module global `_prev_op_idnames: list[str] | None` with
  `_prev_op_ptrs: set[int] | None` (`actions.py:44-46`). Reset to `None` in
  `unregister()` (`:253-260`) — unchanged reset semantics.
- `_appended_start()` and its `global` usages are **deleted**.

### What does not change
- `_format_op()`, the WARN text, both metric names/semantics
  (`blender.action_gap`, `blender.action_captured` gated on `n>0`),
  `_ring_capacity`, the timer, transition polling, `drain_operators()`, and all
  `__init__.py`/`recorder.py` wiring. Aside from the intentional first-poll
  change documented above (the first poll now establishes the baseline and
  emits nothing, where today it emits every ring entry), the observable
  surface is identical; only the new-entry/gap computation is replaced.

### Flow (after)
```
input event ─► recorder.modal() ─(non-motion)─► actions.drain_operators()
                                                     │  (also: 0.1 s timer ─► on_poll())
                                                     ▼
                                         _poll_operators()  [idempotent, main thread]
                                         ├─ cur_ptrs = [op.as_pointer() …]
                                         ├─ new = entries whose ptr ∉ prev_ptrs (in order)
                                         ├─ gap = full ring AND prev_ptrs ∩ cur == ∅
                                         ├─ emit new → blender.action (TRACE); captured=n if n>0
                                         └─ on gap: WARN(+ring_capacity) + action_gap=1
```

## Implementation Steps
1. **`actions.py` — swap identity in.**
   - Rename global `_prev_op_idnames` → `_prev_op_ptrs` (type `set[int] | None`,
     init `None`); update the docstring comment at `:44-46`.
   - Rewrite `_poll_operators()` (`:139-164`) per the Design snippet: build
     `cur_ptrs` via `op.as_pointer()` inside the existing try/except that already
     guards `list(wm.operators)`; compute `new_ops` by pointer membership; compute
     `gap` as *full-ring-and-disjoint*; emit and store `set(cur_ptrs)`. Retain the
     existing per-op try/except around emission (`:156-161`) — `_format_op` can
     raise on a stored entry (`op.bl_idname` at `:123` is unguarded) — and the
     `_prev_op_ptrs = set(cur_ptrs)` update must run unconditionally after the
     loop, so a failed emission never re-emits already-seen ops on the next poll.
   - Delete `_appended_start()` (`:78-112`) and remove it from the `global`
     declaration; keep `_ring_capacity` in `unregister()`'s reset list, and swap
     `_prev_op_idnames` → `_prev_op_ptrs` there (`:253, :260`).
2. **`tests/conftest.py` — pointer-capable fake operators.** The fake ring holds
   plain objects; give the test `FakeOp` a stable `as_pointer()` (e.g. return
   `id(self)`) so identity semantics are exercised. (`FakeOp` lives in
   `test_actions.py`; add the method there — see step 3.)
3. **`tests/test_actions.py` — retarget tests to identity.**
   - Add `def as_pointer(self): return id(self)` to `FakeOp` (`:8-17`).
   - Update the autouse `_wire` fixture (`:20-29`): change its per-test reset
     line `actions._prev_op_idnames = None` (`:23`) to
     `actions._prev_op_ptrs = None` — otherwise it resets a dead attribute and
     `_prev_op_ptrs` leaks state across tests (order-dependent failures).
   - **Delete** the `_appended_start` unit tests (`:43-97`), including the two
     that encode the storm behavior (`test_repeated_identical_operators_not_
     dropped`, `test_repeated_boundary_pattern_not_dropped`).
   - **Rewrite** `test_poll_emits_new_actions_with_params` (`:103-119`): the
     first poll now emits nothing (baseline — see Design); reuse the same
     `select_all` instance on the second poll so only `cube_add` is new →
     expect exactly **one** emission (`cube_add`, with name and params), not two.
   - **Rewrite** `test_poll_overflow_logs_gap_marker` (`:141-151`): its 3-entry
     full turnover is a **non-gap** under the new design (ring not full).
     Retarget it to the full-ring setup — it becomes the "True gap" regression
     test listed below; its old small-buffer shape is covered by the new
     non-full-turnover test.
   - **Rewrite** `test_drain_operators_delegates_to_poll` (`:157-160`): it calls
     `drain_operators()` exactly once with prev state reset to `None` by the
     autouse `_wire` fixture, so under first-poll-baseline semantics that single
     call emits nothing and the assertion fails. First perform a baseline poll
     on an empty ring (`_set_ops(fake_bpy, [])`; `actions._poll_operators()`),
     then set the `OBJECT_OT_delete` op and call `drain_operators()`; keep the
     existing assertion.
   - Update `_set_ops`-driven integration tests to reuse the **same** `FakeOp`
     instances across polls where an entry is meant to *persist* (persistence =
     same pointer). Add regression tests:
     - **Stable buffer storm regression:** poll the identical list (same
       instances) 10× → **zero** `blender.action` emissions and **zero**
       `blender.action_gap` across all 10 polls (the first poll only
       establishes the baseline). This is the direct guard against the
       production bug.
     - **Periodic tail:** a ring like `[s1,m1,t1,s2,m2,t2]` (distinct instances,
       repeating idnames) re-polled unchanged → no new emissions, no gap.
     - **Repeated identical clicks:** append N distinct `VIEW3D_OT_select`
       instances across polls → all N captured.
     - **Append + FIFO drop:** drop oldest instance, append a new one → only the
       new one emitted, no gap.
     - **True gap:** full ring (`_ring_capacity` distinct instances) fully
       replaced by all-new instances → `gap=True`, `blender.action_gap` emitted,
       new entries emitted once.
     - **Non-full turnover is not a gap:** small buffer fully replaced (len <
       capacity) → new entries emitted, **no** gap (it was a clear, not overflow).
   - Keep/adjust: `test_poll_no_change_emits_nothing`,
     `test_poll_params_unavailable_keeps_bl_idname`,
     the three metric tests
     (`test_gap_emits_action_gap_metric`, `test_action_captured_metric_on_new_
     ops`, `test_action_captured_metric_not_emitted_when_no_new_ops`), and
     `test_gap_warn_includes_ring_capacity`. Both gap tests
     (`test_gap_emits_action_gap_metric` and
     `test_gap_warn_includes_ring_capacity`) must switch to the true-gap setup
     (full ring of `_ring_capacity` distinct instances fully replaced) — their
     current 2-entry turnover is no longer a gap.
4. **Docs** — `mkdocs/docs/blender/index.md:113-128` and `blender/README.md:
   175-181`: replace "emits only the operators *appended since* the last drain
   … if the ring turned over entirely … a gap marker" with the identity model:
   *each drain emits entries not seen on the previous drain (tracked by stable
   per-entry identity); a gap is logged only when a full ring turns over entirely
   between drains — the sole condition indicating true FIFO loss.* Update the
   `actions.py` module docstring (`:1-24`) accordingly (it currently says the
   drain "diffs against `_prev_op_idnames`").

## Files to Modify
- `blender/micromegas_blender/actions.py`
- `blender/micromegas_blender/tests/test_actions.py`
- `mkdocs/docs/blender/index.md`
- `blender/README.md`

(No change to `recorder.py` or `__init__.py` — wiring and public API are stable.)

## Trade-offs
- **Pointer identity vs. positional string diff.** Identity is exact and
  collision-free per live entry; the string diff is inherently ambiguous under
  repeats (the root defect). Identity also correctly captures repeated-identical
  operators the string diff could not distinguish.
- **Pointer reuse (ABA).** After a node is freed, a *new* `wmOperator` could be
  allocated at the same address and, if it lands in `prev_set`, be skipped once.
  This is (a) rare within a 108 ms interval, (b) self-correcting (only that one
  poll is affected), and (c) far preferable to the current guaranteed storm.
  Optional hardening if ever observed: key on `(as_pointer(), bl_idname)`; not
  planned now (adds cost for a negligible risk).
- **Why not a lossless operator hook?** Blender exposes **no** operator-executed
  handler or Python-readable report/Info-log list; `wm.operators` is the only
  programmatic source, and it is a lossy 32-slot ring. A truly lossless capture
  would require a C-level hook (not available to a Python add-on). Polling this
  ring with correct identity is the solid ceiling within the Python API — the
  remaining loss (only on genuine >32-op bursts between polls) is now detected
  precisely instead of fabricated.
- **Gap precision.** Gating the gap on *full ring AND disjoint pointer sets* is
  strictly more accurate than the old "any full turnover": it no longer fires on
  small-buffer clears or on periodic patterns, only on real FIFO overflow.
- **Keep the dual drive + timer.** Unchanged; the timer still covers
  modal-suspended windows. With identity dedup it now costs nothing during a
  stable buffer (no emissions, no WARNs) instead of storming.

## Documentation
- `mkdocs/docs/blender/index.md` (`:113-128`) — rewrite the drain description to
  the identity model; the metrics table is unchanged.
- `blender/README.md` (`:175-181`) — same, briefer.
- `actions.py` module docstring (`:1-24`) — describe identity-based draining;
  drop the `_prev_op_idnames`/append-diff wording.

## Testing Strategy
- **Unit** (`test_actions.py`, rewritten per step 3): the storm-regression test
  (repoll a stable buffer many times → zero re-emission, zero gaps) is the
  primary guard; plus periodic-tail, repeated-clicks, append+drop, true-gap, and
  non-full-turnover cases. Metric tests (`action_gap`, `action_captured` gated on
  `n>0`) retained.
- **Fake-runtime fidelity:** `FakeOp.as_pointer()` returns `id(self)`, so reusing
  the same instance across polls models a persisting ring entry and a fresh
  instance models a new registration — matching real `wm.operators` semantics.
- **Manual in Blender:** run a full-screen modal tool (e.g. a paint/sculpt modal)
  for ~30 s over a scene whose recent history is a repeated `select → move →
  toggle` cycle (the reproduced condition). Confirm **no** `blender.action_gap`
  and **no** duplicate `blender.action` entries accrue while idle-in-modal, and
  that a genuinely new operator during the modal is captured exactly once within
  ~0.1 s. Then force a real >32-op `bpy.ops` burst and confirm a single
  `blender.action_gap` fires.
- **Telemetry verification (post-deploy):** query a session's
  `blender.action_gap` rate and per-message duplicate counts (as done during this
  investigation) and confirm the storm is gone.
- Run `poetry run pytest` and `poetry run black` on changed Python before commit.

## Open Questions
None blocking. One optional lever recorded above (key gaps on `(pointer,
bl_idname)` to fully close the ABA window) — deferred unless reuse is ever
observed in telemetry.
