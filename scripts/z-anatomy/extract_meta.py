"""Extract Z-Anatomy structure metadata from the FBX system files (headless Blender).

For every FBX (one per body system) we walk the object graph and, per *canonical
structure*, record: display label, side (l/r), system, parent (partonomy), role
(structure with real geometry / region group / label-only landmark), world-space
AABB (min/max, metres), centroid, and total vertex count.

Object-name conventions in Z-Anatomy:
  <name>            real organ mesh (many verts)   e.g. "Left lobe of thymus", "Kidney.l"
  <name>.g          group / hierarchy empty        e.g. "Thymus.g"
  <name>.t          text-label anchor empty        e.g. "Hilum of spleen.t"
  <name>.j          8-vert label leader-line mesh  e.g. "Thymus.j"   (not anatomy)
  <name>.r / .l     right / left side (kept as identity)
  <name>.001        Blender duplicate-name marker  (stripped, logged)

Coordinates: metres, Z-up (Z superior, X medial-lateral, Y antero-posterior).
All 9 FBX share the same world origin, so cross-system geometry is comparable.

Run (in the rete-blender image):
  blender -b -noaudio --python extract_meta.py -- <out_dir> <fbx1> [<fbx2> ...]
Writes <out_dir>/<system>.jsonl (one JSON record per canonical structure) and a
<out_dir>/_extract_summary.json.
"""
import bpy, sys, os, re, json, mathutils
from collections import defaultdict

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
out_dir = argv[0]
fbx_files = argv[1:]
os.makedirs(out_dir, exist_ok=True)

DUP = re.compile(r"^(.*)\.(\d{3})$")
ROLE_SUFFIX = ("g", "t", "j")
BIG = (1e18, 1e18, 1e18)
NEG = (-1e18, -1e18, -1e18)


def split_name(n):
    """(canonical_id, display_label, role, side). canonical keeps .r/.l; display drops it."""
    base = n
    m = DUP.match(base)
    if m:
        base = m.group(1)
    role = None
    for r in ROLE_SUFFIX:
        if base.endswith("." + r):
            role = r
            base = base[:-2]
            break
    side = None
    display = base
    if base.endswith(".r"):
        side = "right"; display = base[:-2]
    elif base.endswith(".l"):
        side = "left"; display = base[:-2]
    return base, display, role, side


def world_aabb(obj):
    mn = list(BIG); mx = list(NEG)
    for c in obj.bound_box:
        w = obj.matrix_world @ mathutils.Vector(c)
        for i in range(3):
            mn[i] = min(mn[i], w[i]); mx[i] = max(mx[i], w[i])
    return mn, mx


def union(a_mn, a_mx, b_mn, b_mx):
    return ([min(a_mn[i], b_mn[i]) for i in range(3)],
            [max(a_mx[i], b_mx[i]) for i in range(3)])


summary = {}
for fbx in fbx_files:
    system = re.sub(r"\d+$", "", os.path.splitext(os.path.basename(fbx))[0]).strip()
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.fbx(filepath=fbx)
    objs = list(bpy.context.scene.objects)

    # object name -> canonical, for parent resolution
    obj_canon = {}
    for o in objs:
        canon, _disp, _role, _side = split_name(o.name)
        obj_canon[o.name] = canon

    rec = {}  # canonical -> record

    def get(canon, display, side):
        if canon not in rec:
            rec[canon] = {
                "id": canon, "label": display, "side": side, "system": system,
                "parents": set(), "mesh_min": None, "mesh_max": None,
                "label_pos": None, "verts": 0, "has_mesh": False, "n_meshes": 0,
            }
        return rec[canon]

    for o in objs:
        canon, display, role, side = split_name(o.name)
        r = get(canon, display, side)
        # parent (partonomy)
        if o.parent is not None:
            pc = obj_canon.get(o.parent.name)
            if pc and pc != canon:
                r["parents"].add(pc)
        if o.type == "MESH":
            nv = len(o.data.vertices)
            if role == "j" or nv < 12:
                # label leader-line: use its centroid only as a position fallback
                mn, mx = world_aabb(o)
                r["label_pos"] = [(mn[i] + mx[i]) / 2 for i in range(3)]
            else:
                mn, mx = world_aabb(o)
                if r["mesh_min"] is None:
                    r["mesh_min"], r["mesh_max"] = mn, mx
                else:
                    r["mesh_min"], r["mesh_max"] = union(r["mesh_min"], r["mesh_max"], mn, mx)
                r["verts"] += nv
                r["has_mesh"] = True
                r["n_meshes"] += 1
        elif o.type == "EMPTY":
            if role == "t" or r["label_pos"] is None:
                # text-anchor position (only if we don't already have geometry-derived)
                w = o.matrix_world.translation
                r["label_pos"] = [w.x, w.y, w.z]

    # write
    path = os.path.join(out_dir, system.replace(" ", "_") + ".jsonl")
    n = 0
    with open(path, "w", encoding="utf-8") as fh:
        for canon, r in rec.items():
            r["parents"] = sorted(r["parents"])
            r["role"] = ("structure" if r["has_mesh"]
                         else ("landmark" if r["label_pos"] else "group"))
            fh.write(json.dumps(r, ensure_ascii=False) + "\n")
            n += 1
    summary[system] = {
        "file": os.path.basename(path), "objects": len(objs), "canonical": n,
        "with_mesh": sum(1 for r in rec.values() if r["has_mesh"]),
    }
    print(f"=== {system}: {len(objs)} objs -> {n} canonical, "
          f"{summary[system]['with_mesh']} with mesh -> {path}")

with open(os.path.join(out_dir, "_extract_summary.json"), "w", encoding="utf-8") as fh:
    json.dump(summary, fh, indent=2)
print("=== SUMMARY:", json.dumps(summary))
