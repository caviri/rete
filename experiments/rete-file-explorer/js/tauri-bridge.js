// tauri-bridge.js — make the native engine look exactly like the wasm worker.
//
// The browser build talks to a Web Worker that owns a wasm `RemoteGraph`; the
// desktop build talks to Rust commands driving `rete-core` natively. Rather than
// branch the app on which one is present, this exposes the *same* interface a
// `Worker` does — `postMessage`, `onmessage`, `terminate` — so `app.js`
// constructs one or the other in a single line and nothing downstream changes.
//
// That is the claim `rete-fs.js` has been making all along: the engine is a
// seam. This file is the proof.

export const isTauri = () =>
  typeof window !== "undefined" && !!(window.__TAURI__ || window.__TAURI_INTERNALS__);

const invoke = (cmd, args) => {
  const core = window.__TAURI__ && window.__TAURI__.core;
  if (!core || typeof core.invoke !== "function") {
    return Promise.reject(new Error("Tauri core API unavailable"));
  }
  return core.invoke(cmd, args);
};

/**
 * A stand-in for `new Worker("./js/fs-worker.js")`.
 *
 * The worker replies asynchronously and echoes `reqId`; so does this. It also
 * mirrors the worker's habit of attaching a `stats` object to every reply, which
 * is what drives the traffic meter — on the desktop side those are real
 * positional/HTTP reads counted by the same `CountingReader` the CLI uses.
 */
export function makeTauriWorkerShim() {
  const shim = {
    onmessage: null,
    onerror: null,
    terminate() {},
    postMessage(msg) {
      handle(msg).then((reply) => {
        if (shim.onmessage) shim.onmessage({ data: reply });
      });
    },
  };

  const reply = async (msg, payload) => {
    let stats = null;
    try {
      const s = await invoke("graph_stats");
      stats = { fileLength: s.fileLength, bytes: s.bytes, requests: s.requests };
    } catch (_) {
      // Before the first open there is nothing to report; the meter just waits.
    }
    return { ...payload, type: msg.type, reqId: msg.reqId, stats };
  };

  async function handle(msg) {
    try {
      if (msg.type === "open") {
        // The native side hands back the raw 1 KB header so the front end parses
        // it with the same `parseHeader` the browser build uses — one
        // implementation of the section directory, not two.
        const info = await invoke("open_graph", { source: msg.source });
        return await reply(msg, {
          ok: true,
          head: info.head,
          size: info.size,
          cardText: info.card,
          schema: info.schema ? JSON.parse(info.schema) : null,
          schemaError: info.schemaError || null,
          contentHash: info.contentHash,
        });
      }

      if (msg.type === "query") {
        const json = await invoke("run_query", {
          sparql: msg.sparql,
          format: msg.format || "table",
        });
        return await reply(msg, { ok: true, json });
      }

      if (msg.type === "prefix") {
        const results = await invoke("prefix_search", {
          prefix: msg.prefix,
          limit: msg.limit || 40,
        });
        return await reply(msg, { ok: true, results });
      }

      if (msg.type === "text") {
        const results = await invoke("text_search", {
          words: msg.words,
          containsPrefix: msg.containsPrefix ?? null,
          limit: msg.limit || 40,
        });
        return await reply(msg, { ok: true, results });
      }

      if (msg.type === "stats") return await reply(msg, { ok: true });

      // Fail loudly rather than pretending an empty result set is an answer.
      throw new Error(`command not available in the desktop build: ${msg.type}`);
    } catch (err) {
      return {
        type: msg.type,
        reqId: msg.reqId,
        ok: false,
        error: String(err && err.message ? err.message : err),
        stats: null,
      };
    }
  }

  return shim;
}

/** Ask the OS for a `.rete` and return its path, or null if cancelled. */
export async function pickReteFile() {
  const dialog = window.__TAURI__ && window.__TAURI__.dialog;
  if (!dialog) throw new Error("dialog plugin unavailable");
  const picked = await dialog.open({
    multiple: false,
    directory: false,
    filters: [{ name: "Rete graph", extensions: ["rete"] }],
  });
  return typeof picked === "string" ? picked : null;
}
