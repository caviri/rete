# rete-graph — JavaScript client for `.rete` files

Query **local and remote `.rete` graph files with SPARQL** from JavaScript —
in the browser and in Node. A `.rete` file is a single, immutable,
range-queryable RDF graph file ([rete](https://github.com/caviri/rete)): host
it on any HTTP server that supports `Range` requests and query it in place —
the client fetches only the byte ranges a query touches, never the whole
file. This package wraps the same WebAssembly engine that powers the
[rete playground](https://caviri.github.io/rete/playground.html).

## Install

```sh
npm install rete-graph
```

```js
import { open, build } from "rete-graph";

const g = await open(new Uint8Array(await (await fetch("data.rete")).arrayBuffer()));
for (const row of g.query("SELECT ?s ?label WHERE { ?s rdfs:label ?label } LIMIT 5")) {
  console.log(row.s.value, row.label.toJS());
}
```

### Or just a `<script>` tag (p5.js-style)

One self-contained file, engine included — via any npm CDN:

```html
<script src="https://cdn.jsdelivr.net/npm/rete-graph@0.1.0/dist/rete-graph.min.js"></script>
<script>
  (async () => {
    const g = await rete.open(await rete.build("<urn:a> <urn:knows> <urn:b> ."));
    console.log(g.query("ASK { <urn:a> ?p ?o }")); // true
  })();
</script>
```

`dist/rete-graph.js` is the unminified twin.

## Remote graphs (HTTP Range)

```js
const g = await open("https://data.graphplaza.com/boe/boe.rete"); // 447k triples
g.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 5");
g.stats(); // { fileLength, bytes, requests } — a LIMIT query fetches KBs–MBs, not the file
```

Remote opens use synchronous XHR range reads, so they work:

- **in Node** (≥18) — out of the box, via a built-in sync-fetch bridge;
- **in browser web workers** — where sync binary XHR is allowed (this is how
  the playground runs its queries).

On a browser **main thread**, open bytes instead, or move the graph into a
worker. The host serving the file must send CORS headers and honor `Range`.

## API sketch

`query()` returns `{var: Term}` rows for SELECT, a boolean for ASK, and
`[s, p, o]` Term triples for CONSTRUCT/DESCRIBE. A `Term` carries `.kind` /
`.value` / `.datatype` / `.lang`, plus `.toJS()` (number/boolean/BigInt for
common XSD types) and `.n3`. Also: `queryRaw`, `query(q, {reason: true})`
(OWL 2 QL entailment), `prefixSearch`, `textSearch`, `schema`, `graphNames`,
`info`, `card()` / `examples()` (the file's embedded Dataset Card and its
example queries), `shacl(shapes)` (SHACL Core validation), `dump()` /
`nquads()` / `writeNQuads()` / `toNQuads()` (the streaming export below), and
on lazily opened graphs `stats()` / `contentHash()`. `wasm` re-exports the raw
engine for anything this wrapper doesn't wrap.

## Streaming the whole graph out

`dump()` walks every quad lazily — the engine decodes one triple at a time and
the wrapper never holds more than a batch of them, so memory does not grow with
the graph:

```js
for await (const [s, p, o, g] of graph.dump()) {
  // g is the graph Term, or null in the default graph
}
```

`dump({graph})` narrows it: omit for the default graph followed by every named
graph, `null` for the default graph only, an IRI for one named graph.
`dump({raw: true})` yields the engine's N-Triples tokens instead of `Term`s.

For handing a `.rete` to another store, `nquads()` streams ready-made N-Quads
text (the engine writes the lines; nothing is re-serialized in JavaScript):

```js
import { Store } from "oxigraph";
const store = new Store();
for await (const chunk of graph.nquads()) store.load(chunk, { format: "application/n-quads" });

await graph.writeNQuads(createWriteStream("out.nq"));  // Node stream, WritableStream, or fn
const text = await graph.toNQuads();                   // one string — small graphs only
```

Under the hood each wasm call hands back **10 000 quads** (`batch`), which the
generator yields one at a time: one call per quad would make the boundary the
bottleneck, one call for the graph would rebuild the array these methods exist
to avoid. `heapBytes()` lets you check the claim rather than take it — the
engine heap is flat across a full dump, where materializing the same quads with
`SELECT ?s ?p ?o` costs hundreds of MB (see `test/dump-memory.test.mjs`).

This works on **remote graphs too**, with one honest caveat: a full dump
resolves every term and visits every tile, so it ends up fetching essentially
the whole file (and the tiles stay cached). It is how you *export* a remote
graph, not how you peek at one — for that, run a `LIMIT` query.

## Local files, read lazily (Node)

A `file://` URL is read exactly like a remote one — only the byte ranges a
query touches — so a multi-gigabyte graph on disk is queryable without loading
it into memory:

```js
import { pathToFileURL } from "node:url";
const g = await open(pathToFileURL("/data/huge.rete").href);
g.card().title;            // two small reads, whatever the file's size
g.query("SELECT ?s WHERE { ?s a <urn:Thing> } LIMIT 10");
g.stats();                 // { fileLength, bytes, requests }
```

Passing bytes still works and is right for small files; `file://` is the way
to keep big ones out of memory.

## Building from source

The package builds its wasm engine fresh from the repo's crates:

```sh
# from clients/js (needs Rust + wasm-pack; repo convention is Docker)
bash build-wasm.sh     # crates/rete-wasm -> vendor/pkg
npm install && npm test
```

Releases are published to npm by `.github/workflows/js-client-publish.yml`
on a `js-v*` tag. Docs: <https://caviri.github.io/rete/javascript.html>.
