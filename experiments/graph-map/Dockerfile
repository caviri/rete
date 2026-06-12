# Graph-map experiment — turns a .rete community pyramid into a zoomable
# PMTiles map. Standalone toolchain (NOT part of the core rete build):
#   * tippecanoe (Felt fork) — GeoJSON -> vector tiles -> .pmtiles, with
#     per-feature zoom control (each community level becomes a zoom band).
#   * python-igraph — multilevel Louvain coarsening + 2D layout.
#   * pmtiles / shapely — archive + geometry helpers.
# The rete binary itself is NOT rebuilt here; we call the one already built at
# /work/target/release/rete (mounted from the repo), so this image stays lean.
FROM python:3.12-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential git ca-certificates \
        libsqlite3-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

# tippecanoe — Felt fork emits .pmtiles directly (`-o out.pmtiles`).
RUN git clone --depth 1 https://github.com/felt/tippecanoe /tmp/tippecanoe \
    && make -C /tmp/tippecanoe -j"$(nproc)" \
    && make -C /tmp/tippecanoe install \
    && rm -rf /tmp/tippecanoe

RUN pip install --no-cache-dir python-igraph==0.11.* pmtiles==3.* shapely==2.* \
        scikit-learn==1.5.* numpy==2.*

WORKDIR /work
ENTRYPOINT ["python", "experiments/graph-map/build_map.py"]
