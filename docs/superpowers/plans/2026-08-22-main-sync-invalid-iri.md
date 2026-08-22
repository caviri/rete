# Main Synchronization and Invalid-IRI Integration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate current `origin/main` into `feat/rust-optimization` without losing either the measured optimization work or main's invalid-IRI audit/sanitization contract, then re-establish a green Task 6 boundary before Task 7.

**Architecture:** Preserve the long-running feature history with one explicit merge commit instead of rebasing eighty commits. Resolve hand-written Rust, Markdown, scripts, and catalog sources semantically; regenerate HTML and browser artifacts only from the resolved sources. Treat `crates/rete-core/src/iri.rs` as the single invalid-IRI definition and prove all three build paths retain main's audit behavior after the merge.

**Tech Stack:** Git merge machinery, Rust 2021 workspace, Docker Compose devcontainer, `docgen`, canonical `scripts/build_wasm.sh`, browser regression gate.

**Spec:** `docs/superpowers/specs/2026-08-19-unified-build-pipeline-design.md`; existing optimization execution plan `docs/superpowers/plans/2026-08-19-unified-build-pipeline.md`; invalid-IRI behavior is specified by `origin/main` commit `cee7ac6a` and `docs/interop.md`.

## Global Constraints

- Merge `origin/main` into `feat/rust-optimization`; do not rewrite or discard either 80-commit side.
- Physical term IDs remain `u32`; counts and file coordinates remain `u64` with checked materialization conversions.
- `rete build` audits invalid IRIs by default, `--strict` refuses them, and `rete export --sanitize-iris` is opt-in and never claims to repair a schemeless IRI.
- Keep `CURRENT_FORMAT_VERSION` and `MIN_FORMAT_VERSION` at `0x05` until Task 7 deliberately switches production dispatch.
- Use `scripts/build_wasm.sh` as the only producer of browser artifacts.
- Do not use `git clean -x`, destructive resets, force pushes, or broad checkout-based conflict resolution.

---

### Task 1: Record and Start the Merge Checkpoint

**Files:**
- Create: `docs/superpowers/plans/2026-08-22-main-sync-invalid-iri.md`
- Modify: Git history only

**Interfaces:**
- Consumes: clean `feat/rust-optimization` at `4570d597`; `origin/main` at `cee7ac6a`
- Produces: one in-progress merge whose conflicts retain stage-1/base, stage-2/feature, and stage-3/main blobs

- [ ] **Step 1: Verify the exact tips and clean worktree**

```powershell
git status --short --branch
git rev-parse HEAD
git rev-parse origin/main
git rev-list --left-right --count origin/main...HEAD
```

Expected: clean branch; tips `4570d597...` and `cee7ac6a...`; `80 80` divergence.

- [ ] **Step 2: Commit this integration plan**

```powershell
git add docs/superpowers/plans/2026-08-22-main-sync-invalid-iri.md
git commit -m "docs: plan main integration checkpoint"
```

- [ ] **Step 3: Start a no-edit merge**

```powershell
git merge --no-ff --no-commit origin/main
```

Expected: conflicts are preserved for semantic resolution; no merge commit yet.

### Task 2: Resolve Core and CLI Source Semantically

**Files:**
- Modify: `crates/rete-core/src/extbuild.rs`
- Modify: `crates/rete-core/src/file.rs`
- Modify: `crates/rete-core/src/index.rs`
- Modify: `crates/rete-core/src/lib.rs`
- Modify: `crates/rete-wasm/src/lib.rs`
- Modify: `crates/rete-cli/src/commands/url.rs`
- Verify: `crates/rete-core/src/iri.rs`
- Verify: `crates/rete-core/src/ingest.rs`
- Verify: `crates/rete-cli/src/commands/build.rs`
- Verify: `crates/rete-cli/src/commands/export.rs`
- Test: `crates/rete-cli/tests/invalid_iris.rs`

**Interfaces:**
- Consumes: optimization-side family format/build/range-reader implementations and main-side optional-permutation/named-graph/IRI-audit changes
- Produces: compiling combined sources with the same public invalid-IRI CLI contract as `cee7ac6a`

- [ ] **Step 1: Resolve one source conflict at a time from all three stages**

```powershell
git show :1:crates/rete-core/src/file.rs > $env:TEMP/rete-file-base.rs
git show :2:crates/rete-core/src/file.rs > $env:TEMP/rete-file-feature.rs
git show :3:crates/rete-core/src/file.rs > $env:TEMP/rete-file-main.rs
git diff --cc -- crates/rete-core/src/file.rs
```

Apply the same inspection to every conflicted hand-written source. Preserve main's three-permutation masks, named-graph external build, separator dictionary routing, and IRI audit; preserve the feature branch's safe paired-family codec, checked range boundaries, adaptive reads, and telemetry.

- [ ] **Step 2: Prove the invalid-IRI feature exists before broader tests**

```powershell
git grep -n "pub mod iri" -- crates/rete-core/src/lib.rs
git grep -n "IriAudit" -- crates/rete-core/src/ingest.rs crates/rete-cli/src/commands/build.rs
git grep -n -- "--sanitize-iris" crates/rete-cli/src/main.rs docs/cli.md
```

Expected: all three searches return the main-side definitions/call sites.

- [ ] **Step 3: Run the focused invalid-IRI suite**

```powershell
docker compose run --rm dev cargo test -p rete-cli --test invalid_iris -- --nocapture
```

Expected: build warning, strict refusal, sanitizing export, schemeless non-repair, and round-trip cases all pass.

- [ ] **Step 4: Run the focused staged-format and range-boundary suites**

```powershell
docker compose run --rm dev cargo test -p rete-core family_ -- --nocapture
docker compose run --rm dev cargo test -p rete-core materializ -- --nocapture
docker compose run --rm dev cargo test -p rete-core --test ranged -- --nocapture
```

Expected: all pass; production version constants remain `0x05`.

### Task 3: Resolve Documentation, Catalog, and Build Sources

**Files:**
- Modify: `AGENTS.md`
- Modify: `README.md`
- Modify: `docs/BENCHMARK.md`
- Modify: `docs/SPEC.md`
- Modify: `docs/browser.md`
- Modify: `docs/cli.md`
- Modify: `docs/dataset-cards.md`
- Modify: `scripts/build_wasm.sh`
- Modify: `tests/gate/README.md`
- Modify: `web/playground-src/catalog.js`
- Modify: `web/datasets.lock.json`
- Delete: `docs/explore-100mb.html`

**Interfaces:**
- Consumes: both branches' Markdown/source claims and current catalog records
- Produces: a single truthful source set from which all derived HTML/WASM assets can be rebuilt

- [ ] **Step 1: Resolve source prose by claim, not by side**

Retain main's current compatibility, publishing, invalid-IRI, catalog, and browser-build rules. Retain only optimization claims supported by committed benchmark evidence, including the staged `0x06` description and query-time decompression caveats.

- [ ] **Step 2: Accept main's deliberate obsolete-file deletion**

```powershell
git rm -- docs/explore-100mb.html
```

- [ ] **Step 3: Resolve catalog and gate sources before generated pages**

```powershell
node --check web/playground-src/catalog.js
node --check tests/gate/checks/check_card_modal.mjs
node --check tests/gate/checks/test_catalog_matrix.mjs
python -m json.tool web/datasets.lock.json > $null
```

Expected: syntax/JSON checks pass and the current main David Rumsey entries win where the feature's older catalog snapshot conflicts.

### Task 4: Regenerate Derived Documentation and Browser Artifacts

**Files:**
- Regenerate: `docs/*.html`
- Regenerate: `docs/engine/*`
- Regenerate: `docs/playground.html`
- Regenerate: `docs/rete_wasm_async.*`
- Regenerate: `docs/wasm-build.json`

**Interfaces:**
- Consumes: resolved Rust, Markdown, build scripts, catalog, and playground sources
- Produces: byte-consistent derived artifacts; no hand-resolved generated HTML or WASM binaries

- [ ] **Step 1: Regenerate docgen pages**

```powershell
docker compose run --rm dev cargo run -q -p docgen
```

- [ ] **Step 2: Regenerate every browser artifact with the canonical producer**

```powershell
$reteRevision = git rev-parse HEAD
docker compose run --rm -e RETE_SOURCE_REVISION=$reteRevision wasm
```

If the merge is still uncommitted, use the pre-merge feature tip solely as the required non-empty source revision; the final merge commit will be recorded in the subsequent canonical rebuild before branch completion.

- [ ] **Step 3: Confirm no conflict markers or unmerged paths remain**

```powershell
git diff --check
git diff --name-only --diff-filter=U
rg -n "^(<<<<<<<|=======|>>>>>>>)" AGENTS.md README.md crates docs scripts tests web
```

Expected: no output from the latter two checks.

### Task 5: Verify and Commit the Integration Boundary

**Files:**
- Modify: `.superpowers/sdd/2026-08-19-unified-build-pipeline/progress.md` (ignored execution ledger)
- Modify: Git history only

**Interfaces:**
- Consumes: fully resolved/regenerated merge tree
- Produces: one reviewed main-integration merge commit and a green base for Task 7

- [ ] **Step 1: Run the canonical workspace matrix**

```powershell
docker compose run --rm dev cargo fmt --all -- --check
docker compose run --rm dev cargo clippy --workspace --exclude rete-bench --all-targets -- -D warnings
docker compose run --rm dev cargo test --workspace --exclude rete-bench
docker compose run --rm dev cargo test -p rete-core --no-default-features
docker compose run --rm dev cargo build -p rete-core --all-features
docker compose run --rm dev bash scripts/smoke.sh
```

- [ ] **Step 2: Run the browser regression gate because browser artifacts changed**

```powershell
docker compose run --rm dev bash tests/gate/gate.sh
```

- [ ] **Step 3: Commit the merge without a co-author trailer**

```powershell
git add -A
git commit -m "merge: synchronize rust optimization with main"
```

- [ ] **Step 4: Rebuild canonical browser artifacts against the merge revision if the manifest embeds the revision**

```powershell
$reteRevision = git rev-parse HEAD
docker compose run --rm -e RETE_SOURCE_REVISION=$reteRevision wasm
git diff --exit-code -- docs web
```

- [ ] **Step 5: Re-run the Task 6 independent review boundary**

Review the merge delta and the staged-family/materialization invariants before setting Task 6 complete or starting Task 7.

