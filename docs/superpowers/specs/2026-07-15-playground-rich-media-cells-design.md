# Playground Rich Media Cells Design

**Date:** 2026-07-15
**Status:** Approved for implementation
**Branch:** `release-1.0.0-rc1`

## Goal

Make SPARQL table and card results more useful for visual and rich-media data:

- open an inline PDF page in a large, paged modal like the IIIF viewer;
- give every media renderer a consistent link to its original source;
- add opt-in Page preview and Markdown column render types;
- improve desktop image hover previews; and
- make the focused-card carousel wider and easier to navigate horizontally on desktop.

The work extends the playground's existing renderers and modals. It does not
replace them with a new component framework.

## Existing Context

The playground already has independent renderers for images, IIIF manifests,
PDFs, audio, video, turntable clips, 3D meshes, geographic values, and ordinary
links. IIIF, images, maps, and 3D meshes have enlarged views, while PDF pages do
not. Source links also vary: some cells wrap the preview in a link, some show a
small arrow, and some only expose a link inside their enlarged modal.

Ordinary links have a hover preview implemented as a scaled, sandboxed iframe.
The focused-card modal already uses native horizontal scroll snap, but its
desktop container is narrow, its scrollbar is hidden, and ordinary mouse input
does not make the horizontal interaction discoverable.

## Design

### Shared media source link

Add one helper that renders a consistent footer link for a media URL. The link
is always visible below the preview, opens in a new tab, and uses
`rel="noopener noreferrer"`.

Labels are contextual:

- `Open image ↗`
- `Open PDF ↗`
- `Open audio ↗`
- `Open video ↗`
- `Open 3D ↗`
- `Open viewer ↗`
- `Open manifest ↗`
- `Open page ↗`

The helper is used by image, PDF viewer, PDF button, IIIF, audio, video, spin,
3D mesh, 3D viewer-page, and Page preview cells. Existing modal-level links
remain because they are useful while the cell is obscured by a modal.

### PDF page modal

The inline PDF stage is a zoom affordance. Clicking its rendered canvas opens a
large modal at the same page. The modal:

- reuses the `PDFDocumentProxy` already opened by the cell, avoiding a second
  document request;
- renders into its own high-DPI canvas sized to the available viewport;
- offers previous/next buttons and a `page N / total` indicator;
- responds to Left/Right arrows, Escape, backdrop click, and the close button;
- keeps an `Open PDF ↗` source link in its footer; and
- reports loading or rendering failure without breaking the inline viewer.

Moving pages in the modal updates only the modal. Closing it returns the user to
the inline cell without changing table layout. The inline viewer's current page
is used when opening the modal.

This feature does not claim that the remote PDF host supports byte ranges. It
preserves PDF.js's existing full-download fallback.

### Page preview render type

Add `Page preview` to the per-column render menu. It is opt-in: generic URL
columns continue to render as links unless a user or catalog example selects
this type.

Each Page preview cell contains:

- a compact desktop-shaped viewport;
- a lazily hydrated iframe rendered at desktop width and scaled down;
- a loading state and short note explaining that some sites block embedding;
- the hostname; and
- an `Open page ↗` footer link that always works independently of framing.

The iframe uses `loading="lazy"`, `referrerpolicy="no-referrer"`, and a sandbox
that permits scripts but does not permit top navigation, popups, downloads, or
same-origin access. Hydration occurs only as the cell approaches the viewport.
The playground does not attempt to bypass `X-Frame-Options` or CSP restrictions.

### Markdown render type

Add `Markdown` to the per-column render menu. It applies to the lexical value of
an RDF literal and preserves an RDF language badge when present.

The renderer supports:

- headings;
- paragraphs and line breaks;
- ordered and unordered lists;
- blockquotes;
- bold and italic text;
- inline code and fenced code blocks; and
- links.

Safety is mandatory: input is escaped before formatting, raw HTML is never
interpreted, and generated links accept only `http:`, `https:`, and `mailto:`
schemes. Links open in a new tab with `rel="noopener noreferrer"`. No new CDN or
runtime dependency is introduced; the implementation extends the playground's
existing small Markdown renderer.

### Desktop image hover

Hover enlargement is available only when the browser reports a fine pointer and
hover capability. It applies to images in table results and the ordinary card
grid, but not to images inside the focused-card modal or another lightbox.

The preview grows beyond the current 360×340 CSS-pixel ceiling to approximately
`min(560px, available viewport width)` by `min(72vh, available viewport height)`.
JavaScript measures the rendered preview and clamps all four edges to an
eight-pixel viewport margin. It prefers the side of the source image with more
space and falls back to an overlapping, viewport-contained placement when
neither side can hold it.

Touch and coarse-pointer devices do not create the hover preview.

### Focused-card desktop carousel

On desktop, widen the focused-card dialog to approximately
`min(1180px, 96vw)`. The current slide occupies roughly 70–76% of the track so
the previous and next cards have meaningful visible previews on both sides.
The centered card remains visually dominant; neighbors stay slightly scaled and
dimmed.

Navigation remains based on native horizontal scrolling and scroll snap. The
following inputs are supported:

- touch swipe;
- horizontal trackpad gestures;
- Left/Right arrow keys;
- previous/next buttons;
- Shift+wheel horizontal movement;
- mouse click-drag from non-interactive card surfaces; and
- a subtle visible horizontal scrollbar on fine-pointer desktop devices.

Vertical wheel and touch scrolling inside a long card remain vertical. Pointer
drag does not start from links, controls, media players, embedded viewers, form
fields, or selectable code. Mobile retains the current compact modal sizing and
hidden scrollbar.

The modal hint changes from touch-only copy to input-neutral copy such as
`scroll, drag, or use ← →`.

## Architecture

All behavior remains in the current playground source boundaries:

- `web/playground-src/app.js` owns render helpers, lazy hydration, modal state,
  and carousel input handling;
- `web/playground-src/styles.css` owns media footer, Page preview, Markdown, PDF
  modal, hover-preview, and responsive carousel presentation;
- `web/playground.template.html` owns the static focused-card hint; and
- `scripts/build_playground.py` regenerates `docs/playground.html` without a new
  build dependency.

The PDF modal is PDF-specific because it owns PDF.js page rendering. The shared
piece across media renderers is deliberately limited to the source-link footer.
This avoids a risky refactor of the mature IIIF, image, map, and 3D modals.

## Failure Handling

- A PDF modal render failure shows a readable state and retains the source link.
- A blocked Page preview leaves its hostname, explanatory note, and source link.
- Invalid Markdown link schemes render as text rather than clickable anchors.
- Missing or invalid media URLs fall back through the existing cell behavior.
- Closing a modal releases modal-specific canvas or viewer state where practical.

## Testing

Add focused Playwright coverage using local or intercepted fixtures so CI does
not depend on third-party media hosts:

1. Force each media render type and assert its contextual footer label, URL,
   target, and rel attributes.
2. Stub PDF.js, render an inline PDF, open the modal at the current page, navigate
   it, close it with Escape, and assert no second document open occurs.
3. Force Page preview, verify lazy iframe hydration and sandbox attributes, and
   verify the source link survives a blocked-frame fixture.
4. Force Markdown, verify block and inline formatting, and prove raw HTML and a
   `javascript:` link cannot execute.
5. At a desktop viewport, verify the hover preview is larger than the source,
   remains within viewport bounds, and does not appear inside focused cards.
6. Verify the desktop focused-card modal exposes neighbors and responds to
   horizontal scroll, Shift+wheel, drag, buttons, and arrow keys while retaining
   vertical scrolling inside a slide.
7. Run the complete fast playground gate, rebuild `docs/playground.html`, and run
   the relevant browser suite in the Docker Compose Playwright service.

## Documentation

Update `docs/browser.md` with the two new column render types and the consistent
media source-link behavior. Regenerate and commit `docs/browser.html` through
`docgen` if that Markdown source changes, and regenerate
`docs/playground.html` through `scripts/build_playground.py`.

## Non-goals

- Mirroring or linearizing remote PDFs.
- Adding byte-range support to third-party PDF servers.
- Automatically embedding every generic URL.
- Introducing a screenshot service for page thumbnails.
- Supporting raw HTML inside Markdown cells.
- Replacing all existing media modals with one generic modal framework.
