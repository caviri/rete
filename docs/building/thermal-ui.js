// The thermal day: the inputs in the Thermal side panel, the results (chart, table, windows) in the Thermal tab of
// the bottom panel, room temperatures tinted on the floors, the sun on the windows.
import * as THREE from 'three';
import { app } from './app.js';
import { typicalFor } from './thermal.js';

const $ = id => document.getElementById(id);
const esc = s => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');
const COLORS = ['#4e9af1', '#f5a623', '#7ed957', '#e86a92', '#b48cff', '#27c4c4', '#f2d04a'];
const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];


export function initThermalUi(svc, th) {
  const sun = app.sun;
  const css = k => getComputedStyle(document.documentElement).getPropertyValue(k).trim() || '#888';

  // ---------- inputs ----------
  const sec = $('thermSec');   // the panel is in the page; this fills it
  sec.className = 'pad';
  sec.innerHTML = `<p class="note">Sun on the windows and the temperature of each room over one day. Single-node rooms, clear-sky sun, a typical-day outdoor temperature and assumed insulation: for comparing orientations and what-ifs, not a certified calculation. Results are in the Thermal tab.</p>
    <div class="row"><span>Date</span><input type="date" id="thDate" class="inp" aria-label="Date"></div>
    <div class="row"><span>Sky clearness</span><b id="thClearV">100 %</b></div>
    <input type="range" id="thClear" class="rng" min="0" max="100" step="5" value="100" aria-label="Sky clearness">
    <div class="row"><span>Outdoor min / max (°C)</span><span><input type="number" id="thTmin" class="inp" style="width:62px" step="0.5" aria-label="Outdoor minimum"> <input type="number" id="thTmax" class="inp" style="width:62px" step="0.5" aria-label="Outdoor maximum"></span></div>
    <div id="thTyp" class="note"></div>
    <label class="opt"><input type="checkbox" id="thHeat" checked>Heating on (heat pump and floor loops)</label>
    <label class="opt"><input type="checkbox" id="thBypass" checked>Summer bypass on the ventilation unit</label>
    <label class="opt"><input type="checkbox" id="thNightVent" checked>Night ventilation</label>
    <label class="opt"><input type="checkbox" id="thBlinds" checked>External blinds on sunlit windows</label>
    <div class="presets" style="margin-top:8px"><button class="btn primary" type="button" id="thRun">Run day</button><button class="btn" type="button" id="thPlay">Play 24 h</button><button class="btn" type="button" id="thCsv">Export CSV</button></div>
    <label class="opt"><input type="checkbox" id="thTemps">Room temperatures in 3D</label>
    <label class="opt"><input type="checkbox" id="thSun">Sun on the windows</label>
    <div id="thSummary"></div>`;

  // ---------- results: the Thermal tab ----------
  const pan = $('thermalPanel');
  let tab = 'rooms', selWin = null, timer = null, timeNow = () => sun.get().time;
  const show = (open = true) => { pan.hidden = false; const e = $('thEmpty'); if (e) e.hidden = true; if (open && app.shell) app.shell.openTab('thermal'); };
  pan.querySelectorAll('.seg .segb').forEach(b => b.onclick = () => { tab = b.dataset.t; pan.querySelectorAll('.seg .segb').forEach(x => x.classList.toggle('on', x === b)); $('thRooms').hidden = tab !== 'rooms'; $('thWins').hidden = tab !== 'wins'; draw(); });
  if (app.shell) app.shell.onTab('thermal', () => { draw(); });
  app.on('theme', () => draw()); app.on('shellresize', () => draw());

  // ---------- inputs wiring ----------
  const I = th.inputs;
  const syncTyp = () => {
    const d = sun.get().date, m = typicalFor(d);
    $('thTyp').textContent = `Typical day in ${MONTHS[+d.slice(5, 7) - 1]} for Karlsruhe: ${m[0]} to ${m[1]} °C. A typical-day approximation, not weather data.`;
    if (!I.minmaxEdited) { $('thTmin').value = m[0]; $('thTmax').value = m[1]; I.tmin = null; I.tmax = null; }
  };
  const syncDate = () => { $('thDate').value = sun.get().date; syncTyp(); };
  $('thDate').onchange = e => { if (e.target.value) { I.minmaxEdited = false; sun.set({ date: e.target.value }); } };
  let lastDate = sun.get().date;
  app.on('time', () => { const d = sun.get().date; if (d !== lastDate) { lastDate = d; I.minmaxEdited = false; syncDate(); rerun(); } if (sunView.on) updateSun(); if (tempView.on) updateTemps(); drawNow(); });
  $('thClear').oninput = e => { I.clearness = +e.target.value / 100; $('thClearV').textContent = e.target.value + ' %'; rerun(); };
  const tIn = () => { I.minmaxEdited = true; I.tmin = +$('thTmin').value; I.tmax = +$('thTmax').value; if (I.tmax < I.tmin) { I.tmax = I.tmin; } rerun(); };
  $('thTmin').onchange = tIn; $('thTmax').onchange = tIn;
  $('thHeat').onchange = e => { I.heating = e.target.checked; rerun(); };
  $('thBlinds').onchange = e => { I.blinds = e.target.checked; rerun(); };
  $('thBypass').onchange = e => { I.summerBypass = e.target.checked; rerun(); };
  $('thNightVent').onchange = e => { I.nightVent = e.target.checked; rerun(); };
  $('thRun').onclick = () => { runAndShow(); };
  $('thCsv').onclick = () => { if (!th.last()) th.run(); const blob = new Blob([th.csv()], { type: 'text/csv' }), a = document.createElement('a'); a.href = URL.createObjectURL(blob); a.download = `fzk-haus-thermal-${th.last().date}.csv`; a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 2000); };
  let playOff = null;
  const stopPlay = () => { if (playOff) { playOff(); playOff = null; $('thPlay').textContent = 'Play 24 h'; } };
  $('thPlay').onclick = () => {
    if (playOff) { stopPlay(); return; }
    sun.playDay(false); if (!th.last()) runAndShow(); else show();
    let t = sun.get().time >= 23.9 ? 0 : sun.get().time; $('thPlay').textContent = 'Stop';
    playOff = app.addTicker(dt => { t += dt * 24 / 22; if (t >= 24) { t = 24; sun.set({ time: 24 }); stopPlay(); return false; } sun.set({ time: t }); });
  };

  // ---------- running ----------
  function runAndShow(opts) { const r = th.run(opts); show(); if (!tempView.on) { $('thTemps').checked = true; setTempView(true); } render(r); return r; }
  function rerun() { if (pan.hidden && !tempView.on) return; clearTimeout(timer); timer = setTimeout(() => { if (!pan.hidden || tempView.on) { const r = th.run(); render(r); } }, 120); }
  app.on('site', () => { th.invalidate(); syncTyp(); rerun(); if (sunView.on) updateSun(); });
  const IDLE = new Set(['switch', 'light-ceiling', 'light-spot', 'light-wall', 'light-outdoor', 'smoke-detector', 'cooker-hood', 'dishwasher', 'washing-machine']);   // these do not change the model
  app.on('state', e => { if (e && ((svc.has(e.id) && !IDLE.has(svc.thingFor(e.id).d.type)) || ['door', 'window'].includes(e.kind))) rerun(); });

  // ---------- results ----------
  const f1 = v => (v == null || isNaN(v) ? '—' : (+v).toFixed(1));
  function render(r) {
    if (!r) return;
    $('thTitle').textContent = `Thermal day · ${r.date} · sky ${Math.round(r.clearness * 100)} % · outdoor ${f1(r.tmin)} to ${f1(r.tmax)} °C${r.heating ? '' : ' · heating off'}`; if (app.shell) app.shell.setBadge('thermal', '•', true);
    const peak = r.rows.reduce((a, b) => (b.maxC > a.maxC ? b : a)), cold = r.rows.reduce((a, b) => (b.minC < a.minC ? b : a));
    $('thSummary').innerHTML = `Warmest: <b>${esc(peak.room)}</b> ${f1(peak.maxC)} °C at ${peak.peakTime}<br>Coolest: <b>${esc(cold.room)}</b> ${f1(cold.minC)} °C<br>Solar gain through windows <b>${r.totals.solarKWh.toFixed(1)} kWh</b> · heating <b>${r.totals.heatingKWh.toFixed(1)} kWh</b><br><span class="muted">${r.ms} ms${r.converged ? '' : ' · not fully settled'}</span>`;
    const rows = r.rows;
    $('thTable').innerHTML = `<tr><th>Room</th><th>Min °C</th><th>Max °C</th><th>Peak</th><th>Solar kWh</th><th>Heat kWh</th><th>Sun</th></tr>` + rows.map(x => `<tr data-g="${esc(x.guid)}"${x.room === peak.room ? ' class="peak"' : ''}><td><i style="display:inline-block;width:8px;height:8px;border-radius:50%;background:${COLORS[rows.indexOf(x) % 7]};margin-right:6px"></i>${esc(x.room)}</td><td>${f1(x.minC)}</td><td>${f1(x.maxC)}</td><td>${x.peakTime}</td><td>${x.solarKWh.toFixed(1)}</td><td>${x.heatingKWh.toFixed(1)}</td><td>${x.sun}${x.morningShare != null ? ` ${Math.round(x.morningShare * 100)}/${100 - Math.round(x.morningShare * 100)}` : ''}</td></tr>`).join('');
    $('thTable').querySelectorAll('tr[data-g]').forEach(tr => tr.onclick = () => app.select(tr.dataset.g));
    $('thWTable').innerHTML = `<tr><th>Window</th><th>Room</th><th>Azimuth</th><th>m²</th><th>Sun h</th><th>kWh</th></tr>` + r.windowRows.map(w => `<tr data-w="${esc(w.id)}"${w.id === selWin ? ' class="peak"' : ''}><td>${esc(w.label)}</td><td>${esc(w.room)}</td><td>${Math.round(w.azimuthDeg)}°</td><td>${w.areaM2}</td><td>${w.sunHours.toFixed(1)}</td><td>${w.directGainKWh.toFixed(1)}</td></tr>`).join('');
    $('thWTable').querySelectorAll('tr[data-w]').forEach(tr => tr.onclick = () => { selWin = tr.dataset.w; render(th.last()); });
    $('thNote').innerHTML = `Sun column: share of the solar gain before and after solar noon (morning/afternoon). The house is turned ${Math.round(sun.site.northAzimuthDeg)}° from true north, so its walls face ${compassNames()}. Outdoor temperature is a typical-day cosine, the sky is clear scaled by the slider, ground ${r.groundC} °C. ${r.daysIterated} repeated days to settle.`;
    if (!selWin) selWin = r.windowRows.slice().sort((a, b) => b.directGainKWh - a.directGainKWh)[0]?.id;
    draw(); if (tempView.on) updateTemps();
  }
  const compassNames = () => { const a = sun.site.northAzimuthDeg, nm = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW']; return [0, 90, 180, 270].map(d => { const az = (((d - a) % 360) + 360) % 360; return nm[Math.round(az / 45) % 8]; }).join(', ') + ' (model +Y, +X, −Y, −X)'; };

  // ---------- charts ----------
  function setup(cv) { const dpr = Math.min(devicePixelRatio || 1, 2), w = cv.clientWidth || 400, h = cv.clientHeight || 220; if (cv.width !== w * dpr || cv.height !== h * dpr) { cv.width = w * dpr; cv.height = h * dpr; } const g = cv.getContext('2d'); g.setTransform(dpr, 0, 0, dpr, 0, 0); g.clearRect(0, 0, w, h); return { g, w, h }; }
  function axes(g, w, h, ym, yM, yfmt, tickStep) {
    const L = 40, R = 10, Tp = 8, B = 22, x = hr => L + (w - L - R) * hr / 24, y = v => h - B - (h - B - Tp) * (v - ym) / (yM - ym);
    g.strokeStyle = css('--line'); g.fillStyle = css('--dim'); g.font = '11px system-ui,sans-serif'; g.lineWidth = 1; g.textAlign = 'center';
    for (let hr = 0; hr <= 24; hr += 3) { g.beginPath(); g.moveTo(x(hr), Tp); g.lineTo(x(hr), h - B); g.stroke(); g.fillText(String(hr).padStart(2, '0') + ':00', x(hr), h - 6); }
    g.textAlign = 'right';
    for (let v = Math.ceil(ym / tickStep) * tickStep; v <= yM + 1e-6; v += tickStep) { g.beginPath(); g.moveTo(L, y(v)); g.lineTo(w - R, y(v)); g.stroke(); g.fillText(yfmt(v), L - 5, y(v) + 4); }
    return { x, y, L, R, Tp, B };
  }
  function nowLine(g, ax, h) { const t = timeNow(); g.strokeStyle = css('--c-sel'); g.setLineDash([4, 3]); g.beginPath(); g.moveTo(ax.x(t), ax.Tp); g.lineTo(ax.x(t), h - ax.B); g.stroke(); g.setLineDash([]); }
  const clickTime = (cv, ax) => cv.onclick = e => { const r = cv.getBoundingClientRect(), hr = (e.clientX - r.left - ax.L) / (r.width - ax.L - ax.R) * 24; if (hr >= 0 && hr <= 24) sun.set({ time: Math.min(24, hr) }); };
  function drawRooms(r) {
    const cv = $('thChart'); if (!cv.clientWidth) return; const { g, w, h } = setup(cv);
    let ym = Math.min(...r.tOut, ...r.temps.map(t => Math.min(...t))), yM = Math.max(...r.tOut, ...r.temps.map(t => Math.max(...t)));
    ym = Math.floor(ym / 2) * 2 - 1; yM = Math.ceil(yM / 2) * 2 + 1; const step = yM - ym > 24 ? 5 : 2;
    const ax = axes(g, w, h, ym, yM, v => v.toFixed(0), step);
    const line = (arr, col, dash, wd) => { g.strokeStyle = col; g.lineWidth = wd; g.setLineDash(dash || []); g.beginPath(); arr.forEach((v, s) => { const px = ax.x(s / 6), py = ax.y(v); s ? g.lineTo(px, py) : g.moveTo(px, py); }); g.stroke(); g.setLineDash([]); };
    r.temps.forEach((t, i) => line(t, COLORS[i % 7], null, 1.8)); line(r.tOut, css('--dim'), [5, 4], 1.4); nowLine(g, ax, h); clickTime(cv, ax);
    g.fillStyle = css('--dim'); g.textAlign = 'left'; g.fillText('°C', 4, 12);
    const nowS = Math.round(timeNow() * 6);
    $('thLegend').innerHTML = r.temps.map((t, i) => `<span><i style="background:${COLORS[i % 7]}"></i>${esc(r.rows[i].room)} ${f1(t[Math.min(nowS, 144)])}</span>`).join('') + `<span><i style="background:var(--dim)"></i>Outdoor ${f1(r.tOut[Math.min(nowS, 144)])}</span><span>at ${th.hh(timeNow())}</span>`;
  }
  function drawWin(r) {
    const cv = $('thWChart'); if (!cv.clientWidth) return; const { g, w, h } = setup(cv);
    const wr = r.windowRows.find(x => x.id === selWin); if (!wr) return;
    const sh = r.shade.get(selWin), tr = r.windows.get(selWin).trans, mx = Math.max(100, Math.ceil(Math.max(...tr) / 100) * 100);
    const ax = axes(g, w, h, 0, mx, v => v.toFixed(0), mx > 600 ? 200 : mx > 300 ? 100 : 50);
    g.fillStyle = 'rgba(255,210,46,.22)'; g.beginPath(); g.moveTo(ax.x(0), ax.y(0)); for (let s = 0; s < 144; s++) g.lineTo(ax.x((s + 0.5) / 6), ax.y(sh[s] * mx)); g.lineTo(ax.x(24), ax.y(0)); g.fill();
    g.strokeStyle = '#ffb454'; g.lineWidth = 1.8; g.beginPath(); for (let s = 0; s < 144; s++) { const px = ax.x((s + 0.5) / 6), py = ax.y(tr[s]); s ? g.lineTo(px, py) : g.moveTo(px, py); } g.stroke(); nowLine(g, ax, h); clickTime(cv, ax);
    g.fillStyle = css('--dim'); g.textAlign = 'left'; g.fillText('W through the glass', 4, 12);
    $('thWLegend').innerHTML = `<span><i style="background:#ffb454"></i>${esc(wr.label)} · ${esc(wr.room)} · ${Math.round(wr.azimuthDeg)}° · ${wr.areaM2} m²</span><span><i style="background:rgba(255,210,46,.5)"></i>sunlit fraction (full height = 100 %)</span><span>${wr.sunHours.toFixed(1)} h of sun · ${wr.directGainKWh.toFixed(1)} kWh</span>`;
  }
  function draw() { const r = th.last(); if (!r || pan.hidden) return; if (tab === 'rooms') drawRooms(r); else drawWin(r); }
  let raf = 0; function drawNow() { if (pan.hidden || raf) return; raf = requestAnimationFrame(() => { raf = 0; draw(); }); }
  addEventListener('resize', () => draw());

  // ---------- room temperatures on the floors ----------
  const view = new THREE.Group(); view.name = 'thermalView'; app.house.add(view);
  const tempView = { on: false, planes: [], labels: [] };
  const tcol = t => { const k = Math.max(-1, Math.min(1, (t - 24) / 12)), c = new THREE.Color(); return k < 0 ? c.setRGB(0.95 + k * 0.55, 0.95 + k * 0.3, 0.95) : c.setRGB(0.95, 0.95 - k * 0.55, 0.95 - k * 0.65); };   // blue at 12, white at 24, red at 36 degrees
  function buildTempView() {
    for (const r of svc.data.rooms) {
      const sh = new THREE.Shape(r.polygon.map(p => new THREE.Vector2(p[0], p[1])));
      const m = new THREE.Mesh(new THREE.ShapeGeometry(sh), new THREE.MeshBasicMaterial({ color: 0xffffff, transparent: true, opacity: 0.7, depthWrite: false, side: THREE.DoubleSide, polygonOffset: true, polygonOffsetFactor: -2, polygonOffsetUnits: -2, toneMapped: false }));
      m.position.z = r.z0 + 0.03; m.renderOrder = 3; m.userData.room = r.guid; view.add(m); tempView.planes.push(m);
      const c = document.createElement('canvas'); c.width = 256; c.height = 96; const t = new THREE.CanvasTexture(c); t.colorSpace = THREE.SRGBColorSpace;
      const s = new THREE.Sprite(new THREE.SpriteMaterial({ map: t, transparent: true, depthTest: false, depthWrite: false, fog: false })); s.scale.set(1.6, 0.6, 1); s.renderOrder = 9;
      const cx = r.polygon.reduce((a, p) => a + p[0], 0) / r.polygon.length, cy = r.polygon.reduce((a, p) => a + p[1], 0) / r.polygon.length;
      s.position.set(cx, cy, r.z0 + 1.4); s.userData = { canvas: c, room: r.guid, label: r.label }; view.add(s); tempView.labels.push(s);
    }
    view.visible = false;
  }
  buildTempView();
  function updateTemps() {
    const r = th.last(); if (!r) return; const T = th.tempsAt(timeNow());
    tempView.planes.forEach(m => { const z = th.zones.find(x => x.guid === m.userData.room); m.material.color.copy(tcol(T[z.i])); });
    tempView.labels.forEach(s => { const z = th.zones.find(x => x.guid === s.userData.room), c = s.userData.canvas, g = c.getContext('2d'); g.clearRect(0, 0, 256, 96); g.fillStyle = 'rgba(12,17,28,.82)'; g.beginPath(); g.roundRect(8, 8, 240, 80, 14); g.fill(); g.textAlign = 'center'; g.fillStyle = '#c9d4e6'; g.font = '600 24px system-ui,sans-serif'; g.fillText(s.userData.label, 128, 36); g.fillStyle = '#fff'; g.font = '700 32px system-ui,sans-serif'; g.fillText(T[z.i].toFixed(1) + ' °C', 128, 72); s.material.map.needsUpdate = true; });
    app.requestRender();
  }
  function setTempView(on) {
    tempView.on = on; view.visible = on || sunView.on; tempView.planes.forEach(m => { m.visible = on; }); tempView.labels.forEach(s => { s.visible = on; });
    if (on) { if (!svc.isXray()) svc.setXray(true); if (!th.last()) th.run(); updateTemps(); render(th.last()); } app.requestRender(); app.emit('thermalview', { temps: on, sun: sunView.on });
  }
  $('thTemps').onchange = e => setTempView(e.target.checked);

  // ---------- the sun on the windows ----------
  const sunView = { on: false, quads: [] };
  for (const w of th.windows) {
    const off = w.n.clone().multiplyScalar(0.32), g = new THREE.BufferGeometry();
    const p = w.poly.map(v => v.clone().add(off)); g.setFromPoints([p[0], p[1], p[2], p[0], p[2], p[3]]);
    const m = new THREE.Mesh(g, new THREE.MeshBasicMaterial({ color: 0x274a8c, transparent: true, opacity: 0.6, side: THREE.DoubleSide, depthWrite: false, toneMapped: false, polygonOffset: true, polygonOffsetFactor: -3, polygonOffsetUnits: -3 }));
    m.renderOrder = 4; m.visible = false; m.userData.win = w.id; view.add(m); sunView.quads.push(m);
  }
  function updateSun() {
    const d = sun.get().direction, el = sun.get().elevation;
    sunView.fractions = {};
    for (const m of sunView.quads) { const w = th.windows.find(x => x.id === m.userData.win); const f = el > 0.2 ? th.sunlit(w, d) : 0; sunView.fractions[w.id] = f; m.material.color.set(f > 0 ? new THREE.Color(0x274a8c).lerp(new THREE.Color(0xffd22e), f) : 0x274a8c); m.material.opacity = 0.35 + 0.5 * f; }
    app.requestRender();
  }
  function setSunView(on) { sunView.on = on; sunView.quads.forEach(m => { m.visible = on; }); view.visible = on || tempView.on; if (on) updateSun(); app.requestRender(); app.emit('thermalview', { temps: tempView.on, sun: on }); }
  $('thSun').onchange = e => setSunView(e.target.checked);

  syncDate();
  app.thermalUi = { show, render, runAndShow, setTempView, setSunView, sunFractions: () => sunView.fractions || null, isOpen: () => !pan.hidden, get view() { return { temps: tempView.on, sun: sunView.on }; } };
}
