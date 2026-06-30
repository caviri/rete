#!/usr/bin/env python3
"""OWL/XML → N-Triples, the fallback for ontologies `rete build` can't parse.

`rete build` reads RDF/XML directly (most `.owl` files), so try building the .owl
as-is FIRST. Only reach for this when the file is **OWL/XML** (a different syntax
from RDF/XML) — rdflib/rapper can't read it either; owlready2 can.

  pip install --break-system-packages owlready2
  python owl_to_nt.py input.owl output.nt

Then build: rete build output.nt -o out.rete --pyramid-algo types --card
"""
import os
import sys


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: owl_to_nt.py <input.owl> <output.nt>")
    inp, outp = sys.argv[1], sys.argv[2]
    try:
        from owlready2 import get_ontology, default_world
    except ImportError:
        sys.exit("owlready2 not installed: pip install --break-system-packages owlready2")

    path = os.path.abspath(inp)
    uri = "file://" + path.replace(os.sep, "/")
    if not uri.startswith("file:///"):
        uri = uri.replace("file://", "file:///", 1)
    sys.stderr.write(f"loading {uri} …\n")
    get_ontology(uri).load()
    # save the whole quad-store as N-Triples (owlready2 supports the 'ntriples' format)
    default_world.save(file=outp, format="ntriples")
    n = sum(1 for _ in open(outp, encoding="utf-8", errors="replace"))
    sys.stderr.write(f"owl_to_nt: wrote {outp} ({n} lines)\n")


if __name__ == "__main__":
    main()
