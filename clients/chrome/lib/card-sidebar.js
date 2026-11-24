// Dataset Card sidebar in Hugging Face's own vocabulary: h2.text-smd headings,
// dotted-leader dl rows, stat tiles, tag chips. Shows only fields the card carries;
// everything else goes to "More metadata". Card values are untrusted: text only,
// links http(s) only (h() enforces it).
//
//   RetelensCardSidebar.render(container, {card, info, contentHash, exampleCount, fileLength, name})
(function () {
  const { h, num, bytes, safeUrl } = window.RetelensFmt;

  const isObj = (v) => v && typeof v === "object" && !Array.isArray(v);
  const arr = (v) => (v == null ? [] : Array.isArray(v) ? v : [v]);
  const ORCID = /^(?:https?:\/\/orcid\.org\/)?(\d{4}-\d{4}-\d{4}-\d{3}[\dX])$/;
  const short = (u) => u.replace(/^https?:\/\/(www\.)?/, "").replace(/\/$/, "");
  const H2 = "text-smd mb-3 flex items-baseline font-semibold text-gray-800 dark:text-gray-200";

  const link = (href, text) => (safeUrl(href) ? h("a", { href, class: "text-blue-600 hover:underline dark:text-blue-400" }, text ?? href) : h("span", null, text ?? href));

  function licenseNode(lic) {
    const s = typeof lic === "string" ? lic : lic?.name || lic?.id || lic?.url || "see card";
    let href = safeUrl(s) ? s : isObj(lic) && safeUrl(lic.url) ? lic.url : null;
    if (!href) {
      const t = s.trim();
      if (/^[A-Za-z][A-Za-z0-9.+-]*$/.test(t) && /(CC|ODbL|ODC|MIT|Apache|BSD|GPL|MPL|PDDL|Unlicense|ISC|EPL)/i.test(t)) href = "https://spdx.org/licenses/" + t + ".html";
      const cc = t.match(/^CC[ -]BY((?:[ -](?:SA|NC|ND))*)[ -](\d\.\d)$/i);
      if (!href && cc) {
        const parts = cc[1].split(/[ -]/).filter(Boolean).join("-").toLowerCase();
        href = `https://creativecommons.org/licenses/by${parts ? "-" + parts : ""}/${cc[2]}/`;
      } else if (!href && /^CC0/i.test(t)) href = "https://creativecommons.org/publicdomain/zero/1.0/";
    }
    return href ? link(href, s) : h("span", null, s);
  }

  function personNode(p) {
    if (typeof p === "string") {
      const m = p.match(ORCID);
      return m ? link("https://orcid.org/" + m[1], p) : safeUrl(p) ? link(p, short(p)) : h("span", null, p);
    }
    if (!isObj(p)) return h("span", null, String(p));
    const name = p.name || p.fullName || p.label || [p.givenName, p.familyName].filter(Boolean).join(" ") || p.id || p.url || "?";
    const wrap = h("span");
    wrap.append(safeUrl(p.url) ? link(p.url, String(name)) : String(name));
    const orcid = [p.orcid, p.id, p.identifier, p["@id"]].map((x) => (typeof x === "string" ? x.match(ORCID) : null)).find(Boolean);
    if (orcid) wrap.append(" ", link("https://orcid.org/" + orcid[1], "ORCID"));
    if (p.affiliation) wrap.append(h("div", { class: "text-xs text-gray-500" }, typeof p.affiliation === "string" ? p.affiliation : String(p.affiliation.name || "")));
    return wrap;
  }

  function valueNode(v, depth = 0) {
    if (v == null) return h("span", { class: "text-gray-400" }, "null");
    if (typeof v === "string") return safeUrl(v) ? link(v, short(v)) : h("span", null, v);
    if (typeof v === "number" || typeof v === "boolean") return h("span", { class: "font-mono" }, String(v));
    if (Array.isArray(v)) {
      if (v.every((x) => x == null || typeof x !== "object") && v.length <= 6) return h("span", null, v.map(String).join(", "));
      return h("details", null, h("summary", { class: "cursor-pointer text-gray-500" }, `${v.length} entries`), h("pre", { class: "max-h-44 overflow-auto rounded-md border bg-gray-50 p-1.5 text-[11px] dark:border-gray-800 dark:bg-gray-900" }, JSON.stringify(v, null, 1)));
    }
    const keys = Object.keys(v);
    if (depth < 1 && keys.length <= 4 && keys.every((k) => typeof v[k] !== "object" || v[k] == null)) {
      return h("div", null, keys.map((k) => h("div", null, h("span", { class: "text-gray-500" }, k + ": "), valueNode(v[k], depth + 1))));
    }
    return h("details", null, h("summary", { class: "cursor-pointer text-gray-500" }, `${keys.length} fields`), h("pre", { class: "max-h-44 overflow-auto rounded-md border bg-gray-50 p-1.5 text-[11px] dark:border-gray-800 dark:bg-gray-900" }, JSON.stringify(v, null, 1)));
  }

  // HF dotted-leader row
  const row = (label, node) => (node == null ? null : h("dl", { class: "flex items-baseline justify-between gap-1 py-0.5" },
    h("dt", { class: "text-sm text-gray-500 flex-none" }, label),
    h("div", { class: "mx-4 min-w-[1rem] flex-1 border-b border-dotted dark:border-gray-700" }),
    h("dd", { class: "min-w-0 max-w-[65%] break-words text-right text-sm font-semibold" }, node)));
  // multi-value rows stack under the label
  const block = (label, nodes) => (!nodes.length ? null : h("div", { class: "py-0.5" }, h("div", { class: "text-sm text-gray-500" }, label), h("ul", { class: "ml-3 list-disc text-sm" }, nodes.map((n) => h("li", { class: "break-words" }, n)))));
  const section = (title, rows) => {
    rows = rows.filter(Boolean);
    return rows.length ? h("div", { class: "mb-4" }, h("h2", { class: H2 }, title), rows) : null;
  };
  const divider = () => h("div", { class: "divider-column-vertical" });
  const tile = (label, value) => h("div", { class: "mb-1.5 flex min-w-[5rem] max-w-full flex-col overflow-hidden rounded-lg border border-gray-100 px-2 py-1 md:mr-1.5 dark:border-gray-800" },
    h("div", { class: "truncate text-xs text-gray-400" }, label), h("div", { class: "truncate text-sm" }, value));

  function copyButton(text, label) {
    const b = h("button", { type: "button", class: "btn shadow-xs rounded-sm border px-1.5 py-0.5 text-xs font-normal text-gray-700 dark:text-gray-300" }, label);
    b.addEventListener("click", async () => {
      try { await navigator.clipboard.writeText(text); }
      catch (_) { const t = h("textarea", { style: "position:fixed;opacity:0" }); t.value = text; document.body.append(t); t.select(); try { document.execCommand("copy"); } catch (_) {} t.remove(); }
      const old = b.textContent; b.textContent = "Copied"; setTimeout(() => (b.textContent = old), 1200);
    });
    return b;
  }

  const CONSUMED = new Set(["title", "description", "version", "source_date", "license", "source", "publisher", "canonical_url", "derived_from", "keywords", "format_version", "triple_count", "quad_count", "named_graph_count", "term_count", "creators", "creator", "authors", "author", "contributors", "citation", "bibtex", "funding", "acknowledgements", "acknowledgments", "homepage", "documentation", "doi", "identifier"]);

  function render(el, data) {
    const { card, info, contentHash, exampleCount, fileLength, name } = data;
    el.textContent = "";
    el.setAttribute("data-retelens", "card");
    const quads = info?.quads ?? card?.quad_count ?? card?.triple_count;
    const terms = info?.terms ?? card?.term_count;
    const ng = info?.namedGraphs ?? card?.named_graph_count;

    // stat tiles first, like "Number of rows / Total file size"
    const tiles = h("div", { class: "mb-4 flex flex-wrap" });
    if (quads != null) tiles.append(tile("Quads:", num(quads)));
    if (terms != null) tiles.append(tile("Terms:", num(terms)));
    if (fileLength) tiles.append(tile("File size:", bytes(fileLength)));
    el.append(tiles);

    if (!card) { el.append(h("div", { class: "text-sm text-gray-500" }, "This file carries no embedded Dataset Card.")); return; }

    const ver = [card.version && "v" + String(card.version).replace(/^v/i, ""), card.source_date].filter(Boolean).join(" · ");
    el.append(h("div", { class: "mb-3" }, h("div", { class: "break-words text-base font-semibold" }, String(card.title || name || "Untitled graph")), ver ? h("div", { class: "text-xs text-gray-500" }, ver) : null));

    const people = [...arr(card.creators), ...arr(card.creator), ...arr(card.authors), ...arr(card.author)];
    const contribs = arr(card.contributors);
    const citationText = typeof card.citation === "string" ? card.citation : isObj(card.citation) ? card.citation.text || card.citation.apa || null : null;
    const attribution = section("Attribution", [
      people.length ? block(people.length > 1 ? "Creators" : "Creator", people.map(personNode)) : null,
      contribs.length ? block("Contributors", contribs.map(personNode)) : null,
      card.publisher ? row("Publisher", personNode(card.publisher)) : null,
      card.source ? row("Source", safeUrl(card.source) ? link(card.source, short(card.source)) : String(card.source)) : null,
      card.homepage ? row("Homepage", safeUrl(card.homepage) ? link(card.homepage, short(card.homepage)) : String(card.homepage)) : null,
      card.documentation ? row("Docs", safeUrl(card.documentation) ? link(card.documentation, short(card.documentation)) : String(card.documentation)) : null,
      card.doi ? row("DOI", /^10\.\d{4,}\/\S+$/.test(card.doi) ? link("https://doi.org/" + card.doi, card.doi) : valueNode(card.doi)) : null,
      card.funding ? block("Funding", arr(card.funding).map((f) => (typeof f === "string" ? h("span", null, f) : valueNode(f)))) : null,
      card.acknowledgements || card.acknowledgments ? row("Thanks", valueNode(card.acknowledgements || card.acknowledgments)) : null,
      citationText || card.bibtex ? h("div", { class: "py-0.5" }, h("div", { class: "text-sm text-gray-500" }, "Cite"),
        citationText ? h("div", { class: "mb-1 break-words text-xs" }, citationText) : null,
        h("div", { class: "flex flex-wrap gap-1" }, citationText ? copyButton(citationText, "Copy citation") : null, card.bibtex ? copyButton(String(card.bibtex), "Copy BibTeX") : null)) : null,
    ]);
    if (attribution) el.append(attribution, divider());

    if (card.license) el.append(section("License", [row("License", licenseNode(card.license))]), divider());

    const conv = card.extra?.conversion;
    const prov = section("Provenance", [
      card.derived_from ? block("Derived from", arr(card.derived_from).map((u) => (typeof u === "string" && safeUrl(u) ? link(u, short(u)) : h("span", null, String(u))))) : null,
      card.source_date ? row("Source date", String(card.source_date)) : null,
      card.version ? row("Version", String(card.version)) : null,
      conv?.converter ? row("Conversion", h("span", { class: "text-xs font-normal" }, String(conv.converter))) : null,
      card.canonical_url ? row("Canonical", safeUrl(card.canonical_url) ? link(card.canonical_url, short(card.canonical_url)) : String(card.canonical_url)) : null,
      contentHash ? row("Content hash", h("code", { class: "break-all text-[11px] font-normal", title: "blake3-16" }, contentHash)) : null,
    ]);
    if (prov) el.append(prov, divider());

    const stats = section("Statistics", [
      quads != null ? row("Quads", num(quads)) : null,
      terms != null ? row("Terms", num(terms)) : null,
      ng != null ? row("Named graphs", num(ng)) : null,
      fileLength ? row("File size", bytes(fileLength)) : null,
      card.format_version != null ? row("Format", "0x0" + Number(card.format_version).toString(16)) : null,
      exampleCount != null ? row("Example queries", num(exampleCount)) : null,
    ]);
    if (stats) el.append(stats);

    if (arr(card.keywords).length) {
      el.append(divider(), h("div", { class: "mb-4 mt-4" }, h("h2", { class: H2 }, "Keywords"),
        h("div", { class: "flex flex-wrap" }, arr(card.keywords).map((k) => h("span", { class: "mb-1 mr-1 rounded-lg" }, h("span", { class: "tag tag-white" }, h("span", null, String(k))))))));
    }

    const rest = Object.keys(card).filter((k) => !CONSUMED.has(k));
    const extraRest = isObj(card.extra) ? Object.keys(card.extra).filter((k) => k !== "conversion") : [];
    const shown = rest.filter((k) => !(k === "extra" && !extraRest.length));
    if (shown.length) {
      const dl = h("div", { class: "mt-2 space-y-1 text-xs" });
      for (const k of shown) {
        const val = k === "extra" ? Object.fromEntries(extraRest.map((x) => [x, card.extra[x]])) : card[k];
        dl.append(h("div", null, h("div", { class: "font-mono text-gray-400" }, k), h("div", { class: "break-words" }, valueNode(val))));
      }
      el.append(h("details", { class: "mt-2" }, h("summary", { class: "cursor-pointer text-sm text-gray-500" }, `More metadata (${shown.length})`), dl));
    }
  }

  window.RetelensCardSidebar = { render, licenseNode };
})();
