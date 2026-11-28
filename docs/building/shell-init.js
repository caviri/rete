// Starts the shell after the house is on screen: the log's event subscriptions, the dashboard (rail, panels, tabs), the
// status bar, the console and results, the logs tab, the graph tab (its code loads on first open), the commands and menus.
import { app } from './app.js';
import * as LOG from './logs.js';
import { initDashboard, onSide, onTab } from './dashboard.js';
import { initStatusbar } from './statusbar.js';
import { initConsole } from './console.js';
import { initGraphTab } from './graph-tab.js';
import { initActions } from './actions.js';
import { initViewportUi } from './viewport-ui.js';
import { initValidate } from './validate.js';
import { $ } from './shared.js';
import { renderExplorer } from './statevars-ui.js';

export function initShell() {
  LOG.subscribeApp();
  const shell = initDashboard();
  initStatusbar();
  initConsole();
  if (app.pendingResult) { const [r, o] = app.pendingResult; app.pendingResult = null; app.results.show(r, o); }
  // the log tab, its unread badge
  const lg = LOG.mountLogs($('tp-logs')); app.logsUi = lg;
  LOG.subscribe(() => { const n = LOG.unreadCount(); shell.setBadge('logs', n ? (n > 99 ? '99+' : String(n)) : null, !!n); });
  onTab('logs', () => { LOG.setVisible(true); lg.draw(); shell.setBadge('logs', null); });
  app.on('tab', e => { LOG.setVisible(!!e && e.tab === 'logs'); });
  LOG.setVisible(false);
  initGraphTab();
  initActions();
  initViewportUi();
  initValidate();
  // the thermal model loads when its panel opens
  onSide('thermal', () => {
    if (!app.svc) { $('thermSec').innerHTML = '<p class="note">The thermal day needs the services layer, which is not loaded.</p>'; return; }
    if (!app.thermal) app.loadThermal().catch(e => console.error('[building] thermal failed to load', e));
  });
  // the states panel: the variables of the selection, or the overview
  const varBody = () => $('varBody');
  const showVars = () => { const b = varBody(); if (b) renderExplorer(b); };
  let vt = 0; const later = () => { if (shell.state().side !== 'states') return; clearTimeout(vt); vt = setTimeout(showVars, 200); };
  onSide('states', showVars); for (const ev of ['value', 'values', 'reset', 'scenario']) app.on(ev, later);
  // the link in the address bar
  app.on('ready', () => { if (location.hash.length > 1 && /v=1/.test(location.hash)) app.link.apply(location.hash); });
  window.addEventListener('hashchange', () => { if (/v=1/.test(location.hash) && app.ready) app.link.apply(location.hash); });
  return shell;
}
