# The rete diagram system

Every hand-authored SVG in this directory follows one design language. This file
*is* that language — read it before you draw, and the next diagram will match
without archaeology.

The reference implementation is [`rete-anatomy.svg`](rete-anatomy.svg): a real
file's byte layout drawn to scale, labelled with numbers measured from the
published specimen. That figure defines the house style. Everything here is
generalised from it.

---

## 1. The three rules that are not negotiable

**Proportional.** If a rectangle stands for bytes, its width is proportional to
those bytes. A 731-byte dataset card next to a 1.85 GB index is a sliver, and it
should look like a sliver. Never draw a section "big enough to fit the label" —
move the label out instead.

**Measured.** Every number comes from a real, published file, and you can
re-derive it. Name the specimen in the file-name chip so a reader can check you.
See [§7 Verifying numbers](#7-verifying-numbers) for the commands.

**Honest.** If the specimen does not have a pyramid, do not draw a pyramid. An
absent section is drawn as an absent section — that teaches something true
(sections are optional) and costs nothing.

---

## 2. Constraints that decided the design

These are not stylistic preferences. They are the delivery environment.

| Constraint | Consequence |
|---|---|
| Referenced by `<img src>` from Markdown | No external CSS, no scripts, no web fonts, no embedded rasters. Everything inline. |
| Rendered on GitHub *and* the docs site | GitHub serves these with `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; sandbox`. Inline `<style>` is allowed; `<script>` is not, and never will be. |
| The docs site has a theme toggle | Dark mode is handled inside the SVG — see [§4](#4-dark-mode). |
| Phones exist | Must be legible at a **353 px** content width — see [§5](#5-phones). |
| Hand-editable | No tool exports. A diagram is a few hundred lines of readable SVG that the next person can diff and patch. If you cannot read the diff, it is too complicated. |

---

## 3. Tokens

Paste this block verbatim as the first child of `<svg>`. Do not fork the values.

```xml
<style>
  :root{
    --ink:#14171c; --ink2:#454c55; --muted:#5b6570;
    --surface:#f7f8fb; --panel:#ffffff; --line:#c9d2de;
    --cold:#eef1f6; --coldfg:#616b77; --on-chip:#0d1116;
    --data:#2bb673; --data-ink:#17794a;
    --meta:#a259f7; --meta-ink:#7a2fe0;
    --warn:#b32741;
  }
  @media (prefers-color-scheme:dark){
    :root{
      --ink:#e7ecf3; --ink2:#b4bec9; --muted:#98a4b1;
      --surface:#12161c; --panel:#1b212a; --line:#39434f;
      --cold:#232b35; --coldfg:#8d99a6;
      --data:#2fc47e; --data-ink:#5fe0a8;
      --meta:#a978f7; --meta-ink:#c6a4ff;
      --warn:#ff8098;
    }
  }
  svg{fill:var(--ink)}
  text{font-family:-apple-system,"Segoe UI",Roboto,sans-serif}
</style>
```

> **The base fill goes on `svg`, never on `text`.** This is not cosmetic. A
> `text{fill:var(--ink)}` *type rule* outranks a `fill=` presentation attribute,
> so every `<text fill="var(--data-ink)">` in the file renders plain ink and the
> whole colour system silently disappears — including inside `<g fill="…">`
> wrappers. Putting the base on `svg{}` makes descendants *inherit* it instead,
> and inheritance loses to a presentation attribute, which is what you want.
>
> We shipped this bug across the whole directory once. It is invisible in review
> (the diagram still renders, just monochrome) and it made two labels
> white-on-white. If you must colour from CSS, use the `.u-*` utility classes the
> files carry; a class selector beats inheritance and the attribute alike.

### Semantic roles

Colour carries meaning here; it is not decoration. There are exactly two families
and one neutral.

| Token | Means |
|---|---|
| `--data` / `--data-ink` | **Data** — the dictionary and the permutation indexes. The bytes that hold triples. |
| `--meta` / `--meta-ink` | **Metadata** — the header, the dataset card, the pyramid, the section directory. The bytes that describe the data. |
| `--ink` | Structural landmarks: the header block, the footer magic. Things that bound the file. |
| `--cold` + `--line` outline | **Not fetched / not present.** The default state of most of a file. |
| `--warn` | An error, an incoherence, a cost you are being warned about. Use sparingly; a diagram with three red things has none. |

**Never encode meaning in colour alone.** Fetched vs. not-fetched is *solid fill
vs. outlined `--cold` fill* — the fill weight carries it, the hue only reinforces
it. This survives greyscale printing, colour-blind readers, and the Safari
dark-mode gap in [§4](#4-dark-mode).

Every text colour above clears WCAG AA (≥ 4.5:1) against both `--surface` and
`--panel`, in both themes. If you add a colour, verify it before you commit.

> **Labels on a solid `--data` or `--meta` chip use `--on-chip`, never white.**
> White on `--data` is 2.6:1 and fails.

### Type

One family (`-apple-system, Segoe UI, Roboto, sans-serif`), one mono
(`ui-monospace, Consolas, monospace`). Mono means "this is a literal" — a byte
range, an identifier, a section name, a query.

| Role | Size | Weight |
|---|---|---|
| Title | 19 | 700 |
| Deck (the line under the title) | 13 | 400, `--muted` |
| Section label | 15 | 700, mono |
| Measure (a number that matters) | 14 | 700 |
| Body | 13 | 400 |
| Annotation | 12 | 400, `--muted` |
| Legend / micro | 11 | 400 |

Nothing below 11. See [§5](#5-phones) for why that is a floor and not a target.

### Geometry

| Property | Value |
|---|---|
| Radius — byte-strip segment | `4` |
| Radius — node / box | `7` |
| Radius — callout card | `12` |
| Radius — chip (pill) | half the height |
| Stroke — hairline, leader, brace | `1` |
| Stroke — zoom connector | `1.2` |
| Stroke — callout border | `1.3`, `stroke-dasharray="6 4"` |
| Stroke — flow arrow | `1.8` |
| Dash — leader line | `4 3` |
| Dash — absent / never fetched | `5 4` |
| Byte-strip segment height | `18` |
| Gutter between strip segments | `8` |

Callout cards are `--panel` filled with a dashed border in the colour of the
thing they explain, and they hang off a dashed leader from the element itself.
That is the anatomy figure's single most reusable idea: *zoom into a byte range
without redrawing it elsewhere.*

---

## 4. Dark mode

**Approach: `prefers-color-scheme` inside the SVG, as progressive enhancement
over a palette that is already legible either way.**

It is worth writing down why, because two plausible alternatives are wrong and
one browser does not cooperate.

An SVG loaded through `<img>` is a separate document. It cannot see the page's
`data-theme` attribute, so it cannot read the docs site's toggle directly. What
it *can* see is the CSS `color-scheme` property, which propagates from the
embedding element into the image document and drives `prefers-color-scheme`
inside it. The docs CSS already sets `color-scheme: dark` on
`:root[data-theme="dark"]` (`crates/docgen/src/main.rs`), and GitHub sets it from
the user's GitHub theme. So a media query inside the SVG follows both.

Measured, with Playwright, on a page pinned to dark while the *browser* was set
to light:

| Engine | Page-pinned dark reaches the SVG? | OS-level dark reaches the SVG? |
|---|---|---|
| Chromium | yes | yes |
| Firefox | yes | yes |
| **WebKit / Safari** | **no** | **no** |

WebKit does not apply `prefers-color-scheme` inside `<img>`-loaded SVG documents
at all. So on Safari and on iOS, every diagram renders in its light palette
regardless of the page.

That single fact shapes the design: **dark mode may never be load-bearing.** The
light palette has to be a deliberate, self-contained artefact that looks correct
sitting on a dark page — which is why every diagram draws its own `--surface`
card with a `--line` border and rounded corners, instead of bleeding a white
rectangle to the edges. On Safari-dark it reads as a light card on a dark page.
That is a design, not a bug.

Rejected alternatives:

- **`<picture>` with two files per diagram.** Doubles every diagram, and the two
  copies drift. The failure it prevents (Safari) it does not actually prevent for
  the docs-site toggle, because `<picture>`'s `prefers-color-scheme` media query
  is evaluated by the *page*, which on Safari also does not know about pinning
  unless we duplicate the toggle logic into a `<source media>` swap.
- **A single grey palette that is theme-agnostic.** Costs the data/metadata
  colour distinction, which is the most useful thing the anatomy figure does.

---

## 5. Phones

**Every diagram must be legible at a 353 px content width** — that is what the
docs' mobile reading experience gives a page on a 390 px-wide phone.

The arithmetic, which is the whole point: an SVG with a `viewBox` `W` units wide,
rendered at 353 px, scales text by `353 / W`. Legibility bottoms out around
**8 CSS px**. So:

> **Authored font size ≥ viewBox width ÷ 44.**

| viewBox width | Minimum authored font size |
|---|---|
| 480 | 11 |
| 560 | 12.7 |
| 620 | 14.1 |
| 760 | 17.3 |
| 1560 | 35.5 |

**Default to a 560-wide viewBox.** At 560, the type scale in §3 passes as
authored — that is why the scale is what it is. A 760-wide diagram with 12 px
labels is not a diagram on a phone; it is a grey smear.

If a figure genuinely needs more room, do not shrink the type. Give it a phone
layout in the same file:

```xml
<style>
  .narrow{display:none}
  @media (max-width:420px){ .wide{display:none} .narrow{display:inline} }
</style>
```

Media queries inside an `<img>`-loaded SVG evaluate against the **rendered**
width, not the page width. Verified in Chromium, Firefox **and** WebKit — unlike
`prefers-color-scheme`, this one works everywhere. One file, two layouts, no
duplication.

> **Trap:** the contents of `<style>` in an SVG are parsed as XML, not as opaque
> text. A `<` anywhere inside — including in a CSS comment — opens a tag and the
> whole file silently fails to render. Never write `<img>` or `a < b` in a
> stylesheet comment. Validate with an XML parser, not by eye; a broken SVG shows
> up as a tiny broken-image icon, not an error.

Verify with a screenshot at 390×844 before you commit. Not by squinting at a
scaled-down desktop render — actually render it.

---

## 6. Alt text

A diagram that only works visually is half-built.

`<title id="t">` inside the SVG, referenced by `role="img"
aria-labelledby="t"`, and a matching `alt` on the `<img>` in the Markdown. Both,
because GitHub and the docs site surface them differently.

The standard is set by `rete-anatomy.svg`: **describe what the figure shows,
with its numbers, in one sentence a person could act on.** Not "diagram of file
layout" — that is a filename, not a description.

Do not reference colour in alt text. "the fetched sections, shown blue" tells a
screen-reader user nothing and goes stale the moment the palette changes; say
"the fetched sections" and let the visual carry the hue.

---

## 7. Verifying numbers

Before you write a number in a diagram, measure it. Everything below runs in
Docker against a real published file.

```bash
# section byte ranges, counts, permutation mask — straight from the 1 KiB header
curl -s -r 0-1023 https://data.graphplaza.com/dblp/dblp.rete -o /tmp/h
#   offset 24 u64 quad_count · 32 u64 term_count · 44 u16 section_count
#   50 u8 permutation mask (0 = all six) · directory at 64 + i*24

# the dataset card, index never touched
rete card-url https://data.graphplaza.com/<name>/<name>.rete --json

# what a cold open actually costs, per tier
rete cost https://data.graphplaza.com/<name>/<name>.rete '<query>' --json
```

If a number moved, fix the diagram in the same commit that notices. A figure
captioned "measured" that is not measured is worse than no figure.

---

## 8. Checklist

- [ ] Tokens block pasted verbatim; no hard-coded hex outside it.
- [ ] Meaning never carried by hue alone (solid vs. outlined).
- [ ] viewBox ≤ 560 wide, or a `.narrow` phone layout exists.
- [ ] Smallest font ≥ viewBox width ÷ 44.
- [ ] Every number traced to a command in §7, in this commit.
- [ ] `<title>` written to the §6 standard; `alt` on the `<img>` matches it.
- [ ] Absent sections drawn as absent.
- [ ] Screenshotted at 390×844 and at desktop, in both themes.
- [ ] No `<script>`, no external refs, no embedded raster, no tool export.
