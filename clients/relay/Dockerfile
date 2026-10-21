# HTTP Range storage gateway (project-agnostic) — Docker SDK. Serves /data with
# Range + CORS on port 7860. Theme the landing page by shipping a branding.json
# (optional — without it the gateway shows a neutral page).

# The engine wheelhouse, EMPTY unless the build supplies one. A plain
# `docker build` (and the Hugging Face Space) installs rete-graph from PyPI. A
# release, and relay-test.yml, pass the wheels they built from the same commit:
#   docker build --build-context wheels=<dir with rete_graph-*.whl> \
#                --build-arg RETE_GRAPH_VERSION=X.Y.Z clients/relay
# A named build context replaces the stage of the same name, so the engine then
# comes from that directory and pip is not allowed to look anywhere else for it.
# That is what lets a release build this image before PyPI has the version.
FROM scratch AS wheels

FROM python:3.11-slim

WORKDIR /app
# graphviz: server-side layout for the causal_diagram tool (dot -Tsvg).
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends graphviz \
    && rm -rf /var/lib/apt/lists/*
COPY requirements.txt .
# RETE_GRAPH_VERSION, when given, is the engine version the image must carry:
# the build fails rather than ship any other, e.g. a PyPI wheel that won over a
# missing or older local one.
ARG RETE_GRAPH_VERSION=
RUN --mount=type=bind,from=wheels,target=/wheels \
    set -eu; \
    if ls /wheels/rete_graph-*.whl >/dev/null 2>&1; then \
        echo "rete-graph from the build's wheelhouse, not PyPI:"; ls /wheels; \
        pip install --no-cache-dir --no-index --find-links /wheels rete-graph; \
    fi; \
    pip install --no-cache-dir -r requirements.txt; \
    installed=$(python -c 'import importlib.metadata as m; print(m.version("rete-graph"))'); \
    echo "rete-graph $installed installed"; \
    if [ -n "$RETE_GRAPH_VERSION" ] && [ "$installed" != "$RETE_GRAPH_VERSION" ]; then \
        echo "expected rete-graph $RETE_GRAPH_VERSION, got $installed" >&2; exit 1; \
    fi
COPY app.py rete_service.py rete_api.py rete_mcp.py rete_sparql.py rete_media.py rete_author.py rete_qlint.py rete_ask.py catalog.json branding.json ./

# HF persistent storage (and any mounted bucket) lives at /data.
#   CACHE_MB     in-RAM block cache PER worker (total ≈ CACHE_MB × WEB_CONCURRENCY).
#                Hot dictionary chunks come from RAM, not the FUSE bucket — this is
#                what stops the recurring wedge under random-access range bursts.
#   READ_TIMEOUT a stalled bucket read returns 503 after N s instead of hanging
#                (and pinning a threadpool thread forever).
# Tuned to NOT stampede the FUSE bucket under a heavy range-read burst (the wedge):
#   WEB_CONCURRENCY   worker processes (2, matches cpu-basic's 2 vCPU; blocking I/O in
#                     one never stalls the other).
#   THREADPOOL_TOKENS threads per worker for sync reads (64 — was 200).
#   FUSE_CONCURRENCY  MAX concurrent BUCKET reads per worker (the gate — a burst queues
#                     here instead of stampeding the mount). Total bucket reads ≈
#                     FUSE_CONCURRENCY × WEB_CONCURRENCY (≈48), gentle on FUSE.
#   CACHE_MB          in-RAM block cache PER worker (hot dict chunks skip the bucket).
#   READ_TIMEOUT      a stalled/queued read 503s after N s (frees the thread, sheds load).
# Query-plane (SPARQL /api + MCP /mcp) settings:
#   RETE_CACHE_DIR   the disk block-cache goes to container-local /tmp, NOT
#                    /data — /data is a FUSE bucket mount here; writing cache
#                    blocks through FUSE would be slow (or refused) and is
#                    exactly the load the FUSE gate exists to avoid.
#   RETE_MAX_HANDLES resident graph handles PER worker (each caches decoded
#                    dictionary chunks in RAM — keep total modest).
ENV DATA_DIR=/data \
    WEB_CONCURRENCY=2 \
    THREADPOOL_TOKENS=64 \
    FUSE_CONCURRENCY=24 \
    CACHE_MB=512 \
    READ_TIMEOUT=12 \
    RETE_CACHE_DIR=/tmp/rete-cache \
    RETE_CACHE_MAX_MB=2048 \
    RETE_MAX_HANDLES=6
EXPOSE 7860

# Shell form so ${WEB_CONCURRENCY} expands. --limit-concurrency sheds a runaway flood
# as 503 (backpressure) instead of piling up threads → the OOM/wedge; --timeout-keep-alive
# keeps range-read connections warm across a burst.
CMD uvicorn app:app --host 0.0.0.0 --port 7860 --workers ${WEB_CONCURRENCY:-2} --limit-concurrency 128 --timeout-keep-alive 75
