#!/usr/bin/env python3
"""Stochastic scholarly-knowledge-graph generator (N-Triples / N-Quads).

Generates a realistic, richly-typed research-world graph — papers, authors,
venues, institutions, grants, fields — with the statistical structure real
scholarly graphs have:

* **power-law citations** (preferential attachment, recency-biased, papers
  only cite older papers);
* **community structure** (fields cluster authors, venues, and citations;
  the pyramid summarizer gets real communities to find);
* **Zipfian venue popularity** and log-normal institution sizes / team sizes;
* **typed literals** (xsd:integer, xsd:date, xsd:double, xsd:boolean),
  multi-valued properties, and per-field title/abstract vocabulary.

Two orthogonal knobs control the output on demand:

* `--papers N` — size (everything else scales stochastically from it);
* `--noise R` — fraction of *deliberate mess*: structure-breaking rewires
  (cross-field citations/venues/authors, temporal violations), dropped
  optional attributes, and untyped/whitespace-mangled literals. `0.0` is a
  clean graph; `0.3` is a swamp.

`--seed` makes any configuration reproducible; different seeds give natural
variability at the same size/noise point.

Usage:
  uv run python scripts/synth_graph.py --papers 10000 --noise 0.05 -o out.nt
  uv run python scripts/synth_graph.py --papers 50000 --noise 0.2 --quads -o out.nq
  uv run python scripts/synth_graph.py --stats-only --papers 100000

Then: `rete build out.nt -o out.rete`.
"""

from __future__ import annotations

import argparse
import math
import random
import sys
from collections import Counter

EX = "http://ex/"
XSD = "http://www.w3.org/2001/XMLSchema#"
RDF_TYPE = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"
DCT = "http://purl.org/dc/terms/"
CITO = "http://purl.org/spar/cito/"
FOAF = "http://xmlns.com/foaf/0.1/"

FIELD_WORDS = {
    "genomics": ["genome", "sequencing", "variant", "expression", "CRISPR", "transcriptome"],
    "neuroscience": ["cortex", "synaptic", "neuron", "plasticity", "connectome", "dopamine"],
    "machine-learning": ["neural", "transformer", "gradient", "embedding", "attention", "generalization"],
    "climate": ["warming", "carbon", "ocean", "albedo", "permafrost", "monsoon"],
    "materials": ["perovskite", "graphene", "lattice", "superconducting", "alloy", "nanowire"],
    "epidemiology": ["outbreak", "transmission", "cohort", "vaccination", "incidence", "serology"],
    "astronomy": ["exoplanet", "redshift", "supernova", "accretion", "pulsar", "interferometry"],
    "economics": ["equilibrium", "elasticity", "auction", "incentive", "volatility", "welfare"],
    "robotics": ["manipulation", "locomotion", "SLAM", "actuator", "teleoperation", "compliance"],
    "chemistry": ["catalysis", "ligand", "polymer", "electrolyte", "chirality", "photoredox"],
    "linguistics": ["morphology", "syntax", "phoneme", "corpus", "prosody", "etymology"],
    "immunology": ["antibody", "T-cell", "cytokine", "antigen", "autoimmune", "checkpoint"],
}
GENERIC_WORDS = ["analysis", "model", "framework", "dynamics", "evidence", "approach",
                 "structure", "estimation", "characterization", "review", "mechanisms", "theory"]
COUNTRIES = ["Switzerland", "Germany", "France", "Japan", "Brazil", "Kenya", "India",
             "Canada", "Australia", "Norway", "Korea", "Mexico", "Poland", "Vietnam",
             "Chile", "Egypt", "Netherlands", "Italy", "Spain", "Portugal"]
FUNDERS = ["SNSF", "ERC", "NIH", "NSF", "DFG", "JST", "WellcomeTrust", "GatesFoundation"]
INSTITUTION_KINDS = [("University", 0.55), ("Institute", 0.25), ("Hospital", 0.08),
                     ("Company", 0.07), ("Observatory", 0.05)]
SYLLABLES = ["ka", "ri", "to", "an", "be", "lu", "mo", "shi", "ve", "da", "el", "ny",
             "or", "pa", "qu", "sa", "te", "ul", "wa", "zo", "mi", "ha", "ce", "ix"]


def iri(local: str) -> str:
    return f"<{EX}{local}>"


def lit(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def typed(value, kind: str) -> str:
    return f'"{value}"^^<{XSD}{kind}>'


class Gen:
    def __init__(self, args):
        self.rng = random.Random(args.seed)
        self.args = args
        self.noise = args.noise
        self.applied_noise = Counter()
        self.counts = Counter()
        self.out = None

    # ---- small stochastic helpers -------------------------------------
    def zipf_index(self, n: int, s: float = 1.1) -> int:
        """Zipf-distributed index in [0, n) (popular items get most mass)."""
        weights = getattr(self, "_zipf_cache", {}).get((n, s))
        if weights is None:
            weights = [1.0 / (k + 1) ** s for k in range(n)]
            self._zipf_cache = getattr(self, "_zipf_cache", {})
            self._zipf_cache[(n, s)] = weights
        return self.rng.choices(range(n), weights=weights, k=1)[0]

    def lognormal_int(self, mu: float, sigma: float, lo: int, hi: int) -> int:
        return max(lo, min(hi, int(self.rng.lognormvariate(mu, sigma))))

    def person_name(self) -> str:
        r = self.rng
        given = "".join(r.choice(SYLLABLES) for _ in range(r.randint(2, 3))).capitalize()
        family = "".join(r.choice(SYLLABLES) for _ in range(r.randint(2, 4))).capitalize()
        return f"{given} {family}"

    def noisy(self, what: str) -> bool:
        if self.rng.random() < self.noise:
            self.applied_noise[what] += 1
            return True
        return False

    def dropped(self, what: str) -> bool:
        """Optional attributes go missing at half the noise rate."""
        if self.rng.random() < self.noise / 2:
            self.applied_noise[f"missing {what}"] += 1
            return True
        return False

    def mangle(self, text: str) -> str:
        """Whitespace/case mess for literal values (still valid N-Triples)."""
        r = self.rng
        choice = r.randint(0, 2)
        if choice == 0:
            return "  " + text
        if choice == 1:
            return text.upper()
        return text.replace(" ", "  ", 1)

    # ---- emission ------------------------------------------------------
    def emit(self, s: str, p: str, o: str, graph: str | None = None):
        if self.args.quads and graph is not None:
            self.out.write(f"{s} {p} {o} {graph} .\n")
        else:
            self.out.write(f"{s} {p} {o} .\n")
        self.counts["triples"] += 1

    # ---- the world -----------------------------------------------------
    def run(self, out):
        self.out = out
        r = self.rng
        a = self.args
        papers_n = a.papers
        fields = list(FIELD_WORDS)[: a.fields]

        # --- geography & institutions ---
        cities = []
        for country in COUNTRIES:
            for c in range(r.randint(1, 4)):
                city = iri(f"city/{country}/{c}")
                cities.append(city)
                self.emit(city, RDF_TYPE, iri("City"))
                self.emit(city, iri("country"), lit(country))
        self.counts["cities"] = len(cities)

        institutions = []
        inst_sizes = []
        n_inst = max(8, papers_n // 150)
        kinds, kind_w = zip(*INSTITUTION_KINDS)
        for i in range(n_inst):
            kind = r.choices(kinds, weights=kind_w, k=1)[0]
            inst = iri(f"institution/{i}")
            institutions.append(inst)
            inst_sizes.append(self.lognormal_int(3.0, 1.0, 1, 100_000))
            self.emit(inst, RDF_TYPE, iri(kind))
            self.emit(inst, iri("name"), lit(f"{kind} {i}"))
            self.emit(inst, iri("locatedIn"), r.choice(cities))
        self.counts["institutions"] = n_inst

        # --- fields & venues ---
        for f in fields:
            fi = iri(f"field/{f}")
            self.emit(fi, RDF_TYPE, iri("Field"))
            self.emit(fi, iri("name"), lit(f))
        venues_by_field: dict[str, list[str]] = {f: [] for f in fields}
        n_venues = max(3, papers_n // 400)
        for f in fields:
            for v in range(n_venues):
                kind = "Journal" if r.random() < 0.55 else "Conference"
                venue = iri(f"venue/{f}/{v}")
                venues_by_field[f].append(venue)
                self.emit(venue, RDF_TYPE, iri(kind))
                self.emit(venue, iri("name"), lit(f"{kind} of {f} {v}"))
                self.emit(venue, iri("hasField"), iri(f"field/{f}"))
                if kind == "Journal" and not self.dropped("issn"):
                    self.emit(venue, iri("issn"), lit(f"{r.randint(1000, 9999)}-{r.randint(1000, 9999)}"))
                self.emit(venue, iri("openAccess"), typed("true" if r.random() < 0.4 else "false", "boolean"))
        self.counts["venues"] = n_venues * len(fields)

        # --- grants ---
        grants = []
        for g in range(max(2, papers_n // 40)):
            grant = iri(f"grant/{g}")
            grants.append(grant)
            self.emit(grant, RDF_TYPE, iri("Grant"))
            self.emit(grant, iri("funder"), lit(r.choice(FUNDERS)))
            self.emit(grant, iri("grantValue"), typed(self.lognormal_int(12, 1.2, 10_000, 20_000_000), "integer"))
            self.emit(grant, iri("hasField"), iri(f"field/{r.choice(fields)}"))
        self.counts["grants"] = len(grants)

        # --- authors (field communities, weighted affiliations) ---
        n_authors = max(10, int(papers_n * 0.55))
        author_field: list[str] = []
        for i in range(n_authors):
            author = iri(f"author/{i}")
            field = fields[i % len(fields)] if r.random() < 0.8 else r.choice(fields)
            author_field.append(field)
            self.emit(author, RDF_TYPE, iri("Person"))
            self.emit(author, f"<{FOAF}name>", lit(self.person_name()))
            self.emit(author, iri("primaryField"), iri(f"field/{field}"))
            n_aff = 1 if r.random() < 0.85 else 2
            for inst in r.choices(institutions, weights=inst_sizes, k=n_aff):
                target = r.choice(institutions) if self.noisy("affiliation rewire") else inst
                self.emit(author, iri("affiliation"), target)
            if not self.dropped("orcid"):
                self.emit(author, iri("orcid"),
                          lit(f"0000-{r.randint(0, 9999):04}-{r.randint(0, 9999):04}-{r.randint(0, 9999):04}"))
            if not self.dropped("hIndex"):
                self.emit(author, iri("hIndex"), typed(self.lognormal_int(1.8, 0.9, 0, 200), "integer"))
        self.counts["authors"] = n_authors
        authors_in_field = {f: [i for i in range(n_authors) if author_field[i] == f] for f in fields}

        # --- papers: temporal ramp, preferential-attachment citations ---
        y0, y1 = a.years
        years = list(range(y0, y1 + 1))
        year_weights = [1.0 + 2.0 * (y - y0) / max(1, y1 - y0) for y in years]  # more recent papers
        papers: list[tuple[int, str]] = []  # (year, field) per paper, in id order
        cite_targets: list[int] = []  # flattened preferential-attachment pool
        paper_field_pool: dict[str, list[int]] = {f: [] for f in fields}

        for p in range(papers_n):
            field = fields[self.zipf_index(len(fields), 0.7)]
            year = r.choices(years, weights=year_weights, k=1)[0]
            papers.append((year, field))
        papers.sort()  # cite only earlier ids ⇒ sort by year once
        paper_graph = lambda year: iri(f"graph/{year}")

        for pid, (year, field) in enumerate(papers):
            paper = iri(f"paper/{pid}")
            g = paper_graph(year)
            self.emit(paper, RDF_TYPE, iri("Paper"), g)
            words = FIELD_WORDS[field]
            title = f"{r.choice(words).capitalize()} {r.choice(GENERIC_WORDS)} of {r.choice(words)} {r.choice(GENERIC_WORDS)}"
            if self.noisy("mangled literal"):
                title = self.mangle(title)
            self.emit(paper, f"<{DCT}title>", lit(title), g)
            self.emit(paper, f"<{DCT}date>", typed(f"{year}-{r.randint(1, 12):02}-{r.randint(1, 28):02}", "date"), g)
            self.emit(paper, iri("hasField"), iri(f"field/{field}"), g)
            self.emit(paper, iri("doi"), lit(f"10.5555/synth.{pid}"), g)
            if not self.dropped("abstract"):
                sent = " ".join(r.choice(words + GENERIC_WORDS) for _ in range(r.randint(12, 30)))
                self.emit(paper, f"<{DCT}abstract>", lit(f"We study {sent}."), g)
            for _ in range(r.randint(2, 5)):
                if not self.dropped("keyword"):
                    self.emit(paper, iri("keyword"), lit(r.choice(words)), g)
            score = r.lognormvariate(0.0, 0.6)
            self.emit(paper, iri("noveltyScore"), typed(f"{score:.3f}", "double"), g)

            # venue: Zipf within the field (noise ⇒ any field's venue)
            vlist = venues_by_field[field]
            venue = vlist[self.zipf_index(len(vlist))]
            if self.noisy("venue rewire"):
                venue = r.choice(venues_by_field[r.choice(fields)])
            self.emit(paper, iri("publishedIn"), venue, g)

            # authors: log-normal team size, drawn from the field's community
            pool = authors_in_field[field] or list(range(n_authors))
            team_n = self.lognormal_int(1.0, 0.5, 1, 12)
            team = r.sample(pool, k=min(team_n, len(pool)))
            for ai in team:
                target = r.randrange(n_authors) if self.noisy("author rewire") else ai
                self.emit(paper, f"<{DCT}creator>", iri(f"author/{target}"), g)
            for i in range(len(team)):
                for j in range(i + 1, len(team)):
                    self.emit(iri(f"author/{team[i]}"), iri("coauthor"), iri(f"author/{team[j]}"), g)
                    self.emit(iri(f"author/{team[j]}"), iri("coauthor"), iri(f"author/{team[i]}"), g)
            self.counts["coauthor edges"] += len(team) * (len(team) - 1)

            if r.random() < 0.5 and grants:
                self.emit(paper, iri("funding"), r.choice(grants), g)

            # citations: preferential attachment over older papers, same field,
            # recency-biased; noise ⇒ uniform target anywhere (even the future).
            n_cites = self.lognormal_int(1.6, 0.8, 0, 60)
            field_pool = paper_field_pool[field]
            for _ in range(min(n_cites, pid)):
                if self.noisy("citation rewire"):
                    target = r.randrange(papers_n)  # any paper, any year: noise
                elif field_pool and r.random() < 0.8:
                    # preferential attachment: pick from the flattened pool where
                    # heavily-cited papers appear more often; recency bias via
                    # sampling the most recent half of the pool more.
                    pool_src = cite_targets if cite_targets and r.random() < 0.6 else field_pool
                    lo = len(pool_src) // 2 if r.random() < 0.6 else 0
                    target = pool_src[r.randrange(lo, len(pool_src))]
                else:
                    target = r.randrange(pid)
                if target != pid:
                    self.emit(paper, f"<{CITO}cites>", iri(f"paper/{target}"), g)
                    cite_targets.append(target)
                    self.counts["citations"] += 1
            paper_field_pool[field].append(pid)
        self.counts["papers"] = papers_n

    # ---- reporting -------------------------------------------------------
    def report(self):
        e = sys.stderr.write
        a = self.args
        e(f"synth_graph: papers={a.papers} noise={a.noise} seed={a.seed} "
          f"fields={a.fields} years={a.years[0]}-{a.years[1]} quads={a.quads}\n")
        for key in ["triples", "papers", "authors", "venues", "institutions",
                    "cities", "grants", "citations", "coauthor edges"]:
            e(f"  {key}: {self.counts[key]}\n")
        if self.applied_noise:
            total = sum(self.applied_noise.values())
            e(f"  noise events: {total} ({', '.join(f'{k}: {v}' for k, v in sorted(self.applied_noise.items()))})\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--papers", type=int, default=10_000, help="paper count; everything scales from it")
    ap.add_argument("--noise", type=float, default=0.0, help="0..1 mess ratio (rewires, missing attrs, mangled literals)")
    ap.add_argument("--seed", type=int, default=42, help="RNG seed (same args + seed = identical output)")
    ap.add_argument("--fields", type=int, default=12, choices=range(2, len(FIELD_WORDS) + 1),
                    metavar="K", help="number of field communities (2..12)")
    ap.add_argument("--years", type=int, nargs=2, default=(1995, 2025), metavar=("FROM", "TO"))
    ap.add_argument("--quads", action="store_true", help="emit N-Quads with per-year named graphs for paper data")
    ap.add_argument("-o", "--output", default=None, help="output path (default: stdout)")
    ap.add_argument("--stats-only", action="store_true", help="generate, print stats, discard triples")
    args = ap.parse_args()
    if not 0.0 <= args.noise <= 1.0:
        ap.error("--noise must be in [0, 1]")

    gen = Gen(args)
    if args.stats_only:
        class Sink:
            def write(self, _):
                pass
        gen.run(Sink())
    elif args.output:
        with open(args.output, "w", encoding="utf-8", newline="\n") as f:
            gen.run(f)
    else:
        gen.run(sys.stdout)
    gen.report()


if __name__ == "__main__":
    main()
