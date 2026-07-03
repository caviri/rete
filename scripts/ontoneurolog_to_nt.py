#!/usr/bin/env python3
"""OntoNeuroLOG v2.2 (OWL-Lite, RDF/XML) -> merged N-Triples for `rete build`.

OntoNeuroLOG is the multi-layer application ontology of the NeuroLOG project
(neuroimaging data sharing / provenance), grounded on DOLCE. The public v2.2
package ships as 35 RDF/XML `.owl` modules wired by `owl:imports`. `rete build`
reads RDF/XML directly, but the ontology is *split* across modules, so we merge
the local module files into one graph (imports are NOT dereferenced over HTTP --
the irisa.fr import IRIs are dead; every module we need is in the zip).

Source:  https://neurolog.i3s.unice.fr/public_namespace/ontology
Package: https://neurolog.i3s.unice.fr/_media/public_namespace/ontoneurologv2.2.zip
Ref:     Temal, Dojat, Kassel, Gibaud. "Towards an ontology for sharing medical
         images and regions of interest in neuroimaging." J. Biomed. Inform. 2008.
License: v2.2 publicly downloadable; explicit license not stated on the page.
         Attribute B. Gibaud / IRISA-VISAGES and the NeuroLOG project.

Usage:
  python scripts/ontoneurolog_to_nt.py             # download the zip, then merge
  python scripts/ontoneurolog_to_nt.py path/to.zip # use a local copy of the zip
Output: data/ontoneurolog/ontoneurolog.nt
"""
import io
import os
import sys
import zipfile
import urllib.request

from rdflib import Graph, RDF, OWL

ZIP_URL = "https://neurolog.i3s.unice.fr/_media/public_namespace/ontoneurologv2.2.zip"
OUT_DIR = os.path.join("data", "ontoneurolog")
OUT_NT = os.path.join(OUT_DIR, "ontoneurolog.nt")


def load_zip_bytes(argv):
    if len(argv) > 1 and os.path.exists(argv[1]):
        print(f"reading local zip: {argv[1]}")
        with open(argv[1], "rb") as fh:
            return fh.read()
    print(f"downloading {ZIP_URL}")
    req = urllib.request.Request(ZIP_URL, headers={"User-Agent": "rete-build/1.0"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        return resp.read()


def main(argv):
    raw = load_zip_bytes(argv)
    zf = zipfile.ZipFile(io.BytesIO(raw))
    modules = [
        n for n in zf.namelist()
        if n.endswith(".owl") and "__MACOSX" not in n
    ]
    modules.sort()
    print(f"found {len(modules)} OWL modules")

    g = Graph()
    ok, bad = 0, []
    for name in modules:
        data = zf.read(name)
        try:
            g.parse(data=data, format="xml")
            ok += 1
        except Exception as exc:  # noqa: BLE001 - report and continue
            bad.append((name, str(exc)[:120]))
    print(f"parsed OK: {ok}/{len(modules)} modules; total triples (deduped): {len(g)}")
    for name, err in bad:
        print(f"  FAIL {name}: {err}", file=sys.stderr)
    if bad:
        sys.exit(1)

    n_cls = sum(1 for _ in g.subjects(RDF.type, OWL.Class))
    n_op = sum(1 for _ in g.subjects(RDF.type, OWL.ObjectProperty))
    n_dp = sum(1 for _ in g.subjects(RDF.type, OWL.DatatypeProperty))
    print(f"owl:Class={n_cls}  owl:ObjectProperty={n_op}  owl:DatatypeProperty={n_dp}")

    os.makedirs(OUT_DIR, exist_ok=True)
    # nt serializer emits UTF-8 with '\n' endings; write binary so Windows does
    # not translate to CRLF (a stray '\r' in an IRI breaks the NT parser).
    nt = g.serialize(format="nt")
    if isinstance(nt, str):
        nt = nt.encode("utf-8")
    with open(OUT_NT, "wb") as fh:
        fh.write(nt)
    print(f"wrote {OUT_NT} ({os.path.getsize(OUT_NT):,} bytes)")


if __name__ == "__main__":
    main(sys.argv)
