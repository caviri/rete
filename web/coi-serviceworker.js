/* coi-serviceworker — enables cross-origin isolation on static hosts (GitHub
 * Pages, Hugging Face) that don't send COOP/COEP, so SharedArrayBuffer (and
 * thus the rete explorer's parallel range reads) becomes available.
 *
 * It re-serves same-origin responses with `Cross-Origin-Opener-Policy:
 * same-origin` and `Cross-Origin-Embedder-Policy: credentialless`. The
 * credentialless variant keeps cross-origin no-credential subresources (the
 * Hugging Face range fetches) working without requiring CORP on them. The page
 * registers this worker and reloads once; if it's unavailable the explorer
 * still works, reading ranges sequentially.
 *
 * Based on the widely-used coi-serviceworker (Apache-2.0, gzuidhof).
 */
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

self.addEventListener("message", (ev) => {
  if (ev.data && ev.data.type === "deregister") {
    self.registration.unregister().then(() => self.clients.matchAll()).then((cs) =>
      cs.forEach((c) => c.navigate(c.url)));
  }
});

self.addEventListener("fetch", (event) => {
  const r = event.request;
  // Only the top-level navigation needs the isolation headers; the browser then
  // enforces COEP on subresources itself. Crucially we must NOT re-wrap
  // cross-origin subresources (the DuckDB-WASM / sql.js-httpvfs worker scripts
  // and streamed `.wasm` from their CDNs) — doing so breaks their load. Pass
  // everything that isn't a navigation straight through, untouched.
  if (r.mode !== "navigate") return;

  event.respondWith(
    fetch(r)
      .then((response) => {
        if (response.status === 0) return response; // opaque — leave as-is
        const headers = new Headers(response.headers);
        headers.set("Cross-Origin-Embedder-Policy", "credentialless");
        headers.set("Cross-Origin-Opener-Policy", "same-origin");
        return new Response(response.body, {
          status: response.status,
          statusText: response.statusText,
          headers,
        });
      })
      .catch((e) => console.error(e))
  );
});
