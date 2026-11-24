// Hidden engine iframe (extension origin), one per host page. It owns the Worker
// (sync XHR for lazy range reads only works in workers) and bridges it to the
// content script over ONE MessagePort: the first port it is handed, from its parent.
(function () {
  let taken = false;
  window.addEventListener("message", (e) => {
    if (taken || e.source !== parent) return;
    if (e.data?.retelens !== "port" || !e.ports || !e.ports[0]) return;
    taken = true;
    const port = e.ports[0];
    const worker = new Worker(chrome.runtime.getURL("rete-worker.js"));
    port.onmessage = (ev) => worker.postMessage(ev.data);
    worker.onmessage = (ev) => port.postMessage(ev.data);
    port.start();
  });
})();
