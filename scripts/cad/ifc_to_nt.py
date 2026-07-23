"""IFC (building CAD) -> N-Triples for rete.

Three layers, mirroring the z-anatomy 3D graph:
  * BOT (Building Topology Ontology, w3id.org/bot#) — Site/Building/Storey/Space/
    Element + hasStorey/hasSpace/containsElement/adjacentElement/hasSubElement.
  * cad: (w3id.org/rete/cad#) — the IFC specifics: the element's IfcClass, materials,
    quantities (area/volume/length), the door/window navigation graph.
  * geo3 (w3id.org/rete/geo3#) — each element's world-space bounding box + centroid,
    so the same 3D spatial machinery (adjacent3D / contains3D / distance3D) applies.

Instances are identified by their IFC GlobalId (stable GUIDs). Runs in the
ifcopenshell image:  python scripts/cad/ifc_to_nt.py <model.ifc> <out.nt> [modelkey]
"""
import sys, re, math
import ifcopenshell
import ifcopenshell.geom
import ifcopenshell.util.element as ue
import ifcopenshell.util.unit as uu

IFC = sys.argv[1]
OUT = sys.argv[2]
KEY = sys.argv[3] if len(sys.argv) > 3 else "model"

BOT = "https://w3id.org/bot#"
CAD = "https://w3id.org/rete/cad#"
INST = f"https://w3id.org/rete/cad/{KEY}/"
GLB_BASE = f"https://data.graphplaza.com/cad/{KEY}"   # <base>.glb (whole) + <base>-storey-<guid>.glb
GEO = "http://www.opengis.net/ont/geosparql#"
GEO3 = "https://w3id.org/rete/geo3#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
DCT = "http://purl.org/dc/terms/"
XSD = "http://www.w3.org/2001/XMLSchema#"
WKT_DT = GEO + "wktLiteral"
WKT3_DT = GEO3 + "wktLiteral3D"
BOX_DT = GEO3 + "box3dLiteral"

out = open(OUT, "w", encoding="utf-8", newline="\n")
_n = 0
def w(line):
    global _n; out.write(line + "\n"); _n += 1
def esc(s):
    return (str(s).replace("\\", "\\\\").replace('"', '\\"')
            .replace("\n", " ").replace("\r", " ").replace("\t", " "))
def iri(s): return "<" + s + ">"
def t(s, p, o): w(iri(s) + " " + iri(p) + " " + o + " .")
def tl(s, p, o, lang=None, dt=None):
    lit = '"' + esc(o) + '"'
    if lang: lit += "@" + lang
    elif dt: lit += "^^" + iri(dt)
    w(iri(s) + " " + iri(p) + " " + lit + " .")
def guid(e): return re.sub(r"[^A-Za-z0-9_]", "_", e.GlobalId)
def ent(e): return INST + guid(e)

f = ifcopenshell.open(IFC)
scale = uu.calculate_unit_scale(f)   # model length unit -> metres
print("schema", f.schema, "unit-scale-to-m", scale)

# ---------------------------------------------------------------- geometry
st = ifcopenshell.geom.settings()
st.set(st.USE_WORLD_COORDS, True)
LIMIT = 1e4  # metres — clamp stray infinite verts from half-space booleans

def world_box(e):
    try:
        g = ifcopenshell.geom.create_shape(st, e).geometry
    except Exception:
        return None
    v = g.verts
    if not v:
        return None
    mn = [math.inf] * 3; mx = [-math.inf] * 3
    ok = False
    for i in range(0, len(v), 3):
        x, y, z = v[i], v[i + 1], v[i + 2]
        if not (math.isfinite(x) and math.isfinite(y) and math.isfinite(z)):
            continue
        if abs(x) > LIMIT or abs(y) > LIMIT or abs(z) > LIMIT:
            continue
        mn[0] = min(mn[0], x); mn[1] = min(mn[1], y); mn[2] = min(mn[2], z)
        mx[0] = max(mx[0], x); mx[1] = max(mx[1], y); mx[2] = max(mx[2], z)
        ok = True
    if not ok:
        return None
    # geometry already in metres * unit? create_shape returns model units; to metres:
    return ([c * scale for c in mn], [c * scale for c in mx])

def fnum(x): return "%.3f" % x

def emit_geometry(s, box):
    mn, mx = box
    c = [(mn[i] + mx[i]) / 2 for i in range(3)]
    g = s + "/geom"
    t(s, GEO + "hasGeometry", iri(g))
    t(g, RDF, iri(GEO3 + "Geometry3D"))
    tl(g, GEO + "asWKT", "POINT Z(%s %s %s)" % (fnum(c[0]), fnum(c[1]), fnum(c[2])), dt=WKT_DT)
    tl(g, GEO3 + "asWKT3D", "POINT Z(%s %s %s)" % (fnum(c[0]), fnum(c[1]), fnum(c[2])), dt=WKT3_DT)
    tl(g, GEO3 + "box", "BOX3D(%s %s %s, %s %s %s)" % (
        fnum(mn[0]), fnum(mn[1]), fnum(mn[2]), fnum(mx[0]), fnum(mx[1]), fnum(mx[2])), dt=BOX_DT)
    for k, ax in enumerate("xyz"):
        tl(g, GEO3 + ax, fnum(c[k]), dt=XSD + "decimal")
        tl(s, GEO3 + ax, fnum(c[k]), dt=XSD + "decimal")
    return c

# ---------------------------------------------------------------- ontology header
ONT = "https://w3id.org/rete/cad"
t(ONT, RDF, iri(OWL + "Ontology"))
tl(ONT, RDFS + "label", "rete CAD/BIM vocabulary", lang="en")
tl(ONT, DCT + "description", "Building elements from an IFC model as a queryable 3D graph: "
   "BOT topology + IFC element classes + geo3 3D geometry/relations.", lang="en")
# BOT classes we use
for c, lab in [("Zone", "zone"), ("Site", "site"), ("Building", "building"),
               ("Storey", "storey"), ("Space", "space"), ("Element", "element")]:
    tl(BOT + c, RDFS + "label", lab, lang="en")
for p, lab in [("hasStorey", "has storey"), ("hasSpace", "has space"),
               ("containsElement", "contains element"), ("adjacentElement", "adjacent element"),
               ("hasSubElement", "has sub-element"), ("interfaceOf", "interface of")]:
    t(BOT + p, RDF, iri(OWL + "ObjectProperty")); tl(BOT + p, RDFS + "label", lab, lang="en")
# cad object/data properties
def objp(local, lab, com, sub=None, sym=False):
    p = CAD + local
    t(p, RDF, iri(OWL + "ObjectProperty"))
    if sym: t(p, RDF, iri(OWL + "SymmetricProperty"))
    tl(p, RDFS + "label", lab, lang="en"); tl(p, RDFS + "comment", com, lang="en")
    for s in (sub or []): t(p, RDFS + "subPropertyOf", iri(s))
objp("inStorey", "in storey", "The building storey this element sits on.")
objp("boundsSpace", "bounds space", "This element (wall/slab/…) is a boundary of a space.", sub=[BOT + "adjacentElement"])
objp("connectsSpace", "connects space", "A door/window/opening that connects two spaces (navigation).", sym=True)
objp("fillsWall", "fills opening in", "A door/window fills an opening void in this wall.")
objp("adjacentSpace", "adjacent space", "Two spaces that share a boundary element.", sym=True)
objp("glbModel", "3D model (glTF)", "A glTF/GLB 3D model of this spatial structure, for inline preview.")
for local, lab in [("ifcClass", "IFC class"), ("ifcGuid", "IFC GlobalId"), ("material", "material"),
                   ("netArea", "net area (m2)"), ("grossVolume", "gross volume (m3)"),
                   ("elevation", "elevation (m)")]:
    p = CAD + local
    t(p, RDF, iri(OWL + "DatatypeProperty")); tl(p, RDFS + "label", lab, lang="en")

# ---------------------------------------------------------------- spatial structure (BOT)
def kind_class(e):
    if e.is_a("IfcProject"): return None
    if e.is_a("IfcSite"): return BOT + "Site"
    if e.is_a("IfcBuilding"): return BOT + "Building"
    if e.is_a("IfcBuildingStorey"): return BOT + "Storey"
    if e.is_a("IfcSpace"): return BOT + "Space"
    return BOT + "Element"

def label_of(e):
    # spaces carry the room name in LongName (Name is just a number); storeys/
    # elements carry it in Name (their LongName is often an exporter GUID).
    if e.is_a("IfcSpace"):
        return getattr(e, "LongName", None) or getattr(e, "Name", None) or e.is_a()
    return getattr(e, "Name", None) or getattr(e, "LongName", None) or e.is_a()

# emit every spatial + physical product
seen = set()
def emit_node(e):
    if e.GlobalId in seen:
        return
    seen.add(e.GlobalId)
    s = ent(e)
    bc = kind_class(e)
    if bc: t(s, RDF, iri(bc))
    t(s, RDF, iri(CAD + e.is_a()))           # the specific IFC class, e.g. cad:IfcWall
    tl(s, CAD + "ifcClass", e.is_a())
    tl(s, CAD + "ifcGuid", e.GlobalId)
    tl(s, RDFS + "label", str(label_of(e)), lang="en")
    if e.is_a("IfcBuildingStorey") and getattr(e, "Elevation", None) is not None:
        tl(s, CAD + "elevation", fnum(e.Elevation * scale), dt=XSD + "decimal")
    # 3D preview models: the whole building on the Building, each floor on its Storey
    if e.is_a("IfcBuilding"):
        t(s, CAD + "glbModel", iri(GLB_BASE + ".glb"))
    if e.is_a("IfcBuildingStorey"):
        t(s, CAD + "glbModel", iri(GLB_BASE + "-storey-" + guid(e) + ".glb"))

spatial = (f.by_type("IfcSite") + f.by_type("IfcBuilding") +
           f.by_type("IfcBuildingStorey") + f.by_type("IfcSpace"))
for e in spatial:
    emit_node(e)

# containment / aggregation (BOT)
for rel in f.by_type("IfcRelAggregates"):
    parent = rel.RelatingObject
    for ch in rel.RelatedObjects:
        emit_node(parent); emit_node(ch)
        if parent.is_a("IfcBuilding") and ch.is_a("IfcBuildingStorey"):
            t(ent(parent), BOT + "hasStorey", iri(ent(ch)))
        elif ch.is_a("IfcSpace"):
            t(ent(parent), BOT + "hasSpace", iri(ent(ch)))
        else:
            t(ent(parent), BOT + "hasSubElement", iri(ent(ch)))

for rel in f.by_type("IfcRelContainedInSpatialStructure"):
    struct = rel.RelatingStructure
    for el in rel.RelatedElements:
        emit_node(struct); emit_node(el)
        if struct.is_a("IfcBuildingStorey"):
            t(ent(struct), BOT + "containsElement", iri(ent(el)))
            t(ent(el), CAD + "inStorey", iri(ent(struct)))
        elif struct.is_a("IfcSpace"):
            t(ent(struct), BOT + "containsElement", iri(ent(el)))
        else:
            t(ent(struct), BOT + "containsElement", iri(ent(el)))

# ---------------------------------------------------------------- physical elements + geometry
elems = f.by_type("IfcElement")
nbox = 0
for e in elems:
    emit_node(e)
    s = ent(e)
    # material
    mat = ue.get_material(e)
    if mat is not None:
        name = getattr(mat, "Name", None) or (mat.is_a() if hasattr(mat, "is_a") else None)
        if name: tl(s, CAD + "material", str(name))
    # geometry bbox
    b = world_box(e)
    if b:
        emit_geometry(s, b); nbox += 1

# spaces also get geometry (room volumes)
for e in f.by_type("IfcSpace"):
    b = world_box(e)
    if b:
        emit_geometry(ent(e), b); nbox += 1

# ---------------------------------------------------------------- space boundaries (topology)
space_elems = {}   # space guid -> set(element guid)
for rel in f.by_type("IfcRelSpaceBoundary"):
    sp = rel.RelatingSpace; el = rel.RelatedBuildingElement
    if sp is None or el is None:
        continue
    if sp.GlobalId in seen and el.GlobalId in seen:
        t(ent(el), CAD + "boundsSpace", iri(ent(sp)))
        space_elems.setdefault(sp.GlobalId, set()).add(el.GlobalId)
# spaces sharing a boundary element are adjacent
by_elem = {}
for spg, els in space_elems.items():
    for el in els:
        by_elem.setdefault(el, set()).add(spg)
adj = set()
for el, sps in by_elem.items():
    sps = sorted(sps)
    for i in range(len(sps)):
        for j in range(i + 1, len(sps)):
            adj.add((sps[i], sps[j]))
for a, b2 in adj:
    t(INST + re.sub(r"[^A-Za-z0-9_]", "_", a), CAD + "adjacentSpace",
      iri(INST + re.sub(r"[^A-Za-z0-9_]", "_", b2)))
    t(INST + re.sub(r"[^A-Za-z0-9_]", "_", b2), CAD + "adjacentSpace",
      iri(INST + re.sub(r"[^A-Za-z0-9_]", "_", a)))

# ---------------------------------------------------------------- openings -> door/window fills wall
for rel in f.by_type("IfcRelVoidsElement"):     # opening voids a wall
    wall = rel.RelatingBuildingElement; opening = rel.RelatedOpeningElement
    if wall is None or opening is None:
        continue
    for fr in getattr(opening, "HasFillings", []) or []:
        filler = fr.RelatedBuildingElement       # door/window
        if filler is not None and filler.GlobalId in seen and wall.GlobalId in seen:
            t(ent(filler), CAD + "fillsWall", iri(ent(wall)))

out.close()
print(f"elements={len(elems)} spaces={len(f.by_type('IfcSpace'))} boxes={nbox} "
      f"space-adjacencies={len(adj)} TRIPLES={_n} -> {OUT}")
