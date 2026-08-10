# JavaScript API

Welcome to the `rete-graph` JavaScript client! Available on npm, this package brings the exact same WebAssembly engine that powers the [playground](playground-guide.md) to **Node.js and modern browsers**.

Enjoy an idiomatic wrapper with parsed `Term` results, clean IRIs, and an API shape that feels right at home if you've used our [Python client](python.md).

## Installation

```sh
npm install rete-graph
```

## 1. Open and Query a Graph

You can open and query a `.rete` file lazily over the network, or from raw bytes in memory.

```javascript
import { open, build } from "rete-graph";

// Open lazily from a remote URL (Node.js or Browser Web Worker)
const g = await open("https://data.graphplaza.com/boe/boe.rete");

// Run a SPARQL query
const rows = g.query(`
    SELECT ?s ?label WHERE {
        ?s <http://www.w3.org/2000/01/rdf-schema#label> ?label
    } LIMIT 5
`);

for (const row of rows) {
    console.log(row.s.value, "→", row.label.toJS());
}

// Check your network efficiency!
console.log(g.stats()); 
// Output: { fileLength, bytes, requests } — proves it was a lazy range read, not a full download!
```

### Understanding Query Results

The `query()` function returns data based on your SPARQL query type:
- **`SELECT`**: Returns an array of `{variable: Term}` objects.
- **`ASK`**: Returns a boolean.
- **`CONSTRUCT` / `DESCRIBE`**: Returns an array of `[subject, predicate, object]` `Term` triples.

**The `Term` Object**
Every value is wrapped in a `Term` object containing:
- `.kind`: Either `"iri"`, `"literal"`, `"bnode"`, or `"triple"`.
- `.value`, `.datatype`, `.lang`.
- **`.toJS()`**: Intelligently coerces common XSD types into native JavaScript `number`, `boolean`, or `BigInt`.
- **`.n3`**: Returns the term formatted as an N-Triples string.

### More Powerful Methods

Your graph object `g` comes packed with powerful analytical tools:

```javascript
// Enable OWL 2 QL entailment via query rewriting!
g.query(q, { reason: true });   

// Get raw JSON from the engine
g.queryRaw(q);                  

// Fast autocomplete for labels (returns [{label, subject}])
g.prefixSearch("Berl");         

// Full-text search (requires a file built with --text-index)
g.textSearch("volcano");        

// Get a high-level summary of classes and relations
g.schema();                     

// Inspect metadata and hashes
g.graphNames(); 
g.info(); 
g.quads;
g.contentHash();                

// Build a new .rete file from RDF text (returns a Uint8Array)
await build(ntText, "nt");      
```

## 2. Environment Compatibility: Where Remote Opens Work

To keep the WASM engine fast and simple, it relies on synchronous requests for lazy data fetching. This introduces some important environment rules:

| Environment | `open(url)` (Lazy Remote) | `open(bytes)` (Eager Memory) |
|---|---|---|
| **Node ≥ 18** | ✅ *(Uses built-in sync-fetch bridge)* | ✅ |
| **Browser Web Worker** | ✅ *(Uses native sync XHR)* | ✅ |
| **Browser Main Thread** | ❌ *(Browsers block sync binary XHR)* | ✅ |

> **Best Practice for Browsers:** If you are building a web UI, run your remote graph inside a Web Worker so you don't block the main thread. This is exactly how our playground operates. Also, ensure your data host supports CORS and `Range` headers! (See [Hosting your .rete](hosting.md)).

## 3. No Bundler? Use a `<script>` Tag

If you're writing a quick script or a p5.js sketch, you can drop `rete-graph` straight into your HTML—no `npm install` or bundler required. 

```html
<script src="https://cdn.jsdelivr.net/npm/rete-graph@0.3.0/dist/rete-graph.min.js"></script>
<script>
  (async () => {
    // Fetch the file bytes manually (for small files on the main thread)
    const response = await fetch("mydata.rete");
    const bytes = new Uint8Array(await response.arrayBuffer());
    
    // Open eagerly
    const g = await rete.open(bytes);
    
    console.log(g.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 3"));
  })();
</script>
```
*(Tip: Always pin the version in your CDN URL for production stability).*

## 4. Integration with Comunica (and RDF/JS)

If you live in the RDF/JS ecosystem, we have great news! As of version **0.2.0**, `rete-graph` ships with `ReteSource`, a standard RDF/JS Source connector. 

This allows you to plug `.rete` files directly into [Comunica](https://comunica.dev), LDflex, GraphQL-LD, and the Solid ecosystem:

```javascript
import { open, ReteSource } from "rete-graph";

// Wrap your graph in an RDF/JS source
const source = new ReteSource(await open(bytesOrUrl));

// Now use `source` in any Comunica QueryEngine!
```

> **Zero Code Alternative:** Comunica can also query `rete` via standard SPARQL endpoints (perfect for heavy joins over large remote files). For a deep dive on which method to use when, see our dedicated guide: **[Comunica — rete in the RDF/JS ecosystem](comunica.md)**.

## 5. Feature Matrix

Here is a quick overview of what the JS client currently supports:

| Capability | Status | Notes |
|---|---|---|
| SPARQL SELECT / ASK / CONSTRUCT / DESCRIBE | ✅ | `query()` |
| Lazy remote open (HTTP Range) | ✅ | Node + Browser Workers |
| OWL 2 QL reasoned queries | ✅ | `query(q, {reason: true})` |
| Schema profile, prefix & text search | ✅ | Returns clean, idiomatic JS types |
| Build from RDF text | ✅ | `build()` produces uncompressed `.rete` arrays |
| Script-tag single-file usage | ✅ | `dist/rete-graph(.min).js` |
| TypeScript types | ✅ | Bundled `index.d.ts` |
| RDF/JS Source (Comunica, LDflex) | ✅ | Extends into the wider linked data ecosystem |
| `SERVICE` federation | ✅* | Supported via engine (requires Worker/Node) |
| Embedded Examples & Dataset Cards | ⏳ | Planned for future release |

*For contributors looking to build the engine from source or understand the sync-XHR bridge, please see [Client development & releases](clients-dev.md).*
