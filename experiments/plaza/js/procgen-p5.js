// procgen-p5.js — render a dataset's schema as a hand-coloured engraving, the way
// a specimen plate looks in an old natural-history book: aged/foxed parchment, a
// ruled plate border, "specimens" (the classes) shaded with fine ink
// cross-hatching + stipple over a muted colour wash, italic serif labels with
// leader lines, and a plate caption. The graph is still the ontology: nodes =
// card classes (size ∝ instances), edges = class_links, hue = vocabulary.
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
    paperH: 30, paperS: 26, paperL: 10, // aged-at-night
    foxH: 30, foxS: 40, foxL: 30,
    inkH: 38, inkS: 30, inkL: 80, // warm cream pen
    washS: 50, washL: 52,
    neutralH: 35, neutralS: 16, neutralL: 52,
    label: { h: 40, s: 30, l: 88 },
    vignette: "rgba(0,0,0,0.5)",
    grain: 92,
    shadow: { h: 40, s: 20, l: 90, a: 0.11 }, // geo region shadow (lightens dark paper)
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
  const cx = W / 2, cy = H / 2;

  drawPaper(p, pal, W, H, S);
  // Background motifs from the card's signals (temporal first = furthest back).
  if (info.temporal && info.temporalExtent) drawTemporalLine(p, pal, W, H, S, info.temporalExtent);
  if (info.geoWkt) drawGeoShadow(p, pal, W, H, info.bbox);
  const box = info.mode === "ontology"; // ontologies: square nodes (differentiator)
  if (graph) {
    for (const n of graph.nodes) { n.px = n.x * S; n.py = n.y * sy; n.pr = n.r * S; }
    drawSchema(p, graph, pal, S, labels, cx, cy, box);
  } else {
    drawAbstract(p, info, pal, S, sy, labels, cx, cy, box);
  }
  drawFrame(p, pal, W, H);
  drawPlateNumber(p, sd, pal, W, H, S);
  if (labels && info.name) drawCaption(p, info.name, pal, W, H, S, !!(info.temporal && info.temporalExtent));
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

// --- temporal: a faint background timeline (simple line) spanning the extent,
// with start / middle / end labels. Drawn furthest back. -----------------------
function drawTemporalLine(p, pal, W, H, S, extent) {
  const m = Math.round(Math.min(W, H) * 0.045);
  const x0 = m + 4 * S, x1 = W - m - 4 * S;
  const labelY = H - m - 3.5 * S; // labels hug the bottom frame border
  const baseY = labelY - 3 * S; // the axis line sits just above its labels
  const amp = 6 * S;
  // simple line (sparkline), faint
  p.noFill();
  p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.26);
  p.strokeWeight(Math.max(0.6, S * 0.12));
  p.beginShape();
  const n = 40;
  for (let i = 0; i <= n; i++) {
    p.vertex(x0 + (x1 - x0) * (i / n), baseY - p.noise(i * 0.18 + 5) * amp);
  }
  p.endShape();
  // start / middle / end labels
  const a = numOf(extent[0]), bv = numOf(extent[1]);
  const mid = a != null && bv != null ? String(Math.round((a + bv) / 2)) : null;
  p.noStroke();
  p.fill(pal.label.h, pal.label.s, pal.label.l, 0.6);
  p.textFont(SERIF);
  p.textStyle(p.ITALIC);
  p.textSize(2.7 * S);
  p.textAlign(p.LEFT, p.TOP); p.text(fmtTime(extent[0]), x0, labelY);
  if (mid != null) { p.textAlign(p.CENTER, p.TOP); p.text(mid, (x0 + x1) / 2, labelY); }
  p.textAlign(p.RIGHT, p.TOP); p.text(fmtTime(extent[1]), x1, labelY);
}

function fmtTime(v) {
  const s = String(v == null ? "" : v);
  const m = s.match(/-?\d{1,6}/);
  return m ? m[0] : s.slice(0, 10);
}
function numOf(v) {
  const m = String(v).match(/-?\d+/);
  return m ? parseInt(m[0], 10) : null;
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

// --- plate caption ---------------------------------------------------------
function drawCaption(p, name, pal, W, H, S, raised) {
  const m = Math.round(Math.min(W, H) * 0.045);
  const cy = H - m - (raised ? 15 : 4) * S; // lift above the timeline axis when present
  p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.45);
  p.strokeWeight(0.5);
  p.line(W * 0.34, cy - 5 * S * 0.5, W * 0.66, cy - 5 * S * 0.5);
  p.noStroke();
  p.fill(pal.label.h, pal.label.s, pal.label.l, 0.92);
  p.textFont(SERIF);
  p.textStyle(p.ITALIC);
  p.textAlign(p.CENTER, p.BASELINE);
  p.textSize(3.6 * S);
  p.text(name, W / 2, cy);
}

function roman(n) {
  const m = [[10, "X"], [9, "IX"], [5, "V"], [4, "IV"], [1, "I"]];
  let r = "";
  for (const [v, s] of m) while (n >= v) { r += s; n -= v; }
  return r;
}

// A plate number in the top-left corner, the way old specimen plates are numbered.
function drawPlateNumber(p, sd, pal, W, H, S) {
  const n = (sd % 24) + 1;
  const mg = Math.round(Math.min(W, H) * 0.045) + 5;
  p.noStroke();
  p.fill(pal.label.h, pal.label.s, pal.label.l, 0.8);
  p.textFont(SERIF);
  p.textStyle(p.ITALIC);
  p.textAlign(p.LEFT, p.TOP);
  p.textSize(2.9 * S);
  p.text("Pl. " + roman(n), mg, mg);
}

// --- schema (ontology) plate ----------------------------------------------
function drawSchema(p, graph, pal, S, labels, cx, cy, box) {
  const { nodes, edges } = graph;
  for (const e of edges) drawInkEdge(p, nodes[e.a], nodes[e.b], e, pal, S);
  for (const n of nodes) drawSpecimen(p, n.px, n.py, n.pr, n.hue, pal, S, box);
  if (!labels) return;
  const ranked = nodes.slice().sort((a, b) => b.pr - a.pr);
  ranked.slice(0, 2).forEach((n) => drawNodeLabel(p, n, n.label, 5.0, pal, S, true, cx, cy));
  ranked.slice(2, 6).forEach((n) => drawNodeLabel(p, n, n.label, 3.4, pal, S, false, cx, cy));
}

// --- abstract plate (header-only: no schema) -------------------------------
function drawAbstract(p, info, pal, S, sy, labels, cx, cy, box) {
  const n = Math.round(clamp(5 + Math.log10(Math.max(1, info.triples)) * 4.5, 6, 22));
  const nodes = [];
  for (let i = 0; i < n; i++) {
    const a = p.random() * Math.PI * 2, r = Math.pow(p.random(), 0.6) * 36;
    const x = 50 + Math.cos(a) * r + (p.random() - 0.5) * 8;
    const y = 50 + Math.sin(a) * r + (p.random() - 0.5) * 8;
    nodes.push({ px: x * S, py: y * sy, pr: (1.4 + Math.pow(p.random(), 2) * 3.2) * S, hue: (pal.paperH + Math.floor(p.random() * 120) - 60 + 360) % 360 });
  }
  for (let i = 0; i < nodes.length; i++) {
    let best = -1, bd = Infinity;
    for (let j = 0; j < nodes.length; j++) {
      if (j === i) continue;
      const d = (nodes[i].px - nodes[j].px) ** 2 + (nodes[i].py - nodes[j].py) ** 2;
      if (d < bd) { bd = d; best = j; }
    }
    if (best >= 0) drawInkEdge(p, nodes[i], nodes[best], { wn: 0.4 }, pal, S);
  }
  for (const nd of nodes) drawSpecimen(p, nd.px, nd.py, nd.pr, nd.hue, pal, S, box);
  if (labels) {
    const tags = (info.tags || []).slice(0, 3);
    tags.forEach((t, i) => drawNodeLabel(p, nodes[i % nodes.length], t, 3.2, pal, S, false, cx, cy));
  }
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

// --- labels: full-opaque italic serif set out to the side of the specimen,
// joined to it by a short leader tick (so labels never overlap the nodes). -----
function drawNodeLabel(p, node, text, fs, pal, S, bold, cx, cy) {
  const W = 2 * cx, H = 2 * cy, margin = cx * 0.06;
  const size = fs * S;
  p.textFont(SERIF);
  p.textStyle(bold ? p.BOLDITALIC : p.ITALIC);
  p.textSize(size);
  const tw = p.textWidth(text);
  const gap = node.pr + 5 * S * 0.5;
  // Choose the side that actually has room for the whole word; flip if it'd clip.
  const roomRight = W - margin - (node.px + gap);
  const roomLeft = node.px - gap - margin;
  let side = node.px >= cx ? 1 : -1;
  if (side > 0 && roomRight < tw) side = roomLeft > roomRight ? -1 : 1;
  else if (side < 0 && roomLeft < tw) side = roomRight > roomLeft ? 1 : -1;
  const ex = node.px + side * node.pr;
  const lx = node.px + side * gap;
  const ly = Math.max(size, Math.min(H - size * 0.6, node.py));
  // leader tick + a small dot on the node
  p.stroke(pal.inkH, pal.inkS, pal.inkL, 0.6);
  p.strokeWeight(0.5);
  p.line(ex, node.py, lx, ly);
  p.noStroke();
  p.fill(pal.inkH, pal.inkS, pal.inkL, 0.75);
  p.circle(ex, node.py, S * 0.45);
  // tilted (≥5°) text, aligned outward; pale outline then full-opaque ink fill
  const ang = ((p.random() < 0.5 ? -1 : 1) * (5 + p.random() * 6)) * Math.PI / 180;
  p.push();
  p.translate(lx + side * 1.2, ly);
  p.rotate(ang);
  p.textAlign(side > 0 ? p.LEFT : p.RIGHT, p.CENTER);
  p.noFill();
  p.stroke(pal.paperH, pal.paperS, clampL(pal.paperL + 6), 1);
  p.strokeWeight(size * 0.18);
  p.strokeJoin(p.ROUND);
  p.text(text, 0, 0);
  p.noStroke();
  p.fill(pal.label.h, pal.label.s, pal.label.l, 1);
  p.text(text, 0, 0);
  p.pop();
}

const clampL = (l) => Math.max(0, Math.min(100, l));
