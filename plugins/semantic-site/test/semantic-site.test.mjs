// node --test plugins/semantic-site/test/
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, mkdirSync, existsSync, rmSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

import { parseCSV, parseNTriples, readSource } from '../skills/semantic-site/scripts/lib/read.mjs';
import { markdown } from '../skills/semantic-site/scripts/lib/markdown.mjs';
import { humanize, slugify } from '../skills/semantic-site/scripts/lib/model.mjs';
import { build } from '../skills/semantic-site/scripts/build.mjs';
import { check } from '../skills/site-check/scripts/check.mjs';
import { registerTools, checkInput, NAME_RE } from '../runtime/webmcp-kit.js';
import { filterItems, sortItems, searchRows } from '../runtime/query.js';

const HERE = dirname(fileURLToPath(import.meta.url));
const EXAMPLE = join(HERE, '..', 'example', 'site.json');
const tmp = () => mkdtempSync(join(tmpdir(), 'semantic-site-test-'));
const json = (p) => JSON.parse(readFileSync(p, 'utf8'));

test('CSV: quotes, escaped quotes, newlines inside quotes, CRLF, BOM-free header', () => {
  const { header, rows } = parseCSV('a,b,c\r\n1,"x, y","say ""hi"""\n2,"two\nlines",\n');
  assert.deepEqual(header, ['a', 'b', 'c']);
  assert.deepEqual(rows, [['1', 'x, y', 'say "hi"'], ['2', 'two\nlines', '']]);
});

test('N-Triples: escapes, languages, datatypes, blank nodes, N-Quads graph label', () => {
  const t = parseNTriples([
    '# comment',
    '<http://a/s> <http://a/p> "tab\\there \\u00e9 \\"q\\""@en-GB .',
    '<http://a/s> <http://a/n> "42"^^<http://www.w3.org/2001/XMLSchema#integer> .',
    '_:b1 <http://a/p> <http://a/o> <http://a/graph> .',
  ].join('\n'));
  assert.equal(t.length, 3);
  assert.deepEqual(t[0][2], { lit: 'tab\there é "q"', lang: 'en-GB' });
  assert.equal(t[1][2].dt, 'http://www.w3.org/2001/XMLSchema#integer');
  assert.deepEqual(t[2][0], { bnode: '_:b1' });
  assert.throws(() => parseNTriples('<http://a/s> <http://a/p> "open .'), /unterminated literal/);
});

test('readers: N-Triples become records with typed values and inline blank nodes', () => {
  const d = tmp();
  const p = join(d, 'g.nt');
  writeFileSync(p, [
    '<http://x/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://schema.org/Person> .',
    '<http://x/a> <http://schema.org/name> "Ada"@en .',
    '<http://x/a> <http://schema.org/name> "Ada (fr)"@fr .',
    '<http://x/a> <http://schema.org/birthDate> "1815"^^<http://www.w3.org/2001/XMLSchema#gYear> .',
    '<http://x/a> <http://schema.org/height> "1.65"^^<http://www.w3.org/2001/XMLSchema#decimal> .',
    '<http://x/a> <http://schema.org/address> _:addr .',
    '_:addr <http://schema.org/addressLocality> "London" .',
  ].join('\n'));
  const { records, predicates } = readSource(p);
  assert.equal(records.length, 1);
  const r = records[0];
  assert.deepEqual(r.types, ['http://schema.org/Person']);
  assert.equal(r.fields.birthDate[0], '1815');
  assert.equal(r.fields.height[0], 1.65);
  assert.deepEqual(r.fields.address[0], { '@nested': { addressLocality: ['London'] } });
  assert.equal(predicates.name, 'http://schema.org/name');
  rmSync(d, { recursive: true, force: true });
});

test('readers: compacted JSON-LD expands prefixes, terms and @vocab', () => {
  const d = tmp();
  const p = join(d, 'g.jsonld');
  writeFileSync(p, JSON.stringify({
    '@context': { '@vocab': 'https://schema.org/', ex: 'http://ex.org/', knows: { '@id': 'http://xmlns.com/foaf/0.1/knows', '@type': '@id' } },
    '@graph': [
      { '@id': 'ex:a', '@type': 'Person', name: 'A', knows: { '@id': 'ex:b' }, 'ex:age': { '@value': '30', '@type': 'http://www.w3.org/2001/XMLSchema#integer' } },
      { '@id': 'ex:b', '@type': 'Person', name: 'B' },
    ],
  }));
  const { records, predicates } = readSource(p);
  assert.equal(records.length, 2);
  assert.equal(records[0].iri, 'http://ex.org/a');
  assert.deepEqual(records[0].types, ['https://schema.org/Person']);
  assert.deepEqual(records[0].fields.knows, [{ '@id': 'http://ex.org/b' }]);
  assert.deepEqual(records[0].fields.age, [30]);
  assert.equal(predicates.knows, 'http://xmlns.com/foaf/0.1/knows');
  assert.equal(predicates.name, 'https://schema.org/name');
  rmSync(d, { recursive: true, force: true });
});

test('helpers: humanize, slugify, markdown escapes HTML and unsafe links', () => {
  assert.equal(humanize('foundingDate'), 'Founding date');
  assert.equal(humanize('DOI_url'), 'DOI url');
  assert.equal(slugify(' Ünïcode / Name? '), 'Unicode-Name');
  const h = markdown('# T\n\n<script>x</script> [a](javascript:alert(1)) **b**\n\n- one\n- two');
  assert.match(h, /<h1 id="t">T<\/h1>/);
  assert.match(h, /&lt;script&gt;/);
  assert.doesNotMatch(h, /javascript:/);
  assert.match(h, /<ul><li>one<\/li><li>two<\/li><\/ul>/);
});

test('query: filters ignore case and accents, sorts are numeric and keep missing last', () => {
  const items = [
    { id: 'a', title: 'Éclair', f: { year: 2004 } },
    { id: 'b', title: 'apple', f: { year: 1999 } },
    { id: 'c', title: 'Zed', f: {} },
  ];
  assert.deepEqual(filterItems(items, { query: 'ECLAIR' }).map((x) => x.id), ['a']);
  assert.deepEqual(sortItems(items, '-year').map((x) => x.id), ['a', 'b', 'c']);
  assert.deepEqual(sortItems(items, 'year').map((x) => x.id), ['b', 'a', 'c']);
  const idx = { rows: [['c', 'skos', 'SKOS', 'concepts'], ['c', 'x', 'Other', 'mentions skos']] };
  assert.deepEqual(searchRows(idx, { query: 'skos' }).map((h) => h.id), ['skos', 'x']);
});

test('kit: registers with an AbortSignal, validates names and input, falls back to provideContext', async () => {
  const seen = [];
  const ctx = { async registerTool(t, o) { seen.push([t.name, o && o.signal instanceof AbortSignal]); } };
  const tools = [
    { name: 'ok_tool', description: 'A tool that works fine.', inputSchema: { type: 'object', properties: { n: { type: 'integer', maximum: 3 } }, required: ['n'], additionalProperties: false }, execute: async ({ n }) => ({ n }) },
    { name: 'bad name', description: 'Spaces are not allowed.', execute: async () => 1 },
  ];
  const pt = await registerTools(tools, { ctx });
  assert.deepEqual(seen, [['ok_tool', true]]);
  assert.equal(pt.api, 'registerTool');
  assert.deepEqual(await pt.call('ok_tool', { n: 2 }), { n: 2 });
  await assert.rejects(pt.call('ok_tool', { n: 9 }), /at most 3/);
  await assert.rejects(pt.call('ok_tool', {}), /Missing required input "n"/);
  await assert.rejects(pt.call('ok_tool', { n: 1, x: 1 }), /Unknown input "x"/);
  assert.ok(!NAME_RE.test('bad name'));
  let provided = null;
  const old = await registerTools([tools[0]], { ctx: { async provideContext(c) { provided = c.tools.map((t) => t.name); } } });
  assert.equal(old.api, 'provideContext');
  assert.deepEqual(provided, ['ok_tool']);
  assert.throws(() => checkInput({ type: 'object', properties: { e: { type: 'string', enum: ['a'] } } }, { e: 'b' }), /one of: a/);
});

test('build + check: the example site', async () => {
  const out = join(tmp(), 'dist');
  const s = await build(EXAMPLE, { out, quiet: true });
  assert.deepEqual(s.collections.map((c) => [c.id, c.items]), [['vocabularies', 12], ['publishers', 5]]);
  assert.deepEqual(s.warnings, []);

  const r = await check(out);
  assert.deepEqual(r.issues.filter((i) => i.level === 'error'), []);
  assert.ok(r.ok);

  // Links and back-links between a CSV and an RDF graph.
  const w3c = json(join(out, 'api/publishers/world-wide-web-consortium.json'));
  assert.equal(w3c.title, 'World Wide Web Consortium'); // English label chosen over French
  assert.equal(w3c.fields.foundingDate, '1994'); // xsd:gYear stays a string
  assert.equal(w3c.backlinks.length, 8);
  const skos = json(join(out, 'api/vocabularies/skos.json'));
  assert.deepEqual(skos.links.publisher.map((l) => l.id), ['world-wide-web-consortium']);
  assert.deepEqual(skos.jsonld.publisher, { '@id': 'https://www.w3.org/' }); // the linked item's own IRI
  assert.equal(skos.jsonld['@type'], 'DefinedTermSet');
  assert.equal(skos.jsonld.identifier, 'http://www.w3.org/2004/02/skos/core#');

  // Every HTML page loads the WebMCP boot script.
  const home = readFileSync(join(out, 'index.html'), 'utf8');
  assert.match(home, /<script type="module" src="\.\/assets\/boot\.js\?v=/);
  assert.match(readFileSync(join(out, 'vocabularies/skos/index.html'), 'utf8'), /src="\.\.\/\.\.\/assets\/boot\.js/);

  // Tools per page kind.
  const tools = json(join(out, 'api/tools.json'));
  assert.deepEqual(tools.every_page, ['get_site_info', 'search', 'list_items', 'get_item', 'get_facets', 'get_current_page', 'open_page']);
  assert.ok(tools.pages.collection.includes('show_items'));
  assert.ok(tools.pages.home.includes('show_search'));
  assert.equal(tools.tools.search.annotations.untrustedContentHint, true);

  // The list API filters and sorts like the page.
  const list = json(join(out, 'api/vocabularies/index.json'));
  const w3cOnly = sortItems(filterItems(list.items, { where: { publisher: 'world wide web consortium' } }), '-year');
  assert.equal(w3cOnly.length, 8);
  assert.equal(w3cOnly[w3cOnly.length - 1].id, 'rdf');

  // Discovery files.
  assert.match(readFileSync(join(out, 'llms.txt'), 'utf8'), /^# Semantic Web vocabularies/);
  assert.equal(json(join(out, 'api/openapi.json')).openapi, '3.1.0');
  assert.ok(existsSync(join(out, '_headers')));
  assert.ok(!existsSync(join(out, 'sitemap.xml'))); // only with baseUrl
});

test('build: baseUrl makes identifiers absolute and writes a sitemap', async () => {
  const d = tmp();
  const cfg = json(EXAMPLE);
  cfg.baseUrl = 'https://example.org/vocab';
  for (const c of cfg.collections) c.source = join(dirname(EXAMPLE), c.source);
  cfg.intro = join(dirname(EXAMPLE), cfg.intro);
  cfg.pages = cfg.pages.map((p) => ({ ...p, source: join(dirname(EXAMPLE), p.source) }));
  writeFileSync(join(d, 'site.json'), JSON.stringify(cfg));
  const out = join(d, 'dist');
  await build(join(d, 'site.json'), { out, quiet: true });
  const skos = json(join(out, 'api/vocabularies/skos.json'));
  assert.equal(skos.jsonld['@id'], 'https://example.org/vocab/vocabularies/skos/');
  assert.match(readFileSync(join(out, 'sitemap.xml'), 'utf8'), /<loc>https:\/\/example\.org\/vocab\/publishers\/<\/loc>/);
  assert.match(readFileSync(join(out, 'vocabularies/skos/index.html'), 'utf8'), /<link rel="canonical" href="https:\/\/example\.org\/vocab\/vocabularies\/skos\/">/);
  assert.ok((await check(out)).ok);
});

test('build: refuses to wipe a folder it did not write', async () => {
  const d = tmp();
  mkdirSync(join(d, 'mine'));
  writeFileSync(join(d, 'mine', 'keep.txt'), 'precious');
  await assert.rejects(build(EXAMPLE, { out: join(d, 'mine'), quiet: true }), /not written by semantic-site/);
  assert.equal(readFileSync(join(d, 'mine', 'keep.txt'), 'utf8'), 'precious');
  // A folder it wrote is replaced without asking.
  await build(EXAMPLE, { out: join(d, 'site'), quiet: true });
  await build(EXAMPLE, { out: join(d, 'site'), quiet: true });
});

test('check: a page without WebMCP is an error', async () => {
  const d = tmp();
  writeFileSync(join(d, 'index.html'), '<!doctype html><html lang="en"><head><title>x</title><meta name="description" content="d"><meta name="viewport" content="width=device-width"></head><body><a href="missing.html">m</a></body></html>');
  const r = await check(d);
  assert.equal(r.ok, false);
  assert.ok(r.issues.some((i) => /no WebMCP/.test(i.message)));
  assert.ok(r.issues.some((i) => /broken link: missing\.html/.test(i.message)));
});
