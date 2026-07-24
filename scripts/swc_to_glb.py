#!/usr/bin/env python3
"""Convert SWC neuron skeletons to a coherent tube-mesh GLB for web preview.

The tree is split into unbranched PATHS; each path is swept into one continuous
tube with a parallel-transport frame (consecutive cross-sections stay aligned),
so it reads as a smooth neurite rather than a stack of disks. Radii vary per
node (clamped to a visible minimum). LOSSY preview — keep the SWC as truth.

  python swc_to_glb.py <out_dir> <sides> <swc1> [<swc2> ...]
"""
import sys
from collections import defaultdict
from pathlib import Path
import numpy as np
import trimesh

MIN_R_FRAC = 0.006   # min tube radius as a fraction of the skeleton's extent

def parse_swc(path):
    xyz, rad, par, idx = [], [], [], {}
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        p = line.split()
        if len(p) < 7:
            continue
        idx[int(p[0])] = len(xyz)
        xyz.append((float(p[2]), float(p[3]), float(p[4])))
        rad.append(float(p[5])); par.append(int(p[6]))
    xyz = np.asarray(xyz, float); rad = np.asarray(rad, float)
    edges = [(i, idx[pp]) for i, pp in enumerate(par) if pp != -1 and pp in idx]
    return xyz, rad, edges

def build_paths(n, edges):
    """Split the tree into maximal unbranched chains; each starts at a root or a
    branch node (included) so adjacent tubes overlap at junctions."""
    children = defaultdict(list); has_parent = set()
    for c, p in edges:
        children[p].append(c); has_parent.add(c)
    def walk(start, first):
        chain = [start, first]; cur = first
        while True:
            ch = children.get(cur, [])
            if len(ch) == 1:
                cur = ch[0]; chain.append(cur)
            else:
                break
        return chain
    paths = []
    for node in range(n):
        ch = children.get(node, [])
        if (node not in has_parent or len(ch) > 1):
            for c in ch:
                paths.append(walk(node, c))
    return paths

def path_tube(P, R, sides):
    nseg = len(P)
    if nseg < 2:
        return None
    seg = P[1:] - P[:-1]
    sl = np.linalg.norm(seg, axis=1, keepdims=True); sl[sl == 0] = 1
    sd = seg / sl
    T = np.zeros((nseg, 3)); T[0] = sd[0]; T[-1] = sd[-1]
    if nseg > 2:
        T[1:-1] = sd[:-1] + sd[1:]
    tl = np.linalg.norm(T, axis=1, keepdims=True); tl[tl == 0] = 1; T = T / tl
    # parallel-transport an initial normal along the path
    N = np.zeros((nseg, 3))
    a = np.array([0, 0, 1.0]) if abs(T[0, 2]) < 0.9 else np.array([0, 1.0, 0])
    n0 = np.cross(T[0], a); N[0] = n0 / (np.linalg.norm(n0) or 1)
    for i in range(1, nseg):
        v = np.cross(T[i - 1], T[i]); s = np.linalg.norm(v); c = np.dot(T[i - 1], T[i])
        Ni = N[i - 1]
        if s > 1e-9:
            v = v / s; ang = np.arctan2(s, c)
            Ni = (Ni * np.cos(ang) + np.cross(v, Ni) * np.sin(ang)
                  + v * np.dot(v, Ni) * (1 - np.cos(ang)))
        Ni = Ni - np.dot(Ni, T[i]) * T[i]
        N[i] = Ni / (np.linalg.norm(Ni) or 1)
    Bn = np.cross(T, N)
    th = np.linspace(0, 2 * np.pi, sides, endpoint=False)
    ca, sa = np.cos(th), np.sin(th)
    rings = P[:, None, :] + R[:, None, None] * (
        N[:, None, :] * ca[None, :, None] + Bn[:, None, :] * sa[None, :, None])
    verts = rings.reshape(-1, 3)
    s = np.arange(sides); sn = (s + 1) % sides
    faces = []
    for i in range(nseg - 1):
        b, t = i * sides, (i + 1) * sides
        faces.append(np.stack([b + s, b + sn, t + sn], axis=-1))
        faces.append(np.stack([b + s, t + sn, t + s], axis=-1))
    return verts, np.concatenate(faces, axis=0)

def swc_glb(xyz, rad, edges, sides):
    extent = np.linalg.norm(xyz.max(0) - xyz.min(0)) if len(xyz) else 1.0
    R = np.clip(rad, extent * MIN_R_FRAC, None)
    V, F = [], []
    off = 0
    for chain in build_paths(len(xyz), edges):
        idx = np.asarray(chain)
        out = path_tube(xyz[idx], R[idx], sides)
        if out is None:
            continue
        v, f = out
        V.append(v); F.append(f + off); off += len(v)
    if not V:
        return None
    m = trimesh.Trimesh(vertices=np.concatenate(V), faces=np.concatenate(F), process=False)
    _ = m.vertex_normals  # force smooth normals so the exported GLB carries NORMAL
    return m

def main():
    out_dir = Path(sys.argv[1]); out_dir.mkdir(parents=True, exist_ok=True)
    sides = int(sys.argv[2])
    for swc in sys.argv[3:]:
        name = Path(swc).stem
        xyz, rad, edges = parse_swc(swc)
        m = swc_glb(xyz, rad, edges, sides)
        if m is None:
            print(f"  SKIP {name}"); continue
        dest = out_dir / f"{name}.glb"; m.export(dest)
        print(f"  {name}: {len(xyz):,} nodes -> {len(m.vertices):,} verts, "
              f"{dest.stat().st_size/1e6:.2f} MB GLB", flush=True)
    print("=== SWC->GLB DONE")

if __name__ == "__main__":
    main()
