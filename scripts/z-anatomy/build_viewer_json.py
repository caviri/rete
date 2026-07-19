"""Compact index for the standalone 3D explorer (docs/anatomy.html).

Parses web/z-anatomy... actually data/z-anatomy/z-anatomy.nt and emits
web/z-anatomy-viewer.json: the mesh-level structures (those with a glTF node
name), their centroid, tissue/side/system, their 3D-relation neighbours
(adjacent / same-tissue / thermal, as indices into the node array), and any
linked HPO/DOID term labels. Uploaded to R2 next to the GLBs.
"""
import json, re, os

NT = "data/z-anatomy/z-anatomy.nt"
OUT = "web/z-anatomy-viewer.json"
A = "https://w3id.org/rete/anatomy#"
G3 = "https://w3id.org/rete/geo3#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#label"
DESC = "http://purl.org/dc/terms/description"

tri = re.compile(r'^<([^>]+)>\s+<([^>]+)>\s+(.*)\s\.$')

label_en, meshnode, tissue, side, system_of, glb = {}, {}, {}, {}, {}, {}
cx, cy, cz, desc = {}, {}, {}, {}
sys_label = {}
adj, tis, th = {}, {}, {}
disease, pheno = {}, {}
term_label = {}
icd10 = {}   # term IRI -> ICD-10-CM code

def lit(o):
    m = re.match(r'^"(.*)"(?:@(\w+)|\^\^<.*>)?$', o)
    return m.group(1) if m else None
def lang(o):
    m = re.match(r'^".*"@(\w+)$', o)
    return m.group(1) if m else None

for line in open(NT, encoding="utf-8"):
    m = tri.match(line.rstrip("\n"))
    if not m:
        continue
    s, p, o = m.group(1), m.group(2), m.group(3)
    if p == RDFS:
        if o.startswith('"'):
            if lang(o) == "en":
                label_en[s] = lit(o)
                if s.startswith(A) or "obo/" in s:
                    term_label[s] = lit(o)
    elif p == A + "meshNode":
        meshnode[s] = lit(o)
    elif p == A + "tissueType":
        tissue[s] = lit(o)
    elif p == A + "side":
        side[s] = lit(o)
    elif p == A + "inSystem":
        system_of[s] = o.strip("<>")
    elif p == A + "glbFile":
        glb[s] = lit(o)
    elif p == DESC and lang(o) == "en":
        desc[s] = lit(o)[:600]
    elif p == G3 + "x":
        cx[s] = float(lit(o))
    elif p == G3 + "y":
        cy[s] = float(lit(o))
    elif p == G3 + "z":
        cz[s] = float(lit(o))
    elif p == G3 + "adjacent3D":
        adj.setdefault(s, []).append(o.strip("<>"))
    elif p == A + "tissueContinuousWith":
        tis.setdefault(s, set()).add(o.strip("<>"))
    elif p == G3 + "thermallyCoupledWith":
        th.setdefault(s, set()).add(o.strip("<>"))
    elif p == A + "relatedDisease":
        disease.setdefault(s, []).append(o.strip("<>"))
    elif p == A + "relatedPhenotype":
        pheno.setdefault(s, []).append(o.strip("<>"))
    elif p == A + "icd10":
        icd10.setdefault(s, lit(o))

# system labels
for s, sysiri in system_of.items():
    pass
# resolve system labels from label_en of the system IRIs
sys_iris = sorted(set(system_of.values()))
sys_index = {iri: i for i, iri in enumerate(sys_iris)}
sys_names = [label_en.get(iri, iri.rsplit("/", 1)[-1]) for iri in sys_iris]

# nodes = structures that have a mesh node AND a centroid
node_iris = [s for s in meshnode if s in cx and s in system_of]
idx = {iri: i for i, iri in enumerate(node_iris)}

def neigh_idx(s, table):
    out = []
    for o in table.get(s, []):
        if o in idx:
            out.append(idx[o])
    return sorted(set(out))

nodes = []
for iri in node_iris:
    nodes.append({
        "n": meshnode[iri],
        "l": label_en.get(iri, meshnode[iri]),
        "t": tissue.get(iri, ""),
        "sd": side.get(iri, ""),
        "sy": sys_index[system_of[iri]],
        "c": [round(cx[iri], 1), round(cy[iri], 1), round(cz[iri], 1)],
        "adj": neigh_idx(iri, adj),
        "tis": neigh_idx(iri, tis),
        "th": neigh_idx(iri, th),
        "dis": sorted({(term_label.get(t, "") + (" · " + icd10[t] if t in icd10 else ""))
                       for t in disease.get(iri, []) if term_label.get(t)})[:40],
        "phe": sorted({term_label.get(t, "") for t in pheno.get(iri, [])} - {""})[:40],
        "d": desc.get(iri, ""),
    })

glb_by_sys = {}
for iri, url in glb.items():
    if iri in system_of:
        glb_by_sys[sys_index[system_of[iri]]] = url

out = {"systems": sys_names, "glb": glb_by_sys, "nodes": nodes}
os.makedirs(os.path.dirname(OUT), exist_ok=True)
json.dump(out, open(OUT, "w", encoding="utf-8"), separators=(",", ":"), ensure_ascii=False)
sz = os.path.getsize(OUT) / 1e6
print(f"nodes: {len(nodes)} | systems: {len(sys_names)} | {OUT} ({sz:.2f} MB)")
print("systems:", sys_names)
# quick check
liver = next((n for n in nodes if n["l"] == "Liver"), None)
if liver:
    print("Liver: adj", len(liver["adj"]), "tissue-cont", len(liver["tis"]), "thermal", len(liver["th"]),
          "diseases", len(liver["dis"]), "phenotypes", len(liver["phe"]))
