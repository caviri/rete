#!/usr/bin/env python3
"""Topic map: rete communities -> LDA topics -> a zoomable, labelled PMTiles map.

A companion to build_map.py. Where build_map.py draws the *structural* community
pyramid (link topology), this draws the *semantic* one: each rete community is
laid out by the **similarity of its text**, coloured by its dominant **LDA
topic**, and labelled by its top words. Two zoom levels:

  z0  topic regions  — one translucent hull per LDA topic (the legend colours)
  z1+ communities    — points inside, coloured by topic, labelled by top words

Pipeline (rete supplies the two hard parts — the Louvain partition and the
per-community text; LDA + layout are standard downstream steps):

  rete communities --json --profile   -> communities w/ members, text, profile
  CountVectorizer + LatentDirichletAllocation (scikit-learn) -> topic per comm
  TruncatedSVD(2) on TF-IDF            -> 2D text-similarity layout
  per-topic convex hulls + GeoJSON     -> tippecanoe -> topicmap.pmtiles

This is a SIDE EXPERIMENT: it only reads a .rete; nothing here touches core.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys


def log(m: str) -> None:
    print(f"[topic-map] {m}", flush=True)


def run_communities(rete_bin, rete_file, round_, min_size):
    cmd = [rete_bin, "communities", "--json", "--profile", "--min-size", str(min_size)]
    if round_ is not None:
        cmd += ["--round", str(round_)]
    cmd.append(rete_file)
    log("running: " + " ".join(cmd))
    out = subprocess.run(cmd, capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"`rete communities` failed: {out.stderr[:400]}")
    comms = json.loads(out.stdout)
    log(f"{len(comms)} communities (size >= {min_size})")
    return comms


def top_terms(profile, k=3):
    """A short human label from a community's top literal words (Title-cased)."""
    terms = (profile or {}).get("terms") or []
    words = [t[0] if isinstance(t, list) else t for t in terms][:k]
    return " ".join(w.title() for w in words) if words else "?"


def _localname(iri):
    import re
    iri = iri.strip("<>").rstrip("/")
    m = re.search(r"[/#]([^/#]+)$", iri)
    return m.group(1) if m else iri


def top_type(profile):
    """Most common rdf:type class (skipping the generic wikibase Item)."""
    types = [t[0] for t in (profile or {}).get("types") or [] if isinstance(t, list)]
    for v in types:
        ln = _localname(v)
        if ln not in ("Item", "Entity"):
            return ln
    return _localname(types[0]) if types else ""


def top_preds(profile, k=3):
    """Top-k predicates (the community's characteristic properties)."""
    preds = (profile or {}).get("predicates") or []
    return ", ".join(_localname(p[0]) for p in preds[:k] if isinstance(p, list))


def normalize(coords, halfx, halfy):
    import numpy as np
    a = np.asarray(coords, dtype=float)
    lo, hi = a.min(0), a.max(0)
    span = np.where(hi - lo == 0, 1.0, hi - lo)
    out = (a - lo) / span
    out[:, 0] = out[:, 0] * 2 * halfx - halfx
    out[:, 1] = out[:, 1] * 2 * halfy - halfy
    return [[round(float(x), 5), round(float(y), 5)] for x, y in out]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rete_file")
    ap.add_argument("--rete-bin", default="./target/release/rete")
    ap.add_argument("-o", "--output", default="experiments/graph-map/out")
    ap.add_argument("--name", default="topicmap")
    ap.add_argument("--round", type=int, default=None, help="dendrogram round to cut at")
    ap.add_argument("--min-size", type=int, default=2, help="drop communities smaller than this")
    ap.add_argument("--from-json", default=None,
                    help="reuse a precomputed `communities --json --profile` dump "
                         "(skips the Louvain recompute)")
    ap.add_argument("--topics", type=int, default=12, help="number of LDA topics")
    ap.add_argument("--max-features", type=int, default=4000)
    ap.add_argument("--text-cap", type=int, default=2500,
                    help="max literals per community fed to LDA (balances giant communities)")
    ap.add_argument("--maxzoom", type=int, default=9,
                    help="tile maxzoom (the viewer overzooms past this to 20+, so a "
                         "modest value gives smooth deep zoom; high values tile the "
                         "wide layout into millions of cells and are very slow)")
    ap.add_argument("--footprint", type=float, default=55.0)
    args = ap.parse_args()

    import numpy as np
    from shapely.geometry import MultiPoint
    import math
    from collections import Counter
    from sklearn.cluster import AgglomerativeClustering
    from sklearn.decomposition import LatentDirichletAllocation
    from sklearn.feature_extraction.text import CountVectorizer, ENGLISH_STOP_WORDS

    os.makedirs(args.output, exist_ok=True)
    if args.from_json:
        log(f"loading precomputed communities from {args.from_json}")
        raw = open(args.from_json, "rb").read()
        if raw[:2] in (b"\xff\xfe", b"\xfe\xff"):      # UTF-16 (e.g. PowerShell redirect)
            text = raw.decode("utf-16")
        else:
            text = raw.decode("utf-8-sig")
        comms = json.loads(text)
        comms = [c for c in comms if c.get("size", 0) >= args.min_size]
        log(f"{len(comms)} communities (size >= {args.min_size})")
    else:
        comms = run_communities(args.rete_bin, args.rete_file, args.round, args.min_size)
    if len(comms) < args.topics:
        log(f"only {len(comms)} communities — reducing topics to {max(2, len(comms)//2)}")
        args.topics = max(2, len(comms) // 2)
    # Cap each community's text (first N literals) so the few huge communities
    # don't dominate the vocabulary and swamp the topics.
    docs = [" ".join((c.get("text") or [])[:args.text_cap]) for c in comms]

    # Wikimedia-meta / boilerplate noise that otherwise forms junk "topics".
    META = {"wikimedia", "wikipedia", "wikidata", "commons", "page", "pages", "category",
            "categories", "template", "templates", "disambiguation", "begriffskl",
            "rungsseite", "yenik", "list", "lists", "lista", "liste", "elenco", "article",
            "articles", "articolo", "dia", "redirect", "stub", "www", "http", "https",
            "org", "com", "html", "named", "name", "names"}
    stop = list(ENGLISH_STOP_WORDS.union(META))

    log("vectorizing community text…")
    cv = CountVectorizer(stop_words=stop, max_features=args.max_features,
                         token_pattern=r"[A-Za-z][A-Za-z]{2,}", min_df=2, max_df=0.5)
    counts = cv.fit_transform(docs)
    vocab = np.array(cv.get_feature_names_out())

    log(f"fitting LDA ({args.topics} topics, batch)…")
    lda = LatentDirichletAllocation(n_components=args.topics, max_iter=50,
                                    learning_method="batch", random_state=0)
    doc_topic = lda.fit_transform(counts)
    topic_of = doc_topic.argmax(1)
    topic_words = []
    for t in range(args.topics):
        top = lda.components_[t].argsort()[::-1][:6]
        topic_words.append(", ".join(w.title() for w in vocab[top]))

    # Layout: each LDA topic gets an anchor on a ring; its communities scatter in
    # a disc around it (golden-angle fill, area ∝ topic size). Clean topic regions
    # — robust to a few huge communities that wreck a raw text-similarity SVD.
    log("laying out: topic regions on a ring (ordered by similarity)…")
    tc = Counter(int(t) for t in topic_of)
    # order topics around the ring so adjacent regions are the most similar
    # (greedy nearest-neighbour chain over topic-vector cosine similarity).
    cn = lda.components_ / (np.linalg.norm(lda.components_, axis=1, keepdims=True) + 1e-9)
    sim = cn @ cn.T
    order, used = [0], {0}
    while len(order) < args.topics:
        last = order[-1]
        order.append(max((j for j in range(args.topics) if j not in used),
                         key=lambda j: sim[last, j]))
        used.add(order[-1])
    ring = {t: order.index(t) for t in range(args.topics)}
    anchor = {t: (math.cos(2 * math.pi * ring[t] / args.topics) * 1.5,
                  math.sin(2 * math.pi * ring[t] / args.topics) * 1.5) for t in range(args.topics)}
    seen, raw = {}, []
    for t in topic_of:
        t = int(t); k = seen.get(t, 0); seen[t] = k + 1
        ang = k * 2.399963229545                 # golden angle → even disc
        rad = 0.6 * math.sqrt((k + 0.5) / max(1, tc[t]))
        ax, ay = anchor[t]
        raw.append([ax + rad * math.cos(ang), ay + rad * math.sin(ang)])
    coords = normalize(raw, args.footprint, args.footprint * 0.7)

    # palette (distinct hues); the legend maps colour -> topic words
    PALETTE = ["#e6194B", "#3cb44b", "#ffe119", "#4363d8", "#f58231", "#911eb4",
               "#42d4f4", "#f032e6", "#bfef45", "#fabed4", "#469990", "#dcbeff",
               "#9A6324", "#fffac8", "#800000", "#aaffc3", "#808000", "#ffd8b1",
               "#000075", "#a9a9a9"]
    colors = [PALETTE[t % len(PALETTE)] for t in range(args.topics)]

    # PYRAMID: cluster the LDA topics into a few super-groups (the coarse zoom
    # level). One shared LDA, projected onto 3 levels — z0 meta-topics, z1
    # topics, z2 communities — so zooming out shows broad themes, zooming in
    # resolves finer ones (topic ids/colours stay stable across levels).
    M = min(4, max(2, args.topics // 3))
    meta_of = (AgglomerativeClustering(n_clusters=M, metric="cosine", linkage="average")
               .fit_predict(cn) if args.topics > M else np.zeros(args.topics, dtype=int))
    eps = 0.01 * args.footprint

    def hull_ring(pts):
        g = MultiPoint(pts).convex_hull
        if g.geom_type != "Polygon":
            g = g.buffer(eps)
        return [[round(x, 5), round(y, 5)] for (x, y) in g.exterior.coords]

    def trim_words(ws, k=6):
        return ", ".join(list(dict.fromkeys(ws.replace(", ", ",").split(",")))[:k])

    features = []
    # z2: communities (points), coloured by topic + the type/properties popup data
    for c, xy, t in zip(comms, coords, topic_of):
        features.append({
            "type": "Feature", "tippecanoe": {"minzoom": 2, "maxzoom": args.maxzoom},
            "geometry": {"type": "Point", "coordinates": xy},
            "properties": {"level": 2, "kind": "community", "id": int(c["community"]),
                           "size": int(c["size"]), "topic": int(t), "color": colors[t],
                           "label": top_terms(c.get("profile")),
                           "etype": top_type(c.get("profile")), "props": top_preds(c.get("profile"))},
        })
    # z2: connections — each community linked to its nearest peers in topic space
    dt = doc_topic / (np.linalg.norm(doc_topic, axis=1, keepdims=True) + 1e-9)
    csim = dt @ dt.T
    seen_e = set()
    for i in range(len(comms)):
        added = 0
        for j in np.argsort(csim[i])[::-1]:
            j = int(j)
            if j == i:
                continue
            key = (min(i, j), max(i, j))
            if key in seen_e:
                continue
            seen_e.add(key)
            features.append({
                "type": "Feature", "tippecanoe": {"minzoom": 2, "maxzoom": args.maxzoom},
                "geometry": {"type": "LineString", "coordinates": [coords[i], coords[j]]},
                "properties": {"level": 2, "kind": "edge", "w": round(float(csim[i][j]), 3)},
            })
            added += 1
            if added >= 3:
                break
    # z1: topic regions (hull + keyword label)
    topic_meta = []
    for t in range(args.topics):
        pts = [coords[i] for i in range(len(comms)) if topic_of[i] == t]
        if not pts:
            continue
        cx = sum(p[0] for p in pts) / len(pts); cy = sum(p[1] for p in pts) / len(pts)
        features.append({
            "type": "Feature", "tippecanoe": {"minzoom": 1, "maxzoom": args.maxzoom},
            "geometry": {"type": "Polygon", "coordinates": [hull_ring(pts)]},
            "properties": {"level": 1, "kind": "topic", "topic": int(t), "color": colors[t],
                           "members": len(pts), "label": topic_words[t]},
        })
        features.append({
            "type": "Feature", "tippecanoe": {"minzoom": 1, "maxzoom": args.maxzoom},
            "geometry": {"type": "Point", "coordinates": [round(cx, 5), round(cy, 5)]},
            "properties": {"level": 1, "kind": "topic-label", "topic": int(t),
                           "color": colors[t], "label": topic_words[t], "members": len(pts)},
        })
        topic_meta.append({"id": t, "color": colors[t], "words": topic_words[t],
                           "communities": len(pts), "meta": int(meta_of[t])})
    # z0: meta-topic regions (the coarse zoom-out level)
    meta_meta = []
    for mg in range(M):
        ts = [t for t in range(args.topics) if int(meta_of[t]) == mg]
        pts = [coords[i] for i in range(len(comms)) if int(meta_of[topic_of[i]]) == mg]
        if not pts:
            continue
        cx = sum(p[0] for p in pts) / len(pts); cy = sum(p[1] for p in pts) / len(pts)
        words = trim_words(" , ".join(topic_words[t] for t in ts))
        mcolor = colors[ts[0]] if ts else "#888888"
        features.append({
            "type": "Feature", "tippecanoe": {"minzoom": 0, "maxzoom": args.maxzoom},
            "geometry": {"type": "Polygon", "coordinates": [hull_ring(pts)]},
            "properties": {"level": 0, "kind": "meta", "meta": mg, "color": mcolor,
                           "members": len(pts), "label": words},
        })
        features.append({
            "type": "Feature", "tippecanoe": {"minzoom": 0, "maxzoom": args.maxzoom},
            "geometry": {"type": "Point", "coordinates": [round(cx, 5), round(cy, 5)]},
            "properties": {"level": 0, "kind": "meta-label", "meta": mg, "color": mcolor,
                           "label": words, "members": len(pts)},
        })
        meta_meta.append({"id": mg, "color": mcolor, "words": words, "topics": ts})

    geojson = os.path.join(args.output, f"{args.name}.geojson")
    with open(geojson, "w") as f:
        json.dump({"type": "FeatureCollection", "features": features}, f)
    log(f"wrote {len(features)} features -> {geojson}")

    pmtiles = os.path.join(args.output, f"{args.name}.pmtiles")
    cmd = ["tippecanoe", "-o", pmtiles, "-l", "topics", "-n", "rete topic map",
           "--minimum-zoom=0", f"--maximum-zoom={args.maxzoom}",
           "--no-feature-limit", "--no-tile-size-limit", "--read-parallel", "--force", geojson]
    log("tippecanoe: " + " ".join(cmd))
    subprocess.run(cmd, check=True)

    meta = {"source": os.path.basename(args.rete_file), "maxzoom": args.maxzoom,
            "footprint": [args.footprint, args.footprint * 0.7],
            "communities": len(comms), "topics": topic_meta, "meta": meta_meta,
            "levels": [{"z": 0, "kind": "meta", "count": len(meta_meta)},
                       {"z": 1, "kind": "topic", "count": len(topic_meta)},
                       {"z": 2, "kind": "community", "count": len(comms)}]}
    with open(os.path.join(args.output, f"{args.name}.json"), "w") as f:
        json.dump(meta, f, indent=2)
    log(f"done: {pmtiles} ({os.path.getsize(pmtiles)/1e6:.1f} MB), {len(comms)} communities, "
        f"{len(topic_meta)} topics, {len(meta_meta)} meta-topics, {len(seen_e)} edges, maxzoom {args.maxzoom}")


if __name__ == "__main__":
    main()
