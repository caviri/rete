#!/usr/bin/env python3
"""Convert harvested ARCAS records (records.ndjson.gz) to N-Triples.

Model (base https://albala.icolombina.es/arcas/):
  record/<recordId>  a arcas:ArchivalRecord
      rdfs:label / dcterms:title   <- facet_title
      dcterms:date                 <- fechas_ss (each)
      arcas:signatura              <- signatura_view_s (whitespace-collapsed)
      arcas:referenceCode          <- facet_referenceCode
      arcas:classificationCode     <- TI72_s
      arcas:level                  <- facet_levelText
      dc:type                      <- facet_recordDoctypeDescription (if specified)
      arcas:country                <- TI01_s
      arcas:hasDigitalContent      <- facet_media (xsd:boolean)
      arcas:solrId                 <- id
      arcas:inArchive              -> archive/<code>
      dcterms:isPartOf             -> record/<parentRecordId>
  archive/<code>     a arcas:Archive ; rdfs:label <facet_archiveName> ; arcas:code <coding>
"""
import gzip, json, os, re, sys

OUTDIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                      "data", "albala")
NDJSON = os.path.join(OUTDIR, "records.ndjson.gz")
OUTNT = os.path.join(OUTDIR, "albala.nt")

B = "https://albala.icolombina.es/arcas/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
DCT = "http://purl.org/dc/terms/"
DC = "http://purl.org/dc/elements/1.1/"
XSD = "http://www.w3.org/2001/XMLSchema#"
A = B + "ns#"  # arcas vocab


def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def lit(s, dt=None, lang=None):
    s = esc(s)
    if dt:
        return f'"{s}"^^<{dt}>'
    if lang:
        return f'"{s}"@{lang}'
    return f'"{s}"'


def first(rec, k):
    v = rec.get(k) or []
    return v[0] if v else None


def slug(s):
    return re.sub(r"[^A-Za-z0-9._-]+", "_", s).strip("_")


def main():
    out = open(OUTNT, "w", encoding="utf-8")
    w = lambda s, p, o: out.write(f"<{s}> <{p}> {o} .\n")
    archives = {}
    n = 0
    with gzip.open(NDJSON, "rt", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            rec = json.loads(line)
            rid = first(rec, "facet_recordId") or first(rec, "id")
            if not rid:
                continue
            s = f"{B}record/{slug(str(rid))}"
            w(s, RDF + "type", f"<{A}ArchivalRecord>")

            title = first(rec, "facet_title") or first(rec, "title")
            if title:
                w(s, RDFS + "label", lit(title, lang="es"))
                w(s, DCT + "title", lit(title, lang="es"))
            for d in rec.get("fechas_ss") or []:
                if d and d.strip():
                    w(s, DCT + "date", lit(d.strip()))
            sig = first(rec, "signatura_view_s")
            if sig and sig.strip():
                w(s, A + "signatura", lit(re.sub(r"\s+", " ", sig).strip()))
            ref = first(rec, "facet_referenceCode")
            if ref and ref.strip():
                w(s, A + "referenceCode", lit(ref.strip()))
            cls = first(rec, "TI72_s")
            if cls and cls.strip():
                w(s, A + "classificationCode", lit(cls.strip()))
            lvl = first(rec, "facet_levelText")
            if lvl and lvl.strip():
                w(s, A + "level", lit(lvl.strip(), lang="es"))
            dty = first(rec, "facet_recordDoctypeDescription")
            if dty and dty.strip() and dty != "Sin especificar":
                w(s, DC + "type", lit(dty.strip(), lang="es"))
            country = first(rec, "TI01_s")
            if country and country.strip():
                w(s, A + "country", lit(country.strip(), lang="es"))
            media = first(rec, "facet_media")
            if media is not None:
                b = "true" if str(media).lower() == "true" else "false"
                w(s, A + "hasDigitalContent", lit(b, dt=XSD + "boolean"))
            sid = first(rec, "id")
            if sid:
                w(s, A + "solrId", lit(str(sid)))

            coding = first(rec, "facet_archiveCoding")
            aname = first(rec, "facet_archiveName")
            if coding:
                acode = slug(coding)
                airi = f"{B}archive/{acode}"
                w(s, A + "inArchive", f"<{airi}>")
                if acode not in archives:
                    archives[acode] = (airi, aname, coding)

            parent = first(rec, "facet_parentRecordId")
            if parent and str(parent) not in ("", "0", "-1", str(rid)):
                w(s, DCT + "isPartOf", f"<{B}record/{slug(str(parent))}>")
            n += 1
            if n % 10000 == 0:
                print(f"  {n} records", flush=True)

    for acode, (airi, aname, coding) in archives.items():
        out.write(f"<{airi}> <{RDF}type> <{A}Archive> .\n")
        if aname:
            out.write(f'<{airi}> <{RDFS}label> {lit(aname.strip(), lang="es")} .\n')
        out.write(f'<{airi}> <{A}code> {lit(coding)} .\n')

    out.close()
    print(f"DONE: {n} records, {len(archives)} archives -> {OUTNT}")
    for acode, (airi, aname, coding) in archives.items():
        print(f"  archive: {coding}  {aname}")


if __name__ == "__main__":
    main()
