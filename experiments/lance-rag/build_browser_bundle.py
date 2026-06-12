#!/usr/bin/env python3
"""Build a compact, browser-loadable RAG bundle from a .rete graph.

The standalone `ask-browser.html` page can't parse a 12M/120M-triple graph in JS,
so we precompute — server-side — a small bundle it range-reads from the HF Space:

  <name>.rag.json  : {dim, dataset, entities:[{id,label}], edges:[[si,ti,pred]]}
  <name>.rag.f32   : N×dim float32 (L2-normalised) embeddings, entity-aligned

We keep the top-N most-connected, *labelled* entities (browser brute-force is fine
at ~10k), embed their label+text, and keep the entity↔entity edges among them so
the page can still walk graph paths. Two streaming passes over `rete export` keep
memory bounded.

    python build_browser_bundle.py <file.rete> --name wikidata-100MB -o out/ [--top 10000]
"""
from __future__ import annotations

import argparse
import os
import re
import struct
import subprocess
import sys
from collections import defaultdict

TRIPLE = re.compile(r"^(\S+)\s+(\S+)\s+(.+?)\s*\.\s*$")
LIT = re.compile(r'^"((?:[^"\\]|\\.)*)"')
LABELP = ("#label", "/name", "/title", "prefLabel", "schema.org/name")
DESCP = ("description", "abstract")
local = lambda iri: iri.strip("<>").rstrip("/").rsplit("/", 1)[-1].rsplit("#", 1)[-1]


def stream(rete_bin, rete_file):
    proc = subprocess.Popen([rete_bin, "export", rete_file], stdout=subprocess.PIPE,
                            text=True, bufsize=1 << 20)
    for line in proc.stdout:
        m = TRIPLE.match(line)
        if m:
            yield m.group(1), m.group(2), m.group(3)
    if proc.wait() != 0:
        sys.exit("`rete export` failed")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rete_file")
    ap.add_argument("--name", required=True, help="bundle basename, e.g. wikidata-100MB")
    ap.add_argument("--rete-bin", default="./target/release/rete")
    ap.add_argument("-o", "--output", default="experiments/lance-rag/out")
    ap.add_argument("--top", type=int, default=10000, help="keep the top-N most-connected labelled entities")
    ap.add_argument("--model", default="sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2")
    args = ap.parse_args()

    import numpy as np
    from fastembed import TextEmbedding

    # pass 1: label, text, and entity↔entity degree
    label, text, deg = {}, {}, defaultdict(int)
    n = 0
    for s, p, o in stream(args.rete_bin, args.rete_file):
        m = LIT.match(o)
        if m:                                   # literal object → label / text
            val = m.group(1).replace("\\n", " ").replace('\\"', '"').strip()
            if not val:
                continue
            if s not in label and any(k in p for k in LABELP):
                label[s] = val
            if s not in text and (any(k in p for k in LABELP) or any(k in p for k in DESCP)):
                text[s] = val[:240]
        elif o.startswith("<"):                 # entity→entity edge
            deg[s] += 1
            deg[o] += 1
        n += 1
    print(f"[bundle] pass1: {n} triples, {len(label)} labelled, {len(deg)} linked", flush=True)

    cand = [e for e in label if deg.get(e, 0) > 0] or list(label)
    cand.sort(key=lambda e: deg.get(e, 0), reverse=True)
    keep = cand[: args.top]
    idx = {e: i for i, e in enumerate(keep)}
    print(f"[bundle] keeping top {len(keep)} labelled+linked entities", flush=True)

    # pass 2: edges among the kept set
    edges = []
    for s, p, o in stream(args.rete_bin, args.rete_file):
        if o.startswith("<") and s in idx and o in idx:
            edges.append([idx[s], idx[o], local(p)])
    print(f"[bundle] {len(edges)} edges among kept entities", flush=True)

    docs = [(label.get(e, "") + ". " + text.get(e, ""))[:240] for e in keep]
    print(f"[bundle] embedding {len(docs)} entities with {args.model}…", flush=True)
    vecs = np.array(list(TextEmbedding(args.model).embed(docs)), dtype="float32")
    vecs /= (np.linalg.norm(vecs, axis=1, keepdims=True) + 1e-9)   # L2-normalise

    os.makedirs(args.output, exist_ok=True)
    import json
    meta = {"dim": int(vecs.shape[1]), "dataset": args.name,
            "entities": [{"id": e, "label": label.get(e, local(e))} for e in keep],
            "edges": edges}
    with open(os.path.join(args.output, f"{args.name}.rag.json"), "w", encoding="utf-8") as f:
        json.dump(meta, f, ensure_ascii=False)
    with open(os.path.join(args.output, f"{args.name}.rag.f32"), "wb") as f:
        f.write(vecs.tobytes())
    mb = vecs.nbytes / 1e6
    print(f"[bundle] wrote {args.name}.rag.json + .rag.f32 ({len(keep)} × {vecs.shape[1]}, {mb:.1f} MB)", flush=True)


if __name__ == "__main__":
    main()
