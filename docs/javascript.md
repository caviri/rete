# JavaScript API

`rete-graph` on npm is the JavaScript client for `.rete` files: the same
WebAssembly engine behind the [playground](playground-guide.md), packaged for
**browsers and Node** with an idiomatic wrapper — parsed `Term` results,
clean IRIs, the same API shape as the [Python client](python.md).

```sh
npm install rete-graph
```

```js
import { open, build } from "rete-graph";

const g = await open("https://data.graphplaza.com/boe/boe.rete"); // Node / worker
for (const row of g.query(`
    SELECT ?s ?label WHERE {
        ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label
    } LIMIT 5`)) {
  console.log(row.s.value, "→", row.label.toJS());
}
console.log(g.stats()); // { fileLength, bytes, requests } — lazy, not a download
```

## Or a single `<script>` tag — no bundler, no install

p5.js-style: one self-contained file (engine embedded), full and minified,
served by any npm CDN:

```html
<script src="https://cdn.jsdelivr.net/npm/rete-graph@0.1.0/dist/rete-graph.min.js"></script>
<script>
  (async () => {
    const bytes = new Uint8Array(await (await fetch("mydata.rete")).arrayBuffer());
    const g = await rete.open(bytes);
    console.log(g.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 3"));
  })();
</script>
```

`…/dist/rete-graph.js` is the readable twin; pin the version in the URL.

## Where remote opens work

Remote graphs are read with synchronous XHR range requests (that's what lets
the engine stay simple and synchronous):

| Environment | `open(url)` | `open(bytes)` |
|---|---|---|
| Node ≥ 18 | ✅ built-in sync-fetch bridge | ✅ |
| Browser web worker | ✅ native sync XHR | ✅ |
| Browser main thread | ❌ (browsers forbid sync binary XHR) | ✅ |

On a main thread, either fetch the bytes yourself (small files) or run the
graph in a worker — the pattern the playground uses. Hosts must send CORS
headers and honor `Range` ([Hosting your .rete](hosting.md)).

## Query results

`query()` returns `{variable: Term}` rows for SELECT, a boolean for ASK, and
`[s, p, o]` `Term` triples for CONSTRUCT/DESCRIBE. A `Term` has `.kind`
(`"iri" | "literal" | "bnode" | "triple"`), `.value`, `.datatype`, `.lang`,
plus `.toJS()` (number/boolean/BigInt for the common XSD types) and `.n3`.

```js
g.query(q, { reason: true });   // OWL 2 QL entailment by query rewriting
g.queryRaw(q);                  // the engine's raw JSON envelope
g.prefixSearch("Berl");         // label autocomplete → [{label, subject}]
g.textSearch("volcano");        // full-text (files built with --text-index)
g.schema();                     // { classes: [[iri, n]], relations: [[s,p,o,n]] }
g.graphNames(); g.info(); g.quads;
g.contentHash();                // remote graphs: blake3-16 cache key
await build(ntText, "nt");      // RDF text → .rete bytes (Uint8Array)
```

## Comunica (and the RDF/JS ecosystem) {#comunica-and-the-rdfjs-ecosystem}

From **0.2.0** the package ships `ReteSource`, an RDF/JS Source that plugs
`.rete` files into [Comunica](https://comunica.dev), LDflex, GraphQL-LD,
and the Solid ecosystem:

```js
import { open, ReteSource } from "rete-graph";
const source = new ReteSource(await open(bytesOrUrl));
// → sources: [source] in any Comunica QueryEngine
```

Comunica also talks to rete **with zero code** through the gateway's
SPARQL endpoints (full query pushdown — prefer it for heavy joins over big
remote files). Both levels, the exact semantics, and the
which-level-when table live on the dedicated page:
**[Comunica — rete in the RDF/JS ecosystem](comunica.md)**.

## Feature matrix

| Capability | JS | Notes |
|---|---|---|
| SPARQL SELECT / ASK / CONSTRUCT / DESCRIBE | ✅ | `query()` |
| Lazy remote open (HTTP Range) | ✅ | Node + browser workers |
| OWL 2 QL reasoned queries | ✅ | `query(q, {reason: true})` |
| Schema profile, prefix & text search | ✅ | clean IRIs everywhere |
| Build from RDF text | ✅ | `build()` — uncompressed, like the playground |
| Script-tag single-file build | ✅ | `dist/rete-graph(.min).js`, global `rete` |
| TypeScript types | ✅ | bundled `index.d.ts` |
| RDF/JS Source (Comunica, LDflex, GraphQL-LD) | ✅ 0.2.0 | `ReteSource` — see [Comunica](#comunica-and-the-rdfjs-ecosystem) |
| `SERVICE` federation | ✅* | via the engine; same worker/Node constraint |
| Dataset Card / embedded examples | ⏳ | needs a wasm export — planned parity with Python |
| Custom headers / custom readers | ⏳ | planned |
| Builder (card, pyramid options) | ⏳ | use `build()` or the Python/CLI builders |

For contributors — how the package builds its engine from the crates, the
sync-XHR bridge, and the release procedure:
[Client development & releases](clients-dev.md).
