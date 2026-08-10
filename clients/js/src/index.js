// rete-graph: query local and remote `.rete` graph files with SPARQL.
// A thin idiomatic wrapper over the repo's wasm engine (crates/rete-wasm) —
// the same engine behind the CLI, the Python client, and the playground.
// Mirrors the Python client's surface: Term parsing, clean IRIs (never
// `<token>` syntax), and the same open()/build() entry points.
import initWasm, {
  Graph as WasmGraph,
  RemoteGraph as WasmRemoteGraph,
  build as wasmBuild,
  heap_bytes as wasmHeapBytes,
} from "../vendor/pkg/rete_wasm.js";

/**
 * Quads pulled per wasm call by the streaming dump (`dump`, `nquads`).
 *
 * A wasm→JS call costs far more than decoding a triple does, so pulling one
 * quad per call would make the boundary the bottleneck; pulling the whole graph
 * would rebuild the very array these methods exist to avoid. 10 000 amortizes
 * the crossing to nothing while bounding the transient buffer at roughly
 * 10 000 × ~120 B ≈ 1.2 MB — flat no matter how many quads follow.
 */
const DUMP_BATCH = 10_000;

const XSD = "http://www.w3.org/2001/XMLSchema#";
const INT_TYPES = new Set(
  [
    "integer", "long", "int", "short", "byte",
    "nonNegativeInteger", "nonPositiveInteger", "negativeInteger",
    "positiveInteger", "unsignedLong", "unsignedInt", "unsignedShort",
    "unsignedByte",
  ].map((t) => XSD + t),
);
const FLOAT_TYPES = new Set(["decimal", "double", "float"].map((t) => XSD + t));

/** One RDF term from a query solution. */
export class Term {
  /** @param {string} kind @param {string} value */
  constructor(kind, value, datatype = null, lang = null) {
    this.kind = kind; // "iri" | "literal" | "bnode" | "triple"
    this.value = value;
    this.datatype = datatype;
    this.lang = lang;
  }

  /** Parse an N-Triples-style term token as emitted by the engine. */
  static parse(token) {
    if (token.startsWith("<<")) return new Term("triple", token);
    if (token.startsWith("<") && token.endsWith(">"))
      return new Term("iri", token.slice(1, -1));
    if (token.startsWith("_:")) return new Term("bnode", token);
    if (token.startsWith('"')) return parseLiteral(token);
    return new Term("literal", token);
  }

  /** The closest JS value: number/boolean/BigInt for common XSD types. */
  toJS() {
    if (this.kind === "literal" && this.datatype) {
      if (INT_TYPES.has(this.datatype)) {
        const n = Number(this.value);
        return Number.isSafeInteger(n) ? n : BigInt(this.value);
      }
      if (FLOAT_TYPES.has(this.datatype)) return Number(this.value);
      if (this.datatype === XSD + "boolean") return this.value === "true";
    }
    return this.value;
  }

  /** The term back in N-Triples surface form. */
  get n3() {
    if (this.kind === "iri") return `<${this.value}>`;
    if (this.kind === "bnode" || this.kind === "triple") return this.value;
    const body = this.value
      .replaceAll("\\", "\\\\")
      .replaceAll('"', '\\"')
      .replaceAll("\n", "\\n")
      .replaceAll("\r", "\\r");
    if (this.lang) return `"${body}"@${this.lang}`;
    if (this.datatype) return `"${body}"^^<${this.datatype}>`;
    return `"${body}"`;
  }

  toString() {
    return this.value;
  }
}

function unescapeNt(s) {
  if (!s.includes("\\")) return s;
  const simple = { t: "\t", b: "\b", n: "\n", r: "\r", f: "\f", '"': '"', "'": "'", "\\": "\\" };
  let out = "";
  for (let i = 0; i < s.length; ) {
    const c = s[i];
    if (c !== "\\" || i + 1 >= s.length) {
      out += c;
      i += 1;
      continue;
    }
    const e = s[i + 1];
    if (e in simple) {
      out += simple[e];
      i += 2;
    } else if (e === "u" && i + 6 <= s.length) {
      out += String.fromCharCode(parseInt(s.slice(i + 2, i + 6), 16));
      i += 6;
    } else if (e === "U" && i + 10 <= s.length) {
      out += String.fromCodePoint(parseInt(s.slice(i + 2, i + 10), 16));
      i += 10;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

function parseLiteral(token) {
  let i = 1;
  while (i < token.length) {
    if (token[i] === "\\") {
      i += 2;
      continue;
    }
    if (token[i] === '"') break;
    i += 1;
  }
  const lex = unescapeNt(token.slice(1, i));
  const rest = token.slice(i + 1);
  if (rest.startsWith("@")) return new Term("literal", lex, null, rest.slice(1));
  if (rest.startsWith("^^<") && rest.endsWith(">"))
    return new Term("literal", lex, rest.slice(3, -1));
  return new Term("literal", lex);
}

const cleanIri = (token) => Term.parse(token).value;

/**
 * A dump filter value → the N-Triples term token the engine's dictionary stores.
 *
 * Accepts a {@link Term}, an already-canonical token (`<iri>`, `'"x"@en'`,
 * `_:b`), or — this client's normal currency — a clean IRI string, which gets
 * its angle brackets here. `undefined`/`null` mean unbound; the engine spells
 * that `""`, so a caller never needs a separate sentinel.
 */
function term(value, what) {
  if (value === undefined || value === null) return "";
  if (value instanceof Term) return value.n3;
  if (typeof value !== "string")
    throw new TypeError(`the \`${what}\` option takes an IRI string, a Term, null, or undefined`);
  if (value.startsWith("<") || value.startsWith('"') || value.startsWith("_:")) return value;
  return `<${value}>`;
}

// ---------------------------------------------------------------------------
// wasm initialization: lazy, once. The single-file script-tag bundle embeds
// the wasm bytes and hands them over via __setWasmSource; the ESM build loads
// the sibling rete_wasm_bg.wasm (fetch in browsers, readFile in Node).
let embeddedWasm = null;
let ready = null;

/** @internal — used by the script-tag bundle entry. */
export function __setWasmSource(bytes) {
  embeddedWasm = bytes;
}

/** Initialize the WebAssembly engine (open()/build() call this for you). */
export function init(source = null) {
  ready ??= (async () => {
    const src = source ?? embeddedWasm;
    if (src) {
      await initWasm({ module_or_path: src });
    } else if (typeof process !== "undefined" && process.versions?.node) {
      const { readFile } = await import("node:fs/promises");
      const bytes = await readFile(new URL("./rete_wasm_bg.wasm", import.meta.url));
      await initWasm({ module_or_path: bytes });
    } else {
      await initWasm(); // browser: fetch the sibling .wasm
    }
  })();
  return ready;
}

// ---------------------------------------------------------------------------

/** A `.rete` graph opened for querying (bytes, or a URL over HTTP Range). */
export class Graph {
  #g;
  #remote;

  constructor(inner, source, remote) {
    this.#g = inner;
    this.source = source;
    this.#remote = remote;
  }

  /** The engine's raw JSON result envelope. */
  queryRaw(query, { reason = false } = {}) {
    const s = reason ? this.#g.query_reasoned(query, "json") : this.#g.query(query, "json");
    return JSON.parse(s);
  }

  /**
   * Run a SPARQL query: SELECT → array of `{var: Term}` rows, ASK → boolean,
   * CONSTRUCT/DESCRIBE → array of `[s, p, o]` Term triples.
   * `reason: true` turns on OWL 2 QL entailment by query rewriting.
   */
  query(query, opts = {}) {
    const env = this.queryRaw(query, opts);
    switch (env.kind) {
      case "select":
        return env.rows.map((row) =>
          Object.fromEntries(Object.entries(row).map(([k, v]) => [k, Term.parse(v)])),
        );
      case "ask":
        return env.boolean;
      case "construct":
        return env.triples.map((t) => t.map((tok) => Term.parse(tok)));
      default:
        throw new Error(`unexpected result kind: ${env.kind}`);
    }
  }

  /** Label prefix search: `[{label, subject}]` with clean IRIs. */
  prefixSearch(prefix, limit = 20) {
    return JSON.parse(this.#g.prefix_search(prefix, limit)).map(({ label, subject }) => ({
      label,
      subject: cleanIri(subject),
    }));
  }

  /** Full-text search over the file's TEXT_INDEX; returns subject IRIs. */
  textSearch(words, { contains = null, limit = 100 } = {}) {
    const list = typeof words === "string" ? words.split(/\s+/).filter(Boolean) : words;
    return JSON.parse(this.#g.text_search(list, contains ?? undefined, limit)).map((h) =>
      cleanIri(h.subject),
    );
  }

  /** Class/predicate profile: `{classes: [[iri, n]], relations: [[s,p,o,n]]}`. */
  schema() {
    const env = JSON.parse(this.#g.schema());
    return {
      classes: env.classes.map(([c, n]) => [cleanIri(c), n]),
      relations: env.relations.map(([s, p, o, n]) => [cleanIri(s), cleanIri(p), cleanIri(o), n]),
    };
  }

  info() {
    return JSON.parse(this.#g.info?.() ?? "{}");
  }

  /**
   * The embedded Dataset Card — the file's own self-description (title,
   * description, license, provenance, counts, example queries) — or `null`
   * when the file carries none. On a remote graph this is the index-free CARD
   * tier: the metadata section's byte range, nothing else.
   */
  card() {
    const s = this.#g.card();
    return s ? JSON.parse(s) : null;
  }

  /**
   * The example SPARQL queries the file ships with. Rich entries carry
   * `title`/`question`/`dimension`/`tier` alongside `sparql`; run one with
   * `g.query(g.examples()[0].sparql)`. Empty when the file has no card.
   */
  examples() {
    const card = this.card() ?? {};
    return [
      ...(card.queries ?? []).map((q) => ({ ...q })),
      ...(card.example_queries ?? []).map((sparql) => ({ sparql })),
    ];
  }

  /**
   * Validate SHACL Core shapes (Turtle) against the graph. Returns the
   * report as `"json"` (default) or `"text"`. Over a remote graph the default
   * graph validates lazily — only the shapes' target nodes are fetched.
   */
  shacl(shapesTurtle, { graph = null, format = "json" } = {}) {
    const out = this.#g.shacl(shapesTurtle, graph ?? undefined, format);
    return format === "json" ? JSON.parse(out) : out;
  }

  get quads() {
    return this.info().quads;
  }

  // -------------------------------------------------------------------------
  // Streaming dump. See DUMP_BATCH for why this crosses the wasm boundary in
  // batches rather than one quad — or one whole graph — at a time.

  /**
   * Every quad of the graph, streamed lazily:
   * `for await (const [s, p, o, g] of graph.dump())`.
   *
   * (Named `dump()`, not `quads()`, because `graph.quads` is already this
   * client's quad **count** — the engine calls the streaming scan a dump too.)
   *
   * `g` is the graph Term, or `null` for the default graph. Nothing is ever
   * materialized: the engine decodes one triple per step and the wrapper holds
   * at most `batch` of them, so this runs in memory independent of the graph's
   * size — a billion-quad file streams in the same footprint as a thousand.
   *
   * Options:
   * - `graph`: omit (or `undefined`) for the default graph **followed by every
   *   named graph**; `null` for the default graph only; an IRI string for that
   *   named graph only.
   * - `raw: true` yields the engine's N-Triples term tokens (`"<urn:a>"`,
   *   `'"x"@en'`) instead of `Term` objects — no per-term parsing, for when you
   *   are re-serializing rather than inspecting.
   * - `batch`: quads fetched per wasm call (default 10 000).
   */
  async *dump({ graph, subject, predicate, object, raw = false, batch = DUMP_BATCH } = {}) {
    const cursor = this.#cursor({ graph, subject, predicate, object });
    try {
      for (;;) {
        // A flat [s, p, o, g, s, p, o, g, …] array: one JS array per BATCH,
        // not per quad.
        const flat = cursor.next_batch(batch);
        if (flat.length === 0) return;
        for (let i = 0; i < flat.length; i += 4) {
          const g = flat[i + 3];
          yield raw
            ? [flat[i], flat[i + 1], flat[i + 2], g === "" ? null : g]
            : [
                Term.parse(flat[i]),
                Term.parse(flat[i + 1]),
                Term.parse(flat[i + 2]),
                g === "" ? null : Term.parse(g),
              ];
        }
      }
    } finally {
      // Breaking out of the loop calls this: release the engine-side cursor
      // (and the scan state it pins) instead of waiting for a GC that wasm
      // objects never get.
      cursor.free();
    }
  }

  /**
   * The graph as N-Quads text, in chunks — the constant-memory serialization
   * path. Each chunk is a batch's worth of complete lines (`\n`-terminated), so
   * chunks can be concatenated or written straight through:
   *
   * ```js
   * for await (const chunk of graph.nquads()) out.write(chunk);
   * ```
   *
   * Terms are already canonical N-Triples tokens inside the file, so the engine
   * emits the lines directly: one string per batch crosses the wasm boundary
   * instead of four per quad, and nothing is re-serialized in JavaScript.
   * Takes the same `graph` / `batch` options as {@link dump}.
   */
  async *nquads({ graph, subject, predicate, object, batch = DUMP_BATCH } = {}) {
    const cursor = this.#cursor({ graph, subject, predicate, object });
    try {
      for (;;) {
        const chunk = cursor.next_nquads(batch);
        if (chunk.length === 0) return;
        yield chunk;
      }
    } finally {
      cursor.free();
    }
  }

  /**
   * Write the whole graph as N-Quads into `sink`, and return the byte length
   * written. `sink` may be a Node `Writable`, a WHATWG `WritableStream`, or any
   * function taking a string chunk — so:
   *
   * ```js
   * await graph.writeNQuads(createWriteStream("out.nq"));   // Node
   * const parts = []; await graph.writeNQuads((c) => parts.push(c));
   * ```
   *
   * Memory stays flat: one batch of text is live at a time, and backpressure is
   * honored on both stream kinds.
   */
  async writeNQuads(sink, options = {}) {
    const write = sinkWriter(sink);
    let bytes = 0;
    for await (const chunk of this.nquads(options)) {
      bytes += chunk.length;
      await write(chunk);
    }
    await write(null); // release a WritableStream writer's lock
    return bytes;
  }

  /**
   * The whole graph as ONE N-Quads string — the ready-to-load form for
   * `store.load(text, {format: "application/n-quads"})` (Oxigraph, N3.js, …).
   *
   * Unlike {@link nquads} this necessarily holds the entire serialization in
   * memory, so it is for graphs you are willing to materialize; stream with
   * {@link nquads} / {@link writeNQuads} for anything large.
   */
  async toNQuads(options = {}) {
    const parts = [];
    for await (const chunk of this.nquads(options)) parts.push(chunk);
    return parts.join("");
  }

  /**
   * Open an engine-side cursor for the `graph` / `subject` / `predicate` /
   * `object` options of the dump methods.
   *
   * The three term filters are not a row test the wrapper applies — they go into
   * the engine's scan as a triple pattern, which routes to one permutation and
   * drops every index tile whose synopsis proves it cannot match *before*
   * fetching it. That is what makes a filtered dump of a remote graph cost the
   * slice rather than the graph.
   */
  #cursor({ graph, subject, predicate, object }) {
    // undefined → every graph; null → the default graph (the engine's ""
    // sentinel); a string → that named graph, as a clean IRI or an <iri> token.
    let slot;
    if (graph === undefined) slot = undefined;
    else if (graph === null) slot = "";
    else if (typeof graph === "string") slot = graph;
    else throw new TypeError("the `graph` option takes an IRI string, null, or undefined");
    return this.#g.quads(slot, term(subject, "subject"), term(predicate, "predicate"), term(object, "object"));
  }

  graphNames() {
    return JSON.parse(this.#g.graph_names()).map(cleanIri);
  }

  /** Remote graphs: cumulative physical fetch counters; null for bytes. */
  stats() {
    return this.#remote ? JSON.parse(this.#g.stats()) : null;
  }

  /** Remote graphs: blake3-16 hex content hash; null for bytes. */
  contentHash() {
    return this.#remote ? this.#g.content_hash() : null;
  }
}

/**
 * Adapt the three shapes `writeNQuads` accepts — a plain function, a WHATWG
 * `WritableStream`, a Node `Writable` — to one `await write(chunk)`, honoring
 * backpressure so a dump larger than memory cannot outrun its sink. Call with
 * `null` to finish (releases a WritableStream writer's lock).
 */
function sinkWriter(sink) {
  if (typeof sink === "function") return async (chunk) => (chunk === null ? undefined : sink(chunk));
  if (typeof sink?.getWriter === "function") {
    const writer = sink.getWriter();
    return async (chunk) => {
      if (chunk === null) return writer.releaseLock();
      await writer.ready;
      return writer.write(chunk);
    };
  }
  if (typeof sink?.write === "function") {
    // Node Writable: write() returning false means the buffer is full.
    return async (chunk) => {
      if (chunk === null) return undefined;
      if (sink.write(chunk) === false)
        await new Promise((resolve) => sink.once("drain", resolve));
    };
  }
  throw new TypeError("writeNQuads() takes a Writable, a WritableStream, or a function(chunk)");
}

const isBytes = (s) =>
  s instanceof Uint8Array || s instanceof ArrayBuffer || ArrayBuffer.isView(s);

/**
 * Open a `.rete` graph: a `Uint8Array`/`ArrayBuffer` file image, an
 * `http(s)://` URL queried lazily over HTTP Range requests, or — in Node — a
 * `file://` URL read lazily off disk the same way (only the byte ranges a
 * query touches, so a multi-gigabyte local file needs no memory). Lazy opens
 * use synchronous XHR: they work in Node (built-in polyfill) and in browser
 * **web workers** — not on a browser main thread (open bytes there instead).
 */
export async function open(source, { headers } = {}) {
  await init();
  if (headers) {
    throw new Error("custom headers are not supported by the JS client yet");
  }
  if (typeof source === "string" && /^(https?|file):\/\//.test(source)) {
    const isFile = source.startsWith("file:");
    const node = typeof process !== "undefined" && !!process.versions?.node;
    if (isFile && !node) {
      throw new Error("file:// .rete opens are Node-only; pass bytes in the browser");
    }
    if (typeof XMLHttpRequest === "undefined" || isFile) {
      if (node) {
        const { install } = await import("./node-sync-xhr.js");
        install();
      } else {
        throw new Error(
          "remote .rete opens need synchronous XHR: run inside a web worker " +
            "(browser main threads forbid it) or in Node",
        );
      }
    }
    return new Graph(new WasmRemoteGraph(source), source, true);
  }
  if (isBytes(source)) {
    const bytes =
      source instanceof Uint8Array
        ? source
        : new Uint8Array(source instanceof ArrayBuffer ? source : source.buffer);
    return new Graph(new WasmGraph(bytes), "<bytes>", false);
  }
  throw new TypeError("open() takes a Uint8Array/ArrayBuffer or an http(s):// URL");
}

/**
 * Build a complete `.rete` file image from RDF text (`"nt"`, `"nq"`, `"ttl"`)
 * — ready for open(), saving, or uploading. In-wasm builds are uncompressed;
 * use the `rete build` CLI for big datasets.
 */
export async function build(text, format = "nt") {
  await init();
  return wasmBuild(text, format);
}

/**
 * The engine's wasm linear memory in bytes — its high-water mark, since wasm
 * memory grows but never shrinks. Sample it around a `quads()` / `nquads()`
 * drain to *verify* the streaming claim rather than trust it: the growth stays
 * flat however many quads go by. Requires `init()` (any `open()` does it).
 */
export function heapBytes() {
  return wasmHeapBytes();
}

// RDF/JS Source for Comunica / LDflex / GraphQL-LD pipelines.
export { ReteSource } from "./comunica.js";

/**
 * The raw wasm engine — an escape hatch to the exports this wrapper does not
 * surface (`header_ranges`, `schema_url`, `reach`, `why_triples`, …). Same
 * instance the wrapper uses; strings in, JSON strings out.
 */
export * as wasm from "../vendor/pkg/rete_wasm.js";
