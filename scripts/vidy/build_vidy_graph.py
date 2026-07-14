#!/usr/bin/env python3
"""records.jsonl (Archives de la Ville de Lausanne, AtoM EAD harvest) -> a schema.org
archival knowledge graph. Each record is a schema:ArchiveComponent held by the
schema:ArchiveOrganization, part of its parent (the fonds/series hierarchy). Digitised
records link their master PDF via schema:associatedMedia, and the PDF node carries its
byte size (schema:contentSize) + encodingFormat — the link + size + media the user asked
for. Record URLs use AtoM's slug (cote → lowercase, non-alphanumeric → dash)."""
import json, re
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SRC  = REPO / "data" / "vidy" / "records.jsonl"
OUT  = REPO / "data" / "vidy" / "vidy.nt"

VIDY   = "https://w3id.org/rete/vidy#"
B      = "https://w3id.org/rete/vidy/"
SCHEMA = "http://schema.org/"
DCT    = "http://purl.org/dc/terms/"
RDF    = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
XSD    = "http://www.w3.org/2001/XMLSchema#"
SITE   = "https://vidy-archives.lausanne.ch/"
ARCHIVE_IRI = B + "archive/AVL"
ARCHIVE_NAME = "Archives de la Ville de Lausanne"

def slugify(cote): return re.sub(r"[^a-z0-9]+", "-", (cote or "").lower()).strip("-")
def safe(s): return re.sub(r"[^A-Za-z0-9._-]", "_", s)
def esc(s): return str(s).replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "").replace("\t", "\\t")

def main():
    out = OUT.open("w", encoding="utf-8")
    def iri(s, p, o): out.write(f"<{s}> <{p}> <{o}> .\n")
    def lit(s, p, v, dt=None): out.write(f'<{s}> <{p}> "{esc(v)}"' + (f"^^<{dt}>" if dt else "") + " .\n")

    iri(ARCHIVE_IRI, RDF, SCHEMA + "ArchiveOrganization")
    lit(ARCHIVE_IRI, SCHEMA + "name", ARCHIVE_NAME)
    iri(ARCHIVE_IRI, SCHEMA + "url", SITE)

    n = nimg = fonds = 0
    seen_media = set()
    for line in SRC.open(encoding="utf-8"):
        r = json.loads(line)
        cote = r.get("cote")
        if not cote:
            continue
        u = B + "unit/" + safe(cote)
        iri(u, RDF, SCHEMA + "ArchiveComponent"); iri(u, RDF, VIDY + "Unit")
        if r.get("title"): lit(u, SCHEMA + "name", r["title"])
        lit(u, DCT + "identifier", cote)
        iri(u, SCHEMA + "url", SITE + slugify(cote))
        iri(u, SCHEMA + "holdingArchive", ARCHIVE_IRI)
        if r.get("level"): lit(u, VIDY + "level", r["level"])
        if r.get("date"): lit(u, SCHEMA + "temporalCoverage", r["date"])
        if r.get("physdesc"): lit(u, DCT + "extent", r["physdesc"])
        if r.get("container"):
            loc = (r.get("container_type") + ": " if r.get("container_type") else "") + r["container"]
            lit(u, VIDY + "physicalLocation", loc)
        if r.get("producer"): lit(u, DCT + "creator", r["producer"])
        if r.get("publication_status"): lit(u, VIDY + "publicationStatus", r["publication_status"])
        pc = r.get("parent_cote")
        if pc:
            iri(u, SCHEMA + "isPartOf", B + "unit/" + safe(pc))
        else:
            fonds += 1
        pdf = r.get("pdf_url")
        if pdf:
            iri(u, SCHEMA + "associatedMedia", pdf)
            if pdf not in seen_media:
                seen_media.add(pdf)
                iri(pdf, RDF, SCHEMA + "DigitalDocument")
                lit(pdf, SCHEMA + "encodingFormat", "application/pdf")
                if r.get("pdf_size"):
                    lit(pdf, SCHEMA + "contentSize", str(r["pdf_size"]), XSD + "integer")
            nimg += 1
        n += 1
    out.close()
    print(f"units: {n:,}  ·  with PDF: {nimg:,}  ·  top-level (fonds): {fonds:,}  ->  {OUT}")

if __name__ == "__main__":
    main()
