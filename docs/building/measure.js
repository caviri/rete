// Measure a distance between two points picked on the house. Works in the single 3D view: the points come from a ray
// cast of the page's own camera (the picking code of ui.js does the same for selection). Esc or the menu item stops it.
import * as THREE from 'three';
import { app } from './app.js';
import { fmt, $ } from './shared.js';

const group = new THREE.Group(); group.name = 'measure';
const ray = new THREE.Raycaster(), mouse = new THREE.Vector2();
let on = false, pts = [], line = null, label = null, down = null;
const mat = new THREE.MeshBasicMaterial({ color: 0xffd54a, depthTest: false, toneMapped: false });
const vis = o => { for (; o; o = o.parent) if (!o.visible) return false; return true; };

function pointAt(x, y) {
  const r = app.renderer.domElement.getBoundingClientRect(); mouse.set(((x - r.left) / r.width) * 2 - 1, -((y - r.top) / r.height) * 2 + 1); ray.setFromCamera(mouse, app.camera);
  for (const h of ray.intersectObject(app.house, true)) { if (h.object.isSprite || !vis(h.object) || (h.object.userData.alpha ?? 1) <= 0.2 || h.object.parent === group) continue; return h.point.clone(); }
  return null;
}
function clear() { group.clear(); pts = []; line = null; if (label) label.hidden = true; app.requestRender(); }
function addPoint(p) {
  if (pts.length >= 2) clear();
  pts.push(p); const m = new THREE.Mesh(new THREE.SphereGeometry(0.06, 12, 8), mat); m.position.copy(p); m.renderOrder = 20; group.add(m);
  if (pts.length === 2) {
    const g = new THREE.BufferGeometry().setFromPoints(pts); line = new THREE.Line(g, new THREE.LineBasicMaterial({ color: 0xffd54a, depthTest: false, toneMapped: false })); line.renderOrder = 20; group.add(line);
    const d = pts[0].distanceTo(pts[1]), v = pts[1].clone().sub(pts[0]);
    if (!label) { label = document.createElement('div'); label.className = 'pop'; label.style.cssText = 'left:50%;top:10px;transform:translateX(-50%);width:auto;padding:6px 12px;z-index:12'; label.setAttribute('role', 'status'); $('stage').append(label); }
    label.hidden = false; label.innerHTML = `<b>${fmt.len(d)}</b> <span class="muted">· horizontal ${fmt.len(Math.hypot(v.x, v.y))} · height ${fmt.len(Math.abs(v.z))}</span>`;
    app.measure.last = { distance: +d.toFixed(3), a: pts[0].toArray(), b: pts[1].toArray() };
    if (app.log) app.log('info', 'user', `Measured ${d.toFixed(2)} m`, app.measure.last);
  }
  app.requestRender();
}
const onDown = e => { if (e.button === 0) down = [e.clientX, e.clientY]; };
const onUp = e => {
  if (!down || e.button !== 0) return; const moved = Math.hypot(e.clientX - down[0], e.clientY - down[1]) > 5; down = null; if (moved) return;
  const p = pointAt(e.clientX, e.clientY); if (p) addPoint(p);
};
const onKey = e => { if (e.key === 'Escape') stop(); };
export function start() {
  if (on) return; if (app.mode !== 'orbit') { app.notify('Measuring works in the orbit view.'); return; }
  on = true; app.scene.add(group); const cv = app.renderer.domElement; cv.style.cursor = 'crosshair'; cv.addEventListener('pointerdown', onDown); cv.addEventListener('pointerup', onUp); addEventListener('keydown', onKey);
  app.measure.active = true; app.notify('Measure: click two points on the house. Esc stops.', { ms: 8000 }); app.emit('measure', { active: true });
}
export function stop() {
  if (!on) return; on = false; clear(); app.scene.remove(group); const cv = app.renderer.domElement; cv.style.cursor = ''; cv.removeEventListener('pointerdown', onDown); cv.removeEventListener('pointerup', onUp); removeEventListener('keydown', onKey);
  app.measure.active = false; app.emit('measure', { active: false }); app.notify('Measure stopped');
}
export const toggleMeasure = () => (on ? stop() : start());
app.measure = { active: false, last: null, start, stop, toggle: toggleMeasure };
