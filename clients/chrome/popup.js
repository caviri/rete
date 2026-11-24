const input = document.getElementById("url");
const viewer = () => chrome.runtime.getURL("viewer.html");

function openViewer(src, name) {
  src = (src || "").trim();
  if (!/^https?:\/\//i.test(src)) { input.focus(); input.setCustomValidity("Needs an http(s) URL"); input.reportValidity(); return; }
  src = src.replace("/blob/", "/resolve/"); // HF blob pages are HTML; the bytes live under resolve/
  name = name || decodeURIComponent(new URL(src).pathname.split("/").pop() || "graph.rete");
  chrome.tabs.create({ url: viewer() + "?" + new URLSearchParams({ src, name, host: "url" }) });
  window.close();
}

// read the active page's structured data (the toolbar click granted activeTab for this tab)
const readBtn = document.getElementById("readpage");
chrome.tabs.query({ active: true, currentWindow: true }).then(([tab]) => {
  if (!tab || (tab.url && !/^https?:\/\//i.test(tab.url))) {
    readBtn.disabled = true;
    document.getElementById("readhint").textContent = "Only web pages (http and https) can be read; browser pages and the extension store cannot.";
    return;
  }
  readBtn.onclick = async () => {
    readBtn.disabled = true;
    readBtn.textContent = "Reading…";
    await chrome.runtime.sendMessage({ retelens: "readTab", tabId: tab.id }).catch(() => {});
    window.close();
  };
});
document.getElementById("open").onclick = () => openViewer(input.value);
// the default example (the same as on the viewer's start page)
document.getElementById("example").onclick = (e) => { e.preventDefault(); openViewer("https://huggingface.co/datasets/katospiegel/rete-graphs/resolve/main/antequera/antequera-2026-11-24.rete", "antequera-2026-11-24.rete"); };
// the file picker lives on the viewer's landing page (a picker opened in a popup would close it)
document.getElementById("local").onclick = () => { chrome.tabs.create({ url: viewer() + "?pick=1" }); window.close(); };
input.addEventListener("input", () => input.setCustomValidity(""));
input.addEventListener("keydown", (e) => { if (e.key === "Enter") openViewer(input.value); });

// recently opened URLs (title + URL only, kept in chrome.storage.local by the viewer)
chrome.storage.local.get("recents").then(({ recents }) => {
  const ul = document.getElementById("recent");
  const list = Array.isArray(recents) ? recents.slice(0, 8) : [];
  document.getElementById("rh").hidden = !list.length;
  for (const r of list) {
    if (!/^https?:\/\//i.test(r.url)) continue;
    const li = document.createElement("li");
    const b = document.createElement("button");
    b.type = "button";
    const t = document.createElement("div"); t.className = "t"; t.textContent = r.title || r.url;
    const u = document.createElement("div"); u.className = "u"; u.textContent = r.url;
    b.append(t, u);
    b.onclick = () => openViewer(r.url, r.title);
    li.append(b); ul.append(li);
  }
});

// setting: open .rete links in the viewer instead of downloading (on by default)
const box = document.getElementById("redirect");
chrome.storage.local.get(["viewInsteadOfDownload", "previewHosts"]).then((st) => {
  box.checked = st.viewInsteadOfDownload !== false;
  renderHosts(Array.isArray(st.previewHosts) ? st.previewHosts : []);
});
box.addEventListener("change", () => chrome.storage.local.set({ viewInsteadOfDownload: box.checked }));
function renderHosts(list) {
  const ul = document.getElementById("hosts");
  ul.textContent = "";
  document.getElementById("hh").hidden = !list.length;
  for (const host of list) {
    const li = document.createElement("li");
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = host + "  ×";
    b.title = "Stop previewing .rete files from " + host;
    b.onclick = async () => {
      const next = list.filter((x) => x !== host);
      await chrome.storage.local.set({ previewHosts: next });
      try { await chrome.permissions.remove({ origins: ["https://" + host + "/*"] }); } catch (_) {}
      renderHosts(next);
    };
    li.append(b); ul.append(li);
  }
}

// optional "every website" mode: runtime permission for all sites + one generic redirect rule (in the service worker)
const all = document.getElementById("allsites");
chrome.storage.local.get("allSites").then((st) => { all.checked = st.allSites === true; });
all.addEventListener("change", async () => {
  if (all.checked) {
    let ok = false;
    try { ok = await chrome.permissions.request({ origins: ["https://*/*", "http://*/*"] }); } catch (_) {}
    all.checked = ok;
    await chrome.storage.local.set({ allSites: ok });
  } else {
    await chrome.storage.local.set({ allSites: false });
    try { await chrome.permissions.remove({ origins: ["https://*/*", "http://*/*"] }); } catch (_) {}
  }
});

// ------------------------------------------------------------------ turn the inline viewer off / on (rules in storage.local, offRules)
const RULES = "offRules";
const loadRules = async () => { const st = await chrome.storage.local.get(RULES); return Array.isArray(st[RULES]) ? st[RULES] : []; };
const saveRules = (list) => chrome.storage.local.set({ [RULES]: list });
async function activeState() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab) return null;
  const r = await chrome.runtime.sendMessage({ retelens: "tabState", tabId: tab.id }).catch(() => null);
  const st = r && r.state;
  return st && tab.url && st.url && tab.url.split("#")[0] === st.url.split("#")[0] ? st : null;
}
async function renderHere() {
  const st = await activeState();
  document.getElementById("here").hidden = !st;
  document.getElementById("nothere").hidden = !!st;
  if (!st) return;
  const noun = st.keys.repoNoun;
  const rows = [["page", "Show the viewer on this page"], ["repo", `Show the viewer on this ${noun}`], ["site", `Show the viewer on this site (${st.host})`]];
  const rules = await loadRules();
  for (const [kind, text] of rows) {
    const box = document.getElementById("t-" + kind);
    document.getElementById("l-" + kind).textContent = text;
    const value = st.keys[kind];
    box.disabled = !value;
    box.parentElement.hidden = !value;
    box.checked = !rules.includes(`${kind}|${value}`);
    box.onchange = async () => {
      const rule = `${kind}|${value}`;
      const cur = (await loadRules()).filter((r) => r !== rule);
      if (!box.checked) cur.push(rule);
      await saveRules(cur);
      renderOffs();
    };
  }
}
async function renderOffs() {
  const rules = await loadRules();
  const ul = document.getElementById("offs");
  ul.textContent = "";
  document.getElementById("oh").hidden = !rules.length;
  for (const rule of rules) {
    const [kind, ...rest] = rule.split("|");
    const value = rest.join("|");
    const li = document.createElement("li");
    const t = document.createElement("span");
    const noun = kind !== "repo" ? kind : value.startsWith("zenodo.org/") ? "record" : value.includes("/buckets/") ? "bucket" : value.includes("/datasets/") ? "dataset" : "repository";
    t.textContent = `${noun}: ${value}`;
    t.title = value;
    const b = document.createElement("button");
    b.type = "button"; b.textContent = "Remove"; b.title = "Turn the viewer on again here";
    b.onclick = async () => { await saveRules((await loadRules()).filter((r) => r !== rule)); renderOffs(); renderHere(); };
    li.append(t, b); ul.append(li);
  }
}
renderHere(); renderOffs();

// ------------------------------------------------------------------ advanced: read chunk size per host, cache
const MIB = 1048576;
const zsel = document.getElementById("chunk-zenodo"), osel = document.getElementById("chunk-other");
const toSel = (v) => (v === undefined ? "" : v === 0 ? "auto" : String(v / MIB));
async function loadChunks() { const p = (await ReteCache.getSetting("chunkPolicy").catch(() => null)) || {}; zsel.value = toSel(p["zenodo.org"]); osel.value = toSel(p["*"]); }
async function saveChunks() {
  const p = {};
  for (const [key, sel] of [["zenodo.org", zsel], ["*", osel]]) if (sel.value !== "") p[key] = sel.value === "auto" ? 0 : Number(sel.value) * MIB;
  await ReteCache.setSetting("chunkPolicy", p);
}
zsel.onchange = osel.onchange = saveChunks;
async function showUsage() {
  const u = await ReteCache.usage().catch(() => null);
  document.getElementById("usage").textContent = u ? `${(u.bytes / 1e6).toFixed(1)} MB in ${u.chunks} chunks (cap 1 GiB)` : "";
}
document.getElementById("clear").onclick = async () => { await ReteCache.clear(); showUsage(); };
loadChunks(); showUsage();

// basemap under maps: a PMTiles URL (lib/maps.js reads basemapUrl; empty = the default open archive)
const baseIn = document.getElementById("basemap");
chrome.storage.local.get("basemapUrl").then((st) => { baseIn.value = typeof st.basemapUrl === "string" ? st.basemapUrl : ""; });
baseIn.addEventListener("change", async () => {
  const v = baseIn.value.trim();
  if (v && !/^https?:\/\/\S+$/i.test(v)) { baseIn.setCustomValidity("Needs an http(s) URL"); baseIn.reportValidity(); return; }
  baseIn.setCustomValidity("");
  if (v) await chrome.storage.local.set({ basemapUrl: v }); else await chrome.storage.local.remove("basemapUrl");
});
