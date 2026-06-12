#!/usr/bin/env python3
"""Build a Lance vector sidecar for a .rete graph.

For every subject in the graph, concatenate its **literal** objects (labels,
titles, descriptions, abstracts…) into one document, embed it, and write a Lance
dataset of `(entity, label, vector)` rows keyed by the entity's N-Triples IRI
token — the same key the rete dictionary and the Parquet entity tables use, so
neighbour IRIs from rete join straight to Lance rows.

This is a SIDE EXPERIMENT: it only *reads* the graph (via `rete export`); it
never modifies the .rete format.

    python build_vectors.py <file.rete> -o vectors.lance [--rete-bin …] [--index]
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from collections import defaultdict

# <s> <p> <o> .   — o is a literal "…"(@lang|^^type) or an IRI <…> or _:bnode
TRIPLE = re.compile(r'^(\S+)\s+(\S+)\s+(.+?)\s*\.\s*$')
LITERAL = re.compile(r'^"(.*)"(?:@[\w-]+|\^\^\S+)?$', re.S)
LABELISH = ("label", "name", "title", "abstract", "description", "prefLabel", "altLabel")


def log(m): print(f"[lance-rag] {m}", flush=True)


def export_text(rete_bin, rete_file):
    """Stream `rete export` -> {entity_iri: joined literal text, …} + a 'label'."""
    text = defaultdict(list)
    label = {}
    proc = subprocess.Popen([rete_bin, "export", rete_file], stdout=subprocess.PIPE,
                            text=True, bufsize=1 << 20)
    n = 0
    for line in proc.stdout:
        m = TRIPLE.match(line)
        if not m:
            continue
        s, p, o = m.group(1), m.group(2), m.group(3)
        lit = LITERAL.match(o)
        if not lit:
            continue                       # only literal objects become text
        val = lit.group(1).replace("\\n", " ").strip()
        if not val:
            continue
        text[s].append(val)
        if s not in label and any(k.lower() in p.lower() for k in ("label", "name", "title")):
            label[s] = val
        n += 1
    if proc.wait() != 0:
        sys.exit("`rete export` failed")
    log(f"{n} literal triples -> {len(text)} entities with text")
    return text, label


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rete_file")
    ap.add_argument("--rete-bin", default="./target/release/rete")
    ap.add_argument("-o", "--output", default="experiments/lance-rag/out/vectors.lance")
    ap.add_argument("--model", default="BAAI/bge-small-en-v1.5", help="fastembed model (384-dim)")
    ap.add_argument("--max-chars", type=int, default=1200, help="cap per-entity text")
    ap.add_argument("--index", action="store_true", help="build an IVF-PQ ANN index (for big sets)")
    args = ap.parse_args()

    import lancedb
    from fastembed import TextEmbedding

    text, label = export_text(args.rete_bin, args.rete_file)
    ents = sorted(text)
    docs = [" ".join(text[e])[: args.max_chars] for e in ents]

    log(f"embedding {len(docs)} entities with {args.model}…")
    model = TextEmbedding(args.model)
    vecs = list(model.embed(docs))         # 384-dim float32 numpy arrays

    rows = [{"entity": e, "label": label.get(e, ""), "vector": v.tolist()}
            for e, v in zip(ents, vecs)]

    os.makedirs(os.path.dirname(args.output), exist_ok=True)
    db = lancedb.connect(os.path.dirname(args.output))
    name = os.path.splitext(os.path.basename(args.output))[0]
    db.drop_table(name, ignore_missing=True)
    tbl = db.create_table(name, data=rows)
    log(f"wrote {tbl.count_rows()} vectors -> {args.output} (table '{name}')")

    if args.index and tbl.count_rows() >= 256:
        log("building IVF-PQ index…")
        tbl.create_index(num_partitions=min(256, tbl.count_rows() // 16),
                         num_sub_vectors=48, metric="cosine")
        log("index built")
    else:
        log("no ANN index (small set → brute-force search is fine)")


if __name__ == "__main__":
    main()
