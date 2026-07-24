#!/usr/bin/env python3
"""ETH Research Collection (DSpace 7.6 OAI harvest) -> N-Triples scholarly graph.

Two-pass join, both sources keyed by DSpace handle:
  pass 1  raw/xoai/       -> enrichment index: ORIGINAL-bundle file manifest,
                             DSpace entity-relationship edges, entity type.
  pass 2  raw/oai_ethz/   -> emit the graph (works, authors+ORCID, journals,
                             ETH affiliation tree, grants, identifiers, subjects,
                             licence), attaching the pass-1 enrichment.

Canonical IRIs (federates with the rete scholar hub for free):
  work w/ DOI -> https://doi.org/{doi}   else https://hdl.handle.net/{handle}
  person w/ ORCID -> https://orcid.org/{orcid}   else .../ethz/person/{slug}
  serial w/ ISSN -> https://portal.issn.org/resource/ISSN/{issn}
  ETH unit -> .../ethz/unit/{leitzahl-code}   file -> .../ethz/bitstream/{uuid}
  relationship target -> .../ethz/entity/{uuid}

Stdlib only. Run in Docker; emits UTF-8 N-Triples to stdout.
"""

import gzip
import os
import re
import sys
import unicodedata
import xml.etree.ElementTree as ET
from pathlib import Path

MAXPAGES = int(os.environ.get("ETHZ_MAXPAGES", "0"))  # 0 = all; >0 for smoke tests

OAI = "{http://www.openarchives.org/OAI/2.0/}"
RAW = Path("data/ethz-research-collection/raw")
ETHZ = "https://w3id.org/rete/ethz#"
LOCAL = "https://w3id.org/rete/ethz/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
FOAF_NAME = "http://xmlns.com/foaf/0.1/name"
DCT = "http://purl.org/dc/terms/"
BIBO = "http://purl.org/ontology/bibo/"
ORG_SUBORG = "http://www.w3.org/ns/org#subOrganizationOf"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"

out = sys.stdout.buffer
seen = set()  # dedup for shared nodes (persons, journals, units, grants, entities)

# ---- N-Triples emit helpers (bytes, UTF-8) ----------------------------------
_ESC = {"\\": "\\\\", '"': '\\"', "\n": " ", "\r": " ", "\t": " "}
def esc(s): return "".join(_ESC.get(c, c) for c in str(s))
def valid_iri(s): return s and " " not in s and "\n" not in s and "\t" not in s and '"' not in s and "<" not in s and ">" not in s
def w(s, p, o_tok):
    if not valid_iri(s) or not valid_iri(p):
        return
    out.write(f"<{s}> <{p}> {o_tok} .\n".encode("utf-8"))
def wi(s, p, o):                                   # object is an IRI
    if valid_iri(o): w(s, p, f"<{o}>")
def wl(s, p, o):                                   # object is a plain literal
    o = str(o).strip()
    if o: w(s, p, f'"{esc(o)}"')
def wt(s, p, o, dt):                               # typed literal
    w(s, p, f'"{esc(o)}"^^<{dt}>')

def slug(s):
    s = unicodedata.normalize("NFKD", str(s)).encode("ascii", "ignore").decode("ascii")
    s = re.sub(r"[^a-zA-Z0-9]+", "-", s).strip("-").lower()
    return s[:96]

def once(iri):
    if iri in seen:
        return False
    seen.add(iri)
    return True

def local(el):
    t = el.tag
    return t.rsplit("}", 1)[-1] if "}" in t else t

def pages(sub):
    ps = sorted((RAW / sub).glob("page_*.xml.gz"))
    return ps[:MAXPAGES] if MAXPAGES else ps

def records(sub):
    for p in pages(sub):
        with gzip.open(p, "rb") as f:
            root = ET.parse(f).getroot()
        for rec in root.iter(f"{OAI}record"):
            yield rec

def handle_of(rec):
    h = rec.find(f"{OAI}header/{OAI}identifier")
    if h is None or not h.text:
        return None
    return h.text.rsplit(":", 1)[-1]          # 20.500.11850/715512

# ============================================================================
# PASS 1 — xoai enrichment index
# ============================================================================
# relationship type (DSpace) -> ethz property localname; None = drop (covered inline)
REL_MAP = {
    "isPubNewVersionOfPub": "isNewVersionOf", "isPubPreviousVersionOfPub": "isPreviousVersionOf",
    "isPubCitesPub": "cites", "isPubCitedByPub": "isCitedBy",
    "isPubReferencesPub": "references", "isPubReferencedByPub": "isReferencedBy",
    "isPubHasPartPub": "hasPart", "isPubPartOfPub": "isPartOf",
    "isPubSupplementToPub": "isSupplementTo", "isPubSupplementedByPub": "isSupplementedBy",
    "isPubSupplementedByRD": "isSupplementedBy", "isRDSupplementToPub": "isSupplementTo",
    "isPubHasPartRD": "hasPart", "isRDPartOfPub": "isPartOf",
    "isPubVariantFormOfPub": "relatedEntity", "isPubOriginalFormOfPub": "relatedEntity",
    "isPubDerivedFromPub": "relatedEntity", "isPubSourceOfPub": "relatedEntity",
    "isPubContinuedByPub": "relatedEntity", "isPubContinuesPub": "relatedEntity",
    "isPubIdenticalToPub": "relatedEntity",
}
SKIP_REL = {"isAuthorOfPublication", "isJournalOfPublication", "isEditorOfPublication",
            "isSupervisorOfPublication", "isAuthorOfResearchData"}
UUID_RE = re.compile(r"/bitstreams/([0-9a-f-]{36})/")

def build_enrichment():
    idx = {}
    for rec in records("xoai"):
        h = handle_of(rec)
        if h is None:
            continue
        md = rec.find(f"{OAI}metadata")
        if md is None:
            continue
        inner = next((e for e in md if local(e) == "metadata"), None)
        if inner is None:
            continue
        etype = None
        files = []
        rels = []
        for top in inner:
            if local(top) != "element":
                continue
            name = top.get("name")
            if name == "dspace":
                for f in top.iter():
                    if local(f) == "field" and f.get("name") == "value" and (f.text or "").strip():
                        etype = f.text.strip()
            elif name == "bundles":
                for bundle in top:
                    if local(bundle) != "element":
                        continue
                    bname = None
                    for fld in bundle:
                        if local(fld) == "field" and fld.get("name") == "name":
                            bname = (fld.text or "").strip()
                    if bname != "ORIGINAL":
                        continue
                    bs_wrap = next((e for e in bundle if local(e) == "element" and e.get("name") == "bitstreams"), None)
                    if bs_wrap is None:
                        continue
                    for bit in bs_wrap:
                        if local(bit) != "element":
                            continue
                        d = {}
                        for fld in bit:
                            if local(fld) == "field":
                                d[fld.get("name")] = (fld.text or "").strip()
                        if d.get("url"):
                            files.append(d)
            elif name == "relation":
                for rt in top:
                    if local(rt) != "element":
                        continue
                    rtype = rt.get("name")
                    if rtype in SKIP_REL:
                        continue
                    prop = REL_MAP.get(rtype, "relatedEntity")
                    for none in rt:
                        if local(none) == "element" and none.get("name") == "none":
                            for fld in none:
                                if local(fld) == "field" and fld.get("name") == "value":
                                    v = (fld.text or "").strip()
                                    if v:
                                        rels.append((prop, v))
        idx[h] = (etype, files, rels)
    return idx

# ============================================================================
# PASS 2 — emit graph from oai_ethz
# ============================================================================
TYPE_CLASS = {
    "Journal Article": "JournalArticle", "Conference Paper": "ConferencePaper",
    "Doctoral Thesis": "DoctoralThesis", "Master Thesis": "MasterThesis",
    "Book Chapter": "BookChapter", "Report": "Report", "Working Paper": "WorkingPaper",
    "Review Article": "ReviewArticle", "Presentation": "Presentation",
    "Monograph": "Monograph", "Dataset": "Dataset", "Data Collection": "Dataset",
}

def dc_root(rec):
    md = rec.find(f"{OAI}metadata")
    if md is None:
        return None
    return next((e for e in md), None)      # the single oai_dc:dc child

def texts(root, name):
    return [(e.text or "").strip() for e in root if local(e) == name and (e.text or "").strip()]

def first(root, name):
    for e in root:
        if local(e) == name and (e.text or "").strip():
            return e.text.strip()
    return None

def person(work, prop, name, orcid):
    if not name:
        return
    orcid = (orcid or "").strip()
    m = re.search(r"(\d{4}-\d{4}-\d{4}-\d{3}[\dxX])", orcid)
    if m:
        pid = f"https://orcid.org/{m.group(1)}"
    else:
        pid = f"{LOCAL}person/{slug(name)}"
        if not slug(name):
            return
    wi(work, ETHZ + prop, pid)
    if once(pid):
        wi(pid, RDF, ETHZ + "Person")
        wl(pid, RDFS_LABEL, name)
        wl(pid, FOAF_NAME, name)
        if m:
            wl(pid, ETHZ + "orcid", m.group(1))

def person_list(root, work, listname, itemname, prop):
    for lst in root:
        if local(lst) != listname:
            continue
        for it in lst:
            if local(it) != itemname:
                continue
            nm = ov = None
            for c in it:
                lc = local(c)
                if lc.endswith("-name"):
                    nm = (c.text or "").strip()
                elif lc.endswith("-orcid"):
                    ov = (c.text or "").strip()
            person(work, prop, nm, ov)

def affiliation(root, work):
    codes_seen = set()
    for lst in root:
        if local(lst) != "leitzahllist":
            continue
        for lz in lst:
            if local(lz) != "leitzahl" or not (lz.text or "").strip():
                continue
            segs = [s.strip() for s in lz.text.split("::") if s.strip()]
            prev = None
            leaf = None
            for seg in segs:
                m = re.match(r"^(\d{3,})\s*-\s*(.+)$", seg)
                if m:
                    code, uname = m.group(1), m.group(2).strip()
                else:
                    code, uname = slug(seg)[:24] or "x", seg
                unit = f"{LOCAL}unit/{code}"
                if once(unit):
                    wi(unit, RDF, ETHZ + "OrgUnit")
                    wl(unit, RDFS_LABEL, uname)
                    wl(unit, ETHZ + "leitzahlCode", code)
                if prev and prev != unit:
                    edge = (unit, prev)
                    if edge not in seen:
                        seen.add(edge)
                        wi(unit, ORG_SUBORG, prev)
                prev = unit
                leaf = unit
            if leaf and leaf not in codes_seen:
                codes_seen.add(leaf)
                wi(work, ETHZ + "affiliation", leaf)

def grants(root, work):
    for lst in root:
        if local(lst) != "grantlist":
            continue
        for g in lst:
            if local(g) != "grant":
                continue
            d = {local(c).replace("grant-", ""): (c.text or "").strip() for c in g}
            key = d.get("agreementno") or d.get("name") or ""
            if not key:
                continue
            gid = f"{LOCAL}grant/{slug(key)}"
            wi(work, ETHZ + "funding", gid)
            if once(gid):
                wi(gid, RDF, ETHZ + "Grant")
                if d.get("name"):
                    wl(gid, RDFS_LABEL, d["name"])
                if d.get("agreementno"):
                    wl(gid, DCT + "identifier", d["agreementno"])
                if d.get("program"):
                    wl(gid, ETHZ + "program", d["program"])
                fd = d.get("funderdoi", "")
                fn = d.get("fundername", "")
                fm = re.search(r"10\.13039/(\S+)", fd)
                if fm:
                    fid = f"https://doi.org/10.13039/{fm.group(1)}"
                elif fn:
                    fid = f"{LOCAL}funder/{slug(fn)}"
                else:
                    fid = None
                if fid:
                    wi(gid, ETHZ + "funder", fid)
                    if once(fid) and fn:
                        wl(fid, RDFS_LABEL, fn)

def emit(rec, idx):
    root = dc_root(rec)
    if root is None:
        return
    h = handle_of(rec)
    if not h:
        return
    hid = h.rsplit("/", 1)[-1]
    doi = first(root, "identifier-doi")
    bare_doi = None
    if doi:
        bare_doi = re.sub(r"^https?://(dx\.)?doi\.org/", "", doi.strip(), flags=re.I).lower()
    work = f"https://doi.org/{bare_doi}" if bare_doi else f"https://hdl.handle.net/{h}"
    etype, files, rels = idx.get(h, (None, [], []))

    # type + class
    dctype = first(root, "type")
    if etype == "ResearchData":
        cls = ETHZ + TYPE_CLASS.get(dctype, "ResearchData")
    else:
        cls = ETHZ + TYPE_CLASS.get(dctype, "Publication")
    wi(work, RDF, cls)
    if dctype:
        wl(work, ETHZ + "publicationType", dctype)

    # identity + labels
    wl(work, ETHZ + "handle", h)
    wi(work, DCT + "identifier", f"https://hdl.handle.net/{h}")
    if bare_doi:
        wl(work, ETHZ + "doi", bare_doi)
    title = first(root, "title")
    sub = first(root, "title-subtitle")
    if title:
        full = f"{title}: {sub}" if sub else title
        wl(work, RDFS_LABEL, full)
        wl(work, DCT + "title", full)

    # simple datatype fields  (localname -> predicate IRI)
    for name, pred in (
        ("date-issued", DCT + "issued"), ("date-published", DCT + "date"),
        ("publisher", DCT + "publisher"), ("publication-place", ETHZ + "place"),
        ("publication-status", ETHZ + "publicationStatus"),
        ("language-iso", DCT + "language"), ("availability", ETHZ + "availability"),
        ("size", ETHZ + "extent"), ("pages-start", BIBO + "pageStart"),
        ("pages-end", BIBO + "pageEnd"), ("journal-volume", BIBO + "volume"),
        ("journal-issue", BIBO + "issue"), ("book-title", DCT + "isPartOf"),
        ("event", ETHZ + "event"), ("event-location", ETHZ + "eventLocation"),
        ("event-date", ETHZ + "eventDate"),
    ):
        for v in texts(root, name):
            wl(work, pred, v)

    # multivalued identifiers
    for name, pred in (
        ("identifier-isbn", ETHZ + "isbn"), ("identifier-issn", ETHZ + "issn"),
        ("identifier-arxiv", ETHZ + "arxiv"), ("identifier-wos", ETHZ + "wos"),
        ("identifier-scopus", ETHZ + "scopus"), ("identifier-nebis", ETHZ + "nebis"),
        ("code-ddc", ETHZ + "ddc"), ("code-jel", ETHZ + "jel"),
        ("subject", DCT + "subject"), ("description-abstract", DCT + "abstract"),
        ("notes", DCT + "description"), ("identifier-other", ETHZ + "otherIdentifier"),
    ):
        for lst in root:
            if local(lst) == name + "list":
                for it in lst:
                    if (it.text or "").strip():
                        wl(work, pred, it.text.strip())
        for v in texts(root, name):
            wl(work, pred, v)

    # licence
    ru = first(root, "rights-uri")
    if ru:
        wi(work, DCT + "license", ru)
    rl = first(root, "rights-license")
    if rl:
        wl(work, DCT + "rights", rl)

    # people
    person_list(root, work, "contributor-authorlist", "contributor-author", "creator")
    person_list(root, work, "contributor-editorlist", "contributor-editor", "editor")
    person_list(root, work, "contributor-supervisorlist", "contributor-supervisor", "supervisor")

    # journal
    jtitle = first(root, "journal-title")
    if jtitle:
        issns = [v for v in texts(root, "identifier-issn")] + \
                [it.text.strip() for lst in root if local(lst) == "identifier-issnlist"
                 for it in lst if (it.text or "").strip()]
        issn = next((i for i in issns if re.match(r"^\d{4}-\d{3}[\dxX]$", i)), None)
        jid = f"https://portal.issn.org/resource/ISSN/{issn}" if issn else f"{LOCAL}serial/{slug(jtitle)}"
        wi(work, ETHZ + "journal", jid)
        if once(jid):
            wi(jid, RDF, ETHZ + "Journal")
            wl(jid, RDFS_LABEL, jtitle)
            if issn:
                wl(jid, ETHZ + "issn", issn)

    affiliation(root, work)
    grants(root, work)

    # files (ORIGINAL bitstreams, from xoai)
    for d in files:
        m = UUID_RE.search(d["url"])
        fid = f"{LOCAL}bitstream/{m.group(1)}" if m else None
        if not fid:
            continue
        wi(work, ETHZ + "hasFile", fid)
        wi(fid, RDF, ETHZ + "File")
        if d.get("name"):
            wl(fid, RDFS_LABEL, d["name"])
        if d.get("format"):
            wl(fid, DCT + "format", d["format"])
        if d.get("size", "").isdigit():
            wt(fid, ETHZ + "sizeBytes", d["size"], XSD_INT)
        if d.get("checksum"):
            wl(fid, ETHZ + "checksum", d["checksum"])
        if d.get("checksumAlgorithm"):
            wl(fid, ETHZ + "checksumAlgorithm", d["checksumAlgorithm"])
        wi(fid, ETHZ + "downloadURL", d["url"])
        wl(fid, ETHZ + "bundle", "ORIGINAL")

    # DSpace relationship graph (targets are opaque entity UUIDs)
    for prop, uuid in rels:
        ent = f"{LOCAL}entity/{uuid}"
        wi(work, ETHZ + prop, ent)
        if once(ent):
            wi(ent, RDF, ETHZ + "Entity")

def main():
    sys.stderr.write("pass 1: indexing xoai enrichment ...\n")
    idx = build_enrichment()
    sys.stderr.write(f"  indexed {len(idx):,} handles\n")
    sys.stderr.write("pass 2: emitting graph from oai_ethz ...\n")
    n = 0
    for rec in records("oai_ethz"):
        emit(rec, idx)
        n += 1
        if n % 25000 == 0:
            sys.stderr.write(f"  {n:,} records\n")
    sys.stderr.write(f"done: {n:,} records\n")

if __name__ == "__main__":
    main()
