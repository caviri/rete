// Navigator drawn on a small canvas from node positions, with a draggable viewport rectangle.
export class Minimap {
  constructor(host, cy, colorOf, getTheme) {
    this.cy = cy; this.colorOf = colorOf; this.getTheme = getTheme;
    this.canvas = document.createElement("canvas");
    this.canvas.setAttribute("aria-label", "Minimap: drag to move the view");
    this.canvas.setAttribute("role", "img");
    host.appendChild(this.canvas);
    this.ctx = this.canvas.getContext("2d");
    this._raf = 0; this.map = null;
    const down = (ev) => {
      ev.preventDefault(); ev.stopPropagation();
      this.canvas.setPointerCapture(ev.pointerId);
      this.drag = true; this.move(ev);
    };
    this.canvas.addEventListener("pointerdown", down);
    this.canvas.addEventListener("pointermove", (ev) => { if (this.drag) this.move(ev); });
    const up = (ev) => { this.drag = false; try { this.canvas.releasePointerCapture(ev.pointerId); } catch (e) { /* not captured */ } };
    this.canvas.addEventListener("pointerup", up);
    this.canvas.addEventListener("pointercancel", up);
    this.canvas.addEventListener("wheel", (ev) => ev.stopPropagation(), { passive: true });
    this.onviewport = () => this.schedule();
    cy.on("viewport position add remove", this.onviewport);
  }
  schedule() { if (!this._raf) this._raf = requestAnimationFrame(() => { this._raf = 0; this.draw(); }); }
  move(ev) {
    if (!this.map) return;
    const r = this.canvas.getBoundingClientRect();
    const mx = ev.clientX - r.left, my = ev.clientY - r.top;
    const m = this.map;
    const wx = (mx - m.ox) / m.s, wy = (my - m.oy) / m.s;
    const z = this.cy.zoom();
    this.cy.pan({ x: this.cy.width() / 2 - wx * z, y: this.cy.height() / 2 - wy * z });
  }
  draw() {
    const cv = this.canvas, cy = this.cy;
    const dpr = window.devicePixelRatio || 1;
    const w = cv.clientWidth, h = cv.clientHeight;
    if (!w || !h) return;
    if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) { cv.width = Math.round(w * dpr); cv.height = Math.round(h * dpr); }
    const c = this.ctx;
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.clearRect(0, 0, w, h);
    const th = this.getTheme();
    const nodes = cy.nodes(":visible").not(".gk-box");
    const ext = cy.extent();
    let bb = nodes.length ? nodes.boundingBox() : { x1: ext.x1, y1: ext.y1, x2: ext.x2, y2: ext.y2 };
    // include the viewport so the rectangle never leaves the map
    const x1 = Math.min(bb.x1, ext.x1), y1 = Math.min(bb.y1, ext.y1), x2 = Math.max(bb.x2, ext.x2), y2 = Math.max(bb.y2, ext.y2);
    const pad = 8;
    const s = Math.min((w - 2 * pad) / Math.max(x2 - x1, 1), (h - 2 * pad) / Math.max(y2 - y1, 1));
    const ox = (w - (x2 - x1) * s) / 2 - x1 * s, oy = (h - (y2 - y1) * s) / 2 - y1 * s;
    this.map = { s, ox, oy };
    const n = nodes.length;
    const rad = n > 1500 ? 0.9 : n > 300 ? 1.3 : 2;
    nodes.forEach((nd) => {
      const p = nd.position();
      c.fillStyle = this.colorOf(nd);
      c.fillRect(p.x * s + ox - rad / 2, p.y * s + oy - rad / 2, rad, rad);
    });
    const rx = ext.x1 * s + ox, ry = ext.y1 * s + oy, rw = (ext.x2 - ext.x1) * s, rh = (ext.y2 - ext.y1) * s;
    c.fillStyle = th.accent; c.globalAlpha = 0.14; c.fillRect(rx, ry, rw, rh); c.globalAlpha = 1;
    c.strokeStyle = th.accent; c.lineWidth = 1.5; c.strokeRect(rx + 0.5, ry + 0.5, Math.max(rw, 2), Math.max(rh, 2));
  }
  destroy() { this.cy.off("viewport position add remove", this.onviewport); if (this._raf) cancelAnimationFrame(this._raf); this.canvas.remove(); }
}
