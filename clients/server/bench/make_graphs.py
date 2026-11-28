"""Synthetic graphs for the benchmark: /data/g0.rete … g3.rete.

Each has about TRIPLES triples: entities in 20 classes, each with a label, a
language-tagged name, an integer, a decimal and two links to other entities,
the mix of IRIs and literals a real graph has. Seeded, so every run measures
the same files; rebuilt only when the size or the engine version changes.
"""
import importlib.metadata
import json
import os
import pathlib
import random
import time

import rete_graph as rete

GRAPHS = 4
TRIPLES = int(os.environ.get("TRIPLES", "200000"))
OUT = pathlib.Path("/data")
EX = "http://bench.rete.example/"
RDF_TYPE = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"
LABEL = "<http://www.w3.org/2000/01/rdf-schema#label>"
XSD = "http://www.w3.org/2001/XMLSchema#"
WORDS = ("amber", "basalt", "cedar", "delta", "ember", "fjord", "garnet", "harbor",
         "iris", "juniper", "kelp", "lumen", "meadow", "nectar", "onyx", "prairie")

marker = OUT / ".bench.json"
want = {"graphs": GRAPHS, "triples": TRIPLES, "engine": importlib.metadata.version("rete-graph")}
if marker.exists() and json.loads(marker.read_text()) == want:
    print(f"graphs: up to date ({GRAPHS} × ~{TRIPLES} triples)", flush=True)
    raise SystemExit

entities = TRIPLES // 7
for g in range(GRAPHS):
    started = time.time()
    rnd = random.Random(g)
    lines = []
    for i in range(entities):
        s = f"<{EX}e{i}>"
        lines += [
            f"{s} {RDF_TYPE} <{EX}C{i % 20}> .",
            f'{s} {LABEL} "Entity {i} of graph {g}" .',
            f'{s} <{EX}name> "{rnd.choice(WORDS)} {rnd.choice(WORDS)}"@en .',
            f'{s} <{EX}year> "{1900 + rnd.randrange(125)}"^^<{XSD}integer> .',
            f'{s} <{EX}score> "{rnd.random():.4f}"^^<{XSD}decimal> .',
            f"{s} <{EX}cites> <{EX}e{rnd.randrange(entities)}> .",
            f"{s} <{EX}cites> <{EX}e{rnd.randrange(entities)}> .",
        ]
    data = rete.build("\n".join(lines) + "\n", "nt")
    (OUT / f"g{g}.rete").write_bytes(data)
    print(f"graphs: g{g}.rete {len(lines)} triples, {len(data) / 2**20:.1f} MiB, "
          f"{time.time() - started:.1f}s", flush=True)
marker.write_text(json.dumps(want))
