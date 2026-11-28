# rete-server: REST, SPARQL 1.1 Protocol and MCP over .rete graph files.
#
#   docker run -p 7860:7860 -e RETE_TOKEN=change-me ghcr.io/caviri/rete-server
#
# The engine wheelhouse is EMPTY unless the build supplies one. A plain
# `docker build` installs rete-graph from PyPI. A release, and server-test.yml,
# pass the wheels they built from the same commit:
#   docker build --build-context wheels=<dir with rete_graph-*.whl> \
#                --build-arg RETE_GRAPH_VERSION=X.Y.Z clients/server
# A named build context replaces the stage of the same name, so the engine then
# comes from that directory and pip may not look anywhere else for it — which is
# what lets a release build this image before PyPI has the version.
FROM scratch AS wheels

FROM python:3.12-slim

# graphviz: server-side layout for causal_diagram (dot -Tsvg).
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends graphviz \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY pyproject.toml README.md ./
COPY rete_server ./rete_server
# RETE_GRAPH_VERSION, when given, is the engine version the image must carry:
# the build fails rather than ship any other.
ARG RETE_GRAPH_VERSION=
RUN --mount=type=bind,from=wheels,target=/wheels \
    set -eu; \
    if ls /wheels/rete_graph-*.whl >/dev/null 2>&1; then \
        echo "rete-graph from the build's wheelhouse, not PyPI:"; ls /wheels; \
        pip install --no-cache-dir --no-index --find-links /wheels rete-graph; \
    fi; \
    pip install --no-cache-dir ".[ask]"; \
    installed=$(python -c 'import importlib.metadata as m; print(m.version("rete-graph"))'); \
    echo "rete-graph $installed installed"; \
    if [ -n "$RETE_GRAPH_VERSION" ] && [ "$installed" != "$RETE_GRAPH_VERSION" ]; then \
        echo "expected rete-graph $RETE_GRAPH_VERSION, got $installed" >&2; exit 1; \
    fi; \
    rm -rf /src

# An unprivileged user, uid 1000 (what Hugging Face Spaces run as, and the
# usual owner of a bind-mounted host folder). /data holds local .rete files
# (mount a folder there); /cache the optional block cache (mount a volume to
# stay warm on restart). The writable folders take any uid, for platforms that
# pick their own.
RUN useradd --uid 1000 --create-home rete \
    && mkdir -p /data /cache /tmp/rete-generated \
    && chown rete /data /cache /tmp/rete-generated \
    && chmod 1777 /cache /tmp/rete-generated
# A neutral working directory any uid can read (FastMCP's settings look for a
# .env in the working directory; a home folder another uid cannot enter
# would make that lookup fail).
WORKDIR /app
USER rete

ENV RETE_HOST=0.0.0.0 \
    RETE_PORT=7860 \
    RETE_DATA_DIR=/data \
    FORWARDED_ALLOW_IPS="*" \
    PYTHONUNBUFFERED=1
EXPOSE 7860
VOLUME ["/data", "/cache"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD python -c "import os, urllib.request; urllib.request.urlopen(f'http://127.0.0.1:{os.environ.get(\"RETE_PORT\", \"7860\")}/health', timeout=4)"

ENTRYPOINT ["rete-server"]
