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
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { delimiter, dirname, isAbsolute, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { build } from "rete-graph";

import { GraphStore, UsageError } from "./graphs.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
// Stamped by build.mjs from the workspace Cargo.toml; the fallback only shows
// up if this file is ever run unbundled.
const VERSION = typeof __RETE_VERSION__ === "string" ? __RETE_VERSION__ : "0.0.0-dev";
const MAX_ROWS = 200; // rows returned to the model before truncation
const MAX_DESC = 320; // chars of a catalogue description in list_datasets

const INSTRUCTIONS = `\
This server queries and builds .rete files — single-file RDF knowledge graphs
that are read LAZILY BY BYTE RANGE: only the bytes a query touches are ever
fetched, whether the file sits on this machine or on public HTTP storage. A
multi-gigabyte graph therefore answers a selective query in megabytes, and
local graphs work with no network at all.

Two kinds of dataset are reachable, and every tool's \`dataset\` argument takes
either — plus a full https:// URL to any published .rete:
  * LOCAL — .rete files in the folders the user granted this extension.
  * PUBLISHED — the public catalogue (data.graphplaza.com), read over HTTP.

Recommended workflow:
1. list_datasets — what is here, local and published.
2. dataset_card — the graph's own self-description: what it is, its licence,
   provenance, identity (creators, publisher, DOI, citation, when curated)
   and counts. Costs two small reads even on a 2 GB file.
3. dataset_schema — the classes and the subject-class/predicate/object-class
   relations, with counts. USE THESE IRIs EXACTLY; never invent a namespace.
4. example_queries — runnable SPARQL the file ships with. The fastest route to
   a correct query shape; adapt one rather than writing from scratch.
5. sparql_query — SELECT / ASK / CONSTRUCT / DESCRIBE. Always keep a LIMIT
   while exploring. reason=true turns on OWL 2 QL entailment where the graph
   carries an ontology.
6. find_entities — resolve a name to IRIs before asking about a specific thing.
7. validate_shacl — data-quality checks with SHACL Core shapes.
8. build_rete — turn RDF text (Turtle / N-Triples) into a new .rete file in a
   granted folder; it is queryable immediately by its file name.

Every result carries \`stats\`: the bytes and requests actually read. Report
them when the user asks how much data was touched — a good query over a huge
graph reads a tiny fraction of it.`;

// --- helpers ---------------------------------------------------------------

const json = (value) => ({ content: [{ type: "text", text: JSON.stringify(value) }] });

const fail = (error) => ({
  content: [{ type: "text", text: error instanceof Error ? error.message : String(error) }],
  isError: true,
});

/** Wrap a tool body so every failure reaches the model as readable text. */
const guard = (fn) => async (args) => {
  try {
    return await fn(args);
  } catch (error) {
    if (error instanceof UsageError) return fail(error);
    const message = error?.message ?? String(error);
    return fail(new Error(message.includes("\n") ? message.split("\n")[0] : message));
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
const statsOf = (graph, source) => {
  const s = graph.stats();
  if (!s) return null;
  const prev = lastStats.get(graph) ?? { bytes: 0, requests: 0 };
  lastStats.set(graph, { bytes: s.bytes, requests: s.requests });
  const bytes = Math.max(0, s.bytes - prev.bytes);
  return {
    where: source.kind === "local" ? "local file (lazy byte-range reads)" : "remote (HTTP Range)",
    fileBytes: s.fileLength,
    bytesRead: bytes,
    requests: Math.max(0, s.requests - prev.requests),
    fractionRead: s.fileLength ? +(bytes / s.fileLength).toFixed(4) : null,
    sessionBytesRead: s.bytes,
  };
};

const truncate = (text, n) =>
  typeof text === "string" && text.length > n ? `${text.slice(0, n).trimEnd()}…` : text;

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

const server = new McpServer(
  { name: "rete-graphs", version: VERSION },
  { instructions: INSTRUCTIONS },
);

const READ_ONLY = { readOnlyHint: true, destructiveHint: false, idempotentHint: true };
const dataset = z
  .string()
  .describe("a local .rete file name or path, a published catalogue key, or an https:// .rete URL");

// --- tools -----------------------------------------------------------------

server.registerTool(
  "list_datasets",
  {
    title: "List knowledge graphs",
    description:
      "Every .rete graph this extension can reach: the user's LOCAL files (in the granted " +
      "folders) and the PUBLISHED catalogue. Descriptions are abridged — dataset_card gives " +
      "a graph's full self-description. Optionally filter with a free-text query.",
    inputSchema: {
      query: z.string().optional().describe("free-text filter over key, label and description"),
      source: z.enum(["all", "local", "published"]).optional().describe("default: all"),
    },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ query, source = "all" }) => {
    const needle = query?.toLowerCase();
    const hit = (...fields) => !needle || fields.some((f) => String(f ?? "").toLowerCase().includes(needle));

    const local =
      source === "published"
        ? []
        : (await store.localFiles())
            .filter((f) => hit(f.key, f.path))
            .map((f) => ({ key: f.key, path: f.path, bytes: f.size, modified: f.modified }));

    const published =
      source === "local"
        ? []
        : store.catalog
            .filter((d) => hit(d.key, d.label, d.description))
            .map((d) => ({
              key: d.key,
              label: d.label,
              description: truncate(d.description, MAX_DESC),
              triples: d.triples,
              size: d.size,
              license: d.license,
              url: d.url,
            }));

    return json({
      local,
      published,
      note:
        dirs.length === 0
          ? "No local folders granted yet — add them in the extension's settings to query and " +
            "build .rete files on this machine."
          : `Local folders: ${dirs.join(", ")}`,
      counts: { local: local.length, published: published.length },
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
      where: source.kind === "local" ? source.path : source.url,
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
      "Run SPARQL 1.1 (SELECT / ASK / CONSTRUCT / DESCRIBE) against a graph. Keep a LIMIT while " +
      "exploring. reason=true enables OWL 2 QL entailment (subclass/subproperty/domain/range) " +
      "where the graph carries an ontology. Only the byte ranges the query touches are read.",
    inputSchema: {
      dataset,
      query: z.string().describe("the SPARQL query"),
      reason: z.boolean().optional().describe("OWL 2 QL entailment by query rewriting (default false)"),
    },
    annotations: { ...READ_ONLY, openWorldHint: true },
  },
  guard(async ({ dataset: name, query, reason = false }) => {
    const { graph, source } = await store.graph(name);
    const started = Date.now();
    const result = graph.query(query, { reason });
    const ms = Date.now() - started;

    if (typeof result === "boolean") {
      return json({ dataset: source.key, form: "ASK", boolean: result, ms, stats: statsOf(graph, source) });
    }
    if (Array.isArray(result) && Array.isArray(result[0])) {
      const triples = result.slice(0, MAX_ROWS).map((t) => t.map((term) => term.value));
      return json({
        dataset: source.key,
        form: "CONSTRUCT/DESCRIBE",
        count: result.length,
        truncated: Math.max(0, result.length - triples.length),
        triples,
        ms,
        stats: statsOf(graph, source),
      });
    }
    const rows = result.slice(0, MAX_ROWS).map(rowToJson);
    return json({
      dataset: source.key,
      form: "SELECT",
      count: result.length,
      truncated: Math.max(0, result.length - rows.length),
      rows,
      ms,
      reason,
      stats: statsOf(graph, source),
    });
  }),
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
      "Turn RDF text (Turtle, N-Triples or N-Quads) into a new .rete knowledge-graph file in " +
      "one of the granted folders — entirely on this machine, no network. The result is " +
      "queryable immediately: pass the returned file name as `dataset` to any other tool.",
    inputSchema: {
      rdf: z.string().describe("the RDF source text"),
      output_path: z
        .string()
        .describe("where to write it; a bare name lands in the first granted folder"),
      format: z.enum(["ttl", "nt", "nq"]).optional().describe("RDF syntax of `rdf` (default ttl)"),
    },
    annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  },
  guard(async ({ rdf, output_path, format = "ttl" }) => {
    if (dirs.length === 0) {
      throw new UsageError(
        "no folder granted — add one in the extension's settings (Graph folders) before building",
      );
    }
    let target = isAbsolute(output_path) ? output_path : resolve(dirs[0], output_path);
    if (!target.toLowerCase().endsWith(".rete")) target += ".rete";
    if (!dirs.some((dir) => isInside(dir, target))) {
      throw new UsageError(
        `${target} is outside the folders this extension may write to (${dirs.join(", ")})`,
      );
    }
    const bytes = await build(rdf, format);
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, bytes);
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

/** Is `target` inside `dir`? Keeps writes within the folders the user granted. */
function isInside(dir, target) {
  const rel = relative(resolve(dir), resolve(target));
  return rel !== "" && !rel.startsWith("..") && !isAbsolute(rel);
}

// --- go --------------------------------------------------------------------

const transport = new StdioServerTransport();
await server.connect(transport);
console.error(
  `rete-graphs ${VERSION} ready — ${store.catalog.length} published datasets, ` +
    `${dirs.length} granted folder(s)`,
);
