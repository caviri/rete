// ReteSource: an RDF/JS Source over a rete Graph — plugs `.rete` files into
// Comunica (and anything else speaking the RDF/JS Source interface: LDflex,
// GraphQL-LD, Solid libraries).
//
//   import { open, ReteSource } from "rete-graph";
//   const source = new ReteSource(await open(bytesOrUrl));
//   await engine.queryBindings(sparql, { sources: [source] });
//
// Each match(s, p, o, g) call becomes ONE pattern lookup against the file's
// permutation indexes (a tiny SPARQL SELECT under the hood); Comunica then
// performs its own joins over the returned quad streams. That is the right
// trade for local/embedded files; for heavy multi-join queries over big
// REMOTE files, prefer pushing the whole query down instead — the gateway's
// SPARQL 1.1 endpoints (`/sparql/<key-or-url>`) run it inside the engine.
//
// No dependencies: minimal RDF/JS terms (termType/value/equals) and a
// minimal RDF/JS Stream (read() + readable/end/error events) — exactly the
// contracts Comunica's wrappers consume.
import { Term } from "./index.js";

// --------------------------------------------------------------------------
// Minimal RDF/JS data model (just enough for sources: no factory API).

class BaseTerm {
  equals(other) {
    return (
      !!other &&
      other.termType === this.termType &&
      other.value === this.value &&
      (this.termType !== "Literal" ||
        (other.language === this.language &&
          other.datatype.value === this.datatype.value))
    );
  }
}

class NamedNode extends BaseTerm {
  constructor(value) {
    super();
    this.termType = "NamedNode";
    this.value = value;
  }
}

class BlankNode extends BaseTerm {
  constructor(label) {
    super();
    this.termType = "BlankNode";
    this.value = label; // without the `_:`
  }
}

class Literal extends BaseTerm {
  constructor(value, languageOrDatatype) {
    super();
    this.termType = "Literal";
    this.value = value;
    if (typeof languageOrDatatype === "string") {
      this.language = languageOrDatatype;
      this.datatype = new NamedNode(
        "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString",
      );
    } else {
      this.language = "";
      this.datatype =
        languageOrDatatype ??
        new NamedNode("http://www.w3.org/2001/XMLSchema#string");
    }
  }
}

class DefaultGraph extends BaseTerm {
  constructor() {
    super();
    this.termType = "DefaultGraph";
    this.value = "";
  }
}

class Quad {
  constructor(subject, predicate, object, graph) {
    this.termType = "Quad";
    this.value = "";
    this.subject = subject;
    this.predicate = predicate;
    this.object = object;
    this.graph = graph ?? new DefaultGraph();
  }

  equals(other) {
    return (
      !!other &&
      other.termType === "Quad" &&
      this.subject.equals(other.subject) &&
      this.predicate.equals(other.predicate) &&
      this.object.equals(other.object) &&
      this.graph.equals(other.graph)
    );
  }
}

/** Engine Term (or N-Triples token) → RDF/JS term. */
function toRdfjs(t) {
  const term = t instanceof Term ? t : Term.parse(t);
  switch (term.kind) {
    case "iri":
      return new NamedNode(term.value);
    case "bnode":
      return new BlankNode(term.value.replace(/^_:/, ""));
    case "triple": {
      const [s, p, o] = splitQuoted(term.value);
      return new Quad(toRdfjs(s), toRdfjs(p), toRdfjs(o));
    }
    default:
      if (term.lang) return new Literal(term.value, term.lang);
      if (term.datatype) return new Literal(term.value, new NamedNode(term.datatype));
      return new Literal(term.value);
  }
}

/** `<< s p o >>` → its three component tokens (nesting-aware). */
function splitQuoted(token) {
  const inner = token.slice(2, -2).trim();
  const parts = [];
  let depth = 0, inString = false, start = 0;
  for (let i = 0; i < inner.length; i += 1) {
    const c = inner[i];
    if (inString) {
      if (c === "\\") i += 1;
      else if (c === '"') inString = false;
      continue;
    }
    if (c === '"') inString = true;
    else if (inner.startsWith("<<", i)) { depth += 1; i += 1; }
    else if (inner.startsWith(">>", i)) { depth -= 1; i += 1; }
    else if (c === " " && depth === 0) {
      if (i > start) parts.push(inner.slice(start, i));
      start = i + 1;
      if (parts.length === 2) { parts.push(inner.slice(start)); break; }
    }
  }
  if (parts.length < 3) parts.push(inner.slice(start));
  return parts.map((p) => p.trim()).filter(Boolean);
}

const XSD_STRING = "http://www.w3.org/2001/XMLSchema#string";

/** RDF/JS term → N-Triples surface form for the generated SPARQL. */
function toN3(term) {
  switch (term.termType) {
    case "NamedNode":
      return `<${term.value}>`;
    case "Literal": {
      const dt =
        term.language || !term.datatype || term.datatype.value === XSD_STRING
          ? null
          : term.datatype.value;
      return new Term("literal", term.value, dt, term.language || null).n3;
    }
    case "Quad":
      return `<< ${toN3(term.subject)} ${toN3(term.predicate)} ${toN3(term.object)} >>`;
    default:
      throw new TypeError(`cannot serialize ${term.termType} into a pattern`);
  }
}

const isVarish = (t) => t == null || t.termType === "Variable";

// --------------------------------------------------------------------------
// Minimal RDF/JS Stream: read() + readable/end/error. Comunica wraps this
// (asynciterator's WrappingIterator attaches the three events and drains
// read() until null).

class QuadStream {
  #quads;
  #i = 0;
  #handlers = { readable: [], end: [], error: [], data: [] };
  #error;

  constructor(quads, error = null) {
    this.#quads = quads;
    this.#error = error;
    queueMicrotask(() => {
      if (this.#error) return this.#emit("error", this.#error);
      this.#emit("readable");
      if (this.#i >= this.#quads.length) this.#emit("end");
    });
  }

  read() {
    if (this.#i < this.#quads.length) {
      const q = this.#quads[this.#i];
      this.#i += 1;
      if (this.#i >= this.#quads.length) queueMicrotask(() => this.#emit("end"));
      return q;
    }
    return null;
  }

  on(event, handler) {
    (this.#handlers[event] ??= []).push(handler);
    return this;
  }
  once(event, handler) {
    const stream = this;
    const wrap = function (...a) { stream.removeListener(event, wrap); handler.call(this, ...a); };
    return this.on(event, wrap);
  }
  addListener(event, handler) { return this.on(event, handler); }
  removeListener(event, handler) {
    const list = this.#handlers[event] ?? [];
    const at = list.indexOf(handler);
    if (at >= 0) list.splice(at, 1);
    return this;
  }
  emit() { return false; } // external emits are ignored
  destroy() { this.#i = this.#quads.length; return this; }

  #emit(event, ...args) {
    // Node's EventEmitter invokes listeners with `this` bound to the
    // emitter — asynciterator's handlers rely on it (this[DESTINATION]).
    for (const h of [...(this.#handlers[event] ?? [])]) h.call(this, ...args);
  }
}

// --------------------------------------------------------------------------

/**
 * RDF/JS Source over an open rete Graph. Pass it straight to Comunica:
 * `sources: [new ReteSource(graph)]`.
 */
export class ReteSource {
  /** @param {import("./index.js").Graph} graph */
  constructor(graph) {
    this.graph = graph;
  }

  /** All quads matching the pattern, as an RDF/JS quad stream. */
  match(subject, predicate, object, graph) {
    try {
      return new QuadStream(this.#matchArray(subject, predicate, object, graph));
    } catch (error) {
      return new QuadStream([], error);
    }
  }

  /** Number of matching quads (used by query planners for join ordering). */
  countQuads(subject, predicate, object, graph) {
    return this.#matchArray(subject, predicate, object, graph).length;
  }

  #matchArray(subject, predicate, object, graph) {
    // Blank-node arguments cannot be addressed in SPARQL — query with a
    // variable in that slot, then filter by the engine's stable label.
    const wantBnode = (term) =>
      term != null && term.termType === "BlankNode" ? `_:${term.value}` : null;
    const wantS = wantBnode(subject), wantO = wantBnode(object);

    const free = {
      s: isVarish(subject) || subject.termType === "BlankNode",
      p: isVarish(predicate),
      o: isVarish(object) || object.termType === "BlankNode",
    };
    const pattern = [
      free.s ? "?s" : toN3(subject),
      free.p ? "?p" : toN3(predicate),
      free.o ? "?o" : toN3(object),
    ].join(" ");
    const fullyBound = !free.s && !free.p && !free.o;

    const rows = [];
    const defaultGraph = new DefaultGraph();
    const runDefault = () => {
      if (fullyBound) {
        if (this.graph.query(`ASK { ${pattern} }`)) rows.push({ row: {}, graphTerm: defaultGraph });
        return;
      }
      for (const row of this.graph.query(`SELECT * WHERE { ${pattern} }`)) {
        rows.push({ row, graphTerm: defaultGraph });
      }
    };
    const runNamed = (g) => {
      if (g && fullyBound) {
        if (this.graph.query(`ASK { GRAPH <${g.value}> { ${pattern} } }`))
          rows.push({ row: {}, graphTerm: g });
        return;
      }
      const clause = g ? `GRAPH <${g.value}>` : "GRAPH ?g";
      for (const row of this.graph.query(`SELECT * WHERE { ${clause} { ${pattern} } }`)) {
        rows.push({ row, graphTerm: g ?? toRdfjs(row.g) });
      }
    };

    if (graph == null || graph.termType === "Variable") {
      runDefault();
      runNamed(null); // union semantics: default + every named graph
    } else if (graph.termType === "DefaultGraph") {
      runDefault();
    } else if (graph.termType === "NamedNode") {
      runNamed(graph);
    } else {
      throw new TypeError(`cannot match graph term ${graph.termType}`);
    }

    const out = [];
    for (const { row, graphTerm } of rows) {
      // Free positions come from the row; bound ones echo the caller's terms.
      const s = row.s ? toRdfjs(row.s) : subject;
      const p = row.p ? toRdfjs(row.p) : predicate;
      const o = row.o ? toRdfjs(row.o) : object;
      if (wantS && (s.termType !== "BlankNode" || `_:${s.value}` !== wantS)) continue;
      if (wantO && (o.termType !== "BlankNode" || `_:${o.value}` !== wantO)) continue;
      out.push(new Quad(s, p, o, graphTerm));
    }
    return out;
  }
}
