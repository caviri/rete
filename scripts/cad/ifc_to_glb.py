"""IFC -> GLBs with a named node per element (node name = sanitized IFC GlobalId,
matching the viewer index) for the static building explorer + playground previews.

Writes the WHOLE building to <out>.glb, plus ONE GLB per storey to
<dir>/<base>-storey-<storey-guid>.glb (so the playground can show each floor as a
distinct inline 3D preview). World coordinates, metres, Z-up (same frame as the
.rete geo3 boxes). Stray infinite vertices from half-space booleans are dropped;
each mesh is unmerged so the GLB ships flat (crisp) NORMALs — without normals a
PBR material renders black. Runs in the ifcopenshell image:
  python scripts/cad/ifc_to_glb.py <model.ifc> <out.glb>
"""
import sys, re, os, multiprocessing
import numpy as np
import ifcopenshell, ifcopenshell.geom
import ifcopenshell.util.unit as uu
import ifcopenshell.util.element as uel
import trimesh

f = ifcopenshell.open(sys.argv[1])
OUT = sys.argv[2]
OUT_DIR = os.path.dirname(OUT) or "."
BASE = os.path.splitext(os.path.basename(OUT))[0]
scale = uu.calculate_unit_scale(f)
st = ifcopenshell.geom.settings()
st.set(st.USE_WORLD_COORDS, True)
guid = lambda g: re.sub(r"[^A-Za-z0-9_]", "_", g)
LIMIT = 1e4


def storey_guid_of(elem):
    """The sanitized GlobalId of the storey an element sits on (walking up through
    a space if need be), matching the storey IRI ifc_to_nt.py emits."""
    c = uel.get_container(elem)
    hops = 0
    while c is not None and not c.is_a("IfcBuildingStorey") and hops < 6:
        c = uel.get_container(c) or uel.get_aggregate(c)
        hops += 1
    return guid(c.GlobalId) if (c is not None and c.is_a("IfcBuildingStorey")) else None


scene = trimesh.Scene()
storey_scenes = {}          # storey guid -> trimesh.Scene
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
                if len(m.faces):
                    m.unmerge_vertices()          # flat/crisp normals + a real NORMAL accessor
                    _ = m.vertex_normals
                    nm = guid(sh.guid)
                    scene.add_geometry(m, geom_name=nm, node_name=nm)
                    n += 1
                    try:
                        sg = storey_guid_of(f.by_guid(sh.guid))
                    except Exception:
                        sg = None
                    if sg:
                        # share the same mesh (its normals are already computed) —
                        # a copy would drop the normal cache and re-export black
                        storey_scenes.setdefault(sg, trimesh.Scene()).add_geometry(
                            m, geom_name=nm, node_name=nm)
        if not it.next():
            break

scene.export(OUT)
print(f"meshes={n}  ->  {OUT}  ({os.path.getsize(OUT)/1e6:.2f} MB)")
for sg, sc in storey_scenes.items():
    p = os.path.join(OUT_DIR, f"{BASE}-storey-{sg}.glb")
    sc.export(p)
    print(f"  storey {sg}: {len(sc.geometry)} meshes -> {p} ({os.path.getsize(p)/1e6:.2f} MB)")
