#!/usr/bin/env python3
"""Bioexplora (Museu de Ciències Naturals de Barcelona) -> N-Triples.

Builds one RDF graph from the museum's open data (all CC BY 4.0 unless noted):
  * SPECIMENS — the 6 MCNB Darwin Core archives harvested from GBIF/IPT
    (data/bioexplora/dwca/<r>/occurrence.txt + multimedia.txt). Every non-empty
    Darwin Core column is emitted with its REAL term IRI (http://rs.tdwg.org/dwc/
    terms/<term>, dcterms: for the Dublin Core ones) — i.e. we reuse THEIR
    properties. Georeferenced records get a GeoSPARQL POINT; imaged records get
    prop:image -> their IIIF URL (iiif.coeli.cat, CORS-open).
  * 3D MODELS — data/bioexplora/models3d.json (Sketchfab account laboratorinatura,
    the "Atles osteològic" skull scans): prop:sketchfab -> the Sketchfab viewer URL
    (the 3D cell renders it as a launch link).
  * AUDIO — data/bioexplora/audio.json (Eloïsa Matheu nature recordings via
    Xeno-canto; CC BY-NC-ND, NOT MCNB): prop:audio -> the CORS-safe mp3 (audio cell).

A small ontology (TBox) is emitted at the top: bioexplora classes + human labels
for the Darwin Core / media properties used, so the Schema view and Labels read.

Usage: python3 scripts/bioexplora_to_nt.py > data/bioexplora/bioexplora.nt
"""
import csv, json, os, re, sys, urllib.parse


# Coeli media URLs come in two flavours — app.coeli.cat/.../portraitMedia and
# iiif.coeli.cat/.../default.jpg. The iiif endpoint is flaky (503s); the
# portraitMedia one 303-redirects to the CORS-open S3 original and is reliable,
# so normalise every image to it (this also collapses the two URLs of one object
# into a single prop:image). prop:preview points to our bucket WebP mirror.
def coeli_nid(url):
    m = re.search(r"/HeritageObject/(N\w+?)/", url)
    return m.group(1) if m else None


def coeli_portrait(url):
    nid = coeli_nid(url)
    return ("https://app.coeli.cat/coeli/ICUB-NAT/HeritageObject/%s/portraitMedia" % nid) if nid else url


def load_image_mirror():
    """uid -> bucket WebP URL, from scripts/bioexplora_images.sh's TSV (if present)."""
    mp = os.path.join(os.path.dirname(__file__), "..", "data", "bioexplora", "images.tsv")
    out = {}
    if os.path.isfile(mp):
        for line in open(mp, encoding="utf-8"):
            parts = line.rstrip("\n").split("\t")
            if len(parts) == 2:
                out[parts[0]] = parts[1]
    return out

csv.field_size_limit(10 * 1024 * 1024)
sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DWCA = os.path.join(HERE, "data", "bioexplora", "dwca")
DATA = os.path.join(HERE, "data", "bioexplora")

BASE = "https://bioexplora.cat/"
P = BASE + "prop/"
C = BASE + "class/"
DWC = "http://rs.tdwg.org/dwc/terms/"
DCT = "http://purl.org/dc/terms/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
LBL = RDFS + "label"
WKT = "http://www.opengis.net/ont/geosparql#asWKT"
WKT_DT = "http://www.opengis.net/ont/geosparql#wktLiteral"

# Darwin Core columns that are actually Dublin Core terms (everything else -> dwc:).
DC_TERMS = {"type", "modified", "language", "license", "rights", "rightsHolder",
            "accessRights", "bibliographicCitation", "references", "source",
            "identifier", "created", "creator", "format", "title", "description",
            "publisher"}
SKIP_COLS = {"id", "institutionID", "collectionID", "datasetID"}

# Human labels for the properties used (the authored ontology layer).
PROP_LABELS = {
    DWC + "scientificName": "scientific name", DWC + "vernacularName": "common name",
    DWC + "kingdom": "kingdom", DWC + "phylum": "phylum", DWC + "class": "class",
    DWC + "order": "order", DWC + "family": "family", DWC + "genus": "genus",
    DWC + "subgenus": "subgenus", DWC + "specificEpithet": "species epithet",
    DWC + "taxonRank": "taxon rank", DWC + "scientificNameAuthorship": "name authorship",
    DWC + "catalogNumber": "catalogue number", DWC + "otherCatalogNumbers": "other catalogue numbers",
    DWC + "recordNumber": "record number", DWC + "recordedBy": "collector",
    DWC + "individualCount": "individual count", DWC + "sex": "sex", DWC + "lifeStage": "life stage",
    DWC + "occurrenceStatus": "occurrence status", DWC + "preparations": "preparations",
    DWC + "eventDate": "date", DWC + "verbatimEventDate": "verbatim date", DWC + "habitat": "habitat",
    DWC + "continent": "continent", DWC + "country": "country", DWC + "countryCode": "country code",
    DWC + "stateProvince": "state/province", DWC + "county": "county", DWC + "municipality": "municipality",
    DWC + "locality": "locality", DWC + "verbatimLocality": "verbatim locality", DWC + "island": "island",
    DWC + "waterBody": "water body", DWC + "decimalLatitude": "latitude", DWC + "decimalLongitude": "longitude",
    DWC + "coordinateUncertaintyInMeters": "coordinate uncertainty (m)",
    DWC + "minimumElevationInMeters": "min elevation (m)", DWC + "maximumElevationInMeters": "max elevation (m)",
    DWC + "minimumDepthInMeters": "min depth (m)", DWC + "maximumDepthInMeters": "max depth (m)",
    DWC + "typeStatus": "type status", DWC + "identifiedBy": "determiner", DWC + "dateIdentified": "date identified",
    DWC + "basisOfRecord": "basis of record", DWC + "collectionCode": "collection", DWC + "institutionCode": "institution",
    DWC + "datasetName": "dataset", DWC + "occurrenceID": "occurrence id",
    DCT + "license": "license", DCT + "modified": "modified", DCT + "language": "language",
    P + "image": "image", P + "preview": "photo (WebP preview)", P + "sketchfab": "3D model (Sketchfab)", P + "audio": "audio recording",
    P + "mesh": "3D model", P + "thumbnail": "thumbnail", P + "faceCount": "faces", P + "vertexCount": "vertices",
    # the connected-graph layer: shared Taxon / Agent / Place nodes
    P + "taxon": "taxon", P + "parentTaxon": "parent taxon", P + "rank": "rank",
    P + "collectedBy": "collected by", P + "foundIn": "found in",
}

out = sys.stdout
def w(line): out.write(line + "\n")
def iri(s): return "<" + s + ">"
def esc(s):
    return str(s).replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ").replace("\r", " ").replace("\t", " ")
def lit(s): return '"' + esc(s) + '"'
def t(s, p, o): w(iri(s) + " " + iri(p) + " " + o + " .")
def tl(s, p, o): t(s, p, lit(o))
def localname_subject(coll, cat, fallback):
    key = (coll + "/" + cat) if (coll and cat) else (cat or fallback)
    return BASE + "specimen/" + urllib.parse.quote(key, safe="/")


def emit_ontology():
    OWL = "http://www.w3.org/2002/07/owl#"
    for cls, label, comment in [
        ("Specimen", "Specimen", "A natural-history specimen / Darwin Core occurrence held by the MCNB."),
        ("Image", "Image", "A specimen image (IIIF)."),
        ("Model3D", "3D model", "A 3D scan (Sketchfab, Atles osteologic)."),
        ("Recording", "Sound recording", "A nature sound recording (Xeno-canto, E. Matheu; CC BY-NC-ND)."),
        ("Taxon", "Taxon", "A taxonomic name at one rank (kingdom … species), linked to its parent rank via parentTaxon — so the taxonomy is a navigable tree, not flat literals."),
        ("Agent", "Collector", "A person or team that collected specimens (from dwc:recordedBy); shared across all of their records."),
        ("Place", "Place", "A country where specimens were collected; shared across all records from there."),
    ]:
        t(C + cls, RDF, iri(OWL + "Class"))
        tl(C + cls, LBL, label)
        tl(C + cls, RDFS + "comment", comment)
    # Object properties carry rdfs:domain/range so the Schema view draws the real
    # graph (Specimen → Taxon, Specimen → Collector, Taxon → Taxon, …).
    for pid, dom, rng in [
        (P + "taxon", "Specimen", "Taxon"), (P + "parentTaxon", "Taxon", "Taxon"),
        (P + "collectedBy", "Specimen", "Agent"), (P + "foundIn", "Specimen", "Place"),
        (P + "image", "Specimen", "Image"), (P + "preview", "Specimen", "Image"),
    ]:
        t(pid, RDF, iri(OWL + "ObjectProperty"))
        t(pid, RDFS + "domain", iri(C + dom))
        t(pid, RDFS + "range", iri(C + rng))
    for pid, label in PROP_LABELS.items():
        tl(pid, LBL, label)


def parse_occurrences():
    n_spec = n_img = n_geo = n_prev = 0
    mirror = load_image_mirror()
    # Shared nodes for the connected-graph layer (deduped across all records).
    taxa, agents, places = {}, {}, {}

    def node_once(reg, kind, key, label):
        node = reg.get(key)
        if node is None:
            node = BASE + kind + "/" + urllib.parse.quote(key, safe="/")
            reg[key] = node
            t(node, RDF, iri(C + {"taxon": "Taxon", "agent": "Agent", "place": "Place"}[kind]))
            tl(node, LBL, label)
            node_once.fresh = node  # signal "newly created" to the caller
        else:
            node_once.fresh = None
        return node

    for r in sorted(os.listdir(DWCA)):
        occ = os.path.join(DWCA, r, "occurrence.txt")
        if not os.path.isfile(occ):
            continue
        # multimedia (images) keyed by the core record id
        media = {}
        mm = os.path.join(DWCA, r, "multimedia.txt")
        if os.path.isfile(mm):
            with open(mm, encoding="utf-8", errors="replace") as f:
                for row in csv.DictReader(f, delimiter="\t"):
                    url = (row.get("identifier") or "").strip()
                    if url.startswith("http"):
                        media.setdefault(row.get("id", ""), []).append(url)
        with open(occ, encoding="utf-8", errors="replace") as f:
            for row in csv.DictReader(f, delimiter="\t"):
                cat = (row.get("catalogNumber") or "").strip()
                coll = (row.get("collectionCode") or "").strip()
                rid = row.get("id", "")
                subj = localname_subject(coll, cat, rid)
                t(subj, RDF, iri(C + "Specimen"))
                n_spec += 1
                sn = (row.get("scientificName") or "").strip()
                label = (sn + (" — " + cat if cat else "")).strip(" —") or rid
                tl(subj, LBL, label)
                for col, val in row.items():
                    if not val or col in SKIP_COLS:
                        continue
                    val = val.strip()
                    if not val:
                        continue
                    pid = (DCT if col in DC_TERMS else DWC) + col
                    tl(subj, pid, val)
                # GeoSPARQL point (lon lat)
                la, lo = (row.get("decimalLatitude") or "").strip(), (row.get("decimalLongitude") or "").strip()
                if la and lo:
                    try:
                        float(la); float(lo)
                        t(subj, WKT, '"POINT(%s %s)"^^%s' % (lo, la, iri(WKT_DT)))
                        n_geo += 1
                    except ValueError:
                        pass
                # Connected graph: a taxonomy tree (specimen → species → genus →
                # family → … via parentTaxon), the collector, and the country —
                # shared nodes, so you can traverse instead of matching strings.
                parent = finest = None
                for col, rank in (("kingdom", "kingdom"), ("phylum", "phylum"), ("class", "class"),
                                  ("order", "order"), ("family", "family"), ("genus", "genus"),
                                  ("scientificName", "species")):
                    val = (row.get(col) or "").strip()
                    if not val:
                        continue
                    node = node_once(taxa, "taxon", rank + "/" + val, val)
                    if node_once.fresh:
                        tl(node, P + "rank", rank)
                        if parent:
                            t(node, P + "parentTaxon", iri(parent))
                    parent = finest = node
                if finest:
                    t(subj, P + "taxon", iri(finest))
                rb = (row.get("recordedBy") or "").strip()
                if rb and len(rb) <= 160:
                    t(subj, P + "collectedBy", iri(node_once(agents, "agent", rb, rb)))
                co = (row.get("country") or "").strip()
                if co and len(co) <= 80:
                    t(subj, P + "foundIn", iri(node_once(places, "place", co, co)))
                seen = set()
                for url in media.get(rid, []):
                    p = coeli_portrait(url)          # reliable S3-backed source URL
                    if p in seen:
                        continue
                    seen.add(p)
                    t(subj, P + "image", iri(p)); n_img += 1
                    nid = coeli_nid(url)
                    if nid and nid in mirror:         # our fast, durable WebP mirror
                        t(subj, P + "preview", iri(mirror[nid])); n_prev += 1
        sys.stderr.write("  %-14s done\n" % r)
    sys.stderr.write("specimens: %d, image links: %d (%d webp previews), georeferenced: %d\n" % (n_spec, n_img, n_prev, n_geo))
    sys.stderr.write("graph: %d taxa, %d collectors, %d places\n" % (len(taxa), len(agents), len(places)))


def emit_3d():
    path = os.path.join(DATA, "models3d.json")
    if not os.path.isfile(path):
        sys.stderr.write("3d: models3d.json not present yet — skipped\n"); return
    models = json.load(open(path, encoding="utf-8"))
    # Streamable .glb meshes we mirrored to the bucket (scripts/bioexplora_sketchfab.sh
    # downloads each downloadable model, Draco+webp compresses it ~40x and uploads it);
    # meshes.tsv maps uid -> the bucket URL the playground renders inline (3D cell).
    mesh = {}
    mp = os.path.join(DATA, "meshes.tsv")
    if os.path.isfile(mp):
        for line in open(mp, encoding="utf-8"):
            parts = line.rstrip("\n").split("\t")
            if len(parts) == 2:
                mesh[parts[0]] = parts[1]
    n = m3 = 0
    for m in models:
        uid = m.get("uid")
        if not uid:
            continue
        s = BASE + "model3d/" + uid
        t(s, RDF, iri(C + "Model3D")); n += 1
        tl(s, LBL, m.get("name") or uid)
        t(s, P + "sketchfab", iri(m.get("viewerUrl") or ("https://sketchfab.com/models/" + uid)))
        if uid in mesh:
            t(s, P + "mesh", iri(mesh[uid])); m3 += 1
        if m.get("thumbnail"):
            t(s, P + "thumbnail", iri(m["thumbnail"]))
        for k, pid in (("faceCount", P + "faceCount"), ("vertexCount", P + "vertexCount")):
            if m.get(k):
                tl(s, pid, str(m[k]))
        if m.get("license"):
            tl(s, DCT + "license", str(m["license"]))
    sys.stderr.write("3d models: %d (%d with an inline bucket mesh)\n" % (n, m3))


def emit_audio():
    path = os.path.join(DATA, "audio.json")
    if not os.path.isfile(path):
        sys.stderr.write("audio: audio.json not present yet — skipped\n"); return
    recs = json.load(open(path, encoding="utf-8"))
    n = 0
    for r in recs:
        key = r.get("gbifID") or r.get("key") or r.get("references", "")
        if not key:
            continue
        s = BASE + "recording/" + urllib.parse.quote(str(key), safe="")
        t(s, RDF, iri(C + "Recording")); n += 1
        sn = r.get("scientificName") or ""
        tl(s, LBL, (sn + (" — " + r["vernacularName"] if r.get("vernacularName") else "")).strip(" —") or str(key))
        if r.get("audioUrl"):
            t(s, P + "audio", iri(r["audioUrl"]))
        for k, pid in (("scientificName", DWC + "scientificName"), ("vernacularName", DWC + "vernacularName"),
                       ("class", DWC + "class"), ("order", DWC + "order"), ("family", DWC + "family"),
                       ("recordedBy", DWC + "recordedBy"), ("country", DWC + "country"),
                       ("stateProvince", DWC + "stateProvince"), ("locality", DWC + "locality"),
                       ("eventDate", DWC + "eventDate"), ("behavior", DWC + "behavior"),
                       ("license", DCT + "license"), ("rightsHolder", DCT + "rightsHolder"),
                       ("references", DCT + "references")):
            if r.get(k):
                tl(s, pid, str(r[k]))
        la, lo = r.get("decimalLatitude"), r.get("decimalLongitude")
        if la and lo:
            try:
                t(s, WKT, '"POINT(%s %s)"^^%s' % (float(lo), float(la), iri(WKT_DT)))
            except (ValueError, TypeError):
                pass
    sys.stderr.write("recordings: %d\n" % n)


emit_ontology()
parse_occurrences()
emit_3d()
emit_audio()
