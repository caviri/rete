# Headless Blender for building and testing the rete add-on.
#
# Blender 4.5 LTS is the primary target (Python 3.11, the extensions system with
# bundled wheels). `--build-arg BLENDER=5.1.2 --build-arg SERIES=5.1` tests a
# newer series against the same add-on source.
#
# Build:  docker build -t rete-blender clients/blender
# Test:   docker run --rm -v "$PWD":/work -w /work rete-blender \
#             blender -b --factory-startup -noaudio \
#                 --python clients/blender/tests/run_tests.py

# The engine wheelhouse, EMPTY unless the build supplies one, in which case the
# engine is installed from it and nowhere else (see the WITH_ENGINE step):
#   docker build --build-context wheels=<dir with rete_graph-*.whl> \
#                --build-arg RETE_GRAPH_VERSION=X.Y.Z -t rete-blender clients/blender
# release.yml does exactly that with the wheels it built from the tagged commit,
# so the add-on is tested against the engine it bundles, before PyPI has it.
FROM scratch AS wheels

FROM debian:bookworm-slim

ARG BLENDER=4.5.9
ARG SERIES=4.5

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates wget xz-utils zip \
      python3 python3-pip \
      libgl1 libegl1 libxrender1 libxxf86vm1 libxfixes3 libxi6 libsm6 \
      libxkbcommon0 libxrandr2 libxinerama1 libxcursor1 libx11-6 \
      libglib2.0-0 libdbus-1-3 && \
    rm -rf /var/lib/apt/lists/*

RUN wget -q "https://download.blender.org/release/Blender${SERIES}/blender-${BLENDER}-linux-x64.tar.xz" \
        -O /tmp/blender.tar.xz && \
    mkdir -p /opt/blender && \
    tar -xf /tmp/blender.tar.xz -C /opt/blender --strip-components=1 && \
    rm /tmp/blender.tar.xz && \
    ln -s /opt/blender/blender /usr/local/bin/blender

# The add-on's own dependency, installed into Blender's bundled interpreter so
# the source tests exercise the same import path a user's install produces.
# Build with --build-arg WITH_ENGINE=0 to leave it out: that image proves the
# packaged extension's bundled wheels really do install themselves. With no
# wheelhouse the engine comes from PyPI; RETE_GRAPH_VERSION, when given, is the
# version the image must end up with, or the build fails.
ARG WITH_ENGINE=1
ARG RETE_GRAPH_VERSION=
RUN --mount=type=bind,from=wheels,target=/wheels \
    /opt/blender/*/python/bin/python3* -m ensurepip --upgrade && \
    if [ "$WITH_ENGINE" = "1" ]; then \
        if ls /wheels/rete_graph-*.whl >/dev/null 2>&1; then \
            echo "rete-graph from the build's wheelhouse, not PyPI:" && ls /wheels && \
            /opt/blender/*/python/bin/python3* -m pip install --no-cache-dir \
                --no-index --find-links /wheels rete-graph; \
        else \
            /opt/blender/*/python/bin/python3* -m pip install --no-cache-dir "rete-graph>=0.3.6"; \
        fi && \
        installed=$(/opt/blender/*/python/bin/python3* -c \
            'import importlib.metadata as m; print(m.version("rete-graph"))') && \
        echo "rete-graph $installed installed" && \
        if [ -n "$RETE_GRAPH_VERSION" ] && [ "$installed" != "$RETE_GRAPH_VERSION" ]; then \
            echo "expected rete-graph $RETE_GRAPH_VERSION, got $installed" >&2; exit 1; \
        fi; \
    fi

# ifcopenshell (IFC) and laspy[lazrs] (LAS/LAZ/COPC) exercise the CAD and
# point-cloud paths in the tests. Neither is bundled in the shipped extension
# (too large); end users install them, or use the glTF/LAS fallbacks. Skipped in
# the clean image, whose absence proves the graceful "install …" messages.
ARG WITH_IFC=1
RUN if [ "$WITH_ENGINE" = "1" ] && [ "$WITH_IFC" = "1" ]; then \
        /opt/blender/*/python/bin/python3* -m pip install --no-cache-dir \
            ifcopenshell "laspy[lazrs]"; \
    fi

WORKDIR /work
