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

parentPort.on("message", async ({ port, signal, method, url, headers, body }) => {
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

let worker = null;

function syncFetch(method, url, headers, body) {
  if (!worker) {
    worker = new Worker(WORKER_SRC, { eval: true });
    worker.unref(); // never keep the process alive on our account
  }
  const signal = new Int32Array(new SharedArrayBuffer(4));
  const { port1, port2 } = new MessageChannel();
  worker.postMessage({ port: port2, signal, method, url, headers, body }, [port2]);
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

/** Install as the global XMLHttpRequest if none exists (idempotent). */
export function install() {
  if (typeof globalThis.XMLHttpRequest === "undefined") {
    globalThis.XMLHttpRequest = NodeSyncXMLHttpRequest;
  }
}
