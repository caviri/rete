# BCN Stable Dataset Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild Barcelona from its existing RDF source with stable format `0x05`, replace the incompatible R2 object, refresh the catalog lock, and prove all BCN playground examples work.

**Architecture:** The existing 1.36 GB `data/bcn/bcn.nt` is validated and rebuilt inside the devcontainer with the current release CLI, type pyramid, text index, and dataset card. The candidate is verified locally, uploaded through the repository's publish pipeline, then verified through public Range/CORS probes and the exhaustive browser catalog runner.

**Tech Stack:** Rete Rust CLI in Docker, repository `rete-from-graph` and `rete-publish` skills, Python catalog checker, Cloudflare R2 S3 API, Playwright browser gate.

## Global Constraints

- Do not add read support for experimental format `0x04`; rebuild from RDF source.
- Use `data/bcn/bcn.nt` and do not modify or commit ignored raw data/candidates.
- Build with `--pyramid-algo types --text-index --card`.
- Verify header `RETE 0x05`, content hash, indexes, card, and catalog queries before upload.
- Publish to the existing `bcn/bcn.rete` key through the repository R2 tooling.
- Verify browser-visible `Content-Range`, CORS, and all four BCN examples after upload.
- Update and commit `web/datasets.lock.json`; do not commit the `.rete` file.
- Use Docker Compose/devcontainer commands and commit without a coauthor trailer.

---

### Task 1: Validate Source and Build the Stable Candidate

**Files:**
- Read: `data/bcn/bcn.nt`
- Create ignored artifact: `data/bcn/bcn-v5.rete`
- Read: `skills/rete-from-graph/reference/build.md`
- Read: `skills/rete-from-graph/reference/verify.md`

**Interfaces:**
- Produces: locally verified `data/bcn/bcn-v5.rete` with stable header byte 5.

- [ ] **Step 1: Record source and old artifact identities**

```bash
wc -c data/bcn/bcn.nt data/bcn/bcn.rete
sha256sum data/bcn/bcn.nt data/bcn/bcn.rete
python3 -c 'print(open("data/bcn/bcn.rete","rb").read(5).hex())'
```

Expected: source is non-empty and old header is `5245544504` (`RETE` plus format 4).

- [ ] **Step 2: Validate the N-Triples source**

```bash
skills/rete-from-graph/scripts/rete validate /work/data/bcn/bcn.nt
```

Expected: parse succeeds with non-zero triple count and no malformed-line error.

- [ ] **Step 3: Build the stable candidate**

```bash
skills/rete-from-graph/scripts/rete build /work/data/bcn/bcn.nt \
  -o /work/data/bcn/bcn-v5.rete --pyramid-algo types --text-index --card
```

Expected: build completes and reports non-zero dictionary, six indexes, pyramid, text index, and card.

- [ ] **Step 4: Verify the candidate**

```bash
skills/rete-from-graph/scripts/verify_rete.sh /work/data/bcn/bcn-v5.rete
python3 -c 'assert open("data/bcn/bcn-v5.rete","rb").read(5) == b"RETE\x05"'
```

Expected: `info`, `stats`, `verify`, sample SPARQL, footer/content hash, and stable header all pass.

### Task 2: Execute Every BCN Query Locally

**Files:**
- Read: `web/playground-src/catalog.js`
- Read: `data/bcn/bcn-v5.rete`
- Create ignored receipt: `target/bcn-repair/local-query-report.json`

**Interfaces:**
- Consumes: stable candidate from Task 1.
- Produces: evidence that all four checked-in BCN SPARQL examples execute without engine errors.

- [ ] **Step 1: Extract and run all four queries without copying them**

Run Node inside the devcontainer to evaluate `catalog.js`, require exactly four `catalog.examples.bcn` entries, and invoke `/work/target/release/rete sparql /work/data/bcn/bcn-v5.rete "$query" --json` for each query.

The driver records `{index,label,rows}` entries and exits non-zero unless every command exits 0, returns valid JSON, and has non-empty bindings.

Expected: four of four examples return results, including example index 3 (the PDF query from the production report).

- [ ] **Step 2: Save the ignored verification receipt**

Write the driver result to `target/bcn-repair/local-query-report.json`. Confirm `git status --short` does not show the receipt or candidate.

### Task 3: Publish Through the Existing R2 Pipeline

**Files:**
- Read: `skills/rete-publish/reference/catalog.md`
- Modify: `web/datasets.lock.json`
- Read: `.env` only through the repository upload wrapper

**Interfaces:**
- Consumes: verified candidate from Task 1.
- Produces: public `https://data.graphplaza.com/bcn/bcn.rete` at format 5 and a matching lock entry.

- [ ] **Step 1: Capture the expected public preflight failure**

```bash
docker compose run --rm dev python3 scripts/check_dataset_catalog.py --key bcn
```

Expected before upload: FAIL because the public object is format 4.

- [ ] **Step 2: Upload the candidate to the existing key**

Use the CRLF-safe `.env` handling in the publish skill and run:

```bash
skills/rete-publish/scripts/upload_bucket.sh data/bcn/bcn-v5.rete bcn/bcn.rete
```

Expected: upload completes without redirect/token changes and publishes only the completed object.

- [ ] **Step 3: Verify public object and refresh complete lock**

```bash
docker compose run --rm dev python3 scripts/check_dataset_catalog.py --key bcn
docker compose run --rm dev python3 scripts/check_dataset_catalog.py --all --write-lock --report target/catalog/report.json
```

Expected: BCN and all 54 remote objects pass stable format, Range, CORS, size, and hash checks; the lock gains the exact BCN identity.

- [ ] **Step 4: Commit the lock update**

```bash
git add web/datasets.lock.json
git commit -m "chore(catalog): publish stable BCN dataset"
```

### Task 4: Browser Verification and Release Evidence

**Files:**
- Read: `tests/gate/checks/check_catalog_examples.mjs`
- Read: `target/catalog/report.json`

**Interfaces:**
- Verifies the public object through the same UI and range paths used by users.

- [ ] **Step 1: Run all four hosted BCN examples in Chromium**

```bash
docker compose --profile extended-tests run --rm gate-catalog-live \
  bash -lc 'npm ci --no-audit --no-fund && node run.mjs --catalog=all --catalog-dataset=bcn'
```

Expected: four queries pass with no error boxes or page errors; the PDF query returns at least one row.

- [ ] **Step 2: Run public CLI range path for the PDF query**

Extract BCN example index 3 from the catalog and run:

```bash
/work/target/release/rete sparql-url https://data.graphplaza.com/bcn/bcn.rete "$query" --json
```

Expected: exit 0 with non-empty bindings and no unsupported-format error.

- [ ] **Step 3: Verify the production browser report no longer reproduces**

Open `https://caviri.github.io/rete/playground.html#dataset=bcn&load=lazy&mode=sparql&ex=3` in the Playwright Compose image, click Run with bounded retry, and require positive rows, no `.error-box`, and no page/console errors.

- [ ] **Step 4: Run the scheduled catalog command locally**

```bash
docker compose run --rm dev python3 scripts/check_dataset_catalog.py --all --report target/catalog/report-final.json
```

Expected: `catalog: 54/54 stable object(s)`.

- [ ] **Step 5: Review final state**

```bash
git diff --check origin/main...HEAD
git status --short
```

Expected: the lock and planned preview code/docs are the only tracked changes; raw BCN data and user-owned untracked files remain uncommitted.
