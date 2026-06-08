# Rete dev container — everything runs in here, never on the host.
FROM rust:1.92-bookworm

ARG USERNAME=vscode
ARG USER_UID=1000
ARG USER_GID=$USER_UID

# Base tooling + things our crates need (zstd headers, clang for bindgen, etc.)
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        git curl ca-certificates pkg-config \
        build-essential clang lld \
        libzstd-dev \
    && rm -rf /var/lib/apt/lists/*

# Non-root dev user
RUN groupadd --gid $USER_GID $USERNAME \
    && useradd --uid $USER_UID --gid $USER_GID -m $USERNAME

# Rust components: WASM target + the usual dev helpers.
RUN rustup component add clippy rustfmt \
    && rustup target add wasm32-unknown-unknown

# wasm-pack for the browser client build (cargo-installed so it matches the toolchain).
RUN cargo install wasm-pack --locked

USER $USERNAME
ENV CARGO_TERM_COLOR=always
