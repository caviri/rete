// The Markdown emphasis rule: one grammar, six call sites, and the corpus it
// runs on.
//
// WHY THIS EXISTS. `*italic*` is rendered by a single regex, used from six
// places across five files: web/playground-src/app.js (mdLite, mdPlain and
// markdownInline all share one declaration), scripts/preview/card.mjs,
// experiments/plaza/js/rete-card.js, and the two GENERATED copies —
// docs/playground.html and docs/plaza/js/rete-card.js.
//
// The three sources cannot import from one another: app.js is concatenated into
// docs/playground.html as a classic script, card.mjs is Node ESM, and
// rete-card.js is a browser ES module served standalone. This repo ships no
// bundler that reaches all three, and adding one to share a single regex would
// buy less than it costs — every artifact here is deliberately dependency-free
// and self-contained. So the copies are pinned IDENTICAL here instead of
// unified, which turns the next divergence into a failing gate rather than a
// bug someone finds years later on the live site.
//
// They had already drifted before anyone noticed: app.js's mdLite used
// `[^*]+` where the other five used `[^*\n]+`, so emphasis could span a
// paragraph break in the playground and nowhere else.
//
// Three assertions:
//   1. every copy carries the same regex literal, and the same rationale;
//   2. the shipped corpus renders correctly — the literal asterisks that used to
//      be eaten survive, the real italics still italicise;
//   3. every span the rule forms is genuinely flanked, checked against an
//      INDEPENDENT character-level reading of CommonMark §6.2 rather than
//      against the regex itself.
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(new URL("../../../", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"));
const fail = [];
const check = (ok, msg) => { if (!ok) fail.push(msg); };

// ---------------------------------------------------------------- 1. copies --
const SOURCES = [
  "web/playground-src/app.js",
  "scripts/preview/card.mjs",
  "experiments/plaza/js/rete-card.js",
];
// Built from the sources above — a stale artifact means someone edited the rule
// and skipped scripts/build_playground.py / scripts/build_plaza.py.
const GENERATED = [
  "docs/playground.html",
  "docs/plaza/js/rete-card.js",
];

const read = (rel) => fs.readFileSync(path.join(ROOT, rel), "utf8");
const literalOf = (src) => {
  const m = src.match(/^[ \t]*const MD_EMPHASIS = (\/.*\/gu);$/m);
  return m ? m[1] : null;
};

const literals = new Map();
for (const rel of [...SOURCES, ...GENERATED]) {
  const src = read(rel);
  const lit = literalOf(src);
  check(lit !== null, `${rel}: no \`const MD_EMPHASIS = /…/gu;\` declaration found`);
  if (lit) literals.set(rel, lit);
  // The old rule must be gone everywhere, in every spelling it ever had.
  const stale = src.match(/\(\^\|\[\^\*\]\)\\\*\(\[\^\*(?:\\n)?\]\+\)\\\*\(\?!\\\*\)/g);
  check(!stale, `${rel}: the old un-flanked emphasis regex is still present (${stale && stale.length} occurrence(s))`);
}
const distinct = new Set(literals.values());
check(distinct.size <= 1,
  `the emphasis rule has diverged across its copies:\n${[...literals].map(([f, l]) => `    ${f}\n      ${l}`).join("\n")}`);

// The rationale travels with the rule: the comment block above the declaration
// must match too (dedented — app.js sits inside an IIFE, the others do not).
const rationaleOf = (src) => {
  const at = src.search(/^[ \t]*const MD_EMPHASIS = /m);
  if (at < 0) return null;
  const lines = src.slice(0, at).split("\n");
  if (lines.at(-1) === "") lines.pop(); // the newline that ends the line before
  const out = [];
  for (let i = lines.length - 1; i >= 0; i--) {
    if (!/^[ \t]*\/\//.test(lines[i])) break;
    out.unshift(lines[i].replace(/^[ \t]*/, ""));
  }
  return out.join("\n");
};
const rationales = new Map(SOURCES.map((rel) => [rel, rationaleOf(read(rel))]));
check(new Set(rationales.values()).size === 1,
  `the emphasis rule's explanation differs between copies: ${[...rationales.keys()].join(", ")}`);
for (const [rel, r] of rationales) {
  check(r && /CommonMark/.test(r) && /flank/i.test(r),
    `${rel}: the emphasis rule must keep its CommonMark/flanking rationale`);
}

// Every call site must go through the shared constant, not a fresh literal.
const appJs = read("web/playground-src/app.js");
const uses = (appJs.match(/\.replace\(MD_EMPHASIS,/g) || []).length;
check(uses === 3, `web/playground-src/app.js: expected 3 MD_EMPHASIS call sites (mdLite, mdPlain, markdownInline), found ${uses}`);

// ------------------------------------------------------------ the rule ------
// Everything below RUNS the rule, so it needs exactly one of them. When the
// copies have diverged there is no single rule to test — report that and stop,
// rather than testing an arbitrary one (or, worse, an empty pattern).
function report() {
  if (fail.length) {
    console.error("check_md_emphasis: FAIL");
    for (const f of fail) console.error("  ✗ " + f);
    console.log(JSON.stringify({ verdict: "FAIL", failures: fail.length }));
    process.exit(1);
  }
}
if (distinct.size !== 1) { fail.push("cannot test behaviour until the copies agree"); report(); }
const MD_EMPHASIS = new RegExp([...distinct][0].replace(/^\//, "").replace(/\/gu$/, ""), "gu");

// --------------------------------------------------------------- 2. corpus --
// The inline passes that run BEFORE emphasis, so the rule sees what it sees in
// production (links, **bold** and `code` already consumed).
const preEmphasis = (t) => String(t)
  .replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, "$1")
  .replace(/\*\*([^*\n]+)\*\*/g, "$1")
  .replace(/`([^`\n]+)`/g, "$1");
const strip = (t) => { MD_EMPHASIS.lastIndex = 0; return preEmphasis(t).replace(MD_EMPHASIS, "$1$2"); };

// Regression pins: the six shipped strings the un-flanked rule corrupted. Each
// asterisk here is literal notation and must survive verbatim.
const MUST_SURVIVE = [
  ["wikidata-xxl", "rdf:type / wdt:P31, and truthy wdt:* statements (occupation P106, etc.); IRIs stay wikidata.org/{entity,prop/direct}/* so nodes round-trip"],
  ["bne", "Entities at datos.bne.es/resource/* over the BNE's own ontology (datos.bne.es/def/C* classes, P* properties) plus the RDA registry"],
  ["biblissima", "entities at data.biblissima.fr/entity/Q*, truthy statements via prop/direct/P*, P2 = 'instance of'"],
  ["memoria", "mc:bornInProvince / mc:residedIn*), cause/manner of death (mc:cause)"],
  ["orcid:4 tip", "which file answers its predicate (orc:* → orcid, ror:* → ror), so keep the org columns"],
  ["ror:4 tip", "which file answers its predicate (ror:* here, orc:* there); the bound org keeps"],
];
for (const [name, text] of MUST_SURVIVE) {
  const got = strip(text);
  check(got === text, `${name}: emphasis rule still eats literal asterisks\n      in  ${text}\n      out ${got}`);
}

// …and the emphasis that MUST still work, in the shapes the catalog uses.
const MUST_ITALICISE = [
  ["mid-sentence", "edges typed by *how* a model derives", "edges typed by how a model derives"],
  ["multi-word", "the tallest champion is a *Sequoiadendron giganteum* at 46 m", "the tallest champion is a Sequoiadendron giganteum at 46 m"],
  ["clause with comma", "ask both *what was argued* and *who said it, and how*.", "ask both what was argued and who said it, and how."],
  ["start of string", "*verbatim* and a link layer is *added*", "verbatim and a link layer is added"],
  ["after punctuation", "(*near* in meaning)", "(near in meaning)"],
  ["hyphenated", "stored as date-*times* where the shapes ask", "stored as date-times where the shapes ask"],
];
for (const [name, text, want] of MUST_ITALICISE) {
  const got = strip(text);
  check(got === want, `${name}: legitimate italics were lost\n      in   ${text}\n      want ${want}\n      got  ${got}`);
}

// ----------------------------------------------------- 3. live corpus sweep --
// Load every description / label / tip this repo ships and assert two things of
// each span the rule forms: it is genuinely flanked (independent check, below),
// and it never spans a newline.
function loadCorpus() {
  const out = [];
  const add = (origin, text) => { if (typeof text === "string" && text.includes("*")) out.push({ origin, text }); };

  const w = {};
  new Function("window", read("web/playground-src/catalog.js"))(w);
  const cat = w.RETE_PLAYGROUND_CATALOG || {};
  for (const d of cat.datasets || []) { add(`datasets[${d.key}].label`, d.label); add(`datasets[${d.key}].description`, d.description); }
  for (const [k, m] of Object.entries(cat.datasetMeta || {})) add(`datasetMeta[${k}].license`, m.license);
  for (const [k, exs] of Object.entries(cat.examples || {})) exs.forEach((e, i) => {
    add(`examples[${k}][${i}].label`, e.label); add(`examples[${k}][${i}].tip`, e.tip); add(`examples[${k}][${i}].reason`, e.reason);
  });

  const cardsDir = path.join(ROOT, "web/playground-cards");
  if (fs.existsSync(cardsDir)) for (const f of fs.readdirSync(cardsDir).filter((n) => n.endsWith(".json"))) {
    const j = JSON.parse(read(`web/playground-cards/${f}`));
    add(`${f}.description`, j.description); add(`${f}.title`, j.title);
    (j.example_queries || []).forEach((q, i) => { add(`${f}.q[${i}].label`, q.label); add(`${f}.q[${i}].tip`, q.tip); });
  }

  const relay = "clients/relay/catalog.json";
  if (fs.existsSync(path.join(ROOT, relay))) {
    const j = JSON.parse(read(relay));
    const walk = (n, t) => {
      if (Array.isArray(n)) return n.forEach((v, i) => walk(v, `${t}[${i}]`));
      if (n && typeof n === "object") for (const [k, v] of Object.entries(n)) {
        if (typeof v === "string" && /^(label|description|tip|blurb|license|title|reason)$/.test(k)) add(`relay${t}.${k}`, v);
        else walk(v, `${t}.${k}`);
      }
    };
    walk(j, "");
  }
  return out;
}

// CommonMark §6.2, read straight off the spec — deliberately NOT the regex, so a
// typo in the regex cannot satisfy this by construction.
const PUNCT = /[\p{P}\p{S}]/u;
const SPACE = /\s/;
const cls = (ch) => (ch === "" ? "space" : SPACE.test(ch) ? "space" : PUNCT.test(ch) ? "punct" : "word");
const leftFlanking = (before, after) =>
  cls(after) !== "space" && (cls(after) !== "punct" || cls(before) !== "word");
const rightFlanking = (before, after) =>
  cls(before) !== "space" && (cls(before) !== "punct" || cls(after) !== "word");

let spans = 0, strings = 0;
for (const { origin, text } of loadCorpus()) {
  const s = preEmphasis(text);
  strings++;
  MD_EMPHASIS.lastIndex = 0;
  let m;
  while ((m = MD_EMPHASIS.exec(s))) {
    spans++;
    const open = m.index + m[1].length;          // index of the opening "*"
    const close = open + 1 + m[2].length;        // index of the closing "*"
    check(leftFlanking(s.charAt(open - 1) || "", s.charAt(open + 1)),
      `${origin}: opening "*" is not left-flanking — «${m[0].slice(0, 60)}»`);
    check(rightFlanking(s.charAt(close - 1), s.charAt(close + 1) || ""),
      `${origin}: closing "*" is not right-flanking — «${m[0].slice(0, 60)}»`);
    check(!m[2].includes("\n"), `${origin}: emphasis spans a newline — «${m[0].slice(0, 60)}»`);
  }
}
check(spans > 0, "the corpus sweep found no emphasis at all — the loader is probably broken");

report();
console.log(JSON.stringify({
  verdict: "PASS", copies: literals.size, strings, spans,
  pinned: MUST_SURVIVE.length + MUST_ITALICISE.length,
}));
