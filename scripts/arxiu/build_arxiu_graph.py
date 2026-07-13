#!/usr/bin/env python3
"""records.jsonl (Arxius en Línia harvest) -> a schema.org archival knowledge graph.

Each record is a schema:ArchiveComponent held by a schema:ArchiveOrganization (the
archive) and part of a schema:Collection (the fonds). Digital images: schema:image →
the downsized WebP on R2 (renders in the playground), arx:originalImage → the original
full-res JPEG on Gencat's server (the source link, always kept)."""
import json, re
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SRC  = REPO / "data" / "arxiu" / "records.jsonl"
OUT  = REPO / "data" / "arxiu" / "arxiu.nt"

ARX    = "https://w3id.org/rete/arxiu#"
B      = "https://w3id.org/rete/arxiu/"
SCHEMA = "http://schema.org/"
DCT    = "http://purl.org/dc/terms/"
RDF    = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS   = "http://www.w3.org/2000/01/rdf-schema#"
XSD    = "http://www.w3.org/2001/XMLSchema#"

MARK = re.compile(r"</?mark>")
def clean(s):
    if not s: return ""
    return re.sub(r"\s+", " ", MARK.sub("", s).replace("\n", " ")).strip()
def safe(s):
    return re.sub(r"[^A-Za-z0-9._-]", "_", s)
def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "").replace("\t", "\\t")
def as_date(dstr):
    m = re.match(r"(\d{2})-(\d{2})-(\d{4})", dstr or "")
    if not m: return None
    d, mo, y = m.groups()
    if y == "0000" or not (1 <= int(mo) <= 12) or not (1 <= int(d) <= 31): return None
    return f"{y}-{mo}-{d}"

def main():
    out = open(OUT, "w", encoding="utf-8")
    def iri(s, p, o): out.write(f"<{s}> <{p}> <{o}> .\n")
    def lit(s, p, v, dt=None): out.write(f'<{s}> <{p}> "{esc(str(v))}"' + (f"^^<{dt}>" if dt else "") + " .\n")

    archives, fondses = {}, {}
    n = nimg = 0
    for line in SRC.open(encoding="utf-8"):
        r = json.loads(line)
        ref = r.get("codiReferencia")
        if not ref: continue
        u = B + "unit/" + safe(ref)
        iri(u, RDF, SCHEMA + "ArchiveComponent"); iri(u, RDF, ARX + "Unit")
        if r.get("titol"): lit(u, SCHEMA + "name", clean(r["titol"]))
        if r.get("descripcio"): lit(u, SCHEMA + "description", clean(r["descripcio"]))
        lit(u, DCT + "identifier", ref)
        ca = r.get("codiArxiu"); nom = r.get("nomArxiu")
        if ca is not None:
            a = B + "archive/" + str(ca); iri(u, SCHEMA + "holdingArchive", a)
            if ca not in archives: archives[ca] = nom
        cf = r.get("codiReferenciaFons")
        if cf:
            fo = B + "fonds/" + safe(cf); iri(u, SCHEMA + "isPartOf", fo)
            if cf not in fondses: fondses[cf] = (r.get("nomFons"), ca)
        if r.get("cronologia"): lit(u, SCHEMA + "temporalCoverage", clean(r["cronologia"]))
        di = as_date(r.get("dataIniciStr")); df = as_date(r.get("dataFiStr"))
        if di: lit(u, SCHEMA + "startDate", di, XSD + "date")
        if df: lit(u, SCHEMA + "endDate", df, XSD + "date")
        if r.get("tipusContingut"): lit(u, ARX + "contentType", r["tipusContingut"])
        lit(u, ARX + "reserved", "true" if r.get("reservat") else "false", XSD + "boolean")
        if r.get("objecteDigitalUrl"): iri(u, ARX + "originalImage", r["objecteDigitalUrl"])
        if r.get("webp"):
            iri(u, SCHEMA + "image", r["webp"]); nimg += 1
        n += 1

    for ca, nom in sorted(archives.items()):
        a = B + "archive/" + str(ca)
        iri(a, RDF, SCHEMA + "ArchiveOrganization")
        if nom: lit(a, SCHEMA + "name", nom)
    for cf, (nom, ca) in sorted(fondses.items()):
        fo = B + "fonds/" + safe(cf)
        iri(fo, RDF, SCHEMA + "Collection"); iri(fo, RDF, ARX + "Fonds")
        if nom: lit(fo, SCHEMA + "name", clean(nom))
        if ca is not None: iri(fo, SCHEMA + "holdingArchive", B + "archive/" + str(ca))

    out.close()
    print(f"units: {n}, images(webp): {nimg}, archives: {len(archives)}, fonds: {len(fondses)} -> {OUT}")

if __name__ == "__main__":
    main()
