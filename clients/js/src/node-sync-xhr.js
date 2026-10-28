// Synchronous XMLHttpRequest for Node: the wasm engine's remote reader does
// blocking XHR range reads (browser-worker style). Node has no XHR and no
// sync fetch, so this bridges to async fetch in a worker thread and blocks
// the caller with Atomics.wait — the standard sync-over-async pattern.
// Implements exactly the subset web_sys uses: open(m, url, false),
// responseType='arraybuffer', setRequestHeader, send, status,
// getResponseHeader, response / responseText.
//
// `file://` URLs are served from the filesystem with the same HTTP surface the
// engine's reader expects (HEAD → Content-Length, ranged GET → 206 +
// Content-Range). That makes a LOCAL `.rete` lazily queryable byte-range by
// byte-range, exactly like a remote one — so a multi-gigabyte file on disk
// never has to be read into memory to be queried.
//
// The same worker also serves the engine's parallel-read hook,
// `globalThis.reteReadMany` (see `readMany` below). Sync XHR can only have one
// request in flight, so without it every range of a batched read — the
// dictionary terms a result decodes, the tiles a join is about to probe — is
// its own blocking round trip. With it the whole batch is fetched at once and
// the caller blocks once.
import { MessageChannel, Worker, receiveMessageOnPort } from "node:worker_threads";

const WORKER_SRC = `
const { parentPort } = require("node:worker_threads");
const fs = require("node:fs");
const { fileURLToPath } = require("node:url");

// One open descriptor per file, reused across the many small range reads a
// query makes (reopening per read would dominate the cost).
const fds = new Map();
const fdFor = (path) => {
  let fd = fds.get(path);
  if (fd === undefined) {
    fd = fs.openSync(path, "r");
    fds.set(path, fd);
  }
  return fd;
};

function readFile(method, url, headers, result) {
  const path = fileURLToPath(url);
  const size = fs.fstatSync(fdFor(path)).size;
  if (method === "HEAD") {
    result.status = 200;
    result.headers["content-length"] = String(size);
    result.body = new ArrayBuffer(0);
    return;
  }
  const range = /bytes=(\\d+)-(\\d*)/.exec(headers.Range || headers.range || "");
  const start = range ? Number(range[1]) : 0;
  const end = range && range[2] ? Math.min(Number(range[2]), size - 1) : size - 1;
  if (start >= size) {
    result.status = 416;
    result.headers["content-range"] = "bytes */" + size;
    result.body = new ArrayBuffer(0);
    return;
  }
  const len = Math.max(0, end - start + 1);
  const buf = Buffer.allocUnsafe(len);
  let got = 0;
  while (got < len) {
    const n = fs.readSync(fdFor(path), buf, got, len - got, start + got);
    if (n <= 0) break;
    got += n;
  }
  result.status = range ? 206 : 200;
  result.headers["content-length"] = String(got);
  if (range) result.headers["content-range"] = "bytes " + start + "-" + (start + got - 1) + "/" + size;
  result.body = buf.buffer.slice(buf.byteOffset, buf.byteOffset + got);
}

// One batch for reteReadMany: every range read at once (at most \`fanout\` in
// flight) and copied straight into the caller's shared buffer at its place in
// the concatenation. A range that is not exactly what was asked for fails the
// whole batch — the caller then returns null and the engine re-reads it one
// range at a time, under its own validation.
async function readMany({ url, offsets, lens, data, fanout }) {
  const out = new Uint8Array(data);
  const at = [];
  let pos = 0;
  for (const len of lens) {
    at.push(pos);
    pos += len;
  }
  if (url.startsWith("file:")) {
    const fd = fdFor(fileURLToPath(url));
    for (let i = 0; i < offsets.length; i++) {
      let got = 0;
      while (got < lens[i]) {
        const n = fs.readSync(fd, out, at[i] + got, lens[i] - got, offsets[i] + got);
        if (n <= 0) throw new Error("short read at offset " + offsets[i]);
        got += n;
      }
    }
    return;
  }
  let next = 0;
  let failure = null;
  // Each lane pulls the next range until the batch is done or one has failed;
  // waiting for every lane (not just the first rejection) means nothing is
  // still writing into the buffer once the caller is told the outcome.
  const lane = async () => {
    while (failure === null && next < offsets.length) {
      const i = next++;
      const offset = offsets[i];
      const len = lens[i];
      if (len === 0) continue;
      try {
        const resp = await fetch(url, { headers: { Range: "bytes=" + offset + "-" + (offset + len - 1) } });
        const range = resp.headers.get("content-range") || "";
        if (resp.status !== 206 || !range.startsWith("bytes " + offset + "-")) {
          await resp.body?.cancel();
          throw new Error("status " + resp.status + " (" + range + ") for offset " + offset);
        }
        const body = new Uint8Array(await resp.arrayBuffer());
        if (body.length < len) throw new Error("short range at offset " + offset);
        out.set(body.subarray(0, len), at[i]);
      } catch (e) {
        failure ??= e;
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(fanout, offsets.length) }, lane));
  if (failure) throw failure;
}

parentPort.on("message", async (msg) => {
  if (msg.op === "many") {
    let outcome = 1;
    try {
      await readMany(msg);
    } catch {
      outcome = 2;
    }
    Atomics.store(msg.signal, 0, outcome);
    Atomics.notify(msg.signal, 0);
    return;
  }
  const { port, signal, method, url, headers, body } = msg;
  const result = { status: 0, headers: {}, body: null, error: null };
  try {
    if (url.startsWith("file:")) {
      readFile(method, url, headers, result);
    } else {
      const resp = await fetch(url, { method, headers, body: body ?? undefined });
      result.status = resp.status;
      for (const [k, v] of resp.headers) result.headers[k.toLowerCase()] = v;
      result.body = await resp.arrayBuffer();
    }
  } catch (e) {
    result.error = String(e);
  }
  port.postMessage(result, result.body ? [result.body] : []);
  port.close();
  Atomics.store(signal, 0, 1);
  Atomics.notify(signal, 0);
});
`;

// Ranges a batch keeps in flight. The engine plans its batches for this
// fan-out: its reader reports a concurrency of 16 once the hook is installed.
const FANOUT = 16;

let worker = null;

function fetchWorker() {
  if (!worker) {
    worker = new Worker(WORKER_SRC, { eval: true });
    worker.unref(); // never keep the process alive on our account
  }
  return worker;
}

function syncFetch(method, url, headers, body) {
  const signal = new Int32Array(new SharedArrayBuffer(4));
  const { port1, port2 } = new MessageChannel();
  fetchWorker().postMessage({ port: port2, signal, method, url, headers, body }, [port2]);
  Atomics.wait(signal, 0, 0);
  const msg = receiveMessageOnPort(port1);
  port1.close();
  if (!msg) throw new Error(`sync fetch: no response for ${method} ${url}`);
  if (msg.message.error) throw new Error(`sync fetch failed: ${msg.message.error}`);
  return msg.message;
}

export class NodeSyncXMLHttpRequest {
  #method;
  #url;
  #headers = {};
  #resp = null;
  responseType = "";

  open(method, url, async = false) {
    if (async) throw new Error("NodeSyncXMLHttpRequest only supports synchronous mode");
    this.#method = method;
    this.#url = url;
  }

  setRequestHeader(name, value) {
    this.#headers[name] = value;
  }

  overrideMimeType() {}

  send(body = null) {
    this.#resp = syncFetch(this.#method, this.#url, this.#headers, body);
  }

  get status() {
    return this.#resp?.status ?? 0;
  }

  getResponseHeader(name) {
    return this.#resp?.headers[String(name).toLowerCase()] ?? null;
  }

  get responseText() {
    return new TextDecoder().decode(this.#resp?.body ?? new ArrayBuffer(0));
  }

  get response() {
    if (this.responseType === "arraybuffer") return this.#resp?.body ?? new ArrayBuffer(0);
    return this.responseText;
  }
}

/**
 * `globalThis.reteReadMany(url, offsets, lens)`: the engine's parallel-read
 * hook (`read_many_via_pool` in crates/rete-wasm). Reads every range of one
 * batch concurrently and returns them concatenated in request order, or
 * `null` if any range failed — the engine then falls back to reading the
 * batch one range at a time. Offsets are absolute; a `file://` URL is read off
 * disk.
 */
function readMany(url, offsets, lens) {
  let total = 0;
  for (const len of lens) total += len;
  const data = new SharedArrayBuffer(total);
  const signal = new Int32Array(new SharedArrayBuffer(4));
  fetchWorker().postMessage({ op: "many", signal, url, offsets, lens, data, fanout: FANOUT });
  Atomics.wait(signal, 0, 0);
  return signal[0] === 1 ? new Uint8Array(data) : null;
}

/**
 * Install as the global XMLHttpRequest if none exists, and the parallel-read
 * hook alongside it (idempotent). The hook goes in only while the XHR in use
 * is this one: a caller-supplied XHR may carry auth or a proxy that the
 * hook's own fetch would bypass, so that caller keeps every read. Set
 * `globalThis.reteReadMany = null` before the first open() to keep reads
 * strictly one at a time.
 */
export function install() {
  if (typeof globalThis.XMLHttpRequest === "undefined") {
    globalThis.XMLHttpRequest = NodeSyncXMLHttpRequest;
  }
  if (globalThis.XMLHttpRequest === NodeSyncXMLHttpRequest && globalThis.reteReadMany === undefined) {
    globalThis.reteReadMany = readMany;
  }
}
