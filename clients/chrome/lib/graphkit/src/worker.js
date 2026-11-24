// Layout worker: runs fcose / dagre on a headless cytoscape instance so the page stays responsive
// and the Stop button can really stop (the host terminates the worker).
import cytoscape from "cytoscape";
import fcose from "cytoscape-fcose";
import dagre from "cytoscape-dagre";

cytoscape.use(fcose);
cytoscape.use(dagre);

self.onmessage = (ev) => {
  const m = ev.data;
  try {
    const els = [];
    for (const n of m.nodes) {
      const d = { id: n.id };
      if (n.parent) d.parent = n.parent;
      if (n.w) { d.w = n.w; d.h = n.h; }
      els.push({ group: "nodes", data: d, position: n.pos });
    }
    for (const e of m.edges) els.push({ group: "edges", data: { id: e.id, source: e.s, target: e.t } });
    const cy = cytoscape({ headless: true, styleEnabled: true, elements: els, style: [{ selector: "node", style: { width: "data(w)", height: "data(h)" } }, { selector: ":parent", style: { padding: 16 } }] });
    const p = m.params;
    let opts;
    if (m.name === "fcose") {
      opts = { name: "fcose", animate: false, fit: false, quality: p.quality, randomize: p.randomize, padding: p.padding, packComponents: p.packComponents, nodeRepulsion: () => p.nodeRepulsion, idealEdgeLength: () => p.idealEdgeLength, edgeElasticity: () => p.edgeElasticity, nodeSeparation: p.nodeSeparation, gravity: p.gravity, tile: p.tile, numIter: p.numIter, uniformNodeDimensions: false };
      if (p.fixed && p.fixed.length) opts.fixedNodeConstraint = p.fixed;
    } else {
      opts = { name: "dagre", animate: false, fit: false, rankDir: p.rankDir, nodeSep: p.nodeSep, rankSep: p.rankSep, edgeSep: p.edgeSep, ranker: p.ranker, padding: p.padding, nodeDimensionsIncludeLabels: false };
    }
    const layout = cy.layout(opts);
    layout.one("layoutstop", () => {
      const positions = {};
      cy.nodes().forEach((n) => { const q = n.position(); positions[n.id()] = { x: q.x, y: q.y }; });
      self.postMessage({ positions });
    });
    layout.run();
  } catch (e) {
    self.postMessage({ error: String(e && e.message ? e.message : e) });
  }
};
