import { test } from "node:test";
import assert from "node:assert/strict";
import { RECIPES, toSparql, graphTemplate, emptySketch, make, vr, iri } from "../../site/query-like-a-child/model.js";
import { fromSparql } from "../../site/query-like-a-child/parse.js";

const FOAF = "http://xmlns.com/foaf/0.1/";
const ctx = { labelPredicate: FOAF + "name" };
const d = { topClass: { v: FOAF + "Person" }, topLink: { s: { v: FOAF + "Person" }, p: { v: FOAF + "knows" } } };
const back = (text, c = ctx) => { const s = fromSparql(text, c); return s && toSparql(s, c).text; };

test("every recipe survives text → sketch → text", () => {
  for (const r of RECIPES) for (const data of [{}, d]) for (const c of [{}, ctx]) {
    const s = r.build(data);
    // Fill what a recipe leaves for the reader, so nothing is left out of the text.
    for (const b of s.blocks) {
      if (b.t === "is") b.vals = [iri("http://dbpedia.org/resource/Count_Basie")];
      if (b.t === "text") b.q = "basie";
      if (b.t === "compare" && b.val.k === "lit") b.val.v = "1900";
    }
    const text = toSparql(s, c).text;
    assert.equal(back(text, c), text, `${r.id}\n${text}`);
  }
});

test("a query as people write it", () => {
  const text = `# Musicians and who they know
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
PREFIX rel: <http://purl.org/vocab/relationship/>
SELECT DISTINCT ?p ?name ?other WHERE {
  ?p a foaf:Person ; foaf:name ?name ;
     rel:knowsOf ?other , ?third .
  OPTIONAL { ?other foaf:name ?otherName }
  FILTER (CONTAINS(LCASE(STR(?name)), "mary") && ?p != ?other)
} ORDER BY DESC(?name) LIMIT 20`;
  assert.equal(back(text), `PREFIX foaf: <http://xmlns.com/foaf/0.1/>
PREFIX rel: <http://purl.org/vocab/relationship/>

SELECT DISTINCT ?p ?name ?other
WHERE {
  ?p a foaf:Person .
  ?p foaf:name ?name .
  ?p rel:knowsOf ?other .
  ?p rel:knowsOf ?third .
  OPTIONAL {
    ?other foaf:name ?otherName .
  }
  FILTER(CONTAINS(LCASE(STR(?name)), "mary"))
  FILTER(?p != ?other)
}
ORDER BY DESC(?name)
LIMIT 20`);
  const s = fromSparql(text, ctx);
  assert.deepEqual(s.blocks.map((b) => b.t), ["kind", "name", "link", "link", "optional", "text", "compare"]);
});

test("unions, without, values, bind, paths", () => {
  const s = fromSparql(`PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT * WHERE {
  VALUES ?x { <http://dbpedia.org/resource/Count_Basie> foaf:Person }
  { ?x foaf:knows+ ?y } UNION { ?y ^foaf:knows ?x } UNION { ?x foaf:made/foaf:maker ?y }
  FILTER NOT EXISTS { ?y foaf:age ?age . FILTER(?age < 18) }
  ?y foaf:birthday ?b . BIND(YEAR(?b) AS ?year)
  FILTER(YEAR(?b) >= 1900)
  FILTER(LANG(?n) = "en")
  FILTER regex(str(?n), "^count", "i")
}`);
  assert.ok(s);
  assert.deepEqual(s.blocks.map((b) => b.t), ["is", "either", "without", "link", "calc", "compare", "lang", "text"]);
  const e = s.blocks[1];
  assert.equal(e.kids[0].path, "+");
  assert.equal(e.kids2[0].t, "either");
  assert.equal(e.kids2[0].kids[0].path, "^");
  assert.deepEqual(e.kids2[0].kids2.map((b) => [b.s.v, b.o.v]), [["x", "step"], ["step", "y"]]);
  assert.equal(s.answer.limit, null);
  assert.ok(!toSparql(s).text.includes("LIMIT"));
});

test("counting", () => {
  const text = `SELECT ?kind (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s a ?kind } GROUP BY ?kind ORDER BY DESC(?n) LIMIT 10`;
  const s = fromSparql(text);
  assert.deepEqual([s.answer.mode, s.answer.of, s.answer.per, s.answer.sort], ["count", "s", ["kind"], { v: "count", dir: "desc" }]);
  assert.ok(toSparql(s).text.includes("SELECT ?kind (COUNT(DISTINCT ?s) AS ?count)"));
  assert.equal(fromSparql(`SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }`).answer.of, null);
});

test("what the blocks cannot say comes back as null", () => {
  const bad = [
    `SELECT ?s WHERE { ?s ?p ?o } OFFSET 10`,
    `SELECT ?s WHERE { ?s ?p ?o } ORDER BY ?s ?p`,
    `SELECT ?s (COUNT(?o) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s`,
    `SELECT ?s (SUM(?o) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s`,
    `SELECT ?s WHERE { ?s ?p ?o } GROUP BY ?s HAVING (COUNT(?o) > 2)`,
    `SELECT ?s WHERE { ?s <http://a/b>|<http://a/c> ?o }`,
    `SELECT ?s WHERE { ?s ?p ?o . FILTER(CONTAINS(STR(?o), "Mary")) }`,
    `SELECT ?s WHERE { ?s ?p ?o . FILTER(?o > 3 || ?o < 1) }`,
    `SELECT ?s WHERE { ?s ?p ?o . FILTER(isIRI(?o)) }`,
    `SELECT ?s WHERE { { SELECT ?s WHERE { ?s ?p ?o } LIMIT 3 } }`,
    `SELECT ?s WHERE { ?s ?p ?o MINUS { ?s a ?c } }`,
    `SELECT ?s WHERE { GRAPH ?g { ?s ?p ?o } }`,
    `SELECT ?s WHERE { ?s ?p [] }`,
    `SELECT ?s WHERE { ?s ?p true }`,
    `SELECT ?s WHERE { ?s ?p "x"^^<http://example.org/dt> }`,
    `SELECT ?s WHERE { ?s unknown:p ?o }`,
    `ASK { ?s ?p ?o }`,
    `CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }`,
    `SELECT ?s WHERE { ?s ?p ?o`,
    `not a query`,
  ];
  for (const q of bad) assert.equal(fromSparql(q), null, q);
});

test("literals keep their form", () => {
  const q = (o) => `SELECT ?s\nWHERE {\n  ?s <http://example.org/vocab/p> ${o} .\n}`;
  for (const o of [`"Mary"@en`, `"plain"`, `42`, `3.5`, `"1950-03-02"^^xsd:date`, `"1984"`])
    assert.ok(back(q(o)).includes(` ${o} .`), o);
  assert.ok(back(q(`"7"^^<http://www.w3.org/2001/XMLSchema#integer>`)).includes(" 7 ."));
});

test("CONSTRUCT and the graph template of a sketch", () => {
  const s = emptySketch();
  s.blocks = [{ ...make("kind"), c: iri(FOAF + "Person") }, make("name"),
    { ...make("link"), p: iri(FOAF + "knows"), o: vr("other"), path: "+" },
    { ...make("optional"), kids: [{ ...make("link"), s: vr("other"), p: iri(FOAF + "made"), o: vr("work"), path: "^" }] }];
  s.answer.limit = 30;
  assert.equal(toSparql(s, ctx, { construct: true }).text, `PREFIX foaf: <http://xmlns.com/foaf/0.1/>

CONSTRUCT {
  ?thing a foaf:Person .
  ?thing foaf:name ?name .
  ?thing foaf:knows ?other .
  ?work foaf:made ?other .
}
WHERE {
  ?thing a foaf:Person .
  ?thing foaf:name ?name .
  ?thing foaf:knows+ ?other .
  OPTIONAL {
    ?other ^foaf:made ?work .
  }
}
LIMIT 30`);
  const g = graphTemplate(s);
  assert.deepEqual(g.links.map((l) => [l.s.v, l.o.v]), [["thing", "other"], ["work", "other"]]);
  assert.deepEqual(g.names.map((n) => [n.s.v, n.o.v]), [["thing", "name"]]);
  assert.equal(graphTemplate({ ...emptySketch(), blocks: [make("link")] }).links[0].o.v, "value");
});
