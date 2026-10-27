#!/usr/bin/env node
// rete.mcpb — the whole rete engine as a local MCP server for Claude Desktop.
//
// The engine is WebAssembly (crates/rete-wasm, via the rete-graph npm client),
// so this bundle is pure JavaScript plus one architecture-neutral `.wasm`: no
// native modules, no Python, no per-platform builds, and nothing to install.
// It queries `.rete` knowledge graphs wherever they are — files on the user's
// disk (read lazily by byte range, so size is not a limit) and the published
// catalogue on HTTP storage (same lazy reads, over Range requests) — and it
// can build new ones offline from RDF text.
//
// Transport is stdio, so NOTHING may be written to stdout but protocol
// frames; diagnostics go to stderr.
import { createWriteStream } from "node:fs";
import { access, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { once } from "node:events";
import { tmpdir } from "node:os";
import { basename, delimiter, dirname, extname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { Term, build } from "rete-graph";

import { GraphStore, UsageError, formatOf } from "./graphs.mjs";
import { JobManager } from "./jobs.mjs";
import { PartitionRuns } from "./partition.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
// Stamped by build.mjs from the workspace Cargo.toml; the fallback only shows
// up if this file is ever run unbundled.
const VERSION = typeof __RETE_VERSION__ === "string" ? __RETE_VERSION__ : "0.0.0-dev";
const MAX_ROWS = 200; // rows per answer page (query_result can ask for up to PAGE_MAX)
const PAGE_MAX = 1000;
const MAX_DESC = 320; // chars of a catalogue description in list_datasets…
const FULL_DESC = 3; // …unless a filter leaves this few, which get theirs whole
// How long a call waits for a query before answering with its job id. Claude
// gives up on a tool call after 60 s; answering well inside that keeps the
// call — and the query, which carries on either way — from being abandoned.
const WAIT_DEFAULT_S = 40;
const WAIT_MAX_S = 45;

const INSTRUCTIONS = `\
This server queries and builds .rete files — single-file RDF knowledge graphs
that are read LAZILY BY BYTE RANGE: only the bytes a query touches are ever
fetched, whether the file sits on this machine or on public HTTP storage. A
multi-gigabyte graph therefore answers a selective query in megabytes, and
local graphs work with no network at all.

Three kinds of dataset are reachable, and every tool's \`dataset\` argument
takes any of them — plus a full https:// URL to any published .rete:
  * LOCAL — .rete files in the folders the user granted this extension.
  * PUBLISHED — the public catalogue (data.graphplaza.com), read over HTTP.
  * CHAT — RDF the user shared in this conversation, loaded with load_rdf as
    "chat:<name>" and held in memory until the extension restarts.

Recommended workflow:
1. list_datasets — what is here, local and published. Descriptions are
   abridged; a \`query\` that matches only a few graphs returns theirs in full,
   caveats included.
2. dataset_card — the graph's own self-description: what it is, its licence,
   provenance, identity (creators, publisher, DOI, citation, when curated),
   counts, and how to query it well. Costs two small reads even on a 60 GB file.
3. dataset_schema — the classes and the subject-class/predicate/object-class
   relations, with counts. USE THESE IRIs EXACTLY; never invent a namespace.
   Check here that the relation you need is in THIS graph before querying it:
   two graphs about the same things can carry different links (one holds
   citations, another only the bibliographic records).
4. example_queries — runnable SPARQL the file ships with. The fastest route to
   a correct query shape; adapt one rather than writing from scratch.
5. sparql_query — SELECT / ASK / CONSTRUCT / DESCRIBE. Always keep a LIMIT
   while exploring. reason=true turns on OWL 2 QL entailment where the graph
   carries an ontology.
   CONTAINS / STRSTARTS / REGEX need strings: wrap an IRI or a number in STR(),
   e.g. CONTAINS(STR(?s), "x"); otherwise the row is silently dropped, even
   under ! (see warnings).
6. find_entities — resolve a name to IRIs before asking about a specific thing.
7. validate_shacl — data-quality checks with SHACL Core shapes.
8. build_rete — turn RDF text (Turtle / TriG / N-Triples / N-Quads) into a new
   .rete file in a granted folder; it is queryable immediately by its file name.
   For RDF 1.2 Turtle/TriG (<<( s p o )>>, reifiers, {| … |}) pass
   quoted_triple_syntax="rdf12"; the default reads << s p o >> as RDF-star.
9. load_rdf — make RDF shared in the chat queryable (below).

Files the user shares in the chat:
- Claude Desktop does not hand chat attachments to this extension, and a path
  you may see for one (/mnt/user-data/uploads/…) is in Claude's own sandbox,
  not on this machine. For RDF text — a .ttl, .nt, .nq or .trig file, or RDF
  pasted into the message — call load_rdf(name: "<the file's name>",
  rdf: <its full text, unchanged>), then pass "chat:<name>" as \`dataset\` to
  any tool. Too long for one call: send it in parts, every part after the
  first with append=true, each part ending where a statement ends.
- A table or document (CSV, JSON, RDF/XML, JSON-LD): convert it to Turtle
  yourself — one subject per record, IRIs minted under one base, a predicate
  per column — then load_rdf it.
- A .rete file is binary and cannot travel through the chat: ask the user to
  put it in a granted folder, or for its https:// URL. The same for an RDF
  file too large to send: once it sits in a granted folder, load_rdf reads it
  from there by \`path\`.
- save_as keeps a chat graph as a .rete file in a granted folder.

What a query costs — read this before a large extraction:
- A published graph is read over the network, one request per separate place
  in the file a query touches (a fraction of a second each; the engine fetches
  what it can in parallel batches). Cost follows how many SEPARATE entities a
  query visits, not how many rows it returns: one work, DOI or person costs a
  handful of reads; a property (title, year, …) for N entities scattered
  through a large file costs on the order of N reads. Entities stored near each
  other (same publisher, same IRI prefix) are cheaper.
- COUNT first. SELECT (COUNT(?x) AS ?n) costs a few reads and says whether you
  face 50 rows or 50,000.
- What a query read stays cached while the extension keeps that graph open
  (minutes of idling, not hours): re-running or retrying a query soon after
  costs almost nothing, so splitting a big job into steps wastes nothing.
  Cancelling is the exception — it discards what the cancelled query read.

Long queries and big results:
- A call answers within about 40 s. A query still running by then comes back
  as {job_id, state: "running"} and KEEPS RUNNING in the background. Wait for it
  with query_status(job_id) — up to 45 s per call — and call again until the
  state is "done"; that answer carries the first rows. NEVER send a running
  query again: the second copy competes with the first for the same network.
  Up to three queries run at once; cancel_query stops one.
- An answer shows ${MAX_ROWS} rows. When there are more it carries next_cursor:
  page with query_result(job_id, cursor), or write every row to a file in a
  granted folder with query_result(job_id, save_as: "name.csv") (also .tsv,
  .json, .jsonl; .nt for CONSTRUCT). Do not fold rows into one GROUP_CONCAT
  string to get around the page size.
- start_query runs a query you already expect to be slow without waiting.
- Per-entity details for thousands of entities on a large published graph
  (title, venue and year of every work citing a paper; every article of one
  journal with its authors): pass partition_by, the variable each row is
  about — partition_by: "?work". The values of that variable are listed first
  (one cheap lookup), then the query runs a few hundred of them at a time, two
  chunks at once, and every finished chunk is kept on disk. query_status shows
  the keys and chunks done. If it stops — cancelled, a crash, Claude restarted
  — send the SAME call again: it carries on from the last finished chunk. A
  call identical to one still running joins it rather than starting over.
  Choosing the variable:
    * the thing the rows are about, bound in EVERY row: matched in the main
      pattern, not only inside OPTIONAL or one side of a UNION;
    * no ORDER BY (sort the saved file afterwards), and an aggregate only when
      grouped by that variable (GROUP BY ?work);
    * keep the query's most specific constant pattern on it (?work cx:cites
      <paper>, not only ?work a cx:Work): the values are listed from the
      cheapest such pattern, and none at all means listing them costs as much
      as the query (the plan says keys_from_where).
  A query the split would change is refused, saying why. Hand the result over
  with query_result(job_id, save_as: …).

Every result carries \`stats\`: the bytes and requests actually read. Report
them when the user asks how much data was touched — a good query over a huge
graph reads a tiny fraction of it.`;

// --- helpers ---------------------------------------------------------------

const json = (value) => ({ content: [{ type: "text", text: JSON.stringify(value) }] });

const fail = (error) => ({
  content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }],
  isError: true,
});

/**
 * Wrap a tool body so every failure reaches the model as readable text. The
 * request context (`extra.signal` aborts when the client cancels) passes
 * through.
 */
const guard = (fn) => async (args, extra) => {
  try {
    return await fn(args, extra);
  } catch (error) {
    if (error instanceof UsageError) return fail(error);
    const message = error?.message ?? String(error);
    // First line only — but keep the engine's `hint:` line, which is where it
    // says what to change (e.g. that RDF 1.2 Turtle needs
    // quoted_triple_syntax "rdf12").
    const [first, ...rest] = message.split("\n");
    const hint = rest.find((line) => line.trimStart().startsWith("hint:"));
    return fail(new Error(hint ? `${first}\n${hint.trim()}` : first));
  }
};

/** Terms → plain JSON values, keeping IRIs distinguishable from literals. */
const rowToJson = (row) =>
  Object.fromEntries(
    Object.entries(row).map(([k, term]) => [k, term.kind === "iri" ? term.value : term.toJS()]),
  );

// The engine's counters are cumulative per open graph, so they are diffed to
// report what THIS call actually read — the number worth quoting. (Cumulative
// totals are kept alongside; on a small file several calls can add up to more
// than its size, since a re-read of an evicted block is a real fetch.)
const lastStats = new WeakMap();

/** Physical reads for this call, and the fraction of the file they are. */
// --- query diagnostics -------------------------------------------------------
// A FILTER that hits a SPARQL type error (CONTAINS on an IRI, STRSTARTS on a
// number, a language-tag mismatch, …) drops the row by the spec — even under
// `!`, since the error propagates through it (#284) — so the query returns
// fewer rows, or none, and no error. An agent reads "0 rows" as "CONTAINS is
// unsupported". The engine counts those errors; say them in plain words.

const ARG_KIND = {
  iri: "an IRI",
  "blank-node": "a blank node",
  "quoted-triple": "a quoted triple",
  numeric: "a number",
  "typed-literal": "a non-string typed literal",
  "language-mismatch": "a literal whose language tag differs from argument 1",
  unbound: "an unbound variable",
  "invalid-regex": "an invalid regex pattern",
  "not-a-datetime": "a value that is not an xsd:dateTime",
  string: "a string",
  "language-tagged": "a language-tagged literal",
  "invalid-number": "a literal whose text is not a valid number for its datatype",
};

/** An operator (`+`, `<`, `IN`, …) has operands, not numbered arguments. */
const isOperator = (f) => !/^[A-Z_0-9]+$/.test(f);
const position = (w) =>
  isOperator(w.function)
    ? (w.argument === 1 ? "the left operand" : "the right operand")
    : `argument ${w.argument}`;

/** One line an agent will read: the result size, then each warning. */
function warningLine(size, warnings) {
  const errors = warnings.filter((w) => w.severity === "type-error");
  const hints = warnings.filter((w) => w.severity !== "type-error");
  const parts = [size];
  if (errors.length) {
    const what = errors
      .map((w) => {
        const kind = ARG_KIND[w.argKind] ?? w.argKind;
        const times = w.count === 1 ? "1 row" : `${w.count} rows`;
        if (w.argKind === "division-by-zero") return `${w.function} divided by zero (${times}) — ${w.hint}`;
        // About the pair of operands, not one argument: `"2001" > 2000`.
        if (w.argKind === "incomparable") {
          const eg = w.sample ? `, e.g. ${w.sample}` : "";
          return `${w.function} compared two values SPARQL cannot compare (${times}${eg}) — ${w.hint}`;
        }
        return `${w.function} received ${kind} as ${position(w)} (${times}) — ${w.hint}`;
      })
      .join("; ");
    parts.push(
      `${errors.length} FILTER type error${errors.length === 1 ? "" : "s"}: ${what}. ` +
        "(A SPARQL type error makes a FILTER drop the row, even under !, without raising an error.)",
    );
  }
  for (const h of hints) parts.push(`Hint: ${h.message.replace(/^No results\. /, "")}`);
  return parts.join(" ");
}

/** The tool result: JSON first (as before), then the plain line when there is one. */
function withWarnings(value, size, warnings) {
  if (!warnings.length) return json(value);
  const out = json({ ...value, warnings });
  out.content.push({ type: "text", text: warningLine(size, warnings) });
  return out;
}

const statsOf = (graph, source) => {
  const s = graph.stats();
  if (!s) return { where: source.where }; // a chat graph: in memory, nothing read
  const prev = lastStats.get(graph) ?? { bytes: 0, requests: 0 };
  lastStats.set(graph, { bytes: s.bytes, requests: s.requests });
  const bytes = Math.max(0, s.bytes - prev.bytes);
  return {
    where: source.where,
    fileBytes: s.fileLength,
    bytesRead: bytes,
    requests: Math.max(0, s.requests - prev.requests),
    fractionRead: s.fileLength ? +(bytes / s.fileLength).toFixed(4) : null,
    sessionBytesRead: s.bytes,
  };
};

const truncate = (text, n) =>
  typeof text === "string" && text.length > n ? `${text.slice(0, n).trimEnd()}…` : text;

// --- query jobs ----------------------------------------------------------------
// sparql_query and start_query run on worker threads (jobs.mjs); a job's rows
// are kept as the engine's term tokens and turned into JSON a page at a time.

/** An engine term token → the JSON value an answer carries (see rowToJson). */
const tokenValue = (token) => {
  const term = Term.parse(token);
  if (term.kind === "iri") return term.value;
  const value = term.toJS();
  return typeof value === "bigint" ? value.toString() : value; // JSON has no BigInt
};

const tokenRow = (row) => Object.fromEntries(Object.entries(row).map(([k, t]) => [k, tokenValue(t)]));

const seconds = (s, fallback) => Math.round(1000 * Math.min(WAIT_MAX_S, Math.max(0, s ?? fallback)));

/** The rows (SELECT) or triples (CONSTRUCT/DESCRIBE) of a finished job. */
const itemsOf = (job) => (job.env.kind === "construct" ? job.env.triples : job.env.rows);

function jobStats(job) {
  const s = job.stats;
  if (!s) return { where: job.where };
  return {
    where: job.where,
    fileBytes: s.fileBytes,
    bytesRead: s.bytesRead,
    requests: s.requests,
    fractionRead: s.fileBytes ? +(s.bytesRead / s.fileBytes).toFixed(4) : null,
    sessionBytesRead: s.sessionBytesRead,
  };
}

/** Where a job stands, for any state. */
function jobStatus(job) {
  const progress = jobs.progress(job);
  return {
    job_id: job.id,
    state: job.state,
    dataset: job.dataset,
    elapsed_ms: (job.endedAt ?? Date.now()) - job.createdAt,
    progress: {
      reads: progress.reads,
      bytes_read: progress.bytes,
      rows: job.env && job.env.kind !== "ask" ? itemsOf(job).length : null,
    },
    ...(job.partition ? { partition: { ...job.partition, chunks_running: job.run.running } } : {}),
    ...(job.error ? { error: job.error } : {}),
  };
}

/** A job that has not finished yet: what it is doing, and what to call next. */
function stillRunning(job) {
  const waiting = job.state === "queued";
  const p = job.partition;
  return json({
    ...jobStatus(job),
    note: p
      ? `Running in chunks by ${p.by}: ` +
        (p.keys === null
          ? `${p.phase}. `
          : `${p.keys_done} of ${p.keys} keys done (${p.chunks_done} chunks, ${p.rows} rows so far` +
            (p.resumed_keys ? `; ${p.resumed_keys} keys resumed from an earlier run` : "") +
            "). ") +
        `Wait for it with query_status({job_id: "${job.id}"}), up to ${WAIT_MAX_S} s per call. Every ` +
        "finished chunk is kept on disk: if this stops — cancelled, a crash, a restart — sending the same " +
        "call again carries on from there."
      : (waiting
          ? "Waiting for a free worker: three queries are already running. "
          : "Still running in the background; this call returned before the client's time limit. ") +
        `Wait for it with query_status({job_id: "${job.id}"}), up to ${WAIT_MAX_S} s per call, and call ` +
        "again until state is done. Do NOT send the query again: a second copy would compete with " +
        `this one. cancel_query({job_id: "${job.id}"}) stops it.`,
  });
}

/**
 * A finished job's answer: one page of it, in the shape sparql_query has
 * always returned, plus the cursor to the next page.
 */
function jobAnswer(job, offset = 0, limit = MAX_ROWS) {
  if (job.state === "failed") throw new Error(job.error);
  if (job.state === "cancelled") throw new UsageError(`query ${job.id} was cancelled`);
  const env = job.env;
  const head = {
    dataset: job.dataset,
    job_id: job.id,
    ...(job.partition
      ? {
          partition: {
            by: job.partition.by,
            keys: job.partition.keys,
            chunks: job.partition.chunks_done,
            resumed_keys: job.partition.resumed_keys,
          },
        }
      : {}),
  };
  const tail = { ms: job.ms, stats: jobStats(job) };
  if (env.kind === "ask") {
    return withWarnings({ ...head, form: "ASK", boolean: env.boolean, ...tail }, `ASK = ${env.boolean}.`, job.warnings);
  }
  const construct = env.kind === "construct";
  const all = itemsOf(job);
  const page = all.slice(offset, offset + limit);
  const end = offset + page.length;
  const next = end < all.length ? String(end) : null;
  const noun = construct ? "triple" : "row";
  return withWarnings(
    {
      ...head,
      form: construct ? "CONSTRUCT/DESCRIBE" : "SELECT",
      count: all.length,
      ...(offset ? { offset } : {}),
      truncated: all.length - page.length,
      ...(construct
        ? { triples: page.map((t) => t.map((token) => Term.parse(token).value)) }
        : { vars: env.vars, rows: page.map(tokenRow) }),
      next_cursor: next,
      ...(next
        ? {
            note:
              `${noun}s ${offset + 1}–${end} of ${all.length}. Next page: query_result({job_id: ` +
              `"${job.id}", cursor: "${next}"}). Every ${noun} to a file: query_result({job_id: ` +
              `"${job.id}", save_as: "<name>.${construct ? "nt" : "csv"}"}).`,
          }
        : {}),
      ...(construct ? {} : { reason: job.reason }),
      ...tail,
    },
    `${all.length} ${noun}${all.length === 1 ? "" : "s"}.`,
    job.warnings,
  );
}

/**
 * Queue a query on a worker — or, with `partitionBy`, start (or join, or
 * resume) running it in chunks. The graph is resolved (and checked) here first.
 */
async function submitQuery(name, query, reason, partitionBy) {
  const source = await store.resolve(name);
  const spec = {
    dataset: source.key,
    key: `${source.cacheKey}#${await store.stamp(source)}`,
    target: source.openTarget,
    query,
    reason,
  };
  if (partitionBy !== undefined) {
    if (!partitionBy.trim()) throw new UsageError("partition_by needs a variable name, e.g. \"?work\"");
    return runs.start({ ...spec, var: partitionBy.trim(), where: source.where });
  }
  const job = jobs.submit(spec);
  job.where = source.where;
  return job;
}

function jobById(id) {
  const job = jobs.get(id);
  if (!job) {
    throw new UsageError(
      `no query ${id} — job ids last as long as the extension runs (the newest 50 finished ` +
        "ones are kept), so a restart forgets them. Call query_status() for the jobs it holds, or " +
        "send the query again.",
    );
  }
  return job;
}

// Formats query_result can write, by file extension.
const SAVE_FORMATS = { ".csv": "csv", ".tsv": "tsv", ".json": "json", ".jsonl": "jsonl", ".ndjson": "jsonl", ".nt": "nt" };

const csvCell = (s) => (/[",\r\n]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s);
const tsvCell = (s) => s.replace(/[\t\r\n]+/g, " ");

/**
 * Write every row (or triple) of a finished job into a granted folder. A name
 * already taken gets a numbered sibling rather than being overwritten, and the
 * file appears whole or not at all (written aside, then renamed).
 */
async function saveJob(job, outputPath) {
  if (dirs.length === 0) {
    throw new UsageError(
      "no folder granted — add one in the extension's settings (Graph folders) to save results, " +
        "or page through them with query_result's cursor",
    );
  }
  if (job.env.kind === "ask") throw new UsageError("an ASK answer is a single boolean — there is nothing to save");
  let ext = extname(outputPath).toLowerCase();
  if (!ext) {
    ext = job.env.kind === "construct" ? ".nt" : ".csv";
    outputPath += ext;
  }
  const format = SAVE_FORMATS[ext];
  if (!format) {
    throw new UsageError(`save_as needs one of ${Object.keys(SAVE_FORMATS).join(" ")} — got "${ext}"`);
  }
  const construct = job.env.kind === "construct";
  if (format === "nt" && !construct) throw new UsageError(".nt holds triples: save a SELECT as .csv, .tsv, .json or .jsonl");

  const target = await freeName(grantedPath(outputPath));
  await mkdir(dirname(target), { recursive: true });
  const part = `${target}.part`;
  const out = createWriteStream(part, { encoding: "utf8" });
  const write = async (text) => {
    if (!out.write(text)) await once(out, "drain");
  };
  const items = itemsOf(job);
  const vars = construct ? ["s", "p", "o"] : job.env.vars;
  const lexical = (token) => (token === undefined ? "" : Term.parse(token).value);
  const cells = (item) => (construct ? item.slice(0, 3) : vars.map((v) => item[v])).map(lexical);
  try {
    if (format === "csv") await write(`${vars.map(csvCell).join(",")}\r\n`);
    if (format === "tsv") await write(`${vars.map(tsvCell).join("\t")}\n`);
    if (format === "json") await write("[");
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (format === "csv") await write(`${cells(item).map(csvCell).join(",")}\r\n`);
      else if (format === "tsv") await write(`${cells(item).map(tsvCell).join("\t")}\n`);
      else if (format === "nt") await write(`${item.slice(0, 3).join(" ")} .\n`);
      else {
        const value = construct ? Object.fromEntries(vars.map((v, k) => [v, tokenValue(item[k])])) : tokenRow(item);
        await write(format === "json" ? `${i ? "," : ""}\n${JSON.stringify(value)}` : `${JSON.stringify(value)}\n`);
      }
    }
    if (format === "json") await write("\n]\n");
    out.end();
    await once(out, "finish");
  } catch (error) {
    out.destroy();
    throw error;
  }
  await rename(part, target);
  return { path: target, format, [construct ? "triples" : "rows"]: items.length, bytes: out.bytesWritten };
}

/** `path`, or `path` with -2, -3, … before its extension if that name is taken. */
async function freeName(path) {
  const ext = extname(path);
  const stem = path.slice(0, path.length - ext.length);
  for (let n = 1; ; n++) {
    const candidate = n === 1 ? path : `${stem}-${n}${ext}`;
    const taken = await access(candidate).then(() => true, () => false);
    if (!taken) return candidate;
  }
}

// --- wiring ----------------------------------------------------------------

/**
 * Directories the user granted. Claude Desktop expands a `multiple: true`
 * directory config into one argv entry per folder; RETE_GRAPH_DIRS (delimiter
 * separated) is the equivalent for running this server by hand.
 */
function allowedDirs() {
  const fromArgs = process.argv.slice(2).filter(Boolean);
  const fromEnv = (process.env.RETE_GRAPH_DIRS ?? "").split(delimiter).filter(Boolean);
  return [...new Set([...fromArgs, ...fromEnv])].map((d) => resolve(d));
}

async function loadCatalog() {
  try {
    return JSON.parse(await readFile(resolve(HERE, "catalog.json"), "utf8"));
  } catch {
    return { datasets: [] }; // a bundle without the snapshot still serves local files
  }
}

const dirs = allowedDirs();
const store = new GraphStore({ allowedDirs: dirs, catalog: await loadCatalog() });
const jobs = new JobManager(new URL("./worker.mjs", import.meta.url));
// Checkpoints of partitioned queries: in the system's temp folder, so they
// survive a restart of Claude Desktop without writing into the user's folders.
const runs = new PartitionRuns(jobs, process.env.RETE_MCPB_RUNS || join(tmpdir(), "rete-mcpb", "partitions"));

const server = new McpServer(
  { name: "rete-graphs", version: VERSION },
  { instructions: INSTRUCTIONS },
);

const READ_ONLY = { readOnlyHint: true, destructiveHint: false, idempotentHint: true };
const partitionBy = z
  .string()
  .optional()
  .describe(
    'split the query into resumable chunks by this variable, e.g. "?work": its values are listed ' +
      "first, then the query runs a few hundred of them at a time and every finished chunk is kept " +
      "on disk — for per-entity details over thousands of entities. Re-sending the same call " +
      "resumes after a stop. The variable must be bound in every row (not only inside OPTIONAL); " +
      "no ORDER BY; aggregates only grouped by it",
  );
const dataset = z
  .string()
  .describe(
    "a local .rete file name or path, a published catalogue key, an https:// .rete URL, or " +
      "chat:<name> for RDF loaded from the chat with load_rdf",
  );

// --- tools -----------------------------------------------------------------

server.registerTool(
  "list_datasets",
  {
    title: "List knowledge graphs",
    description:
      "Every .rete graph this extension can reach: the user's LOCAL files (in the granted " +
      "folders) and the PUBLISHED catalogue, with size and triple count. Descriptions are " +
      `abridged unless a free-text query narrows the list to ${FULL_DESC} or fewer graphs — read ` +
      "a large graph's full description before querying it, since its caveats come last. " +
      "dataset_card gives a graph's whole self-description.",
    inputSchema: {
      query: z.string().optional().describe("free-text filter over key, label and description"),
      source: z
        .enum(["all", "local", "chat", "published"])
        .optional()
        .describe("default: all; chat = RDF loaded from the conversation with load_rdf"),
    },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ query, source = "all" }) => {
    const needle = query?.toLowerCase();
    const hit = (...fields) => !needle || fields.some((f) => String(f ?? "").toLowerCase().includes(needle));
    const want = (kind) => source === "all" || source === kind;

    const local = !want("local")
      ? []
      : (await store.localFiles())
          .filter((f) => hit(f.key, f.path))
          .map((f) => ({ key: f.key, path: f.path, bytes: f.size, modified: f.modified }));

    const chat = !want("chat")
      ? []
      : store
          .chatGraphs()
          .filter((c) => hit(c.source.key))
          .map((c) => ({
            key: c.source.key,
            format: c.format,
            parts: c.parts,
            rdf_bytes: c.size,
            bytes: c.source.bytes.length,
            loaded: c.loadedAt,
          }));

    const matches = !want("published") ? [] : store.catalog.filter((d) => hit(d.key, d.label, d.description));
    // A filter that narrows the catalogue to a few graphs gets their whole
    // descriptions: the caveats that matter most for a large graph ("keep
    // queries selective", what it does NOT contain) sit past the abridged cut.
    const whole = Boolean(needle) && matches.length <= FULL_DESC;
    const published = matches.map((d) => ({
      key: d.key,
      label: d.label,
      description: whole ? d.description : truncate(d.description, MAX_DESC),
      triples: d.triples,
      size: d.size,
      license: d.license,
      url: d.url,
    }));

    return json({
      local,
      ...(chat.length ? { chat } : {}),
      published,
      note:
        dirs.length === 0
          ? "No local folders granted yet — add them in the extension's settings to query and " +
            "build .rete files on this machine."
          : `Local folders: ${dirs.join(", ")}`,
      counts: { local: local.length, chat: chat.length, published: published.length },
    });
  }),
);

server.registerTool(
  "dataset_card",
  {
    title: "Dataset card",
    description:
      "The Dataset Card embedded in the .rete file itself: title, description, licence, " +
      "provenance and — when the publisher curated them — identity fields (version, creators " +
      "as ORCID IRIs, publisher as a ROR IRI, DOI, citation), plus counts and often example " +
      "queries. Index-free — reads only the header and the card's byte range, so it is just " +
      "as cheap on a multi-gigabyte graph.",
    inputSchema: { dataset },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ dataset: name }) => {
    const { graph, source } = await store.graph(name);
    const card = graph.card();
    return json({
      dataset: source.key,
      where: source.path ?? source.url ?? source.where,
      card: card ?? null,
      note: card ? undefined : "this file carries no embedded card — try dataset_schema",
      stats: statsOf(graph, source),
    });
  }),
);

server.registerTool(
  "dataset_schema",
  {
    title: "Dataset schema",
    description:
      "The graph's classes with instance counts, and its subject-class/predicate/object-class " +
      "relations. Read from the file's baked schema pyramid, not by scanning. USE THE RETURNED " +
      "IRIs EXACTLY when writing SPARQL — inventing a namespace is the classic silent-0-rows bug.",
    inputSchema: {
      dataset,
      limit: z.number().int().min(1).max(500).optional().describe("classes/relations to return (default 60)"),
    },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ dataset: name, limit = 60 }) => {
    const { graph, source } = await store.graph(name);
    let schema = { classes: [], relations: [] };
    let note;
    try {
      schema = graph.schema();
    } catch (error) {
      note = `no schema pyramid in this file (${error.message}); query rdf:type directly instead`;
    }
    return json({
      dataset: source.key,
      info: graph.info(),
      graphs: graph.graphNames(),
      classes: schema.classes.slice(0, limit),
      relations: schema.relations.slice(0, limit),
      truncated: {
        classes: Math.max(0, schema.classes.length - limit),
        relations: Math.max(0, schema.relations.length - limit),
      },
      note,
      stats: statsOf(graph, source),
    });
  }),
);

server.registerTool(
  "example_queries",
  {
    title: "Example queries",
    description:
      "Runnable SPARQL the dataset ships with, from its embedded card (falling back to the " +
      "catalogue). The fastest route to a correct query: adapt one of these rather than " +
      "writing a query from scratch.",
    inputSchema: { dataset },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ dataset: name }) => {
    const { graph, source } = await store.graph(name);
    const embedded = graph.examples();
    const queries = embedded.length ? embedded : (source.entry?.examples ?? []);
    return json({
      dataset: source.key,
      queries,
      note: queries.length ? undefined : "this dataset ships no example queries — start from dataset_schema",
      stats: statsOf(graph, source),
    });
  }),
);

server.registerTool(
  "sparql_query",
  {
    title: "Run SPARQL",
    description:
      "Run SPARQL 1.1 (SELECT / ASK / CONSTRUCT / DESCRIBE) against a graph and wait for the " +
      `answer — up to ${WAIT_DEFAULT_S} s. A query still running by then is NOT lost: the answer is ` +
      "its job_id, it keeps running, and query_status waits for it (never send it again). Answers " +
      `show ${MAX_ROWS} rows; next_cursor leads to the rest (query_result pages them or saves ` +
      "them all to a file). Keep a LIMIT while exploring, and COUNT before a large extraction. " +
      "reason=true enables OWL 2 QL entailment (subclass/subproperty/domain/range) where the " +
      "graph carries an ontology. CONTAINS / STRSTARTS / REGEX need string arguments: use " +
      "STR(?iri) or STR(?number). FILTER type errors (silently false in SPARQL) come back as " +
      "`warnings`. partition_by runs a large extraction in resumable chunks.",
    inputSchema: {
      dataset,
      query: z.string().describe("the SPARQL query"),
      reason: z.boolean().optional().describe("OWL 2 QL entailment by query rewriting (default false)"),
      wait_seconds: z
        .number()
        .min(0)
        .max(WAIT_MAX_S)
        .optional()
        .describe(`how long to wait before answering with the job id instead (default ${WAIT_DEFAULT_S})`),
      partition_by: partitionBy,
    },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ dataset: name, query, reason = false, wait_seconds, partition_by }, extra) => {
    const called = Date.now();
    const job = await submitQuery(name, query, reason, partition_by);
    await jobs.wait(job, seconds(wait_seconds, WAIT_DEFAULT_S), extra?.signal);
    // The client cancelled THIS call (the user stopped it): stop the query
    // too, so nothing keeps reading for an answer nobody will see — unless
    // the call only joined a split query that was already running. A wait
    // that merely ran out of time leaves it running, to be collected by job id.
    if (extra?.signal?.aborted && job.createdAt >= called) jobs.cancel(job);
    return jobs.isTerminal(job) ? jobAnswer(job) : stillRunning(job);
  }),
);

server.registerTool(
  "start_query",
  {
    title: "Start a SPARQL query in the background",
    description:
      "Start a SPARQL query without waiting for it and get its job_id at once. Use it for a " +
      "query you expect to be slow (details for thousands of entities on a large published " +
      "graph) or to run up to three at the same time. Then query_status waits for it and " +
      "query_result pages through or saves its rows. Same query language and options as " +
      "sparql_query; with partition_by it runs in resumable chunks.",
    inputSchema: {
      dataset,
      query: z.string().describe("the SPARQL query"),
      reason: z.boolean().optional().describe("OWL 2 QL entailment by query rewriting (default false)"),
      partition_by: partitionBy,
    },
    annotations: { ...READ_ONLY, idempotentHint: false, openWorldHint: true },
  },
  guard(async ({ dataset: name, query, reason = false, partition_by }) => {
    const called = Date.now();
    const job = await submitQuery(name, query, reason, partition_by);
    const joined = job.createdAt < called;
    return json({
      ...jobStatus(job),
      note:
        (joined ? `The same split query was already ${job.state === "done" ? "done" : "running"} as ${job.id}; ` +
          "this call joined it rather than starting it twice. " : "Running. ") +
        `query_status({job_id: "${job.id}"}) waits for it, up to ${WAIT_MAX_S} s per call.`,
    });
  }),
);

server.registerTool(
  "query_status",
  {
    title: "Wait for a query",
    description:
      `Wait up to ${WAIT_MAX_S} s for a query job to finish, then say where it stands: state ` +
      "(queued / running / done / failed / cancelled) and progress (reads, bytes read, elapsed). " +
      "Once done, the answer carries the first rows, exactly as sparql_query would. While it is " +
      "still running, just call this again — the query never restarts. Without job_id, lists " +
      "the jobs this extension holds.",
    inputSchema: {
      job_id: z.string().optional().describe("the job to wait for; omit to list all jobs"),
      wait_seconds: z
        .number()
        .min(0)
        .max(WAIT_MAX_S)
        .optional()
        .describe(`longest wait before answering (default ${WAIT_DEFAULT_S})`),
    },
    annotations: READ_ONLY,
  },
  guard(async ({ job_id, wait_seconds }, extra) => {
    if (!job_id) {
      return json({
        jobs: jobs.list().map((job) => ({ ...jobStatus(job), query: truncate(job.query, 160) })),
      });
    }
    const job = jobById(job_id);
    // A cancelled call only ends this wait; the query carries on.
    await jobs.wait(job, seconds(wait_seconds, WAIT_DEFAULT_S), extra?.signal);
    return jobs.isTerminal(job) ? jobAnswer(job) : stillRunning(job);
  }),
);

server.registerTool(
  "query_result",
  {
    title: "Page through or save a query's results",
    description:
      `Read a finished query's rows a page at a time (cursor from next_cursor; up to ${PAGE_MAX} ` +
      "per page), or write ALL of them to a file in a granted folder with save_as — .csv " +
      "(default), .tsv, .json, .jsonl, or .nt for CONSTRUCT triples. Saving puts no rows in the " +
      "conversation, so it is the way to hand over thousands of rows. Nothing is re-run.",
    inputSchema: {
      job_id: z.string().describe("the job, from sparql_query / start_query"),
      cursor: z.string().optional().describe("next_cursor from the previous page (default: the first row)"),
      limit: z.number().int().min(1).max(PAGE_MAX).optional().describe(`rows per page (default ${MAX_ROWS})`),
      save_as: z
        .string()
        .optional()
        .describe("write every row here instead: a file name (lands in the first granted folder) or a path"),
    },
    annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  },
  guard(async ({ job_id, cursor, limit = MAX_ROWS, save_as }) => {
    const job = jobById(job_id);
    if (!jobs.isTerminal(job)) return stillRunning(job);
    if (save_as !== undefined) {
      if (job.state !== "done") return jobAnswer(job); // says why there is nothing to save
      return json({ job_id: job.id, saved: await saveJob(job, save_as) });
    }
    const offset = cursor === undefined ? 0 : Number(cursor);
    if (!Number.isInteger(offset) || offset < 0) {
      throw new UsageError(`not a cursor: "${cursor}" — pass next_cursor from the previous answer`);
    }
    return jobAnswer(job, offset, limit);
  }),
);

server.registerTool(
  "cancel_query",
  {
    title: "Cancel a query",
    description:
      "Stop a queued or running query job. A running one is stopped mid-read by shutting its " +
      "worker down, so what it had read so far is not kept: a narrower or batched version sent " +
      "afterwards starts cold. A query split with partition_by keeps its finished chunks on disk: " +
      "sending the same call again resumes from them.",
    inputSchema: { job_id: z.string().describe("the job to stop") },
    annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  },
  guard(async ({ job_id }) => json(jobStatus(jobs.cancel(jobById(job_id))))),
);

server.registerTool(
  "find_entities",
  {
    title: "Find entities",
    description:
      "Resolve a name to entity IRIs — label prefix search over the graph's label index, plus " +
      "full-text search where the file carries a text index. Do this FIRST when a question " +
      "names a specific thing, then use the returned IRI in a query.",
    inputSchema: {
      dataset,
      text: z.string().describe("the name or words to look for"),
      limit: z.number().int().min(1).max(100).optional().describe("default 20"),
    },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ dataset: name, text, limit = 20 }) => {
    const { graph, source } = await store.graph(name);
    const hits = [];
    const seen = new Set();
    for (const { label, subject } of graph.prefixSearch(text, limit)) {
      if (seen.has(subject)) continue;
      seen.add(subject);
      hits.push({ subject, label, via: "label-prefix" });
    }
    try {
      for (const subject of graph.textSearch(text, { limit })) {
        if (seen.has(subject)) continue;
        seen.add(subject);
        hits.push({ subject, via: "text-index" });
      }
    } catch {
      // no TEXT_INDEX in this file: prefix hits are the whole answer
    }
    return json({
      dataset: source.key,
      hits: hits.slice(0, limit),
      note: hits.length ? undefined : "nothing matched — try fewer words, or a different spelling",
      stats: statsOf(graph, source),
    });
  }),
);

server.registerTool(
  "describe_entity",
  {
    title: "Describe entity",
    description:
      "Everything the graph says about one IRI: its outgoing statements and the statements " +
      "pointing at it. Use after find_entities to see how an entity is modelled.",
    inputSchema: {
      dataset,
      iri: z.string().describe("the entity IRI, without angle brackets"),
      limit: z.number().int().min(1).max(500).optional().describe("statements per direction (default 100)"),
    },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ dataset: name, iri, limit = 100 }) => {
    const { graph, source } = await store.graph(name);
    const bare = iri.replace(/^<|>$/g, "");
    // The IRI is interpolated into a query, so anything that could close the
    // term and start new syntax is rejected rather than escaped.
    if (/[\s<>"{}|\\^`]/.test(bare)) {
      throw new UsageError(`not a usable IRI: ${iri}`);
    }
    const node = `<${bare}>`;
    const out = graph.query(`SELECT ?p ?o WHERE { ${node} ?p ?o } LIMIT ${limit}`).map(rowToJson);
    const inc = graph.query(`SELECT ?s ?p WHERE { ?s ?p ${node} } LIMIT ${limit}`).map(rowToJson);
    return json({
      dataset: source.key,
      iri,
      outgoing: out,
      incoming: inc,
      note: out.length || inc.length ? undefined : "no statements — check the IRI with find_entities",
      stats: statsOf(graph, source),
    });
  }),
);

server.registerTool(
  "validate_shacl",
  {
    title: "Validate with SHACL",
    description:
      "Validate SHACL Core shapes (Turtle) against a graph and return the conformance report. " +
      "Lazy: only the shapes' target nodes are read, so a shape over a huge graph stays cheap.",
    inputSchema: {
      dataset,
      shapes: z.string().describe("SHACL Core shapes as Turtle"),
      format: z.enum(["json", "text"]).optional().describe("report format (default json)"),
    },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ dataset: name, shapes, format = "json" }) => {
    const { graph, source } = await store.graph(name);
    const report = graph.shacl(shapes, { format });
    return json({ dataset: source.key, report, stats: statsOf(graph, source) });
  }),
);

server.registerTool(
  "build_rete",
  {
    title: "Build a .rete graph",
    description:
      "Turn RDF text (Turtle, TriG, N-Triples or N-Quads) into a new .rete knowledge-graph " +
      "file in one of the granted folders — entirely on this machine, no network. The result " +
      "is queryable immediately: pass the returned file name as `dataset` to any other tool.",
    inputSchema: {
      rdf: z.string().describe("the RDF source text"),
      output_path: z
        .string()
        .describe("where to write it; a bare name lands in the first granted folder"),
      format: z
        .enum(["ttl", "trig", "nt", "nq"])
        .optional()
        .describe("RDF syntax of `rdf` (default ttl)"),
      quoted_triple_syntax: z
        .enum(["rdf-star", "rdf12"])
        .optional()
        .describe(
          "what Turtle/TriG `<< s p o >>` means: rdf-star (default) = a quoted triple; " +
            "rdf12 = RDF 1.2, where `<<( s p o )>>` is a triple term, `<< … >>` a reifier " +
            "and `{| … |}` an annotation. Ignored for nt/nq",
        ),
    },
    annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  },
  guard(async ({ rdf, output_path, format = "ttl", quoted_triple_syntax = "rdf-star" }) => {
    if (dirs.length === 0) {
      throw new UsageError(
        "no folder granted — add one in the extension's settings (Graph folders) before building",
      );
    }
    const target = grantedPath(output_path.toLowerCase().endsWith(".rete") ? output_path : `${output_path}.rete`);
    const bytes = await build(rdf, format, { quotedTripleSyntax: quoted_triple_syntax });
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, bytes);
    store.changed(target);
    const { graph } = await store.graph(target);
    return json({
      path: target,
      bytes: bytes.length,
      info: graph.info(),
      dataset: target.split(/[\\/]/).pop().replace(/\.rete$/i, ""),
      note: "queryable now — pass `dataset` to sparql_query. In-wasm builds are uncompressed; " +
        "for very large datasets use the `rete build` CLI.",
    });
  }),
);

server.registerTool(
  "load_rdf",
  {
    title: "Query RDF shared in the chat",
    description:
      "Make RDF the user shared in the chat queryable: an attached .ttl / .nt / .nq / .trig file, " +
      "or RDF pasted into a message. Claude Desktop does not hand attachments to extensions, so " +
      "pass the file's full text, unchanged, as `rdf`. It is built into a graph held in memory — " +
      "no granted folder needed, nothing written to disk — and every tool takes it as dataset " +
      '"chat:<name>" until the extension restarts. Too long for one call: send it in parts, the ' +
      "rest with append=true, each part ending where a statement ends (prefixes from the first " +
      "part carry over). Without append, a second call replaces the graph. CSV, JSON, RDF/XML or " +
      "JSON-LD: convert to Turtle first. A file the user saved in a granted folder is read by " +
      "`path` instead; save_as keeps the graph as a .rete file there.",
    inputSchema: {
      name: z.string().describe("the shared file's name, e.g. people.ttl — the dataset becomes chat:people"),
      rdf: z.string().optional().describe("the RDF text, exactly as shared (or one part of it)"),
      path: z.string().optional().describe("instead of rdf: an RDF text file in a granted folder"),
      format: z
        .enum(["ttl", "trig", "nt", "nq"])
        .optional()
        .describe("RDF syntax (default: from the file's extension, else ttl)"),
      append: z
        .boolean()
        .optional()
        .describe("add this part to chat:<name> rather than replace it (default false)"),
      quoted_triple_syntax: z
        .enum(["rdf-star", "rdf12"])
        .optional()
        .describe("as for build_rete: rdf12 reads RDF 1.2 Turtle/TriG (default rdf-star)"),
      save_as: z
        .string()
        .optional()
        .describe("also write the whole graph as a .rete file in a granted folder (never overwrites)"),
    },
    annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  },
  guard(async ({ name, rdf, path, format, append = false, quoted_triple_syntax = "rdf-star", save_as }) => {
    if ((rdf === undefined) === (path === undefined)) {
      throw new UsageError("pass the RDF as `rdf` (its text) or as `path` (a file in a granted folder) — one of the two");
    }
    const text = rdf ?? (await store.readGrantedText(path));
    const loaded = await store.loadChat(name, text, {
      format: format ?? (path === undefined ? undefined : (formatOf(name) ?? formatOf(path))),
      append,
      quotedTripleSyntax: quoted_triple_syntax,
    });
    const { graph, source } = await store.graph(loaded.source.key);
    const info = graph.info();
    const saved = save_as === undefined ? undefined : await saveChat(loaded, save_as);
    return json({
      dataset: source.key,
      statements: info.quads,
      parts: loaded.parts,
      format: loaded.format,
      rdf_bytes: loaded.size,
      info,
      ...(saved ? { saved } : {}),
      note:
        `Queryable now: pass dataset "${source.key}" to sparql_query, dataset_schema, find_entities ` +
        "and the rest. Held in memory until the extension restarts" +
        (saved
          ? `; also saved as ${saved.path} (dataset "${saved.dataset}").`
          : " — save_as keeps it as a .rete file in a granted folder."),
    });
  }),
);

/** Write a chat graph's file image into a granted folder, beside any file of that name. */
async function saveChat(loaded, outputPath) {
  if (dirs.length === 0) {
    throw new UsageError(
      "no folder granted — add one in the extension's settings (Graph folders) to save a graph. " +
        `${loaded.source.key} stays queryable in memory meanwhile.`,
    );
  }
  const target = await freeName(grantedPath(outputPath.toLowerCase().endsWith(".rete") ? outputPath : `${outputPath}.rete`));
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, new Uint8Array(loaded.source.bytes)); // a copy, off the shared memory
  store.changed(target);
  return { path: target, bytes: loaded.source.bytes.length, dataset: basename(target).replace(/\.rete$/i, "") };
}

/** Is `target` inside `dir`? Keeps writes within the folders the user granted. */
function isInside(dir, target) {
  const rel = relative(resolve(dir), resolve(target));
  return rel !== "" && !rel.startsWith("..") && !isAbsolute(rel);
}

/** Where a file this extension writes goes: a bare name lands in the first granted folder. */
function grantedPath(path) {
  const target = isAbsolute(path) ? path : resolve(dirs[0], path);
  if (!dirs.some((dir) => isInside(dir, target))) {
    throw new UsageError(`${target} is outside the folders this extension may write to (${dirs.join(", ")})`);
  }
  return target;
}

// --- go --------------------------------------------------------------------

const transport = new StdioServerTransport();
await server.connect(transport);
console.error(
  `rete-graphs ${VERSION} ready — ${store.catalog.length} published datasets, ` +
    `${dirs.length} granted folder(s)`,
);
