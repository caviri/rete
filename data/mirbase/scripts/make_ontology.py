#!/usr/bin/env python3
"""Convert the authored ontology (mirbase.ttl) to N-Triples for the build.

`data/mirbase/mirbase.ttl` is the source of truth — hand-authored, commented,
and validated by .claude/skills/data-ontology/scripts/validate_ontology.py.
The builder consumes one N-Triples stream, so this emits mirbase-ontology.nt
from it rather than keeping a second hand-maintained copy that could drift.

    bash data/mirbase/scripts/py.sh make_ontology.py
"""
from __future__ import annotations

from pathlib import Path

from rdflib import Graph

BASE = Path(__file__).resolve().parent.parent
SRC = BASE / "mirbase.ttl"
OUT = BASE / "mirbase-ontology.nt"


def main() -> None:
    g = Graph()
    g.parse(SRC, format="turtle")
    g.serialize(destination=OUT, format="nt", encoding="utf-8")
    # rdflib writes one triple per line; count them for the build log
    n = sum(1 for line in OUT.read_text(encoding="utf-8").splitlines() if line.strip())
    print(f"ok  {SRC.name} -> {OUT.name} ({n:,} triples, {len(g):,} in graph)")


if __name__ == "__main__":
    main()
