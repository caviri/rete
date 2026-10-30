// Partitioned queries (partition_by), over stdio as Claude Desktop drives them:
//   - a split query answers exactly what the whole query answers;
//   - its chunks are checkpointed, and the same call sent to a NEW server —
//     Claude restarted — carries on from them instead of starting over;
//   - a split that would change the answer is refused, saying why;
//   - cancelling keeps the chunks; an identical call joins a running one.
// RETE_MCPB_CHUNK fixes the chunk size, so the chunk counts do not depend on
// how fast this machine is.
import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { build } from "rete-graph";

const here = dirname(fileURLToPath(import.meta.url));
const SERVER = join(here, "..", "build", "server", "index.mjs");
const EX = "https://example.org/";
const N = 120; // works; every other one cites :X, every fourth has a year
const CHUNK = 7;

function works() {
  const lines = [];
  for (let i = 0; i < N; i++) {
    const w = `<${EX}work/${String(i).padStart(3, "0")}>`;
    lines.push(`${w} <${EX}title> "Work ${i}" .`);
    lines.push(`${w} <${EX}author> <${EX}person/${i % 9}> .`);
    if (i % 2 === 0) lines.push(`${w} <${EX}cites> <${EX}X> .`);
    if (i % 4 === 0) lines.push(`${w} <${EX}year> "${1900 + i}"^^<http://www.w3.org/2001/XMLSchema#integer> .`);
  }
  return lines.join("\n") + "\n";
}

const P = `PREFIX : <${EX}> `;
const DETAILS = `${P}SELECT ?w ?t ?y WHERE { ?w :cites :X ; :title ?t OPTIONAL { ?w :year ?y } }`;

async function fixture() {
  const dir = await mkdtemp(join(tmpdir(), "rete-mcpb-part-"));
  await writeFile(join(dir, "works.rete"), await build(works(), "nt"));
  const runs = await mkdtemp(join(tmpdir(), "rete-mcpb-runs-"));
  return { dir, runs };
}

/** A fresh server — a fresh Claude Desktop session — sharing `runs` with any other. */
async function connect(t, { dir, runs }) {
  const client = new Client({ name: "partition", version: "0" });
  await client.connect(
    new StdioClientTransport({
      command: process.execPath,
      args: [SERVER, dir],
      env: { ...process.env, RETE_MCPB_RUNS: runs, RETE_MCPB_CHUNK: String(CHUNK) },
    }),
  );
  t.after(() => client.close());
  const call = (name, args) => client.callTool({ name, arguments: args });
  return { client, call };
}

const payload = (result) => {
  assert.ok(!result.isError, `tool errored: ${result.content?.[0]?.text}`);
  return JSON.parse(result.content[0].text);
};

/** Wait for a job to end; its answer. */
async function collect(call, answer) {
  while (answer.state === "running" || answer.state === "queued") {
    answer = payload(await call("query_status", { job_id: answer.job_id, wait_seconds: 10 }));
  }
  return answer;
}

const sorted = (rows) => rows.map((r) => JSON.stringify(r)).sort();

test("a split query is the whole query, and a new session resumes it from its checkpoint", async (t) => {
  const fx = await fixture();
  const first = await connect(t, fx);
  const whole = payload(await first.call("sparql_query", { dataset: "works", query: DETAILS }));
  assert.equal(whole.count, N / 2);

  const split = await collect(
    first.call,
    payload(await first.call("sparql_query", { dataset: "works", query: DETAILS, partition_by: "?w" })),
  );
  assert.deepEqual(sorted(split.rows), sorted(whole.rows));
  const chunks = Math.ceil(N / 2 / CHUNK);
  assert.deepEqual(split.partition, { by: "?w", keys: N / 2, chunks, resumed_keys: 0, keys_from: "pattern" });

  // The checkpoint: the plan, the keys, one file per chunk.
  const [run] = await readdir(fx.runs);
  const files = await readdir(join(fx.runs, run));
  assert.ok(files.includes("state.json") && files.includes("keys.json"), String(files));
  const parts = files.filter((f) => /^c\d+-\d+\.json$/.test(f)).sort();
  assert.equal(parts.length, chunks);

  // Interrupted before three of its chunks came in: their files are missing.
  const lost = parts.slice(0, 3);
  for (const f of lost) await rm(join(fx.runs, run, f));
  const lostKeys = lost.map((f) => f.match(/c(\d+)-(\d+)/)).reduce((n, m) => n + (m[2] - m[1]), 0);
  await first.client.close();

  // Claude restarted: a new server, the same call.
  const second = await connect(t, fx);
  const resumed = await collect(
    second.call,
    payload(await second.call("sparql_query", { dataset: "works", query: DETAILS, partition_by: "w" })),
  );
  assert.equal(resumed.partition.resumed_keys, N / 2 - lostKeys);
  assert.deepEqual(sorted(resumed.rows), sorted(whole.rows));

  // Another query does not borrow that checkpoint.
  const other = await collect(
    second.call,
    payload(
      await second.call("sparql_query", {
        dataset: "works",
        query: `${P}SELECT ?w ?a WHERE { ?w :cites :X ; :author ?a }`,
        partition_by: "?w",
      }),
    ),
  );
  assert.equal(other.partition.resumed_keys, 0);
  assert.equal(other.count, N / 2);
});

test("LIMIT stops early, DISTINCT and CONSTRUCT are deduplicated, and refusals say why", async (t) => {
  const { call } = await connect(t, await fixture());
  const run = async (query, extra = {}) =>
    collect(call, payload(await call("sparql_query", { dataset: "works", query, partition_by: "?w", ...extra })));

  const limited = await run(`${P}SELECT ?w WHERE { ?w :cites :X } LIMIT 10 OFFSET 3`);
  assert.equal(limited.count, 10);
  assert.ok(limited.partition.chunks < Math.ceil(N / 2 / CHUNK), `ran ${limited.partition.chunks} chunks`);

  const authors = await run(`${P}SELECT DISTINCT ?a WHERE { ?w :cites :X ; :author ?a }`);
  assert.equal(authors.count, 9);

  const graph = await run(`${P}CONSTRUCT { :X :citedBy ?w } WHERE { ?w :cites :X }`);
  assert.equal(graph.count, N / 2);

  for (const [query, why] of [
    [`${P}SELECT ?w WHERE { ?w :cites :X } ORDER BY ?w`, /ORDER BY/],
    [`${P}SELECT (COUNT(?w) AS ?n) WHERE { ?w :cites :X }`, /aggregate/],
    [`${P}SELECT ?w ?y WHERE { ?w :title ?t OPTIONAL { ?w :year ?y } }`, /unbound/],
  ]) {
    const by = why.source === "unbound" ? "?y" : "?w";
    const result = await call("sparql_query", { dataset: "works", query, partition_by: by });
    assert.ok(result.isError, `expected a refusal for ${query}`);
    assert.match(result.content[0].text, /cannot split this query/);
    assert.match(result.content[0].text, why);
  }
});

/** works.rete over HTTP, every answer held back by `delayMs` — slow enough to catch running. */
async function slowGraph(t, delayMs) {
  const bytes = Buffer.from(await build(works(), "nt"));
  const server = createServer((req, res) => {
    setTimeout(() => {
      const m = /bytes=(\d+)-(\d*)/.exec(req.headers.range ?? "");
      if (!m) {
        res.writeHead(200, { "Content-Length": bytes.length, "Accept-Ranges": "bytes" });
        return res.end(req.method === "HEAD" ? undefined : bytes);
      }
      const start = Number(m[1]);
      const end = m[2] ? Math.min(Number(m[2]), bytes.length - 1) : bytes.length - 1;
      res.writeHead(206, { "Content-Range": `bytes ${start}-${end}/${bytes.length}`, "Content-Length": end - start + 1 });
      res.end(bytes.subarray(start, end + 1));
    }, delayMs);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => {
    server.closeAllConnections();
    server.close();
  });
  return `http://127.0.0.1:${server.address().port}/works.rete`;
}

test("an identical call joins the running split; a cancelled one resumes when sent again", async (t) => {
  const fx = await fixture();
  const { call } = await connect(t, fx);
  const args = { dataset: await slowGraph(t, 300), query: DETAILS, partition_by: "?w" };
  const a = payload(await call("start_query", args));
  const b = payload(await call("start_query", args));
  assert.equal(b.job_id, a.job_id);
  assert.match(b.note, /joined it/);
  // Listed once, with its progress; its steps are not listed.
  const { jobs } = payload(await call("query_status", {}));
  assert.equal(jobs.length, 1);
  assert.equal(jobs[0].partition.by, "?w");

  const cancelled = payload(await call("cancel_query", { job_id: a.job_id }));
  assert.equal(cancelled.state, "cancelled");
  const again = await collect(call, payload(await call("sparql_query", args)));
  assert.notEqual(again.job_id, a.job_id);
  assert.equal(again.count, N / 2);
  const state = JSON.parse(await readFile(join(fx.runs, (await readdir(fx.runs))[0], "state.json"), "utf8"));
  assert.equal(state.done.length, Math.ceil(N / 2 / CHUNK));
});

test("the values can come from a VALUES list, or from another job's rows", async (t) => {
  const { call } = await connect(t, await fixture());
  const run = async (args) =>
    collect(call, payload(await call("sparql_query", { dataset: "works", partition_by: "?w", ...args })));

  // A VALUES list in the query is the keys as it stands; a listed work the
  // graph lacks just has no rows.
  const listed = await run({
    query: `${P}SELECT ?w ?t WHERE { VALUES ?w { <${EX}work/000> <${EX}work/002> <${EX}work/005> <${EX}work/nope> } ?w :title ?t }`,
  });
  assert.equal(listed.partition.keys_from, "values");
  assert.equal(listed.partition.keys, 4);
  assert.equal(listed.count, 3);

  // Entities listed by one query (in the real case, on another graph), the
  // details asked of a second one: keys_job hands the first job's rows over.
  const cited = payload(
    await call("sparql_query", { dataset: "works", query: `${P}SELECT ?w WHERE { ?w :cites :X } LIMIT 12` }),
  );
  // ?w :title ?t alone binds ?w with no constant: without keys_job its keys
  // would come from the whole WHERE clause.
  const details = await run({ query: `${P}SELECT ?w ?t WHERE { ?w :title ?t }`, keys_job: cited.job_id });
  assert.equal(details.partition.keys_from, `job ${cited.job_id}`);
  assert.equal(details.partition.keys, 12);
  assert.equal(details.count, 12);
  assert.deepEqual(details.rows.map((r) => r.w).sort(), cited.rows.map((r) => r.w).sort());

  // The listing's only column serves whatever its name.
  const renamed = payload(
    await call("sparql_query", { dataset: "works", query: `${P}SELECT ?paper WHERE { ?paper :cites :X } LIMIT 3` }),
  );
  assert.equal((await run({ query: `${P}SELECT ?w ?t WHERE { ?w :title ?t }`, keys_job: renamed.job_id })).count, 3);

  const refuse = async (args, why) => {
    const result = await call("sparql_query", { dataset: "works", query: `${P}SELECT ?w ?t WHERE { ?w :title ?t }`, ...args });
    assert.ok(result.isError, `expected a refusal for ${JSON.stringify(args)}`);
    assert.match(result.content[0].text, why);
  };
  await refuse({ keys_job: cited.job_id }, /goes with partition_by/);
  await refuse({ partition_by: "?w", keys_job: "q999" }, /no query q999/);
  const two = payload(
    await call("sparql_query", { dataset: "works", query: `${P}SELECT ?a ?b WHERE { ?a :cites ?b } LIMIT 2` }),
  );
  await refuse({ partition_by: "?w", keys_job: two.job_id }, /no \?w column/);
});
