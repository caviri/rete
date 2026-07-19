"""Emit the Z-Anatomy 3D knowledge graph as N-Triples.

Combines:
  - structures + partonomy + geometry (from derived/*.jsonl, geometry propagated
    up the part hierarchy so group nodes get a bounding box)
  - multilingual labels, descriptions, tissue class, region membership (aux.json)
  - materialized 3D spatial relations computed from the mesh AABBs:
      geo3:adjacent3D          symmetric — surfaces in contact (gap <= GAP_MM)
      anat:tissueContinuousWith  adjacency between same-tissue structures  ("same tissue")
      geo3:thermallyCoupledWith  adjacency where a perfused/metabolic tissue is
                                 involved — an illustrative typed physical relation
                                 layered on the spatial substrate ("even heat")
  - GeoSPARQL geometry (geo:asWKT POINT Z, 2D-projection-safe) PLUS a real 3D
    serialization (geo3:asWKT3D / geo3:box) + numeric cx/cy/cz for plain-SPARQL 3D math
  - lexical cross-links to HPO phenotypes + Disease Ontology diseases (bridge.json)

Coordinates: millimetres, Cartesian anatomical frame  +X=left  +Y=anterior  +Z=superior.
"""
import json, glob, os, re, sys
import numpy as np

DERIVED = "data/z-anatomy/derived"
OUT = "data/z-anatomy/z-anatomy.nt"
MESH_BASE = "https://data.graphplaza.com/z-anatomy/glb/"

# namespaces
ANAT = "https://w3id.org/rete/anatomy#"
ASTR = "https://w3id.org/rete/anatomy/structure/"
ASYS = "https://w3id.org/rete/anatomy/system/"
AREG = "https://w3id.org/rete/anatomy/region/"
GEO = "http://www.opengis.net/ont/geosparql#"
GEO3 = "https://w3id.org/rete/geo3#"
OBO = "http://purl.obolibrary.org/obo/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
DCT = "http://purl.org/dc/terms/"
XSD = "http://www.w3.org/2001/XMLSchema#"

WKT_DT = GEO + "wktLiteral"
WKT3_DT = GEO3 + "wktLiteral3D"
BOX_DT = GEO3 + "box3dLiteral"

GAP_MM = 3.0      # surfaces within 3 mm are "in contact"
DEG_CAP = 32      # keep at most this many nearest contacts per structure
PERFUSED = {"artery", "vein", "muscle", "viscus"}  # heat sources for thermal coupling

SYS_CODE = {
    "SkeletalSystem": "skel", "MuscularSystem": "musc", "NervousSystem": "nerv",
    "CardioVascular": "card", "Joints": "jnt", "VisceralSystem": "visc",
    "LymphoidOrgans": "lymph", "Regions of human body": "reg", "References": "ref",
}
TISSUE_CLASS = {
    "bone": "Bone", "muscle": "Muscle", "nerve": "Nerve", "artery": "Artery",
    "vein": "Vein", "ligament": "Ligament", "viscus": "Viscus",
    "lymphoid": "LymphoidStructure", "fascia": "Fascia", "skin": "Skin",
}

# ---------------------------------------------------------------- helpers
out = open(OUT, "w", encoding="utf-8", newline="\n")
_n = 0
def w(line):
    global _n
    out.write(line + "\n"); _n += 1
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

def strip_side(n):
    return n[:-2] if (n.endswith(".r") or n.endswith(".l")) else n

_slug_used = {}
def slug(system, sid):
    code = SYS_CODE.get(system, "x")
    base = re.sub(r"[^A-Za-z0-9]+", "_", sid).strip("_") or "n"
    key = code + "/" + base
    if key in _slug_used and _slug_used[key] != (system, sid):
        i = 2
        while (code + "/" + base + "_" + str(i)) in _slug_used:
            i += 1
        key = code + "/" + base + "_" + str(i)
    _slug_used[key] = (system, sid)
    return ASTR + key

# ---------------------------------------------------------------- load
aux = json.load(open(os.path.join(DERIVED, "aux.json"), encoding="utf-8"))
bridge = json.load(open(os.path.join(DERIVED, "bridge.json"), encoding="utf-8"))
tr, desc, tissue, regions = aux["translations"], aux["descriptions"], aux["tissue"], aux["regions"]

# Z-Anatomy muscle/bone attachment-marker suffixes (origin/insertion decals):
#   <base>.o[n][l|r]  <base>.e[n][l|r]  <base>.i  <base>.s   -> fold into <base>,
# taking the structure's side from the marker's trailing l/r. These are rendering
# footprints, not separate organs; folding them reconstructs attachment-only muscles
# (Masseter, Quadriceps, …) and keeps the true left/right split.
MARKER = re.compile(r"^(.*)\.((?:o|e|i|s)\d*)([lr]?)$")

def fold_id(sid):
    m = MARKER.match(sid)
    if not m:
        return sid
    base, _att, side = m.group(1), m.group(2), m.group(3)
    if base.endswith(".r") or base.endswith(".l"):
        return base                      # base already carries a side
    return base + ("." + side if side else "")

def merge_rec(dst, src):
    if src.get("mesh_min"):
        if not dst.get("mesh_min"):
            dst["mesh_min"], dst["mesh_max"] = list(src["mesh_min"]), list(src["mesh_max"])
        else:
            dst["mesh_min"] = [min(dst["mesh_min"][i], src["mesh_min"][i]) for i in range(3)]
            dst["mesh_max"] = [max(dst["mesh_max"][i], src["mesh_max"][i]) for i in range(3)]
    if src.get("label_pos") and not dst.get("label_pos"):
        dst["label_pos"] = src["label_pos"]
    dst["verts"] = dst.get("verts", 0) + src.get("verts", 0)
    dst["has_mesh"] = dst.get("has_mesh") or src.get("has_mesh")
    dst["parents"] = sorted(set(dst.get("parents", [])) | set(src.get("parents", [])))

structs = {}          # key (system,id) -> record  (markers folded into base)
raw_n = 0
for jf in sorted(glob.glob(os.path.join(DERIVED, "*.jsonl"))):
    for line in open(jf, encoding="utf-8"):
        r = json.loads(line); raw_n += 1
        fid = fold_id(r["id"])
        key = (r["system"], fid)
        if fid != r["id"]:
            r["side"] = "left" if fid.endswith(".l") else ("right" if fid.endswith(".r") else r.get("side"))
        r["id"] = fid
        if key in structs:
            merge_rec(structs[key], r)
        else:
            structs[key] = r
# re-point parents through the fold and drop self/broken parents
for (sysm, sid), r in structs.items():
    r["parents"] = sorted({fold_id(p) for p in r.get("parents", []) if fold_id(p) != sid})
    r["role"] = "structure" if r["has_mesh"] else ("landmark" if r.get("label_pos") else "group")
print(f"structures: {len(structs)} (folded from {raw_n} raw canonical records)")

# assign IRIs and index by (system,id)
for (sysm, sid), r in structs.items():
    r["iri"] = slug(sysm, sid)
    r["disp"] = strip_side(sid)

# resolve parents to IRIs (parents are canonical ids in the same system)
for (sysm, sid), r in structs.items():
    r["parent_iris"] = []
    for p in r.get("parents", []):
        pr = structs.get((sysm, p))
        if pr:
            r["parent_iris"].append(pr["iri"])

# ---------------------------------------------------------------- geometry propagation (mm)
def mm(v): return [x * 1000.0 for x in v]
# children map for bottom-up union
children = {}
for (sysm, sid), r in structs.items():
    for p in r.get("parents", []):
        children.setdefault((sysm, p), []).append((sysm, sid))

def own_box(r):
    if r.get("mesh_min"):
        return mm(r["mesh_min"]), mm(r["mesh_max"])
    if r.get("label_pos"):
        p = mm(r["label_pos"]); return list(p), list(p)
    return None

_box_memo = {}
def eff_box(key, seen=None):
    if key in _box_memo:
        return _box_memo[key]
    seen = seen or set()
    if key in seen:
        return None
    seen.add(key)
    r = structs[key]
    mn = mx = None
    ob = own_box(r)
    if ob:
        mn, mx = list(ob[0]), list(ob[1])
    for ck in children.get(key, []):
        cb = eff_box(ck, seen)
        if cb:
            if mn is None:
                mn, mx = list(cb[0]), list(cb[1])
            else:
                mn = [min(mn[i], cb[0][i]) for i in range(3)]
                mx = [max(mx[i], cb[1][i]) for i in range(3)]
    res = (mn, mx) if mn is not None else None
    _box_memo[key] = res
    return res

for key, r in structs.items():
    b = eff_box(key)
    r["box"] = b
    # display/relation geometry: a structure's OWN mesh box is the accurate surface
    # extent; only pure group/landmark nodes fall back to the descendant union.
    if r.get("has_mesh") and r.get("mesh_min"):
        r["gbox"] = (mm(r["mesh_min"]), mm(r["mesh_max"]))
    else:
        r["gbox"] = b
    r["gctr"] = ([(r["gbox"][0][i] + r["gbox"][1][i]) / 2 for i in range(3)]
                 if r["gbox"] else None)

# ---------------------------------------------------------------- classes / properties (vocab)
def cls(local, label, comment, sub=None):
    c = ANAT + local
    t(c, RDF, iri(OWL + "Class")); tl(c, RDFS + "label", label, lang="en")
    tl(c, RDFS + "comment", comment, lang="en")
    for s in (sub or []):
        t(c, RDFS + "subClassOf", iri(s))

def objp(iri_, label, comment, sub=None, symmetric=False, dom=None, rng=None):
    t(iri_, RDF, iri(OWL + "ObjectProperty"))
    if symmetric: t(iri_, RDF, iri(OWL + "SymmetricProperty"))
    tl(iri_, RDFS + "label", label, lang="en"); tl(iri_, RDFS + "comment", comment, lang="en")
    for s in (sub or []): t(iri_, RDFS + "subPropertyOf", iri(s))
    if dom: t(iri_, RDFS + "domain", iri(dom))
    if rng: t(iri_, RDFS + "range", iri(rng))

# ontology header
ONT = "https://w3id.org/rete/anatomy"
t(ONT, RDF, iri(OWL + "Ontology"))
tl(ONT, RDFS + "label", "Z-Anatomy 3D Knowledge Graph vocabulary", lang="en")
tl(ONT, DCT + "description",
   "Anatomical structures of the human body with 3D geometry and spatial relations, "
   "extending GeoSPARQL to three dimensions (geo3). Structures derived from the "
   "Z-Anatomy project (CC BY-SA 4.0); phenotype/disease links from HPO and the "
   "Human Disease Ontology.", lang="en")

cls("AnatomicalStructure", "anatomical structure",
    "A material anatomical structure of the human body; also a geo:Feature carrying 3D geometry.",
    sub=[GEO + "Feature", OBO + "UBERON_0000061"])
cls("BodySystem", "body system", "A major organ system (skeletal, muscular, nervous, …).",
    sub=[ANAT + "AnatomicalStructure"])
cls("Region", "body region", "A topographic region of the body (head, thorax, limb, …).",
    sub=[ANAT + "AnatomicalStructure"])
cls("Landmark", "anatomical landmark",
    "A named point/feature on a structure represented by a label anchor, without its own volume.",
    sub=[ANAT + "AnatomicalStructure"])
for loc, lab in TISSUE_CLASS.items():
    human = {"LymphoidStructure": "lymphoid structure"}.get(lab, lab.lower())
    cls(lab, human, f"An anatomical structure of tissue class '{human}'.",
        sub=[ANAT + "AnatomicalStructure"])
# geometry class
t(GEO3 + "Geometry3D", RDF, iri(OWL + "Class"))
t(GEO3 + "Geometry3D", RDFS + "subClassOf", iri(GEO + "Geometry"))
tl(GEO3 + "Geometry3D", RDFS + "label", "3D geometry", lang="en")
tl(GEO3 + "Geometry3D", RDFS + "comment",
   "A 3D geometry in the z-anatomy Cartesian frame (millimetres; +X=left, +Y=anterior, "
   "+Z=superior). geo:asWKT carries a POINT Z whose Z is ignored by 2D GeoSPARQL "
   "(a transverse-plane projection); geo3:asWKT3D / geo3:box carry the full 3D form.", lang="en")

# object properties
objp(ANAT + "partOf", "part of", "This structure is anatomically part of another.",
     sub=[OBO + "BFO_0000050"], dom=ANAT + "AnatomicalStructure", rng=ANAT + "AnatomicalStructure")
objp(ANAT + "hasPart", "has part", "Inverse of anat:partOf.", sub=[OBO + "BFO_0000051"])
t(ANAT + "partOf", OWL + "inverseOf", iri(ANAT + "hasPart"))
objp(ANAT + "inSystem", "in system", "The body system this structure belongs to.",
     rng=ANAT + "BodySystem")
objp(ANAT + "inRegion", "in region", "A body region this structure belongs to.",
     rng=ANAT + "Region")
objp(GEO3 + "adjacent3D", "adjacent in 3D",
     f"The two structures' surfaces are in contact in 3D (bounding boxes within {GAP_MM:g} mm). "
     "Extends GeoSPARQL topology (cf. rcc8ec / sfTouches) to three dimensions.",
     symmetric=True, dom=ANAT + "AnatomicalStructure", rng=ANAT + "AnatomicalStructure")
objp(GEO3 + "contains3D", "contains in 3D",
     "This structure's 3D bounding box spatially contains the other's. Extends GeoSPARQL "
     "sfContains / rcc8 TPP-inverse to 3D.")
objp(GEO3 + "within3D", "within in 3D", "Inverse of geo3:contains3D.")
t(GEO3 + "contains3D", OWL + "inverseOf", iri(GEO3 + "within3D"))
objp(ANAT + "tissueContinuousWith", "tissue-continuous with",
     "Adjacent structures of the SAME tissue class — connected because they are the same "
     "kind of tissue (e.g. adjoining bones, muscle bellies, vessel segments).",
     sub=[GEO3 + "adjacent3D"], symmetric=True)
objp(GEO3 + "thermallyCoupledWith", "thermally coupled with",
     "Adjacent structures that can exchange heat, where at least one side is a perfused or "
     "metabolically active tissue (artery, vein, muscle, viscus). An illustration of a typed "
     "physical relation (heat conduction) layered on the 3D spatial adjacency substrate — the "
     "template for future physiological couplings.", sub=[GEO3 + "adjacent3D"], symmetric=True)
objp(ANAT + "relatedPhenotype", "related phenotype",
     "An HPO phenotype term whose label/synonym lexically names this structure "
     "(annotation-derived, not an asserted anatomical axiom).", rng=OWL + "Thing")
objp(ANAT + "relatedDisease", "related disease",
     "A Human Disease Ontology term whose label lexically names this structure "
     "(annotation-derived).", rng=OWL + "Thing")
# datatype-ish / asset properties (declared as datatype properties)
for pid, lab, com in [
    ("side", "side", "Body side: left or right."),
    ("tissueType", "tissue type", "Tissue class label (bone, muscle, artery, …)."),
    ("zoomLevel", "detail level", "Z-Anatomy semantic-zoom level (1 = coarsest)."),
    ("vertexCount", "vertex count", "Number of mesh vertices (geometry complexity)."),
    ("glbFile", "GLB asset", "URL of the Draco-compressed GLB for this structure's body system."),
    ("meshNode", "mesh node name", "Node name to isolate this structure inside the system GLB."),
]:
    p = ANAT + pid
    t(p, RDF, iri(OWL + "DatatypeProperty")); tl(p, RDFS + "label", lab, lang="en")
    tl(p, RDFS + "comment", com, lang="en")
for pid, lab, com in [
    ("asWKT3D", "3D WKT geometry", "Full 3D geometry serialization (POINT Z / MULTIPOINT Z) in mm."),
    ("box", "3D bounding box", "Axis-aligned bounding box BOX3D(minx miny minz, maxx maxy maxz) in mm."),
    ("x", "centroid X (mm)", "Centroid X (medial-lateral; +=left)."),
    ("y", "centroid Y (mm)", "Centroid Y (antero-posterior; +=anterior)."),
    ("z", "centroid Z (mm)", "Centroid Z (superior-inferior; +=superior)."),
    ("sizeMm", "bounding-box diagonal (mm)", "Diagonal length of the bounding box."),
]:
    p = GEO3 + pid
    t(p, RDF, iri(OWL + "DatatypeProperty")); tl(p, RDFS + "label", lab, lang="en")
    tl(p, RDFS + "comment", com, lang="en")

# ---------------------------------------------------------------- systems + regions
sys_iri = {}
for sysm in set(r["system"] for r in structs.values()):
    si = ASYS + SYS_CODE.get(sysm, re.sub(r"\W+", "_", sysm))
    sys_iri[sysm] = si
    t(si, RDF, iri(ANAT + "BodySystem")); tl(si, RDFS + "label", sysm, lang="en")

region_iri = {}
def get_region(name):
    if name not in region_iri:
        ri = AREG + re.sub(r"[^A-Za-z0-9]+", "_", name).strip("_")
        region_iri[name] = ri
        t(ri, RDF, iri(ANAT + "Region")); tl(ri, RDFS + "label", name, lang="en")
    return region_iri[name]

# ---------------------------------------------------------------- emit structures
inline_terms = set()
def emit_term(tid, meta):
    if tid in inline_terms:
        return
    inline_terms.add(tid)
    if tid.startswith("HP:"):
        ti = OBO + "HP_" + tid[3:]
    elif tid.startswith("DOID:"):
        ti = OBO + "DOID_" + tid[5:]
    else:
        return None
    tl(ti, RDFS + "label", meta["name"], lang="en")
    if meta.get("def"):
        tl(ti, SKOS + "definition", meta["def"], lang="en")
    return ti

def fnum(v): return ("%.1f" % v)

for (sysm, sid), r in structs.items():
    s = r["iri"]; disp = r["disp"]
    t(s, RDF, iri(ANAT + "AnatomicalStructure"))
    t(s, RDF, iri(GEO + "Feature"))
    # role class: a true landmark is a leaf label point (no own mesh, no parts)
    if r["role"] == "landmark" and not r.get("mesh_min") and not children.get((sysm, sid)):
        t(s, RDF, iri(ANAT + "Landmark"))
    # tissue class
    tc = tissue.get(sid)
    if tc and tc in TISSUE_CLASS:
        t(s, RDF, iri(ANAT + TISSUE_CLASS[tc]))
        tl(s, ANAT + "tissueType", tc)
    # labels
    tl(s, RDFS + "label", disp, lang="en")
    if r.get("side"):
        tl(s, ANAT + "side", r["side"])
        tl(s, SKOS + "prefLabel", f"{disp} ({r['side']})", lang="en")
    trr = tr.get(disp)
    if trr:
        for lg in ("la", "fr", "es", "pt"):
            if trr.get(lg):
                tl(s, RDFS + "label", trr[lg], lang=lg)
    d = desc.get(disp)
    if d:
        tl(s, DCT + "description", d, lang="en")
    # system / region
    t(s, ANAT + "inSystem", iri(sys_iri[sysm]))
    for rg in regions.get(sid, []):
        t(s, ANAT + "inRegion", iri(get_region(rg)))
    # partonomy
    for pi in r["parent_iris"]:
        t(s, ANAT + "partOf", iri(pi))
        t(pi, ANAT + "hasPart", iri(s))
    # geometry
    if r["gbox"]:
        mn, mx = r["gbox"]; c = r["gctr"]
        gnode = s + "/geom"
        t(s, GEO + "hasGeometry", iri(gnode))
        t(gnode, RDF, iri(GEO3 + "Geometry3D"))
        tl(gnode, GEO + "asWKT", "POINT Z(%s %s %s)" % (fnum(c[0]), fnum(c[1]), fnum(c[2])), dt=WKT_DT)
        tl(gnode, GEO3 + "asWKT3D", "POINT Z(%s %s %s)" % (fnum(c[0]), fnum(c[1]), fnum(c[2])), dt=WKT3_DT)
        tl(gnode, GEO3 + "box", "BOX3D(%s %s %s, %s %s %s)" % (
            fnum(mn[0]), fnum(mn[1]), fnum(mn[2]), fnum(mx[0]), fnum(mx[1]), fnum(mx[2])), dt=BOX_DT)
        tl(gnode, GEO3 + "x", fnum(c[0]), dt=XSD + "decimal")
        tl(gnode, GEO3 + "y", fnum(c[1]), dt=XSD + "decimal")
        tl(gnode, GEO3 + "z", fnum(c[2]), dt=XSD + "decimal")
        diag = sum((mx[i] - mn[i]) ** 2 for i in range(3)) ** 0.5
        tl(gnode, GEO3 + "sizeMm", fnum(diag), dt=XSD + "decimal")
        # also expose centroid coords on the structure for convenient FILTER math
        tl(s, GEO3 + "x", fnum(c[0]), dt=XSD + "decimal")
        tl(s, GEO3 + "y", fnum(c[1]), dt=XSD + "decimal")
        tl(s, GEO3 + "z", fnum(c[2]), dt=XSD + "decimal")
    # mesh asset (only real-mesh structures)
    if r.get("has_mesh"):
        tl(s, ANAT + "glbFile", MESH_BASE + sysm.replace(" ", "_") + ".glb")
        tl(s, ANAT + "meshNode", sid)
        tl(s, ANAT + "vertexCount", str(r["verts"]), dt=XSD + "integer")
    # zoom level
    if sid in aux["zoom"]:
        tl(s, ANAT + "zoomLevel", str(aux["zoom"][sid]), dt=XSD + "integer")
    # cross-ontology links (by side-less display label)
    for hid, name in bridge["phenotypes"].get(disp, []):
        ti = emit_term(hid, bridge["terms"][hid])
        if ti: t(s, ANAT + "relatedPhenotype", iri(ti))
    for did, name in bridge["diseases"].get(disp, []):
        ti = emit_term(did, bridge["terms"][did])
        if ti: t(s, ANAT + "relatedDisease", iri(ti))

print("triples after structures:", _n, "| inline ontology terms:", len(inline_terms))

# ---------------------------------------------------------------- 3D spatial relations
# Participants: every structure with its own mesh (accurate surface box), plus
# compact CONTAINER organs (a group node, e.g. Heart/Lung, whose union box is < the
# ORGAN_MAX diagonal) so organ-level adjacency exists too. Oversized system/region
# nodes are excluded (their box would touch everything).
ORGAN_MAX = 250.0  # mm bounding-box diagonal cutoff for container organs
def diag(gb):
    mn, mx = gb
    return sum((mx[i] - mn[i]) ** 2 for i in range(3)) ** 0.5

mesh_keys = []
for k, r in structs.items():
    gb = r["gbox"]
    if not gb:
        continue
    if r.get("has_mesh") and r.get("mesh_min"):
        mesh_keys.append(k)
    elif children.get(k) and 0 < diag(gb) < ORGAN_MAX:
        mesh_keys.append(k)
minb = np.array([structs[k]["gbox"][0] for k in mesh_keys])
maxb = np.array([structs[k]["gbox"][1] for k in mesh_keys])
ctr = (minb + maxb) / 2.0
n = len(mesh_keys)
print("adjacency over", n, "participants")

# direct parent/child exclusion (over ALL structures, so container↔part is excluded)
pc = set()
for k, r in structs.items():
    for pi in r["parent_iris"]:
        pc.add((r["iri"], pi)); pc.add((pi, r["iri"]))

adj_count = tc_count = th_count = 0
emitted = set()
CHUNK = 256
for i0 in range(0, n, CHUNK):
    i1 = min(i0 + CHUNK, n)
    # overlap of inflated boxes: min_i <= max_j+gap and max_i >= min_j-gap on all axes
    A_min = minb[i0:i1][:, None, :]; A_max = maxb[i0:i1][:, None, :]
    ov = np.all((A_min <= maxb[None, :, :] + GAP_MM) & (A_max >= minb[None, :, :] - GAP_MM), axis=2)
    for ii in range(i1 - i0):
        i = i0 + ii
        js = np.nonzero(ov[ii])[0]
        js = js[js != i]
        if len(js) == 0:
            continue
        # rank by centroid distance, cap degree
        d = np.linalg.norm(ctr[js] - ctr[i], axis=1)
        order = np.argsort(d)[:DEG_CAP]
        ki = mesh_keys[i]; si = structs[ki]["iri"]; ti_ = tissue.get(ki[1])
        for oj in order:
            j = int(js[oj])
            kj = mesh_keys[j]; sj = structs[kj]["iri"]
            if si == sj:
                continue
            pair = (si, sj) if si < sj else (sj, si)
            if pair in emitted or pair in pc:
                continue
            emitted.add(pair)
            a, b = pair
            t(a, GEO3 + "adjacent3D", iri(b)); t(b, GEO3 + "adjacent3D", iri(a))
            adj_count += 1
            tj_ = tissue.get(kj[1])
            if ti_ and tj_ and ti_ == tj_:
                t(a, ANAT + "tissueContinuousWith", iri(b)); t(b, ANAT + "tissueContinuousWith", iri(a))
                tc_count += 1
            if (ti_ in PERFUSED) or (tj_ in PERFUSED):
                t(a, GEO3 + "thermallyCoupledWith", iri(b)); t(b, GEO3 + "thermallyCoupledWith", iri(a))
                th_count += 1

print(f"adjacency edges: {adj_count} | tissue-continuous: {tc_count} | thermally-coupled: {th_count}")
out.close()
print("TOTAL TRIPLES:", _n, "->", OUT)
