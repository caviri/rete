// The shared app object. Every module attaches what it owns to it; the next
// pass (systems.js) uses nothing else. See README.md for the full list.
const listeners = new Map();

export const app = {
  version: 1,
  // filled by scene.js
  scene: null, camera: null, renderer: null, controls: null, house: null,
  // filled by main.js
  graph: null,                 // the fzk-viewer.json index
  nodeByGuid: new Map(),       // IFC GUID -> index node {n,l,c,k,st,bd,adj,...,i}
  meshByGuid: new Map(),       // IFC GUID -> Object3D (a Mesh, or a Group for doors/windows)
  roomByGuid: new Map(),       // IFC GUID -> {guid,label,storey,box,center,floorZ,node}
  systems: null,               // parsed docs/building-systems.json, or null
  mode: 'orbit',               // 'orbit' | 'walk'
  ready: false,

  on(ev, fn) {
    if (!listeners.has(ev)) listeners.set(ev, new Set());
    listeners.get(ev).add(fn);
    return () => app.off(ev, fn);
  },
  off(ev, fn) { const s = listeners.get(ev); if (s) s.delete(fn); },
  emit(ev, payload) {
    const s = listeners.get(ev);
    if (!s) return;
    for (const fn of [...s]) {
      try { fn(payload); } catch (e) { console.error('[building] handler for "' + ev + '" failed', e); }
    }
  },
};

window.reteBuilding = app;   // for scripts, tests and the next pass
