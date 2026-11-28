// Batches: many elements drawn as a few meshes.
//
// The house and the services layer are made of hundreds of small things. Each of them
// keeps an invisible "proxy" object in the scene (picking, bounding boxes, ray casts, the
// walk collision and the thermal shading all keep using them) but is drawn from a merged
// geometry: one batch per texture / side / shadow flags (a "key"), two meshes per batch,
// one for the opaque parts and one for the see-through ones.
//
// What used to be per-object material state is now per-vertex data:
//   color  (vec3)  the colour of the part, already tinted, in linear space
//   aMat   (vec2)  roughness, metalness
//   aEm    (vec4)  emissive rgb (already times its intensity), alpha
// A small patch in the standard material reads aMat and aEm. Highlight, dimming, live/dead,
// x-ray and glow are all writes into those arrays for the vertex range of one element.
//
// Visibility is the index buffer: sync() rebuilds the two index lists of a batch when the
// visibility of an element (its proxy and all its parents) or the pass of a part (opaque or
// see-through, from its alpha) changed. Hidden elements cost nothing and cast no shadow.
//
// Moving parts (door leaves, window sashes, levers) are "dynamic" elements: their local
// vertices are kept and pose(el) writes them again through the proxy's current matrix.
import * as THREE from 'three';
import { app } from './app.js';

export const PROXY = new THREE.MeshBasicMaterial({ visible: false });   // not drawn, still ray cast
const NOOP = () => {};
const TRANS_BELOW = 0.9;     // alpha under this draws in the see-through pass
const tmpM = new THREE.Matrix4(), tmpN = new THREE.Matrix3(), tmpV = new THREE.Vector3();

function patch(m) {
  m.userData.cap = { value: 0 };   // section.js: 1 while a cut is on, the back faces of closed solids then draw as a flat hatched cut face
  m.onBeforeCompile = s => {
    s.uniforms.uCap = m.userData.cap;
    s.vertexShader = s.vertexShader
      .replace('#include <common>', '#include <common>\nattribute vec2 aMat; attribute vec4 aEm; varying vec2 vMat; varying vec4 vEm;')
      .replace('#include <color_vertex>', '#include <color_vertex>\n vMat = aMat; vEm = aEm;');
    s.fragmentShader = s.fragmentShader
      .replace('#include <common>', '#include <common>\nvarying vec2 vMat; varying vec4 vEm; uniform float uCap;')
      // a cut: the back face of a closed solid is the inside seen through the cut; draw it as a flat, darker, hatched cut face
      .replace('#include <opaque_fragment>', 'if (uCap > 0.5 && !gl_FrontFacing) { vec3 cc = mix(diffuseColor.rgb, vec3(0.05, 0.06, 0.09), 0.55); cc = mix(cc, vec3(0.85), step(mod(gl_FragCoord.x + gl_FragCoord.y, 8.0), 1.5) * 0.2); outgoingLight = cc; }\n#include <opaque_fragment>')
      .replace('#include <color_fragment>', '#include <color_fragment>\n diffuseColor.a *= vEm.a;')
      .replace('#include <roughnessmap_fragment>', 'float roughnessFactor = vMat.x;')
      .replace('#include <metalnessmap_fragment>', 'float metalnessFactor = vMat.y;')
      .replace('#include <emissivemap_fragment>', '#include <emissivemap_fragment>\n totalEmissiveRadiance += vEm.rgb;');
  };
  m.customProgramCacheKey = () => 'rete-batch-2';
  return m;
}

class Batch {
  constructor(set, key, spec) { this.set = set; this.key = key; this.spec = spec; this.pending = []; this.parts = []; this.dirtyAttr = false; this.dirtyIdx = true; }
}

export class BatchSet {
  // opts: { name, parent, onMaterial(mat, spec), anisotropy }
  constructor(opts) {
    this.name = opts.name; this.parent = opts.parent; this.opts = opts;
    this.batches = new Map(); this.elements = []; this.root = new THREE.Group(); this.root.name = 'batch-' + opts.name;
    this.parent.add(this.root); this.changed = true; this.built = false; this.inv = new THREE.Matrix4();
    app.house.updateMatrixWorld(true); this.inv.copy(app.house.matrixWorld).invert();
    (app.batchSetList || (app.batchSetList = new Set())).add(this);
  }
  // a unit of state and visibility: owner is the proxy object whose visibility chain decides
  element(owner, { dynamic = false, src = null, data = null } = {}) {
    const el = { owner, parts: [], vis: true, dynamic, src: src || owner, data, set: this, st: { color: null, alpha: null, emissive: null, ei: 0 }, alphaNow: 1 };
    this.elements.push(el); return el;
  }
  localMatrix(src) { return new THREE.Matrix4().multiplyMatrices(this.inv, src.matrixWorld); }
  keyOf(s) { return [s.map ? s.map.uuid : '-', s.side || 0, s.cast ? 1 : 0, s.receive === false ? 0 : 1, s.flat ? 1 : 0, s.env ? s.envBase || 1 : 0, s.offset || 0].join('|'); }
  // spec: { map, uvScale:[u,v], color:Color, rough, metal, alpha, emissive:Color|null, ei, side, cast, receive, flat, env, envBase, glass }
  // range: {start,count} in vertices of a non-indexed geometry
  addPart(el, geometry, matrix, spec, range) {
    const key = this.keyOf(spec); let b = this.batches.get(key);
    if (!b) { b = new Batch(this, key, spec); this.batches.set(key, b); }
    const part = { el, batch: b, spec, v0: 0, vn: 0, i0: 0, in: 0, trans: false, glass: !!spec.glass, base: spec.color.clone(), alpha0: spec.alpha ?? 1, em0: spec.emissive ? spec.emissive.clone().multiplyScalar(spec.ei ?? 1) : new THREE.Color(0, 0, 0), rest: null };
    b.pending.push({ part, geometry, matrix: matrix.clone(), range }); el.parts.push(part);
    return part;
  }
  build() {
    app.house.updateMatrixWorld(true);
    this.inv.copy(app.house.matrixWorld).invert();
    for (const b of this.batches.values()) this._buildBatch(b);
    this.built = true;
    for (const el of this.elements) { if (el.dynamic) this.pose(el, true); this._write(el); }
    this.sync(true);
    return this;
  }
  _buildBatch(b) {
    let nv = 0, ni = 0;
    for (const p of b.pending) {
      const g = p.geometry, P = g.attributes.position, r = p.range;
      p.nv = r && !g.index ? r.count : P.count;
      p.ni = g.index ? g.index.count : p.nv;
      nv += p.nv; ni += p.ni;
    }
    const pos = new Float32Array(nv * 3), nor = new Float32Array(nv * 3), uv = new Float32Array(nv * 2), col = new Float32Array(nv * 3), mat = new Float32Array(nv * 2), em = new Float32Array(nv * 4);
    const src = new Uint32Array(ni);
    let v = 0, ii = 0;
    for (const p of b.pending) {
      const g = p.geometry, P = g.attributes.position, N = g.attributes.normal, U = g.attributes.uv, part = p.part, s = part.spec;
      const off = p.range && !g.index ? p.range.start : 0;
      tmpN.getNormalMatrix(p.matrix);
      const us = s.uvScale || [1, 1];
      if (part.el.dynamic) { part.rest = { pos: new Float32Array(p.nv * 3), nor: new Float32Array(p.nv * 3) }; }
      for (let k = 0; k < p.nv; k++) {
        const si = off + k, di = v + k;
        tmpV.fromBufferAttribute(P, si);
        if (part.rest) { part.rest.pos[k * 3] = tmpV.x; part.rest.pos[k * 3 + 1] = tmpV.y; part.rest.pos[k * 3 + 2] = tmpV.z; }
        tmpV.applyMatrix4(p.matrix); pos[di * 3] = tmpV.x; pos[di * 3 + 1] = tmpV.y; pos[di * 3 + 2] = tmpV.z;
        if (N) {
          tmpV.fromBufferAttribute(N, si);
          if (part.rest) { part.rest.nor[k * 3] = tmpV.x; part.rest.nor[k * 3 + 1] = tmpV.y; part.rest.nor[k * 3 + 2] = tmpV.z; }
          tmpV.applyMatrix3(tmpN).normalize(); nor[di * 3] = tmpV.x; nor[di * 3 + 1] = tmpV.y; nor[di * 3 + 2] = tmpV.z;
        }
        if (U) { uv[di * 2] = U.getX(si) * us[0]; uv[di * 2 + 1] = U.getY(si) * us[1]; }
        mat[di * 2] = s.rough ?? 0.8; mat[di * 2 + 1] = s.metal ?? 0;
      }
      part.v0 = v; part.vn = p.nv; part.i0 = ii; part.in = p.ni;
      if (g.index) { const I = g.index; for (let k = 0; k < p.ni; k++) src[ii + k] = I.getX(k) + v; }
      else for (let k = 0; k < p.ni; k++) src[ii + k] = v + k;
      v += p.nv; ii += p.ni; b.parts.push(part);
    }
    b.pending = null;
    b.pos = new THREE.BufferAttribute(pos, 3); b.nor = new THREE.BufferAttribute(nor, 3); b.uv = new THREE.BufferAttribute(uv, 2);
    b.col = new THREE.BufferAttribute(col, 3); b.mat = new THREE.BufferAttribute(mat, 2); b.em = new THREE.BufferAttribute(em, 4);
    for (const a of [b.pos, b.nor, b.col, b.mat, b.em]) a.setUsage(THREE.DynamicDrawUsage);
    b.src = src; b.nv = nv;
    const s = b.spec, mk = trans => {
      const m = patch(new THREE.MeshStandardMaterial({ map: s.map || null, vertexColors: true, side: s.side || THREE.FrontSide, flatShading: !!s.flat, roughness: 1, metalness: 1,
        transparent: trans, depthWrite: !trans, polygonOffset: !!s.offset, polygonOffsetFactor: s.offset || 0, polygonOffsetUnits: (s.offset || 0) * 2 }));   // lower offset wins where faces share a plane
      if (trans) m.forceSinglePass = true;
      m.clippingPlanes = app.clipPlanes || null; m.clipShadows = true;   // the three section planes, shared (section.js); a plane that is off keeps everything
      m.userData.capable = !trans;
      if (s.env && app.envTexture) { m.envMap = app.envTexture; m.envMapIntensity = s.envBase || 1; m.userData.envBase = s.envBase || 1; }
      if (s.map) s.map.anisotropy = this.opts.anisotropy || 1;
      if (this.opts.onMaterial) this.opts.onMaterial(m, s);
      return m;
    };
    b.mats = { opaque: mk(false), trans: mk(true) };
    b.meshes = {};
    for (const pass of ['opaque', 'trans']) {
      const g = new THREE.BufferGeometry();
      g.setAttribute('position', b.pos); g.setAttribute('normal', b.nor); g.setAttribute('uv', b.uv); g.setAttribute('color', b.col); g.setAttribute('aMat', b.mat); g.setAttribute('aEm', b.em);
      g.setIndex(new THREE.BufferAttribute(new Uint32Array(ni), 1)); g.index.setUsage(THREE.DynamicDrawUsage); g.setDrawRange(0, 0);
      g.boundingSphere = new THREE.Sphere(new THREE.Vector3(6, 5, 2), 60);
      const mesh = new THREE.Mesh(g, b.mats[pass]);
      mesh.frustumCulled = false; mesh.raycast = NOOP; mesh.matrixAutoUpdate = false;
      mesh.castShadow = pass === 'opaque' && !!s.cast; mesh.receiveShadow = s.receive !== false;
      mesh.userData.batch = b; mesh.name = this.name + ':' + pass; mesh.visible = false;
      b.meshes[pass] = mesh; this.root.add(mesh);
    }
  }

  // ---- state ----
  // style: null (back to base) or {color: hex|Color, opacity, emissive: hex|Color, ei}
  setStyle(el, style) {
    const st = el.st;
    if (!style) { st.color = null; st.alpha = null; st.emissive = null; st.ei = 0; }
    else {
      st.color = style.color != null ? (st.color || new THREE.Color()).set(style.color) : null;
      st.alpha = style.opacity != null ? style.opacity : null;
      st.emissive = style.emissive != null ? (st.emissive || new THREE.Color()).set(style.emissive) : null;
      st.ei = style.ei ?? 0.4;
    }
    this._write(el);
  }
  // for things with their own look rules (the services layer): explicit values per part
  setLook(el, { color, alpha, emissive }) {
    const st = el.st; st.color = color ? (st.color || new THREE.Color()).copy(color) : null; st.alpha = alpha != null ? alpha : null;
    st.emissive = emissive ? (st.emissive || new THREE.Color()).copy(emissive) : null; st.ei = 1;
    this._write(el);
  }
  setHidden(el, h) { if (!!el.hidden === !!h) return; el.hidden = !!h; for (const p of el.parts) p.batch.dirtyIdx = true; this.changed = true; }
  setBaseEmissive(el, color, intensity) { for (const p of el.parts) p.em0.set(color).multiplyScalar(intensity); this._write(el); }
  _write(el) {
    if (!this.built) return;
    const st = el.st;
    for (const p of el.parts) {
      const b = p.batch, c = st.color || p.base, a = st.alpha != null ? st.alpha : p.alpha0, e = st.emissive ? st.emissive : p.em0, ei = st.emissive ? st.ei : 1;
      const col = b.col.array, em = b.em.array, r = c.r, g = c.g, bl = c.b, er = e.r * ei, eg = e.g * ei, eb = e.b * ei;
      for (let k = p.v0, n = p.v0 + p.vn; k < n; k++) { col[k * 3] = r; col[k * 3 + 1] = g; col[k * 3 + 2] = bl; em[k * 4] = er; em[k * 4 + 1] = eg; em[k * 4 + 2] = eb; em[k * 4 + 3] = a; }
      b.dirtyAttr = true;
      const tr = a < TRANS_BELOW; if (tr !== p.trans) { p.trans = tr; b.dirtyIdx = true; }
      el.alphaNow = a;
    }
    this.changed = true;
  }
  pose(el, quiet) {
    el.src.updateWorldMatrix(true, false);
    tmpM.multiplyMatrices(this.inv, el.src.matrixWorld); tmpN.getNormalMatrix(tmpM);
    for (const p of el.parts) {
      const b = p.batch, rp = p.rest.pos, rn = p.rest.nor, pos = b.pos.array, nor = b.nor.array;
      for (let k = 0; k < p.vn; k++) {
        tmpV.set(rp[k * 3], rp[k * 3 + 1], rp[k * 3 + 2]).applyMatrix4(tmpM); const d = (p.v0 + k) * 3; pos[d] = tmpV.x; pos[d + 1] = tmpV.y; pos[d + 2] = tmpV.z;
        tmpV.set(rn[k * 3], rn[k * 3 + 1], rn[k * 3 + 2]).applyMatrix3(tmpN).normalize(); nor[d] = tmpV.x; nor[d + 1] = tmpV.y; nor[d + 2] = tmpV.z;
      }
      b.dirtyAttr = true; b.posDirty = true;
    }
    if (!quiet) this.changed = true;
  }

  // ---- visibility and upload; call before every render ----
  sync(force) {
    if (!this.built) return false;
    const memo = new Map();
    const vis = o => { if (!o) return true; let v = memo.get(o); if (v === undefined) { v = o.visible && vis(o.parent); memo.set(o, v); } return v; };
    for (const el of this.elements) {
      const v = !el.hidden && vis(el.owner);
      if (v !== el.vis || force) { el.vis = v; for (const p of el.parts) p.batch.dirtyIdx = true; this.changed = true; }
    }
    for (const b of this.batches.values()) {
      if (b.dirtyIdx) {
        b.dirtyIdx = false;
        const io = b.meshes.opaque.geometry.index, it = b.meshes.trans.geometry.index; let no = 0, nt = 0;
        for (const p of b.parts) {
          if (!p.el.vis) continue;
          const arr = p.trans ? it.array : io.array; let n = p.trans ? nt : no;
          arr.set(b.src.subarray(p.i0, p.i0 + p.in), n); n += p.in; if (p.trans) nt = n; else no = n;
        }
        io.needsUpdate = true; it.needsUpdate = true;
        b.meshes.opaque.geometry.setDrawRange(0, no); b.meshes.trans.geometry.setDrawRange(0, nt);
        b.meshes.opaque.visible = no > 0; b.meshes.trans.visible = nt > 0; b.count = [no / 3, nt / 3];
      }
      if (b.dirtyAttr) { b.dirtyAttr = false; for (const a of [b.col, b.em]) a.needsUpdate = true; if (b.posDirty) { b.posDirty = false; b.pos.needsUpdate = true; b.nor.needsUpdate = true; } }
    }
    const c = this.changed; this.changed = false; return c && this.opts.shadow !== false;   // services do not cast shadows
  }
  dispose() {
    for (const b of this.batches.values()) { if (!b.meshes) continue; for (const k of ['opaque', 'trans']) { b.meshes[k].geometry.dispose(); b.mats[k].dispose(); } }
    this.root.removeFromParent(); if (app.batchSetList) app.batchSetList.delete(this);
  }
  stats() { let drawable = 0, tris = 0; this.batches.forEach(b => { if (!b.meshes) return; for (const k of ['opaque', 'trans']) if (b.meshes[k].visible) { drawable++; } tris += (b.count || [0, 0])[0] + (b.count || [0, 0])[1]; }); return { batches: this.batches.size, meshes: drawable, tris, elements: this.elements.length }; }
}
