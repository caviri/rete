// The playground's address normalizer (normalizeReteUrl in
// web/playground-src/app.js), run on its own in a VM context: the address field
// and #url= both pass through it, and what it lets by goes straight to the
// range reader.
//
//   node tests/gate/checks/test_url_normalize.mjs
//
// Two ways it went wrong (#235 audit, X7): `localhost:8090/x.rete` read as a URI
// with the scheme `localhost:` and was refused, and a bare `https:` or
// `https:///x` passed and failed later, deep in the reader.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { expect } from "./_expect.mjs";

const root = process.env.RETE_ROOT
  || path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const source = await readFile(`${root}/web/playground-src/app.js`, "utf8");
const t = expect("test_url_normalize");

// Lift the function out by brace matching: app.js is one big IIFE.
const marker = "function normalizeReteUrl(raw)";
const start = source.indexOf(marker);
t.ok("found", start >= 0, "normalizeReteUrl is missing from app.js");
let end = -1;
if (start >= 0) {
  let depth = 0;
  for (let i = source.indexOf("{", start); i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    if (source[i] === "}" && --depth === 0) { end = i + 1; break; }
  }
}
t.ok("closed", end > start, "normalizeReteUrl has no closing brace");

let cases = 0;
if (end > start) {
  const context = { URL };
  vm.runInNewContext(`${source.slice(start, end)}; this.normalize = normalizeReteUrl;`, context);
  const normalize = context.normalize;
  const accepted = [
    // A scheme-less address means https, as before.
    ["example.org/graph.rete", "https://example.org/graph.rete"],
    ["  example.org/graph.rete  ", "https://example.org/graph.rete"],
    ["//example.org/graph.rete", "https://example.org/graph.rete"],
    ["example.com:8443/x.rete", "https://example.com:8443/x.rete"],
    ["10.0.0.7:8080/x.rete", "https://10.0.0.7:8080/x.rete"],
    // host:port is an address, not the scheme `localhost:` — https like any
    // scheme-less address (check_url_param pins that for #url=).
    ["localhost:8090/x.rete", "https://localhost:8090/x.rete"],
    ["LOCALHOST:8090/x.rete", "https://LOCALHOST:8090/x.rete"],
    ["127.0.0.1:8090/x.rete", "https://127.0.0.1:8090/x.rete"],
    ["[::1]:8090/x.rete", "https://[::1]:8090/x.rete"],
    ["localhost:8090", "https://localhost:8090"],
    // An explicit http(s) URL is kept as typed.
    ["http://localhost:8090/x.rete", "http://localhost:8090/x.rete"],
    ["https://data.graphplaza.com/mtg/mtg.rete", "https://data.graphplaza.com/mtg/mtg.rete"],
    ["HTTPS://Example.org/X.rete?v=1#frag", "HTTPS://Example.org/X.rete?v=1#frag"],
  ];
  for (const [input, want] of accepted) {
    t.equal(`accept ${JSON.stringify(input)}`, normalize(input), want);
    cases += 1;
  }
  const refused = [
    "", "   ", null, undefined,
    // Schemes that are not http(s), whatever follows them.
    "javascript:alert(1)",
    "javascript:80/x",
    "data:application/octet-stream,abc",
    "file:///tmp/x.rete",
    "ftp://example.org/x.rete",
    // http(s) with no host: these used to pass.
    "https:",
    "http:",
    "https:/x.rete",
    "https:///x.rete",
    "http://",
    // A port no URL can carry.
    "example.com:99999/x.rete",
    "localhost:99999/x.rete",
  ];
  for (const input of refused) {
    t.equal(`refuse ${JSON.stringify(input)}`, normalize(input), null);
    cases += 1;
  }
}

t.finish({ cases });
