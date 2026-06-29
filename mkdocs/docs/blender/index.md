# Blender Add-on

The Micromegas Blender add-on captures session telemetry — user actions,
lifecycle events, and performance metrics — from a running Blender instance
and ships them to a Micromegas ingestion server.  This gives your team
visibility into Blender stability and performance without modifying Blender
itself or requiring a custom build.

## Architecture

```
Blender process
├── Blender C/C++ core ──(crash)──► Blender's own *.crash.txt
└── Embedded CPython
    └── micromegas_blender add-on
        ├── crash_harvester  (ships prior crash files on next launch)
        ├── handlers         (bpy.app.handlers + bpy.msgbus + periodic metrics)
        ├── recorder         (persistent modal operator — raw input events)
        ├── actions          (operator-history poller — semantic actions)
        └── binding          (ctypes → libmicromegas_capi)
                                       │
                         libmicromegas_capi.{so,dll}
                         └── HttpEventSink ── OS thread ──► ingestion-srv
```

The native library owns its own OS thread for HTTP transport.
Python code never blocks on the network.

## Installation

### Prerequisites

- Blender 4.2 or later (stock public build, x86-64)
- A running [Micromegas ingestion server](../getting-started.md)

### Install the add-on

1. Download the pre-built extension zip from the releases page.
2. The extension zip bundles the pre-compiled `libmicromegas_capi.so` / `micromegas_capi.dll` for both platforms.
3. In Blender: **Edit → Preferences → Extensions → Install from Disk…** → select the `.zip` file.
4. Enable **Micromegas Telemetry**.

### Manual install (development)

```bash
# 1. Build the native library
cd rust/
cargo build --package micromegas-capi --release

# 2. Copy it into the add-on's lib/ directory
cp target/release/libmicromegas_capi.so \
   ../blender/micromegas_blender/lib/

# 3. Symlink or copy the add-on directory into Blender's extensions path
ln -s $(pwd)/../blender/micromegas_blender \
   ~/.config/blender/4.x/extensions/user_default/micromegas_blender
```

## Configuration

All configuration is via environment variables set before launching Blender
(system-wide or via the launcher script used in your studio pipeline).

| Variable | Required | Description |
|---|---|---|
| `MICROMEGAS_TELEMETRY_URL` | Yes | Ingestion server, e.g. `http://ingestion:9000` |
| `MICROMEGAS_INGESTION_API_KEY` | No | API key for authenticated ingestion |
| `MICROMEGAS_OIDC_TOKEN_ENDPOINT` | No | OIDC token endpoint (alternative auth) |
| `MICROMEGAS_OIDC_CLIENT_ID` | No | OIDC client ID |
| `MICROMEGAS_OIDC_CLIENT_SECRET` | No | OIDC client secret |

## What is captured

### Process fingerprint (at startup)

These properties are attached to the process record and are the dimensions
that make stability and performance analysis possible:

- Blender version + build hash
- OS version, platform
- Per-launch session UUID
- Add-on version
- GPU renderer, vendor, backend, and driver/GL version (`gpu_renderer`,
  `gpu_vendor`, `gpu_backend`, `gpu_driver`) — the #1 crash dimension for a DCC
  tool (skipped in `--background` mode where no GPU context exists)
- Enabled third-party add-ons as a sorted `name@version` list (`enabled_addons`,
  capped ~2 KB) — a leading cause of Blender instability
- CPU core count, total RAM (`cpu_count`, `total_ram_mb`)
- Embedded Python version (`python_version`)
- Active render engine (`render_engine`)
- Headless vs interactive (`background`)

### User actions (continuous)

Two complementary streams:

**Raw input events** — captured via a persistent modal operator (`recorder`):

- Discrete key/mouse events (type, area) — logged at TRACE level
- Throttled mouse motion (at most once per second) — logged at TRACE level

**Semantic actions** — captured by the operator-history poller (`actions`):

- Each operator the user invoked (`bl_idname`, name, **and parameters** when
  available) drained from `bpy.context.window_manager.operators` and logged to
  `blender.action` at TRACE level. This is the "what did the user click" stream
  — adding a cube, switching to edit mode, running a modifier.
- Mode / workspace / active-tool transitions → `blender.mode`,
  `blender.workspace`, `blender.tool` (TRACE).
- Runtime add-on enable/disable → `blender.addon_state` (INFO) — a mid-session
  enable is a prime crash trigger.

Draining is **event-driven**: on every discrete input event (key/mouse/scroll)
the recorder modal calls `drain_operators()` so the ring is drained at
per-keystroke cadence, well within the 32-operator hard cap. A ~1 s timer runs
as a backstop for periods when the recorder modal is suspended or receiving only
motion events.

The ring is small and ordered (oldest→newest); each drain emits only the
operators appended since the last drain. If the ring turned over entirely between
two consecutive recorder events (e.g. a script/macro burst), a `possible gap`
marker is logged rather than silently dropping — actions are never lost without a
signal. Two metrics track capture health:

| Metric | Unit | Description |
|---|---|---|
| `blender.action_captured` | count | Operators successfully logged (emitted only when > 0) |
| `blender.action_gap` | count | Ring-overflow events between drains |

!!! note "Coverage is high but not 100%"
    The recorder modal is suspended while a full-screen sub-modal (knife, grab, …)
    is running; the timer backstop covers that window. The ring is hard-capped at
    32 entries by Blender (`MAX_OP_REGISTERED`) and cannot be resized via Python.
    Operator parameter extraction from stored history entries is best-effort:
    when parameters are unavailable the action is still logged with its `bl_idname`
    and name.

### Lifecycle events (logged at INFO)

- Blend file loaded / saved
- Undo / redo
- Render start / complete / cancel
- Frame change

### Python exceptions

Unhandled exceptions that reach the embedded interpreter's top level are shipped
as ERROR-level logs to the `blender.exception` target with the full traceback
(capped ~4 KB), then chained to the previous `sys.excepthook`.

!!! note "Backstop, not full coverage"
    Blender wraps operators, `bpy.app.handlers`, and `bpy.app.timers` in its own
    C-level execution handlers and reports their exceptions through its console,
    so those never reach `sys.excepthook`. The add-on's *own* timers and
    handlers guard themselves; this hook catches the comparatively rare
    exceptions that do propagate to the interpreter top level.

### Performance metrics

| Metric | Unit | Description |
|---|---|---|
| `blender.depsgraph_update_interval_ms` | ms | Wall-clock interval between consecutive depsgraph updates (includes idle time between edits — not pure evaluation time; Blender exposes no pre-eval hook) |
| `blender.render_duration_s` | s | Wall time per render |
| `blender.blend_size_mb` | mb | `.blend` file size at save |
| `blender.rss_mb` | mb | Process resident memory, sampled every ~30 s (Linux + Windows) |
| `blender.object_count` | count | Objects in the active scene, sampled every ~30 s |
| `blender.frame` | frame | Current frame number at frame-change |

### Crash capture (Phase 1: harvest on next launch)

On startup the add-on scans for `*.crash.txt` files written by Blender after
an abnormal exit.  If found, the file is claimed via atomic rename and
uploaded as a FATAL-level log entry.

This approach is:

- **Dedup-safe**: two concurrent Blender instances each try to `rename()` the
  crash file; only the winner uploads it.
- **Best-effort**: if the upload fails after claiming, the crash is lost (no
  retry queue).  This is intentional — Phase 1 measures how lossy the free
  path is before investing in a Crashpad-based Phase 2.

## Cardinality

The add-on is built for environments where the telemetry is wanted in full, so
there is no privacy gating, scrubbing, or opt-in: full Python tracebacks,
operator parameters, and the enabled-add-on list (with real names) are all
captured by default. The one remaining discipline is **cardinality**, a
producer-side constraint unrelated to privacy:

- Metric **names** and log **targets** are always from a bounded, low-cardinality
  set (e.g. `blender.action`, `blender.rss_mb`) — never per-asset names, file
  paths, or session IDs.
- Free-form, high-cardinality values (operator parameters, file paths,
  tracebacks, the add-on list) go only in the log **message body**, which is
  unbounded-safe.

## Querying the data

After the add-on is active, use the Micromegas query interface:

```sql
-- Last hour of Blender log events
SELECT time, level, target, msg
FROM log_entries
WHERE target LIKE 'blender.%'
ORDER BY time DESC
LIMIT 100;

-- Average render duration by day
SELECT
    date_trunc('day', time) AS day,
    avg(value) AS avg_render_s
FROM metrics
WHERE name = 'blender.render_duration_s'
GROUP BY 1
ORDER BY 1 DESC;

-- Crash events from the last 7 days
SELECT time, msg
FROM log_entries
WHERE target = 'blender.crash'
  AND level = 1  -- FATAL
ORDER BY time DESC;

-- Reconstruct one session's action sequence (what the user clicked).
-- Scope to the process with view_instance so only that session's blocks are
-- read, instead of scanning the whole log_entries view.
SELECT time, msg
FROM view_instance('log_entries', 'my_process_id')
WHERE target = 'blender.action'
ORDER BY time DESC
LIMIT 100;
```

## Troubleshooting

**Add-on loads but no data appears in Micromegas**

- Check that `MICROMEGAS_TELEMETRY_URL` is set and reachable from the Blender machine.
- Check Blender's system console (Window → Toggle System Console on Windows, or
  launch from terminal on Linux) for `[Micromegas]` error messages.

**"native library not found" in the system console**

The prebuilt `libmicromegas_capi.so` / `micromegas_capi.dll` is missing from the
add-on's `lib/` directory.  Re-install from the extension zip, or follow the
manual install steps above.

**Multiple Blender instances — will they conflict?**

No.  Each instance initializes its own `TelemetryGuard` with a unique
session UUID.  The transport thread is per-process.  Crash-file claiming via
atomic rename ensures no double-reporting.
