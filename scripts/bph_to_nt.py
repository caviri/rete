#!/usr/bin/env python3
"""Convert harvested Embassy of the Free Mind / BPH book JSON (data/bph/books/*.json,
from scripts/fetch_bph.py) into an N-Triples knowledge graph for `rete build`.

Model (namespaces):
  efm:   https://data.embassyofthefreemind.com/           (entity base)
  efmo:  https://data.embassyofthefreemind.com/ontology#  (custom props/classes)
  dcterms, schema (schema.org), skos, wgs84 (geo), rdfs, rdf
Entities: Book, Page, Person (author), Concept (subject term / category / collection),
Place (geocoded publication/birth place). Books link to pages (images), subjects,
authors, collections, places (lat/long -> Geo tab), related books, AI significance.
"""
import glob
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BOOKS = os.path.join(ROOT, "data", "bph", "books")
OUT = os.path.join(ROOT, "data", "bph", "bph.nt")

EFM = "https://data.embassyofthefreemind.com/"
EFMO = "https://data.embassyofthefreemind.com/ontology#"
DCT = "http://purl.org/dc/terms/"
SCHEMA = "http://schema.org/"
SKOS = "http://www.w3.org/2004/02/skos/core#"
WGS = "http://www.w3.org/2003/01/geo/wgs84_pos#"
GEO = "http://www.opengis.net/ont/geosparql#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
XSD = "http://www.w3.org/2001/XMLSchema#"

_slug_re = re.compile(r"[^a-z0-9]+")


def slug(s):
    s = (s or "").strip().lower()
    s = _slug_re.sub("-", s).strip("-")
    return s or "x"


_ctrl_re = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f  ]")


def esc(s):
    s = str(s)
    s = _ctrl_re.sub(" ", s)          # drop stray control / line-separator chars
    s = s.replace("\\", "\\\\").replace('"', '\\"')
    s = s.replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")
    return s


def clean_iri(u):
    """URLs from the source occasionally contain embedded whitespace/newlines;
    an IRI may not. Strip whitespace and reject anything still malformed."""
    u = re.sub(r"\s+", "", str(u))
    if not u or "<" in u or ">" in u or '"' in u or "{" in u or "}" in u:
        return None
    return u


class W:
    def __init__(self, fh):
        self.fh = fh
        self.n = 0

    def iri(self, s, p, o):
        s = clean_iri(s); o = clean_iri(o)
        if not s or not o:
            return
        self.fh.write(f"<{s}> <{p}> <{o}> .\n"); self.n += 1

    def lit(self, s, p, o, lang=None, dt=None):
        s = clean_iri(s)
        if not s:
            return
        o = esc(o)
        if lang:
            self.fh.write(f'<{s}> <{p}> "{o}"@{lang} .\n')
        elif dt:
            self.fh.write(f'<{s}> <{p}> "{o}"^^<{dt}> .\n')
        else:
            self.fh.write(f'<{s}> <{p}> "{o}" .\n')
        self.n += 1


def year_of(o):
    y = o.get("year") or o.get("published")
    if y is None:
        return None
    m = re.search(r"-?\d{3,4}", str(y))
    return m.group(0) if m else None


def main():
    files = sorted(glob.glob(os.path.join(BOOKS, "*.json")))
    print(f"converting {len(files)} books -> {OUT}")
    emitted_terms, emitted_places, emitted_people, emitted_cats, emitted_cols = (
        set(), set(), set(), set(), set())
    fh = open(OUT, "w", encoding="utf-8")
    w = W(fh)
    nbooks = npages = 0
    for f in files:
        try:
            o = json.load(open(f, encoding="utf-8"))
        except Exception:
            continue
        bid = o.get("id") or os.path.splitext(os.path.basename(f))[0]
        B = f"{EFM}book/{bid}"
        nbooks += 1
        w.iri(B, RDF + "type", SCHEMA + "Book")
        w.iri(B, RDF + "type", EFMO + "Book")
        title = o.get("title")
        disp = o.get("display_title") or o.get("english_title")
        label = disp or title or bid
        w.lit(B, RDFS + "label", label)
        if title:
            w.lit(B, DCT + "title", title)
            w.lit(B, EFMO + "originalTitle", title)
        if disp:
            w.lit(B, SCHEMA + "name", disp, lang="en")
        if o.get("author"):
            w.lit(B, DCT + "creator", o["author"])
            aid = o.get("author_id") or slug(o["author"])
            A = f"{EFM}author/{aid}"
            w.iri(B, SCHEMA + "author", A)
            if aid not in emitted_people:
                emitted_people.add(aid)
                w.iri(A, RDF + "type", SCHEMA + "Person")
                w.lit(A, RDFS + "label", o["author"])
                w.lit(A, SCHEMA + "name", o["author"])
        y = year_of(o)
        if y:
            w.lit(B, DCT + "date", y)
            w.lit(B, SCHEMA + "datePublished", y, dt=XSD + "gYear")
        for k, p in (("publisher", DCT + "publisher"), ("place_published", EFMO + "placePublished"),
                     ("printer", EFMO + "printer"), ("format", EFMO + "format"),
                     ("language", DCT + "language")):
            if o.get(k):
                w.lit(B, p, o[k])
        if o.get("pages_count"):
            w.lit(B, SCHEMA + "numberOfPages", o["pages_count"], dt=XSD + "integer")
        if o.get("description"):
            w.lit(B, DCT + "description", o["description"])
        # AI enrichment
        summ = (o.get("summary") or {}).get("data") if isinstance(o.get("summary"), dict) else o.get("summary")
        if summ:
            w.lit(B, EFMO + "summary", summ)
        rs = (o.get("reading_summary") or {}).get("overview") if isinstance(o.get("reading_summary"), dict) else None
        if rs:
            w.lit(B, EFMO + "readingSummary", rs)
        qa = ((o.get("quality_assessment") or {}).get("ai_scores") or {})
        for dim, prop in (("historical_significance", "historicalSignificance"),
                          ("scholarly_value", "scholarlyValue"),
                          ("visual_appeal", "visualAppeal"),
                          ("accessibility", "accessibility")):
            d = qa.get(dim) or {}
            if isinstance(d, dict) and d.get("score") is not None:
                w.lit(B, EFMO + prop, d["score"], dt=XSD + "integer")
        if o.get("quality_score") is not None:
            w.lit(B, EFMO + "qualityScore", o["quality_score"], dt=XSD + "integer")
        # bibliographic extras
        md = o.get("metadata") or {}
        if md.get("shelf_mark"):
            w.lit(B, EFMO + "shelfmark", md["shelf_mark"])
        if md.get("bibliography"):
            w.lit(B, EFMO + "bibliography", md["bibliography"])
        if o.get("ustc_id"):
            w.lit(B, EFMO + "ustcId", o["ustc_id"])
            w.iri(B, RDFS + "seeAlso", f"https://www.ustc.ac.uk/editions/{o['ustc_id']}")
        # images + links
        cover = (o.get("image_thumb") or o.get("thumbnail_blob")
                 or o.get("thumbnail") or o.get("image_display"))
        if cover:
            w.iri(B, SCHEMA + "image", cover)
            w.iri(B, EFMO + "coverImage", cover)
        w.iri(B, EFMO + "iiifManifest", f"https://sourcelibrary.org/api/iiif/{bid}/manifest")
        if o.get("slug"):
            w.iri(B, RDFS + "seeAlso", f"https://www.embassyofthefreemind.com/digital-collection-search?view=books&book={o['slug']}")
        w.lit(B, EFMO + "contributingLibrary", "Bibliotheca Philosophica Hermetica")
        # categories -> concept
        for c in (o.get("categories") or []):
            cs = slug(c); C = f"{EFM}category/{cs}"
            w.iri(B, DCT + "subject", C)
            if cs not in emitted_cats:
                emitted_cats.add(cs)
                w.iri(C, RDF + "type", SKOS + "Concept")
                w.lit(C, SKOS + "prefLabel", c); w.lit(C, RDFS + "label", c)
        # collections
        for c in (o.get("collections") or []):
            cs = slug(c); C = f"{EFM}collection/{cs}"
            w.iri(B, EFMO + "inCollection", C)
            if cs not in emitted_cols:
                emitted_cols.add(cs)
                w.iri(C, RDF + "type", EFMO + "Collection")
                w.lit(C, RDFS + "label", c.replace("-", " "))
        # subject index terms
        for t in ((o.get("index") or {}).get("vocabulary") or [])[:200]:
            term = t.get("term") if isinstance(t, dict) else t
            if not term:
                continue
            ts = slug(term); T = f"{EFM}term/{ts}"
            w.iri(B, DCT + "subject", T)
            if ts not in emitted_terms:
                emitted_terms.add(ts)
                w.iri(T, RDF + "type", SKOS + "Concept")
                w.lit(T, SKOS + "prefLabel", term); w.lit(T, RDFS + "label", term)
        # geocoded places
        for loc in (o.get("locations") or []):
            if loc.get("lat") is None or loc.get("lng") is None:
                continue
            city = loc.get("city") or ""; country = loc.get("country") or ""
            plabel = ", ".join([x for x in (city, country) if x]) or "place"
            ps = slug(plabel); P = f"{EFM}place/{ps}"
            prop = SCHEMA + "locationCreated" if loc.get("type") == "publication" else EFMO + "relatedPlace"
            w.iri(B, prop, P)
            if ps not in emitted_places:
                emitted_places.add(ps)
                w.iri(P, RDF + "type", SCHEMA + "Place")
                w.lit(P, RDFS + "label", plabel)
                w.lit(P, WGS + "lat", loc["lat"], dt=XSD + "decimal")
                w.lit(P, WGS + "long", loc["lng"], dt=XSD + "decimal")
                w.lit(P, GEO + "asWKT", f"POINT({loc['lng']} {loc['lat']})", dt=GEO + "wktLiteral")
        # related books
        for rb in ((o.get("related_books") or {}).get("direct") or []):
            rid = rb.get("id")
            if rid:
                w.iri(B, EFMO + "relatedBook", f"{EFM}book/{rid}")
        # chapters (structure)
        for i, ch in enumerate((o.get("chapters") or [])):
            if not ch.get("title") and not ch.get("titleEn"):
                continue
            CH = f"{EFM}book/{bid}/chapter/{i}"
            w.iri(B, EFMO + "hasChapter", CH)
            w.iri(CH, RDF + "type", EFMO + "Chapter")
            w.lit(CH, RDFS + "label", ch.get("titleEn") or ch.get("title"))
            if ch.get("title"):
                w.lit(CH, EFMO + "originalTitle", ch["title"])
            if ch.get("pageNumber") is not None:
                w.lit(CH, EFMO + "startPage", ch["pageNumber"], dt=XSD + "integer")
        # pages (image URLs)
        for pg in (o.get("pages") or []):
            pid = pg.get("id")
            if not pid:
                continue
            P = f"{EFM}page/{pid}"
            img = pg.get("display_photo") or pg.get("photo") or pg.get("cropped_photo")
            if not img:
                continue
            npages += 1
            w.iri(P, RDF + "type", EFMO + "Page")
            w.iri(P, SCHEMA + "isPartOf", B)
            w.iri(B, EFMO + "hasPage", P)
            pn = pg.get("page_number")
            if pn is not None:
                w.lit(P, EFMO + "pageNumber", pn, dt=XSD + "integer")
            w.iri(P, SCHEMA + "image", img)
            th = pg.get("thumbnail") or pg.get("image_thumb")
            if th:
                w.iri(P, EFMO + "thumbnail", th)
    fh.close()
    print(f"done: {nbooks} books, {npages} pages, {w.n} triples -> {OUT}")
    print(f"  terms={len(emitted_terms)} places={len(emitted_places)} people={len(emitted_people)} "
          f"categories={len(emitted_cats)} collections={len(emitted_cols)}")


if __name__ == "__main__":
    main()
