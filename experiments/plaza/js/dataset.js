// dataset.js — one dataset's page: its full card, companion downloads, and a
// live explore panel (autocomplete + SPARQL) backed by a WASM worker.
import { readReteCard, liteCardFromHeader, fmtBytes } from "./rete-card.js";
import { imageInfoFromCard } from "./procgen.js";
import { renderFingerprint } from "./procgen-p5.js";
import { mountSchemaUML } from "./schema-uml.js";
import { derivedFacets } from "./facets.js";
import { detectProviders } from "./providers.js";
import { mountTableExplorer } from "./tables-duckdb.js";
import { usedOntologies } from "./vocabs.js";

const root = document.getElementById("root");
const fmt = (n) => (n == null ? "—" : Intl.NumberFormat().format(n));
const esc = (s) =>
  String(s == null ? "" : s).replace(/[&<>"]/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])
  );

// Outbound links: upgrade public http→https (but leave localhost alone) and
// resolve relative paths to absolute (for the copy-link button).
const httpsify = (u) => {
  const s = String(u || "");
  return /^http:\/\/(localhost|127\.0\.0\.1)/i.test(s) ? s : s.replace(/^http:\/\//i, "https://");
};
const absUrl = (u) => { try { return new URL(u, location.href).href; } catch (_) { return u; } };
let REMOTE_TOKEN = ""; // from the manifest; appended to bucket URLs that lack one
const withToken = (u) => {
  const s = String(u || "");
  if (!REMOTE_TOKEN || !/hf\.space|katospiegel/i.test(s) || /[?&]token=/.test(s)) return s;
  return s + (s.includes("?") ? "&" : "?") + "token=" + REMOTE_TOKEN;
};

const themeNow = () => (document.documentElement.dataset.theme === "light" ? "light" : "dark");
const HERO = { w: 960, h: 600 };
let ENTRY = null; // manifest entry, for the explore panel
let HERO_INFO = null; // image spec, kept so we can re-skin on theme toggle

main();

// Re-skin only the hero image on theme change (leaves the explore session intact).
window.addEventListener("plaza-theme", async () => {
  if (!HERO_INFO) return;
  const url = await renderFingerprint(HERO_INFO, { theme: themeNow(), ...HERO, labels: true });
  const art = document.querySelector(".detail-hero .art");
  if (art && url) art.innerHTML = `<img class="art-img" src="${url}" alt="">`;
});

async function main() {
  const key = new URLSearchParams(location.search).get("key");
  const manifest = await fetch("plaza.json").then((r) => r.json());
  REMOTE_TOKEN = manifest.remoteToken || "";
  const entry = manifest.datasets.find((d) => d.key === key);
  if (!entry) {
    root.innerHTML = `<div class="warnbox">Unknown dataset <code>${esc(key)}</code>. <a href="index.html">Back to the plaza.</a></div>`;
    return;
  }
  ENTRY = entry;
  document.title = `rete plaza — ${entry.title || entry.key}`;

  let header = null,
    card = null,
    cardErr = null,
    size = null;
  try {
    const r = await readReteCard(entry.rete);
    header = r.header;
    card = r.card || liteCardFromHeader(header, entry);
    size = r.size;
  } catch (e) {
    cardErr = String(e);
    card = liteCardFromHeader({ quadCount: null, termCount: null, version: null }, entry);
  }

  HERO_INFO = imageInfoFromCard(card, entry, header);
  let heroUrl = null;
  try { heroUrl = await renderFingerprint(HERO_INFO, { theme: themeNow(), ...HERO, labels: true }); } catch (_) {}

  render(entry, card, header, cardErr, heroUrl, size);
  wireHero();
  const gEl = document.getElementById("schemaGraph");
  const iEl = document.getElementById("schemaInfo");
  if (gEl && iEl) { try { mountSchemaUML(card, gEl, iEl).catch(() => {}); } catch (_) {} }
  wireSchemaFullscreen(card, entry);
  const tbl = document.getElementById("tblExplore");
  if (tbl) { try { mountTableExplorer(tbl, entry, REMOTE_TOKEN); } catch (_) {} }
  setupExplore(entry, card);
}

// Open the UML schema in a large modal/lightbox (re-mounts ELK at full size).
function wireSchemaFullscreen(card, entry) {
  const btn = document.getElementById("schemaFs");
  const modal = document.getElementById("modal");
  if (!btn || !modal) return;
  const close = () => { modal.hidden = true; document.getElementById("modalCard").innerHTML = ""; };
  btn.addEventListener("click", () => {
    document.getElementById("modalCard").innerHTML = `
      <button class="modal-x" id="modalX" aria-label="Close">×</button>
      <h2 style="margin:0 0 10px;font-size:18px">Ontology / schema — ${esc(entry.title || entry.key)}</h2>
      <div class="schema-wrap modal-schema">
        <div id="schemaGraphFs" class="schema-graph"></div>
        <div id="schemaInfoFs" class="schema-info"></div>
      </div>`;
    modal.hidden = false;
    document.getElementById("modalX").onclick = close;
    mountSchemaUML(card, document.getElementById("schemaGraphFs"), document.getElementById("schemaInfoFs")).catch(() => {});
  });
  modal.addEventListener("click", (e) => { if (e.target === modal) close(); });
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !modal.hidden) close(); });
}

// Wire the "copy link" button under the thumbnail (the dropdown is plain <a>s).
function wireHero() {
  const btn = document.getElementById("copyRete");
  if (!btn) return;
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(btn.dataset.url);
      btn.textContent = "Copied!";
    } catch (_) {
      btn.textContent = "Copy failed";
    }
    setTimeout(() => (btn.textContent = "Copy link"), 1400);
  });
}

function render(entry, card, header, cardErr, heroUrl, size) {
  const hasSchema = (card.classes && card.classes.length) || (card.class_links && card.class_links.length);
  const cat = entry.category || ((entry.tags || []).some((t) => ["ontology", "owl", "schema", "obo", "rdfs"].includes(String(t).toLowerCase())) ? "ontology" : "dataset");
  const img = heroUrl ? `<img class="art-img" src="${heroUrl}" alt="">` : "";
  const imgCap = hasSchema
    ? `Hand-inked schema portrait — classes sized by instances, class-to-class relations, coloured by vocabulary.`
    : `Hand-inked fingerprint of <b>${esc(entry.key)}</b> — this file ships no schema profile (rebuild with <code>--card</code> for a class portrait).`;
  const triples = card.triple_count ?? card.quad_count;
  const coh = card.coherence;
  const cohPill =
    coh && typeof coh.coherent === "boolean"
      ? coh.coherent
        ? `<span class="pill ok">coherent</span>`
        : `<span class="pill warn">incoherent · ${coh.inconsistency_count || 0}</span>`
      : "";
  // Extra derived facets not already shown as pills above (geometry, time, langs, vocab count).
  const lic = card.license || entry.license;
  const heroFacets = derivedFacets(card, entry).filter(
    (f) => !["remote", "bundled", "incoherent", "header-only"].includes(f) && f !== lic
  );
  const heroLinks = entry.links || [];

  root.innerHTML = `
    <div class="detail-hero">
      <div>
        <div class="art">${img}</div>
        <div class="art-cap">${imgCap}</div>
        ${filesDropdown(entry, header)}
      </div>
      <div>
        <h1>${esc(splitTitle(entry.title || entry.key).name)}</h1>
        ${splitTitle(entry.title || entry.key).sub ? `<div class="hero-subtitle">${esc(splitTitle(entry.title || entry.key).sub)}</div>` : ""}
        <div class="desc">${esc(card.description || entry.blurb || "")}</div>
        <div class="facts">
          ${size != null ? `<span><b>${fmtBytes(size)}</b> file</span>` : ""}
          ${triples != null ? `<span><b>${fmt(triples)}</b> triples</span>` : ""}
          ${card.named_graph_count ? `<span><b>${fmt(card.named_graph_count)}</b> graphs</span>` : ""}
          ${card.term_count != null ? `<span><b>${fmt(card.term_count)}</b> terms</span>` : ""}
          ${header ? `<span>format v<b>${header.version}</b></span>` : ""}
        </div>
        <div class="facts">
          <span class="pill">${cat === "ontology" ? "ontology" : "dataset"}</span>
          ${card.license ? `<span class="pill">${esc(card.license)}</span>` : ""}
          ${cohPill}
          ${card._lite ? `<span class="pill" title="no embedded card; stats from header">header-only card</span>` : ""}
          ${header ? `<span class="pill mono" title="blake3-16 content hash">${header.contentHash.slice(0, 12)}…</span>` : ""}
        </div>
        ${heroFacets.length ? `<div class="hero-facets">${heroFacets.map((f) => `<span class="pill facet">${esc(f)}</span>`).join("")}</div>` : ""}
        ${heroLinks.length ? `<div class="hero-links">${heroLinks.map((l) => `<a href="${esc(httpsify(l.url))}" target="_blank" rel="noopener">${esc(l.label)} ↗</a>`).join(" &nbsp;·&nbsp; ")}</div>` : ""}
      </div>
    </div>

    ${cardErr ? `<div class="warnbox">Couldn't read the card from the file (${esc(cardErr)}). Showing manifest data only.</div>` : ""}

    ${hasSchema ? `<div class="section"><h2>Ontology / schema <button class="fs-btn" id="schemaFs">⤢ Fullscreen</button></h2>
      <div class="schema-wrap">
        <div id="schemaGraph" class="schema-graph"></div>
        <div id="schemaInfo" class="schema-info"></div>
      </div>
      <div class="notice">UML class diagram (à la OpenPULSE) — boxes are classes with their literal properties; arrows are object properties, routed orthogonally by ELK. Hover/click a class for details; <b>⤢ Fullscreen</b> for a bigger canvas.</div>
    </div>` : ""}

    ${aboutSection(card, header)}
    ${profileSection(card)}
    ${vocabSection(card, entry)}
    ${connectionsSection(card, entry)}
    ${signalsSection(card)}

    <div class="section">
      <h2>Explore</h2>
      <div class="explore" id="explore"></div>
    </div>

    ${(entry.companions || []).some((c) => c.kind === "parquet") ? `<div class="section"><h2>Explore tables</h2><div class="explore" id="tblExplore"></div></div>` : ""}

    <div class="section">
      <details>
        <summary style="cursor:pointer;color:var(--faint)">raw card JSON</summary>
        <textarea class="sparql" readonly style="min-height:220px;margin-top:10px">${esc(JSON.stringify(card, null, 2))}</textarea>
      </details>
    </div>`;
}

function aboutSection(card, header) {
  const rows = [];
  if (card.source) rows.push(["source", `<a href="${esc(httpsify(card.source))}" target="_blank" rel="noopener">${esc(card.source)}</a>`]);
  if (card.created) rows.push(["created", esc(card.created)]);
  const sig = card.signals || {};
  if (sig.base_iri) rows.push(["base IRI", `<span class="mono">${esc(sig.base_iri)}</span>`]);
  if (sig.label_predicate) rows.push(["label predicate", `<span class="mono">${esc(sig.label_predicate)}</span>`]);
  if (sig.default_lang) rows.push(["default language", esc(sig.default_lang)]);
  if (header) rows.push(["content hash", `<span class="mono">${esc(header.contentHash)}</span>`]);
  if (!rows.length) return "";
  return `<div class="section"><h2>About</h2><div class="panel"><dl class="kv">${rows
    .map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`)
    .join("")}</dl></div></div>`;
}

function profileSection(card) {
  const preds = card.predicates || [];
  const classes = card.classes || [];
  if (!preds.length && !classes.length)
    return `<div class="section"><h2>Profile</h2><div class="notice">This file ships no embedded profile (built without <code>--card</code>). Run a query below to derive predicates and classes live.</div></div>`;
  return `<div class="section"><h2>Profile</h2><div class="cols">
    ${preds.length ? `<div class="panel"><div class="notice" style="margin:0 0 10px">top predicates</div>${barlist(preds)}</div>` : ""}
    ${classes.length ? `<div class="panel"><div class="notice" style="margin:0 0 10px">classes by instance count</div>${barlist(classes)}</div>` : ""}
  </div></div>`;
}

function barlist(pairs, limit = 12) {
  const top = pairs.slice(0, limit);
  const max = Math.max(1, ...top.map((p) => p[1]));
  return `<div class="barlist">${top
    .map(
      ([label, n]) => `<div class="row">
        <span class="lbl mono" title="${esc(label)}">${esc(shortIri(label))}</span>
        <span class="n">${fmt(n)}</span>
        <span class="track"><i style="width:${Math.max(3, (n / max) * 100)}%"></i></span>
      </div>`
    )
    .join("")}</div>`;
}

function vocabSection(card, entry) {
  const onts = usedOntologies(card, entry);
  const raw = card.vocabularies || [];
  if (!onts.length && !raw.length) return "";
  return `<div class="section"><h2>Ontologies &amp; vocabularies</h2>
    ${onts.length ? `<div class="conns">${onts
      .map((o) => `<a class="conn" href="${esc(httpsify(o.url))}" target="_blank" rel="noopener">${esc(o.name)}</a>`)
      .join("")}</div>` : ""}
    ${raw.length ? `<details style="margin-top:10px"><summary style="cursor:pointer;color:var(--faint);font-size:12.5px">${raw.length} raw namespaces</summary>
      <div class="tags" style="gap:8px;margin-top:8px">${raw
        .map((ns) => `<span class="mono" style="font-size:11.5px;color:var(--muted);background:var(--panel);border:1px solid var(--line);padding:3px 8px;border-radius:8px">${esc(ns)}</span>`)
        .join("")}</div></details>` : ""}
    <div class="notice">The ontologies and vocabularies this dataset is built with (its predicate/class namespaces, named where recognised — including OBO sub-ontologies like CHMO/RXNO read from the class IRIs).</div></div>`;
}

function connectionsSection(card, entry) {
  const provs = detectProviders(card, entry);
  if (!provs.length)
    return `<div class="section"><h2>Connected to</h2><div class="notice">Self-contained — no external identifier providers detected.</div></div>`;
  return `<div class="section"><h2>Connected to</h2>
    <div class="conns">${provs
      .map((pv) => `<a class="conn" href="${esc(httpsify(pv.url))}" target="_blank" rel="noopener">${esc(pv.name)}${pv.note ? ` <small>${esc(pv.note)}</small>` : ""}</a>`)
      .join("")}</div>
    <div class="notice">External databases / identifier providers this dataset links out to — detected from the IRIs it uses, plus curated cross-references.</div></div>`;
}

function signalsSection(card) {
  const s = card.signals;
  if (!s) return "";
  const rows = [];
  if (s.geo_wkt || s.geo_latlong) rows.push(["geometry", s.geo_wkt ? "WKT literals" : "lat/long"]);
  if (s.temporal_extent) rows.push(["temporal extent", `${esc(s.temporal_extent[0])} → ${esc(s.temporal_extent[1])}`]);
  if (s.spatial_bbox) rows.push(["bounding box", `[${s.spatial_bbox.map((x) => x.toFixed(2)).join(", ")}]`]);
  if (s.numeric_predicates && s.numeric_predicates.length) rows.push(["numeric predicates", s.numeric_predicates.slice(0, 4).map((p) => `<span class="mono">${esc(shortIri(p))}</span>`).join(", ")]);
  if (s.link_predicates && s.link_predicates.length) rows.push(["link predicates", s.link_predicates.slice(0, 4).map((p) => `<span class="mono">${esc(shortIri(p))}</span>`).join(", ")]);
  if (!rows.length) return "";
  return `<div class="section"><h2>Signals</h2><div class="panel"><dl class="kv">${rows
    .map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`)
    .join("")}</dl></div></div>`;
}

// Files as a dropdown under the thumbnail + a copy-link button (the .rete first,
// then Parquet/DuckDB/SQLite companions). Links are https-upgraded, new tab.
function filesDropdown(entry, header) {
  const files = [
    { kind: "rete", label: ".rete — range-queryable graph", url: entry.rete, note: header ? `${fmt(header.quadCount)} triples` : null },
    ...(entry.companions || []),
  ];
  const copyUrl = httpsify(withToken(absUrl(entry.rete)));
  return `<div class="files">
    <details class="files-dd">
      <summary>Files &amp; downloads ▾</summary>
      <div class="files-menu">
        ${files
          .map(
            (x) => `<a class="file-opt" href="${esc(httpsify(withToken(absUrl(x.url))))}" target="_blank" rel="noopener" download>
              <span class="ft ${esc(x.kind)}">${esc(x.kind)}</span>
              <span>${esc(x.label || x.kind)}${x.verified === false ? " <small>(by convention)</small>" : ""}${x.note ? ` <small>· ${esc(x.note)}</small>` : ""}</span>
            </a>`
          )
          .join("")}
      </div>
    </details>
    <button class="copy-link" id="copyRete" data-url="${esc(copyUrl)}" title="Copy the .rete link to clipboard">Copy link</button>
  </div>`;
}

// ---------------------------------------------------------------------------
// Explore panel: WASM worker + autocomplete + SPARQL
// ---------------------------------------------------------------------------
function setupExplore(entry, card) {
  const host = document.getElementById("explore");
  const typePred = entry.typePredicate || "a";
  const starters = buildStarters(card, typePred);

  host.innerHTML = `
    <div class="ac">
      <input class="search" id="ac" type="search" placeholder="Autocomplete: type a label to find an entity…" autocomplete="off" disabled />
      <div class="ac-list" id="aclist" hidden></div>
    </div>
    <div class="starter" id="starter"></div>
    <textarea class="sparql" id="sparql" spellcheck="false">${esc(starters[0].sparql)}</textarea>
    <div class="run-row">
      <button class="run" id="run" disabled>Run query</button>
      <span class="status" id="status">starting engine…</span>
    </div>
    <div class="results" id="results" hidden></div>
    <div class="notice">${entry.kind === "remote-lazy" ? "Remote dataset: selective queries fault in only the bytes they touch; whole-predicate aggregates read more." : "Bundled dataset: fully loaded into the in-browser engine."}</div>`;

  const starterEl = document.getElementById("starter");
  starters.forEach((s) => {
    const b = document.createElement("button");
    b.textContent = s.title;
    b.onclick = () => {
      document.getElementById("sparql").value = s.sparql;
      runQuery();
    };
    starterEl.appendChild(b);
  });

  let worker, openP, rpcId = 0;
  const pending = new Map();
  try {
    // Classic worker: it importScripts the no-modules WASM build (see plaza-worker.js).
    worker = new Worker(new URL("./plaza-worker.js", import.meta.url));
  } catch (e) {
    document.getElementById("status").textContent = "Live explore couldn't start a worker: " + e;
    return;
  }
  openP = new Promise((resolve) => {
    worker.onmessage = (e) => {
      const m = e.data;
      if (m.type === "opened") return resolve(m);
      const p = pending.get(m.reqId);
      if (p) {
        pending.delete(m.reqId);
        m.ok ? p.res(m) : p.rej(new Error(m.error));
      }
    };
  });
  const rpc = (msg) =>
    new Promise((res, rej) => {
      const reqId = ++rpcId;
      pending.set(reqId, { res, rej });
      worker.postMessage({ ...msg, reqId });
    });

  // Open the graph: remote → lazy URL; bundled → fetch bytes once, transfer in.
  (async () => {
    try {
      if (entry.kind === "remote-lazy") {
        worker.postMessage({ type: "open", mode: "remote", url: entry.rete });
      } else {
        const buf = await fetch(entry.rete).then((r) => r.arrayBuffer());
        worker.postMessage({ type: "open", mode: "local", bytes: buf }, [buf]);
      }
      const res = await openP;
      const status = document.getElementById("status");
      if (!res.ok) {
        status.textContent = "Couldn't open the dataset: " + res.error;
        return;
      }
      status.textContent = "ready";
      document.getElementById("run").disabled = false;
      document.getElementById("ac").disabled = false;
    } catch (e) {
      document.getElementById("status").textContent = String(e);
    }
  })();

  // --- query --- (hoisted so the starter buttons bound above can call it)
  async function runQuery() {
    const sparql = document.getElementById("sparql").value.trim();
    if (!sparql) return;
    const status = document.getElementById("status");
    const runBtn = document.getElementById("run");
    runBtn.disabled = true;
    status.textContent = "running…";
    const t0 = performance.now();
    try {
      const { json } = await rpc({ type: "query", sparql });
      renderResults(JSON.parse(json));
      status.textContent = `${((performance.now() - t0) / 1000).toFixed(2)}s`;
    } catch (e) {
      renderError(String(e));
      status.textContent = "error";
    } finally {
      runBtn.disabled = false;
    }
  }
  document.getElementById("run").onclick = runQuery;

  // --- autocomplete ---
  const acInput = document.getElementById("ac");
  const acList = document.getElementById("aclist");
  let acTimer, acItems = [], acActive = -1;
  acInput.addEventListener("input", () => {
    clearTimeout(acTimer);
    const prefix = acInput.value.trim();
    if (prefix.length < 2) return hideAc();
    acTimer = setTimeout(async () => {
      try {
        const { results } = await rpc({ type: "prefix", prefix, limit: 25 });
        showAc(results);
      } catch (e) {
        showAc([], String(e));
      }
    }, 180);
  });
  acInput.addEventListener("keydown", (e) => {
    if (acList.hidden) return;
    if (e.key === "ArrowDown") { acActive = Math.min(acActive + 1, acItems.length - 1); paintAc(); e.preventDefault(); }
    else if (e.key === "ArrowUp") { acActive = Math.max(acActive - 1, 0); paintAc(); e.preventDefault(); }
    else if (e.key === "Enter" && acActive >= 0) { pickAc(acItems[acActive]); e.preventDefault(); }
    else if (e.key === "Escape") hideAc();
  });
  document.addEventListener("click", (e) => { if (!host.contains(e.target)) hideAc(); });

  function showAc(results, err) {
    acItems = results || [];
    acActive = -1;
    if (err) { acList.innerHTML = `<div class="empty-ac">${esc(err)}</div>`; acList.hidden = false; return; }
    if (!acItems.length) { acList.innerHTML = `<div class="empty-ac">no matching labels (the file may carry no label index)</div>`; acList.hidden = false; return; }
    paintAc();
    acList.hidden = false;
  }
  function paintAc() {
    acList.innerHTML = acItems
      .map((it, i) => `<div class="item ${i === acActive ? "active" : ""}" data-i="${i}"><span>${esc(it.label)}</span><span class="iri">${esc(it.subject)}</span></div>`)
      .join("");
    [...acList.children].forEach((el) => (el.onclick = () => pickAc(acItems[+el.dataset.i])));
  }
  function hideAc() { acList.hidden = true; }
  function pickAc(it) {
    if (!it) return;
    hideAc();
    acInput.value = it.label;
    const subj = /^[<_]/.test(it.subject) ? it.subject : `<${it.subject}>`;
    document.getElementById("sparql").value =
      `SELECT ?p ?o WHERE {\n  ${subj} ?p ?o .\n} LIMIT 200`;
    runQuery();
  }

  // --- results table ---
  function renderResults(out) {
    const box = document.getElementById("results");
    box.hidden = false;
    if (!out || out.kind === "ask") {
      box.innerHTML = `<div style="padding:14px">ASK → <b>${out && out.boolean ? "true" : "false"}</b></div>`;
      return;
    }
    if (out.kind === "construct") {
      const t = out.triples || [];
      box.innerHTML = table(["subject", "predicate", "object"], t.map((r) => r.map(cell)));
      if (!t.length) box.innerHTML = `<div style="padding:14px;color:var(--faint)">CONSTRUCT produced no triples.</div>`;
      return;
    }
    const vars = out.vars || [];
    const rows = (out.rows || []).map((row) => vars.map((v) => cell(row[v])));
    box.innerHTML = rows.length
      ? table(vars, rows) + `<div class="notice" style="padding:8px 12px">${rows.length} rows</div>`
      : `<div style="padding:14px;color:var(--faint)">No rows.</div>`;
  }
  function renderError(msg) {
    const box = document.getElementById("results");
    box.hidden = false;
    box.innerHTML = `<div class="warnbox" style="border-radius:10px">${esc(msg)}</div>`;
  }
}

function table(cols, rows) {
  return `<table class="rs"><thead><tr>${cols.map((c) => `<th>${esc(c)}</th>`).join("")}</tr></thead>
    <tbody>${rows.map((r) => `<tr>${r.map((c) => `<td>${c}</td>`).join("")}</tr>`).join("")}</tbody></table>`;
}

// Render one RDF term as compact HTML (IRIs linked, literals shown plainly).
function cell(term) {
  if (term == null) return `<span style="color:var(--faint)">—</span>`;
  const s = String(term);
  if (s.startsWith("<") && s.endsWith(">")) {
    const iri = s.slice(1, -1);
    return `<a href="${esc(httpsify(iri))}" target="_blank" rel="noopener" title="${esc(iri)}">${esc(shortIri(iri))}</a>`;
  }
  const lit = s.match(/^"([\s\S]*)"(?:@([\w-]+)|\^\^<(.+)>)?$/);
  if (lit) {
    const tag = lit[2] ? ` <span style="color:var(--faint)">@${esc(lit[2])}</span>` : lit[3] ? ` <span style="color:var(--faint)" title="${esc(lit[3])}">^^${esc(shortIri(lit[3]))}</span>` : "";
    return esc(lit[1]) + tag;
  }
  return esc(s);
}

function shortIri(iri) {
  const s = String(iri).replace(/^<|>$/g, "");
  const m = s.match(/[#/]([^#/]+)\/?$/);
  return m ? m[1] : s;
}

function splitTitle(t) {
  const s = String(t || "");
  const m = s.match(/^(.*?)\s+[—–-]\s+(.*)$/);
  return m ? { name: m[1].trim(), sub: m[2].trim() } : { name: s.trim(), sub: "" };
}

function buildStarters(card, typePred) {
  const out = [];
  // Prefer the file's own auto-generated, vocabulary-instantiated query library.
  for (const q of card.queries || []) {
    if (q && q.sparql) out.push({ title: q.title || q.id || q.dimension || "query", sparql: q.sparql });
    if (out.length >= 8) break;
  }
  // Always offer a few generic starters that work on any graph.
  out.push({ title: "Sample triples", sparql: `SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 50` });
  out.push({ title: "Top predicates", sparql: `SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n) LIMIT 25` });
  out.push({ title: "Classes by size", sparql: `SELECT ?c (COUNT(*) AS ?n) WHERE { ?s ${typePred} ?c } GROUP BY ?c ORDER BY DESC(?n) LIMIT 25` });
  return out;
}
