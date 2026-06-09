# Rete dev container: everything runs in here, never on the host.
FROM rust:1.92-bookworm

ARG USERNAME=vscode
ARG USER_UID=1000
ARG USER_GID=$USER_UID

# Keep Rust tooling visible even in login shells started by editors or humans.
ENV PATH="/usr/local/cargo/bin:${PATH}"

# Base tooling + things our crates need (zstd headers, clang for bindgen, etc.)
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        git curl ca-certificates pkg-config \
        build-essential clang lld \
        libzstd-dev \
        python3 python3-pip python3-venv \
    && rm -rf /var/lib/apt/lists/*

RUN printf '%s\n' 'export PATH="/usr/local/cargo/bin:$PATH"' > /etc/profile.d/rust-cargo-path.sh

# uv is the repository's Python script runner; it is only used at generation
# time and is not a runtime dependency of the static playground.
RUN pip3 install --no-cache-dir --break-system-packages uv

# Non-root dev user
RUN groupadd --gid $USER_GID $USERNAME \
    && useradd --uid $USER_UID --gid $USER_GID -m $USERNAME

# Rust components: WASM target + the usual dev helpers.
RUN rustup component add clippy rustfmt \
    && rustup target add wasm32-unknown-unknown \
    && cargo fmt --version \
    && cargo clippy --version

# wasm-pack for the browser client build (cargo-installed so it matches the toolchain).
RUN cargo install wasm-pack --locked

# The image builds tools as root, but development runs as vscode. Cargo needs to
# update registry/git caches and rustup metadata during normal checks.
RUN chown -R $USERNAME:$USERNAME /usr/local/cargo /usr/local/rustup

USER $USERNAME
ENV CARGO_TERM_COLOR=always
