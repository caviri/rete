# The Dataset Card

The card travels inside the `.rete` file. It is the first thing a person, a
catalog or an agent reads (`dataset_card`, `rete card`), and a remote reader
gets it from two small range requests. A graph without a good card is a graph
nobody trusts. Fill all of it that you know, and do not invent what you do not.

## Fields

| Field | What goes in it |
|---|---|
| `title` | A short name for the dataset, not the file name. |
| `description` | Markdown, up to 8 KiB. What it contains, how many of what, the time span, the main classes, what it is for and what it is not for. Paragraphs, lists and links; no raw HTML. |
| `license` | An SPDX id (`CC0-1.0`, `CC-BY-4.0`, `ODbL-1.0`) or a license URL. The license that applies to the data you put in, not the one you would like. |
| `source` | Where the data came from: a URL, a paper, an agency. |
| `source_date` | The date of the source snapshot (`2026-11-01`). |
| `version` | The release of the data: a date or semver. Not the format version. |
| `creators` | A list of `{ "name": ..., "orcid": "https://orcid.org/0000-0002-1825-0097" }`. ORCID as a full IRI. |
| `publisher` | `{ "name": ..., "ror": "https://ror.org/02s376052" }`. ROR as a full IRI. |
| `doi` | The dataset's DOI as an IRI: `https://doi.org/10.5281/zenodo.0000000`. Only when it exists (Zenodo, DataCite). Mint one by depositing the file, then rebuild the card with it. |
| `cite_as` | The citation you want people to use. |
| `keywords` | Free-text tags. |
| `theme` | Controlled-vocabulary IRIs, such as an EU data-theme URI. |
| `canonical_url` | Where the file is published, once it is. |
| `sparql_endpoint` | A live endpoint for the same data, if one exists. |
| `derived_from` | A list of URLs of the files this was built from. |
| `extra` | An object of your own fields (at most 64 keys, 8 KB). For things no field covers; never a place to hide the license. |

Counts (`triple_count`, `term_count`, named graphs) and the format version are
filled in by the build. Do not write them.

The server's `build_rete` takes the same fields in its `card` argument, and
`examples` for starter queries. The CLI takes them as flags (`--title`,
`--license`, `--source`, `--description`, `--created`) or, for everything, a
JSON file with `--card-file card.json`. Explicit flags override the file.

## What a good description says

- One line: what the dataset is.
- What is in it, with numbers: "3 papers, 4 people, 3 citation links".
- Where it came from and how it was processed, in a sentence.
- How to start: the main classes and one example question.
- Known gaps: what is missing, what is uncertain.

```markdown
Papers on early computing, the people who wrote them and who cited whom.

Contains 3 papers and 4 authors from 1843 to 1952, with Wikidata links for
each person. Compiled by hand from the papers' reference lists in 2026.

Start with `schema:ScholarlyArticle`, linked to `schema:Person` by
`schema:author` and to other papers by `schema:citation`.

Limits: citations are only those listed in the three papers; this is not a
bibliography of the field.
```

## Starter queries

Starter queries are what a newcomer runs first, so each must run and return rows.

- Write three to six, from simplest to most interesting: counts by class, a list with labels, a join across two classes, an aggregate that answers a real question.
- Give each a `title` and a plain-language `question`.
- Use full prefixes inside each query: the card shows them out of context.
- Add a `LIMIT` to lists.
- Run every one against the built file before you ship it. A local build measures the queries it derives itself and drops one that returns nothing; yours are only reported (`rete card-audit file.rete` lists which still answer). On the server you check them yourself.

Server shape: `[{ "title": "...", "question": "...", "sparql": "..." }]`.
CLI `--card-file` shape: `"example_queries": ["SELECT ...", ...]`.

A local build with `--card` also derives about twenty starter queries from the
data (counts, label lists, top classes and predicates) and measures what each
costs. A server build writes your curated card and counts, and the schema is
read from the file by `dataset_schema`; the derived profile is the reason to
build locally when the graph will be published.

## Updating a card without rebuilding

`rete recard old.rete -o new.rete --card-file card.json` replaces the card and
the build record and leaves the data alone. The card is part of the file's
content hash, so the file's identity changes: record the new hash wherever you
published the old one.
