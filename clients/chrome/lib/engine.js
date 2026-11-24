// Promise client for the engine worker. The transport is either
//   - a Worker (extension pages: standalone viewer), or
//   - a MessagePort that engine.html (a hidden extension iframe, one per HF page)
//     bridges to its Worker. Both speak {id, op, ...} / {id, ok, result|error}.
// Exposes window.RetelensEngine.
(function () {
  class Engine {
    constructor(transport) {
      this.seq = 1;
      this.pending = new Map();
      this.isReady = false;
      this.ready = new Promise((res) => (this._ready = res));
      this.attach(transport || new Worker(chrome.runtime.getURL("rete-worker.js")));
    }
    attach(transport) {
      this.t = transport;
      this.t.onmessage = (ev) => {
        const m = ev.data;
        if (!m) return;
        if (m.id === 0) { this.isReady = true; return this._ready(); }
        const p = this.pending.get(m.id);
        if (!p) return;
        this.pending.delete(m.id);
        if (m.ok) p.resolve(m.result);
        else p.reject(Object.assign(new Error(m.error.message), m.error));
      };
      this.t.onerror = (e) => {
        const err = Object.assign(new Error("engine worker failed: " + (e.message || "load error")), { code: "worker" });
        for (const p of this.pending.values()) p.reject(err);
        this.pending.clear();
      };
      if (this.t.start) this.t.start(); // MessagePort
    }
    // re-send everything still waiting (used after the transport was replaced)
    resend() { for (const p of this.pending.values()) this.t.postMessage(p.msg); }
    call(op, args = {}) {
      return new Promise((resolve, reject) => {
        const id = this.seq++;
        const msg = { id, op, ...args };
        this.pending.set(id, { resolve, reject, msg });
        this.t.postMessage(msg);
      });
    }
    open(url) { return this.call("open", { url }); }
    openLocal(file, opts) { return this.call("openLocal", { file, lazy: !!(opts && opts.lazy) }); }
    meta() { return this.call("meta"); }
    examples() { return this.call("examples"); }
    schema() { return this.call("schema"); }
    run(query) { return this.call("run", { query }); }
    page(handle, offset, limit) { return this.call("page", { handle, offset, limit }); }
    free(handle) { return this.call("free", { handle }); }
    stats() { return this.call("stats"); }
    search(text, limit) { return this.call("search", { text, limit }); }
  }

  // Content scripts: create the hidden engine iframe and hand it one MessagePort.
  Engine.viaIframe = function () {
    const url = chrome.runtime.getURL("engine.html");
    const origin = new URL(url).origin;
    const spawn = () => {
      const f = document.createElement("iframe");
      f.src = url;
      f.setAttribute("data-retelens-engine", "1");
      f.setAttribute("aria-hidden", "true");
      f.tabIndex = -1;
      f.style.cssText = "position:fixed;width:0;height:0;border:0;opacity:0;pointer-events:none;left:-10px;top:-10px";
      const loaded = new Promise((res) => f.addEventListener("load", res, { once: true }));
      document.body.append(f);
      const ch = new MessageChannel();
      loaded.then(() => f.contentWindow.postMessage({ retelens: "port" }, origin, [ch.port2]));
      return { f, port: ch.port1 };
    };
    const first = spawn();
    const eng = new Engine(first.port);
    eng.frame = first.f;
    // watchdog: if the worker never says it is ready, start over once with a fresh frame
    setTimeout(() => {
      if (eng.isReady) return;
      console.debug("[retelens] engine not ready after 6 s, restarting the engine frame");
      first.f.remove();
      const again = spawn();
      eng.frame = again.f;
      eng.attach(again.port);
      eng.resend();
    }, 6000);
    return eng;
  };

  window.RetelensEngine = Engine;
})();
