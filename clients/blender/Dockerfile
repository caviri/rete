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
# packaged extension's bundled wheels really do install themselves.
ARG WITH_ENGINE=1
RUN /opt/blender/*/python/bin/python3* -m ensurepip --upgrade && \
    if [ "$WITH_ENGINE" = "1" ]; then \
        /opt/blender/*/python/bin/python3* -m pip install --no-cache-dir "rete-graph>=0.2.3"; \
    fi

# ifcopenshell exercises the IFC-import path in the tests. It is NOT bundled in
# the shipped extension (too large); end users install it themselves, or use the
# cad:glbModel column instead. Skipped in the clean image, whose absence of it
# proves the graceful "install ifcopenshell" message.
ARG WITH_IFC=1
RUN if [ "$WITH_ENGINE" = "1" ] && [ "$WITH_IFC" = "1" ]; then \
        /opt/blender/*/python/bin/python3* -m pip install --no-cache-dir ifcopenshell; \
    fi

WORKDIR /work
