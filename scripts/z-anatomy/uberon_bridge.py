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

def despell(s):  # British<->American: oesophagus/esophagus, tumour/tumor, colour...
    return s.replace("oe", "e").replace("ae", "e").replace("our", "or")

def keyset(s):   # all normalized lookup keys for a label
    ks = {s, singular(s)}
    ks |= {despell(k) for k in list(ks)}
    return {k for k in ks if len(k) >= 4}


# ---- UBERON id -> {label, synonyms}  +  part_of / is_a parent graphs ----
ub_label, ub_syn, ub_partof, ub_isa = {}, {}, {}, {}
cur = None
def uref(tok):  # "UBERON:0000948 ! heart" -> IRI
    m = re.match(r"(UBERON:\d+)", tok)
    return OBO + m.group(1).replace(":", "_") if m else None
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
        elif line.startswith("is_a: ") and cur.get("id"):
            p = uref(line[6:])
            if p: ub_isa.setdefault(cur["id"], set()).add(p)
        elif line.startswith("relationship: part_of ") and cur.get("id"):
            p = uref(line[len("relationship: part_of "):])
            if p: ub_partof.setdefault(cur["id"], set()).add(p)
print("UBERON terms:", len(ub_label), "| part_of:", len(ub_partof), "| is_a:", len(ub_isa))

# ---- z-anatomy labels: normalized -> display label ----
import glob
aux = json.load(open(os.path.join(DERIVED, "aux.json"), encoding="utf-8"))
disp_by_norm = {}
def add_key(k, disp):
    if len(k) >= 4 and k not in STOP:
        for kk in keyset(k):
            if kk not in STOP:
                disp_by_norm.setdefault(kk, disp)
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

# ---- UBERON id -> z-anatomy display label (label or synonym match) ----
ub2anat = {}
for uid, lab in ub_label.items():
    cands = [norm(lab)] + [norm(s) for s in ub_syn.get(uid, [])]
    for c in cands:
        hit = next((disp_by_norm[k] for k in keyset(c) if k in disp_by_norm), None)
        if hit:
            ub2anat[uid] = hit; break
print("UBERON->z-anatomy resolved (direct):", len(ub2anat))

# ---- roll-up: a UBERON part we can't mesh -> the nearest containing UBERON term
# we CAN mesh (walk up part_of/is_a). Locates e.g. cardiac muscle -> Heart,
# bronchiole -> Lung, renal glomerulus -> Kidney.
def _walk(uid, graph, maxdepth):
    seen = {uid}; frontier = [uid]
    for _ in range(maxdepth):
        nxt = [p for n in frontier for p in graph.get(n, ()) if p not in seen]
        for p in nxt:
            seen.add(p)
        found = {ub2anat[p] for p in nxt if p in ub2anat}
        if found:
            return found
        frontier = nxt
        if not frontier:
            break
    return set()

_roll = {}
def rollup(uid, maxdepth=5):
    """Nearest meshed organ for a UBERON term: itself if mapped, else the nearest
    ancestor. part_of (part -> containing organ) is preferred over is_a (subtype ->
    category), so a coronary artery lands on the Heart, not 'systemic arteries'."""
    if uid in _roll:
        return _roll[uid]
    if uid in ub2anat:
        _roll[uid] = {ub2anat[uid]}; return _roll[uid]
    merged = {}
    for m in (ub_partof, ub_isa):
        for k, v in m.items():
            merged.setdefault(k, set()).update(v)
    res = _walk(uid, ub_partof, maxdepth) or _walk(uid, merged, maxdepth)
    _roll[uid] = res
    return res


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


def extract(owl_path, prefix, cap_note, inherit=False):
    g = Graph()
    print(f"parsing {os.path.basename(owl_path)} …")
    g.parse(owl_path)
    print(f"  {len(g)} triples")
    pfx = OBO + prefix + "_"
    classes = [s for s in g.subjects(RDF.type, OWL.Class) if isinstance(s, URIRef) and str(s).startswith(pfx)]

    # each class's OWN UBERON locations (from its someValuesFrom restrictions)
    direct = {}
    for c in classes:
        us = collect_uberon(g, c)
        if us:
            direct[c] = us

    # named is_a graph (this ontology only) — DOID axiomatises location at the
    # general level ('heart disease' -> heart), so a leaf with no location of its
    # own inherits from its nearest ancestor that has one.
    dparent = {}
    if inherit:
        for s, o in g.subject_objects(RDFS.subClassOf):
            if (isinstance(s, URIRef) and isinstance(o, URIRef)
                    and str(s).startswith(pfx) and str(o).startswith(pfx)):
                dparent.setdefault(s, set()).add(o)

    def uberon_for(c, maxdepth=5):
        if c in direct:
            return direct[c]
        if not inherit:
            return set()
        seen = {c}; frontier = [c]
        for _ in range(maxdepth):
            nxt = [p for n in frontier for p in dparent.get(n, ()) if p not in seen]
            for p in nxt:
                seen.add(p)
            got = set()
            for p in nxt:
                got |= direct.get(p, set())
            if got:
                return got
            frontier = nxt
            if not frontier:
                break
        return set()

    links, terms = {}, {}
    for c in classes:
        lab = g.value(c, RDFS.label)
        if lab is None or (c, OWL.deprecated, None) in g:
            continue
        anat_labels = set()
        for u in uberon_for(c):
            anat_labels |= rollup(u)     # UBERON part -> nearest meshed organ
        if not anat_labels:
            continue
        tid = prefix + ":" + str(c).split("_")[-1]
        terms[tid] = str(lab)
        for al in anat_labels:
            links.setdefault(al, set()).add((tid, str(lab)))
    del g
    print(f"  {cap_note}: {len(classes)} classes, {sum(len(v) for v in links.values())} links to {len(links)} structures")
    return links, terms


dis_links, dis_terms = extract(DOWL, "DOID", "diseases via UBERON", inherit=True)
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
