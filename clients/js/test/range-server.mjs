// A Range-capable HTTP server for the remote tests — the JS twin of the
// Python suite's fixture, with one Node-specific twist: it must run in its
// OWN worker thread. The client's sync-XHR bridge blocks the main thread in
// Atomics.wait while a fetch worker calls the server — a main-thread server
// could never respond (deadlock). The engine requires 206 answers, so the
// handler implements real byte serving.
//
// Options, for the parallel-read tests: `delayMs` holds every answer back so
// overlapping requests are observable, `ignoreRange` answers 200 with the
// whole body (a host without Range support), and `stats()` reports how many
// requests came in and the most that were in flight at once.
import { Worker } from "node:worker_threads";

const SERVER_SRC = `
const { parentPort, workerData } = require("node:worker_threads");
const { createServer } = require("node:http");
const payload = Buffer.from(workerData.payload);
const { delayMs, ignoreRange } = workerData;
const stats = { requests: 0, inFlight: 0, maxInFlight: 0 };
const server = createServer((req, res) => {
  stats.requests += 1;
  stats.inFlight += 1;
  stats.maxInFlight = Math.max(stats.maxInFlight, stats.inFlight);
  res.on("close", () => { stats.inFlight -= 1; });
  setTimeout(() => answer(req, res), delayMs);
});
function answer(req, res) {
  if (req.method === "HEAD") {
    res.writeHead(200, {
      "Accept-Ranges": "bytes",
      "Content-Length": String(payload.length),
    });
    return res.end();
  }
  const spec = req.headers.range;
  if (spec && spec.startsWith("bytes=") && !ignoreRange) {
    const [startS, endS] = spec.slice(6).split("-", 2);
    const start = Number(startS);
    const end = endS ? Math.min(Number(endS), payload.length - 1) : payload.length - 1;
    if (start >= payload.length) {
      res.writeHead(416, { "Content-Range": "bytes */" + payload.length });
      return res.end();
    }
    const chunk = payload.subarray(start, end + 1);
    res.writeHead(206, {
      "Content-Range": "bytes " + start + "-" + end + "/" + payload.length,
      "Content-Length": String(chunk.length),
    });
    return res.end(chunk);
  }
  res.writeHead(200, { "Content-Length": String(payload.length) });
  res.end(payload);
}
server.listen(0, "127.0.0.1", () => parentPort.postMessage(server.address().port));
parentPort.on("message", (msg) => {
  if (msg === "stats") return parentPort.postMessage({ ...stats });
  server.close(() => process.exit(0));
});
`;

export function serveBytes(payload, { delayMs = 0, ignoreRange = false } = {}) {
  return new Promise((resolve) => {
    const worker = new Worker(SERVER_SRC, {
      eval: true,
      workerData: { payload, delayMs, ignoreRange },
    });
    worker.once("message", (port) => {
      resolve({
        url: `http://127.0.0.1:${port}/graph.rete`,
        stats: () => {
          worker.postMessage("stats");
          return new Promise((r) => worker.once("message", r));
        },
        close: () => {
          worker.postMessage("close");
          return new Promise((r) => worker.once("exit", r));
        },
      });
    });
  });
}
