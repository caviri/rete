#!/usr/bin/env node
// Profile a data source before writing site.json, or draft one.
//
//   node inspect.mjs <source>... [--draft]
//
// Prints, per source: record count, RDF classes with counts, and each field
// with its predicate IRI, coverage, distinct values, inferred type and samples.
// --draft prints a starting site.json instead: one collection per file, or one
// per class for RDF sources with several classes.

import { basename, extname } from 'node:path';
import { readSource, localName } from './lib/read.mjs';
import { inferType, slugify, humanize } from './lib/model.mjs';

const args = process.argv.slice(2);
const draft = args.includes('--draft');
const paths = args.filter((a) => !a.startsWith('--'));
if (!paths.length) {
  console.log('usage: node inspect.mjs <source>... [--draft]');
  process.exit(1);
}

const isObj = (v) => v !== null && typeof v === 'object';
const show = (v) => (isObj(v) ? v['@id'] || v['@value'] || (v['@nested'] ? '{…}' : JSON.stringify(v)) : String(v));

function profile(records, predicates) {
  const n = records.length;
  const fields = new Map();
  for (const r of records) {
    for (const [k, vals] of Object.entries(r.fields)) {
      let f = fields.get(k);
      if (!f) { f = { name: k, iri: predicates[k], present: 0, values: [], distinct: new Set() }; fields.set(k, f); }
      f.present++;
      for (const v of vals) {
        if (f.values.length < 2000) f.values.push(v);
        if (f.distinct.size < 1000) f.distinct.add(show(v));
      }
    }
  }
  return [...fields.values()].map((f) => ({
    name: f.name,
    iri: f.iri,
    coverage: n ? f.present / n : 0,
    distinct: f.distinct.size >= 1000 ? '1000+' : f.distinct.size,
    type: inferType(f.values.map((v) => (isObj(v) && '@language' in v ? v['@value'] : v)), !!f.iri),
    samples: [...f.distinct].slice(0, 3),
  }));
}

const site = { title: 'My site', description: '', lang: 'en', collections: [], pages: [] };
for (const p of paths) {
  const { records, predicates } = readSource(p);
  const types = new Map();
  for (const r of records) for (const t of r.types) types.set(t, (types.get(t) || 0) + 1);
  const classes = [...types.entries()].sort((a, b) => b[1] - a[1]);

  if (draft) {
    const groups = classes.length > 1 ? classes.slice(0, 8).map(([t]) => t) : [null];
    for (const t of groups) {
      const id = slugify(t ? localName(t) : basename(p, extname(p))).toLowerCase().replace(/[._]/g, '-') || 'items';
      const c = { id, title: humanize(t ? localName(t) : id), description: '', source: p };
      if (t) c.rdfType = t;
      if (t && /^https?:\/\/schema\.org\//.test(t)) c.type = localName(t);
      site.collections.push(c);
    }
    continue;
  }

  console.log(`\n${p}: ${records.length} records${classes.length ? `, ${classes.length} classes` : ''}`);
  for (const [t, n] of classes.slice(0, 20)) console.log(`  class ${t}  ${n}`);
  const groups = classes.length > 1 ? classes.slice(0, 8) : [[null, records.length]];
  for (const [t] of groups) {
    const recs = t ? records.filter((r) => r.types.includes(t)) : records;
    if (t) console.log(`\n  ${localName(t)} (${recs.length})`);
    for (const f of profile(recs, predicates)) {
      console.log(`    ${f.name.padEnd(22)} ${f.type.padEnd(7)} ${(f.coverage * 100).toFixed(0).padStart(3)}%  ${String(f.distinct).padStart(5)} distinct  ${f.samples.map((s) => JSON.stringify(s.length > 40 ? `${s.slice(0, 39)}…` : s)).join(', ')}${f.iri ? `\n${' '.repeat(28)}${f.iri}` : ''}`);
    }
  }
}
if (draft) console.log(JSON.stringify(site, null, 2));
