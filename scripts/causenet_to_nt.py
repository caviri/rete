#!/usr/bin/env python3
"""CauseNet (causenet.org) JSONL -> N-Triples, fully lossless.

Reads ``causenet-full.jsonl`` (or ``.jsonl.bz2``) — the high-recall causality
graph Heindorf et al. extracted from Wikipedia + ClueWeb12 — and emits one
N-Triples graph that keeps **every** field, including all source sentences.

Model (vocabulary under ``https://causenet.org/ontology#`` = ``cn:``):

  concept node   https://causenet.org/concept/<urlencoded>
      cn:Concept ; rdfs:label "smoking"
  direct edge    <cause> cn:causes <effect>           # the transitive/reach predicate
  relation node  https://causenet.org/relation/<enc(cause)>/<enc(effect)>
      a cn:CausalRelation ; cn:cause <cause> ; cn:effect <effect> ;
      cn:support N^^xsd:integer ;                      # TRUE number of sources
      cn:hasSource _:s1, _:s2, …                       # one blank node per source
  source node    _:sN  a cn:{ClueWeb12Sentence|WikipediaSentence|WikipediaInfobox|
                              WikipediaList}Source ;
      cn:sentence "…" ; cn:pattern "[[cause]]/N\t…\t[[effect]]/N" ;
      + every payload field (page ids/titles/timestamps/headings/template args/…)

Every JSONL field maps to exactly one triple, so the graph round-trips. The
``cn:support`` count is the real source count even though sources are also kept
in full. Sources are blank nodes (build-local, predicate-bound provenance) to
keep the dictionary off the ~55 M source IRIs.

Concept type+label triples are emitted once per distinct concept (an in-memory
set), because the rete streaming assembler does not de-duplicate identical
id-triples — every emitted line must be unique.

Usage (in Docker, writing to a big scratch drive):
  python scripts/causenet_to_nt.py /data/causenet-full.jsonl.bz2 /scratch/causenet-full.nt
"""

from __future__ import annotations

import bz2
import io
import sys
from urllib.parse import quote

try:
    import orjson as _json

    def loads(b):
        return _json.loads(b)
except ModuleNotFoundError:  # stdlib fallback
    import json as _json

    def loads(b):
        return _json.loads(b)

ONT = "https://causenet.org/ontology#"
CONCEPT = "https://causenet.org/concept/"
RELATION = "https://causenet.org/relation/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS_LABEL = "http://www.w3.org/2000/01/rdf-schema#label"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"
XSD_DT = "http://www.w3.org/2001/XMLSchema#dateTime"

# Per-source-type: the class IRI local name, and (payload key -> property local
# name, datatype) so every field is preserved. ``sentence`` and ``path_pattern``
# are shared and handled separately.
SOURCE_CLASS = {
    "clueweb12_sentence": "ClueWeb12SentenceSource",
    "wikipedia_sentence": "WikipediaSentenceSource",
    "wikipedia_infobox": "WikipediaInfoboxSource",
    "wikipedia_list": "WikipediaListSource",
}
# payload key -> (cn property local name, datatype IRI or None for plain string)
FIELD = {
    "sentence": ("sentence", None),
    "path_pattern": ("pattern", None),
    "wikipedia_page_id": ("wikipediaPageId", None),
    "wikipedia_page_title": ("wikipediaPageTitle", None),
    "wikipedia_revision_id": ("wikipediaRevisionId", None),
    "wikipedia_revision_timestamp": ("wikipediaRevisionTimestamp", XSD_DT),
    "sentence_section_heading": ("sentenceSectionHeading", None),
    "sentence_section_level": ("sentenceSectionLevel", None),
    "clueweb12_page_id": ("clueweb12PageId", None),
    "clueweb12_page_reference": ("clueweb12PageReference", None),
    "clueweb12_page_timestamp": ("clueweb12PageTimestamp", XSD_DT),
    "infobox_template": ("infoboxTemplate", None),
    "infobox_title": ("infoboxTitle", None),
    "infobox_argument": ("infoboxArgument", None),
    "list_toc_parent_title": ("listTocParentTitle", None),
    "list_toc_section_heading": ("listTocSectionHeading", None),
    "list_toc_section_level": ("listTocSectionLevel", None),
}

# N-Triples literal escaping. Backslash/quote/TAB/LF/CR get the short ECHAR
# forms; every other C0 control char (0x00-0x1F) and DEL (0x7F) — which turn up
# in web-scraped ClueWeb12 sentences and are illegal raw in an N-Triples literal
# — gets a \u00XX UCHAR escape. Printable UTF-8 is kept raw (legal in NT 1.1).
_ESC_MAP = {
    0x5C: "\\\\",
    0x22: '\\"',
    0x09: "\\t",
    0x0A: "\\n",
    0x0D: "\\r",
}
for _c in list(range(0x00, 0x20)) + [0x7F]:
    _ESC_MAP.setdefault(_c, f"\\u{_c:04X}")
_ESC = str.maketrans(_ESC_MAP)


def lit(s: str) -> str:
    return '"' + s.translate(_ESC) + '"'


def concept_iri(c: str) -> str:
    return CONCEPT + quote(c, safe="")


def open_in(path: str):
    if path.endswith(".bz2"):
        return bz2.open(path, "rb")
    return open(path, "rb")


def main() -> None:
    inp, outp = sys.argv[1], sys.argv[2]
    seen_concepts: set[str] = set()
    n_rel = 0
    n_src = 0
    n_triples = 0
    buf: list[str] = []
    BUFMAX = 16384

    out = io.open(outp, "w", encoding="utf-8", buffering=1 << 22)
    write = out.write

    def emit(s_term: str, p_local: str, o_term: str) -> None:
        # s_term / o_term already fully formatted (<iri>, _:b, or "lit"...).
        buf.append(f"{s_term} <{ONT}{p_local}> {o_term} .\n")

    n_bad = 0
    with open_in(inp) as f:
        for raw in f:
            if not raw or raw == b"\n":
                continue
            try:
                d = loads(raw)
                cr = d["causal_relation"]
                cause = cr["cause"]["concept"]
                effect = cr["effect"]["concept"]
            except Exception:
                n_bad += 1
                continue
            if not cause or not effect:
                continue
            c_iri = concept_iri(cause)
            e_iri = concept_iri(effect)
            c_term = f"<{c_iri}>"
            e_term = f"<{e_iri}>"

            # Concepts (once each): type + label.
            for con, term, lab in ((cause, c_term, cause), (effect, e_term, effect)):
                if con not in seen_concepts:
                    seen_concepts.add(con)
                    buf.append(f"{term} <{RDF_TYPE}> <{ONT}Concept> .\n")
                    buf.append(f"{term} <{RDFS_LABEL}> {lit(lab)} .\n")
                    n_triples += 2

            # Direct causal edge (the reach/path predicate).
            emit(c_term, "causes", e_term)

            # Reified relation node.
            r_iri = f"{RELATION}{quote(cause, safe='')}/{quote(effect, safe='')}"
            r_term = f"<{r_iri}>"
            buf.append(f"{r_term} <{RDF_TYPE}> <{ONT}CausalRelation> .\n")
            emit(r_term, "cause", c_term)
            emit(r_term, "effect", e_term)
            sources = d.get("sources", [])
            buf.append(
                f'{r_term} <{ONT}support> "{len(sources)}"^^<{XSD_INT}> .\n'
            )
            n_triples += 5  # causes + type + cause + effect + support

            # Sources (blank nodes), every field preserved.
            for src in sources:
                n_src += 1
                b = f"_:s{n_src}"
                cls = SOURCE_CLASS.get(src.get("type"), "Source")
                buf.append(f"{r_term} <{ONT}hasSource> {b} .\n")
                buf.append(f"{b} <{RDF_TYPE}> <{ONT}{cls}> .\n")
                n_triples += 2
                for k, v in src.get("payload", {}).items():
                    fld = FIELD.get(k)
                    if fld is None or v is None or v == "":
                        continue
                    prop, dt = fld
                    if dt:
                        o = f"{lit(v)}^^<{dt}>"
                    else:
                        o = lit(v)
                    buf.append(f"{b} <{ONT}{prop}> {o} .\n")
                    n_triples += 1

            n_rel += 1
            if len(buf) >= BUFMAX:
                write("".join(buf))
                buf.clear()
            if n_rel % 1_000_000 == 0:
                sys.stderr.write(
                    f"  {n_rel:,} relations, {n_src:,} sources, "
                    f"{n_triples:,} triples, {len(seen_concepts):,} concepts\n"
                )
                sys.stderr.flush()

    if buf:
        write("".join(buf))
    out.close()
    sys.stderr.write(
        f"DONE: {n_rel:,} relations, {n_src:,} sources, "
        f"{len(seen_concepts):,} concepts, {n_triples:,} triples, "
        f"{n_bad} bad lines -> {outp}\n"
    )


if __name__ == "__main__":
    main()
