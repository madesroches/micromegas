# syntax=docker/dockerfile:1.7
FROM ubuntu:22.04

ARG RUNNER_VERSION=2.332.0
# Default to Azure mirror — archive.ubuntu.com has been intermittent.
# Override with --build-arg UBUNTU_MIRROR=<host> to pick a different mirror.
ARG UBUNTU_MIRROR=azure.archive.ubuntu.com

ENV DEBIAN_FRONTEND=noninteractive

# Keep .deb files and apt lists so the BuildKit cache mounts below
# survive between builds (the default ubuntu image's docker-clean
# hook deletes them after every apt-get run).
RUN rm -f /etc/apt/apt.conf.d/docker-clean \
    && echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache

# Redirect archive + security to UBUNTU_MIRROR (Azure carries both pockets).
RUN sed -i \
    -e "s|http://archive\.ubuntu\.com/ubuntu|http://${UBUNTU_MIRROR}/ubuntu|g" \
    -e "s|http://security\.ubuntu\.com/ubuntu|http://${UBUNTU_MIRROR}/ubuntu|g" \
    /etc/apt/sources.list

# System dependencies
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    clang \
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
    wget

# Node.js 20 + Yarn 4 (via corepack). Shared COREPACK_HOME so the runner user
# reads the same prepared binary that root sets up — without this, switching
# USER would force corepack to re-fetch yarn from the network on every container.
ENV COREPACK_HOME=/opt/corepack
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && corepack enable \
    && corepack prepare yarn@4.14.1 --activate \
    && chmod -R a+rX /opt/corepack

# Go 1.21
RUN GOARCH=$([ "$(uname -m)" = "x86_64" ] && echo "amd64" || echo "arm64") \
    && curl -fsSL "https://go.dev/dl/go1.21.13.linux-${GOARCH}.tar.gz" | tar -C /usr/local -xz
ENV PATH="/usr/local/go/bin:${PATH}"

# Poetry
RUN pip3 install --no-cache-dir poetry

# Playwright system dependencies (for Grafana E2E tests)
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    npx playwright install-deps chromium

# Firefox from Mozilla's official apt repo (the apt "firefox" on 22.04 is a snap
# shim, and PPAs have no mirror network so ppa:mozillateam is a single point of
# failure). Mozilla's repo is CDN-hosted and independent of Canonical.
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    install -d -m 0755 /etc/apt/keyrings \
    && wget -qO /etc/apt/keyrings/packages.mozilla.org.asc https://packages.mozilla.org/apt/repo-signing-key.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/packages.mozilla.org.asc] https://packages.mozilla.org/apt mozilla main" > /etc/apt/sources.list.d/mozilla.list \
    && printf 'Package: *\nPin: origin packages.mozilla.org\nPin-Priority: 1000\n' > /etc/apt/preferences.d/mozilla \
    && apt-get update && apt-get install -y --no-install-recommends firefox \
    && GECKO_VERSION=$(curl -fsSL https://api.github.com/repos/mozilla/geckodriver/releases/latest | python3 -c "import sys,json; print(json.load(sys.stdin)['tag_name'])") \
    && curl -fsSL "https://github.com/mozilla/geckodriver/releases/download/${GECKO_VERSION}/geckodriver-${GECKO_VERSION}-linux64.tar.gz" | tar -xz -C /usr/local/bin

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
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    /home/runner/bin/installdependencies.sh

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

# Set up cache directory — populated lazily; dev_worker mounts a named volume
# at /cache so the rust subtree persists across ephemeral containers.
USER root
RUN mkdir -p /cache/rust/cargo-home /cache/rust/target-native /cache/rust/target-wasm \
    && chown -R runner:runner /cache
USER runner

# Runtime env — cargo registry/git under /cache/rust, image-installed tools on PATH
ENV CARGO_HOME=/cache/rust/cargo-home
ENV CARGO_TARGET_DIR=/cache/rust/target-native
ENV PATH="/home/runner/.cargo/bin:/cache/rust/cargo-home/bin:/home/runner/go/bin:/usr/local/go/bin:${PATH}"

# Entrypoint
COPY --chown=runner:runner docker/github-runner-entrypoint.sh /home/runner/entrypoint.sh
RUN chmod +x /home/runner/entrypoint.sh

ENTRYPOINT ["/home/runner/entrypoint.sh"]
