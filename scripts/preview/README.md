# Social previews (`scripts/preview/`)

Everything that makes a shared rete link unfurl into something worth clicking:
the Open Graph tags on every page, the pre-rendered 1200×630 card images, and the
per-example share pages the playground hands out.

## The problem this solves

The playground keeps its state in the URL **fragment**:

```
playground.html#dataset=hugging-face&load=lazy&mode=sparql&ex=1
```

A fragment is never sent to a server, and no unfurler runs the page's JavaScript.
Every one of those links therefore previews as the same generic playground card —
the dataset, the question and the answer are all invisible. Query strings would
not help either: a static host serves the same `playground.html` regardless.

The only fix is a **distinct URL per shareable thing**. So each catalog example
gets `docs/q/<dataset>-<n>.html` and each dataset `docs/d/<dataset>.html`, each
carrying its own tags and card image, and each forwarding a human to the exact
playground deep link it describes. `web/playground-src/app.js` (`shareableUrl`)
hands out those URLs from 🔗 and **Share**.

## The pipeline

```sh
cargo run -p docgen                    # docs/*.html get their tags (Rust, see below)
scripts/preview/run.sh capture         # run every example, record its real answer
scripts/preview/run.sh build           # inject + cards + pages
scripts/preview/run.sh check           # the same assertions the gate makes
```

| Step | Script | Produces |
| --- | --- | --- |
| capture | `capture.mjs` | `web/preview/answers.json` + `web/preview/shots/*.png` |
| inject | `inject_og.mjs` | tags in the pre-built app pages **and their `web/*.template.html`** |
| cards | `render_cards.mjs` | `docs/og/{q,d,doc}/*.png` |
| pages | `build_pages.mjs` | `docs/q/*.html`, `docs/d/*.html`, `docs/shared.html` |

`card.mjs` holds the model and the markup; `ogHtml()` renders the PNG and
`pageHtml()` (in `build_pages.mjs`) renders the landing page from the *same*
model, so what a crawler unfurls and what a visitor lands on cannot drift apart.

Everything runs in the Playwright Docker image — `run.sh` is the only entry
point, and it installs its one npm dependency into `tests/gate/node_modules`.

## Where each piece of text comes from

There is exactly one source for every string, which is why nothing here is
hand-maintained:

- **example cards** — the catalog (`web/playground-src/catalog.js`): label, tip,
  family, query, dataset scale and licence.
- **the answer** — measured, never written: `capture.mjs` opens the built
  `docs/playground.html` in Chromium, clicks the catalog's own example button,
  presses Run and scrapes the result table and the `#qmeta` line (row counts,
  timings, range requests, bytes read). A drawing view (graph / map / timeline)
  is screenshotted instead and the card shows that.
- **docs pages** — `crates/docgen/src/main.rs` derives each page's description
  from the first paragraph of its Markdown and writes the tags itself, so
  re-rendering the site keeps them.
- **pre-built app pages** (playground, yasgui, atlas …) — `inject_og.mjs`, which
  inherits each app's description from its guide page. It patches the built page
  *and* the `web/*.template.html` it came from, so the next rebuild of that page
  does not silently drop the preview.
- **card images** — `docs_models.mjs` reads the tags back out of the shipped
  HTML. A page opts into a card simply by pointing `og:image` at
  `og/doc/<slug>.png`.

## Re-running it

The capture is the expensive step: 637 examples over 91 datasets, most of them
live HTTP-range reads against multi-gigabyte files. It is append-only and
resumable — re-running skips everything that already produced data, so a failed
dataset can be topped up on its own:

```sh
scripts/preview/run.sh capture --dataset=hugging-face --force
scripts/preview/run.sh capture --shots-only          # just refresh the thumbnails
scripts/preview/run.sh capture --reader=sync --concurrency=1   # slow but sturdy
```

### The two budgets, and why they differ

The playground imposes **no** query timeout: press Run on a whole-graph predicate
summary over 673.5M triples and you wait, and you get an answer. Every timeout
in `capture.mjs` is therefore a statement about the harness's patience, never
about whether the example works — and a record that says `Timeout … exceeded` is
this file's own budget coming back at you.

| flag | default | applies to | why |
| --- | ---: | --- | --- |
| `--timeout` | 90 s | embedded datasets | in memory; a local wasm query that needs 90 s **is** a regression |
| `--remote-timeout` | 300 s | `remote-lazy` datasets | live HTTP range over multi-GB files; the same 300 s `check_catalog_examples.mjs` has always allowed |
| `--open-timeout` | 180 s | opening a dataset, once per dataset | faulting the dictionary directory over range before any query runs |

One example is genuinely beyond any sane budget rather than merely slow. Mark it
in the catalog, next to the query, with the reason as the flag's value:

```js
{"label": "…", "skipCapture": "why this one cannot be swept", "q": "…"}
```

`capture.mjs` then never runs it, and `check_catalog_answers.mjs` accepts the
missing entry — but fails if a `skipCapture` example ever turns up with a good
answer, so the exclusion cannot outlive its reason. It is for **cost**, never for
a query that is merely wrong: a broken example must be fixed or deleted.

### Why a partial capture is safe

`answers.json` is committed; the JSONL cache it is consolidated from is
**gitignored**, so a clean checkout has none. Since `capture` finalizes when it
finishes — including `capture --dataset=x` — that combination used to mean that
capturing one dataset on a fresh clone rewrote `answers.json` from a cache
holding only that dataset, deleting every other answer and churning ~1,100
generated files behind it.

Finalize is now additive by construction, and the guarantee is the default:

- the committed `answers.json` is the **base**, not the output — cache records
  are merged over it, so a partial capture can only add or update;
- the key set is the **whole catalog**, never the `--dataset`/`--scope` subset;
- a cached **failure never supersedes** an answer that already worked;
- an output with fewer answers than the committed file **aborts**, naming the
  count it was about to drop; nothing is written;
- a **missing cache is seeded** from the committed `answers.json`, so a clean
  checkout behaves exactly like an incremental one.

The destructive operations still exist, but only ever spelled out in full:

```sh
scripts/preview/run.sh finalize --allow-shrink   # accept dropping answers (deleted examples)
scripts/preview/run.sh finalize --rebuild        # ignore the committed file, use the cache alone
scripts/preview/run.sh capture --force           # re-measure everything
```

The other subcommands carry no equivalent hazard: `inject`, `cards` and `pages`
read committed inputs only (`answers.json`, `web/preview/shots/`,
`docs/og/cards.json`, `docs/*.html`) and never delete an output.

Cards are content-addressed (a `.png.src` sidecar holds a hash of the markup),
so `run.sh cards` only re-renders what actually changed.

After changing anything in `web/playground-src/`, rebuild the page the visitor
gets (`bash scripts/build_wasm.sh`) — CI diffs `docs/playground.html` against a
fresh build.

## What the gate checks

`tests/gate/checks/check_social_previews.mjs` (tier G0) fails if a catalog
example has no share page or no card, if any `og:image` 404s or is relative
(unfurlers drop relative URLs silently), or if a rendered docs page or app page
lost its tags. It is a static check — no browser, no network.

`tests/gate/checks/check_catalog_answers.mjs` (tier G0) reads the ANSWERS. A
shipped catalog example must have a recorded, successful one. It may not be
recorded as answering nothing — no rows, or the single row `COUNT` owes an
aggregate filled with nothing but zeros — unless the example carries
`allowEmpty: true` (and then it must really be empty, so the flag cannot become a
mute button). Since #212 it may not be recorded as `ok: false` either — a hang,
an engine error and an empty result are all the same fact to a reader — nor may
it be missing from the file altogether. `skipCapture: "<why>"` is the only
exemption, and it is checked in both directions like `allowEmpty`.

That check exists because this file is the ONLY measurement of the ~60
`remote-lazy` datasets: the live sweep, `check_catalog_examples.mjs`, defaults to
`--scope=embedded` because sweeping the multi-gigabyte ones costs hours. Nine
examples sat here recorded at 0 rows across several releases, and nothing read
them. If it goes red, re-run the query, fix it, and re-capture that dataset:

```sh
scripts/preview/run.sh capture --dataset=<key> --force
scripts/preview/run.sh build
```

A stale `answers.json` is a real defect, not bookkeeping: the share page of an
example with no answer ships with no **Answer:** line at all.
