# ARM64 Cross-Compilation in Docker Plan

## Overview

Add `linux/arm64` support to all production Dockerfiles so that an x86-64 developer can build
ARM64 images without QEMU emulation. The build relies on a Debian cross-compile toolchain
(`g++-aarch64-linux-gnu`) — a pattern already
proven in `local_test_env/arm64/Dockerfile`. The build script gains an `--arm64` flag that
drives `docker buildx` for multi-arch manifest publishing.

## Current State

### Production Dockerfiles (all x86-native today)

All five service Dockerfiles follow the same two-stage pattern:

```
FROM rust:1-bookworm AS builder
...
RUN cargo build --release --bin <name>          # native only

FROM debian:bookworm-slim
COPY --from=builder /build/rust/target/release/<name> /usr/local/bin/
```

Files: `docker/ingestion.Dockerfile`, `docker/flight-sql.Dockerfile`,
`docker/http-gateway.Dockerfile`, `docker/admin.Dockerfile`.

The monolith adds WASM and Node stages before the Rust stage:
`docker/monolith.Dockerfile` (4 stages), `docker/all-in-one.Dockerfile` (4 stages).

`wasm-builder.Dockerfile` uses `dpkg --print-architecture` to select the Binaryen binary
(line 17-24). This returns the **host** architecture, not the build target — on an x86-64
host doing `docker buildx build --platform linux/arm64`, it returns `amd64` and downloads
the wrong binary. The file must be updated to use `$TARGETARCH` instead (mapping `arm64` to
`aarch64` as needed for the binaryen archive name).

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

The Rust builder stage in each Dockerfile gains an `ARG TARGETARCH` that Docker BuildKit
sets automatically. When `TARGETARCH=arm64` the stage installs the cross toolchain and builds
for `aarch64-unknown-linux-gnu`; otherwise it builds natively.

```dockerfile
FROM rust:1-bookworm AS rust-builder

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

The runtime stage must re-declare `ARG TARGETARCH` (Docker does not carry an `ARG` across a
`FROM` boundary), then reference the correct path. Because Docker `COPY` doesn't support
shell variable expansion, a `RUN cp` is used instead:

```dockerfile
FROM debian:bookworm-slim
ARG TARGETARCH
RUN if [ "$TARGETARCH" = "arm64" ]; then ARCH_PATH="aarch64-unknown-linux-gnu/"; else ARCH_PATH=""; fi && \
    cp /build/rust/target/${ARCH_PATH}release/<name> /usr/local/bin/<name>
```

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

### Monolith / all-in-one / analytics-web — Node frontend stage

`node:20-alpine` supports arm64 natively. The Rust builder stage follows the same pattern
above; the binary copy in the runtime stage must use `RUN cp` as above.

`build_docker_images.py` defines `WASM_SERVICES = {"analytics-web", "all", "monolith"}` —
all three services depend on the wasm-builder image via a `FROM ${WASM_IMAGE} AS wasm-builder`
stage. Therefore, when `--arm64` is active, **all three** require a multiarch wasm-builder
image, not just monolith/all-in-one.

`wasm-builder` is already arch-aware at runtime, but `ensure_wasm_builder()` in
`build_docker_images.py` calls plain `docker build` with no `--platform`, producing only an
`amd64` local image. When BuildKit processes a `--platform linux/arm64` build it attempts to
resolve every `FROM` stage to that platform; it will fail (or fall back to QEMU) because no
`linux/arm64` manifest exists for the wasm-builder image. When `--arm64` is active,
`ensure_wasm_builder()` must therefore be updated to call
`docker buildx build --platform linux/arm64 --load` so BuildKit can resolve the stage for
that platform. The WASM artifacts themselves are arch-neutral and work unchanged.

Note: multi-arch manifest creation and pushing are out of scope for the build script — the
script only needs to produce a single-arch image matching the requested target platform.

### build_docker_images.py

Add `--arm64` flag. When set, the script:

1. Uses `docker buildx build --platform linux/arm64` instead of `docker build`.
2. Tags images with `<name>:latest-arm64` (and version tag) locally, or pushes a multi-arch
   manifest if `--push` is also passed.
3. For the `--push` + `--arm64` path, builds both `linux/amd64` and `linux/arm64` and uses
   `docker buildx build --platform linux/amd64,linux/arm64 --push` in a single pass so
   DockerHub gets a fat manifest automatically.

Key invariant: the WASM artifacts are arch-neutral, but the wasm-builder image must have a
`linux/arm64` manifest available so BuildKit can resolve the `FROM ${WASM_IMAGE}` stage when
targeting arm64. `ensure_wasm_builder()` must use
`docker buildx build --platform linux/arm64 --load` when `--arm64` is active (single-arch,
matching the pattern for all other services).

## Implementation Steps

### Phase 1 — `.cargo/config.toml` and simple service Dockerfiles

1. **`.cargo/config.toml`** — add `[target.aarch64-unknown-linux-gnu]` linker entry (for local cross-compilation; Docker builds rely on the inline env var instead — see design section).
2. **`docker/ingestion.Dockerfile`** — apply the cross-compile pattern to the builder stage.
3. **`docker/flight-sql.Dockerfile`** — same.
4. **`docker/http-gateway.Dockerfile`** — same.
5. **`docker/admin.Dockerfile`** — same.
6. **`docker/analytics-web.Dockerfile`** — apply the cross-compile pattern to the
   `rust-builder` stage (compiles `analytics-web-srv`); leave the WASM and Node stages
   unchanged.

### Phase 2 — Complex Dockerfiles

7. **`docker/monolith.Dockerfile`** — update only the `rust-builder` stage; leave WASM and
   Node stages unchanged.
8. **`docker/all-in-one.Dockerfile`** — same cross-compile pattern as monolith, but the
   `cargo build` command has five `--bin` flags and the runtime stage has five `COPY` lines
   that each become a `RUN cp` block. The conditional `cargo build` snippet:

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

   And each of the five `COPY --from=rust-builder` lines becomes a `RUN cp` block in the
   runtime stage, which must re-declare `ARG TARGETARCH` after its `FROM` line:

   ```dockerfile
   FROM debian:bookworm-slim
   ARG TARGETARCH
   RUN if [ "$TARGETARCH" = "arm64" ]; then ARCH_PATH="aarch64-unknown-linux-gnu/"; else ARCH_PATH=""; fi && \
       cp /build/rust/target/${ARCH_PATH}release/telemetry-ingestion-srv /usr/local/bin/telemetry-ingestion-srv && \
       cp /build/rust/target/${ARCH_PATH}release/flight-sql-srv /usr/local/bin/flight-sql-srv && \
       cp /build/rust/target/${ARCH_PATH}release/telemetry-admin /usr/local/bin/telemetry-admin && \
       cp /build/rust/target/${ARCH_PATH}release/http-gateway-srv /usr/local/bin/http-gateway-srv && \
       cp /build/rust/target/${ARCH_PATH}release/analytics-web-srv /usr/local/bin/analytics-web-srv
   ```

### Phase 3 — Build script

9. **`build/build_docker_images.py`** — add `--arm64` flag, switch from `docker build` to
   `docker buildx build --platform` when set. Handle single-arch local build vs multi-arch
   push.

### Phase 4 — Smoke test

10. On an x86-64 machine, run:
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
| `docker/analytics-web.Dockerfile` | Cross-compile pattern in `rust-builder` stage |
| `docker/monolith.Dockerfile` | Cross-compile pattern in `rust-builder` stage |
| `docker/all-in-one.Dockerfile` | Same |
| `build/build_docker_images.py` | `--arm64` flag, `docker buildx` integration |
| `docker/wasm-builder.Dockerfile` | Replace `dpkg --print-architecture` with `$TARGETARCH` (mapping `arm64` → `aarch64` for the binaryen archive name) so cross-platform builds fetch the correct binary |

`ensure_wasm_builder()` in the build script must build a single-arch `linux/arm64` image
when `--arm64` is active (see `build_docker_images.py` row above).

## Trade-offs

**Cross-compilation vs QEMU emulation**

QEMU emulation (approach A) requires zero Dockerfile changes — just add `--platform linux/arm64`
to `docker build`. It works but Rust compilation under QEMU is 5–10× slower (~60 min for a
full build vs ~8 min native). Chosen approach: cross-compilation (approach B), because
`local_test_env/arm64/Dockerfile` already demonstrated it works and the build time stays
acceptable for a daily-use workflow.

**Single-arch vs fat manifest**

For local development, building only `linux/arm64` is sufficient. Multi-arch fat manifests
(pushed to DockerHub) are driven by `--push --arm64` in the build script and benefit CI
and DockerHub consumers. This is optional — the Dockerfiles themselves are the primary
deliverable.

## Testing Strategy

1. Build `ingestion` with `--arm64` on an x86-64 Linux machine and confirm the image
   runs with `docker run --platform linux/arm64 ... --help`.
2. Build `monolith` with `--arm64` and verify the web UI is reachable (the Node stage is
   unchanged, so this mainly validates binary copying).
3. Confirm the existing x86 build still works after Dockerfile changes (regression test).
4. On an actual ARM64 machine (or CI runner), run `docker build` without `--arm64` and
   confirm it falls back to the native path.

## Decisions

- **CI**: No ARM CI runner for now. Cross-compilation on x86 is the only supported path.
- **`all-in-one`**: Included — its Rust builder stage is structurally identical to
  `monolith.Dockerfile` (same cross-compile pattern, five binaries instead of one).
