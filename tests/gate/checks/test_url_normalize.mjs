import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

const source = await readFile("/work/web/playground-src/app.js", "utf8");
const marker = "function normalizeReteUrl(raw)";
const start = source.indexOf(marker);
assert.notEqual(start, -1, "normalizeReteUrl is missing from app.js");

const bodyStart = source.indexOf("{", start);
let depth = 0;
let end = -1;
for (let i = bodyStart; i < source.length; i += 1) {
  if (source[i] === "{") depth += 1;
  if (source[i] === "}") {
    depth -= 1;
    if (depth === 0) { end = i + 1; break; }
  }
}
assert.notEqual(end, -1, "normalizeReteUrl has no closing brace");

const context = { URL };
vm.runInNewContext(`${source.slice(start, end)}; this.normalize = normalizeReteUrl;`, context);
const normalize = context.normalize;

const accepted = new Map([
  ["localhost:8090/x.rete", "https://localhost:8090/x.rete"],
  ["example.com:8443/x.rete", "https://example.com:8443/x.rete"],
  ["[::1]:8090/x.rete", "https://[::1]:8090/x.rete"],
  ["//example.com/x.rete", "https://example.com/x.rete"],
  ["http://example.com/x.rete", "http://example.com/x.rete"],
]);
for (const [input, expected] of accepted) {
  assert.equal(normalize(input), expected, input);
}

for (const input of [
  "javascript:alert(1)",
  "data:application/octet-stream,abc",
  "https:",
  "https:///x.rete",
  "http://",
  "example.com:99999/x.rete",
]) {
  assert.equal(normalize(input), null, input);
}

console.log(JSON.stringify({ verdict: "PASS", checks: accepted.size + 6 }));
