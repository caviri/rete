// versions.js — the PR-preview discovery contract. Assertions are COLLECTED, not
// thrown (see _expect.mjs), so a failure prints `{"verdict":"FAIL", failures:[…]}`
// with the value it actually got instead of dying on the first bad one.
import fs from "node:fs";
import { expect } from "./_expect.mjs";


const SOURCE = "/work/web/playground-src/versions.js";
const t = expect("test_versions");

const SHA = "91ac238000000000000000000000000000000000";
const canonical = {
  number: 72,
  title: "Add streaming parser <unsafe>",
  head: { sha: SHA, repo: { full_name: "caviri/rete" } },
};
const fork = {
  number: 73,
  title: "Fork preview",
  head: { sha: "c04d112000000000000000000000000000000000", repo: { full_name: "fork/rete" } },
};

let previewUrl = "";
let discovered = 0;
let calls = [];
try {
  if (!fs.existsSync(SOURCE)) throw new Error(`${SOURCE} must exist`);
  const window = {};
  new Function("window", fs.readFileSync(SOURCE, "utf8"))(window);
  const api = window.RETE_PLAYGROUND_VERSIONS;
  if (!api) throw new Error("versions.js must expose RETE_PLAYGROUND_VERSIONS");

  previewUrl = api.previewUrl(canonical);
  t.equal("previewUrl", previewUrl, `https://preview.graphplaza.com/pr-72/${SHA}/playground.html`);
  t.equal(
    "versionHref",
    api.versionHref(
      "https://preview.graphplaza.com/pr-72/x/playground.html",
      "#dataset=bcn&load=lazy&mode=sparql&ex=3",
    ),
    "https://preview.graphplaza.com/pr-72/x/playground.html#dataset=bcn&load=lazy&mode=sparql&ex=3",
  );
  t.equal("eligiblePull:canonical", api.eligiblePull(canonical), true);
  t.equal("eligiblePull:fork", api.eligiblePull(fork), false);
  t.equal("eligiblePull:shortSha",
    api.eligiblePull({ number: 1, head: { sha: "short", repo: { full_name: "caviri/rete" } } }), false);

  function memoryStorage() {
    const values = new Map();
    return {
      getItem: (key) => values.has(key) ? values.get(key) : null,
      setItem: (key, value) => values.set(key, String(value)),
    };
  }

  const fetch = async (url, options = {}) => {
    calls.push({ url, method: options.method || "GET" });
    if (String(url).startsWith("https://api.github.com/")) {
      return { ok: true, json: async () => [canonical, fork] };
    }
    return { ok: url === api.previewUrl(canonical) };
  };
  const storage = memoryStorage();
  const first = await api.discoverPreviews({ fetch, storage, now: () => 1000 });
  discovered = first.length;
  t.deepEqual("discoverPreviews", first, [{
    number: 72,
    title: "Add streaming parser <unsafe>",
    sha: SHA,
    url: api.previewUrl(canonical),
  }]);
  t.deepEqual("discoveryRequests", calls.map((call) => call.method), ["GET", "HEAD"]);

  const second = await api.discoverPreviews({ fetch, storage, now: () => 2000 });
  t.deepEqual("cachedPreviews", second, first, "fresh session cache must return the same previews");
  t.equal("cacheRequests", calls.length, 2, "fresh cache must avoid GitHub and R2 requests");

  const malformed = await api.discoverPreviews({
    fetch: async () => ({ ok: true, json: async () => ({ not: "an array" }) }),
    storage: null,
    now: () => 0,
  });
  t.deepEqual("malformedApiResponse", malformed, []);

  const unavailable = await api.discoverPreviews({
    fetch: async (url) => String(url).startsWith("https://api.github.com/")
      ? { ok: true, json: async () => [canonical] }
      : { ok: false },
    storage: null,
    now: () => 0,
  });
  t.deepEqual("unavailablePreview", unavailable, []);
} catch (error) {
  t.threw("versions.js contract", error);
}

t.finish({
  previewUrl,
  discovered,
  cacheRequests: calls.length,
});
