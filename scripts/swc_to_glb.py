#!/usr/bin/env python3
"""Convert SWC neuron skeletons to lightweight tube-mesh GLB for web preview.

Each SWC edge (node -> parent) becomes a low-poly cylinder; all edges are built
vectorised with numpy into one mesh and exported as GLB (trimesh). LOSSY preview
of a skeleton reconstruction — keep the SWC as the analytical source of truth.

  python swc_to_glb.py <out_dir> <sides> <swc1> [<swc2> ...]

Coordinates are used as-is (viewer auto-frames); radii are clamped to a visible
minimum relative to the skeleton's extent so thin neurites stay renderable.
"""
import sys
from pathlib import Path
import numpy as np
import trimesh

def parse_swc(path):
    ids, xyz, rad, par = [], [], [], []
    idx = {}
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        p = line.split()
        if len(p) < 7:
            continue
        nid = int(p[0])
        idx[nid] = len(ids)
        ids.append(nid)
        xyz.append((float(p[2]), float(p[3]), float(p[4])))
        rad.append(float(p[5]))
        par.append(int(p[6]))
    xyz = np.array(xyz, dtype=np.float64)
    rad = np.array(rad, dtype=np.float64)
    # edges: (child_row, parent_row) for nodes with a valid parent
    e = [(i, idx[pp]) for i, pp in enumerate(par) if pp != -1 and pp in idx]
    return xyz, rad, np.array(e, dtype=np.int64)

def tube_mesh(xyz, rad, edges, sides=7, min_r_frac=0.012):
    p0 = xyz[edges[:, 1]]  # parent end
    p1 = xyz[edges[:, 0]]  # child end
    r = np.maximum(rad[edges[:, 0]], rad[edges[:, 1]])
    extent = np.linalg.norm(xyz.max(0) - xyz.min(0))
    r = np.clip(r, extent * min_r_frac, None)  # keep thin neurites visible

    d = p1 - p0
    L = np.linalg.norm(d, axis=1, keepdims=True)
    L[L == 0] = 1.0
    d = d / L
    # per-edge orthonormal frame perpendicular to d
    a = np.tile(np.array([1.0, 0.0, 0.0]), (len(d), 1))
    flip = np.abs(d[:, 0]) > 0.9
    a[flip] = np.array([0.0, 1.0, 0.0])
    u = np.cross(d, a); u /= np.linalg.norm(u, axis=1, keepdims=True)
    v = np.cross(d, u)

    ang = np.linspace(0, 2 * np.pi, sides, endpoint=False)
    ca, sa = np.cos(ang), np.sin(ang)
    # ring offsets per edge per side: (E, sides, 3)
    ring = (u[:, None, :] * ca[None, :, None] + v[:, None, :] * sa[None, :, None]) * r[:, None, None]
    v0 = p0[:, None, :] + ring   # (E, sides, 3)
    v1 = p1[:, None, :] + ring
    E = len(edges)
    verts = np.concatenate([v0, v1], axis=1).reshape(-1, 3)  # per edge: sides bottom + sides top

    # side faces (two triangles per quad) indexing into each edge's 2*sides block
    base = (np.arange(E) * (2 * sides))[:, None]
    s = np.arange(sides)
    sn = (s + 1) % sides
    b0 = base + s; b1 = base + sn
    t0 = base + sides + s; t1 = base + sides + sn
    f1 = np.stack([b0, b1, t1], axis=-1).reshape(-1, 3)
    f2 = np.stack([b0, t1, t0], axis=-1).reshape(-1, 3)
    faces = np.concatenate([f1, f2], axis=0)
    return trimesh.Trimesh(vertices=verts, faces=faces, process=False)

def main():
    out_dir = Path(sys.argv[1]); out_dir.mkdir(parents=True, exist_ok=True)
    sides = int(sys.argv[2])
    for swc in sys.argv[3:]:
        name = Path(swc).stem
        xyz, rad, edges = parse_swc(swc)
        if len(edges) == 0:
            print(f"  SKIP {name}: no edges"); continue
        m = tube_mesh(xyz, rad, edges, sides=sides)
        dest = out_dir / f"{name}.glb"
        m.export(dest)
        print(f"  {name}: {len(xyz):,} nodes / {len(edges):,} edges -> "
              f"{len(m.vertices):,} verts, {dest.stat().st_size/1e6:.2f} MB GLB", flush=True)
    print("=== SWC->GLB DONE")

if __name__ == "__main__":
    main()
