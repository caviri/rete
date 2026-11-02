#!/usr/bin/env python3
"""Write a synthetic solar-systems knowledge graph as N-Triples.

The graph is the one from the JupyterLite notebook "Anatomy of a .rete",
scaled by a size parameter: the same ontology (Planet and Moon are subclasses
of CelestialBody, `orbits` has a domain and a range), the same namespace, and
the eight bodies of our own system first. After them come N synthetic
planetary systems, each with a star, a few planets and some moons.

Deterministic for a given (--systems, --seed). No dependencies.

    python gen_solar_kg.py --systems 100 -o solar.nt
"""
import argparse
import random
import sys

NS = "https://example.org/solar/"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
XSD = "http://www.w3.org/2001/XMLSchema#"

# The notebook's bodies: name -> (class, parent, radius in km). The Moon gets
# the IRI "Luna" here so that it does not share an IRI with the class Moon.
SOL = {
    "Sun":     ("Star",   None,      695_700),
    "Mercury": ("Planet", "Sun",       2_440),
    "Venus":   ("Planet", "Sun",       6_052),
    "Earth":   ("Planet", "Sun",       6_371),
    "Luna":    ("Moon",   "Earth",     1_737),
    "Mars":    ("Planet", "Sun",       3_390),
    "Phobos":  ("Moon",   "Mars",         11),
    "Deimos":  ("Moon",   "Mars",          6),
}
LABEL = {"Luna": "Moon"}


def iri(name):
    return f"<{NS}{name}>"


def lit(text, lang=None, dtype=None):
    text = text.replace("\\", "\\\\").replace('"', '\\"')
    if lang:
        return f'"{text}"@{lang}'
    if dtype:
        return f'"{text}"^^<{XSD}{dtype}>'
    return f'"{text}"'


def ontology():
    for cls in ("CelestialBody", "Star", "Planet", "Moon", "PlanetarySystem"):
        yield iri(cls), f"<{RDF_TYPE}>", f"<{RDFS}Class>"
    for cls in ("Star", "Planet", "Moon"):
        yield iri(cls), f"<{RDFS}subClassOf>", iri("CelestialBody")
    yield iri("orbits"), f"<{RDFS}domain>", iri("CelestialBody")
    yield iri("orbits"), f"<{RDFS}range>", iri("CelestialBody")


def body(name, cls, parent, radius, system, label, rng):
    s = iri(name)
    yield s, f"<{RDF_TYPE}>", iri(cls)
    yield s, f"<{RDFS}label>", lit(label, lang="en")
    yield s, iri("radius_km"), lit(str(radius), dtype="integer")
    yield s, iri("inSystem"), iri(system)
    if parent:
        yield s, iri("orbits"), iri(parent)
    if cls != "Star":
        period = round(rng.uniform(0.5, 60000) if cls == "Planet" else rng.uniform(0.3, 400), 2)
        yield s, iri("orbitalPeriodDays"), lit(str(period), dtype="decimal")
    if cls == "Planet":
        atmo = "true" if rng.random() < 0.6 else "false"
        yield s, iri("hasAtmosphere"), lit(atmo, dtype="boolean")


def triples(systems, seed):
    rng = random.Random(seed)
    yield from ontology()

    # System 0: our own, as in the notebook.
    yield iri("Sol"), f"<{RDF_TYPE}>", iri("PlanetarySystem")
    yield iri("Sol"), f"<{RDFS}label>", lit("Solar System", lang="en")
    for name, (cls, parent, radius) in SOL.items():
        yield from body(name, cls, parent, radius, "Sol", LABEL.get(name, name), rng)

    # Systems 1..N: synthetic.
    for i in range(1, systems + 1):
        sys_name = f"S{i}"
        yield iri(sys_name), f"<{RDF_TYPE}>", iri("PlanetarySystem")
        yield iri(sys_name), f"<{RDFS}label>", lit(f"System {i}", lang="en")
        star = f"S{i}_star"
        yield from body(star, "Star", None, rng.randint(150_000, 1_500_000),
                        sys_name, f"Star {i}", rng)
        for p in range(rng.randint(2, 6)):
            planet = f"S{i}_p{p}"
            yield from body(planet, "Planet", star, rng.randint(1_000, 80_000),
                            sys_name, f"System {i} planet {p}", rng)
            for m in range(rng.choice((0, 0, 1, 1, 2, 3))):
                yield from body(f"{planet}_m{m}", "Moon", planet, rng.randint(2, 3_000),
                                sys_name, f"System {i} planet {p} moon {m}", rng)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--systems", type=int, default=100,
                    help="synthetic systems after the Solar System (about 60 triples each)")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("-o", "--out", default="-", help="output file (default: stdout)")
    args = ap.parse_args()

    out = sys.stdout if args.out == "-" else open(args.out, "w", encoding="utf-8", newline="\n")
    n = 0
    with out:
        for s, p, o in triples(args.systems, args.seed):
            out.write(f"{s} {p} {o} .\n")
            n += 1
    print(f"{n} triples", file=sys.stderr)


if __name__ == "__main__":
    main()
