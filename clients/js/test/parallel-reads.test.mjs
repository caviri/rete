// The Node bridge's parallel-read hook, `globalThis.reteReadMany`: the engine
// hands it a batch of ranges (dictionary terms a result decodes, tiles a join
// is about to probe) and it fetches them concurrently instead of one blocking
// round trip at a time. Three things must hold: the rows are exactly the ones
// sequential reads give, the reads really do overlap, and a batch that cannot
// be served faithfully comes back null so the engine falls back.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { build, open } from "../dist/index.js";
import { serveBytes } from "./range-server.mjs";

// A citation-shaped graph: many works citing one hub, each with a title and a
// year — the shape of the Crossref query that motivated the hook.
//
// The engine merges reads that sit close together (64 KiB apart for
// dictionary terms, 4 KiB for tiles), so a batch only has several ranges when
// what a query needs is spread out. Hence titles that are long and
// incompressible (a hash, so they sort in no relation to the work) and keys
// 1,000 works apart: the details query below hands the hook one 7-range batch.
// Short titles or neighbouring keys collapse into a single read, and the
// hook — rightly — is never called.
const N = 20_000;
const EX = "https://example.org/";
const work = (i) => `${EX}work/${String(i).padStart(6, "0")}`;
const LABEL = "http://www.w3.org/2000/01/rdf-schema#label";
const INT = "http://www.w3.org/2001/XMLSchema#integer";

const title = (i) => {
  let s = "";
  for (let k = 0; s.length < 256; k++) s += createHash("sha256").update(`${i}:${k}`).digest("hex");
  return s.slice(0, 256);
};

function citations() {
  const lines = [];
  for (let i = 0; i < N; i++) {
    const s = `<${work(i)}>`;
    lines.push(`${s} <${EX}cites> <${EX}hub> .`);
    lines.push(`${s} <${LABEL}> "${title(i)}" .`);
    lines.push(`${s} <${EX}year> "${1990 + (i % 35)}"^^<${INT}> .`);
  }
  return lines.join("\n") + "\n";
}

const KEYS = Array.from({ length: 20 }, (_, k) => work(k * 1_000));
const DETAILS = `SELECT ?c ?title ?year WHERE {
  VALUES ?c { ${KEYS.map((k) => `<${k}>`).join(" ")} }
  ?c <${EX}cites> <${EX}hub> .
  OPTIONAL { ?c <${LABEL}> ?title }
  OPTIONAL { ?c <${EX}year> ?year }
}`;
const ALL_CITING = `SELECT ?c WHERE { ?c <${EX}cites> <${EX}hub> }`;

/** Rows as sorted N-Triples strings: equal results compare equal in any order. */
const canon = (rows) =>
  rows
    .map((row) =>
      Object.entries(row)
        .map(([k, v]) => `${k}=${v.n3}`)
        .sort()
        .join(" "),
    )
    .sort();

let fixture = null;
const graphBytes = async () => (fixture ??= await build(citations()));

/**
 * Run `fn` with the hook wrapped to count the batches the engine hands it.
 * open() installs the hook, so a lazy open must have happened first.
 */
async function counting(fn) {
  const real = globalThis.reteReadMany;
  assert.equal(typeof real, "function", "a lazy open() installs the hook");
  const seen = { batches: 0, ranges: 0 };
  globalThis.reteReadMany = (url, offsets, lens) => {
    seen.batches += 1;
    seen.ranges += offsets.length;
    return real(url, offsets, lens);
  };
  try {
    await fn();
  } finally {
    globalThis.reteReadMany = real;
  }
  return seen;
}

/** Run `fn` with the hook switched off, the documented opt-out. */
async function sequential(fn) {
  const real = globalThis.reteReadMany;
  globalThis.reteReadMany = null;
  try {
    return await fn();
  } finally {
    globalThis.reteReadMany = real;
  }
}

test("batched reads overlap, and return exactly the rows sequential reads do", async () => {
  const bytes = await graphBytes();
  const inMemory = await open(bytes);
  const truth = { details: canon(inMemory.query(DETAILS)), all: canon(inMemory.query(ALL_CITING)) };
  assert.equal(truth.details.length, KEYS.length);
  assert.equal(truth.all.length, N);

  const parallel = await serveBytes(bytes, { delayMs: 5 });
  const serial = await serveBytes(bytes, { delayMs: 5 });
  try {
    const g = await open(parallel.url);
    let got;
    const seen = await counting(() => {
      got = { details: canon(g.query(DETAILS)), all: canon(g.query(ALL_CITING)) };
    });
    assert.deepEqual(got, truth);
    assert.ok(seen.batches > 0, "the engine should hand the hook at least one batch");
    assert.ok(seen.ranges > seen.batches, "a batch carries several ranges");
    assert.ok((await parallel.stats()).maxInFlight > 1, "a batch's ranges should be in flight together");

    const off = await sequential(async () => {
      const h = await open(serial.url);
      return { details: canon(h.query(DETAILS)), all: canon(h.query(ALL_CITING)) };
    });
    assert.deepEqual(off, truth);
    assert.equal((await serial.stats()).maxInFlight, 1, "with the hook off, reads stay one at a time");
  } finally {
    await parallel.close();
    await serial.close();
  }
});

test("file:// graphs take the same batches off disk", async () => {
  const bytes = await graphBytes();
  const truth = canon((await open(bytes)).query(DETAILS));
  const dir = await mkdtemp(join(tmpdir(), "rete-js-par-"));
  const path = join(dir, "citations.rete");
  await writeFile(path, bytes);

  const g = await open(pathToFileURL(path).href);
  let got;
  const seen = await counting(() => {
    got = canon(g.query(DETAILS));
  });
  assert.deepEqual(got, truth);
  assert.ok(seen.batches > 0);
});

test("the hook returns exactly the requested spans, or null", async () => {
  const bytes = await graphBytes();
  const server = await serveBytes(bytes);
  const noRange = await serveBytes(bytes, { ignoreRange: true });
  const dir = await mkdtemp(join(tmpdir(), "rete-js-par-"));
  const path = join(dir, "citations.rete");
  await writeFile(path, bytes);
  try {
    await open(server.url); // installs the hook
    const hook = globalThis.reteReadMany;
    const offsets = Float64Array.of(10, 5_000, 100, bytes.length - 1);
    const lens = Float64Array.of(7, 300, 1, 1);
    const expected = Buffer.concat([...offsets].map((o, i) => bytes.subarray(o, o + lens[i])));

    assert.deepEqual(Buffer.from(hook(server.url, offsets, lens)), expected);
    assert.deepEqual(Buffer.from(hook(pathToFileURL(path).href, offsets, lens)), expected);

    // A host that ignores Range (200 + whole body) must not be sliced into
    // "ranges": null, so the engine's sequential path reports it properly.
    assert.equal(hook(noRange.url, offsets, lens), null);
    // Past the end of the file: 416 on the wire, null here.
    assert.equal(hook(server.url, Float64Array.of(0, bytes.length + 10), Float64Array.of(4, 4)), null);
  } finally {
    await server.close();
    await noRange.close();
  }
});

test("a batch keeps at most 16 ranges in flight", async () => {
  const bytes = await graphBytes();
  const server = await serveBytes(bytes, { delayMs: 30 });
  try {
    await open(server.url);
    const n = 64;
    const offsets = Float64Array.from({ length: n }, (_, i) => i * 1_000);
    const lens = new Float64Array(n).fill(100);
    const before = (await server.stats()).requests;
    const t = Date.now();
    const out = globalThis.reteReadMany(server.url, offsets, lens);
    const ms = Date.now() - t;
    assert.equal(out.length, n * 100);
    const stats = await server.stats();
    assert.equal(stats.requests - before, n);
    assert.ok(stats.maxInFlight > 1 && stats.maxInFlight <= 16, `in flight: ${stats.maxInFlight}`);
    // 64 × 30 ms one at a time would be ~1.9 s; four waves of 16 are ~120 ms.
    assert.ok(ms < 64 * 30, `took ${ms} ms`);
  } finally {
    await server.close();
  }
});
