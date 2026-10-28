// A query split by one variable and run chunk by chunk — partitionPlan() plus
// queryRaw's `partition` option — answers exactly what the whole query does,
// on a graph in memory and on one read lazily over HTTP Range.
import assert from "node:assert/strict";
import test from "node:test";

import { build, open } from "../dist/index.js";
import { serveBytes } from "./range-server.mjs";

const EX = "http://ex.org/";
const P = `PREFIX : <${EX}> `;

function works() {
  const lines = [];
  for (let i = 0; i < 40; i++) {
    const w = `<${EX}w${i}>`;
    lines.push(`${w} <${EX}title> "Title ${i}" .`);
    lines.push(`${w} <${EX}author> <${EX}a${i % 5}> .`);
    if (i % 2 === 0) lines.push(`${w} <${EX}cites> <${EX}X> .`);
    if (i % 4 === 0) lines.push(`${w} <${EX}year> "${1990 + i}"^^<http://www.w3.org/2001/XMLSchema#integer> .`);
  }
  return lines.join("\n") + "\n";
}

/** The query in chunks of `size` keys, merged as the plan says. */
function chunked(g, query, variable, size) {
  const plan = g.partitionPlan(query, variable);
  const keys = g.queryRaw(plan.keys_query).rows.map((r) => r[plan.var]);
  const seen = new Set();
  let rows = [];
  for (let i = 0; i < keys.length; i += size) {
    const env = g.queryRaw(query, { partition: { var: plan.var, keys: keys.slice(i, i + size) } });
    for (const row of env.kind === "construct" ? env.triples : env.rows) {
      const k = JSON.stringify(row);
      if (plan.dedupe && seen.has(k)) continue;
      seen.add(k);
      rows.push(row);
    }
  }
  rows = rows.slice(plan.offset, plan.limit === null ? undefined : plan.offset + plan.limit);
  return { plan, rows, keys };
}

const sorted = (rows) => rows.map((r) => JSON.stringify(r)).sort();

function check(g) {
  for (const q of [
    "SELECT ?w ?t ?y WHERE { ?w :cites :X ; :title ?t OPTIONAL { ?w :year ?y } }",
    "SELECT DISTINCT ?a WHERE { ?w :cites :X ; :author ?a }",
    "SELECT ?w (COUNT(?a) AS ?n) WHERE { ?w :author ?a } GROUP BY ?w",
    "CONSTRUCT { ?w :named ?t } WHERE { ?w :cites :X ; :title ?t }",
  ]) {
    const whole = g.queryRaw(P + q);
    const { rows } = chunked(g, P + q, "?w", 3);
    assert.deepEqual(sorted(rows), sorted(whole.kind === "construct" ? whole.triples : whole.rows), q);
  }

  // The keys come from the one constant pattern, not from the titles.
  const { plan, keys } = chunked(g, `${P}SELECT ?w ?t WHERE { ?w :title ?t ; :cites :X }`, "w", 7);
  assert.equal(plan.keys_from_where, false);
  assert.match(plan.keys_query, /cites/);
  assert.equal(keys.length, 20);

  const limited = chunked(g, `${P}SELECT ?w WHERE { ?w :cites :X } LIMIT 4 OFFSET 3`, "w", 5);
  assert.deepEqual([limited.plan.offset, limited.plan.limit], [3, 4]);
  assert.equal(limited.rows.length, 4);

  assert.throws(() => g.partitionPlan(`${P}SELECT ?w WHERE { ?w :cites :X } ORDER BY ?w`, "w"), /ORDER BY/);
  assert.throws(
    () => g.partitionPlan(`${P}SELECT ?w ?y WHERE { ?w :title ?t OPTIONAL { ?w :year ?y } }`, "y"),
    /unbound/,
  );
}

test("chunks of a split query are the whole query, in memory", async () => {
  check(await open(await build(works(), "nt")));
});

test("chunks of a split query are the whole query, over HTTP Range", async () => {
  const server = await serveBytes(await build(works(), "nt"));
  try {
    check(await open(server.url));
  } finally {
    await server.close();
  }
});
