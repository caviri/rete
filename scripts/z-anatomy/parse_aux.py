"""Parse Z-Anatomy auxiliary data (host Python, no Blender):
  - Translations0.txt        -> multilingual labels (en/la/fr/es/pt)
  - OriginalDescriptions/*   -> English descriptions
  - Layers/Collections*.csv  -> tissue class + region/group membership + zoom level

Join key is the side-less English label; canonical ids keep .r/.l for identity.
Writes data/z-anatomy/derived/aux.json.
"""
import csv, json, os, re, glob

ROOT = "data/z-anatomy/raw/Z-Anatomy-PC-Version/Z-Anatomy-PC-Version/Resources"
OUT = "data/z-anatomy/derived/aux.json"

# tissue class per tissue-CSV
TISSUE = {
    "Arteries": "artery", "Veins": "vein", "Bones": "bone", "Muscles": "muscle",
    "Nerves": "nerve", "Ligaments": "ligament", "Lymph": "lymphoid",
    "Fasciae": "fascia", "Skin": "skin", "Viscera": "viscus",
}


def strip_side(name):
    if name.endswith(".r") or name.endswith(".l"):
        return name[:-2]
    return name


def clean(cell):
    return (cell or "").strip()


# --- translations ---
translations = {}
tp = os.path.join(ROOT, "Translations0.txt")
with open(tp, encoding="utf-8", errors="replace") as fh:
    header = fh.readline()
    for line in fh:
        parts = [p.strip() for p in line.rstrip("\n").split(";")]
        if len(parts) < 2 or not parts[0]:
            continue
        en = parts[0]
        translations[en] = {
            "la": parts[1] if len(parts) > 1 else "",
            "fr": parts[2] if len(parts) > 2 else "",
            "es": parts[3] if len(parts) > 3 else "",
            "pt": parts[4] if len(parts) > 4 else "",
        }
print("translations:", len(translations))

# --- descriptions ---
descriptions = {}
for f in glob.glob(os.path.join(ROOT, "Descriptions", "OriginalDescriptions", "*.txt")):
    label = os.path.splitext(os.path.basename(f))[0]
    txt = open(f, encoding="utf-8", errors="replace").read()
    # collapse blank lines / whitespace; drop a leading ALL-CAPS title echo
    lines = [l.strip() for l in txt.splitlines()]
    body = " ".join(l for l in lines if l)
    body = re.sub(r"\s+", " ", body).strip()
    # drop a leading uppercase echo of the label
    if body[:len(label)].upper() == label.upper():
        body = body[len(label):].strip()
    if body:
        descriptions[label] = body[:4000]
print("descriptions:", len(descriptions))

# --- collections: tissue class + region/group membership + zoom level ---
tissue = {}       # canonical id -> tissue class
zoom = {}         # canonical id -> min zoom level (1 = coarsest)
regions = {}      # canonical id -> set of region/group names

for f in sorted(glob.glob(os.path.join(ROOT, "Layers", "Collections - *.csv"))):
    name = os.path.basename(f)[len("Collections - "):-4]
    with open(f, encoding="utf-8", errors="replace", newline="") as fh:
        rows = list(csv.reader(fh))
    if not rows:
        continue
    headers = [clean(h) for h in rows[0]]
    is_tissue = name in TISSUE
    for row in rows[1:]:
        for ci, cell in enumerate(row):
            v = clean(cell)
            if not v:
                continue
            if is_tissue:
                tissue.setdefault(v, TISSUE[name])
                # column header like "Bones-3" -> level 3
                m = re.search(r"-(\d+)$", headers[ci] if ci < len(headers) else "")
                lvl = int(m.group(1)) if m else 1
                if v not in zoom or lvl < zoom[v]:
                    zoom[v] = lvl
            else:
                # BONUS / Group-*: column header is a region/group name
                grp = headers[ci] if ci < len(headers) else name
                if grp:
                    regions.setdefault(v, set()).add(grp)

regions = {k: sorted(v) for k, v in regions.items()}
print("tissue-classified:", len(tissue), "| region-memberships:", len(regions))

out = {"translations": translations, "descriptions": descriptions,
       "tissue": tissue, "zoom": zoom, "regions": regions}
os.makedirs(os.path.dirname(OUT), exist_ok=True)
with open(OUT, "w", encoding="utf-8") as fh:
    json.dump(out, fh, ensure_ascii=False)
print("wrote", OUT)

# quick coverage check against extracted structures
import glob as g
ids = set()
labels = set()
for jf in g.glob("data/z-anatomy/derived/*.jsonl"):
    for line in open(jf, encoding="utf-8"):
        r = json.loads(line)
        ids.add(r["id"]); labels.add(strip_side(r["id"]))
tl_hit = sum(1 for l in labels if l in translations)
de_hit = sum(1 for l in labels if l in descriptions)
ti_hit = sum(1 for i in ids if i in tissue)
print(f"coverage: {len(labels)} labels | translations {tl_hit} | descriptions {de_hit} | tissue {ti_hit}/{len(ids)} ids")
