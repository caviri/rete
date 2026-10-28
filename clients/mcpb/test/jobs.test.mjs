// Query jobs, driven over stdio exactly as Claude Desktop drives the server.
// The properties that matter to a client with a 60 s limit per call:
//   - a slow query answers with a job id instead of outliving the call, keeps
//     running, and is collected later;
//   - the server stays responsive while it runs (other queries, status, cancel);
//   - cancelling — explicitly, or by the client abandoning the call — stops it;
//   - every row is reachable: paged by cursor, or saved to a file.
// A slow graph is a small one behind a Range server that holds every answer
// back, so "slow" is deterministic and needs no network.
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
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
const N = 450; // more rows than one answer page (200)
const EX = "https://example.org/";

function numbered() {
  const lines = [];
  for (let i = 0; i < N; i++) {
    lines.push(`<${EX}item/${String(i).padStart(4, "0")}> <${EX}n> "${i}"^^<http://www.w3.org/2001/XMLSchema#integer> .`);
  }
  return lines.join("\n") + "\n";
}
const ALL = `SELECT ?s ?n WHERE { ?s <${EX}n> ?n }`;

/** A granted folder holding numbered.rete. */
async function fixture() {
  const dir = await mkdtemp(join(tmpdir(), "rete-mcpb-jobs-"));
  await writeFile(join(dir, "numbered.rete"), await build(numbered(), "nt"));
  return dir;
}

/** numbered.rete over HTTP, every answer held back by `delayMs`. */
async function slowGraph(t, delayMs) {
  const bytes = Buffer.from(await build(numbered(), "nt"));
  const server = createServer((req, res) => {
    setTimeout(() => {
      const m = /bytes=(\d+)-(\d*)/.exec(req.headers.range ?? "");
      if (!m) {
        res.writeHead(200, { "Content-Length": bytes.length, "Accept-Ranges": "bytes" });
        return res.end(req.method === "HEAD" ? undefined : bytes);
      }
      const start = Number(m[1]);
      const end = m[2] ? Math.min(Number(m[2]), bytes.length - 1) : bytes.length - 1;
      res.writeHead(206, {
        "Content-Range": `bytes ${start}-${end}/${bytes.length}`,
        "Content-Length": end - start + 1,
      });
      res.end(bytes.subarray(start, end + 1));
    }, delayMs);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => {
    server.closeAllConnections();
    server.close();
  });
  return `http://127.0.0.1:${server.address().port}/numbered.rete`;
}

async function connect(t, dir) {
  const client = new Client({ name: "jobs", version: "0" });
  await client.connect(new StdioClientTransport({ command: process.execPath, args: [SERVER, dir] }));
  t.after(() => client.close());
  const call = (name, args, options) => client.callTool({ name, arguments: args }, undefined, options);
  return { client, call };
}

const payload = (result) => {
  assert.ok(!result.isError, `tool errored: ${result.content?.[0]?.text}`);
  return JSON.parse(result.content[0].text);
};

test("serves the job tools", async (t) => {
  const { client } = await connect(t, await fixture());
  const names = (await client.listTools()).tools.map((tool) => tool.name);
  for (const name of ["start_query", "query_status", "query_result", "cancel_query"]) {
    assert.ok(names.includes(name), `missing ${name}`);
  }
});

test("every row is reachable: pages by cursor, nothing re-run", async (t) => {
  const { call } = await connect(t, await fixture());
  const first = payload(await call("sparql_query", { dataset: "numbered", query: ALL }));
  assert.equal(first.count, N);
  assert.equal(first.rows.length, 200);
  assert.equal(first.truncated, N - 200);
  assert.deepEqual(first.vars, ["s", "n"]);
  assert.equal(first.next_cursor, "200");
  assert.match(first.note, /query_result/);
  assert.match(first.job_id, /^q\d+$/);

  const seen = new Set(first.rows.map((r) => r.s));
  let cursor = first.next_cursor;
  while (cursor) {
    const page = payload(await call("query_result", { job_id: first.job_id, cursor }));
    assert.equal(page.offset, Number(cursor));
    for (const row of page.rows) seen.add(row.s);
    cursor = page.next_cursor;
  }
  assert.equal(seen.size, N, "the pages together hold every row once");

  const big = payload(await call("query_result", { job_id: first.job_id, limit: 1000 }));
  assert.equal(big.rows.length, N);
  assert.equal(big.next_cursor, null);
  assert.equal(typeof big.rows[0].n, "number", "typed literals keep their JSON type");

  const bad = await call("query_result", { job_id: first.job_id, cursor: "-3" });
  assert.ok(bad.isError);
  assert.match(bad.content[0].text, /not a cursor/);
  const gone = await call("query_result", { job_id: "q999" });
  assert.ok(gone.isError);
  assert.match(gone.content[0].text, /no query q999/);
});

test("save_as writes every row into the granted folder, never over a file", async (t) => {
  const dir = await fixture();
  const { call } = await connect(t, dir);
  const { job_id } = payload(await call("sparql_query", { dataset: "numbered", query: `${ALL} ORDER BY ?n` }));

  const csv = payload(await call("query_result", { job_id, save_as: "items.csv" })).saved;
  assert.equal(csv.path, join(dir, "items.csv"));
  assert.equal(csv.rows, N);
  const lines = (await readFile(csv.path, "utf8")).trimEnd().split("\r\n");
  assert.equal(lines.length, N + 1);
  assert.equal(lines[0], "s,n");
  assert.equal(lines[1], `${EX}item/0000,0`);

  const again = payload(await call("query_result", { job_id, save_as: "items.csv" })).saved;
  assert.equal(again.path, join(dir, "items-2.csv"), "an existing file is never overwritten");

  const asJson = payload(await call("query_result", { job_id, save_as: "items.json" })).saved;
  const parsed = JSON.parse(await readFile(asJson.path, "utf8"));
  assert.equal(parsed.length, N);
  assert.deepEqual(parsed[0], { s: `${EX}item/0000`, n: 0 });

  const outside = await call("query_result", { job_id, save_as: join(tmpdir(), "escape.csv") });
  assert.ok(outside.isError);
  assert.match(outside.content[0].text, /outside the folders/);
  const wrongKind = await call("query_result", { job_id, save_as: "items.nt" });
  assert.ok(wrongKind.isError);

  const triples = payload(
    await call("sparql_query", { dataset: "numbered", query: `CONSTRUCT { ?s <${EX}m> ?n } WHERE { ?s <${EX}n> ?n }` }),
  );
  assert.equal(triples.count, N);
  const nt = payload(await call("query_result", { job_id: triples.job_id, save_as: "items" })).saved;
  assert.equal(nt.format, "nt");
  assert.equal((await readFile(nt.path, "utf8")).trimEnd().split("\n").length, N);
});

test("a query that outlives the wait comes back as a job, and is collected later", async (t) => {
  const url = await slowGraph(t, 400);
  const { call } = await connect(t, await fixture());

  const started = Date.now();
  const running = payload(await call("sparql_query", { dataset: url, query: ALL, wait_seconds: 0.5 }));
  assert.ok(Date.now() - started < 5_000, "the call answered without waiting for the query");
  assert.ok(["queued", "running"].includes(running.state), running.state);
  assert.match(running.note, /query_status/);
  assert.match(running.note, /Do NOT send the query again/);

  let done = payload(await call("query_status", { job_id: running.job_id, wait_seconds: 30 }));
  while (done.state === "running") {
    done = payload(await call("query_status", { job_id: running.job_id, wait_seconds: 30 }));
  }
  assert.equal(done.count, N, "query_status hands over the answer once done");
  assert.equal(done.rows.length, 200);
  assert.equal(done.stats.where, "remote (HTTP Range)");

  const listed = payload(await call("query_status", {})).jobs;
  const mine = listed.find((j) => j.job_id === running.job_id);
  assert.equal(mine.state, "done");
  assert.equal(mine.progress.rows, N);
  assert.ok(mine.progress.reads > 0, "reads were counted");
});

test("the server answers other calls while a query runs, and cancel stops it", async (t) => {
  const url = await slowGraph(t, 5_000);
  const { call } = await connect(t, await fixture());

  const slow = payload(await call("start_query", { dataset: url, query: ALL }));
  assert.equal(slow.state, "running");

  const t0 = Date.now();
  const quick = payload(await call("sparql_query", { dataset: "numbered", query: "ASK { ?s ?p ?o }" }));
  assert.equal(quick.boolean, true);
  assert.ok(Date.now() - t0 < 4_000, `a quick query waited ${Date.now() - t0} ms behind the slow one`);

  const still = payload(await call("query_status", { job_id: slow.job_id, wait_seconds: 0 }));
  assert.equal(still.state, "running");

  const cancelled = payload(await call("cancel_query", { job_id: slow.job_id }));
  assert.equal(cancelled.state, "cancelled");
  const after = await call("query_status", { job_id: slow.job_id });
  assert.ok(after.isError);
  assert.match(after.content[0].text, /cancelled/);

  // The worker it ran on is gone; the next query gets a fresh one.
  assert.equal(payload(await call("sparql_query", { dataset: "numbered", query: ALL })).count, N);
});

test("a client that abandons the call cancels its query", async (t) => {
  const url = await slowGraph(t, 5_000);
  const { call } = await connect(t, await fixture());

  const abort = new AbortController();
  const pending = call("sparql_query", { dataset: url, query: ALL, wait_seconds: 30 }, { signal: abort.signal });
  setTimeout(() => abort.abort(), 500);
  await assert.rejects(pending);

  let job;
  for (let i = 0; i < 50 && job?.state !== "cancelled"; i++) {
    [job] = payload(await call("query_status", {})).jobs;
    if (job?.state !== "cancelled") await new Promise((r) => setTimeout(r, 100));
  }
  assert.equal(job.state, "cancelled");
});

test("a narrow list_datasets filter returns full descriptions", async (t) => {
  const { call } = await connect(t, await fixture());
  const all = payload(await call("list_datasets", { source: "published" })).published;
  // Any graph whose description the listing abridges, found again by its
  // label — no particular catalogue entry's wording is assumed.
  let checked = 0;
  for (const abridged of all.filter((d) => d.description?.endsWith("…"))) {
    const narrow = payload(await call("list_datasets", { query: abridged.label })).published;
    if (narrow.length > 3) continue; // a label shared too widely to narrow the list
    const full = narrow.find((d) => d.key === abridged.key);
    assert.ok(full.description.length > abridged.description.length, abridged.key);
    assert.ok(!full.description.endsWith("…"), abridged.key);
    if (++checked === 3) break;
  }
  assert.ok(checked > 0, "no abridged description could be narrowed to");

  const broad = payload(await call("list_datasets", { query: "a" })).published;
  assert.ok(broad.length > 3);
  assert.ok(broad.some((d) => d.description?.endsWith("…")), "a broad filter still abridges");
});
