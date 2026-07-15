# Playground Rich Media Cells Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add PDF modal zoom, consistent media source links, Page preview and Markdown render types, and improved desktop card/image interactions to the playground.

**Architecture:** Extend the existing renderer functions in `web/playground-src/app.js`, sharing only a small media-footer helper while retaining the specialized IIIF, PDF, image, and 3D modal implementations. Add one local-fixture Playwright check that exercises the real render-type menus and desktop interactions without third-party network dependencies, then regenerate the tracked playground and browser documentation.

**Tech Stack:** Browser JavaScript, CSS, PDF.js 4.7.76 API contract, Playwright 1.49, Python playground builder, Rust `docgen`, Docker Compose/devcontainer.

## Global Constraints

- Work only in `D:/pro/rete-release-1.0.0-rc1` on `release-1.0.0-rc1`.
- Keep one PR: update existing draft PR #66; do not create a second PR.
- Commit without a `Co-Authored-By` trailer.
- Do not introduce a Markdown, screenshot, or UI framework dependency.
- Raw Markdown HTML must remain escaped; clickable Markdown links allow only `http:`, `https:`, and `mailto:`.
- Page previews are opt-in, lazy, sandboxed, and cannot navigate the parent page.
- Preserve mobile carousel behavior and vertical scrolling inside long cards.
- Regenerate `docs/playground.html` and, when `docs/browser.md` changes, `docs/browser.html`.

---

### Task 1: Add the Rich-Media Browser Contract

**Files:**
- Create: `tests/gate/checks/check_rich_media_cells.mjs`
- Modify: `tests/gate/run.mjs`

**Interfaces:**
- Consumes: the existing builder UI, render-type `<select class="coltype">`, media DOM classes, and focused-card modal.
- Produces: a deterministic G2 check named `check_rich_media_cells` covering the complete approved interaction contract.

- [ ] **Step 1: Write the failing Playwright check**

Build a local N-Triples graph in-browser with three rows and values for image,
PDF, audio, video, spin, 3D mesh, generic page, and Markdown. Route all fixture
URLs and the PDF.js/model-viewer CDN modules locally. The check must:

```js
const FIXTURE = `<https://fixture.test/row/1> <https://fixture.test/title> "First card" .
<https://fixture.test/row/1> <https://fixture.test/image> <https://fixture.test/media/one.jpg> .
<https://fixture.test/row/1> <https://fixture.test/pdf> <https://fixture.test/media/book.pdf> .
<https://fixture.test/row/1> <https://fixture.test/audio> <https://fixture.test/media/sound.mp3> .
<https://fixture.test/row/1> <https://fixture.test/video> <https://fixture.test/media/movie.mp4> .
<https://fixture.test/row/1> <https://fixture.test/spin> <https://fixture.test/item-spin/one.webm> .
<https://fixture.test/row/1> <https://fixture.test/model> <https://fixture.test/media/model.glb> .
<https://fixture.test/row/1> <https://fixture.test/page> <https://fixture.test/page/one> .
<https://fixture.test/row/1> <https://fixture.test/markdown> "# Heading\\n\\n- one\\n- two\\n\\n**bold** [safe](https://example.test/) [bad](javascript:alert(1)) <script>window.__markdownPwned=1</script>"@en .
<https://fixture.test/row/2> <https://fixture.test/title> "Second card" ; <https://fixture.test/image> <https://fixture.test/media/two.jpg> .
<https://fixture.test/row/3> <https://fixture.test/title> "Third card" ; <https://fixture.test/image> <https://fixture.test/media/three.jpg> .`;
```

After querying row 1, force each column through its real menu and assert:

```js
await page.selectOption('select.coltype[data-col="markdown"]', "markdown");
await page.selectOption('select.coltype[data-col="page"]', "page");
await page.selectOption('select.coltype[data-col="pdf"]', "pdf");
// Repeat for image/audio/video/spin/model.
```

Require contextual `.media-source` links, safe Markdown block elements with no
`script` element or `javascript:` href, a lazy sandboxed Page preview iframe,
and a PDF modal that opens/navigates/closes while `window.__pdfOpenCount === 1`.

Run a three-row cards query at a 1440×1000 viewport. Assert the grid hover zoom
is larger than its source and remains inside the viewport, hover is suppressed
inside `#cardFocusModal`, the focused slide and both neighbors intersect the
track, and Shift+wheel plus mouse drag change the focused index.

Print one JSON object ending with `{ "verdict": "PASS" }` and exit nonzero on
any failed assertion, matching the existing G2 check convention.

- [ ] **Step 2: Register the check**

Add this tuple to `G2` in `tests/gate/run.mjs` immediately after
`check_query_shapes`:

```js
["check_rich_media_cells", "rich media renderers + desktop card carousel", 120000, false],
```

- [ ] **Step 3: Run the focused check and verify RED**

Run:

```sh
docker compose run --rm gate bash -lc \
  'npm ci --no-audit --no-fund && node run.mjs --local --only=check_rich_media_cells'
```

Expected: FAIL because `markdown` and `page` are absent, PDF has no modal, and
the unified media/footer and desktop interaction contracts are not implemented.

- [ ] **Step 4: Commit the failing contract**

```sh
git add tests/gate/checks/check_rich_media_cells.mjs tests/gate/run.mjs
git commit -m "test(playground): specify rich media cell interactions"
```

---

### Task 2: Add Shared Media Links, Page Preview, and Markdown

**Files:**
- Modify: `web/playground-src/app.js`
- Modify: `web/playground-src/styles.css`
- Test: `tests/gate/checks/check_rich_media_cells.mjs`

**Interfaces:**
- Produces: `mediaSourceLink(url, kind) -> string`, `pagePreviewCell(term) -> string`, `hydratePagePreviews(scope)`, `markdownCell(term, raw) -> string`, and new column types `page` and `markdown`.
- Consumed by: all existing media renderers, the global hydration observer, cards, tables, and Task 3's PDF footer.

- [ ] **Step 1: Add the shared source-link helper and adopt it everywhere**

Implement a single escaped helper with contextual labels:

```js
const MEDIA_SOURCE_LABEL = {
  image: "Open image ↗", pdf: "Open PDF ↗", audio: "Open audio ↗",
  video: "Open video ↗", model3d: "Open 3D ↗", viewer3d: "Open viewer ↗",
  iiif: "Open manifest ↗", page: "Open page ↗",
};
function mediaSourceLink(url, kind) {
  const up = httpsUpgrade(url);
  return `<div class="media-footer"><a class="media-source media-source-${esc(kind)}" ` +
    `href="${esc(up)}" target="_blank" rel="noopener noreferrer">` +
    `${esc(MEDIA_SOURCE_LABEL[kind] || "Open source ↗")}</a></div>`;
}
```

Place it after metadata/navigation in image, PDF button/viewer, successful and
failed IIIF, audio, video, spin, 3D mesh, and 3D viewer-page cells.

- [ ] **Step 2: Add the safe Markdown renderer**

Implement tokenized inline parsing so code and links are escaped before emphasis,
then a line-oriented block parser for headings, paragraphs, lists, blockquotes,
and fenced code. Validate links before emitting anchors:

```js
function safeMarkdownHref(raw) {
  try {
    const u = new URL(raw);
    return /^(https?:|mailto:)$/.test(u.protocol) ? raw : "";
  } catch (_e) { return ""; }
}
function markdownCell(t, raw) {
  const lang = t.lang ? ` <span class="t-lang">@${esc(t.lang)}</span>` : "";
  return `<td class="lit markdown-cell" title="${esc(raw)}"><div class="markdown-body">` +
    `${markdownBlocks(t.value)}</div>${lang}</td>`;
}
```

Raw HTML remains text. Unsafe links render as escaped label/URL text without an
anchor. Add `case "markdown"` to `prettyCell` and `["markdown", "Markdown"]` to
`COL_TYPES`.

- [ ] **Step 3: Add the Page preview renderer and lazy hydrator**

Render a hostname, loading frame, explanatory note, and source footer. Keep the
URL in `data-page-url` until a shared `IntersectionObserver` sees the cell:

```js
function pagePreviewCell(t) {
  const url = httpsUpgrade(t.value);
  let host = t.value;
  try { host = new URL(url).host.replace(/^www\./, ""); } catch (_e) {}
  return `<td class="iri page-preview-cell" data-page-url="${esc(url)}">` +
    `<div class="page-preview-host">${esc(host)}</div>` +
    `<div class="page-preview-frame"><div class="page-preview-loading"><span class="spindle"></span></div></div>` +
    `<div class="page-preview-note">Some sites block embedding.</div>` +
    `${mediaSourceLink(url, "page")}</td>`;
}
```

Hydration inserts exactly one iframe with `loading="lazy"`,
`sandbox="allow-scripts"`, and `referrerpolicy="no-referrer"`. Add
`hydratePagePreviews` to card-focus near-slide hydration and the existing output
MutationObserver. Add `case "page"` and `["page", "Page preview"]`.

- [ ] **Step 4: Style the footer, Markdown, and Page preview**

Add focused CSS for `.media-footer`, `.media-source`, `.markdown-body`, and
`.page-preview-*`. The inline iframe renders at a desktop canvas (about 1200×760)
and scales into a roughly 280×180 cell viewport. Markdown lists/headings use
compact result-cell spacing and code wraps without widening the table.

- [ ] **Step 5: Run the focused check**

Run the Task 1 command. Expected: footer, Page preview, and Markdown assertions
pass; PDF-modal and carousel assertions remain RED.

- [ ] **Step 6: Commit the render types**

```sh
git add web/playground-src/app.js web/playground-src/styles.css
git commit -m "feat(playground): add rich media cell renderers"
```

---

### Task 3: Add the PDF Page Modal

**Files:**
- Modify: `web/playground-src/app.js`
- Modify: `web/playground-src/styles.css`
- Test: `tests/gate/checks/check_rich_media_cells.mjs`

**Interfaces:**
- Produces: `ensurePdfModal()`, `openPdfModal(doc, url, page)`, `pdfModalGo(delta)`, and a shared `renderPdfCanvas(doc, page, canvas, maxWidth, maxHeight)` helper.
- Consumes: the existing PDF.js loader and Task 2's `mediaSourceLink`.

- [ ] **Step 1: Extract reusable PDF canvas rendering**

Replace duplicate scale/canvas setup with a promise-returning helper that accepts
explicit CSS bounds and device-pixel ratio. The inline renderer continues to cap
pages at 300×380 CSS pixels.

- [ ] **Step 2: Add the lazy PDF modal**

Create one modal on first use with backdrop, close button, canvas/loading stage,
previous/next controls, page indicator, and `Open PDF ↗`. Store only
`{ doc, url, page, busy }`, render the current page at available viewport size,
and never call `pdfjs.getDocument` from the modal.

- [ ] **Step 3: Wire inline state to modal state**

Make the inline stage an accessible zoom button. Once its document exists,
clicking opens the modal on `cur`. Support Escape/backdrop close and Left/Right
navigation. Disable page controls at the first/last page and expose loading/error
text in the modal.

- [ ] **Step 4: Style the modal responsively**

Use a fixed overlay above the card/image lightboxes, a `min(1080px, 96vw)` box,
a viewport-bounded canvas stage, and compact navigation/footer. On phones use
full-width controls and preserve page readability.

- [ ] **Step 5: Run the focused check**

Expected: PDF modal opens on the inline page, page navigation works, Escape
closes it, and the stub records exactly one document open. Carousel assertions
remain RED.

- [ ] **Step 6: Commit the PDF modal**

```sh
git add web/playground-src/app.js web/playground-src/styles.css
git commit -m "feat(playground): add enlarged PDF page viewer"
```

---

### Task 4: Improve Desktop Hover and Focused Cards

**Files:**
- Modify: `web/playground-src/app.js`
- Modify: `web/playground-src/styles.css`
- Modify: `web/playground.template.html`
- Test: `tests/gate/checks/check_rich_media_cells.mjs`

**Interfaces:**
- Produces: desktop-only viewport-clamped hover preview and mouse/trackpad/keyboard/scrollbar card-carousel navigation.
- Preserves: touch swipe, mandatory scroll snap, vertical slide scrolling, media/control interaction, and mobile sizing.

- [ ] **Step 1: Restrict and enlarge hover zoom**

Before scheduling hover, require `(hover: hover) and (pointer: fine)` and reject
images inside `#cardFocusModal`, `.img-lb`, or another modal. Increase the CSS
cap to `min(560px, calc(100vw - 16px))` by
`min(72vh, calc(100vh - 16px))`. Update positioning to choose the side with more
space and clamp the measured preview to an eight-pixel viewport margin.

- [ ] **Step 2: Widen desktop focused cards and expose neighbors**

Under a desktop fine-pointer media query, make the modal about
`min(1180px, 96vw)`, slides about 74% of the track, and edge spacers about 13%.
Keep the current scale/opacity distinction and show a slim horizontal scrollbar.
Do not change mobile dimensions.

- [ ] **Step 3: Add desktop horizontal inputs**

Attach a non-passive wheel handler while the modal is open. Native `deltaX`
remains native; Shift+wheel maps its dominant delta onto `scrollLeft` and calls
`preventDefault`. Add mouse pointer drag with a movement threshold, pointer
capture, and an exclusion selector covering links, buttons, inputs, selects,
code, model-viewer, audio, video, and IIIF/PDF controls. Suppress the field-zoom
click immediately after a drag.

- [ ] **Step 4: Update the interaction hint**

Change the focused-card hint to `scroll, drag, or use ← →` on desktop-compatible
copy while keeping the accessible Prev/Next buttons.

- [ ] **Step 5: Run the focused check and verify GREEN**

Run the Task 1 command. Expected: PASS with all media, PDF, hover, neighbor, and
horizontal-navigation assertions green.

- [ ] **Step 6: Run the complete local gate**

```sh
docker compose run --rm gate bash -lc \
  'npm ci --no-audit --no-fund && node run.mjs --local'
```

Expected: all G0, G1, and non-live G2 checks pass.

- [ ] **Step 7: Commit desktop interactions**

```sh
git add web/playground-src/app.js web/playground-src/styles.css web/playground.template.html tests/gate/checks/check_rich_media_cells.mjs tests/gate/run.mjs
git commit -m "feat(playground): improve desktop media exploration"
```

---

### Task 5: Document and Regenerate the Playground

**Files:**
- Modify: `docs/browser.md`
- Regenerate: `docs/browser.html`
- Regenerate: `docs/playground.html`

**Interfaces:**
- Consumes: final source render types and interactions.
- Produces: tracked documentation matching shipped playground behavior.

- [ ] **Step 1: Update browser documentation**

Document Page preview and Markdown in the render-type section, contextual source
links for every media cell, PDF modal interaction, and the desktop focused-card
navigation inputs. Preserve caveats about third-party CORS/CSP/range behavior.

- [ ] **Step 2: Regenerate documentation in the devcontainer**

```sh
docker compose run --rm dev cargo run -q -p docgen
docker compose run --rm dev uv run python scripts/build_playground.py
```

- [ ] **Step 3: Verify generated files are deterministic**

Run both commands a second time, then:

```sh
git diff --check
git status --short
```

Expected: only intended source, tests, documentation, and generated HTML are
modified; the second generation introduces no additional diff.

- [ ] **Step 4: Run generated-page checks**

```sh
docker compose run --rm gate bash -lc \
  'npm ci --no-audit --no-fund && node run.mjs fast && node run.mjs --local --only=check_rich_media_cells'
```

Expected: G0/G1 and the focused real-browser check pass against the generated
page.

- [ ] **Step 5: Commit documentation**

```sh
git add docs/browser.md docs/browser.html docs/playground.html
git commit -m "docs: describe rich playground media cells"
```

---

### Task 6: Final Verification, Push, and PR Preview

**Files:**
- Review: all changes since `b1a6d7618e069ac514df01ea8a4bbd19906d9093`
- Update: existing GitHub PR #66 only

**Interfaces:**
- Produces: a pushed `release-1.0.0-rc1`, updated PR #66, green checks, and an automatic static preview URL for user review.

- [ ] **Step 1: Run final local verification**

```sh
docker compose run --rm gate bash -lc \
  'npm ci --no-audit --no-fund && node run.mjs --local'
docker compose run --rm dev bash -lc \
  'cargo fmt --all -- --check && cargo test --workspace --exclude rete-bench'
git diff --check origin/main...HEAD
git status --short
```

Expected: all commands exit zero and the worktree is clean.

- [ ] **Step 2: Review commits and requirement coverage**

Confirm the diff covers every design section: media links, PDF modal, Page
preview, Markdown, desktop hover, carousel sizing/input, tests, and docs. Confirm
no commit contains a co-author trailer.

- [ ] **Step 3: Push the existing branch**

```sh
git push origin release-1.0.0-rc1
```

- [ ] **Step 4: Update PR #66 description**

Add a concise rich-media summary and test commands to the existing PR. Do not
open another PR.

- [ ] **Step 5: Verify GitHub checks and preview publication**

Use `gh pr checks 66 --watch` with bounded polling, inspect failures if any, and
verify the preview publish check exposes the commit-addressed
`preview.graphplaza.com` playground. Report that URL for manual static review.

- [ ] **Step 6: Leave the PR unmerged for user review**

Do not merge. The user checks the static preview first; merge only after their
explicit approval.
