#!/usr/bin/env node
/* Linear A corpus → N-Triples for `rete build`.
 *
 * Source: github.com/mwenge/lineara.xyz  (the LinearA Explorer), file
 * LinearAInscriptions.js — `var inscriptions = new Map([...])` keyed by GORILA
 * site-code (HT1, ZA8, ARKH1a…). The corpus text derives from the transcriptions
 * of Louis Godart & Jean-Pierre Olivier (GORILA, École Française d'Athènes, 1976–)
 * and the tabulation of George Douros; compiled by mwenge. No explicit license is
 * stated on the repo; inscription *images* are © École Française d'Athènes and are
 * NOT included here — this is the scholarly text/metadata layer only, built as a
 * research/educational derivative with full attribution.
 *
 * We model a graph that is genuinely useful for Linear A analysis (sign/word
 * sequences shared across documents) and explorable as a network:
 *   Inscription ──site/scribe/support/context──> (facets)
 *   Inscription ──word──> Word ──sign──> Sign        (Inscription ──sign──> Sign too)
 * plus transcription (Linear A Unicode) + transliteration (Latin) literals and the
 * administrative numerals.
 *
 * Usage (in the dev container, has node):
 *   node scripts/lineara_to_nt.js data/lineara/repo/LinearAInscriptions.js > data/lineara/lineara.nt
 */
const fs = require("fs");

const inPath = process.argv[2] || "data/lineara/repo/LinearAInscriptions.js";
const code = fs.readFileSync(inPath, "utf8");
// The file is `var inscriptions = new Map([...]);` — evaluate it in a tiny sandbox
// and hand back the Map. Avoids every JSON-vs-JS-literal / astral-Unicode pitfall.
const inscriptions = new Function("Map", code + "\n;return inscriptions;")(Map);

const BASE = "https://lineara.xyz/";
const P = BASE + "prop/";
const C = BASE + "class/";
const RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const LBL = "http://www.w3.org/2000/01/rdf-schema#label";

const out = [];
const seen = new Set();              // de-dupe type+label triples for shared nodes
const iri = s => "<" + s + ">";
function localName(s){ return encodeURIComponent(String(s)).replace(/[()'!*]/g, c => "%" + c.charCodeAt(0).toString(16).toUpperCase()); }
function lit(s){
  return '"' + String(s)
    .replace(/\\/g, "\\\\").replace(/"/g, '\\"')
    .replace(/\n/g, "\\n").replace(/\r/g, "\\r").replace(/\t/g, "\\t") + '"';
}
function t(s, p, o){ out.push(iri(s) + " " + iri(p) + " " + o + " ."); }
function node(id, cls, label){            // a shared facet node (site/scribe/word/sign…)
  if(seen.has(id)) return;
  seen.add(id);
  t(id, RDF, iri(C + cls));
  if(label !== undefined && label !== null && String(label).length) t(id, LBL, lit(label));
}

const DIVIDER = "\u{10101}";              // 𐄁 Linear A word divider
const isNewline = w => w === "\n" || w.trim() === "";
const isDivider = w => w === DIVIDER || w === "|";
const isNumeral = w => /^[0-9]+([./][0-9]+)?$/.test(w);

let nIns = 0, nWord = 0, nSign = 0;
for (const [codeKey, v] of inscriptions) {
  // Skip the appended sign-decomposition entries (their value is an Array).
  if (!v || typeof v !== "object" || Array.isArray(v) || !v.name) continue;
  nIns++;
  const sid = BASE + "inscription/" + localName(v.name);
  t(sid, RDF, iri(C + "Inscription"));
  t(sid, LBL, lit(v.name));

  // --- categorical facets (each becomes a typed, labelled node) ---
  const facet = (val, cls, prop, prefix) => {
    if (!val || !String(val).trim()) return;
    const id = BASE + prefix + "/" + localName(val);
    node(id, cls, val);
    t(sid, P + prop, iri(id));
  };
  facet(v.site,    "Site",    "site",    "site");
  facet(v.scribe,  "Scribe",  "scribe",  "scribe");
  facet(v.support, "Support", "support", "support");
  facet(v.context, "Period",  "context", "period");   // Minoan period, e.g. LMIB

  if (v.findspot && String(v.findspot).trim()) t(sid, P + "findspot", lit(v.findspot));
  if (v.imageRights) t(sid, P + "imageRights", lit(v.imageRights));
  if (v.transcription) t(sid, P + "transcription", lit(String(v.transcription).replace(/\n/g, " ").trim()));

  // --- transliteration: words → signs (the analysable sequence layer) ---
  const tw = Array.isArray(v.transliteratedWords) ? v.transliteratedWords : [];
  const translit = [];
  for (const raw of tw) {
    const w = String(raw);
    if (isNewline(w) || isDivider(w)) continue;
    if (isNumeral(w)) { t(sid, P + "numeral", lit(w)); continue; }
    translit.push(w);
    const wid = BASE + "word/" + localName(w);
    if (!seen.has(wid)) nWord++;
    node(wid, "Word", w);
    t(sid, P + "word", iri(wid));
    // a "word" is a sequence of signs joined by '-': QE-RA2-U → QE, RA2, U
    for (const sg of w.split("-")) {
      const s = sg.trim();
      if (!s || isNumeral(s)) continue;
      const sigid = BASE + "sign/" + localName(s);
      if (!seen.has(sigid)) nSign++;
      node(sigid, "Sign", s);
      t(wid,  P + "sign", iri(sigid));   // word → sign (sign inventory of a word)
      t(sid,  P + "sign", iri(sigid));   // inscription → sign (sign × document network)
    }
  }
  if (translit.length) t(sid, P + "transliteration", lit(translit.join(" ")));
}

process.stderr.write(`lineara: ${nIns} inscriptions, ${nWord} distinct words, ${nSign} distinct signs, ${out.length} triples\n`);
process.stdout.write(out.join("\n") + "\n");
