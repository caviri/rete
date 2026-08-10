// Type declarations for rete-graph — query .rete graph files with SPARQL.

export type TermKind = "iri" | "literal" | "bnode" | "triple";

export class Term {
  kind: TermKind;
  value: string;
  datatype: string | null;
  lang: string | null;
  constructor(kind: TermKind, value: string, datatype?: string | null, lang?: string | null);
  /** Parse an N-Triples-style term token as emitted by the engine. */
  static parse(token: string): Term;
  /** number/boolean/BigInt for common XSD datatypes, else the string value. */
  toJS(): string | number | boolean | bigint;
  /** The term back in N-Triples surface form. */
  readonly n3: string;
}

export type Row = Record<string, Term>;
export type Triple = [Term, Term, Term];

/** `[subject, predicate, object, graph]`; `graph` is null in the default graph. */
export type Quad = [Term, Term, Term, Term | null];
/** The same, as the engine's raw N-Triples term tokens (`dump({raw: true})`). */
export type RawQuad = [string, string, string, string | null];

export interface QueryOptions {
  /** OWL 2 QL entailment by query rewriting. */
  reason?: boolean;
}

export interface DumpOptions {
  /**
   * Omit for the default graph followed by every named graph; `null` for the
   * default graph only; an IRI for that named graph only.
   */
  graph?: string | null;
  /**
   * Restrict the dump to a triple pattern. These **prune index tiles** rather
   * than filtering rows: the engine routes the scan to the permutation that
   * sorts on the bound components and drops every tile whose synopsis proves it
   * cannot match, without fetching it. Dumping one predicate of a large remote
   * graph therefore costs the slice, not the graph.
   *
   * A clean IRI (`"http://ex/p"`), an N-Triples token (`'"x"@en'`, `"_:b"`), or
   * a `Term`. A term the file's dictionary does not contain yields nothing.
   */
  subject?: string | Term | null;
  predicate?: string | Term | null;
  object?: string | Term | null;
  /** Quads pulled per wasm call (default 10 000). Bounds memory, not results. */
  batch?: number;
}

/** A sink for {@link Graph.writeNQuads}: Node stream, web stream, or callback. */
export type NQuadsSink =
  | { write(chunk: string): unknown; once?(event: string, cb: () => void): unknown }
  | { getWriter(): { ready: Promise<void>; write(chunk: string): unknown; releaseLock(): void } }
  | ((chunk: string) => unknown);

export class Graph {
  readonly source: string;
  /** SELECT → Row[]; ASK → boolean; CONSTRUCT/DESCRIBE → Triple[]. */
  query(query: string, opts?: QueryOptions): Row[] | boolean | Triple[];
  /** The engine's raw JSON result envelope. */
  queryRaw(query: string, opts?: QueryOptions): unknown;
  prefixSearch(prefix: string, limit?: number): { label: string; subject: string }[];
  textSearch(
    words: string | string[],
    opts?: { contains?: string | null; limit?: number },
  ): string[];
  schema(): {
    classes: [iri: string, count: number][];
    relations: [s: string, p: string, o: string, count: number][];
  };
  info(): { quads: number; terms: number; pyramidLevels: number; namedGraphs: number };
  /** The embedded Dataset Card, or null when the file carries none. */
  card(): Record<string, unknown> | null;
  /** Example SPARQL queries from the card; `sparql` plus optional rich fields. */
  examples(): { sparql: string; title?: string; question?: string }[];
  /** Validate SHACL Core shapes (Turtle); lazy over remote graphs. */
  shacl(
    shapesTurtle: string,
    opts?: { graph?: string | null; format?: "json" | "text" },
  ): unknown;
  /** The dataset's quad count (the streaming dump is {@link Graph.dump}). */
  readonly quads: number;
  /**
   * Every quad, streamed lazily and in memory that does not grow with the
   * graph: `for await (const [s, p, o, g] of graph.dump())`.
   */
  dump(opts?: DumpOptions & { raw?: false }): AsyncGenerator<Quad>;
  dump(opts: DumpOptions & { raw: true }): AsyncGenerator<RawQuad>;
  /** The graph as N-Quads text, in whole-line chunks — constant memory. */
  nquads(opts?: DumpOptions): AsyncGenerator<string>;
  /** Stream the graph as N-Quads into a sink; returns the length written. */
  writeNQuads(sink: NQuadsSink, opts?: DumpOptions): Promise<number>;
  /** The whole graph as ONE N-Quads string (materializes it — see nquads). */
  toNQuads(opts?: DumpOptions): Promise<string>;
  graphNames(): string[];
  /** Remote graphs only: cumulative fetch counters; null for bytes graphs. */
  stats(): { fileLength: number; bytes: number; requests: number } | null;
  /** Remote graphs only: blake3-16 hex content hash; null for bytes graphs. */
  contentHash(): string | null;
}

/**
 * Open a `.rete` graph: bytes, an http(s) URL read lazily over HTTP Range, or
 * (Node only) a `file://` URL read lazily off disk — same byte-range path, so
 * a huge local file is queryable without loading it. Lazy opens use
 * synchronous XHR: Node and browser web workers only (browser main threads
 * forbid sync binary XHR — open bytes there).
 */
export function open(
  source: Uint8Array | ArrayBuffer | string,
  opts?: { headers?: Record<string, string> },
): Promise<Graph>;

/** The raw wasm engine — escape hatch for exports this wrapper doesn't wrap. */
export const wasm: Record<string, (...args: never[]) => unknown>;

/** Build a complete `.rete` file image from RDF text ("nt", "nq", "ttl"). */
export function build(text: string, format?: "nt" | "nq" | "ttl"): Promise<Uint8Array>;

/** Initialize the wasm engine explicitly (open()/build() do it lazily). */
export function init(source?: BufferSource | WebAssembly.Module | URL | string | null): Promise<void>;

/**
 * The engine's wasm linear memory in bytes — a high-water mark, since wasm
 * memory grows but never shrinks. Sample it around a dump to verify that
 * streaming really does not grow with the graph.
 */
export function heapBytes(): number;

/**
 * RDF/JS Source over an open Graph — plugs `.rete` files into Comunica,
 * LDflex, GraphQL-LD, and anything speaking the RDF/JS Source interface:
 * `sources: [new ReteSource(graph)]`. Each match() is one pattern lookup
 * against the file's indexes; Comunica performs the joins. For heavy
 * multi-join queries over big REMOTE files prefer full pushdown via a
 * SPARQL endpoint (`rete serve`, or the gateway's `/sparql/<key-or-url>`).
 */
export class ReteSource {
  constructor(graph: Graph);
  readonly graph: Graph;
  /** RDF/JS Stream of quads matching the pattern (null/Variable = free). */
  match(subject?: unknown, predicate?: unknown, object?: unknown, graph?: unknown): unknown;
  /** Number of matching quads (helps query planners order joins). */
  countQuads(subject?: unknown, predicate?: unknown, object?: unknown, graph?: unknown): number;
}
