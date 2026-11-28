// The command registry and the keyboard shortcuts. One table: menus, the command palette, the About dialog
// and the key handler all read the same entries.
//   defineCommands([{ id, title, group, keys?, run(arg), enabled?() -> a reason string (disabled) | false, true or nothing (can run), checked?() }])
//   keys: 'Mod+K' (Ctrl on Windows and Linux, Cmd on macOS), 'Shift+F', 'I', or a sequence 'G C' (press G, then C).
import { app } from './app.js';

const cmds = new Map();
let pending = null, pendingT = 0;
const isMac = /Mac|iPhone|iPad/.test(navigator.platform || '');

export function defineCommands(list) { for (const c of list) cmds.set(c.id, c); }
export const getCommand = id => cmds.get(id);
export const listCommands = () => [...cmds.values()];
export function disabledReason(c) {
  if (!c.enabled) return null;
  let r; try { r = c.enabled(); } catch { r = 'not available'; }
  return typeof r === 'string' && r ? r : null;   // a reason string disables the command; true, false or nothing means it can run
}
export async function runCommand(id, arg) {
  const c = cmds.get(id); if (!c) throw new Error('No command "' + id + '".');
  const why = disabledReason(c); if (why) { app.notify && app.notify(c.title + ': ' + why); return false; }
  if (app.log) app.log('info', 'user', c.title, { command: id, arg: arg === undefined ? undefined : String(arg).slice(0, 80) });
  try { const r = await c.run(arg); app.emit('command', { id }); return r === undefined ? true : r; }
  catch (e) { if (app.log) app.log('error', 'user', c.title + ' failed: ' + (e.message || e), { command: id }); throw e; }
}

// ---- key labels and matching ----
export function keyLabel(spec) {
  if (!spec) return '';
  return spec.split(' ').map(k => k.split('+').map(p => p === 'Mod' ? (isMac ? '⌘' : 'Ctrl') : p === 'Shift' && isMac ? '⇧' : p).join(isMac ? '' : '+')).join(' then ');
}
function matches(e, spec) {
  const parts = spec.split('+'), key = parts.pop();
  const mod = parts.includes('Mod'), shift = parts.includes('Shift'), alt = parts.includes('Alt');
  const hasMod = isMac ? e.metaKey : e.ctrlKey;
  if (mod !== hasMod) return false;
  if (alt !== e.altKey) return false;
  if (!mod && (e.ctrlKey || e.metaKey)) return false;
  const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  const want = key.length === 1 ? key.toLowerCase() : key;
  if (k !== want) return false;
  if (key.length === 1 && /[a-z]/i.test(key) || key.length > 1) { if (shift !== e.shiftKey) return false; }
  return true;
}
const typing = t => t && (/^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) || t.isContentEditable);

export function bindShortcuts() {
  addEventListener('keydown', e => {
    if (e.defaultPrevented || e.isComposing) return;
    const inField = typing(e.target), walking = app.mode === 'walk' || app.mode === 'fly';
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey;
    // a second key of a sequence
    if (pending) {
      const first = pending; pending = null; clearTimeout(pendingT); app.notify && app.notify('');
      if (plain && !inField && e.key.length === 1) {
        for (const c of cmds.values()) if (c.keys && c.keys === first + ' ' + e.key.toUpperCase() ) { e.preventDefault(); runCommand(c.id); return; }
      }
      return;
    }
    for (const c of cmds.values()) {
      if (!c.keys) continue;
      const seq = c.keys.split(' ');
      const spec = seq[0];
      const modded = /Mod|Alt/.test(spec);
      if (!modded && (inField || (walking && !c.inWalk))) continue;   // single keys are not shortcuts while typing or walking
      if (!matches(e, spec)) continue;
      if (seq.length > 1) { e.preventDefault(); pending = spec; app.notify && app.notify(spec + ' …  (' + seq.slice(1).join(' ') + ')', { ms: 1200 }); pendingT = setTimeout(() => { pending = null; }, 1200); return; }
      if (c.global === false && inField) continue;
      e.preventDefault(); runCommand(c.id); return;
    }
  });
}
// the table the About dialog prints
export function shortcutTable() {
  return listCommands().filter(c => c.keys || c.keyNote).map(c => ({ id: c.id, title: c.title, keys: c.keys ? keyLabel(c.keys) : c.keyNote, group: c.group }));
}
