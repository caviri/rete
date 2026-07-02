#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
Build an N-Triples graph for the BVPB Ramón Llull collection from the harvested
Dublin Core RDF (rdf/*.rdf.xml) + the manifest (meta/records.jsonl).

Each record becomes an edm:ProvidedCHO whose subject IRI is its stable BVPB
landing-page URL.  DC fields are carried through; the digitised object (PDF) is
linked with edm:isShownBy, the landing page with edm:isShownAt, the image viewer
with foaf:page.  Output: data/bvpb/ramon_llull/ramon_llull.nt
"""
import os, re, json, glob
import xml.etree.ElementTree as ET

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..",
                                    "data", "bvpb", "ramon_llull"))
RECORDS = os.path.join(ROOT, "meta", "records.jsonl")
OUT = os.path.join(ROOT, "ramon_llull.nt")
IMGBASE = "https://bvpb.mcu.es/ramon_llull/es/catalogo_imagenes/grupo.do?path="

DC = "http://purl.org/dc/elements/1.1/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
EDM = "http://www.europeana.eu/schemas/edm/"
FOAF = "http://xmlns.com/foaf/0.1/"


def esc(s):
    return (s.replace("\\", "\\\\").replace('"', '\\"')
             .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t"))


def lit(s, lang=None):
    s = esc(s.strip())
    return f'"{s}"@{lang}' if lang else f'"{s}"'


def iri(u):
    return f"<{u}>"


def load_records():
    out = {}
    with open(RECORDS, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                r = json.loads(line); out[r["control"]] = r
    return out


def parse_dc(path):
    """Return list of (localname, text, lang) from the DC rdf:Description."""
    tree = ET.parse(path)
    desc = tree.getroot().find(f"{{{RDF}}}Description")
    fields = []
    for el in list(desc):
        tag = el.tag.split("}")[-1]
        txt = (el.text or "").strip()
        if not txt:
            continue
        lang = el.get(f"{{http://www.w3.org/XML/1998/namespace}}lang")
        fields.append((tag, txt, lang))
    return fields


def main():
    recs = load_records()
    n_rec = n_trip = 0
    with open(OUT, "w", encoding="utf-8") as w:
        def emit(s, p, o):
            nonlocal n_trip
            w.write(f"{s} {p} {o} .\n"); n_trip += 1

        for ctrl, rec in sorted(recs.items()):
            rdfp = os.path.join(ROOT, rec.get("rdf") or "")
            if not rec.get("rdf") or not os.path.exists(rdfp):
                continue
            fields = parse_dc(rdfp)
            ident = next((t for tag, t, _ in fields if tag == "identifier"
                          and t.startswith("http")), None)
            subj_url = ident or f"https://bvpb.mcu.es/es/consulta/registro.do?id={rec.get('id')}"
            s = iri(subj_url)
            n_rec += 1

            emit(s, iri(RDF + "type"), iri(EDM + "ProvidedCHO"))
            emit(s, iri(DC + "identifier"), lit(ctrl))          # BVPB control number
            emit(s, iri("http://purl.org/dc/terms/isPartOf"),
                 lit("BVPB — Ramón Llull", "es"))

            for tag, txt, lang in fields:
                if tag == "identifier" and txt.startswith("http"):
                    continue  # that's the subject itself
                emit(s, iri(DC + tag), lit(txt, lang))

            # digital-object links
            emit(s, iri(EDM + "isShownAt"), iri(subj_url))
            for p in (rec.get("pdf_paths") or []):
                emit(s, iri(EDM + "isShownBy"), iri(IMGBASE + p))
            # if pdf phase hasn't run yet, link every grupo as a candidate
            if not rec.get("pdf_paths"):
                for p in (rec.get("grupos") or []):
                    emit(s, iri(FOAF + "page"), iri(IMGBASE + p))
            for p in (rec.get("viewer_paths") or []):
                emit(s, iri(FOAF + "page"), iri(IMGBASE + p))

    print(f"wrote {n_trip} triples for {n_rec} records -> {OUT}")


if __name__ == "__main__":
    main()
