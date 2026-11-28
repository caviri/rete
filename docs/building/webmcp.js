// WebMCP: the page's actions as tools for a browser agent, the same shape as
// docs/bim-pair.html. What an agent does shows in the page. Without WebMCP in
// the browser nothing is registered; the tools are still on window.reteBuildingTools.
import * as THREE from 'three';
import { app } from './app.js';
import { pretty } from './classes.js';
import { QNS, PRE_SERVICES, runSparql, toResult, cleanTerm } from './queries.js';

const MCP_MAX = 18000;

function agentPill(text, bad) {
  let el = document.getElementById('agentPill');
  if (!el) { el = document.createElement('div'); el.id = 'agentPill'; el.setAttribute('role', 'status'); (document.getElementById('stage') || document.body).appendChild(el); }
  const stage = document.getElementById('stage'); if (stage) stage.classList.add('agent-on');   // the hint in the corner gives way to the pill
  el.textContent = text; el.style.borderColor = bad ? 'var(--bad)' : ''; el.style.opacity = '1';
  clearTimeout(agentPill.t); agentPill.t = setTimeout(() => { el.style.opacity = '0'; if (stage) stage.classList.remove('agent-on'); }, 6000);
}
const brief = v => { try { const s = JSON.stringify(v); return s == null ? '' : s.length > 140 ? s.slice(0, 140) + '…' : s; } catch { return ''; } };

const r2 = v => Math.round(v * 100) / 100;
const roomsArr = () => [...app.roomByGuid.values()];
function findRoom(x) {
  const s = String(x || '').toLowerCase();
  const r = roomsArr().find(r => r.guid === x) || roomsArr().find(r => r.label.toLowerCase() === s) || roomsArr().find(r => r.label.toLowerCase().startsWith(s) && s);
  if (!r) throw new Error('No room "' + x + '". Rooms: ' + roomsArr().map(r => r.label).join(', ') + '.');
  return r;
}
function findNode(x) {
  if (x == null || x === '') throw new Error('id is required (an element GUID or its unique label).');
  const n = app.nodeByGuid.get(String(x));
  if (n) return n;
  let by = app.graph.nodes.filter(n => n.l.toLowerCase() === String(x).toLowerCase());
  if (by.length > 1) by = by.filter(n => n.c !== 'IfcOpeningElement');
  if (by.length === 1) return by[0];
  if (by.length > 1) throw new Error(`"${x}" is ambiguous (${by.length} elements); use a GUID from list_elements.`);
  throw new Error('No element "' + x + '". Use list_elements to find one.');
}
const storeyIdx = s => {
  if (s == null || s === '') return null;
  if (typeof s === 'number' || /^\d+$/.test(String(s))) { const i = +s; if (i < 0 || i >= app.graph.storeys.length) throw new Error('storey must be 0..' + (app.graph.storeys.length - 1)); return i; }
  const i = app.graph.storeys.findIndex(t => t.l.toLowerCase().startsWith(String(s).toLowerCase()));
  if (i < 0) throw new Error('No storey "' + s + '". Storeys: ' + app.graph.storeys.map((t, k) => k + ' ' + t.l).join(', ') + '.');
  return i;
};
const roomOfNode = n => (n.bd || []).map(i => app.graph.nodes[i]).filter(r => r.k === 'Space').map(r => r.l);

function counts() {
  const by = {}; app.graph.nodes.forEach(n => { by[n.c] = (by[n.c] || 0) + 1; });
  return { elements: app.graph.nodes.filter(n => n.k !== 'Space').length, rooms: roomsArr().length, doors: by.IfcDoor || 0, windows: by.IfcWindow || 0, byClass: by };
}
function walkState() {
  const w = app.walk.info();
  return w.active ? { active: true, x: w.x, y: w.y, z: w.z, storey: w.storey, room: w.room } : { active: false };
}
function pageState() {
  const sts = app.stateList().filter(s => s.kind === 'door' || s.kind === 'window');
  return {
    selection: app.selectedInfo(), visible: app.getVisibility(), colourMode: app.getColourMode(),
    sun: (({ date, time, azimuthDeg, elevationDeg, isDay }) => ({ date, time, azimuthDeg, elevationDeg, isDay }))(sunJson()),
    quality: app.getQuality(), interiorLights: app.getInteriorLights(), mode: app.mode, walk: walkState(),
    counts: counts(), open: sts.filter(s => s.state !== 'closed').map(s => ({ id: s.id, label: s.label, state: s.state })),
    lastQuery: app.lastQueryInfo(), loading: !app.ready, systemsLayer: !!app.systems,
  };
}
function sunJson() {
  const g = app.sun.get(), d = g.direction;
  return { date: g.date, time: g.timeStr, hours: r2(g.time), azimuthDeg: r2(g.azimuth), elevationDeg: r2(g.elevation), isDay: g.isDay,
    direction: [r2(d.x), r2(d.y), r2(d.z)], sunrise: g.sunriseStr, solarNoon: app.sun.hoursStr(g.solarNoon), sunset: g.sunsetStr, interiorLights: app.getInteriorLights(), site: g.site };
}
function roomInfo(r) {
  const s = r.box.getSize(new THREE.Vector3());
  return { id: r.guid, label: r.label, storey: r.storey, storeyLabel: app.graph.storeys[r.storey]?.l, floorAreaM2: r2(s.x * s.y), centre: [r2(r.center.x), r2(r.center.y), r2(r.floorZ)], size: [r2(s.x), r2(s.y), r2(s.z)] };
}

export function buildingTools() {
  const ro = { readOnlyHint: true };
  const obj = (properties, required) => ({ type: 'object', properties: properties || {}, required: required || [], additionalProperties: false });
  return [
    { name: 'get_state', annotations: ro,
      description: 'What the viewer shows: the selection, which classes and storeys are visible, the colour mode, time of day, whether you are walking (and where, in which room), counts, open doors and windows. Call this first.',
      inputSchema: obj(), execute: async () => pageState() },
    { name: 'list_elements', annotations: ro,
      description: 'Find building elements (walls, doors, windows, slabs, stair, railings, beams, members). Filter by IFC class, storey, room and text in the label. Rooms themselves come from list_rooms. Returns GUIDs for the other tools.',
      inputSchema: obj({ class: { type: 'string', description: 'IFC class, such as IfcDoor, Door, Window, Wall, Slab, Stair.' }, storey: { type: ['integer', 'string'], description: 'Storey index (0 = ground floor) or name.' }, room: { type: 'string', description: 'A room name or GUID: elements that bound that room.' }, text: { type: 'string', description: 'Part of the label.' }, limit: { type: 'integer', minimum: 1, maximum: 200, description: 'Default 50.' } }),
      execute: async a => {
        let cls = null; if (a.class) cls = app.normClass(a.class);
        const st = storeyIdx(a.storey); const room = a.room ? findRoom(a.room) : null;
        const txt = a.text ? String(a.text).toLowerCase() : null;
        const pool = app.graph.nodes.filter(n => n.k !== 'Space' && (!cls || n.c === cls) && (st == null || n.st === st) && (!room || (n.bd || []).includes(room.node.i)) && (!txt || n.l.toLowerCase().includes(txt)));
        const lim = Math.min(200, Math.max(1, Math.floor(+a.limit) || 50));
        const items = pool.slice(0, lim).map(n => { const o = { id: n.n, label: n.l, class: n.c, storey: n.st >= 0 ? n.st : null }; const s = app.getState(n.n); if (s) o.state = s; const r = roomOfNode(n); if (r.length) o.rooms = r; return o; });
        return { total: pool.length, returned: items.length, items };
      } },
    { name: 'select_element',
      description: 'Select an element or room as a click would: it is highlighted, the camera frames it and the info panel shows its details (floor, material, rooms it bounds, state). Pass a GUID or a unique label.',
      inputSchema: obj({ id: { type: 'string', description: 'GUID from list_elements or list_rooms, or a unique label.' } }, ['id']),
      execute: async a => { const n = findNode(a.id); app.select(n.n); return { selected: app.selectedInfo(), state: app.getState(n.n) || null, rooms: roomOfNode(n) }; } },
    { name: 'set_visibility',
      description: 'Choose what is drawn. classes is the full list of classes to show (names like Wall, Door, IfcSlab); storeys is the list of storey indexes to show; roof and roomNames are switches. Leave a field out to keep it as it is.',
      inputSchema: obj({ classes: { type: 'array', items: { type: 'string' } }, storeys: { type: 'array', items: { type: 'integer' } }, roof: { type: 'boolean' }, roomNames: { type: 'boolean' } }),
      execute: async a => { const v = app.setVisibility(a); return { visible: v }; } },
    { name: 'list_rooms', annotations: ro,
      description: 'The rooms of the house with their floor, floor area and centre (metres, IFC frame, Z up).',
      inputSchema: obj(), execute: async () => ({ rooms: roomsArr().map(roomInfo) }) },
    { name: 'go_to_room',
      description: 'Bring a room into view. In orbit mode the camera frames the room; in walk mode (or with mode "walk") the walker is placed in the room, on the right floor.',
      inputSchema: obj({ room: { type: 'string', description: 'Room name (Wohnen, Küche, Bad, Buero, Schlafzimmer, Flur, Galerie) or GUID.' }, mode: { type: 'string', enum: ['orbit', 'walk'] } }, ['room']),
      execute: async a => {
        const r = findRoom(a.room); const want = a.mode || app.mode;
        if (want === 'walk') {
          let [x, y] = app.walk.findFree(r.center.x, r.center.y, r.storey);
          if (app.walk.active) app.walk.teleport(x, y, { storey: r.storey }); else app.walk.enter({ x, y, storey: r.storey, yaw: 0, lock: false });
          return { mode: 'walk', walk: walkState(), room: r.label };
        }
        if (app.walk.active) app.walk.leave();
        app.setVisibility({ storeys: app.graph.storeys.map((s, i) => i) });
        app.select(r.guid); return { mode: 'orbit', room: roomInfo(r) };
      } },
    { name: 'set_element_state',
      description: 'Open, close or tilt a door or window. Doors: closed, open. Windows: closed, tilted, open. The leaf swings on its hinge; use list_states for ids and what is allowed.',
      inputSchema: obj({ id: { type: 'string', description: 'GUID or unique label of a door or window.' }, state: { type: 'string', enum: ['closed', 'open', 'tilted'] } }, ['id', 'state']),
      execute: async a => { const n = findNode(a.id); app.setState(n.n, a.state); return { id: n.n, label: n.l, state: app.getState(n.n), allowed: app.states(n.n) }; } },
    { name: 'list_states', annotations: ro,
      description: 'Everything that has a state (doors and windows), with its current state and the allowed values.',
      inputSchema: obj(), execute: async () => ({ items: app.stateList().map(s => ({ id: s.id, label: s.label, kind: s.kind, state: s.state, allowed: s.values })) }) },
    { name: 'get_sun', annotations: ro,
      description: 'Where the sun is: date, local time, azimuth (degrees clockwise from true north), elevation, whether it is day, the direction toward the sun in the model frame, and sunrise, solar noon and sunset for that date. Also the site (latitude, longitude, time zone, true north).',
      inputSchema: obj(), execute: async () => sunJson() },
    { name: 'set_sun',
      description: 'Place the sun by date and local time at the site (Karlsruhe, Europe/Berlin unless the model says otherwise). date is YYYY-MM-DD, time is HH:MM or hours 0 to 24; give either or both. Shadows, sky and light follow; below the horizon it is night and the rooms are lit from inside. Returns what get_sun returns.',
      inputSchema: obj({ date: { type: 'string', description: 'YYYY-MM-DD, for example 2026-06-21.' }, time: { type: ['string', 'number'], description: 'Local clock time, HH:MM or hours (13.5).' } }),
      execute: async a => { if (a.date == null && a.time == null) throw new Error('Give a date, a time, or both.'); app.sun.playDay(false); app.sun.set({ date: a.date, time: a.time }); return sunJson(); } },
    { name: 'set_colour_mode',
      description: 'realistic: materials and textures (the default). class: flat colours by IFC class, as in the legend of the left panel.',
      inputSchema: obj({ mode: { type: 'string', enum: ['realistic', 'class'] } }, ['mode']),
      execute: async a => { if (!['realistic', 'class'].includes(a.mode)) throw new Error('mode must be realistic or class.'); app.setColourMode(a.mode); return { colourMode: app.getColourMode() }; } },
    { name: 'walk',
      description: 'Enter or leave walk mode (first person, eye height 1.65 m). Entering starts outside the front door, facing it, or in the room you name. The walker cannot pass walls, closed doors or closed windows.',
      inputSchema: obj({ enter: { type: 'boolean', description: 'true to walk, false to go back to the orbit view. Default true.' }, room: { type: 'string', description: 'Start in this room.' } }),
      execute: async a => {
        if (a.enter === false) { if (app.walk.active) app.walk.leave(); return { mode: app.mode, walk: walkState() }; }
        if (a.room) { const r = findRoom(a.room); const [x, y] = app.walk.findFree(r.center.x, r.center.y, r.storey); if (app.walk.active) app.walk.teleport(x, y, { storey: r.storey }); else app.walk.enter({ x, y, storey: r.storey, yaw: 0, lock: false }); }
        else app.walk.enter({ lock: false });
        return { mode: app.mode, walk: walkState() };
      } },
    { name: 'list_examples', annotations: ro,
      description: 'The ready SPARQL queries, with their text. Prefixes: bot, cad, geo, geo3, geof3, rdfs; with the services layer also brick, sys, s. Elements are <https://w3id.org/rete/cad/fzk-haus/GUID>, services things are <https://w3id.org/rete/cad/fzk-haus/sys/ID>. The services group runs on the combined graph (house, services, thermal inputs).',
      inputSchema: obj(), execute: async () => ({ prefixes: QNS + (app.svc ? PRE_SERVICES : ''), graph: app.graphSource || null, examples: (app.examples || []).map((e, i) => ({ index: i, label: e.label, query: e.q })) }) },
    { name: 'run_sparql', annotations: ro,
      description: 'Run a SPARQL SELECT or ASK over the graph behind this page (the house, and with the services layer also the services and the thermal inputs) and get the rows back. The query and its table also show in the page, and matching elements highlight in 3D, unless show is false. Values are RDF terms as written in N-Triples.',
      inputSchema: obj({ query: { type: 'string', maxLength: 20000, description: 'SPARQL text with its PREFIX lines (see list_examples).' }, limit: { type: 'integer', minimum: 1, maximum: 500, description: 'Rows to return. Default 50.' }, show: { type: 'boolean', description: 'Show it in the page. Default true.' } }, ['query']),
      execute: async a => {
        const q = String(a.query || '').trim(); if (!q) throw new Error('query is empty.');
        const j = await runSparql(q);
        if (j.kind === 'ask') return { kind: 'ask', boolean: j.boolean };
        if (j.kind !== 'select') throw new Error('Only SELECT and ASK are answered here (got ' + j.kind + ').');
        if (a.show !== false) { if (app.console) app.console.setText(q); const r = toResult('Agent query', j); r.query = q; r.source = 'agent'; app.showResult(r, { open: true }); }
        const lim = Math.min(500, Math.max(1, Math.floor(+a.limit) || 50));
        let rows = j.rows.slice(0, lim).map(r => { const o = {}; j.vars.forEach(v => { if (r[v] != null && r[v] !== '') o[v] = String(r[v]).slice(0, 600); }); return o; });
        while (rows.length && JSON.stringify(rows).length > MCP_MAX) rows = rows.slice(0, Math.floor(rows.length * 0.8));
        const out = { kind: 'select', vars: j.vars, rows, returned: rows.length, total: j.rows.length };
        if (rows.length < j.rows.length) out.truncated = true;
        return out;
      } },
  ];
}

const reg = { tools: [], byName: {}, ctx: null, ac: null, names: [] };
// A tool whose module is not loaded yet loads it on its first call (the tool list is the same either way).
async function ensure(name, a) {
  if (name === 'get_thermal_model' || name === 'run_thermal_day' || name === 'get_window_sun') await app.loadThermal();
  else if ((name === 'walk' && a.enter !== false) || (name === 'go_to_room' && a.mode === 'walk')) await app.loadWalk();
}
function wrapTool(t) {
  return Object.assign({}, t, { execute: async a => {
    agentPill('Agent · ' + t.name + ' …'); const t0 = performance.now(); app.agentBusy = (app.agentBusy || 0) + 1;
    try {
      await app.readyPromise; await ensure(t.name, a || {}); const out = await t.execute(a || {}); const ms = Math.round(performance.now() - t0);
      agentPill('Agent · ' + t.name + ' ✓'); app.lastAgentCall = t.name + ' ✓';
      if (app.log) app.log('info', 'agent', `${t.name} · ${ms} ms`, { tool: t.name, args: brief(a), result: brief(out), bytes: (JSON.stringify(out) || '').length, ms });
      app.emit('agent', { tool: t.name, ok: true, ms }); return out;
    } catch (e) {
      const ms = Math.round(performance.now() - t0);
      agentPill('Agent · ' + t.name + ' ✗ ' + String(e.message || e).slice(0, 90), true); app.lastAgentCall = t.name + ' ✗';
      if (app.log) app.log('error', 'agent', `${t.name} failed: ${String(e.message || e).slice(0, 200)}`, { tool: t.name, args: brief(a), error: String(e.message || e).slice(0, 300), ms });
      app.emit('agent', { tool: t.name, ok: false, ms }); throw e;
    } finally { app.agentBusy = Math.max(0, (app.agentBusy || 1) - 1); }
  } });
}
function announce() {
  window.reteWebMCP = { supported: reg.names.length > 0, tools: reg.names.slice() };
  try { document.dispatchEvent(new CustomEvent('rete:webmcp', { detail: window.reteWebMCP })); } catch (_e) { /* ignore */ }
}
function register(list) {
  const { ctx } = reg;
  if (ctx && typeof ctx.registerTool === 'function') {
    list.forEach(t => { try { ctx.registerTool(t, { signal: reg.ac.signal }); reg.names.push(t.name); } catch (e) { console.debug('[rete webmcp] registerTool failed for', t.name, e && e.message); } });
  } else if (ctx && typeof ctx.provideContext === 'function') {
    try { ctx.provideContext({ tools: reg.tools }); list.forEach(t => reg.names.push(t.name)); } catch (e) { console.debug('[rete webmcp] provideContext failed', e && e.message); }
  }
}
// More tools after the page is built (the services layer adds its own). Same conventions as the first ones.
export function addTools(list) {
  const wrapped = list.map(wrapTool); wrapped.forEach(t => { reg.tools.push(t); reg.byName[t.name] = t; });
  register(wrapped); announce();
}
export function wireWebMcp() {
  window.reteBuildingTools = {
    list: () => reg.tools.map(t => ({ name: t.name, description: t.description, inputSchema: t.inputSchema, readOnly: !!(t.annotations && t.annotations.readOnlyHint) })),
    call: (name, args) => reg.byName[name] ? reg.byName[name].execute(args || {}) : Promise.reject(new Error('no tool named ' + name)),
  };
  try { reg.ctx = document.modelContext || null; } catch (_e) { /* none */ }
  if (!reg.ctx) { try { reg.ctx = navigator.modelContext || null; } catch (_e) { /* none */ } }
  reg.ac = new AbortController();
  addEventListener('pagehide', () => { try { reg.ac.abort(); } catch (_e) { /* ignore */ } }, { once: true });
  const tools = buildingTools().map(wrapTool); tools.forEach(t => { reg.tools.push(t); reg.byName[t.name] = t; });
  register(tools); announce();
}

// ---------- the shell: panels, logs, graph view, about, link ----------
export function shellTools() {
  const ro = { readOnlyHint: true };
  const obj = (properties, required) => ({ type: 'object', properties: properties || {}, required: required || [], additionalProperties: false });
  const txt = id => { const e = document.getElementById(id); return e ? e.textContent.replace(/\s+/g, ' ').trim() : ''; };
  const iriOfId = async x => {
    const s = String(x || ''); if (!s) throw new Error('id is required: an element GUID or label, a device id, or an IRI.');
    if (/^https?:\/\//.test(s)) return s;
    if (app.svc && app.svc.has(s)) return (app.systems.base || '') + s;
    const n = (() => { try { return findNode(s); } catch { return null; } })(); if (n) return 'https://w3id.org/rete/cad/fzk-haus/' + n.n;
    throw new Error('No thing "' + s + '". Use list_elements, list_devices or an IRI.');
  };
  return [
    { name: 'get_ui', annotations: ro,
      description: 'The state of the interface: which side panel, bottom tab and inspector are open, their sizes, the theme, density and units, the layout and camera mode, the selection and what the status bar shows (hover or room, sun, quality, graph source, WebMCP).',
      inputSchema: obj(), execute: async () => {
        const st = app.shell.state(), lay = (app.viewport && (app.viewport.getLayout ? app.viewport.getLayout() : app.viewport.layout)) || 'single';
        return { ...st, layout: lay, cameraMode: (app.camera && app.camera.getMode) ? app.camera.getMode() : app.mode, quality: app.getQuality(), selection: app.selectedInfo(), logUnread: app.logs ? app.logs.unread : 0,
          statusBar: { hover: txt('stHover'), selection: txt('stSel'), mode: txt('stMode'), sun: txt('stSun'), indicators: txt('stInd'), quality: txt('stQual'), graph: txt('stGraph'), webmcp: txt('stMcp'), message: txt('stMsg') },
          panels: app.shell.RAIL.map(r => r[0]), tabs: app.shell.TABS.map(t => t[0]) };
      } },
    { name: 'open_panel',
      description: 'Open or close a part of the interface. panel "side" with id explore, services, sun, thermal, states or query; panel "bottom" with id console, results, graph, thermal, scenarios or logs (open false collapses it); panel "inspector" (open true or false). Returns what get_ui returns.',
      inputSchema: obj({ panel: { type: 'string', enum: ['side', 'bottom', 'inspector'] }, id: { type: 'string' }, open: { type: 'boolean', description: 'Default true.' }, maximise: { type: 'boolean', description: 'Bottom panel only.' } }, ['panel']),
      execute: async a => {
        const on = a.open !== false, sh = app.shell;
        if (a.panel === 'side') { if (on) sh.openSide(a.id || 'explore'); else sh.closeSide(); }
        else if (a.panel === 'bottom') { if (on) sh.openTab(a.id || 'console'); else sh.closeBottom(); if (a.maximise != null) sh.maximiseBottom(!!a.maximise); }
        else if (a.panel === 'inspector') { if (on) sh.openInspector(); else sh.closeInspector(); }
        else throw new Error('panel must be side, bottom or inspector.');
        await new Promise(r => setTimeout(r, 80)); return sh.state();
      } },
    { name: 'get_logs', annotations: ro,
      description: 'The log of the page, newest first: user actions, queries with timings and row counts, simulations, scenarios, data loading, performance notes, errors and every tool call (source "agent"). Filter by source, minimum level (debug, info, warn, error) and text.',
      inputSchema: obj({ source: { type: 'string', description: 'user, query, simulation, scenario, data, perf, agent, app or error.' }, level: { type: 'string', enum: ['debug', 'info', 'warn', 'error'] }, text: { type: 'string' }, limit: { type: 'integer', minimum: 1, maximum: 200, description: 'Default 40.' } }),
      execute: async a => { const lim = Math.min(200, Math.max(1, Math.floor(+a.limit) || 40)); const items = app.logs.query({ source: a.source, level: a.level, text: a.text, limit: lim }).map(e => ({ time: new Date(e.t).toISOString().slice(11, 23), level: e.level, source: e.source, message: e.message, data: e.data })); const o = { returned: items.length, items }; return capList(o, 'items'); } },
    { name: 'show_in_graph',
      description: 'Open the Graph tab and focus a thing: an element or room (GUID or unique label), a device or run (id), or an IRI. Optionally switch the grouping first (class, namespace, system, storey, room, circuit). Returns whether it is in the view and the clusters that were opened.',
      inputSchema: obj({ id: { type: 'string' }, grouping: { type: 'string' } }, ['id']),
      execute: async a => { const iri = await iriOfId(a.id); const ok = await app.graphTab.focus(iri, a.grouping); const s = app.graphTab.state(); return { focused: !!ok, iri, grouping: s && s.grouping, expanded: s && s.expanded, visibleNodes: s && s.counts.nodes, selected: s && s.selected }; } },
    { name: 'get_graph_view', annotations: ro,
      description: 'What the Graph tab shows: the grouping and the groupings available, the clusters with their member counts and highlights, the nodes that are visible, the counts, the selection. Opens and loads the view if it is not there yet.',
      inputSchema: obj({ limit: { type: 'integer', minimum: 1, maximum: 300, description: 'Nodes to list. Default 60.' } }),
      execute: async a => {
        if (!app.graphTab.mounted) { app.shell.openTab('graph'); await app.graphTab.ready(); }
        const s = app.graphTab.state(), lim = Math.min(300, Math.max(1, Math.floor(+a.limit) || 60)); const cl = c => ({ id: c.id, label: c.label, count: c.count, highlight: c.highlight || 0 });
        return capList({ grouping: s.grouping, groupings: s.groupings.map(g => ({ id: g.id, label: g.label, available: g.available })), universe: s.universe, expanded: s.expanded, path: s.path.map(p => p.label || p.id), counts: s.counts, selected: s.selected, highlight: s.highlight, clusters: s.clusters.map(cl), nodes: s.nodes.slice(0, lim).map(n => ({ iri: n.iri, label: n.label, cluster: n.cluster })), nodesTotal: s.nodes.length }, 'nodes');
      } },
    { name: 'get_about', annotations: ro,
      description: 'The About content as data: what the page is, the house and what is real versus designed, the data files with sizes, the dataset card (title, licence, triple count), model notes, keyboard shortcuts and the WebMCP tools.',
      inputSchema: obj(), execute: async () => { const m = await import('./about.js'); const o = await m.aboutAsData(); return capList({ ...o, shortcuts: (await import('./commands.js')).shortcutTable() }, 'shortcuts'); } },
    { name: 'list_shacl_examples', annotations: ro,
      description: 'The SHACL example shapes in the data (id, name, description, what they expect: conforms or violations, the expected count and focus nodes). Empty until the data has them.',
      inputSchema: obj(), execute: async () => ({ supported: await app.validate.supported(), examples: app.validate.examples().map(e => ({ id: e.id, label: e.label, description: e.description, expect: e.expect, expectedCount: e.expectedCount, expectedFocus: e.expectedFocus })) }) },
    { name: 'run_shacl',
      description: 'Validate the graph against SHACL shapes in the browser: give an example id from list_shacl_examples, or shapes as Turtle text. Shows the report in the Validate tab and highlights the failing things in 3D. Returns conforms, the violation count and the violations (capped).',
      inputSchema: obj({ id: { type: 'string' }, shapes: { type: 'string', maxLength: 20000 }, show: { type: 'boolean', description: 'Open the Validate tab. Default true.' } }),
      execute: async a => {
        if (!a.id && !a.shapes) throw new Error('Give an example id or shapes text.');
        if (a.show !== false) app.shell.openTab('validate');
        const r = await app.validate.run({ id: a.id, shapes: a.id ? undefined : a.shapes });
        const o = { example: r.example, conforms: r.conforms, count: r.count, expectedCount: r.expectedCount, ms: r.ms, violations: r.results.slice(0, 60).map(v => ({ focusNode: v.focusNode, path: v.resultPath, value: v.value, message: v.resultMessage || (v.messages || []).join(' '), severity: v.resultSeverity || v.severity })) };
        return capList(o, 'violations');
      } },
    { name: 'get_link', annotations: ro,
      description: 'A link that reopens the current view: layout, camera, date and time, layers, selection, sections, the open panel and tab are in the URL hash.',
      inputSchema: obj(), execute: async () => ({ url: app.link.url(), hash: app.link.encode() }) },
    { name: 'open_link',
      description: 'Restore a view from a link made by get_link (a full URL or just the hash).',
      inputSchema: obj({ link: { type: 'string' } }, ['link']),
      execute: async a => { const ok = await app.link.apply(a.link); if (!ok) throw new Error('Not a link of this page (it must contain v=1).'); return { opened: true, now: app.link.encode() }; } },
  ];
}
// the tools of the rendering side (webmcp-viewport.js), when the module is there; the shell's own tools
export async function addExtraTools() {
  try { addTools(shellTools()); } catch (e) { console.error('[building] the shell tools failed to register', e); }
  try { const m = await import('./webmcp-viewport.js'); if (m && typeof m.viewportTools === 'function') addTools(m.viewportTools(app)); }
  catch (e) { if (app.log) app.log('debug', 'app', 'No viewport tools: ' + String(e.message || e).slice(0, 120)); }
}

// ---------- the services layer ----------
const capList = (obj, key, max = MCP_MAX) => { while (JSON.stringify(obj).length > max && obj[key].length > 1) { obj[key] = obj[key].slice(0, Math.floor(obj[key].length * 0.8)); obj.truncated = true; } return obj; };
const thingRef = t => ({ id: t.id, label: t.d.label, type: t.d.type || t.d.kind, system: t.system });
export function systemsTools() {
  const ro = { readOnlyHint: true };
  const obj = (properties, required) => ({ type: 'object', properties: properties || {}, required: required || [], additionalProperties: false });
  const svc = () => { if (!app.svc) throw new Error('The building-services layer is not loaded.'); return app.svc; };
  const th = () => { if (!app.thermal) throw new Error('The thermal model is not loaded.'); return app.thermal; };
  const dev = id => { const t = svc().thingFor(String(id || '')); if (!t) throw new Error(`No device or run "${id}". Use list_devices to find ids.`); return t; };
  const sysIds = () => svc().sysList.map(s => s.id).join(', ');
  const scenResult = r => {
    const groups = svc().byRoom(r.affected), out = {};
    Object.keys(groups).sort().forEach(k => { out[k] = groups[k]; });
    return { id: r.id, label: r.label, description: r.description, switchedOff: r.off, affectedCount: r.affected.length, expectedCount: r.expected, sameAsStoredAnswer: r.same, affectedRunCount: r.affectedRuns.length, affectedByRoom: out };
  };
  return [
    { name: 'list_systems', annotations: ro,
      description: 'The building systems (electrical, lighting, network, cold and hot water, drainage, heating, ventilation, cooling, safety): colour, number of devices and runs, which layers are visible, and what the supply starts from. These services are a designed example, not part of the IFC.',
      inputSchema: obj(), execute: async () => ({ note: svc().data.note, xray: svc().isXray(), isolated: svc().isolated, systems: svc().layerState().map(l => ({ ...l, sources: svc().sys.get(l.id).sources })) }) },
    { name: 'set_system_visibility',
      description: 'Show or hide the layer of one system (or "all"). Lighting is on by default; the rest is off so the house reads as a house.',
      inputSchema: obj({ system: { type: 'string', description: 'A system id from list_systems, or "all".' }, visible: { type: 'boolean' } }, ['system', 'visible']),
      execute: async a => { if (a.system === 'all') svc().layers.forEach(l => l.setVisible(!!a.visible)); else svc().setLayer(String(a.system), !!a.visible); return { layers: svc().layerState().map(l => ({ id: l.id, visible: l.visible })) }; } },
    { name: 'list_devices', annotations: ro,
      description: 'Find devices (sockets, switches, lights, breakers, valves, fixtures, units) and runs (cables, pipes, ducts). Filter by system, type, room and text in the label. kind is "nodes" or "runs". Returns ids for the other tools, with state and whether each is live.',
      inputSchema: obj({ system: { type: 'string' }, type: { type: 'string', description: 'Node type such as socket, breaker, light-ceiling, valve, wc; or run kind cable, pipe.' }, room: { type: 'string', description: 'Room name.' }, text: { type: 'string' }, kind: { type: 'string', enum: ['nodes', 'runs'] }, limit: { type: 'integer', minimum: 1, maximum: 200, description: 'Default 40.' } }),
      execute: async a => {
        if (a.system && !svc().sys.has(a.system)) throw new Error(`No system "${a.system}". Systems: ${sysIds()}.`);
        const pool = svc().find({ system: a.system, type: a.type, room: a.room, text: a.text, kind: a.kind }), lim = Math.min(200, Math.max(1, Math.floor(+a.limit) || 40));
        const o = { total: pool.length, items: pool.slice(0, lim).map(t => { const r = thingRef(t); if (t.room) r.room = t.room; if (t.hasState) r.state = svc().stateOf(t); r.live = t.live; return r; }) };
        o.returned = o.items.length; return capList(o, 'items');
      } },
    { name: 'get_device', annotations: ro,
      description: 'Everything about one device or run: properties, state, whether it is live and why not, what feeds it and what it feeds, the circuit, loop or zone it belongs to, what it controls.',
      inputSchema: obj({ id: { type: 'string' } }, ['id']),
      execute: async a => {
        const t = dev(a.id), s = svc(), o = s.info(t);
        o.fedBy = (s.parents.get(t.id) || []); o.feeds = (s.children.get(t.id) || []); o.controls = s.controls.get(t.id) || []; o.controlledBy = s.controlledBy.get(t.id) || [];
        o.groups = (s.groupsOf.get(t.id) || []).map(g => ({ id: g.id, kind: g.kind, label: g.label, props: g.props }));
        if (o.fedBy.length > 40) o.fedBy = o.fedBy.slice(0, 40); if (o.feeds.length > 60) { o.feedsCount = o.feeds.length; o.feeds = o.feeds.slice(0, 60); }
        app.select(t.id); return o;
      } },
    { name: 'set_device_state',
      description: 'Set the state of a device: lights on/off, switches (a switch turns the lights it controls), breakers and RCDs on/off, valves open/closed, heat pump, ventilation unit, router, air conditioner and so on (allowed values come from get_device). Everything downstream of an off or closed device loses power or water; the result says what is not live now.',
      inputSchema: obj({ id: { type: 'string' }, state: { type: 'string' } }, ['id', 'state']),
      execute: async a => { const t = dev(a.id); const r = svc().setState(t.id, a.state); app.select(t.id); return { ...r, notLiveNow: svc().nodes.filter(x => !x.live).length, allowed: app.states(t.id) }; } },
    { name: 'trace_supply',
      description: 'Follow the supply of a device back to its source (a socket to the junction, breaker, RCD, meter and grid connection; a tap to the water entry). The path and the cables or pipes on it are highlighted in the page.',
      inputSchema: obj({ id: { type: 'string' } }, ['id']),
      execute: async a => { const t = dev(a.id), r = svc().showTrace(t.id); return { from: t.id, source: r.source, path: r.path.map((id, i) => ({ id, label: r.labels[i] })), runs: r.runs, text: r.labels.join(' -> ') }; } },
    { name: 'list_scenarios', annotations: ro,
      description: 'Ready what-if scenarios (a breaker or RCD trips, the water or power is cut, the heat pump or router is off) with what each switches off and how many things it affects.',
      inputSchema: obj(), execute: async () => ({ scenarios: svc().scenarios.map(s => ({ id: s.id, label: s.label, description: s.description, switchesOff: s.off, affectedCount: s.affectedCount })) }) },
    { name: 'apply_scenario',
      description: 'Apply a scenario: it switches its devices off, follows the supply links, highlights everything affected, and lists the affected things grouped by room. With run_query the same question is asked of the graph with SPARQL and the two counts are compared.',
      inputSchema: obj({ id: { type: 'string' }, run_query: { type: 'boolean', description: 'Also run the scenario SPARQL on the graph. Default false.' } }, ['id']),
      execute: async a => {
        const r = svc().applyScenario(String(a.id)), o = scenResult(r);
        if (r.kind) o.kind = r.kind;
        if (a.run_query) { const q = await svc().runScenarioQuery(r.id); o.graph = { count: q.graphCount, modelCount: q.modelCount, sameSet: q.same, approximate: !!q.approximate, file: (q.graphUrl || '').split('/').pop() }; app.showResult(q.result, { open: true }); }
        o.sparql = r.sparql;
        while (JSON.stringify(o).length > MCP_MAX && Object.keys(o.affectedByRoom).length) { const k = Object.keys(o.affectedByRoom).sort((x, y) => o.affectedByRoom[y].length - o.affectedByRoom[x].length)[0]; o.affectedByRoom[k] = o.affectedByRoom[k].slice(0, 5).concat(['…']); o.truncated = true; if (JSON.stringify(o).length < MCP_MAX) break; if (o.affectedByRoom[k].length <= 6) { delete o.sparql; } }
        return o;
      } },
    { name: 'reset_states',
      description: 'Put every service state back to its default (lights off, breakers on, valves open ...) and, unless openings is false, close the doors and windows. Clears scenario highlights.',
      inputSchema: obj({ openings: { type: 'boolean', description: 'Also reset doors and windows. Default true.' } }),
      execute: async a => { svc().resetStates({ openings: a.openings !== false }); return { reset: true, notLiveNow: svc().nodes.filter(x => !x.live).length }; } },
    { name: 'set_xray',
      description: 'X-ray makes walls, slabs and the roof translucent so the services inside show. isolate fades everything but one system (a system id; null or "" to go back).',
      inputSchema: obj({ on: { type: 'boolean' }, isolate: { type: ['string', 'null'], description: 'System id from list_systems, or null.' } }),
      execute: async a => { if (a.on != null) svc().setXray(!!a.on); if (a.isolate !== undefined) svc().isolate(a.isolate || null); return { xray: svc().isXray(), isolated: svc().isolated }; } },
    { name: 'get_site', annotations: ro,
      description: 'The site of the house: name, latitude, longitude, time zone, and where true north points in the model (degrees clockwise from the model +Y axis). The IFC says the house is turned 50 degrees.',
      inputSchema: obj(), execute: async () => { const s = app.sun.site; return { name: s.name, latitude: s.latitude, longitude: s.longitude, timezone: s.timezone, northAzimuthDeg: +s.northAzimuthDeg.toFixed(2), trueNorthInModel: s.trueNorth.map(v => +v.toFixed(4)), source: svc().data.site.source }; } },
    { name: 'set_site',
      description: 'Change the site: turn true north (northAzimuthDeg, clockwise from model +Y), or latitude, longitude, time zone. The sun, shadows and the thermal results follow. A what-if for orientation: turn north by 180 degrees and run_thermal_day again.',
      inputSchema: obj({ northAzimuthDeg: { type: 'number' }, latitude: { type: 'number' }, longitude: { type: 'number' }, timezone: { type: 'string' } }),
      execute: async a => { const o = {}; for (const k of ['northAzimuthDeg', 'latitude', 'longitude', 'timezone']) if (a[k] != null) o[k] = a[k]; if (!Object.keys(o).length) throw new Error('Give at least one of northAzimuthDeg, latitude, longitude, timezone.'); const s = app.sun.setSite(o); return { latitude: s.latitude, longitude: s.longitude, timezone: s.timezone, northAzimuthDeg: +s.northAzimuthDeg.toFixed(2) }; } },
    { name: 'get_thermal_model', annotations: ro,
      description: 'The inputs of the thermal model: per room volume, set point, air change rates, internal gains, thermal capacity and design heat load; the constructions with U-values; the glazing; the windows with orientation and area; surfaces per room; and what the model assumes. Read this before trusting run_thermal_day.',
      inputSchema: obj(), execute: async () => { const m = th().model(); return capList({ ...m }, 'windows', MCP_MAX); } },
    { name: 'run_thermal_day',
      description: 'Simulate one day (10 minute steps, repeated until it closes): clear-sky sun scaled by clearness, ray-cast window shading, one temperature per room, the thermostat on the underfloor heating. Uses the current states of windows, doors, the ventilation unit and the heat pump; what you do not give is back at its default (clear sky, the typical outdoor temperatures of the month, heating on, blinds off). Returns per room min, max, time of the maximum, solar gain and heating energy in kWh, and an hourly series. A comparison tool, not a certified calculation.',
      inputSchema: obj({ date: { type: 'string', description: 'YYYY-MM-DD. Default: the date of the sun control.' }, clearness: { type: 'number', minimum: 0, maximum: 1, description: '1 clear sky, 0 overcast. Default 1.' }, outdoorMinC: { type: 'number' }, outdoorMaxC: { type: 'number' }, heating: { type: 'boolean', description: 'Default true.' }, blinds: { type: 'boolean', description: 'External blinds: a sunlit window is shaded while its room is above about 24 C. Default true.' }, summerBypass: { type: 'boolean', description: 'The ventilation unit stops recovering heat while the room is above its set point and the outdoor air is cooler. Default true.' }, nightVentilation: { type: 'boolean', description: 'More air change at night while the room is above about 23 C and the outdoor air is cooler. Default true.' } }),
      execute: async a => {
        if (a.date) { const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(a.date)); const ok = m && new Date(Date.UTC(+m[1], +m[2] - 1, +m[3])).getUTCDate() === +m[3] && +m[2] >= 1 && +m[2] <= 12; if (!ok) throw new Error('date must be a real date as YYYY-MM-DD, got "' + a.date + '".'); app.sun.playDay(false); app.sun.set({ date: String(a.date) }); }
        // what is not given goes back to its default, so a run depends only on what it says and on the states in the page
        const I = th().inputs; I.clearness = a.clearness != null ? Math.min(1, Math.max(0, +a.clearness)) : 1; const el = document.getElementById('thClear'); if (el) { el.value = Math.round(I.clearness * 100); document.getElementById('thClearV').textContent = el.value + ' %'; }
        const typ = th().typicalFor(app.sun.get().date); I.minmaxEdited = a.outdoorMinC != null || a.outdoorMaxC != null;
        if (I.minmaxEdited) { I.tmin = a.outdoorMinC ?? typ[0]; I.tmax = a.outdoorMaxC ?? typ[1]; if (I.tmax < I.tmin) I.tmax = I.tmin; } else { I.tmin = null; I.tmax = null; }
        const a1 = document.getElementById('thTmin'), a2 = document.getElementById('thTmax'); if (a1) { a1.value = I.tmin ?? typ[0]; a2.value = I.tmax ?? typ[1]; }
        I.heating = a.heating !== false; const hel = document.getElementById('thHeat'); if (hel) hel.checked = I.heating;
        I.blinds = a.blinds !== false; const bel = document.getElementById('thBlinds'); if (bel) bel.checked = I.blinds;
        I.summerBypass = a.summerBypass !== false; const sbe = document.getElementById('thBypass'); if (sbe) sbe.checked = I.summerBypass;
        I.nightVent = a.nightVentilation !== false; const nve = document.getElementById('thNightVent'); if (nve) nve.checked = I.nightVent;
        const r = app.thermalUi.runAndShow();
        const hourly = [], idx = []; for (let h = 0; h <= 24; h++) idx.push(h * 6);
        const o = { date: r.date, clearness: r.clearness, outdoorMinC: r.tmin, outdoorMaxC: r.tmax, heating: r.heating, ventilationHeatRecovery: r.hrv, options: { blinds: r.blinds, summerBypass: r.summerBypass, nightVentilation: r.nightVent }, groundC: r.groundC, runMs: r.ms, settled: r.converged, totals: r.totals, rooms: r.rows.map(x => ({ room: x.room, minC: x.minC, maxC: x.maxC, peakTime: x.peakTime, swingK: x.swingK, solarGainKWh: x.solarKWh, heatingKWh: x.heatingKWh, sunOnRoom: x.sun, setpointC: x.setpointC })),
          hourly: { hours: idx.map(i => i / 6), outdoorC: idx.map(i => +r.tOut[i].toFixed(1)), roomC: Object.fromEntries(r.rows.map((x, k) => [x.room, idx.map(i => +r.temps[k][i].toFixed(2))])) },
          windows: r.windowRows.map(w => ({ id: w.id, label: w.label, room: w.room, azimuthDeg: w.azimuthDeg, sunHours: w.sunHours, directGainKWh: w.directGainKWh })) };
        void hourly; return capList(o, 'windows');
      } },
    { name: 'get_window_sun', annotations: ro,
      description: 'The sun on windows at one moment: sunlit fraction (rays from the window against the roof, walls and reveals), incident irradiance in W/m2 and the power through the glass. All windows, or one by id or label. Time is local hours or HH:MM on the date of the sun control, unless a date is given.',
      inputSchema: obj({ window: { type: 'string', description: 'Window id (surf-...) or label such as EG-Fenster-5. Default all.' }, time: { type: ['string', 'number'] }, date: { type: 'string' }, clearness: { type: 'number', minimum: 0, maximum: 1, description: 'Default 1 (clear sky).' } }),
      execute: async a => {
        let time = a.time; if (typeof time === 'string') { const m = /^(\d{1,2}):(\d{2})$/.exec(time); if (!m) throw new Error('time must be hours or HH:MM'); time = +m[1] + m[2] / 60; }
        const r = th().windowSun(time, a.date, a.clearness ?? 1); if (a.window) { const k = String(a.window).toLowerCase(); r.windows = r.windows.filter(w => w.id.toLowerCase() === k || w.label.toLowerCase() === k); if (!r.windows.length) throw new Error(`No window "${a.window}". Use get_thermal_model for the windows.`); }
        return r;
      } },
  ];
}

// ---------- speed and look: performance, quality, scenery ----------
export function perfTools() {
  const ro = { readOnlyHint: true };
  const obj = (properties, required) => ({ type: 'object', properties: properties || {}, required: required || [], additionalProperties: false });
  const loaded = () => {
    const mods = ['batch', 'scenery'].filter(k => k === 'batch' ? !!app.batchSetList : !!app.scenery);
    if (app.svc) mods.push('services'); if (app.walk && !app.walk.stub) mods.push('walk'); if (app.thermal) mods.push('thermal');
    return mods;
  };
  return [
    { name: 'get_performance', annotations: ro,
      description: 'How the page draws: renderer counts (draw calls, triangles, geometries, textures, shader programs) of the last frame, the quality level and what it sets, the last frame times, the batches that draw the house and the services, which modules are loaded, and the load timings in ms since the page started.',
      inputSchema: obj(),
      execute: async () => {
        const i = app.renderer.info, L = app.qualityLevels()[app.getQuality()];
        return {
          quality: app.getQuality(), automatic: app.getQualityAuto(), level: L, pixelRatio: app.renderer.getPixelRatio(), shadows: app.renderer.shadowMap.enabled,
          render: { drawCalls: i.render.calls, triangles: i.render.triangles, lines: i.render.lines, points: i.render.points },
          memory: { geometries: i.memory.geometries, textures: i.memory.textures, programs: i.programs ? i.programs.length : null },
          frames: { rendered: app.frameCount, lastRenderMs: r2(app.perf.renderMs), recentFrameMs: r2(app.perf.frameMs), animating: app.isAnimating() },
          batches: [...(app.batchSetList || [])].map(s => ({ name: s.name, ...s.stats() })),
          scenery: app.scenery ? app.scenery.get() : null,
          modulesLoaded: loaded(), timingsMs: { ...app.timing },
        };
      } },
    { name: 'set_quality',
      description: 'Set the quality level: low (pixel ratio 1, no shadows, little scenery, still), medium (pixel ratio 1.5, 1024 px shadows, scenery at 24 frames/s) or high (pixel ratio 2, 2048 px shadows, full scenery). It is chosen from the device at start; this overrides it.',
      inputSchema: obj({ level: { type: 'string', enum: ['low', 'medium', 'high'] } }, ['level']),
      execute: async a => {
        if (!['low', 'medium', 'high'].includes(a.level)) throw new Error('level must be low, medium or high.');
        app.setQuality(a.level); return { quality: app.getQuality(), settings: app.qualityLevels()[a.level] };
      } },
    { name: 'set_scenery',
      description: 'The stylised surroundings (dithered sky with sun, moon, stars and clouds; ground, trees, bushes, rocks, fence, path; birds and fireflies). on shows or hides them; dither is the strength of the dither pattern 0 to 1; wind 0 to 1 sways the trees and drifts the clouds (0 stops the animation); animate false freezes it; treesShade lets the trees shade the windows in the thermal model (default off). Without arguments it returns the current settings.',
      inputSchema: obj({ on: { type: 'boolean' }, dither: { type: 'number', minimum: 0, maximum: 1 }, wind: { type: 'number', minimum: 0, maximum: 1 }, animate: { type: 'boolean' }, treesShade: { type: 'boolean' } }),
      execute: async a => { const s = await app.loadScenery(); const o = {}; for (const k of ['on', 'dither', 'wind', 'animate', 'treesShade']) if (a[k] != null) o[k] = a[k]; return s.set(o); } },
  ];
}
