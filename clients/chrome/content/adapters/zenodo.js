// Zenodo adapter (InvenioRDM 15 on Semantic UI). Contract: wacz-zenodo-eval/REPORT.md + zenodo-dom.json.
//
// On a record page with .rete files:
//   - a Preview button (ui compact mini button, eye icon) before Download on each .rete row of #file-list-table;
//     Zenodo wires its own preview handler at load to buttons that exist then, so ours opens our panel itself
//   - a preview panel shaped like Zenodo's accordion right after h2#files-heading, open by default, with the shared
//     Explorer in it; ?preview_file=<name> is kept with replaceState like Zenodo does
//   - ?studio=1: the Explorer full width under Zenodo's header (back control, breadcrumb)
//   - a "Graph card" box in the sidebar after Rights, a header label "rete 0x07 · 119.9 M statements"
//   - attribution merges the file's card with the record metadata in #recordVersions[data-record] (no request)
//   - a link/filename scanner (content/scanner.js) as a safety net
// The engine reads /records/<id>/files/<name>?download=1 (Content-Range is readable with the zenodo.org host permission).
//
// Untrusted: card fields, record metadata and file names reach the DOM only through text nodes / h() (http(s) hrefs only).
(function () {
  const list = (self.__retelensAdapters = self.__retelensAdapters || []);
  const { h, safeUrl, num, bytes } = window.RetelensFmt;
  const { Explorer, Session } = window.RetelensExplorer;

  const RE_REC = /^\/records?\/(\d+)\/?$/;
  const ORCID = /^(?:https?:\/\/orcid\.org\/)?(\d{4}-\d{4}-\d{4}-\d{3}[\dX])$/;
  const isRete = (n) => /\.rete$/i.test(String(n).split(/[?#]/)[0]);
  const fileUrl = (id, name) => `${location.origin}/records/${id}/files/${encodeURIComponent(name)}?download=1`;
  const compact = (n) => (n >= 1e9 ? (n / 1e9).toFixed(1) + " B" : n >= 1e6 ? (n / 1e6).toFixed(1) + " M" : n >= 1e3 ? (n / 1e3).toFixed(1) + " k" : String(n));

  // ---------------------------------------------------------------- record metadata (data-record on #recordVersions)
  function readRecord() {
    const el = document.getElementById("recordVersions");
    let raw = null;
    try { raw = JSON.parse(el.getAttribute("data-record")); } catch (_) {}
    return raw && typeof raw === "object" ? normRecord(raw) : null;
  }
  const str = (v) => (typeof v === "string" ? v : v && typeof v === "object" ? v.en || Object.values(v).find((x) => typeof x === "string") || "" : "");
  function normRecord(r) {
    const md = r.metadata || {};
    const out = { id: String(r.id || r.recid || ""), concept: null, doi: null, conceptDoi: null, version: null, date: null, license: null, creators: [], files: [], title: str(md.title) };
    out.concept = (r.parent && r.parent.id) || r.conceptrecid || null;
    out.doi = (r.pids && r.pids.doi && r.pids.doi.identifier) || r.doi || md.doi || null;
    out.conceptDoi = (r.parent && r.parent.pids && r.parent.pids.doi && r.parent.pids.doi.identifier) || r.conceptdoi || null;
    const idx = r.versions && r.versions.index;
    out.version = md.version || (r.ui && r.ui.version) || (idx ? "v" + idx : null);
    out.date = md.publication_date || null;
    const rights = (r.ui && r.ui.rights && r.ui.rights[0]) || (md.rights && md.rights[0]) || null;
    if (rights) out.license = { id: rights.id || null, title: str(rights.title_l10n || rights.title) || rights.id || null, url: (rights.props && rights.props.url) || rights.link || null };
    else if (md.license && md.license.id) out.license = { id: md.license.id, title: md.license.id, url: null };
    for (const c of md.creators || []) {
      const p = c.person_or_org;
      const name = p ? p.name : c.name;
      if (!name) continue;
      const oc = p ? ((p.identifiers || []).find((i) => i.scheme === "orcid") || {}).identifier : c.orcid;
      const om = typeof oc === "string" ? oc.match(ORCID) : null;
      const aff = p ? (c.affiliations || []).map((a) => a.name).filter(Boolean).join("; ") : c.affiliation;
      out.creators.push({ name: String(name), orcid: om ? om[1] : null, affil: aff ? String(aff) : "" });
    }
    const ent = r.files && r.files.entries;
    if (ent && typeof ent === "object") for (const [k, v] of Object.entries(ent)) out.files.push({ name: v.key || k, size: v.size });
    else if (Array.isArray(r.files)) for (const f of r.files) out.files.push({ name: f.key, size: f.size });
    return out;
  }

  // ---------------------------------------------------------------- normalising for the merge
  const canonName = (n) => String(n || "").toLowerCase().normalize("NFKD").replace(/[̀-ͯ]/g, "").replace(/[^a-z0-9]+/g, " ").trim().split(" ").filter(Boolean).sort().join(" ");
  // Zenodo legacy ids (cc-zero, cc-by-4.0) and SPDX ids (CC0-1.0, CC-BY-4.0) meet here
  function canonLicense(s) {
    let t = String(s || "").toLowerCase().replace(/^https?:\/\/.*?\/licenses?\//, "").replace(/[^a-z0-9]+/g, "");
    if (/^cc(zero|0)/.test(t)) return "cc0";
    t = t.replace(/(\d)0$/, "$1").replace(/(\d)$/, "$1"); // by-4.0 -> by4
    return t.replace(/^creativecommons/, "cc");
  }
  const canonDoi = (d) => String(d || "").toLowerCase().replace(/^https?:\/\/(dx\.)?doi\.org\//, "").replace(/^doi:/, "").trim();
  const canonVer = (v) => String(v || "").toLowerCase().replace(/^v/, "").trim();
  const arr = (v) => (v == null ? [] : Array.isArray(v) ? v : [v]);
  function cardPeople(card) {
    return [...arr(card.creators), ...arr(card.creator), ...arr(card.authors), ...arr(card.author)].map((p) => {
      if (typeof p === "string") { const m = p.match(ORCID); return m ? { name: p, orcid: m[1], affil: "" } : { name: p, orcid: null, affil: "" }; }
      if (!p || typeof p !== "object") return null;
      const name = p.name || p.fullName || p.label || [p.givenName, p.familyName].filter(Boolean).join(" ");
      if (!name) return null;
      const oc = [p.orcid, p.id, p.identifier, p["@id"]].map((x) => (typeof x === "string" ? x.match(ORCID) : null)).find(Boolean);
      return { name: String(name), orcid: oc ? oc[1] : null, affil: typeof p.affiliation === "string" ? p.affiliation : p.affiliation && p.affiliation.name ? String(p.affiliation.name) : "" };
    }).filter(Boolean);
  }
  const sameSet = (a, b) => a.length === b.length && a.every((x) => b.includes(x));

  // ---------------------------------------------------------------- the sidebar box
  const FROM_CARD = "from the file's card", FROM_ZEN = "from Zenodo";
  function person(p) {
    const w = h("div", null, h("span", null, p.name));
    if (p.orcid) w.append(" ", h("a", { href: "https://orcid.org/" + p.orcid, title: "ORCID " + p.orcid }, "ORCID"));
    if (p.affil) w.append(h("div", { class: "text-muted font-tiny" }, p.affil));
    return w;
  }
  const dtdd = (label, ...nodes) => (nodes.length && nodes.every((n) => n != null) ? [h("dt", { class: "ui tiny header" }, label), h("dd", null, ...nodes)] : []);
  // two sources of one fact: equal -> once; different -> both, labelled
  function twoWay(label, cardNode, cardKey, zenNode, zenKey, merged) {
    if (cardNode == null && zenNode == null) return [];
    if (cardNode == null || zenNode == null) return dtdd(label, cardNode ?? zenNode);
    if (cardKey === zenKey) return dtdd(label, merged || zenNode);
    return dtdd(label, h("div", null, h("small", { class: "text-muted" }, FROM_CARD + ": "), cardNode), h("div", null, h("small", { class: "text-muted" }, FROM_ZEN + ": "), zenNode));
  }
  const linkOrText = (u, text) => (safeUrl(u) ? h("a", { href: u }, text || u.replace(/^https?:\/\//, "")) : h("span", null, String(text || u)));

  function renderBox(body, data, rec) {
    const card = data.card || null;
    body.textContent = "";
    const dl = h("dl", { class: "details-list" });
    const add = (nodes) => { for (const n of nodes) dl.append(n); };
    if (card && card.title && (!rec || card.title !== rec.title)) add(dtdd("Title", h("span", null, String(card.title))));

    // creators
    const zen = rec ? rec.creators : [], cd = card ? cardPeople(card) : [];
    if (zen.length || cd.length) {
      const list = (ps) => h("div", null, ps.map(person));
      if (zen.length && cd.length && sameSet(zen.map((p) => canonName(p.name)), cd.map((p) => canonName(p.name)))) {
        const merged = zen.map((p) => ({ ...p, orcid: p.orcid || (cd.find((c) => canonName(c.name) === canonName(p.name)) || {}).orcid || null, affil: p.affil || (cd.find((c) => canonName(c.name) === canonName(p.name)) || {}).affil || "" }));
        add(dtdd("Creators", list(merged)));
      } else if (zen.length && cd.length) {
        add(dtdd("Creators", h("div", null, h("small", { class: "text-muted" }, FROM_CARD + ":"), list(cd)), h("div", { class: "mt-5" }, h("small", { class: "text-muted" }, FROM_ZEN + ":"), list(zen))));
      } else add(dtdd("Creators", list(zen.length ? zen : cd)));
    }

    // DOI, concept DOI
    const cardDoi = card && card.doi ? canonDoi(card.doi) : null;
    const doiNode = (d) => (/^10\.\d{4,}\/\S+$/.test(d) ? h("a", { href: "https://doi.org/" + d }, d) : h("span", null, d));
    add(twoWay("DOI", cardDoi ? doiNode(cardDoi) : null, cardDoi, rec && rec.doi ? doiNode(canonDoi(rec.doi)) : null, rec && rec.doi ? canonDoi(rec.doi) : null));
    if (rec && rec.conceptDoi) add(dtdd("Concept DOI", doiNode(canonDoi(rec.conceptDoi)), h("div", { class: "text-muted font-tiny" }, "all versions")));

    // version, dates
    add(twoWay("Version", card && card.version ? h("span", null, String(card.version)) : null, card && canonVer(card.version), rec && rec.version ? h("span", null, String(rec.version)) : null, rec && canonVer(rec.version)));
    if (rec && rec.date) add(dtdd("Published", h("span", null, rec.date), h("small", { class: "text-muted" }, " on Zenodo")));
    if (card && card.source_date) add(dtdd("Source date", h("span", null, String(card.source_date)), h("small", { class: "text-muted" }, " in the file's card")));

    // licence
    const cl = card && card.license ? (typeof card.license === "string" ? card.license : card.license.id || card.license.name || card.license.url || "") : "";
    const zl = rec && rec.license;
    add(twoWay("Licence",
      cl ? window.RetelensCardSidebar.licenseNode(card.license) : null, cl ? canonLicense(cl) : null,
      zl ? (zl.url && safeUrl(zl.url) ? h("a", { href: zl.url }, zl.title || zl.id) : h("span", null, zl.title || zl.id)) : null, zl ? canonLicense(zl.id || zl.title) : null));

    if (card && card.source) add(dtdd("Source", linkOrText(String(card.source))));
    if (card && card.homepage) add(dtdd("Homepage", linkOrText(String(card.homepage))));
    const kw = card ? arr(card.keywords).map(String).filter(Boolean).slice(0, 8) : [];
    if (kw.length) add(dtdd("Keywords", h("div", null, kw.map((k) => h("span", { class: "ui tiny label mb-5" }, k)))));
    body.append(dl);

    // statistics
    const info = data.info || {}, c = card || {};
    const quads = info.quads ?? c.quad_count ?? c.triple_count, terms = info.terms ?? c.term_count, ng = info.namedGraphs ?? c.named_graph_count;
    const rows = [
      ["Statements", quads != null ? num(quads) : null], ["Terms", terms != null ? num(terms) : null], ["Named graphs", ng != null ? num(ng) : null],
      ["File size", data.fileLength ? bytes(data.fileLength) : null], ["Format", data.format != null ? "rete 0x0" + Number(data.format).toString(16) : null],
      ["Example queries", data.exampleCount != null ? num(data.exampleCount) : null], ["Content hash", data.contentHash ? h("code", { class: "break-all text-xs" }, data.contentHash) : null],
    ].filter((r) => r[1] != null);
    if (rows.length) body.append(h("table", { class: "ui definition table fluid" }, h("tbody", null, rows.map(([k, v]) => h("tr", null, h("td", { class: "collapsing" }, k), h("td", null, v))))));
    if (!card) body.append(h("div", { class: "text-muted" }, "This file carries no embedded Dataset Card."));
  }

  // ---------------------------------------------------------------- adapter
  list.push({
    id: "zenodo",
    matches: (loc) => loc.hostname === "zenodo.org",
    _test: { renderBox, normRecord, canonLicense }, // test/zenodo-box.html exercises the merge on synthetic data

    // rule keys for "turn off on this record": the concept record covers every version
    keys(loc) {
      const m = loc.pathname.match(RE_REC);
      if (!m) return {};
      let concept = null;
      try { const r = readRecord(); concept = r && r.concept; } catch (_) {}
      return { repo: "zenodo.org/concept/" + (concept || m[1]), noun: "record" };
    },

    async mount(ctx) {
      const m = location.pathname.match(RE_REC);
      const rec = m ? readRecord() : null;
      const recId = m ? m[1] : null;
      let reteFiles = [];

      if (m) {
        const table = await ctx.waitFor("#file-list-table", 6000);
        if (!ctx.alive()) return;
        const seen = new Map();
        if (table) for (const a of table.querySelectorAll("tbody tr td.ten.wide a[href]")) {
          let u; try { u = new URL(a.getAttribute("href"), location.href); } catch (_) { continue; }
          const name = decodeURIComponent(u.pathname.split("/").pop() || "");
          if (isRete(name) && !seen.has(name)) seen.set(name, { name, size: null });
        }
        if (rec) for (const f of rec.files) if (isRete(f.name) && !seen.has(f.name)) seen.set(f.name, { name: f.name, size: f.size });
        reteFiles = [...seen.values()];
      }
      const names = new Set(reteFiles.map((f) => f.name));
      const rowHook = reteFiles.length ? setupRecord(ctx, recId, rec, reteFiles) : null;

      // the safety net: links and filenames anywhere on the page
      window.RetelensScanner.start({
        ctx,
        textRoots: "td, li, p",
        handledHit: (hit) => !!(rowHook && recId && names.has(hit.name) && hit.url === fileUrl(recId, hit.name)),
        nameLink({ url, name }) {
          const a = document.createElement("a");
          a.setAttribute("href", location.pathname + "?preview_file=" + encodeURIComponent(name));
          a.setAttribute("title", "Preview with rete");
          a.style.cssText = "text-decoration:underline dotted";
          a.addEventListener("click", (e) => { e.preventDefault(); rowHook.open(name, true); });
          return a;
        },
        handled: (a) => !!(rowHook && a.closest("#file-list-table")),
        linkUrl(a) {
          let u; try { u = new URL(a.getAttribute("href"), location.href); } catch (_) { return null; }
          const name = decodeURIComponent(u.pathname.split("/").pop());
          const mm = u.pathname.match(/^\/records?\/(\d+)\/files\//) || u.pathname.match(/^\/api\/records\/(\d+)\/files\//);
          if (u.origin === location.origin && mm) { u.searchParams.set("download", "1"); return { url: u.origin + u.pathname.replace(/\/content$/, "") + "?download=1", name }; }
          return { url: u.href, name };
        },
        nameUrl(tn, name) {
          const base = name.split("/").pop();
          if (recId && names.has(base) && !tn.parentElement.closest("#file-list-table") && name === base) return { url: fileUrl(recId, base), name: base };
          const row = tn.parentElement && tn.parentElement.closest("tr, li");
          const near = row && [...row.querySelectorAll("a[href]")].map((a) => { try { return new URL(a.getAttribute("href"), location.href); } catch (_) { return null; } }).find((u) => u && u.origin === location.origin && /^\/records?\/\d+/.test(u.pathname));
          const id = near && near.pathname.match(/^\/records?\/(\d+)/)[1];
          return id ? { url: fileUrl(id, base), name: base } : null;
        },
        affordance({ url, name }) {
          const a = document.createElement("a");
          a.className = "ui compact mini button ml-5";
          a.setAttribute("role", "button");
          a.setAttribute("href", "#");
          a.setAttribute("title", "Preview " + name + " with rete");
          const i = document.createElement("i"); i.className = "eye icon"; i.setAttribute("aria-hidden", "true");
          a.append(i, "Preview");
          a.addEventListener("click", (e) => {
            e.preventDefault();
            if (rowHook && names.has(name) && url === fileUrl(recId, name)) rowHook.open(name, true);
            else window.open(chrome.runtime.getURL("viewer.html") + "?" + new URLSearchParams({ src: url, name, host: "zenodo" }), "_blank", "noopener");
          });
          return a;
        },
      });
    },
  });

  // ---------------------------------------------------------------- a record with .rete files
  function setupRecord(ctx, recId, rec, reteFiles) {
    const files = reteFiles.map((f) => ({ path: f.name, src: fileUrl(recId, f.name), label: f.name, download: fileUrl(recId, f.name), size: f.size || null }));
    const session = new Session(ctx.engine(), files);
    const eng = ctx.engine();
    const qs = () => new URLSearchParams(location.search);
    const inStudio = () => qs().get("studio") === "1";
    // Explorer state in the URL: file -> ?preview_file= (Zenodo's own deep link), the rest as is
    const params = {
      get: (k) => qs().get(k === "file" ? "preview_file" : k),
      set(obj) {
        const u = new URL(location.href);
        for (const [k, v] of Object.entries(obj)) { const key = k === "file" ? "preview_file" : k; if (v == null) u.searchParams.delete(key); else u.searchParams.set(key, v); }
        history.replaceState(history.state, "", u);
      },
    };

    // ---- the preview panel (Zenodo's accordion markup, ids of our own)
    const panel = document.createElement("div");
    panel.className = "ui accordion panel mb-10 open";
    panel.setAttribute("data-retelens", "panel");
    const head = document.createElement("h3"); head.className = "active title panel-heading open m-0";
    const trigger = document.createElement("div");
    trigger.className = "trigger"; trigger.setAttribute("role", "button"); trigger.id = "retelens-preview-trigger"; trigger.tabIndex = 0;
    trigger.setAttribute("aria-controls", "retelens-preview-panel"); trigger.setAttribute("aria-expanded", "true"); trigger.setAttribute("aria-label", "Graph preview");
    const title = document.createElement("span"); title.id = "retelens-file-title"; title.textContent = files[0].label;
    const angle = document.createElement("i"); angle.className = "angle right icon"; angle.setAttribute("aria-hidden", "true");
    trigger.append(title, angle); head.append(trigger);
    const region = document.createElement("div");
    region.className = "active content preview-container pt-0 open"; region.id = "retelens-preview-panel";
    region.setAttribute("role", "region"); region.setAttribute("aria-labelledby", "retelens-preview-trigger");
    const inner = document.createElement("div");
    const note = h("div", { class: "rl-zen-note warn", hidden: true, role: "status" });
    region.append(inner); panel.append(head, region);
    const setOpen = (open) => {
      for (const el of [panel, head, region]) el.classList.toggle("open", open);
      head.classList.toggle("active", open); region.classList.toggle("active", open);
      trigger.setAttribute("aria-expanded", String(open));
    };
    const toggle = () => setOpen(!region.classList.contains("active"));
    ctx.on(trigger, "click", toggle);
    ctx.on(trigger, "keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); toggle(); } });

    const heading = document.getElementById("files-heading");
    const section = document.getElementById("record-files");
    if (heading) heading.after(panel); else if (section) section.prepend(panel);
    ctx.track(panel);

    // engine port: rate-limit notices from the worker
    ctx.on(eng.t, "message", (ev) => {
      const d = ev.data;
      if (d && d.id === -2) { note.textContent = d.notice || ""; note.hidden = !d.notice; }
    });

    const boxEx = new Explorer({ session, mode: "box", urlKeys: ["file", "view"], params, onStudio: (st) => goStudio(st), skin: "zenodo" });
    inner.append(boxEx.el, note);
    let studio = null, boxStarted = false;

    // ---- sidebar box + header label
    const licenses = document.getElementById("licenses");
    const anchor = licenses && licenses.closest(".sidebar-container");
    const side = document.querySelector("aside.sidebar");
    let boxBody = null;
    if (anchor || side) {
      const box = document.createElement("div");
      box.className = "sidebar-container";
      box.setAttribute("data-retelens", "card");
      const hd = document.createElement("h2"); hd.className = "ui medium top attached header mt-0"; hd.textContent = "Graph card";
      boxBody = document.createElement("div"); boxBody.className = "ui segment bottom attached rdm-sidebar"; boxBody.id = "retelens-card";
      box.append(hd, boxBody);
      if (anchor) anchor.after(box); else side.append(box);
      ctx.track(box);
    }
    const labelRow = document.querySelector("#record-info .right.floated.right.aligned.column");
    let label = null;
    if (labelRow) {
      label = document.createElement("span");
      label.className = "ui label horizontal small neutral mb-5";
      label.setAttribute("role", "note"); label.setAttribute("data-retelens", "label");
      label.textContent = "rete";
      labelRow.append(label);
      ctx.track(label);
    }
    session.onChange(() => {
      if (ctx.dead) return;
      const d = session.cardData();
      title.textContent = session.file ? session.file.label : "";
      if (boxBody) { try { renderBox(boxBody, d, rec); } catch (e) { console.debug("[retelens] sidebar", e); } }
      if (label) {
        const q = d.info && d.info.quads;
        label.textContent = ["rete" + (d.format != null ? " 0x0" + Number(d.format).toString(16) : ""), q != null ? compact(q) + " statements" : null].filter(Boolean).join(" · ");
      }
    });

    // ---- row buttons
    function hookRows() {
      const table = document.getElementById("file-list-table");
      if (!table) return;
      for (const tr of table.querySelectorAll("tbody tr")) {
        const a = tr.querySelector("td.ten.wide a[href]");
        if (!a) continue;
        let u; try { u = new URL(a.getAttribute("href"), location.href); } catch (_) { continue; }
        const name = decodeURIComponent(u.pathname.split("/").pop() || "");
        if (!isRete(name) || tr.querySelector("[data-retelens-row]")) continue;
        const holder = tr.querySelector("td.right.aligned > span") || tr.querySelector("td.right.aligned");
        if (!holder) continue;
        const btn = document.createElement("a");
        btn.className = "ui compact mini button";
        btn.setAttribute("role", "button");
        btn.setAttribute("href", location.pathname + "?preview_file=" + encodeURIComponent(name));
        btn.setAttribute("data-retelens", "row"); btn.setAttribute("data-retelens-row", "");
        btn.setAttribute("title", "Preview " + name + " with rete");
        const i = document.createElement("i"); i.className = "eye icon"; i.setAttribute("aria-hidden", "true");
        btn.append(i, "Preview");
        btn.addEventListener("click", (e) => { e.preventDefault(); open(name, true); });
        holder.prepend(btn); // before Download
        ctx.track(btn);
      }
    }
    hookRows();
    ctx.watch(document.getElementById("record-files") || document.body, hookRows, 200);

    // ---- opening a file in the panel (the Preview button)
    function open(name, scroll) {
      const i = files.findIndex((f) => f.path === name);
      if (i < 0) return;
      if (inStudio()) { const u = new URL(location.href); u.searchParams.delete("studio"); history.pushState(null, "", u); apply(); }
      setOpen(true);
      title.textContent = name;
      params.set({ file: name });
      if (session.idx !== i) boxEx.switchFile(i);
      if (scroll) panel.scrollIntoView({ behavior: "smooth", block: "start" });
    }

    // ---- studio (full width, Zenodo's header kept)
    const mainEl = () => document.querySelector("main#main") || document.querySelector("main");
    function goStudio(st) {
      const u = new URL(location.href);
      u.searchParams.set("studio", "1");
      if (st && st.q) u.searchParams.set("q", String(st.q));
      if (st && st.console) u.searchParams.set("console", "1");
      history.pushState(null, "", u); apply();
    }
    function backToRecord() { const u = new URL(location.href); for (const k of ["studio", "q", "console"]) u.searchParams.delete(k); history.pushState(null, "", u); apply(); }
    function buildStudio() {
      const f = () => session.file || files[0];
      const bar = document.createElement("div");
      bar.className = "ui secondary pointing menu";
      bar.setAttribute("data-retelens-bar", "");
      const back = document.createElement("a"); back.className = "item"; back.setAttribute("href", location.pathname); back.setAttribute("data-retelens-back", ""); back.setAttribute("aria-label", "Back to record");
      const bi = document.createElement("i"); bi.className = "arrow left icon"; bi.setAttribute("aria-hidden", "true"); back.append(bi, "Back to record");
      back.addEventListener("click", (e) => { e.preventDefault(); backToRecord(); });
      const crumbWrap = document.createElement("div"); crumbWrap.className = "item";
      const bc = document.createElement("div"); bc.className = "ui breadcrumb";
      const sec = (text, href) => { const a = document.createElement(href ? "a" : "div"); a.className = href ? "section" : "active section"; if (href) a.setAttribute("href", href); a.textContent = text; return a; };
      const div = () => { const s = document.createElement("span"); s.className = "divider"; s.textContent = "/"; return s; };
      const cur = sec(f().label); cur.id = "retelens-crumb-file";
      bc.append(sec("Zenodo", "/"), div(), sec(rec && rec.title ? rec.title : "Record " + recId, location.pathname), div(), cur);
      crumbWrap.append(bc);
      const right = document.createElement("div"); right.className = "right menu";
      const dlItem = document.createElement("div"); dlItem.className = "item";
      const dl = document.createElement("a"); dl.className = "ui compact mini button"; dl.setAttribute("role", "button"); dl.setAttribute("href", f().download);
      const di = document.createElement("i"); di.className = "download icon"; di.setAttribute("aria-hidden", "true"); dl.append(di, "Download");
      dlItem.append(dl); right.append(dlItem);
      bar.append(back, crumbWrap, right);
      const ex = new Explorer({ session, mode: "studio", urlKeys: ["file", "q", "view"], params, topbar: true, skin: "zenodo" });
      const body = document.createElement("div");
      body.style.cssText = "display:flex;flex-direction:column;min-height:520px";
      body.append(ex.el);
      const outer = document.createElement("div");
      outer.setAttribute("data-retelens", "studio");
      outer.style.cssText = "padding:0 1rem";
      outer.append(bar, body);
      const setH = () => { const top = outer.getBoundingClientRect().top + scrollY, bh = bar.getBoundingClientRect().height; body.style.height = Math.max(520, innerHeight - top - bh - 8) + "px"; };
      return { el: outer, ex, body, cur, dl, setH, started: false };
    }
    function apply() {
      const main = mainEl();
      if (!main) return;
      if (inStudio()) {
        if (!studio) studio = buildStudio();
        for (const c of main.children) if (c !== studio.el) ctx.hide(c);
        if (!studio.el.isConnected) { main.append(studio.el); ctx.track(studio.el); }
        studio.cur.textContent = (session.file || files[0]).label;
        studio.setH();
        if (!studio.started) { studio.started = true; studio.ex.start(); }
      } else {
        if (studio && studio.el.isConnected) studio.el.remove();
        for (const c of main.children) ctx.show(c);
        if (!boxStarted) { boxStarted = true; boxEx.start(); }
      }
    }
    ctx.on(window, "popstate", apply);
    ctx.on(window, "resize", () => { if (studio && inStudio()) studio.setH(); });

    // start: the file in ?preview_file= (or the first), the panel open by default
    const want = qs().get("preview_file");
    if (want && names(files).includes(want)) title.textContent = want;
    apply(); // studio on load, or start the box
    return { open };
  }
  function names(files) { return files.map((f) => f.path); }
})();
