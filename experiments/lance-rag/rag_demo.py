#!/usr/bin/env python3
"""Node + 1-hop RAG retrieval over a .rete graph + a Lance vector sidecar.

  1. ask rete for the node's 1-hop neighbours (out- and in-edges),
  2. pull those neighbours' vectors from Lance,
  3. CLUSTER the local neighbourhood (k-means) so a RAG gets tight, de-duplicated
     context instead of a hub's noisy neighbour dump,
  4. (optional) EXPAND with Lance ANN — semantically similar entities the graph
     doesn't link — and flag which are also structural neighbours.

    python rag_demo.py <file.rete> <node-iri> --lance out/vectors.lance \
        [--rete-bin …] [--clusters 3] [--expand 10]
"""
from __future__ import annotations

import argparse
import re
import subprocess

import numpy as np

TRIPLE = re.compile(r"^(\S+)\s+(\S+)\s+(.+?)\s*\.\s*$")
IRI = re.compile(r"^<[^>]+>$")


def neighbours(rete_bin, rete_file, node):
    """1-hop IRIs: objects of the node's out-edges + subjects of its in-edges."""
    nb = set()
    out = subprocess.run([rete_bin, "query", rete_file, "--subject", node],
                         capture_output=True, text=True)
    for ln in out.stdout.splitlines():
        m = TRIPLE.match(ln)
        if m and IRI.match(m.group(3)):
            nb.add(m.group(3))
    inn = subprocess.run([rete_bin, "query", rete_file, "--object", node],
                         capture_output=True, text=True)
    for ln in inn.stdout.splitlines():
        m = TRIPLE.match(ln)
        if m and IRI.match(m.group(1)):
            nb.add(m.group(1))
    nb.discard(node)
    return nb


def short(iri, label):
    name = label or re.sub(r".*[/#]([^/#>]+)>?$", r"\1", iri)
    return f"{name}  {iri}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rete_file")
    ap.add_argument("node", help="entity IRI, e.g. <http://ex/p1>")
    ap.add_argument("--rete-bin", default="./target/release/rete")
    ap.add_argument("--lance", default="experiments/lance-rag/out/vectors.lance")
    ap.add_argument("--clusters", type=int, default=3)
    ap.add_argument("--expand", type=int, default=0, help="Lance ANN: also pull N similar entities")
    args = ap.parse_args()

    import lancedb
    import os
    from sklearn.cluster import KMeans

    db = lancedb.connect(os.path.dirname(args.lance))
    tbl = db.open_table(os.path.splitext(os.path.basename(args.lance))[0])
    df = tbl.to_pandas()
    vec = {r.entity: np.asarray(r.vector, dtype="float32") for r in df.itertuples()}
    lab = {r.entity: r.label for r in df.itertuples()}

    nb = neighbours(args.rete_bin, args.rete_file, args.node)
    hood = [e for e in nb if e in vec]
    print(f"\nnode {short(args.node, lab.get(args.node, ''))}")
    print(f"1-hop neighbours: {len(nb)} ({len(hood)} with vectors)\n")
    if not hood:
        print("(no embedded neighbours — nothing to cluster)")
        return

    members = ([args.node] if args.node in vec else []) + hood
    X = np.vstack([vec[e] for e in members])
    k = max(1, min(args.clusters, len(members)))
    labels = KMeans(n_clusters=k, n_init=10, random_state=0).fit_predict(X) if k >= 2 else [0] * len(members)

    print(f"── neighbourhood clustered into {k} group(s) ──")
    for c in range(k):
        grp = [members[i] for i in range(len(members)) if labels[i] == c]
        print(f"\n  cluster {c}  ({len(grp)}):")
        for e in grp:
            mark = " ←query" if e == args.node else ""
            print(f"    · {short(e, lab.get(e, ''))}{mark}")

    if args.expand and args.node in vec:
        print(f"\n── Lance ANN expansion (top {args.expand} semantically similar) ──")
        res = tbl.search(vec[args.node]).limit(args.expand + 1).to_pandas()
        for r in res.itertuples():
            if r.entity == args.node:
                continue
            tag = "structural+semantic" if r.entity in nb else "semantic-only (no edge)"
            print(f"    · {short(r.entity, r.label)}   [{tag}]")


if __name__ == "__main__":
    main()
