# hugging-face

Metadata graph of the **Hugging Face Hub**: every public model, dataset, and
space, the arXiv + Daily Papers corpus, posts, and — harvested from the Hub
API — the people layer: user/org profiles, org membership, and the
follower network.

Two sources, one snapshot date (2026-07-23):

1. **Bulk backbone** — [`cfahlgren1/hub-stats`](https://huggingface.co/datasets/cfahlgren1/hub-stats)
   (Apache-2.0, refreshed daily), pinned to revision
   `4c7906281206eb8c8445711afba1c9f53f54e599` (2026-07-23T13:41Z).
   Six Parquet files: models, datasets, spaces, arxiv_papers, daily_papers, posts.
2. **People layer** — the public Hub API (`/api/users/{u}/overview`,
   `/api/organizations/{o}/overview|members|followers`, `/api/users/{u}/followers|following`),
   harvested with the resumable scripts in `scripts/`. Anonymous rate limit is
   500 req/5 min; authenticated 2,500 req/5 min (token via `HF_TOKEN`, never on disk).

License: hub-stats is **Apache-2.0**; the API metadata is factual public-profile
data (attribution: "Metadata from the Hugging Face Hub, huggingface.co").
**Buckets**: HF's new storage-bucket repo type has *no public listing endpoint yet*
— we carry `num_buckets` per user/org (from the overview API) but cannot
enumerate the buckets themselves.

## Layout

```
data/hugging-face/
  README.md
  SHA256SUMS.txt              # checksums of the six hub-stats parquets
  raw/
    hub-stats/                # the pinned backbone, as-is (+ REVISION.txt)
      models.parquet          # 2,932,554 rows — id, author, tags, likes, downloads(AllTime),
                              #   safetensors params, gguf, baseModels, inferenceProviderMapping…
      datasets.parquet        #   972,446 rows — id, author, tags, likes, downloads, cardData…
      spaces.parquet          # 1,428,760 rows — id, author, sdk, tags, likes, cardData…
      arxiv_papers.parquet    #    94,404 rows — arxiv id, title, authors, summary, upvotes…
      daily_papers.parquet    #    16,763 rows — paper + submitter + HF-linked authors structs
      posts.parquet           #     1,520 rows — social posts w/ mentions, reactions
  raw/authors/                # authors_seed.tsv|parquet — distinct author universe + hints
  raw/api/                    # JSONL harvest shards + _done.txt resume state
    profiles/  members/  followers/  following/
  parquet/                    # consolidated people + pointer tables (see below)
  scripts/
```

## People + pointer tables (`parquet/`, built by to_parquet.py)

| file | contents |
|---|---|
| `users.parquet` | per-user profile: fullname, isPro, numModels/Datasets/Spaces/Kernels/**Buckets**/Papers/Upvotes/Likes/Followers/Following |
| `orgs.parquet` | per-org profile: fullname, verified, plan, numUsers + same counters |
| `org_members.parquet` | org→user edges (members API ∪ user-overview `orgs[]`) |
| `followers.parquet` | follower→followee edges (users *and* orgs as followees) |
| `following.parquet` | user→followed edges (optional stage) |
| `repo_papers.parquet` | model/dataset → arXiv id (from `arxiv:` tags) |
| `model_base_models.parquet` | model → base model + relation (finetune/quantized/…) |
| `model_datasets.parquet` | model → dataset it was trained/tuned on (tags ∪ card metadata) |
| `paper_hf_authors.parquet` | daily-paper → author name + linked HF account |
| `space_links.parquet` | space → models/datasets it uses (Hub-computed `expand[]` sweep ∪ card) |
| `profile_misses.parquet` | seed names that 404'd (deleted/renamed accounts) |

## Reproduce

```bash
# 1. backbone (~2.5 GB)
bash data/hugging-face/scripts/download.sh

# 2. author universe from the parquets
MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
  bash -c "pip -q install duckdb && python data/hugging-face/scripts/extract_authors.py"

# 3. harvest (ONE detached container chains all four stages; resumable;
#    token from the host `hf` login; ~2-3 days for profiles, then edges)
bash data/hugging-face/scripts/run_harvest.sh all
#    (or stage-by-stage: profiles | members | followers | following)

# 4. consolidate + profile
MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
  bash -c "pip -q install duckdb && python data/hugging-face/scripts/to_parquet.py"
MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
  bash -c "pip -q install duckdb && python data/hugging-face/scripts/inspect_parquet.py"
```

Harvests are resumable: stop/restart at will, `_done.txt` carries the state;
truncated walks (a `--max-pages` cap, default 200k edges/account) are written
to the shard as explicit `*_truncated` records — no silent caps.

## Dataset shape (final, harvest completed 2026-07-26)

The full harvest ran 2026-07-23 → 2026-07-26 in one container
(`run_harvest.sh all`): space-links sweep 13 min → 1,354,365 profiles 51.5 h →
members 91 min → followers 5.8 h → following 3.6 h. Zero truncated walks;
only 94 seed names 404'd on both endpoints.

| table | rows | note |
|---|---|---|
| `users.parquet` | 1,315,773 | 22,614 buckets counted across user accounts |
| `orgs.parquet` | 38,498 | 1,241 buckets across orgs |
| `followers.parquet` | 2,905,301 | onto users AND orgs |
| `following.parquet` | 567,191 | user → followed account |
| `org_members.parquet` | 457,195 | members API ∪ user-overview `orgs[]` |
| `space_links.parquet` | 981,507 | Hub-computed "space uses model/dataset" |
| `model_base_models.parquet` | 842,649 | finetune / quantized / adapter / merge |
| `model_datasets.parquet` | 350,688 | model → training dataset |
| `repo_papers.parquet` | 766,758 | model/dataset → arXiv id |
| `paper_hf_authors.parquet` | 159,980 | incl. linked HF accounts |
| `profile_misses.parquet` | 94 | deleted/renamed accounts |

Backbone (raw/hub-stats): 2,932,554 models · 1,428,760 spaces · 972,446
datasets · 94,404 arXiv papers · 16,763 daily papers · 1,520 posts.
Full per-column fill report: `scripts/inspect_parquet.py`.

## Ontology & the knowledge graph

`hugging-face.ttl` (prefix `hf:` = `https://w3id.org/rete/huggingface#`) models
the Hub as a scholarly-social graph, **aligned to the rete scholar hub**
(`data/scholar/scholar.ttl`, now v1.4.0 with hf wired in):

- Classes: `hf:User` ⊑ `scholar:Person`; `hf:Organization` ⊑ `scholar:Organization`;
  `hf:Model`/`hf:DatasetRepo`/`hf:Space`/`hf:Paper` ⊑ `scholar:Work`
  (+ schema.org supers); `hf:Post` ⊑ `schema:SocialMediaPosting`.
- Derivation fabric: `hf:finetunedFrom`/`quantizedFrom`/`adapterFor`/`mergedFrom`
  ⊑ `hf:baseModel` ⊑ `prov:wasDerivedFrom` + `schema:isBasedOn`;
  `hf:trainedOn` ⊑ `prov:wasDerivedFrom`; `hf:usesModel`/`hf:usesDataset` ⊑
  `hf:uses` ⊑ `dcterms:requires`; `hf:citesPaper` ⊑ `schema:citation`.
- Social: `hf:follows` ⊑ `sioc:follows`; `hf:memberOf` ⊑ `org:memberOf`.
- **Canonical IRIs** (scholar policy): papers mint at
  `https://doi.org/10.48550/arxiv.{id}` — they auto-merge with
  DataCite/Crossref/OpenAIRE records of the same preprint in a union graph;
  `hf:doi` ⊑ `scholar:doi`. Accounts/repos mint at their real
  `https://huggingface.co/...` URLs.
- Validated: 13-ontology union (scholar + 11 datasets + hf), 0 broken
  cross-references (`data-ontology` skill guard).

### hugging-face.rete

Built from all fields of the tables above (85,142,499 statements + ontology;
zero-valued counters elided; cardData JSON, sibling manifests, safetensors
per-dtype breakdowns, BibTeX blobs and AI summaries stay in the Parquet
companions):

```bash
# emit N-Triples (~12 GB, ~13 min)
MSYS_NO_PATHCONV=1 docker run --rm -v "D:/pro/rete:/w" -w //w python:3.12-slim \
  bash -c "pip -q install pyarrow && python data/hugging-face/scripts/build_nt.py > /w/data/hugging-face/hugging-face.nt"
# ontology → NT (rdflib), then validate + build (see git history for exact flags)
skills/rete-from-graph/scripts/rete validate /work/data/hugging-face/hugging-face.nt
skills/rete-from-graph/scripts/rete build /work/data/hugging-face/hugging-face.nt \
  /work/data/hugging-face/ontology.nt -o /work/data/hugging-face/hugging-face.rete \
  --pyramid-algo types --card --title "Hugging Face Hub" ...
```

Note: `--memory-budget-mb` (external build) works but skips the pyramid and the
rich card; on a ≥32 GiB machine prefer the monolithic build.

**Built 2026-07-27**: `hugging-face.rete` — 85,142,709 statements (84,650,044
unique quads), 31,712,951 terms, types pyramid (1 level), 53.6 KB embedded card,
**1.16 GB**. Monolithic build: ~25 min, ≤6 GiB RAM. `rete verify` green;
spot-checked: base-model genealogy (top target: distilbert-base-uncased,
12,052 finetunes), social joins (org members × follower counts), and
`?w a scholar:Work` under `--entail`. **SHIPPED** — remote-lazy at
`https://data.graphplaza.com/hugging-face/hugging-face.rete`, in the playground.

### hugging-face-full.rete — the lossless counterpart

The graph build above deliberately leaves out the bulky non-graph payloads.
`--full` puts **everything** in, and answers the question "how much of the
compression was real?":

```bash
python data/hugging-face/scripts/build_nt.py --full | gzip -1 > hugging-face-full.nt.gz
{ cat ontology.nt; gzip -dc hugging-face-full.nt.gz; } | rete build - --format nt \
  -o hugging-face-full.rete --card --memory-budget-mb 16000 --tmp-dir _build_tmp
```

| | `hugging-face` | `hugging-face-full` | source Parquet |
|---|---|---|---|
| statements | 85,142,709 | **272,283,708** (254,397,090 unique) | — |
| dictionary terms | 31,712,951 | **83,657,609** | — |
| size | 1.16 GB | **2.71 GB** | **2.67 GB** |
| pyramid / card | types, 53.6 KB | none, 700 B (external build) | — |

**The full graph is the same size as the Parquet it fully represents (1.02×) —
while carrying four sorted index permutations and answering queries over HTTP
range reads.** So the earlier 2.3× "compression" was not compression at all: it
was the 71% of Parquet bytes (file manifests, config/card JSON, `_id`) that the
graph build omits. Like-for-like, RDF-in-`.rete` costs about what columnar
Parquet costs, and buys indexing.

What `--full` adds:

- **File manifests** — 60.7M model + 36.3M space entries as `hf:file` *literals*.
  They are literals by design: the source carries only a filename, and ~97M
  entries share ~36M distinct values, so the dictionary dedupes them 2.7×
  (minting a node per file would have cost ~200M extra triples for no
  information). Reverse lookup works: 1,174,032 repos contain a `config.json`.
- **JSON blobs kept twice** — verbatim (`hf:cardDataJson`, `hf:configJson`,
  `hf:ggufJson`, so nothing structural is lost) *and* path-flattened into
  queryable triples under the `card#` / `config#` / `gguf#` key namespaces.
- **Reified inference offers** (`hf:InferenceOffer`) — provider, status, task,
  context length, per-token pricing, measured latency and tokens/second.
- **Per-dtype parameter counts** (`param#BF16`, `param#F32`, …), transformers
  auto-classes, `_id`, BibTeX citations, AI summaries/keywords, paper media and
  GitHub stars, and every post content block, reaction and commentator.

Two honest caveats: RDF is a *set*, so 17.9M duplicate statements (repeated
values inside flattened arrays) collapse — lossless for querying, but array
multiplicity lives only in the verbatim JSON. And a few columns that merely
repeat an account's own profile (a post author's avatar/follower count, a
paper submitter's avatar) are represented once on the account node instead of
per row.

<!-- r2-backup -->
## Storage — mirrored on Cloudflare R2

Built artifacts for this dataset are mirrored on Cloudflare R2 (public, HTTP-range served). The local copies were **reclaimed 2026-07-30** to free disk; re-fetch from the URLs below, or rebuild via `scripts/`.

| file | size | URL |
|---|--:|---|
| `hugging-face.rete` | 1.16 GB | https://data.graphplaza.com/hugging-face/hugging-face.rete |
