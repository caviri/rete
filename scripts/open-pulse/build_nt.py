#!/usr/bin/env python3
"""Open-Pulse JSON-LD  ->  N-Triples.

The Open-Pulse GitHub-metadata crawler emits one JSON-LD document per crawled
entity (repository / user / organization). The same entity is re-extracted
across many experiment batches, so we first pick ONE file per `source_url`
(the richest = most @graph nodes), then expand each chosen document to RDF with
rdflib and stream it to a single N-Triples file.

Duplicate triples across files are fine: `rete build` stores an RDF set, so the
final .rete is deduplicated regardless. We keep the raw (possibly duplicated)
stream cheap to produce; the canonical de-duplicated .nt is exported back out of
the built .rete.

Outputs (under --out-dir):
  open-pulse.raw.nt        raw N-Triples (may contain duplicate lines)
  chosen_files.tsv         source_url \t detected_type \t chosen_path (manifest)
  build_stats.json         counts for the report / parquet stage
"""
import argparse
import json
import os
import sys
import time
from collections import Counter

import rdflib

GH2 = "https://github.com/https://github.com/"
GH1 = "https://github.com/"
G = "gme-internal:"
# seed timestamp fields, newest wins (GitHub-side; the only recency signal present)
TS_FIELDS = (G + "github_updated_at", G + "updated_at", G + "pushed_at")


def norm_iri(s):
    """Collapse the extractor's double (or deeper) github:// prefix bug."""
    if not isinstance(s, str) or GH2 not in s:
        return s
    while GH2 in s:
        s = s.replace(GH2, GH1)
    return s


def _sid(v):
    if isinstance(v, dict):
        return v.get("@id")
    return v if isinstance(v, str) else None


def flatten_nested(graph):
    """Promote nested gme values that JSON-LD expansion would otherwise drop.

    `gme-internal:ror_country` is a bare-keyed object {country_name, country_code}
    with no @context mapping, so JSON-LD expansion discards its contents and the
    predicate points at an empty blank node. Replace it with a clean literal
    (country name) + a sibling country-code literal so the fact survives into RDF.
    """
    for n in graph:
        v = n.get(G + "ror_country")
        if isinstance(v, dict):
            cn, cc = v.get("country_name"), v.get("country_code")
            if cn:
                n[G + "ror_country"] = cn
            else:
                n.pop(G + "ror_country", None)
            if cc:
                n[G + "ror_country_code"] = cc


def enumerate_metadata_files(base):
    """Every per-entity JSON-LD metadata file under the artifacts tree."""
    out = []
    for root, _dirs, files in os.walk(base):
        b = os.path.basename(root)
        is_meta = ("metadata" in b) or root.replace("\\", "/").endswith(
            ("sdsc-ordes-full/metadata", "reextract-2026-06/metadata-json",
             "epfl-enac-full/metadata-json")
        )
        if not is_meta:
            continue
        for f in files:
            if f.endswith(".json"):
                out.append(os.path.join(root, f))
    return out


def _type_of(n):
    t = n.get("@type")
    return t if isinstance(t, str) else (t[0] if isinstance(t, list) and t else None)


def _seed_recency(graph, su):
    """Newest seed timestamp for this entity, '' if none.

    Seed = the node that IS this entity: @id (double-prefix repaired) == su, or
    the node whose githubUsername/githubOrganizationHandle == su (persons keyed
    by ORCID, etc.). Falls back to the max timestamp anywhere in the graph.
    """
    seed = None
    for n in graph:
        if norm_iri(n.get("@id")) == su:
            seed = n
            break
    if seed is None:
        for n in graph:
            h = _sid(n.get("pulse:githubUsername")) or _sid(n.get("pulse:githubOrganizationHandle"))
            if norm_iri(h) == su:
                seed = n
                break
    nodes = [seed] if seed is not None else graph
    best = ""
    for n in nodes:
        for f in TS_FIELDS:
            v = n.get(f)
            if isinstance(v, str) and v > best:
                best = v
    return best


def choose_latest_per_entity(files):
    """Group by source_url; keep the LATEST *sufficiently complete* extraction.

    The same entity is re-crawled across batches at different depths. A shallow
    re-crawl (e.g. repo-only, 2 nodes) must not evict a full earlier extraction,
    so we first drop candidates below 50% of the entity's richest node count,
    then take the newest by seed github timestamp (tie-break richest, then path).

    Returns {source_url: (path, detected_type, node_count, recency)}, plus a list
    of (chosen_nodes, richest_nodes) pairs for degradation reporting.
    """
    groups = {}  # su -> list[(recency, node_count, path, dt)]
    errors = 0
    for p in files:
        try:
            with open(p, "r", encoding="utf-8") as fh:
                d = json.load(fh)
        except Exception:
            errors += 1
            continue
        su = d.get("source_url")
        out = d.get("output")
        if not su or not isinstance(out, dict) or "@graph" not in out:
            continue
        graph = out["@graph"]
        groups.setdefault(su, []).append(
            (_seed_recency(graph, su), len(graph), p, d.get("detected_type"))
        )

    best = {}
    degraded = []
    for su, cands in groups.items():
        maxn = max(c[1] for c in cands)
        gate = max(3, maxn * 0.5)
        pool = [c for c in cands if c[1] >= gate] or cands
        # newest first: recency desc, node_count desc, path desc (deterministic)
        rec, n, p, dt = max(pool, key=lambda c: (c[0], c[1], c[2]))
        best[su] = (p, dt, n, rec)
        if n < maxn:
            degraded.append((n, maxn))
    return best, errors, degraded


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True, help="artifacts .quest-artifacts dir")
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--limit", type=int, default=0, help="debug: only N entities")
    args = ap.parse_args()

    os.makedirs(args.out_dir, exist_ok=True)
    t0 = time.time()

    print("enumerating metadata files ...", flush=True)
    files = enumerate_metadata_files(args.base)
    print(f"  {len(files)} JSON-LD files", flush=True)

    print("selecting latest complete extraction per source_url ...", flush=True)
    best, parse_errors, degraded = choose_latest_per_entity(files)
    print(f"  {len(best)} unique entities  ({parse_errors} unreadable)", flush=True)
    if degraded:
        losses = [m - n for n, m in degraded]
        print(f"  {len(degraded)} entities where latest < richest "
              f"(mean {sum(losses)/len(losses):.1f} nodes fewer, max {max(losses)})",
              flush=True)

    chosen = sorted(best.items())  # deterministic order
    if args.limit:
        chosen = chosen[: args.limit]

    manifest_path = os.path.join(args.out_dir, "chosen_files.tsv")
    nt_path = os.path.join(args.out_dir, "open-pulse.raw.nt")
    type_counts = Counter()
    triples_written = 0
    conv_errors = 0
    normalized_lines = 0

    with open(manifest_path, "w", encoding="utf-8", newline="\n") as man, \
         open(nt_path, "wb") as ntf:
        man.write("source_url\tdetected_type\trecency\tnode_count\tpath\n")
        for i, (su, (path, dt, n, rec)) in enumerate(chosen):
            type_counts[dt] += 1
            man.write(f"{su}\t{dt}\t{rec}\t{n}\t{path}\n")
            try:
                with open(path, "r", encoding="utf-8") as fh:
                    d = json.load(fh)
                flatten_nested(d["output"].get("@graph", []))
                g = rdflib.Graph()
                g.parse(data=json.dumps(d["output"]), format="json-ld")
                data = g.serialize(format="nt", encoding="utf-8")
                if GH2.encode() in data:
                    before = data
                    while GH2.encode() in data:
                        data = data.replace(GH2.encode(), GH1.encode())
                    if data != before:
                        normalized_lines += 1
                ntf.write(data)
                triples_written += len(g)
            except Exception as e:  # noqa: BLE001
                conv_errors += 1
                if conv_errors <= 5:
                    print(f"  !! {path}: {e}", file=sys.stderr)
            if (i + 1) % 5000 == 0:
                print(f"  {i + 1}/{len(chosen)}  ({triples_written} triples, "
                      f"{time.time() - t0:.0f}s)", flush=True)

    stats = {
        "metadata_files_scanned": len(files),
        "unique_entities": len(best),
        "entities_converted": len(chosen),
        "selection": "latest complete extraction per source_url (>=50% richest, "
                     "newest github timestamp)",
        "entities_latest_below_richest": len(degraded),
        "type_counts": dict(type_counts),
        "raw_triples_written": triples_written,
        "files_iri_normalized": normalized_lines,
        "parse_errors": parse_errors,
        "conversion_errors": conv_errors,
        "seconds": round(time.time() - t0, 1),
    }
    with open(os.path.join(args.out_dir, "build_stats.json"), "w", encoding="utf-8") as fh:
        json.dump(stats, fh, indent=2)
    print(json.dumps(stats, indent=2), flush=True)


if __name__ == "__main__":
    main()
