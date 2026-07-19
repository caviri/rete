"""Lexical bridge: Z-Anatomy structures -> HPO phenotypes + Disease Ontology diseases.

These are ANNOTATION-derived links (a structure's English/Latin label occurring in
an ontology term's label/synonym), not asserted anatomical axioms. HPO uses regular
naming ("Abnormality of the LIVER", "Abnormal HEART morphology", "LIVER hypoplasia"),
so we strip the boilerplate to a core anatomy phrase and EXACT-match it to an anatomy
label -> high precision. DOID names are freer, so we match an organ-level anatomy
label as a whole phrase, restricted to viscus/organ labels to stay precise.

Writes data/z-anatomy/derived/bridge.json.
"""
import json, os, re, glob

HP_OBO = "data/human-phenotype-ontology/raw/hp.obo"
DO_OBO = "data/disease-ontology/raw/HumanDiseaseOntology-2026-06-30/HumanDiseaseOntology-2026-06-30/src/ontology/doid.obo"
AUX = "data/z-anatomy/derived/aux.json"
OUT = "data/z-anatomy/derived/bridge.json"

# generic anatomy words that must never be a match key on their own (too broad)
STOP = {
    "body", "canal", "process", "surface", "head", "neck", "back", "root", "base",
    "wall", "cavity", "duct", "line", "margin", "border", "angle", "notch", "crest",
    "space", "band", "arch", "fold", "sheath", "branch", "trunk", "bone", "muscle",
    "nerve", "artery", "vein", "joint", "gland", "region", "part", "layer", "system",
    "tract", "node", "ligament", "membrane", "septum", "ridge", "groove", "fossa",
    "sinus", "tubercle", "spine", "cornu", "horn", "lobe", "segment", "ala", "wing",
    "opening", "aperture", "foramen", "hiatus", "band", "ring", "plate", "column",
}


def parse_obo(path, prefix):
    terms = {}
    cur = None
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.rstrip("\n")
        if line == "[Term]":
            cur = {"id": None, "name": None, "syn": [], "def": None, "obsolete": False}
        elif line.startswith("[") and line != "[Term]":
            cur = None
        elif cur is not None:
            if line.startswith("id: ") and line[4:].startswith(prefix):
                cur["id"] = line[4:].strip()
            elif line.startswith("name: "):
                cur["name"] = line[6:].strip()
            elif line.startswith("def: "):
                m = re.match(r'def: "(.*)"', line)
                if m:
                    cur["def"] = m.group(1)
            elif line.startswith("synonym: "):
                m = re.match(r'synonym: "(.*?)"\s+(\w+)', line)
                if m and m.group(2) in ("EXACT", "NARROW"):
                    cur["syn"].append(m.group(1))
            elif line == "is_obsolete: true":
                cur["obsolete"] = True
            if cur.get("id") and cur.get("name") and (line == "" or line.startswith("is_obsolete")):
                if not cur["obsolete"]:
                    terms[cur["id"]] = cur
    # final flush handled inline; ensure last term captured
    return {k: v for k, v in terms.items() if not v["obsolete"]}


def norm(s):
    s = s.lower().strip()
    s = re.sub(r"[()]", "", s)
    s = re.sub(r"\s+", " ", s)
    return s


def singularize(s):
    for a, b in [("ies", "y"), ("ves", "f"), ("ses", "s"), ("s", "")]:
        if s.endswith(a) and len(s) - len(a) >= 3:
            return s[: -len(a)] + b
    return s


# --- anatomy labels ---
aux = json.load(open(AUX, encoding="utf-8"))
labels = {}   # normalized label -> canonical display label
organ_labels = set()  # subset that are viscus/organ-level (for DOID)
tissue = aux["tissue"]

def strip_side(n):
    return n[:-2] if (n.endswith(".r") or n.endswith(".l")) else n

disp_by_norm = {}
for jf in glob.glob("data/z-anatomy/derived/*.jsonl"):
    for line in open(jf, encoding="utf-8"):
        r = json.loads(line)
        disp = r["label"]
        n = norm(disp)
        if len(n) >= 4 and n not in STOP:
            disp_by_norm[n] = disp
            disp_by_norm[singularize(n)] = disp
        if tissue.get(r["id"]) == "viscus" or r["system"] in ("VisceralSystem", "LymphoidOrgans"):
            if len(n) >= 4 and n not in STOP:
                organ_labels.add(n); organ_labels.add(singularize(n))
# add latin labels as keys too
for en, tr in aux["translations"].items():
    la = norm(tr.get("la", ""))
    if len(la) >= 5 and la not in STOP and norm(en) in disp_by_norm:
        disp_by_norm.setdefault(la, disp_by_norm[norm(en)])

print("anatomy match-keys:", len(disp_by_norm), "| organ-level keys:", len(organ_labels))

# --- HPO extraction ---
hp = parse_obo(HP_OBO, "HP:")
print("HP terms:", len(hp))
PREFIXES = [
    "abnormality of the ", "abnormality of ", "abnormal ", "aplasia of the ",
    "aplasia of ", "hypoplasia of the ", "hypoplasia of ", "agenesis of the ",
    "agenesis of ", "hyperplasia of the ", "atrophy of the ", "atrophy of ",
    "aplasia/hypoplasia of the ", "aplasia/hypoplasia of ", "neoplasm of the ",
    "neoplasm of ", "malformation of the ", "inflammation of the ",
]
SUFFIXES = [" morphology", " hypoplasia", " aplasia", " agenesis", " atrophy",
            " hyperplasia", " dysplasia", " neoplasm", " malformation", " abnormality"]

def core_phrase(name):
    n = norm(name)
    for p in PREFIXES:
        if n.startswith(p):
            n = n[len(p):]; break
    for s in SUFFIXES:
        if n.endswith(s):
            n = n[: -len(s)]; break
    return n.strip()

anat_pheno = {}   # display label -> list of (HPid, name)
terms_out = {}
for hid, t in hp.items():
    cand = {core_phrase(t["name"])}
    for s in t["syn"]:
        cand.add(core_phrase(s))
    matched = None
    for c in cand:
        if c in disp_by_norm:
            matched = disp_by_norm[c]; break
        sc = singularize(c)
        if sc in disp_by_norm:
            matched = disp_by_norm[sc]; break
    if matched:
        anat_pheno.setdefault(matched, []).append([hid, t["name"]])
        terms_out[hid] = {"name": t["name"], "def": t.get("def")}

print("anatomy labels with phenotype links:", len(anat_pheno),
      "| total pheno links:", sum(len(v) for v in anat_pheno.values()))

# --- DOID extraction (organ-level whole-phrase) ---
do = parse_obo(DO_OBO, "DOID:")
print("DO terms:", len(do))
# index organ labels by word for quick candidate lookup
anat_dis = {}
organ_disp = {k: disp_by_norm[k] for k in organ_labels if k in disp_by_norm}
for did, t in do.items():
    hay = norm(t["name"])
    words = set(hay.split())
    for k, disp in organ_disp.items():
        # whole-phrase, word-boundary
        if k in hay and re.search(r"(^|\W)" + re.escape(k) + r"($|\W)", hay):
            anat_dis.setdefault(disp, []).append([did, t["name"]])
            terms_out[did] = {"name": t["name"], "def": t.get("def")}
print("anatomy labels with disease links:", len(anat_dis),
      "| total disease links:", sum(len(v) for v in anat_dis.values()))

out = {"phenotypes": anat_pheno, "diseases": anat_dis, "terms": terms_out}
os.makedirs(os.path.dirname(OUT), exist_ok=True)
json.dump(out, open(OUT, "w", encoding="utf-8"), ensure_ascii=False)
print("wrote", OUT)

# samples for precision review
print("\n--- SAMPLE phenotype links ---")
for lab in ["Liver", "Heart", "Kidney", "Spleen", "Femur", "Optic nerve", "Cerebellum"]:
    if lab in anat_pheno:
        print(f"  {lab}: {[n for _, n in anat_pheno[lab][:5]]}")
print("--- SAMPLE disease links ---")
for lab in list(anat_dis)[:8]:
    print(f"  {lab}: {[n for _, n in anat_dis[lab][:4]]}")
