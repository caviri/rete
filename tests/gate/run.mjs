// The regression gate runner — executes inside the Playwright Docker image.
// Usage: node run.mjs [fast] [--local] [--only=<substr>] [--deployed]
//        node run.mjs --catalog=embedded|all [--catalog-dataset=<substr>]
//
// Tiers:
//   G0 static   — app.js/catalog.js parse, built page inline-scripts parse,
//                 catalog examples use only declared prefixes.
//   G1 node     — the PRODUCTION async wasm + Asyncify driver runs a real lazy
//                 query against a local range server (no browser).
//   G2 browser  — the load-mode × wasm-variant × device matrix in the selected
//                 Playwright browser (Chromium by default).
//   (--deployed adds a live GitHub Pages lazy check — informational, it can lag
//    a push by a minute; it does not flip the exit code.)
import { spawn, execSync } from "node:child_process";
import fs from "node:fs";
import vm from "node:vm";

const ROOT = "/work";
const args = process.argv.slice(2);
const FAST = args.includes("fast");
const LOCAL_ONLY = args.includes("--local");
const only = (args.find((a) => a.startsWith("--only=")) || "").slice(7);
const DEPLOYED = args.includes("--deployed");
const CATALOG_SCOPE = (args.find((a) => a.startsWith("--catalog=")) || "").slice(10);
const CATALOG_DATASET = (args.find((a) => a.startsWith("--catalog-dataset=")) || "").slice(18);
const results = [];
const t0 = Date.now();

function record(tier, name, ok, note = "") {
  results.push({ tier, name, ok, note });
  console.log(`${ok ? "  ✓" : "  ✗ FAIL"} [${tier}] ${name}${note ? " — " + note : ""}`);
}

// ---------- G0 static ----------
function g0() {
  // FIRST: are the .rete fixtures the ones the recipe produced? Every tier
  // below reads them, and a substituted one used to surface as a failure in
  // whichever check happened to notice — check_card_modal blaming the
  // playground for a downloaded file. Ask the question where the answer names
  // the fixture.
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_fixture_provenance.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    const ok = verdict && verdict.verdict === "PASS";
    record(
      "G0",
      "gate fixtures match their recipe (tests/gate/fixtures/manifest.json)",
      ok,
      ok ? `${verdict.fixtures} fixtures, built by ${(verdict.builders || []).join(", ") || "(unknown)"}` : out.slice(-300),
    );
  } catch (e) {
    // stderr first: _expect.mjs puts the compact one-line summary there, and it
    // is the line that names the fixture and the command that repairs it.
    record("G0", "gate fixtures match their recipe (tests/gate/fixtures/manifest.json)", false,
      String(e.stderr || e.stdout || e).slice(-300));
  }
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/test_catalog_matrix.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    const ok = verdict && verdict.verdict === "PASS";
    record(
      "G0",
      // Count from the run, not from a literal: a hard-coded label goes stale
      // the first time someone adds an example, and then says the wrong number
      // on every green run.
      `catalog exhaustive matrix${ok ? ` (${verdict.allQueries} queries)` : ""}`,
      ok,
      ok ? "" : out.slice(-160),
    );
  } catch (e) {
    record("G0", "catalog exhaustive matrix", false, String(e.stderr || e.stdout || e).slice(-160));
  }
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_wasm_api.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    record(
      "G0",
      "generated WASM API contract",
      verdict && verdict.verdict === "PASS",
      verdict && verdict.verdict === "PASS" ? "" : out.slice(-160),
    );
  } catch (e) {
    record(
      "G0",
      "generated WASM API contract",
      false,
      String(e.stderr || e.stdout || e).slice(-160),
    );
  }
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_wasm_boot.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    record(
      "G0",
      "web and no-modules WASM boot",
      verdict && verdict.verdict === "PASS",
      verdict && verdict.verdict === "PASS" ? "" : out.slice(-160),
    );
  } catch (e) {
    record(
      "G0",
      "web and no-modules WASM boot",
      false,
      String(e.stderr || e.stdout || e).slice(-160),
    );
  }
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_yasgui_wasm_parity.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    record(
      "G0",
      "yasgui embeds the canonical WASM glue",
      verdict && verdict.verdict === "PASS",
      verdict && verdict.verdict === "PASS" ? "" : out.slice(-200),
    );
  } catch (e) {
    record(
      "G0",
      "yasgui embeds the canonical WASM glue",
      false,
      String(e.stderr || e.stdout || e).slice(-200),
    );
  }

  // The asyncify glue must normalize every wasm pointer it dereferences: above
  // 2 GiB an `i32` import arrives sign-extended and `mem.set` throws. The browser
  // matrix cannot see this — its async check runs a small dataset, and the bug
  // needs a heap past 2 GiB (~150 MB of range reads). Assert the property instead.
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_async_pointer_safety.mjs`, {
      encoding: "utf8",
    });
    record("G0", "async glue pointer safety (>2 GiB heaps)", /\[G0\]/.test(out), out.slice(-160));
  } catch (e) {
    record(
      "G0",
      "async glue pointer safety (>2 GiB heaps)",
      false,
      String(e.stderr || e.stdout || e).slice(-160),
    );
  }
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/test_url_normalize.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    record(
      "G0",
      "remote URL normalization and scheme refusal",
      verdict && verdict.verdict === "PASS",
      verdict && verdict.verdict === "PASS" ? `${verdict.checks} cases` : out.slice(-160),
    );
  } catch (e) {
    record(
      "G0",
      "remote URL normalization and scheme refusal",
      false,
      String(e.stderr || e.stdout || e).slice(-160),
    );
  }
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_social_previews.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    record(
      "G0",
      "social previews (share pages + og:image)",
      verdict && verdict.verdict === "PASS",
      verdict && verdict.verdict === "PASS"
        ? `${verdict.sharePages} share pages, ${verdict.pagesWithPreview} pages with a card`
        : out.slice(-200),
    );
  } catch (e) {
    record("G0", "social previews (share pages + og:image)", false,
      String(e.stderr || e.stdout || e).slice(-200));
  }
  // The Markdown emphasis rule has six call sites across five files — app.js
  // holds three, and two of the files are generated. Nothing can import it into
  // all of them, so assert the copies are byte-identical and that the shipped
  // descriptions still render right: an un-flanked rule silently ate literal
  // asterisks out of six live cards.
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_md_emphasis.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    const ok = verdict && verdict.verdict === "PASS";
    record(
      "G0",
      "markdown emphasis rule (identical copies + flanking)",
      ok,
      ok ? `${verdict.copies} copies, ${verdict.spans} spans over ${verdict.strings} strings` : out.slice(-240),
    );
  } catch (e) {
    record("G0", "markdown emphasis rule (identical copies + flanking)", false,
      String(e.stderr || e.stdout || e).slice(-240));
  }
  for (const f of ["web/playground-src/app.js", "web/playground-src/catalog.js", "web/playground-src/versions.js"]) {
    try { execSync(`node --check ${ROOT}/${f}`, { stdio: "pipe" }); record("G0", `parse ${f}`, true); }
    catch (e) { record("G0", `parse ${f}`, false, String(e.stderr || e).slice(0, 120)); }
  }
  try {
    const html = fs.readFileSync(`${ROOT}/docs/playground.html`, "utf8");
    // Match <script …>…</script> keeping the open-tag attrs so we can skip
    // non-classic types: type="module" (ESM — new vm.Script would reject
    // import/export), and data islands (application/json, text/html templates).
    const re = /<script((?![^>]*\bsrc=)[^>]*)>([\s\S]*?)<\/script>/g;
    let m, n = 0, bad = 0, msg = "";
    while ((m = re.exec(html))) {
      const attrs = m[1] || "", body = m[2];
      const type = (attrs.match(/\btype\s*=\s*["']([^"']+)["']/) || [])[1];
      if (type && !/^(text|application)\/(java|ecma)script$/i.test(type)) continue; // module / json / template
      if (!body.trim()) continue;
      n++;
      try { new vm.Script(body); } catch (e) { bad++; msg = e.message; }
    }
    record("G0", `playground.html inline scripts (${n})`, bad === 0, bad ? msg.slice(0, 120) : "");
  } catch (e) { record("G0", "playground.html inline scripts", false, String(e).slice(0, 120)); }
  // The async wasm variant is rebuilt SEPARATELY (build_playground_async.sh);
  // build_playground.py only COPIES it — so an engine change can leave it stale
  // while the sync wasm is fresh, and G1/G2 would pass on the old binary. Flag it
  // if the async source wasm is older than the sync one.
  try {
    const syncW = `${ROOT}/web/pkg-nomodules/rete_wasm_bg.wasm`;
    const asyncW = `${ROOT}/web/pkg-nomodules-async/rete_wasm_bg.wasm`;
    if (fs.existsSync(syncW) && fs.existsSync(asyncW)) {
      const st = (p) => fs.statSync(p).mtimeMs;
      const fresh = st(asyncW) >= st(syncW) - 5000; // 5 s tolerance
      record("G0", "async wasm not older than sync wasm", fresh,
        fresh ? "" : "async variant STALE — run scripts/build_playground_async.sh");
    } else {
      record("G0", "async wasm present", fs.existsSync(asyncW), fs.existsSync(asyncW) ? "" : "web/pkg-nomodules-async missing");
    }
  } catch (e) { record("G0", "async wasm freshness", false, String(e).slice(0, 120)); }
  // Catalog example queries: every prefixed name must be PREFIX-declared.
  try {
    const src = fs.readFileSync(`${ROOT}/web/playground-src/catalog.js`, "utf8");
    const w = {}; new Function("window", src)(w);
    const ex = (w.RETE_PLAYGROUND_CATALOG || {}).examples || {};
    let checked = 0; const bad = [];
    for (const ds of Object.keys(ex)) (ex[ds] || []).forEach((e, i) => {
      if (!e || !e.q) return; checked++;
      let s = e.q.replace(/<[^>]*>/g, " ").replace(/"[^"]*"/g, '""').replace(/'[^']*'/g, "''");
      const declared = new Set(); let m2; const pre = /PREFIX\s+([A-Za-z][\w.\-]*)?:/gi;
      while ((m2 = pre.exec(s))) declared.add((m2[1] || "").toLowerCase()); // "" = default prefix
      // A prefixed name is <prefix>:<local> where prefix may be EMPTY (the default
      // prefix, `:local`). The delimiter class must include {, [, =, >, } etc. — the
      // old scanner missed `{br:s`, `?o=ns:x`, and the whole empty-prefix case.
      const used = new Set(); const u = /(?:^|[\s(){}\[\]^,;.|\/*!=><+])([A-Za-z][\w.\-]*)?:[A-Za-z0-9_%]/g;
      while ((m2 = u.exec(s))) used.add((m2[1] || "").toLowerCase());
      const missing = [...used].filter((p) => !declared.has(p));
      if (missing.length) bad.push(`${ds}#${i}:${missing.join(",")}`);
    });
    record("G0", `catalog examples declared prefixes (${checked})`, bad.length === 0, bad.slice(0, 3).join(" "));
  } catch (e) { record("G0", "catalog examples declared prefixes", false, String(e).slice(0, 120)); }
  // A full-text index is OPT-IN at build time, so the catalog's prose and the
  // file's sections drift silently (CONTAINS still answers — by full scan).
  // `boe` and `memoria` advertised an index their published files never had.
  // Offline half: the `textIndex:` declaration and the prose must agree.
  // Network half (flag vs the section actually served): check_dataset_catalog.py.
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_text_index_claims.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    const ok = verdict && verdict.verdict === "PASS";
    record(
      "G0",
      "full-text index claims match the catalog's declaration",
      ok,
      ok ? `${verdict.declaringTextIndex} declare one, ${verdict.surfacesScanned} surfaces` : out.slice(-240),
    );
  } catch (e) {
    record("G0", "full-text index claims match the catalog's declaration", false,
      String(e.stdout || e.stderr || e).slice(-240));
  }
  // The live example sweep (check_catalog_examples) defaults to scope=embedded,
  // so the ~60 remote-lazy datasets are never asserted here — their answers are
  // measured by scripts/preview/capture.mjs and committed to answers.json, and
  // until now nothing read that file. Nine examples sat recorded at 0 rows.
  // Offline by construction: committed JSON + committed catalog, no network.
  //
  // Since #212 the same check also reads the OTHER two ways an example can fail
  // to answer, both of which used to be exempt: a record that came back `ok:
  // false` (a hang, an engine error, a capture that threw) and an example with
  // no record at all. Fifty-two entries lived in the first exemption and the
  // catalog's most expensive query lived in the second.
  try {
    const out = execSync(`node ${ROOT}/tests/gate/checks/check_catalog_answers.mjs`, {
      encoding: "utf8",
    });
    const verdict = lastJson(out);
    const ok = verdict && verdict.verdict === "PASS";
    record(
      "G0",
      "every catalog example has a recorded answer with something in it",
      ok,
      ok
        ? `${verdict.measured} counted + ${verdict.drawings} drawn, ${verdict.allowEmpty} allowEmpty, `
          + `${verdict.skipCapture} skipCapture`
        : out.slice(-240),
    );
  } catch (e) {
    record("G0", "every catalog example has a recorded answer with something in it", false,
      String(e.stdout || e.stderr || e).slice(-240));
  }
}

// ---------- servers ----------
function serve(dir) {
  return new Promise((resolve, reject) => {
    const p = spawn("node", [`${ROOT}/tests/gate/serve.mjs`, dir, "0"], { stdio: ["ignore", "pipe", "pipe"] });
    let output = "", settled = false;
    const timeout = setTimeout(() => {
      if (settled) return;
      settled = true; p.kill("SIGKILL"); reject(new Error(`gate server did not start: ${output.slice(-200)}`));
    }, 10000);
    const onData = (data) => {
      output += data;
      const match = /http:\/\/127\.0\.0\.1:(\d+)/.exec(output);
      if (!settled && match) {
        settled = true; clearTimeout(timeout); resolve({ process: p, port: Number(match[1]) });
      }
    };
    p.stdout.on("data", onData); p.stderr.on("data", onData);
    p.on("exit", (code) => {
      if (settled) return;
      settled = true; clearTimeout(timeout); reject(new Error(`gate server exited ${code}: ${output.slice(-200)}`));
    });
  });
}

// ---------- child runner ----------
// cwd matters: browser checks run from tests/gate so ESM resolves the locally
// installed `playwright`; the node harness runs from ROOT (it reads docs/… paths).
function runChild(cmd, argv, env, timeoutMs, cwd = `${ROOT}/tests/gate`, stream = false) {
  return new Promise((res) => {
    const p = spawn(cmd, argv, { cwd, env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    p.stdout.on("data", (d) => { out += d; if (stream) process.stdout.write(d); });
    p.stderr.on("data", (d) => { out += d; if (stream) process.stderr.write(d); });
    const t = setTimeout(() => { p.kill("SIGKILL"); res({ code: 124, out: out + "\n[TIMEOUT]" }); }, timeoutMs);
    p.on("exit", (code) => { clearTimeout(t); res({ code, out }); });
  });
}
// Extract a check's JSON verdict robustly. The old greedy /\{[\s\S]*\}/ grabbed
// from the FIRST { (often a Node "(node:12) Warning … { code:'X' }" on stderr) to
// the LAST }, so any brace-bearing warning made it return null → false RED. This
// scans every top-level {…} (string-aware, so braces inside "…" don't miscount)
// and returns the LAST one that JSON.parses — the check's real result object.
function lastJson(out) {
  let best = null;
  for (let i = 0; i < out.length; i++) {
    if (out[i] !== "{") continue;
    let depth = 0, inStr = false, esc = false, j = i;
    for (; j < out.length; j++) {
      const c = out[j];
      if (inStr) { if (esc) esc = false; else if (c === "\\") esc = true; else if (c === '"') inStr = false; }
      else if (c === '"') inStr = true;
      else if (c === "{") depth++;
      else if (c === "}") { depth--; if (depth === 0) break; }
    }
    if (depth === 0 && j < out.length) {
      try { best = JSON.parse(out.slice(i, j + 1)); } catch (e) { /* not JSON */ }
      i = j; // skip the whole object so its NESTED { aren't treated as new starts
    }              // (else a nested {fields:{…}} would win over the outer verdict)
  }
  return best;
}

// ---------- G1 node async harness ----------
async function g1(port) {
  const fixture = `${ROOT}/tests/gate/.cache/worldcup2026.rete`;
  if (!fs.existsSync(fixture)) { record("G1", "async wasm harness", false, "fixture missing — run `bash tests/gate/fixtures.sh`"); return; }
  const q = [
    "PREFIX wc: <https://w3id.org/rete/worldcup#>",
    "PREFIX sc: <http://schema.org/>",
    "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>",
    "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>",
    "SELECT ?num ?pos ?player ?club ?dob WHERE {",
    "  <https://w3id.org/rete/worldcup/2026/team/Argentina> wc:squadPlayer ?p .",
    "  ?p sc:name ?player .",
    "  OPTIONAL { ?p wc:shirtNumber ?num }",
    "  OPTIONAL { ?p wc:position ?pos }",
    "  OPTIONAL { ?p sc:birthDate ?dob }",
    "  OPTIONAL { ?p wc:clubAtTournament ?c . ?c rdfs:label ?club }",
    "} ORDER BY xsd:integer(?num)",
  ].join("\n");
  const r = await runChild("node", [`${ROOT}/tests/gate/asyncify_e2e.cjs`],
    { RETE_URL: `http://127.0.0.1:${port}/worldcup2026.rete`, RETE_Q: q }, 60000, ROOT);
  const rows = (r.out.match(/rows=(\d+)/) || [])[1];
  record("G1", "async wasm + Asyncify driver (lazy, 4×OPTIONAL + ORDER BY cast)", r.code === 0 && Number(rows) > 0,
    r.code === 0 ? `${rows} rows` : r.out.split("\n").filter(Boolean).slice(-2).join(" | ").slice(0, 160));

  // Length probe must retry a cold first request (the "could not determine length"
  // first-load bug), tested against the real shipped __reteDoLen.
  const lp = await runChild("node", [`${ROOT}/tests/gate/checks/check_lenprobe.mjs`], { GLUE: `${ROOT}/docs/rete_wasm_async.js` }, 20000, ROOT);
  const lj = lastJson(lp.out);
  record("G1", "async length probe retries a cold first request", lp.code === 0 && lj && lj.verdict === "PASS",
    lp.code === 0 ? "" : (lj ? JSON.stringify(lj.checks || lj).slice(0, 160) : lp.out.slice(-160)));
}

// ---------- G2 browser matrix ----------
const G2 = [
  ["check_rich_media_cells", "rich media renderers + desktop card carousel", 120000, false],
  ["check_version_picker", "production + automatic PR preview selector", 90000, false],
  ["check_diag", "embedded + error diagnostics block", 90000, false],
  ["check_worldcup", "desktop lazy DEFAULT (async) · worldcup ex=0 · live R2", 120000, true],
  ["check_lazy_async", "desktop lazy async-forced · mtg GROUP BY · live R2", 240000, true],
  ["check_sync_read", "desktop lazy SYNC-forced · worldcup squad · live R2", 120000, true],
  ["check_ios_default", "iPhone UA → auto sync routing + query runs", 120000, true],
  ["check_settings_mobile", "phone-viewport Settings (no overflow, storage, session)", 120000, false],
  ["check_copy", "clipboard: parse-error Copy-log + share button", 90000, false],
  ["check_url_param", "#url= opens an off-catalog .rete; javascript: refused", 120000, false],
  ["check_load_modal", "Load pre-modal: drop/URL/examples routes; URL route end to end; phone width", 150000, false],
  ["check_default_graph_hint", "empty-default-graph explainer (resident + carded remote); absent on ordinary files", 180000, false],
  ["check_card_examples", "the file's OWN card queries populate the examples panel (off-catalog + local + catalog supplement; deduped; zero-row kept)", 240000, false],
  ["check_union_graphs", "⛁ All graphs union toggle: off by default, 0→union when on (remote + resident), announced, explainer suppressed", 240000, false],
  ["check_deeplink_view_state", "deep links carry the view: union/reason round-trip WITH differing results, strategy/round/fed/view/labels restored, default hash unchanged", 300000, false],
  ["check_card_modal", "Dataset Card modal: rendered + coloured JSON, remote & resident", 120000, false],
  ["check_len_probe_hostile", "host under-reports length via HEAD + hides Content-Range (#95)", 120000, false],
  ["check_clear", "Clear everything empties 4 stores + Cache API", 90000, false],
  ["check_worker_init", "broken engine wasm surfaces an error (no infinite hang)", 90000, true],
  ["check_refresh_session", "Settings ↻ Refresh actually reloads the document", 90000, false],
  ["check_async_fallback", "async assets 404 → degrades to sync reader, still runs", 120000, true],
  ["check_query_shapes", "property paths + CONSTRUCT→graph + reasoning (embedded)", 90000, false],
  ["check_boe_reason", "BOE OWL 2 QL reasoning over live R2 (0 → N with 🧠 Reason)", 150000, true],
  ["check_davidrumsey_spatial", "davidrumsey six place fields roll up under dct:spatial over live R2 (N with 🧠 Reason → 0 without)", 420000, true],
  ["check_enac", "EPFL ENAC repositories by lab over live R2", 150000, true],
  ["check_recent_build", "RECENTLY-BUILT file, bound predicate+object over live R2", 150000, true],
  ["check_schema_empty", "no-pyramid dataset shows honest empty schema (no stale scholar leak)", 120000, true],
  ["check_schema_render", "with-pyramid remote schema + ontology diagram render on async default", 120000, true],
  ["check_ontology_docs", "ReSpec-style ontology reference (classes+props+definitions) renders, incl. --no-pyramid dblp", 150000, true],
  ["check_map_geo", "embedded GeoSPARQL → Tiles · local PMTiles fixture", 90000, false],
  ["check_service_success", "successful SERVICE join · local SPARQL JSON endpoint", 90000, false],
  ["check_builder", "in-browser N-Quads build → open bytes → query Alice", 90000, false],
  ["check_builder_card", "in-browser build writes the file's Dataset Card (CLI-identical validation; derived profile + build record absent, not empty)", 150000, false],
  ["check_cache_mode", "whole-file cache persists across reload · zero second read", 120000, false],
  ["check_cache_url", "off-catalog URL cache: size-first consent · zero-network reload + deep link", 240000, false],
  ["check_optional_tabs", "Ask AI + Semantic/RAG initialize without model downloads", 90000, false],
  ["check_local_lazy", "a LOCAL .rete opens through the range reader (both engines) and is never read whole; small files still load whole", 900000, false],
];
async function g2(port) {
  for (const [name, label, timeout, requiresLiveR2] of G2) {
    if (only && !name.includes(only)) continue;
    if (LOCAL_ONLY && requiresLiveR2) continue;
    const r = await runChild("node", [`${ROOT}/tests/gate/checks/${name}.mjs`], { PGPORT: String(port) }, timeout);
    const j = lastJson(r.out);
    const ok = r.code === 0 && j && j.verdict === "PASS";
    record("G2", `${name} — ${label}`, ok, ok ? "" : (j ? JSON.stringify(j).slice(0, 160) : r.out.split("\n").filter(Boolean).slice(-2).join(" | ").slice(0, 160)));
  }
}

// ---------- optional: every catalog query in the real playground ----------
async function catalogSweep(port) {
  const scope = CATALOG_SCOPE || "embedded";
  const timeout = scope === "all" ? 6 * 60 * 60 * 1000 : 20 * 60 * 1000;
  const r = await runChild(
    "node",
    [`${ROOT}/tests/gate/checks/check_catalog_examples.mjs`],
    {
      PGPORT: String(port),
      RETE_CATALOG_SCOPE: scope,
      RETE_CATALOG_DATASET: CATALOG_DATASET,
    },
    timeout,
    `${ROOT}/tests/gate`,
    true,
  );
  const j = lastJson(r.out);
  const ok = r.code === 0 && j && j.verdict === "PASS";
  const firstFailure = j && j.failures && j.failures[0];
  const note = ok
    ? `${j.queries} queries across ${j.datasets} datasets in ${j.browser}; ${j.reportPath}`
    : (firstFailure
        ? `${j.failures.length} failed; first ${firstFailure.id}: ${firstFailure.error || firstFailure.qmeta}; ${j.reportPath}`
        : r.out.slice(-240));
  record("G2-catalog", `${scope} catalog examples`, ok, note);
}

// ---------- optional: deployed page (informational) ----------
async function deployed() {
  const r = await runChild("node", [`${ROOT}/tests/gate/checks/check_deployed.mjs`], {}, 150000);
  const j = lastJson(r.out);
  const ok = r.code === 0 && j && j.verdict === "PASS";
  console.log(`  ${ok ? "✓" : "⚠"} [live] deployed GitHub Pages lazy query ${ok ? "" : "— " + (j ? JSON.stringify(j).slice(0, 200) : r.out.slice(-200))}`);
}

// ---------- main ----------
const servers = [];
try {
  console.log("── G0 static ──");
  g0();
  if (!only) {
    const nodeServer = await serve(`${ROOT}/tests/gate/.cache`);
    servers.push(nodeServer.process);
    console.log("── G1 engine-in-node ──");
    await g1(nodeServer.port);
  }
  if (!FAST) {
    const browserServer = await serve(`${ROOT}/docs`);
    servers.push(browserServer.process);
    console.log(CATALOG_SCOPE
      ? "── G2 exhaustive catalog browser matrix ──"
      : "── G2 browser matrix ──");
    if (CATALOG_SCOPE) await catalogSweep(browserServer.port);
    else await g2(browserServer.port);
    if (DEPLOYED) { console.log("── live (informational) ──"); await deployed(); }
  }
} finally { servers.forEach((s) => { try { s.kill(); } catch (e) { /* ignore */ } }); }

const fails = results.filter((r) => !r.ok);
console.log(`\n${"─".repeat(60)}\nGATE ${fails.length ? "RED" : "GREEN"} — ${results.length - fails.length}/${results.length} passed · ${((Date.now() - t0) / 1000).toFixed(0)}s`);
if (fails.length) { console.log("Failing:"); fails.forEach((f) => console.log(`  ✗ [${f.tier}] ${f.name}${f.note ? " — " + f.note : ""}`)); }
process.exit(fails.length ? 1 : 0);
