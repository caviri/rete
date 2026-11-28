// The services layer in the shell: the system rows with x-ray and isolate (Services panel), the device list (States
// panel), the info rows of a pick (Inspector), the search entries, the Scenarios tab and the second group of ready
// SPARQL queries.
import { app } from './app.js';
import { PRE_SERVICES, runSparql, toResult } from './queries.js';
import { $, esc, has } from './shared.js';

export function initSystemsUi(svc) {
  const data = svc.data;
  const absent = $('svcAbsent'); if (absent) absent.remove();

  // ---------- layers, x-ray, isolate ----------
  const lay = $('layers');
  const head = document.createElement('div');
  head.innerHTML = `<p class="note">A designed example, not part of the original IFC: electrical, lighting, network, water, drainage, heating, ventilation, cooling, safety and structure. Cables are drawn thicker than they are so they read.</p>
    <div class="bar"><button class="btn sm" type="button" id="svcAll">All</button><button class="btn sm" type="button" id="svcNone">None</button></div>
    <label class="opt"><input type="checkbox" id="xrayCb"><span>X-ray: see through walls, floors and roof</span></label>
    <div class="row"><span>Isolate one system</span><select id="isoSel" class="sel" style="width:130px" aria-label="Isolate one system"><option value="">all systems</option>${svc.sysList.map(s => `<option value="${s.id}">${esc(s.label)}</option>`).join('')}</select></div>
    <div class="svc-h" style="margin-top:10px">Systems</div>`;
  lay.prepend(head);
  $('layersSec').querySelector('summary').textContent = 'Building services';
  for (const s of svc.sysList) {
    const h = app.registerLayer({ id: 'svc-' + s.id, label: s.label, color: s.color, group: svc.sysGroup.get(s.id), count: svc.counts(s.id), visible: s.id === 'lighting' });
    svc.layers.set(s.id, h);
  }
  $('svcAll').onclick = () => svc.layers.forEach(l => l.setVisible(true));
  $('svcNone').onclick = () => svc.layers.forEach(l => l.setVisible(false));
  $('xrayCb').onchange = e => svc.setXray(e.target.checked);
  app.on('xray', e => { $('xrayCb').checked = svc.xrayWanted || e.on; });
  $('isoSel').onchange = e => svc.isolate(e.target.value || null);
  app.on('isolate', e => { $('isoSel').value = e.system || ''; });

  // ---------- the info of a pick ----------
  const unit = (k, v) => {
    if (typeof v === 'boolean') return v ? 'yes' : 'no';
    if (typeof v !== 'number') return String(v);
    const u = [[/(^|[a-z])W$/, ' W'], [/KW$/, ' kW'], [/A$/, ' A'], [/Mm$/, ' mm'], [/M3h$/, ' m³/h'], [/M$/, ' m'], [/L$/, ' l'], [/Lmin$/, ' l/min'], [/C$/, ' °C'], [/V$/, ' V'], [/mA$/, ' mA']].find(([re]) => re.test(k));
    return v + (u ? u[1] : '');
  };
  const nice = k => k.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/^./, c => c.toUpperCase());
  const link = (id, label) => { const a = document.createElement('button'); a.type = 'button'; a.className = 'lnk'; a.textContent = label || svc.thingFor(id)?.d.label || id; a.onclick = () => app.select(id); return a; };
  function listBlock(title, ids, cap = 14) {
    if (!ids.length) return null;
    const box = document.createElement('div'); box.innerHTML = `<div class="svc-h">${esc(title)} <span class="badge">${ids.length}</span></div>`;
    const ul = document.createElement('ul'); ul.className = 'svc-list';
    ids.slice(0, cap).forEach(id => { const t = svc.thingFor(id); const li = document.createElement('li'); li.tabIndex = 0; li.setAttribute('role', 'button'); li.innerHTML = `<span>${esc(t ? t.d.label : id)}</span><span class="m">${esc(t ? (t.d.type || t.d.kind) : '')}</span>`; li.onclick = () => app.select(id); ul.appendChild(li); });
    if (ids.length > cap) { const li = document.createElement('li'); li.className = 'muted'; li.textContent = `+ ${ids.length - cap} more`; ul.appendChild(li); }
    box.appendChild(ul); return box;
  }
  app.registerInfo(thing => {
    if (thing.kind !== 'pickable' || !svc.has(thing.id)) return null;
    const t = svc.thingFor(thing.id), d = t.d, out = document.createElement('div');
    const rows = [];
    if (t.kind === 'node') {
      rows.push(['Room', t.room || 'outside'], ['Floor', app.graph.storeys[d.storey]?.l || String(d.storey)], ['IFC class', d.ifc], ['Position (m)', d.p.map(v => v.toFixed(2)).join(', ')]);
    } else rows.push(['Kind', d.kind], ['From', null, d.from], ['To', null, d.to], ['Diameter', (d.d * 1000).toFixed(0) + ' mm' + (d.kind === 'cable' ? ' (drawn thicker)' : '')]);
    for (const [k, v] of Object.entries(d.props || {})) rows.push([nice(k), unit(k, v)]);
    const sec = (title, body, open = true) => { const dd = document.createElement('details'); dd.className = 'sec'; dd.open = open; dd.style.margin = '0 -12px'; dd.innerHTML = `<summary>${esc(title)}</summary>`; const b = document.createElement('div'); b.className = 'sec-b'; b.append(body); dd.append(b); return dd; };
    const tab = document.createElement('table'); tab.className = 'proptab';
    for (const [k, v, id] of rows) { const tr = document.createElement('tr'); const td = document.createElement('td'); td.textContent = k; const tv = document.createElement('td'); if (id) tv.appendChild(link(id)); else tv.textContent = v; tr.append(td, tv); tab.appendChild(tr); }
    out.appendChild(sec('Properties', tab));
    // state and buttons
    const live = document.createElement('div');
    if (t.hasState) {
      const s = svc.stateOf(t), box = document.createElement('div');
      box.innerHTML = `<div class="kv"><b>State:</b> <span class="stnow">${esc(s)}</span></div>`;
      const bt = document.createElement('div'); bt.className = 'stbtns';
      for (const v of app.states(t.id)) { const b = document.createElement('button'); b.type = 'button'; b.className = 'btn sm' + (v === s ? ' on' : ''); b.textContent = v.charAt(0).toUpperCase() + v.slice(1); b.setAttribute('aria-pressed', String(v === s)); b.onclick = () => svc.setState(t.id, v); bt.appendChild(b); }
      box.appendChild(bt); live.appendChild(box);
    }
    const lv = document.createElement('div'); lv.className = 'kv';
    if (t.live) lv.innerHTML = `<b>Supply:</b> <span class="live-ok">live</span>`;
    else { lv.innerHTML = `<b>Supply:</b> <span class="live-bad">not live</span>`; if (t.why) { const w = document.createElement('div'); w.className = 'live-bad'; w.style.fontSize = '12.5px'; w.append(t.why.text + ' · '); w.appendChild(link(t.why.by, 'go to it')); lv.appendChild(w); } }
    live.appendChild(lv); out.appendChild(sec('State and supply', live));
    // supply links
    if (t.kind === 'node') {
      const bl = [listBlock('Fed by', svc.parents.get(t.id) || []), listBlock('Feeds', svc.children.get(t.id) || []), listBlock('Controls', svc.controls.get(t.id) || []), listBlock('Controlled by', svc.controlledBy.get(t.id) || [])].filter(Boolean);
      if (bl.length) { const w = document.createElement('div'); bl.forEach(b => w.appendChild(b)); out.appendChild(sec('Links', w, false)); }
    }
    // circuit, loop, zone
    const gs = svc.groupsOf.get(t.id) || [];
    if (gs.length) {
      const g = document.createElement('div');
      gs.forEach(gr => {
        const p = gr.props || {}; let extra = '';
        if (p.loadW != null && p.ratingA) extra = ` · ${p.loadW} W on ${p.ratingA} A (${Math.round(100 * p.loadW / (p.ratingA * 230))} % of ${p.ratingA * 230} W)`;
        else if (p.heatLoadW != null) extra = ` · design heat load ${p.heatLoadW} W, set point ${p.setpointC} °C`;
        const row = document.createElement('div'); row.className = 'kv'; row.append(gr.label + extra);
        if (p.breaker && svc.has(p.breaker)) { row.append(' · '); row.appendChild(link(p.breaker, 'breaker')); }
        g.appendChild(row);
      });
      out.appendChild(sec('Belongs to', g));
    }
    // trace
    const tr = document.createElement('div');
    const b1 = document.createElement('button'); b1.type = 'button'; b1.className = 'btn sm'; b1.textContent = 'Trace to source'; b1.onclick = () => svc.showTrace(t.id);
    const b2 = document.createElement('button'); b2.type = 'button'; b2.className = 'btn sm'; b2.style.marginLeft = '6px'; b2.textContent = 'Clear trace'; b2.onclick = () => svc.clearTrace();
    tr.append(b1, b2);
    const lt = svc.lastTrace;
    if (lt && (lt.path.includes(t.id) || lt.id === t.id)) {
      const p = document.createElement('div'); p.className = 'kv'; p.style.fontSize = '12px'; p.style.marginTop = '6px';
      p.innerHTML = `<b>Path:</b> ` + lt.labels.map(esc).join(' → ') + `<br><span class="muted">${lt.runs.length} cable or pipe run(s) on the way</span>`;
      tr.appendChild(p);
    }
    out.appendChild(sec('Trace', tr));
    return out;
  });
  const refreshIfSelected = () => { const s = app.selectedInfo(); if (s && s.kind === 'pickable' && svc.has(s.id)) app.refreshInfo(); };
  app.on('svclive', refreshIfSelected); app.on('trace', refreshIfSelected);

  // ---------- search ----------
  app.registerSearch(q => svc.find({ text: q }).map(t => ({ label: t.d.label, tag: svc.sys.get(t.system).label + ' · ' + (t.d.type || t.d.kind), run: () => app.select(t.id) })));

  // ---------- the devices with a state (States panel) ----------
  const devs = svc.nodes.filter(t => t.hasState).sort((a, b) => a.d.type.localeCompare(b.d.type) || a.d.label.localeCompare(b.d.label));
  const db = $('devBody'); $('devCount').textContent = `(${devs.length})`;
  db.innerHTML = `<input class="inp" id="devFilter" type="search" placeholder="Filter by name or type" aria-label="Filter devices" style="width:100%;margin-bottom:6px"><div id="devList" style="max-height:260px;overflow:auto"></div>`;
  const drawDevs = () => {
    const f = $('devFilter').value.trim().toLowerCase(), box = $('devList'); box.innerHTML = '';
    const pool = devs.filter(t => !f || t.d.label.toLowerCase().includes(f) || t.d.type.includes(f)); let n = 0;
    for (const t of pool) {
      if (++n > 120) { const p = document.createElement('p'); p.className = 'note'; p.textContent = `${pool.length - 120} more: refine the filter`; box.append(p); break; }
      const row = document.createElement('div'); row.className = 'strow'; row.dataset.dev = t.id;
      row.innerHTML = `<button class="lnk stl" type="button" title="${esc(t.d.label)}">${esc(t.d.label.split(',')[0])}</button><button class="btn sm stb" type="button" aria-label="Change the state of ${esc(t.d.label)}">${esc(svc.stateOf(t))}</button>`;
      row.querySelector('.stl').onclick = () => app.select(t.id); row.querySelector('.stb').onclick = () => { const vs = app.states(t.id), i = vs.indexOf(svc.stateOf(t)); svc.setState(t.id, vs[(i + 1) % vs.length]); };
      box.append(row);
    }
  };
  $('devFilter').oninput = drawDevs; drawDevs();
  let devT = 0; app.on('state', e => { if (!svc.has(e.id)) return; const b = document.querySelector(`.strow[data-dev="${CSS.escape(e.id)}"] .stb`); if (b) b.textContent = e.state; });
  void devT;

  // ---------- result highlight ----------
  app.on('result', e => { svc.tints.select = new Set(); svc.setTint('result', e.picks || []); });
  app.on('resultclear', () => { svc.tints.select = new Set(); svc.setTint('result', []); });

  // ---------- scenarios (the Scenarios tab) ----------
  const tp = $('tp-scenarios'); const emptyMsg = $('scEmpty'); if (emptyMsg) emptyMsg.remove();
  const kinds = [['supply', 'Supply: what loses power or water'], ['structure', 'Structure: what loses its support']];
  tp.innerHTML = `<div class="scen-grid"><div class="scen-list" id="scenList" role="group" aria-label="Scenarios"></div><div class="scen-out" id="scenOut"><p class="muted">Switch something off or remove a wall, and see what is affected. The answer is also a SPARQL query on the graph.</p></div></div>`;
  const list = $('scenList');
  for (const [k, title] of kinds) {
    const items = data.scenarios.filter(s => (s.kind || 'supply') === k); if (!items.length) continue;
    const h = document.createElement('div'); h.className = 'svc-h'; h.textContent = `${title} (${items.length})`; list.append(h);
    for (const s of items) { const b = document.createElement('button'); b.type = 'button'; b.className = 'scen-btn'; b.dataset.id = s.id; b.dataset.kind = k; b.innerHTML = `${esc(s.label)}<small>${esc(s.affectedCount != null ? s.affectedCount + ' affected' : '')}${k === 'structure' ? ' (exact)' : ''}</small>`; b.onclick = () => applyScenario(s.id); list.append(b); }
  }
  const rb = document.createElement('div'); rb.className = 'bar'; rb.style.marginTop = '10px'; rb.innerHTML = `<button class="btn sm" type="button" id="scenReset">Reset all states</button>`; list.append(rb);
  $('scenReset').onclick = () => { resetAll(); };
  const scenSide = $('scenSide'); if (scenSide) scenSide.innerHTML = `<button class="btn sm" type="button" id="scenOpen">Open the scenarios</button>`;
  const so = $('scenOpen'); if (so) so.onclick = () => app.shell.openTab('scenarios');

  const resetAll = () => { svc.resetStates(); return true; };   // clears either kind and restores removed elements
  const applyScenario = id => svc.applyScenario(id);          // both kinds, the same result shape (structure.js computes what loses its support)
  app.scenarioApi = { apply: applyScenario, reset: resetAll };
  app.on('scenario', r => {
    list.querySelectorAll('.scen-btn').forEach(b => { const on = !!r && b.dataset.id === r.id; b.classList.toggle('on', on); b.setAttribute('aria-pressed', String(on)); });
    if (!r) $('scenOut').innerHTML = '<p class="muted">No scenario applied.</p>'; else showScenario(r);
  });
  function showScenario(r) {
    const groups = svc.byRoom(r.affected), out = $('scenOut'); const structural = r.kind === 'structure'; const sc0 = data.scenarios.find(x => x.id === r.id) || {};
    const off = r.off.map(id => svc.thingFor(id)?.d.label || app.nodeByGuid.get(id)?.l || id);
    out.innerHTML = `<div class="svc-h" style="margin-top:0">${esc(r.label)}</div><div class="muted" style="font-size:12px;margin-bottom:4px">${esc(r.description)}</div>
      <div class="kv"><b>${structural ? 'Removed' : 'Switched off'}:</b> ${esc(off.join(', '))}</div>
      <div class="kv"><b>Affected:</b> ${r.affected.length} things${structural ? '' : ` (${r.affectedRuns.length} cables or pipes dim too)`} <span class="${r.same ? 'scen-ok' : 'scen-no'}">${structural ? '· the exact set from the data' : r.same ? '· same as the stored answer' : '· differs from the stored answer (' + r.expected + ')'}</span></div>
      <div id="scenGroups"></div>
      <details><summary class="muted">The SPARQL for this</summary><pre>${esc(r.sparql.replace(/ (PREFIX|SELECT|WHERE|ORDER) /g, '\n$1 ').replace(/\{ /g, '{\n  ').replace(/ \. /g, ' .\n  '))}</pre>${structural && sc0.sparqlNote ? `<div class="note"><b>Approximation.</b> ${esc(sc0.sparqlNote)}</div>` : ''}</details>
      <div class="bar" style="margin-top:6px"><button class="btn sm" type="button" id="scenRun">Run on the graph</button></div><div id="scenRes" class="muted" style="font-size:12px"></div>`;
    const gbox = $('scenGroups');
    Object.keys(groups).sort().forEach(room => {
      const d = document.createElement('details'); d.innerHTML = `<summary>${esc(room)} <span class="badge">${groups[room].length}</span></summary>`;
      const ul = document.createElement('ul'); ul.className = 'svc-list';
      groups[room].forEach(id => { const t = svc.thingFor(id); const li = document.createElement('li'); li.tabIndex = 0; li.setAttribute('role', 'button'); li.innerHTML = `<span>${esc(t ? t.d.label : id)}</span><span class="m">${esc(t ? t.d.type : '')}</span>`; li.onclick = () => app.select(id); ul.appendChild(li); });
      d.appendChild(ul); gbox.appendChild(d);
    });
    $('scenRun').onclick = async () => {
      const res = $('scenRes'); res.textContent = 'running on the graph…';
      try {
        const q = await svc.runScenarioQuery(r.id);
        res.innerHTML = q.approximate
          ? `Graph (an approximation): <b>${q.graphCount}</b> things bear on the removed element · exact set: <b>${q.modelCount}</b> · <span class="${q.same ? 'scen-ok' : 'scen-no'}">${q.same ? 'the exact set is inside the graph answer' : 'the exact set is not inside the graph answer'}</span>`
          : `Graph: <b>${q.graphCount}</b> · model: <b>${q.modelCount}</b> · <span class="${q.same ? 'scen-ok' : 'scen-no'}">${q.same ? 'the same set of things' : 'different sets'}</span><br><span class="muted">${esc((q.graphUrl || '').split('/').pop())}</span>`;
        app.showResult(q.result, { open: true }); app.scenarioQuery = { id: r.id, graphCount: q.graphCount, modelCount: q.modelCount, same: q.same, approximate: !!q.approximate };
        if (app.log) app.log('info', 'scenario', `Graph answer for "${r.label}": ${q.graphCount} things, model ${q.modelCount}, ${q.same ? 'same' : 'different'}${q.approximate ? ' (approximation)' : ''}`);
      } catch (e) { res.textContent = 'Query error: ' + (e.message || e); }
    };
  }

  // ---------- the second group of ready queries ----------
  const Q = (label, desc, q) => ({ label, desc, q, pre: PRE_SERVICES });
  app.addExamples('Building services (SPARQL on the graph)', [
    Q('Sockets per room', 'How many wall sockets each room has (sys:nodeType "socket"); the rooms light up.',
      `SELECT ?r ?room (COUNT(?s) AS ?sockets) WHERE {\n  ?r a bot:Space ; rdfs:label ?room ; bot:containsElement ?s .\n  ?s sys:nodeType "socket" . FILTER(LANG(?room) = "en")\n} GROUP BY ?r ?room ORDER BY DESC(?sockets)`),
    Q('Load per circuit against its breaker', 'Connected load of every circuit against what its breaker allows at 230 V; the breakers light up.',
      `SELECT ?cb ?label ?loadW ?ratingA (ROUND(100 * ?loadW / (?ratingA * 230)) AS ?percent) WHERE {\n  ?g a sys:Circuit ; rdfs:label ?label ; sys:loadW ?loadW ; sys:ratingA ?ratingA ; sys:breaker ?cb . FILTER(LANG(?label) = "en")\n} ORDER BY DESC(?loadW)`),
    Q('What the kitchen breaker feeds', 'Everything downstream of breaker C10 by brick:feeds, with its room.',
      `SELECT ?thing ?label ?room WHERE {\n  s:cb-10 brick:feeds+ ?thing . ?thing rdfs:label ?label . FILTER(LANG(?label) = "en")\n  OPTIONAL { ?r bot:containsElement ?thing . ?r rdfs:label ?room . FILTER(LANG(?room) = "en") }\n} ORDER BY ?thing`),
    Q('What RCD 1 protects, by kind', 'Counts of things behind RCD 1, by type.',
      `SELECT ?type (COUNT(?thing) AS ?n) WHERE {\n  s:rcd-1 brick:feeds+ ?thing . ?thing sys:nodeType ?type .\n} GROUP BY ?type ORDER BY DESC(?n)`),
    Q('South-facing windows: area and g-value', 'Windows whose outward normal points between 135 and 225 degrees from true north, with area and solar transmittance; the windows light up.',
      `SELECT ?w ?label ?az ?area ?g WHERE {\n  ?s a sys:Surface ; sys:kind "window" ; sys:azimuthDeg ?az ; sys:areaM2 ?area ; sys:gValue ?g ; sys:element ?w .\n  ?w rdfs:label ?label . FILTER(?az >= 135 && ?az <= 225 && LANG(?label) = "en")\n} ORDER BY ?az`),
    Q('Surfaces of the bathroom with U-values', 'Every surface that bounds the bathroom: kind, what is on the other side, area and U-value; the walls and slabs light up.',
      `SELECT ?e ?kind ?boundary ?area ?u WHERE {\n  ?room rdfs:label "Bathroom"@en .\n  ?s sys:room ?room ; sys:kind ?kind ; sys:boundary ?boundary ; sys:areaM2 ?area .\n  OPTIONAL { ?s sys:uValue ?u } OPTIONAL { ?s sys:element ?e }\n} ORDER BY ?kind`),
    Q('Rooms by design heat load', 'Design heat load and floor area of each room (simplified EN 12831), largest first.',
      `SELECT ?r ?label ?load ?area WHERE {\n  ?z a sys:Zone ; sys:room ?r ; sys:heatLoadW ?load ; sys:floorAreaM2 ?area .\n  ?r rdfs:label ?label . FILTER(LANG(?label) = "en")\n} ORDER BY DESC(?load)`),
    Q('Fixtures that drain to the sewer', 'Which fixtures reach the sewer connection through the drainage network.',
      `SELECT ?fixture ?label WHERE {\n  ?fixture sys:drainsTo ?d . ?d brick:feeds+ s:sewer-connection .\n  ?fixture rdfs:label ?label . FILTER(LANG(?label) = "en")\n}`),
  ]);
}
