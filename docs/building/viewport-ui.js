// Controls for the rendering side's API: structure colours, ground cutaway, section cuts (Services panel, and the
// pop-over in the viewport). Every control feature-detects its API when it is drawn and shows as not available when
// the API is absent; nothing here throws.
import { app } from './app.js';
import { $, esc, has } from './shared.js';
import { icon } from './icons.js';

const MODES = [['material', 'Material'], ['role', 'Role'], ['load-bearing', 'Load bearing'], ['from-ifc', 'IFC or designed']];
const CUTS = [['none', 'As it is'], ['trench', 'Trench'], ['hidden', 'Hidden']];
const NA = 'not available yet';

export const sectionState = () => (has('section.get') ? app.section.get() : null);

function seg(label, items, cur, onPick, can) {
  return `<div class="row"><span>${label}</span></div><div class="seg" role="group" aria-label="${esc(label)}" style="display:flex;flex-wrap:wrap">${items.map(([v, t]) => `<button class="segb" type="button" data-v="${v}" aria-pressed="${v === cur}" ${can ? '' : `aria-disabled="true" title="${NA}"`}>${esc(t)}</button>`).join('')}</div>`;
}
export function drawServicesView() {
  const box = $('svcViewBody'); if (!box) return;
  const canMode = has('structure.setColourMode'), canCut = has('ground.setCutaway'), canSec = has('section.set');
  const mode = canMode ? app.structure.getColourMode() : null, cut = has('ground.getCutaway') ? app.ground.getCutaway() : null, sec = sectionState();
  let leg = ''; if (canMode) { try { const l = app.structure.legend(); if (l) leg = `<div class="ins-chips" style="margin:6px 0">${l.items.slice(0, 24).map(i => `<span class="chip"><i style="background:${esc(i.color)}"></i>${esc(i.label)}</span>`).join('')}</div>`; } catch { /* none */ } }
  let st = ''; if (has('structure.state')) { try { const s = app.structure.state(); st = `<div class="kv" style="margin-top:8px"><b>Removed:</b> ${s.removed.length} · <b>lost their support:</b> ${s.unsupported.length} <button class="btn sm" type="button" data-restore style="margin-left:6px" ${s.removed.length ? '' : 'disabled'}>Restore all</button></div>`; } catch { /* none */ } }
  box.innerHTML = `${seg('Structure colours', MODES, mode, null, canMode)}${leg}
    ${seg('Ground', CUTS, cut, null, canCut)}
    <div class="row" style="margin-top:8px"><span>Section cuts</span><span>${['x', 'y', 'z'].map(a => `<label class="opt" style="display:inline-flex;margin-left:8px"><input type="checkbox" data-sec="${a}" ${sec && sec[a] && sec[a].enabled ? 'checked' : ''} ${canSec ? '' : `disabled title="${NA}"`}>${a.toUpperCase()}</label>`).join('')}</span></div>
    <button class="btn sm" type="button" data-secpop ${canSec ? '' : `aria-disabled="true" title="${NA}"`}>Section settings…</button>${st}
    ${canMode || canCut || canSec ? '' : `<p class="note">The structure layer, the ground cutaway and the section cuts are not available yet.</p>`}`;
}
export function openSectionPop() {
  const pop = $('sectionPop'); pop.hidden = false;
  const draw = () => {
    const s = sectionState() || {}, hb = app.houseBox;
    pop.innerHTML = `<div style="display:flex;justify-content:space-between;align-items:center"><h3>Section cuts</h3><button class="ibtn" type="button" id="secX" aria-label="Close">${icon('close')}</button></div>` + ['x', 'y', 'z'].map(a => {
      const v = s[a] || {}, lo = Math.floor((hb ? hb.min[a] : -2) - 1), hi = Math.ceil((hb ? hb.max[a] : 14) + 1);
      return `<div class="vrow"><div class="vh"><label class="opt"><input type="checkbox" data-a="${a}" data-f="enabled" ${v.enabled ? 'checked' : ''}>Section ${a.toUpperCase()}</label><label class="opt"><input type="checkbox" data-a="${a}" data-f="flip" ${v.flip ? 'checked' : ''}>Flip</label></div><input type="range" data-a="${a}" data-f="position" min="${lo}" max="${hi}" step="0.05" value="${v.position ?? (lo + hi) / 2}" aria-label="Position of section ${a.toUpperCase()}"></div>`;
    }).join('') + `<label class="opt"><input type="checkbox" data-apply ${s.applyTo3D ? 'checked' : ''}>Apply to the 3D view</label><p class="note">The plan obeys the Z cut, the front elevation Y, the side elevation X.</p>`;
    $('secX').onclick = () => { pop.hidden = true; };
  };
  draw();
  pop.oninput = pop.onchange = e => {
    const el = e.target;
    try { if (el.dataset.apply !== undefined) app.section.setApplyTo3D(el.checked); else if (el.dataset.a) { const f = el.dataset.f; app.section.set(el.dataset.a, { [f]: f === 'position' ? +el.value : el.checked }); } } catch (err) { app.notify(String(err.message || err)); }
  };
  pop.onkeydown = e => { if (e.key === 'Escape') { pop.hidden = true; } };
  const f = pop.querySelector('input'); if (f) f.focus();
}
export function initViewportUi() {
  const box = $('svcViewBody');
  box.addEventListener('click', e => {
    const b = e.target.closest('.segb'); if (b && b.getAttribute('aria-disabled') !== 'true') {
      const grp = b.parentElement.getAttribute('aria-label'); try { if (grp === 'Ground') app.ground.setCutaway(b.dataset.v); else app.structure.setColourMode(b.dataset.v); } catch (err) { app.notify(String(err.message || err)); } drawServicesView(); return;
    }
    if (e.target.closest('[data-secpop]') && has('section.set')) openSectionPop();
    if (e.target.closest('[data-restore]')) { try { app.structure.restore(); } catch (err) { app.notify(String(err.message || err)); } drawServicesView(); }
  });
  box.addEventListener('change', e => { const a = e.target.dataset.sec; if (!a) return; try { app.section.set(a, { enabled: e.target.checked }); } catch (err) { app.notify(String(err.message || err)); } });
  // in the four-view layout the 3D hint and the colour legend would sit on top of the plan and the elevations: the page marks the layout for its CSS
  const mark = () => { try { document.body.dataset.layout = has('viewport.getLayout') ? app.viewport.getLayout() : 'single'; } catch { document.body.dataset.layout = 'single'; } };
  app.on('layout', mark); app.on('ready', mark); mark();
  for (const ev of ['ready', 'cutaway', 'structure', 'structurecolour', 'section', 'layout', 'reset', 'interactive']) app.on(ev, drawServicesView);
  drawServicesView();
}
