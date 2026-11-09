# Build a `.rete`, step by step

This tutorial builds one `.rete` file twice, once from Python and once with the
command line, and then looks at what is inside it and what the build cost. The
data is a synthetic knowledge graph of planetary systems. It is the graph from
the [JupyterLite notebook](jupyterlite-guide.md) "Anatomy of a .rete", scaled
by a size parameter, so you can run it in seconds and then make it a thousand
times larger.

Every command and every output below was run with rete **0.5.0**, which writes
format `0x06` and reads `0x05` and `0x06`. Outputs are trimmed with `…` where
they are long.

You need Docker. The CLI and the Python package both run in containers, so
nothing else is installed on your machine. If you already have a native `rete`
command or a Python environment, use them and skip the Docker wrappers.

```sh
mkdir rete-tutorial && cd rete-tutorial
alias rete='docker run --rm -i -v "$PWD:/data" -w /data ghcr.io/caviri/rete-cli:0.5.0'
alias py='docker run --rm -i -v "$PWD:/data" -w /data python:3.12-slim'
```

On Windows Git Bash, set `MSYS_NO_PATHCONV=1` first. In PowerShell, see the
[Quickstart](getting-started.md#getting-the-cli) for the wrapper function. The
other install routes are on the [install page](install.md).

## 1. Generate the graph

The generator is [`examples/gen_solar_kg.py`](https://github.com/caviri/rete/blob/main/examples/gen_solar_kg.py).
Save it next to your working directory. It has no dependencies and is
deterministic for a given `--systems` and `--seed`.

The graph has the notebook's ontology: `Planet`, `Moon` and (new here) `Star`
are subclasses of `CelestialBody`, and `orbits` has a domain and a range. The
data starts with the notebook's eight bodies in the namespace
`https://example.org/solar/`, followed by `--systems` generated systems, each
with a star, two to six planets and up to three moons per planet. Bodies have a
class, an English label, a radius in km and an `inSystem` link; planets and
moons have an orbital period, and planets say whether they have an atmosphere.
Nothing is typed `CelestialBody` directly.

```sh
py python gen_solar_kg.py --systems 100 -o solar.nt
```

```text
6103 triples
```

The generator reports the triples it wrote on stderr. The first lines are the
ontology; every body then has a few lines like these:

```sh
grep '/Mars> ' solar.nt | head -n 7
```

```text
<https://example.org/solar/Mars> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://example.org/solar/Planet> .
<https://example.org/solar/Mars> <http://www.w3.org/2000/01/rdf-schema#label> "Mars"@en .
<https://example.org/solar/Mars> <https://example.org/solar/radius_km> "3390"^^<http://www.w3.org/2001/XMLSchema#integer> .
<https://example.org/solar/Mars> <https://example.org/solar/inSystem> <https://example.org/solar/Sol> .
<https://example.org/solar/Mars> <https://example.org/solar/orbits> <https://example.org/solar/Sun> .
<https://example.org/solar/Mars> <https://example.org/solar/orbitalPeriodDays> "30446.39"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<https://example.org/solar/Mars> <https://example.org/solar/hasAtmosphere> "true"^^<http://www.w3.org/2001/XMLSchema#boolean> .
```

Each system adds about 60 triples, so `--systems` is the size knob:

| `--systems` | triples | N-Triples file |
|---:|---:|---:|
| 100 | 6,103 | 0.7 MB |
| 1,600 | 101,770 | 12 MB |
| 16,000 | 992,447 | 122 MB |
| 160,000 | 9,910,209 | 1.2 GB |
| 480,000 | 29,762,321 | 3.7 GB |

The rest of the tutorial uses 100 systems, and [section 7](#7-time-and-memory)
uses the larger ones.

## 2. Build with Python

The Python package is `rete-graph` on PyPI; you import it as `rete_graph`.
A `Builder` records what you ask for, and nothing is parsed until `run()`. The
[Python build tutorial](python-build-tutorial.md) covers every builder call.
Save this as `build_solar.py`:

```python
import rete_graph as rete

builder = (
    rete.Builder()
    .add_file("solar.nt")
    .card(
        title="Solar systems (synthetic)",
        description="A synthetic knowledge graph of planetary systems: the eight bodies of our own, then generated systems with a star, planets and moons. Made for the rete build tutorial.",
        license="CC0-1.0",
        version="1.0.0",
        created="2026-11-02",
        keywords=["synthetic", "tutorial", "astronomy"],
        example_queries=[
            "PREFIX s: <https://example.org/solar/> SELECT ?moon WHERE { ?moon s:orbits s:Mars }"
        ],
        extra={"pipeline": {"generator": "examples/gen_solar_kg.py", "systems": 100, "seed": 7}},
    )
    .derive_card()
    .pyramid(algo="types")
)
builder.run()
print(builder.stats)
builder.export("solar-py.rete")
```

`derive_card()` asks the Python builder for the same derived card profile the
CLI writes (section 4). `pyramid(algo="types")` groups the graph's communities
by `rdf:type` class. Run it:

```sh
py sh -c 'pip install -q --root-user-action=ignore --disable-pip-version-check rete-graph==0.5.0 && python build_solar.py'
```

```text
{'statements': 6103, 'defaultTriples': 6103, 'namedGraphs': 0, 'terms': 3880, 'pyramidLevels': 1}
```

`stats` reports 6,103 statements over 3,880 distinct terms. `export()` wrote
`solar-py.rete`. Now open it and ask questions. Save this as `query_solar.py`:

```python
import rete_graph as rete

g = rete.open("solar-py.rete")
print(g.quads, "triples,", g.terms, "terms")

PREFIXES = """
PREFIX s: <https://example.org/solar/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
"""

# 1. How many bodies of each kind?
rows = g.query(PREFIXES + """
    SELECT ?class (COUNT(?b) AS ?n) WHERE { ?b a ?class . ?class a rdfs:Class }
    GROUP BY ?class ORDER BY DESC(?n)""")
for r in rows:
    print(r["class"].value.rsplit("/", 1)[1], r["n"].value)

# 2. Which moons orbit Mars?
for r in g.query(PREFIXES + """
    SELECT ?moon ?name WHERE { ?moon s:orbits s:Mars ; rdfs:label ?name }
    ORDER BY ?name"""):
    print(r["moon"].value, r["name"].value)

# 3. Nothing is typed CelestialBody. With reason=True the query is rewritten
#    through the ontology (Planet, Moon and Star are subclasses of it).
q = PREFIXES + "SELECT (COUNT(DISTINCT ?b) AS ?n) WHERE { ?b a s:CelestialBody }"
print("asserted:", g.query(q)[0]["n"].value, "| with reasoning:", g.query(q, reason=True)[0]["n"].value)

# The file's own description.
print(g.card()["title"], "|", g.card()["extra"])
print(g.schema()["classes"][:2])
```

```sh
py sh -c 'pip install -q --root-user-action=ignore --disable-pip-version-check rete-graph==0.5.0 && python query_solar.py'
```

```text
6103 triples, 3880 terms
Moon 456
Planet 393
PlanetarySystem 101
Star 101
https://example.org/solar/Deimos Deimos
https://example.org/solar/Phobos Phobos
asserted: 0 | with reasoning: 950
Solar systems (synthetic) | {'pipeline': {'generator': 'examples/gen_solar_kg.py', 'seed': 7, 'systems': 100}}
[('https://example.org/solar/Moon', 456), ('https://example.org/solar/Planet', 393)]
```

The third query is the point of the ontology. Asked plainly, it finds no
`CelestialBody`, because none is asserted. With `reason=True` the engine
rewrites the query through the subclass axioms (OWL 2 QL) and finds all 950,
without adding a triple to the file. See [Reasoning](reasoning.md).

Python also exposes how the file was built and whether it is intact:

```python
import rete_graph as rete

image = open("solar-py.rete", "rb").read()
print("verify:", rete.verify(image))
rec = rete.build_record(image)
print(rec["front_end"], rec["engine"]["version"], rec["counts"])
print(rec["params"])
```

```text
verify: True
python 0.5.0 {'default_graph_triples': 6103, 'duplicates_dropped': 0, 'named_graph_quads': 0, 'named_graphs': 0, 'statements_read': 6103, 'statements_unique': 6103, 'terms': 3880}
{'codec': 'zstd', 'command': 'build_dataset', 'format': 'nt', 'label_index': True, 'named_graphs': 'quad', 'permutations': 6, 'pyramid_algo': 'types', 'write': {…}}
```

The record says `front_end` is `python`. Its `run` block has the wall time, the
phases, the peak RSS of the process, its memory limit and the CPUs, as the
CLI's record does; `add_file` names each input by its file name (`solar.nt`).

## 3. Build with the CLI

Same input, same card. This time the card comes from a JSON file of curated
fields. Save it as `solar-card.json`:

```json
{
  "title": "Solar systems (synthetic)",
  "description": "A synthetic knowledge graph of planetary systems: the eight bodies of our own, then generated systems with a star, planets and moons. Made for the rete build tutorial.",
  "license": "CC0-1.0",
  "version": "1.0.0",
  "created": "2026-11-02",
  "keywords": [
    "synthetic",
    "tutorial",
    "astronomy"
  ],
  "example_queries": [
    "PREFIX s: <https://example.org/solar/> SELECT ?moon WHERE { ?moon s:orbits s:Mars }"
  ],
  "extra": {
    "pipeline": {
      "generator": "examples/gen_solar_kg.py",
      "systems": 100,
      "seed": 7
    }
  }
}
```

```sh
rete build solar.nt -o solar.rete --card-file solar-card.json --pyramid-algo types --label-index
```

```text
embedded dataset card (29970 bytes of metadata)
measured 20 starter queries against the built file
embedded build info (3265 bytes in a 4943-byte section, outside the content hash)
wrote solar.rete: 6103 triples, 3880 terms, 1 pyramid level(s), 91898 bytes
```

`--card-file` makes the build write a [Dataset Card](dataset-cards.md) and a
build record. `--pyramid-algo types` is the same choice as in Python.
`--label-index` writes the label index described in
[section 9](#9-optional-flags). The build also ran 20 starter queries against
the file it had just written; the card keeps those that answer.

`rete info` prints a summary of the header: the format, the counts, the codecs,
then the front of the file and its sections (`--raw` adds the header's fields
as Rust `Debug` output, for a bug report):

```sh
rete info solar.rete | sed -n '/^format 0x06/,/^    pyramid/p'
```

```text
format 0x06 — 91898 bytes, content hash b38ff62dbf5262ec1e50e4ef364be23e
  flags        : none
  front        : root 456 B at 128, card region 24842 B (card 3382 B, record 4943 B), data at 26080
  counts       :
    quad_count: 6103
    term_count: 3880
    graph_count: 0
    build_info_len: 4943
  codecs       : dictionary zstd, blocks zstd, internal zstd
  pyramid      : 1 level(s)
  sections     :
    schema                 25426 + 479          digest 6d9ba70882efc7199926a4a943793ba9
    dict-leaves            25905 + 61           digest 541def2fa3e22fd17bb6033bcd55df0e
    index-leaves           25966 + 114          digest 1371d31af6b27495099b4685dc7011e5
    dict                   26080 + 9373         digest a6afc5d3be52796f9bd73205ccbefa73
    index                  35453 + 54943        digest 84fdc42618e0ed9da127b844e1500cbb
    labels                 90396 + 1114         digest 01e2e2dc987750d92ad5cac474dc2257
    pyramid                91510 + 380          digest 1b335d5add44a8da100358527e380800
```

`rete verify` re-reads every section and checks its digest:

```sh
rete verify solar.rete
```

```text
OK — content hash matches, and every section digest (format 0x06)
```

The Python file and the CLI file have the same content hash:

```sh
rete info solar-py.rete | grep '^format 0x06'
rete info solar.rete | grep '^format 0x06'
```

```text
format 0x06 — 83780 bytes, content hash b38ff62dbf5262ec1e50e4ef364be23e
format 0x06 — 91898 bytes, content hash b38ff62dbf5262ec1e50e4ef364be23e
```

The two files are not the same size. The hash covers the data and the card, not
the build record, and the CLI's record is larger because it also holds the wall
times and the measured cost of each starter query.

Query it from the command line. The second query asks the file for the
ontology's answer instead of the asserted one:

```sh
rete sparql solar.rete "PREFIX s: <https://example.org/solar/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> SELECT ?class (COUNT(?b) AS ?n) WHERE { ?b a ?class . ?class a rdfs:Class } GROUP BY ?class ORDER BY DESC(?n)"
rete sparql solar.rete "PREFIX s: <https://example.org/solar/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> SELECT ?moon ?name WHERE { ?moon s:orbits s:Mars ; rdfs:label ?name } ORDER BY ?name"
rete sparql solar.rete "PREFIX s: <https://example.org/solar/> SELECT (COUNT(DISTINCT ?b) AS ?n) WHERE { ?b a s:CelestialBody }"
rete sparql solar.rete "PREFIX s: <https://example.org/solar/> SELECT (COUNT(DISTINCT ?b) AS ?n) WHERE { ?b a s:CelestialBody }" --entail
```

Each command prints its rows to stdout (and `N solution(s)` to stderr):

```text
?class=<https://example.org/solar/Moon>  ?n="456"^^<http://www.w3.org/2001/XMLSchema#integer>
?class=<https://example.org/solar/Planet>  ?n="393"^^<http://www.w3.org/2001/XMLSchema#integer>
?class=<https://example.org/solar/PlanetarySystem>  ?n="101"^^<http://www.w3.org/2001/XMLSchema#integer>
?class=<https://example.org/solar/Star>  ?n="101"^^<http://www.w3.org/2001/XMLSchema#integer>
?moon=<https://example.org/solar/Deimos>  ?name="Deimos"@en
?moon=<https://example.org/solar/Phobos>  ?name="Phobos"@en
?n="0"^^<http://www.w3.org/2001/XMLSchema#integer>
?n="950"^^<http://www.w3.org/2001/XMLSchema#integer>
```

`rete search` looks a label up by prefix in the label index:

```sh
rete search solar.rete "System 4 planet 0"
```

```text
System 4 planet 0	<https://example.org/solar/S4_p0>
System 4 planet 0 moon 0	<https://example.org/solar/S4_p0_m0>
System 4 planet 0 moon 1	<https://example.org/solar/S4_p0_m1>
```

## 4. The card

The card is a JSON document inside the file. It describes the dataset to
someone who has not opened it. It sits at the front of the file, so a reader
can fetch it without touching the data. It is the first thing to look at in an
unknown graph. Print it:

```sh
rete card solar.rete | head -n 40
```

```text
Dataset Card
  title        : Solar systems (synthetic)
  description  : A synthetic knowledge graph of planetary systems: the eight bodies of our own, then generated systems with a star, planets and moons. Made for the rete …
  license      : CC0-1.0
  created      : 2026-11-02
  keywords     : astronomy, synthetic, tutorial
  version      : 1.0.0
  custom fields (1):
      pipeline = {"generator":"examples/gen_solar_kg.py","seed":7,"systems":100}
  triples      : 6103
  terms        : 3880
  checksum     : b38ff62dbf5262ec1e50e4ef364be23e  (blake3-16 content hash)
  vocabularies : 3
      http://www.w3.org/1999/02/22-rdf-syntax-ns#
      http://www.w3.org/2000/01/rdf-schema#
      https://example.org/solar/
  predicates (10):
          1056  <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>
          1051  <http://www.w3.org/2000/01/rdf-schema#label>
           950  <https://example.org/solar/inSystem>
           950  <https://example.org/solar/radius_km>
           849  <https://example.org/solar/orbitalPeriodDays>
           849  <https://example.org/solar/orbits>
           393  <https://example.org/solar/hasAtmosphere>
             3  <http://www.w3.org/2000/01/rdf-schema#subClassOf>
             1  <http://www.w3.org/2000/01/rdf-schema#domain>
             1  <http://www.w3.org/2000/01/rdf-schema#range>
  classes (5):
           456  <https://example.org/solar/Moon>
           393  <https://example.org/solar/Planet>
           101  <https://example.org/solar/PlanetarySystem>
           101  <https://example.org/solar/Star>
             5  <http://www.w3.org/2000/01/rdf-schema#Class>
  datatypes (4):
          1051  <http://www.w3.org/1999/02/22-rdf-syntax-ns#langString>
           950  <http://www.w3.org/2001/XMLSchema#integer>
           849  <http://www.w3.org/2001/XMLSchema#decimal>
           393  <http://www.w3.org/2001/XMLSchema#boolean>
  languages (2):
          2192  (untagged)
…
```

The fields fall into three groups; the
[Dataset Card reference](dataset-card-reference.md#whats-in-a-card) lists all of
them.

- **Curated**, from your card file: `title`, `description`, `license`,
  `version`, `created`, `keywords`, `example_queries`, and `extra`. The
  `extra` object holds your own fields. Here it records how the graph was made:
  `extra.pipeline` carries the generator, the number of systems and the seed.
- **Derived**, from the data: `triple_count`, `term_count`, the `predicates`
  and `classes` with their counts, `vocabularies`, `datatypes`, `languages`,
  `class_links`, the top hubs, and `signals` such as `label_predicate` and
  `base_iri`. The 20 starter `queries` are also derived, using this graph's own
  names.
- **Measured**: the `build` record, described below.

`rete card solar.rete --json` prints the same document as JSON. To see its
shape:

```sh
rete card solar.rete --json > card.json
py python -c "import json; c=json.load(open('card.json')); print(sorted(c)); print(c['extra']); print(c['signals']['base_iri'])"
```

```text
['build', 'class_links', 'classes', 'created', 'datatypes', 'description', 'example_queries', 'extra', 'format_version', 'in_hubs', 'keywords', 'languages', 'license', '…
{'pipeline': {'generator': 'examples/gen_solar_kg.py', 'seed': 7, 'systems': 100}}
https://example.org/solar/
{'default_graph_triples': 6103, 'duplicates_dropped': 0, 'named_graph_quads': 0, 'named_graphs': 0, 'statements_read': 6103, 'statements_unique': 6103, 'terms': 3880}
```

`signals.base_iri` is the namespace most subjects share. It is derived from the
data, and the starter queries use it to tell local IRIs from external ones.
When the derived value is wrong, for example on a graph with very many
namespaces, set `base_iri` in the card file. Python's `Builder.card()` has no
`base_iri` keyword: it would end up in `extra`, so set it from a card file.

To curate a card after the build, change the JSON and run `recard`. It
rewrites the card region of the file and leaves every data byte alone:

```sh
py python - <<'PY'
import json
c = json.load(open("solar-card.json"))
c["version"] = "1.0.1"
c["base_iri"] = "https://example.org/solar/"
c["description"] += " Systems are generated; only the first eight bodies are real."
json.dump(c, open("solar-card-v2.json", "w"), indent=2)
PY
cp solar.rete solar-curated.rete
rete recard solar-curated.rete --in-place --card-file solar-card-v2.json
rete verify solar-curated.rete
```

```text
solar-curated.rete (in place): 20 starter queries (0 dropped), content hash b38ff62dbf5262ec1e50e4ef364be23e → a847f866566d3c07c1a6721ce8d2588c, 30031 B card in the card region's free space, no data byte moved; 0 s
OK — content hash matches, and every section digest (format 0x06)
```

The last line of `recard`'s output is the summary above; it also prints one progress
line per starter query on stderr. The file is the same length, because the new
card and record fit in the card region's free space. `rete card solar-curated.rete --json`
now shows `version` `1.0.1`.

The card is inside the content hash, so editing it changes the file's identity
(`rete verify` still passes; the hash is a new one). See
[Update a published card](dataset-cards.md#maintaining-a-published-card).

### The build record

The `build` key of the card is the **build record**. It is not part of the
content hash, so the same input gives the same hash however fast the machine
was. It lists the flags that shaped the file, the engine version and commit,
each input by file name, size and BLAKE3, the statement counts, the wall time of
each phase, the peak memory, and the cost of each starter query:

```text
  build:
      built at    : 2026-11-02T13:26:52Z (started 2026-11-02T13:26:52Z)
      front end   : cli
      engine      : rete-cli 0.5.0, commit 7ec430d6d87c58a4, x86_64-unknown-linux-gnu (release); features: compression, parallel, rdf12-turtle, unicode-regex
      host        : linux x86_64
      command     : rete build --permutations 6 --named-graphs quad --pyramid-algo types --label-index --quoted-triple-syntax rdf-star --tile-kib 64 --dict-chunk-kib 64 --zstd-level 9 --card-file solar-card.json
      card file   : solar-card.json (639 B, blake3 06fed5bdc91190a2…)
      writer      : zstd, 64 KiB tiles, 64 KiB dictionary chunks, zstd level 9 (sized frames)
      card top-N  : 100
      inputs      : solar.nt (nt), 729,062 B, blake3 6a08f6713b3e1f56…
      statements  : 6,103 read, 6,103 unique (0 duplicate(s) dropped)
                    6,103 in the default graph, 0 in 0 named graph(s); 3,880 terms
      wall time   : 115 ms on 32 CPU(s)
      peak RAM    : 23.6 MiB of a 4.0 GiB limit
      phases      : parse+dictionary 10 ms · parse+encode 9 ms · card_derive 6 ms · pyramid 1 ms
                    labels+schema 6 ms · dictionary_section 0 ms · index_permutations 1 ms · serialize 2 ms
                    starter_queries 38 ms · write 38 ms
```

The phases are the "relative times" of a build. Section 7 uses them.

## 5. The schema

Every build writes a SCHEMA section, a few kilobytes that sit at the front of
the file. It holds:

- the **class hierarchy** from `rdfs:subClassOf`, with each class's depth;
- **rollups**: instance counts at each level of that hierarchy, so a level-0
  view counts all stars, planets and moons together as `CelestialBody`;
- the **lateral links** between classes (planets `orbits` stars);
- `query_stats`: for each predicate, how many subjects and objects it has and
  the largest fan-out in each direction;
- **characteristic sets**: the groups of predicates that entities actually
  carry together.

The query planner uses `query_stats` and the characteristic sets to choose a
join order without reading the index, and the explorer uses the hierarchy and
rollups to draw the graph at several zoom levels before it fetches any triples.
The format is in [SPEC §7.5](SPEC.md#75-the-schema-section-kind-9).

`rete stats` prints the planner statistics and the characteristic sets:

```sh
rete stats solar.rete
```

```text
solar.rete — 91898 bytes
  statements            : 6103 (all graphs)
  default-graph triples : 6103
  named graphs          : 0
  distinct terms        : 3880
  pyramid levels        : 1
  term id widths        : u32 (subjects, objects, predicates, graphs)
  tile synopsis         : yes (range readers prune tiles by a bound secondary)
  index permutations    : 6 (SPO/POS/OSP/SOP/PSO/OPS)
  compression           : zstd
  communities           : 6
  predicates (top 10):
        1056  <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>
        1051  <http://www.w3.org/2000/01/rdf-schema#label>
         950  <https://example.org/solar/inSystem>
         950  <https://example.org/solar/radius_km>
         849  <https://example.org/solar/orbitalPeriodDays>
         849  <https://example.org/solar/orbits>
         393  <https://example.org/solar/hasAtmosphere>
           3  <http://www.w3.org/2000/01/rdf-schema#subClassOf>
           1  <http://www.w3.org/2000/01/rdf-schema#domain>
           1  <http://www.w3.org/2000/01/rdf-schema#range>
  planner stats (query_stats, 10 predicates):
        1056  <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>  (1056 subj, 5 obj; ≤1/subj, ≤456/obj · functional)
        1051  <http://www.w3.org/2000/01/rdf-schema#label>  (1051 subj, 1051 obj; ≤1/subj, ≤1/obj · functional · inverse-functional)
         950  <https://example.org/solar/inSystem>  (950 subj, 101 obj; ≤1/subj, ≤19/obj · functional)
         950  <https://example.org/solar/radius_km>  (950 subj, 913 obj; ≤1/subj, ≤3/obj · functional)
         849  <https://example.org/solar/orbitalPeriodDays>  (849 subj, 846 obj; ≤1/subj, ≤2/obj · functional)
         849  <https://example.org/solar/orbits>  (849 subj, 367 obj; ≤1/subj, ≤6/obj · functional)
         393  <https://example.org/solar/hasAtmosphere>  (393 subj, 2 obj; ≤1/subj, ≤241/obj · functional)
           3  <http://www.w3.org/2000/01/rdf-schema#subClassOf>  (3 subj, 1 obj; ≤1/subj, ≤3/obj · functional)
           1  <http://www.w3.org/2000/01/rdf-schema#domain>  (1 subj, 1 obj; ≤1/subj, ≤1/obj · functional · inverse-functional)
           1  <http://www.w3.org/2000/01/rdf-schema#range>  (1 subj, 1 obj; ≤1/subj, ≤1/obj · functional · inverse-functional)
  entity shapes (characteristic sets, top 7):
         456 subj  {type, label, inSystem, orbitalPeriodDays, orbits, radius_km}
         393 subj  {type, label, hasAtmosphere, inSystem, orbitalPeriodDays, orbits, radius_km}
         101 subj  {type, label}
         101 subj  {type, label, inSystem, radius_km}
           3 subj  {type, subClassOf}
           2 subj  {type}
           1 subj  {domain, range}
  label index: 1051 labels, 1169 bytes (LABEL_INDEX — `rete search solar.rete <prefix>`)
```

Read the entity shapes as a summary of the data: 456 subjects have the six
predicates of a moon, 393 have the seven of a planet. In `query_stats`, every
predicate here is `functional` (at most one value per subject), and `label` is
also `inverse-functional` (each label names one thing). The planner uses facts
like these.

`rete schema` prints the class-to-class relations, and `rete summary --level 0`
the most abstract level of the hierarchy:

```sh
rete schema solar.rete | head -12
rete summary solar.rete --level 0 | head -8
```

```text
classes (5 types):
       456  <https://example.org/solar/Moon>
       393  <https://example.org/solar/Planet>
       101  <https://example.org/solar/PlanetarySystem>
       101  <https://example.org/solar/Star>
         5  <http://www.w3.org/2000/01/rdf-schema#Class>
relations:
  (untyped) --<http://www.w3.org/2000/01/rdf-schema#domain>--> <http://www.w3.org/2000/01/rdf-schema#Class>  ×1
  (untyped) --<http://www.w3.org/2000/01/rdf-schema#range>--> <http://www.w3.org/2000/01/rdf-schema#Class>  ×1
  <http://www.w3.org/2000/01/rdf-schema#Class> --<http://www.w3.org/2000/01/rdf-schema#subClassOf>--> <http://www.w3.org/2000/01/rdf-schema#Class>  ×3
  <https://example.org/solar/Moon> --<http://www.w3.org/2000/01/rdf-schema#label>--> (literal)  ×456
  <https://example.org/solar/Moon> --<https://example.org/solar/inSystem>--> <https://example.org/solar/PlanetarySystem>  ×456
…
```

```text
schema pyramid level 0 — depth 0 (round 0), 3 class(es):
         950  <https://example.org/solar/CelestialBody>
         101  <https://example.org/solar/PlanetarySystem>
           5  <http://www.w3.org/2000/01/rdf-schema#Class>
  relations at this level (9):
         950  <https://example.org/solar/CelestialBody> --<http://www.w3.org/2000/01/rdf-schema#label>-> (literal)
         950  <https://example.org/solar/CelestialBody> --<https://example.org/solar/inSystem>-> <https://example.org/solar/PlanetarySystem>
         950  <https://example.org/solar/CelestialBody> --<https://example.org/solar/radius_km>-> (literal)
…
```

At level 0 the 950 bodies are one `CelestialBody`, the sum of 101 stars, 393
planets and 456 moons. `--no-schema` leaves the section out. `--no-pyramid`
does not, and neither does the memory-bounded build.

## 6. What happens during a build

`rete build` turns text into a file in a few steps. The phase names are the
ones in the build record.

1. **Parse.** The input is read as N-Triples, N-Quads, Turtle, TriG, RDF/XML or
   Jelly, optionally gzipped. For N-Triples, N-Quads and Jelly files the in-RAM build reads
   the input twice: pass 1 (`parse+dictionary`) collects the distinct terms,
   pass 2 (`parse+encode`) reads it again and replaces every term by its id,
   so the parsed text is never held in memory. Turtle, TriG and stdin take a
   different path that parses once and holds the statements.
2. **Dictionary.** Each distinct IRI, literal and blank node is stored once. The
   terms are sorted, split by role into four sections (used as both subject
   and object, subject only, object only, predicates), and front-coded: each
   term stores the length it shares with the previous one plus the rest. Ids are
   assigned in that order, so an id tells you the term's role. A triple is now
   three integers. Duplicate statements are dropped here; the record counts them.
3. **Indexes.** The id triples are sorted six ways (SPO, POS, OSP, SOP, PSO,
   OPS), so every triple pattern, with whichever positions it binds, is a
   contiguous range of one index. Each index is cut into tiles of about 64 KiB
   and each tile is compressed on its own with zstd. `--permutations 3` keeps
   only the first three.
4. **Optional sections.** The pyramid (communities of the graph), the label
   index and the text index are built from the same ids. The SCHEMA section is
   always built.
5. **Card.** The profile is derived, the starter queries are run against the
   finished file, and the card and the build record are written.
6. **Write.** The data is written at its final offset and hashed as it goes,
   then the front is written at offset 0, then the footer.

The layout that results (format `0x06`, [SPEC §4](SPEC.md#4-file-layout)):

```text
offset 0     +-------------------------------------------------+
             | HEADER, 128 bytes                               |
             |   counts, codecs, presence bits, content hash,  |
             |   where the card and the root are               |
128          +-------------------------------------------------+
             | ROOT BLOCK                                      |
             |   section table: kind, offset, length, digest   |
             +-------------------------------------------------+
             | CARD REGION                                     |
             |   card | build record | free space for recard   |
front_base   +-------------------------------------------------+
             | FRONT SECTIONS                                  |
             |   schema (query_stats, characteristic sets)     |
             |   leaf directories: dictionary, indexes, labels |
data_offset  +-------------------------------------------------+
             | DATA SECTIONS, one run per section              |
             |   dictionary chunks | index tiles | label blocks|
             |   text postings (if built) | pyramid (if built) |
             +-------------------------------------------------+
file_len-8   | FOOTER  06 00 00 00 "RETE"                      |
             +-------------------------------------------------+
```

A reader's first request is bytes 0 to 65,535 ([SPEC §4.6](SPEC.md#46-opening-a-file)).
That one response holds the header, the root, the card, the planner statistics
and, on a small file, every directory. The file length is in the header, so the
client sends no `HEAD`. On a large file the directories do not all fit, and the
query fetches the ones it needs as small ranges. It then fetches only the
dictionary chunks and index tiles it touches, each a range request of about
64 KiB. Because every index is sorted, the tiles for one subject, or for one
predicate, sit next to each other, and a scan is one contiguous read. Section 8
shows the result on a 106 MB file: a lookup reads about 3% of it.

Each section has a BLAKE3 digest in the root, and the content hash covers all
of them and the card, which is what `rete verify` checks. The record is outside
the hash. Older `0x05` files keep the same payload blocks but put the card and
the directories elsewhere; `rete convert` rewrites one as `0x06`.

## 7. Time and memory

This section measures the build at several sizes of the same graph. The
figures are measurements; the one extrapolation is marked.

**How it was measured.** The generator wrote graphs of 101,770, 992,447,
9,910,209 and 29,762,321 triples (`--systems` 1,600, 16,000, 160,000 and
480,000). Each build ran in its own container started with `--memory 4g`, one at
a time, using the 0.5.0 release binary. The wall time is the process's. The peak
is the largest resident set size of the build process (`getrusage`, the same
figure as `/usr/bin/time -v`), and the spill is the size of the `--tmp-dir`,
polled every 0.25 s. The container's cgroup peak was 15 to 30 MiB above the
resident-set figure in every run and is not shown. The host was a 32-thread
Windows machine running Docker Desktop, with a separate 16 GB build container
running on it the whole time, so take the absolute times as indicative and the
ratios as the result.

Every run used `--no-pyramid --label-index --card-file solar-card.json`, because
the memory-bounded build writes no pyramid. The build of section 3, with
`--pyramid-algo types`, cost the same to within 5% in time and memory at 1 and
10 million triples (65.6 s and 1,093 MB against 63.0 s and 1,098 MB at 10
million).

You can read the same kind of figures from any file the CLI built. `rete card`
prints the build record's wall time, peak memory and per-phase times:

```sh
rete card solar.rete | sed -n '/^      wall time/,/starter_queries/p'
```

### Results

Wall time, peak resident memory, spill and output size, and the same numbers per
million triples. `costs` means the build also measured the starter queries, which
is the default when a card is written. `no costs` is `--no-card-costs`.
`bounded 512` is `--memory-budget-mb 512`.

| graph | build | wall s | s per M triples | peak MB | MB per M triples | spill MB | file MB | bytes per triple |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 0.10 M | CLI in RAM, costs | 1.9 | 18.6 | 69 | 673 | | 1.14 | 11.2 |
| 0.10 M | CLI in RAM, no costs | 0.57 | 5.6 | 63 | 620 | | 1.13 | 11.1 |
| 0.10 M | Python | 0.36 | 3.5 | 112 | 1,096 | | 1.13 | 11.1 |
| 0.99 M | CLI in RAM, costs | 19.2 | 19.4 | 201 | 203 | | 10.60 | 10.7 |
| 0.99 M | CLI in RAM, no costs | 5.0 | 5.0 | 171 | 172 | | 10.60 | 10.7 |
| 0.99 M | CLI bounded 512, costs | 18.1 | 18.2 | 330 | 332 | 10 | 10.60 | 10.7 |
| 0.99 M | CLI bounded 512, no costs | 4.5 | 4.5 | 327 | 329 | 10 | 10.60 | 10.7 |
| 0.99 M | CLI bounded 128, costs | 18.0 | 18.2 | 215 | 216 | 11 | 10.60 | 10.7 |
| 0.99 M | Python | 3.6 | 3.7 | 627 | 631 | | 10.60 | 10.7 |
| 9.9 M | CLI in RAM, costs | 63.0 | 6.4 | 1,098 | 111 | | 105.9 | 10.7 |
| 9.9 M | CLI in RAM, no costs | 53.7 | 5.4 | 1,092 | 110 | | 105.9 | 10.7 |
| 9.9 M | CLI bounded 512, costs | 53.1 | 5.4 | 645 | 65 | 114 | 105.9 | 10.7 |
| 9.9 M | CLI bounded 512, no costs | 43.4 | 4.4 | 434 | 44 | 114 | 105.9 | 10.7 |
| 9.9 M | CLI bounded 1500, costs | 51.8 | 5.2 | 1,086 | 110 | 108 | 105.9 | 10.7 |
| 29.8 M | CLI in RAM, costs | 175.4 | 5.9 | 3,096 | 104 | | 322.8 | 10.8 |
| 29.8 M | CLI bounded 512, costs | 138.5 | 4.7 | 1,946 | 65 | 363 | 322.8 | 10.8 |
| 29.8 M | CLI bounded 512, no costs | 134.6 | 4.5 | 473 | 16 | 371 | 322.8 | 10.8 |

MB is 2^20 bytes. The two Python rows are
`Builder.add_file(...).derive_card().pyramid(algo="types")` in a 4 GiB container;
the figure is the whole interpreter's peak, input text and file image included.
File sizes differ by a few kilobytes between builds because the build record
differs.

### What the numbers say

- **A build is linear in the number of triples.** Without the starter-query
  measurement it took 4.4 to 5.4 seconds per million triples on this machine,
  from 1 million to 30 million (the 30 million figure is the bounded build). The
  0.10 M row looks slower per triple because process start is a larger share of
  half a second. Peak memory in RAM is about 110 MB per million triples, and the
  file takes 10.7 bytes per triple with a label index. At 29.8 million triples
  the in-RAM build peaked at 3.1 GB in a 4 GB container. Extending the same line,
  it would stop fitting at about 35 to 40 million triples. That figure is an
  extrapolation, not a measurement.
- **Most of the time is parsing.** The build record gives the phases. At
  9.9 million triples in RAM without costs, in seconds: parse pass 1
  (`parse+dictionary`) 15.1, parse pass 2 (`parse+encode`) 19.5, `card_derive`
  15.4, `labels+schema` 2.0, `index_permutations` 0.6, `dictionary_section`
  0.06, `serialize` 0.4, `write` 0.6. The six sorted indexes, which give the file
  its shape, are about 1% of the time.
- **Measuring the starter queries can dominate.** With a card, the build runs the
  20 starter queries against the finished file to record what each costs. At
  0.99 million triples that took 14.3 s of a 19.2 s build (`starter_queries`
  7.1 s, `write` 7.2 s; the [CLI reference](cli.md#building) says the costs are
  measured again on the file as written). At 9.9 million it took 10.6 s. The
  queries are bounded, so the cost stops growing with the graph, but on a graph
  of a million triples it makes the build four times longer. `--no-card-costs`
  skips it, and the card then has no `query_costs`. Keep the measurement for a
  file you publish; skip it for a scratch build.
- **Python is fast and memory-hungry.** The builder takes the whole input as text
  and returns the whole file as bytes, so it holds both: about 630 MB per million
  triples, against 170 to 200 for the CLI. It is faster per triple here because
  it measures no starter queries and records no phases. Use it up to a few
  million triples, then use the CLI.

### The memory-bounded build

`--memory-budget-mb N` builds the same file with less memory. It reads the input
once, cuts it into chunks sized from the budget, writes each chunk's sorted terms
and id triples to `--tmp-dir` (zstd-compressed), merges the chunks into one
dictionary, remaps the chunks to global ids, and builds each index with an
external sort. The sections move from the spill into the output one by one and
are deleted as they are copied. The [CLI reference](cli.md#building) has the
details. At any moment it holds a chunk or a merge buffer, not the graph.

What the measurements show:

- **The file is the same.** The content hash of the 9.9 M file is
  `1c0f352778aa73111f1fff5667267cc5` for the in-RAM build and for the bounded
  builds at 256, 512 and 1500 MiB, with and without costs. At 29.8 M it is
  `8c828261561ec0f0db035834866ead95` for both builds. Only the build record
  differs.
- **It is not slower here.** At 9.9 M it took 53 s against 63 s in RAM, and at
  29.8 M 138 s against 175 s. Its card derivation streams the sorted sections
  and is much quicker (1.4 s against 15 s at 9.9 M).
- **It needs disk.** The spill peaked at 11 to 12 bytes per triple, about the
  size of the output: 114 MB at 9.9 M and 363 MB at 29.8 M. Put `--tmp-dir` on a
  disk with that much room.
- **Below a few million triples it saves nothing.** At 0.99 M a budget of 512 used
  330 MB, more than the 201 MB of the in-RAM build, because the merge has a fixed
  floor. A budget of 128 brought it down to 215 MB.
- **With a card, the budget does not cover the starter-query measurement.** At
  29.8 M under a budget of 512 the build finished its own work inside 473 MB
  (`--no-card-costs`). With costs the process peaked at 1,946 MB, because the
  measurement reads the finished file with its own cache. The build record does
  not include that step: it reports 493 MB for that run. For a hard memory limit,
  add `--no-card-costs`.

Use the bounded build when the in-RAM peak (about 110 MB per million triples,
plus a margin) would not fit in memory, or when the graph is tens of millions
of triples or more. It writes no pyramid, so a file that needs the
community pyramid has to be built in RAM or re-pyramided afterwards with
`rete repyramid`.

`rete estimate` projects the size, time and spill before you start. Give it the
flags you will build with. It reads the input, builds a sample of it in memory
with the same writer, and writes nothing. For the 9.9 M file, with
`--memory-budget-mb 512 --no-pyramid --label-index --card`, it says 92.9 to
126.7 MB, 40 to 90 s and a spill of 99 to 165 MB. The build took 50 to 53 s with
the card's starter queries, wrote 105.9 MB and spilled 114 MiB (120 MB). In
0.5.5 and earlier the same command said 47.5 to 71.3 MB, 20 to 34 s and 2.1 GB: it
modelled neither the label index nor the card, and took the spill from the size
of the input. The [CLI reference](cli.md#building) has the method and the error at
the other sizes.

## 8. Use the file over HTTP

A `.rete` file needs no database server. Any host that answers `Range` requests
with `206 Partial Content` can serve it, and the client reads only the bytes a
query needs. [Hosting your .rete](hosting.md) lists what a host must support and
has recipes for R2, S3, Zenodo and GitHub Pages.

The 91 KB file from section 3 is too small to show this: a remote file of at
most 8 MiB is read whole after the first request. So this section uses the
9.9 million triple graph from section 7, built with the same flags as before
(106 MB):

```sh
py python gen_solar_kg.py --systems 160000 -o solar-big.nt
rete build solar-big.nt -o solar-big.rete --card-file solar-card.json --pyramid-algo types --label-index
```

To try it without a hosting account, serve the working directory with the
repository's range-aware dev server,
[`scripts/range_server.py`](https://github.com/caviri/rete/blob/main/scripts/range_server.py)
(Python's own `http.server` ignores `Range`). Here it runs in a container:

```sh
docker run -d --name tut-range -p 8000:8000 -v "$PWD:/srv:ro" -w /srv python:3.12-slim python range_server.py 8000 /srv
```

(Copy `range_server.py` into the directory first. When the CLI itself runs in
Docker, as here, the host is `host.docker.internal`; a native `rete` uses
`127.0.0.1`.) Check that ranges work with a `GET`. The check in
[Hosting](hosting.md#the-ten-second-check) uses `HEAD`, which this dev server
answers with a plain 200:

```sh
curl -s -r 0-1023 -D - -o /dev/null http://127.0.0.1:8000/solar-big.rete
```

```text
HTTP/1.0 206 Partial Content
Server: SimpleHTTP/0.6 Python/3.12.16
Date: …
Content-Type: application/octet-stream
Content-Range: bytes 0-1023/105891840
Content-Length: 1024
Accept-Ranges: bytes
```

Read the card. `card-url` reads the first 128 KiB (the header, the root, the card region and
the statistics) in one request, and never the index:

```sh
rete card-url http://host.docker.internal:8000/solar-big.rete | head -n 12
```

```text
fetched 131072 of 105891840 bytes in 1 range request(s) — index NOT fetched
Dataset Card
  title        : Solar systems (synthetic)
  description  : A synthetic knowledge graph of planetary systems: the eight bodies of our own, then generated systems with a star, planets and moons. Made for the rete build tutorial.
  license      : CC0-1.0
  created      : 2026-11-02
  keywords     : astronomy, synthetic, tutorial
  version      : 1.0.0
  custom fields (1):
      pipeline = {"generator":"examples/gen_solar_kg.py","seed":7,"systems":100}
  triples      : 9910209
  terms        : 4284013
  checksum     : c848d355aeccfe17886a375823302169  (blake3-16 content hash)
…
```

The card still says `pipeline.systems` is 100 because the same card file was
reused; change it when you reuse a card.

Run queries with `sparql-url`. It reports what it fetched:

```sh
rete sparql-url http://host.docker.internal:8000/solar-big.rete "PREFIX s: <https://example.org/solar/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> SELECT ?moon ?name WHERE { ?moon s:orbits s:Mars ; rdfs:label ?name } ORDER BY ?name"
rete sparql-url http://host.docker.internal:8000/solar-big.rete "PREFIX s: <https://example.org/solar/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> SELECT ?class (COUNT(?b) AS ?n) WHERE { ?b a ?class . ?class a rdfs:Class } GROUP BY ?class ORDER BY DESC(?n)"
```

```text
2 solution(s)
(fetched 3407872 bytes in 11 range request(s); file is 105891840 bytes)
?moon=<https://example.org/solar/Deimos>  ?name="Deimos"@en
?moon=<https://example.org/solar/Phobos>  ?name="Phobos"@en

4 solution(s)
(fetched 1310720 bytes in 9 range request(s); file is 105891840 bytes)
?class=<https://example.org/solar/Moon>  ?n="745402"^^<http://www.w3.org/2001/XMLSchema#integer>
?class=<https://example.org/solar/Planet>  ?n="639683"^^<http://www.w3.org/2001/XMLSchema#integer>
?class=<https://example.org/solar/PlanetarySystem>  ?n="160001"^^<http://www.w3.org/2001/XMLSchema#integer>
?class=<https://example.org/solar/Star>  ?n="160001"^^<http://www.w3.org/2001/XMLSchema#integer>
```

The first query read 3.3 MB of the 106 MB file in 11 requests, about 3%. The
count of every class has to touch the type index, and it read 1.3 MB in 9
requests. `rete cost` previews the same thing before you run a query, and
works on a local file or a URL:

```sh
rete cost solar-big.rete "PREFIX s: <https://example.org/solar/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> SELECT ?moon ?name WHERE { ?moon s:orbits s:Mars ; rdfs:label ?name }" | sed -n '5,13p'
```

```text
  current engine access: lazy-tiles
  query predicates: <http://www.w3.org/2000/01/rdf-schema#label>, <https://example.org/solar/orbits>
  summary exact answer: unavailable (query shape is not exactly answerable from summary predicate totals)
  summary overview: 5151904 bytes in 4 range request(s) · skips index
  routed pattern open: unavailable (query is not a single default-graph triple pattern)
  lazy query open: 131072 bytes in 1 range request(s) · reads index
  full query open: 105891840 bytes in 2 range request(s) · reads index
  index section: offset 5160233 · len 99424196
  note: SPARQL evaluation opens lazily (tile directories only) and range-fetches index tiles as the query touches them; the full query-open figure is the eager upper bou…
```

`rete federate` runs one query over several sources, local paths and URLs mixed.
Here, the remote file and the 6,103 triple local one:

```sh
rete federate http://host.docker.internal:8000/solar-big.rete solar.rete --query "PREFIX s: <https://example.org/solar/> SELECT ?moon WHERE { ?moon s:orbits s:Mars }"
```

```text
  http://host.docker.internal:8000/solar-big.rete: 2 row(s) in 44.2ms (fetched 655360 bytes in 4 range request(s))
  solar.rete: 0 row(s) in 2.6ms
2 solution(s)
federated 2 source(s): 2 queried, 0 pruned (routing on); 2 merged result(s)
?moon=<https://example.org/solar/Deimos>
?moon=<https://example.org/solar/Phobos>
```

It merges rows by term, so each moon appears once. See
[Federated queries](federation.md) for what it does and does not join.

Stop the server with `docker rm -f tut-range`.

For a browser, the host must also send CORS headers, which this dev server does
not. Once the file is on a real host, paste its URL into the
[playground](playground-guide.md) under **+ Add source** to query it from a page,
with the same range requests.

## 9. Optional flags

These change what the file contains. Each is off by default. The
[CLI reference](cli.md#building) has the full list.

- **`--label-index`** writes a label index: the display label of every labelled
  subject, sorted, so `rete search <file> <prefix>` and autocomplete read a few
  small ranges and no dictionary. Section 3 used it. Without the flag the file
  has no label index.
- **`--text-index`** writes a word index over the literals, for
  `rete search --contains <word>` and fast SPARQL `CONTAINS`. It costs space: on
  the 100-system file it adds 15.8 KB to a 92 KB file (107,922 bytes in total).
  [Full-text index](cli.md#full-text-index---text-index-and-its-scope) has its
  scope flags.

  ```sh
  rete build solar.nt -o solar-text.rete --card-file solar-card.json --pyramid-algo types --label-index --text-index
  rete search solar-text.rete --contains planet --limit 3
  ```

  ```text
  <https://example.org/solar/S100_p0>
  <https://example.org/solar/S100_p1>
  <https://example.org/solar/S100_p2>
  ```

- **`--named-graphs`** chooses how the graphs of an N-Quads or TriG input are
  stored. Format `0x06` has one layout, `quad`, so the flag is only there to
  refuse the old `per-graph` one. This tutorial's graph has no named graphs.
  See [Named graphs in the quad index](compatibility.md#named-graphs-in-the-quad-index)
  and [SPEC §6.5](SPEC.md#65-quad-index-quad_index-section-optional).

## Where next

- [Dataset Cards](dataset-cards.md) and the [card reference](dataset-card-reference.md).
- [Format specification](SPEC.md) for the byte layout.
- [CLI reference](cli.md) for every `rete build` flag.
- [Python build tutorial](python-build-tutorial.md) for the builder's other
  options, and [JupyterLite](jupyterlite-guide.md) to run the notebook in a tab.
- [Hosting your .rete](hosting.md) and the [Playground](playground-guide.md).
