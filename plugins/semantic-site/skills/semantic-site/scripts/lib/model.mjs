// site.json + sources -> the site model: collections of items with typed
// fields, resolved links between items (and back), facets and list columns.

import { resolve } from 'node:path';
import { readSource, localName } from './read.mjs';

export const RESERVED = new Set(['api', 'assets', 'agents', 'index.html', '404.html', 'llms.txt', 'sitemap.xml', 'robots.txt']);
const ID_RE = /^[a-z0-9][a-z0-9-]{0,62}$/;
const LABELS = ['title', 'name', 'label', 'prefLabel', 'headline', 'caption', 'fullName', 'givenName'];
const SUMMARIES = ['description', 'abstract', 'summary', 'comment', 'definition', 'scopeNote', 'text'];

export function slugify(s, max = 80) {
  const out = String(s ?? '')
    .normalize('NFD').replace(/\p{M}/gu, '')
    .replace(/[^A-Za-z0-9._-]+/g, '-')
    .replace(/^[-.]+|[-.]+$/g, '')
    .slice(0, max)
    .replace(/[-.]+$/g, '');
  return out;
}

// "foundingDate" -> "Founding date", "doi_url" -> "DOI url" stays readable: acronyms keep their case.
export function humanize(name) {
  const words = String(name)
    .replace(/[_-]+/g, ' ')
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .trim()
    .split(/\s+/)
    .map((w, i) => (/^[A-Z0-9]{2,}$/.test(w) ? w : i === 0 ? w[0].toUpperCase() + w.slice(1) : w.toLowerCase()));
  return words.join(' ') || String(name);
}

const isObj = (v) => v !== null && typeof v === 'object';

function typeMatches(types, want) {
  return [].concat(want).some((w) => types.some((t) => t === w || t.endsWith(`/${w}`) || t.endsWith(`#${w}`)));
}

function pickLang(vals, lang) {
  const tagged = vals.filter((v) => isObj(v) && '@language' in v);
  if (!tagged.length) return vals;
  const plain = vals.filter((v) => !(isObj(v) && '@language' in v));
  const pref = tagged.filter((v) => v['@language'] === lang || v['@language'].startsWith(`${lang}-`));
  let keep;
  if (pref.length) keep = pref;
  else if (plain.length) keep = [];
  else keep = tagged.filter((v) => v['@language'] === tagged[0]['@language']);
  return [...plain, ...keep.map((v) => v['@value'])];
}

// typedSource: values came from RDF, whose literals already carry their datatype,
// so a string of digits (an xsd:gYear, an identifier) stays a string.
export function inferType(vals, typedSource) {
  const prims = vals.filter((v) => !isObj(v));
  const objs = vals.filter(isObj);
  if (!vals.length) return 'text';
  if (objs.length && objs.every((v) => v['@id']) && !prims.length) return 'iri';
  if (objs.some((v) => v['@nested'])) return 'nested';
  if (!prims.length) return 'text';
  if (prims.every((v) => typeof v === 'boolean')) return 'boolean';
  if (prims.every((v) => typeof v === 'number' || (!typedSource && /^-?\d+(\.\d+)?$/.test(String(v))))) return 'number';
  if (prims.every((v) => /^https?:\/\/\S+$/.test(String(v)))) return 'url';
  if (prims.every((v) => /^\d{4}-\d{2}-\d{2}([T ][\d:.]+(Z|[+-]\d{2}:?\d{2})?)?$/.test(String(v)))) return 'date';
  return 'text';
}

export function buildModel(cfg, baseDir) {
  const lang = cfg.lang || 'en';
  const sources = new Map();
  const load = (p) => {
    const abs = resolve(baseDir, p);
    if (!sources.has(abs)) sources.set(abs, readSource(abs));
    return sources.get(abs);
  };
  const warnings = [];
  const pagePaths = new Set((cfg.pages || []).map((p) => p.path));

  const collections = [];
  for (const cc of cfg.collections || []) {
    if (!ID_RE.test(cc.id || '')) throw new Error(`Collection id "${cc.id}" must be lowercase letters, digits and "-" (it is a URL segment).`);
    if (RESERVED.has(cc.id) || pagePaths.has(cc.id)) throw new Error(`Collection id "${cc.id}" clashes with a reserved path or a page.`);
    if (collections.some((c) => c.id === cc.id)) throw new Error(`Two collections are called "${cc.id}".`);
    if (!cc.source) throw new Error(`Collection "${cc.id}" has no source.`);
    const src = load(cc.source);
    let recs = src.records;
    if (cc.rdfType) recs = recs.filter((r) => typeMatches(r.types, cc.rdfType));
    if (!recs.length) warnings.push(`Collection "${cc.id}" is empty (source ${cc.source}${cc.rdfType ? `, rdfType ${[].concat(cc.rdfType).join(', ')}` : ''}).`);
    const fcfg = cc.fields || {};

    // Normalise values: languages, splitting, hidden fields, numbers.
    const order = [];
    const items = recs.map((r, n) => {
      const values = {};
      for (const name of r.order) {
        const fc = fcfg[name] || {};
        if (fc.hide) continue;
        let vals = pickLang(r.fields[name] || [], lang);
        if (fc.split) vals = vals.flatMap((v) => (typeof v === 'string' ? v.split(fc.split).map((x) => x.trim()).filter(Boolean) : [v]));
        if (!vals.length) continue;
        values[name] = vals;
        if (!order.includes(name)) order.push(name);
      }
      if (r.types.length && !values.type && !(fcfg.type && fcfg.type.hide)) {
        values.type = r.types.map((t) => ({ '@id': t }));
        if (!order.includes('type')) order.push('type');
      }
      return { n, iri: r.iri, types: r.types.slice(), values, backlinks: [] };
    });

    // Label and summary fields.
    const coverage = (f) => items.filter((it) => it.values[f]).length;
    const labelField = cc.label || LABELS.filter((f) => order.includes(f)).sort((a, b) => coverage(b) - coverage(a))[0] || null;
    const summaryField = cc.summary === null ? null : cc.summary || SUMMARIES.find((f) => order.includes(f) && f !== labelField) || null;

    // Field metadata.
    const fieldOrder = [
      ...(cc.order || []),
      ...[labelField, summaryField].filter(Boolean),
      ...order,
    ].filter((f, i, a) => f && a.indexOf(f) === i && order.includes(f));
    const fields = fieldOrder.map((name) => {
      const fc = fcfg[name] || {};
      const vals = items.flatMap((it) => it.values[name] || []);
      const meta = {
        name,
        label: fc.label || (name === 'type' ? 'Type' : humanize(name)),
        type: fc.type || inferType(vals, !!src.predicates[name] || name === 'type'),
      };
      if (src.predicates[name]) meta.iri = src.predicates[name];
      if (name === 'type' && !src.predicates.type) meta.iri = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
      if (fc.property) meta.property = fc.property;
      if (fc.link) meta.link = fc.link;
      return meta;
    });
    const typed = Object.fromEntries(fields.map((f) => [f.name, f]));
    for (const it of items) {
      for (const [name, vals] of Object.entries(it.values)) {
        if (typed[name] && typed[name].type === 'number' && (!typed[name].iri || (fcfg[name] && fcfg[name].type === 'number'))) {
          it.values[name] = vals.map((v) => (typeof v === 'string' && /^-?\d+(\.\d+)?$/.test(v) ? Number(v) : v));
        }
      }
    }

    // Slugs and labels.
    const used = new Set();
    for (const it of items) {
      const keyVal = cc.key ? (it.values[cc.key] || [])[0] : undefined;
      let base = slugify(keyVal !== undefined ? (isObj(keyVal) ? keyVal['@id'] && localName(keyVal['@id']) : keyVal)
        : it.iri ? localName(it.iri) : '');
      base = base.toLowerCase();
      if (!base) base = String(it.n + 1);
      let slug = base;
      for (let k = 2; used.has(slug); k++) slug = `${base}-${k}`;
      used.add(slug);
      it.slug = slug;
      it.key = keyVal !== undefined ? (isObj(keyVal) ? keyVal['@id'] : String(keyVal)) : undefined;
      const lv = labelField ? (it.values[labelField] || []).find((v) => !isObj(v)) : undefined;
      it.label = lv !== undefined ? String(lv) : it.iri ? localName(it.iri) : slug;
      const sv = summaryField ? (it.values[summaryField] || []).find((v) => !isObj(v)) : undefined;
      it.summary = sv !== undefined ? String(sv) : '';
    }
    if (cc.key && items.some((it) => it.key === undefined)) warnings.push(`Collection "${cc.id}": some items have no "${cc.key}"; their ids fall back to the IRI or row number.`);

    collections.push({
      cfg: cc,
      id: cc.id,
      title: cc.title || humanize(cc.id),
      description: cc.description || '',
      type: cc.type || null, // null: the items' own RDF classes, else schema.org Thing
      labelField,
      summaryField,
      fields,
      items,
      predicates: src.predicates,
      sort: cc.sort || 'title',
    });
  }

  // ---- links between items
  const byIri = new Map();
  const byId = new Map(collections.map((c) => [c.id, c]));
  for (const c of collections) {
    c.byKey = new Map();
    c.byLabel = new Map();
    c.bySlug = new Map();
    for (const it of c.items) {
      if (it.iri && !byIri.has(it.iri)) byIri.set(it.iri, { c, it });
      if (it.key !== undefined) c.byKey.set(String(it.key).toLowerCase(), it);
      c.bySlug.set(it.slug.toLowerCase(), it);
      if (!c.byLabel.has(it.label.toLowerCase())) c.byLabel.set(it.label.toLowerCase(), it);
    }
  }
  for (const c of collections) {
    for (const f of c.fields) {
      if (f.link && !byId.has(f.link)) throw new Error(`Collection "${c.id}", field "${f.name}": link target "${f.link}" is not a collection.`);
    }
    const linkOf = Object.fromEntries(c.fields.filter((f) => f.link).map((f) => [f.name, byId.get(f.link)]));
    const linkedFields = new Map();
    for (const it of c.items) {
      for (const [name, vals] of Object.entries(it.values)) {
        it.values[name] = vals.map((v) => {
          let hit = null;
          const T = linkOf[name];
          if (isObj(v) && v['@id']) {
            const g = byIri.get(v['@id']);
            if (g && (!T || g.c === T)) hit = g;
          } else if (T && !isObj(v)) {
            const k = String(v).toLowerCase();
            const t = T.byKey.get(k) || T.bySlug.get(k) || T.byLabel.get(k);
            if (t) hit = { c: T, it: t };
            else {
              const g = byIri.get(String(v));
              if (g && g.c === T) hit = g;
            }
          }
          if (!hit || (hit.c === c && hit.it === it)) return v;
          const fmeta = c.fields.find((x) => x.name === name);
          hit.it.backlinks.push({ c: c.id, field: name, fieldLabel: fmeta ? fmeta.label : name, slug: it.slug, label: it.label });
          if (!linkedFields.has(name)) linkedFields.set(name, new Set());
          linkedFields.get(name).add(hit.c.id);
          return { '@link': { c: hit.c.id, slug: hit.it.slug } };
        });
      }
    }
    for (const f of c.fields) {
      const targets = linkedFields.get(f.name);
      if (targets && !f.link && targets.size === 1) f.link = [...targets][0];
      if (targets && (f.type === 'iri' || f.type === 'text') && c.items.every((it) => (it.values[f.name] || []).every((v) => isObj(v) && v['@link']))) f.type = 'link';
    }
  }

  // ---- display strings, facets, columns
  const label = (v) => {
    if (!isObj(v)) return String(v);
    if (v['@link']) {
      const t = byId.get(v['@link'].c).bySlug.get(v['@link'].slug.toLowerCase());
      return t ? t.label : v['@link'].slug;
    }
    if (v['@id']) return v['@id'];
    if (v['@nested']) {
      // "Libby Miller (Person)" when the blank node has a name; "k: v; k: v" otherwise.
      const n = v['@nested'];
      const short = (x) => (isObj(x) && x['@id'] && !x['@link'] ? localName(x['@id']) : label(x));
      const nameKey = LABELS.find((k) => n[k] && n[k].length);
      const types = (n.type || []).map(short);
      if (nameKey) return `${label(n[nameKey][0])}${types.length ? ` (${types.join(', ')})` : ''}`;
      return Object.entries(n).map(([k, vs]) => `${k}: ${vs.map(short).join(', ')}`).join('; ');
    }
    return JSON.stringify(v);
  };
  const facetLabel = (v) => (isObj(v) && v['@id'] && !v['@link'] ? localName(v['@id']) : label(v));

  for (const c of collections) {
    const n = c.items.length;
    const distinct = (f) => {
      const m = new Map();
      for (const it of c.items) for (const v of new Set((it.values[f] || []).map(facetLabel))) m.set(v, (m.get(v) || 0) + 1);
      return m;
    };
    let facets = c.cfg.facets;
    if (!facets) {
      facets = [];
      if (n >= 5) {
        const cands = c.fields
          .filter((f) => f.name !== c.labelField && f.name !== c.summaryField && f.type !== 'nested')
          .map((f) => {
            const d = distinct(f.name);
            const cov = c.items.filter((it) => it.values[f.name]).length / n;
            const per = c.items.reduce((s, it) => s + (it.values[f.name] || []).length, 0) / Math.max(1, n * cov);
            return { f: f.name, d: d.size, cov, per };
          })
          .filter((x) => x.d >= 2 && x.d <= 30 && x.d < n && x.cov >= 0.3 && x.per <= 3)
          .sort((a, b) => b.cov - a.cov || a.d - b.d);
        facets = cands.slice(0, 3).map((x) => x.f);
      }
    }
    for (const f of facets) if (!c.fields.some((x) => x.name === f)) throw new Error(`Collection "${c.id}": facet "${f}" is not a field. Fields: ${c.fields.map((x) => x.name).join(', ')}.`);
    c.facets = facets;
    c.facetValues = Object.fromEntries(facets.map((f) => [f, [...distinct(f).entries()]
      .sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0])))
      .map(([value, count]) => ({ value, count }))]));
    let columns = c.cfg.columns;
    if (!columns) {
      columns = [...facets];
      const extra = c.fields.find((f) => (f.type === 'number' || f.type === 'date') && !columns.includes(f.name) && f.name !== c.labelField);
      if (extra) columns.push(extra.name);
      columns = columns.slice(0, 4);
    }
    for (const f of columns) if (!c.fields.some((x) => x.name === f)) throw new Error(`Collection "${c.id}": column "${f}" is not a field.`);
    c.columns = columns;
    const sortField = c.sort.replace(/^-/, '');
    c.filterable = [...new Set([...columns, ...facets, ...(sortField !== 'title' && sortField !== 'id' && c.fields.some((f) => f.name === sortField) ? [sortField] : [])])];
    if (n > 20000) warnings.push(`Collection "${c.id}" has ${n} items: that is ${2 * n} files. Consider a SPARQL backend (site.json "sparql") for large graphs.`);
  }

  return { cfg, lang, collections, byId, label, facetLabel, warnings };
}
