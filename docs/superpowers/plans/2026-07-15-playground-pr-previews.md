# Automatic Playground PR Previews Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically publish every green same-repository PR playground to the existing `preview.graphplaza.com` R2 origin and let users switch between production and active previews.

**Architecture:** An unprivileged PR job builds the exact head SHA and uploads a fixed static artifact. Trusted default-branch workflows validate and publish that artifact with bucket-scoped credentials, smoke-test it, and delete superseded or closed PR prefixes. A self-contained browser module discovers open PRs from GitHub, probes deterministic preview URLs, and populates a non-blocking version selector.

**Tech Stack:** GitHub Actions, Docker/devcontainer, Python 3.11 `unittest` plus boto3 at deployment time, browser JavaScript, Playwright 1.49, Cloudflare R2 S3 API.

## Global Constraints

- Build and test through `compose.yaml` and `.devcontainer/Dockerfile`; do not rely on host Rust/WASM tooling.
- Only PRs whose head repository is exactly `caviri/rete` receive previews.
- PR-controlled jobs receive no R2 credentials and no write permissions.
- Preview objects live under `pr-<number>/<40-character-head-sha>/` on `preview.graphplaza.com`.
- Publish a new SHA completely and smoke-test it before deleting the previous SHA.
- Closing or merging deletes the complete PR prefix immediately and idempotently.
- Production remains on GitHub Pages and works when GitHub or preview discovery fails.
- Preserve `location.hash` when switching versions; do not copy query-string cache busters.
- Visible build stamps use 12 characters; provenance and object paths use the full 40-character SHA.
- Commit without a coauthor trailer and preserve unrelated untracked files.

---

### Task 1: Pure Preview Discovery Module

**Files:**
- Create: `web/playground-src/versions.js`
- Create: `tests/gate/checks/test_versions.mjs`

**Interfaces:**
- Produces: `window.RETE_PLAYGROUND_VERSIONS` with `previewUrl(pr)`, `versionHref(url, hash)`, `eligiblePull(pr)`, `discoverPreviews(options)`, and `initVersionPicker(options)`.
- Consumed by: Task 2 calls `initVersionPicker`; Node and browser tests use the pure helpers.

- [ ] **Step 1: Write the failing Node contract test**

Evaluate the classic script in a fake `window` and assert exact URL/filter behavior:

```js
assert.equal(api.previewUrl({ number: 72, head: { sha: SHA } }),
  `https://preview.graphplaza.com/pr-72/${SHA}/playground.html`);
assert.equal(api.versionHref("https://preview.graphplaza.com/x/playground.html", "#dataset=bcn&ex=3"),
  "https://preview.graphplaza.com/x/playground.html#dataset=bcn&ex=3");
assert.equal(api.eligiblePull({ head: { repo: { full_name: "caviri/rete" }, sha: SHA } }), true);
assert.equal(api.eligiblePull({ head: { repo: { full_name: "fork/rete" }, sha: SHA } }), false);
```

Use fake `fetch` and session storage to prove only successful `HEAD` previews are returned, cached discovery avoids a second GitHub request for five minutes, malformed payloads return `[]`, and PR titles remain plain strings.

- [ ] **Step 2: Run the test to verify it fails**

```bash
docker compose run --rm gate bash -lc 'npm ci --no-audit --no-fund && node checks/test_versions.mjs'
```

Expected: FAIL because `web/playground-src/versions.js` does not exist.

- [ ] **Step 3: Implement the minimal classic-script module**

Create this public boundary and implement the cache/GitHub/HEAD details behind it:

```js
(function (root) {
  "use strict";
  const REPO = "caviri/rete";
  const API = `https://api.github.com/repos/${REPO}/pulls?state=open&per_page=100`;
  const PREVIEW = "https://preview.graphplaza.com";
  const CACHE_KEY = "retePreviewVersionsV1";
  const CACHE_MS = 5 * 60 * 1000;

  function eligiblePull(pr) {
    return !!(pr && pr.number > 0 && /^[0-9a-f]{40}$/i.test(pr.head && pr.head.sha || "")
      && pr.head.repo && pr.head.repo.full_name === REPO);
  }
  function previewUrl(pr) {
    return `${PREVIEW}/pr-${pr.number}/${pr.head.sha}/playground.html`;
  }
  function versionHref(url, hash) { return `${url}${hash || ""}`; }
  async function discoverPreviews(options = {}) {
    const fetcher = options.fetch || root.fetch.bind(root);
    const storage = options.storage === undefined ? root.sessionStorage : options.storage;
    const now = options.now || Date.now;
    try {
      const cached = storage && JSON.parse(storage.getItem(CACHE_KEY) || "null");
      if (cached && cached.expires > now() && Array.isArray(cached.previews)) return cached.previews;
      const response = await fetcher(API, { headers: { Accept: "application/vnd.github+json" } });
      if (!response.ok) return [];
      const pulls = await response.json();
      if (!Array.isArray(pulls)) return [];
      const previews = (await Promise.all(pulls.filter(eligiblePull).map(async (pr) => {
        const url = previewUrl(pr);
        const probe = await fetcher(url, { method: "HEAD", cache: "no-store" });
        return probe.ok ? { number: pr.number, title: String(pr.title || ""), sha: pr.head.sha, url } : null;
      }))).filter(Boolean);
      if (storage) storage.setItem(CACHE_KEY, JSON.stringify({ expires: now() + CACHE_MS, previews }));
      return previews;
    } catch (_error) { return []; }
  }
  async function initVersionPicker(options = {}) {
    const doc = options.document || root.document;
    const location = options.location || root.location;
    const select = doc && doc.getElementById("versionSelect");
    if (!select) return [];
    const current = root.RETE_PREVIEW;
    const base = current && current.baseSha ? current.baseSha : root.RETE_BUILD;
    select.options[0].textContent = `Production${base ? " · " + String(base).slice(0, 7) : ""}`;
    const previews = await discoverPreviews(options);
    for (const preview of previews) {
      const option = doc.createElement("option");
      option.value = preview.url;
      option.textContent = `PR #${preview.number} · ${preview.title} · ${preview.sha.slice(0, 7)}`;
      select.appendChild(option);
    }
    if (current) select.value = previewUrl({ number: current.number, head: { sha: current.headSha } });
    select.onchange = () => location.assign(versionHref(select.value, location.hash));
    return previews;
  }

  root.RETE_PLAYGROUND_VERSIONS = {
    eligiblePull, previewUrl, versionHref, discoverPreviews, initVersionPicker,
  };
})(window);
```

Use `options.fetch`, `options.storage`, `options.now`, `options.location`, and `options.document` injection. Catch discovery/cache errors and return `[]`; never throw into playground boot.

- [ ] **Step 4: Run the Node test to verify it passes**

Run Step 2 again. Expected: PASS with a JSON verdict for discovery, filtering, caching, and URLs.

- [ ] **Step 5: Commit the module**

```bash
git add web/playground-src/versions.js tests/gate/checks/test_versions.mjs
git commit -m "feat(playground): add preview version discovery"
```

### Task 2: Version Selector UI and Generated Playground

**Files:**
- Modify: `web/playground.template.html`
- Modify: `web/playground-src/styles.css`
- Modify: `web/playground-src/app.js`
- Modify: `scripts/build_playground.py`
- Modify: `tests/gate/run.mjs`
- Create: `tests/gate/checks/check_version_picker.mjs`
- Regenerate: `docs/playground.html`

**Interfaces:**
- Consumes: `window.RETE_PLAYGROUND_VERSIONS.initVersionPicker` from Task 1.
- Produces: `#versionSelect`, `#previewBadge`, `window.RETE_PREVIEW`, and embedded `versions.js`.

- [ ] **Step 1: Write the failing Playwright selector test**

Route the GitHub pulls endpoint to one canonical PR and one fork, route the canonical preview `HEAD` to 200, open local playground state `#dataset=bcn&load=lazy&mode=sparql&ex=3`, and assert:

```js
await page.waitForFunction(() => document.querySelectorAll("#versionSelect option").length === 2);
const labels = await page.locator("#versionSelect option").allTextContents();
assert.match(labels[0], /^Production/);
assert.match(labels[1], /PR #72 .* 91ac238/);
await page.selectOption("#versionSelect", { index: 1 });
await page.waitForURL(/preview\.graphplaza\.com.*#dataset=bcn&load=lazy&mode=sparql&ex=3/);
```

In a second context return GitHub 500 and assert the editor boots with one production option and no rendered error.

- [ ] **Step 2: Run the browser test to verify it fails**

```bash
docker compose run --rm gate bash -lc 'npm ci --no-audit --no-fund && node run.mjs --local --only=check_version_picker'
```

Expected: FAIL because `#versionSelect` is missing.

- [ ] **Step 3: Add template, styles, build inclusion, and boot wiring**

Add the top-bar control and metadata before `.top-actions`:

```html
<label class="version-picker" for="versionSelect">
  <span>Version</span>
  <select id="versionSelect" aria-label="Playground version">
    <option value="https://caviri.github.io/rete/playground.html">Production</option>
  </select>
</label>
<span id="previewBadge" class="preview-badge hidden">PR preview</span>
<script>
window.RETE_BUILD = "__BUILD_VERSION__";
window.RETE_PREVIEW = null;
</script>
```

Use absolute production documentation links. Add responsive CSS so the selector truncates on desktop/tablet and does not overflow at 560px. Add `VERSIONS_JS = SRC / "versions.js"` and `__PLAYGROUND_VERSIONS_JS__` replacement/placeholder validation in `build_playground.py`.

Call discovery without awaiting it in `boot()`:

```js
try {
  const versions = window.RETE_PLAYGROUND_VERSIONS;
  if (versions) Promise.resolve(versions.initVersionPicker())
    .catch((e) => console.warn("preview discovery", e));
} catch (e) { console.warn("preview discovery", e); }
```

Add `versions.js` to G0 parsing and `check_version_picker` to G2 as a local test.

- [ ] **Step 4: Regenerate in the devcontainer**

```bash
docker compose run --rm -e RETE_BUILD_STAMP=1.0.0-rc.1 dev uv run python scripts/build_playground.py
```

Expected: generated HTML contains the selector and no source placeholder.

- [ ] **Step 5: Run static and browser tests**

```bash
docker compose run --rm gate bash -lc 'npm ci --no-audit --no-fund && node run.mjs fast'
docker compose run --rm gate bash -lc 'npm ci --no-audit --no-fund && node run.mjs --local --only=check_version_picker'
```

Expected: both gates are green.

- [ ] **Step 6: Commit the UI**

```bash
git add web/playground.template.html web/playground-src/styles.css web/playground-src/app.js web/playground-src/versions.js scripts/build_playground.py tests/gate/run.mjs tests/gate/checks/check_version_picker.mjs docs/playground.html
git commit -m "feat(playground): add version selector"
```

### Task 3: Trusted Preview Artifact Store

**Files:**
- Create: `scripts/preview_store.py`
- Create: `scripts/tests/test_preview_store.py`

**Interfaces:**
- Produces CLI commands `upload --root PATH --pr NUMBER --head-sha SHA --base-sha SHA --title TITLE` and `cleanup --pr NUMBER [--keep-sha SHA]`.
- Reads: `PREVIEW_S3_API_ENDPOINT`, `PREVIEW_ACCESS_KEY_ID`, `PREVIEW_SECRET_ACCESS_KEY`, and `PREVIEW_BUCKET`.
- Allows only: `playground.html`, `rete_wasm_async.js`, `rete_wasm_async.wasm`, `coi-serviceworker.js`, `wasm-build.json`.

- [ ] **Step 1: Write failing Python unit tests**

Use `unittest`, temporary files, and a fake S3 client:

```python
self.assertEqual(store.object_prefix(72, SHA), f"pr-72/{SHA}/")
self.assertEqual(store.preview_url(72, SHA),
    f"https://preview.graphplaza.com/pr-72/{SHA}/playground.html")
with self.assertRaisesRegex(ValueError, "40-character"):
    store.object_prefix(72, "bad")
```

Assert `validate_artifact` rejects missing, extra, symlinked, oversized, mismatched `window.RETE_BUILD`, and mismatched `wasm-build.json` inputs. Assert `inject_preview_metadata` replaces exactly one `window.RETE_PREVIEW = null;` using `json.dumps`. Prove cleanup deletes old SHA keys, preserves `keep_sha`, batches at 1,000, and is idempotent.

- [ ] **Step 2: Run tests to verify they fail**

```bash
docker compose run --rm dev python3 -m unittest scripts.tests.test_preview_store -v
```

Expected: FAIL because `scripts/preview_store.py` is absent.

- [ ] **Step 3: Implement validation, upload, and cleanup**

Implement these entry points:

```python
ALLOWED = {
    "playground.html", "rete_wasm_async.js", "rete_wasm_async.wasm",
    "coi-serviceworker.js", "wasm-build.json",
}
MAX_ARTIFACT_BYTES = 64 * 1024 * 1024

def object_prefix(pr_number: int, head_sha: str) -> str:
    if pr_number < 1 or not re.fullmatch(r"[0-9a-f]{40}", head_sha):
        raise ValueError("PR must be positive and SHA must be 40-character lowercase hex")
    return f"pr-{pr_number}/{head_sha}/"

def preview_url(pr_number: int, head_sha: str) -> str:
    return f"https://preview.graphplaza.com/{object_prefix(pr_number, head_sha)}playground.html"

def validate_artifact(root: Path, head_sha: str) -> list[Path]:
    files = sorted(path for path in root.iterdir() if path.is_file() and not path.is_symlink())
    if {path.name for path in files} != ALLOWED:
        raise ValueError("artifact must contain exactly the preview allowlist")
    if sum(path.stat().st_size for path in files) > MAX_ARTIFACT_BYTES:
        raise ValueError("artifact exceeds 64 MiB")
    manifest = json.loads((root / "wasm-build.json").read_text(encoding="utf-8"))
    if manifest.get("gitCommit") != head_sha:
        raise ValueError("wasm-build.json does not match head SHA")
    html = (root / "playground.html").read_text(encoding="utf-8")
    if f'window.RETE_BUILD = "{head_sha[:12]}";' not in html:
        raise ValueError("playground build stamp does not match head SHA")
    return files

def inject_preview_metadata(html: str, metadata: dict) -> str:
    marker = "window.RETE_PREVIEW = null;"
    if html.count(marker) != 1:
        raise ValueError("preview metadata marker must occur exactly once")
    return html.replace(marker, f"window.RETE_PREVIEW = {json.dumps(metadata, separators=(',', ':'))};")

def upload_preview(client, bucket: str, root: Path, metadata: dict) -> str:
    files = validate_artifact(root, metadata["headSha"])
    prefix = object_prefix(metadata["number"], metadata["headSha"])
    with tempfile.TemporaryDirectory() as directory:
        stage = Path(directory)
        for source in files:
            shutil.copy2(source, stage / source.name)
        page = stage / "playground.html"
        page.write_text(inject_preview_metadata(page.read_text(encoding="utf-8"), metadata), encoding="utf-8")
        for source in sorted(stage.iterdir()):
            immutable = source.suffix in {".js", ".wasm"} and source.name != "coi-serviceworker.js"
            client.upload_file(str(source), bucket, prefix + source.name, ExtraArgs={
                "ContentType": mimetypes.guess_type(source.name)[0] or "application/octet-stream",
                "CacheControl": "public,max-age=31536000,immutable" if immutable else "no-store",
            })
    return preview_url(metadata["number"], metadata["headSha"])

def cleanup_preview(client, bucket: str, pr_number: int, keep_sha: str | None = None) -> int:
    prefix = f"pr-{pr_number}/"
    token = None
    deleted = 0
    while True:
        request = {"Bucket": bucket, "Prefix": prefix}
        if token: request["ContinuationToken"] = token
        page = client.list_objects_v2(**request)
        keys = [item["Key"] for item in page.get("Contents", [])
                if not keep_sha or not item["Key"].startswith(prefix + keep_sha + "/")]
        for start in range(0, len(keys), 1000):
            batch = keys[start:start + 1000]
            client.delete_objects(Bucket=bucket, Delete={"Objects": [{"Key": key} for key in batch]})
            deleted += len(batch)
        if not page.get("IsTruncated"): break
        token = page["NextContinuationToken"]
    return deleted
```

Use `upload_file` with correct content types. Set `no-store` for HTML/JSON/service-worker files and `public,max-age=31536000,immutable` for SHA-scoped JS/WASM. Import boto3 only inside `make_client`; never load `.env` in CI.

- [ ] **Step 4: Run tests to verify they pass**

Run Step 2 again. Expected: all preview-store tests pass.

- [ ] **Step 5: Commit the store**

```bash
git add scripts/preview_store.py scripts/tests/test_preview_store.py
git commit -m "build(preview): add trusted R2 publisher"
```

### Task 4: Secure Automatic GitHub Workflows

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `.github/workflows/preview-publish.yml`
- Create: `.github/workflows/preview-cleanup.yml`
- Create: `scripts/tests/test_preview_workflows.py`

**Interfaces:**
- Consumes: Task 3's CLI and the exact preview artifact.
- Produces: artifact `playground-preview-<full-head-sha>` and automatic R2 publication/cleanup.

- [ ] **Step 1: Write failing workflow policy tests**

Use text-level `unittest` assertions including:

```python
self.assertIn("github.event.pull_request.head.repo.full_name == 'caviri/rete'", ci)
self.assertIn("workflow_run", publish)
self.assertIn("playground-preview", publish)
self.assertNotIn("pull_request_target", publish)
self.assertIn("pull_request_target", cleanup)
self.assertIn("types: [closed]", cleanup)
self.assertNotRegex(cleanup, r"ref:\s*\$\{\{.*head")
```

Also require the CI preview job to have `permissions: contents: read`, no secret references, and exact-head checkout. Require privileged workflows to use environment `playground-preview` and never checkout a PR ref.

- [ ] **Step 2: Run tests to verify they fail**

```bash
docker compose run --rm dev python3 -m unittest scripts.tests.test_preview_workflows -v
```

Expected: FAIL because preview workflows/jobs are absent.

- [ ] **Step 3: Add the unprivileged exact-head build job**

Add this PR-only boundary to CI, followed by pinned-image load, `scripts/build_wasm.sh`, fixed-file staging, and one-day artifact upload:

```yaml
preview:
  if: github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == 'caviri/rete'
  permissions:
    contents: read
  needs: image
  steps:
    - uses: actions/checkout@v6
      with:
        ref: ${{ github.event.pull_request.head.sha }}
    - uses: actions/upload-artifact@v7
      with:
        name: playground-preview-${{ github.event.pull_request.head.sha }}
        path: target/playground-preview
        retention-days: 1
```

Pass the full head SHA as `GITHUB_SHA` and its first 12 characters as `RETE_BUILD_STAMP` inside the container.

- [ ] **Step 4: Add trusted publish and cleanup workflows**

`preview-publish.yml` triggers on completed `CI`, requires success, resolves the associated PR, requires open state/canonical head/current SHA, downloads exactly `playground-preview-$HEAD_SHA`, installs boto3, uploads, installs Playwright, runs `check_deployed.mjs` against the exact preview URL, then calls cleanup with `--keep-sha "$HEAD_SHA"`.

`preview-cleanup.yml` triggers only on `pull_request_target` `closed`, checks out the default branch, installs boto3, and calls `cleanup --pr "$PR_NUMBER"`. Both privileged jobs use environment `playground-preview` and the four `PREVIEW_*` secrets. Neither checks out the PR ref nor executes artifact files.

- [ ] **Step 5: Run workflow policy checks**

```bash
docker compose run --rm dev python3 -m unittest scripts.tests.test_preview_workflows -v
```

Expected: policy tests pass.

- [ ] **Step 6: Commit workflows**

```bash
git add .github/workflows/ci.yml .github/workflows/preview-publish.yml .github/workflows/preview-cleanup.yml scripts/tests/test_preview_workflows.py
git commit -m "ci: publish isolated playground previews"
```

### Task 5: Harden Hosted Verification

**Files:**
- Modify: `tests/gate/checks/check_deployed.mjs`
- Modify: `.github/workflows/pages.yml`
- Modify: `scripts/tests/test_preview_workflows.py`

**Interfaces:**
- Consumes: `runWithRetry` from `_util.mjs` and `scripts/check_dataset_catalog.py`.
- Produces: hosted JSON verdict with `tries`, retaining exact-SHA and clean-console assertions.

- [ ] **Step 1: Add failing static contract assertions**

Require `check_deployed.mjs` to import/call `runWithRetry`; require Pages to run `python3 scripts/check_dataset_catalog.py --all` after deployment.

- [ ] **Step 2: Run policy test to verify it fails**

Run Task 4 Step 5. Expected: FAIL on retry/catalog requirements.

- [ ] **Step 3: Align the verifier with established live checks**

Wait for the selected example and enabled Run control, allow the same short initialization settle used by `check_worldcup.mjs`, and replace one-click polling with:

```js
const out = await runWithRetry(page, { tries: 3, steps: 60, stepMs: 1000 });
```

Keep exact build equality, positive rows, no error block, and zero page/console errors mandatory. Include `tries` in output. Add the existing stable-format/Range/CORS/lock inventory as a Pages live gate.

- [ ] **Step 4: Reproduce hosted verification through Compose**

```bash
docker compose run --rm -e DEPLOYED_URL=https://caviri.github.io/rete/ -e EXPECTED_SHA=b56258536e2c gate bash -lc 'npm ci --no-audit --no-fund && node checks/check_deployed.mjs'
```

Expected: exact build and positive worldcup rows, potentially after a retry. The separate catalog inventory remains red only for BCN until the dataset plan runs.

- [ ] **Step 5: Commit verifier hardening**

```bash
git add tests/gate/checks/check_deployed.mjs .github/workflows/pages.yml scripts/tests/test_preview_workflows.py
git commit -m "test(pages): harden deployed playground checks"
```

### Task 6: Full Preview Verification

**Files:**
- Modify only if verification exposes a focused defect in Tasks 1-5 files.

**Interfaces:**
- Verifies all preview deliverables together.

- [ ] **Step 1: Run deterministic tests**

```bash
docker compose run --rm dev python3 -m unittest discover -s scripts/tests -v
docker compose run --rm gate bash -lc 'npm ci --no-audit --no-fund && node run.mjs fast'
```

Expected: all Python tests and G0 checks pass.

- [ ] **Step 2: Run selector tests in Chromium and Firefox**

```bash
docker compose run --rm gate bash -lc 'npm ci --no-audit --no-fund && node run.mjs --local --only=check_version_picker'
docker compose --profile extended-tests run --rm gate-firefox bash -lc 'npm ci --no-audit --no-fund && node run.mjs --local --only=check_version_picker'
```

Expected: both browsers pass discovery, fallback, and hash-preserving navigation.

- [ ] **Step 3: Run repository release checks**

```bash
docker compose run --rm -e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0=/work check
```

Expected: release check exits 0.

- [ ] **Step 4: Review final diff and commits**

```bash
git diff --check origin/main...HEAD
git log --oneline --decorate origin/main..HEAD
git status --short
```

Expected: only planned tracked changes plus the user's pre-existing untracked files; no generated drift or coauthor trailers.
