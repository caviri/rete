// A Range-capable HTTP server for the remote tests — the JS twin of the
// Python suite's fixture, with one Node-specific twist: it must run in its
// OWN worker thread. The client's sync-XHR bridge blocks the main thread in
// Atomics.wait while a fetch worker calls the server — a main-thread server
// could never respond (deadlock). The engine requires 206 answers, so the
// handler implements real byte serving.
import { Worker } from "node:worker_threads";

const SERVER_SRC = `
const { parentPort, workerData } = require("node:worker_threads");
const { createServer } = require("node:http");
const payload = Buffer.from(workerData);
const server = createServer((req, res) => {
  if (req.method === "HEAD") {
    res.writeHead(200, {
      "Accept-Ranges": "bytes",
      "Content-Length": String(payload.length),
    });
    return res.end();
  }
  const spec = req.headers.range;
  if (spec && spec.startsWith("bytes=")) {
    const [startS, endS] = spec.slice(6).split("-", 2);
    const start = Number(startS);
    const end = endS ? Math.min(Number(endS), payload.length - 1) : payload.length - 1;
    const chunk = payload.subarray(start, end + 1);
    res.writeHead(206, {
      "Content-Range": "bytes " + start + "-" + end + "/" + payload.length,
      "Content-Length": String(chunk.length),
    });
    return res.end(chunk);
  }
  res.writeHead(200, { "Content-Length": String(payload.length) });
  res.end(payload);
});
server.listen(0, "127.0.0.1", () => parentPort.postMessage(server.address().port));
parentPort.on("message", () => server.close(() => process.exit(0)));
`;

export function serveBytes(payload) {
  return new Promise((resolve) => {
    const worker = new Worker(SERVER_SRC, { eval: true, workerData: payload });
    worker.once("message", (port) => {
      resolve({
        url: `http://127.0.0.1:${port}/graph.rete`,
        close: () => {
          worker.postMessage("close");
          return new Promise((r) => worker.once("exit", r));
        },
      });
    });
  });
}
