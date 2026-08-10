// G0 — a shipped catalog example must not be recorded as answering nothing.
//
// THE HOLE THIS CLOSES. Two mechanisms already asserted "a curated example must
// produce data", and between them they left the whole remote catalog uncovered:
//
//   * `check_catalog_examples.mjs` runs every example in a real browser and
//     fails a clean "0 row(s)" — but its default scope is `embedded`. The
//     ~60 `remote-lazy` datasets (the multi-gigabyte ones on R2) are skipped,
//     because sweeping them live would cost hours and a hundred network
//     round-trips per gate run.
//   * PR #176 made `rete build` refuse to ship a CARD starter query it measured
//     at zero rows — but a card's starter queries and the playground catalog's
//     examples are different surfaces with different authors.
//
// The remote examples ARE measured: `scripts/preview/capture.mjs` runs every one
// of them in Chromium against the live file and writes the result to
// `web/preview/answers.json`, which is committed. Nothing read it. So nine
// examples sat in that file recorded as `"ok": false, "count": 0` across
// several releases — six of `gharchive-2026-06`, two of `gharchive` and one of
// `wikidata-xxl` — all of them starter queries a visitor clicks first. The
// measurement existed, was committed, and rotted unread.
//
// This check reads it. Offline, by construction: the committed JSON and the
// committed catalog, no network — the gate must not need ~100 range requests to
// go green. The live half stays where it belongs (`run.sh capture`, and
// `check_catalog_examples.mjs --scope=all` when someone asks for it).
//
// THE CONTRACT — three ways an answer can be nothing, and one way it cannot
// claim to be:
//
//   1. count === 0                          ⇒ nothing
//   2. one row whose every cell is 0        ⇒ nothing
//   3. a drawing view that drew 0           ⇒ nothing
//   …each allowed only if the example carries `allowEmpty: true`, and
//   4. allowEmpty                           ⇒ the answer must really be nothing
//
// (2) exists because a row count alone is not enough. `SELECT (COUNT(*) AS ?n)`
// with no GROUP BY returns exactly one row no matter what, so an aggregate that
// finds nothing records `count: 1, ok: true` and sails past (1). Not
// hypothetical: `gharchive-2026-06:0` ("The cut, in one number: PyTorch's June
// 2026 stars") shipped the answer 0 for months while its own tip and the dataset
// description said 39 — the tenth defect of this batch, and the only one a row
// count could not see.
//
// (3) exists because 39 examples are `graph` / `map` / `time` views with no row
// count at all — they record `count: null` and a qmeta summary ("graph: 34
// nodes, 17 edges", "67 mapped feature(s)", "112 dated item(s) · 1835–2026").
// Skipping every null count would leave uncovered precisely the examples where
// no number is visible to a reader. See DRAWING_SHAPES for how narrowly it is
// read, and why the obvious looser reading is wrong.
//
// (4) is what keeps the flag from becoming a mute button: an example that
// answers cannot claim it is legitimately empty, or a stale opt-out hides the
// next real zero. `allowEmpty` exists for a query whose POINT is the empty
// answer — a SHACL "no violations" probe — and `capture.mjs` already honours it
// (`ok = !(count === 0 && !allowEmpty)`), so a correctly-flagged example records
// `ok: true` with `count: 0` and passes here.
//
// Each rule was checked for noise against the committed file before being added:
// of 552 counted and 39 drawn answers, (1) matches 8, (2) matches exactly one —
// the defect — and (3) matches none.
//
// WHAT USED TO BE OUT OF SCOPE, AND WHY IT NO LONGER IS (#212). Entries that
// are `count: null` AND not `ok` were counted as `unmeasured` and waved through
// — "a capture-infrastructure problem, not a claim that the example is empty".
// Fifty-two of 644 sat there, and the exemption is what let them sit: a query
// that HANGS and a query that RETURNS NOTHING are different failures, and only
// the second was guarded, so the first was invisible for as long as anyone cared
// to leave it. Thirty-nine of the fifty-two were not even a dataset problem —
// `capture.mjs` waited for the example library to render exactly as many buttons
// as the run happened to be MEASURING, which on any resume is fewer than the
// page draws, so the wait could not succeed and the whole dataset was recorded
// as "dataset open failed: page.waitForFunction: Timeout … exceeded". The
// exemption then swallowed the evidence. Underneath it were ordinary zero-row
// defects of exactly the class (1) already catches.
//
// So the contract is now: a shipped catalog example must have a RECORDED,
// SUCCESSFUL answer. Three ways to satisfy it and no fourth —
//
//   A. `ok: true` with something in it (the rules above), or
//   B. `ok: true, count: 0` with `allowEmpty: true` (the empty answer IS the
//      point — a SHACL "no violations" probe), or
//   C. `skipCapture: "<why>"` on the example, for one the sweep must not run at
//      all. The flag's VALUE is the justification, so it cannot be set without
//      recording one, and like `allowEmpty` it is not a mute button: an example
//      that carries it and nevertheless has a good recorded answer fails, so a
//      stale exclusion cannot outlive its reason.
//
// An example the catalog ships with no entry in answers.json at all fails too.
// That hole had a name before it had a check: `wikiart:0` FILTERs an artist
// across the whole `wa:imageData` predicate — 223,082 inline WebP literals on a
// 25.4 GB graph — and had not returned after twenty minutes. It appeared in NO
// count of broken examples, including the one this file's own payload printed,
// precisely because no capture ever survived long enough to write a record. The
// worst example in the catalog was the one thing the measurement could not see.
//
// Assertions go through `_expect.mjs` (#186): the runner reads the last JSON
// verdict, never the exit code, so a check that throws reports a stack trace
// instead of naming the examples that answer nothing — which is the one fact
// this check exists to produce.
import fs from "node:fs";
import { expect } from "./_expect.mjs";

const t = expect("check_catalog_answers");
const ROOT = new URL("../../../", import.meta.url);

let catalog = null;
let answers = null;
try {
  const source = fs.readFileSync(new URL("web/playground-src/catalog.js", ROOT), "utf8");
  const window = {};
  new Function("window", source)(window);
  catalog = window.RETE_PLAYGROUND_CATALOG;
  if (!catalog || !catalog.examples) {
    throw new Error("catalog.js did not expose RETE_PLAYGROUND_CATALOG.examples");
  }
  answers = JSON.parse(fs.readFileSync(new URL("web/preview/answers.json", ROOT), "utf8")).answers;
  if (!answers || typeof answers !== "object") {
    throw new Error("web/preview/answers.json has no `answers` object");
  }
} catch (error) {
  t.threw("load the catalog and the captured answers", error);
  t.finish({
    measured: 0, drawings: 0, emptyWithoutOptOut: 0, zeroAggregates: 0,
    blankPictures: 0, allowEmpty: 0, unmeasured: 0, orphans: 0,
  });
  process.exit(1);
}

// `allowEmpty` and `skipCapture` live on the catalog example, not in
// answers.json — both are statements of intent by whoever wrote the query, and
// they belong beside it.
const flagged = new Map();
for (const [dataset, examples] of Object.entries(catalog.examples)) {
  examples.forEach((example, index) => {
    flagged.set(`${dataset}:${index}`, {
      allowEmpty: !!example.allowEmpty,
      // A string, never a boolean: the reason is the flag. Whitespace does not
      // count as a reason.
      skipCapture: String(example.skipCapture || "").trim(),
      label: example.label || `example ${index}`,
    });
  });
}

// "One row, all zeros" — the shape of an aggregate that counted nothing. Only a
// numeric zero counts; a legitimate "0" of text (a label, an id) would have
// siblings in the row that are not zero, and a row of nothing but zeros carries
// no information a reader could want.
function isAllZeroSingleRow(answer) {
  if (!Array.isArray(answer.rows) || answer.rows.length !== 1) return false;
  const cells = answer.rows[0];
  if (!Array.isArray(cells) || cells.length === 0) return false;
  return cells.every((cell) => /^0(?:\.0+)?$/.test(String((cell && cell.text) || "").trim()));
}

// A drawing view's answer is its qmeta summary — "graph: 34 nodes, 17 edges",
// "67 mapped feature(s)", "112 dated item(s) · 1835–2026". Only these three
// shapes are judged, and only the numbers they name, because only there is the
// number THE ANSWER. `geoadmin-tiles:0` proves why the loose reading is wrong:
// "vector tiles · 0 feature(s) highlighted" draws a whole tile map and the zero
// counts highlights, not content. An unrecognized summary is left alone.
//
// Everything after the first "|" is the transfer report, whose "0 B of 45.4 MB"
// would otherwise read as an answer of zero, so it never gets looked at.
const DRAWING_SHAPES = [
  /^graph:\s*(\d[\d,]*)\s+nodes?,\s*(\d[\d,]*)\s+edges?/i,
  /^(\d[\d,]*)\s+mapped\s+feature/i,
  /^(\d[\d,]*)\s+dated\s+item/i,
];

// The playground says in `#qmeta` when it STOPPED rather than answered — the tab
// hit its WebAssembly memory ceiling, the worker was cancelled, the reader was
// swapped mid-flight. `capture.mjs` now records those as `ok: false`, but records
// written before it did are still in the committed file carrying `ok: true`, and
// an entry is classified by what its qmeta SAYS, not by a flag that may predate
// the rule. Same list `check_catalog_examples.mjs` has always used.
const GAVE_UP = /cancelled|switched readers|browser'?s limit|browser limit/i;

function drewNothing(qmeta) {
  const summary = String(qmeta || "").split("|")[0].trim();
  for (const shape of DRAWING_SHAPES) {
    const m = shape.exec(summary);
    if (m) return m.slice(1).every((n) => Number(String(n).replace(/,/g, "")) === 0);
  }
  return false;
}

let measured = 0;
let drawings = 0;
let empty = 0;
let zeroAggregate = 0;
let blankPictures = 0;
let allowed = 0;
let neverAnswered = 0;
let unrecorded = 0;
let skipped = 0;
let orphans = 0;

// The head of a capture failure, for the report. `error` is the playground's own
// error box scraped verbatim — emoji, advice paragraph and a "Copy full log"
// button caption all run together — so keep the first words and drop the rest.
function why(answer) {
  const text = String(answer.error || "").replace(/\s+/g, " ").trim();
  if (text) return text.slice(0, 110);
  if (GAVE_UP.test(answer.qmeta || "")) return `gave up — ${String(answer.qmeta).split("|")[0].trim()}`;
  return "recorded ok: false with no error text";
}

for (const [id, answer] of Object.entries(answers)) {
  const example = flagged.get(id);
  // An id the catalog no longer defines: `finalize` prunes those, and one left
  // behind says nothing about whether a SHIPPED example answers. Counted, not
  // judged.
  if (!example) { orphans++; continue; }

  const counted = answer.count !== null && answer.count !== undefined;
  // Never answered at all: the capture threw, the engine errored, or the
  // playground said it gave up. Formerly the `unmeasured` exemption; see the
  // header for the fifty-two entries that lived in it.
  //
  // A record that DID come back with a row count is deliberately not routed
  // here even when it is `ok: false`, because `ok: false, count: 0` is the
  // zero-row class and rule (1) below says so in the words that repair it —
  // "0 row(s)", with the allowEmpty escape named. Reporting that as "never
  // answered" would be true and useless.
  if ((!counted && answer.ok !== true) || GAVE_UP.test(answer.qmeta || "")) {
    if (example.skipCapture) { skipped++; continue; }
    neverAnswered++;
    t.equal(id, `never answered — ${why(answer)} — ${example.label}`, "a recorded, successful answer",
      "a shipped catalog example is recorded as NOT answering — a hang and an engine error are"
        + " failures exactly as much as an empty result. Re-run it:"
        + " scripts/preview/run.sh capture --dataset=<key> --force ; then fix the query, or if it"
        + " genuinely cannot be swept set skipCapture: \"<why>\" on the example in"
        + " web/playground-src/catalog.js");
    continue;
  }
  if (counted) measured++; else drawings++;
  // The flag is not a mute button, in either direction: an example excluded from
  // the sweep must not also be sitting here with a perfectly good answer.
  if (example.skipCapture) {
    skipped++;
    t.equal(id, `skipCapture with an answer — ${example.label}`, "skipCapture only when there is no answer",
      "this example answers, so the exclusion is stale — drop skipCapture from web/playground-src/catalog.js");
  }
  if (example.allowEmpty) allowed++;

  // Answered nothing — in whichever way this view can say it.
  const noRows = counted && answer.count === 0;
  const zeroSum = counted && !noRows && isAllZeroSingleRow(answer);
  const blankPicture = !counted && drewNothing(answer.qmeta);
  const nothing = noRows || zeroSum || blankPicture;

  if (nothing && !example.allowEmpty) {
    // The failure line has to name the example and say what it measured, because
    // that is the whole repair instruction: go run this query and find out why.
    const seen = noRows ? "0 row(s)"
      : zeroSum ? "one row of nothing but 0"
        : `drew nothing — ${String(answer.qmeta || "").split("|")[0].trim()}`;
    const because = zeroSum
      ? " (SPARQL owes a bare COUNT its one row regardless, so the row count cannot see this)"
      : "";
    if (noRows) empty++; else if (zeroSum) zeroAggregate++; else blankPictures++;
    t.equal(id, `${seen} — ${example.label}`, "an answer with something in it",
      `a shipped catalog example was measured answering NOTHING${because} — fix the query so it answers,`
        + " or if the empty answer IS the point (a SHACL \"no violations\" check) set allowEmpty: true on"
        + " the example with a comment saying why, then re-run"
        + " scripts/preview/run.sh capture --dataset=<key> --force");
  } else if (!nothing && example.allowEmpty) {
    // The flag is not a mute button. An example that answers must not claim to
    // be legitimately empty, or the next real zero hides behind a stale opt-out.
    t.equal(id, `allowEmpty with an answer — ${example.label}`, "allowEmpty only when the answer is empty",
      "this example answers, so allowEmpty is stale — drop the flag from web/playground-src/catalog.js");
  }
  // `ok` is capture.mjs's own reading of the same rule; a hand-edited `ok: true`
  // over a zero count must not smuggle one past.
  if (noRows && answer.ok === true && !example.allowEmpty) {
    t.equal(`${id}.ok`, true, false, "answers.json marks a zero-row example ok — capture.mjs never writes that");
  }
  // A count came back, none of the "nothing" shapes matched, and the capture
  // still refused it — an error box drawn over a partial table, say. Rare, but
  // the whole point of #212 is that no `ok: false` may pass unread.
  if (!nothing && answer.ok !== true) {
    neverAnswered++;
    t.equal(id, `not ok despite ${answer.count} row(s) — ${why(answer)} — ${example.label}`,
      "a recorded, successful answer",
      "the capture recorded a result AND rejected it — re-run the example and read the error box:"
        + " scripts/preview/run.sh capture --dataset=<key> --force");
  }
}

// A shipped example with NO record at all. Nothing above can see it — every loop
// so far walks answers.json — which is exactly how the catalog's most expensive
// query stayed out of every count of broken examples (see the header).
for (const [id, example] of flagged) {
  if (id in answers) continue;
  if (example.skipCapture) { skipped++; continue; }
  unrecorded++;
  t.equal(id, `no recorded answer at all — ${example.label}`, "a recorded, successful answer",
    "a shipped catalog example has NO entry in web/preview/answers.json — an example that was never"
      + " measured is not a passing example, it is an unmeasured one. Capture it:"
      + " scripts/preview/run.sh capture --dataset=<key> ; or if it genuinely cannot be swept set"
      + " skipCapture: \"<why>\" on the example in web/playground-src/catalog.js");
}

t.finish({
  measured, drawings, emptyWithoutOptOut: empty, zeroAggregates: zeroAggregate,
  blankPictures, allowEmpty: allowed, neverAnswered, unrecorded, skipCapture: skipped, orphans,
});
