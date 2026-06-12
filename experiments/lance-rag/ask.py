#!/usr/bin/env python3
"""GraphRAG query: a natural-language question -> ranked nodes + the PATHS
(sequences of triples) connecting them.

  1. embed the question (fastembed),
  2. Lance ANN -> the top-k most relevant entity nodes (the entry points),
  3. rete BFS -> the shortest path of triples between the #1 node and each other,
     so the answer is not isolated nodes but *how the graph connects them*.

This is the retrieval core an LLM agent would call: it returns ranked nodes +
connecting triples as grounded context. (Wrap with an LLM to phrase the final
answer — e.g. the Claude API — kept out of scope here.)

    python ask.py <file.rete> "your question" --lance out/vectors.lance [--topk 5] [--maxhops 4]
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
from collections import deque

import numpy as np

TRIPLE = re.compile(r"^(\S+)\s+(\S+)\s+(.+?)\s*\.\s*$")
IRI = re.compile(r"^<[^>]+>$")


def edges_of(rete_bin, rete_file, node):
    """1-hop edges of `node` as (predicate, neighbour, direction) tuples."""
    es = []
    out = subprocess.run([rete_bin, "query", rete_file, "--subject", node],
                         capture_output=True, text=True)
    for ln in out.stdout.splitlines():
        m = TRIPLE.match(ln)
        if m and IRI.match(m.group(3)):
            es.append((m.group(2), m.group(3), "out"))
    inn = subprocess.run([rete_bin, "query", rete_file, "--object", node],
                         capture_output=True, text=True)
    for ln in inn.stdout.splitlines():
        m = TRIPLE.match(ln)
        if m and IRI.match(m.group(1)):
            es.append((m.group(2), m.group(1), "in"))
    return es


def shortest_path(rete_bin, rete_file, src, dst, maxhops=4):
    """BFS over the graph; return the path src->dst as a list of (s,p,o) triples."""
    if src == dst:
        return []
    prev = {src: None}                       # node -> (parent, predicate, direction)
    frontier = [src]
    for _ in range(maxhops):
        nxt = []
        for node in frontier:
            for pred, nb, direction in edges_of(rete_bin, rete_file, node):
                if nb in prev:
                    continue
                prev[nb] = (node, pred, direction)
                if nb == dst:
                    return _reconstruct(prev, dst)
                nxt.append(nb)
        frontier = nxt
        if not frontier:
            break
    return None                              # unreachable within maxhops


def _reconstruct(prev, dst):
    triples, cur = [], dst
    while prev[cur] is not None:
        parent, pred, direction = prev[cur]
        triples.append((parent, pred, cur) if direction == "out" else (cur, pred, parent))
        cur = parent
    return list(reversed(triples))


def short(iri, label):
    return label or re.sub(r".*[/#]([^/#>]+)>?$", r"\1", iri)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rete_file")
    ap.add_argument("question")
    ap.add_argument("--rete-bin", default="./target/release/rete")
    ap.add_argument("--lance", default="experiments/lance-rag/out/vectors.lance")
    ap.add_argument("--topk", type=int, default=5)
    ap.add_argument("--maxhops", type=int, default=4)
    ap.add_argument("--model", default="BAAI/bge-small-en-v1.5")
    args = ap.parse_args()

    import lancedb
    from fastembed import TextEmbedding

    qvec = next(TextEmbedding(args.model).embed([args.question]))
    db = lancedb.connect(os.path.dirname(args.lance))
    tbl = db.open_table(os.path.splitext(os.path.basename(args.lance))[0])
    hits = tbl.search(np.asarray(qvec, dtype="float32")).limit(args.topk).to_pandas()

    print(f'\nQ: "{args.question}"\n')
    print(f"── top {len(hits)} ranked nodes ──")
    ranked = []
    for i, r in enumerate(hits.itertuples()):
        dist = getattr(r, "_distance", None)
        print(f"  {i+1}. {short(r.entity, r.label)}   {r.entity}"
              + (f"   (d={dist:.3f})" if dist is not None else ""))
        ranked.append(r.entity)

    if len(ranked) < 2:
        return
    anchor = ranked[0]
    print(f"\n── paths from #1 ({short(anchor, hits.iloc[0].label)}) to the others ──")
    for tgt in ranked[1:]:
        path = shortest_path(args.rete_bin, args.rete_file, anchor, tgt, args.maxhops)
        if path is None:
            print(f"\n  → {short(tgt, '')}: no path within {args.maxhops} hops")
        elif not path:
            print(f"\n  → {short(tgt, '')}: same node")
        else:
            print(f"\n  → {short(tgt, '')}  ({len(path)} hops):")
            for s, p, o in path:
                print(f"      {short(s,'')}  --{short(p,'')}-->  {short(o,'')}")


if __name__ == "__main__":
    main()
