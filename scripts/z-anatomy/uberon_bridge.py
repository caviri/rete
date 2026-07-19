"""Upgrade the anatomy<->disease/phenotype bridge using FORMAL location axioms.

Disease Ontology and HPO both carry OWL axioms that point a term at the anatomy it
concerns, via UBERON: DOID `disease has location` (RO_0004026) / located_in, and
HPO's EQ logical definitions (inheres_in some UBERON). We extract every
someValuesFrom restriction whose filler is a UBERON class, resolve the UBERON term
to a z-anatomy structure by label/synonym (anatomy<->anatomy matching, reliable),
and so link e.g. 'myocardial infarction' -> heart, 'cholelithiasis' -> gallbladder —
which the disease-name lexical match could never catch.

Merges the result INTO data/z-anatomy/derived/bridge.json (keeps the lexical links).
"""
import json, os, re
from rdflib import Graph, URIRef, BNode, RDF, RDFS, OWL

DERIVED = "data/z-anatomy/derived"
BRIDGE = os.path.join(DERIVED, "bridge.json")
UBERON_OBO = "data/uberon-ontology/raw/uberon-basic.obo"
DOWL = "data/disease-ontology/raw/HumanDiseaseOntology-2026-06-30/HumanDiseaseOntology-2026-06-30/src/ontology/doid.owl"
HPOWL = "data/human-phenotype-ontology/raw/hp.owl"

OBO = "http://purl.obolibrary.org/obo/"
UB = OBO + "UBERON_"

# generic anatomy words we won't match on their own (kept from the lexical bridge)
STOP = {"body","structure","system","organ","part","region","tissue","wall","cavity",
        "surface","zone","tube","duct","tract","gland","membrane","fluid","set","group"}


def norm(s):
    s = s.lower().strip()
    s = re.sub(r"[()]", "", s)
    s = re.sub(r"\s+", " ", s)
    return s

def singular(s):
    for a, b in [("ies","y"),("ves","f"),("ses","s"),("s","")]:
        if s.endswith(a) and len(s)-len(a) >= 3:
            return s[:-len(a)]+b
    return s


# ---- UBERON id -> {label, synonyms} ----
ub_label, ub_syn = {}, {}
cur = None
for line in open(UBERON_OBO, encoding="utf-8", errors="replace"):
    line = line.rstrip("\n")
    if line == "[Term]":
        cur = {"id": None, "syn": []}
    elif line.startswith("[") and line != "[Term]":
        cur = None
    elif cur is not None:
        if line.startswith("id: UBERON:"):
            cur["id"] = OBO + "UBERON_" + line.split(":")[-1].strip()
        elif line.startswith("name: ") and cur.get("id"):
            ub_label[cur["id"]] = line[6:].strip()
        elif line.startswith("synonym: ") and cur.get("id"):
            m = re.match(r'synonym: "(.*?)"\s+(\w+)', line)
            if m and m.group(2) in ("EXACT", "NARROW"):
                ub_syn.setdefault(cur["id"], []).append(m.group(1))
print("UBERON terms:", len(ub_label))

# ---- z-anatomy labels: normalized -> display label ----
import glob
aux = json.load(open(os.path.join(DERIVED, "aux.json"), encoding="utf-8"))
disp_by_norm = {}
def add_key(k, disp):
    if len(k) >= 4 and k not in STOP:
        disp_by_norm.setdefault(k, disp)
        disp_by_norm.setdefault(singular(k), disp)
def strip_side(n): return n[:-2] if n.endswith(".r") or n.endswith(".l") else n
for jf in glob.glob(os.path.join(DERIVED, "*.jsonl")):
    for line in open(jf, encoding="utf-8"):
        r = json.loads(line); disp = strip_side(r["id"])
        add_key(norm(disp), disp)
for en, tr in aux["translations"].items():
    if norm(en) in disp_by_norm:
        la = norm(tr.get("la", ""))
        if la: add_key(la, disp_by_norm[norm(en)])
print("z-anatomy match keys:", len(disp_by_norm))

# ---- UBERON id -> z-anatomy display label (label or synonym exact match) ----
ub2anat = {}
for uid, lab in ub_label.items():
    cands = [norm(lab)] + [norm(s) for s in ub_syn.get(uid, [])]
    for c in cands:
        if c in disp_by_norm:
            ub2anat[uid] = disp_by_norm[c]; break
        if singular(c) in disp_by_norm:
            ub2anat[uid] = disp_by_norm[singular(c)]; break
print("UBERON->z-anatomy resolved:", len(ub2anat))


def collect_uberon(g, start):
    """UBERON URIs reachable from `start` through its blank-node axiom structure
    (restrictions / intersections / lists) — i.e. the term's OWN location axioms,
    not inherited from named superclasses."""
    seen, out, stack = set(), set(), [start]
    while stack:
        node = stack.pop()
        if node in seen:
            continue
        seen.add(node)
        for _p, o in g.predicate_objects(node):
            if isinstance(o, URIRef):
                s = str(o)
                if s.startswith(UB):
                    out.add(s)
            elif isinstance(o, BNode):
                stack.append(o)
    return out


def extract(owl_path, prefix, cap_note):
    g = Graph()
    print(f"parsing {os.path.basename(owl_path)} …")
    g.parse(owl_path)
    print(f"  {len(g)} triples")
    links = {}   # anat display label -> {(termid, name)}
    terms = {}
    pfx = OBO + prefix + "_"
    classes = [s for s in g.subjects(RDF.type, OWL.Class) if isinstance(s, URIRef) and str(s).startswith(pfx)]
    for c in classes:
        lab = g.value(c, RDFS.label)
        if lab is None:
            continue
        if (c, OWL.deprecated, None) in g:
            continue
        anat_labels = set()
        for u in collect_uberon(g, c):
            if u in ub2anat:
                anat_labels.add(ub2anat[u])
        if not anat_labels:
            continue
        tid = prefix + ":" + str(c).split("_")[-1]
        terms[tid] = str(lab)
        for al in anat_labels:
            links.setdefault(al, set()).add((tid, str(lab)))
    del g
    print(f"  {cap_note}: {len(classes)} classes, {sum(len(v) for v in links.values())} links to {len(links)} structures")
    return links, terms


dis_links, dis_terms = extract(DOWL, "DOID", "diseases via UBERON")
phe_links, phe_terms = extract(HPOWL, "HP", "phenotypes via UBERON")

# ---- merge into existing bridge.json ----
bridge = json.load(open(BRIDGE, encoding="utf-8"))
def merge(dst_dict, links, terms):
    # dst_dict: {label: [[id,name],...]}
    existing = {k: {tuple(x) for x in v} for k, v in dst_dict.items()}
    for lab, pairs in links.items():
        existing.setdefault(lab, set()).update(pairs)
    for k, v in existing.items():
        dst_dict[k] = sorted([list(x) for x in v])
    bridge["terms"].update({tid: {"name": nm, "def": bridge["terms"].get(tid, {}).get("def")}
                            for tid, nm in terms.items()})

before_d = sum(len(v) for v in bridge["diseases"].values())
before_p = sum(len(v) for v in bridge["phenotypes"].values())
merge(bridge["diseases"], dis_links, dis_terms)
merge(bridge["phenotypes"], phe_links, phe_terms)
after_d = sum(len(v) for v in bridge["diseases"].values())
after_p = sum(len(v) for v in bridge["phenotypes"].values())
json.dump(bridge, open(BRIDGE, "w", encoding="utf-8"), ensure_ascii=False)
print(f"\ndisease links {before_d} -> {after_d} | phenotype links {before_p} -> {after_p}")
print(f"structures with diseases: {len(bridge['diseases'])} | with phenotypes: {len(bridge['phenotypes'])}")
