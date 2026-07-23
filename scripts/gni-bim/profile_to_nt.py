"""Profile every IFC in the GNI BIM corpus into one RDF graph (gnibim:).

Each model becomes a node with its subset (fundamentals / project), discipline
(architectural / structural / single), STEP schema, element/storey/space counts,
file size, and a per-IFC-class element tally. Paired architectural + structural
models of one project are linked (gnibim:pairedWith) under a gnibim:Project.

Fast: counts instances by type, never touches geometry. Files above a size
threshold are recorded from their header only (schema + size) so a 536 MB model
can't OOM the run. Runs in the ifcopenshell image:
  python scripts/gni-bim/profile_to_nt.py <corpus-root> <out.nt>
"""
import sys, os, re, glob, json
from collections import Counter

ROOT, OUT = sys.argv[1], sys.argv[2]
BIG_MB = 250                                  # above this, header-only (no full parse)
INST = "https://w3id.org/rete/gnibim/"
GB = "https://w3id.org/rete/gnibim#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
OWL = "http://www.w3.org/2002/07/owl#"
XSD = "http://www.w3.org/2001/XMLSchema#"

out = open(OUT, "w", encoding="utf-8")
def w(s): out.write(s + "\n")
def esc(s): return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
def iri(s): return "<" + s + ">"
def t(s, p, o): w(iri(s) + " " + iri(p) + " " + o + " .")
def tl(s, p, o, dt=None, lang=None):
    o = '"' + esc(str(o)) + '"'
    if dt: o += "^^" + iri(dt)
    elif lang: o += "@" + lang
    w(iri(s) + " " + iri(p) + " " + o + " .")

# ---- ontology ----
w(iri(GB.rstrip("#")) + " " + iri(RDF) + " " + iri(OWL + "Ontology") + " .")
for c, lab in [("Model", "BIM model"), ("Project", "BIM project"),
               ("ElementClass", "IFC element class"), ("Tally", "element tally")]:
    tl(GB + c, RDFS + "label", lab, lang="en")
for p, lab in [("subset", "subset"), ("discipline", "discipline"), ("schema", "IFC schema"),
               ("elementCount", "element count"), ("storeyCount", "storey count"),
               ("spaceCount", "space count"), ("fileSizeMB", "file size (MB)"),
               ("count", "count"), ("parsed", "fully parsed")]:
    t(GB + p, RDF, iri(OWL + "DatatypeProperty")); tl(GB + p, RDFS + "label", lab, lang="en")
for p, lab in [("project", "project"), ("hasModel", "has model"), ("pairedWith", "paired with"),
               ("tally", "element tally"), ("ifcClass", "IFC class")]:
    t(GB + p, RDF, iri(OWL + "ObjectProperty")); tl(GB + p, RDFS + "label", lab, lang="en")

classes_seen = set()
def class_node(cls):
    if cls not in classes_seen:
        classes_seen.add(cls)
        c = INST + "class/" + cls
        t(c, RDF, iri(GB + "ElementClass")); tl(c, RDFS + "label", cls, lang="en")
    return INST + "class/" + cls

def profile(path):
    """(schema, n_elements|None, n_storey, n_space, Counter|None)"""
    schema, ne, ns, nsp, cc = "?", None, 0, 0, None
    # schema from the header (cheap, always works)
    try:
        with open(path, "r", encoding="utf-8", errors="ignore") as fh:
            head = fh.read(4096)
        m = re.search(r"FILE_SCHEMA\(\('([^']+)'", head)
        if m: schema = m.group(1)
    except Exception:
        pass
    if os.path.getsize(path) > BIG_MB * 1e6:
        return schema, None, 0, 0, None
    try:
        import ifcopenshell
        f = ifcopenshell.open(path)
        schema = f.schema
        els = f.by_type("IfcElement")
        cc = Counter(e.is_a() for e in els)
        ne = len(els); ns = len(f.by_type("IfcBuildingStorey")); nsp = len(f.by_type("IfcSpace"))
    except Exception as e:
        print(f"  ! parse failed {os.path.basename(path)}: {str(e)[:80]}", flush=True)
    return schema, ne, ns, nsp, cc

def emit_model(mid, label, subset, discipline, path):
    s = INST + "model/" + mid
    sz = os.path.getsize(path) / 1e6
    schema, ne, ns, nsp, cc = profile(path)
    t(s, RDF, iri(GB + "Model")); tl(s, RDFS + "label", label, lang="en")
    tl(s, GB + "subset", subset); tl(s, GB + "discipline", discipline)
    tl(s, GB + "schema", schema); tl(s, GB + "fileSizeMB", round(sz, 2), dt=XSD + "decimal")
    tl(s, GB + "parsed", "true" if ne is not None else "false", dt=XSD + "boolean")
    if ne is not None:
        tl(s, GB + "elementCount", ne, dt=XSD + "integer")
        tl(s, GB + "storeyCount", ns, dt=XSD + "integer")
        tl(s, GB + "spaceCount", nsp, dt=XSD + "integer")
        for cls, n in cc.items():
            tn = s + "/tally/" + cls
            t(s, GB + "tally", iri(tn))
            t(tn, RDF, iri(GB + "Tally")); t(tn, GB + "ifcClass", iri(class_node(cls)))
            tl(tn, GB + "count", n, dt=XSD + "integer")
    return s, ne

summary = []
# 2025 BIM Fundamentals — 208 single-discipline models of one building shape
fund = sorted(glob.glob(os.path.join(ROOT, "2025_BIMfundamentals", "**", "model_*.ifc"), recursive=True),
              key=lambda p: int(re.search(r"model_(\d+)", os.path.basename(p)).group(1)))
for p in fund:
    num = re.search(r"model_(\d+)", os.path.basename(p)).group(1)
    s, ne = emit_model("f/model_" + num, f"Fundamentals model {num}", "fundamentals", "single", p)
    summary.append({"id": "f/model_" + num, "n": ne})
    print(f"fundamentals model_{num}: {ne} elements", flush=True)

# 2026 BIM Projects — paired architectural + structural per team
proj = glob.glob(os.path.join(ROOT, "2026_BIMprojects", "**", "model_*.ifc"), recursive=True)
by_project = {}
for p in proj:
    m = re.match(r"model_(\d+)_(arc|structure)", os.path.basename(p))
    if not m: continue
    by_project.setdefault(m.group(1), {})[m.group(2)] = p
for pnum in sorted(by_project, key=int):
    proj_iri = INST + "project/project_" + pnum
    t(proj_iri, RDF, iri(GB + "Project")); tl(proj_iri, RDFS + "label", f"Project {pnum}", lang="en")
    disc = {"arc": "architectural", "structure": "structural"}
    mids = {}
    for kind, path in by_project[pnum].items():
        mid = f"p/model_{pnum}_{kind}"
        s, ne = emit_model(mid, f"Project {pnum} — {disc[kind]}", "project", disc[kind], path)
        t(proj_iri, GB + "hasModel", iri(s)); t(s, GB + "project", iri(proj_iri))
        mids[kind] = s
        summary.append({"id": mid, "n": ne})
        print(f"project_{pnum} {kind}: {ne} elements", flush=True)
    if "arc" in mids and "structure" in mids:
        t(mids["arc"], GB + "pairedWith", iri(mids["structure"]))
        t(mids["structure"], GB + "pairedWith", iri(mids["arc"]))

out.close()
json.dump(summary, open(OUT + ".summary.json", "w"))
print(f"\nDONE: {len(summary)} models -> {OUT}")
