# miRBase 22.1

The reference catalogue of published microRNA sequences and annotation, as raw
files, Parquet, and one range-queryable `.rete`.

- **Source**: <https://www.mirbase.org/> (University of Manchester)
- **Release**: 22.1 — the MySQL dump is stamped `Dump completed on 2018-05-23`,
  the GFF3 files `##date 2018-3-5`; the database is `mirna_22b`.
- **Downloaded**: 2026-07-27
- **Licence**: **public domain**. Verbatim from `raw/LICENSE`:
  > miRBase is in the public domain. It is not copyrighted. You may freely
  > modify, redistribute, or use it for any purpose.
- **Citation**: Kozomara A, Birgaoanu M, Griffiths-Jones S. *miRBase: from
  microRNA sequences to function.* Nucleic Acids Res. 2019;47(D1):D155–D162.

## What is here

```
data/mirbase/
  raw/                     146 MB, as downloaded (gitignored)
    hairpin.fa             38,589 stem-loop precursor sequences
    mature.fa              48,885 mature miRNA sequences
    hairpin_high_conf.fa    3,320 high-confidence stem-loops
    mature_high_conf.fa     5,563 high-confidence matures
    miRNA.dat              EMBL flat file — the richest single distribution
    miRNA_high_conf.dat    EMBL, high-confidence subset
    miRNA.str              secondary structures (308,712 lines)
    miRNA.csv              flat spreadsheet export (UTF-8 BOM)
    miRNA.dead             withdrawn entries
    miRNA.diff             changes vs the previous release
    README, LICENSE
    genomes/               31 GFF3 files — assembly-stamped genome coordinates
    database_files/        the 17-table relational dump + tables.sql
    _wrapped_html/         the literal server responses (see Gotchas)
  parquet/                 14 MB — all 31 tables as columnar files
  schema.json              JSON Schema draft 2020-12 for those 31 tables
  croissant.jsonld         MLCommons Croissant 1.0 (with per-file sha256)
  mirbase.ttl              the OWL 2 QL ontology — source of truth
  mirbase-ontology.nt      generated from mirbase.ttl for the build stream
  mirbase.rete             39 MB — 2,739,316 triples, the queryable graph
  scripts/                 everything below, all Dockerised
```

## Reproduce

```bash
bash data/mirbase/scripts/download.sh              # raw/ (+ un-wrap, + checksums)
bash data/mirbase/scripts/py.sh profile_raw.py     # schema/statistics report

bash data/mirbase/scripts/py.sh tables_to_parquet.py   # 17 relational tables
bash data/mirbase/scripts/py.sh fa_to_parquet.py       # 4 FASTA files
bash data/mirbase/scripts/py.sh gff3_to_parquet.py     # 31 GFF3 files
bash data/mirbase/scripts/py.sh embl_to_parquet.py     # 2 EMBL files

bash data/mirbase/scripts/py.sh emit_metadata.py        # schema.json + croissant.jsonld
bash data/mirbase/scripts/build_rete.sh                # mirbase.ttl + data -> mirbase.rete

bash data/mirbase/scripts/roundtrip_test.sh            # raw -> parquet -> raw
bash data/mirbase/scripts/roundtrip_rete_test.sh       # .rete -> parquet -> raw

# ontology checks
docker run --rm -v "$PWD:/w" -w //w mirbase-py:latest \
  python .claude/skills/data-ontology/scripts/validate_ontology.py data/mirbase/mirbase.ttl
./target/release/rete reason data/mirbase/mirbase.rete   # coherence
```

Everything runs in Docker; only `curl` runs on the host. `py.sh` builds a tiny
`mirbase-py` image (python + pyarrow) on first use.

## Round-trip guarantee

Both directions are verified byte-for-byte against the files miRBase shipped,
not merely "parsed OK".

| chain | files | result |
|---|---|---|
| `raw -> Parquet -> raw` | 4 FASTA + 2 EMBL + 31 GFF3 | **37/37 byte-identical** |
| `.rete -> Parquet -> raw` | hairpin.fa, mature.fa, 31 GFF3 | **33/33 byte-identical** |

`embl_to_parquet.py` verifies itself as it writes: it re-serialises every record
through `parquet_to_embl.py` and, for anything that would not reproduce exactly,
falls back to storing the original block. **That fallback is currently unused —
all 38,589 + 3,320 records reconstruct from structure alone (100%).**

EMBL is not regenerated from the `.rete`: the graph deliberately models miRNA
*semantics*, not EMBL line-wrapping. The Parquet layer is the lossless
interchange format; the `.rete` is the queryable view. FASTA and GFF3 *do*
regenerate byte-exactly from the `.rete` because record order is recoverable
(hairpins by accession; matures by parent stem-loop then offset; GFF3 ordinals
are encoded in the region IRIs).

## Metadata artifacts

| file | standard | what it answers |
|---|---|---|
| `schema.json` | JSON Schema draft 2020-12 | the columns & types of all 31 Parquet tables |
| `croissant.jsonld` | MLCommons Croissant 1.0 | the distribution: file objects (with sha256) + record sets |
| `mirbase.ttl` | OWL 2 (QL-safe) | what it all *means* — classes, relations, join keys |

Both machine-readable artifacts validate (`jsonschema` draft-2020-12 check;
`mlcroissant` reports 31 recordSets). Regenerate with:

```bash
bash data/mirbase/scripts/py.sh emit_metadata.py
```

## The ontology

`data/mirbase/mirbase.ttl` is the source of truth (hand-authored, commented);
`make_ontology.py` converts it to `mirbase-ontology.nt` for the build stream.
11 classes, 13 object properties, 21 datatype properties, 0 broken
cross-references, and the graph is **coherent** under `rete reason`.

miRBase publishes no ontology, so this one is deliberately mostly *alignment* —
every external term below was verified to exist before being mapped:

| concept | modelled as |
|---|---|
| stem-loop (MI…) | `mb:StemLoop` ⊑ `mb:Feature` ⊑ SO:0000110, and ⊑ **SO:0001244** `pre_miRNA` |
| mature (MIMAT…) | `mb:MatureMiRNA` ⊑ `mb:Feature`, and ⊑ **SO:0000276** `miRNA` |
| genome coordinates | `faldo:Region` / `ExactPosition` / strand classes; `mb:location` ⊑ **`faldo:location`** |
| chromosome / contig | `mb:ReferenceSequence` — the target of `faldo:reference` |
| species | **NCBITaxon IRIs**; `mb:organism` ⊑ **`obo:RO_0002162`** (*in taxon*); `mb:Species` ⊑ `schema:Taxon` |
| literature | `fabio:JournalArticle` at `pubmed.ncbi.nlm.nih.gov/<pmid>`, linked by `dcterms:isReferencedBy` |
| families | `mb:Family` ⊑ `skos:Concept` |
| cross-references | `rdfs:seeAlso` to RNAcentral, Rfam, EntrezGene, HGNC, MGI |
| labels / ids | `rdfs:label`, `skos:prefLabel`/`altLabel`, `dcterms:identifier` |

Because the alignment is real, SO queries work through the hierarchy:

```bash
rete sparql mirbase.rete --entail \
  'PREFIX obo: <http://purl.obolibrary.org/obo/>
   SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s a obo:SO_0000110 }'
# 88,402 — every stem-loop and mature, reached via mb:Feature ⊑ SO:0000110.
# NB: use COUNT(DISTINCT ?s); the QL rewrite is a UNION and repeats solutions.
```

Counts in the built file: 39,233 stem-loops · 49,168 matures · 66,217 FALDO
regions · 7,865 reference sequences · 53,320 placements · 21,196 transcript
contexts · 1,983 families · 914 articles · 898 dead entries · 286 species ·
146 assemblies.

### Three modelling decisions worth knowing

**`mb:Placement` — mature offsets are reified.** A mature can sit on several
stem-loops at **different** offsets (`MIMAT0007005` is 96–117 on `oan-mir-153-1`
but 39–60 on `oan-mir-153-2`). Hanging `mb:matureFrom` off the mature alone
recorded two contradictory values. Offsets therefore live on a `mb:Placement`
joining one mature to one stem-loop — and are *also* put on the mature directly
only when it has exactly one parent.

**`faldo:reference` needs a resource, not a string.** It is an
`owl:ObjectProperty` pointing at the contig/chromosome. An earlier draft gave it
a bare `"chr17"` literal — a range violation. Chromosomes are now
`mb:ReferenceSequence` entities, minted per (assembly-or-species, name) since
`chr1` and `supercont1.1` recur across genomes. This also makes "everything on
chr17 of GRCh38" a first-class query.

**Polymorphic properties get no domain.** `mb:assembly` is used on regions,
reference sequences *and* species; declaring any one as its `rdfs:domain` would
make a reasoner infer the others are that class. Similarly the GFF3 per-copy
parent is `mb:parentStemLoop` (domain `faldo:Region`) rather than reusing
`mb:derivesFrom` (domain `mb:MatureMiRNA`) — otherwise every region would be
inferred to be a mature miRNA. No `owl:AllDisjointClasses` is asserted at all:
`mb:DeadEntry` and `mb:StemLoop` legitimately share accession IRIs.

### Genome coordinates come from two sources, and both are kept

They cover different things, so each region records which one it came from via
`dcterms:source`:

- `"gff3"` — the 31 curated files. Assembly-stamped (`GRCh38`, `GRCm38`, …) and
  covering **both** stem-loops and matures. 31,015 features.
- `"chromosome_build"` — the `mirna_chromosome_build` table. **153 species**
  (far beyond the 31 GFF3 files), stem-loops only, no assembly stamp.
  35,202 rows covering 33,920 stem-loops (86.5% of all entries).

```sparql
PREFIX mb:    <https://w3id.org/rete/mirbase#>
PREFIX faldo: <http://biohackathon.org/resource/faldo#>
PREFIX rdfs:  <http://www.w3.org/2000/01/rdf-schema#>
PREFIX dct:   <http://purl.org/dc/terms/>
SELECT ?chr ?start ?end ?asm ?src WHERE {
  ?hp rdfs:label "hsa-mir-21" ; mb:location ?r .
  ?r faldo:reference ?chr ; dct:source ?src ;
     faldo:begin/faldo:position ?start ;
     faldo:end/faldo:position   ?end .
  OPTIONAL { ?r mb:assembly/rdfs:label ?asm }
}
# chr17 59841266 59841337 GRCh38 (gff3) + the same from chromosome_build
```

### `mb:Placement` — why mature offsets are reified

A mature miRNA can sit on **several** stem-loops at **different** offsets:
`MIMAT0007005` is at 96–117 on `oan-mir-153-1` but 39–60 on `oan-mir-153-2`.
Hanging `mb:matureFrom` off the mature alone would record two contradictory
values, so offsets live on an `mb:Placement` node joining one mature to one
stem-loop. For convenience the offsets are *also* placed directly on the mature
whenever it has exactly one parent (the unambiguous case).

```sparql
SELECT ?hp ?from ?to WHERE {
  ?pl mb:mature <https://www.mirbase.org/mature/MIMAT0007005> ;
      mb:stemLoop ?h ; mb:matureFrom ?from ; mb:matureTo ?to .
  ?h rdfs:label ?hp
}
```

## Gotchas (all hard-won here — read before touching the scripts)

1. **The same file is served two different ways, and one of them is HTML.**
   `/download/<file>` returns raw bytes but `/download/CURRENT/<file>` returns
   the payload rendered into a template — `<p>`, `<br>` for newlines, `&gt;`
   for `>`. Only *some* files exist at the raw path. Everything in
   `database_files/`, plus `*_high_conf.*`, `miRNA.str`, `README` and `LICENSE`,
   is **HTML-wrapped only**. `unwrap_html.py` reverses it, and proves the
   transform is lossless by un-wrapping `hairpin.fa` — which the server happens
   to serve *both* ways — and asserting byte-equality with the raw copy
   (6,132,877 bytes, exact).

2. **Never name a script `inspect.py`.** It lands on `sys.path` and shadows the
   stdlib module numpy imports, giving
   `AttributeError: module 'inspect' has no attribute 'cleandoc'`. The profiler
   here is `profile_raw.py`.

3. **`MI0023465` is corrupt in miRBase's own export.** Its tab-separated fields
   are shifted: description empty, sequence carries a leading space, `comment`
   holds `" 124"`, and `auto_species` is `1` — which resolves to *Amphimedon
   queenslandica* although the entry is *Brugia malayi* (`bma-mir-5863-2`).
   miRBase omits it from `hairpin.fa`, `miRNA.dat` and `mirna_pre_mature`.
   The graph keeps the entry but flags it `mb:sourceRowMalformed true` and
   refuses to emit the wrong species link.

4. **The relational `mature_name` is stale for 8 entries** relative to the name
   miRBase actually publishes in `mature.fa` (`MIMAT0001107` is `gga-miR-222` in
   the table, `gga-miR-222a` in the FASTA). The published name wins for
   `rdfs:label`; the table's name is kept as `skos:altLabel`. The GFF3 `Name=`
   attribute can differ again, so it is recorded on the region.

5. **A mature can appear more than once in one GFF3 file** — one row per copy —
   and the duplicates get suffixed IDs: `ID=MIMAT0027618_1;Alias=MIMAT0027618`.
   Key entities off `Alias`; `ID` and the per-copy `Derives_from` belong to the
   region.

6. **`mature.fa` ships 25 entries that `mirna_mature` marks `dead_flag=1`**, so
   do not filter matures by the dead flag when regenerating it. `hairpin.fa`,
   by contrast, *does* exclude dead entries.

7. **Reference blocks are richer than they look.** Some carry two `RX` lines
   (a MEDLINE *and* a PUBMED id), and some carry an `RC` retraction/erratum note
   — which miRBase places *after* `RL`, not in the usual EMBL position after
   `RN`. Missing either was worth 54 non-reconstructing records.

8. **`miRNA.csv` has a UTF-8 BOM**; read it as `utf-8-sig`.

9. **Only 31 species have GFF3 files.** Probing for others (`ssc`, `oar`, `gma`,
   …) returns the 404 HTML page. Use `mirna_chromosome_build` for the other
   species with coordinates.

10. **`docker build` needs a Windows-style path on Windows.** `/d/pro/rete/...`
    fails with `path not found`; `py.sh` resolves it with `pwd -W`. Scripts run
    inside the container also need repo-relative output paths, not host paths.

11. **77 withdrawn entries "forward" to themselves** (`MI0000039` → `MI0000039`),
    which means "no replacement". Emitting that would assert — via the `rdfs:range`
    of `mb:forwardTo` — that 77 accessions absent from the stem-loop table are
    stem-loops, so the self-reference is dropped. Found only because the OWL
    entailment count came out 77 too high.

12. **One genuinely dangling forward remains**: dead entry `MI0000506` forwards to
    `MI0001399`, which does not exist anywhere in miRBase's own tables. That is
    real source data, so it is kept as-is — it is why an `--entail` count of
    `SO:0001244` returns 39,234 rather than 39,233.

13. **A test must never mutate the artifact it is testing.** An earlier
    `roundtrip_rete_test.sh` moved `parquet/` aside and restored it from an
    `EXIT` trap so the converters would read the rete-derived tables; when the
    restore did not fire, the whole directory was lost (recoverable — it
    regenerates from `raw/` — but avoidable). The converters now take the input
    Parquet directory as an argument, and the test only ever writes into its own
    scratch directories.

## Scripts

| script | does |
|---|---|
| `download.sh` | fetch everything, un-wrap, checksum |
| `unwrap_html.py` | HTML-wrapped payload → plain text (self-testing) |
| `profile_raw.py` | schema + statistics report over the whole raw drop |
| `tables_to_parquet.py` | 17 relational tables → Parquet (schema read from `tables.sql`) |
| `fa_to_parquet.py` / `parquet_to_fa.py` | FASTA ↔ Parquet |
| `embl_to_parquet.py` / `parquet_to_embl.py` | EMBL ↔ Parquet (self-verifying) |
| `gff3_to_parquet.py` / `parquet_to_gff3.py` | GFF3 ↔ Parquet |
| `emit_metadata.py` | emit + validate `schema.json` and `croissant.jsonld` |
| `make_ontology.py` | `mirbase.ttl` → `mirbase-ontology.nt` for the build |
| `parquet_to_nt.py` | Parquet → N-Triples (the graph mapping) |
| `rete_to_parquet.py` | `.rete` → Parquet (the reverse) |
| `build_rete.sh` | stream ontology + triples into `rete build` |
| `roundtrip_test.sh` / `roundtrip_rete_test.sh` | the two byte-identity proofs |
| `diagnose_embl.py` | dev aid: show why a record failed to reconstruct |

## Next step

Not yet published. To put it in the playground, hand off to the **rete-publish**
skill (R2 upload + catalog registration + Range/CORS check).

<!-- r2-backup -->
## Storage — mirrored on Cloudflare R2

Built artifacts for this dataset are mirrored on Cloudflare R2 (public, HTTP-range served). The local copies were **reclaimed 2026-07-30** to free disk; re-fetch from the URLs below, or rebuild via `scripts/`.

The `parquet/` layer (**31 files, 14.0 MB**) is mirrored at `https://data.graphplaza.com/mirbase/parquet/` — query it directly with DuckDB (see `data/scholar/PARQUET_QUERY.md`) or re-fetch to rebuild.
