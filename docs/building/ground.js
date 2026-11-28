// The ground cutaway: look at what is under the lawn.
//
// API (on the ground object: app.ground, a THREE.Mesh; event 'cutaway' {mode}):
//   app.ground.setCutaway('none' | 'trench' | 'hidden', {structure: true})
//        trench: the terrain and the scenery ground are cut away around the footprint of the house down to the foundation base, in a
//                neat excavated box with earth-coloured sides, so the foundations and slab layers are seen from outside
//        hidden: no ground, no scenery ground, no plants, no contact shadow (drawn so only in the 3D pane)
//        structure (default true): the Structure layer is switched on, if there is one
//   app.ground.getCutaway() -> mode
//   app.ground.trench() -> {min: [x, y], max: [x, y], topZ, baseZ} | null
// The trench is a clipping hole in the two ground meshes (clipIntersection: only the box in plan is clipped) plus a lining of five
// faces. The lawn plate that comes with the GLB is hidden while a cutaway is on.
import * as THREE from 'three';
import { app } from './app.js';

let mode = 'none', lining = null, planes = null, box = null;
const MARGIN = 1.5;

function groundMaterials() {
  const out = []; if (app.ground) out.push(app.ground.material);
  const sg = app.scene.getObjectByName('sceneryGround'); if (sg) out.push(sg.material);
  return out;
}
function baseZ() { return (app.systems && app.systems.structure && app.systems.structure.foundationBaseZ) ?? -1.1; }
function topZ() { return app.ground ? app.ground.position.z : -0.03; }
function build() {
  const b = app.houseBox; const x0 = b.min.x - MARGIN, x1 = b.max.x + MARGIN, y0 = b.min.y - MARGIN, y1 = b.max.y + MARGIN, zt = topZ(), zb = baseZ();
  box = { min: [x0, y0], max: [x1, y1], topZ: zt, baseZ: zb };
  planes = [new THREE.Plane(new THREE.Vector3(-1, 0, 0), x0), new THREE.Plane(new THREE.Vector3(1, 0, 0), -x1), new THREE.Plane(new THREE.Vector3(0, -1, 0), y0), new THREE.Plane(new THREE.Vector3(0, 1, 0), -y1)];
  if (lining) { lining.removeFromParent(); lining.geometry.dispose(); }
  const P = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], pos = [];
  const quad = (a, b2, c, d) => pos.push(...a, ...b2, ...c, ...a, ...c, ...d);
  for (let i = 0; i < 4; i++) { const p = P[i], q = P[(i + 1) % 4]; quad([p[0], p[1], zb], [q[0], q[1], zb], [q[0], q[1], zt], [p[0], p[1], zt]); }
  quad([x0, y0, zb], [x1, y0, zb], [x1, y1, zb], [x0, y1, zb]);
  const g = new THREE.BufferGeometry(); g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3)); g.computeVertexNormals();
  const m = new THREE.MeshStandardMaterial({ color: '#6b5238', roughness: 1, metalness: 0, side: THREE.DoubleSide });
  lining = new THREE.Mesh(g, m); lining.name = 'trench'; lining.userData.noClip = true; lining.raycast = () => {}; lining.visible = false; app.scene.add(lining);
}
function lawnPlate(hide) {   // the plain lawn plate of the GLB, under the house
  app.meshByGuid.forEach((o, g) => { if (!app.nodeByGuid.has(g) && o.userData.els) o.userData.els.forEach(e => { if (e) e.set.setHidden(e, hide || (app.scenery ? app.scenery.get().on : false)); }); });
}
function apply() {
  const trench = mode === 'trench';
  if (trench && !planes) build();
  for (const m of groundMaterials()) {
    if (trench) { m.clippingPlanes = planes; m.clipIntersection = true; } else { m.clippingPlanes = null; m.clipIntersection = false; }
    m.needsUpdate = true;
  }
  if (lining) lining.visible = trench;
  if (app.lighting) app.lighting.contact().visible = mode === 'none';
  lawnPlate(mode !== 'none');
  app.requestRender();
}
export function initGround() {
  const g = app.ground; if (!g) return;
  g.setCutaway = (m, o = {}) => {
    if (!['none', 'trench', 'hidden'].includes(m)) throw new Error('Cutaway must be "none", "trench" or "hidden".');
    mode = m; if (m === 'trench' && !planes) build(); apply();
    if (m !== 'none' && o.structure !== false && app.svc) { const l = app.svc.layers.get('structure'); if (l && !l.isVisible()) l.setVisible(true); }
    app.emit('cutaway', { mode }); return mode;
  };
  g.getCutaway = () => mode; g.trench = () => (planes ? { ...box, min: box.min.slice(), max: box.max.slice() } : null);
  app.on('scenery', () => { if (mode !== 'none') apply(); });
}
