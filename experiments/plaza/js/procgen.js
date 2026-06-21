// procgen.js — a "semantic fingerprint" image generated from a dataset's own
// profile. When the card carries a schema (classes + class_links) the picture is
// a literal **portrait of that schema**:
//   • each disc is a class, its area ∝ instance count;
//   • each line is a real class-to-class relation (card.class_links), weight ∝ count;
//   • colour is the class's vocabulary (namespace → stable hue);
//   • a deterministic force layout (seeded by the file's blake3 hash) clusters
//     connected classes;
//   • class names are drawn with depth — one or two large in front, a few small,
//     blurred and faded in the back (a cheap 3D parallax);
//   • motifs overlay detected signals (geo graticule, temporal sweep, incoherence);
//   • a film grain keeps the flat vectors from looking sterile.
// The whole image adapts to the light/dark theme. Files with no embedded schema
// fall back to an abstract constellation. Everything is deterministic (no
// Date/random): same dataset + same theme → same image.

// --- seeded PRNG (xmur3 string hash -> mulberry32) -------------------------
function xmur3(str) {
  let h = 1779033703 ^ str.length;
  for (let i = 0; i < str.length; i++) {
    h = Math.imul(h ^ str.charCodeAt(i), 3432918353);
    h = (h << 13) | (h >>> 19);
  }
  return () => {
    h = Math.imul(h ^ (h >>> 16), 2246822507);
    h = Math.imul(h ^ (h >>> 13), 3266489909);
    return (h ^= h >>> 16) >>> 0;
  };
}
function mulberry32(a) {
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const TAU = Math.PI * 2;
const clamp = (x, lo, hi) => Math.max(lo, Math.min(hi, x));
export const hueOf = (s) => xmur3("ns:" + s)() % 360;

function namespaceOf(iri) {
  const s = String(iri).replace(/^<|>$/g, "");
  const m = s.match(/^(.*[#/])[^#/]+$/);
  return m ? m[1] : null; // null for sentinels like "(literal)"
}
function localName(iri) {
  const s = String(iri).replace(/^[<(]|[>)]$/g, "");
  const m = s.match(/[#/]([^#/]+)\/?$/);
  return (m ? m[1] : s).slice(0, 18);
}

// Theme palette derived from the dataset's dominant vocabulary hue.
function palette(theme, base) {
  if (theme === "light")
    return {
      bg0: `hsl(${base} 52% 96%)`,
      bg1: `hsl(${(base + 40) % 360} 40% 86%)`,
      ink: `hsl(${base} 40% 30%)`, // edge "pen" colour
      nodeS: 58,
      nodeL: 52,
      neutral: "hsl(35 20% 60%)",
      labelFill: `hsl(${base} 55% 22%)`, // dark ink letters
      labelHalo: `hsl(${(base + 28) % 360} 70% 93%)`, // soft coloured outline
      nodeStroke: `hsl(${base} 40% 28%)`,
      grain: 0.1,
    };
  return {
    bg0: `hsl(${base} 36% 13%)`,
    bg1: `hsl(${(base + 40) % 360} 40% 7%)`,
    ink: `hsl(${base} 30% 84%)`,
    nodeS: 60,
    nodeL: 58,
    neutral: "hsl(35 16% 58%)",
    labelFill: "#f4efe2", // warm cream letters
    labelHalo: `hsl(${base} 65% 12%)`, // dark coloured outline
    nodeStroke: `hsl(${base} 50% 10%)`,
    grain: 0.13,
  };
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Build the image spec from a (full or lite) card + manifest entry.
 *  mode "ontology" sizes nodes by property count (+ square nodes); "dataset"
 *  (default) sizes by class instance count (+ round specimens). */
export function imageInfoFromCard(card, entry, header, mode = "dataset") {
  const sig = (card && card.signals) || {};
  return {
    seed: (header && header.contentHash) || (card && card.content_hash) || entry.key,
    mode,
    name: entry.key, // used to label header-only (schema-less) images
    tags: (entry.tags || []).slice(0, 3),
    triples: (card && (card.triple_count || card.quad_count)) || 1,
    classes: (card && card.classes) || [],
    links: (card && card.class_links) || [],
    vocabularies: (card && card.vocabularies) || [],
    geo: !!(sig.geo_wkt || sig.geo_latlong || (entry.tags || []).includes("geospatial")),
    geoWkt: !!(sig.geo_wkt || entry.geoWkt),
    bbox: sig.spatial_bbox || entry.bbox || null, // [minLon, minLat, maxLon, maxLat]
    temporal:
      !!(sig.temporal_extent || entry.temporalExtent || (sig.time_predicates && sig.time_predicates.length)) ||
      (entry.tags || []).includes("temporal"),
    temporalExtent: sig.temporal_extent || entry.temporalExtent || null, // [min, max]
    incoherent: !!(card && card.coherence && card.coherence.coherent === false),
  };
}

/** SVG string for a dataset. opts.theme = "light"|"dark"; opts.labels (default true for schema). */
export function proceduralSVG(info = {}, opts = {}) {
  const theme = opts.theme === "light" ? "light" : "dark";
  const labels = opts.labels !== false;
  const seedStr = String(info.seed || "rete");
  const rnd = mulberry32(xmur3(seedStr)());
  const uid = "p" + (xmur3(seedStr)() % 1e6).toString(36);
  const grainSeed = xmur3("g" + seedStr)() % 100;
  const roughSeed = xmur3("r" + seedStr)() % 100;

  const graph = buildSchemaGraph(info);
  const base = graph ? graph.bgHue : hueOf(info.vocabularies[0] || seedStr);
  const pal = palette(theme, base);

  const parts = [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" preserveAspectRatio="xMidYMid slice" role="img" aria-label="dataset schema fingerprint">`,
    `<defs><radialGradient id="${uid}bg" cx="50%" cy="38%" r="78%">`,
    `<stop offset="0%" stop-color="${pal.bg0}"/><stop offset="100%" stop-color="${pal.bg1}"/></radialGradient>`,
    `<filter id="${uid}g" x="0" y="0" width="100%" height="100%">`,
    `<feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="${grainSeed}" stitchTiles="stitch" result="n"/>`,
    `<feColorMatrix in="n" type="saturate" values="0"/></filter>`,
    // Roughen: low-frequency turbulence displaces the graph so straight lines and
    // perfect circles become wobbly, hand-drawn strokes (also "noises" the edges).
    `<filter id="${uid}r" x="-6%" y="-6%" width="112%" height="112%">`,
    `<feTurbulence type="turbulence" baseFrequency="0.028" numOctaves="2" seed="${roughSeed}" result="t"/>`,
    `<feDisplacementMap in="SourceGraphic" in2="t" scale="2.2" xChannelSelector="R" yChannelSelector="G"/></filter>`,
    `<filter id="${uid}blur" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="0.35"/></filter>`,
    `</defs>`,
    `<rect width="100" height="100" fill="url(#${uid}bg)"/>`,
  ];

  if (info.geo) {
    parts.push(`<g stroke="${pal.ink}" stroke-width="0.4" fill="none" opacity="0.16">`);
    parts.push(`<ellipse cx="50" cy="50" rx="40" ry="18"/><ellipse cx="50" cy="50" rx="40" ry="32"/>`);
    parts.push(`<line x1="50" y1="8" x2="50" y2="92"/></g>`);
  }
  if (info.temporal) {
    const y = (20 + rnd() * 60).toFixed(1);
    parts.push(`<line x1="2" y1="${y}" x2="98" y2="${y}" stroke="${pal.ink}" stroke-width="0.5" stroke-dasharray="1 3" opacity="0.28"/>`);
  }

  if (graph) renderSchema(parts, graph, pal, uid, labels, seedStr);
  else renderAbstract(parts, info, rnd, pal, uid);

  parts.push(`<rect width="100" height="100" filter="url(#${uid}g)" opacity="${pal.grain}" style="mix-blend-mode:overlay"/>`);

  if (info.incoherent) {
    const cx = (18 + rnd() * 64).toFixed(1),
      cy = (18 + rnd() * 64).toFixed(1);
    parts.push(
      `<circle cx="${cx}" cy="${cy}" r="3.2" fill="none" stroke="hsl(0 75% 62%)" stroke-width="0.9"/>`,
      `<line x1="${cx - 2.2}" y1="${cy - 2.2}" x2="${+cx + 2.2}" y2="${+cy + 2.2}" stroke="hsl(0 75% 62%)" stroke-width="0.9"/>`
    );
  }

  parts.push(`</svg>`);
  return parts.join("");
}

// ---------------------------------------------------------------------------
// Schema portrait
// ---------------------------------------------------------------------------
export function buildSchemaGraph(info, cap = 16) {
  const classCount = new Map();
  for (const [iri, n] of info.classes) classCount.set(iri, n);

  const edgeW = new Map();
  const incident = new Map();
  for (const l of info.links || []) {
    const a = l.s_class,
      b = l.o_class;
    if (!a || !b || a === b) continue;
    const key = a < b ? a + "\t" + b : b + "\t" + a;
    edgeW.set(key, (edgeW.get(key) || 0) + (l.count || 1));
    incident.set(a, (incident.get(a) || 0) + (l.count || 1));
    incident.set(b, (incident.get(b) || 0) + (l.count || 1));
  }
  if (!classCount.size && !edgeW.size) return null;

  const nodeIds = new Map();
  const topClasses = [...classCount.entries()].sort((x, y) => y[1] - x[1]);
  for (const [iri, n] of topClasses.slice(0, Math.ceil(cap / 2))) nodeIds.set(iri, n);
  for (const [key, w] of [...edgeW.entries()].sort((x, y) => y[1] - x[1])) {
    if (nodeIds.size >= cap) break;
    for (const id of key.split("\t")) if (!nodeIds.has(id)) nodeIds.set(id, classCount.get(id) || incident.get(id) || w);
  }
  for (const [iri, n] of topClasses) {
    if (nodeIds.size >= cap) break;
    if (!nodeIds.has(iri)) nodeIds.set(iri, n);
  }

  const ids = [...nodeIds.keys()];
  // Node radius: ontologies size by PROPERTY count (links out of the class);
  // datasets size by class INSTANCE count (the selection mass).
  const propCount = new Map();
  for (const l of info.links || []) if (l.s_class) propCount.set(l.s_class, (propCount.get(l.s_class) || 0) + 1);
  const ont = info.mode === "ontology";
  const dispMass = (id) => (ont ? propCount.get(id) || 1 : nodeIds.get(id) || 1);
  const maxDisp = Math.max(1, ...ids.map(dispMass));
  const nodes = ids.map((id) => {
    const ns = namespaceOf(id);
    return {
      id,
      mass: dispMass(id),
      r: clamp(1.6 + 4.4 * Math.sqrt(dispMass(id) / maxDisp), 1.6, 5.6),
      ns,
      hue: ns ? hueOf(ns) : null,
      label: localName(id),
    };
  });
  const index = new Map(ids.map((id, i) => [id, i]));
  const edges = [];
  let maxW = 1,
    minW = Infinity;
  for (const [key, w] of edgeW) {
    const [a, b] = key.split("\t");
    if (index.has(a) && index.has(b)) {
      edges.push({ a: index.get(a), b: index.get(b), w });
      maxW = Math.max(maxW, w);
      minW = Math.min(minW, w);
    }
  }
  const logMin = Math.log(minW || 1),
    logSpan = Math.max(0.001, Math.log(maxW) - logMin);
  for (const e of edges) e.wn = clamp((Math.log(e.w) - logMin) / logSpan, 0, 1);

  const top = nodes.slice().sort((x, y) => y.mass - x.mass)[0];
  const bgHue = top && top.hue != null ? top.hue : hueOf(info.vocabularies[0] || "rete");

  layout(nodes, edges, mulberry32(xmur3("L" + info.seed)()));
  return { nodes, edges, bgHue };
}

function layout(nodes, edges, rnd, iters = 170) {
  const N = nodes.length;
  nodes.forEach((n, i) => {
    const a = (i / N) * TAU + rnd() * 0.7;
    const r = 20 + rnd() * 8;
    n.x = 50 + Math.cos(a) * r;
    n.y = 50 + Math.sin(a) * r;
    n.vx = n.vy = 0;
  });
  const k = 17;
  for (let it = 0; it < iters; it++) {
    for (let i = 0; i < N; i++)
      for (let j = i + 1; j < N; j++) {
        let dx = nodes[i].x - nodes[j].x,
          dy = nodes[i].y - nodes[j].y;
        const d2 = dx * dx + dy * dy + 0.01,
          d = Math.sqrt(d2);
        const f = ((k * k) / d2) * 0.5;
        dx = (dx / d) * f;
        dy = (dy / d) * f;
        nodes[i].vx += dx; nodes[i].vy += dy;
        nodes[j].vx -= dx; nodes[j].vy -= dy;
      }
    for (const e of edges) {
      const a = nodes[e.a],
        b = nodes[e.b];
      let dx = a.x - b.x,
        dy = a.y - b.y;
      const d = Math.sqrt(dx * dx + dy * dy) + 0.01;
      const f = ((d * d) / k) * 0.02 * (0.3 + e.wn);
      dx = (dx / d) * f;
      dy = (dy / d) * f;
      a.vx -= dx; a.vy -= dy;
      b.vx += dx; b.vy += dy;
    }
    for (const n of nodes) {
      n.vx += (50 - n.x) * 0.01;
      n.vy += (50 - n.y) * 0.01;
      n.x += clamp(n.vx, -4, 4);
      n.y += clamp(n.vy, -4, 4);
      n.vx *= 0.85; n.vy *= 0.85;
    }
  }
  fit(nodes, 14, 86);
}

function fit(nodes, lo, hi) {
  let minx = Infinity, miny = Infinity, maxx = -Infinity, maxy = -Infinity;
  for (const n of nodes) {
    minx = Math.min(minx, n.x); maxx = Math.max(maxx, n.x);
    miny = Math.min(miny, n.y); maxy = Math.max(maxy, n.y);
  }
  const s = Math.min((hi - lo) / Math.max(1e-3, maxx - minx), (hi - lo) / Math.max(1e-3, maxy - miny));
  const cx = (minx + maxx) / 2, cy = (miny + maxy) / 2;
  for (const n of nodes) {
    n.x = 50 + (n.x - cx) * s;
    n.y = 50 + (n.y - cy) * s;
  }
}

const SERIF = "Georgia,'Iowan Old Style','Palatino Linotype','Times New Roman',ui-serif,serif";

function renderSchema(parts, graph, pal, uid, labels, seedStr) {
  const { nodes, edges } = graph;
  const ranked = nodes.slice().sort((a, b) => b.r - a.r);
  const front = labels ? ranked.slice(0, 2) : [];
  const back = labels ? ranked.slice(2, 6) : [];
  const ernd = mulberry32(xmur3("E" + seedStr)()); // deterministic edge "bow"

  // Back labels: behind the graph — small, lightly blurred, faded (far depth).
  if (back.length) {
    parts.push(`<g filter="url(#${uid}blur)" font-family="${SERIF}" font-style="italic" fill="${pal.labelFill}" opacity="0.5">`);
    back.forEach((n, i) => {
      const fs = (3.0 - i * 0.28).toFixed(2);
      parts.push(`<text x="${n.x.toFixed(1)}" y="${(n.y + n.r + +fs).toFixed(1)}" text-anchor="middle" font-size="${fs}">${escapeXml(n.label)}</text>`);
    });
    parts.push(`</g>`);
  }

  // The graph itself, run through the roughen filter so strokes look drawn by hand.
  parts.push(`<g filter="url(#${uid}r)">`);

  // Edges: gently bowed curves (not straight lines), each with a seeded offset.
  parts.push(`<g stroke="${pal.ink}" fill="none" stroke-linecap="round">`);
  for (const e of edges) {
    const a = nodes[e.a], b = nodes[e.b];
    const dx = b.x - a.x, dy = b.y - a.y, len = Math.hypot(dx, dy) || 1;
    const bow = (ernd() - 0.5) * (1.6 + len * 0.1);
    const mx = (a.x + b.x) / 2 + (-dy / len) * bow;
    const my = (a.y + b.y) / 2 + (dx / len) * bow;
    parts.push(
      `<path d="M${a.x.toFixed(1)} ${a.y.toFixed(1)} Q${mx.toFixed(1)} ${my.toFixed(1)} ${b.x.toFixed(1)} ${b.y.toFixed(1)}" stroke-width="${(0.3 + e.wn * 1.3).toFixed(2)}" opacity="${(0.28 + e.wn * 0.42).toFixed(2)}"/>`
    );
  }
  parts.push(`</g>`);

  // Dots: soft filled discs (the displacement makes them slightly irregular).
  parts.push(`<g stroke-linejoin="round">`);
  for (const n of nodes) {
    const fill = n.hue == null ? pal.neutral : `hsl(${n.hue} ${pal.nodeS}% ${pal.nodeL}%)`;
    parts.push(
      `<circle cx="${n.x.toFixed(1)}" cy="${n.y.toFixed(1)}" r="${n.r.toFixed(2)}" fill="${fill}" fill-opacity="0.92" stroke="${pal.nodeStroke}" stroke-width="0.5" stroke-opacity="0.55"/>`
    );
  }
  parts.push(`</g></g>`); // close dots, close roughen group

  // Front labels: on top, large, crisp serif with a coloured outline (near depth).
  if (front.length) {
    parts.push(`<g font-family="${SERIF}" font-weight="700" fill="${pal.labelFill}" stroke="${pal.labelHalo}" stroke-width="1.1" paint-order="stroke" stroke-linejoin="round">`);
    front.forEach((n, i) => {
      const fs = i === 0 ? 6.4 : 5.0;
      parts.push(`<text x="${n.x.toFixed(1)}" y="${(n.y - n.r - 1.4).toFixed(1)}" text-anchor="middle" font-size="${fs}">${escapeXml(n.label)}</text>`);
    });
    parts.push(`</g>`);
  }
}

function renderAbstract(parts, info, rnd, pal, uid) {
  const n = Math.round(clamp(5 + Math.log10(Math.max(1, info.triples)) * 4.5, 6, 26));
  const baseHue = parseInt((pal.bg0.match(/hsl\((\d+)/) || [0, 210])[1], 10);
  const nodes = [];
  for (let i = 0; i < n; i++) {
    const a = rnd() * TAU,
      r = Math.pow(rnd(), 0.6) * 40;
    nodes.push({
      x: 50 + Math.cos(a) * r + (rnd() - 0.5) * 8,
      y: 50 + Math.sin(a) * r + (rnd() - 0.5) * 8,
      rad: 1.1 + Math.pow(rnd(), 2) * 3.2,
      hue: (baseHue + Math.floor(rnd() * 80) - 40 + 360) % 360,
    });
  }
  parts.push(`<g filter="url(#${uid}r)">`); // same hand-drawn roughen as the schema graph
  parts.push(`<g stroke="${pal.ink}" stroke-width="0.35" stroke-linecap="round" opacity="0.42">`);
  for (let i = 0; i < nodes.length; i++) {
    let best = -1, bd = Infinity;
    for (let j = 0; j < nodes.length; j++) {
      if (j === i) continue;
      const d = (nodes[i].x - nodes[j].x) ** 2 + (nodes[i].y - nodes[j].y) ** 2;
      if (d < bd) { bd = d; best = j; }
    }
    if (best >= 0) parts.push(`<line x1="${nodes[i].x.toFixed(1)}" y1="${nodes[i].y.toFixed(1)}" x2="${nodes[best].x.toFixed(1)}" y2="${nodes[best].y.toFixed(1)}"/>`);
  }
  parts.push(`</g><g>`);
  for (const nd of nodes)
    parts.push(`<circle cx="${nd.x.toFixed(1)}" cy="${nd.y.toFixed(1)}" r="${nd.rad.toFixed(2)}" fill="hsl(${nd.hue} 55% ${pal.nodeL}%)" fill-opacity="0.92"/>`);
  parts.push(`</g></g>`);
}

function escapeXml(s) {
  return String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
}
