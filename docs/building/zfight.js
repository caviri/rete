// A diagnostic for z-fighting: pairs of visible faces that lie in the same plane (within about 1 mm), face the
// same way (or one of them is double-sided) and overlap by more than a few cm2. Two such faces get the same depth
// and flicker where they overlap, unless they have different polygon offsets (the one with the smaller offset wins
// everywhere) or are separated some other way.
//
// app.findCoplanar() returns { pairs, handled, unhandled, groups }. It reads the geometry the page draws: the
// proxies of the batches (house, services) with their per-part polygon offset, and every other visible mesh.
// "handled" means the two sides have a different effective offset (or one is a decal drawn after the other).
import * as THREE from 'three';
import { app } from './app.js';

const MIN_AREA = 0.0005;      // m2: overlap that shows
const TOL = 0.0012;           // m: the planes are the same

function vis(o) { for (; o; o = o.parent) if (!o.visible) return false; return true; }
const ownerName = o => {
  for (let p = o; p; p = p.parent) {
    if (p.userData && p.userData.thingId) return 'svc:' + (app.svc && app.svc.thingFor(p.userData.thingId) ? (app.svc.thingFor(p.userData.thingId).d.type || p.userData.thingId) : p.userData.thingId);
    if (p.userData && p.userData.guid) { const n = app.nodeByGuid.get(p.userData.guid); return (n ? n.c.replace('Ifc', '') : 'plate') + (o.userData.role ? '/' + o.userData.role : ''); }
  }
  return o.name || o.type;
};

export function collect() {
  const tris = [];
  app.scene.updateMatrixWorld(true);
  const sets = new Set();
  app.scene.traverse(o => {
    if (!o.isMesh || o.isInstancedMesh || o.isSprite) return;
    if (o.userData.batch) return;                       // the merged meshes: their proxies are read instead
    if (o.name === 'baseSky' || o.name === 'skyDome') return;
    if (!vis(o)) return;
    const els = o.userData.els, batched = !!(els && els[0]) || !!o.userData.el;
    const m = Array.isArray(o.material) ? o.material[0] : o.material;
    if (!batched && m && m.visible === false) return;   // invisible hit boxes
    const g = o.geometry, P = g.attributes.position; if (!P) return;
    const el = batched ? (o.userData.el || els.find(e => e && e.set.root.visible)) : null;
    if (batched && !el) return;
    const label = ownerName(o);
    if (label === 'Space' && app.rooms && app.rooms.getStyle && app.rooms.getStyle() !== 'fill') return;   // a room volume is drawn as lines, not as faces, except in the fill style
    const idx = g.index;
    const n = (idx ? idx.count : P.count) / 3, mw = o.matrixWorld;
    const a = new THREE.Vector3(), b = new THREE.Vector3(), c = new THREE.Vector3();
    for (let t = 0; t < n; t++) {
      const i0 = idx ? idx.getX(t * 3) : t * 3, i1 = idx ? idx.getX(t * 3 + 1) : t * 3 + 1, i2 = idx ? idx.getX(t * 3 + 2) : t * 3 + 2;
      a.fromBufferAttribute(P, i0).applyMatrix4(mw); b.fromBufferAttribute(P, i1).applyMatrix4(mw); c.fromBufferAttribute(P, i2).applyMatrix4(mw);
      let off = 0, side = 0, look = '';
      if (batched) {
        let part = el.parts[0];
        if (el.parts.length > 1 && g.groups.length) { const gi = g.groups.findIndex(gr => i0 >= gr.start && i0 < gr.start + gr.count); if (gi >= 0 && el.parts[gi]) part = el.parts[gi]; }
        off = part.spec.offset || 0; side = part.spec.side || 0; look = part.base.getHexString() + '|' + (part.spec.map ? part.spec.map.uuid : '') + '|' + part.alpha0;
      } else { look = m && m.color ? m.color.getHexString() + '|' + (m.map ? m.map.uuid : '') + '|' + m.opacity : o.uuid; off = m && m.polygonOffset ? m.polygonOffsetFactor : 0; side = m ? m.side : 0; if (m && m.transparent && m.depthWrite === false) off -= 0.5; }
      tris.push({ a: a.clone(), b: b.clone(), c: c.clone(), owner: o, label, off, side, look, ro: o.renderOrder || 0 });
    }
  });
  return tris;
}

const n3 = new THREE.Vector3(), e1 = new THREE.Vector3(), e2 = new THREE.Vector3();
function prep(t) {
  e1.subVectors(t.b, t.a); e2.subVectors(t.c, t.a); n3.crossVectors(e1, e2);
  const area = n3.length() / 2; if (area < 1e-5) return false;
  n3.normalize(); t.area = area; t.n = n3.clone(); t.d = n3.dot(t.a); return true;
}
// area of the intersection of two coplanar triangles, in 2D after dropping the dominant axis
function overlap(t, u) {
  const k = Math.abs(t.n.x) > Math.abs(t.n.y) ? (Math.abs(t.n.x) > Math.abs(t.n.z) ? 0 : 2) : (Math.abs(t.n.y) > Math.abs(t.n.z) ? 1 : 2);
  const f = (p) => k === 0 ? [p.y, p.z] : k === 1 ? [p.x, p.z] : [p.x, p.y];
  let P = [f(t.a), f(t.b), f(t.c)]; const Q = [f(u.a), f(u.b), f(u.c)];
  const sgn = (A) => (A[1][0] - A[0][0]) * (A[2][1] - A[0][1]) - (A[1][1] - A[0][1]) * (A[2][0] - A[0][0]);
  if (sgn(Q) < 0) Q.reverse(); if (sgn(P) < 0) P.reverse();
  for (let i = 0; i < 3 && P.length; i++) {
    const A = Q[i], B = Q[(i + 1) % 3], out = [];
    const inside = (p) => (B[0] - A[0]) * (p[1] - A[1]) - (B[1] - A[1]) * (p[0] - A[0]) >= -1e-9;
    for (let j = 0; j < P.length; j++) {
      const C = P[j], D = P[(j + 1) % P.length], ci = inside(C), di = inside(D);
      if (ci) out.push(C);
      if (ci !== di) {
        const dx = D[0] - C[0], dy = D[1] - C[1], ex = B[0] - A[0], ey = B[1] - A[1], den = dx * ey - dy * ex;
        if (Math.abs(den) > 1e-12) { const s = ((A[0] - C[0]) * ey - (A[1] - C[1]) * ex) / den; out.push([C[0] + dx * s, C[1] + dy * s]); }
      }
    }
    P = out;
  }
  let s = 0; for (let i = 0; i < P.length; i++) { const p = P[i], q = P[(i + 1) % P.length]; s += p[0] * q[1] - q[0] * p[1]; }
  return Math.abs(s) / 2;
}

export function findCoplanar() {
  const tris = collect().filter(prep);
  const buckets = new Map();
  const key = (t, dq) => [Math.round(t.n.x * 50), Math.round(t.n.y * 50), Math.round(t.n.z * 50), dq].join(',');
  tris.forEach((t, i) => {
    // one orientation per plane, so back-to-back faces meet in the same bucket
    let flip = t.n.x < -1e-6 || (Math.abs(t.n.x) <= 1e-6 && (t.n.y < -1e-6 || (Math.abs(t.n.y) <= 1e-6 && t.n.z < 0)));
    t.flip = flip; if (flip) { t.n.negate(); t.d = -t.d; t.facing = -1; } else t.facing = 1;
    const dq = Math.round(t.d / 0.002);
    for (const k of [dq - 1, dq, dq + 1]) { const kk = key(t, k); if (k === dq) { (buckets.get(kk) || buckets.set(kk, []).get(kk)).push(i); } }
  });
  const groups = new Map(), pairs = [];
  for (const [kk, list] of buckets) {
    const parts = kk.split(','), dq = +parts[3];
    const neigh = [list]; for (const dd of [1]) { const nk = [parts[0], parts[1], parts[2], dq + dd].join(','); if (buckets.has(nk)) neigh.push(buckets.get(nk)); }
    for (let li = 0; li < neigh.length; li++) {
      const L2 = neigh[li];
      for (let i = 0; i < list.length; i++) for (let j = li === 0 ? i + 1 : 0; j < L2.length; j++) {
        const t = tris[list[i]], u = tris[L2[j]];
        if (t.owner === u.owner) continue;
        if (Math.abs(t.d - u.d) > TOL) continue;
        const same = t.facing * 1 === u.facing;     // same facing after the flip means same original orientation
        const dsided = t.side === THREE.DoubleSide || u.side === THREE.DoubleSide;
        if (!same && !dsided) continue;            // back to back, both single-sided: only one is ever seen
        if (t.a.distanceToSquared(u.a) > 400) continue;
        const A = overlap(t, u); if (!(A > MIN_AREA)) continue;
        const handled = t.off !== u.off || t.ro !== u.ro || (t.look !== '' && t.look === u.look);   // the same colour and texture: flicker cannot be seen
        const names = [t.label, u.label].sort().join(' | ');
        const gk = names + (handled ? ' [handled]' : '');
        const g = groups.get(gk) || groups.set(gk, { pair: names, handled, n: 0, area: 0, at: t.a.toArray().map(v => +v.toFixed(2)), off: [t.off, u.off] }).get(gk);
        g.n++; g.area += A; pairs.push(1);
      }
    }
  }
  const list = [...groups.values()].map(g => ({ ...g, area: +g.area.toFixed(3) })).sort((a, b) => b.area - a.area);
  return { triangles: tris.length, pairs: pairs.length, handled: list.filter(g => g.handled), unhandled: list.filter(g => !g.handled), groups: list.length };
}
