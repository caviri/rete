// The npm package is built with rete-wasm's `unicode-regex` feature
// (build-wasm.sh), so REGEX answers as the CLI does: `\b`, `\w` and `(?i)` know
// non-ASCII letters. The browser playground keeps the ASCII-only `regex-lite`
// (docs/sparql.md has the table). If this fails, the package was built without
// `--features unicode-regex`.
import assert from "node:assert/strict";
import test from "node:test";

import { build, open } from "../dist/index.js";

const NT = `<http://ex/a> <http://ex/name> "Jitka Svobodová" .
<http://ex/b> <http://ex/name> "SVOBODOVÁ, J." .
<http://ex/c> <http://ex/name> "Svobodováová" .
<http://ex/d> <http://ex/name> "Ludmila Dömeová" .
<http://ex/e> <http://ex/name> "plain ascii" .
`;

function subjects(g, filter) {
  const rows = g.query(`SELECT ?s WHERE { ?s <http://ex/name> ?n FILTER(${filter}) }`);
  return rows.map((r) => r.s.value).sort();
}

test("REGEX word boundaries, classes and case folding are Unicode", async () => {
  const g = await open(await build(NT, "nt"));
  // A boundary after "á", "Á" folding to "á", and no hit inside "Svobodováová".
  assert.deepEqual(subjects(g, String.raw`REGEX(?n, "\\bsvobodová\\b", "i")`), [
    "http://ex/a",
    "http://ex/b",
  ]);
  // `\w` covers "ö" and "á": each name is two words.
  assert.deepEqual(subjects(g, String.raw`REGEX(?n, "^\\w+ \\w+$")`), [
    "http://ex/a",
    "http://ex/d",
    "http://ex/e",
  ]);
});
