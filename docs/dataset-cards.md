# Dataset Cards

A Dataset Card describes the data inside a `.rete` file: its title, licence,
source and a profile of the classes and relationships it contains. Read the
card before querying an unfamiliar graph. It helps you choose identifiers
from the actual data rather than guessing them.

## Read a card {#reading-a-card}

```sh
rete card data.rete
rete card data.rete --json
rete card-url https://example.org/data.rete
```

The first command prints a readable summary; `--json` returns the structured
metadata. Replace the example URL with a published graph. A remote card read
uses the header and metadata ranges rather than downloading the graph's indexes.
Current source builds write format `0x06`, with the card region at the front;
older `0x05` files locate metadata through their section directory. Published
readers through 0.4.2 cannot read `0x06`; see [Compatibility](compatibility.md).
In the [Playground](playground-guide.md), open **Card** to inspect the same
metadata.

A file may have no card. A profile may also be partial: capped lists and
`truncated` do not mean the graph contains only the displayed entries.
See the [field reference](dataset-card-reference.md#whats-in-a-card) for the
distinction between curated, derived and measured values.

## Describe a graph you build {#building-a-card}

```sh
rete build data.nt -o data.rete --card \
  --title "My graph" --license "CC0-1.0" \
  --source "https://example.org/source"
rete card data.rete
```

Use the licence and source that actually apply to your data. Rete derives the
profile; you supply the description and provenance. A bare build does not
add a card. `--card` or a card metadata flag opts into one.

For a description, creators or example queries, put the curated metadata in
a JSON file and pass `--card-file card.json`. Explicit command-line metadata
overrides the corresponding file values. The
[complete example](dataset-card-reference.md#building-a-card) shows the schema.

Descriptions support [Markdown](dataset-card-reference.md#the-description-markdown-not-html),
not raw HTML. Use [`extra`](dataset-card-reference.md#custom-fields-the-extra-bag)
for bounded publisher-defined fields rather than inventing top-level keys.

## Update a published card {#maintaining-a-published-card}

Fixing a card generator changes future builds, not files already published.
Inspect the existing card first, preserve its curated fields, and use
`rete recard` when only the card and build record need changing. For example,
`--keep-curated` carries the existing publisher metadata forward; `--card-file`
supplies it explicitly. Read the
[maintenance procedure](dataset-card-reference.md#maintaining-a-published-card)
before running the operation.

The card is part of the file's content hash, so changing it changes the file's
identity. Keep a recovery copy before replacing a published file, verify the
result and its metadata, then update the published copy and any recorded hash.
`--in-place` avoids copying the data sections but still changes the original
file; it is not a backup or an unchanged identity.
In `0x06`, the update uses free space in the front card region. If the new
card and record do not fit, the operation falls back to copying the file.

## Look up the details

The [Dataset Card reference](dataset-card-reference.md) covers every field,
starter queries, named-graph profiles, build records and JSON-LD/Croissant
projections. The [format specification](SPEC.md) owns the on-disk contract;
[hosting](hosting.md) explains how to serve a graph.

<details>
<summary>Detailed topics and links from the earlier guide</summary>

<p id="whats-in-a-card"><a href="dataset-card-reference.md#whats-in-a-card">What&#x27;s in a card</a></p>
<p id="the-full-text-signal-measured-not-stored"><a href="dataset-card-reference.md#the-full-text-signal-measured-not-stored">The full-text signal: measured, not stored</a></p>
<p id="the-permutation-signal-measured-not-stored"><a href="dataset-card-reference.md#the-permutation-signal-measured-not-stored">The permutation signal: measured, not stored</a></p>
<p id="the-quoted-triple-signal-measured-not-stored"><a href="dataset-card-reference.md#the-quoted-triple-signal-measured-not-stored">The quoted-triple signal: measured, not stored</a></p>
<p id="time-dates-not-digits"><a href="dataset-card-reference.md#time-dates-not-digits">Time: dates, not digits</a></p>
<p id="querying-the-star-layer"><a href="dataset-card-reference.md#querying-the-star-layer">Querying the star layer</a></p>
<p id="where-to-get-theme-iris"><a href="dataset-card-reference.md#where-to-get-theme-iris">Where to get theme IRIs</a></p>
<p id="one-card-three-versions"><a href="dataset-card-reference.md#one-card-three-versions">One card, three versions</a></p>
<p id="the-description-markdown-not-html"><a href="dataset-card-reference.md#the-description-markdown-not-html">The description: Markdown, not HTML</a></p>
<p id="supported"><a href="dataset-card-reference.md#supported">Supported</a></p>
<p id="raw-html-is-not-supported--and-that-is-the-point"><a href="dataset-card-reference.md#raw-html-is-not-supported--and-that-is-the-point">Raw HTML is not supported — and that is the point</a></p>
<p id="writing-a-multi-line-description"><a href="dataset-card-reference.md#writing-a-multi-line-description">Writing a multi-line description</a></p>
<p id="how-long-may-it-be"><a href="dataset-card-reference.md#how-long-may-it-be">How long may it be?</a></p>
<p id="where-a-description-is-shown"><a href="dataset-card-reference.md#where-a-description-is-shown">Where a description is shown</a></p>
<p id="custom-fields-the-extra-bag"><a href="dataset-card-reference.md#custom-fields-the-extra-bag">Custom fields: the extra bag</a></p>
<p id="first-class-field-or-the-bag-the-rule"><a href="dataset-card-reference.md#first-class-field-or-the-bag-the-rule">First-class field or the bag? The rule</a></p>
<p id="why-one-bag-not-prefixed-keys"><a href="dataset-card-reference.md#why-one-bag-not-prefixed-keys">Why one bag, not prefixed keys</a></p>
<p id="what-custom-fields-actually-cost--and-the-limits"><a href="dataset-card-reference.md#what-custom-fields-actually-cost--and-the-limits">What custom fields actually cost — and the limits</a></p>
<p id="overflow-the-build-fails"><a href="dataset-card-reference.md#overflow-the-build-fails">Overflow: the build fails</a></p>
<p id="deterministic-hashed-tamper-evident"><a href="dataset-card-reference.md#deterministic-hashed-tamper-evident">Deterministic, hashed, tamper-evident</a></p>
<p id="how-the-projections-treat-custom-fields"><a href="dataset-card-reference.md#how-the-projections-treat-custom-fields">How the projections treat custom fields</a></p>
<p id="build-conditions-the-build-info-section"><a href="dataset-card-reference.md#build-conditions-the-build-info-section">Build conditions: the build-info section</a></p>
<p id="phases"><a href="dataset-card-reference.md#phases">Phases</a></p>
<p id="inputs-names-never-paths-digests-in-the-same-pass"><a href="dataset-card-reference.md#inputs-names-never-paths-digests-in-the-same-pass">Inputs: names, never paths; digests in the same pass</a></p>
<p id="written-last-in-place"><a href="dataset-card-reference.md#written-last-in-place">Written last, in place</a></p>
<p id="reproducible-records"><a href="dataset-card-reference.md#reproducible-records">Reproducible records</a></p>
<p id="compatibility"><a href="dataset-card-reference.md#compatibility">Compatibility</a></p>
<p id="in-json-ld"><a href="dataset-card-reference.md#in-json-ld">In JSON-LD</a></p>
<p id="why-it-sits-outside-the-content-hash"><a href="dataset-card-reference.md#why-it-sits-outside-the-content-hash">Why it sits outside the content hash</a></p>
<p id="the-starter-query-cost-figures"><a href="dataset-card-reference.md#the-starter-query-cost-figures">The starter-query cost figures</a></p>
<p id="a-build-does-not-ship-a-query-it-just-measured-at-zero-rows"><a href="dataset-card-reference.md#a-build-does-not-ship-a-query-it-just-measured-at-zero-rows">A build does not ship a query it just measured at zero rows</a></p>
<p id="measuring-a-file-that-already-exists"><a href="dataset-card-reference.md#measuring-a-file-that-already-exists">Measuring a file that already exists</a></p>
<p id="the-one-row-smoke-query"><a href="dataset-card-reference.md#the-one-row-smoke-query">The one-row smoke query</a></p>
<p id="the-three-tier-exploration-model"><a href="dataset-card-reference.md#the-three-tier-exploration-model">The three-tier exploration model</a></p>
<p id="the-starter-query-library"><a href="dataset-card-reference.md#the-starter-query-library">The starter-query library</a></p>
<p id="bounded-by-construction"><a href="dataset-card-reference.md#bounded-by-construction">Bounded by construction</a></p>
<p id="presence-is-not-co-occurrence"><a href="dataset-card-reference.md#presence-is-not-co-occurrence">Presence is not co-occurrence</a></p>
<p id="named-graph-datasets"><a href="dataset-card-reference.md#named-graph-datasets">Named-graph datasets</a></p>
<p id="fixing-the-card-on-an-existing-named-graph-rete"><a href="dataset-card-reference.md#fixing-the-card-on-an-existing-named-graph-rete">Fixing the card on an existing named-graph .rete</a></p>
<p id="how-a-card-goes-stale"><a href="dataset-card-reference.md#how-a-card-goes-stale">How a card goes stale</a></p>
<p id="survey-first--the-survey-is-free-the-re-card-is-not"><a href="dataset-card-reference.md#survey-first--the-survey-is-free-the-re-card-is-not">Survey first — the survey is free, the re-card is not</a></p>
<p id="deciding-one-file-rete-card-audit"><a href="dataset-card-reference.md#deciding-one-file-rete-card-audit">Deciding one file: rete card-audit</a></p>
<p id="which-re-card-path"><a href="dataset-card-reference.md#which-re-card-path">Which re-card path</a></p>
<p id="what-a-re-card-changes--and-what-it-must-not"><a href="dataset-card-reference.md#what-a-re-card-changes--and-what-it-must-not">What a re-card changes — and what it must not</a></p>
<p id="carrying-forward-is-a-floor--filling-a-blank-is-not-rewriting"><a href="dataset-card-reference.md#carrying-forward-is-a-floor--filling-a-blank-is-not-rewriting">Carrying forward is a floor — filling a blank is not rewriting</a></p>
<p id="publishing-the-result"><a href="dataset-card-reference.md#publishing-the-result">Publishing the result</a></p>
<p id="interoperability-json-at-rest-rdf-on-demand"><a href="dataset-card-reference.md#interoperability-json-at-rest-rdf-on-demand">Interoperability: JSON at rest, RDF on demand</a></p>
<p id="the-json-ld-projection---format-jsonld"><a href="dataset-card-reference.md#the-json-ld-projection---format-jsonld">The JSON-LD projection (--format jsonld)</a></p>
<p id="croissant--the-honest-subset---format-croissant"><a href="dataset-card-reference.md#croissant--the-honest-subset---format-croissant">Croissant — the honest subset (--format croissant)</a></p>
<p id="back-compatibility"><a href="dataset-card-reference.md#back-compatibility">Back-compatibility</a></p>
<p id="in-the-browser"><a href="dataset-card-reference.md#in-the-browser">In the browser</a></p>
<p id="how-it-sits-in-the-file"><a href="dataset-card-reference.md#how-it-sits-in-the-file">How it sits in the file</a></p>

</details>
