#!/usr/bin/env python3
"""Re-express MIrA's entity↔Wikidata links as a shareable SSSOM mapping set.

A mapping is a CLAIM, not a fact — so the community shares mapping sets with
provenance via SSSOM (Simple Standard for Sharing Ontological Mappings): a TSV +
a YAML metadata header, with a defined RDF serialisation. This takes MIrA's
`owl:sameAs` links (baked into mira.rete) and emits them as a STANDALONE mapping
set, two ways:

  mira-wikidata.sssom.tsv          the canonical sharing artifact (TSV + YAML header)
  mira-wikidata-mappings.nt        RDF to build into a linkset .rete: the direct
                                   skos:exactMatch triples (so the federation join can
                                   route through this file) + per-mapping provenance
                                   (owl:Axiom reification with justification/confidence)
                                   + mapping-set metadata (a void:Linkset / SSSOM header).

Input: the rete CLI dump of MIrA's sameAs links (scripts produce it), one solution
per line: `?s=<iri> ?label="…" ?wd=<iri>`.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DUMP = os.path.join(HERE, "data", "mira", "sameas.txt")
TSV = os.path.join(HERE, "data", "mira", "mira-wikidata.sssom.tsv")
NT = os.path.join(HERE, "data", "mira", "mira-wikidata-mappings.nt")

SKOS = "http://www.w3.org/2004/02/skos/core#"
SSSOM = "https://w3id.org/sssom/"
SEMAPV = "https://w3id.org/semapv/vocab/"
OWL = "http://www.w3.org/2002/07/owl#"
DCT = "http://purl.org/dc/terms/"
VOID = "http://rdfs.org/ns/void#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
XSD = "http://www.w3.org/2001/XMLSchema#"
SET = "https://mira.ie/mappings/mira-wikidata"
DATE = "2026-06-29"

LINE = re.compile(r'\?s=<([^>]+)>(?:\s+\?label=("(?:[^"\\]|\\.)*"(?:@[\w-]+)?))?\s+\?wd=<([^>]+)>')


def lit(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main():
    rows = []
    for ln in open(DUMP, encoding="utf-8"):
        m = LINE.search(ln)
        if not m:
            continue
        s, label, wd = m.group(1), m.group(2), m.group(3)
        lbl = ""
        if label:
            lbl = re.sub(r'^"|"(?:@[\w-]+)?$', "", label)
        rows.append((s, lbl, wd))
    rows.sort()

    # ---- SSSOM TSV (YAML metadata header in #-comments, then the table) ----------
    hdr = [
        "# curie_map:",
        "#   skos: http://www.w3.org/2004/02/skos/core#",
        "#   semapv: https://w3id.org/semapv/vocab/",
        "#   mira: https://mira.ie/entity/",
        "#   wikidata: http://www.wikidata.org/entity/",
        f"# mapping_set_id: {SET}",
        '# mapping_set_version: "1.0"',
        "# license: https://creativecommons.org/licenses/by-nc-sa/4.0/",
        "# mapping_provider: https://www.mira.ie",
        "# creator_id: https://www.mira.ie",
        f"# mapping_date: {DATE}",
        "# mapping_tool: scripts/mira_sssom.py",
        "# comment: MIrA (Manuscripts with Irish Associations) entities reconciled to Wikidata; re-expressed from owl:sameAs as skos:exactMatch.",
    ]
    cols = ["subject_id", "subject_label", "predicate_id", "object_id",
            "mapping_justification", "confidence"]
    with open(TSV, "w", encoding="utf-8") as f:
        f.write("\n".join(hdr) + "\n" + "\t".join(cols) + "\n")
        for s, lbl, wd in rows:
            sid = s.replace("https://mira.ie/entity/", "mira:")
            oid = wd.replace("http://www.wikidata.org/entity/", "wikidata:")
            f.write("\t".join([sid, lbl, "skos:exactMatch", oid,
                               "semapv:ManualMappingCuration", "1.0"]) + "\n")
    print(f"SSSOM: {len(rows)} mappings -> {TSV}")

    # ---- RDF for the linkset .rete ----------------------------------------------
    out = []

    def t(s, p, o):
        out.append(f"<{s}> <{p}> {o} .")

    # mapping-set metadata (a VoID linkset described with SSSOM terms)
    t(SET, RDF + "type", f"<{VOID}Linkset>")
    t(SET, RDF + "type", f"<{SSSOM}MappingSet>")
    # NB: use dcterms:title (not rdfs:label) for the set's own name, so this linkset's
    # predicate set doesn't claim rdfs:label — that routes cleanly to MIrA for the
    # mapped entities. (A linkset describes LINKS; it doesn't relabel the entities.)
    t(SET, DCT + "title", lit("MIrA ↔ Wikidata mappings"))
    t(SET, DCT + "license", "<https://creativecommons.org/licenses/by-nc-sa/4.0/>")
    t(SET, DCT + "creator", lit("MIrA (Pádraic Moran, University of Galway)"))
    t(SET, DCT + "created", f'"{DATE}"^^<{XSD}date>')
    t(SET, VOID + "linkPredicate", f"<{SKOS}exactMatch>")
    t(SET, SSSOM + "mapping_justification", f"<{SEMAPV}ManualMappingCuration>")

    for i, (s, lbl, wd) in enumerate(rows, 1):
        # the direct, queryable mapping triple (this is what the join routes through)
        t(s, SKOS + "exactMatch", f"<{wd}>")
        # provenance: an owl:Axiom reifying the mapping, with SSSOM annotations
        ax = f"{SET}/m{i}"
        t(ax, RDF + "type", f"<{OWL}Axiom>")
        t(ax, OWL + "annotatedSource", f"<{s}>")
        t(ax, OWL + "annotatedProperty", f"<{SKOS}exactMatch>")
        t(ax, OWL + "annotatedTarget", f"<{wd}>")
        t(ax, SSSOM + "mapping_justification", f"<{SEMAPV}ManualMappingCuration>")
        t(ax, SSSOM + "confidence", f'"1.0"^^<{XSD}decimal>')
        t(ax, SSSOM + "mapping_set", f"<{SET}>")
        if lbl:
            t(ax, SSSOM + "subject_label", lit(lbl))

    open(NT, "w", encoding="utf-8").write("\n".join(out) + "\n")
    print(f"RDF: {len(out)} triples -> {NT}")


if __name__ == "__main__":
    main()
