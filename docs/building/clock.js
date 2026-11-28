// A small clock in the corner of the viewport: local time, date, a sun or moon icon and the sun's elevation.
// It follows the sun controls (also during "Play day"). Drag on it or scroll over it to move the time in steps
// of 10 minutes; a click opens the sun controls.
import { app } from './app.js';

const CSS = `
#clock{position:absolute;right:14px;bottom:98px;z-index:8;display:flex;align-items:center;gap:8px;padding:6px 11px;
  background:var(--overlay);backdrop-filter:blur(6px);border:1px solid var(--line);border-radius:12px;color:var(--ink);
  cursor:ew-resize;user-select:none;touch-action:none;font-size:12px;line-height:1.25}
#clock:hover{border-color:var(--accent)}
#clock svg{width:22px;height:22px;flex:0 0 auto}
#clock b{font-size:17px;font-weight:600;font-variant-numeric:tabular-nums;letter-spacing:.02em}
#clock small{display:block;color:var(--dim);font-size:11px;font-variant-numeric:tabular-nums}
body.walking #clock{bottom:auto;top:56px}
@media (max-width:640px){#clock{bottom:88px;padding:5px 9px}#clock b{font-size:15px}}
`;
const SUN = '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="4.6" fill="#ffd54a"/><g stroke="#ffd54a" stroke-width="1.8" stroke-linecap="round"><path d="M12 2.5v2.4M12 19.1v2.4M2.5 12h2.4M19.1 12h2.4M5.3 5.3l1.7 1.7M17 17l1.7 1.7M5.3 18.7L7 17M17 7l1.7-1.7"/></g></svg>';
const MOON = '<svg viewBox="0 0 24 24"><path d="M15.5 3.2a9 9 0 1 0 5.3 12.9A7.4 7.4 0 0 1 15.5 3.2z" fill="#cdd8f5"/></svg>';
const MON = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

export function initClock() {
  if (document.getElementById('clock')) return;
  const st = document.createElement('style'); st.textContent = CSS; document.head.appendChild(st);
  const el = document.createElement('div'); el.id = 'clock'; el.title = 'Local time. Drag or scroll to change it, click for the sun controls.';
  el.innerHTML = '<span id="clkI"></span><div><b id="clkT">--:--</b><small id="clkD"></small></div><small id="clkE" style="margin-left:2px"></small>';
  document.getElementById('stage').appendChild(el);
  const $ = id => document.getElementById(id);
  let icon = null;
  const show = () => {
    const g = app.sun.get(), up = g.elevation > 0;
    $('clkT').textContent = g.timeStr;
    $('clkD').textContent = `${+g.date.slice(8, 10)} ${MON[+g.date.slice(5, 7) - 1]} ${g.date.slice(0, 4)}`;
    $('clkE').innerHTML = `sun<br>${g.elevation >= 0 ? '+' : '−'}${Math.abs(g.elevation).toFixed(0)}°`;
    const want = up ? 'sun' : 'moon';
    if (icon !== want) { icon = want; $('clkI').innerHTML = up ? SUN : MOON; }
  };
  const scrub = steps => {
    const g = app.sun.get(); app.sun.playDay(false);
    const t = Math.min(24, Math.max(0, Math.round(g.time * 6) / 6 + steps / 6));
    app.sun.set({ time: t });
  };
  let drag = null;
  el.addEventListener('pointerdown', e => { drag = { x: e.clientX, moved: 0, acc: 0 }; el.setPointerCapture(e.pointerId); e.stopPropagation(); });
  el.addEventListener('pointermove', e => {
    if (!drag) return;
    const dx = e.clientX - drag.x; drag.x = e.clientX; drag.moved += Math.abs(dx); drag.acc += dx;
    const n = Math.trunc(drag.acc / 10); if (n) { drag.acc -= n * 10; scrub(n); }
  });
  el.addEventListener('pointerup', e => {
    if (!drag) return; const tap = drag.moved < 5; drag = null;
    if (!tap) return;
    if (app.shell) app.shell.openSide('sun');   // the Sun panel; phones show it as a sheet
    const t = $('tod'); if (t) { const d = t.closest('details'); if (d) d.open = true; t.scrollIntoView({ block: 'center' }); t.focus({ preventScroll: true }); }
  });
  el.addEventListener('pointercancel', () => { drag = null; });
  el.addEventListener('wheel', e => { e.preventDefault(); e.stopPropagation(); scrub(e.deltaY < 0 ? 1 : -1); }, { passive: false });
  app.on('time', show); app.on('site', show);
  show();
}
