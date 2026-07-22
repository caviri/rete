# Comunica — rete in the RDF/JS ecosystem

[Comunica](https://comunica.dev) is the modular JavaScript SPARQL framework
behind LDflex, GraphQL-LD, and much of the Solid ecosystem. rete plugs into
it at two levels — pick by where the joins should run.

## Level 1 — zero code: rete datasets as SPARQL endpoints

Every published dataset — and **any** `.rete` URL on the web — is a
standard SPARQL 1.1 endpoint via the gateway, and Comunica speaks to
endpoints natively (verified, this exact command):

```sh
$ npx -y -p @comunica/query-sparql comunica-sparql \
    "sparql@https://katospiegel-rete.hf.space/sparql/boe" \
    "SELECT ?title WHERE { <https://www.boe.es/eli/es/c/1978/12/27/(1)> <http://data.europa.eu/eli/ontology#title> ?title }"
[{"title":"\"Constitución Española.\""}]
```

```js
import { QueryEngine } from "@comunica/query-sparql";

const engine = new QueryEngine();
const bindings = await (await engine.queryBindings(sparql, {
  sources: [
    { type: "sparql", value: "https://katospiegel-rete.hf.space/sparql/boe" },
    // …mix freely with TPF, RDF files, other endpoints — Comunica federates.
  ],
})).toArray();
```

The whole query is **pushed down** to the rete engine server-side: rete
runs its own optimized joins over the file's indexes and Comunica receives
finished bindings. Use this level for heavy multi-join queries over big
remote files, and to let Comunica federate rete data with everything else
it speaks. Unregistered files work too:
`…/sparql/https://example.org/any.rete`.

## Level 2 — native: `ReteSource` (npm `rete-graph` ≥ 0.2.0)

An [RDF/JS Source](https://rdf.js.org/stream-spec/) over an open graph —
local bytes or a lazy URL — pluggable into any Comunica pipeline with no
server anywhere:

```js
import { QueryEngine } from "@comunica/query-sparql";
import { open, ReteSource } from "rete-graph";

const source = new ReteSource(await open("https://data.graphplaza.com/boe/boe.rete"));
const bindings = await (await new QueryEngine().queryBindings(
  `SELECT ?who ?label WHERE {
     ?s <urn:x:knows> ?who .
     ?who <http://www.w3.org/2000/01/rdf-schema#label> ?label .
   }`,
  { sources: [source] },
)).toArray();
```

What the source does, precisely:

- **`match(s, p, o, g)`** is one pattern lookup against the file's
  permutation indexes (a fully-bound pattern becomes an `ASK`). Comunica
  then executes the joins itself over the returned quad streams.
- **`countQuads(…)`** is implemented, so Comunica's planner can order
  joins by cardinality.
- **RDF/JS semantics** are honored: a `null` graph argument matches the
  default graph ∪ every named graph, `DefaultGraph`/`NamedNode` narrow it,
  blank-node arguments are matched by their stable labels, datatypes and
  language tags survive round-trips, and RDF-star quoted triples come back
  as nested RDF/JS `Quad`s.
- **Zero dependencies** — the package ships its own minimal RDF/JS terms
  and stream (nothing else enters your bundle).

## Which level, when

| Situation | Use |
|---|---|
| Big remote file, multi-join query | **Level 1** — full pushdown, rete's own joins |
| Local/embedded file in a JS app | **Level 2** — no server round-trips at all |
| Mixing rete with TPF / files / other endpoints | either; Comunica federates both |
| LDflex / GraphQL-LD / Solid libraries | **Level 2** — they consume RDF/JS sources |

Level 3 — native Comunica actors that auto-recognize `.rete` URLs by
content type — is deliberately not built yet; the two levels above cover
the use cases without tracking Comunica's actor API across majors. Ask for
it in the [issues](https://github.com/caviri/rete/issues) if your pipeline
needs it.

Tests behind this page: the client's suite runs the real
`@comunica/query-sparql` engine over a `ReteSource` (multi-pattern joins,
datatype/language fidelity, named graphs), and the published package is
smoke-tested from a clean `npm install`. See also
[the JavaScript client](javascript.md) and
[triple-store interop](interop.md).
