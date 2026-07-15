# Automatic Playground PR Previews and Stable Dataset Repair

**Status:** Approved design, pending implementation plan

**Date:** 2026-07-15

**Branch:** `main`

## Decision

Publish an isolated playground preview automatically for every open pull request whose
head branch belongs to `caviri/rete`. A preview is published only after the exact PR
head commit passes CI. The production playground presents a version selector containing
production and every currently available open-PR preview. Updating a PR replaces its
previous preview, and closing or merging the PR removes its preview immediately.

Preview files are served from the existing `preview.graphplaza.com` Cloudflare R2
origin. That origin and its CORS policy are already configured. Production remains on
GitHub Pages at `https://caviri.github.io/rete/`; preview JavaScript therefore does not
share an origin, browser storage, or caches with production.

The same work repairs the Barcelona (`bcn`) release dataset. Its hosted and local
`.rete` artifact has experimental format byte `0x04`, while the 1.0 reader intentionally
accepts only stable format generation `0x05`. The existing local N-Triples source is
rebuilt with the release writer, verified, and republished at the existing URL.

## Goals

- Give every same-repository PR a browser-accessible playground for its latest commit.
- Publish only tested artifacts and display the exact commit identity in the preview.
- Keep R2 publishing credentials out of all workflows that execute PR-controlled code.
- Let users switch between production and open PRs without losing the selected dataset,
  example, query mode, loading strategy, or other fragment-backed state.
- Remove closed, merged, and superseded previews rather than accumulating old versions.
- Keep preview failures and GitHub API failures from impairing the production playground.
- Repair BCN for the stable 1.0 format and prevent another incompatible remote object
  from entering the catalog unnoticed.
- Exercise preview publication, selection, cleanup, and live querying through the
  repository's Docker Compose/devcontainer test path.

## Non-goals

- Fork pull requests do not receive automatic previews. Their code must never reach a
  workflow with R2 write credentials.
- Closed PR previews and superseded commit previews are not retained as archives.
- The first version selector contains production and open PRs only. Historical release
  tags can be added later without changing the preview storage model.
- PRs do not publish large dataset copies. They use the stable production catalog and
  deterministic local fixtures; a future format feature can add a small fixture without
  duplicating the remote corpus.
- Preview publication does not replace the GitHub Pages production workflow.
- This work does not add read support for experimental `.rete` format `0x04`. Pre-1.0
  files are rebuilt from source as required by the stable-format policy.

## Public URLs and Object Layout

Each preview is immutable at an exact head SHA while it is active:

```text
https://preview.graphplaza.com/pr-<number>/<40-char-head-sha>/playground.html
https://preview.graphplaza.com/pr-<number>/<40-char-head-sha>/rete_wasm_async.js
https://preview.graphplaza.com/pr-<number>/<40-char-head-sha>/rete_wasm_async.wasm
https://preview.graphplaza.com/pr-<number>/<40-char-head-sha>/coi-serviceworker.js
https://preview.graphplaza.com/pr-<number>/<40-char-head-sha>/wasm-build.json
```

The publisher accepts only this fixed file allowlist, rejects symlinks and unexpected
paths, and imposes an artifact size ceiling. HTML and metadata use `Cache-Control:
no-store`; SHA-addressed JavaScript and WASM use immutable caching. Preview HTML carries
`robots` `noindex,nofollow` metadata and an unmistakable PR-preview label.

There is no mutable `current` directory and no shared registry object. The production
selector obtains open PR metadata from GitHub, derives the exact URL from each PR number
and head SHA, and includes only URLs whose `playground.html` answers a successful `HEAD`.
This avoids concurrent writers corrupting a central versions manifest.

## Workflow Boundaries

### 1. Unprivileged PR build

The normal `pull_request` CI remains unprivileged. A preview-build job:

1. Runs only when `github.event.pull_request.head.repo.full_name` is `caviri/rete`.
2. Checks out the exact `pull_request.head.sha`, not the synthetic merge ref.
3. Uses the pinned devcontainer image and the repository WASM/playground build scripts.
4. Runs focused static and browser checks against the generated playground.
5. Sets `GITHUB_SHA` and generated provenance to the full PR head SHA and
   `RETE_BUILD_STAMP` to its canonical 12-character abbreviation.
6. Uploads the allowlisted static files as a short-retention GitHub Actions artifact.

The broader CI continues testing the merge result. Publication requires the entire CI
workflow, including the head preview build and merge-result gates, to succeed.

### 2. Trusted publication

A `workflow_run` workflow defined on the default branch handles successful completed CI
runs. It has access only to bucket-scoped credentials for the preview R2 bucket. It does
not execute files, scripts, actions, or commands from the PR checkout.

Before uploading, the publisher:

1. Requires the source event to be `pull_request` and the CI conclusion to be success.
2. Resolves the associated PR through the GitHub API.
3. Requires the PR to be open and its head repository to equal `caviri/rete`.
4. Requires the PR's current head SHA to match the preview artifact name and
   `wasm-build.json` provenance.
5. Downloads only the expected artifact from the completed workflow run.
6. Validates file names, regular-file status, sizes, the full provenance SHA, and the
   matching 12-character build stamp.

It uploads the complete new SHA prefix before deleting every older prefix under the same
PR number. A failure before the upload completes leaves the previous working preview in
place. A stale run exits successfully without publishing or deleting anything.

The publisher uses a GitHub `playground-preview` environment restricted to the default
branch workflow and R2 credentials that cannot write to the production dataset bucket.
The environment has no manual reviewer because publication is automatic.

### 3. Close and merge cleanup

A metadata-only `pull_request_target` workflow handles the `closed` event. It uses the
trusted default-branch workflow, never checks out or executes PR code, and deletes the
complete `pr-<number>/` prefix. The same cleanup operation is exposed through a manual
`workflow_dispatch` repair path that requires an explicit PR number.

Cleanup is idempotent: deleting an already absent prefix succeeds. Preview HTML is never
edge-cached, so a deleted preview stops loading as soon as R2 deletion completes. Cached
SHA-addressed assets cannot bootstrap a preview without its deleted HTML.

## Playground Version Selector

The top bar gains a compact, keyboard-accessible selector with these labels:

```text
Production · b562585
PR #72 · Add streaming parser · 91ac238
PR #74 · Playground map changes · c04d112
```

Production HTML receives the exact 12-character `window.RETE_BUILD` SHA from the Pages
workflow. Preview HTML receives the same canonical abbreviation of its head SHA plus a
structured `window.RETE_PREVIEW` value holding the PR number, title, and full head SHA.
The current entry is selected and preview pages show a visible `PR preview` badge.

On startup, the selector:

1. Adds production immediately; preview discovery never blocks editor initialization.
2. Fetches up to 100 open PRs from the public GitHub API.
3. Filters to `caviri/rete` head repositories and safely renders titles with `textContent`.
4. Derives each exact R2 URL and checks it with `HEAD`.
5. Adds only successfully published previews.
6. Caches discovery in session storage for five minutes while honoring a refreshed PR
   head SHA on the next discovery.

Changing versions navigates to the selected `playground.html` while preserving
`location.hash`. Query-string deployment cache busters are not copied. If GitHub is rate
limited, R2 is unavailable, or discovery returns malformed data, the selector remains a
working production-only control and records a non-fatal diagnostic in the console.

## Dataset Compatibility and BCN Repair

A range probe of every current remote catalog URL and shard found 54 objects: 53 already
carry stable header `RETE 0x05`, and only `bcn` carries `RETE 0x04`.

BCN is repaired from the existing `data/bcn/bcn.nt` source with the release CLI and the
catalog's required build options: type pyramid, text index, and embedded dataset card.
Before publication, verification requires:

- header magic `RETE` and version byte `0x05`;
- `rete inspect` success and expected non-zero graph/card/index metadata;
- successful execution of all four BCN catalog queries against the local candidate;
- a browser open and representative PDF/media query against a temporary candidate URL;
- HTTP Range, `Content-Range`, `Content-Length`, and CORS visibility checks.

After these checks, the verified object replaces `bcn/bcn.rete`. R2 object replacement
must expose either the old complete object or the new complete object, never a partial
upload. The hosted header and all four browser queries are rechecked after publication.

A lightweight live compatibility check extracts every remote URL and shard from the
catalog, requests bytes `0-4`, and requires `RETE 0x05`. It uses bounded network retries
and runs in the post-deploy/scheduled live tier, not deterministic PR CI. Its report names
the dataset, URL, status, magic, and version for every failure.

## Deployed-Site Verification

The existing Pages verifier continues requiring the exact deployed production SHA and a
real lazy R2 query. It is aligned with the established browser checks by waiting for
playground and dataset initialization and using the shared bounded query retry helper.
Retries do not weaken assertions: success still requires the exact build, non-zero rows,
no rendered error, and no browser page or console errors.

Preview verification uses the same contract against the exact R2 preview URL before an
older preview is deleted. A failed smoke check leaves the previous preview active and
fails publication.

## Testing

### Deterministic tests

- Node tests for canonical preview URL construction, same-repository filtering, title
  escaping, current-version selection, five-minute caching, and hash preservation.
- Node tests for artifact allowlisting, provenance/SHA validation, stale-run rejection,
  prefix selection, and idempotent cleanup planning.
- Playwright tests with mocked GitHub and preview responses for dropdown rendering,
  production fallback, keyboard use, navigation, unavailable previews, and API failure.
- Static workflow-policy tests proving the unprivileged job has no secrets or write
  permissions and the privileged workflows never checkout a PR ref.
- Existing catalog-matrix and generated-playground freshness checks.

### Docker Compose and hosted tests

- Build the preview artifact with the repository devcontainer service.
- Run Chromium and Firefox playground gates through the optional Compose services.
- Serve a generated preview locally and verify its exact stamp and PR badge.
- Exercise the trusted publisher in dry-run mode against a fake S3 endpoint or isolated
  test prefix, including upload-before-delete and close cleanup.
- Run the exact hosted preview smoke check after R2 upload.
- Run the remote `RETE 0x05` inventory check and the four BCN examples in the optional
  live catalog tier.

## Failure Handling and Operations

- A failed or cancelled CI run publishes nothing.
- A stale successful run publishes nothing and cannot delete the newer preview.
- An upload or smoke-test failure retains the previous preview.
- A cleanup retry is safe and can be dispatched manually.
- A GitHub API failure degrades the selector to production only.
- A preview-host failure affects neither GitHub Pages nor normal playground startup.
- R2 credentials are scoped to the preview bucket and rotated without rebuilding the
  site; production dataset credentials are separate.
- Workflow logs identify the PR, expected SHA, object prefix, uploaded allowlist, smoke
  result, and cleanup result without printing credentials.

## Acceptance Criteria

- Opening or updating a same-repository PR automatically publishes its latest exact head
  SHA after all CI gates pass.
- Production and preview pages display their exact build identity.
- Production lists every successfully published open same-repository PR and no fork,
  failed, stale, closed, or unavailable preview.
- Switching between versions preserves the current playground fragment state.
- A superseded preview is deleted only after its replacement passes the hosted smoke
  check.
- Closing or merging a PR removes its selector entry and R2 prefix; repeated cleanup is
  harmless.
- No PR-controlled process receives R2 credentials or privileged token permissions.
- Preview code is isolated from the production origin, marked `noindex`, and visibly
  identified as experimental.
- The Pages verifier tolerates initialization/network transients but still enforces the
  exact build, successful query result, and clean browser diagnostics.
- Hosted `bcn.rete` uses header `RETE 0x05`, passes all four catalog examples, and renders
  its representative media/PDF result.
- The live header inventory reports all 54 current remote objects as stable `0x05`.
- Deterministic, Compose browser, workflow-policy, cleanup, and hosted smoke tests pass.
