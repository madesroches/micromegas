# Blender Add-on Metrics & RCA Telemetry Completeness Plan

## Overview
Two threads, addressed together because they touch the same files:

1. **Fix the metric gaps in issue [#1168](https://github.com/madesroches/micromegas/issues/1168)** — make per-process memory cross-platform and periodically sampled, and fix the `blender.eval_ms` metric (misleading name + samples that span a file-load boundary).
2. **Close the telemetry-completeness gaps for root-cause analysis** — per the follow-up objective: *"have enough data for root cause analysis of any issue."* Today the add-on captures a thin fingerprint and a handful of metrics. The dimensions that actually let you slice a crash or a hang (GPU/driver, enabled third-party add-ons, Python exceptions) are missing. This plan does a gap analysis and adds the missing signals.

The work is entirely in the Python add-on (`blender/micromegas_blender/`) and its docs. No changes to the Rust C ABI are required — `mm_log` / `mm_metric_i` / `mm_metric_f` and the `mm_init` process-properties array already expose everything needed.

## Current State

### Process fingerprint (`__init__.py:_build_process_properties`)
Attached once at `mm_init`. Currently: `session_id`, `addon_version`, `blender_version`, `blender_version_hash`, `platform` (`sys.platform`), `os_version` (`platform.version()`).

Notably **absent** — though the original extension plan (`tasks/completed/blender_o11y_extension_plan.md:84`) explicitly listed them as "the dimensions that make stability analysis possible": **GPU/driver**, **enabled add-ons + versions**, CPU model/core count, total RAM, Python version, render engine.

### Metrics (`handlers.py`)
| Metric | Emitted from | Cadence |
|---|---|---|
| `blender.eval_ms` | `_on_depsgraph_update_post` | every depsgraph update |
| `blender.render_duration_s` | `_on_render_post` | per render |
| `blender.blend_size_mb` | `_on_save_post` | per save |
| `blender.rss_mb` | `_on_load_post` → `_emit_memory_metric` | **only on file load** |
| `blender.frame` | `_on_frame_change_post` | per frame change |

- `_emit_memory_metric` (`handlers.py:129`) reads `/proc/self/status` `VmRSS` — **Linux only**. The add-on ships a Windows DLL, so Windows artists get no memory metric. Emitted only from `_on_load_post`, so it misses session-long memory growth.
- `_on_depsgraph_update_post` (`handlers.py:115`) emits the wall-clock interval between consecutive depsgraph callbacks as `blender.eval_ms`. The name implies evaluation duration but the value includes idle time between edits. `_last_depsgraph_time` (`handlers.py:21`) is a module global never reset on load, so the first sample after a load spans the load boundary and is inflated.

### Periodic timer (`__init__.py:_periodic_flush`)
A `bpy.app.timers` callback fires every 30 s and only calls `_lib.flush(_handle)`. **This is the natural home for periodic sampling** (RSS, CPU, scene complexity) — it already runs on a fixed cadence.

### Lifecycle / events (`handlers.py`, `recorder.py`)
Lifecycle: load/save/undo/redo, render start/complete/cancel, frame change, depsgraph. User input: key/mouse/scroll/operator events via the modal recorder. **No capture of Python exceptions** raised inside the embedded interpreter (operator/timer/handler errors) — the single most useful signal for RCA of add-on and scripting issues.

### System-wide vs per-process
`mm_init` spawns the Rust `system_monitor` (`rust/telemetry-sink/src/lib.rs:467`) which emits **machine-wide** `total_memory` / `used_memory` / `free_memory` / `cpu_usage`. These do **not** describe the Blender process — the add-on's own metrics are the only per-process signal.

### Build / platform reality (important for the macOS acceptance criterion)
`build/build_blender_plugin.py` builds **only** `x86_64-unknown-linux-gnu` (`.so`) and `x86_64-pc-windows-gnu` (`.dll`). There is **no macOS `.dylib`**, so on macOS `binding._get_lib_path()` resolves to a `.so` that does not exist and the add-on stays inactive (`__init__.py:_load_lib` prints "native library not found"). See Open Questions.

## Design

### Part 1 — Issue #1168 fixes

#### 1a. Cross-platform, periodic process memory
Add a platform-dispatched RSS reader to `handlers.py` and call it from the periodic timer instead of only on load.

```python
# handlers.py
import sys

_rss_reader = None  # cached resolved reader, picked once on first call

def _read_process_rss_mb() -> float:
    """Resident set size of THIS process in MB, or 0.0 if unavailable."""
    if sys.platform == "linux":
        with open("/proc/self/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) / 1024.0   # kB -> MB
        return 0.0
    if sys.platform == "win32":
        import ctypes
        from ctypes import wintypes
        class _PMC(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD),
                        ("PageFaultCount", wintypes.DWORD),
                        ("PeakWorkingSetSize", ctypes.c_size_t),
                        ("WorkingSetSize", ctypes.c_size_t),
                        # ... remaining PROCESS_MEMORY_COUNTERS fields
                       ]
        counters = _PMC(); counters.cb = ctypes.sizeof(_PMC)
        # K32GetProcessMemoryInfo is in kernel32 on Win7+ (no psapi import needed)
        if ctypes.windll.kernel32.K32GetProcessMemoryInfo(
                ctypes.windll.kernel32.GetCurrentProcess(),
                ctypes.byref(counters), counters.cb):
            return counters.WorkingSetSize / (1024 * 1024)
        return 0.0
    return 0.0   # macOS descoped — see Resolved Decisions
```

- Keep `blender.rss_mb` as the metric name (no consumer churn); only the cadence and platform coverage change.
- Wrap the whole reader in try/except → return `0.0`; emit only when `> 0` (preserves current silent-failure behavior).

#### 1b. Fix `blender.eval_ms`
**Rename** to `blender.depsgraph_update_interval_ms` (unit `ms`) — this is what the value actually measures (interval between depsgraph updates), and there is no Blender handler API to get true scene-eval duration (no pre-eval hook with a paired timer). **Reset** `_last_depsgraph_time` to `0.0` in `_on_load_post` so the first post-load sample is skipped instead of spanning the load boundary.

```python
@bpy.app.handlers.persistent
def _on_load_post(scene, depsgraph=None):
    global _last_depsgraph_time
    _last_depsgraph_time = 0.0          # skip first post-load interval sample
    _log(_b.LEVEL_INFO, "blender.lifecycle", "blend file loaded")
    # (memory now sampled by the periodic timer, not here)

@bpy.app.handlers.persistent
def _on_depsgraph_update_post(scene, depsgraph):
    global _last_depsgraph_time
    now = time.monotonic()
    if _last_depsgraph_time > 0.0:
        _metric_f("blender.depsgraph_update_interval_ms", "ms",
                  (now - _last_depsgraph_time) * 1000.0)
    _last_depsgraph_time = now
```

#### 1c. Periodic-sampling hook
Add `handlers.on_periodic()` and call it from the timer. This keeps all metric emission in `handlers.py` (the timer in `__init__.py` stays a thin flush+sample dispatcher — open/closed: new periodic metrics are added inside `on_periodic`, not by editing the timer).

```python
# handlers.py
def on_periodic() -> None:
    rss = _read_process_rss_mb()
    if rss > 0:
        _metric_f("blender.rss_mb", "mb", rss)
    # Part 3 periodic metrics (cpu %, scene complexity, undo depth) plug in here.

# __init__.py:_periodic_flush
def _periodic_flush():
    if _lib and _handle:
        try:
            from . import handlers
            handlers.on_periodic()
        except Exception:
            pass
        _lib.flush(_handle)
    return 30.0
```

### Part 2 — Fingerprint completeness (highest RCA value, lowest cost)

Extend `_build_process_properties` in `__init__.py`. These are process properties (set once, low cardinality) — exactly the dimensions you group/filter by when triaging. All wrapped in try/except so a failure on one never blocks init.

| Property | Source | Why it matters for RCA |
|---|---|---|
| `gpu_renderer` | `gpu.platform.renderer_get()` | GPU model — the #1 crash dimension for a DCC tool |
| `gpu_vendor` | `gpu.platform.vendor_get()` | NVIDIA/AMD/Intel — driver-class bucketing |
| `gpu_backend` | `gpu.platform.backend_type_get()` | OpenGL / Metal / Vulkan |
| `gpu_driver` | `gpu.platform.version_get()` | driver/GL version string |
| `cpu_count` | `os.cpu_count()` | core count |
| `total_ram_mb` | platform sysinfo (see note) | RAM ceiling for OOM analysis |
| `python_version` | `sys.version.split()[0]` | embedded CPython version |
| `enabled_addons` | `addon_utils` / `bpy.context.preferences.addons` | third-party add-ons — leading cause of Blender instability |
| `background` | `bpy.app.background` | headless render-farm vs interactive |

Notes:
- `gpu.platform.*` requires a GPU context; in `--background` mode it may raise — guard each call individually and skip on failure (don't lose the whole fingerprint).
- `enabled_addons`: serialize as a sorted, comma-joined `name@version` string with real names (cap length, e.g. ~1–2 KB). It is one process property, not per-add-on dimensions, so cardinality stays controlled. (Privacy is a non-issue — see Resolved Decisions.)
- `total_ram_mb`: no stdlib cross-platform call without psutil. Use `os.sysconf('SC_PHYS_PAGES') * SC_PAGE_SIZE` on Linux/macOS and `GlobalMemoryStatusEx` (ctypes) on Windows, or omit if it adds too much code — the Rust `total_memory` system metric already covers the machine.

### Part 3 — Additional periodic metrics (medium value)
Emitted from `handlers.on_periodic()`:

| Metric | Unit | Source | RCA use |
|---|---|---|---|
| `blender.undo_steps` | `count` | `bpy.context.window_manager` undo depth if exposed, else skip | memory-growth correlation |
| `blender.object_count` | `count` | `len(bpy.context.scene.objects)` | scene-complexity correlation with hangs/OOM |

Numeric metric values are unbounded scalars (fine) — only metric *names* must stay bounded, which they are.

**Per-process CPU% deliberately excluded.** It would be process-specific (the Rust `system_monitor`'s `cpu_usage` is `global_cpu_usage()` — the whole machine, all processes), so it is *not* strictly redundant. But it duplicates the system metric closely on a typical single-Blender workstation, and would cost platform-specific code (`/proc/self/stat`, `GetProcessTimes`) plus cached delta state across timer ticks. RSS is the per-process signal that is genuinely irreplaceable, and Part 1 already covers it.

### Part 4 — Python exception capture (high value, slightly more involved)
Install a `sys.excepthook` wrapper in `register()` that ships unhandled exceptions in the embedded interpreter as `ERROR`-level logs to a `blender.exception` target, then chains to the previous hook. Restore the original hook in `unregister()`. Ship the **full traceback** (including file paths and locals-free frames) — privacy is a non-issue in the target environment (see Resolved Decisions); cap the message at ~4 KB.

**Limitation — `sys.excepthook` will not catch most operator/handler/timer exceptions.** Blender installs its own C-level execution wrappers around operators, `bpy.app.handlers`, and `bpy.app.timers`: when those raise, Blender catches the exception internally and reports it via its own console/report mechanism — it never propagates to the top of the main interpreter loop, so `sys.excepthook` does not fire. The hook therefore fires only for the comparatively rare exceptions that do reach the interpreter top level, **not** for the operator/timer/handler failures this plan most wants to capture. Keep the hook as a backstop, but do not rely on it for the RCA targets.

**Complementary capture for the sites the add-on owns.** The realistic mechanism is to wrap the add-on's *own* registered callbacks — the handlers and timers it installs (e.g. `_periodic_flush`, the operator-history poll timer, the lifecycle/depsgraph handlers in `handlers.py`) and any operators the add-on itself defines — in a try/except that logs the exception to `blender.exception` (ERROR, full traceback, ~4 KB cap) and re-raises or returns as appropriate. This catches failures inside the code the add-on controls (where Blender's own wrapper would otherwise swallow them), which is the coverage the plan actually needs. It does not capture exceptions inside *other* add-ons' callbacks — those remain only in Blender's own report stream — and the plan does not claim to.

```python
# __init__.py
_prev_excepthook = None

def _telemetry_excepthook(exc_type, exc_value, exc_tb):
    try:
        import traceback
        text = "".join(traceback.format_exception(exc_type, exc_value, exc_tb))[:4096]
        if _lib and _handle:
            _lib.log(_handle, 2, "blender.exception", text)  # ERROR=2
    finally:
        if _prev_excepthook:
            _prev_excepthook(exc_type, exc_value, exc_tb)
```

### Part 5 — Semantic action capture (the "what did the user click" log)
**This is the highest-value addition for RCA after the fingerprint.** Today `recorder.py` logs only *raw input events* (`LEFTMOUSE PRESS area=VIEW_3D`, key codes) — it cannot tell you the user added a cube, switched to edit mode, or ran a modifier. The docs already overclaim this as "Operator invocations" (`mkdocs/docs/blender/index.md:88`), which is inaccurate and should be corrected.

Blender records nearly every button/menu/shortcut action as a **registered operator** in `bpy.context.window_manager.operators` — the same ring buffer the Info editor displays and "Copy as Python" reads. Each entry exposes `bl_idname` (e.g. `OBJECT_OT_delete`, `MESH_OT_primitive_cube_add`) and `name` ("Delete", "Add Cube"). Draining this buffer gives the semantic action stream — the equivalent of each click — with bounded cardinality (the operator name set is fixed).

**Design — operator-history poller** (new `handlers.on_poll_operators()` or a small module):
- The buffer is a small ordered ring (oldest→newest, historically ~last 10). It must be drained faster than the 30 s flush or rapid clicking overflows it between polls. Use a dedicated `bpy.app.timers` callback at ~1 s.
- Track the tail seen last poll by the `bl_idname` sequence of the most recent few entries. On each poll, locate that anchor in the current buffer and emit everything after it as `blender.action` logs (TRACE). If the anchor can't be found (overflowed since last poll), emit the whole buffer and log a "possible gap" marker — never silently drop.
- Message body: `bl_idname` (+ `name`), **plus operator parameters** when available (e.g. `wm.open_mainfile(filepath=...)`). Privacy is a non-issue in the target corporate environment (users want the telemetry — see Resolved Decisions), so capture parameters by default for maximum RCA signal. Cardinality is not a concern here either: parameters go in the log *message body*, not a metric dimension or log target, so they don't intern/leak. Cap message length (e.g. ~4 KB) to bound payload size.
- **Open risk — parameter extraction on stored history entries.** `bl_idname` and `name` are reliably present on the stored `window_manager.operators` entries, but parameter extraction is not guaranteed: those entries are `OperatorProperties`/macro instances, and `as_keywords()` (or equivalent) is reliable on a *live* operator instance, less so on stored history entries. **Validate this against a real Blender build during implementation.** Defensive fallback regardless of outcome: always log `bl_idname`, attempt parameter extraction inside its own try/except, and omit parameters (logging just `bl_idname`/`name`) when they are unavailable — never assume the extraction call succeeds.

```python
# new operator-history drain, called from a ~1s timer
_last_action_anchor: list[str] = []   # bl_idnames of the last few seen, newest last

def on_poll_operators() -> None:
    try:
        ops = bpy.context.window_manager.operators   # ring buffer, oldest->newest
        idnames = [op.bl_idname for op in ops]
        new = _diff_since_anchor(idnames, _last_action_anchor)  # entries after anchor
        for op in ops[len(idnames) - len(new):]:
            msg = op.bl_idname                                   # always available, bounded
            try:                                                 # params: open risk, may be absent
                params = dict(op.as_keywords())
                if params:
                    msg = f"{msg} {params}"[:4096]
            except Exception:
                pass                                             # omit params, keep bl_idname
            _log(_b.LEVEL_TRACE, "blender.action", msg)
        _set_anchor(idnames)
    except Exception:
        pass
```

**Adjacent semantic signals worth capturing** (all bounded, all answer "what state was the user in"):
- **Mode changes** (`object`/`edit`/`sculpt`/`pose`) — derivable from `OBJECT_OT_mode_set` in the operator stream, or poll `bpy.context.mode` on a state-change basis. Log `blender.mode` transitions.
- **Active workspace / editor focus change** — poll `bpy.context.workspace.name` (bounded) on change → `blender.workspace`.
- **Active tool change** — toolbar tool id from `bpy.context.workspace.tools`.
- **Runtime add-on enable/disable** — pairs with the enabled-add-ons fingerprint; a mid-session enable is a prime crash trigger.

**What stays out:** only selection changes (too frequent, low RCA value relative to volume). Operator parameters are now included (privacy resolved).

### Scope / build order
All five parts are in scope (the objective is maximum RCA signal). Suggested build order by value-per-effort, but all ship together:
1. **Part 1** — required by #1168.
2. **Part 5 (semantic action capture)** — the "what did the user click" log; closes the gap between raw input events and actual actions, and corrects an inaccurate doc claim. Highest action-replay value.
3. **Part 2 (fingerprint — GPU + enabled add-ons especially)** — biggest dimension payoff per line of code.
4. **Part 4 (exceptions)** — captures the actual failures users hit.
5. **Part 3 (extra periodic metrics)** — object count, undo depth.

## Implementation Steps

### Phase 1 — Issue #1168 (committed scope)
1. `handlers.py`: add `_read_process_rss_mb()` (Linux + Windows; macOS descoped). Remove `_emit_memory_metric`'s `/proc`-only body; replace its call in `_on_load_post`.
2. `handlers.py`: add `on_periodic()` that emits `blender.rss_mb` from the reader.
3. `__init__.py`: `_periodic_flush` calls `handlers.on_periodic()` before flush.
4. `handlers.py`: rename `blender.eval_ms` → `blender.depsgraph_update_interval_ms`; reset `_last_depsgraph_time = 0.0` in `_on_load_post`.
5. Create unit tests (see Testing).

### Phase 2 — Fingerprint
6. `__init__.py:_build_process_properties`: add GPU (`gpu.platform.*`), `enabled_addons`, `cpu_count`, `python_version`, `background`, optional `total_ram_mb`. Each in its own try/except.

### Phase 3 — Semantic action capture
7. New operator-history poller (in `recorder.py` or a new `actions.py`): ~1 s timer draining `bpy.context.window_manager.operators` → `blender.action` TRACE logs (always `bl_idname` + `name`; parameters via `as_keywords()` inside its own try/except, omitted when unavailable — validate availability on a real build; message capped ~4 KB). Anchor/diff logic with gap marker on overflow.
8. Mode/workspace/tool transition logging; runtime add-on enable/disable.
9. Register/unregister the new timer in `__init__.py` alongside `_periodic_flush`.

### Phase 4 — Exceptions + extra metrics
10. `__init__.py`: install/restore `sys.excepthook` wrapper shipping full tracebacks as `blender.exception` ERROR logs (message capped ~4 KB).
11. `handlers.py:on_periodic`: add `blender.object_count`, `blender.undo_steps`.

### Phase 5 — Docs
12. Update `mkdocs/docs/blender/index.md` (see Documentation), including correcting the "Operator invocations" claim.
13. Correct `recorder.py`'s module docstring (lines 15–17): drop the "operator `bl_idname` is logged" / `VERBOSE_PARAMS` claims — `modal()` logs only raw input events, no operator capture and no such preference exists.

## Files to Modify
- `blender/micromegas_blender/handlers.py` — RSS reader, `on_periodic`, depsgraph rename + reset.
- `blender/micromegas_blender/__init__.py` — periodic hook call, fingerprint props, excepthook, action-poll timer registration.
- `blender/micromegas_blender/recorder.py` (or new `actions.py`) — operator-history poller, mode/workspace/tool transitions; correct the module docstring's inaccurate operator-`bl_idname`/`VERBOSE_PARAMS` claim (lines 15–17).
- `mkdocs/docs/blender/index.md` — metrics table, fingerprint list, semantic-action section, corrected "Operator invocations" claim, new sections.
- Tests under the add-on's test location (see Testing).

## Trade-offs
- **Rename vs. measure true eval duration** (#1168 option): the Blender handler API exposes no pre-eval hook, so true scene-eval duration isn't obtainable from `depsgraph_update_post`. Renaming to `depsgraph_update_interval_ms` is honest about what is measured; chosen over a misleading metric or an unimplementable one.
- **ctypes vs. psutil** for cross-platform memory/CPU: psutil would be one clean API but is a heavy third-party dep that would have to be vendored into the extension zip. The add-on is deliberately dependency-light (pure ctypes binding); a few platform branches keep it that way.
- **Enabled add-ons as one joined string vs. per-add-on events**: a single bounded process property avoids cardinality blow-up and keeps the data queryable as a dimension; the cost is substring matching when querying. Acceptable.
- **Renaming `blender.eval_ms` breaks existing dashboards/queries**: there is no compatibility shim — the metric is new (shipped in #1170, June 2026) with minimal adoption, so a clean rename beats carrying a misnamed alias.

## Bandwidth Estimation

Rough per-artist estimate for the full set of signals. Micromegas batches events into per-stream blocks and ships them compressed; events are compact binary (interned static metadata + 8-byte timestamp + payload), so metric streams compress very well. Assumptions stated inline; treat as order-of-magnitude.

**Per-stream rates** (active editing; raw = pre-compression bytes/sec):

| Stream | Part | events/s (active avg) | bytes/event | raw B/s |
|---|---|---|---|---|
| System monitor (`used/free_memory`, `cpu_usage`) — *existing* | — | ~15 (3 every ~200 ms) | ~24 | ~360 |
| `depsgraph_update_interval_ms` metric | 1 | ~5 (bursts to 30–60 during modal drag) | ~24 | ~120 |
| Raw input events (recorder) — *existing* | — | ~2 | ~50 | ~100 |
| Semantic actions + params | 5 | ~1 | ~200 | ~200 |
| `frame` metric (during playback only) | — | ~2 avg (24–60 while playing) | ~24 | ~48 |
| Throttled mouse motion — *existing* | — | 1 | ~24 | ~24 |
| Periodic metrics (`rss_mb`, `object_count`, `undo_steps`) | 1/3 | ~0.1 (3 per 30 s) | ~24 | ~2 |
| **Total (active)** | | | | **~850 B/s** |

- **Idle** (file open, artist away): only the system monitor + periodic metrics remain → **~360 B/s** raw.
- **Compression**: assume ~4× effective on the wire (conservative — repetitive metric streams do better). → **~210 B/s active**, **~90 B/s idle** compressed.

**Aggregated:**

| Window | Estimate (compressed) |
|---|---|
| Active hour | ~0.75 MB |
| Idle hour (file open) | ~0.3 MB |
| 8 h workday (~4 h active + 4 h open-idle) | **~4–5 MB / artist / day** |
| 100-artist studio | ~0.4–0.5 GB / day |
| 1,000-artist fleet | ~4–5 GB / day |

**Notes / outliers:**
- **Bursts dominate, not steady state.** Interactive modal transforms (continuous `depsgraph_update_post`) and timeline playback (`frame` per frame) spike to 30–60 events/s for seconds at a time → short ~1–2 KB/s raw (~0.5 KB/s compressed) bursts, smoothed by the 30 s flush buffer. Still negligible in aggregate.
- **The existing system monitor (~15 ev/s) is the largest steady stream** — larger than most of what this plan adds. If bandwidth ever matters, the highest-leverage knob is its ~200 ms cadence, not the add-on.
- **One-time / rare:** process fingerprint (~1–2 KB at startup, incl. add-on list), Python exceptions (~4 KB each, rare), crash-file harvest (up to 512 KB, capped, per crash). None affect steady-state.
- **Throttle candidate:** if the `depsgraph_update_interval_ms` burst rate proves noisy, rate-limit it in `_on_depsgraph_update_post` (e.g. emit at most every N ms). Not needed for bandwidth, possibly for signal cleanliness.

Bottom line: **single-digit MB per artist per day**, dominated by the pre-existing system monitor; the new RCA signals add well under half of that.

## Documentation
`mkdocs/docs/blender/index.md`:
- **Performance metrics table**: rename `blender.eval_ms` → `blender.depsgraph_update_interval_ms` with corrected description; change `blender.rss_mb` description from "at file load" to "sampled every ~30 s"; add any Part 3 metrics.
- **Process fingerprint** section: add GPU/driver, enabled add-ons, CPU count, Python version, background-mode.
- **User actions** section: correct the inaccurate "Operator invocations (type, area)" claim (line 88) — today only raw input events are logged. Also rewrite the "Coverage is high but not 100%" admonition (lines 91–95): drop the now-contradictory "Operator parameter values are not captured by default … sensitive scene/asset names" note, since the new approach captures parameters by default. Document the new `blender.action` operator-history stream (Part 5), the ~1 s poll, the ring-buffer gap caveat, and that operator parameters are included.
- New subsection under "What is captured" for **Python exceptions** (if Part 4 ships) — full tracebacks shipped as `blender.exception` ERROR logs.
- **Privacy and cardinality** section: rewrite — privacy gating is removed (corporate environment, telemetry wanted); the remaining rule is purely cardinality (bounded metric names / log targets; free-form values only in log bodies). Note the enabled-add-ons string and operator parameters are captured in full.

## Testing Strategy
The add-on has no existing test harness — `blender/micromegas_blender/` has no `tests/` dir and nothing imports `micromegas_blender`/`binding`/`handlers`. These tests are created from scratch. Add a `tests/` directory under `blender/micromegas_blender/` (pure-pytest, `bpy`/`gpu` mocked) so the binding and handlers can be exercised out-of-Blender; no Blender runtime is required.
- Unit-test `_read_process_rss_mb()` on Linux (CI) — returns `> 0` for the test process; assert MB magnitude is plausible. Windows branch verified manually or mocked.
- Test the depsgraph reset: simulate two `_on_depsgraph_update_post` calls with a `_on_load_post` between them; assert the first post-load call emits nothing (no metric until a second sample).
- Fingerprint: mock `bpy`/`gpu` and assert props dict contains the new keys and survives individual source failures (one raising doesn't drop the rest).
- Operator-history poller: feed a mock `window_manager.operators` ring buffer across successive polls — assert only newly appended `bl_idname`s are emitted, no duplicates on a no-change poll, and an overflow (anchor not found) emits a gap marker rather than silently dropping.
- excepthook: install, raise, assert an ERROR `blender.exception` log was emitted and the previous hook was chained.
- `binding.py` stays pure-ctypes and bpy-free so the new tests can run out-of-Blender.

## Open Questions
None outstanding — scope, macOS, and privacy are all resolved below.

## Resolved Decisions
- **Scope — all five parts.** The objective is maximum RCA signal; everything ships in one change (#1168 fixes + fingerprint + semantic action capture + exceptions + extra metrics). Build order in "Scope / build order" above.
- **macOS — descoped.** The user does not care about macOS at this point. Implement RSS for Linux + Windows only; drop the macOS branch from `_read_process_rss_mb` (no `darwin` stub). This intentionally narrows #1168's stated acceptance criterion ("Linux, Windows, and macOS") to Linux + Windows, which also matches reality — the build (`build_blender_plugin.py`) ships no macOS `.dylib`, so the add-on is dormant on macOS regardless.
- **Privacy / data sensitivity — non-issue.** This runs in a corporate environment where users *want* the telemetry. No gating, scrubbing, or opt-in preferences. Concretely: ship **full Python tracebacks** (Part 4), capture **operator parameters** in the action log (Part 5), and emit the **enabled-add-ons list** with real names (Part 2) — all by default. The only remaining discipline is *cardinality* of metric names/targets (a producer-side constraint, unrelated to privacy): keep metric names and log targets bounded; free-form values go in log message bodies, which is unbounded-safe.
