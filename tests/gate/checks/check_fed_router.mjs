// The playground's cross-source router, run on the SHIPPED code against small
// in-memory sources — no browser, no network, so it pins the routing decisions
// themselves rather than whatever a live file happens to hold today.
//
// It extracts the planner region of web/playground-src/app.js (FED_TOK through
// fedTargetsLabel) and gives it one dependency, a `querySource` that answers
// ASK and the generated SELECT sub-queries from the mock triples below.
//
// The cases are the two the old router got wrong (it sent each pattern to the
// ONE source that bound its subject whenever that source had the predicate):
//   - the Crossref title join: crossref-citations holds a single rdfs:label (from
//     a property declaration), so titles of citing works were looked up there
//     and the join came back empty;
//   - a sharded dataset with a partner: every pattern went to its first shard;
// plus the routing that must NOT change: an exclusive typed star stays one
// sub-query, and an endpoint still answers for the entities it bound.
import fs from "node:fs";
import { expect } from "./_expect.mjs";

const SRC = fs.readFileSync(process.env.APP_JS || "/work/web/playground-src/app.js", "utf8");
const a = SRC.indexOf("  const FED_TOK = ");
const b = SRC.indexOf("  // Try the cross-source join; on any miss");
const t = expect("check_fed_router");
if (a < 0 || b < 0 || b < a) {
  t.ok("planner region found in app.js", false, "FED_TOK … fedTargetsLabel markers moved");
  t.finish({});
  process.exit(1);
}

// ---- a tiny triple store: ASK / SELECT over the sub-queries fedStepQuery writes
const PFX = {
  rdf: "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
  rdfs: "http://www.w3.org/2000/01/rdf-schema#",
  cx: "https://w3id.org/rete/crossref#",
  schema: "http://schema.org/",
  ex: "https://example.org/",
  wdt: "http://www.wikidata.org/prop/direct/",
  skos: "http://www.w3.org/2004/02/skos/core#",
};
const iri = (q) => { const [p, l] = q.split(":"); return `<${PFX[p]}${l}>`; };
let api; // the extracted planner, filled below
const calls = [];
function evalQuery(src, q) {
  const prefixes = {};
  for (const m of q.matchAll(/PREFIX\s+([A-Za-z0-9_-]*):\s*<([^>]*)>/gi)) prefixes[m[1]] = m[2];
  const norm = (tok) => (tok[0] === "?" || tok[0] === "<" || tok[0] === '"') ? tok : `<${api.fedExpandIri(tok, prefixes)}>`;
  const body = q.slice(q.indexOf("{") + 1, q.lastIndexOf("}"));
  let values = null;
  const vm = /VALUES\s*\(([^)]*)\)\s*\{([\s\S]*?)\n\s*\}/.exec(body);
  let where = body;
  if (vm) {
    const vars = vm[1].trim().split(/\s+/).map((v) => v.slice(1));
    const rows = [...vm[2].matchAll(/\(([^)]*)\)/g)].map((m) => m[1].match(/<[^>]*>|"(?:[^"\\]|\\.)*"(?:@[\w-]+|\^\^<[^>]*>)?/g));
    values = rows.map((r) => Object.fromEntries(vars.map((v, k) => [v, r[k]])));
    where = body.replace(vm[0], " ");
  }
  where = where.replace(/FILTER\s*\((?:[^()]|\([^()]*\))*\)/gi, " "); // the mocks use no FILTER
  const pats = api.fedParseTriples(where).map((p) => ({ s: norm(p.s), p: norm(p.p), o: norm(p.o) }));
  let sols = values || [{}];
  for (const p of pats) {
    const next = [];
    for (const sol of sols) for (const [s, pr, o] of src.triples) {
      const b2 = { ...sol }; let ok = true;
      for (const [term, val] of [[p.s, s], [p.p, pr], [p.o, o]]) {
        if (term[0] === "?") { const v = term.slice(1); if (b2[v] != null && b2[v] !== val) ok = false; else b2[v] = val; }
        else if (term !== val) ok = false;
      }
      if (ok) next.push(b2);
    }
    sols = next;
  }
  if (/^\s*(PREFIX[^\n]*\n)*\s*ASK/i.test(q)) return { kind: "ask", boolean: sols.length > 0, rows: [], vars: [] };
  const lim = /LIMIT\s+(\d+)/i.exec(q);
  return { kind: "select", rows: lim ? sols.slice(0, +lim[1]) : sols, vars: [] };
}
const querySource = (src, q) => { calls.push({ src: src.id, ask: /\bASK\b/.test(q) }); return Promise.resolve(evalQuery(src, q)); };
api = new Function("querySource", SRC.slice(a, b) +
  "\nreturn { fedParse, fedParseTriples, fedExpandIri, fedRelevance, fedPlan, fedJoinExec, fedTargetsLabel };")(querySource);

async function route(sources, q) {
  const parsed = api.fedParse(q);
  const rel = await api.fedRelevance(parsed, sources);
  const plan = api.fedPlan(parsed, sources, rel);
  let out = null;
  if (plan && plan.steps.length >= 2) {
    out = await api.fedJoinExec(parsed, sources, plan, (src, sub) => querySource(src, sub).then((r) => r.rows));
  }
  return { parsed, plan, out, labels: plan ? plan.assign.map((js) => api.fedTargetsLabel(sources, js)) : null };
}
const P = (s) => s.split("\n").map((l) => l.trim()).filter(Boolean).join("\n");
const PREFIXES = Object.entries(PFX).map(([k, v]) => `PREFIX ${k}: <${v}>`).join("\n") + "\n";

const main = async () => {
  // 1. The Crossref title join. citations: cx:cites edges + ONE rdfs:label
  //    (a property declaration); crossref: the works' titles and venues.
  const doi = (n) => `<https://doi.org/10.1/${n}>`;
  const target = "<https://doi.org/10.1038/nature12373>";
  const citations = { id: "self", kind: "remote", url: "mem://citations", label: "crossref-citations", triples: [
    ...[1, 2, 3].map((n) => [doi(n), iri("cx:cites"), target]),
    [doi(9), iri("cx:cites"), doi(1)],
    [iri("cx:citedDOIAsWritten"), iri("rdfs:label"), '"cited DOI as written"'],
    [iri("cx:citedDOIAsWritten"), iri("rdf:type"), "<http://www.w3.org/2002/07/owl#DatatypeProperty>"],
  ] };
  const crossref = { id: "f1", kind: "remote", url: "mem://crossref", label: "crossref", triples: [
    ...[1, 2, 3, 9].map((n) => [doi(n), iri("rdfs:label"), `"Title ${n}"`]),
    ...[1, 2, 3].map((n) => [doi(n), iri("cx:containerTitle"), '"Nature Physics"']),
  ] };
  {
    const r = await route([citations, crossref], PREFIXES + P(`
      SELECT ?citing ?title WHERE {
        ?citing cx:cites <https://doi.org/10.1038/nature12373> .
        ?citing rdfs:label ?title .
      } LIMIT 20`));
    t.ok("title join: planned as a cross-source join", !!(r.plan && r.plan.steps.length === 2));
    t.equal("title join: rows (crossref's titles of the 3 citing works)", r.out ? r.out.rows.length : 0, 3);
    t.equal("title join: the label pattern runs on BOTH files", r.labels && r.labels[1], "crossref-citations + crossref");
    t.equal("title join: cx:cites stays on the citation file", r.labels && r.labels[0], "crossref-citations");
  }
  // The ASKs are cached per remote file: planning the same query again asks nothing.
  {
    const before = calls.filter((c) => c.ask).length;
    await route([citations, crossref], PREFIXES + "SELECT ?c ?t WHERE { ?c cx:cites <https://doi.org/10.1038/nature12373> . ?c rdfs:label ?t }");
    t.equal("source-selection ASKs are cached per file", calls.filter((c) => c.ask).length - before, 0);
  }

  // 2. A sharded dataset with a partner: the label of an item lives in whichever
  //    shard holds that item. The partner (a linkset) maps people to items.
  const wd = (n) => `<http://www.wikidata.org/entity/Q${n}>`;
  const shard = (k, items) => ({ id: k ? "f1-s" + k : "f1", group: "f1", shard: k > 0, kind: "remote", url: "mem://wd" + k,
    label: k ? "wikidata-xxl · shard " + k : "wikidata-xxl",
    triples: items.flatMap((n) => [[wd(n), iri("rdfs:label"), `"item ${n}"@en`], [wd(n), iri("wdt:P31"), wd(5)]]) });
  const linkset = { id: "self", kind: "remote", url: "mem://linkset", label: "mira-wikidata", triples:
    [1, 2, 3, 4, 5, 6].map((n) => [`<https://mira.ie/p/${n}>`, iri("skos:exactMatch"), wd(100 + n)]) };
  const shards = [0, 1, 2].map((k) => shard(k, [100 + 2 * k + 1, 100 + 2 * k + 2]));
  {
    const r = await route([linkset, ...shards], PREFIXES + P(`
      SELECT ?p ?item ?label WHERE {
        ?p skos:exactMatch ?item .
        ?item rdfs:label ?label .
      }`));
    t.equal("sharded partner: rows from all 3 shards", r.out ? r.out.rows.length : 0, 6);
    t.equal("sharded partner: the label pattern goes to every shard", r.labels && r.labels[1], "wikidata-xxl (all 3 shards)");
  }
  // …and a star the sharded side alone answers runs per shard, one subject at a time.
  {
    const r = await route([linkset, ...shards], PREFIXES + P(`
      SELECT ?p ?item ?label WHERE {
        ?p skos:exactMatch ?item .
        ?item rdfs:label ?label ; wdt:P31 ?type .
      }`));
    t.equal("sharded star: rows from all 3 shards", r.out ? r.out.rows.length : 0, 6);
    const star = r.plan && r.plan.steps.find((st) => st.pats.length === 2);
    t.ok("sharded star: label + P31 of one subject are ONE step", !!star);
    t.equal("sharded star: that step targets all 3 shards", star ? star.targets.length : 0, 3);
  }

  // 3. Unchanged: a typed star only one dataset has stays one sub-query there,
  //    even though rdf:type and rdfs:label are in both (the class decides).
  const books = { id: "self", kind: "remote", url: "mem://bph", label: "bph", triples: [
    ["<https://ex.org/b1>", iri("rdf:type"), iri("schema:Book")],
    ["<https://ex.org/b1>", iri("rdfs:label"), '"A book"'],
    ["<https://ex.org/b1>", iri("ex:edition"), "<https://ex.org/e1>"],
  ] };
  const editions = { id: "f1", kind: "remote", url: "mem://ustc", label: "ustc", triples: [
    ["<https://ex.org/e1>", iri("rdf:type"), iri("ex:Edition")],
    ["<https://ex.org/e1>", iri("ex:printer"), '"Plantin"'],
    ["<https://ex.org/p1>", iri("rdfs:label"), '"a printer"'],
  ] };
  {
    const r = await route([books, editions], PREFIXES + P(`
      SELECT ?title ?printer WHERE {
        ?b a schema:Book ; ex:edition ?e .
        ?e ex:printer ?printer .
        ?b rdfs:label ?title .
      }`));
    t.equal("typed star: `a schema:Book` is routed to bph alone", r.labels && r.labels[0], "bph");
    t.equal("typed star: joined row", r.out ? r.out.rows.length : 0, 1);
    t.equal("typed star: bph's exclusive patterns are one step", r.plan ? r.plan.steps[0].pats.length : 0, 2);
  }

  // 4. Unchanged: a live endpoint answers what nobody else has, and the
  //    properties of the entities it bound — never the other patterns.
  const lib = { id: "self", kind: "remote", url: "mem://fuero", label: "fuero_juzgo", triples: [
    ["<https://ex.org/lib1>", "<http://www.w3.org/2002/07/owl#sameAs>", wd(7)],
    ["<https://ex.org/lib1>", iri("rdfs:label"), '"Biblioteca"'],
  ] };
  const endpoint = { id: "ep", kind: "endpoint", label: "Wikidata", endpoint: "https://query.wikidata.org/sparql",
    triples: [[wd(7), iri("wdt:P17"), wd(29)], [wd(29), iri("rdfs:label"), '"Spain"@en']] };
  {
    const r = await route([lib, endpoint], PREFIXES + P(`
      PREFIX owl: <http://www.w3.org/2002/07/owl#>
      SELECT ?library ?country WHERE {
        ?lib owl:sameAs ?wd ; rdfs:label ?library .
        ?wd wdt:P17 ?c . ?c rdfs:label ?country .
      }`));
    t.equal("endpoint: owl:sameAs stays local", r.labels && r.labels[0], "fuero_juzgo");
    t.equal("endpoint: wdt:P17 goes to the endpoint", r.labels && r.labels[2], "Wikidata");
    t.equal("endpoint: ?c's label is asked of the endpoint that bound ?c (and locally)", r.labels && r.labels[3], "fuero_juzgo + Wikidata");
    t.equal("endpoint: joined row", r.out ? r.out.rows.length : 0, 1);
    t.equal("endpoint: never sent an ASK", calls.filter((c) => c.src === "ep" && c.ask).length, 0);
  }

  // 5. One dataset answers everything → no join (the union path runs it whole).
  {
    const r = await route([citations, crossref], PREFIXES + "SELECT ?c WHERE { ?c cx:cites <https://doi.org/10.1038/nature12373> }");
    t.equal("single-dataset query: no join plan", r.plan, null);
  }
  t.finish({ cases: 5 });
};
main().catch((e) => { t.ok("ran without throwing", false, String(e && e.stack || e).slice(0, 300)); t.finish({}); });
