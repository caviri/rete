// A small, honest thermal model of the house for one day: clear-sky sun on every exterior
// surface, ray-cast shading of the windows, one temperature node per room, a thermostat
// on the underfloor heating. The inputs are in the systems file (`thermal`, `site`); the
// states of windows, doors, the ventilation unit and the heat pump come from the page.
//
//   C_i dT_i/dt = sum_j H_ij (T_j - T_i) + (H_out,i + H_vent,i)(T_out - T_i) + H_gnd,i (T_gnd - T_i)
//                 + Q_internal,i + Q_solar,i + Q_heat,i
//
// Backward Euler, 10 minute steps, the day repeated until it closes on itself.
import * as THREE from 'three';
import { app } from './app.js';

const H_E = 25;               // W/m2K, outside surface coefficient (for the sol-air temperature)
const RHO_CP = 0.34;          // Wh/m3K, air
const OPEN_COUPLING = 0.06;   // m/s, exchange velocity through an open-plan boundary
// What a real house does in summer (each can be switched off, they are on by default):
//  - summer bypass: the ventilation unit stops recovering heat while a room is above its set point and the outdoor air is cooler
//  - night ventilation: extra air changes at night while the room is above 23 C and the outdoor air is cooler
//  - external blinds: a sunlit window lets 20 % of the gain through while its room is above 24 C (fully closed from 25 C)
const BLIND_ABOVE = 24, BLIND_F = 0.2;
const NIGHT_ABOVE = 23, NIGHT_ACH = 2.0;
const SUMMER_OUT_MIN = 10;   // bypass and night air only while the outdoor air is at least this warm: in winter nothing changes
const P_BAND = 1.5;           // K, proportional band of the room thermostat
const STEPS = 144, DT = 600;  // 10 minutes
const FLOW = { windowOpen: 180, windowTilted: 40, doorOpen: 300, intDoorOpen: 200 };   // m3/h
const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const rad = d => d * Math.PI / 180;

// Typical mean daily minimum and maximum per month for Karlsruhe, degrees C. Approximate
// long-term values written down by hand: not weather data.
export const TYPICAL = [[-0.8, 5.0], [-0.5, 7.2], [2.6, 12.0], [5.8, 17.2], [10.0, 21.5], [13.3, 25.0], [15.2, 27.2], [14.8, 27.0], [11.2, 22.0], [7.2, 15.8], [3.2, 9.2], [0.5, 5.5]];
const ASHRAE_C = [0.058, 0.06, 0.071, 0.097, 0.121, 0.134, 0.136, 0.122, 0.092, 0.073, 0.063, 0.057];   // diffuse / direct normal, by month
const TG_MEAN = 10.5;         // mean ground temperature under the house, C
export const typicalFor = date => TYPICAL[clamp((+String(date).slice(5, 7) || 6) - 1, 0, 11)];

// ---------- clear sky ----------
const dayOfYear = date => { const [y, m, d] = String(date).split('-').map(Number); return Math.round((Date.UTC(y, m - 1, d) - Date.UTC(y, 0, 0)) / 86400000); };
// direct normal and diffuse horizontal irradiance, W/m2, for a clear sky. elevation in degrees.
export function clearSky(date, elevation) {
  if (elevation <= 0.2) return { dni: 0, dhi: 0, ghi: 0, airMass: null };
  const h = elevation, am = 1 / (Math.sin(rad(h)) + 0.50572 * Math.pow(h + 6.07995, -1.6364));   // Kasten and Young
  const i0 = 1353 * (1 + 0.033 * Math.cos(rad(360 * (dayOfYear(date) - 3) / 365)));
  const dni = i0 * Math.pow(0.7, Math.pow(am, 0.678));                                            // Meinel
  const m = +String(date).slice(5, 7) - 1, dd = +String(date).slice(8, 10);
  const c0 = ASHRAE_C[m], c1 = ASHRAE_C[(m + (dd > 15 ? 1 : 11)) % 12], w = Math.abs(dd - 15) / 30;
  const dhi = (c0 * (1 - w) + c1 * w) * dni;
  return { dni, dhi, ghi: dni * Math.sin(rad(h)) + dhi, airMass: am };
}
// scale by the clearness slider: 1 clear, 0 overcast (no beam, a diffuse sky at 20 % of the clear global)
export function scaleSky(c, k) {
  if (!c.ghi) return c;
  return { dni: c.dni * k * k, dhi: k * c.dhi + (1 - k) * 0.2 * c.ghi, ghi: 0, airMass: c.airMass };
}

// ---------- the model ----------
export function initThermal(svc) {
  const data = svc.data, T = data.thermal;
  const zones = T.zones.map((z, i) => ({ i, guid: z.room, label: data.rooms.find(r => r.guid === z.room).label, V: z.volumeM3, area: z.floorAreaM2, ach: z.airChangesPerHour, achEff: z.effectiveAirChangesPerHour ?? z.airChangesPerHour, qint: z.internalGainsW, set: z.setpointC, C: z.thermalCapacityJK, load: z.heatLoadW }));
  const zi = new Map(zones.map(z => [z.guid, z.i])), nz = zones.length;
  const cons = new Map(T.constructions.map(c => [c.id, c])), glz = new Map(T.glazings.map(g => [g.id, g]));
  const surfaces = T.surfaces.map(s => ({ ...s, i: zi.get(s.room), j: s.otherRoom ? zi.get(s.otherRoom) : null, n: new THREE.Vector3(...s.normal).normalize(),
    U: s.uValue ?? (s.construction ? cons.get(s.construction)?.uValue : null) ?? 0, alpha: s.construction ? (cons.get(s.construction)?.solarAbsorptance ?? 0.6) : 0, g: s.glazing ? glz.get(s.glazing) : null }));
  const windows = surfaces.filter(s => s.kind === 'window' && s.boundary === 'outside').map(s => {
    const poly = s.polygon.map(p => new THREE.Vector3(...p));
    const label = (app.nodeByGuid.get(s.element) || {}).l || s.id;
    return { ...s, poly, label, roomLabel: zones[s.i].label };
  });
  const roofish = surfaces.filter(s => s.boundary === 'outside' && s.kind !== 'window');
  const winById = new Map(windows.map(w => [w.id, w])), winByEl = new Map(windows.map(w => [w.element, w]));

  const inputs = { clearness: 1, tmin: null, tmax: null, heating: true, blinds: true, summerBypass: true, nightVent: true, minmaxEdited: false };
  let last = null, shadeCache = null, targets = null;

  // ---- shading by rays against the house ----
  const ray = new THREE.Raycaster();
  function getTargets() {
    if (targets) return withTrees(targets); targets = [];
    app.nodeByGuid.forEach((n, g) => { if (n.c === 'IfcWallStandardCase' || n.c === 'IfcSlab') { const o = app.meshByGuid.get(g); if (o) targets.push(o); } });
    app.house.updateMatrixWorld(true); return withTrees(targets);
  }
  // the trees of the scenery cast shade only when "trees shade the house" is on (default off)
  const withTrees = base => { const x = app.scenery ? app.scenery.shadeTargets() : []; return x.length ? base.concat(x) : base; };
  const grid = [0.17, 0.5, 0.83];
  function samples(w) {   // 3 x 3 points on the quad, in the glass plane (the window sits in the wall, its polygon is on the inner face)
    const [a, b, , d] = w.poly, out = [];
    let shift = 0; const o = app.meshByGuid.get(w.element);
    if (o) { o.updateWorldMatrix(true, true); shift = clamp(new THREE.Box3().setFromObject(o).getCenter(new THREE.Vector3()).sub(a).dot(w.n), 0, 0.4); }
    const off = w.n.clone().multiplyScalar(shift + 0.02);
    for (const u of grid) for (const v of grid) out.push(a.clone().lerp(b, u).add(d.clone().sub(a).multiplyScalar(v)).add(off));
    return out;
  }
  const winSamples = new Map(windows.map(w => [w.id, samples(w)]));
  // fraction of the window that sees the sun (0..1)
  const sunlit = (w, dir) => app.section ? app.section.noFilter(() => sunlitRaw(w, dir)) : sunlitRaw(w, dir);   // a section cut does not change what shades a window
  function sunlitRaw(w, dir) {
    if (w.n.dot(dir) <= 0.01) return 0;
    const tg = getTargets(); ray.far = 200; let lit = 0, pts = winSamples.get(w.id);
    for (const p of pts) { ray.set(p, dir); if (!ray.intersectObjects(tg, true).length) lit++; }
    return lit / pts.length;
  }
  const northKey = () => { const s = app.sun.site; return [s.latitude, s.longitude, s.northAzimuthDeg].map(v => (+v).toFixed(3)).join('|'); };
  function shading(date, sunSteps) {   // cached per date and site; windows x steps
    const key = date + '|' + northKey() + '|' + (app.scenery && app.scenery.get().treesShade ? 'trees' : '');
    if (shadeCache && shadeCache.key === key) return shadeCache.v;
    const v = new Map(windows.map(w => [w.id, new Float32Array(STEPS)]));
    sunSteps.forEach((s, k) => { if (s.elevation <= 0.2) return; for (const w of windows) v.get(w.id)[k] = sunlit(w, s.direction); });
    shadeCache = { key, v }; return v;
  }

  // ---- states from the page ----
  const stateOf = id => { try { return app.getState(id); } catch { return undefined; } };
  const live = id => { try { return svc.isLive(id); } catch { return true; } };
  // the set point and a pinned room temperature are in the state-variable store (statevars.js): a person can change them
  const storeSet = i => { const SV = app.statevars; if (!SV) return null; try { const v = SV.getValue('room-setpoint', zones[i].guid); return typeof v === 'number' ? v : null; } catch { return null; } };
  const pinnedT = () => zones.map(z => { const SV = app.statevars, p = SV && SV.pinned().find(x => x.variable === 'room-air-temperature' && x.thing === z.guid); return p ? p.value : null; });
  function heatSpec() {   // per room: what the thermostat can ask for
    const spec = zones.map(() => ({ qmax: 0, set: null, avail: false, loops: [] }));
    for (const g of data.groups) {
      if (g.kind !== 'zone' || g.system !== 'heating' || !g.props || !g.props.thermostat) continue;
      const th = data.nodes.find(n => n.id === g.props.thermostat); if (!th || th.room == null) continue;
      const i = zi.get(th.room); if (i == null) continue;
      const parts = g.members.filter(m => { const n = svc.nodeById.get(m); return n && (n.type === 'floor-loop' || n.type === 'towel-radiator'); });
      const on = parts.filter(m => live(m));
      spec[i] = { qmax: on.reduce((s, m) => s + (svc.nodeById.get(m).props.powerW || 0), 0), set: storeSet(i) ?? g.props.setpointC ?? zones[i].set, avail: live(th.id) && on.length > 0, loops: parts, thermostat: th.id };
    }
    return spec;
  }
  function conductances() {
    const Hout = new Float64Array(nz), Hg = new Float64Array(nz), Hv = new Float64Array(nz), Hp = Array.from({ length: nz }, () => new Float64Array(nz));
    const pair = new Map();
    const hrv = svc.nodeById.has('hrv-unit') ? live('hrv-unit') && stateOf('hrv-unit') !== 'off' : true;
    for (const s of surfaces) {
      const i = s.i; if (i == null) continue; const A = s.areaM2;
      if (s.boundary === 'outside') {
        let st = s.element ? stateOf(s.element) : null;
        Hout[i] += s.U * A;
        if (s.kind === 'window') { if (st === 'open') Hv[i] += RHO_CP * FLOW.windowOpen; else if (st === 'tilted') Hv[i] += RHO_CP * FLOW.windowTilted; }
        else if (s.kind === 'door' && st === 'open') Hv[i] += RHO_CP * FLOW.doorOpen;
      } else if (s.boundary === 'ground') Hg[i] += s.U * A;
      else if (s.boundary === 'unheated' || (s.boundary === 'room' && s.j == null)) Hout[i] += 0.5 * s.U * A;
      else if (s.j != null) {
        let H = s.kind === 'open' ? RHO_CP * OPEN_COUPLING * 3600 * A : s.U * A;
        if (s.kind === 'door' && s.element && stateOf(s.element) === 'open') H += RHO_CP * FLOW.intDoorOpen;
        const key = Math.min(i, s.j) + '-' + Math.max(i, s.j), p = pair.get(key) || { a: [], b: [] };
        (i < s.j ? p.a : p.b).push(H); pair.set(key, p);
      }
    }
    pair.forEach((p, key) => { const [a, b] = key.split('-').map(Number); const sa = p.a.reduce((x, y) => x + y, 0), sb = p.b.reduce((x, y) => x + y, 0); const H = p.a.length && p.b.length ? (sa + sb) / 2 : sa + sb; Hp[a][b] = H; Hp[b][a] = H; });
    // Hv: the open windows and doors. The ventilation unit is separate, because the bypass and the night air change with the temperature.
    const Hhr = new Float64Array(nz), Hraw = new Float64Array(nz), Hnv = new Float64Array(nz);
    zones.forEach(z => { Hhr[z.i] = RHO_CP * z.V * (hrv ? z.achEff : z.ach); Hraw[z.i] = RHO_CP * z.V * z.ach; Hnv[z.i] = RHO_CP * z.V * NIGHT_ACH; });
    return { Hout, Hg, Hv, Hp, hrv, Hhr, Hraw, Hnv };
  }

  // ---- the day ----
  function solve(A, b, n) {   // Gaussian elimination, A is n x n (row-major Float64Array), modified
    for (let c = 0; c < n; c++) {
      let p = c; for (let r = c + 1; r < n; r++) if (Math.abs(A[r * n + c]) > Math.abs(A[p * n + c])) p = r;
      if (p !== c) { for (let k = c; k < n; k++) { const t = A[c * n + k]; A[c * n + k] = A[p * n + k]; A[p * n + k] = t; } const t = b[c]; b[c] = b[p]; b[p] = t; }
      const d = A[c * n + c];
      for (let r = c + 1; r < n; r++) { const f = A[r * n + c] / d; if (!f) continue; for (let k = c; k < n; k++) A[r * n + k] -= f * A[c * n + k]; b[r] -= f * b[c]; }
    }
    const x = new Float64Array(n);
    for (let r = n - 1; r >= 0; r--) { let s = b[r]; for (let k = r + 1; k < n; k++) s -= A[r * n + k] * x[k]; x[r] = s / A[r * n + r]; }
    return x;
  }
  function run(o = {}) {
    const t0 = performance.now();
    const date = o.date || app.sun.get().date;
    const k = clamp(o.clearness ?? inputs.clearness, 0, 1);
    const typ = typicalFor(date);
    const tmin = o.tmin ?? inputs.tmin ?? typ[0], tmax = o.tmax ?? inputs.tmax ?? typ[1];
    const heating = o.heating ?? inputs.heating, blinds = o.blinds ?? inputs.blinds, bypassOn = o.summerBypass ?? inputs.summerBypass, nightOn = o.nightVent ?? inputs.nightVent;
    // sun and sky at the middle of every step
    const sunSteps = [], sky = [];
    for (let s = 0; s < STEPS; s++) { const p = app.sun.position({ date, time: (s + 0.5) / 6 }); sunSteps.push(p); sky.push(scaleSky(clearSky(date, p.elevation), k)); sky[s].ghi = sky[s].dni * Math.sin(rad(Math.max(p.elevation, 0))) + sky[s].dhi; }
    const shade = shading(date, sunSteps);
    // sun on every exterior surface
    const winW = new Map(windows.map(w => [w.id, { irr: new Float32Array(STEPS), trans: new Float32Array(STEPS) }]));
    const solarF = zones.map(() => new Float64Array(STEPS)), solarB = zones.map(() => new Float64Array(STEPS)), solar = zones.map(() => new Float64Array(STEPS)), opaque = zones.map(() => new Float64Array(STEPS));
    for (let s = 0; s < STEPS; s++) {
      const e = sunSteps[s].elevation; if (e <= 0.2) { for (const w of roofish) { const cf = Math.max(0, w.n.z); if (!w.alpha) continue; opaque[w.i][s] += w.U * w.areaM2 * (-0.9 * 63 * cf) / H_E; } continue; }
      const d = sunSteps[s].direction, c = sky[s];
      const irr = (n, sh) => c.dni * Math.max(0, n.dot(d)) * sh + c.dhi * (1 + n.z) / 2 + c.ghi * 0.2 * (1 - n.z) / 2;
      for (const w of windows) { const sh = shade.get(w.id)[s], I = irr(w.n, sh), g = w.g, tr = I * w.areaM2 * (1 - g.frameFraction) * g.gValue; winW.get(w.id).irr[s] = I; winW.get(w.id).trans[s] = tr; solarF[w.i][s] += tr; solarB[w.i][s] += tr * (sh > 0.05 ? BLIND_F : 1); }
      for (const w of roofish) { if (!w.alpha) continue; const I = irr(w.n, 1), lw = 0.9 * 63 * Math.max(0, w.n.z); opaque[w.i][s] += w.U * w.areaM2 * (w.alpha * I - lw) / H_E; }
    }
    // the room model
    const { Hout, Hg, Hv, Hp, hrv, Hhr, Hraw, Hnv } = conductances(), spec = heatSpec();
    const cnt = zones.map(() => ({ byp: 0, nv: 0, bl: 0 }));
    const rowSum = zones.map(z => { let s = 0; for (let j = 0; j < nz; j++) s += Hp[z.i][j]; return s; });
    const tOut = new Float64Array(STEPS + 1), mean = (tmin + tmax) / 2, amp = (tmax - tmin) / 2;
    for (let s = 0; s <= STEPS; s++) tOut[s] = mean + amp * Math.cos(2 * Math.PI * (s / 6 - 15) / 24);
    const tGnd = TG_MEAN + 0.3 * (mean - TG_MEAN);
    const pin = pinnedT();   // a room whose temperature a person fixed is a fixed-temperature node: it holds that value all day
    const Tn = new Float64Array(nz).map((_, i) => pin[i] ?? spec[i].set ?? zones[i].set);
    const A = new Float64Array(nz * nz), b = new Float64Array(nz);
    const temps = zones.map(() => new Float32Array(STEPS + 1)), heat = zones.map(() => new Float32Array(STEPS));
    let day = 0, resid = 0;
    for (day = 0; day < 160; day++) {
      const start = Float64Array.from(Tn);
      for (const c of cnt) { c.byp = 0; c.nv = 0; c.bl = 0; }
      for (let s = 0; s < STEPS; s++) {
        const night = sunSteps[s].elevation <= 0.2;
        for (let i = 0; i < nz; i++) {
          const sp = spec[i]; const q = heating && sp.avail ? sp.qmax * clamp((sp.set - Tn[i]) / P_BAND, 0, 1) : 0;
          if (day >= 0) heat[i][s] = q;
          // what the house does about the heat, from the room temperature at the end of the last step
          const Tp = Tn[i], byp = bypassOn && hrv && Tp > (sp.set ?? zones[i].set) && tOut[s + 1] < Tp && tOut[s + 1] >= SUMMER_OUT_MIN, nv = nightOn && night && Tp > NIGHT_ABOVE && tOut[s + 1] < Tp - 1 && tOut[s + 1] >= SUMMER_OUT_MIN, bl = blinds ? clamp(Tp - BLIND_ABOVE, 0, 1) : 0;
          if (byp) cnt[i].byp++; if (nv) cnt[i].nv++; if (bl > 0.5) cnt[i].bl++;
          const hx = Hout[i] + Hv[i] + (byp ? Hraw[i] : Hhr[i]) + (nv ? Hnv[i] : 0), sol = solarF[i][s] * (1 - bl) + solarB[i][s] * bl;
          solar[i][s] = sol;
          for (let j = 0; j < nz; j++) A[i * nz + j] = j === i ? 0 : -Hp[i][j];
          A[i * nz + i] = zones[i].C / DT + hx + Hg[i] + rowSum[i];
          b[i] = zones[i].C / DT * Tn[i] + hx * tOut[s + 1] + Hg[i] * tGnd + zones[i].qint + sol + opaque[i][s] + q;
          if (pin[i] != null) { for (let j = 0; j < nz; j++) A[i * nz + j] = 0; A[i * nz + i] = 1; b[i] = pin[i]; heat[i][s] = 0; }
        }
        const x = solve(A.slice(), b.slice(), nz); for (let i = 0; i < nz; i++) Tn[i] = x[i];
        if (day >= 0) for (let i = 0; i < nz; i++) temps[i][s + 1] = Tn[i];
      }
      resid = 0; for (let i = 0; i < nz; i++) resid = Math.max(resid, Math.abs(Tn[i] - start[i]));
      if (resid < 0.002) break;
    }
    for (let i = 0; i < nz; i++) temps[i][0] = temps[i][STEPS];
    // summary
    const hrs = new Float32Array(STEPS + 1).map((_, s) => s / 6);
    const toK = a => { let t = 0; for (const v of a) t += v; return t * DT / 3.6e6; };
    const rows = zones.map(z => {
      const tt = temps[z.i]; let mn = 1e9, mx = -1e9, tm = 0; for (let s = 0; s <= STEPS; s++) { if (tt[s] < mn) mn = tt[s]; if (tt[s] > mx) { mx = tt[s]; tm = s / 6; } }
      let am = 0, pm = 0; solar[z.i].forEach((v, s) => { if (s / 6 + 1 / 12 < solarNoon(date)) am += v; else pm += v; });
      const sun = am + pm < 1 ? '—' : am > 0.65 * (am + pm) ? 'morning' : pm > 0.65 * (am + pm) ? 'afternoon' : 'both';
      return { room: z.label, guid: z.guid, minC: +mn.toFixed(2), maxC: +mx.toFixed(2), peakAt: tm, peakTime: hh(tm), swingK: +(mx - mn).toFixed(2), meanC: +(temps[z.i].reduce((a, v) => a + v, 0) / (STEPS + 1)).toFixed(2),
        solarKWh: +toK(solar[z.i]).toFixed(2), opaqueSolarKWh: +toK(opaque[z.i]).toFixed(2), heatingKWh: +toK(heat[z.i]).toFixed(2), morningShare: am + pm > 1 ? +(am / (am + pm)).toFixed(2) : null, bypassHours: +(cnt[z.i].byp / 6).toFixed(1), nightVentHours: +(cnt[z.i].nv / 6).toFixed(1), blindHours: +(cnt[z.i].bl / 6).toFixed(1), sun, setpointC: spec[z.i].set ?? z.set, heatingAvailable: spec[z.i].avail && heating, pinned: pin[z.i] != null };
    });
    const wrows = windows.map(w => { const r = winW.get(w.id), f = shade.get(w.id); let sf = 0, n = 0; f.forEach(v => { if (v > 0) { sf += v; n++; } }); return { id: w.id, label: w.label, room: w.roomLabel, azimuthDeg: w.azimuthDeg, areaM2: w.areaM2, sunHours: +(sf / 6).toFixed(2), directGainKWh: +toK(r.trans).toFixed(2) }; });
    last = { date, clearness: k, tmin, tmax, heating, blinds, summerBypass: bypassOn, nightVent: nightOn, hrv, groundC: +tGnd.toFixed(1), hours: hrs, tOut, temps, solar, opaque, heat, windows: winW, shade, rows, windowRows: wrows, converged: resid < 0.002, daysIterated: day + 1, residualK: resid,
      totals: { solarKWh: +rows.reduce((a, r) => a + r.solarKWh, 0).toFixed(2), opaqueSolarKWh: +rows.reduce((a, r) => a + r.opaqueSolarKWh, 0).toFixed(2), heatingKWh: +rows.reduce((a, r) => a + r.heatingKWh, 0).toFixed(2) },
      ms: Math.round(performance.now() - t0) };
    app.emit('thermal', last);
    publish({ surfaces: false });
    return last;
  }
  const hh = h => { const m = Math.round(h * 60) % 1440; return String(Math.floor(m / 60)).padStart(2, '0') + ':' + String(m % 60).padStart(2, '0'); };
  const solarNoon = date => app.sun.sunTimes(date).solarNoon ?? 12;

  // a window's sun at one moment (not the whole day)
  function windowSun(time, date, clearness) {
    date = date || app.sun.get().date; const h = typeof time === 'number' ? time : app.sun.get().time;
    const p = app.sun.position({ date, time: h }), c = scaleSky(clearSky(date, p.elevation), clamp(clearness ?? inputs.clearness, 0, 1)); c.ghi = c.dni * Math.sin(rad(Math.max(p.elevation, 0))) + c.dhi;
    return { date, time: h, elevationDeg: +p.elevation.toFixed(1), azimuthDeg: +p.azimuth.toFixed(1), dni: Math.round(c.dni), dhi: Math.round(c.dhi), windows: windows.map(w => {
      const lit = p.elevation > 0.2 ? sunlit(w, p.direction) : 0, I = p.elevation > 0.2 ? c.dni * Math.max(0, w.n.dot(p.direction)) * lit + c.dhi * (1 + w.n.z) / 2 + c.ghi * 0.2 * (1 - w.n.z) / 2 : 0;
      return { id: w.id, label: w.label, room: w.roomLabel, azimuthDeg: w.azimuthDeg, areaM2: w.areaM2, sunlitFraction: +lit.toFixed(2), irradianceWm2: Math.round(I), transmittedW: Math.round(I * w.areaM2 * (1 - w.g.frameFraction) * w.g.gValue) };
    }) };
  }
  // temperature of every room at an hour of the last run
  function tempsAt(hours) { if (!last) return null; const s = clamp(hours * 6, 0, STEPS), a = Math.floor(s), b = Math.min(STEPS, a + 1), f = s - a; return zones.map(z => last.temps[z.i][a] * (1 - f) + last.temps[z.i][b] * f); }
  function csv() {
    if (!last) return '';
    const head = ['time', 'hour', 'outdoor_C', ...zones.flatMap(z => [`${z.label}_C`, `${z.label}_solar_W`, `${z.label}_heat_W`])];
    const out = [head.join(',')];
    for (let s = 0; s <= STEPS; s++) { const sm = Math.min(s, STEPS - 1); out.push([hh(s / 6), (s / 6).toFixed(3), last.tOut[s].toFixed(2), ...zones.flatMap(z => [last.temps[z.i][s].toFixed(3), last.solar[z.i][sm].toFixed(1), last.heat[z.i][sm].toFixed(1)])].join(',')); }
    return out.join('\n');
  }
  const model = () => ({
    note: 'One temperature node per room, clear-sky sun, a typical-day outdoor temperature, assumed insulation values. For comparing orientations and what-ifs, not a certified calculation.',
    zones: zones.map(z => ({ room: z.label, volumeM3: z.V, floorAreaM2: z.area, setpointC: z.set, airChangesPerHour: z.ach, withHeatRecovery: z.achEff, internalGainsW: z.qint, thermalCapacityMJperK: +(z.C / 1e6).toFixed(1), designHeatLoadW: z.load })),
    constructions: T.constructions.map(c => ({ id: c.id, label: c.label, uValue: c.uValue, arealHeatCapacityJm2K: c.arealHeatCapacity, absorptance: c.solarAbsorptance })), glazings: T.glazings, weather: T.weather, assumptions: T.assumptions,
    surfacesPerRoom: zones.map(z => { const by = {}; surfaces.filter(s => s.i === z.i).forEach(s => { const key = s.kind + '/' + s.boundary; const e = by[key] || (by[key] = { n: 0, areaM2: 0, uValue: s.U || null }); e.n++; e.areaM2 = +(e.areaM2 + s.areaM2).toFixed(2); }); return { room: z.label, surfaces: by }; }),
    windows: windows.map(w => ({ id: w.id, label: w.label, room: w.roomLabel, azimuthDeg: w.azimuthDeg, areaM2: w.areaM2, uValue: w.U, gValue: w.g.gValue, frameFraction: w.g.frameFraction })),
    modelAssumptions: { solarModel: 'Kasten-Young air mass, Meinel direct normal, ASHRAE monthly diffuse ratio, ground reflectance 0.2', windowGain: 'I x area x (1 - frame) x g x shaded fraction of the beam', openingFlows: FLOW, openCouplingMs: OPEN_COUPLING, thermostatBandK: P_BAND, groundTemperature: `${TG_MEAN} C mean, moving 0.3 of the way with the outdoor mean`, outdoor: 'cosine between min (03:00) and max (15:00); typical-day values by month, not weather data', summerBypass: 'on by default: while a room is above its set point and the outdoor air is cooler, the ventilation unit stops recovering heat (the full air change rate instead of the effective one), only while the outdoor air is at least 10 C', nightVentilation: `on by default: at night (sun below the horizon) while a room is above ${NIGHT_ABOVE} C and the outdoor air is more than 1 K cooler (and at least 10 C), ${NIGHT_ACH} extra air changes per hour`, blinds: `on by default: external blinds; a sunlit window lets ${BLIND_F * 100} % of its gain through while its room is above ${BLIND_ABOVE} C (fully closed from ${BLIND_ABOVE + 1} C)`, heating: 'underfloor loops and the towel radiator at their design output, proportional over 1.5 K below the set point, only when the heat pump, the thermostat and the loop are live', timeStep: '10 minutes, backward Euler, the day repeated until it closes' },
  });

  // ---- results into the state-variable store (statevars.js), at the time the page shows ----
  const outside = surfaces.filter(s => s.boundary === 'outside' && s.polygon && s.polygon.length >= 3);
  const surfPts = new Map();
  function surfPts_get(s) {
    if (surfPts.has(s.id)) return surfPts.get(s.id);
    const P = s.polygon.map(p => new THREE.Vector3(...p)), out = [];
    if (P.length === 4) { for (const u of grid) for (const v of grid) out.push(P[0].clone().lerp(P[1], u).add(P[3].clone().sub(P[0]).multiplyScalar(v))); }
    else { const c = P.reduce((a, p) => a.add(p), new THREE.Vector3()).multiplyScalar(1 / P.length); out.push(c.clone()); for (const p of P) out.push(p.clone().lerp(c, 0.2)); }
    surfPts.set(s.id, out); return out;
  }
  // the fraction of a wall, roof or door that sees the sun: rays to the sun from 9 points just outside the face, ignoring the element itself
  function surfaceSunlit(s, dir) {
    if (s.n.dot(dir) <= 0.01) return 0;
    return app.section.noFilter(() => {
      const tg = getTargets(), pts = surfPts_get(s); ray.far = 200; let lit = 0;
      for (const p of pts) {
        ray.set(p.clone().addScaledVector(s.n, 0.03), dir);
        const hits = ray.intersectObjects(tg, true); let blocked = false;
        for (const h of hits) { let o = h.object, g = null; while (o) { if (o.userData.guid) { g = o.userData.guid; break; } o = o.parent; } if (g !== s.element) { blocked = true; break; } }
        if (!blocked) lit++;
      }
      return lit / pts.length;
    });
  }
  function publish(o = {}) {
    const SV = app.statevars; if (!SV) return null;
    const g = app.sun.get(), date = g.date, hour = g.time, out = {};
    if (last && last.date === date) { const t = tempsAt(hour); if (t) { SV.writeSim('room-air-temperature', zones.map((z, i) => [z.guid, t[i]])); out.rooms = zones.length; } }
    const c = conductances(); SV.writeSim('room-air-change-rate', zones.map(z => [z.guid, (c.hrv ? z.achEff : z.ach) + c.Hv[z.i] / (RHO_CP * z.V)]));
    if (o.surfaces !== false) {
      const p = app.sun.position({ date, time: hour }), sk = scaleSky(clearSky(date, p.elevation), clamp(inputs.clearness, 0, 1)); sk.ghi = sk.dni * Math.sin(rad(Math.max(p.elevation, 0))) + sk.dhi;
      const lf = [], ir = [], day = p.elevation > 0.2;
      for (const s of outside) {
        const w = winById.get(s.id), lit = !day ? 0 : w ? sunlit(w, p.direction) : surfaceSunlit(s, p.direction);
        const I = !day ? 0 : sk.dni * Math.max(0, s.n.dot(p.direction)) * lit + sk.dhi * (1 + s.n.z) / 2 + sk.ghi * 0.2 * (1 - s.n.z) / 2;
        lf.push([s.id, lit]); ir.push([s.id, I]);
      }
      SV.writeSim('surface-sunlit-fraction', lf); SV.writeSim('surface-irradiance', ir); out.surfaces = outside.length;
    }
    app.emit('statevalues', out); return out;
  }
  const api = { publish, inputs, zones, windows, run, last: () => last, windowSun, tempsAt, csv, model, clearSky, scaleSky, typicalFor, sunlit, invalidate() { shadeCache = null; last = null; }, STEPS, hh, solarNoon, surfaces };
  app.thermal = api;
  app.on('site', () => { shadeCache = null; });
  return api;
}
