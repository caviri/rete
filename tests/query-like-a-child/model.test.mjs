import { test } from "node:test";
import assert from "node:assert/strict";
import { RECIPES, emptySketch, make, vr, iri, lit, hole, toSparql, variables, describe, rename, tidy, humanize, prefixer, nameOf } from "../../site/query-like-a-child/model.js";

const FOAF = "http://xmlns.com/foaf/0.1/";
const body = (q) => q.text.slice(q.text.indexOf("SELECT"));

test("an empty sketch is the match-everything query", () => {
  const q = toSparql(emptySketch());
  assert.equal(q.text, "SELECT *\nWHERE {\n  ?thing ?property ?value .\n}\nLIMIT 50");
  assert.deepEqual(q.vars, ["thing", "property", "value"]);
});

test("open slots become variables of their own", () => {
  const s = emptySketch();
  s.blocks = [make("kind"), make("link")];
  const q = toSparql(s);
  assert.match(q.text, /\?thing a \?kind \./);
  assert.match(q.text, /\?thing \?property \?value \./);
  assert.deepEqual(variables(s).map((v) => v.name), ["thing", "kind", "property", "value"]);
  assert.equal(variables(s)[1].open, true);
});

test("two open slots of the same role get different names", () => {
  const s = emptySketch();
  s.blocks = [make("link"), make("link")];
  assert.match(toSparql(s).text, /\?thing \?property \?value \.\n {2}\?thing \?property2 \?value2 \./);
});

test("a hole never takes a name the sketch already uses", () => {
  const s = emptySketch();
  s.blocks = [{ ...make("link"), o: vr("property") }];
  assert.match(toSparql(s).text, /\?thing \?property2 \?property \./);
});

test("names: the file's label property, or the usual ones", () => {
  const s = emptySketch();
  s.blocks = [make("name")];
  assert.match(toSparql(s, { labelPredicate: FOAF + "name" }).text, /\?thing foaf:name \?name \./);
  assert.match(toSparql(s).text, /\?thing rdfs:label\|schema:name\|sdo:name\|foaf:name\|skos:prefLabel\|dcterms:title \?name \./);
});

test("prefixes are declared once, unknown namespaces get a name", () => {
  const s = emptySketch();
  s.blocks = [{ ...make("kind"), c: iri(FOAF + "Person") }, { ...make("link"), p: iri("http://example.org/music/playedWith"), o: vr("other") }];
  const q = toSparql(s);
  assert.match(q.text, /^PREFIX foaf: <http:\/\/xmlns.com\/foaf\/0.1\/>\nPREFIX music: <http:\/\/example.org\/music\/>\n\nSELECT/);
  assert.match(q.text, /\?thing music:playedWith \?other \./);
});

test("IRIs that cannot be a prefixed name stay whole", () => {
  const px = prefixer();
  assert.equal(px.term("http://dbpedia.org/resource/Mary_Lou_Williams"), "dbr:Mary_Lou_Williams");
  assert.equal(px.term("http://dbpedia.org/resource/Earl_%22Fatha%22_Hines"), "<http://dbpedia.org/resource/Earl_%22Fatha%22_Hines>");
  assert.equal(px.term("urn:x"), "<urn:x>");
});

test("filters", () => {
  const s = emptySketch();
  s.blocks = [
    make("name"),
    { ...make("text"), v: vr("name"), q: 'Wil"l' },
    { ...make("compare"), v: vr("n"), op: ">=", val: lit("1900", "date") },
    { ...make("compare"), v: vr("n"), op: "<", val: lit("3.5", "number") },
    { ...make("compare"), v: vr("d"), op: "<", val: lit("1950-03-01", "date") },
    { ...make("lang"), v: vr("name"), lang: "en" },
    { ...make("calc"), v: vr("d"), as: vr("year") },
  ];
  const t = toSparql(s).text;
  assert.ok(t.includes('FILTER(CONTAINS(LCASE(STR(?name)), "wil\\"l"))'));
  assert.ok(t.includes("FILTER(YEAR(?n) >= 1900)"));
  assert.ok(t.includes("FILTER(?n < 3.5)"));
  assert.ok(t.includes('FILTER(?d < "1950-03-01"^^xsd:date)'));
  assert.ok(t.includes('FILTER(LANG(?name) = "en")'));
  assert.ok(t.includes("BIND(YEAR(?d) AS ?year)"));
});

test("an unfinished filter is left out and reported", () => {
  const s = emptySketch();
  s.blocks = [make("name"), { ...make("text"), v: vr("name") }, make("is")];
  const q = toSparql(s);
  assert.ok(!q.text.includes("FILTER"));
  assert.ok(!q.text.includes("VALUES"));
  assert.equal(q.problems.length, 2);
});

test("groups", () => {
  const s = emptySketch();
  s.blocks = [
    make("kind"),
    { ...make("optional"), kids: [make("name")] },
    { ...make("without"), kids: [make("link")] },
    { ...make("either"), kids: [{ ...make("link"), o: vr("a") }], kids2: [{ ...make("link"), o: vr("b") }] },
    { ...make("optional"), kids: [] },
  ];
  const t = body(toSparql(s));
  assert.ok(t.includes("  OPTIONAL {\n    ?thing rdfs:label"));
  assert.ok(t.includes("  FILTER NOT EXISTS {\n    ?thing ?property ?value .\n  }"));
  assert.ok(t.includes("  {\n    ?thing ?property2 ?a .\n  } UNION {\n    ?thing ?property3 ?b .\n  }"));
  assert.equal((t.match(/OPTIONAL/g) || []).length, 1);
});

test("paths need a named property", () => {
  const s = emptySketch();
  s.blocks = [{ ...make("link"), p: iri(FOAF + "knows"), o: vr("other"), path: "+" }, { ...make("link"), p: iri(FOAF + "knows"), o: vr("x"), path: "^" }, { ...make("link"), path: "*" }];
  const q = toSparql(s);
  assert.ok(q.text.includes("?thing foaf:knows+ ?other ."));
  assert.ok(q.text.includes("?thing ^foaf:knows ?x ."));
  assert.ok(q.text.includes("?thing ?property ?value ."));
  assert.equal(q.problems.length, 1);
});

test("is exactly → VALUES", () => {
  const s = emptySketch();
  s.blocks = [{ ...make("is"), vals: [iri("http://dbpedia.org/resource/Count_Basie"), iri("http://dbpedia.org/resource/Art_Tatum")] }];
  assert.ok(toSparql(s).text.includes("VALUES ?thing { dbr:Count_Basie dbr:Art_Tatum }"));
});

test("answer: show, distinct, sort, limit", () => {
  const s = emptySketch();
  s.blocks = [make("kind"), make("name")];
  s.answer = { ...s.answer, show: ["name", "gone"], distinct: true, sort: { v: "name", dir: "desc" }, limit: 7 };
  const q = toSparql(s);
  assert.ok(q.text.includes("SELECT DISTINCT ?name\nWHERE"));
  assert.ok(q.text.endsWith("ORDER BY DESC(?name)\nLIMIT 7"));
  assert.deepEqual(q.vars, ["name"]);
});

test("answer: count per group", () => {
  const s = emptySketch();
  s.blocks = [{ ...make("kind"), c: vr("kind") }];
  s.answer = { ...s.answer, mode: "count", of: "thing", per: ["kind"], limit: 10 };
  const q = toSparql(s);
  assert.ok(q.text.includes("SELECT ?kind (COUNT(DISTINCT ?thing) AS ?count)"));
  assert.ok(q.text.endsWith("GROUP BY ?kind\nORDER BY DESC(?count)\nLIMIT 10"));
  s.answer.of = null; s.answer.per = [];
  assert.ok(toSparql(s).text.includes("SELECT (COUNT(*) AS ?count)\nWHERE"));
});

test("rename and tidy keep the answer in step", () => {
  const s = emptySketch();
  s.blocks = [make("kind"), make("name")];
  s.answer.show = ["thing", "name"]; s.answer.sort = { v: "name", dir: "asc" };
  rename(s, "thing", "?the person");
  assert.deepEqual(s.answer.show, ["the_person", "name"]);
  assert.ok(toSparql(s).text.includes("?the_person a ?kind ."));
  s.blocks.pop();
  tidy(s);
  assert.deepEqual(s.answer.show, ["the_person"]);
  assert.equal(s.answer.sort, null);
});

test("describe says what each variable is", () => {
  const s = emptySketch();
  s.blocks = [{ ...make("kind"), c: iri(FOAF + "Person") }, { ...make("link"), p: iri("http://purl.org/vocab/relationship/knowsOf"), o: vr("other") }, make("name")];
  const d = describe(s);
  assert.deepEqual(d.get("thing"), ["a Person", "has knows of ?other", "named ?name"]);
  assert.deepEqual(d.get("other"), ["the knows of of ?thing"]);
  assert.deepEqual(d.get("name"), ["the name of ?thing"]);
});

test("humanize", () => {
  assert.equal(humanize("knowsOf"), "knows of");
  assert.equal(humanize("birth_date"), "birth date");
  assert.equal(humanize("Person"), "Person");
  assert.equal(humanize("Mary_Lou_Williams"), "Mary Lou Williams");
  assert.equal(humanize("hasURL"), "has URL");
  assert.equal(humanize("MusicGroup"), "Music Group");
  assert.equal(nameOf(iri("http://www.wikidata.org/entity/Q42")), "Q42");
  assert.equal(nameOf(iri("http://x/y", "Why")), "Why");
});

test("every recipe is a valid sketch, with and without a file", () => {
  const d = { topClass: { v: FOAF + "Person" }, topLink: { s: { v: FOAF + "Person" }, p: { v: FOAF + "knows" } } };
  for (const r of RECIPES) for (const ctx of [{}, d]) {
    const q = toSparql(r.build(ctx));
    assert.match(q.text, /SELECT[\s\S]+WHERE \{\n[\s\S]+\n\}/, r.id);
    for (const v of q.vars) assert.ok(v === "count" || q.text.includes("?" + v), `${r.id}: ?${v}`);
  }
});

test("hole() and make() give independent objects", () => {
  const a = make("link"), b = make("link");
  a.p.k = "var";
  assert.equal(b.p.k, "hole");
  assert.notEqual(a.id, b.id);
  assert.equal(hole().k, "hole");
});
