FROM ubuntu:22.04

ARG RUNNER_VERSION=2.332.0

ENV DEBIAN_FRONTEND=noninteractive

# System dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    curl \
    git \
    jq \
    libicu-dev \
    libssl-dev \
    mold \
    pkg-config \
    python3 \
    python3-pip \
    sudo \
    unzip \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Node.js 20 + Yarn
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && npm install -g yarn \
    && rm -rf /var/lib/apt/lists/*

# Go 1.21
RUN GOARCH=$([ "$(uname -m)" = "x86_64" ] && echo "amd64" || echo "arm64") \
    && curl -fsSL "https://go.dev/dl/go1.21.13.linux-${GOARCH}.tar.gz" | tar -C /usr/local -xz
ENV PATH="/usr/local/go/bin:${PATH}"

# Poetry
RUN pip3 install --no-cache-dir poetry

# Playwright system dependencies (for Grafana E2E tests)
RUN npx playwright install-deps chromium

# Create runner user with passwordless sudo
RUN useradd -m -s /bin/bash runner \
    && echo "runner ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

# Install GitHub Actions runner
USER runner
WORKDIR /home/runner
RUN ARCH=$([ "$(uname -m)" = "x86_64" ] && echo "x64" || echo "arm64") \
    && curl -fsSL "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-${ARCH}-${RUNNER_VERSION}.tar.gz" | tar -xz

# Install runner system dependencies
USER root
RUN /home/runner/bin/installdependencies.sh && rm -rf /var/lib/apt/lists/*

# Install Rust for runner user (default CARGO_HOME=~/.cargo during build)
USER runner
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/home/runner/.cargo/bin:${PATH}"

# Rust WASM target and cargo tools
RUN rustup target add wasm32-unknown-unknown \
    && cargo install cargo-machete \
    && cargo install wasm-pack

# wasm-bindgen-cli — version must match workspace Cargo.lock
COPY --chown=runner:runner rust/datafusion-wasm/Cargo.lock /tmp/wasm-cargo.lock
RUN WASM_BINDGEN_VERSION=$(python3 -c "import re; t=open('/tmp/wasm-cargo.lock').read(); m=re.search(r'\[\[package\]\]\s*name\s*=\s*\"wasm-bindgen\"\s*version\s*=\s*\"([^\"]+)\"', t); print(m.group(1))") \
    && cargo install wasm-bindgen-cli --version "${WASM_BINDGEN_VERSION}" \
    && rm /tmp/wasm-cargo.lock

# Mage (for Grafana plugin builds)
RUN go install github.com/magefile/mage@latest

# Set up cache directory (volume mount point)
USER root
RUN mkdir -p /cache/cargo-home /cache/target-native /cache/target-wasm \
    && chown -R runner:runner /cache
USER runner

# Runtime env — cargo registry/git cached on volume, image-installed tools on PATH
ENV CARGO_HOME=/cache/cargo-home
ENV CARGO_TARGET_DIR=/cache/target-native
ENV PATH="/home/runner/.cargo/bin:/cache/cargo-home/bin:/home/runner/go/bin:/usr/local/go/bin:${PATH}"

# Entrypoint
COPY --chown=runner:runner docker/github-runner-entrypoint.sh /home/runner/entrypoint.sh
RUN chmod +x /home/runner/entrypoint.sh

ENTRYPOINT ["/home/runner/entrypoint.sh"]
