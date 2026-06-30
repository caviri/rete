#!/usr/bin/env python3
"""Smithsonian Open Access 3D models -> N-Triples (with streamable .glb URLs).

Source: the PUBLIC, keyless `smithsonian-open-access` S3 bucket (CC0 / public
domain), `3d/` prefix. Each model is `3d/<uuid>/` with a `scene.svx.json` (a
Voyager scene) listing `.glb` derivatives (Low/Medium/High, usage Web3D) and
`metas[].collection` = {title, edanRecordId, sceneTitle}. We take the MEDIUM
(Draco-compressed) `.glb` as the streamable mesh URL the playground renders
inline, plus the title, the museum unit (from the EDAN id) and the catalogue
number (from the asset filename). No API key, no rate limit.

Usage: python3 scripts/smithsonian3d_to_nt.py [LIMIT] > data/smithsonian3d/smithsonian3d.nt
"""
import sys, re, os, glob, json, urllib.request, urllib.parse
from concurrent.futures import ThreadPoolExecutor

sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")

S3 = "https://smithsonian-open-access.s3.amazonaws.com"
BASE = "https://3d.si.edu/"
P = BASE + "prop/"
C = BASE + "class/"
# Pre-rendered Blender turntables (scripts/render_turntables.sh) live in the bucket;
# we attach them to the matching model so the playground can play a lightweight spin
# preview (webm/gif) without loading the full GLB. Only emitted for uuids we rendered.
SPIN_DIR = os.path.join(os.path.dirname(__file__), "..", "data", "smithsonian3d", "turntables")
SPIN_BASE = "https://katospiegel-rete.hf.space/data/playground/smithsonian3d-spin"
SPIN_TOK = "token=sfdbgf1094by21hd128ru39802"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
LBL = "http://www.w3.org/2000/01/rdf-schema#label"
LIMIT = int(sys.argv[1]) if len(sys.argv) > 1 else 0

# Smithsonian unit code (prefix of the EDAN id) -> museum name.
UNIT_PREFIXES = [
    ("nmnh", "National Museum of Natural History"),
    ("nasm", "National Air and Space Museum"),
    ("nmah", "National Museum of American History"),
    ("nmaahc", "National Museum of African American History and Culture"),
    ("nmai", "National Museum of the American Indian"),
    ("npg", "National Portrait Gallery"),
    ("saam", "Smithsonian American Art Museum"),
    ("chndm", "Cooper Hewitt, Smithsonian Design Museum"),
    ("npm", "National Postal Museum"),
    ("fsg", "Freer Gallery of Art and Arthur M. Sackler Gallery"),
    ("hmsg", "Hirshhorn Museum and Sculpture Garden"),
    ("acm", "Anacostia Community Museum"),
    ("si", "Smithsonian Institution"),
]


def get(u, timeout=40):
    return urllib.request.urlopen(u, timeout=timeout).read().decode("utf-8", "replace")


def list_uuids():
    out, tok, pages = [], None, 0
    while True:
        u = f"{S3}/?list-type=2&delimiter=/&prefix=3d/&max-keys=1000"
        if tok:
            u += "&continuation-token=" + urllib.parse.quote(tok)
        xml = get(u)
        pages += 1
        out += re.findall(r"<Prefix>3d/([^/<]+)/</Prefix>", xml)
        m = re.search(r"<NextContinuationToken>([^<]+)</NextContinuationToken>", xml)
        if m and pages < 12:
            tok = m.group(1)
        else:
            break
    return out


def unit_of(edan):
    m = re.match(r"edanmdm:([a-z0-9]+?)_", edan or "")
    code = m.group(1) if m else ""
    for pre, name in UNIT_PREFIXES:
        if code.startswith(pre):
            return code, name
    return code, code or "Smithsonian Institution"


def scene(uuid):
    """Return {uuid,title,edan,glb,catalog} or None."""
    try:
        doc = json.loads(get(f"{S3}/3d/{uuid}/scene.svx.json"))
    except Exception:
        return None
    title = edan = ""
    for mt in (doc.get("metas") or []):
        coll = mt.get("collection") or {}
        title = title or coll.get("title") or coll.get("sceneTitle") or ""
        edan = edan or coll.get("edanRecordId") or ""
    # pick the Medium (Draco) Web3D .glb; fall back Low -> High
    by_q = {}
    for mdl in (doc.get("models") or []):
        for der in (mdl.get("derivatives") or []):
            if der.get("usage") != "Web3D":
                continue
            for a in (der.get("assets") or []):
                uri = str(a.get("uri", ""))
                if uri.endswith(".glb") and "nondraco" not in uri:
                    by_q[der.get("quality")] = uri
    glb = by_q.get("Medium") or by_q.get("Low") or by_q.get("High")
    if not glb or not title:
        return None
    cat = ""
    m = re.match(r"([A-Za-z]+\d[\w.-]*)", glb)  # e.g. USNM153798_cranium...
    if m:
        cat = m.group(1).split("_")[0]
    return {"uuid": uuid, "title": title, "edan": edan, "glb": glb, "catalog": cat}


def lit(s):
    return '"' + str(s).replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ").replace("\r", " ").replace("\t", " ").strip() + '"'


def main():
    uuids = list_uuids()
    sys.stderr.write(f"smithsonian3d: {len(uuids)} model folders listed\n")
    if LIMIT:
        uuids = uuids[:LIMIT]
    rows = []
    with ThreadPoolExecutor(max_workers=12) as ex:
        for r in ex.map(scene, uuids):
            if r:
                rows.append(r)
    sys.stderr.write(f"smithsonian3d: {len(rows)} models with a title + Web3D .glb\n")

    have_spin = {os.path.basename(p)[:-5] for p in glob.glob(os.path.join(SPIN_DIR, "*.webm"))}
    sys.stderr.write(f"smithsonian3d: {len(have_spin)} pre-rendered turntables found\n")

    out, seen_units = [], {}
    iri = lambda s: "<" + s + ">"
    def t(s, p, o): out.append(f"{iri(s)} {iri(p)} {o} .")
    for r in rows:
        s = BASE + "object/" + r["uuid"]
        t(s, RDF, iri(C + "Model3D"))
        t(s, LBL, lit(r["title"]))
        mesh = f"{S3}/3d/{r['uuid']}/{r['glb']}"
        t(s, P + "mesh", iri(mesh))                       # streamable .glb -> inline 3D cell
        if r["uuid"] in have_spin:                        # lightweight spin preview (no WebGL)
            t(s, P + "spinVideo", iri(f"{SPIN_BASE}/{r['uuid']}.webm?{SPIN_TOK}"))
            t(s, P + "spinGif", iri(f"{SPIN_BASE}/{r['uuid']}.gif?{SPIN_TOK}"))
        if r["edan"]:
            t(s, P + "edanId", lit(r["edan"]))
            t(s, P + "record", iri("https://www.si.edu/object/" + urllib.parse.quote(r["edan"], safe=":")))
            code, name = unit_of(r["edan"])
            if code:
                uid = BASE + "unit/" + code
                if uid not in seen_units:
                    seen_units[uid] = 1
                    t(uid, RDF, iri(C + "Unit")); t(uid, LBL, lit(name))
                t(s, P + "unit", iri(uid))
        if r["catalog"]:
            t(s, P + "catalogNumber", lit(r["catalog"]))

    sys.stderr.write(f"smithsonian3d: {len(out)} triples, {len(seen_units)} units\n")
    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
