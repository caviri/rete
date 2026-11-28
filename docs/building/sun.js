// The sun, physically placed: NOAA solar position (declination, equation of time,
// hour angle) for a date, a local clock time and the site, turned into a direction in
// the model frame with the model's true north. Also: sunrise/sunset, the sun path arc,
// the compass rose and "play day".
//
// position(dateTime) is pure: no scene, no state. A thermal simulation can sample it.
import * as THREE from 'three';
import { app } from './app.js';

const rad = d => d * Math.PI / 180, deg = r => r * 180 / Math.PI;
const DEFAULT_SITE = { latitude: 49.10, longitude: 8.43, timezone: 'Europe/Berlin', trueNorth: [0, 1], northAzimuthDeg: 0, name: 'Karlsruhe (default)' };
let site = { ...DEFAULT_SITE };
let date = null, time = 12;   // local date 'YYYY-MM-DD' and local clock hours (decimal)
let playing = null, pathGroup = null, pathOn = false, marker = null;

// ---------- time zones ----------
const fmtCache = new Map();
function tzOffsetMin(utcMs, tz) {
  let f = fmtCache.get(tz);
  if (!f) { f = new Intl.DateTimeFormat('en-US', { timeZone: tz, hourCycle: 'h23', year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: 'numeric', second: 'numeric' }); fmtCache.set(tz, f); }
  const p = {}; f.formatToParts(new Date(utcMs)).forEach(x => { p[x.type] = +x.value; });
  return (Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second) - Math.floor(utcMs / 1000) * 1000) / 60000;
}
function parseDate(d) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(d));
  if (!m) throw new Error('date must be YYYY-MM-DD, got "' + d + '".');
  return [+m[1], +m[2], +m[3]];
}
export function parseTime(t) {
  if (typeof t === 'number') { if (!isFinite(t) || t < 0 || t > 24) throw new Error('time must be 0 to 24 hours.'); return t; }
  const m = /^(\d{1,2}):(\d{2})(?::(\d{2}))?$/.exec(String(t));
  if (!m || +m[1] > 24 || +m[2] > 59) throw new Error('time must be HH:MM or hours 0 to 24, got "' + t + '".');
  return +m[1] + m[2] / 60 + (m[3] ? +m[3] / 3600 : 0);
}
function localToUtc(dateStr, hours, tz) {
  const [y, mo, d] = parseDate(dateStr);
  const base = Date.UTC(y, mo - 1, d) + hours * 3600000;
  let off = tzOffsetMin(base, tz), utc = base - off * 60000;
  const off2 = tzOffsetMin(utc, tz); if (off2 !== off) utc = base - off2 * 60000;
  return utc;
}
export const hoursStr = h => { if (h == null) return '—'; const m = Math.round(h * 60) % 1440; return String(Math.floor(m / 60)).padStart(2, '0') + ':' + String(m % 60).padStart(2, '0'); };

// ---------- NOAA solar maths ----------
function solarTerms(utcMs) {
  const jd = utcMs / 86400000 + 2440587.5, T = (jd - 2451545) / 36525;
  const L0 = (280.46646 + T * (36000.76983 + T * 0.0003032)) % 360;
  const M = 357.52911 + T * (35999.05029 - 0.0001537 * T), Mr = rad(M);
  const e = 0.016708634 - T * (0.000042037 + 0.0000001267 * T);
  const C = Math.sin(Mr) * (1.914602 - T * (0.004817 + 0.000014 * T)) + Math.sin(2 * Mr) * (0.019993 - 0.000101 * T) + Math.sin(3 * Mr) * 0.000289;
  const omega = 125.04 - 1934.136 * T;
  const lambda = L0 + C - 0.00569 - 0.00478 * Math.sin(rad(omega));
  const eps = 23 + (26 + (21.448 - T * (46.815 + T * (0.00059 - T * 0.001813))) / 60) / 60 + 0.00256 * Math.cos(rad(omega));
  const decl = deg(Math.asin(Math.sin(rad(eps)) * Math.sin(rad(lambda))));
  const y = Math.tan(rad(eps / 2)) ** 2, l0 = rad(L0);
  const eot = 4 * deg(y * Math.sin(2 * l0) - 2 * e * Math.sin(Mr) + 4 * e * y * Math.sin(Mr) * Math.cos(2 * l0) - 0.5 * y * y * Math.sin(4 * l0) - 1.25 * e * e * Math.sin(2 * Mr));
  return { decl, eot };
}
function horizontal(utcMs, lat, lon) {
  const { decl, eot } = solarTerms(utcMs);
  const minutes = ((utcMs % 86400000) + 86400000) % 86400000 / 60000;
  let tst = (minutes + eot + 4 * lon) % 1440; if (tst < 0) tst += 1440;
  let ha = tst / 4 - 180; if (ha < -180) ha += 360;
  const la = rad(lat), de = rad(decl), h = rad(ha);
  const cosZ = Math.sin(la) * Math.sin(de) + Math.cos(la) * Math.cos(de) * Math.cos(h);
  const Z = Math.acos(Math.min(1, Math.max(-1, cosZ)));
  const arg = Math.min(1, Math.max(-1, ((Math.sin(la) * Math.cos(Z)) - Math.sin(de)) / (Math.cos(la) * Math.sin(Z) || 1e-9)));
  const a = deg(Math.acos(arg));
  const az = ha > 0 ? (a + 180) % 360 : (540 - a) % 360;
  return { azimuth: az, elevation: 90 - deg(Z), declination: decl, eot };
}
const toDir = (az, el, s) => {
  const th = rad(az + s.northAzimuthDeg), ce = Math.cos(rad(el));
  return new THREE.Vector3(Math.sin(th) * ce, Math.cos(th) * ce, Math.sin(rad(el)));
};
function utcOf(dt, s) {
  if (dt instanceof Date) return dt.getTime();
  if (typeof dt === 'number') return dt;
  if (typeof dt === 'string') { const t = Date.parse(dt); if (isNaN(t)) throw new Error('cannot read date-time "' + dt + '"; give ISO with an offset, or {date, time}.'); return t; }
  if (dt && dt.date != null) return localToUtc(dt.date, parseTime(dt.time ?? 12), s.timezone);
  throw new Error('position needs a Date, an ISO string, or {date, time}.');
}
// Pure. dateTime: Date | ms | ISO string with offset | {date:'YYYY-MM-DD', time: hours | 'HH:MM'} (local to the site).
// Returns {azimuth (deg from true north, clockwise), elevation (deg), direction (unit Vector3 toward the sun, model frame), isDay, declination}.
export function position(dateTime, siteOverride) {
  const s = { ...site, ...(siteOverride || {}) };
  if (siteOverride && siteOverride.trueNorth && siteOverride.northAzimuthDeg == null) s.northAzimuthDeg = deg(Math.atan2(siteOverride.trueNorth[0], siteOverride.trueNorth[1]));
  const r = horizontal(utcOf(dateTime, s), s.latitude, s.longitude);
  return { azimuth: r.azimuth, elevation: r.elevation, direction: toDir(r.azimuth, r.elevation, s), isDay: r.elevation > 0, declination: r.declination };
}
// sunrise, solar noon and sunset (local decimal hours) for a local date; null when the sun does not rise/set
export function sunTimes(dateStr, s = site) {
  const [y, mo, d] = parseDate(dateStr);
  const dayStart = Date.UTC(y, mo - 1, d);
  const noonUtc = localToUtc(dateStr, 12, s.timezone);
  let t = solarTerms(noonUtc), res = {};
  for (let k = 0; k < 2; k++) {
    const la = rad(s.latitude), de = rad(t.decl);
    const c = Math.cos(rad(90.833)) / (Math.cos(la) * Math.cos(de)) - Math.tan(la) * Math.tan(de);
    const noonMin = 720 - 4 * s.longitude - t.eot;
    if (c < -1 || c > 1) { res = { sunrise: null, sunset: null, solarNoon: null, polar: c < -1 ? 'day' : 'night' }; break; }
    const ha = deg(Math.acos(c));
    const toLocal = min => { const u = dayStart + min * 60000; return (u + tzOffsetMin(u, s.timezone) * 60000 - dayStart) / 3600000; };
    res = { sunrise: toLocal(noonMin - 4 * ha), sunset: toLocal(noonMin + 4 * ha), solarNoon: toLocal(noonMin) };
    t = solarTerms(dayStart + (noonMin - 4 * ha) * 60000);   // refine at sunrise
  }
  return res;
}

// ---------- state ----------
const todayStr = () => { const n = new Date(); return n.getFullYear() + '-' + String(n.getMonth() + 1).padStart(2, '0') + '-' + String(n.getDate()).padStart(2, '0'); };
export function get() {
  const p = position({ date, time }), st = sunTimes(date);
  return {
    date, time, timeStr: hoursStr(time), azimuth: +p.azimuth.toFixed(2), elevation: +p.elevation.toFixed(2),
    direction: p.direction.clone(), isDay: p.isDay, sunrise: st.sunrise, sunset: st.sunset, solarNoon: st.solarNoon,
    sunriseStr: hoursStr(st.sunrise), sunsetStr: hoursStr(st.sunset), site: { ...site },
  };
}
function apply(silent) {
  const p = position({ date, time });
  app.applySun(p.direction, p.elevation);
  if (marker) { marker.visible = p.isDay && pathOn; marker.position.copy(app.HOUSE_CENTER).addScaledVector(p.direction, PATH_R); }
  app.requestRender();
  if (!silent) app.emit('time', { ...get() });
}
// set({date?, time?}) : date 'YYYY-MM-DD', time hours (0..24) or 'HH:MM', local to the site
export function set(o = {}) {
  if (o.date != null) { parseDate(o.date); const dateChanged = o.date !== date; date = o.date; if (dateChanged) buildPath(); }
  if (o.time != null) time = parseTime(o.time);
  apply();
  return get();
}
export function setSite(s = {}) {
  const n = { ...site };
  if (s.latitude != null) { if (!(Math.abs(+s.latitude) <= 90)) throw new Error('latitude must be -90..90'); n.latitude = +s.latitude; }
  if (s.longitude != null) { if (!(Math.abs(+s.longitude) <= 180)) throw new Error('longitude must be -180..180'); n.longitude = +s.longitude; }
  if (s.timezone != null) { try { tzOffsetMin(Date.now(), s.timezone); } catch { throw new Error('unknown time zone "' + s.timezone + '" (use an IANA name such as Europe/Berlin).'); } n.timezone = s.timezone; }
  if (Array.isArray(s.trueNorth) && s.trueNorth.length >= 2) { n.trueNorth = [+s.trueNorth[0], +s.trueNorth[1]]; n.northAzimuthDeg = deg(Math.atan2(n.trueNorth[0], n.trueNorth[1])); }
  if (s.northAzimuthDeg != null && !Array.isArray(s.trueNorth)) { n.northAzimuthDeg = +s.northAzimuthDeg; n.trueNorth = [Math.sin(rad(n.northAzimuthDeg)), Math.cos(rad(n.northAzimuthDeg))]; }
  if (s.name) n.name = s.name;
  site = n; buildPath(); apply();
  app.emit('site', { ...site });
  return { ...site };
}

// ---------- play day ----------
export function playDay(on = true) {
  if (!on) { if (playing) { playing(); playing = null; app.emit('play', { playing: false }); } return false; }
  if (playing) return true;
  const st = sunTimes(date); const a = st.sunrise ?? 6, b = st.sunset ?? 18;
  if (time < a || time >= b - 0.05) time = a;
  playing = app.addTicker(dt => {
    time += dt * (b - a) / 14;                        // a day in about 14 seconds
    if (time >= b) { time = b; apply(); playing = null; app.emit('play', { playing: false }); return false; }
    apply();
  });
  app.emit('play', { playing: true });
  return true;
}

// ---------- the sun path in the sky ----------
const PATH_R = 70;
function label(txt) {
  const c = document.createElement('canvas'); c.width = 64; c.height = 32;
  const g = c.getContext('2d'); g.font = '600 24px system-ui,sans-serif'; g.textAlign = 'center'; g.textBaseline = 'middle'; g.fillStyle = '#ffe9a8'; g.fillText(txt, 32, 17);
  const t = new THREE.CanvasTexture(c); t.colorSpace = THREE.SRGBColorSpace;
  const s = new THREE.Sprite(new THREE.SpriteMaterial({ map: t, transparent: true, depthWrite: false, fog: false }));
  s.scale.set(4.5, 2.25, 1); return s;
}
function buildPath() {
  if (!pathGroup) return;
  pathGroup.clear();
  if (!date) return;
  const pts = [], ticks = [];
  for (let h = 0; h <= 24; h += 1 / 12) {
    const p = position({ date, time: h });
    if (p.elevation > -1) { pts.push(app.HOUSE_CENTER.clone().addScaledVector(p.direction, PATH_R)); if (Math.abs(h - Math.round(h)) < 1e-6 && p.elevation > 0.5) ticks.push([Math.round(h), p]); }
  }
  if (pts.length > 1) {
    pathGroup.add(new THREE.Line(new THREE.BufferGeometry().setFromPoints(pts), new THREE.LineBasicMaterial({ color: 0xffc94a, fog: false, transparent: true, opacity: 0.9 })));
  }
  ticks.forEach(([h, p]) => {
    const c = app.HOUSE_CENTER.clone().addScaledVector(p.direction, PATH_R);
    const dot = new THREE.Mesh(new THREE.SphereGeometry(0.7, 8, 6), new THREE.MeshBasicMaterial({ color: 0xffe08a, fog: false })); dot.position.copy(c); pathGroup.add(dot);
    const l = label(String(h)); l.position.copy(c).addScaledVector(p.direction, 0.1); l.position.z += 2.2; pathGroup.add(l);
  });
  app.requestRender();
}
export function setPathVisible(v) {
  pathOn = !!v; pathGroup.visible = pathOn; if (marker) marker.visible = pathOn && get().isDay;
  if (pathOn) buildPath(); app.requestRender(); app.emit('sunpath', { visible: pathOn });
}

// ---------- the compass rose ----------
function initCompass() {
  const box = document.getElementById('compass'); if (!box) return;
  box.innerHTML = `<svg viewBox="-50 -50 100 100" width="84" height="84"><circle r="46" fill="rgba(12,17,28,.78)" stroke="#2c3a52"/>
    <g id="cmpRot"><path d="M0,-40 L7,0 L0,6 L-7,0Z" fill="#ff6b5b"/><path d="M0,40 L7,0 L0,6 L-7,0Z" fill="#8b97ad"/>
    <text y="-27" x="0" text-anchor="middle" font-size="13" font-weight="700" fill="#fff" transform="translate(0,0)">N</text>
    <circle id="cmpSun" r="5" cx="0" cy="-34" fill="#ffd54a" stroke="#7a5a00" display="none"/></g></svg>`;
  const rot = box.querySelector('#cmpRot'), sunDot = box.querySelector('#cmpSun');
  let last = '';
  const fwd = new THREE.Vector3();
  const upd = () => {
    app.camera.getWorldDirection(fwd);
    if (Math.hypot(fwd.x, fwd.y) < 0.35) { fwd.set(0, 1, 0).applyQuaternion(app.camera.quaternion); }   // looking steeply down: screen-up leads
    fwd.z = 0; if (fwd.lengthSq() < 1e-6) return; fwd.normalize();
    const n = site.trueNorth, ln = Math.hypot(n[0], n[1]) || 1, nx = n[0] / ln, ny = n[1] / ln;
    const right = { x: fwd.y, y: -fwd.x };
    const a = deg(Math.atan2(nx * right.x + ny * right.y, nx * fwd.x + ny * fwd.y));
    const g = get();
    const key = a.toFixed(1) + '|' + g.azimuth.toFixed(0) + g.isDay;
    if (key === last) return; last = key;
    rot.setAttribute('transform', `rotate(${a.toFixed(1)})`);
    sunDot.setAttribute('display', g.isDay ? 'inline' : 'none');
    sunDot.setAttribute('transform', `rotate(${g.azimuth.toFixed(1)}) translate(0,0)`);
  };
  app.on('render', upd); app.on('site', () => { last = ''; });
}

export function initSun() {
  pathGroup = new THREE.Group(); pathGroup.name = 'sunPath'; pathGroup.visible = false;
  marker = new THREE.Mesh(new THREE.SphereGeometry(2.2, 16, 12), new THREE.MeshBasicMaterial({ color: 0xfff1b0, fog: false }));
  marker.visible = false; pathGroup.add(marker);
  // the marker lives outside the cleared group contents
  app.scene.add(pathGroup);
  const origClear = pathGroup.clear.bind(pathGroup);
  pathGroup.clear = () => { origClear(); pathGroup.add(marker); return pathGroup; };
  date = todayStr();
  const st = sunTimes(date); time = st.solarNoon ?? 12;
  const sun = { get, set, position, setSite, sunTimes, playDay, isPlaying: () => !!playing, setPathVisible, get site() { return { ...site }; }, pathVisible: () => pathOn, hoursStr };
  app.sun = sun; app.setSite = setSite;
  initCompass();
  apply(true);
}
// from the systems file, when it has a site object
export function siteFromSystems(sys) {
  if (!sys || !sys.site) return false;
  const s = sys.site, o = {};
  if (s.latitude != null) o.latitude = s.latitude; if (s.longitude != null) o.longitude = s.longitude; if (s.timezone) o.timezone = s.timezone;
  if (Array.isArray(s.trueNorth)) o.trueNorth = s.trueNorth; else if (s.northAzimuthDeg != null) o.northAzimuthDeg = s.northAzimuthDeg;
  o.name = s.name || 'site from the systems file';
  setSite(o); return true;
}
