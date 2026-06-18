# ARM64 Cross-Compilation in Docker Plan

## Overview

Add `linux/arm64` support to all production Dockerfiles so that an x86-64 developer can build
ARM64 images without QEMU emulation. The build relies on a Debian cross-compile toolchain
(`g++-aarch64-linux-gnu`) — a pattern already
proven in `local_test_env/arm64/Dockerfile`. The build script gains an `--arm64` flag that
drives `docker buildx` to produce a single-arch arm64 image loaded into the local docker
image store. Multi-arch manifest publishing is out of scope (see Trade-offs).

## Current State

### Production Dockerfiles (all x86-native today)

All four service Dockerfiles follow the same two-stage pattern:

```
FROM rust:1-bookworm AS builder
...
RUN cargo build --release --bin <name>          # native only

FROM debian:bookworm-slim
COPY --from=builder /build/rust/target/release/<name> /usr/local/bin/
```

Files: `docker/ingestion.Dockerfile`, `docker/flight-sql.Dockerfile`,
`docker/http-gateway.Dockerfile`, `docker/admin.Dockerfile`.

These four files name the build stage `AS builder` and copy via `COPY --from=builder ...`. The
cross-compile pattern below uses `AS rust-builder`/`COPY --from=rust-builder ...` only as an
illustrative template — implementers must apply it to **whatever each file's existing builder
stage is named** (these four keep `builder`; the multi-stage Dockerfiles already use
`rust-builder`), using that actual name in both the `FROM ... AS <stage>` and the matching
`COPY --from=<stage>` references. No stage rename is implied.

The monolith adds WASM and Node stages before the Rust stage:
`docker/monolith.Dockerfile` (4 stages), `docker/all-in-one.Dockerfile` (4 stages).

`wasm-builder.Dockerfile` uses `dpkg --print-architecture` to select the Binaryen binary
(line 17-24). This reports the architecture of the stage it runs in — but that logic only
executes when the `micromegas-wasm-builder:latest` image is **built** (`ensure_wasm_builder()`
runs a plain amd64 `docker build`), not when that image is later **consumed** via
`FROM ${WASM_IMAGE} AS wasm-builder`. So `dpkg`/binaryen selection is already settled at
wasm-builder build time and is unaffected by how the consuming Dockerfiles resolve the stage.
The three WASM-service Dockerfiles currently use a plain, unpinned
`FROM ${WASM_IMAGE} AS wasm-builder` that references a prebuilt shared image across build
invocations — this design **replaces** that with an inlined, `--platform=$BUILDPLATFORM`-pinned
`wasm-builder` stage carrying the body of `wasm-builder.Dockerfile` (see Design and Phase
steps). Inlining avoids the cross-build image-resolution problem on the arm64 path (the buildx
`docker-container` driver cannot read the daemon image store as a `FROM` base — see Design). The
WASM output is arch-neutral (`wasm32-unknown-unknown`) and consumed only via
`COPY --from=wasm-builder`.

The `node:20-alpine` `frontend-builder` stage in those three Dockerfiles is likewise currently
unpinned, so under `docker buildx build --platform linux/arm64` it would resolve to arm64 and
run `yarn install`/`yarn build` under QEMU. This design also adds a `--platform=$BUILDPLATFORM`
pin to the `frontend-builder` stage so yarn runs natively on the amd64 host (its output is
arch-neutral static assets).

### Cross-compile proof-of-concept

`local_test_env/arm64/Dockerfile` already solved the hard parts:
- Installs `g++-aarch64-linux-gnu` and `libc6-dev-arm64-cross`
- Adds `rustup target add aarch64-unknown-linux-gnu`

The workspace links no OpenSSL: `rust/Cargo.lock` has no `openssl-sys`/`openssl` entry
(only the unrelated `openssl-probe` CA-cert locator), and `rust/Cargo.toml` configures
`reqwest` with `rustls-tls` and `tonic` with `tls-ring`/`tls-native-roots`. No OpenSSL
cross-build is needed.

### `.cargo/config.toml`

Only defines a linker override for `x86_64-unknown-linux-gnu`. No entry for
`aarch64-unknown-linux-gnu`.

### Build script

`build/build_docker_images.py` calls `docker build` directly (no `buildx`, no `--platform`).

## Design

### Cross-compile pattern for Rust builder stages

The Rust builder stage in each Dockerfile is pinned to the native build host with
`FROM --platform=$BUILDPLATFORM` so its `RUN` steps execute natively (no QEMU) while emitting
the arm64 target binary. Without this pin, `docker buildx build --platform linux/arm64`
resolves every unpinned `FROM` to the **target** platform (arm64) and runs the Rust
compilation under QEMU — exactly the slow path this plan rejects.

The stage gains an `ARG TARGETARCH` that Docker BuildKit sets automatically. When
`TARGETARCH=arm64` the stage installs the cross toolchain and builds for
`aarch64-unknown-linux-gnu`; otherwise it builds natively.

```dockerfile
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS rust-builder

ARG TARGETARCH

# Install cross toolchain only when needed
RUN if [ "$TARGETARCH" = "arm64" ]; then \
      apt-get update && \
      apt-get install -y --no-install-recommends \
        g++-aarch64-linux-gnu libc6-dev-arm64-cross && \
      rm -rf /var/lib/apt/lists/*; \
    fi

RUN if [ "$TARGETARCH" = "arm64" ]; then \
      rustup target add aarch64-unknown-linux-gnu; \
    fi

WORKDIR /build
COPY rust/ ./rust/
WORKDIR /build/rust

RUN if [ "$TARGETARCH" = "arm64" ]; then \
      CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
      cargo build --release --target aarch64-unknown-linux-gnu --bin <name>; \
    else \
      cargo build --release --bin <name>; \
    fi
```

The runtime stage starts fresh `FROM debian:bookworm-slim`, so it has no `/build/rust/target`
of its own — artifacts must be pulled across the stage boundary with `COPY --from=rust-builder`.
Because Dockerfile `ARG` defaults don't support shell-style conditional expansion, the builder
stage first materializes the binary at a single arch-independent path with a `RUN cp`, then the
runtime stage `COPY`s from there — keeping the runtime `COPY` source free of variables:

```dockerfile
# --- end of rust-builder stage ---
ARG TARGETARCH
RUN if [ "$TARGETARCH" = "arm64" ]; then ARCH_PATH="aarch64-unknown-linux-gnu/"; else ARCH_PATH=""; fi && \
    cp /build/rust/target/${ARCH_PATH}release/<name> /build/<name>

# --- runtime stage ---
FROM debian:bookworm-slim
COPY --from=rust-builder /build/<name> /usr/local/bin/<name>
```

The `RUN cp` runs inside the builder stage (which *does* have `/build/rust/target/...`),
normalizing the per-arch path to `/build/<name>`. The runtime `COPY --from=rust-builder` then
crosses the stage boundary with a static source path and no shell expansion. The native amd64
build (plain `docker build`, no `TARGETARCH`) takes the empty-`ARCH_PATH` branch.

### `.cargo/config.toml` addition

```toml
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
```

This is useful for **local cross-compilation** (developer machine or `local_test_env/arm64`),
where the repo root `.cargo/config.toml` is present on disk and Cargo picks it up automatically.

**Inside Docker containers the file has no effect.** All production Dockerfiles copy only
`rust/` into the image (`COPY rust/ ./rust/`); the repo-root `.cargo/` directory is never
added to the build context. The linker is therefore configured via the inline
`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc` env var in the `RUN`
command shown above — that is the authoritative mechanism for Docker builds.

If `.cargo/config.toml` is ever needed inside Docker (e.g. for additional target settings),
add `COPY .cargo/ /build/.cargo/` to each Dockerfile before the `cargo build` step.

### Monolith / all-in-one / analytics-web — WASM and Node frontend stages

The Rust builder stage follows the same pattern above; the binary copy crosses into the
runtime stage via `COPY --from=rust-builder` from the normalized `/build/<name>` path as above.

Both pre-Rust stages must be pinned to the build host to keep the no-QEMU goal. Today both are
unpinned in `docker/monolith.Dockerfile`, `docker/all-in-one.Dockerfile`, and
`docker/analytics-web.Dockerfile`. For the frontend stage this design changes:

- `FROM node:20-alpine AS frontend-builder` → `FROM --platform=$BUILDPLATFORM node:20-alpine AS frontend-builder`

The Node pin is the no-QEMU mechanism for the frontend stage: `yarn install`/`yarn build` run
natively on the amd64 host and emit arch-neutral static assets consumed via
`COPY --from=frontend-builder`. Without the pin, `docker buildx build --platform linux/arm64`
would resolve the unpinned stage to arm64 and run yarn under QEMU (correct output, but slow).

For the wasm stage, this design replaces the cross-invocation
`FROM ${WASM_IMAGE} AS wasm-builder` reference with an **inlined**
`FROM --platform=$BUILDPLATFORM rust:1-bookworm AS wasm-builder` stage carrying the full body of
`docker/wasm-builder.Dockerfile` (see the Fix paragraph below). This avoids the cross-build
image-resolution problem on the arm64 path (the `docker-container` driver cannot read the daemon
image store as a `FROM` base — see Fix). The stage is `--platform=$BUILDPLATFORM`-pinned so the
wasm build runs natively on the amd64 host (no QEMU), and its output is arch-neutral
(`wasm32-unknown-unknown`), consumed only via `COPY --from=wasm-builder`. `build_docker_images.py`
defines `WASM_SERVICES = {"analytics-web", "all", "monolith"}` — all three services produce the
wasm artifacts via this inlined stage.

The `--platform=$BUILDPLATFORM` pin fixes **arch resolution** (it stops BuildKit from
looking up a non-existent arm64 variant of the tag), but on the arm64 path it does **not** make
a prebuilt wasm-builder image reachable from the buildx builder. The arm64 path runs under a
`docker buildx` builder using the `docker-container` driver (see Prerequisites), which runs
BuildKit in an isolated container that does **not** share the host Docker daemon's image store.
A `micromegas-wasm-builder:latest` image built ahead of time (whether via plain `docker build`
or `docker buildx build --load`) lives only in the daemon store; with the `docker-container`
driver, `FROM ${WASM_IMAGE}` is resolved from a registry, not the daemon store, so a later
separate service build still fails with manifest-not-found. `--load` cannot fix this — it
exports to the daemon store, which is exactly the store this driver cannot read as a `FROM`
base.

Fix (chosen approach — inline the wasm stage): instead of referencing a prebuilt image across
build invocations, the three WASM-service Dockerfiles **inline the wasm-builder body as their
own first stage** (a single multi-stage build). There is no cross-invocation `FROM ${WASM_IMAGE}`
left to resolve — the wasm artifacts are produced in-build and consumed via
`COPY --from=wasm-builder` exactly as today. This removes the cross-build image-resolution
problem entirely.

```dockerfile
# Stage 1: WASM query engine (inlined from wasm-builder.Dockerfile)
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS wasm-builder
# ... full body of docker/wasm-builder.Dockerfile (binaryen, wasm-bindgen-cli,
#     cargo build --target wasm32-unknown-unknown, wasm-opt, pkg/package.json) ...
```

The stage is `--platform=$BUILDPLATFORM`-pinned, so the wasm build still runs natively on the
amd64 host (no QEMU) under `docker buildx build --platform linux/arm64`, and the WASM output is
arch-neutral (`wasm32-unknown-unknown`). The tradeoff: the wasm build now runs as part of each
service build rather than being prebuilt once. To avoid a new source of drift, the inlined
stage should be kept in sync with `docker/wasm-builder.Dockerfile`.

Because the wasm stage is inlined, `ensure_wasm_builder()` is **no longer needed for the arm64
path** — there is no shared image to prebuild. The native amd64 path may keep referencing the
prebuilt `${WASM_IMAGE}` (where the daemon store is the build's store and the existing
`ensure_wasm_builder()` + plain `docker build` work fine), or it may use the same inlined stage;
either way the arm64 path does not call `ensure_wasm_builder()`.

Note: multi-arch manifest creation and pushing are out of scope for the build script — the
script only needs to produce a single-arch image matching the requested target platform.

### build_docker_images.py

`build_image(service, version, push=False)` currently builds with two `-t` tags
(`<name>:{version}` and `<name>:latest`) via plain `docker build`. Add an `arm64: bool`
parameter so the signature becomes `build_image(service, version, push=False, arm64=False)`.
`main()` parses an `--arm64` flag and passes it through to `build_image()`.

When `arm64` is set, the script:

1. Uses `docker buildx build --platform linux/arm64` instead of `docker build`.
2. Tags the image with both `<name>:{version}-arm64` and `<name>:latest-arm64` (the same
   two-`-t` pattern as the native path, with the `-arm64` suffix added to each tag).
3. Passes `--load` so the tagged single-arch image is written into the local docker image
   store (the `docker-container` buildx driver otherwise only populates the build cache,
   leaving nothing for `docker run` to find). `--load` and `--push` are mutually exclusive in
   buildx; this path is `--load`-only.

`main()` guards against the contradictory `--arm64 --push` combination: because the arm64
path is `--load`-only and `--load`/`--push` cannot coexist, `main()` errors out early with a
clear message (e.g. `--push is not supported with --arm64; multi-arch publishing is out of
scope`) rather than assembling an invalid buildx command. (Multi-arch fat-manifest publishing
is the deferred future work; see Trade-offs.)

`build_image()` returns the actual tags it applied so the run summary is accurate. The result
dict records the `-arm64`-suffixed tags when `arm64=True` (rather than the hardcoded bare
`:{version}` / `:latest`), and `main()`'s summary prints those recorded tags — otherwise the
summary would advertise non-suffixed tags that were never created on the arm64 path.

The assembled command is a single invocation, e.g.:

```
docker buildx build --platform linux/arm64 --load \
  -t <name>:{version}-arm64 -t <name>:latest-arm64 \
  -f docker/<service>.Dockerfile .
```

Multi-arch manifest publishing (a `--platform linux/amd64,linux/arm64 --push` fat manifest)
is out of scope; see Trade-offs for deferred future work.

Key invariant: the WASM artifacts are arch-neutral, and on the arm64 path the wasm-builder is
**inlined as a `--platform=$BUILDPLATFORM`-pinned first stage** in each WASM-service Dockerfile
(see Design). There is no cross-invocation `FROM ${WASM_IMAGE}` to resolve, so the
`docker-container` driver's isolated image store is a non-issue: the wasm artifacts are produced
in-build and consumed via `COPY --from=wasm-builder`. Consequently `ensure_wasm_builder()` is
**not called on the arm64 path** — there is no shared image to prebuild, no arch-suffixed tag,
arch-keyed memo, or `--build-arg WASM_IMAGE=...`. The native amd64 path is unchanged (it may
keep using the prebuilt `${WASM_IMAGE}` via the existing `ensure_wasm_builder()`). See the
Design section.

## Implementation Steps

### Phase 1 — `.cargo/config.toml` and simple service Dockerfiles

1. **`.cargo/config.toml`** — add `[target.aarch64-unknown-linux-gnu]` linker entry (for local cross-compilation; Docker builds rely on the inline env var instead — see design section).
2. **`docker/ingestion.Dockerfile`** — apply the cross-compile pattern to the builder stage.
3. **`docker/flight-sql.Dockerfile`** — same.
4. **`docker/http-gateway.Dockerfile`** — same.
5. **`docker/admin.Dockerfile`** — same.
6. **`docker/analytics-web.Dockerfile`** — apply the cross-compile pattern to the
   `rust-builder` stage (compiles `analytics-web-srv`), inline the wasm-builder body as a
   `--platform=$BUILDPLATFORM`-pinned `wasm-builder` stage, and add the
   `--platform=$BUILDPLATFORM` pin to the `frontend-builder` stage `FROM` line (see Design).

### Phase 2 — Complex Dockerfiles

7. **`docker/monolith.Dockerfile`** — apply the cross-compile pattern to the `rust-builder`
   stage, inline the wasm-builder body as a `--platform=$BUILDPLATFORM`-pinned `wasm-builder`
   stage, and add the `--platform=$BUILDPLATFORM` pin to the `frontend-builder` stage `FROM`
   line (see Design).
8. **`docker/all-in-one.Dockerfile`** — same cross-compile pattern and pins as monolith, but the
   `cargo build` command has five `--bin` flags and the runtime stage keeps its five
   `COPY --from=rust-builder` lines (retargeted to the normalized `/build/<name>` paths). The
   conditional `cargo build` snippet:

   ```dockerfile
   RUN if [ "$TARGETARCH" = "arm64" ]; then \
         CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
         cargo build --release --target aarch64-unknown-linux-gnu \
           --bin telemetry-ingestion-srv \
           --bin flight-sql-srv \
           --bin telemetry-admin \
           --bin http-gateway-srv \
           --bin analytics-web-srv; \
       else \
         cargo build --release \
           --bin telemetry-ingestion-srv \
           --bin flight-sql-srv \
           --bin telemetry-admin \
           --bin http-gateway-srv \
           --bin analytics-web-srv; \
       fi
   ```

   To normalize the per-arch path, the `rust-builder` stage ends with a single `RUN cp` block
   (re-declaring `ARG TARGETARCH`) that stages all five binaries at static `/build/` paths;
   the runtime stage then uses five plain `COPY --from=rust-builder` lines with no variable
   expansion (so no `RUN cp` is needed in the runtime stage):

   ```dockerfile
   # --- end of rust-builder stage ---
   ARG TARGETARCH
   RUN if [ "$TARGETARCH" = "arm64" ]; then ARCH_PATH="aarch64-unknown-linux-gnu/"; else ARCH_PATH=""; fi && \
       cp /build/rust/target/${ARCH_PATH}release/telemetry-ingestion-srv /build/telemetry-ingestion-srv && \
       cp /build/rust/target/${ARCH_PATH}release/flight-sql-srv /build/flight-sql-srv && \
       cp /build/rust/target/${ARCH_PATH}release/telemetry-admin /build/telemetry-admin && \
       cp /build/rust/target/${ARCH_PATH}release/http-gateway-srv /build/http-gateway-srv && \
       cp /build/rust/target/${ARCH_PATH}release/analytics-web-srv /build/analytics-web-srv

   # --- runtime stage ---
   FROM debian:bookworm-slim
   COPY --from=rust-builder /build/telemetry-ingestion-srv /usr/local/bin/telemetry-ingestion-srv
   COPY --from=rust-builder /build/flight-sql-srv /usr/local/bin/flight-sql-srv
   COPY --from=rust-builder /build/telemetry-admin /usr/local/bin/telemetry-admin
   COPY --from=rust-builder /build/http-gateway-srv /usr/local/bin/http-gateway-srv
   COPY --from=rust-builder /build/analytics-web-srv /usr/local/bin/analytics-web-srv
   ```

### Phase 3 — Build script

9. **`build/build_docker_images.py`** — add an `--arm64` flag parsed in `main()` and threaded
   into `build_image(service, version, push=False, arm64=False)`. When `arm64` is set, switch
   from `docker build` to `docker buildx build --platform linux/arm64 --load`, tagging the
   image with both `<name>:{version}-arm64` and `<name>:latest-arm64` (two `-t` flags on the
   single buildx invocation, mirroring the native two-tag pattern). `--load` writes the
   single-arch image into the local docker store for the smoke test. Multi-arch manifest
   publishing is out of scope (see Trade-offs).
   - Skip the WASM prebuild on the arm64 path: guard the `if service in WASM_SERVICES`
     `ensure_wasm_builder()` call with `and not arm64`, since the wasm-builder is inlined as a
     build stage for arm64 (see Design) and prebuilding into the daemon store would be unused.

### Prerequisites (one-time, per build host)

Before running the `--arm64` build or the runtime smoke test on an x86-64 host:

- **buildx builder**: `docker buildx build --platform ...` requires a `docker-container`
  builder; the default `docker` driver does not support `--platform`. Create and select one
  once with `docker buildx create --use` (idempotent — reuse the existing builder on later runs).
- **QEMU/binfmt** (only for the runtime smoke test, not the cross-compiled build): register the
  arm64 emulator so the x86 host can *run* the arm64 image, via
  `docker run --privileged --rm tonistiigi/binfmt --install all`. This is a one-time host setup;
  Docker Desktop registers binfmt automatically.

### Phase 4 — Smoke test

10. On an x86-64 machine (after the Prerequisites above), run:
   ```
   python build/build_docker_images.py --arm64 ingestion
   docker run --rm --platform linux/arm64 \
     marcantoinedesroches/micromegas-ingestion:latest-arm64 --help
   ```
   The `--platform` flag makes Docker use QEMU at **run** time only (for the smoke test);
   the binary itself was cross-compiled without QEMU.

## Files to Modify

| File | Change |
|---|---|
| `.cargo/config.toml` | Add `[target.aarch64-unknown-linux-gnu]` linker entry |
| `docker/ingestion.Dockerfile` | Cross-compile pattern in builder stage |
| `docker/flight-sql.Dockerfile` | Same |
| `docker/http-gateway.Dockerfile` | Same |
| `docker/admin.Dockerfile` | Same |
| `docker/analytics-web.Dockerfile` | Cross-compile pattern in `rust-builder` stage; inline wasm-builder as a `--platform=$BUILDPLATFORM`-pinned `wasm-builder` stage; add `--platform=$BUILDPLATFORM` pin to `frontend-builder` stage |
| `docker/monolith.Dockerfile` | Cross-compile pattern in `rust-builder` stage; inline wasm-builder as a `--platform=$BUILDPLATFORM`-pinned `wasm-builder` stage; add `--platform=$BUILDPLATFORM` pin to `frontend-builder` stage |
| `docker/all-in-one.Dockerfile` | Same |
| `build/build_docker_images.py` | `--arm64` flag, `docker buildx` integration |

The three WASM-service Dockerfiles inline the wasm-builder body as a
`--platform=$BUILDPLATFORM`-pinned first stage (see Design), and add a `--platform=$BUILDPLATFORM`
pin on their `frontend-builder` `FROM` line (currently unpinned). With the wasm stage inlined,
there is no cross-invocation `FROM ${WASM_IMAGE}` to resolve, so `ensure_wasm_builder()` is
**not called on the arm64 path** (no shared image to prebuild). The inlined stage should be kept
in sync with `docker/wasm-builder.Dockerfile`. The native amd64 path is unchanged
(`ensure_wasm_builder()` + the prebuilt `${WASM_IMAGE}`).

## Trade-offs

**Cross-compilation vs QEMU emulation**

QEMU emulation (approach A) requires zero Dockerfile changes — just add `--platform linux/arm64`
to `docker build`. It works but Rust compilation under QEMU is 5–10× slower (~60 min for a
full build vs ~8 min native). Chosen approach: cross-compilation (approach B), because
`local_test_env/arm64/Dockerfile` already demonstrated it works and the build time stays
acceptable for a daily-use workflow.

**Single-arch vs fat manifest**

For local development, building only `linux/arm64` is sufficient, and that is all the build
script does (`--arm64` → single-arch `--load`). Multi-arch fat manifests (a single
`--platform linux/amd64,linux/arm64 --push` pass to DockerHub) would benefit CI and DockerHub
consumers, but are explicitly **out of scope / deferred future work** — the Dockerfiles
themselves are the primary deliverable.

## Testing Strategy

1. Build `ingestion` with `--arm64` on an x86-64 Linux machine and confirm the image
   runs with `docker run --platform linux/arm64 ... --help`.
2. Build `monolith` with `--arm64` and verify the web UI is reachable (the Node/WASM stages
   are BUILDPLATFORM-pinned, so this mainly validates binary copying and arch-neutral asset
   reuse).
3. Confirm the existing x86 build still works after Dockerfile changes (regression test).
4. On an actual ARM64 machine (or CI runner), run `docker build` without `--arm64` and
   confirm it falls back to the native path.

## Decisions

- **CI**: No ARM CI runner for now. Cross-compilation on x86 is the only supported path.
- **`all-in-one`**: Included — its Rust builder stage is structurally identical to
  `monolith.Dockerfile` (same cross-compile pattern, five binaries instead of one).
