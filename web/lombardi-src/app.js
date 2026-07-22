/* Redrawing Mark Lombardi — a reading of lombardi.rete, in ink.
 *
 * Everything on the sheet comes out of the .rete over HTTP range: the list of
 * works, the nodes and arcs of the open drawing, and the card behind each name.
 * Nothing is pre-baked. The engine runs in a worker because RemoteGraph reads
 * through synchronous XHR, which browsers only allow off the main thread.
 *
 * The drawing itself is a reading, not a facsimile. The topology, the arc types
 * and the year markers are Lombardi's; the placement is a force layout. Two
 * things carry his hand across: arcs are drawn as CIRCULAR ARCS (never straight
 * lines), and the year markers are pinned along the bottom in date order, the
 * timeline he ruled across the foot of so many sheets.
 */
"use strict";

const $ = (id) => document.getElementById(id);
const PFX = `PREFIX lo: <https://w3id.org/rete/lombardi/>
PREFIX lom: <http://www.lombardinetworks.net/lombardi.owl#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX schema: <https://schema.org/>
PREFIX dcterms: <http://purl.org/dc/terms/>
`;

function b64ToBytes(b64) {
  const bin = atob(b64);
  const u = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
  return u;
}
const esc = (s) => String(s == null ? "" : s).replace(/[&<>"]/g,
  (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const fmtBytes = (n) => n < 1024 ? `${n} B`
  : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(2)} MB`;

/* Terms arrive in the engine's lexical "table" form: <iri>, "lit", "lit"@en. */
function plain(term) {
  if (term == null) return null;
  const s = String(term);
  if (s.startsWith("<") && s.endsWith(">")) return s.slice(1, -1);
  const m = s.match(/^"([\s\S]*)"(?:@[\w-]+|\^\^<.+>)?$/);
  return m ? m[1] : s;
}
const localName = (iri) => String(iri || "").split(/[#/]/).filter(Boolean).pop();

/* ------------------------------------------------------------------ engine */

const engine = {
  worker: null, seq: 0, pending: new Map(), bytes: 0, ready: null,

  boot() {
    const src = $("reteGlue").textContent + "\n" + $("workerSrc").textContent;
    this.worker = new Worker(URL.createObjectURL(new Blob([src], { type: "text/javascript" })));
    this.worker.onmessage = (e) => {
      const m = e.data || {};
      if (m.type === "progress") { this.bytes = m.bytes; paintTraffic(); return; }
      const p = this.pending.get(m.reqId);
      if (!p) return;
      this.pending.delete(m.reqId);
      m.ok ? p.resolve(m) : p.reject(new Error(m.error || "engine error"));
    };
    this.worker.onerror = (e) => {
      for (const p of this.pending.values()) p.reject(new Error(e.message || "worker crashed"));
      this.pending.clear();
    };
    const w = b64ToBytes(RETE_WASM_B64);
    this.ready = this.call({ type: "init", wasm: w.buffer }, [w.buffer]);
    return this.ready;
  },

  call(msg, transfer) {
    return new Promise((resolve, reject) => {
      msg.reqId = ++this.seq;
      this.pending.set(msg.reqId, { resolve, reject });
      this.worker.postMessage(msg, transfer || []);
    });
  },

  async open() {
    await this.ready;
    return this.call({ type: "open", key: "lombardi", mode: "remote", url: RETE_URL });
  },

  /* Every query on this page goes through here, so each one lands in the log
   * with what it actually cost in bytes off the wire. */
  async ask(label, sparql) {
    await this.ready;
    const r = await this.call({ type: "query", key: "lombardi", sparql: PFX + sparql });
    const env = JSON.parse(r.json);
    const rows = (env.rows || []).map((row) => {
      const o = {};
      for (const k of env.vars || Object.keys(row)) o[k] = plain(row[k]);
      return o;
    });
    logQuery(label, sparql, rows.length, r.ms, r.traffic);
    return rows;
  },
};

/* -------------------------------------------------------------- query log */

function logQuery(label, sparql, rows, ms, traffic) {
  const li = document.createElement("li");
  const body = sparql.trim().split("\n").slice(0, 5).join("\n");
  const cost = traffic && traffic.bytes
    ? ` · ${fmtBytes(traffic.bytes)} in ${traffic.requests} range req`
    : " · served from cache";
  li.innerHTML = `<div class="q">${esc(body)}</div>` +
    `<div class="r">${esc(label)} — ${rows} row${rows === 1 ? "" : "s"} · ${Math.round(ms)} ms${esc(cost)}</div>`;
  const list = $("qlog");
  list.insertBefore(li, list.firstChild);
  while (list.children.length > 14) list.removeChild(list.lastChild);
}

function paintTraffic() {
  $("note-r").textContent =
    `${fmtBytes(engine.bytes)} read of 1.0 MB · only what the queries touched`;
}

/* ---------------------------------------------------------------- queries */

const Q = {
  works: `SELECT ?w ?title ?nodes ?arcs ?from ?to ?dims WHERE {
  ?w a lo:Drawing ; rdfs:label ?title ; lo:nodeCount ?nodes ; lo:edgeCount ?arcs .
  OPTIONAL { ?w lo:narrationStart ?from . ?w lo:narrationEnd ?to }
  OPTIONAL { ?w lo:dimensions ?dims }
} ORDER BY ?title`,

  nodes: (w) => `SELECT ?n ?label ?type ?wc ?year WHERE {
  <${w}> lo:depicts ?n .
  ?n rdfs:label ?label ; a ?type ; lo:workCount ?wc .
  OPTIONAL { ?n lo:year ?year }
}`,

  arcs: (w) => `SELECT ?s ?o ?type ?drawn ?amount WHERE {
  ?e lo:inDrawing <${w}> ; lo:source ?s ; lo:target ?o ; lo:arcType ?t .
  ?t rdfs:label ?type ; lo:drawnAs ?drawn .
  OPTIONAL { ?e lo:amount ?amount }
}`,

  /* The two directions are asked SEPARATELY and on purpose. Written as one
   * `{…} UNION {…}` query the planner stops pushing the bound endpoint into the
   * branches and the same 17 rows take ~50 s instead of ~1 s — locally as well
   * as over HTTP, so it is the plan and not the fetching. Two small queries in
   * parallel are both faster and easier to read. */
  arcsFrom: (n) => `SELECT ?other ?label ?t ?w ?amount WHERE {
  ?e lo:source <${n}> ; lo:target ?other ; lo:arcType ?t ; lo:inDrawing ?w .
  ?other rdfs:label ?label .
  OPTIONAL { ?e lo:amount ?amount }
}`,

  arcsTo: (n) => `SELECT ?other ?label ?t ?w ?amount WHERE {
  ?e lo:target <${n}> ; lo:source ?other ; lo:arcType ?t ; lo:inDrawing ?w .
  ?other rdfs:label ?label .
  OPTIONAL { ?e lo:amount ?amount }
}`,

  /* The notation table, fetched once: arc class → its name and the mark on the
   * paper. 21 rows, so the card never has to join it per click. */
  notation: `SELECT ?t ?label ?drawn WHERE { ?t rdfs:label ?label ; lo:drawnAs ?drawn }`,

  /* The physical sheet, for the 17 drawings MoMA holds. Their collection data is
   * CC0; the IMAGE is not — it stays on moma.org and the graph carries a rights
   * statement next to it. */
  object: (w) => `SELECT ?img ?full ?page ?date ?medium ?dims ?credit ?accession ?rights WHERE {
  <${w}> lo:momaImage ?img ; lo:momaPage ?page ; lo:accession ?accession .
  OPTIONAL { <${w}> lo:momaImageFull ?full }
  OPTIONAL { <${w}> schema:dateCreated ?date }
  OPTIONAL { <${w}> schema:artMedium ?medium }
  OPTIONAL { <${w}> lo:dimensions ?dims }
  OPTIONAL { <${w}> lo:creditLine ?credit }
  OPTIONAL { <${w}> dcterms:rights ?rights }
}`,

  alsoIn: (n) => `SELECT ?w ?title WHERE {
  <${n}> lo:appearsIn ?w . ?w rdfs:label ?title
} ORDER BY ?title`,
};

/* ------------------------------------------------------- Lombardi's marks
 * In his notation the LINE STYLE is the meaning, so the renderer keys off the
 * arc class rather than inventing a palette. dash: SVG dasharray. tip: what
 * closes the arc — an arrowhead, the double bar he used for a blocked deal, or
 * nothing at all for a plain association. */
const MARKS = {
  InfluenceControl:         { dash: null,    tip: "arrow",  red: false },
  FinancialTransaction:     { dash: "5 3.5", tip: "arrow",  red: false },
  FinancialConnection:      { dash: "5 3.5", tip: null,     red: false },
  FinancialAssociation:     { dash: "5 3.5", tip: "both",   red: false },
  Association:              { dash: null,    tip: "both",   red: false },
  Connection:               { dash: null,    tip: null,     red: false },
  BlockedFailed:            { dash: null,    tip: "bar",    red: false },
  BlockedFailedTransaction: { dash: "5 3.5", tip: "bar",    red: false },
  SaleTransfer:             { dash: "9 4",   tip: "arrow",  red: false },
  SaleProperty:             { dash: "2 3",   tip: null,     red: false },
  Final:                    { dash: null,    tip: "arrow",  red: true  },
  YearArrow:                { dash: null,    tip: "arrow",  red: false },
  YearLine:                 { dash: null,    tip: null,     red: false },
  SingleNearby:             { dash: "1 4",   tip: null,     red: false },
};
const markOf = (t) => MARKS[t] || { dash: null, tip: "arrow", red: false };

/* ------------------------------------------------------- deterministic RNG
 * Same drawing every visit: a sheet of paper doesn't rearrange itself. */
function rng(seed) {
  let s = 0;
  for (const ch of String(seed)) s = (s * 31 + ch.charCodeAt(0)) >>> 0;
  return () => { s ^= s << 13; s ^= s >>> 17; s ^= s << 5; s >>>= 0; return s / 4294967296; };
}

/* --------------------------------------------------------------- layout
 * Fruchterman-Reingold, with the year markers pinned along the foot of the
 * sheet in date order — Lombardi's timeline. */
let W = 1600, H = 1000;
const PAD = 90;

/* Roughly how much room a name takes once lettered, so the separation pass can
 * treat each node as the BOX its text occupies rather than a point. Names are
 * wide and short, which is why the drawings spread sideways. */
function boxOf(d) {
  if (d.year) return { hw: 15, hh: 15 };
  const chars = Math.min(d.label.length, 30);
  const size = d.kind === "FinalInfo" || d.kind === "Final" ? 8.6 : (d.wc > 1 ? 10.4 : 9.6);
  return { hw: (chars * size * 0.56) / 2 + 9, hh: 11 };
}

/* The width:height ratio of the real sheet, parsed from MoMA's catalogued
 * dimensions ("50 x 120\" (127 x 304.8 cm)"). We can't place the names where
 * Lombardi placed them — those coordinates were never digitized — but we CAN give
 * the sheet the true proportions of the object: the BNL drawing then reads as the
 * 2.4:1 banner it actually is (127 x 305 cm), not a generic rectangle. */
function aspectFromDims(dims) {
  if (!dims) return null;
  const cm = dims.match(/\(([\d.]+)\s*[x×]\s*([\d.]+)\s*cm\)/i)
          || dims.match(/([\d.]+)\s*[x×]\s*([\d.]+)\s*cm/i);
  const any = cm || dims.match(/([\d.]+)\s*[x×]\s*([\d.]+)/);
  if (!any) return null;
  const h = parseFloat(any[1]), w = parseFloat(any[2]);   // MoMA lists height × width
  if (!(h > 0 && w > 0)) return null;
  return Math.max(0.85, Math.min(2.6, w / h));             // clamp the extremes
}

function layout(nodes, edges, seed, ratio) {
  const rand = rng(seed);
  const n = nodes.length;
  // a bigger cast needs a bigger sheet — he worked up to five feet wide
  const r = ratio || (1 / 0.70);                 // default landscape if unknown
  const baseW = Math.max(1500, Math.min(3000, 980 + n * 6.2));
  // keep the drawable AREA roughly constant across ratios, so a wide sheet gets
  // wider rather than just flatter and more crowded
  W = Math.round(baseW * Math.sqrt(r / (1 / 0.70)));
  H = Math.round(W / r);
  const idx = new Map(nodes.map((d, i) => [d.id, i]));
  const years = nodes.filter((d) => d.year).sort((a, b) => a.year - b.year || a.label.localeCompare(b.label));

  nodes.forEach((d, i) => {
    const a = (i / n) * Math.PI * 2;
    d.x = W / 2 + Math.cos(a) * (W * 0.3) * (0.55 + rand() * 0.75);
    d.y = H / 2 + Math.sin(a) * (H * 0.3) * (0.55 + rand() * 0.75);
    d.pin = false;
  });
  // the timeline: evenly spaced along the bottom, ordered by year
  years.forEach((d, i) => {
    d.x = PAD + ((i + 0.5) / years.length) * (W - 2 * PAD);
    d.y = H - 104;
    d.pin = true;
  });

  const area = (W - 2 * PAD) * (H - 2 * PAD);
  const k = Math.sqrt(area / Math.max(n, 1)) * 0.82;
  const iters = n > 220 ? 260 : n > 90 ? 420 : 600;
  let temp = W / 9;

  const dispX = new Float64Array(n), dispY = new Float64Array(n);
  for (let it = 0; it < iters; it++) {
    dispX.fill(0); dispY.fill(0);
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        let dx = nodes[i].x - nodes[j].x, dy = nodes[i].y - nodes[j].y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 0.01) { dx = (rand() - 0.5) * 0.6; dy = (rand() - 0.5) * 0.6; d2 = dx * dx + dy * dy + 0.01; }
        const d = Math.sqrt(d2), f = (k * k) / d;
        const ux = (dx / d) * f, uy = (dy / d) * f;
        dispX[i] += ux; dispY[i] += uy; dispX[j] -= ux; dispY[j] -= uy;
      }
    }
    for (const e of edges) {
      const i = idx.get(e.s), j = idx.get(e.o);
      if (i === undefined || j === undefined || i === j) continue;
      const dx = nodes[i].x - nodes[j].x, dy = nodes[i].y - nodes[j].y;
      const d = Math.hypot(dx, dy) || 0.01, f = (d * d) / k;
      const ux = (dx / d) * f, uy = (dy / d) * f;
      dispX[i] -= ux; dispY[i] -= uy; dispX[j] += ux; dispY[j] += uy;
    }
    for (let i = 0; i < n; i++) {
      const d = nodes[i];
      if (d.pin) continue;
      // a whisper of gravity, so detached fragments don't drift off the sheet
      dispX[i] += (W / 2 - d.x) * 0.012;
      dispY[i] += (H / 2 - d.y) * 0.012;
      const len = Math.hypot(dispX[i], dispY[i]) || 1;
      const step = Math.min(len, temp);
      d.x += (dispX[i] / len) * step;
      d.y += (dispY[i] / len) * step;
      d.x = Math.max(PAD, Math.min(W - PAD, d.x));
      d.y = Math.max(PAD, Math.min(H - PAD * 1.35, d.y));
    }
    temp *= 0.975;
  }

  /* Second pass: pull the LETTERING apart. The force layout only knows points,
   * so long names still sit on top of each other. Treat each node as the box its
   * text occupies and push overlapping boxes apart along the shallower axis.
   * This is what buys the drawing its air — Lombardi's sheets are mostly paper. */
  for (const d of nodes) d.box = boxOf(d);
  for (let it = 0; it < 240; it++) {
    let moved = 0;
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        const a = nodes[i], b = nodes[j];
        if (a.pin && b.pin) continue;
        const ox = (a.box.hw + b.box.hw + 7) - Math.abs(a.x - b.x);
        const oy = (a.box.hh + b.box.hh + 5) - Math.abs(a.y - b.y);
        if (ox <= 0 || oy <= 0) continue;         // boxes clear of each other
        moved++;
        // shove along whichever axis needs the smaller correction
        if (ox / (a.box.hw + b.box.hw) < oy / (a.box.hh + b.box.hh)) {
          const s = (a.x <= b.x ? -1 : 1) * ox * 0.5;
          if (!a.pin) a.x += s; if (!b.pin) b.x -= s;
        } else {
          const s = (a.y <= b.y ? -1 : 1) * oy * 0.5;
          if (!a.pin) a.y += s; if (!b.pin) b.y -= s;
        }
      }
    }
    if (!moved) break;
  }

  // keep every name whole on the sheet, margins measured from its own box
  for (const d of nodes) {
    if (d.pin) continue;
    d.x = Math.max(d.box.hw + 14, Math.min(W - d.box.hw - 14, d.x));
    d.y = Math.max(d.box.hh + 20, Math.min(H - 168, d.y));
  }
  return nodes;
}

/* ---------------------------------------------------------------- drawing */

/* A circular arc from (x1,y1) to (x2,y2) bulging by `bulge` of the chord.
 * This one function is most of the Lombardi look: he never drew a straight
 * line between two names. */
function arcPath(x1, y1, x2, y2, bulge, sweep) {
  const d = Math.hypot(x2 - x1, y2 - y1) || 1;
  const h = Math.max(bulge * d, 4);
  const r = (d * d / 4 + h * h) / (2 * h);
  return `M${x1.toFixed(1)} ${y1.toFixed(1)} A${r.toFixed(1)} ${r.toFixed(1)} 0 0 ${sweep} ${x2.toFixed(1)} ${y2.toFixed(1)}`;
}

const SVG_NS = "http://www.w3.org/2000/svg";
const mk = (name, attrs) => {
  const el = document.createElementNS(SVG_NS, name);
  for (const k in attrs) if (attrs[k] != null) el.setAttribute(k, attrs[k]);
  return el;
};

const DEFS = `
<defs>
  <!-- paper tooth -->
  <filter id="grain" x="0" y="0" width="100%" height="100%">
    <feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="4" seed="7" result="n"/>
    <feColorMatrix in="n" type="saturate" values="0"/>
    <feComponentTransfer><feFuncA type="linear" slope="0.055"/></feComponentTransfer>
  </filter>
  <!-- the wobble of a hand holding a pencil: displace every stroke slightly -->
  <filter id="ink" x="-12%" y="-12%" width="124%" height="124%">
    <feTurbulence type="fractalNoise" baseFrequency="0.017" numOctaves="3" seed="3" result="t"/>
    <feDisplacementMap in="SourceGraphic" in2="t" scale="2.4" xChannelSelector="R" yChannelSelector="G"/>
  </filter>
  <filter id="drop" x="-6%" y="-6%" width="112%" height="112%">
    <feDropShadow dx="0" dy="5" stdDeviation="11" flood-color="#4a3c20" flood-opacity="0.24"/>
  </filter>
  <radialGradient id="vig" cx="50%" cy="46%" r="76%">
    <stop offset="60%" stop-color="#000" stop-opacity="0"/>
    <stop offset="100%" stop-color="#5b4a28" stop-opacity="0.13"/>
  </radialGradient>
  <marker id="ah" viewBox="0 0 10 10" refX="9.4" refY="5" markerWidth="7" markerHeight="7"
          orient="auto-start-reverse" markerUnits="strokeWidth">
    <path d="M1 1.6 L9.2 5 L1 8.4" fill="none" stroke="#2f2a22" stroke-width="1.25"
          stroke-linecap="round" stroke-linejoin="round"/>
  </marker>
  <marker id="ah-r" viewBox="0 0 10 10" refX="9.4" refY="5" markerWidth="7" markerHeight="7"
          orient="auto-start-reverse" markerUnits="strokeWidth">
    <path d="M1 1.6 L9.2 5 L1 8.4" fill="none" stroke="#a4302a" stroke-width="1.25"
          stroke-linecap="round" stroke-linejoin="round"/>
  </marker>
  <!-- the double bar he closed a blocked or failed deal with -->
  <marker id="bar" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7"
          orient="auto-start-reverse" markerUnits="strokeWidth">
    <path d="M5.4 1 L5.4 9 M8.2 1 L8.2 9" fill="none" stroke="#2f2a22" stroke-width="1.2" stroke-linecap="round"/>
  </marker>
</defs>`;

const state = {
  works: [], work: null, nodes: [], edges: [], byId: new Map(),
  notation: new Map(),   // arc class IRI → {label, drawn}
  sel: null, view: { x: 0, y: 0, w: W, h: H },
  // arrange / trace mode:
  trace: false, original: null, image: null, bgOpacity: 0.68,
  arcEls: [],            // arc <path> elements, for live redraw while dragging
};

function drawSheet() {
  const svg = $("sheet");
  svg.innerHTML = DEFS;
  svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");
  state.view = { x: 0, y: 0, w: W, h: H };

  // A sheet of paper lying on a table. The table is drawn far outside the
  // viewBox so that whichever way the stage letterboxes, the view stays warm.
  const table = { x: -W, y: -H, width: W * 3, height: H * 3 };
  svg.appendChild(mk("rect", Object.assign({ fill: "#d9cdae" }, table)));
  svg.appendChild(mk("rect", Object.assign({ fill: "#7d6f4c", filter: "url(#grain)" }, table)));
  svg.appendChild(mk("rect", { x: 0, y: 0, width: W, height: H, fill: "#ece3ce", filter: "url(#drop)" }));
  // In trace mode the paper texture is skipped so the photograph reads clearly.
  if (!state.trace) {
    svg.appendChild(mk("rect", { x: 0, y: 0, width: W, height: H, fill: "#8a7a52", filter: "url(#grain)" }));
    svg.appendChild(mk("rect", { x: 0, y: 0, width: W, height: H, fill: "url(#vig)" }));
  }

  // Everything drawn in pencil sits in one group, tipped a fraction of a degree
  // the way a sheet lies on a table — but flat when tracing, so screen deltas map
  // cleanly to sheet coordinates and the exported positions are unrotated.
  const sheet = mk("g", state.trace ? { id: "pencil" }
                                     : { id: "pencil", transform: `rotate(-0.28 ${W / 2} ${H / 2})` });
  svg.appendChild(sheet);

  const gArcs = mk("g", { filter: "url(#ink)", fill: "none", "stroke-linecap": "round" });
  const gNodes = mk("g", {});
  sheet.appendChild(gArcs);
  sheet.appendChild(gNodes);
  refreshBackground();          // drops the original photo behind the names, if tracing

  // ---- the timeline rule, if this drawing has year markers
  const years = state.nodes.filter((d) => d.year);
  if (years.length > 1) {
    const y = Math.max(...years.map((d) => d.y));
    const x1 = Math.min(...years.map((d) => d.x)), x2 = Math.max(...years.map((d) => d.x));
    gArcs.appendChild(mk("path", {
      d: `M${x1 - 34} ${y} L${x2 + 34} ${y}`,
      stroke: "#2f2a22", "stroke-width": 0.7, "stroke-opacity": 0.36,
    }));
  }

  // ---- arcs
  const rand = rng(state.work.w + "|arcs");
  state.edges.forEach((e, i) => {
    const a = state.byId.get(e.s), b = state.byId.get(e.o);
    if (!a || !b) return;
    const m = markOf(e.type);
    const sweep = rand() < 0.5 ? 0 : 1;
    const bulge = 0.10 + rand() * 0.13;
    const p = mk("path", {
      d: arcPath(a.x, a.y, b.x, b.y, bulge, sweep),
      stroke: m.red ? "#a4302a" : "#2f2a22",
      // pencil pressure varies; nothing on his sheets is mechanically uniform
      "stroke-width": (0.95 + rand() * 0.5).toFixed(2),
      "stroke-opacity": (m.red ? 0.8 : 0.66 + rand() * 0.16).toFixed(2),
      "stroke-dasharray": m.dash,
    });
    if (m.tip === "arrow") p.setAttribute("marker-end", m.red ? "url(#ah-r)" : "url(#ah)");
    if (m.tip === "both") { p.setAttribute("marker-end", "url(#ah)"); p.setAttribute("marker-start", "url(#ah)"); }
    if (m.tip === "bar") p.setAttribute("marker-end", "url(#bar)");
    p.dataset.s = e.s; p.dataset.o = e.o;
    p.dataset.bulge = bulge.toFixed(3); p.dataset.sweep = sweep;   // for live redraw while dragging
    p.appendChild(mk("title", {})).textContent = `${a.label} → ${b.label} · ${e.type} (${e.drawn})${e.amount ? " · " + e.amount : ""}`;
    gArcs.appendChild(p);
  });
  state.arcEls = [...gArcs.querySelectorAll("path[data-s]")];

  // ---- names
  for (const d of state.nodes) {
    const g = mk("g", { class: "node", transform: `translate(${d.x.toFixed(1)},${d.y.toFixed(1)})` });
    g.style.cursor = "pointer";
    g.dataset.id = d.id;

    if (d.year) {
      // a two-digit year in a small circle, as he ringed them
      g.appendChild(mk("circle", { r: 12.5, fill: "#ece3ce", stroke: "#2f2a22",
        "stroke-width": 0.9, "stroke-opacity": 0.62, filter: "url(#ink)" }));
      const t = mk("text", { "text-anchor": "middle", y: 3.6, "font-size": 10.5,
        fill: "#2f2a22", "fill-opacity": 0.85,
        "font-family": "ui-sans-serif, system-ui, sans-serif" });
      t.textContent = String(d.year).slice(2);
      g.appendChild(t);
    } else {
      const isFinal = d.kind === "FinalInfo" || d.kind === "Final";
      const label = d.label.length > 30 ? d.label.slice(0, 29) + "…" : d.label;
      const t = mk("text", {
        "text-anchor": "middle", y: 4,
        "font-size": isFinal ? 8.6 : (d.wc > 1 ? 10.4 : 9.6),
        "font-family": "ui-sans-serif, 'Avenir Next', 'Segoe UI', system-ui, sans-serif",
        "letter-spacing": ".055em",
        "font-weight": d.kind === "Institution" ? 600 : 400,
        "font-style": isFinal ? "italic" : null,
        fill: isFinal ? "#a4302a" : "#2f2a22",
        "fill-opacity": isFinal ? 0.86 : 0.92,
        // the ink stops where the lettering is — he drew around his own words
        stroke: "#ece3ce", "stroke-width": 3.2, "paint-order": "stroke",
        "stroke-linejoin": "round",
      });
      t.textContent = isFinal ? label : label.toUpperCase();
      g.appendChild(t);
      // institutions carry a fine rule beneath the name
      if (d.kind === "Institution") {
        const w = label.length * (d.wc > 1 ? 6.0 : 5.6) * 0.5;
        g.appendChild(mk("path", { d: `M${-w} 8.4 L${w} 8.4`, stroke: "#2f2a22",
          "stroke-width": 0.6, "stroke-opacity": 0.4, filter: "url(#ink)" }));
      }
    }
    g.appendChild(mk("title", {})).textContent =
      `${d.label} — ${d.kind}${d.wc > 1 ? ` · appears in ${d.wc} drawings` : ""}`;
    if (state.trace) {
      makeNodeDraggable(g, d);          // drag names onto the drawing behind them
    } else {
      g.style.cursor = "pointer";
      g.addEventListener("click", (ev) => { ev.stopPropagation(); selectNode(d.id, g); });
      g.addEventListener("mouseenter", () => highlight(d.id));
      g.addEventListener("mouseleave", () => highlight(state.sel));
    }
    gNodes.appendChild(g);
  }

  // ---- the title block, bottom left, the way he titled a sheet in pencil
  const tb = mk("g", { transform: `translate(34, ${H - 34})`, filter: "url(#ink)" });
  const title = mk("text", { "font-size": 15, "letter-spacing": ".13em", fill: "#2f2a22",
    "fill-opacity": .82, "font-family": "ui-sans-serif, system-ui, sans-serif" });
  title.textContent = state.work.title.toUpperCase();
  tb.appendChild(title);
  const sub = mk("text", { y: 15, "font-size": 9.4, "letter-spacing": ".07em", fill: "#2f2a22",
    "fill-opacity": .5, "font-family": "ui-sans-serif, system-ui, sans-serif" });
  sub.textContent = `AFTER MARK LOMBARDI · ${state.nodes.length} NODES · ${state.edges.length} ARCS` +
    (state.work.from ? ` · NARRATES ${state.work.from}–${state.work.to}` : "");
  tb.appendChild(sub);
  sheet.appendChild(tb);

  highlight(null);
}

/* Hovering or selecting a name lifts its own arcs out of the weave. */
function highlight(id) {
  const arcs = $("sheet").querySelectorAll("path[data-s]");
  arcs.forEach((p) => {
    if (!id) { p.style.opacity = ""; return; }
    const on = p.dataset.s === id || p.dataset.o === id;
    p.style.opacity = on ? "1" : "0.17";
    if (on) p.style.strokeWidth = "1.8";
    else p.style.strokeWidth = "";
  });
  $("sheet").querySelectorAll("g.node").forEach((g) => {
    g.style.opacity = !id ? "" : (g.dataset.id === id ? "1" : "0.42");
  });
}

/* --------------------------------------------------- arrange / trace mode
 * The names can't be placed where Lombardi placed them — those coordinates were
 * never digitized. So this mode hands the job to a person: it drops the original
 * drawing in behind the names and lets you drag each one onto its counterpart,
 * then download the coordinates you traced. That JSON is exactly the (x,y) layer
 * Tolksdorf's data is missing — feed a folder of them back and the redrawing
 * becomes the real drawing. */

function refreshBackground() {
  const sheet = document.getElementById("pencil");
  if (!sheet) return;
  const old = sheet.querySelector("image.bg");
  if (old) old.remove();
  if (!(state.trace && state.image)) return;
  const img = mk("image", { x: 0, y: 0, width: W, height: H,
    preserveAspectRatio: "none", opacity: state.bgOpacity });
  img.setAttribute("class", "bg");
  img.setAttribute("href", state.image);
  img.setAttributeNS("http://www.w3.org/1999/xlink", "xlink:href", state.image);
  sheet.insertBefore(img, sheet.firstChild);      // behind the arcs and names
}

function redrawIncidentArcs(id) {
  for (const p of state.arcEls) {
    if (p.dataset.s === id || p.dataset.o === id) {
      const a = state.byId.get(p.dataset.s), b = state.byId.get(p.dataset.o);
      if (a && b) p.setAttribute("d", arcPath(a.x, a.y, b.x, b.y, +p.dataset.bulge, +p.dataset.sweep));
    }
  }
}

function makeNodeDraggable(g, d) {
  g.style.cursor = "move";
  let drag = null;
  g.addEventListener("pointerdown", (e) => {
    e.stopPropagation();
    const rect = $("sheet").getBoundingClientRect();
    drag = { px: e.clientX, py: e.clientY, moved: 0,
             sx: state.view.w / rect.width, sy: state.view.h / rect.height };
    g.setPointerCapture(e.pointerId);
    g.parentNode.appendChild(g);          // raise the name being moved
  });
  g.addEventListener("pointermove", (e) => {
    if (!drag) return;
    const mvx = e.clientX - drag.px, mvy = e.clientY - drag.py;
    d.x += mvx * drag.sx; d.y += mvy * drag.sy;
    drag.moved += Math.hypot(mvx, mvy);
    drag.px = e.clientX; drag.py = e.clientY;
    g.setAttribute("transform", `translate(${d.x.toFixed(1)},${d.y.toFixed(1)})`);
    redrawIncidentArcs(d.id);
  });
  const end = () => {
    if (!drag) return;
    if (drag.moved < 4) selectNode(d.id, g);   // a tap inspects; a drag moves
    else savePositions();
    drag = null;
  };
  g.addEventListener("pointerup", end);
  g.addEventListener("pointercancel", end);
}

const posKey = () => "lombardi:pos:" + (state.work ? localName(state.work.w) : "");

function savePositions() {
  try {
    const pos = {};
    for (const d of state.nodes) pos[d.id] = [Math.round(d.x), Math.round(d.y)];
    localStorage.setItem(posKey(), JSON.stringify({ w: W, h: H, pos }));
  } catch (_) { /* private mode / quota — the download still works */ }
}

function loadPositions() {
  try {
    const raw = localStorage.getItem(posKey());
    if (!raw) return false;
    const s = JSON.parse(raw), m = s.pos || s, sw = s.w || W, sh = s.h || H;
    let any = false;
    for (const d of state.nodes) if (m[d.id]) {
      d.x = m[d.id][0] * (W / sw); d.y = m[d.id][1] * (H / sh); any = true;
    }
    return any;
  } catch (_) { return false; }
}

function downloadPositions() {
  if (!state.work) return;
  const data = {
    dataset: "lombardi", work: state.work.w, networkId: localName(state.work.w),
    title: state.work.title,
    sheet: { width: W, height: H, note: "origin top-left; x in [0,width], y in [0,height]" },
    original: state.original ? {
      moma: state.original.page, image: state.image,
      dimensions: state.original.dims, accession: state.original.accession,
    } : null,
    rights: "Node coordinates only. Any MoMA image referenced is © The Estate of "
      + "Mark Lombardi, linked not redistributed, and not covered by CC BY-NC-SA.",
    tracedIn: "docs/lombardi.html",
    nodes: state.nodes.map((d) => Object.assign(
      { id: d.id, name: d.label, type: d.kind, x: Math.round(d.x), y: Math.round(d.y) },
      d.year ? { year: d.year } : {})),
  };
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }));
  a.download = `lombardi-${localName(state.work.w)}-positions.json`;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  flash(`downloaded ${state.nodes.length} positions`);
}

function importPositions(file) {
  const reader = new FileReader();
  reader.onload = () => {
    try {
      const data = JSON.parse(reader.result);
      const by = new Map((data.nodes || []).map((n) => [n.id, n]));
      let any = false;
      for (const d of state.nodes) {
        const n = by.get(d.id);
        if (n && isFinite(n.x) && isFinite(n.y)) { d.x = +n.x; d.y = +n.y; any = true; }
      }
      if (!any) return flash("no matching names in that file");
      drawSheet(); savePositions();
      flash(`loaded ${(data.nodes || []).length} positions`);
    } catch (_) { flash("could not read that JSON"); }
  };
  reader.readAsText(file);
}

function updateArrangeAvailability() {
  const has = !!state.image;
  const op = $("opacity"); if (op) op.disabled = !has;
  const nobg = $("nobg"); if (nobg) nobg.hidden = !(state.trace && !has);
}

function setArrange(on) {
  state.trace = on;
  $("arrange").classList.toggle("on", on);
  $("arrangebar").hidden = !on;
  hideCard();
  drawSheet();
  $("note-l").textContent = on
    ? "arrange · drag each name onto the drawing behind it, then ↓ JSON"
    : `${state.nodes.length} names · ${state.edges.length} arcs · click any name`;
  updateArrangeAvailability();
}

function flash(msg) {
  const el = $("note-l"); const prev = el.dataset.base || el.textContent;
  el.dataset.base = prev; el.textContent = msg;
  clearTimeout(flash._t);
  flash._t = setTimeout(() => { el.textContent = el.dataset.base || ""; }, 2600);
}

/* --------------------------------------------------------------- the card */

function hideCard() {
  state.sel = null;
  $("indexcard").classList.add("gone");
  highlight(null);
}

/* Drop the index card near the name that was clicked, then clamp it onto the
 * stage. Called only when a new name is clicked (anchor given) or the card was
 * closed — a click WITHIN the card leaves it where the reader dragged it. */
function positionCard(anchorEl) {
  const card = $("indexcard");
  const stage = $("stage").getBoundingClientRect();
  const cw = 312, ch = Math.min(stage.height * 0.74, 430);
  let cx, cy;
  if (anchorEl && anchorEl.getBoundingClientRect) {
    const r = anchorEl.getBoundingClientRect();
    const nx = r.left + r.width / 2 - stage.left, ny = r.top + r.height / 2 - stage.top;
    cx = nx + 26;
    if (cx + cw > stage.width - 12) cx = nx - cw - 26;   // flip to the other side
    cy = ny - 46;
  } else {
    cx = stage.width - cw - 26; cy = 58;
  }
  card.style.left = Math.max(12, Math.min(stage.width - cw - 12, cx)) + "px";
  card.style.top = Math.max(12, Math.min(Math.max(12, stage.height - ch - 12), cy)) + "px";
}

/* Make the card draggable by its head, and closable. Bound once per open. */
function bindCardChrome() {
  const card = $("indexcard");
  card.querySelector(".ic-x").onclick = () => hideCard();
  const head = card.querySelector(".ic-head");
  let drag = null;
  head.onpointerdown = (e) => {
    if (e.target.closest(".ic-x")) return;
    drag = { x: e.clientX, y: e.clientY,
             l: parseFloat(card.style.left) || 0, t: parseFloat(card.style.top) || 0 };
    card.classList.add("drag"); head.setPointerCapture(e.pointerId);
  };
  head.onpointermove = (e) => {
    if (!drag) return;
    card.style.left = (drag.l + e.clientX - drag.x) + "px";
    card.style.top = (drag.t + e.clientY - drag.y) + "px";
  };
  const stop = () => { drag = null; card.classList.remove("drag"); };
  head.onpointerup = stop; head.onpointercancel = stop;
}

async function selectNode(id, anchorEl) {
  const d = state.byId.get(id);
  if (!d) return;
  const card = $("indexcard");
  const reposition = anchorEl || card.classList.contains("gone");
  state.sel = id;
  highlight(id);

  card.innerHTML =
    `<button class="ic-x" title="Close (Esc)">×</button>` +
    `<div class="ic-head"><p class="nm">${esc(d.label)}</p>` +
    `<div class="kind">${esc(d.kind)}${d.year ? " · " + d.year : ""}</div></div>` +
    `<div id="cardbody"><div class="card-empty" style="padding:8px 0">reading the graph…</div></div>`;
  if (reposition) positionCard(anchorEl);
  card.classList.remove("gone");
  bindCardChrome();

  let out, inc, also;
  try {
    [out, inc, also] = await Promise.all([
      engine.ask(`arcs out of · ${d.label}`, Q.arcsFrom(id)),
      engine.ask(`arcs into · ${d.label}`, Q.arcsTo(id)),
      d.wc > 1 ? engine.ask(`also drawn in · ${d.label}`, Q.alsoIn(id)) : Promise.resolve([]),
    ]);
  } catch (err) {
    $("cardbody").innerHTML = `<div class="card-empty">The card would not come out of the
      graph: ${esc(err.message || err)}</div>`;
    return;
  }
  if (state.sel !== id) return;   // a faster click overtook this one

  const here = (r) => r.w === state.work.w;
  const notate = (t) => state.notation.get(t) || { label: localName(t), drawn: "an arc" };

  const relHtml = (list, arrow) => list.length ? `<ul class="rel">` + list.map((r) => {
    const known = state.byId.has(r.other);
    const n = notate(r.t);
    return `<li>` +
      `<span class="dirmark">${arrow}</span> ` +
      `<span class="${known ? "who" : ""}"${known ? ` data-go="${esc(r.other)}"` : ""}>${esc(r.label)}</span>` +
      (r.amount ? ` <span class="amt">${esc(r.amount)}</span>` : "") +
      `<span class="how">${esc(n.label)} — ${esc(n.drawn)}</span></li>`;
  }).join("") + `</ul>` : "";

  const outHere = out.filter(here), incHere = inc.filter(here);
  const elsewhere = [...out.filter((r) => !here(r)), ...inc.filter((r) => !here(r))];

  $("cardbody").innerHTML = `
    <div class="stat">
      <div><b>${outHere.length}</b><i>reaches out</i></div>
      <div><b>${incHere.length}</b><i>reached by</i></div>
      <div><b>${d.wc}</b><i>drawing${d.wc === 1 ? "" : "s"}</i></div>
    </div>
    ${outHere.length ? `<h2 style="margin:15px 0 2px" class="kind">Reaches out to</h2>${relHtml(outHere, "→")}` : ""}
    ${incHere.length ? `<h2 style="margin:15px 0 2px" class="kind">Reached by</h2>${relHtml(incHere, "←")}` : ""}
    ${!outHere.length && !incHere.length
      ? `<div class="card-empty" style="padding:8px 0">Drawn on this sheet with no arc of its own.</div>` : ""}
    ${elsewhere.length ? `<h2 style="margin:16px 0 2px" class="kind">And on other sheets</h2>
      ${relHtml(elsewhere, "·")}` : ""}
    ${also.length > 1 ? `<h2 style="margin:16px 0 2px" class="kind">Also drawn in</h2>
      <ul class="xref">${also.filter((a) => a.w !== state.work.w)
        .map((a) => `<li data-work="${esc(a.w)}">${esc(a.title)}</li>`).join("")}</ul>
      <div class="foot" style="padding:9px 0 0">Lombardi drew this same actor on
      another sheet, and Tolksdorf gave it the same id — which is what makes the
      51 drawings one graph rather than 51.</div>` : ""}`;

  // clicking a name in the card re-reads that node's card, IN PLACE (no anchor)
  $("cardbody").querySelectorAll("[data-go]").forEach((el) =>
    el.addEventListener("click", () => selectNode(el.dataset.go)));
  $("cardbody").querySelectorAll("[data-work]").forEach((el) =>
    el.addEventListener("click", () => openWork(el.dataset.work, d.id)));
}

/* --------------------------------------------------------- the original
 * A photograph of the actual sheet, pinned beside the redrawing — for the 17
 * works MoMA holds. The image is loaded straight from moma.org and never copied
 * here; the caption carries the credit and the rights line from the graph. */
async function paintOriginal(iri) {
  const box = $("original");
  box.hidden = true;
  box.innerHTML = "";
  state.original = null;
  state.image = null;
  updateArrangeAvailability();
  let rows;
  try {
    rows = await engine.ask("the original object", Q.object(iri));
  } catch (err) { return; }
  if (!rows.length || state.work.w !== iri) return;
  const r = rows[0];
  state.original = r;
  state.image = r.full || r.img;       // 2000px for tracing/zoom; 1024 as fallback
  updateArrangeAvailability();
  const cap = [r.date, r.medium, r.dims].filter(Boolean).join(" · ");
  box.innerHTML =
    `<button class="plate-open" title="See the original, larger">` +
    `<img src="${esc(r.img)}" alt="Photograph of the original drawing, courtesy MoMA"></button>` +
    `<div class="cap"><b>The original sheet</b>${cap ? " — " + esc(cap) : ""}` +
    (r.credit ? `<span class="cr">${esc(r.credit)}</span>` : "") +
    `<span class="cr">MoMA ${esc(r.accession)} · artwork © The Estate of Mark Lombardi, ` +
    `image courtesy MoMA — <a href="${esc(r.page)}" target="_blank" rel="noopener">see it there</a></span></div>`;
  box.hidden = false;
  // click the plate to study the real arrangement full-size
  box.querySelector(".plate-open").addEventListener("click", () => openLightbox(r));
  // if MoMA ever stops serving it, take the plate away rather than leave a blank frame
  box.querySelector("img").addEventListener("error", () => { box.hidden = true; });
  // if we're already tracing, drop the (now-known) image in behind the names
  if (state.trace) refreshBackground();
}

/* The photograph, full-size — so the actual arrangement of Lombardi's hand is
 * there to read, even though the redrawing beside it can only echo the topology. */
function openLightbox(r) {
  const lb = $("lightbox");
  const cap = [r.date, r.medium, r.dims].filter(Boolean).join(" · ");
  lb.innerHTML =
    `<img src="${esc(r.full || r.img)}" alt="The original drawing by Mark Lombardi, courtesy MoMA">` +
    `<div class="lb-cap">${esc(cap)} · MoMA ${esc(r.accession)} — artwork © The Estate of ` +
    `Mark Lombardi, image courtesy MoMA · ` +
    `<a href="${esc(r.page)}" target="_blank" rel="noopener">see it at MoMA ↗</a></div>`;
  lb.classList.remove("gone");
  lb.onclick = (e) => { if (!e.target.closest("a")) lb.classList.add("gone"); };
}

/* ------------------------------------------------------------ the legend */

function paintLegend() {
  const present = [...new Set(state.edges.map((e) => e.type))];
  const drawn = new Map(state.edges.map((e) => [e.type, e.drawn]));
  $("legend").innerHTML = present.sort().map((t) => {
    const m = markOf(t);
    const stroke = m.red ? "#a4302a" : "#2f2a22";
    const tip = m.tip === "arrow" ? ` marker-end="url(#lah)"` : "";
    return `<li><svg width="46" height="14" viewBox="0 0 46 14">
        <defs><marker id="lah" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6"
          orient="auto-start-reverse"><path d="M1 2 L9 5 L1 8" fill="none" stroke="${stroke}" stroke-width="1.4"/></marker></defs>
        <path d="M2 11 Q23 -1 44 8" fill="none" stroke="${stroke}" stroke-width="1.2"
          ${m.dash ? `stroke-dasharray="${m.dash}"` : ""}${tip}/>
      </svg><span><b>${esc(t)}</b> — ${esc(drawn.get(t) || "")}</span></li>`;
  }).join("");
}

/* ---------------------------------------------------------- open a drawing */

async function openWork(iri, focusNode) {
  const w = state.works.find((x) => x.w === iri);
  if (!w) return;
  state.work = w;
  state.image = null; state.original = null;
  hideCard();
  $("curtain").classList.remove("gone");
  $("curtain-s").textContent = w.title;
  $("bar").firstElementChild.style.width = "22%";
  [...$("works").children].forEach((li) => li.classList.toggle("on", li.dataset.w === iri));

  const [nodes, arcs] = await Promise.all([
    engine.ask(`nodes · ${w.title.slice(0, 34)}`, Q.nodes(iri)),
    engine.ask(`arcs · ${w.title.slice(0, 34)}`, Q.arcs(iri)),
  ]);
  $("bar").firstElementChild.style.width = "68%";

  state.nodes = nodes.map((r) => ({
    id: r.n, label: r.label, kind: localName(r.type),
    wc: parseInt(r.wc, 10) || 1, year: r.year ? parseInt(r.year, 10) : null,
  }));
  state.byId = new Map(state.nodes.map((d) => [d.id, d]));
  state.edges = arcs.map((r) => ({ s: r.s, o: r.o, type: r.type, drawn: r.drawn, amount: r.amount }));

  layout(state.nodes, state.edges, iri, aspectFromDims(w.dims));
  loadPositions();          // bring back a saved tracing for this work, if any
  drawSheet();
  paintLegend();
  const prop = w.dims ? " · sheet to scale" : "";
  $("note-l").textContent =
    `${state.nodes.length} names · ${state.edges.length} arcs${prop} · click any name`;
  paintOriginal(iri);
  $("bar").firstElementChild.style.width = "100%";
  setTimeout(() => $("curtain").classList.add("gone"), 130);
  if (focusNode && state.byId.has(focusNode)) {
    const g = document.querySelector(`#sheet g.node[data-id="${CSS.escape(focusNode)}"]`);
    selectNode(focusNode, g);
  }
}

/* --------------------------------------------------------------- the list */

function paintWorks(filter) {
  const f = (filter || "").trim().toLowerCase();
  const list = state.works.filter((w) => !f || w.title.toLowerCase().includes(f));
  $("works").innerHTML = list.map((w) =>
    `<li data-w="${esc(w.w)}"${state.work && w.w === state.work.w ? ' class="on"' : ""}>${esc(w.title)}
      <span class="m">${w.nodes} nodes · ${w.arcs} arcs${w.from ? ` · ${w.from}–${w.to}` : ""}</span></li>`
  ).join("") || `<li style="color:#8b8271">nothing matches</li>`;
  [...$("works").children].forEach((li) =>
    li.dataset.w && li.addEventListener("click", () => openWork(li.dataset.w)));
}

/* ------------------------------------------------------------- pan & zoom */

function wireStage() {
  const svg = $("sheet");
  const apply = () => svg.setAttribute("viewBox",
    `${state.view.x} ${state.view.y} ${state.view.w} ${state.view.h}`);
  const zoom = (f, cx = state.view.x + state.view.w / 2, cy = state.view.y + state.view.h / 2) => {
    const w = Math.max(W * 0.12, Math.min(W * 1.6, state.view.w * f));
    const h = w * (H / W);
    state.view.x = cx - (cx - state.view.x) * (w / state.view.w);
    state.view.y = cy - (cy - state.view.y) * (h / state.view.h);
    state.view.w = w; state.view.h = h;
    apply();
  };
  $("zin").onclick = () => zoom(0.8);
  $("zout").onclick = () => zoom(1.25);
  $("zfit").onclick = () => { state.view = { x: 0, y: 0, w: W, h: H }; apply(); };

  svg.addEventListener("wheel", (e) => {
    e.preventDefault();
    const r = svg.getBoundingClientRect();
    const cx = state.view.x + ((e.clientX - r.left) / r.width) * state.view.w;
    const cy = state.view.y + ((e.clientY - r.top) / r.height) * state.view.h;
    zoom(e.deltaY > 0 ? 1.12 : 0.89, cx, cy);
  }, { passive: false });

  let drag = null;
  svg.addEventListener("pointerdown", (e) => {
    drag = { x: e.clientX, y: e.clientY, vx: state.view.x, vy: state.view.y };
    svg.classList.add("drag"); svg.setPointerCapture(e.pointerId);
  });
  svg.addEventListener("pointermove", (e) => {
    if (!drag) return;
    const r = svg.getBoundingClientRect();
    state.view.x = drag.vx - ((e.clientX - drag.x) / r.width) * state.view.w;
    state.view.y = drag.vy - ((e.clientY - drag.y) / r.height) * state.view.h;
    apply();
  });
  const stop = () => { drag = null; svg.classList.remove("drag"); };
  svg.addEventListener("pointerup", stop);
  svg.addEventListener("pointercancel", stop);
  svg.addEventListener("click", (e) => { if (e.target === svg) hideCard(); });
}

/* ------------------------------------------------------------------- boot */

(async function main() {
  $("stamp").textContent = BUILD_STAMP;
  wireStage();
  $("search").addEventListener("input", (e) => paintWorks(e.target.value));
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    const lb = $("lightbox");
    if (!lb.classList.contains("gone")) { lb.classList.add("gone"); return; }
    if (!$("indexcard").classList.contains("gone")) hideCard();
  });

  // arrange / trace toolbar
  $("arrange").onclick = () => setArrange(!state.trace);
  $("opacity").oninput = (e) => {
    state.bgOpacity = e.target.value / 100;
    const img = document.querySelector("#pencil image.bg");
    if (img) img.setAttribute("opacity", state.bgOpacity);
  };
  $("dljson").onclick = downloadPositions;
  $("resetpos").onclick = () => {
    try { localStorage.removeItem(posKey()); } catch (_) {}
    layout(state.nodes, state.edges, state.work.w, aspectFromDims(state.work.dims));
    drawSheet(); flash("layout reset");
  };
  $("ldjson").onchange = (e) => { if (e.target.files[0]) importPositions(e.target.files[0]); e.target.value = ""; };

  try {
    await engine.boot();
    $("bar").firstElementChild.style.width = "12%";
    await engine.open();
    for (const r of await engine.ask("the notation table", Q.notation)) {
      state.notation.set(r.t, { label: r.label, drawn: r.drawn });
    }
    state.works = (await engine.ask("the 51 drawings", Q.works))
      .map((r) => ({ w: r.w, title: r.title, nodes: +r.nodes, arcs: +r.arcs, from: r.from, to: r.to, dims: r.dims }));
    paintWorks("");
    // open on the Nugan Hand Bank: a CIA-tied merchant bank that collapsed in
    // 1980, small enough to read whole and dense enough to show the notation
    const first = state.works.find((w) => /Nugan Hand/i.test(w.title)) || state.works[0];
    await openWork(first.w);
  } catch (err) {
    $("curtain").innerHTML =
      `<div><div class="t">The sheet stayed blank.</div>
       <div class="s">${esc(err.message || err)}</div></div>`;
  }
})();
