// "How it works" drawn, and what it costs to publish a graph.
//
// Every number here is measured or published: reads.js holds what this page's
// own reader fetched from R2 for each demo question, the file anatomy is
// `rete info` on bph-2026-07-01.rete, servers.js holds what QLever and
// Oxigraph used to serve the same graphs, and PRICES carries each provider's
// price with the page it was read from.

import { READS, MEASURED } from "./reads.js";
import { SERVERS, ENGINES, SERVERS_MEASURED } from "./servers.js";

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
const REDUCED = matchMedia("(prefers-reduced-motion: reduce)").matches;
const GiB = 2 ** 30;

function h(tag, attrs, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === "class") n.className = v;
    else if (k === "style") n.style.cssText = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) n.append(k instanceof Node ? k : document.createTextNode(String(k)));
  return n;
}

function fmtBytes(n) {
  if (!Number.isFinite(n)) return "?";
  if (n < 1000) return `${n} B`;
  const u = ["KB", "MB", "GB", "TB"];
  let v = n / 1000, i = 0;
  while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
  return `${v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${u[i]}`;
}
const fmtPct = (p) => (p >= 10 ? p.toFixed(0) : p >= 1 ? p.toFixed(1) : p >= 0.1 ? p.toFixed(2) : p >= 0.01 ? p.toFixed(3) : p.toFixed(4)) + "%";
function fmtS(s) {
  if (s < 10) return `${s.toFixed(1)} s`;
  if (s < 90) return `${Math.round(s)} s`;
  if (s < 5400) return `${Math.round(s / 60)} min`;
  return `${(s / 3600).toFixed(s < 36000 ? 1 : 0)} h`;
}
function fmtCount(n) {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} billion`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 1 : 2)} million`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} thousand`;
  return String(n);
}
function fmtUSD(v) {
  if (v === 0) return "$0";
  if (v < 0.01) return "under $0.01";
  return `$${v.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}
const cents = (v) => Math.round(v * 100) / 100;
const cssVar = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();

// Run fn once the node is on screen.
function whenSeen(node, fn, threshold = 0.2) {
  const io = new IntersectionObserver(([e]) => { if (e.isIntersecting) { io.disconnect(); fn(); } }, { threshold });
  io.observe(node);
}

// A label goes inside its block only when it fits with room to spare.
function fitLabels(scope) {
  for (const l of $$(".ff-lbl", scope)) {
    l.hidden = false;
    l.hidden = l.scrollWidth + 14 > l.parentElement.clientWidth;
  }
}

/* ───────────────────────── 1. What is inside a file ───────────────────────── */

// `rete info` on https://data.graphplaza.com/bph/bph-2026-07-01.rete (format 0x08).
const ANATOMY = {
  file: "bph-2026-07-01.rete", length: 57511947, statements: 5741344,
  parts: [
    { key: "front", name: "Front", bytes: 81719, cls: "p1", what: "Header, Dataset Card, schema, and directories saying where everything below starts" },
    { key: "dict", name: "Dictionary", bytes: 8530173, cls: "p2", what: "Every IRI and literal, stored once and numbered" },
    { key: "index", name: "Indexes", bytes: 31986866, cls: "p3", what: "The statements sorted six ways (SPO, POS, OSP, SOP, PSO, OPS), so any pattern is one stretch of bytes" },
    { key: "labels", name: "Labels", bytes: 2731604, cls: "p4", what: "Names, for search and display" },
    { key: "pyramid", name: "Summary", bytes: 14181577, cls: "p5", what: "A small map of the graph: its classes and its communities" },
  ],
  front: [["Header", 843], ["Dataset Card", 32600], ["Schema", 2120], ["Dictionary directory", 14709], ["Index directory", 22863], ["Label directory", 8584]],
};

(function fileFigure() {
  const root = $("#figFile");
  if (!root) return;
  const A = ANATOMY;
  const legend = h("ul", { class: "ff-legend" });
  const bar = h("div", {
    class: "ff-bar", role: "img",
    "aria-label": `${A.file}, ${fmtBytes(A.length)}: ` + A.parts.map((p) => `${p.name} ${fmtBytes(p.bytes)}`).join(", "),
  });
  const link = (k, on) => $$(`[data-k="${k}"]`, root).forEach((n) => n.classList.toggle("on", on));
  A.parts.forEach((p, i) => {
    const seg = h("span", {
      class: `ff-seg ${p.cls}`, "data-k": p.key,
      style: `--i:${i};` + (p.key === "front" ? "flex:0 0 10px" : `flex:${p.bytes} 1 0`),
    }, p.key === "front" ? null : h("span", { class: "ff-lbl" }, fmtBytes(p.bytes)));
    const li = h("li", { "data-k": p.key },
      h("span", { class: `sw ${p.cls}`, "aria-hidden": "true" }),
      h("b", {}, p.name), h("span", { class: "ff-sz" }, fmtBytes(p.bytes)),
      h("span", { class: "ff-what" }, p.what));
    for (const n of [seg, li]) {
      n.addEventListener("pointerenter", () => link(p.key, true));
      n.addEventListener("pointerleave", () => link(p.key, false));
    }
    bar.append(seg); legend.append(li);
  });
  const frontBar = h("div", {
    class: "ff-bar ff-bar-front", role: "img",
    "aria-label": "The first 82 KB: " + A.front.map(([n, b]) => `${n} ${fmtBytes(b)}`).join(", "),
  }, A.front.map(([n, b], i) => h("span", { class: "ff-seg p1", style: `--i:${i};flex:${b} 1 0` }, h("span", { class: "ff-lbl" }, n))));
  root.append(
    h("figcaption", { class: "fig-cap" },
      h("b", {}, "Inside a real file"),
      h("span", {}, `${A.file} · ${fmtBytes(A.length)} · ${fmtCount(A.statements)} statements`)),
    h("div", { class: "ff-in", "aria-hidden": "true" },
      ["Turtle", "N-Triples", "N-Quads", "TriG", "RDF/XML", "JSON-LD", "Jelly"].map((f, i) => h("span", { class: "ff-chip", style: `--i:${i}` }, f)),
      h("span", { class: "ff-arrow" }, "rete build ↓")),
    bar,
    h("div", { class: "ff-zoom" },
      h("div", { class: "ff-fan", "aria-hidden": "true" }),
      frontBar,
      h("p", { class: "ff-note" },
        h("b", {}, "The first 82 KB"),
        " hold the header, the card, the schema and the three directories. A reader's first request asks for 131 KB, so one request opens a file of any size.")),
    legend);
  const refit = () => fitLabels(root);
  new ResizeObserver(refit).observe(bar);
  refit();
})();

/* ───────────────────────── 3. The reads a question makes ───────────────────────── */

const RTT = 0.03; // seconds per request, one at a time: the page's reader is synchronous

(function readsFigure() {
  const root = $("#figReads");
  if (!root) return;
  let g = READS.find((x) => x.key === "hugging-face") || READS[0];
  let q = g.questions[0];
  let mbps = 50;
  let sim = null, raf = 0, t0 = 0, onScreen = false, played = false, done = false;

  const graphChips = h("div", { class: "chips fr-chips", role: "radiogroup", "aria-label": "Graph" });
  const qChips = h("div", { class: "chips chips-q fr-chips", role: "radiogroup", "aria-label": "Question" });
  const speed = h("div", { class: "seg fr-speed", role: "radiogroup", "aria-label": "Connection speed" });
  const replay = h("button", { class: "btn btn-ghost btn-sm", type: "button" }, "Replay");
  const laneA = lane("rete", "reads the parts the question needs");
  const laneB = lane("Download first", "fetches the whole file, then asks");
  const verdict = h("p", { class: "fr-verdict", "aria-live": "polite" });
  const measured = h("p", { class: "fr-foot" });

  function lane(name, sub) {
    const clock = h("span", { class: "fr-clock" });
    const canvas = h("canvas", { class: "fr-strip", "aria-hidden": "true" });
    const meta = h("p", { class: "fr-meta" });
    const node = h("div", { class: "fr-lane" },
      h("div", { class: "fr-head" }, h("b", {}, name), h("span", {}, sub), clock),
      h("div", { class: "fr-strip-wrap" }, canvas, h("span", { class: "strip-cap strip-cap-l" }, "byte 0"), h("span", { class: "strip-cap strip-cap-r fr-end" })),
      meta);
    return { node, clock, canvas, meta, end: $(".fr-end", node) };
  }

  function chip(label, small, checked, onPick) {
    return h("button", { class: "chip", type: "button", role: "radio", "aria-checked": String(checked), onclick: onPick },
      label, small ? h("small", {}, small) : null);
  }
  function renderChips() {
    graphChips.replaceChildren(...READS.map((x) => chip(x.name, fmtBytes(x.length), x === g, () => { g = x; q = x.questions[0]; renderChips(); start(); })));
    qChips.replaceChildren(...g.questions.map((x) => chip(x.label, null, x === q, () => { q = x; renderChips(); start(); })));
    speed.replaceChildren(...[10, 50, 200].map((m) => h("button", {
      type: "button", role: "radio", "aria-checked": String(m === mbps),
      onclick: () => { mbps = m; renderChips(); start(); },
    }, `${m} Mbit/s`)));
  }

  // Requests one after another: each costs a round trip plus its bytes over the link.
  function plan() {
    const bps = (mbps * 1e6) / 8;
    const reqs = [];
    let t = 0;
    for (const [a, len] of g.open.spans) { const d = RTT + len / bps; reqs.push({ a, len, t0: t, t1: t + d, open: true }); t += d; }
    for (const [a, len] of q.spans) { const d = RTT + len / bps; reqs.push({ a, len, t0: t, t1: t + d }); t += d; }
    const bytes = g.open.bytes + q.bytes, requests = g.open.requests + q.requests;
    return { reqs, T: t, bytes, requests, dl: RTT + g.length / bps };
  }

  function start() {
    cancelAnimationFrame(raf); raf = 0; done = false;
    sim = plan();
    // Show the whole read in 2 to 5 seconds of animation, whatever it takes on the wire.
    sim.speed = sim.T / Math.min(5, Math.max(2, sim.T));
    t0 = performance.now();
    laneA.end.textContent = laneB.end.textContent = fmtBytes(g.length);
    verdict.textContent = "";
    measured.textContent = `Measured ${MEASURED} with this page's reader on a home connection, the question itself took ${fmtS(q.ms / 1000)} and returned ${q.rows} row${q.rows === 1 ? "" : "s"}. The timings above are worked out from the bytes and requests, so you can change the connection.`;
    if (REDUCED || !onScreen) { draw(sim.T); if (REDUCED || played) finish(); return; }
    played = true;
    raf = requestAnimationFrame(tick);
  }
  function tick(now) {
    raf = 0;
    const t = ((now - t0) / 1000) * sim.speed;
    draw(Math.min(t, sim.T));
    if (t >= sim.T) { finish(); return; }
    if (onScreen && !document.hidden) raf = requestAnimationFrame(tick);
  }
  function finish() {
    done = true;
    draw(sim.T);
    const pctFile = (sim.bytes / g.length) * 100;
    if (sim.bytes >= g.length * 0.5) {
      verdict.textContent = `This file is small enough that rete reads all of it, so downloading it first is just as quick: ${fmtS(sim.T)} either way at ${mbps} Mbit/s.`;
    } else {
      const done = Math.min(100, (sim.T / sim.dl) * 100);
      verdict.textContent = `rete has the answer after ${fmtS(sim.T)}, having read ${fmtPct(pctFile)} of the file. By then the download is ${fmtPct(done)} done; it needs ${fmtS(sim.dl)} in all.`;
    }
  }

  function setup(c) {
    const r = c.getBoundingClientRect();
    const dpr = Math.min(devicePixelRatio || 1, 2);
    c.width = Math.max(1, r.width * dpr); c.height = Math.max(1, r.height * dpr);
    const ctx = c.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, r.width, r.height);
    return [ctx, r.width, r.height];
  }
  function ticks(ctx, W, H) {
    ctx.fillStyle = cssVar("--border");
    for (let i = 1; i < 10; i++) ctx.fillRect(Math.round((W * i) / 10), H - 5, 1, 5);
  }
  function draw(t) {
    const L = g.length, acc = cssVar("--accent"), warm = cssVar("--accent-2"), deep = cssVar("--accent-deep");
    // Lane A: each request lights its stretch of the file when it arrives.
    {
      const [ctx, W, H] = setup(laneA.canvas);
      ticks(ctx, W, H);
      let got = 0, n = 0;
      for (const r of sim.reqs) {
        if (t < r.t0) break;
        const x0 = (r.a / L) * W, wd = Math.max(2, (r.len / L) * W);
        if (t >= r.t1) {
          n++; got += r.len;
          ctx.globalAlpha = 1; ctx.fillStyle = r.open ? warm : acc;
          ctx.fillRect(x0, 4, wd, H - 14);
        } else {
          // in flight: a beam over where it reads
          const f = (t - r.t0) / (r.t1 - r.t0);
          ctx.globalAlpha = 0.25 + 0.5 * f; ctx.fillStyle = r.open ? warm : acc;
          ctx.fillRect(x0 - 3, 0, wd + 6, H - 6);
          ctx.globalAlpha = 1; ctx.fillStyle = deep;
          ctx.fillRect(x0, 4, wd * f, H - 14);
        }
      }
      ctx.globalAlpha = 1;
      // Linked Jazz's two opening reads overlap; count what was fetched, not the union.
      const bytes = t >= sim.T ? sim.bytes : got, reqs = t >= sim.T ? sim.requests : n;
      laneA.clock.textContent = fmtS(t);
      laneA.meta.textContent = `${fmtBytes(bytes)} in ${reqs} request${reqs === 1 ? "" : "s"}, ${fmtPct((bytes / L) * 100)} of the file` + (t >= sim.T ? ". Answered." : "");
    }
    // Lane B: the same link, filling the file from byte 0.
    {
      const [ctx, W, H] = setup(laneB.canvas);
      ticks(ctx, W, H);
      const f = Math.min(1, Math.max(0, (t - RTT) / (sim.dl - RTT)));
      ctx.fillStyle = cssVar("--muted"); ctx.globalAlpha = 0.55;
      ctx.fillRect(0, 4, Math.max(f > 0 ? 2 : 0, f * W), H - 14);
      ctx.globalAlpha = 1;
      laneB.clock.textContent = fmtS(Math.min(t, sim.dl));
      laneB.meta.textContent = f >= 1
        ? `${fmtBytes(L)} downloaded. Now it can be queried.`
        : `${fmtBytes(f * L)} of ${fmtBytes(L)} downloaded, ${fmtPct(f * 100)}`;
    }
  }

  root.append(
    h("figcaption", { class: "fig-cap" }, h("b", {}, "One question, two ways"),
      h("span", {}, "The colored marks are the ranges this page's reader really fetched for that question")),
    h("div", { class: "fr-controls" },
      h("div", { class: "fr-ctl" }, h("span", { class: "row-label" }, "Graph"), graphChips),
      h("div", { class: "fr-ctl" }, h("span", { class: "row-label" }, "Question"), qChips),
      h("div", { class: "fr-ctl fr-ctl-end" }, h("span", { class: "row-label" }, "Link"), speed, replay)),
    laneA.node, laneB.node,
    h("p", { class: "fr-key" },
      h("span", { class: "k k-open" }), "opening the file ",
      h("span", { class: "k k-q" }), "the question's reads ",
      h("span", { class: "k k-dl" }), "a plain download"),
    verdict, measured);
  replay.addEventListener("click", () => { played = true; start(); });
  renderChips();
  new IntersectionObserver(([e]) => {
    const was = onScreen;
    onScreen = e.isIntersecting;
    // first time on screen, or back on screen with the run cut short: play it (again)
    if (onScreen && !was && (!played || !done)) start();
  }, { threshold: 0.35 }).observe(root);
  sim = plan(); sim.speed = 1; draw(0);
  laneA.end.textContent = laneB.end.textContent = fmtBytes(g.length);
  let rt = 0;
  addEventListener("resize", () => { clearTimeout(rt); rt = setTimeout(() => { if (sim && !raf) draw(played ? sim.T : 0); }, 120); });
})();

/* ───────────────────────── 4. Any size ───────────────────────── */

(function ladderFigure() {
  const root = $("#figLadder");
  if (!root) return;
  const keys = ["bph", "hugging-face", "opencitations-index"];
  const units = { bph: "MB", "hugging-face": "GB", "opencitations-index": "100 GB" };
  const rows = keys.map((k) => READS.find((g) => g.key === k)).filter(Boolean);
  const avg = (g) => g.questions.reduce((s, q) => s + q.bytes, 0) / g.questions.length;
  const list = h("ol", { class: "fl-list" });
  rows.forEach((g, i) => {
    const share = (avg(g) / g.length) * 100;
    list.append(h("li", { class: "fl-row", style: `--i:${i}` },
      h("span", { class: "fl-unit", "aria-hidden": "true" }, units[g.key]),
      h("div", { class: "fl-name" }, h("b", {}, g.name), h("span", {}, `${fmtBytes(g.length)} · ${fmtCount(g.statements)} statements`)),
      h("div", { class: "fl-open" }, h("span", { class: "fl-pill" }, `opens with ${fmtBytes(g.open.bytes)}, ${g.open.requests} request`)),
      h("div", { class: "fl-q" },
        h("span", {}, `a question reads ${fmtBytes(avg(g))} on average, `, h("b", {}, fmtPct(share)), " of the file"),
        h("span", { class: "fl-meter", "aria-hidden": "true" }, h("span", { style: `--w:${Math.max(0.6, share)}%` })))));
  });
  const [a, b] = [rows[0], rows[rows.length - 1]];
  root.append(
    h("figcaption", { class: "fig-cap" }, h("b", {}, "Same first read, any size"),
      h("span", {}, "The demo's questions, measured on the files on R2")),
    list,
    h("p", { class: "fl-note" },
      `From ${a.name} to ${b.name} the file is ${Math.round(b.length / a.length).toLocaleString("en-US")} times bigger; a question reads ${(avg(b) / avg(a)).toFixed(1)} times more.`));
})();

/* ───────────────────────── Cost ───────────────────────── */

// Checked 2026-11-14 on each provider's own page; the AWS figures again on
// 2026-11-15 in the price-list files that AWS's pricing pages load.
const PRICES = {
  checked: "2026-11-14",
  r2: { gbMonth: 0.015, freeGB: 10, perMillionReads: 0.36, freeReads: 10e6, src: "https://developers.cloudflare.com/r2/pricing/" },
  s3: { gbMonth: 0.023, perGet: 0.0004 / 1000, src: "https://aws.amazon.com/s3/pricing/" },
  // Data out to the internet, shared by S3 and EC2: the first 100 GB a month are free, and the
  // tiers ("first 10 TB ... beyond the global free tier") start after them.
  awsOut: { freeGB: 100, tiers: [[10240, 0.09], [40960, 0.085], [102400, 0.07], [Infinity, 0.05]] },
  pages: { fileBytes: 100 * 2 ** 20, softBytes: 100e9, src: "https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits" },
  zenodo: { recordBytes: 50e9, src: "https://help.zenodo.org/docs/deposit/manage-files/" },
  // Hugging Face, checked 2026-11-18 on /storage, /pricing and /docs/hub/storage-limits. Public
  // storage is free and "best-effort"; "beyond the first few gigabytes" it asks for PRO ($9 a
  // month, "up to 10TB included" public). It names no number for "a few", so files up to
  // freeBytes are priced on a free account. "Egress is included up to a generous 8:1 ratio of
  // your total storage"; the pages say neither over what period nor what happens past it.
  hf: { checked: "2026-11-18", freeBytes: 5e9, proMonth: 9, proPublicBytes: 10e12, egressRatio: 8, src: "https://huggingface.co/storage" },
  // AWS Graviton, on demand, Linux, us-east-1: [name, GiB of RAM, $ an hour]. Each has 2 vCPUs,
  // as the servers had when they were measured.
  ec2: {
    machines: [["t4g.nano", 0.5, 0.0042], ["t4g.micro", 1, 0.0084], ["t4g.small", 2, 0.0168], ["t4g.medium", 4, 0.0336],
      ["t4g.large", 8, 0.0672], ["r7g.large", 16, 0.1071]],
    hours: 730, gp3GBMonth: 0.08, systemGB: 8, osBytes: 512 * 2 ** 20, src: "https://aws.amazon.com/ec2/pricing/on-demand/",
  },
};

// Up to a million: at that rate a server gets a question every 2.6 s on average, and the slowest
// answer measured took under a second, so one machine keeps up. Busier traffic was not measured.
const READERS = [[1e3, "1,000"], [1e5, "100,000"], [1e6, "1 million"]];

// A reader opens the file and asks one of the demo's questions.
function perReader(g) {
  const n = g.questions.length;
  return {
    bytes: g.open.bytes + g.questions.reduce((s, q) => s + q.bytes, 0) / n,
    requests: g.open.requests + g.questions.reduce((s, q) => s + q.requests, 0) / n,
  };
}

// AWS data out to the internet for one month, in dollars, for `gb` GiB.
function awsOut(gb) {
  let rest = Math.max(0, gb - PRICES.awsOut.freeGB), cost = 0;
  for (const [size, price] of PRICES.awsOut.tiers) {
    const take = Math.min(rest, size);
    cost += take * price; rest -= take;
    if (rest <= 0) break;
  }
  return cost;
}

// A server for graph g, from what QLever or Oxigraph used when it was measured. The times were
// measured with the index in memory, so the machine is the smallest whose memory holds the
// server's peak, its index and 512 MiB for the system. The disk is 8 GB plus the index (billed
// in whole GB), and each reader's answer is sent out once.
function serverRow(g, engine, s, readers) {
  const P = PRICES.ec2;
  const m = P.machines.find(([, ram]) => ram * GiB >= s.servePeakBytes + s.indexBytes + P.osBytes);
  if (!m) return null;
  const diskGB = Math.ceil(P.systemGB + s.indexBytes / GiB);
  const machine = m[2] * P.hours, disk = diskGB * P.gp3GBMonth, out = awsOut((readers * s.answerBytes) / GiB);
  return {
    name: `${engine.name} server`, server: true, total: machine + disk + out,
    sub: `an AWS ${m[0]} (${m[1]} GiB), running all month`,
    parts: [["machine", machine], [`${diskGB} GB disk`, disk], ["data out", out]],
    note: `it peaked at ${fmtBytes(s.servePeakBytes)} of memory, with a ${fmtBytes(s.indexBytes)} index`,
  };
}

function costRows(g, readers) {
  const { bytes, requests } = perReader(g);
  const gb = g.length / GiB, outGB = (readers * bytes) / GiB, reads = readers * requests;
  const P = PRICES;
  const rows = [];
  {
    const storage = Math.max(0, gb - P.r2.freeGB) * P.r2.gbMonth;
    const ops = (Math.max(0, reads - P.r2.freeReads) / 1e6) * P.r2.perMillionReads;
    rows.push({ name: "Cloudflare R2", sub: "object storage, no charge for data out", total: storage + ops,
      parts: [["storage", storage], ["requests", ops], ["data out", 0]] });
  }
  {
    const storage = gb * P.s3.gbMonth, ops = reads * P.s3.perGet, out = awsOut(outGB);
    rows.push({ name: "Amazon S3", sub: "object storage, us-east-1", total: storage + ops + out,
      parts: [["storage", storage], ["requests", ops], ["data out", out]] });
  }
  {
    const name = "Hugging Face bucket", out = readers * bytes, incl = P.hf.egressRatio * g.length;
    const egress = out > incl
      ? `but data out is included only up to ${P.hf.egressRatio}× the file (${fmtBytes(incl)}), and these readers fetch ${fmtBytes(out)} a month`
      : "and data out is included";
    if (g.length <= P.hf.freeBytes) rows.push({ name, sub: "public, on a free account", total: 0, parts: [], note: `free, best effort, ${egress}` });
    else if (g.length <= P.hf.proPublicBytes) rows.push({ name, sub: "public, on a PRO account", total: P.hf.proMonth,
      parts: [["PRO account", P.hf.proMonth]], note: `PRO includes up to 10 TB, ${egress}` });
    else rows.push({ name, sub: "public", unfit: "Past the 10 TB of public storage a PRO account includes." });
  }
  if (g.length > P.pages.fileBytes) rows.push({ name: "GitHub Pages", sub: "a public repository", unfit: "A git repository refuses files over 100 MiB." });
  else rows.push({ name: "GitHub Pages", sub: "a public repository", total: 0, parts: [],
    note: readers * bytes > P.pages.softBytes ? `but ${fmtBytes(readers * bytes)} a month is past its 100 GB soft limit` : "free for a public repository" });
  if (g.length > P.zenodo.recordBytes) rows.push({ name: "Zenodo", sub: "a research archive", unfit: "A record holds at most 50 GB." });
  else rows.push({ name: "Zenodo", sub: "a research archive", total: 0, parts: [], note: "free, with a DOI" });
  const measured = SERVERS[g.key];
  if (!measured) {
    rows.push({ name: "A SPARQL server", sub: "QLever or Oxigraph", server: true,
      unfit: `Not measured. We ran the servers with 4 GB of memory; ${fmtCount(g.statements)} statements is past what we tested.` });
  } else {
    for (const e of ENGINES) {
      const s = measured[e.key];
      const row = s && serverRow(g, e, s, readers);
      rows.push(row || { name: `${e.name} server`, sub: "", server: true, unfit: "Not measured on this graph." });
    }
  }
  // Each part is rounded to the cent and the total is the sum of the rounded parts, so the
  // figures on the page add up.
  return rows.map((r) => {
    if (!r.parts || !r.parts.length) return r;
    const parts = r.parts.map(([k, v]) => [k, cents(v), v]);
    return { ...r, parts, total: cents(parts.reduce((t, p) => t + p[1], 0)) };
  });
}

(function costFigure() {
  const root = $("#costBox");
  if (!root) return;
  let g = READS.find((x) => x.key === "hugging-face") || READS[READS.length - 1];
  let readers = 1e5;
  const graphChips = h("div", { class: "chips", role: "radiogroup", "aria-label": "Graph" });
  const readerSeg = h("div", { class: "seg", role: "radiogroup", "aria-label": "Readers a month" });
  const lead = h("p", { class: "cost-lead", "aria-live": "polite" });
  const list = h("ol", { class: "cost-rows" });
  const foot = h("p", { class: "cost-foot" });

  function render() {
    graphChips.replaceChildren(...READS.map((x) => h("button", {
      class: "chip", type: "button", role: "radio", "aria-checked": String(x === g),
      onclick: () => { g = x; render(); },
    }, x.name, h("small", {}, fmtBytes(x.length)))));
    readerSeg.replaceChildren(...READERS.map(([n, label]) => h("button", {
      type: "button", role: "radio", "aria-checked": String(n === readers),
      onclick: () => { readers = n; render(); },
    }, label)));
    const rows = costRows(g, readers);
    const max = Math.max(...rows.map((r) => r.total || 0));
    list.replaceChildren(...rows.map((r, i) => h("li", { class: "cost-row" + (r.server ? " is-server" : "") + (r.unfit ? " is-unfit" : ""), style: `--i:${i}` },
      h("div", { class: "cr-name" }, h("b", {}, r.name), h("span", {}, r.sub)),
      r.unfit
        ? h("div", { class: "cr-unfit" }, r.unfit)
        : h("div", { class: "cr-bar" },
          h("span", { class: "cr-fill", style: `--w:${max ? (r.total / max) * 100 : 0}%` }),
          h("b", { class: "cr-val" }, fmtUSD(r.total))),
      r.unfit ? null : h("div", { class: "cr-parts" },
        r.parts.map(([k, v, raw]) => h("span", {}, `${k} ${v === 0 && raw > 0 ? "under $0.01" : v === 0 && r.name === "Cloudflare R2" && k === "data out" ? "free" : fmtUSD(v)}`)),
        r.note ? h("span", { class: "cr-note" }, r.note) : null))));
    const r2 = rows[0].total;
    const cheapest = rows.filter((r) => r.server && !r.unfit).sort((a, b) => a.total - b.total)[0];
    const { bytes, requests } = perReader(g);
    lead.replaceChildren(
      h("b", {}, g.name), ` (${fmtBytes(g.length)}), ${READERS.find(([n]) => n === readers)[1]} readers a month: `,
      r2 === 0 ? "R2's free allowance covers it." : h("span", {}, h("b", {}, fmtUSD(r2)), " a month on R2."),
      cheapest
        ? h("span", {}, " The cheapest server we measured for it, ", cheapest.name.replace(/ server$/, ""), ": ", h("b", {}, fmtUSD(cheapest.total)), " a month.")
        : " We have not measured a server for a graph this size.");
    foot.textContent = `A reader opens the file and asks one of the demo's questions: ${fmtBytes(bytes)} in ${requests.toFixed(0)} requests on average for this graph, measured ${MEASURED}. Servers measured ${SERVERS_MEASURED}. Prices checked ${PRICES.checked}, Hugging Face's ${PRICES.hf.checked}.`;
  }
  root.append(
    h("div", { class: "cost-controls" },
      h("div", { class: "fr-ctl" }, h("span", { class: "row-label" }, "Graph"), graphChips),
      h("div", { class: "fr-ctl" }, h("span", { class: "row-label" }, "Readers"), readerSeg)),
    lead, h("p", { class: "cost-unit" }, "A month, in US dollars"), list, foot);
  render();
})();
