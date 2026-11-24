// Service worker.
//  1. injects graphkit.js into a tab's content-script world on request (the Graph view)
//  2. context menu: "Preview with rete" on .rete links, "Open a .rete file..." on the action icon
//  3. declarativeNetRequest: top-level navigations to a .rete file (GitHub raw, raw.githubusercontent.com, HF resolve,
//     Zenodo files, and hosts the user allowed) open the viewer in the same tab instead of downloading.
//     main_frame only; URLs with download=true|1 (the hosts' own Download buttons) are never redirected.
//  4. downloads: the viewer's Download button (chrome.downloads, so the redirect rules do not apply)
//  5. "rete viewer" context menu on the supported hosts: turn the inline viewer off/on for a page, a repository /
//     bucket / record, or a site. Rules live in chrome.storage.local (offRules); the content scripts react live.
//  6. the .rete link scanner for hosts the user granted (registered as a dynamic content script)
const VIEWER = () => chrome.runtime.getURL("viewer.html");
const SETTING = "viewInsteadOfDownload"; // default on
const HOSTS_KEY = "previewHosts";        // hosts the user allowed: ["example.org", ...]
const ALL_KEY = "allSites";              // optional "every website" mode (runtime permission granted)

// ------------------------------------------------------------------ context menu
const HOST_PATTERNS = ["https://huggingface.co/*", "https://github.com/*", "https://zenodo.org/*"];
const RULES_KEY = "offRules";
// [kind, on?]  kind: page | repo | site
const SWITCHES = [["page", false], ["repo", false], ["site", false], ["page", true], ["repo", true], ["site", true]];
const switchId = (kind, on) => `retelens-${on ? "on" : "off"}-${kind}`;

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    const ctxs = ["page", "selection", "link", "image", "frame"];
    chrome.contextMenus.create({ id: "retelens-root", title: "rete viewer", contexts: ctxs, documentUrlPatterns: HOST_PATTERNS, visible: false });
    for (const [kind, on] of SWITCHES) {
      chrome.contextMenus.create({ id: switchId(kind, on), parentId: "retelens-root", title: switchTitle(kind, on, "repository"), contexts: ctxs, documentUrlPatterns: HOST_PATTERNS, visible: false });
    }
    chrome.contextMenus.create({
      id: "retelens-preview-link", title: "Preview with rete", contexts: ["link"],
      targetUrlPatterns: ["*://*/*.rete", "*://*/*.rete?*"],
    });
    chrome.contextMenus.create({ id: "retelens-open-file", title: "Open a .rete file…", contexts: ["action"] });
  });
  syncRules();
  syncScanners();
});
chrome.runtime.onStartup.addListener(() => { syncRules(); syncScanners(); });

// exposed on globalThis so tests can call it (native context menus cannot be clicked from automation)
globalThis.retelensMenuClick = (info, tab) => {
  if (typeof info.menuItemId === "string" && /^retelens-(on|off)-(page|repo|site)$/.test(info.menuItemId)) return switchRule(info.menuItemId, tab);
  if (info.menuItemId === "retelens-preview-link" && info.linkUrl && /^https?:\/\//i.test(info.linkUrl)) {
    const name = decodeURIComponent(new URL(info.linkUrl).pathname.split("/").pop() || "graph.rete");
    return chrome.tabs.create({ url: VIEWER() + "?" + new URLSearchParams({ src: info.linkUrl, name, host: "url" }) });
  }
  if (info.menuItemId === "retelens-open-file") return chrome.tabs.create({ url: VIEWER() });
};
chrome.contextMenus.onClicked.addListener((info, tab) => globalThis.retelensMenuClick(info, tab));

// ------------------------------------------------------------------ turn off / on (per tab state from the content scripts)
// Chrome has no "menu shown" event: the content script reports its state (page/repo/site keys and which rules match) when
// it loads, when the URL changes and when the rules change; the menu items are updated from that on tab activation / update.
function switchTitle(kind, on, noun) {
  const what = kind === "page" ? "this page" : kind === "site" ? "this site" : "this " + noun;
  return on ? `Turn on again on ${what}` : `Turn off on ${what}`;
}
const tabKey = (id) => "tab" + id;
async function getTabState(id) { const s = await chrome.storage.session.get(tabKey(id)); return s[tabKey(id)] || null; }
async function refreshMenu(tabId) {
  let tab = null;
  try { tab = await chrome.tabs.get(tabId); } catch (_) { return; }
  if (!tab.active) return;
  const st = await getTabState(tabId);
  const url = tab.url || "";
  const fresh = !!(st && st.url && url.split("#")[0] === st.url.split("#")[0]);
  const vis = (kind, on) => {
    if (!fresh) return false;
    if (kind === "repo" && !st.keys.repo) return false;
    return on ? !!st.off[kind] : !st.off[kind];
  };
  const any = SWITCHES.some(([k, on]) => vis(k, on));
  const upd = (id, p) => chrome.contextMenus.update(id, p).catch(() => {});
  await upd("retelens-root", { visible: any });
  for (const [kind, on] of SWITCHES) await upd(switchId(kind, on), { visible: vis(kind, on), title: switchTitle(kind, on, fresh ? st.keys.repoNoun : "repository") });
}
async function switchRule(menuItemId, tab) {
  const m = menuItemId.match(/^retelens-(on|off)-(page|repo|site)$/);
  const st = tab && (await getTabState(tab.id));
  if (!m || !st) return;
  const value = st.keys[m[2]];
  if (!value) return;
  const rule = `${m[2]}|${value}`;
  const { [RULES_KEY]: cur } = await chrome.storage.local.get(RULES_KEY);
  const list = (Array.isArray(cur) ? cur : []).filter((r) => r !== rule);
  if (m[1] === "off") list.push(rule);
  await chrome.storage.local.set({ [RULES_KEY]: list });
}
chrome.tabs.onActivated.addListener(({ tabId }) => refreshMenu(tabId));
chrome.tabs.onUpdated.addListener((tabId, change) => { if (change.url || change.status === "loading") refreshMenu(tabId); });
chrome.tabs.onRemoved.addListener((tabId) => chrome.storage.session.remove(tabKey(tabId)).catch(() => {}));

// ------------------------------------------------------------------ redirect rules
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
// [urlRegex (group 1 = the URL to preview), human label]
const BUILTIN = [
  String.raw`^(https://github\.com/[^/]+/[^/?#]+/raw/[^?#]+\.rete)(?:\?.*)?$`,
  String.raw`^(https://raw\.githubusercontent\.com/[^?#]+\.rete)(?:\?.*)?$`,
  String.raw`^(https://huggingface\.co/(?:datasets|buckets)/[^?#]+/resolve/[^?#]+\.rete)(?:\?.*)?$`,
  String.raw`^(https://zenodo\.org/records/[0-9]+/files/[^?#]+\.rete)(?:\?.*)?$`,
];
async function syncRules() {
  const store = await chrome.storage.local.get([SETTING, HOSTS_KEY, ALL_KEY]);
  const on = store[SETTING] !== false;
  const hosts = Array.isArray(store[HOSTS_KEY]) ? store[HOSTS_KEY] : [];
  const regexes = [...BUILTIN, ...hosts.map((h) => `^(https://${esc(h)}/[^?#]+\\.rete)(?:\\?.*)?$`)];
  if (store[ALL_KEY] === true) regexes.push(String.raw`^(https?://[^?#]*\.rete)(?:\?[^#]*)?$`);
  const rules = [];
  let id = 1;
  if (on) {
    // explicit download intent beats every redirect rule
    rules.push({ id: id++, priority: 10, action: { type: "allow" }, condition: { regexFilter: String.raw`[?&]download=(?:true|1)(?:&|$)`, resourceTypes: ["main_frame"], isUrlFilterCaseSensitive: false } });
    for (const rx of regexes) {
      rules.push({ id: id++, priority: 1, action: { type: "redirect", redirect: { regexSubstitution: `${VIEWER()}?host=url&src=\\1` } }, condition: { regexFilter: rx, resourceTypes: ["main_frame"], isUrlFilterCaseSensitive: false } });
    }
  }
  const old = (await chrome.declarativeNetRequest.getDynamicRules()).map((r) => r.id);
  await chrome.declarativeNetRequest.updateDynamicRules({ removeRuleIds: old, addRules: rules });
}
chrome.storage.onChanged.addListener((ch, area) => { if (area === "local" && (SETTING in ch || HOSTS_KEY in ch || ALL_KEY in ch)) { syncRules(); syncScanners(); } });
chrome.permissions.onAdded.addListener(() => syncScanners());
chrome.permissions.onRemoved.addListener(() => syncScanners());

// ------------------------------------------------------------------ scanner on granted hosts
// The three built-in hosts have their own content scripts. For every other host the user granted (per-host prompt or
// "every website"), the same lightweight scanner is registered as a dynamic content script: no <all_urls> at install.
const SCAN_ID = "retelens-scan";
const BUILTIN_MATCHES = ["https://huggingface.co/*", "https://*.huggingface.co/*", "https://github.com/*", "https://zenodo.org/*"];
async function syncScanners() {
  try {
    const st = await chrome.storage.local.get([HOSTS_KEY, ALL_KEY]);
    let matches = [];
    if (st[ALL_KEY] === true && (await chrome.permissions.contains({ origins: ["https://*/*", "http://*/*"] }))) matches = ["https://*/*", "http://*/*"];
    else for (const h of Array.isArray(st[HOSTS_KEY]) ? st[HOSTS_KEY] : []) { const o = `https://${h}/*`; if (await chrome.permissions.contains({ origins: [o] })) matches.push(o); }
    const have = await chrome.scripting.getRegisteredContentScripts({ ids: [SCAN_ID] });
    if (have.length) await chrome.scripting.unregisterContentScripts({ ids: [SCAN_ID] });
    if (!matches.length) return;
    await chrome.scripting.registerContentScripts([{ id: SCAN_ID, matches, excludeMatches: BUILTIN_MATCHES, js: ["content/scanner.js", "content/scan-any.js"], runAt: "document_idle", persistAcrossSessions: true }]);
  } catch (e) { console.warn("[retelens] scanner registration failed", e); }
}
globalThis.retelensSyncScanners = syncScanners;

// ------------------------------------------------------------------ messages
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (msg?.retelens === "pageState" && sender.tab) {
    chrome.storage.session.set({ [tabKey(sender.tab.id)]: msg }).then(() => refreshMenu(sender.tab.id));
    return false;
  }
  if (msg?.retelens === "openViewer" && sender.tab && typeof msg.src === "string" && /^https?:\/\//i.test(msg.src)) {
    // from the scanner badge on a page the user granted: the viewer opens next to the tab
    chrome.tabs.create({ url: VIEWER() + "?" + new URLSearchParams({ src: msg.src, name: String(msg.name || "").slice(0, 200), host: "url" }), index: sender.tab.index + 1, openerTabId: sender.tab.id });
    return false;
  }
  if (msg?.retelens === "tabState" && typeof msg.tabId === "number") {
    getTabState(msg.tabId).then((st) => sendResponse({ state: st }));
    return true;
  }
  if (msg?.retelens === "loadGraphKit" && sender.tab) {
    // the Graph view: graphkit.js (1.4 MB) is injected into the content-script world when first needed, not on every page
    chrome.scripting
      .executeScript({ target: { tabId: sender.tab.id, frameIds: [sender.frameId || 0] }, files: ["lib/graphkit/graphkit.js"], world: "ISOLATED" })
      .then(() => sendResponse({ ok: true }))
      .catch((e) => sendResponse({ ok: false, error: String(e) }));
    return true;
  }
  if (msg?.retelens === "download" && typeof msg.url === "string" && /^https?:\/\//i.test(msg.url)) {
    // the URL is requested with download=1 so a redirect rule can never turn it back into a preview
    const u = new URL(msg.url);
    if (!u.searchParams.has("download")) u.searchParams.set("download", "1");
    const filename = decodeURIComponent(u.pathname.split("/").pop() || "graph.rete").replace(/[\\/:*?"<>|]/g, "_");
    chrome.downloads.download({ url: u.href, filename, saveAs: false }).then((id) => sendResponse({ ok: true, id }), (e) => sendResponse({ ok: false, error: String(e) }));
    return true;
  }
  if (msg?.retelens === "allowHost" && typeof msg.host === "string" && /^[a-z0-9.-]+$/i.test(msg.host)) {
    chrome.storage.local.get(HOSTS_KEY).then(({ previewHosts }) => {
      const list = Array.isArray(previewHosts) ? previewHosts : [];
      if (!list.includes(msg.host)) list.push(msg.host);
      return chrome.storage.local.set({ [HOSTS_KEY]: list });
    }).then(() => sendResponse({ ok: true }));
    return true;
  }
});

// ------------------------------------------------------------------ downloads fallback (every host, from install)
// A .rete download we did not start ourselves is cancelled and erased (nothing lands on disk or stays in the list)
// and the file opens in the viewer instead. Explicit download intent and our own downloads are never touched.
const INTENT = /[?&](?:download=(?:true|1)|dl=1)(?:&|$)/i;
const isReteName = (n) => typeof n === "string" && /\.rete$/i.test(n.split(/[?#]/)[0]);
const pathEndsRete = (u) => { try { return isReteName(new URL(u).pathname); } catch (_) { return false; } };
async function interceptable(item) {
  const st = await chrome.storage.local.get(SETTING);
  if (st[SETTING] === false) return false;
  if (item.byExtensionId === chrome.runtime.id) return false;                 // started by our Download button
  if (INTENT.test(item.url || "") || INTENT.test(item.finalUrl || "")) return false;
  return pathEndsRete(item.url) || pathEndsRete(item.finalUrl) || isReteName(item.filename);
}
async function openInViewer(src) {
  const viewer = VIEWER() + "?" + new URLSearchParams({ src, host: "url" });
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  // same tab when it is blank, a new tab, or still on the .rete URL (typed or pasted); otherwise a new tab next to it
  const url = tab && (tab.url || tab.pendingUrl);
  if (tab && (!url || url === "about:blank" || /^chrome:\/\/newtab/.test(url) || url === src)) { if (url) return chrome.tabs.update(tab.id, { url: viewer }); }
  return chrome.tabs.create({ url: viewer, index: tab ? tab.index + 1 : undefined, openerTabId: tab ? tab.id : undefined, active: true });
}
// Every step is logged (console + globalThis.__retelensDl, last 60) so a test can read it from the service worker.
const dlLog = (kind, item, extra) => {
  const row = { t: Date.now(), kind, id: item && item.id, url: item && item.url, finalUrl: item && item.finalUrl, filename: item && item.filename, state: item && item.state, by: item && item.byExtensionId, ...extra };
  (globalThis.__retelensDl = globalThis.__retelensDl || []).push(row);
  if (globalThis.__retelensDl.length > 60) globalThis.__retelensDl.shift();
  console.debug("[retelens] download", row);
};
chrome.downloads.onCreated.addListener((item) => dlLog("created", item));
chrome.downloads.onChanged.addListener((d) => { if (d.state || d.error) dlLog("changed", { id: d.id }, { state: d.state && d.state.current, error: d.error && d.error.current }); });
// The download is HELD in "determining filename" (return true) until we have decided. The old code called suggest()
// at once and cancelled later, which lost the race for small files: a 200 KB file was already on disk when
// the cancel arrived.
chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  dlLog("determining", item);
  let answered = false;
  const answer = () => { if (!answered) { answered = true; try { suggest(); } catch (_) {} } };
  interceptable(item).then(async (yes) => {
    if (!yes) return answer();
    const src = item.finalUrl || item.url;
    dlLog("intercept", item, { src });
    try { await chrome.downloads.cancel(item.id); } catch (e) { dlLog("cancel-failed", item, { error: String(e) }); }
    try { await chrome.downloads.erase({ id: item.id }); } catch (_) {}
    answer();
    await openInViewer(src);
  }).catch((e) => { dlLog("error", item, { error: String(e) }); answer(); });
  return true;
});
