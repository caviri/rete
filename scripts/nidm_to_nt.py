#!/usr/bin/env python3
"""NIDM (Neuroimaging Data Model) → merged N-Triples for `rete build`.

Builds a compact, explorable NIDM dataset that pairs with the ontoneurolog
dataset ([[ontoneurolog-dataset]]). NIDM is the RDF/PROV model for describing
the neuroimaging data lifecycle (subjects, acquisitions, provenance). We merge:

  - ds000030.ttl        real cohort instances: the UCLA Consortium for
                        Neuropsychiatric Phenomics (OpenNeuro ds000030) — subjects
                        as nidm:AcquisitionObject / prov:Entity with ncit:age,
                        ncit:gender, ncit:diagnosis (CONTROL / SCHZ / BIPOLAR /
                        ADHD) + PROV provenance (wasGeneratedBy / wasAttributedTo).
  - CENTRAL_E03925.ttl  a second small NIDM-Experiment example document.
  - nidm-experiment.owl the NIDM-Experiment terms/schema (Turtle despite the .owl
                        extension — content-sniffed).
  - ontoneurolog_instruments_import.ttl
                        THE FEDERATION BRIDGE: the OntoNeuroLOG v3.0 assessment-
                        instrument taxonomy that NIDM-Experiment reuses. Its class
                        LOCAL-NAMES (assessment-instrument, questionnaire,
                        behavioural-instrument, scale-item, numerical-score, …)
                        coincide with the ontoneurolog v2.2 dataset's instrument
                        classes — so a cross-source join on STRAFTER(STR(?c),"#")
                        links the two datasets on the shared OntoNeuroLOG concept
                        (v2.2 irisa.fr namespace ↔ v3.0 neurolog.unice.fr namespace).

Source repo: https://github.com/incf-nidash/nidm-specs (INCF NIDASH / BIRN DDWG).
License: NIDM specs are an open community standard (no explicit LICENSE file);
         ds000030 phenotype data derives from the OpenNeuro CC0 dataset (UCLA CNP).
         Attribute the NIDM working group + OpenNeuro ds000030.

Usage:  python scripts/nidm_to_nt.py
Output: data/nidm/nidm.nt
"""
import io
import os
import re
import sys
import urllib.request
import zipfile

from rdflib import Graph, OWL, RDF, URIRef

RAW = "https://raw.githubusercontent.com/incf-nidash/nidm-specs/master/nidm/nidm-experiment"
FILES = [
    ("scripts/class/ds000030.ttl", "turtle"),
    ("scripts/CENTRAL_E03925.ttl", "turtle"),
    ("imports/ontoneurolog_instruments_import.ttl", "turtle"),
]
# NOTE: the generic NIDM-Experiment terms file (terms/nidm-experiment.owl) is
# DELIBERATELY excluded. It contributes rdfs:comment/rdfs:label on nidm: terms,
# which would make rdfs:comment a shared predicate across this dataset and the
# ontoneurolog dataset — and the playground's cross-source splitter routes a
# pattern to the source that *owns* its subject variable if that source *can*
# answer the predicate. Since the bridge (?v2 via owl:sameAs) is owned by NIDM,
# a shared rdfs:comment would keep the "?v2 rdfs:comment" pattern on NIDM (which
# has no comment for the v2.2 IRIs) and the federation join would return 0 rows.
# Dropping it makes rdfs:comment NIDM-absent, so that pattern correctly routes to
# the ontoneurolog source. The cohort/PROV/instrument examples don't need it.
# The OntoNeuroLOG v2.2 package (same one the ontoneurolog dataset is built from) —
# used to mint the owl:sameAs federation bridge between NIDM's v3.0 instrument IRIs
# and the ontoneurolog dataset's v2.2 instrument IRIs (matched by concept local-name).
ONL_ZIP = "https://neurolog.i3s.unice.fr/_media/public_namespace/ontoneurologv2.2.zip"
V30_NS = "http://neurolog.unice.fr/ontoneurolog/v3.0/instrument.owl#"
OUT_DIR = os.path.join("data", "nidm")
OUT_NT = os.path.join(OUT_DIR, "nidm.nt")


def v22_instrument_iris():
    """{local-name: v2.2 IRI} for OntoNeuroLOG v2.2 instrument-module classes."""
    req = urllib.request.Request(ONL_ZIP, headers={"User-Agent": "rete-build/1.0"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        zf = zipfile.ZipFile(io.BytesIO(resp.read()))
    out = {}
    for name in zf.namelist():
        if "__MACOSX" in name or not name.endswith("instrument-owl-lite.owl"):
            continue
        txt = zf.read(name).decode("utf-8", "replace")
        base = re.search(r'xml:base="([^"]+)"', txt).group(1)
        for m in re.finditer(r'owl:Class rdf:about="#([^"]+)"', txt):
            out[m.group(1)] = base + "#" + m.group(1)
    return out


def add_bridge(g):
    """Emit <v3.0 instrument class> owl:sameAs <v2.2 instrument class> for every
    concept whose local-name is shared — the honest federation link to the
    ontoneurolog dataset (same OntoNeuroLOG taxonomy, two IRI versions)."""
    v22 = v22_instrument_iris()
    n = 0
    for c in set(g.subjects(RDF.type, OWL.Class)):
        s = str(c)
        if not s.startswith(V30_NS):
            continue
        ln = s[len(V30_NS):]
        if ln in v22:
            g.add((c, OWL.sameAs, URIRef(v22[ln])))
            n += 1
    return n


def fetch(path):
    url = f"{RAW}/{path}"
    req = urllib.request.Request(url, headers={"User-Agent": "rete-build/1.0"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        return resp.read()


def parse_into(g, data, hint):
    # content-sniff: some .owl files here are actually Turtle. Try the hint,
    # then fall back to the other RDF syntaxes.
    for fmt in [hint, "turtle", "xml"]:
        try:
            before = len(g)
            g.parse(data=data, format=fmt)
            return len(g) - before, fmt
        except Exception:
            continue
    raise RuntimeError("could not parse in any known RDF syntax")


def main():
    g = Graph()
    for path, hint in FILES:
        data = fetch(path)
        added, fmt = parse_into(g, data, hint)
        print(f"  {path}: +{added} triples (fmt={fmt})")
    bridged = add_bridge(g)
    print(f"  federation bridge: +{bridged} owl:sameAs (v3.0 -> ontoneurolog v2.2)")
    print(f"total merged triples (deduped): {len(g)}")

    os.makedirs(OUT_DIR, exist_ok=True)
    nt = g.serialize(format="nt")
    if isinstance(nt, str):
        nt = nt.encode("utf-8")
    with open(OUT_NT, "wb") as fh:
        fh.write(nt)
    print(f"wrote {OUT_NT} ({os.path.getsize(OUT_NT):,} bytes)")


if __name__ == "__main__":
    main()
