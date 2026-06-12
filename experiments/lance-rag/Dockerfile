# lance-rag experiment — a cloud-friendly VECTOR sidecar for a .rete graph.
# rete supplies the symbolic 1-hop neighbourhood of a node; a Lance dataset
# supplies per-entity embeddings + ANN. Together: retrieve a node's structural
# neighbours, pull their vectors, cluster them, and (optionally) expand the set
# with semantically-similar entities the graph doesn't link — a RAG context.
#
# Light deps on purpose: fastembed (ONNX, no torch) for embeddings, lancedb for
# the vector store, scikit-learn for the local clustering. The rete binary is
# the one already built at /work/target/release/rete (mounted), not rebuilt.
FROM python:3.12-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN pip install --no-cache-dir \
        lancedb \
        fastembed \
        scikit-learn \
        "numpy<2"

WORKDIR /work
