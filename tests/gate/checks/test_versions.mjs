import assert from "node:assert/strict";
import fs from "node:fs";


const SOURCE = "/work/web/playground-src/versions.js";
assert.ok(fs.existsSync(SOURCE), "versions.js must exist");

const window = {};
new Function("window", fs.readFileSync(SOURCE, "utf8"))(window);
const api = window.RETE_PLAYGROUND_VERSIONS;
assert.ok(api, "versions.js must expose RETE_PLAYGROUND_VERSIONS");

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

assert.equal(
  api.previewUrl(canonical),
  `https://preview.graphplaza.com/pr-72/${SHA}/playground.html`,
);
assert.equal(
  api.versionHref(
    "https://preview.graphplaza.com/pr-72/x/playground.html",
    "#dataset=bcn&load=lazy&mode=sparql&ex=3",
  ),
  "https://preview.graphplaza.com/pr-72/x/playground.html#dataset=bcn&load=lazy&mode=sparql&ex=3",
);
assert.equal(api.eligiblePull(canonical), true);
assert.equal(api.eligiblePull(fork), false);
assert.equal(api.eligiblePull({ number: 1, head: { sha: "short", repo: { full_name: "caviri/rete" } } }), false);

function memoryStorage() {
  const values = new Map();
  return {
    getItem: (key) => values.has(key) ? values.get(key) : null,
    setItem: (key, value) => values.set(key, String(value)),
  };
}

const calls = [];
const fetch = async (url, options = {}) => {
  calls.push({ url, method: options.method || "GET" });
  if (String(url).startsWith("https://api.github.com/")) {
    return { ok: true, json: async () => [canonical, fork] };
  }
  return { ok: url === api.previewUrl(canonical) };
};
const storage = memoryStorage();
const first = await api.discoverPreviews({ fetch, storage, now: () => 1000 });
assert.deepEqual(first, [{
  number: 72,
  title: "Add streaming parser <unsafe>",
  sha: SHA,
  url: api.previewUrl(canonical),
}]);
assert.deepEqual(calls.map((call) => call.method), ["GET", "HEAD"]);

const second = await api.discoverPreviews({ fetch, storage, now: () => 2000 });
assert.deepEqual(second, first, "fresh session cache must return the same previews");
assert.equal(calls.length, 2, "fresh cache must avoid GitHub and R2 requests");

const malformed = await api.discoverPreviews({
  fetch: async () => ({ ok: true, json: async () => ({ not: "an array" }) }),
  storage: null,
  now: () => 0,
});
assert.deepEqual(malformed, []);

const unavailable = await api.discoverPreviews({
  fetch: async (url) => String(url).startsWith("https://api.github.com/")
    ? { ok: true, json: async () => [canonical] }
    : { ok: false },
  storage: null,
  now: () => 0,
});
assert.deepEqual(unavailable, []);

console.log(JSON.stringify({
  verdict: "PASS",
  previewUrl: api.previewUrl(canonical),
  discovered: first.length,
  cacheRequests: calls.length,
}));
