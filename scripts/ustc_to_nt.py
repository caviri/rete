#!/usr/bin/env python3
"""Convert the harvested USTC edition subset (data/ustc/editions/*.json) into an
N-Triples graph that FEDERATES with the bph dataset.

Edition IRI = https://www.ustc.ac.uk/editions/{sn}  -- exactly what the bph graph
puts in rdfs:seeAlso, so a cross-source query joins bph book -> seeAlso -> USTC
edition. We also emit a back-link (u:embassyBook) to the bph book IRI.

Vocab: schema.org + Dublin Core + SKOS + a small ustc ontology (u:).
Entities: Edition, Person (author/printer), Library (holdings), Place, Concept
(classification), Copy (a physical exemplar with shelfmark).
"""
import argparse
import glob
import json
import os
import re

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EDS = os.path.join(ROOT, "data", "ustc", "editions")
BPH = os.path.join(ROOT, "data", "bph", "books")
OUT = os.path.join(ROOT, "data", "ustc", "ustc.nt")

UE = "https://www.ustc.ac.uk/editions/"
UID = "https://www.ustc.ac.uk/id/"
U = "https://www.ustc.ac.uk/ontology#"
EFM = "https://data.embassyofthefreemind.com/"
DCT = "http://purl.org/dc/terms/"
SCHEMA = "http://schema.org/"
SKOS = "http://www.w3.org/2004/02/skos/core#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
XSD = "http://www.w3.org/2001/XMLSchema#"

_slug = re.compile(r"[^a-z0-9]+")
_ctrl = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f  ]")


def slug(s):
    s = _slug.sub("-", (s or "").strip().lower()).strip("-")
    return s or "x"


def esc(s):
    s = _ctrl.sub(" ", str(s))
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")


def ci(u):
    u = re.sub(r"\s+", "", str(u))
    return u if u and "<" not in u and ">" not in u and '"' not in u else None


class W:
    def __init__(s, fh): s.fh = fh; s.n = 0
    def iri(s, a, p, b):
        a, b = ci(a), ci(b)
        if a and b: s.fh.write(f"<{a}> <{p}> <{b}> .\n"); s.n += 1
    def lit(s, a, p, o, lang=None, dt=None):
        a = ci(a)
        if not a: return
        o = esc(o)
        if lang: s.fh.write(f'<{a}> <{p}> "{o}"@{lang} .\n')
        elif dt: s.fh.write(f'<{a}> <{p}> "{o}"^^<{dt}> .\n')
        else: s.fh.write(f'<{a}> <{p}> "{o}" .\n')
        s.n += 1


def bph_map():
    """USTC number -> bph book IRI, for the back-link."""
    m = {}
    for f in glob.glob(os.path.join(BPH, "*.json")):
        try:
            o = json.load(open(f, encoding="utf-8"))
        except Exception:
            continue
        u = o.get("ustc_id")
        if u and o.get("id"):
            m[str(u).strip()] = f"{EFM}book/{o['id']}"
    return m


def year_of(v):
    m = re.search(r"-?\d{3,4}", str(v or ""))
    return m.group(0) if m else None


def resolve_iiif(url):
    """USTC hosts no images; it links out to digitisations. For the open,
    IIIF-capable providers, DETERMINISTICALLY construct a IIIF manifest and/or a
    cover thumbnail from the link's id (no fetching). Returns (manifest, cover),
    either may be None. Verified recipes: MDZ/BSB, Gallica, ONB, Google Books."""
    u = url or ""
    m = re.search(r"bsb\d{6,}", u)                       # Bavarian State Library / MDZ
    if m and ("digitale-sammlungen" in u or "mdz-nbn" in u or "bvb:12-bsb" in u):
        b = m.group(0)
        return (f"https://api.digitale-sammlungen.de/iiif/presentation/v2/{b}/manifest",
                f"https://api.digitale-sammlungen.de/iiif/image/v2/{b}_00001/full/300,/0/default.jpg")
    m = re.search(r"ark:/12148/([a-z0-9]+)", u)          # Gallica (BnF)
    if m and "gallica" in u:
        a = f"ark:/12148/{m.group(1)}"
        return (f"https://gallica.bnf.fr/iiif/{a}/manifest.json",
                f"https://gallica.bnf.fr/iiif/{a}/f1/full/,300/0/default.jpg")
    m = re.search(r"data\.onb\.ac\.at/ABO/(\+\w+)", u)   # Austrian National Library
    if m:
        return (f"https://iiif.onb.ac.at/presentation/ABO/{m.group(1)}/manifest", None)
    m = re.search(r"[?&]id=([\w-]+)", u)                 # Google Books (cover only, no IIIF)
    if m and "books.google" in u:
        return (None, f"https://books.google.com/books/content?id={m.group(1)}&printsec=frontcover&img=1&zoom=1")
    return (None, None)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--full", action="store_true",
                    help="read the FULL internal crawl (data/ustc/crawl/**) -> ustc-full.nt "
                         "(LOCAL only — do NOT publish)")
    a = ap.parse_args()
    if a.full:
        files = sorted(glob.glob(os.path.join(ROOT, "data", "ustc", "crawl", "**", "*.json"),
                                 recursive=True))
        out = os.path.join(ROOT, "data", "ustc", "ustc-full.nt")
    else:
        files = sorted(glob.glob(os.path.join(EDS, "*.json")))
        out = OUT
    print(f"converting {len(files)} records ({'FULL crawl' if a.full else '727 subset'}) -> {out}")
    bmap = bph_map()
    fh = open(out, "w", encoding="utf-8")
    w = W(fh)
    seen = set()
    n_ed = n_copy = 0
    for f in files:
        r = json.load(open(f, encoding="utf-8"))
        ed = r.get("edition") or {}
        sn = str(ed.get("sn") or os.path.splitext(os.path.basename(f))[0])
        E = UE + sn
        n_ed += 1
        w.iri(E, RDF + "type", SCHEMA + "Book")
        w.iri(E, RDF + "type", U + "Edition")
        title = ed.get("std_title")
        if title:
            w.lit(E, RDFS + "label", title)
            w.lit(E, DCT + "title", title)
        w.lit(E, U + "ustcNumber", sn)
        if bmap.get(sn):
            w.iri(E, U + "embassyBook", bmap[sn])   # federation back-link
        if ed.get("std_imprint"):
            w.lit(E, U + "imprint", ed["std_imprint"])
        y = year_of(ed.get("year"))
        if y:
            w.lit(E, DCT + "date", y)
            w.lit(E, SCHEMA + "datePublished", y, dt=XSD + "gYear")
        for k, p in (("format", U + "format"), ("pagination", U + "pagination"),
                     ("type", U + "documentType"), ("signatures", U + "signatures")):
            if ed.get(k):
                w.lit(E, p, ed[k])
        # authors
        for i in range(1, 9):
            nm = ed.get(f"author_name_{i}")
            if not nm:
                continue
            A = UID + "agent/" + slug(nm)
            w.lit(E, DCT + "creator", nm)
            w.iri(E, SCHEMA + "author", A)
            if A not in seen:
                seen.add(A)
                w.iri(A, RDF + "type", SCHEMA + "Person")
                w.lit(A, RDFS + "label", nm); w.lit(A, SCHEMA + "name", nm)
            role = ed.get(f"author_role_{i}")
            if role:
                w.lit(E, U + "authorRole", role)
        # printers
        for i in range(1, 5):
            nm = ed.get(f"printer_name_{i}")
            if not nm:
                continue
            P = UID + "agent/" + slug(nm)
            w.iri(E, U + "printer", P)
            if P not in seen:
                seen.add(P)
                w.iri(P, RDF + "type", SCHEMA + "Person")
                w.lit(P, RDFS + "label", nm); w.lit(P, SCHEMA + "name", nm)
        # place
        place = ed.get("place")
        if place:
            PL = UID + "place/" + slug(place)
            w.iri(E, SCHEMA + "locationCreated", PL)
            w.lit(E, U + "place", place)
            if PL not in seen:
                seen.add(PL)
                w.iri(PL, RDF + "type", SCHEMA + "Place")
                w.lit(PL, RDFS + "label", place)
                if ed.get("country"):
                    w.lit(PL, U + "country", ed["country"])
                if ed.get("region"):
                    w.lit(PL, U + "region", ed["region"])
        if ed.get("country"):
            w.lit(E, U + "country", ed["country"])
        # languages
        for i in range(1, 6):
            lg = ed.get(f"language_{i}")
            if lg:
                w.lit(E, DCT + "language", lg)
        # classifications -> subject concepts
        for i in range(1, 6):
            cl = ed.get(f"classification_{i}")
            if not cl:
                continue
            C = UID + "classification/" + slug(cl)
            w.iri(E, DCT + "subject", C)
            if C not in seen:
                seen.add(C)
                w.iri(C, RDF + "type", SKOS + "Concept")
                w.lit(C, SKOS + "prefLabel", cl); w.lit(C, RDFS + "label", cl)
        # holdings / copies
        libs_here = set()
        for cp in (r.get("copies") or []):
            libname = cp.get("name")
            if not libname:
                continue
            city = cp.get("city") or ""
            L = UID + "library/" + slug(libname + "-" + city)
            if L not in libs_here:
                libs_here.add(L)
                w.iri(E, U + "heldBy", L)
            if L not in seen:
                seen.add(L)
                w.iri(L, RDF + "type", SCHEMA + "Library")
                w.lit(L, RDFS + "label", libname)
                if city:
                    w.lit(L, U + "city", city)
                if cp.get("country"):
                    w.lit(L, U + "country", cp["country"])
            cid = cp.get("id")
            if cid:
                CO = UID + "copy/" + str(cid)
                n_copy += 1
                w.iri(E, U + "hasCopy", CO)
                w.iri(CO, RDF + "type", U + "Copy")
                w.iri(CO, U + "atLibrary", L)
                w.iri(CO, DCT + "isPartOf", E)
                if cp.get("shelfmark"):
                    w.lit(CO, U + "shelfmark", cp["shelfmark"])
        # digitisations
        for dg in (r.get("digitisations") or []):
            url = ci(dg.get("url"))
            if not url:
                continue
            w.iri(E, U + "digitalCopy", url)
            w.iri(E, RDFS + "seeAlso", url)
            if dg.get("provider"):
                w.lit(E, U + "digitisationProvider", dg["provider"])
            man, cov = resolve_iiif(dg.get("url"))       # deterministic IIIF/cover, open providers
            if man:
                w.iri(E, U + "iiifManifest", man)
            if cov:
                w.iri(E, SCHEMA + "image", cov)
                w.iri(E, U + "coverImage", cov)
        # references (bibliography)
        for rf in (r.get("references") or []):
            code = rf.get("ustc_reference_code") or ""
            det = rf.get("detail") or ""
            ttl = rf.get("title") or ""
            txt = " ".join(x for x in (code, ttl, det) if x).strip()
            if txt:
                w.lit(E, U + "bibReference", txt)
    fh.close()
    print(f"done: {n_ed} editions, {n_copy} copies, {w.n} triples -> {out}")


if __name__ == "__main__":
    main()
