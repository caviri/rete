// rete-graph: query local and remote `.rete` graph files with SPARQL.
// A thin idiomatic wrapper over the repo's wasm engine (crates/rete-wasm) —
// the same engine behind the CLI, the Python client, and the playground.
// Mirrors the Python client's surface: Term parsing, clean IRIs (never
// `<token>` syntax), and the same open()/build() entry points.
import initWasm, {
  Graph as WasmGraph,
  RemoteGraph as WasmRemoteGraph,
  build as wasmBuild,
} from "../vendor/pkg/rete_wasm.js";

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

  get quads() {
    return this.info().quads;
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

const isBytes = (s) =>
  s instanceof Uint8Array || s instanceof ArrayBuffer || ArrayBuffer.isView(s);

/**
 * Open a `.rete` graph: a `Uint8Array`/`ArrayBuffer` file image, or an
 * `http(s)://` URL queried lazily over HTTP Range requests. Remote opens use
 * synchronous XHR: works in Node (built-in polyfill) and in browser **web
 * workers** — not on a browser main thread (open bytes there instead).
 */
export async function open(source, { headers } = {}) {
  await init();
  if (headers) {
    throw new Error("custom headers are not supported by the JS client yet");
  }
  if (typeof source === "string" && /^https?:\/\//.test(source)) {
    if (typeof XMLHttpRequest === "undefined") {
      if (typeof process !== "undefined" && process.versions?.node) {
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
