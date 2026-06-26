# Micromegas Blender Add-on

Captures Blender session telemetry — user actions, lifecycle events, and
performance metrics — and ships them to a Micromegas ingestion server.

## How it works

The add-on is a Blender Python package (`micromegas_blender`) that loads
a prebuilt native library (`micromegas-capi`) via `ctypes`.  The native
library owns a background OS thread that handles HTTP transport; the GIL is
never held by the transport, so uploads continue even while Python is blocked.

```
Blender process
├── Embedded CPython
│   └── micromegas_blender (add-on)
│       ├── modal recorder   — operator invocations, key/mouse events
│       ├── bpy.app.handlers — load/save, undo/redo, render, frame change
│       ├── bpy.msgbus       — active-object property edits
│       └── ctypes binding
│                 │
│                 ▼
└── libmicromegas_capi.so / micromegas_capi.dll
      └── background thread ──► ingestion server (HTTP)
```

---

## Prerequisites

| Tool | Version | Notes |
|------|---------|-------|
| Rust + Cargo | stable ≥ 1.75 | via [rustup](https://rustup.rs) |
| MinGW-w64 | any | only for Windows cross-compile from Linux (see below) |
| Blender | ≥ 4.2 | stock public build (4.5 tested) |
| Python | 3.11 | the one bundled with Blender (no separate install needed) |

No build tools are required on the artist's machine — the native library is
shipped prebuilt inside the add-on directory.

---

## Building the native library

Use the build script at `build/build_blender_plugin.py`.  It compiles
`micromegas-capi` for the target platforms and copies the resulting
libraries directly into `blender/micromegas_blender/lib/`.

```bash
# from the repo root — build for both Linux and Windows
python3 build/build_blender_plugin.py

# build only one platform
python3 build/build_blender_plugin.py --platform linux
python3 build/build_blender_plugin.py --platform windows
```

| Platform | How it builds | Output |
|----------|--------------|--------|
| Linux | native `cargo build` | `libmicromegas_capi.so` |
| Windows on Windows | native `cargo build` | `micromegas_capi.dll` |
| Windows from Linux/WSL | MinGW-w64 cross-compiler (no Docker) | `micromegas_capi.dll` (GNU toolchain) |

For the Windows cross-compile from Linux, install the prerequisites once:

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64
```

> **MSVC DLL for production:** the CI workflow `capi-release.yml` builds an
> MSVC-toolchain `.dll` on a native Windows runner.  The GNU-toolchain DLL
> is compatible with stock Blender and sufficient for development and testing.

After the script runs the `lib/` directory is ready:

```
blender/
└── micromegas_blender/
    ├── __init__.py
    ├── binding.py
    ├── handlers.py
    ├── recorder.py
    ├── crash_harvester.py
    └── lib/
        ├── libmicromegas_capi.so   ← Linux
        └── micromegas_capi.dll    ← Windows
```

---

## Installing the add-on in Blender

### Option A — Install from a zip

1. Run the build script (see [Building the native library](#building-the-native-library) above).
   It produces `blender/micromegas_blender.zip`.

2. In Blender: **Edit → Preferences → Extensions → Install from Disk…**
   Select `micromegas_blender.zip` and click **Install Extension**.

   Alternatively, drag the zip file onto the Blender window.

3. Enable the extension by checking the box next to
   **Micromegas Telemetry**.

### Option B — Install from the source tree (development)

Symlink or copy the `micromegas_blender/` directory (with `lib/` populated)
into Blender's user add-on scripts directory.  Replace `4.5` with your
Blender version if different.

```bash
# Linux / WSL
ln -s "$(pwd)/blender/micromegas_blender" \
      "$HOME/.config/blender/4.5/extensions/user_default/micromegas_blender"
```

```powershell
# Windows (run as administrator or with Developer Mode enabled)
New-Item -ItemType Junction `
  -Path "$env:APPDATA\Blender Foundation\Blender\4.5\extensions\user_default\micromegas_blender" `
  -Target "$PWD\blender\micromegas_blender"
```

After linking, enable the extension in **Edit → Preferences → Extensions**.

---

## Configuration

All configuration is via environment variables set **before** launching
Blender (system-wide, via the launcher, or shell profile):

| Variable | Required | Description |
|----------|----------|-------------|
| `MICROMEGAS_TELEMETRY_URL` | Yes | Ingestion server endpoint, e.g. `http://ingest.example.com:9000` |
| `MICROMEGAS_INGESTION_API_KEY` | No | API key for authenticated ingestion |
| `MICROMEGAS_OIDC_TOKEN_ENDPOINT` | No | OIDC token endpoint (alternative auth) |
| `MICROMEGAS_OIDC_CLIENT_ID` | No | OIDC client ID |
| `MICROMEGAS_OIDC_CLIENT_SECRET` | No | OIDC client secret |

If `MICROMEGAS_TELEMETRY_URL` is not set the add-on loads but remains inactive
(it prints a warning to the Blender console).

### Quick local test

```bash
export MICROMEGAS_TELEMETRY_URL=http://127.0.0.1:9000
blender
```

---

## What gets captured

### Process fingerprint (set at startup, attached to every event)

- Blender version and build hash
- Operating system and version
- Per-launch UUID (session ID)
- Add-on version

### User actions (via modal recorder + `bpy.app.handlers` + `bpy.msgbus`)

- Operator invocations (type and area, not parameters)
- Key and mouse-button events (throttled, continuous motion not recorded)
- Active-object type changes
- File load, save, undo, redo

### Performance metrics

| Metric | Unit | Source |
|--------|------|--------|
| `blender.eval_ms` | ms | Depsgraph update interval |
| `blender.render_duration_s` | s | render_pre → render_post |
| `blender.blend_size_mb` | mb | File size on save |
| `blender.rss_mb` | mb | `/proc/self/status` (Linux) |
| `blender.frame` | frame | frame_change_post |

### Crash reports (on next launch)

On startup the add-on scans the system temp directory for `*.crash.txt` files
left by a prior abnormal Blender exit.  Each file is claimed atomically (to
avoid double-reporting across concurrent instances) and shipped as a
`FATAL`-level log.  The last user actions before the crash are already in the
telemetry stream under the prior session's fingerprint — no extra local store
is maintained.

---

## Privacy and cardinality

- **No file paths, scene names, or asset names** are emitted as metric
  dimensions or log fields.
- Operator parameters are not captured by default (only the operator type).
- Metric dimension values are bounded (operator type, area type) — no
  per-session or per-asset values as dimensions.
- The session UUID is used only as a process property for fingerprinting, not
  as a metric dimension.

---

## Verifying it works

Enable the Blender **System Console** (Window → Toggle System Console on
Windows; or launch from a terminal on Linux) and look for:

```
[Micromegas] Micromegas add-on registered
[Micromegas] session_id=<uuid>
```

To confirm events reach the server, run a query after a short session:

```bash
micromegas-query "SELECT time, level, target, msg FROM log_entries \
  WHERE target LIKE 'blender.%' LIMIT 20" --begin 1h
```

---

## Troubleshooting

| Symptom | Likely cause |
|---------|-------------|
| `native library not found` in console | `lib/` directory missing or wrong filename; rebuild and copy the `.so`/`.dll` |
| `telemetry init failed` | `MICROMEGAS_TELEMETRY_URL` is unset or the server is unreachable at startup |
| No rows in the server after a session | Server URL incorrect, or events are still buffered — wait 30 s for the periodic flush or restart Blender to trigger `mm_shutdown` |
| Add-on inactive after disabling then re-enabling it in the same session | The native telemetry layer initializes once per process and cannot be reinitialized; restart Blender |
| Add-on not listed after install | Zip wrapped the files in an extra parent directory; the zip must contain `blender_manifest.toml` and `__init__.py` at its root (not inside a `micromegas_blender/` folder) |
