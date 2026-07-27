# Build + test the pure-Java rete client, end to end, with no host toolchain.
# Build context is the REPO ROOT (the wasm crate path-deps ../../../crates):
#
#   docker build -f clients/java/Dockerfile -t rete-java .
#   docker run --rm rete-java            # re-run the test suite (offline)
#
# or via compose:  docker compose run --rm java
#
# Stage 1 compiles the C-ABI crate to an import-free wasm module; stage 2 drops
# that wasm into the JAR's resources and runs `mvn verify` (Chicory runs the
# wasm inside the JVM — no wasm-bindgen, no native library).

# ---- stage 1: Rust -> wasm engine -------------------------------------------
FROM rust:1.92-bookworm AS wasm
WORKDIR /src
RUN rustup target add wasm32-unknown-unknown
# The excluded FFI crate resolves rete-core through the workspace root, so the
# root manifest and every workspace member must be present. crates/ + the two
# root manifests are enough; nothing else is needed to build the wasm.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY clients/java/ffi ./clients/java/ffi
RUN cargo build --release --target wasm32-unknown-unknown \
        --manifest-path clients/java/ffi/Cargo.toml

# ---- stage 2: Java client build + test --------------------------------------
FROM maven:3.9-eclipse-temurin-21 AS java
WORKDIR /app
# Copy every module POM first so dependency resolution caches across source
# edits (the wasm and Java sources change far more often than the deps).
COPY clients/java/pom.xml ./pom.xml
COPY clients/java/rete-client/pom.xml ./rete-client/pom.xml
COPY clients/java/rete-rdf4j/pom.xml ./rete-rdf4j/pom.xml
# Best-effort warm: strict go-offline trips on the reactor's inter-module dep
# (rete-rdf4j → rete-client, not yet built), which is fine — the online verify
# below fetches whatever is missing and caches it into this layer.
RUN mvn -B -q dependency:go-offline || true
# Module sources.
COPY clients/java/rete-client/src ./rete-client/src
COPY clients/java/rete-rdf4j/src ./rete-rdf4j/src
# The freshly built engine into the resource path Rete.load() reads.
COPY --from=wasm \
    /src/clients/java/ffi/target/wasm32-unknown-unknown/release/rete_ffi.wasm \
    ./rete-client/src/main/resources/io/github/caviri/rete/rete_ffi.wasm
# Online: go-offline does not fetch surefire's dynamically-resolved test
# provider (surefire-junit-platform), so the first verify must reach central;
# it caches everything, which is why the CMD re-run below can stay offline.
RUN mvn -B verify

# Default: re-run the suite offline (deps + wasm already baked into the image).
CMD ["mvn", "-B", "-o", "verify"]
