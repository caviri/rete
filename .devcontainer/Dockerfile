# Rete dev container: everything runs in here, never on the host.

# The independent N-Quads referee, as a binary rather than a container.
#
# `scripts/export_scholar_nquads.sh` refuses to record a dump `done` until
# something that did NOT produce it has parsed the dump. Until now the only way
# to run that parser was `docker run oxigraph/oxigraph`, which a test running
# INSIDE this container cannot do — so the step was verified by hand and by
# nothing else. The binary is lifted out of the pinned upstream image rather
# than rebuilt from source, so the parser in here and the parser the driver
# reaches for on a developer's host are the same bytes and cannot disagree.
#
# Pinned, never `:latest`: this parser's verdict is a publication gate, and a
# gate must not move underneath a release.
ARG OXIGRAPH_VERSION=0.5.11
FROM oxigraph/oxigraph:${OXIGRAPH_VERSION} AS oxigraph

FROM rust:1.92-bookworm

ARG USERNAME=vscode
ARG USER_UID=1000
ARG USER_GID=$USER_UID
ARG WASM_PACK_VERSION=0.15.0
ARG CARGO_LLVM_COV_VERSION=0.8.7
ARG CARGO_AUDIT_VERSION=0.22.2
ARG CARGO_DENY_VERSION=0.20.2
ARG CARGO_SEMVER_CHECKS_VERSION=0.48.0
ARG CARGO_PUBLIC_API_VERSION=0.52.0
ARG CARGO_FUZZ_VERSION=0.13.2
ARG CARGO_CYCLONEDX_VERSION=0.5.9
ARG ASYNCIFY_NIGHTLY=nightly-2026-07-01

# Keep Rust tooling visible even in login shells started by editors or humans.
ENV PATH="/usr/local/cargo/bin:${PATH}"

# Base tooling + things our crates need (zstd headers, clang for bindgen, etc.)
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        git curl ca-certificates pkg-config \
        build-essential clang lld \
        libzstd-dev \
        python3 python3-pip python3-venv \
        nodejs npm binaryen \
    && rm -rf /var/lib/apt/lists/*

RUN printf '%s\n' 'export PATH="/usr/local/cargo/bin:$PATH"' > /etc/profile.d/rust-cargo-path.sh

# uv is the repository's Python script runner; it is only used at generation
# time and is not a runtime dependency of the static playground.
RUN pip3 install --no-cache-dir --break-system-packages uv

# Non-root dev user
RUN groupadd --gid $USER_GID $USERNAME \
    && useradd --uid $USER_UID --gid $USER_GID -m $USERNAME

# Rust components: WASM target + the usual dev helpers. Asyncify needs a pinned
# nightly with rust-src so scripts never install a floating toolchain at runtime.
RUN rustup component add clippy rustfmt \
    && rustup target add wasm32-unknown-unknown \
    && rustup toolchain install ${ASYNCIFY_NIGHTLY} --profile minimal --component rust-src \
    && rustup target add --toolchain ${ASYNCIFY_NIGHTLY} wasm32-unknown-unknown \
    && cargo fmt --version \
    && cargo clippy --version

# Release tools are pinned in the image so local Compose and CI run the same
# binaries. None of these tools is installed dynamically by a repository script.
RUN cargo install wasm-pack --version ${WASM_PACK_VERSION} --locked \
    && cargo install cargo-llvm-cov --version ${CARGO_LLVM_COV_VERSION} --locked \
    && cargo install cargo-audit --version ${CARGO_AUDIT_VERSION} --locked \
    && cargo install cargo-deny --version ${CARGO_DENY_VERSION} --locked \
    && cargo install cargo-semver-checks --version ${CARGO_SEMVER_CHECKS_VERSION} --locked \
    && cargo install cargo-public-api --version ${CARGO_PUBLIC_API_VERSION} --locked \
    && cargo install cargo-fuzz --version ${CARGO_FUZZ_VERSION} --locked \
    && cargo install cargo-cyclonedx --version ${CARGO_CYCLONEDX_VERSION} --locked

# `rust-toolchain.toml` also requires these stable components. Install them in
# the image so the first checkout never needs to mutate rustup over the network.
RUN rustup component add llvm-tools-preview rust-src

# The image builds tools as root, but development runs as vscode. Cargo needs to
# update registry/git caches and rustup metadata during normal checks.
RUN chown -R $USERNAME:$USERNAME /usr/local/cargo /usr/local/rustup

USER $USERNAME
ENV CARGO_TERM_COLOR=always

# Browser-target wasm-bindgen tests run in the same pinned release image. Keep
# Chromium in its own layer so adding/updating browser test support does not
# invalidate the expensive Rust tooling layers above.
USER root
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends chromium \
    && rm -rf /var/lib/apt/lists/*

# The scholar export driver's own toolchain. Last layers, for the same reason
# Chromium is: changing them must not invalidate the expensive Rust tooling.
#   pigz  the driver compresses the dump with it and decompresses to prove the
#         gzip stream is complete
#   time  `/usr/bin/time -v` is where the per-export peak RSS comes from; the
#         driver falls back to the cgroup peak when it is missing, which is a
#         coarser number and never exercised the parser of the -v report
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends pigz time \
    && rm -rf /var/lib/apt/lists/*

# ~22 MB, one binary: `oxigraph convert` with the output discarded is a parser,
# not a store and not a server, so nothing else from that image is wanted.
# tests/scholar/parse_check.sh is what this exists for.
COPY --from=oxigraph /usr/local/bin/oxigraph /usr/local/bin/oxigraph

USER $USERNAME
