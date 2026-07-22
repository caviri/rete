# R toolchain for the extendr client: regenerate `R/extendr-wrappers.R` and run
# the testthat suite without a local R install.
#
# This is the recipe from docs/clients-dev.md ("Build and test (Docker, nothing
# on the host)") baked into an image, so it is a cached `docker compose run`
# instead of a copy-pasted one-liner that reinstalls Rust and every R package
# on each invocation.
#
#   docker compose run --rm r        # regenerate wrappers + docs
#   docker compose run --rm r-test   # regenerate, then run testthat
#
# Deliberately NOT folded into .devcontainer/Dockerfile: that image is rebuilt
# by nearly every CI job (ci.yml, release.yml native/wasm/assemble), and the R
# package tree would add ~1 GB and several minutes to all of them for tooling
# only the R client needs. Same reasoning as the Playwright image behind `gate`.
#
# rocker/r2u serves every CRAN package as an apt binary, so `install.packages()`
# resolves to prebuilt .debs — no source compiles, no toolchain surprises.
FROM rocker/r2u:jammy

# Match the workspace's pinned toolchain (rust-toolchain.toml). The extendr
# crate is an ordinary cargo build against rete-core, so a drifting rustc here
# would compile the client differently from everything else in the repo.
# (docs/clients-dev.md's one-liner uses `stable`; pinning is stricter.)
ARG RUST_VERSION=1.92.0
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# security.ubuntu.com is unreachable from some networks (plain HTTP to a single
# host); archive.ubuntu.com mirrors the same jammy-security pocket. This matters
# more than it looks: bspm shells out to `apt-get update` on every
# install.packages(), and apt treats ANY unreachable source as a hard error, so
# one blocked host fails the entire R install with a bare "Execution halted".
RUN sed -i 's|http://security\.ubuntu\.com/ubuntu|http://archive.ubuntu.com/ubuntu|g' \
        /etc/apt/sources.list \
    && sed -i 's|http://security\.ubuntu\.com/ubuntu|http://archive.ubuntu.com/ubuntu|g' \
        /etc/apt/sources.list.d/*.list 2>/dev/null || true

# NOTE: do not clear /var/lib/apt/lists here. On r2u, `install.packages()` is
# routed through bspm to apt, so the R install below needs a populated package
# index; wiping it makes every R package "not available" in a way that only
# surfaces later as a missing-namespace error. Cleanup happens after that step.
RUN apt-get update -qq \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        pkg-config

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain ${RUST_VERSION} --profile minimal \
    && chmod -R a+rwX ${RUSTUP_HOME} ${CARGO_HOME} \
    && rustc --version

# rextendr regenerates the wrappers (and recompiles the crate to do it —
# plain devtools::document() does not); roxygen2 the .Rd docs and NAMESPACE.
# jsonlite is a hard dependency of the package itself.
RUN Rscript -e 'install.packages(c("rextendr", "devtools", "roxygen2", "testthat", "jsonlite", "knitr", "rmarkdown"))' \
    && Rscript -e 'for (p in c("rextendr","devtools","roxygen2","testthat","jsonlite")) if (!requireNamespace(p, quietly = TRUE)) stop("missing: ", p)' \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work/clients/r
