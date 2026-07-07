#!/usr/bin/env python3
"""Convert the unified BCUL records (normalized/bcul.jsonl) to N-Triples for `rete build`.

Builds a GRAPH, not flat rows: creators, subjects, places, languages and collections
become shared IRI nodes, so the same author/subject/fonds links all the works it
touches — that shared structure is the "digital twin" network.

Vocabularies: schema.org + Dublin Core Terms (+ rdf/rdfs). IIIF manifests and
thumbnails are emitted as IRI objects so the rete playground renders them inline.

Usage: python jsonl_to_nt.py [--in bcul.jsonl] [--out bcul.nt]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import urllib.parse
from pathlib import Path

# A valid N-Triples IRIREF: a scheme, then no space/control/<>"{}|^`\ characters.
IRI_OK = re.compile(r'^[A-Za-z][A-Za-z0-9+.\-]*:[^\x00-\x20<>"{}|^`\\]*$')


def clean_iri(url):
    """Return a valid IRI, or None. Dirty MARC 856 fields hold junk (raw spaces,
    even embedded HTML/PHP); percent-encode what's fixable (e.g. mailto with
    spaces) and drop the rest so the N-Triples stays parseable."""
    if not url:
        return None
    u = url.strip()
    if not u:
        return None
    if IRI_OK.match(u):
        return u
    enc = urllib.parse.quote(u, safe="/:?#[]@!$&'()*+,;=~._-%")
    return enc if IRI_OK.match(enc) else None

REPO = Path(__file__).resolve().parents[2]
BASE = "https://data.bcu-lausanne.ch/"
SCHEMA = "http://schema.org/"
DCT = "http://purl.org/dc/terms/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
XSD = "http://www.w3.org/2001/XMLSchema#"

# unified type -> schema.org class
TYPE_MAP = {
    "text": "Book", "serial": "Periodical", "manuscript-text": "Manuscript",
    "manuscript-music": "Manuscript", "manuscript-cartographic": "Manuscript",
    "cartographic": "Map", "still-image": "ImageObject", "moving-image": "VideoObject",
    "sound-music": "MusicRecording", "sound-nonmusic": "AudioObject",
    "notated-music": "MusicComposition", "electronic": "SoftwareApplication",
    "object": "CreativeWork", "archive": "ArchiveComponent", "mixed-material": "ArchiveComponent",
    "collection": "Collection", "issue": "PublicationIssue", "kit": "CreativeWork",
}

_slug_re = re.compile(r"[^a-z0-9]+")


def h(text: str) -> str:
    return hashlib.sha1(text.strip().lower().encode("utf-8")).hexdigest()[:16]


def esc(s: str) -> str:
    return (s.replace("\\", "\\\\").replace('"', '\\"')
            .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


class Writer:
    def __init__(self, fh):
        self.fh = fh
        self.n = 0
        self._agents = set()
        self._concepts = set()
        self._places = set()
        self._collections = set()
        self._langs = set()
        self._libraries = set()

    def triple(self, s, p, o):
        self.fh.write(f"<{s}> <{p}> {o} .\n")
        self.n += 1

    def lit(self, s, p, value, lang=None, dt=None):
        v = f'"{esc(str(value))}"'
        if lang:
            v += f"@{lang}"
        elif dt:
            v += f"^^<{dt}>"
        self.triple(s, p, v)

    def iri(self, s, p, o):
        self.triple(s, p, f"<{o}>")

    # shared nodes (emit label once)
    def agent(self, name):
        u = f"{BASE}agent/{h(name)}"
        if name not in self._agents:
            self._agents.add(name)
            self.iri(u, RDF + "type", SCHEMA + "Person")
            self.lit(u, SCHEMA + "name", name)
            self.lit(u, RDFS + "label", name)
        return u

    def concept(self, term):
        u = f"{BASE}concept/{h(term)}"
        if term not in self._concepts:
            self._concepts.add(term)
            self.iri(u, RDF + "type", SCHEMA + "DefinedTerm")
            self.lit(u, RDFS + "label", term)
        return u

    def place(self, name):
        u = f"{BASE}place/{h(name)}"
        if name not in self._places:
            self._places.add(name)
            self.iri(u, RDF + "type", SCHEMA + "Place")
            self.lit(u, SCHEMA + "name", name)
        return u

    def collection(self, name):
        u = f"{BASE}collection/{h(name)}"
        if name not in self._collections:
            self._collections.add(name)
            self.iri(u, RDF + "type", SCHEMA + "Collection")
            self.lit(u, RDFS + "label", name)
        return u

    def library(self, name):
        u = f"{BASE}library/{h(name)}"
        if name not in self._libraries:
            self._libraries.add(name)
            self.iri(u, RDF + "type", SCHEMA + "Library")
            self.lit(u, SCHEMA + "name", name)
            self.lit(u, RDFS + "label", name)
        return u

    def language(self, code):
        u = f"{BASE}language/{_slug_re.sub('-', code.strip().lower())}"
        if code not in self._langs:
            self._langs.add(code)
            self.iri(u, RDF + "type", SCHEMA + "Language")
            self.lit(u, RDFS + "label", code)
        return u


def subject_iri(rec) -> str:
    src, lid = rec["source"], rec["local_id"]
    return clean_iri(rec.get("record_url")) or f"{BASE}item/{src}/{lid}"


def convert(rec, w: Writer):
    s = subject_iri(rec)
    cls = TYPE_MAP.get(rec.get("type"), "CreativeWork")
    w.iri(s, RDF + "type", SCHEMA + cls)
    w.lit(s, DCT + "type", rec.get("type") or "")
    w.lit(s, SCHEMA + "identifier", rec["id"])
    w.iri(s, SCHEMA + "isPartOf", f"{BASE}source/{rec['source']}")

    if rec.get("title"):
        w.lit(s, SCHEMA + "name", rec["title"])
        w.lit(s, DCT + "title", rec["title"])
    if rec.get("title_full"):
        w.lit(s, SCHEMA + "alternateName", rec["title_full"])
    if rec.get("description"):
        w.lit(s, SCHEMA + "description", rec["description"][:2000])

    for c in rec.get("creators", []):
        name = c.get("name")
        if not name:
            continue
        a = w.agent(name)
        w.iri(s, SCHEMA + "creator" if c.get("main") else SCHEMA + "contributor", a)
        if c.get("role"):
            w.lit(s, BASE + "role", c["role"])

    pub = rec.get("publication") or {}
    if pub.get("publisher"):
        w.lit(s, SCHEMA + "publisher", pub["publisher"])
    if pub.get("place"):
        w.iri(s, SCHEMA + "locationCreated", w.place(pub["place"]))
    if pub.get("date"):
        w.lit(s, DCT + "date", pub["date"])
    ds, de = rec.get("date_start"), rec.get("date_end")
    if isinstance(ds, int) and 100 <= ds <= 2035:
        w.lit(s, SCHEMA + "startDate", ds, dt=XSD + "gYear")
    if isinstance(de, int) and 100 <= de <= 2035:
        w.lit(s, SCHEMA + "endDate", de, dt=XSD + "gYear")

    for lg in rec.get("languages", []):
        w.iri(s, SCHEMA + "inLanguage", w.language(lg))
    for subj in rec.get("subjects", []):
        w.iri(s, SCHEMA + "about", w.concept(subj))
    for g in rec.get("genres", []):
        w.lit(s, SCHEMA + "genre", g)
    for pl in rec.get("places", []):
        w.iri(s, SCHEMA + "spatialCoverage", w.place(pl))
    for col in rec.get("collections", []):
        w.iri(s, DCT + "isPartOf", w.collection(col))

    # holdings — WHERE the item physically lives (Alma AVA/AVE)
    for hd in rec.get("holdings", []):
        lib = hd.get("library")
        if lib:
            w.iri(s, BASE + "heldAt", w.library(lib))
        if hd.get("call_number"):
            w.lit(s, BASE + "callNumber", hd["call_number"])
        if hd.get("availability"):
            w.lit(s, BASE + "availability", hd["availability"])
        if hd.get("location"):
            w.lit(s, BASE + "shelvingLocation", hd["location"])
        hu = clean_iri(hd.get("url"))
        if hu:
            w.iri(s, SCHEMA + "url", hu)

    if rec.get("shelfmark"):
        w.lit(s, BASE + "shelfmark", rec["shelfmark"])
    if rec.get("extent"):
        w.lit(s, DCT + "extent", rec["extent"])
    if rec.get("rights"):
        w.lit(s, DCT + "rights", rec["rights"])
    if rec.get("provider"):
        w.lit(s, SCHEMA + "provider", rec["provider"])
    ru = clean_iri(rec.get("record_url"))
    if ru:
        w.iri(s, SCHEMA + "url", ru)
    mani = clean_iri(rec.get("iiif_manifest"))
    if mani:
        w.iri(s, BASE + "iiifManifest", mani)
    thumb = clean_iri(rec.get("thumbnail_url"))
    if thumb:
        w.iri(s, SCHEMA + "thumbnailUrl", thumb)
    for fobj in rec.get("files", []):
        furl = clean_iri(fobj.get("url"))
        if furl:
            w.iri(s, SCHEMA + "associatedMedia", furl)

    ids = rec.get("identifiers") or {}
    for k in ("isbn", "issn", "doi", "rero"):
        for v in (ids.get(k) or []):
            w.lit(s, DCT + "identifier", f"{k}:{v}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inp", default=str(REPO / "data" / "bcul" / "normalized" / "bcul.jsonl"))
    ap.add_argument("--out", default=str(REPO / "data" / "bcul" / "bcul.nt"))
    args = ap.parse_args()

    n_rec = 0
    with open(args.inp, encoding="utf-8") as fin, open(args.out, "w", encoding="utf-8") as fout:
        # source nodes
        w = Writer(fout)
        for src in ("patrinum", "renouvaud", "ecodices", "scriptorium"):
            u = f"{BASE}source/{src}"
            w.iri(u, RDF + "type", SCHEMA + "Collection")
            w.lit(u, RDFS + "label", f"BCU Lausanne — {src}")
        for line in fin:
            line = line.strip()
            if not line:
                continue
            convert(json.loads(line), w)
            n_rec += 1
            if n_rec % 100000 == 0:
                print(f"  {n_rec:,} records -> {w.n:,} triples", flush=True)
    print(f"done: {n_rec:,} records -> {w.n:,} triples -> {args.out}")


if __name__ == "__main__":
    main()
