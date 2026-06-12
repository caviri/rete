#!/usr/bin/env python3
"""Project a .rete community pyramid into a zoomable PMTiles graph-map.

Each granularity level becomes a map zoom band: the coarsest super-communities
show when zoomed out (the "continents"); finer rete pyramid communities appear
as you zoom in. Node size ~ connectivity (superedge weight) so the
highest-degree hubs dominate at low zoom — the "highest-connection nodes" idea.

This is a SIDE EXPERIMENT: it only *reads* a .rete (via `rete summary`) and the
community pyramid already inside it. Nothing here touches the core format.

Pipeline:
  rete summary <file>            -> weighted community graph (pyramid round 0)
  igraph multilevel Louvain      -> a dendrogram of coarser super-communities
  igraph DRL layout              -> 2D coords for the base communities
  size-weighted centroids upward -> coords for each coarser level
  GeoJSON (per-feature minzoom)  -> tippecanoe -> graphmap.pmtiles
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections import defaultdict

INTERNAL_RE = re.compile(r"^\s*C(\d+) \(internal\) C\d+\s+via\s+<[^>]*>\s+x(\d+)")
CROSS_RE = re.compile(r"^\s*C(\d+) -> C(\d+)\s+via\s+<[^>]*>\s+x(\d+)")


def log(msg: str) -> None:
    print(f"[graph-map] {msg}", flush=True)


def parse_summary(rete_bin: str, rete_file: str):
    """Stream `rete summary` -> (internal_weight, cross_weight) dicts."""
    internal = defaultdict(int)
    cross = defaultdict(int)  # (a, b) with a < b -> summed weight across predicates
    proc = subprocess.Popen(
        [rete_bin, "summary", rete_file],
        stdout=subprocess.PIPE, text=True, bufsize=1 << 20,
    )
    n = 0
    for line in proc.stdout:
        m = CROSS_RE.match(line)
        if m:
            a, b, w = int(m.group(1)), int(m.group(2)), int(m.group(3))
            if a != b:
                cross[(a, b) if a < b else (b, a)] += w
            else:
                internal[a] += w
            n += 1
            continue
        m = INTERNAL_RE.match(line)
        if m:
            internal[int(m.group(1))] += int(m.group(2))
            n += 1
    rc = proc.wait()
    if rc != 0:
        sys.exit(f"`rete summary` exited {rc}")
    log(f"parsed {n} superedge lines: {len(cross)} cross pairs, {len(internal)} internal")
    return internal, cross


def build_graph(internal, cross, max_base: int):
    """Assemble the base community graph; cap to the top `max_base` by size."""
    import igraph as ig

    # size := internal weight + total cross-degree (connectivity proxy)
    size = defaultdict(int)
    for c, w in internal.items():
        size[c] += w
    for (a, b), w in cross.items():
        size[a] += w
        size[b] += w

    kept = sorted(size, key=lambda c: size[c], reverse=True)
    dropped = 0
    if len(kept) > max_base:
        dropped = len(kept) - max_base
        kept = kept[:max_base]
        log(f"capping base communities {len(size)} -> {max_base} (dropped {dropped} smallest)")
    idx = {c: i for i, c in enumerate(kept)}

    edges, weights = [], []
    for (a, b), w in cross.items():
        if a in idx and b in idx:
            edges.append((idx[a], idx[b]))
            weights.append(w)

    g = ig.Graph(n=len(kept), edges=edges)
    g.es["weight"] = weights
    sizes = [size[c] for c in kept]
    log(f"base graph: {g.vcount()} nodes, {g.ecount()} edges")
    return g, sizes, kept, dropped


def layout_base(g):
    """2D layout of the base community graph (DRL scales to ~100k nodes)."""
    log("laying out base graph (DRL)…")
    try:
        lay = g.layout_drl(weights="weight")
    except Exception as e:  # pragma: no cover - fallback path
        log(f"DRL failed ({e}); falling back to Fruchterman-Reingold")
        lay = g.layout_fruchterman_reingold(weights="weight")
    return [(p[0], p[1]) for p in lay]


def coarsen(g):
    """Multilevel Louvain dendrogram (finest -> coarsest), the same family of
    algorithm rete's own pyramid is built with."""
    levels = g.community_multilevel(weights="weight", return_levels=True)
    log(f"Louvain dendrogram: {len(levels)} level(s) "
        f"({', '.join(str(len(set(l.membership))) for l in levels)} groups)")
    return levels


def normalizer(coords, halfx, halfy):
    """Map raw layout coords into a centered lon/lat box of half-size
    (halfx, halfy). A small box leaves empty world around the graph, so the
    map can zoom out and show the whole thing 'little'."""
    xs = [c[0] for c in coords]
    ys = [c[1] for c in coords]
    minx, maxx, miny, maxy = min(xs), max(xs), min(ys), max(ys)
    dx = (maxx - minx) or 1.0
    dy = (maxy - miny) or 1.0

    def to_lonlat(x, y):
        lon = (x - minx) / dx * (2 * halfx) - halfx
        lat = (y - miny) / dy * (2 * halfy) - halfy
        return [round(lon, 5), round(lat, 5)]

    return to_lonlat


def human(n) -> str:
    n = int(n)
    return f"{n / 1000:.1f}k" if n >= 1000 else str(n)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rete_file", help="path to the .rete file")
    ap.add_argument("--rete-bin", default="./target/release/rete")
    ap.add_argument("-o", "--output", default="experiments/graph-map/out")
    ap.add_argument("--name", default="graphmap")
    ap.add_argument("--max-base", type=int, default=60000,
                    help="cap base communities laid out (keep top-N by connectivity)")
    ap.add_argument("--zoom-pad", type=int, default=2,
                    help="extra tile zoom levels past the finest community level")
    ap.add_argument("--footprint", type=float, default=55.0,
                    help="half-width (deg) of the centered layout box; smaller => more zoom-out room")
    args = ap.parse_args()

    os.makedirs(args.output, exist_ok=True)

    internal, cross = parse_summary(args.rete_bin, args.rete_file)
    if not internal and not cross:
        sys.exit("no communities found in summary — does the file have a pyramid?")

    g, sizes, base_cids, dropped = build_graph(internal, cross, args.max_base)
    base_coords = layout_base(g)
    louvain = coarsen(g)

    # Zoom-level stack: coarsest Louvain level first (z=0) … finest Louvain …
    # then the base communities themselves as the most detailed level (max z).
    stack = list(reversed(louvain)) + [None]  # None marks the base level
    nlevels = len(stack)
    maxzoom = (nlevels - 1) + args.zoom_pad
    halfx, halfy = args.footprint, args.footprint * 0.7
    to_lonlat = normalizer(base_coords, halfx, halfy)

    # Cross edges in base-index space (only kept nodes).
    idx = {c: i for i, c in enumerate(base_cids)}
    cross_e = []
    for (a, b), w in cross.items():
        if a in idx and b in idx:
            cross_e.append((idx[a], idx[b], w))

    from shapely.geometry import MultiPoint

    xr = [c[0] for c in base_coords]
    yr = [c[1] for c in base_coords]
    eps = 0.01 * (((max(xr) - min(xr)) + (max(yr) - min(yr))) / 2 or 1.0)

    def hull_ring(idxs):
        """Convex-hull boundary (as a lon/lat ring) of a group's base members —
        the 'big node' polygon that its children sit inside."""
        geom = MultiPoint([base_coords[i] for i in idxs]).convex_hull
        if geom.geom_type != "Polygon":          # 1-2 points / collinear
            geom = geom.buffer(eps or 1e-3)
        return [list(to_lonlat(x, y)) for (x, y) in geom.exterior.coords]

    # Collected for the 3D side-elevation export (graphmap-3d.json): node
    # positions per level, intra-level edges, parent->child links between strata.
    node_xy, node_sz, level_mem, edges3d = {}, {}, {}, []

    features = []
    level_meta = []
    for z, clustering in enumerate(stack):
        if clustering is None:  # base level — one point per rete community
            for i, cid in enumerate(base_cids):
                lon, lat = to_lonlat(*base_coords[i])
                node_xy[(z, int(cid))] = [lon, lat]
                node_sz[(z, int(cid))] = int(sizes[i])
                features.append({
                    "type": "Feature",
                    "tippecanoe": {"minzoom": z, "maxzoom": maxzoom},
                    "geometry": {"type": "Point", "coordinates": [lon, lat]},
                    "properties": {"level": z, "kind": "community",
                                   "id": int(cid), "size": int(sizes[i]),
                                   "label": human(sizes[i])},
                })
            level_meta.append({"z": z, "kind": "community", "count": len(base_cids)})
            continue
        level_mem[z] = clustering.membership

        # Louvain super-level: group base nodes, then emit a boundary polygon +
        # a labelled centroid marker + aggregated inter-group edges (with ids).
        membership = clustering.membership
        groups = defaultdict(list)
        for i, gid in enumerate(membership):
            groups[gid].append(i)

        centroid = {}
        for gid, idxs in groups.items():
            wsum = sum((sizes[i] or 1) for i in idxs)
            cx = sum(base_coords[i][0] * (sizes[i] or 1) for i in idxs) / wsum
            cy = sum(base_coords[i][1] * (sizes[i] or 1) for i in idxs) / wsum
            centroid[gid] = (cx, cy)
            ssize = sum(sizes[i] for i in idxs)
            lon, lat = to_lonlat(cx, cy)
            node_xy[(z, int(gid))] = [lon, lat]
            node_sz[(z, int(gid))] = int(ssize)
            features.append({  # boundary polygon — children live inside it
                "type": "Feature",
                "tippecanoe": {"minzoom": z, "maxzoom": maxzoom},
                "geometry": {"type": "Polygon", "coordinates": [hull_ring(idxs)]},
                "properties": {"level": z, "kind": "hull", "id": int(gid),
                               "size": int(ssize), "members": len(idxs)},
            })
            features.append({  # node marker + label
                "type": "Feature",
                "tippecanoe": {"minzoom": z, "maxzoom": maxzoom},
                "geometry": {"type": "Point", "coordinates": [lon, lat]},
                "properties": {"level": z, "kind": "super", "id": int(gid),
                               "size": int(ssize), "members": len(idxs),
                               "label": human(ssize)},
            })

        eagg = defaultdict(int)
        for a, b, w in cross_e:
            ga, gb = membership[a], membership[b]
            if ga != gb:
                eagg[(ga, gb) if ga < gb else (gb, ga)] += w
        for (ga, gb), w in eagg.items():
            (lon1, lat1) = to_lonlat(*centroid[ga])
            (lon2, lat2) = to_lonlat(*centroid[gb])
            edges3d.append({"level": z, "a": int(ga), "b": int(gb), "w": int(w)})
            features.append({
                "type": "Feature",
                "tippecanoe": {"minzoom": z, "maxzoom": maxzoom},
                "geometry": {"type": "LineString",
                             "coordinates": [[lon1, lat1], [lon2, lat2]]},
                "properties": {"level": z, "kind": "edge",
                               "src": int(ga), "dst": int(gb), "weight": int(w)},
            })
        level_meta.append({"z": z, "kind": "super",
                           "count": len(groups), "edges": len(eagg)})

    # parent->child links between adjacent super levels (Louvain is nested, so a
    # finer group's base members share one coarser group). Skip the 60k base
    # level (too many lines); its plane shows as the densest stratum.
    links3d = []
    super_levels = sorted(level_mem)
    for zi in range(len(super_levels) - 1):
        zc, zf = super_levels[zi], super_levels[zi + 1]   # coarse, fine
        coarse, fine = level_mem[zc], level_mem[zf]
        seen = {}
        for base_i, fg in enumerate(fine):
            if fg not in seen:
                seen[fg] = coarse[base_i]                 # parent = coarse group
        for fg, parent in seen.items():
            links3d.append({"from": {"level": zf, "id": int(fg)},
                            "to": {"level": zc, "id": int(parent)}})

    # base communities -> their finest-super parent: connects the bottom plane
    # to the pyramid. ~60k lines, so kept separate (its own viewer toggle).
    baselinks3d = []
    if level_mem:
        finest = max(level_mem)
        base_z = nlevels - 1
        fmem = level_mem[finest]
        for i, cid in enumerate(base_cids):
            baselinks3d.append({"from": {"level": base_z, "id": int(cid)},
                                "to": {"level": finest, "id": int(fmem[i])}})

    nodes3d = [{"level": z, "id": i, "x": xy[0], "y": xy[1], "size": node_sz[(z, i)]}
               for (z, i), xy in node_xy.items()]
    with open(os.path.join(args.output, f"{args.name}-3d.json"), "w") as f:
        json.dump({"footprint": [halfx, halfy], "levels": nlevels, "nodes": nodes3d,
                   "edges": edges3d, "links": links3d, "baselinks": baselinks3d}, f)
    log(f"3D export: {len(nodes3d)} nodes, {len(edges3d)} intra-edges, "
        f"{len(links3d)} parent-links, {len(baselinks3d)} base-links")

    geojson = os.path.join(args.output, f"{args.name}.geojson")
    with open(geojson, "w") as f:
        json.dump({"type": "FeatureCollection", "features": features}, f)
    log(f"wrote {len(features)} features -> {geojson}")

    pmtiles = os.path.join(args.output, f"{args.name}.pmtiles")
    cmd = [
        "tippecanoe", "-o", pmtiles, "-l", "graph", "-n", "rete graph map",
        "--minimum-zoom=0", f"--maximum-zoom={maxzoom}",
        "--no-feature-limit", "--no-tile-size-limit",
        "--read-parallel", "--force", geojson,
    ]
    log("tippecanoe: " + " ".join(cmd))
    subprocess.run(cmd, check=True)

    meta = {
        "source": os.path.basename(args.rete_file),
        "maxzoom": maxzoom, "levels": level_meta,
        "footprint": [halfx, halfy],
        "base_communities": len(base_cids), "dropped": dropped,
    }
    with open(os.path.join(args.output, f"{args.name}.json"), "w") as f:
        json.dump(meta, f, indent=2)

    size_mb = os.path.getsize(pmtiles) / 1e6
    log(f"done: {pmtiles} ({size_mb:.1f} MB), {nlevels} zoom levels, maxzoom {maxzoom}")
    log("levels: " + json.dumps(level_meta))


if __name__ == "__main__":
    main()
