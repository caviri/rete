// procgen-p5.js — render a dataset's schema as a hand-coloured engraving, the way
// a specimen plate looks in an old natural-history book: aged/foxed parchment, a
// ruled plate border, "specimens" (the classes) shaded with fine ink
// cross-hatching + stipple over a muted colour wash, italic serif labels with
// leader lines, and a plate caption.
//
// What the picture says, all of it read from the card:
//   - a node is a class, its area ∝ instances, its hue the vocabulary; the label
//     carries the instance count. "(literal)" and "(untyped)" are not classes and
//     are never drawn.
//   - an arrow is a class-to-class relation; the strongest ones are named. A link
//     from a class to itself is drawn as a loop.
//   - the caption counts classes, relations and triples; the top-right corner
//     gives the time span when the card has one.
//   - a card with no classes gets its top properties as bars; a file with no card
//     gets its counts. Nothing is invented to fill the plate.
//
// Rendered once to a PNG data URL (so the grid re-paints cheaply). Deterministic:
// p5 randomSeed/noiseSeed come from the file's content hash.
import { buildSchemaGraph, hueOf } from "./procgen.js";

const SERIF = "Georgia, 'Palatino Linotype', 'Times New Roman', serif";
const clamp = (x, lo, hi) => Math.max(lo, Math.min(hi, x));
const TAU = Math.PI * 2;

function hash32(str) {
  let h = 1779033703 ^ str.length;
  for (let i = 0; i < str.length; i++) {
    h = Math.imul(h ^ str.charCodeAt(i), 3432918353);
    h = (h << 13) | (h >>> 19);
  }
  h = Math.imul(h ^ (h >>> 16), 2246822507);
  h = Math.imul(h ^ (h >>> 13), 3266489909);
  return (h ^= h >>> 16) >>> 0;
}

// Naturalist palettes. ink is the engraving pen; wash is the light hand-colour.
function palette(theme, base) {
  if (theme === "light")
    return {
      paperH: 41, paperS: 32, paperL: 89, // warm parchment
      foxH: 28, foxS: 45, foxL: 38, // foxing/age spots
      inkH: 28, inkS: 46, inkL: 24, // sepia ink
      washS: 42, washL: 58, // hand-colour wash (uses the class hue)
      neutralH: 35, neutralS: 16, neutralL: 55,
      label: { h: 28, s: 50, l: 22 }, // sepia letters
      vignette: "rgba(70,52,30,0.16)",
      grain: 12,
      shadow: { h: 30, s: 28, l: 24, a: 0.13 }, // geo region shadow (darkens paper)
    };
  return {
    // Dark plate in the site's green-teal (hues 158-165), no sepia: the home
    // page used to recolour a sepia plate with a CSS hue-rotate.
    paperH: 158, paperS: 24, paperL: 8.5,
    foxH: 165, foxS: 38, foxL: 26,
    inkH: 163, inkS: 34, inkL: 80, // pale mint pen
    washS: 50, washL: 52,
    neutralH: 160, neutralS: 14, neutralL: 52,
    label: { h: 160, s: 38, l: 90 },
    vignette: "rgba(0,0,0,0.5)",
    grain: 92,
    shadow: { h: 165, s: 24, l: 88, a: 0.1 }, // geo region shadow (lightens dark paper)
  };
}

/** Render to a PNG data URL. opts: {theme, w, h, labels}. Resolves null if p5 missing. */
export function renderFingerprint(info, opts = {}) {
  return new Promise((resolve) => {
    if (typeof p5 === "undefined") return resolve(null);
    const W = opts.w || 520;
    const H = opts.h || Math.round(W * 0.625);
    const theme = opts.theme === "light" ? "light" : "dark";
    const labels = opts.labels !== false;

    const holder = document.createElement("div");
    holder.style.cssText = "position:absolute;left:-99999px;top:0";
    document.body.appendChild(holder);

    const sketch = (p) => {
      p.setup = () => {
        const cv = p.createCanvas(W, H);
        p.pixelDensity(1);
        p.colorMode(p.HSL, 360, 100, 100, 1);
        try { drawAll(p, info, theme, W, H, labels); } catch (e) { /* keep partial */ }
        let url = null;
        try { url = cv.elt.toDataURL("image/png"); } catch (e) {}
        p.noLoop();
        p.remove();
        holder.remove();
        resolve(url);
      };
    };
    new p5(sketch, holder);
  });
}

const compact = (n) => {
  n = Number(n) || 0;
  const f = (v, u) => (v >= 100 ? Math.round(v) : v >= 10 ? Math.round(v * 10) / 10 : Math.round(v * 100) / 100) + u;
  if (n >= 1e9) return f(n / 1e9, "B");
  if (n >= 1e6) return f(n / 1e6, "M");
  if (n >= 1e4) return f(n / 1e3, "k");
  return n.toLocaleString("en-US");
};
const plural = (n, one, many) => `${compact(n)} ${n === 1 ? one : many}`;
function predName(iri) {
  const s = String(iri).replace(/^<|>$/g, "");
  const m = s.match(/[#/]([^#/]+)\/?$/);
  const n = m ? m[1] : s;
  return n.length > 24 ? n.slice(0, 23) + "…" : n;
}

function drawAll(p, info, theme, W, H, labels) {
  const S = W / 100; // unit for sizes; X maps by S, Y by sy (canvas is 16:10)
  const sy = H / 100;
  const seedStr = String(info.seed || "rete");
  const sd = hash32(seedStr);
  p.randomSeed(sd);
  p.noiseSeed(sd);

  const graph = buildSchemaGraph(info);
  const base = graph ? graph.bgHue : hueOf((info.vocabularies && info.vocabularies[0]) || seedStr);
  const pal = palette(theme, base);
  const box = info.mode === "ontology"; // ontologies: square nodes (differentiator)
  const hasProps = !!(info.predicates && info.predicates.length);

  drawPaper(p, pal, W, H, S);
  if (info.geoWkt) drawGeoShadow(p, pal, W, H, info.bbox);
  const facts = [];
  if (graph) {
    // Nodes keep clear of the caption line at the bottom.
    for (const n of graph.nodes) { n.px = n.x * S; n.py = (45 + (n.y - 50) * 0.76) * sy; n.pr = n.r * S; }
    drawSchema(p, graph, pal, S, labels, W, H, box);
    facts.push(plural(graph.classTotal, "class", "classes"));
    if (graph.relationTotal) facts.push(plural(graph.relationTotal, "relation", "relations"));
  } else if (hasProps) {
    drawProfile(p, info.predicates, pal, W, H, S, labels);
    facts.push(plural(info.predicates.length, "property", "properties"));
  } else {
    drawCounts(p, info, pal, W, H, S);
  }
  if (info.triples > 1 && (graph || hasProps)) facts.push(`${compact(info.triples)} triples`);
  drawFrame(p, pal, W, H);
  if (labels && info.temporalExtent) drawCorner(p, `${fmtTime(info.temporalExtent[0])} to ${fmtTime(info.temporalExtent[1])}`, pal, W, H, S);
  if (labels && facts.length) drawCaption(p, facts.join("  ·  "), pal, W, H, S);
}

// --- aged parchment --------------------------------------------------------
function drawPaper(p, pal, W, H, S) {
  p.noStroke();
  p.background(pal.paperH, pal.paperS, pal.paperL, 1);
  // tonal unevenness
  for (let i = 0; i < 6; i++) {
    p.fill(pal.paperH, pal.paperS, clampL(pal.paperL + p.random(-6, 5)), 0.22);
    p.ellipse(p.random(W), p.random(H), p.random(W * 0.3, W * 0.7));
  }
  // foxing — small irregular age spots
  for (let i = 0; i < 26; i++) {
    const x = p.random(W), y = p.random(H), s = p.random(1.5, 5) * S * 0.5;
    p.fill(pal.foxH, pal.foxS, pal.foxL, p.random(0.03, 0.11));
    for (let k = 0; k < 3; k++) p.ellipse(x + p.random(-s, s), y + p.random(-s, s), p.random(s * 0.6, s * 1.4));
  }
  // fibres
  p.strokeWeight(1);
  for (let i = 0; i < 240; i++) {
    const x = p.random(W), y = p.random(H), a = p.random(Math.PI), len = p.random(2, 8);
    p.stroke(pal.paperH, pal.paperS, clampL(pal.paperL + p.random(-12, 9)), 0.09);
    p.line(x, y, x + Math.cos(a) * len, y + Math.sin(a) * len);
  }
  // tooth speckle
  p.noStroke();
  const cnt = Math.floor((W * H) / 95);
  for (let i = 0; i < cnt; i++) {
    const x = p.random(W), y = p.random(H), n = p.noise(x * 0.05, y * 0.05);
    if (n > 0.54) { p.fill(pal.paperH, 12, pal.grain, 0.05 * (n - 0.4)); p.rect(x, y, 1.2, 1.2); }
  }
  // aged-edge vignette
  const ctx = p.drawingContext;
  const g = ctx.createRadialGradient(W / 2, H * 0.46, W * 0.12, W / 2, H * 0.5, W * 0.8);
  g.addColorStop(0, "rgba(0,0,0,0)");
  g.addColorStop(1, pal.vignette);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, W, H);
}

// --- geo: a faint "shadow" silhouette of the region (a few combined organic
// blobs, proportioned to the WKT bounding box) --------------------------------
function drawGeoShadow(p, pal, W, H, bbox) {
  let ar = 1.6;
  if (bbox && bbox.length === 4) {
    const lon = Math.abs(bbox[2] - bbox[0]) || 1, lat = Math.abs(bbox[3] - bbox[1]) || 1;
    ar = clamp(lon / lat, 0.5, 3.2);
  }
  const cx = W * 0.5, cy = H * 0.46;
  const rw = W * 0.36, rh = rw / ar;
  const sh = pal.shadow;
  p.noStroke();
  p.fill(sh.h, sh.s, sh.l, sh.a);
  const blobs = 3 + Math.floor(p.random() * 3);
  for (let b = 0; b < blobs; b++) {
    const ox = cx + (p.random() - 0.5) * rw * 0.7;
    const oy = cy + (p.random() - 0.5) * rh * 0.7;
    const segs = 16, verts = [];
    for (let i = 0; i < segs; i++) {
      const a = (i / segs) * TAU;
      const rr = 0.4 + 0.6 * p.noise(Math.cos(a) * 1.3 + b * 5, Math.sin(a) * 1.3 + b * 5);
      verts.push([ox + Math.cos(a) * rr * rw * 0.5, oy + Math.sin(a) * rr * rh * 0.5]);
    }
    p.beginShape();
    p.curveVertex(verts[verts.length - 1][0], verts[verts.length - 1][1]);
    for (const v of verts) p.curveVertex(v[0], v[1]);
    p.curveVertex(verts[0][0], verts[0][1]);
    p.curveVertex(verts[1][0], verts[1][1]);
    p.endShape(p.CLOSE);
  }
}

function fmtTime(v) {
  const s = String(v == null ? "" : v);
  const m = s.match(/-?\d{1,6}/);
  return m ? m[0] : s.slice(0, 10);
}
// --- ruled plate border ----------------------------------------------------
function drawFrame(p, pal, W, H) {
  const m = Math.round(Math.min(W, H) * 0.045);
  p.noFill();
  p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.7);
  p.strokeWeight(1.3);
  p.rect(m, m, W - 2 * m, H - 2 * m);
  p.strokeWeight(0.5);
  p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.5);
  p.rect(m + 3, m + 3, W - 2 * m - 6, H - 2 * m - 6);
}

// --- plate caption: the counts, centred above the bottom rule ----------------
function drawCaption(p, text, pal, W, H, S) {
  const m = Math.round(Math.min(W, H) * 0.045);
  const by = H - m - 3.6 * S;
  p.textFont(SERIF);
  p.textStyle(p.ITALIC);
  p.textSize(3.7 * S);
  const tw = p.textWidth(text);
  p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.4);
  p.strokeWeight(0.5);
  p.line(W / 2 - tw / 2, by - 4.6 * S, W / 2 + tw / 2, by - 4.6 * S);
  p.noStroke();
  p.fill(pal.label.h, pal.label.s, pal.label.l, 0.9);
  p.textAlign(p.CENTER, p.BASELINE);
  p.text(text, W / 2, by);
}

// Top-right corner note (the time span). Top-left is left free for the page's
// own badge.
function drawCorner(p, text, pal, W, H, S) {
  const mg = Math.round(Math.min(W, H) * 0.045) + 1.6 * S;
  p.noStroke();
  p.fill(pal.label.h, pal.label.s, pal.label.l, 0.72);
  p.textFont(SERIF);
  p.textStyle(p.ITALIC);
  p.textAlign(p.RIGHT, p.TOP);
  p.textSize(3.4 * S);
  p.text(text, W - mg, mg);
}

// --- schema plate ----------------------------------------------------------
function drawSchema(p, graph, pal, S, labels, W, H, box) {
  const { nodes, edges } = graph;
  for (const e of edges) drawInkEdge(p, nodes[e.a], nodes[e.b], e, pal, S);
  for (const n of nodes) if (n.loop) drawLoop(p, n, pal, S, W, H);
  for (const n of nodes) drawSpecimen(p, n.px, n.py, n.pr, n.hue, pal, S, box);
  if (!labels) return;

  // Labels never overlap a node or each other: every placed box is remembered,
  // and a label that finds no free spot is left out (the biggest class always
  // gets one).
  const placed = nodes.map((n) => ({ x0: n.px - n.pr, y0: n.py - n.pr, x1: n.px + n.pr, y1: n.py + n.pr, node: n }));
  placed.push({ x0: 0, y0: 0, x1: W * 0.27, y1: H * 0.2 }); // the page's badge sits here
  placed.push({ x0: W * 0.7, y0: 0, x1: W, y1: H * 0.13 }); // the time span
  const big = W >= 800; // the dataset page draws at twice the resolution: a few more labels fit
  const k = big ? 0.9 : 1;
  const ranked = nodes.slice().sort((a, b) => b.pr - a.pr);
  ranked.slice(0, big ? 9 : 7).forEach((n, i) => {
    let sub = n.count && !box ? compact(n.count) : "";
        drawNodeLabel(p, n, n.label, sub, (i < 2 ? 5.4 : 4.2) * k, pal, S, i < 2, W, H, placed, i === 0);
  });
  // Name the strongest relations.
  const named = edges.slice().sort((a, b) => b.w - a.w).slice(0, big ? 6 : 4);
  for (const e of named) {
    const a = nodes[e.a], b = nodes[e.b];
    for (const t of [0.5, 0.36, 0.64]) {
      if (drawFloatLabel(p, e.pred, a.px + (b.px - a.px) * t, a.py + (b.py - a.py) * t, 3.3 * k, pal, S, W, H, placed)) break;
    }
  }
  for (const n of nodes) if (n.loop) drawFloatLabel(p, n.loop.pred, n.loopX, n.loopY, 3.3 * k, pal, S, W, H, placed);
}

// A link from a class to itself: a small ring on the node's outer side.
function drawLoop(p, n, pal, S, W, H) {
  let dx = n.px - W / 2, dy = n.py - H * 0.46;
  const d = Math.hypot(dx, dy);
  if (d < 1) { dx = 0.5; dy = -0.87; } else { dx /= d; dy /= d; }
  const lr = Math.max(n.pr * 0.62, 2.2 * S);
  const lx = n.px + dx * (n.pr + lr * 0.55), ly = n.py + dy * (n.pr + lr * 0.55);
  p.noFill();
  p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.62);
  p.strokeWeight(Math.max(0.6, S * 0.24));
  p.circle(lx, ly, lr * 2);
  n.loopX = lx + dx * (lr + 2.6 * S);
  n.loopY = ly + dy * (lr + 2.6 * S);
}

// --- no classes in the card: its most used properties, as bars ---------------
function drawProfile(p, predicates, pal, W, H, S, labels) {
  const rows = predicates.slice().sort((a, b) => b[1] - a[1]).slice(0, 6);
  const max = Math.max(1, ...rows.map((r) => r[1]));
  const x0 = W * 0.13, x1 = W * 0.87;
  const top = H * 0.2, step = (H * 0.56) / rows.length;
  p.textFont(SERIF);
  rows.forEach(([iri, n], i) => {
    const y = top + i * step;
    const w = Math.max(1.5 * S, (x1 - x0) * Math.sqrt(n / max));
    const hue = hueOf(String(iri).replace(/^<|>$/g, "").replace(/[^#/]+$/, ""));
    p.noStroke();
    p.fill(hue, pal.washS, pal.washL, 0.55);
    p.rect(x0, y + step * 0.5, w, step * 0.26, S * 0.4);
    if (!labels) return;
    p.fill(pal.label.h, pal.label.s, pal.label.l, 0.95);
    p.textStyle(p.ITALIC);
    p.textSize(3.9 * S);
    p.textAlign(p.LEFT, p.BASELINE);
    p.text(predName(iri), x0, y + step * 0.4);
    p.fill(pal.label.h, pal.label.s, pal.label.l, 0.62);
    p.textSize(3.4 * S);
    p.textAlign(p.RIGHT, p.BASELINE);
    p.text(compact(n), x1, y + step * 0.4);
  });
}

// --- no card in the file: what the header alone says -------------------------
function drawCounts(p, info, pal, W, H, S) {
  p.noStroke();
  p.textFont(SERIF);
  p.textAlign(p.CENTER, p.BASELINE);
  p.fill(pal.label.h, pal.label.s, pal.label.l, 0.95);
  p.textStyle(p.BOLD);
  p.textSize(11 * S);
  p.text(info.triples > 1 ? compact(info.triples) : "?", W / 2, H * 0.5);
  p.textStyle(p.ITALIC);
  p.textSize(4 * S);
  p.fill(pal.label.h, pal.label.s, pal.label.l, 0.75);
  p.text("triples" + (info.terms ? `  ·  ${compact(info.terms)} terms` : ""), W / 2, H * 0.5 + 6.5 * S);
  p.fill(pal.label.h, pal.label.s, pal.label.l, 0.5);
  p.textSize(3.3 * S);
  p.text("no schema profile in this file", W / 2, H * 0.5 + 12 * S);
}

// --- a node: colour wash + clipped cross-hatching + stipple + ink outline.
// Datasets get an organic round "specimen"; ontologies get a square "box". -----
function drawSpecimen(p, cx, cy, r, hue, pal, S, box) {
  const ctx = p.drawingContext;
  const h = hue == null ? pal.neutralH : hue;
  let verts = null, bx, by, bw, brr;
  if (box) {
    bw = 2 * r; bx = cx - r; by = cy - r; brr = Math.max(1.2, r * 0.3);
  } else {
    verts = [];
    const segs = 30;
    for (let i = 0; i < segs; i++) {
      const ang = (i / segs) * Math.PI * 2;
      const rr = r * (1 + (p.noise(Math.cos(ang) * 1.4 + cx * 0.02, Math.sin(ang) * 1.4 + cy * 0.02) - 0.5) * 0.14);
      verts.push([cx + Math.cos(ang) * rr, cy + Math.sin(ang) * rr]);
    }
  }
  // soft colour wash
  p.noStroke();
  p.fill(h, pal.washS, pal.washL, 0.5);
  if (box) p.rect(bx, by, bw, bw, brr); else blob(p, verts);

  // engraving: hatch + cross-hatch, clipped to the node
  const ink = (a) => `hsla(${pal.inkH}, ${pal.inkS}%, ${pal.inkL}%, ${a})`;
  ctx.save();
  if (box) ctxRoundRect(ctx, bx, by, bw, bw, brr); else ctxBlobPath(ctx, verts);
  ctx.clip();
  const R = r + 3;
  const sp = Math.max(1.4, S * 0.34);
  ctx.translate(cx, cy);
  ctx.rotate(-Math.PI / 4 + (p.noise(cx, cy) - 0.5) * 0.5);
  ctx.lineWidth = Math.max(0.4, S * 0.1);
  for (let x = -R; x <= R; x += sp) {
    ctx.strokeStyle = ink(0.12 + 0.5 * ((x + R) / (2 * R)));
    ctx.beginPath(); ctx.moveTo(x, -R); ctx.lineTo(x, R); ctx.stroke();
  }
  ctx.rotate(Math.PI / 2);
  for (let x = 0; x <= R; x += sp * 1.5) {
    ctx.strokeStyle = ink(0.1 + 0.25 * (x / R));
    ctx.beginPath(); ctx.moveTo(x, -R); ctx.lineTo(x, R); ctx.stroke();
  }
  ctx.restore();
  // stipple flecks
  p.noStroke();
  p.fill(pal.inkH, pal.inkS, pal.inkL, 0.5);
  const dots = Math.round(r * 0.8);
  for (let i = 0; i < dots; i++) {
    const a = p.random() * Math.PI * 2, rad = Math.sqrt(p.random()) * r * 0.85;
    p.circle(cx + Math.cos(a) * rad, cy + Math.sin(a) * rad, S * 0.18);
  }
  // confident ink outline
  p.noFill();
  p.strokeJoin(p.ROUND);
  if (box) {
    p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.85);
    p.strokeWeight(Math.max(0.5, S * 0.14));
    p.rect(bx, by, bw, bw, brr);
  } else {
    for (let k = 0; k < 2; k++) {
      p.stroke(pal.inkH, pal.inkS, pal.inkL, k === 0 ? 0.85 : 0.3);
      p.strokeWeight(Math.max(0.5, S * (k === 0 ? 0.14 : 0.22)));
      blob(p, verts.map((v) => [v[0] + (p.random() - 0.5) * 0.25, v[1] + (p.random() - 0.5) * 0.25]));
    }
  }
}

function ctxRoundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function drawInkEdge(p, a, b, e, pal, S) {
  const dx = b.px - a.px, dy = b.py - a.py, len = Math.hypot(dx, dy) || 1;
  const bow = (p.random() - 0.5) * (len * 0.08 + 2 * S * 0.5);
  p.noFill();
  p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.2 + (e.wn || 0.3) * 0.34);
  p.strokeWeight(Math.max(0.4, (0.4 + (e.wn || 0.3) * 1.1) * S * 0.32));
  p.strokeCap(p.ROUND);
  const pts = [];
  const steps = 14;
  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    const perp = Math.sin(t * Math.PI) * bow;
    const jx = (p.noise(t * 3 + a.px * 0.02, a.py * 0.02) - 0.5) * 1.6;
    const jy = (p.noise(t * 3 + b.px * 0.02, b.py * 0.02) - 0.5) * 1.6;
    pts.push([a.px + dx * t + (-dy / len) * perp + jx, a.py + dy * t + (dx / len) * perp + jy]);
  }
  p.beginShape();
  p.curveVertex(pts[0][0], pts[0][1]);
  for (const pt of pts) p.curveVertex(pt[0], pt[1]);
  p.curveVertex(pts[pts.length - 1][0], pts[pts.length - 1][1]);
  p.endShape();
  // arrowhead where the line meets the target class
  if (e.pred && len > b.pr + 3 * S) {
    const ux = dx / len, uy = dy / len, hs = Math.max(1.3 * S, 3);
    const tx = b.px - ux * (b.pr + 0.5 * S), ty = b.py - uy * (b.pr + 0.5 * S);
    p.noStroke();
    p.fill(pal.inkH, pal.inkS, pal.inkL, 0.5 + (e.wn || 0.3) * 0.35);
    p.triangle(tx, ty, tx - ux * hs - uy * hs * 0.45, ty - uy * hs + ux * hs * 0.45, tx - ux * hs + uy * hs * 0.45, ty - uy * hs - ux * hs * 0.45);
  }
}

function blob(p, vpx) {
  p.beginShape();
  p.curveVertex(vpx[vpx.length - 1][0], vpx[vpx.length - 1][1]);
  for (const v of vpx) p.curveVertex(v[0], v[1]);
  p.curveVertex(vpx[0][0], vpx[0][1]);
  p.curveVertex(vpx[1][0], vpx[1][1]);
  p.endShape();
}
function ctxBlobPath(ctx, vpx) {
  ctx.beginPath();
  ctx.moveTo(vpx[0][0], vpx[0][1]);
  for (let i = 1; i < vpx.length; i++) ctx.lineTo(vpx[i][0], vpx[i][1]);
  ctx.closePath();
}

// --- labels ----------------------------------------------------------------
const hits = (r, placed, skip) => placed.some((q) => q.node !== skip && r.x0 < q.x1 && r.x1 > q.x0 && r.y0 < q.y1 && r.y1 > q.y0);

function inkText(p, text, x, y, size, pal, alpha) {
  p.noFill();
  p.stroke(pal.paperH, pal.paperS, clampL(pal.paperL + 3), 0.9);
  p.strokeWeight(size * 0.2);
  p.strokeJoin(p.ROUND);
  p.text(text, x, y);
  p.noStroke();
  p.fill(pal.label.h, pal.label.s, pal.label.l, alpha);
  p.text(text, x, y);
}

// A class name with its instance count, set beside the node where there is
// room (near first, then further out, eight directions) and joined to it by a
// leader line.
function drawNodeLabel(p, node, text, sub, fs, pal, S, bold, W, H, placed, force) {
  const margin = Math.round(Math.min(W, H) * 0.045) + 1.2 * S;
  const size = fs * S, subSize = fs * 0.76 * S;
  p.textFont(SERIF);
  p.textStyle(bold ? p.BOLDITALIC : p.ITALIC);
  p.textSize(size);
  const tw = p.textWidth(text);
  p.textStyle(p.NORMAL);
  p.textSize(subSize);
  const sw = sub ? p.textWidth(sub) + 1.1 * S : 0;
  const w = tw + sw, h = size * 1.15;
  const sx = node.px >= W / 2 ? 1 : -1, sy2 = node.py >= H * 0.46 ? 1 : -1;
  const dirs = [[sx, 0], [-sx, 0], [sx, sy2], [sx, -sy2], [0, sy2], [0, -sy2], [-sx, sy2], [-sx, -sy2]];
  const boxAt = (dx, dy, dist) => {
    const n = Math.hypot(dx, dy);
    const ax = node.px + (dx / n) * dist, ay = node.py + (dy / n) * dist; // leader end
    const x = dx > 0 ? ax + 0.6 * S : dx < 0 ? ax - 0.6 * S - w : ax - w / 2;
    const y = dx === 0 ? ay + dy * h * 0.55 : ay;
    return { ax, ay, x, y, r: { x0: x - S * 0.6, y0: y - h / 2, x1: x + w + S * 0.6, y1: y + h / 2 } };
  };
  const ok = (r) => r.x0 >= margin && r.x1 <= W - margin && r.y0 >= margin && r.y1 <= H - margin - 8.5 * S && !hits(r, placed, node);
  let pick = null;
  search: for (const extra of [1.6, 5.5, 10.5, 16]) {
    for (const [dx, dy] of dirs) {
      const b = boxAt(dx, dy, node.pr + extra * S);
      if (ok(b.r)) { pick = b; break search; }
    }
  }
  if (!pick) {
    if (!force) return false;
    pick = boxAt(0, 1, node.pr + 1.6 * S);
    pick.x = clamp(pick.x, margin, W - margin - w);
    pick.r = { x0: pick.x, y0: pick.y - h / 2, x1: pick.x + w, y1: pick.y + h / 2 };
  }
  placed.push(pick.r);
  // leader, from the node's rim
  const ldx = pick.ax - node.px, ldy = pick.ay - node.py, ll = Math.hypot(ldx, ldy) || 1;
  if (ll > node.pr + 2.4 * S) {
    p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.5);
    p.strokeWeight(0.6);
    p.line(node.px + (ldx / ll) * node.pr, node.py + (ldy / ll) * node.pr, pick.ax, pick.ay);
  }
  p.textAlign(p.LEFT, p.CENTER);
  p.textStyle(bold ? p.BOLDITALIC : p.ITALIC);
  p.textSize(size);
  inkText(p, text, pick.x, pick.y, size, pal, 1);
  if (sub) {
    p.textStyle(p.NORMAL);
    p.textSize(subSize);
    inkText(p, sub, pick.x + tw + 1.1 * S, pick.y + (size - subSize) * 0.18, subSize, pal, 0.7);
  }
  return true;
}

// A relation name, centred on a point; nudged up or down if the spot is taken,
// dropped if neither is free.
function drawFloatLabel(p, text, x, y, fs, pal, S, W, H, placed) {
  if (!text || x == null) return false;
  const margin = Math.round(Math.min(W, H) * 0.045) + 1.2 * S;
  const size = fs * S;
  p.textFont(SERIF);
  p.textStyle(p.ITALIC);
  p.textSize(size);
  const w = p.textWidth(text), h = size * 1.15;
  for (const dy of [0, -h, h]) {
    const r = { x0: x - w / 2 - S * 0.5, y0: y + dy - h / 2, x1: x + w / 2 + S * 0.5, y1: y + dy + h / 2 };
    if (r.x0 < margin || r.x1 > W - margin || r.y0 < margin || r.y1 > H - margin - 8.5 * S) continue;
    if (hits(r, placed, null)) continue;
    placed.push(r);
    p.textAlign(p.CENTER, p.CENTER);
    inkText(p, text, x, y + dy, size, pal, 0.8);
    return true;
  }
  return false;
}

const clampL = (l) => Math.max(0, Math.min(100, l));
