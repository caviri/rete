// The memory claim, MEASURED — not asserted.
//
// `graph.dump()` / `graph.nquads()` are supposed to stream in memory that does
// not grow with the graph. The engine's wasm linear memory grows but never
// shrinks, so its size is an honest high-water mark; each measurement below
// runs in its own child process (see measure-dump.mjs) so nothing else has
// already grown the heap it samples.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const MiB = 1024 * 1024;

function measure(triples, mode) {
  const out = execFileSync(
    process.execPath,
    [join(here, "measure-dump.mjs"), String(triples), mode],
    { encoding: "utf8", maxBuffer: 1 << 28 },
  );
  return JSON.parse(out.trim().split("\n").pop());
}

// A generous ceiling: the batch buffers plus allocator slack, nowhere near the
// tens of MB the same quads occupy once materialized.
const BOUNDED = 4 * MiB;

test("streaming 800k quads grows the engine heap no more than streaming 100k", async () => {
  const small = measure(100_000, "bytes");
  const large = measure(800_000, "bytes");

  assert.equal(small.streamed, small.quads);
  assert.equal(large.streamed, large.quads);
  assert.equal(large.quads, 8 * small.quads);

  // The scan hands ~32 MB of N-Triples text across the boundary at 800k...
  assert.ok(large.ntBytes > 30 * MiB, `expected a big stream, got ${large.ntBytes} B`);
  // ...and the heap does not follow it. Eight times the quads, no more memory.
  assert.ok(
    large.streamGrowth <= small.streamGrowth + BOUNDED,
    `stream growth scaled with the graph: ${small.streamGrowth} B at 100k vs ${large.streamGrowth} B at 800k`,
  );
  assert.ok(large.streamGrowth <= BOUNDED, `stream growth ${large.streamGrowth} B exceeds the bound`);
  assert.ok(large.nquadsGrowth <= BOUNDED, `nquads growth ${large.nquadsGrowth} B exceeds the bound`);
});

test("the same quads materialized cost an order of magnitude more", async () => {
  const m = measure(800_000, "bytes");

  // The control is real: it does return every quad.
  assert.equal(m.materializedRows, m.quads);
  assert.ok(
    m.materializeGrowth > 50 * MiB,
    `expected the materializing control to be expensive, got ${m.materializeGrowth} B`,
  );
  assert.ok(
    m.streamGrowth * 10 < m.materializeGrowth,
    `streaming (${m.streamGrowth} B) is not decisively cheaper than materializing (${m.materializeGrowth} B)`,
  );
});

test("a lazily range-read graph streams within a bound too", async () => {
  // Here the index is NOT resident at open: tiles fault in as the scan
  // advances and stay cached, so the first pass pays for the index (a fraction
  // of the file, and only ONE of the six permutations). The second pass — the
  // N-Quads text — is the clean signal: it emits tens of MB with the index
  // already in hand, and adds essentially nothing.
  const m = measure(800_000, "lazy");

  assert.equal(m.streamed, m.quads);
  assert.ok(m.ntBytes > 30 * MiB);
  assert.ok(
    m.nquadsGrowth <= BOUNDED,
    `a second streaming pass grew the heap by ${m.nquadsGrowth} B`,
  );
  // Even including the index it faulted, the whole first pass stays far under
  // what materializing the same quads costs.
  assert.ok(
    m.streamGrowth * 10 < m.materializeGrowth,
    `lazy streaming (${m.streamGrowth} B) is not decisively cheaper than materializing (${m.materializeGrowth} B)`,
  );
});
