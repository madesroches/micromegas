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
        ├── handlers         (bpy.app.handlers + bpy.msgbus)
        ├── recorder         (persistent modal operator)
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

### User actions (continuous)

Captured via a persistent modal operator + `bpy.app.handlers`:

- Discrete key/mouse events (type, area) — logged at TRACE level
- Operator invocations (type, area) — logged at TRACE level
- Throttled mouse motion (at most once per second) — logged at TRACE level

!!! note "Coverage is high but not 100%"
    The modal operator can be suspended while a full-screen sub-modal is
    running.  Operator parameter values are not captured by default
    (only `bl_idname` / event type to avoid logging potentially sensitive
    scene/asset names).

### Lifecycle events (logged at INFO)

- Blend file loaded / saved
- Undo / redo
- Render start / complete / cancel
- Frame change

### Performance metrics

| Metric | Unit | Description |
|---|---|---|
| `blender.eval_ms` | ms | Scene evaluation time between depsgraph updates |
| `blender.render_duration_s` | s | Wall time per render |
| `blender.blend_size_mb` | mb | `.blend` file size at save |
| `blender.rss_mb` | mb | Process resident memory at file load |
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

## Privacy and cardinality

The add-on enforces strict cardinality and privacy rules:

- Metric property values are always from a **bounded, low-cardinality set**
  (event type, area type, status — never per-asset names, session IDs, or
  file paths).
- Scene and asset names, file paths, and operator parameter values are
  **never emitted** as metric dimensions or log targets.
- Log messages for lifecycle events name the *type* of event, not the content
  (e.g. `"blend file saved"`, not the file path).

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
