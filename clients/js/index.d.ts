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

export interface QueryOptions {
  /** OWL 2 QL entailment by query rewriting. */
  reason?: boolean;
}

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
  readonly quads: number;
  graphNames(): string[];
  /** Remote graphs only: cumulative fetch counters; null for bytes graphs. */
  stats(): { fileLength: number; bytes: number; requests: number } | null;
  /** Remote graphs only: blake3-16 hex content hash; null for bytes graphs. */
  contentHash(): string | null;
}

/**
 * Open a `.rete` graph: bytes, or an http(s) URL read lazily over HTTP Range.
 * Remote opens use synchronous XHR: Node and browser web workers only
 * (browser main threads forbid sync binary XHR — open bytes there).
 */
export function open(
  source: Uint8Array | ArrayBuffer | string,
  opts?: { headers?: Record<string, string> },
): Promise<Graph>;

/** Build a complete `.rete` file image from RDF text ("nt", "nq", "ttl"). */
export function build(text: string, format?: "nt" | "nq" | "ttl"): Promise<Uint8Array>;

/** Initialize the wasm engine explicitly (open()/build() do it lazily). */
export function init(source?: BufferSource | WebAssembly.Module | URL | string | null): Promise<void>;
