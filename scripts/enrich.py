#!/usr/bin/env python3
"""Enrich an OpenCitations citation graph with DETERMINISTIC SYNTHETIC metadata.

The citation edges, DOIs, and publication years are REAL (from OpenCitations).
Everything this script adds — titles, authors, coauthorship, venues, disciplines,
keywords, full dates, citation counts — is **fabricated** (seeded by a hash of
each DOI, so it's reproducible) purely to make the demo graph rich and
multi-relational. It is NOT real bibliographic metadata. Do not treat it as such.

Reads `data/opencitations/cites-all.nt`, writes:
  - data/opencitations/enriched-all.nt           (everything)
  - data/opencitations/enriched-<year>.nt        (sharded by citing year)

Vocab: dcterms (title/creator/subject/issued), foaf (Person/name),
prism:publicationName, plus ex: for discipline/coauthor/citationCount/synthetic.
"""
import hashlib
import os
import re
import sys

OUT = sys.argv[1] if len(sys.argv) > 1 else "data/opencitations"
SRC = os.path.join(OUT, "cites-all.nt")

DCT = "http://purl.org/dc/terms/"
FOAF = "http://xmlns.com/foaf/0.1/"
PRISM = "http://prismstandard.org/namespaces/basic/2.0/publicationName"
EX = "http://ex/"
XSD_INT = "http://www.w3.org/2001/XMLSchema#integer"
RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"

# Disciplines, each with keyword pool + venues + title fragments.
DISCIPLINES = {
    "Biology": {
        "kw": ["protein", "genome", "cell", "rna", "enzyme", "mutation", "expression",
               "pathway", "molecular", "sequencing", "phenotype", "microbiome"],
        "venues": ["Journal of Molecular Biology", "Cell Reports", "Nature Genetics"],
    },
    "ComputerScience": {
        "kw": ["neural", "network", "learning", "algorithm", "model", "inference",
               "optimization", "embedding", "transformer", "dataset", "training", "graph"],
        "venues": ["NeurIPS Proceedings", "Journal of Machine Learning", "IEEE Transactions"],
    },
    "Medicine": {
        "kw": ["patient", "clinical", "trial", "therapy", "diagnosis", "treatment",
               "disease", "cohort", "outcome", "biomarker", "cancer", "vaccine"],
        "venues": ["The Lancet", "JAMA", "New England Journal of Medicine"],
    },
    "Chemistry": {
        "kw": ["catalyst", "synthesis", "reaction", "molecule", "compound", "polymer",
               "bond", "spectroscopy", "crystal", "solvent", "ligand", "oxidation"],
        "venues": ["Journal of the American Chemical Society", "Angewandte Chemie", "Chem"],
    },
    "Physics": {
        "kw": ["quantum", "particle", "field", "energy", "symmetry", "lattice",
               "photon", "spin", "phase", "relativity", "boson", "tensor"],
        "venues": ["Physical Review Letters", "Journal of High Energy Physics", "Nature Physics"],
    },
    "Bioinformatics": {
        "kw": ["alignment", "structure", "prediction", "folding", "annotation",
               "homology", "pipeline", "benchmark", "database", "model", "residue", "contact"],
        "venues": ["Bioinformatics", "PLOS Computational Biology", "BMC Bioinformatics"],
    },
}
DNAMES = list(DISCIPLINES)
FIRST = ["Alex", "Sam", "Maria", "Wei", "Yuki", "Omar", "Lena", "Raj", "Sofia", "Ivan",
         "Chen", "Aisha", "Pablo", "Nina", "Tomas", "Fatima", "Erik", "Mei", "Diego", "Anna"]
LAST = ["Smith", "Wang", "Garcia", "Kumar", "Muller", "Rossi", "Tanaka", "Khan", "Lopez",
        "Novak", "Silva", "Cohen", "Ahmed", "Petrov", "Kim", "Dubois", "Haas", "Costa"]


def h(s, salt=""):
    return int(hashlib.md5((salt + s).encode()).hexdigest(), 16)


def pick(pool, n, seed):
    out, i = [], 0
    while len(out) < n and i < n * 5:
        c = pool[(seed + i * 2654435761) % len(pool)]
        if c not in out:
            out.append(c)
        i += 1
    return out


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"')


# --- collect citing DOIs + years from the real data ------------------------
date_re = re.compile(r'^<https://doi.org/([^>]+)> <http://purl.org/dc/terms/date> "([^"]*)"')
papers = {}  # doi -> year
if not os.path.exists(SRC):
    sys.exit(f"missing {SRC} — run scripts/fetch_opencitations.py first")
for line in open(SRC, encoding="utf-8"):
    m = date_re.match(line)
    if m:
        papers[m.group(1)] = m.group(2)
print(f"{len(papers)} citing papers to enrich", file=sys.stderr)

# Author pool sized so authors are shared across papers (→ coauthorship structure).
n_authors = max(40, len(papers) // 6)


def author_iri(i):
    return f"<{EX}author/{i}>"


def enrich_triples(doi):
    """Synthetic metadata triples for one citing DOI (list of N-Triples lines)."""
    s = f"<https://doi.org/{doi}>"
    seed = h(doi)
    disc = DNAMES[seed % len(DNAMES)]
    info = DISCIPLINES[disc]
    kws = pick(info["kw"], 3 + seed % 3, seed)
    venue = info["venues"][(seed >> 3) % len(info["venues"])]
    title = f"{kws[0].capitalize()} {kws[1]} for {disc.lower()}: a {kws[-1]} study"
    n_auth = 1 + seed % 3
    authors = [(seed >> (4 + j)) % n_authors for j in range(n_auth)]
    authors = list(dict.fromkeys(authors))  # dedup
    cit = seed % 250
    month = 1 + (seed >> 5) % 12
    day = 1 + (seed >> 9) % 28
    year = papers[doi]

    out = [
        f'{s} <{DCT}title> "{esc(title)}" .',
        f'{s} <{EX}discipline> <{EX}discipline/{disc}> .',
        f'{s} <{PRISM}> "{esc(venue)}" .',
        f'{s} <{DCT}issued> "{year}-{month:02d}-{day:02d}" .',
        f'{s} <{EX}citationCount> "{cit}"^^<{XSD_INT}> .',
        f'{s} <{EX}synthetic> "true" .',
    ]
    for kw in kws:
        out.append(f'{s} <{DCT}subject> "{kw}" .')
    for a in authors:
        out.append(f"{s} <{DCT}creator> {author_iri(a)} .")
    # Coauthorship edges (both directions) among this paper's authors.
    for i in range(len(authors)):
        for j in range(i + 1, len(authors)):
            out.append(f"{author_iri(authors[i])} <{EX}coauthor> {author_iri(authors[j])} .")
            out.append(f"{author_iri(authors[j])} <{EX}coauthor> {author_iri(authors[i])} .")
    return out


def author_triples_for(doi):
    seed = h(doi)
    n_auth = 1 + seed % 3
    authors = list(dict.fromkeys([(seed >> (4 + j)) % n_authors for j in range(n_auth)]))
    out = []
    for a in authors:
        fn = FIRST[h(str(a), "f") % len(FIRST)]
        ln = LAST[h(str(a), "l") % len(LAST)]
        out.append(f"{author_iri(a)} <{RDF_TYPE}> <{FOAF}Person> .")
        out.append(f'{author_iri(a)} <{FOAF}name> "{fn} {ln}" .')
    return out


# --- emit ------------------------------------------------------------------
HEADER = ("# SYNTHETIC METADATA (demo). Citation edges, DOIs, years = REAL "
          "(OpenCitations).\n# Titles, authors, venues, disciplines, keywords, "
          "coauthorship = FABRICATED,\n# deterministically from a hash of each DOI. "
          "NOT real bibliographic data.\n")

# Re-emit the original citation lines + enrichment, sharded by year.
shards = {}
seen_author = set()
all_lines = [HEADER]
for doi, year in papers.items():
    block = [
        f"<https://doi.org/{doi}> <http://purl.org/spar/cito/cites> "
        f"<https://doi.org/10.1038/s41586-021-03819-2> .",
        f'<https://doi.org/{doi}> <{DCT}date> "{year}" .',
        f"<https://doi.org/{doi}> <{RDF_TYPE}> <http://purl.org/spar/fabio/JournalArticle> .",
    ]
    block += enrich_triples(doi)
    for at in author_triples_for(doi):
        a_iri = at.split()[0]
        if (a_iri, at) not in seen_author:
            seen_author.add((a_iri, at))
            block.append(at)
    text = "\n".join(block) + "\n"
    all_lines.append(text)
    shards.setdefault(year, []).append(text)

with open(os.path.join(OUT, "enriched-all.nt"), "w", encoding="utf-8") as f:
    f.writelines(all_lines)
for y, blocks in sorted(shards.items()):
    with open(os.path.join(OUT, f"enriched-{y}.nt"), "w", encoding="utf-8") as f:
        f.write(HEADER)
        f.writelines(blocks)
print(f"wrote enriched-all.nt + {len(shards)} year shards to {OUT}", file=sys.stderr)
