// Menus and the command palette. Roles: menubar > menuitem (opens a menu); menu > menuitem, menuitemcheckbox,
// menuitemradio. Keys: arrows, Home, End, Enter, Space, Esc, Tab, type-ahead; Left and Right move between menus.
//
// An item is one of
//   { label, run(), keys?, meta?, disabled?: () => reason | false, keepOpen? }              a command
//   { label, check: () => bool, run(), keepOpen? }                                           a checkbox item
//   { label, radio: () => bool, run(), keepOpen? }                                           a radio item
//   { heading: 'Text' }   { sep: true }
// Menu specs are functions, so counts and checks are read when the menu opens.
import { app } from './app.js';
import { keyLabel } from './commands.js';

let openMenu = null;   // {el, opener, items, bar}

const el = (tag, cls, attrs) => { const e = document.createElement(tag); if (cls) e.className = cls; if (attrs) for (const k in attrs) e.setAttribute(k, attrs[k]); return e; };

export function closeMenu(restore = true) {
  if (!openMenu) return;
  const { el: m, opener } = openMenu; openMenu = null;
  m.remove();
  if (opener) { opener.setAttribute('aria-expanded', 'false'); if (restore) opener.focus({ preventScroll: true }); }
  removeEventListener('pointerdown', outside, true);
}
const outside = e => { if (openMenu && !openMenu.el.contains(e.target) && !(openMenu.opener && openMenu.opener.contains(e.target))) closeMenu(false); };

function buildItem(it, menuEl) {
  if (it.sep) return el('div', 'msep', { role: 'separator' });
  if (it.heading) { const h = el('div', 'mh', { role: 'presentation' }); h.textContent = it.heading; return h; }
  const role = it.check ? 'menuitemcheckbox' : it.radio ? 'menuitemradio' : 'menuitem';
  const b = el('button', 'mi', { role, type: 'button', tabindex: '-1' });
  b.textContent = it.label;
  if (it.meta != null) { const s = el('span', 'm'); s.textContent = it.meta; b.append(s); }
  else if (it.keys) { const s = el('span', 'key'); s.textContent = keyLabel(it.keys); b.append(s); }
  const sync = () => {
    if (it.check) b.setAttribute('aria-checked', String(!!it.check()));
    if (it.radio) b.setAttribute('aria-checked', String(!!it.radio()));
    const why = it.disabled ? it.disabled() : false;
    if (why) { b.setAttribute('aria-disabled', 'true'); b.title = typeof why === 'string' ? why : 'not available'; } else { b.removeAttribute('aria-disabled'); b.removeAttribute('title'); }
  };
  sync(); b._sync = sync; b._it = it;
  return b;
}

// show a menu for `items` anchored below (or beside) `anchor`
export function showMenu(anchor, items, { label = 'Menu', bar = null, side = false } = {}) {
  closeMenu(false);
  const m = el('div', 'menu', { role: 'menu', 'aria-label': label });
  for (const it of items) m.append(buildItem(it, m));
  document.body.append(m);
  const r = anchor.getBoundingClientRect();
  m.style.left = '0px'; m.style.top = '0px';
  const w = m.offsetWidth, h = m.offsetHeight;
  let x = side ? r.right + 4 : r.left, y = side ? r.top : r.bottom + 2;
  x = Math.max(4, Math.min(x, innerWidth - w - 4)); if (y + h > innerHeight - 4) y = Math.max(4, Math.min(r.top - h - 2, innerHeight - h - 4)); if (y + h > innerHeight - 4) y = 4;
  m.style.left = x + 'px'; m.style.top = y + 'px';
  anchor.setAttribute('aria-expanded', 'true');
  openMenu = { el: m, opener: anchor, items, bar };
  addEventListener('pointerdown', outside, true);
  const live = () => [...m.querySelectorAll('.mi')];
  const focusAt = i => { const l = live(); if (!l.length) return; const k = (i + l.length) % l.length; l[k].focus({ preventScroll: false }); };
  const activate = b => {
    const it = b._it; if (!it || b.getAttribute('aria-disabled') === 'true') return;
    const keep = it.keepOpen;
    if (!keep) closeMenu(true);
    const res = it.run && it.run();
    if (keep) { live().forEach(x => x._sync && x._sync()); }
    return res;
  };
  let tbuf = '', tt = 0;
  m.addEventListener('click', e => { const b = e.target.closest('.mi'); if (b) activate(b); });
  m.addEventListener('keydown', e => {
    const l = live(), i = l.indexOf(document.activeElement);
    const k = e.key;
    if (k === 'ArrowDown') { e.preventDefault(); focusAt(i + 1); }
    else if (k === 'ArrowUp') { e.preventDefault(); focusAt(i < 0 ? -1 : i - 1); }
    else if (k === 'Home') { e.preventDefault(); focusAt(0); }
    else if (k === 'End') { e.preventDefault(); focusAt(l.length - 1); }
    else if (k === 'Escape') { e.preventDefault(); e.stopPropagation(); closeMenu(true); }
    else if (k === 'Tab') { closeMenu(true); }
    else if ((k === 'ArrowRight' || k === 'ArrowLeft') && bar) { e.preventDefault(); const bs = [...bar.querySelectorAll('[data-menu]')], j = bs.indexOf(openMenu.opener); const nb = bs[(j + (k === 'ArrowRight' ? 1 : -1) + bs.length) % bs.length]; nb.focus(); nb.click(); }
    else if ((k === 'Enter' || k === ' ') && i >= 0) { e.preventDefault(); activate(l[i]); }
    else if (k.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();   // type-ahead: the key is for the menu, not a shortcut (typing "me" must not start Measure)
      clearTimeout(tt); tbuf += k.toLowerCase(); tt = setTimeout(() => { tbuf = ''; }, 600);
      const start = tbuf.length > 1 ? i : i + 1; const order = [...l.slice(start), ...l.slice(0, start)];
      const hit = order.find(b => b.textContent.toLowerCase().startsWith(tbuf)); if (hit) hit.focus();
    }
  });
  const first = live().find(b => b.getAttribute('aria-disabled') !== 'true') || live()[0];
  if (first) first.focus({ preventScroll: true });
  return m;
}

// ---- the menu bar ----
export function initMenubar(bar, specs, allBtn) {
  const btns = [...bar.querySelectorAll('[data-menu]')];
  btns.forEach((b, i) => { b.tabIndex = i === 0 ? 0 : -1; });
  const open = b => { const f = specs[b.dataset.menu]; if (!f) return; showMenu(b, f(), { label: b.textContent.trim(), bar }); };
  btns.forEach(b => {
    b.addEventListener('click', () => { if (openMenu && openMenu.opener === b) closeMenu(true); else open(b); });
    b.addEventListener('pointerenter', () => { if (openMenu && openMenu.bar === bar && openMenu.opener !== b) open(b); });
    b.addEventListener('focus', () => { btns.forEach(x => { x.tabIndex = x === b ? 0 : -1; }); });
    b.addEventListener('keydown', e => {
      const i = btns.indexOf(b);
      if (e.key === 'ArrowRight') { e.preventDefault(); btns[(i + 1) % btns.length].focus(); }
      else if (e.key === 'ArrowLeft') { e.preventDefault(); btns[(i - 1 + btns.length) % btns.length].focus(); }
      else if (e.key === 'ArrowDown' || e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(b); }
      else if (e.key === 'Home') { e.preventDefault(); btns[0].focus(); }
      else if (e.key === 'End') { e.preventDefault(); btns[btns.length - 1].focus(); }
    });
  });
  if (allBtn) allBtn.addEventListener('click', () => {
    if (openMenu && openMenu.opener === allBtn) { closeMenu(true); return; }
    const items = []; btns.forEach(b => { items.push({ heading: b.textContent.trim() }); items.push(...specs[b.dataset.menu]()); });
    showMenu(allBtn, items, { label: 'Menu' });
  });
  return { open };
}

// ---- the command palette ----
let pal = null;
export function closePalette() { if (!pal) return; const { root, back } = pal; pal = null; root.remove(); if (back && back.focus) back.focus({ preventScroll: true }); }
// sources: [(query) => [{kind, label, tag?, run}]]
export function openPalette(sources, initial = '') {
  if (pal) { closePalette(); return; }
  closeMenu(false);
  const back = document.activeElement;
  const root = el('div', '', { id: 'palette', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Command palette' });
  root.innerHTML = '<div class="box"><input type="text" role="combobox" aria-expanded="true" aria-controls="palList" aria-autocomplete="list" placeholder="Search elements, rooms, devices, commands and queries" autocomplete="off" spellcheck="false"><div id="palList" role="listbox" aria-label="Results"></div><div class="pal-foot"><span><kbd>↑</kbd> <kbd>↓</kbd> move</span><span><kbd>Enter</kbd> run</span><span><kbd>Esc</kbd> close</span></div></div>';
  document.body.append(root);
  const inp = root.querySelector('input'), list = root.querySelector('#palList');
  pal = { root, back };
  let hits = [], sel = 0;
  const paint = () => {
    list.innerHTML = '';
    hits.forEach((h, i) => {
      const d = el('div', 'pi', { role: 'option', id: 'pal-' + i, 'aria-selected': String(i === sel) });
      const k = el('span', 'kind'); k.textContent = h.kind; const t = el('span'); t.textContent = h.label; d.append(k, t);
      if (h.tag) { const g = el('span', 'tag'); g.textContent = h.tag; d.append(g); }
      d.addEventListener('click', () => { go(i); });
      d.addEventListener('pointermove', () => { if (sel !== i) { sel = i; mark(); } });
      list.append(d);
    });
    if (!hits.length) { const d = el('div', 'empty'); d.style.padding = '10px 14px'; d.textContent = 'Nothing found.'; list.append(d); }
    inp.setAttribute('aria-activedescendant', hits.length ? 'pal-' + sel : '');
  };
  const mark = () => { [...list.children].forEach((c, i) => c.setAttribute('aria-selected', String(i === sel))); inp.setAttribute('aria-activedescendant', 'pal-' + sel); const c = list.children[sel]; if (c && c.scrollIntoView) c.scrollIntoView({ block: 'nearest' }); };
  const search = () => {
    const q = inp.value.trim().toLowerCase(); hits = [];
    for (const s of sources) { try { hits.push(...s(q)); } catch (e) { console.error('[building] palette source failed', e); } }
    sel = 0; paint();
  };
  const go = i => { const h = hits[i]; if (!h) return; closePalette(); try { h.run(); } catch (e) { app.notify && app.notify('Failed: ' + (e.message || e)); } };
  inp.addEventListener('input', search);
  inp.addEventListener('keydown', e => {
    if (e.key === 'ArrowDown') { e.preventDefault(); sel = Math.min(hits.length - 1, sel + 1); mark(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); sel = Math.max(0, sel - 1); mark(); }
    else if (e.key === 'Enter') { e.preventDefault(); go(sel); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closePalette(); }
    else if (e.key === 'Tab') { e.preventDefault(); }
  });
  root.addEventListener('pointerdown', e => { if (e.target === root) closePalette(); });
  inp.value = initial; search(); inp.focus();
}
export const paletteOpen = () => !!pal;
export const menuIsOpen = () => !!openMenu;
