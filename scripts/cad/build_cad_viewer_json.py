"""Compact index for the static building explorer (docs/building.html), parsed
from the IFC .nt. Node key `n` = the sanitized IFC GUID = the GLB node name = the
last path segment of the element IRI (so SPARQL result IRIs map straight to meshes).

  python build_cad_viewer_json.py <model.nt> <out.json> <glb-url>
"""
import json, re, sys

NT, OUT, GLB = sys.argv[1], sys.argv[2], sys.argv[3]
BOT = "https://w3id.org/bot#"; CAD = "https://w3id.org/rete/cad#"; G3 = "https://w3id.org/rete/geo3#"
LBL = "http://www.w3.org/2000/01/rdf-schema#label"; TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
tri = re.compile(r'^<([^>]+)>\s+<([^>]+)>\s+(.*)\s\.$')
lit = lambda o: (re.match(r'^"(.*)"(?:@[\w-]+|\^\^<.*>)?$', o) or [None, None])[1]

label, ifcclass, kind, elev = {}, {}, {}, {}
cx, cy, cz, half = {}, {}, {}, {}
material, area = {}, {}
instorey, adjspace, bounds, storey_of, connects = {}, {}, {}, {}, {}

for line in open(NT, encoding="utf-8"):
    m = tri.match(line.rstrip("\n"))
    if not m: continue
    s, p, o = m.group(1), m.group(2), m.group(3)
    if p == LBL and o.startswith('"'): label[s] = lit(o)
    elif p == TYPE:
        ot = o.strip("<>")
        if ot == BOT + "Space": kind[s] = "Space"
        elif ot == BOT + "Storey": kind[s] = "Storey"
        elif ot == BOT + "Building": kind[s] = "Building"
        elif ot == BOT + "Element": kind.setdefault(s, "Element")
    elif p == CAD + "ifcClass": ifcclass[s] = lit(o)
    elif p == CAD + "elevation": elev[s] = float(lit(o))
    elif p == CAD + "inStorey": instorey[s] = o.strip("<>")
    elif p == BOT + "hasSpace": storey_of[o.strip("<>")] = s   # storey -> space (aggregation)
    elif p == CAD + "adjacentSpace": adjspace.setdefault(s, []).append(o.strip("<>"))
    elif p == CAD + "boundsSpace": bounds.setdefault(s, []).append(o.strip("<>"))
    elif p == CAD + "connectsSpace": connects.setdefault(s, []).append(o.strip("<>"))
    elif p == CAD + "material" and o.startswith('"'):
        v = lit(o) or ""
        if v and not re.match(r"(Ifc|Radial|Solid|Default|<Unnamed)", v):  # skip exporter artifacts
            material[s] = v
    elif p == CAD + "netArea": area[s] = float(lit(o))
    elif p == G3 + "x": cx[s] = float(lit(o))
    elif p == G3 + "y": cy[s] = float(lit(o))
    elif p == G3 + "z": cz[s] = float(lit(o))
    elif p == G3 + "box" and s.endswith("/geom"):
        mm = re.match(r"BOX3D\(([-.\d ]+),\s*([-.\d ]+)\)", lit(o) or "")
        if mm:
            mn = [float(x) for x in mm.group(1).split()]; mx = [float(x) for x in mm.group(2).split()]
            half[s[:-5]] = [round((mx[i] - mn[i]) / 2, 3) for i in range(3)]

guid = lambda iri: iri.rsplit("/", 1)[-1]
storeys = sorted([s for s, k in kind.items() if k == "Storey"], key=lambda s: elev.get(s, 0))
sidx = {s: i for i, s in enumerate(storeys)}
storey_json = [{"l": label.get(s, guid(s)), "el": elev.get(s, 0)} for s in storeys]

# nodes = anything with geometry (elements + spaces)
node_iris = [s for s in half if s in cx]
gidx = {s: i for i, s in enumerate(node_iris)}
classes = sorted({ifcclass[s] for s in node_iris if s in ifcclass})

def glist(m, s):
    return sorted({gidx[o] for o in m.get(s, []) if o in gidx})

nodes = []
for s in node_iris:
    nd = {
        "n": guid(s), "l": label.get(s, guid(s)),
        "c": ifcclass.get(s, ""), "k": kind.get(s, "Element"),
        "st": sidx.get(instorey.get(s, storey_of.get(s, "")), -1),
        "cc": [round(cx[s], 3), round(cy[s], 3), round(cz[s], 3)],
        "b": half.get(s, [0.1, 0.1, 0.1]),
        "adj": glist(adjspace, s),      # adjacent spaces (for a space)
        "bd": glist(bounds, s),         # spaces this element bounds
        "cn": glist(connects, s),       # spaces a door connects
    }
    if s in material: nd["mt"] = material[s]
    if s in area: nd["ar"] = round(area[s], 2)
    nodes.append(nd)
out = {"glb": GLB, "classes": classes, "storeys": storey_json, "nodes": nodes}
json.dump(out, open(OUT, "w", encoding="utf-8"), separators=(",", ":"), ensure_ascii=False)
print(f"nodes={len(nodes)} classes={len(classes)} storeys={len(storeys)} -> {OUT}")
