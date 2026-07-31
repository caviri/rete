// rete-fs.js — project a .rete graph onto a filesystem.
//
// This is the transferable core of the experiment: no DOM, no fetch of its own.
// It takes an `engine` (something that can answer SPARQL and label lookups) plus
// the file's own self-description (header + card + baked schema), and exposes a
// lazy tree — folders, files, listings, previews — over several *views*.
//
// The point of multiple views is that RDF is a graph, not a tree, so there is no
// single honest folder hierarchy. There are several, each true in its own way:
//
//   types       classes as folders, instances as files      (semantic)
//   namespace   the IRI's own path, like a URL tree          (lexical)
//   predicates  one table per relation                       (tabular)
//   graphs      named graphs as top-level volumes            (dataset)
//   sections    the physical byte layout of the file itself  (archive)
//
// Only `types`, `predicates` and `sections` are free: they read the header and
// the baked schema pyramid, never the triple index. `namespace` samples. Every
// listing is paginated, because a class can hold 50M instances and a folder
// pane cannot.

// ---------------------------------------------------------------- constants

export const HEADER_LEN = 1024;
const SECTION_DIR_OFFSET = 64;
const SECTION_ENTRY_LEN = 24;

// SectionKind, mirroring crates/rete-core/src/header.rs.
const SECTION_KINDS = {
  1: { name: "METADATA", blurb: "The Dataset Card: the file's own description, in JSON." },
  2: { name: "DICTIONARY", blurb: "Front-coded term table. Every IRI and literal, stored once." },
  3: { name: "INDEX", blurb: "The six permutation streams (SPO/POS/OSP/SOP/PSO/OPS), tiled." },
  4: { name: "PYRAMID META", blurb: "Community summary + the baked schema this browser reads." },
  5: { name: "NAMED GRAPHS", blurb: "Per-graph permutation containers, sharing the dictionary." },
  6: { name: "TEXT INDEX", blurb: "Token table + posting lists for word/CONTAINS search." },
};

// Predicates whose object is worth showing as a thumbnail in the icon view.
// Deliberately a whitelist: sniffing every value for image-ness would mean
// pulling every triple of every listed resource.
const IMAGE_PREDICATES = [
  "https://schema.org/image",
  "http://schema.org/image",
  "https://schema.org/thumbnailUrl",
  "http://schema.org/thumbnailUrl",
  "https://schema.org/thumbnail",
  "http://schema.org/thumbnail",
  "http://xmlns.com/foaf/0.1/depiction",
  "http://xmlns.com/foaf/0.1/img",
  "http://www.wikidata.org/prop/direct/P18",
  "http://www.europeana.eu/schemas/edm/isShownBy",
  "http://www.europeana.eu/schemas/edm/preview",
  "http://purl.org/dc/terms/hasVersion",
  "http://www.w3.org/2006/vcard/ns#photo",
];

// Whitelisting image *predicates* is not enough — plenty of datasets invent
// their own (`mtg` uses a bare `…/image`). So the thumbnail probe also matches
// on the value looking like an image URL, and on the predicate's local name.
// Both halves run inside the one decoration query, over the same scan.
const IMAGE_URL_RE = /\.(jpe?g|png|webp|gif|avif|svg)([?#]|$)|\/full\/[^/]+\/0\/default\.jpg/i;
const IMAGE_PRED_RE = /(image|thumb|depict|photo|picture|img|isShownBy|preview)/i;
const IMAGE_VALUE_SPARQL_RE = "\\\\.(jpe?g|png|webp|gif|avif|svg)([?#]|$)";

const LABEL_PREDICATES = [
  "http://www.w3.org/2000/01/rdf-schema#label",
  "http://www.w3.org/2004/02/skos/core#prefLabel",
  "https://schema.org/name",
  "http://schema.org/name",
  "http://purl.org/dc/terms/title",
  "http://purl.org/dc/elements/1.1/title",
  "http://xmlns.com/foaf/0.1/name",
  "http://www.w3.org/2004/02/skos/core#altLabel",
];

const RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

export const PAGE_SIZE = 200;

// ------------------------------------------------------------------ header

/**
 * Parse the fixed 1 KB header into the full section directory. Unlike the
 * plaza's reader (which only wants the card) this keeps every entry, because
 * the section directory *is* the archive's table of contents — the `sections`
 * view renders it directly.
 */
export function parseHeader(buf) {
  const b = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (b.length < HEADER_LEN) throw new Error(`header too small: ${b.length} bytes`);
  const dec = new TextDecoder();
  if (dec.decode(b.subarray(0, 4)) !== "RETE") throw new Error("not a .rete file (bad magic)");

  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const u16 = (o) => dv.getUint16(o, true);
  const u32 = (o) => dv.getUint32(o, true);
  const u64 = (o) => Number(dv.getBigUint64(o, true)); // offsets never exceed 2^53

  const sectionCount = u16(44);
  const sections = [];
  for (let i = 0; i < sectionCount; i++) {
    const p = SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN;
    if (p + SECTION_ENTRY_LEN > b.length) break;
    const kind = u16(p);
    const known = SECTION_KINDS[kind];
    sections.push({
      kind,
      name: known ? known.name : `UNKNOWN (${kind})`,
      blurb: known ? known.blurb : "A section this build does not recognize — forward compatibility.",
      flags: u16(p + 2),
      offset: u64(p + 8),
      length: u64(p + 16),
    });
  }

  return {
    version: b[4],
    flags: b[5],
    hasQuads: (b[5] & 1) !== 0,
    contentHash: Array.from(b.subarray(8, 24), (x) => x.toString(16).padStart(2, "0")).join(""),
    quadCount: u64(24),
    termCount: u64(32),
    pyramidLevels: u16(40),
    dictCodec: b[42],
    blockCodec: b[43],
    schemaMetaLen: u32(46),
    sections,
  };
}

/** Fetch [start, end] inclusive over HTTP Range, tolerating a server that ignores it. */
export async function rangeFetch(url, start, end) {
  const res = await fetch(url, { headers: { Range: `bytes=${start}-${end}` } });
  if (!res.ok && res.status !== 206) throw new Error(`HTTP ${res.status} for ${url}`);
  const body = new Uint8Array(await res.arrayBuffer());
  return res.status === 206 ? body : body.subarray(start, end + 1);
}

/** Total byte length of a remote file, from the Content-Range of a 1-byte probe. */
export async function remoteSize(url) {
  const res = await fetch(url, { headers: { Range: "bytes=0-0" } });
  const cr = res.headers.get("Content-Range");
  if (cr) {
    const m = /\/(\d+)$/.exec(cr);
    if (m) return Number(m[1]);
  }
  const len = res.headers.get("Content-Length");
  return len ? Number(len) : 0;
}

/**
 * Read a .rete's self-description without touching its index: the 1 KB header,
 * then the metadata section it points at. Two range requests, whatever the file
 * weighs — this is what lets the browser open a 17 GB archive instantly.
 */
export async function readSelfDescription(url) {
  const head = await rangeFetch(url, 0, HEADER_LEN - 1);
  const header = parseHeader(head);
  const size = await remoteSize(url);
  let card = null;
  const meta = header.sections.find((s) => s.kind === 1);
  if (meta && meta.length > 0) {
    try {
      const raw = await rangeFetch(url, meta.offset, meta.offset + meta.length - 1);
      card = JSON.parse(new TextDecoder().decode(raw));
    } catch (_) {
      card = null; // a card that won't parse is not worth failing the open over
    }
  }
  return { header, card, size };
}

// -------------------------------------------------------------- term helpers

/** Parse an N-Triples-shaped term string into {iri, value, datatype, lang}. */
export function parseTerm(v) {
  const s = String(v == null ? "" : v);
  if (s.startsWith("<") && s.endsWith(">")) return { iri: true, value: s.slice(1, -1) };
  if (s.startsWith("_:")) return { iri: false, blank: true, value: s };
  const m = /^"((?:[^"\\]|\\.)*)"(?:\^\^<([^>]+)>|@([\w-]+))?$/s.exec(s);
  if (m) {
    return {
      iri: false,
      value: m[1].replace(/\\(.)/g, (_, c) => ({ n: "\n", t: "\t", r: "\r" })[c] || c),
      datatype: m[2] || null,
      lang: m[3] || null,
    };
  }
  return { iri: false, value: s, datatype: null, lang: null };
}

/** The last meaningful segment of an IRI — the part that reads like a filename. */
export function localName(iri) {
  if (!iri) return "";
  const cut = Math.max(iri.lastIndexOf("#"), iri.lastIndexOf("/"), iri.lastIndexOf(":"));
  const tail = cut >= 0 && cut < iri.length - 1 ? iri.slice(cut + 1) : iri;
  return decodeURIComponent(tail) || iri;
}

/** The namespace part of an IRI: everything up to and including the last separator. */
export function namespaceOf(iri) {
  const cut = Math.max(iri.lastIndexOf("#"), iri.lastIndexOf("/"));
  return cut >= 0 ? iri.slice(0, cut + 1) : iri;
}

const esc = (iri) => `<${iri}>`;

export function humanBytes(n) {
  if (!Number.isFinite(n)) return "—";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n < 10 && i > 0 ? n.toFixed(2) : n < 100 && i > 0 ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
}

export const humanCount = (n) => (Number.isFinite(n) ? n.toLocaleString("en-US") : "—");

// ------------------------------------------------------------------ context

/**
 * Everything a view needs. `engine.query(sparql, format)` returns the playground
 * result envelope; `meta` is the header/card/schema read at open time.
 */
export function makeContext({ engine, meta }) {
  return {
    engine,
    meta,
    cache: new Map(),
    // Which predicates supply display names. Defaults to the usual suspects,
    // but every dataset names things its own way — `farmacos-es` has no
    // rdfs:label at all — so the UI lets you point this at any literal
    // predicate the schema knows about. Changing it invalidates `deco:` cache
    // entries, which is what `forgetLabels` is for.
    labelPredicates: [...LABEL_PREDICATES],
    forgetLabels() {
      for (const k of [...this.cache.keys()]) if (k.startsWith("deco:")) this.cache.delete(k);
    },
    /** Run a SELECT and hand back its rows as arrays of raw term strings. */
    async select(sparql) {
      const env = JSON.parse(await this.engine.query(sparql, "table"));
      if (env.kind === "select") return { vars: env.vars || [], rows: env.rows || [] };
      if (env.kind === "ask") return { vars: ["boolean"], rows: [{ boolean: String(env.boolean) }] };
      return { vars: [], rows: [] };
    },
    /**
     * Best-effort display name *and* thumbnail for a batch of IRIs, in one
     * query. Pure decoration: an IRI is already a perfectly good filename, so a
     * failure here degrades to local names rather than breaking the listing.
     * Returns a Map of iri → { label, image }.
     */
    async labels(iris) {
      const want = iris.filter((i) => !this.cache.has(`deco:${i}`));
      if (want.length) {
        const values = want.map(esc).join(" ");
        const chosen = this.labelPredicates;

        const absorb = (rows) => {
          for (const r of rows) {
            const s = parseTerm(r.s), p = parseTerm(r.p), l = parseTerm(r.l);
            if (!s.iri || !l.value) continue;
            const key = `deco:${s.value}`;
            const cur = this.cache.get(key) || { label: null, image: null, rank: Infinity };
            const looksImage =
              IMAGE_PREDICATES.includes(p.value) ||
              IMAGE_URL_RE.test(l.value) ||
              (IMAGE_PRED_RE.test(p.value) && /^https?:/i.test(l.value));
            if (looksImage) {
              if (!cur.image) cur.image = l.value;
            } else {
              // Respect the picker's order: an earlier predicate wins over a
              // later one no matter which row came back first.
              const rank = chosen.indexOf(p.value);
              if (rank >= 0 && rank < cur.rank) { cur.label = l.value; cur.rank = rank; }
            }
            this.cache.set(key, cur);
          }
        };

        // Two plain queries rather than one clever disjunction: the engine
        // rejects `?p IN (…) || REGEX(…)` at parse time ("expected
        // ENCODE_FOR_URI"), though either half parses on its own.
        const preds = [...chosen, ...IMAGE_PREDICATES].map(esc).join(", ");
        const qNamed = `SELECT ?s ?p ?l WHERE { VALUES ?s { ${values} } ?s ?p ?l . FILTER(?p IN (${preds})) }`;
        try {
          absorb((await this.select(qNamed)).rows);
        } catch (err) {
          // Decoration is optional, but a silent failure here is
          // indistinguishable from "this dataset has no labels".
          console.warn("[rete-fs] label query failed:", err.message, "\n", qNamed);
        }

        // Fallback for datasets that name their image predicate something of
        // their own: match on the value looking like an image URL. Only worth a
        // second round-trip when the whitelist found nothing on this page.
        const anyImage = want.some((i) => (this.cache.get(`deco:${i}`) || {}).image);
        if (!anyImage) {
          const qLooks = `SELECT ?s ?p ?l WHERE { VALUES ?s { ${values} } ?s ?p ?l . FILTER(REGEX(STR(?l), "${IMAGE_VALUE_SPARQL_RE}", "i")) }`;
          try {
            absorb((await this.select(qLooks)).rows);
          } catch (err) {
            console.warn("[rete-fs] image probe failed:", err.message);
          }
        }
        for (const i of want) if (!this.cache.has(`deco:${i}`)) this.cache.set(`deco:${i}`, { label: null, image: null });
      }
      const out = new Map();
      for (const i of iris) out.set(i, this.cache.get(`deco:${i}`) || { label: null, image: null });
      return out;
    },
  };
}

/**
 * Every predicate in the file whose object is a literal, biggest first — the
 * candidates for "use this as the display name". Read from the baked schema, so
 * offering the choice costs nothing.
 */
export function candidateLabelPredicates(ctx) {
  const totals = new Map();
  for (const r of schemaRelations(ctx)) {
    if (r.object !== "(literal)") continue;
    totals.set(r.predicate, (totals.get(r.predicate) || 0) + r.count);
  }
  return [...totals.entries()]
    .sort((a, b) => b[1] - a[1])
    .map(([iri, count]) => ({ iri, count, name: localName(iri) }));
}

// ------------------------------------------------------------------- views

const dirNode = (view, id, name, extra = {}) => ({ view, id, name, kind: "dir", ...extra });
const fileNode = (view, id, name, extra = {}) => ({ view, id, name, kind: "file", ...extra });

/**
 * Build a background label pass for a page of IRIs.
 *
 * Labels are decoration: an IRI's local name is already a perfectly good
 * filename. Looking them up is not cheap — on a big remote file a batch of 200
 * `?s ?p ?l` probes faults in a lot of dictionary chunks — so the listing must
 * never wait for them. `list()` hands back this callback instead; the UI paints
 * rows immediately and patches names in as chunks land.
 */
function deferLabels(ctx, iris, chunk = 50) {
  return async (apply) => {
    for (let i = 0; i < iris.length; i += chunk) {
      const slice = iris.slice(i, i + chunk);
      let got;
      try {
        got = await ctx.labels(slice);
      } catch (_) {
        return; // labels are optional; a failure just leaves local names
      }
      apply(got);
    }
  };
}

/**
 * A resource's own contents: the resources it points at, and the ones pointing
 * back. This is what makes the tree unbounded — you descend Person → knows →
 * Person → knows → … for as long as the graph keeps going, which is the honest
 * shape of the data rather than a limitation of the browser.
 *
 * Only IRI objects become children; literals are the *contents* of the file and
 * live in the preview pane. `trail` carries the ancestor IRIs so a cycle is
 * marked rather than silently walked forever.
 */
export async function resourceChildren(ctx, node) {
  const iri = node.iri;
  const trail = node.trail || [];
  const [out, back] = await Promise.all([
    ctx.select(`SELECT ?p ?o WHERE { ${esc(iri)} ?p ?o } LIMIT 600`),
    ctx.select(`SELECT ?s ?p WHERE { ?s ?p ${esc(iri)} } LIMIT 200`),
  ]);

  const links = [];
  const seen = new Set();
  for (const r of out.rows) {
    const p = parseTerm(r.p), o = parseTerm(r.o);
    if (!o.iri || p.value === RDF_TYPE) continue;
    const key = `${p.value}|${o.value}`;
    if (seen.has(key)) continue;
    seen.add(key);
    links.push({ predicate: p.value, target: o.value });
  }
  links.sort((a, b) => a.predicate.localeCompare(b.predicate) || a.target.localeCompare(b.target));

  const childTrail = [...trail, iri];
  const items = links.map((l) => {
    const cycles = trail.includes(l.target);
    return {
      view: node.view,
      id: `res:${l.target}`,
      name: localName(l.target),
      kind: "dir", // a resource is a folder *and* a file: it has contents and links
      resource: true,
      iri: l.target,
      trail: childTrail,
      cycles,
      detail: `${localName(l.predicate)}${cycles ? " ↺" : ""}`,
    };
  });

  if (back.rows.length) {
    items.push({
      view: node.view,
      id: `back:${iri}`,
      name: "↩ referenced by",
      kind: "dir",
      backrefs: back.rows.map((r) => ({ subject: parseTerm(r.s).value, predicate: parseTerm(r.p).value })),
      trail: childTrail,
      detail: `${humanCount(back.rows.length)}${back.rows.length >= 200 ? "+" : ""} incoming`,
    });
  }

  return {
    items,
    decorate: deferLabels(ctx, items.filter((i) => i.iri).map((i) => i.iri)),
    note: items.length
      ? null
      : "No outgoing links — this resource holds only literal values. Open it to read them.",
  };
}

/** The `↩ referenced by` pseudo-folder: whatever points at the parent. */
function backrefChildren(ctx, node) {
  const items = node.backrefs.map((b) => ({
    view: node.view,
    id: `res:${b.subject}`,
    name: localName(b.subject),
    kind: "dir",
    resource: true,
    iri: b.subject,
    trail: node.trail || [],
    detail: `↩ ${localName(b.predicate)}`,
  }));
  return { items, decorate: deferLabels(ctx, items.map((i) => i.iri)) };
}

/** Classes from the baked schema, biggest first. Index-free. */
function schemaClasses(ctx) {
  const s = ctx.meta.schema;
  if (!s || !Array.isArray(s.classes)) return [];
  return s.classes
    .map(([iri, n]) => ({ iri: iri.replace(/^<|>$/g, ""), count: n }))
    .sort((a, b) => b.count - a.count);
}

/** Class→predicate→class relations from the baked schema. Index-free. */
function schemaRelations(ctx) {
  const s = ctx.meta.schema;
  if (!s || !Array.isArray(s.relations)) return [];
  return s.relations.map(([sub, pred, obj, n]) => ({
    subject: sub.replace(/^<|>$/g, ""),
    predicate: pred.replace(/^<|>$/g, ""),
    object: obj.replace(/^<|>$/g, ""),
    count: n,
  }));
}

// --- types -----------------------------------------------------------------

const typesView = {
  id: "types",
  label: "Types",
  icon: "◆",
  hint: "Classes as folders, instances as files. Read from the baked schema pyramid — no index access.",

  async list(ctx, node) {
    if (node && node.backrefs) return backrefChildren(ctx, node);
    if (node && node.resource) return resourceChildren(ctx, node);
    if (!node) {
      const classes = schemaClasses(ctx);
      if (!classes.length) {
        return { items: [], note: "This file carries no baked schema pyramid, so classes cannot be listed without scanning it." };
      }
      return {
        items: classes.map((c) =>
          dirNode("types", `types:${c.iri}`, localName(c.iri), {
            iri: c.iri,
            count: c.count,
            detail: `${humanCount(c.count)} instances`,
          })
        ),
        decorate: deferLabels(ctx, classes.slice(0, 60).map((c) => c.iri)),
        note: `${classes.length} classes, from the schema pyramid.`,
      };
    }

    // Inside a class: a page of its instances. `?s a <C>` is a contiguous POS
    // scan, so paging costs the tiles that page actually covers.
    const offset = node.offset || 0;
    const q = `SELECT ?s WHERE { ?s ${esc(RDF_TYPE)} ${esc(node.iri)} } LIMIT ${PAGE_SIZE + 1} OFFSET ${offset}`;
    const { rows } = await ctx.select(q);
    const more = rows.length > PAGE_SIZE;
    const page = rows.slice(0, PAGE_SIZE).map((r) => parseTerm(r.s).value);

    const items = page.map((iri) => ({
      view: "types",
      id: `res:${iri}`,
      name: localName(iri),
      kind: "dir",
      resource: true,
      iri,
      classIri: node.iri,
      trail: [],
    }));
    items.unshift(
      fileNode("types", `shape:${node.iri}`, "_shape.json", {
        special: "shape",
        iri: node.iri,
        detail: "property shape, from the schema",
      })
    );
    return {
      items,
      more,
      nextOffset: offset + PAGE_SIZE,
      decorate: deferLabels(ctx, page),
      note: `showing ${humanCount(offset + 1)}–${humanCount(offset + page.length)} of ${humanCount(node.count)}`,
    };
  },
};

// --- namespace -------------------------------------------------------------

const namespaceView = {
  id: "namespace",
  label: "Namespace",
  icon: "▤",
  hint: "The IRI's own path, treated as a folder tree. Built from a bounded sample of each class — a map, not a census.",

  async list(ctx, node) {
    if (node && node.backrefs) return backrefChildren(ctx, node);
    if (node && node.resource) return resourceChildren(ctx, node);
    const trie = await buildNamespaceTrie(ctx);
    const prefix = node ? node.path : [];
    let cursor = trie;
    for (const seg of prefix) {
      cursor = cursor.children.get(seg);
      if (!cursor) return { items: [], note: "nothing here" };
    }

    const items = [];
    for (const [seg, child] of [...cursor.children.entries()].sort((a, b) => b[1].count - a[1].count)) {
      const path = [...prefix, seg];
      if (child.children.size === 0 && child.leafIri) {
        items.push({
          view: "namespace", id: `res:${child.leafIri}`, name: seg,
          kind: "dir", resource: true, iri: child.leafIri, path, trail: [],
        });
      } else {
        items.push(
          dirNode("namespace", `ns:${path.join("/")}`, seg, {
            path,
            count: child.count,
            detail: `${humanCount(child.count)} sampled`,
          })
        );
      }
    }
    return {
      items,
      note: node ? null : `sampled ${humanCount(trie.count)} IRIs across the largest classes`,
    };
  },
};

/**
 * Build the IRI trie once per session. Classes and predicates come free from the
 * schema; instances are sampled — a bounded number of subjects from each of the
 * largest classes — because enumerating every subject of a billion-triple file
 * is exactly what this format exists to avoid.
 */
async function buildNamespaceTrie(ctx) {
  if (ctx.cache.has("nstrie")) return ctx.cache.get("nstrie");

  const root = { children: new Map(), count: 0 };
  const add = (iri) => {
    const m = /^([a-z][a-z0-9+.-]*):\/\/([^/]*)(.*)$/i.exec(iri);
    const segs = m
      ? [`${m[1]}://${m[2]}`, ...m[3].split(/[/#]/).filter(Boolean)]
      : iri.split(/[/#:]/).filter(Boolean);
    let cur = root;
    cur.count++;
    for (let i = 0; i < segs.length; i++) {
      const seg = decodeURIComponent(segs[i]);
      if (!cur.children.has(seg)) cur.children.set(seg, { children: new Map(), count: 0 });
      cur = cur.children.get(seg);
      cur.count++;
      if (i === segs.length - 1) cur.leafIri = iri;
    }
  };

  for (const c of schemaClasses(ctx)) add(c.iri);
  for (const r of schemaRelations(ctx)) if (r.predicate.startsWith("http")) add(r.predicate);

  const top = schemaClasses(ctx).slice(0, 8);
  for (const c of top) {
    try {
      const { rows } = await ctx.select(
        `SELECT ?s WHERE { ?s ${esc(RDF_TYPE)} ${esc(c.iri)} } LIMIT 120`
      );
      for (const r of rows) {
        const t = parseTerm(r.s);
        if (t.iri) add(t.value);
      }
    } catch (_) { /* one class failing to sample must not sink the view */ }
  }

  ctx.cache.set("nstrie", root);
  return root;
}

// --- predicates ------------------------------------------------------------

const predicatesView = {
  id: "predicates",
  label: "Predicates",
  icon: "≡",
  hint: "One table per relation, with exact counts from the schema pyramid. Each is a two-column file you can export as CSV.",

  async list(ctx, node) {
    const rels = schemaRelations(ctx);
    if (!rels.length) {
      return { items: [], note: "This file carries no baked schema pyramid, so relations cannot be listed without scanning it." };
    }
    if (!node) {
      const byPredicate = new Map();
      for (const r of rels) {
        const e = byPredicate.get(r.predicate) || { count: 0, shapes: [] };
        e.count += r.count;
        e.shapes.push(r);
        byPredicate.set(r.predicate, e);
      }
      const items = [...byPredicate.entries()]
        .sort((a, b) => b[1].count - a[1].count)
        .map(([iri, e]) =>
          dirNode("predicates", `pred:${iri}`, localName(iri), {
            iri,
            count: e.count,
            shapes: e.shapes,
            detail: `${humanCount(e.count)} triples · ${e.shapes.length} shape${e.shapes.length === 1 ? "" : "s"}`,
          })
        );
      return { items, note: `${items.length} predicates, exact counts, no index read.` };
    }

    // Inside a predicate: the table itself, plus one narrowed table per shape.
    const items = [
      fileNode("predicates", `ptable:${node.iri}`, `${localName(node.iri)}.csv`, {
        special: "ptable",
        iri: node.iri,
        detail: `all ${humanCount(node.count)} pairs`,
      }),
    ];
    for (const s of node.shapes || []) {
      items.push(
        fileNode("predicates", `ptable:${node.iri}|${s.subject}|${s.object}`,
          `${localName(s.subject)}→${localName(s.object)}.csv`, {
            special: "ptable",
            iri: node.iri,
            shape: s,
            detail: `${humanCount(s.count)} pairs`,
          })
      );
    }
    return { items };
  },
};

// --- graphs ----------------------------------------------------------------

const graphsView = {
  id: "graphs",
  label: "Graphs",
  icon: "⬒",
  hint: "Named graphs as top-level volumes. Quads only — a file with no named graphs has just the default graph.",

  async list(ctx, node) {
    if (node && node.backrefs) return backrefChildren(ctx, node);
    if (node && node.resource) return resourceChildren(ctx, node);
    if (!node) {
      if (!ctx.meta.header.hasQuads) {
        return { items: [], note: "This file holds a single default graph — there are no named graphs to browse." };
      }
      const { rows } = await ctx.select("SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } } LIMIT 500");
      const items = rows.map((r) => {
        const g = parseTerm(r.g).value;
        return dirNode("graphs", `graph:${g}`, localName(g), { iri: g, detail: g });
      });
      return { items, note: `${items.length} named graphs` };
    }

    const offset = node.offset || 0;
    const q = `SELECT ?s WHERE { GRAPH ${esc(node.iri)} { ?s ?p ?o } } LIMIT ${PAGE_SIZE + 1} OFFSET ${offset}`;
    const { rows } = await ctx.select(q);
    const more = rows.length > PAGE_SIZE;
    const seen = new Set();
    const items = [];
    for (const r of rows.slice(0, PAGE_SIZE)) {
      const iri = parseTerm(r.s).value;
      if (seen.has(iri)) continue;
      seen.add(iri);
      items.push({
        view: "graphs", id: `res:${iri}`, name: localName(iri),
        kind: "dir", resource: true, iri, graphIri: node.iri, trail: [],
      });
    }
    return {
      items, more, nextOffset: offset + PAGE_SIZE,
      decorate: deferLabels(ctx, items.map((i) => i.iri)),
    };
  },
};

// --- sections --------------------------------------------------------------

const sectionsView = {
  id: "sections",
  label: "Sections",
  icon: "▮",
  hint: "The physical archive: the header's typed section directory, with real byte offsets. Costs zero queries — it is the first 1 KB of the file.",

  async list(ctx, node) {
    if (node) return { items: [] };
    const { header, size } = ctx.meta;
    const items = [
      fileNode("sections", "sect:header", "HEADER", {
        special: "section",
        section: { name: "HEADER", offset: 0, length: HEADER_LEN, kind: 0,
          blurb: "The directory itself: format version, counts, content hash, and the offset+length of every section below." },
        detail: `0 – ${HEADER_LEN} · ${humanBytes(HEADER_LEN)}`,
      }),
    ];
    for (const s of [...header.sections].sort((a, b) => a.offset - b.offset)) {
      if (!s.length) continue;
      items.push(
        fileNode("sections", `sect:${s.kind}`, s.name, {
          special: "section",
          section: s,
          detail: `${humanBytes(s.length)} · ${((s.length / (size || 1)) * 100).toFixed(1)}% of file`,
        })
      );
    }
    if (ctx.meta.card) {
      items.push(fileNode("sections", "sect:card", "_README.md", { special: "card", detail: "the Dataset Card, rendered" }));
    }
    return { items, note: `${items.length} sections · ${humanBytes(size)} total` };
  },
};

export const VIEWS = [typesView, namespaceView, predicatesView, graphsView, sectionsView];
export const VIEW_BY_ID = new Map(VIEWS.map((v) => [v.id, v]));

// --------------------------------------------------------------- file bodies

/**
 * Open a "file". Returns { title, subtitle, tabs: [{id, label, kind, ...}] }
 * where kind is one of: properties | table | text | json | sectionmap.
 * The UI renders tabs; this decides what a node's contents actually are.
 */
export async function openFile(ctx, node) {
  if (node.special === "section") return sectionFile(ctx, node);
  if (node.special === "card") return cardFile(ctx);
  if (node.special === "shape") return shapeFile(ctx, node);
  if (node.special === "ptable") return predicateTable(ctx, node);
  if (!node.iri) throw new Error("this folder has no contents of its own");
  return resourceFile(ctx, node);
}

/** A resource: its outgoing properties, its incoming references, and its Turtle. */
async function resourceFile(ctx, node) {
  const iri = node.iri;
  const [out, incoming] = await Promise.all([
    ctx.select(`SELECT ?p ?o WHERE { ${esc(iri)} ?p ?o } LIMIT 1000`),
    ctx.select(`SELECT ?s ?p WHERE { ?s ?p ${esc(iri)} } LIMIT 300`),
  ]);

  const props = out.rows.map((r) => ({ predicate: parseTerm(r.p), object: parseTerm(r.o) }));
  const refs = incoming.rows.map((r) => ({ subject: parseTerm(r.s), predicate: parseTerm(r.p) }));

  const label = props.find((p) => LABEL_PREDICATES.includes(p.predicate.value));
  const types = props.filter((p) => p.predicate.value === RDF_TYPE).map((p) => p.object.value);

  let turtle = null;
  try {
    const env = JSON.parse(await ctx.engine.query(`DESCRIBE ${esc(iri)}`, "ttl"));
    turtle = env.text || null;
  } catch (_) { /* DESCRIBE is a convenience; the property table is the truth */ }

  const json = {};
  json["@id"] = iri;
  for (const p of props) {
    const key = localName(p.predicate.value);
    const val = p.object.iri ? { "@id": p.object.value }
      : p.object.lang ? { "@value": p.object.value, "@language": p.object.lang }
      : p.object.datatype ? { "@value": p.object.value, "@type": p.object.datatype }
      : p.object.value;
    if (json[key] === undefined) json[key] = val;
    else if (Array.isArray(json[key])) json[key].push(val);
    else json[key] = [json[key], val];
  }

  return {
    title: label ? label.object.value : localName(iri),
    subtitle: iri,
    types,
    downloads: [
      { label: "JSON-LD", ext: "json", mime: "application/ld+json", body: () => JSON.stringify(json, null, 2) },
      ...(turtle ? [{ label: "Turtle", ext: "ttl", mime: "text/turtle", body: () => turtle }] : []),
    ],
    tabs: [
      { id: "props", label: `Properties (${props.length})`, kind: "properties", props, refs },
      { id: "json", label: "JSON-LD", kind: "json", value: json },
      ...(turtle ? [{ id: "ttl", label: "Turtle", kind: "text", value: turtle }] : []),
    ],
  };
}

/** A class's property shape, straight from the schema. No data is read. */
async function shapeFile(ctx, node) {
  const rels = schemaRelations(ctx).filter((r) => r.subject === node.iri);
  const shape = {
    class: node.iri,
    instances: node.count,
    properties: rels
      .sort((a, b) => b.count - a.count)
      .map((r) => ({ predicate: r.predicate, range: r.object, count: r.count })),
  };
  return {
    title: `${localName(node.iri)} — shape`,
    subtitle: node.iri,
    downloads: [{ label: "JSON", ext: "json", mime: "application/json", body: () => JSON.stringify(shape, null, 2) }],
    tabs: [
      {
        id: "shape", label: `Properties (${rels.length})`, kind: "table",
        vars: ["predicate", "range", "triples"],
        rows: shape.properties.map((p) => [localName(p.predicate), localName(p.range), humanCount(p.count)]),
      },
      { id: "json", label: "JSON", kind: "json", value: shape },
    ],
  };
}

/** A predicate's value table — the sheet a spreadsheet user actually wants. */
async function predicateTable(ctx, node) {
  const limit = node.limit || 500;
  const shape = node.shape;
  const where = shape && shape.subject !== "(untyped)" && shape.subject !== "(literal)"
    ? `?s ${esc(RDF_TYPE)} ${esc(shape.subject)} . ?s ${esc(node.iri)} ?o`
    : `?s ${esc(node.iri)} ?o`;
  const { rows } = await ctx.select(`SELECT ?s ?o WHERE { ${where} } LIMIT ${limit}`);

  const pairs = rows.map((r) => [parseTerm(r.s), parseTerm(r.o)]);
  const csv = () =>
    ["subject,object", ...pairs.map(([s, o]) => `${csvCell(s.value)},${csvCell(o.value)}`)].join("\n");

  return {
    title: localName(node.iri),
    subtitle: node.iri,
    downloads: [
      { label: "CSV", ext: "csv", mime: "text/csv", body: csv },
      {
        label: "JSON", ext: "json", mime: "application/json",
        body: () => JSON.stringify(pairs.map(([s, o]) => ({ subject: s.value, object: o.value })), null, 2),
      },
    ],
    tabs: [
      {
        id: "table", label: `Rows (${pairs.length}${pairs.length >= limit ? "+" : ""})`, kind: "table",
        vars: ["subject", "object"],
        rows: pairs.map(([s, o]) => [s.iri ? localName(s.value) : s.value, o.iri ? localName(o.value) : o.value]),
        iris: pairs.map(([s, o]) => [s.iri ? s.value : null, o.iri ? o.value : null]),
      },
    ],
    note: pairs.length >= limit ? `capped at ${limit} rows — export for the rest` : null,
  };
}

/** A physical section: what it is, where it starts, how big it is. */
function sectionFile(ctx, node) {
  const s = node.section;
  const { size, header } = ctx.meta;
  return {
    title: s.name,
    subtitle: `bytes ${humanCount(s.offset)} – ${humanCount(s.offset + s.length - 1)}`,
    downloads: [],
    tabs: [
      {
        id: "map", label: "Extent", kind: "sectionmap",
        section: s, fileSize: size, blurb: s.blurb,
        rows: [
          ["kind", `${s.kind}${s.name ? ` (${s.name})` : ""}`],
          ["offset", `${humanCount(s.offset)} bytes`],
          ["length", `${humanCount(s.length)} bytes (${humanBytes(s.length)})`],
          ["share of file", `${((s.length / (size || 1)) * 100).toFixed(2)}%`],
          ["format version", `0x${header.version.toString(16).padStart(2, "0")}`],
          ["content hash", header.contentHash],
        ],
      },
    ],
  };
}

/** The Dataset Card, as the archive's README. */
function cardFile(ctx) {
  const card = ctx.meta.card || {};
  const lines = [];
  if (card.title) lines.push(`# ${card.title}`, "");
  if (card.description) lines.push(card.description, "");
  const facts = [];
  for (const [k, v] of Object.entries(card)) {
    if (["title", "description", "exampleQueries", "examples"].includes(k)) continue;
    if (v == null || typeof v === "object") continue;
    facts.push(`- **${k}**: ${v}`);
  }
  if (facts.length) lines.push("## Facts", "", ...facts, "");
  return {
    title: card.title || "Dataset Card",
    subtitle: "the file's own description, read from the METADATA section",
    downloads: [
      { label: "JSON", ext: "json", mime: "application/json", body: () => JSON.stringify(card, null, 2) },
      { label: "Markdown", ext: "md", mime: "text/markdown", body: () => lines.join("\n") },
    ],
    tabs: [
      { id: "md", label: "README", kind: "text", value: lines.join("\n") },
      { id: "json", label: "JSON", kind: "json", value: card },
    ],
  };
}

// ------------------------------------------------------------------ extract

const csvCell = (v) => (/[",\n]/.test(v) ? `"${String(v).replace(/"/g, '""')}"` : String(v));

/**
 * Extract a folder: pull its triples out of the archive as a real file. This is
 * the verb the archive metaphor promises and that RDF tooling never offers —
 * "give me this folder as a spreadsheet".
 */
export async function extract(ctx, node, { format = "csv", limit = 5000 } = {}) {
  let q, filename;
  // A predicate folder yields two columns, so its N-Triples form has to put the
  // predicate back in the middle — the other folders already select all three.
  let fixedPredicate = null;
  if (node.view === "types" && node.iri) {
    q = `SELECT ?s ?p ?o WHERE { ?s ${esc(RDF_TYPE)} ${esc(node.iri)} . ?s ?p ?o } LIMIT ${limit}`;
    filename = localName(node.iri);
  } else if (node.view === "predicates" && node.iri) {
    q = `SELECT ?s ?o WHERE { ?s ${esc(node.iri)} ?o } LIMIT ${limit}`;
    filename = localName(node.iri);
    fixedPredicate = node.iri;
  } else if (node.view === "graphs" && node.iri) {
    q = `SELECT ?s ?p ?o WHERE { GRAPH ${esc(node.iri)} { ?s ?p ?o } } LIMIT ${limit}`;
    filename = localName(node.iri);
  } else {
    throw new Error("this folder cannot be extracted");
  }

  const { vars, rows } = await ctx.select(q);
  const terms = rows.map((r) => vars.map((v) => parseTerm(r[v])));

  if (format === "csv") {
    const body = [vars.join(","), ...terms.map((t) => t.map((x) => csvCell(x.value)).join(","))].join("\n");
    return { filename: `${filename}.csv`, mime: "text/csv", body, count: rows.length };
  }
  if (format === "json") {
    const body = JSON.stringify(
      terms.map((t) => Object.fromEntries(t.map((x, i) => [vars[i], x.value]))), null, 2
    );
    return { filename: `${filename}.json`, mime: "application/json", body, count: rows.length };
  }
  const body = rows
    .map((r) => {
      const cols = vars.map((v) => r[v]);
      const triple = fixedPredicate ? [cols[0], esc(fixedPredicate), cols[1]] : cols;
      return `${triple.join(" ")} .`;
    })
    .join("\n");
  return { filename: `${filename}.nt`, mime: "application/n-triples", body, count: rows.length };
}
