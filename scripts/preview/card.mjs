// The shared model + markup behind every social preview.
//
// One model, two renderings:
//   ogHtml(model)   — a standalone 1200x630 document, screenshotted to a PNG that
//                     becomes og:image (render_cards.mjs).
//   pageHtml(model) — the shareable landing page that carries the Open Graph
//                     tags and forwards a human to the live playground
//                     (build_pages.mjs).
//
// Keeping both in one module is the point: what a crawler unfurls and what a
// visitor lands on are then guaranteed to describe the same thing.
import fs from "node:fs";
import path from "node:path";

// ---------------------------------------------------------------- palette ----
// The docs-site / playground tokens (crates/docgen/src/main.rs, web/playground-src/styles.css).
export const T = {
  ink: "#17211d", muted: "#66746e", paper: "#f6f8f7", surface: "#ffffff",
  side: "#eef4ef", line: "#d9e2de", lineStrong: "#aebfb8",
  accent: "#147d69", accentDark: "#0b4f42", accent2: "#c84f2f", amber: "#b98112",
  code: "#eef3f1", codeBorder: "#cfd9d5", tint: "#eef6f2", tintStrong: "#e3f0ec",
  onAccent: "#ffffff",
  // Georgia never resolves in the render image; Charter (which does) is the
  // sturdy book serif the design falls back to, so cards render identically
  // everywhere they are built.
  serif: 'Georgia, "Bitstream Charter", Charter, "Liberation Serif", serif',
  mono: '"Cascadia Mono", "Liberation Mono", ui-monospace, SFMono-Regular, Menlo, monospace',
  sans: '"Liberation Sans", -apple-system, "Segoe UI", Helvetica, Arial, sans-serif',
};

export const OG_W = 1200, OG_H = 630;
export const DEFAULT_BASE = "https://caviri.github.io/rete/";

export function esc(s) {
  return String(s == null ? "" : s)
    .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

/** Catalog descriptions are rich HTML; social text has to be plain. */
export function plain(s, max = 0) {
  let out = String(s || "")
    .replace(/<br\s*\/?>/gi, " ")
    .replace(/<[^>]+>/g, "")
    .replace(/&nbsp;/g, " ").replace(/&amp;/g, "&").replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'")
    .replace(/\s+/g, " ")
    .trim();
  if (max && out.length > max) {
    // Cut on a word boundary, and prefer ending on a sentence when one is close.
    const cut = out.slice(0, max);
    const stop = cut.lastIndexOf(". ");
    out = (stop > max * 0.55 ? cut.slice(0, stop + 1) : cut.replace(/\s+\S*$/, "") + "…");
  }
  return out;
}

/** `http://ex/causes` → `causes`; long literals get an ellipsis. */
export function shortValue(text, max = 34) {
  let s = String(text || "").trim();
  if (/^https?:\/\/\S+$/.test(s) && !/\s/.test(s)) {
    const tail = s.replace(/[/#]$/, "").split(/[#/]/).filter(Boolean).pop();
    const host = (s.match(/^https?:\/\/([^/]+)/) || [])[1];
    s = tail && tail !== host ? tail : (host || s);
    try { s = decodeURIComponent(s); } catch { /* keep the raw form */ }
  }
  s = s.replace(/^"|"$/g, "");
  return s.length > max ? s.slice(0, max - 1).trimEnd() + "…" : s;
}

export const isNumeric = (text) => /^-?[\d,]+(\.\d+)?$/.test(String(text || "").trim());

/** A compact headline for the answer: "20 rows", "16 nodes, 30 edges", "3.2 s". */
export function answerHeadline(answer) {
  if (!answer) return "";
  if (answer.count != null) {
    const unit = answer.unit === "row" ? "row" : (answer.unit || "row");
    return `${answer.count.toLocaleString("en-US")} ${unit}${answer.count === 1 ? "" : "s"}`;
  }
  // Non-tabular views state their own shape ("graph: 16 nodes, 30 edges | 11 ms").
  const head = String(answer.qmeta || "").split("|")[0].trim();
  return head.replace(/^graph:\s*/, "");
}

/** The timing `#qmeta` ends with, humanized (43131 ms reads worse than 43.1 s). */
export function answerTiming(answer) {
  const all = [...String((answer && answer.qmeta) || "").matchAll(/([\d][\d.,]*)\s*(ms|s|min)\b/g)];
  if (!all.length) return "";
  const [, raw, unit] = all[all.length - 1];
  const value = Number(raw.replace(/,/g, ""));
  if (!Number.isFinite(value)) return "";
  if (unit === "ms" && value >= 1000) return `${(value / 1000).toFixed(1)} s`;
  if (unit === "ms") return `${value < 10 ? value.toFixed(1) : Math.round(value)} ms`;
  return `${raw} ${unit}`;
}

/**
 * The range-read cost, when the run actually paid one: rete's whole claim is
 * "answered N MB of a G-sized file", so the card states the measured numbers.
 * A later example in the same sweep can be served entirely from the session
 * cache ("0 B ... served from cache") — there is no honest network figure to
 * show for those, so they report nothing.
 */
export function answerNetwork(answer) {
  const qmeta = String((answer && answer.qmeta) || "");
  const bytes = /([\d.,]+\s*[KMGT]?B)\s+of\s+([\d.,]+\s*[KMGT]?B)\s+fetched/.exec(qmeta);
  if (!bytes || /^0(\.0+)?\s*B$/i.test(bytes[1].trim())) return null;
  const requests = /(\d[\d,]*)\s+range req/.exec(qmeta);
  return {
    fetched: bytes[1].replace(/\s+/g, " ").trim(),
    total: bytes[2].replace(/\s+/g, " ").trim(),
    requests: requests ? Number(requests[1].replace(/,/g, "")) : null,
  };
}

// ------------------------------------------------------------- the model ----

export function loadCatalog(root) {
  const src = fs.readFileSync(path.join(root, "web", "playground-src", "catalog.js"), "utf8");
  const w = {};
  new Function("window", src)(w);
  return w.RETE_PLAYGROUND_CATALOG;
}

export function loadAnswers(root) {
  const file = path.join(root, "web", "preview", "answers.json");
  if (!fs.existsSync(file)) return {};
  return JSON.parse(fs.readFileSync(file, "utf8")).answers || {};
}

export const exampleSlug = (dataset, index) => `${dataset}-${index}`;

/** The playground deep link a share page forwards to — the URL users already share. */
export function playgroundHash(dataset, kind, index) {
  const params = new URLSearchParams({ dataset });
  if (kind === "remote-lazy") params.set("load", "lazy");
  params.set("mode", "sparql");
  if (index != null) params.set("ex", String(index));
  return `playground.html#${params}`;
}

/** Dataset facts shared by every card: icon, scale, how it loads. */
function datasetModel(catalog, key) {
  const dataset = catalog.datasets.find((d) => d.key === key) || { key };
  const meta = (catalog.datasetMeta || {})[key] || {};
  const extra = (catalog.datasetExtra || {})[key] || {};
  // The catalog label is "<file> — <what it is>"; the tail is the human name,
  // written to follow the dash, so it needs a capital to stand as a title.
  const label = plain(dataset.label || key);
  const name = label
    .replace(/^\S+\.rete\s*[—-]\s*/, "")
    .replace(/\s*\((remote|embedded)[^)]*\)\s*$/i, "")
    .replace(/^\p{Ll}/u, (c) => c.toUpperCase());
  return {
    key,
    icon: extra.icon || "◆",
    tags: extra.tags || [],
    name: name || key,
    label,
    description: plain(dataset.description || "", 0),
    triples: meta.triples || "",
    size: meta.size || "",
    license: plain(meta.license || "").split(/[;(]/)[0].trim(),
    remote: dataset.kind === "remote-lazy",
    kind: dataset.kind,
  };
}

export function buildModels(root, { base = DEFAULT_BASE } = {}) {
  const catalog = loadCatalog(root);
  const answers = loadAnswers(root);
  const models = [];

  for (const [key, examples] of Object.entries(catalog.examples)) {
    const dataset = datasetModel(catalog, key);
    examples.forEach((example, index) => {
      const answer = answers[`${key}:${index}`] || null;
      models.push({
        kind: "example",
        slug: exampleSlug(key, index),
        dir: "q",
        title: plain(example.label || `Example ${index + 1}`),
        family: example.family || "",
        view: example.view || "table",
        tip: plain(example.tip || ""),
        query: example.q || "",
        dataset,
        answer: answer && answer.ok ? answer : null,
        target: playgroundHash(key, dataset.kind, index),
        base,
      });
    });
    models.push({
      kind: "dataset",
      slug: key,
      dir: "d",
      title: dataset.name,
      family: "",
      view: "table",
      tip: dataset.description,
      dataset,
      examples: examples.map((e, i) => ({
        label: plain(e.label), family: e.family || "", index: i,
        answer: answers[`${key}:${i}`] && answers[`${key}:${i}`].ok ? answers[`${key}:${i}`] : null,
      })),
      target: playgroundHash(key, dataset.kind, null),
      base,
    });
  }
  return models;
}

// ------------------------------------------------------------ social text ----

/** og:title — what a feed shows in bold. */
export function socialTitle(model) {
  if (model.kind === "example") return `${model.title} — ${model.dataset.key}`;
  if (model.kind === "doc") return `${model.title} · rete`;
  return `${model.dataset.key} — ${model.dataset.name}`;
}

/** og:description — the answer first, because that is the hook. */
export function socialDescription(model) {
  if (model.kind === "doc") return plain(model.summary, 200);
  const scale = [model.dataset.triples && `${model.dataset.triples} triples`, model.dataset.size]
    .filter(Boolean).join(" · ");
  if (model.kind === "dataset") {
    const lead = plain(model.dataset.description, 150);
    return `${lead} ${scale ? `(${scale})` : ""} — query it in your browser, no server, no download.`.replace(/\s+/g, " ").trim();
  }
  const head = model.answer ? answerHeadline(model.answer) : "";
  const timing = model.answer ? answerTiming(model.answer) : "";
  const result = head ? `Answer: ${head}${timing ? ` in ${timing}` : ""}. ` : "";
  const tip = plain(model.tip, 150);
  return `${result}${tip ? tip + " " : ""}Runs live in your browser over ${model.dataset.key}${scale ? ` (${scale})` : ""} — no server.`
    .replace(/\s+/g, " ").trim();
}

export const ogImagePath = (model) => `og/${model.dir}/${model.slug}.png`;
export const sharePath = (model) => `${model.dir}/${model.slug}.html`;

// ------------------------------------------------------- the card markup ----

function miniTable(answer, { rows: maxRows = 4, cols: maxCols = 3 } = {}) {
  const columns = (answer.columns || []).slice(0, maxCols);
  if (!columns.length) return "";
  const dropped = (answer.columns || []).length - columns.length;
  const shown = (answer.rows || []).slice(0, maxRows);

  // Group large counts by thousands — but only per COLUMN, and only when that
  // whole column is integers and at least one of them is five digits or more.
  // Deciding cell by cell would print "12,052" next to "6844" in one column,
  // and a column of four-digit years must never grow commas.
  const grouped = columns.map((_, i) => {
    const values = shown.map((row) => (row[i] || {}).text || "").filter((t) => t !== "");
    if (!values.length || !values.every((t) => /^-?\d+$/.test(t.trim()))) return false;
    return values.some((t) => t.replace("-", "").trim().length >= 5);
  });

  const body = shown.map((row) => {
    const cells = row.slice(0, maxCols).map((cell, i) => {
      const num = isNumeric(cell.text);
      const value = grouped[i]
        ? Number(cell.text.trim()).toLocaleString("en-US")
        : shortValue(cell.text, i === 0 ? 30 : 22);
      return `<td class="${num ? "num" : ""}">${cell.media && !value ? "▣ media" : esc(value)}</td>`;
    }).join("");
    return `<tr>${cells}${dropped > 0 ? '<td class="more">…</td>' : ""}</tr>`;
  }).join("");
  if (!body) return "";
  const head = columns.map((c) => `<th>${esc(shortValue(c.label || c.var, 18))}</th>`).join("")
    + (dropped > 0 ? `<th class="more">+${dropped}</th>` : "");
  return `<table class="mini"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
}

function queryPeek(query, lines = 6) {
  const body = String(query || "").split("\n")
    .filter((l) => !/^\s*PREFIX\b/i.test(l))   // prefixes are noise in a preview
    .filter((l) => l.trim())
    .slice(0, lines);
  if (!body.length) return "";
  const html = body.map((line) => esc(line.length > 58 ? line.slice(0, 57) + "…" : line)
    .replace(/\b(SELECT|WHERE|GROUP BY|ORDER BY|LIMIT|DISTINCT|COUNT|OPTIONAL|FILTER|UNION|CONSTRUCT|ASK|VALUES|BIND|AS|DESC|ASC)\b/g,
      '<span class="kw">$1</span>')
    .replace(/(\?[A-Za-z_][\w]*)/g, '<span class="v">$1</span>'))
    .join("\n");
  return `<pre class="peek">${html}</pre>`;
}

/** The answer panel: real rows when there are rows, the rendered view when it draws, else the query. */
function answerPanel(model, { shotSrc = "" } = {}) {
  const answer = model.answer;
  if (answer && shotSrc) {
    return `<figure class="shot"><img src="${esc(shotSrc)}" alt="" /></figure>`;
  }
  const table = answer ? miniTable(answer) : "";
  if (table) return `<div class="panel">${table}</div>`;
  const peek = queryPeek(model.query);
  if (peek) return `<div class="panel">${peek}</div>`;
  return "";
}

const CARD_CSS = `
*{box-sizing:border-box;margin:0;padding:0}
/* Flat, not gradient: these cards are committed by the hundred, and a smooth
   1200px gradient needs a dithered palette that triples the PNG (240 KB vs 40 KB
   after pngquant) for a difference nobody sees in a feed. */
.card{width:${OG_W}px;height:${OG_H}px;position:relative;overflow:hidden;
  background:${T.tint};
  color:${T.ink};font-family:${T.sans};display:flex;flex-direction:column;
  padding:46px 54px 40px;gap:0}
.card::after{content:"";position:absolute;inset:0;border:14px solid ${T.accent};
  border-width:0 0 0 14px}
.net{position:absolute;right:-118px;top:-124px;width:430px;height:430px;opacity:.11;pointer-events:none}
.head{display:flex;align-items:center;justify-content:space-between;gap:20px;position:relative;z-index:1}
.brand{display:flex;align-items:baseline;gap:10px}
.brand b{font-family:${T.serif};font-size:30px;font-weight:700;letter-spacing:-.4px;color:${T.accentDark}}
.brand span{font-size:15px;letter-spacing:.16em;text-transform:uppercase;color:${T.muted}}
.chip{font-size:15px;letter-spacing:.1em;text-transform:uppercase;color:${T.accentDark};
  background:${T.surface};border:1px solid ${T.lineStrong};border-radius:999px;padding:7px 16px;white-space:nowrap}
.title{font-family:${T.serif};font-weight:700;line-height:1.1;letter-spacing:-.6px;
  margin:26px 0 0;color:${T.ink};display:-webkit-box;-webkit-box-orient:vertical;overflow:hidden;position:relative;z-index:1}
.title.s1{font-size:56px;-webkit-line-clamp:2}
.title.s2{font-size:46px;-webkit-line-clamp:3}
.title.s3{font-size:38px;-webkit-line-clamp:3}
.body{display:flex;gap:30px;align-items:stretch;margin-top:24px;flex:1;min-height:0;position:relative;z-index:1}
.left{flex:1 1 auto;min-width:0;display:flex;flex-direction:column;justify-content:center}
.panel{background:${T.surface};border:1px solid ${T.line};border-radius:14px;padding:16px 18px;
  box-shadow:0 10px 26px rgba(32,47,41,.07);overflow:hidden;max-height:250px}
/* Shrink-to-fit, so a two-column answer reads as a pair instead of a label and
   a number stranded at opposite edges of the card. */
table.mini{border-collapse:collapse;width:auto;min-width:78%;max-width:100%;font-size:19px}
table.mini th{text-align:left;font-weight:600;font-size:14px;letter-spacing:.09em;text-transform:uppercase;
  color:${T.accentDark};border-bottom:2px solid ${T.tintStrong};padding:0 14px 8px 0;white-space:nowrap}
table.mini td{padding:9px 14px 9px 0;border-bottom:1px solid ${T.code};white-space:nowrap;
  color:${T.ink};max-width:340px;overflow:hidden;text-overflow:ellipsis}
table.mini tr:last-child td{border-bottom:0}
table.mini td.num{font-variant-numeric:tabular-nums;font-weight:600;color:${T.accentDark}}
table.mini .more{color:${T.muted};font-weight:400}
pre.peek{font-family:${T.mono};font-size:17px;line-height:1.5;color:${T.ink};white-space:pre;overflow:hidden}
pre.peek .kw{color:#8a3d5a;font-weight:600}
pre.peek .v{color:${T.accent}}
/* The drawing views (graph / map / timeline) show the answer they actually drew.
   The panel is height-bound, so object-fit:cover (filling it, cropping the
   margins) shows the drawing larger than contain, which would letterbox it into
   a thumbnail. The capture already frames the drawing itself, not the panel. */
.shot{background:${T.surface};border:1px solid ${T.line};border-radius:14px;overflow:hidden;
  box-shadow:0 10px 26px rgba(32,47,41,.07);height:266px;display:flex}
.shot img{width:100%;height:100%;object-fit:cover;object-position:center}
.rail{flex:0 0 300px;display:flex;flex-direction:column;justify-content:center;gap:12px;
  border-left:1px solid ${T.lineStrong};padding-left:28px}
.rail .icon{font-size:44px;line-height:1}
.rail .key{font-family:${T.mono};font-size:23px;font-weight:600;color:${T.accentDark};
  word-break:break-word;line-height:1.2}
.rail .facts{font-size:18px;color:${T.muted};line-height:1.5}
.rail .facts b{color:${T.ink};font-weight:600;font-variant-numeric:tabular-nums}
.rail .answer{margin-top:4px;padding-top:12px;border-top:1px solid ${T.line}}
.rail .answer .big{font-family:${T.serif};font-size:32px;font-weight:700;color:${T.accent};line-height:1.15}
.rail .answer .sub{font-size:17px;color:${T.muted};margin-top:2px}
.foot{display:flex;align-items:center;justify-content:space-between;gap:18px;margin-top:22px;
  padding-top:18px;border-top:1px solid ${T.lineStrong};position:relative;z-index:1}
/* The call to action reads as a button. A card in a feed is an ad for one
   click, and an unfurl linter is right that a flat sentence does not look like
   one — so the action is a filled pill and the reassurance sits beside it. */
.foot .cta{display:flex;align-items:center;gap:16px;min-width:0}
.foot .cta .btn{background:${T.accent};color:${T.onAccent};font-size:21px;font-weight:600;
  padding:11px 24px;border-radius:999px;white-space:nowrap;line-height:1.2}
.foot .cta .why{font-size:18px;color:${T.muted};white-space:nowrap}
.foot .url{font-family:${T.mono};font-size:17px;color:${T.muted};white-space:nowrap}
.bullets{list-style:none;display:flex;flex-direction:column;gap:11px}
.bullets li{font-size:22px;color:${T.ink};line-height:1.3;display:flex;gap:11px;align-items:baseline;
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.bullets li::before{content:"▸";color:${T.accent};font-size:18px;flex:0 0 auto}
.lead{font-size:23px;line-height:1.42;color:${T.ink};display:-webkit-box;-webkit-box-orient:vertical;
  -webkit-line-clamp:4;overflow:hidden}
.card.doc .body{align-items:center}
.card.doc .lead{font-size:26px;-webkit-line-clamp:5;max-width:1000px}
/* A dataset card has no result panel to centre on, so its list and its facts
   hang from the title instead of floating in the middle of the card. */
.card.dataset .left,.card.dataset .rail{justify-content:flex-start;padding-top:8px}
.card.dataset .left{gap:16px}
.card.dataset .lead{-webkit-line-clamp:3;font-size:21px;color:${T.muted}}
`;

/** The decorative net — rete is a net, and it doubles as the brand mark. */
const NET_SVG = `<svg class="net" viewBox="0 0 200 200" fill="none" aria-hidden="true">
  <g stroke="${T.accent}" stroke-width="1.1">
    <path d="M30 40 L100 20 L170 55 L150 130 L70 160 L20 110 Z"/>
    <path d="M100 20 L70 160 M30 40 L150 130 M170 55 L20 110 M100 20 L20 110 M30 40 L70 160"/>
    <path d="M100 90 L30 40 M100 90 L170 55 M100 90 L150 130 M100 90 L70 160 M100 90 L20 110 M100 90 L100 20"/>
  </g>
  <g fill="${T.accentDark}">
    ${[[30, 40], [100, 20], [170, 55], [150, 130], [70, 160], [20, 110], [100, 90]]
      .map(([x, y]) => `<circle cx="${x}" cy="${y}" r="${x === 100 && y === 90 ? 7 : 5}"/>`).join("")}
  </g>
</svg>`;

function titleClass(title) {
  const n = String(title || "").length;
  return n <= 46 ? "s1" : n <= 78 ? "s2" : "s3";
}

/** The footer: one button-shaped action, the reassurance beside it, the domain. */
function footHtml(action) {
  return `<div class="foot">
      <div class="cta"><span class="btn">${esc(action)}</span>
        <span class="why">no server · no download · no account</span></div>
      <div class="url">caviri.github.io/rete</div>
    </div>`;
}

/** A documentation page's card: section chip, page title, its opening paragraph. */
function docCard(model) {
  return `<div class="card doc">
    ${NET_SVG}
    <div class="head">
      <div class="brand"><b>rete</b><span>docs</span></div>
      ${model.section ? `<div class="chip">${esc(model.section)}</div>` : ""}
    </div>
    <h1 class="title ${titleClass(model.title)}">${esc(model.title)}</h1>
    <div class="body">
      <div class="left"><p class="lead">${esc(plain(model.summary, 300))}</p></div>
    </div>
    ${footHtml(model.section === "Explore in the browser" ? "Open it in your browser →" : "Read the guide →")}
  </div>`;
}

/** The card body — identical content for the PNG and the landing page. */
export function cardInner(model, { shotSrc = "" } = {}) {
  if (model.kind === "doc") return docCard(model);
  const d = model.dataset;
  const scale = [d.triples && `<b>${esc(d.triples)}</b> triples`, d.size && esc(d.size)]
    .filter(Boolean).join(" · ");
  const load = d.remote ? "remote · HTTP range" : "embedded in the page";
  const head = model.answer ? answerHeadline(model.answer) : "";
  const timing = model.answer ? answerTiming(model.answer) : "";

  const net = model.answer ? answerNetwork(model.answer) : null;
  const sub = net
    ? `${net.requests ? `${net.requests} range request${net.requests === 1 ? "" : "s"} · ` : ""}${esc(net.fetched)} of ${esc(net.total)} read`
    : (timing ? `answered in ${esc(timing)}` : "answered in the browser");
  const rail = `<aside class="rail">
      <div class="icon">${esc(d.icon)}</div>
      <div class="key">${esc(d.key)}</div>
      <div class="facts">${scale}<br />${esc(load)}${
        model.kind === "dataset" && d.license ? `<br />${esc(d.license)}` : ""}</div>
      ${head ? `<div class="answer"><div class="big">${esc(head)}</div>
        <div class="sub">${sub}</div>
        ${net && timing ? `<div class="sub">in ${esc(timing)}</div>` : ""}</div>` : ""}
    </aside>`;

  let left;
  if (model.kind === "dataset") {
    // The questions this graph answers ARE the pitch, so lead with them; a
    // sparsely-exampled dataset falls back to describing itself.
    const shown = (model.examples || []).slice(0, 5);
    const picks = shown.map((e) => `<li>${esc(e.label)}</li>`).join("");
    left = `<div class="left">
      ${picks ? `<ul class="bullets">${picks}</ul>` : ""}
      ${shown.length < 4 ? `<p class="lead">${esc(plain(d.description, 260))}</p>` : ""}
    </div>`;
  } else {
    left = `<div class="left">${answerPanel(model, { shotSrc })}</div>`;
  }

  const chip = model.kind === "dataset"
    ? `${(model.examples || []).length} example queries`
    : (model.family || model.view);

  return `<div class="card${model.kind === "dataset" ? " dataset" : ""}">
    ${NET_SVG}
    <div class="head">
      <div class="brand"><b>rete</b><span>${model.kind === "dataset" ? "dataset" : "playground"}</span></div>
      ${chip ? `<div class="chip">${esc(chip)}</div>` : ""}
    </div>
    <h1 class="title ${titleClass(model.title)}">${esc(model.title)}</h1>
    <div class="body">${left}${rail}</div>
    ${footHtml(model.kind === "dataset" ? "Explore this graph →" : "Run this query →")}
  </div>`;
}

/** Standalone document whose single element is the 1200x630 card. */
export function ogHtml(model, opts = {}) {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8" />
<title>${esc(socialTitle(model))}</title>
<style>html,body{width:${OG_W}px;height:${OG_H}px;background:${T.paper}}${CARD_CSS}</style>
</head><body>${cardInner(model, opts)}</body></html>`;
}

export { CARD_CSS };
