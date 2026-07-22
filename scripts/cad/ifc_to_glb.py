"""IFC -> one Draco-free GLB with a named node per element (node name = sanitized
IFC GlobalId, matching the viewer index) for the static building explorer.

World coordinates, metres, Z-up (same frame as the .rete geo3 boxes). Stray
infinite vertices from half-space booleans are dropped (faces referencing an
out-of-range vertex are removed). Runs in the ifcopenshell image:
  python scripts/cad/ifc_to_glb.py <model.ifc> <out.glb>
"""
import sys, re, multiprocessing
import numpy as np
import ifcopenshell, ifcopenshell.geom
import ifcopenshell.util.unit as uu
import trimesh

f = ifcopenshell.open(sys.argv[1])
OUT = sys.argv[2]
scale = uu.calculate_unit_scale(f)
st = ifcopenshell.geom.settings()
st.set(st.USE_WORLD_COORDS, True)
guid = lambda g: re.sub(r"[^A-Za-z0-9_]", "_", g)
LIMIT = 1e4

scene = trimesh.Scene()
n = 0
it = ifcopenshell.geom.iterator(st, f, max(1, multiprocessing.cpu_count() - 1))
if it.initialize():
    while True:
        sh = it.get()
        verts = np.asarray(sh.geometry.verts, dtype=np.float64).reshape(-1, 3) * scale
        faces = np.asarray(sh.geometry.faces, dtype=np.int64).reshape(-1, 3)
        if len(verts) and len(faces):
            good = np.all(np.abs(verts) < LIMIT, axis=1)
            faces = faces[good[faces].all(axis=1)]
            if len(faces):
                m = trimesh.Trimesh(vertices=verts, faces=faces, process=False)
                m.remove_unreferenced_vertices()
                if len(m.vertices):
                    nm = guid(sh.guid)
                    scene.add_geometry(m, geom_name=nm, node_name=nm)
                    n += 1
        if not it.next():
            break
scene.export(OUT)
import os
print(f"meshes={n}  ->  {OUT}  ({os.path.getsize(OUT)/1e6:.2f} MB)")
