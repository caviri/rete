// How a room is drawn: by its border, not as a translucent solid that can swallow the camera.
//
// API (app.rooms; event 'roomstyle' {style}):
//   setStyle('outline' | 'floor' | 'fill' | 'off')   default 'outline'
//        outline: a room that is shown (the Rooms class is on) or highlighted (selected, adjacent, a query result) is drawn as the crisp
//                 edge lines of its volume; a highlight is a bright line in the highlight colour, a shown room a faint one
//        floor:   the outline plus a faint tint of the floor of a highlighted room
//        fill:    the old translucent volume (kept for plan and section views); never drawn for the room the camera is inside, in walk or fly mode
//        off:     nothing is drawn for rooms
//   getStyle()    outline(guid) -> the THREE.LineSegments of a room (or null);  floor(guid) -> its floor tint mesh;  insideRoom() -> guid | null
// Everything that used to be a filled room volume goes through here: the IfcSpace meshes (class "Rooms", selection, query results) and the
// colour-by-variable overlay (statevars.js: floor tint and outline; the volume only in the fill style).
import * as THREE from 'three';
import { app } from './app.js';

let style = 'outline', group = null, inside = null;
const lines = new Map(), floors = new Map(), hl = new Map();   // guid -> LineSegments, floor mesh, highlight style
const FAINT = '#8393ab';
const deepVisible = o => { for (; o; o = o.parent) if (!o.visible) return false; return true; };
const isSpace = guid => { const n = app.nodeByGuid.get(guid); return !!n && n.k === 'Space'; };

function build() {
  if (group || !app.roomByGuid.size) return;
  group = new THREE.Group(); group.name = 'roomOutlines'; app.house.add(group);
  const faint = new THREE.LineBasicMaterial({ color: FAINT, transparent: true, opacity: 0.55, fog: false, toneMapped: false });
  for (const r of app.roomByGuid.values()) {
    const mesh = app.meshByGuid.get(r.guid); if (!mesh || !mesh.isMesh) continue;
    mesh.updateWorldMatrix(true, false);
    const ln = new THREE.LineSegments(new THREE.EdgesGeometry(mesh.geometry, 35), faint.clone()); ln.applyMatrix4(mesh.matrixWorld); ln.matrixAutoUpdate = false;
    ln.material.clippingPlanes = app.clipPlanes; ln.raycast = () => {}; ln.userData.noClip = true; ln.visible = false; ln.renderOrder = 4; group.add(ln); lines.set(r.guid, ln);
    const sz = r.box.getSize(new THREE.Vector3()), fl = new THREE.Mesh(new THREE.PlaneGeometry(sz.x - 0.04, sz.y - 0.04), new THREE.MeshBasicMaterial({ color: '#4ade80', transparent: true, opacity: 0.3, depthWrite: false, side: THREE.DoubleSide, fog: false, toneMapped: false, polygonOffset: true, polygonOffsetFactor: -3, polygonOffsetUnits: -3 }));
    fl.position.set(r.center.x, r.center.y, r.floorZ + 0.03); fl.raycast = () => {}; fl.userData.noClip = true; fl.visible = false; fl.renderOrder = 5; fl.material.clippingPlanes = app.clipPlanes; group.add(fl); floors.set(r.guid, fl);
  }
}
const roomAt = p => { for (const r of app.roomByGuid.values()) if (r.box.containsPoint(p)) return r.guid; return null; };
export function insideRoom() { return app.mode === 'walk' || app.mode === 'fly' ? roomAt(app.camera.position) : null; }
function refresh() {
  build(); const outline = style === 'outline' || style === 'floor';
  for (const [g, ln] of lines) {
    const h = hl.get(g), mesh = app.meshByGuid.get(g);
    ln.visible = outline && (!!h || deepVisible(mesh));
    if (ln.visible) { ln.material.color.set(h ? h.color : FAINT); ln.material.opacity = h ? 1 : 0.55; ln.material.depthTest = !h; }
    const fl = floors.get(g); fl.visible = style === 'floor' && !!h; if (fl.visible) fl.material.color.set(h.color);
  }
  // the filled volumes of the colour overlay: only in the fill style, never around the camera
  const ov = app.scene.getObjectByName('statevarColour');
  if (ov) for (const m of ov.children) if (m.userData.kind === 'vol') m.visible = !!m.userData.want && style === 'fill' && m.userData.room !== inside;
}
function repaint(guid) { const o = app.meshByGuid.get(guid); if (o) app.paint(o, o.userData.rawStyle || null); }
function wrapPaint() {
  const orig = app.paint;
  app.paint = (obj, style2) => {
    const g = obj && obj.userData && obj.userData.guid;
    if (!g || !isSpace(g)) return orig(obj, style2);
    obj.userData.rawStyle = style2 || null;
    if (style2 && style2.color != null) hl.set(g, { color: '#' + new THREE.Color(style2.color).getHexString() }); else hl.delete(g);
    if (style === 'fill') return orig(obj, g === inside ? { ...(style2 || {}), opacity: 0 } : style2);
    return orig(obj, { ...(style2 || {}), opacity: 0 });   // no filled volume: the border says where the room is
  };
}
export function setStyle(s) {
  if (!['outline', 'floor', 'fill', 'off'].includes(s)) throw new Error('Room style must be outline, floor, fill or off.');
  style = s; for (const r of app.roomByGuid.values()) repaint(r.guid);
  refresh(); app.requestRender(); app.emit('roomstyle', { style }); return style;
}
export function initRooms() {
  wrapPaint();
  app.addBeforeRender(() => { const i = insideRoom(); if (i !== inside) { const was = inside; inside = i; if (style === 'fill') { if (was) repaint(was); if (i) repaint(i); } } refresh(); });
  app.rooms = { setStyle, getStyle: () => style, outline: g => lines.get(g) || null, floor: g => floors.get(g) || null, insideRoom, highlighted: () => [...hl.keys()] };
  app.on('select', () => app.requestRender()); setStyle('outline');
}
