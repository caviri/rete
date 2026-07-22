#!/usr/bin/env python3
"""Imaging Plaza (https://imaging-plaza.epfl.ch) -> N-Triples.

Source: the nightly GraphDB backup published as `graph.trig` by
sdsc-ordes/imaging-plaza-backups. That TriG carries four graphs; we take only

    <https://imaging-plaza.epfl.ch/finalGraph>

which is the *published* catalogue -- exactly what the public site serves at
/api/softwares/search. The `temporaryGraph` (unreviewed drafts) and GraphDB's
own RDFS axiom graph are deliberately dropped.

Two things the source graph needs before it makes a good queryable dataset:

1. Skolemisation. 554 of its nodes are blank -- images, datasets, funding
   records, notebooks. The source already IDs Persons/Organizations under
   `https://imaging-plaza.epfl.ch/instance#<hash>`, so we mint the same shape
   of IRI for the rest, deterministically (parent path + content), so a rebuild
   from a later backup keeps the same IRIs for unchanged records.

2. A link layer. Every IRI-ish value in the source is a plain string literal,
   because the SHACL shapes declare `sh:datatype xsd:string` for them. We keep
   those triples untouched (the file still validates against
   ImagingOntologyShapes.ttl) and *add* IRI-valued links under predicates the
   shapes do not constrain, so the graph federates:

       owl:sameAs        Person   -> https://orcid.org/....
       dcterms:license   Software -> https://spdx.org/licenses/....
       dcterms:references Software -> https://doi.org/....
       schema:funder     Software -> the funder Organization node
       rdfs:label        Organization (from schema:legalName)

Usage:
    python scripts/imaging-plaza/imaging_plaza_to_nt.py \
        data/imaging-plaza/raw/graph.trig \
        -o data/imaging-plaza/imaging-plaza.nt
"""

from __future__ import annotations

import argparse
import hashlib
import re
import sys
from collections import Counter, defaultdict

from rdflib import BNode, Dataset, Literal, URIRef
from rdflib.namespace import RDF

FINAL_GRAPH = URIRef("https://imaging-plaza.epfl.ch/finalGraph")
INST = "https://imaging-plaza.epfl.ch/instance#"

SCHEMA = "http://schema.org/"
SD = "https://w3id.org/okn/o/sd#"
MD4I = "http://w3id.org/nfdi4ing/metadata4ing#"
IMAG = "https://imaging-plaza.epfl.ch/ontology#"
DCTERMS = "http://purl.org/dc/terms/"
OWL_SAMEAS = URIRef("http://www.w3.org/2002/07/owl#sameAs")
RDFS_LABEL = URIRef("http://www.w3.org/2000/01/rdf-schema#label")

S = lambda t: URIRef(SCHEMA + t)  # noqa: E731
SDT = lambda t: URIRef(SD + t)  # noqa: E731

ORCID_RE = re.compile(r"(\d{4}-\d{4}-\d{4}-\d{3}[\dX])")
DOI_RE = re.compile(r"(10\.\d{4,9}/[^\s\"<>]+)", re.I)


# --------------------------------------------------------------------------- #
# N-Triples serialisation
# --------------------------------------------------------------------------- #
_ESCAPES = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t"}


def esc(s: str) -> str:
    out = []
    for ch in s:
        if ch in _ESCAPES:
            out.append(_ESCAPES[ch])
        elif ord(ch) < 0x20:
            out.append(f"\\u{ord(ch):04X}")
        else:
            out.append(ch)
    return "".join(out)


def iri(u: str) -> str:
    # A stray space/newline inside <> breaks every N-Triples parser.
    return "<" + u.strip().replace(" ", "%20").replace("\n", "").replace("\r", "") + ">"


def term(t) -> str:
    if isinstance(t, URIRef):
        return iri(str(t))
    if isinstance(t, Literal):
        s = f'"{esc(str(t))}"'
        if t.language:
            return f"{s}@{t.language}"
        if t.datatype:
            return f"{s}^^{iri(str(t.datatype))}"
        return s
    raise TypeError(f"unskolemised {type(t).__name__}: {t!r}")


# --------------------------------------------------------------------------- #
# Skolemisation
# --------------------------------------------------------------------------- #
def skolemise(graph) -> dict[BNode, URIRef]:
    """Map every blank node to a stable `inst:` IRI.

    The id is derived from the node's *position* (parent IRI + predicate) plus
    its own content, so two sibling datasets with identical descriptions stay
    distinct while an unchanged record keeps its IRI across backups.
    """
    parents: dict[BNode, list[tuple[str, str]]] = defaultdict(list)
    for s, p, o in graph:
        if isinstance(o, BNode):
            parents[o].append((str(s), str(p)))

    def content(node, depth: int = 0) -> str:
        if depth > 6:
            return "..."
        parts = []
        for p, o in graph.predicate_objects(node):
            if isinstance(o, BNode):
                parts.append(f"{p}|B({content(o, depth + 1)})")
            else:
                parts.append(f"{p}|{o}")
        return "".join(sorted(parts))

    mapping: dict[BNode, URIRef] = {}
    used: Counter[str] = Counter()
    nodes = {t for s, p, o in graph for t in (s, o) if isinstance(t, BNode)}
    for bn in sorted(nodes, key=str):
        anchor = "".join(sorted(f"{s}|{p}" for s, p in parents.get(bn, []))) or "ORPHAN"
        sig = f"{anchor}{content(bn)}"
        h = hashlib.sha1(sig.encode("utf-8")).hexdigest()[:32]
        used[h] += 1
        if used[h] > 1:  # exact duplicate signature -> keep them distinct
            h = hashlib.sha1(f"{sig}#{used[h]}".encode("utf-8")).hexdigest()[:32]
        mapping[bn] = URIRef(INST + h)
    return mapping


# --------------------------------------------------------------------------- #
# Normalisers for the derived link layer
# --------------------------------------------------------------------------- #
def orcid_iri(value: str) -> str | None:
    m = ORCID_RE.search(value.strip())
    return f"https://orcid.org/{m.group(1)}" if m else None


def spdx_iri(value: str) -> str | None:
    v = value.strip()
    if not v.startswith(("http://spdx.org/licenses/", "https://spdx.org/licenses/")):
        return None
    ident = v.rsplit("/", 1)[-1]
    if ident.endswith(".html"):
        ident = ident[: -len(".html")]
    return f"https://spdx.org/licenses/{ident}" if ident else None


def doi_iri(value: str) -> str | None:
    m = DOI_RE.search(value.strip())
    if not m:
        return None
    return "https://doi.org/" + m.group(1).rstrip(".,;)")


# --------------------------------------------------------------------------- #
def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("trig", help="graph.trig from sdsc-ordes/imaging-plaza-backups")
    ap.add_argument("-o", "--out", required=True, help="output .nt")
    ap.add_argument("--graph", default=str(FINAL_GRAPH), help="named graph to extract")
    args = ap.parse_args()

    ds = Dataset()
    ds.parse(args.trig, format="trig")
    g = ds.graph(URIRef(args.graph))
    if not len(g):
        print(f"error: graph {args.graph} is empty", file=sys.stderr)
        return 1
    print(f"source graph {args.graph}: {len(g):,} triples", file=sys.stderr)

    sk = skolemise(g)
    print(f"skolemised {len(sk):,} blank nodes -> inst: IRIs", file=sys.stderr)
    res = lambda t: sk[t] if isinstance(t, BNode) else t  # noqa: E731

    stats = Counter()
    seen: set[str] = set()
    lines: list[str] = []

    def emit(s, p, o) -> None:
        line = f"{term(res(s))} {term(res(p))} {term(res(o))} ."
        if line not in seen:
            seen.add(line)
            lines.append(line)

    # 1. the source graph, verbatim (modulo skolemisation)
    for s, p, o in g:
        emit(s, p, o)
    stats["source"] = len(lines)

    # 2. derived link layer -------------------------------------------------- #
    for person, val in g.subject_objects(URIRef(MD4I + "orcidId")):
        if (o := orcid_iri(str(val))) is not None:
            before = len(lines)
            emit(person, OWL_SAMEAS, URIRef(o))
            stats["owl:sameAs -> ORCID"] += len(lines) - before

    for sw, val in g.subject_objects(S("license")):
        if (o := spdx_iri(str(val))) is not None:
            before = len(lines)
            emit(sw, URIRef(DCTERMS + "license"), URIRef(o))
            stats["dcterms:license -> SPDX"] += len(lines) - before

    for sw, val in g.subject_objects(S("citation")):
        if (o := doi_iri(str(val))) is not None:
            before = len(lines)
            emit(sw, URIRef(DCTERMS + "references"), URIRef(o))
            stats["dcterms:references -> DOI"] += len(lines) - before

    # software -> hasFunding -> FundingInformation -> fundingSource -> Organization
    for sw in g.subjects(RDF.type, S("SoftwareSourceCode")):
        for fund in g.objects(sw, SDT("hasFunding")):
            for org in g.objects(fund, SDT("fundingSource")):
                before = len(lines)
                emit(sw, S("funder"), org)
                stats["schema:funder -> Organization"] += len(lines) - before

    for org, name in g.subject_objects(S("legalName")):
        before = len(lines)
        emit(org, RDFS_LABEL, name)
        stats["rdfs:label (Organization)"] += len(lines) - before

    with open(args.out, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(lines) + "\n")

    print(f"\nwrote {args.out}: {len(lines):,} triples", file=sys.stderr)
    for k, v in stats.items():
        print(f"  {v:6,d}  {k}", file=sys.stderr)
    n_sw = len(set(g.subjects(RDF.type, S("SoftwareSourceCode"))))
    print(f"\n  {n_sw} schema:SoftwareSourceCode", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
