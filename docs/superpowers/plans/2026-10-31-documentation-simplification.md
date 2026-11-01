# Human-friendly documentation implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Help readers understand rete and complete a first task without navigating a catalogue of features and experiments.

**Architecture:** Keep the existing Markdown/Rust docgen site. Separate the list of published pages from the sidebar, introduce a few task-oriented entry pages, and relocate selected applications to `rete-apps` in stages.

**Tech stack:** Markdown, `crates/docgen`, existing Python link checker, Docker/devcontainer; existing app builders and browser checks for later migrations.

**Spec:** The user's request on 2026-10-31, approved for implementation in a worktree and PR on 2026-11-01.

**First-delivery scope:** This branch implements the renderer/navigation separation, eight task entry pages, the shorter first-use guides and Dataset Card guide/reference split, plus targeted factual and supporting-document corrections. Actual app extraction into `rete-apps` remains a subsequent delivery: the destination has no repository/build/hosting contract yet. Existing app URLs and sources remain available through the local gallery. Exhaustive specialist/SDK prose consolidation also remains follow-up work; the dispositions below describe the overall roadmap, not a claim that every page was rewritten in this PR.

**Audit baseline:** `rete` main at `3469d64c`. Three subagents reviewed core prose, navigation/supporting documentation, and applications. All 56 root `docs/*.md` pages are covered by the disposition table. Supporting READMEs, build/runbooks and skills were inspected for overlap and contradictions. Historical plans/changelog entries are archival; generated notebook distributions, vendored docs, ignored dataset research and other worktrees are outside the editorial rewrite.

This is an editorial and structural audit. Narrative bodies were reviewed; dense tables and code in long references were sampled. It does not certify every command, API claim or published application—the execution checks below are still required.

## Constraints and review focus

- Preserve working commands, API contracts, compatibility exceptions, data provenance and licences. Shorter introductory prose must not weaken technical reference material.
- Preserve published page URLs and important fragments. Keep moved content reachable before retiring its old location.
- Edit Markdown and generators, not generated HTML. Use the repository's Docker build workflow and commit generated output separately when implementing.
- Keep dataset pipelines in `rete-datasets`; do not relocate them into `rete-apps`. Do not rewrite publishers' curated dataset descriptions.
- `D:/pro/Links/rete-apps` is currently empty and is not a Git repository. A working build, dependency and hosting arrangement is a prerequisite for actual app relocation.

Review five failure cases explicitly: a page removed from navigation must still rebuild; an old deep link must still resolve; a newcomer must finish the first example without contributor setup; an app must build without an adjacent rete checkout; factual simplification must preserve runtime and compatibility limits.

## What needs to change

The main problem is mixed purpose: tutorials, complete references, contributor instructions and application showcases compete for equal attention.

| Finding | Evidence | Consequence |
|---|---|---|
| Seven sidebar groups, 55 primary document entries; 17 browser entries alone | `crates/docgen/src/main.rs:42` | Readers must choose among too many unrelated destinations. Expanded app children raise the rendered navigation to 93 links, excluding branding/footer. |
| Navigation also controls rendering and previous/next order | `crates/docgen/src/main.rs:164`, `:328` | Removing an entry currently stops rebuilding that page. Decouple these responsibilities first. |
| One authored page is already outside the rendering list | `docs/graph-algorithms.md`, absent from `SECTIONS` | Publication must not depend on sidebar prominence. |
| Overview repeats most of the sidebar, plus client and feature inventories | `docs/index.md` | The first page should explain the product and offer a few useful starting points. |
| Getting started mixes first use with source builds, index internals, stress testing and contributor checks | `docs/getting-started.md` | A first successful query takes too much reading. |
| Dataset Cards places “Reading a card” near line 1729 | `docs/dataset-cards.md:1729` | A basic user task is buried after schema details and implementation history. |
| Status and terminology drift across copies | `docs/index.md:5`, `:69`, `:145`; `Cargo.toml:21`; sidebar `release.md` label | The overview says v0.3.0 while the checkout is 0.4.2; it also mixes “crates.io pending” with “first release.” The release guide is still labelled “1.0 release candidate.” |
| Some explanations contradict their runtime or each other | Football/subtitle guides; semantic zoom; compatibility; agent frameworks | Correct the explanation before compressing it. |

The shortest pages are often the domain showcases. They mostly need a different home and lower prominence, not aggressive cutting. Conversely, Atlas has few headings but a long feature inventory: fewer headings alone will not produce clearer prose.

**Approaches considered:** collapsing the current groups is quick but retains the same competing choices. Moving every demo immediately would reduce clutter but entangle desktop, engine and notebook builds. The recommendation is to simplify navigation and reading paths first, then relocate selected apps with their dependencies.

## Proposed reading experience

Four sidebar sections, **15 primary links instead of 55** (about 73% fewer). Keep launch buttons inside the relevant pages; do not recreate the entire app catalogue as nested sidebar links.

```text
Start here                    Guides
  Overview                      Build and publish
  Quickstart                    Query and analyze
  Graph concepts                Language clients
  Playground                    Tools and integrations
                                Worked examples

Reference                     Project
  CLI                           Apps and experiments
  SPARQL support                Contributing
  API and technical reference
  Compatibility and limits
```

The guide entry pages should help readers choose a task with a short explanation and a small set of links. They must not become new encyclopedias or lists of every feature. Each retained specialist page belongs to one of these entry pages or the reference directory; none should become an orphan. No new search system is required for this change—the current site has no site-wide search index to preserve.

Use the existing `index.md`, `getting-started.md`, `intro.md`, `playground-guide.md`, `cli.md`, `sparql.md` and `compatibility.md` URLs. Add eight short pages: `build-and-publish.md`, `query-and-analyze.md`, `clients.md`, `integrations.md`, `recipes.md`, `reference.md`, `apps.md`, and `contributing.md`.

Keep three featured learning examples: the SBOM walkthrough, Atlas, and a Python notebook. Playground remains the primary interactive entry point. Other showcases stay discoverable through the app gallery and relevant topic pages.

Writing rules for the rewrite:

- Start with what the reader can do and why it is useful. Explain unfamiliar terms at first use.
- Follow one complete example: goal, action, result, explanation, next step. Show expected output where it helps a reader know the command worked.
- Aim for three to five substantial sections on ordinary guides. Avoid one heading per flag, repeated “why/how/notes” fragments, and deeply nested headings. This is guidance, not a quota for specifications and API references.
- Give each concept one canonical explanation. Link to it from SDK READMEs, skills and other guides instead of copying it.
- Keep limitations next to the action they affect. Move engineering incident narratives and superseded instructions to developer/history material; retain their current operational rule.
- Use precise, plain sentences. Avoid absolute promises such as “never a scan,” “tables can't answer this,” or “no packet leaves the machine” when the behavior depends on the query or hosted model.
- Use page length as a review signal, not a deletion target. For the first pass, aim for roughly 30–40% less prose across Overview, Quickstart and Playground, excluding code and necessary caveats. Do not force that reduction on the format, CLI or compatibility reference.

Example opening for the overview:

> Rete stores RDF data in one `.rete` file. You can query it on your computer or publish it on a web host that supports byte-range requests. A client can then read the parts needed for a SPARQL query without downloading the file first. Selective queries may read very little; broad queries can still scan substantial data.
>
> Try a dataset in the Playground, or follow the quickstart to build and query your own file.

For Dataset Cards, put “Read a card” before “Add metadata,” then explain how to inspect and update it. Move the complete field schema and projections into `dataset-card-reference.md`; keep the update rules and content-hash consequences easy to find. A short guide plus a separate exhaustive reference is more useful than a shortened reference that omits contracts.

## What stays and what moves

Removing a sidebar entry, moving its guide, and relocating its source/runtime are separate changes. Stage them separately.

| Stage | Candidates | Conditions |
|---|---|---|
| Simplify navigation now | All domain showcases and specialist browser tools | Keep existing URLs and builds. Link from `apps.md`, integrations and worked examples. |
| First actual relocation | Graph-map/topic-map/3D, Ask the graph/Lance-RAG, WebGPU coherence | Move guides, source, builders, output assets and test responsibilities together. These are independent experiments rather than core format documentation. |
| Next, selected showcases | Football, subtitles, anatomy, building/BIM, neuromorphology, Lombardi; optionally JS lab | Verify each asset/worker dependency. Retain short featured links where they teach a core task. JS lab should remain reachable from JavaScript docs. |
| Keep in rete initially | Playground, Atlas, JupyterLite notebooks, SBOM and small CLI examples | These provide a coherent learning path and important engine/client coverage. |
| Defer a repository move | File Explorer/Tauri, Plaza, Yasgui, polyglot | Resolve shared UI, worker, engine and release dependencies first. Their `experiments/` location is not sufficient reason to move them. |

Important boundaries:

- Do not move `examples/` wholesale. Small RDF inputs are documentation examples and test fixtures, not apps.
- `clients/tauri/scripts/sync-frontend.sh` consumes `experiments/rete-file-explorer`; its CI tests and release process must keep working.
- Graph-map and Lance-RAG already accept `--rete-bin`; replace their checkout-relative default with a pinned CLI dependency. RAG also needs an explicit model/sidecar download and integrity manifest.
- Anatomy, building and BIM use adjacent `engine/rete_wasm.js`; Lombardi shares Yasgui's worker. Use a versioned engine/worker artifact contract, not a copied Rust engine or a dependency on `../rete`.
- The WebGPU benchmark under `experiments/webgpu-coherence/` and `docs/webgpu.html` are distinct artifacts. Inventory both; do not assume one generates the other.
- Keep the published datasets in place. Repository migration should change application ownership, not data URLs.

Proposed destination layout: `rete-apps/apps/<app>/` owns that app's source, guide and tests; shared build/dependency metadata and the gallery live at the root. Reuse current app toolchains. Choose the remote and public hosting URL during destination setup; the plan does not assume either already exists.

## Implementation sequence

**Delivery record (2026-11-01):** Tasks 1 and 2 are complete. Task 3 is implemented and its CLI quickstart, SBOM and card commands were exercised against the published image. Task 5 has targeted corrections and the editorial checklist; its broader consolidation remains open. Task 4 has not started. First-delivery validation covers generated-page reproducibility, local links, retained fragments, page reachability, desktop/mobile navigation and social previews; cross-site migration checks are not applicable until Task 4. Unchecked items below retain the remaining roadmap and broader verification requirements.

### 1. Make navigation independent of publication

**Files:** `crates/docgen/src/main.rs`, its existing test module, `scripts/check_docs_links.py`, `scripts/tests/test_check_docs_links.py` only if additional link handling is needed.

- [x] Introduce an explicit registry of published Markdown pages with title/category metadata. Render every registered page regardless of sidebar visibility; register `graph-algorithms.md` as well.
- [x] Check registry completeness against root `docs/*.md`: every file is registered or explicitly excluded with a reason. Keep nested historical plans outside public-page discovery.
- [x] Keep a small sidebar definition referencing that registry. Keep the mobile drawer derived from the sidebar. Use an explicit curated learning order for Overview → Quickstart → Graph concepts → Playground; specialist pages get a parent/back link rather than arbitrary next steps through every app.
- [x] Replace the tests asserting that reading order equals the entire sidebar. Add behavior tests: a registered hidden page renders; missing pages are not linked; sidebar/registry IDs are unique; learning links resolve; a technical page has a usable parent.
- [x] Preserve heading IDs during rewrites with the existing explicit-ID support or alias anchors. Record page/fragment moves before changing them.
- [x] Check `rewrite_links` against local documents, external GitHub Markdown URLs, query strings and fragments. Its current string replacements must not turn an external source link into an HTML destination that does not exist.
- [x] Run `docker compose run --rm dev cargo test -p docgen` and the link-checker unit tests. Deliver this as a focused renderer change before cutting navigation entries.

### 2. Ship the smaller sidebar and useful entry pages

**Files:** the eight new `docs/*.md` entry pages listed above; `docs/index.md`; `crates/docgen/src/main.rs`; generated HTML.

- [x] Implement the four groups and 15 primary entries shown above. Put release procedures under Contributing and language-specific details under Language clients.
- [x] Populate entry pages with task descriptions and links using the disposition table. Keep `apps.md` local and functional before any external gallery exists.
- [x] Remove the duplicate giant documentation directory from Overview. Give it a short explanation, a suitable-use/limits paragraph, and links to try, build or integrate rete.
- [x] Replace the reader-facing footer's generator command with a quiet source/edit link; document generation belongs in Contributing.
- [x] Verify all 56 existing pages are reachable from an entry/reference page or an intentional compatibility page. No app submenus should reconstruct the old 93-link tree.
- [x] Render the site and inspect desktop, narrow mobile, keyboard navigation and JavaScript-disabled navigation. Check visible labels, active-page context and the drawer's contents.

### 3. Rewrite the main learning path and fix misleading claims

**Files:** `docs/getting-started.md`, `intro.md`, `playground-guide.md`, `scenario.md`, `dataset-cards.md`; new `dataset-card-reference.md`; `docs/contributing.md`; the factual correction targets below.

- [ ] Quickstart: use one small, reproducible fixture and one install route; build, query, inspect the answer, then offer publishing. Move source builds and test commands to Contributing; link to other install routes. Include shell-specific setup only where required to run the example.
- [ ] Playground: open one dataset, run a query, understand results/bytes fetched, then share. Link out to detailed card/media/API explanations.
- [ ] Graph concepts and SBOM: tell one connected story; explain the query result rather than inventorying syntax. Keep the extra variants reachable as recipes.
- [ ] Split the Dataset Cards user path from the full reference without dropping fields, recard rules, identity/provenance or old anchors.
- [ ] Resolve these concrete contradictions against current source/tests: football/subtitle JSON snapshots versus live graph reads; schema versus community-summary cost; compatibility feature exceptions and Cypher status; hosted-model data flow in `agent-frameworks.md`; outdated notebook count; overview/package versions; stale raw WASM build instructions; card placement before versus after data; physical community tiles described as both current and future in `SPEC.md`.
- [ ] Reconcile `hosting.md`'s HTTP-200 rejection claim with the bounded fallback described in `cli.md`; distinguish browser and native readers. Also check SPEC claims about split groups and query-time inference against the implemented features before editing normative text.
- [ ] Run the displayed quickstart and representative client/card examples in the supported toolchain. Check actual outputs. Compare before/after prose and headings, then review whether each retained paragraph helps the user's task.

### 4. Prepare rete-apps and migrate one experiment at a time

**Files:** new `../rete-apps/README.md`, app manifests/build configuration and `apps/<app>/`; matching source/guide/build references in rete; `docs/apps.md`.

- [ ] Establish destination repository ownership, supported build commands and public hosting location before any URL switch. Produce a local preview first.
- [ ] Start with Graph-map. Record its inputs, outputs, pinned CLI, datasets, licence and tests; make a fresh destination checkout build without `../rete` or ignored data/dev directories.
- [ ] Follow with Lance-RAG/Ask the graph and WebGPU. Record models, sidecars, engine/worker versions and all output pages explicitly. Keep core protocol tests in rete.
- [ ] Update imports, builder paths, app/guide backlinks and the gallery. Preserve old guide and runtime URLs with deliberate compatibility pages/redirects, including query strings and fragments; map changed fragments where needed.
- [ ] Keep old fragment IDs as alias anchors in compatibility pages, or extend the checker to understand the explicit migration map. The existing local checker does not follow JavaScript redirects.
- [ ] Update canonical/OG URLs and social preview ownership in `crates/docgen/src/main.rs`, `scripts/preview/inject_og.mjs` and `scripts/preview/docs_models.mjs`. Refresh affected cards and run `tests/gate/checks/check_social_previews.mjs` through its existing harness. That check skips missing HTML, so also verify every migration-map source and destination explicitly.
- [ ] Account for every old generated HTML/preview artifact: retain it intentionally, replace it with compatibility output, or retire it after link verification. Generators do not currently clean obsolete output automatically.
- [ ] Verify real app behavior and remote assets from the destination preview. Only remove old build ownership after the new host is live and old links reach it. Retain the previous hosted artifacts until that check passes.
- [ ] Apply the same process to the second wave; do not silently expand into File Explorer, Plaza, Yasgui or polyglot extraction.

### 5. Consolidate specialist and supporting documentation

**Files:** remaining `docs/*.md` in the table; root/client/crate READMEs; `CONTRIBUTING.md`; relevant `scripts/*/README.md` and `skills/` references.

- [ ] Give CLI/SPARQL/specification pages concise introductions and navigable reference structure; retain detailed contracts. Keep benchmarks/conformance linked and dated rather than prominent in the beginner path.
- [ ] Reduce SDK landing pages to install, first query, result interpretation and links to API details. Keep package README essentials because registry readers may never visit the site.
- [ ] Cover Java, Blender, MCP bundle, relay and Docker documentation too: put end-user setup first, leave ABI/build/release details in their owning references, and move dated performance/incident reports out of introductory flows. Preserve the concise R and crate README structure.
- [ ] Reconcile repeated guidance: R2 serving versus HF backup in `skills/README.md`; crates.io status in client skills/READMEs; named-graph support and `--in-place` in recard docs; aggregation/lazy-read claims across build/query skills; Tauri platform/file-association claims.
- [ ] Correct Java's “import-free” and whole-file-copy claims to distinguish its byte-array and range APIs. Distinguish the CLI README's `Content-Range` exposure recommendation from the reader's actual requirements.
- [ ] Replace obsolete language-specific release-tag instructions in `clients-dev.md` with the unified release policy. Give public readers reproducible alternatives for hosting/benchmark commands that currently reference ignored internal scripts, or label those measurements as internal.
- [ ] Move maintainer incident narratives out of user guides while keeping the current operational instruction and evidence link. Preserve historical changelog/spec/plan records rather than rewriting history as current advice.
- [ ] Add a short editorial checklist to `CONTRIBUTING.md`: identify audience/task, explain before enumerating, link to canonical detail, preserve limitations/anchors, and check claims against the version being documented.

### 6. Validate the result and hand it over

- [ ] Run `docker compose run --rm dev cargo test -p docgen`.
- [ ] Run `docker compose run --rm dev python3 -m unittest discover -s scripts/tests -p test_check_docs_links.py`.
- [ ] Generate with `docker compose run --rm dev cargo run -q -p docgen`, then run `docker compose run --rm dev python3 scripts/check_docs_links.py`.
- [ ] Check existing `scripts/release_check.sh docs` after generated artifacts are recorded as required by that drift check. Keep source, authored docs and generated artifacts in separate commits.
- [ ] Re-run browser/app checks only for apps and runtime assets touched. Engine/playground changes require the repository's full regression gate and canonical browser build; navigation prose alone does not require rebuilding WASM.
- [ ] Check cross-site redirects and deep links separately: the local HTML checker intentionally ignores external HTTP(S) destinations.
- [ ] Walk through four reader tasks from the homepage: run a first query, build/publish a file, choose a language client, and find an app. Target at most two navigation choices to the relevant starting page; verify on mobile too.
- [ ] Confirm 4 sidebar groups/15 primary entries, no orphaned retained pages, no broken local links/anchors, and no dependence on a removed app source. Review the prose against the writing rules, not just a word count.

## Disposition of all 56 documentation pages

“Move” means a staged destination after the app migration checks; until then the page remains published at its current URL. “Reference” means retained and reachable through a landing page, not deleted.

| Existing page | Proposed treatment / entry point |
|---|---|
| `index.md` | Rewrite as concise overview; remove duplicate sitemap. |
| `intro.md` | Keep Graph concepts; explain RDF through one connected example. |
| `getting-started.md` | Rewrite as Quickstart; move contributor/testing material. |
| `scenario.md` | Keep SBOM worked example; combine fragmented steps. |
| `playground-guide.md` | Keep primary interactive tutorial; shorten first-use path. |
| `plaza-guide.md` | Tools and integrations; defer app extraction. |
| `file-explorer.md` | Tools and integrations; preserve Tauri ownership. |
| `yasgui-guide.md` | Tools and integrations; defer shared-worker extraction. |
| `jupyterlite-guide.md` | Language clients / worked examples; retain notebooks initially. |
| `jslab-guide.md` | JavaScript worked example; optional later app move. |
| `atlas.md` | Featured worked example; keep initially, simplify feature inventory. |
| `ask-the-graph.md` | Move with Lance-RAG in first app wave. |
| `football.md` | Move with replay apps in second wave; correct JSON data-flow claim. |
| `subtitles-guide.md` | Move with timeline app; correct contradictory data-flow claim. |
| `anatomy-guide.md` | Move with app after shared-engine contract exists. |
| `building-guide.md` | Move with app; keep a GeoSPARQL example link. |
| `bim-pair-guide.md` | Move with app after shared-engine contract exists. |
| `lombardi-guide.md` | Move app later; retain notebook/example links and worker dependency. |
| `webgpu-guide.md` | Move first wave; clearly label experimental behavior. |
| `graph-map.md` | Move first wave with all map viewers/builders. |
| `neuro-showcase-guide.md` | Move second wave with cell manifest and mesh dependencies. |
| `cli.md` | Keep complete CLI reference; remove duplicated tutorial/history prose. |
| `sparql.md` | Keep complete language support reference with concise orientation. |
| `geosparql.md` | Query and analyze; explain one spatial/time query. |
| `shacl.md` | Query and analyze; explain validation result before full options. |
| `reasoning.md` | Query and analyze; distinguish query rewriting, materialization and coherence. |
| `federation.md` | Query and analyze; distinguish union, joins and SERVICE limits. |
| `manifest.md` | Build and publish; retain manifest/WAL semantics as reference. |
| `semantic-zoom.md` | Keep concept/technical guide; separate schema and community read costs. |
| `compatibility.md` | Keep primary reference; lead with compatibility matrix and exceptions. |
| `python.md` | Language clients; first query plus canonical API links. |
| `python-build-tutorial.md` | Python worked example; link from client page, not global nav. |
| `javascript.md` | Language clients; distinguish Node, worker and main-thread usage. |
| `comunica.md` | Tools and integrations; retain RDF/JS contract and small example. |
| `r.md` | Language clients; concise installation and first query. |
| `blender.md` | Tools and integrations; core client remains in rete. |
| `agents.md` | Tools and integrations; choose MCP/plugin/local path by task. |
| `agent-frameworks.md` | Agent integration reference/recipe; correct hosted-model privacy claim. |
| `fallacies.md` | Short agent worked example; longer experiment material beside its source. |
| `dataset-cards.md` | Short read/create/update guide; extract exhaustive field reference. |
| `hosting.md` | Build and publish; canonical Range/CORS guidance. |
| `interop.md` | Build and publish / integration reference; distinguish export, endpoint, federation. |
| `media-companions.md` | Publishing reference; link from relevant media/SQL tasks. |
| `release.md` | Contributing only; correct sidebar label and remove duplicated status history. |
| `topic-modeling.md` | Keep short analysis recipe; move long app pipeline with Graph-map. |
| `multi-criteria.md` | Keep CLI analysis worked example. |
| `graph-algorithms.md` | Register for rendering; link from analysis/Python, retain algorithm reference. |
| `architecture.md` | Technical reference; reconcile obsolete future-work statements. |
| `SPEC.md` | Preserve complete normative format reference. |
| `rust-api.md` | Language clients / technical reference; keep embedding example. |
| `browser.md` | Technical API reference; distinguish public JS client from low-level WASM. |
| `clients-dev.md` | Contributing; preserve client build/release details. |
| `parallel-browser.md` | Experimental technical reference; retain threading/hosting limitations. |
| `data-engineering.md` | Advanced build/publish guide; separate conversion/large-build/VKG choices. |
| `BENCHMARK.md` | Technical reference; retain reproducibility, measurements and scope. |
| `conformance.md` | Technical reference; retain dated coverage/limits, move incident narrative. |

**First delivery:** renderer separation, four-section sidebar, useful entry pages and the rewritten first-use path. App relocation is a subsequent delivery, so the documentation improvement does not wait for a new hosting setup.
