// A query worker: one engine on its own thread. The server (jobs.mjs) hands it
// one query at a time and never runs engine code itself, because a range read
// is a synchronous wait — rete-graph's Node bridge blocks the calling thread in
// Atomics.wait until the bytes arrive. On the server's thread that froze the
// whole MCP server for as long as a query read; here it freezes only this
// worker, which the server can terminate() mid-read to cancel the query.
//
// Progress: the engine reports every fetch it makes on its own to
// `globalThis.reteProgress`, and fetches its batches through
// `globalThis.reteReadMany` (installed by the first open(), when this
// rete-graph has it). Both add to two counters shared with the server —
// reads and bytes — which it reads while the query is still running.
import { parentPort, workerData } from "node:worker_threads";

import { open } from "rete-graph";

const MAX_OPEN = 4; // graphs kept open here, each with its own block cache

const counters = new BigInt64Array(workerData.counters); // [reads, bytes]
const count = (reads, bytes) => {
  Atomics.add(counters, 0, BigInt(reads));
  Atomics.add(counters, 1, BigInt(bytes));
};
globalThis.reteProgress = (bytes, _spans, n) => count(n ?? 1, bytes);

let batchesCounted = false;
function countBatches() {
  if (batchesCounted || typeof globalThis.reteReadMany !== "function") return;
  const fetchBatch = globalThis.reteReadMany;
  // A batch that comes back null is re-read range by range, and those reads
  // reach reteProgress — so only a served batch is counted here.
  globalThis.reteReadMany = (url, offsets, lens) => {
    const bytes = fetchBatch(url, offsets, lens);
    if (bytes) count(offsets.length, bytes.length);
    return bytes;
  };
  batchesCounted = true;
}

// Keyed by the server's cache key, not the URL: for a local file the key
// changes when its bytes may have (see GraphStore.stamp), so a rebuilt file
// is opened afresh rather than answered from the previous bytes' cache.
const graphs = new Map();

async function graphFor(key, target) {
  let graph = graphs.get(key);
  if (graph) {
    graphs.delete(key); // refresh LRU position
  } else {
    graph = await open(target);
    countBatches();
  }
  graphs.set(key, graph);
  while (graphs.size > MAX_OPEN) graphs.delete(graphs.keys().next().value);
  return graph;
}

// The engine's counters are cumulative per open graph; each answer reports
// what was read since the previous one on that graph — so the first also
// counts opening it, as on the server's own graphs (statsOf).
const lastStats = new WeakMap();

// op "query" runs a query — one chunk of a split one when `keys` is given;
// op "plan" plans splitting it by `var` (partition.mjs), and says which bytes
// it planned on, so a checkpoint is never resumed against another file.
parentPort.on("message", async ({ job, key, target, op = "query", query, reason, var: variable, keys }) => {
  try {
    const graph = await graphFor(key, target);
    const before = lastStats.get(graph) ?? { bytes: 0, requests: 0 };
    const started = Date.now();
    // The raw envelope: rows stay as the engine's term tokens, the compact
    // form the server keeps and converts a page at a time.
    const result =
      op === "plan"
        ? { plan: graph.partitionPlan(query, variable), contentHash: graph.contentHash() }
        : { env: graph.queryRaw(query, { reason, partition: keys && { var: variable, keys } }) };
    const ms = Date.now() - started;
    const after = graph.stats();
    if (after) lastStats.set(graph, after);
    parentPort.postMessage({
      job,
      ok: true,
      ...result,
      warnings: op === "plan" ? [] : graph.lastWarnings(),
      ms,
      stats: after && {
        fileBytes: after.fileLength,
        bytesRead: Math.max(0, after.bytes - before.bytes),
        requests: Math.max(0, after.requests - before.requests),
        sessionBytesRead: after.bytes,
      },
    });
  } catch (error) {
    parentPort.postMessage({ job, ok: false, error: error?.message ?? String(error) });
  }
});
