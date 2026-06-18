# Multi-stage build for micromegas-monolith
# (single-process image: ingestion + FlightSQL + maintenance + web)

# Stage 1: WASM query engine (inlined from wasm-builder.Dockerfile)
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS wasm-builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends clang curl ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Install recent binaryen from GitHub (Debian's version is too old for externref)
ARG BINARYEN_VERSION=126
ARG BINARYEN_SHA256_X86_64=e487e0eac1f02a6739816c617270b033e5d3f8ca90439301fd0286460322fd76
ARG BINARYEN_SHA256_AARCH64=4013cbcee8928abca015884e3f89d01804f6e1d9f40a4ea01dcdd0aba3e609f5
RUN ARCH=$(dpkg --print-architecture) && \
    if [ "$ARCH" = "arm64" ]; then BINARYEN_ARCH="aarch64"; EXPECTED_SHA256="$BINARYEN_SHA256_AARCH64"; \
    else BINARYEN_ARCH="x86_64"; EXPECTED_SHA256="$BINARYEN_SHA256_X86_64"; fi && \
    curl -fsSL "https://github.com/WebAssembly/binaryen/releases/download/version_${BINARYEN_VERSION}/binaryen-version_${BINARYEN_VERSION}-${BINARYEN_ARCH}-linux.tar.gz" \
         -o /tmp/binaryen.tar.gz && \
    echo "${EXPECTED_SHA256}  /tmp/binaryen.tar.gz" | sha256sum -c - && \
    tar xzf /tmp/binaryen.tar.gz -C /usr/local --strip-components=1 && \
    rm /tmp/binaryen.tar.gz

RUN rustup target add wasm32-unknown-unknown

WORKDIR /build/rust/datafusion-wasm

# Copy Cargo.lock first to cache wasm-bindgen-cli installation
COPY rust/datafusion-wasm/Cargo.lock ./
RUN WASM_BINDGEN_VERSION=$(grep -A1 'name = "wasm-bindgen"' Cargo.lock | grep version | head -1 | sed 's/.*"\(.*\)".*/\1/') && \
    cargo install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION"

# Copy full Rust source (datafusion-wasm has path deps on workspace crates)
COPY rust/ /build/rust/
RUN cargo build --target wasm32-unknown-unknown --release

# Generate JS bindings and optimize
RUN mkdir -p pkg && \
    wasm-bindgen target/wasm32-unknown-unknown/release/micromegas_datafusion_wasm.wasm \
        --out-dir pkg --target web && \
    wasm-opt pkg/micromegas_datafusion_wasm_bg.wasm -Os --enable-reference-types -o pkg/micromegas_datafusion_wasm_bg.wasm

# Write package.json for the WASM package (keep in sync with WASM_PACKAGE_JSON in build.py)
RUN printf '{\n  "name": "micromegas-datafusion-wasm",\n  "version": "0.1.0",\n  "type": "module",\n  "main": "micromegas_datafusion_wasm.js",\n  "types": "micromegas_datafusion_wasm.d.ts"\n}\n' > pkg/package.json

# Stage 2: Build frontend
FROM --platform=$BUILDPLATFORM node:20-alpine AS frontend-builder

WORKDIR /app
RUN corepack enable
COPY analytics-web-app/package.json analytics-web-app/yarn.lock analytics-web-app/.yarnrc.yml ./
# Local link: dependency must exist for yarn to create the symlink
COPY analytics-web-app/src/lib/datafusion-wasm/ ./src/lib/datafusion-wasm/
RUN yarn install --immutable

COPY analytics-web-app/ ./
COPY --from=wasm-builder /build/rust/datafusion-wasm/pkg/ ./src/lib/datafusion-wasm/
RUN yarn build

# Stage 3: Build Rust backend (monolith binary)
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS rust-builder

ARG TARGETARCH

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
      cargo build --release --target aarch64-unknown-linux-gnu --bin micromegas-monolith; \
    else \
      cargo build --release --bin micromegas-monolith; \
    fi

RUN if [ "$TARGETARCH" = "arm64" ]; then ARCH_PATH="aarch64-unknown-linux-gnu/"; else ARCH_PATH=""; fi && \
    cp /build/rust/target/${ARCH_PATH}release/micromegas-monolith /build/micromegas-monolith

# Stage 4: Runtime
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=rust-builder /build/micromegas-monolith /usr/local/bin/
COPY --from=frontend-builder /app/dist /app/frontend

EXPOSE 9000 50051 3000

# Always-required web-role vars; override via -e for real deployments.
ENV MICROMEGAS_WEB_CORS_ORIGIN=http://localhost:3000 \
    MICROMEGAS_BASE_PATH=/

ENTRYPOINT ["micromegas-monolith"]
CMD ["--roles", "all", \
     "--listen-endpoint-http", "0.0.0.0:9000", \
     "--frontend-dir", "/app/frontend"]
