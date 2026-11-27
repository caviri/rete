// Input readers. Every reader returns { records, predicates } where
//   record = { iri?, types: [string], fields: { name: [value] }, order: [name] }
//   value  = string | number | boolean
//          | { "@id": iri }                        a resource
//          | { "@value": string, "@language": l }  a language-tagged string
//          | { "@nested": { name: [value] } }      a blank node, shown inline
//   predicates = { fieldName: predicateIRI } for sources that carry IRIs.
//
// Supported: .csv/.tsv, .json (array, {items|data|records}, or JSON-LD),
// .jsonld, .ndjson/.jsonl, .nt/.nq (N-Triples / N-Quads). Anything else
// (Turtle, RDF/XML, a SPARQL endpoint, a spreadsheet) converts to one of these
// first — see the skill's reference/config.md.

import { readFileSync } from 'node:fs';
import { extname } from 'node:path';

const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const XSD = 'http://www.w3.org/2001/XMLSchema#';
const NUMERIC = new Set(['integer', 'decimal', 'double', 'float', 'int', 'long', 'short', 'byte',
  'nonNegativeInteger', 'positiveInteger', 'negativeInteger', 'nonPositiveInteger',
  'unsignedInt', 'unsignedLong', 'unsignedShort', 'unsignedByte'].map((t) => XSD + t));

export function readSource(path) {
  const ext = extname(path).toLowerCase();
  const text = readFileSync(path, 'utf8').replace(/^\uFEFF/, '');
  switch (ext) {
    case '.csv': return fromRows(parseCSV(text, ','));
    case '.tsv': return fromRows(parseCSV(text, '\t'));
    case '.nt': case '.nq': return fromTriples(parseNTriples(text, path));
    case '.ndjson': case '.jsonl':
      return fromObjects(text.split(/\r?\n/).filter((l) => l.trim()).map((l, i) => {
        try { return JSON.parse(l); } catch (e) { throw new Error(`${path}:${i + 1}: ${e.message}`); }
      }));
    case '.json': case '.jsonld': {
      let doc;
      try { doc = JSON.parse(text); } catch (e) { throw new Error(`${path}: ${e.message}`); }
      if (ext === '.jsonld' || isJsonLd(doc)) return fromJsonLd(doc);
      if (Array.isArray(doc)) return fromObjects(doc);
      for (const k of ['items', 'data', 'records', 'results']) if (Array.isArray(doc[k])) return fromObjects(doc[k]);
      throw new Error(`${path}: expected an array of objects, {"items": [...]}, or JSON-LD.`);
    }
    default:
      throw new Error(`${path}: unsupported input "${ext}". Use CSV, TSV, JSON, JSON-LD, NDJSON, N-Triples or N-Quads.`);
  }
}

// ---------------------------------------------------------------- CSV

export function parseCSV(text, sep = ',') {
  const rows = [];
  let row = [];
  let field = '';
  let q = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (q) {
      if (ch === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; } else q = false;
      } else field += ch;
    } else if (ch === '"' && field === '') q = true;
    else if (ch === sep) { row.push(field); field = ''; }
    else if (ch === '\n' || ch === '\r') {
      if (ch === '\r' && text[i + 1] === '\n') i++;
      row.push(field); field = '';
      if (row.length > 1 || row[0] !== '') rows.push(row);
      row = [];
    } else field += ch;
  }
  if (q) throw new Error('CSV: a quoted field is not closed.');
  if (field !== '' || row.length) { row.push(field); rows.push(row); }
  if (!rows.length) return { header: [], rows: [] };
  const header = rows[0].map((h) => h.trim());
  return { header, rows: rows.slice(1) };
}

function fromRows({ header, rows }) {
  const records = rows.map((r) => {
    const fields = {};
    header.forEach((h, i) => {
      const v = (r[i] ?? '').trim();
      if (h && v !== '') fields[h] = [v];
    });
    const rec = { types: [], fields, order: header.filter(Boolean) };
    if (fields['@id']) { rec.iri = fields['@id'][0]; delete fields['@id']; }
    return rec;
  });
  return { records, predicates: {} };
}

// ---------------------------------------------------------------- plain JSON

function plain(v) {
  if (v == null) return [];
  if (Array.isArray(v)) return v.flatMap(plain);
  if (typeof v === 'object') {
    if (typeof v['@id'] === 'string' && Object.keys(v).length === 1) return [{ '@id': v['@id'] }];
    const nested = {};
    for (const [k, x] of Object.entries(v)) { const vals = plain(x); if (vals.length) nested[k] = vals; }
    return Object.keys(nested).length ? [{ '@nested': nested }] : [];
  }
  return [v];
}

function fromObjects(list) {
  const records = [];
  for (const o of list) {
    if (!o || typeof o !== 'object' || Array.isArray(o)) continue;
    const fields = {};
    const order = [];
    let iri;
    for (const [k, v] of Object.entries(o)) {
      if (k === '@id') { iri = String(v); continue; }
      const vals = plain(v);
      if (vals.length) { fields[k] = vals; order.push(k); }
    }
    records.push({ iri, types: [], fields, order });
  }
  return { records, predicates: {} };
}

// ---------------------------------------------------------------- JSON-LD (compacted)

function isJsonLd(doc) {
  if (!doc || typeof doc !== 'object') return false;
  if (Array.isArray(doc)) return doc.length > 0 && doc.every((x) => x && typeof x === 'object' && ('@id' in x || '@type' in x));
  return '@context' in doc || '@graph' in doc;
}

// Just enough context processing to recover predicate IRIs: prefixes, terms,
// @vocab, and the bare "https://schema.org" context. Remote contexts are not fetched.
function contextOf(ctx, base = {}) {
  const out = { ...base };
  for (const c of [].concat(ctx || [])) {
    if (typeof c === 'string') {
      if (/^https?:\/\/schema\.org\/?$/.test(c)) out['@vocab'] = 'https://schema.org/';
      continue;
    }
    if (c && typeof c === 'object') Object.assign(out, c);
  }
  return out;
}

function expandTerm(key, ctx, depth = 0) {
  if (depth > 5) return null;
  const def = ctx[key];
  if (typeof def === 'string') return /^[a-z][a-z0-9+.-]*:/i.test(def) ? expandTerm(def, ctx, depth + 1) || def : null;
  if (def && typeof def === 'object' && typeof def['@id'] === 'string') return expandTerm(def['@id'], ctx, depth + 1) || def['@id'];
  const m = /^([^:/]+):(.*)$/.exec(key);
  if (m && !m[2].startsWith('//')) {
    const p = ctx[m[1]];
    const pre = typeof p === 'string' ? p : p && p['@id'];
    if (pre) return pre + m[2];
  }
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(key) || key.startsWith('urn:')) return key;
  if (ctx['@vocab'] && !key.startsWith('@')) return ctx['@vocab'] + key;
  return null;
}

function fromJsonLd(doc) {
  const rootCtx = contextOf(Array.isArray(doc) ? null : doc['@context']);
  const top = Array.isArray(doc) ? doc : doc['@graph'] ? [].concat(doc['@graph']) : [doc];
  const records = [];
  const predicates = {};
  const seen = new Map();
  let anon = 0;

  function value(v, ctx) {
    if (v == null) return [];
    if (Array.isArray(v)) return v.flatMap((x) => value(x, ctx));
    if (typeof v !== 'object') return [v];
    if ('@list' in v) return value(v['@list'], ctx);
    if ('@set' in v) return value(v['@set'], ctx);
    if ('@value' in v) {
      const lang = v['@language'];
      const type = v['@type'] && (expandTerm(v['@type'], ctx) || v['@type']);
      if (lang) return [{ '@value': String(v['@value']), '@language': lang }];
      if (type && NUMERIC.has(type) && !Number.isNaN(Number(v['@value']))) return [Number(v['@value'])];
      return [v['@value']];
    }
    const keys = Object.keys(v).filter((k) => k !== '@context');
    const id = v['@id'] || v.id;
    if (id && keys.length === 1) return [{ '@id': expandTerm(id, ctx) || id }];
    if (id) { node(v, ctx); return [{ '@id': expandTerm(id, ctx) || id }]; }
    const nested = {};
    const c2 = contextOf(v['@context'], ctx);
    for (const k of keys) {
      if (k === '@type' || k === 'type') continue;
      const vals = value(v[k], c2);
      if (vals.length) nested[k] = vals;
    }
    return Object.keys(nested).length ? [{ '@nested': nested }] : [];
  }

  function node(n, parentCtx) {
    if (!n || typeof n !== 'object') return;
    const ctx = contextOf(n['@context'], parentCtx);
    const rawId = n['@id'] || n.id;
    const iri = rawId ? expandTerm(rawId, ctx) || rawId : undefined;
    let rec = iri ? seen.get(iri) : undefined;
    if (!rec) {
      rec = { iri, types: [], fields: {}, order: [] };
      records.push(rec);
      if (iri) seen.set(iri, rec);
      else rec.anon = ++anon;
    }
    for (const t of [].concat(n['@type'] || n.type || [])) {
      const ti = expandTerm(t, ctx) || t;
      if (!rec.types.includes(ti)) rec.types.push(ti);
    }
    for (const [k, v] of Object.entries(n)) {
      if (k.startsWith('@') || k === 'id' || k === 'type') continue;
      const vals = value(v, ctx);
      if (!vals.length) continue;
      const iriK = expandTerm(k, ctx);
      const name = k.includes(':') || k.includes('/') ? localName(iriK || k) : k;
      if (iriK && !predicates[name]) predicates[name] = iriK;
      if (!rec.fields[name]) { rec.fields[name] = []; rec.order.push(name); }
      rec.fields[name].push(...vals);
    }
  }

  for (const n of top) node(n, rootCtx);
  return { records, predicates };
}

// ---------------------------------------------------------------- N-Triples / N-Quads

export function localName(iri) {
  const s = String(iri).replace(/[#/]+$/, '');
  const m = /[#/:]([^#/:]+)$/.exec(s);
  return m ? m[1] : s;
}

function unescapeNT(s, where) {
  return s.replace(/\\(u[0-9A-Fa-f]{4}|U[0-9A-Fa-f]{8}|[tbnrf"'\\])/g, (_, e) => {
    switch (e[0]) {
      case 't': return '\t';
      case 'b': return '\b';
      case 'n': return '\n';
      case 'r': return '\r';
      case 'f': return '\f';
      case '"': return '"';
      case "'": return "'";
      case '\\': return '\\';
      case 'u': case 'U': {
        const cp = parseInt(e.slice(1), 16);
        if (cp > 0x10ffff) throw new Error(`${where}: bad escape \\${e}`);
        return String.fromCodePoint(cp);
      }
      default: return e;
    }
  });
}

export function parseNTriples(text, name = 'input') {
  const triples = [];
  const lines = text.split(/\r?\n/);
  for (let ln = 0; ln < lines.length; ln++) {
    const line = lines[ln];
    const where = `${name}:${ln + 1}`;
    let i = 0;
    const ws = () => { while (i < line.length && (line[i] === ' ' || line[i] === '\t')) i++; };
    const term = () => {
      ws();
      const ch = line[i];
      if (ch === '<') {
        const end = line.indexOf('>', i);
        if (end < 0) throw new Error(`${where}: unterminated IRI`);
        const iri = unescapeNT(line.slice(i + 1, end), where);
        i = end + 1;
        return { iri };
      }
      if (ch === '_' && line[i + 1] === ':') {
        let j = i + 2;
        while (j < line.length && !/[\s.]/.test(line[j])) j++;
        // a blank node label may contain dots, but not end with one
        while (j < line.length && line[j] === '.' && j + 1 < line.length && !/\s/.test(line[j + 1])) {
          j++;
          while (j < line.length && !/[\s.]/.test(line[j])) j++;
        }
        const b = line.slice(i, j);
        i = j;
        return { bnode: b };
      }
      if (ch === '"') {
        let j = i + 1;
        let raw = '';
        while (j < line.length) {
          if (line[j] === '\\') { raw += line[j] + (line[j + 1] ?? ''); j += 2; continue; }
          if (line[j] === '"') break;
          raw += line[j++];
        }
        if (line[j] !== '"') throw new Error(`${where}: unterminated literal`);
        i = j + 1;
        const lit = { lit: unescapeNT(raw, where) };
        if (line[i] === '@') {
          let k = i + 1;
          while (k < line.length && /[A-Za-z0-9-]/.test(line[k])) k++;
          lit.lang = line.slice(i + 1, k);
          i = k;
        } else if (line[i] === '^' && line[i + 1] === '^') {
          i += 2;
          const dt = term();
          if (!dt.iri) throw new Error(`${where}: bad datatype`);
          lit.dt = dt.iri;
        }
        return lit;
      }
      throw new Error(`${where}: unexpected "${line.slice(i, i + 20)}"`);
    };
    ws();
    if (i >= line.length || line[i] === '#') continue;
    const s = term();
    const p = term();
    const o = term();
    ws();
    if (line[i] !== '.') term(); // N-Quads graph label, ignored
    ws();
    if (line[i] !== '.') throw new Error(`${where}: expected "." at the end of the statement`);
    if (!p.iri) throw new Error(`${where}: the predicate must be an IRI`);
    triples.push([s, p.iri, o]);
  }
  return triples;
}

function fromTriples(triples) {
  const bySubject = new Map();
  const predicates = {};
  const nameOf = new Map();
  const taken = new Map();

  function fieldName(p) {
    let n = nameOf.get(p);
    if (n) return n;
    const base = localName(p).replace(/[^A-Za-z0-9_.-]/g, '_') || 'field';
    n = base;
    if (taken.has(n) && taken.get(n) !== p) {
      const segs = p.replace(/[#/][^#/]*$/, '').split(/[/#:.]+/).filter((x) => /^[A-Za-z][A-Za-z-]*$/.test(x) && !/^(www|http|https|org|com|net|ns)$/.test(x));
      n = `${(segs.pop() || 'x').replace(/-/g, '_')}_${base}`;
      let k = 2;
      while (taken.has(n) && taken.get(n) !== p) n = `${base}_${k++}`;
    }
    taken.set(n, p);
    nameOf.set(p, n);
    predicates[n] = p;
    return n;
  }

  const key = (t) => (t.iri ? `<${t.iri}>` : t.bnode);
  for (const [s, p, o] of triples) {
    if (s.lit) continue;
    const k = key(s);
    let rec = bySubject.get(k);
    if (!rec) {
      rec = { iri: s.iri, bnode: s.bnode, types: [], fields: {}, order: [] };
      bySubject.set(k, rec);
    }
    if (p === RDF_TYPE && o.iri) {
      if (!rec.types.includes(o.iri)) rec.types.push(o.iri);
      continue;
    }
    const n = fieldName(p);
    let v;
    if (o.iri) v = { '@id': o.iri };
    else if (o.bnode) v = { '@bnode': o.bnode };
    else if (o.lang) v = { '@value': o.lit, '@language': o.lang.toLowerCase() };
    else if (o.dt && NUMERIC.has(o.dt) && o.lit.trim() !== '' && !Number.isNaN(Number(o.lit))) v = Number(o.lit);
    else if (o.dt === XSD + 'boolean') v = o.lit === 'true' || o.lit === '1';
    else v = o.lit;
    if (!rec.fields[n]) { rec.fields[n] = []; rec.order.push(n); }
    rec.fields[n].push(v);
  }

  // Blank nodes become inline values of whatever points at them (one level deep).
  const inline = (v, depth) => {
    if (!v || typeof v !== 'object' || !v['@bnode']) return v;
    const b = bySubject.get(v['@bnode']);
    if (!b || depth > 2) return null;
    const nested = {};
    for (const [k, vals] of Object.entries(b.fields)) nested[k] = vals.map((x) => inline(x, depth + 1)).filter((x) => x != null);
    if (b.types.length) nested.type = b.types.map((t) => ({ '@id': t }));
    return { '@nested': nested };
  };
  const records = [];
  for (const rec of bySubject.values()) {
    if (!rec.iri) continue;
    for (const k of Object.keys(rec.fields)) rec.fields[k] = rec.fields[k].map((v) => inline(v, 0)).filter((v) => v != null);
    delete rec.bnode;
    records.push(rec);
  }
  return { records, predicates };
}
