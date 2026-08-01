// Measure the streaming dump's memory cost in a FRESH process, and print one
// JSON line: `node measure-dump.mjs <triples> <bytes|lazy>`.
//
// The metric is the wasm engine's linear-memory size, which grows but never
// shrinks — so it is a true high-water mark, and it has to be sampled in a
// process whose engine has not already been made to grow by something else.
// Hence the isolation: the fixture is built in a child of this script (a
// build() here would leave a large freed hole that every later allocation
// would fall into for free, making any result look flat), and the materializing
// control runs LAST for the same reason.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { heapBytes, open } from "../dist/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const triples = Number(process.argv[2] ?? 100_000);
const mode = process.argv[3] ?? "bytes";

// Many triples over few distinct terms: the dictionary stays small so the
// numbers are dominated by the triple stream, not by term storage.
const lines = [];
for (let i = 0; i < triples; i += 1) {
  lines.push(`<urn:s:${i % 4000}> <urn:p:${i % 50}> <urn:o:${i}> .\n`);
}
const path = join(mkdtempSync(join(tmpdir(), "rete-dump-")), "big.rete");
execFileSync(process.execPath, [join(here, "build-fixture.mjs"), path], {
  input: lines.join(""),
  maxBuffer: 1 << 30,
});

// `bytes` puts the whole file image in the engine up front (index resident);
// `lazy` reads it by byte range, so tiles fault in as the scan advances.
const graph =
  mode === "bytes" ? await open(readFileSync(path)) : await open(pathToFileURL(path).href);

const openHeap = heapBytes();

// 1. Stream every quad, keeping none.
let streamed = 0;
for await (const _quad of graph.dump({ raw: true })) streamed += 1;
const afterStream = heapBytes();

// 2. Stream the same graph as N-Quads text, keeping none.
let ntBytes = 0;
for await (const chunk of graph.nquads()) ntBytes += chunk.length;
const afterNQuads = heapBytes();

// 3. The control: the same triples, materialized (engine-side Vec + JSON
//    envelope + JS array). MUST run last — it is what makes the heap grow.
const beforeQuery = heapBytes();
const rows = graph.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o }");
const afterQuery = heapBytes();

process.stdout.write(
  `${JSON.stringify({
    mode,
    triples,
    fileBytes: statSync(path).size,
    quads: graph.quads,
    streamed,
    ntBytes,
    openHeap,
    streamGrowth: afterStream - openHeap,
    nquadsGrowth: afterNQuads - afterStream,
    materializedRows: rows.length,
    materializeGrowth: afterQuery - beforeQuery,
  })}\n`,
);
