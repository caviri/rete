// search.js — one search field over several kinds of thing.
//
// The plaza has datasets, but it also has ontologies, vocabularies, topical
// tags, licences and external providers, and a visitor rarely knows which of
// those their word is. So the field searches all of them and groups the
// suggestions by what they ARE: picking a dataset opens it, picking anything
// else adds a filter. Free text always falls through to filtering the grid.
//
// The caller owns the data and the actions; this module owns the interaction.

const PER_GROUP = 5;
const GROUP_ORDER = ["dataset", "ontology", "vocabulary", "tag", "licence", "provider"];
const GROUP_LABEL = {
  dataset: "Datasets",
  ontology: "Ontologies",
  vocabulary: "Vocabularies",
  tag: "Topics",
  licence: "Licences",
  provider: "Connected to",
};
const GROUP_ICON = {
  dataset: "◧", ontology: "◇", vocabulary: "❯", tag: "#", licence: "©", provider: "↔",
};

const esc = (s) =>
  String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

/** Bold the matched run, so it is obvious WHY a suggestion is there. */
function mark(label, q) {
  const i = label.toLowerCase().indexOf(q);
  if (i < 0 || !q) return esc(label);
  return `${esc(label.slice(0, i))}<b>${esc(label.slice(i, i + q.length))}</b>${esc(label.slice(i + q.length))}`;
}

/**
 * @param {object} o
 * @param {HTMLInputElement} o.input
 * @param {HTMLElement} o.panel
 * @param {() => Array<{type:string,label:string,meta?:string,pick:Function}>} o.getIndex
 * @param {(q: string) => void} o.onQuery   free-text query changed
 */
export function mountSearch({ input, panel, getIndex, onQuery }) {
  let items = [];
  let cursor = -1;

  const close = () => {
    panel.hidden = true;
    input.setAttribute("aria-expanded", "false");
    cursor = -1;
  };

  function suggest() {
    const q = input.value.trim().toLowerCase();
    if (!q) return close();

    const byGroup = new Map();
    for (const it of getIndex()) {
      if (!it.label.toLowerCase().includes(q)) continue;
      const bucket = byGroup.get(it.type) || [];
      if (bucket.length < PER_GROUP) bucket.push(it);
      byGroup.set(it.type, bucket);
    }

    items = [];
    let html = "";
    for (const g of GROUP_ORDER) {
      const bucket = byGroup.get(g);
      if (!bucket || !bucket.length) continue;
      html += `<div class="pz-ac-group">${GROUP_LABEL[g] || g}</div>`;
      for (const it of bucket) {
        html +=
          `<button class="pz-ac-item" data-i="${items.length}" type="button">` +
          `<span class="pz-ac-ic">${GROUP_ICON[g] || "•"}</span>` +
          `<span class="pz-ac-lab">${mark(it.label, q)}</span>` +
          `<span class="pz-ac-meta">${esc(it.meta || "")}</span></button>`;
        items.push(it);
      }
    }

    panel.innerHTML = html || `<div class="pz-ac-empty">Nothing matches “${esc(input.value.trim())}”.</div>`;
    panel.hidden = false;
    input.setAttribute("aria-expanded", "true");
    cursor = -1;

    for (const el of panel.querySelectorAll(".pz-ac-item")) {
      el.addEventListener("mousedown", (e) => {
        // mousedown, not click: `blur` would close the panel first.
        e.preventDefault();
        choose(Number(el.dataset.i));
      });
    }
  }

  function choose(i) {
    const it = items[i];
    if (!it) return;
    input.value = "";
    onQuery("");
    close();
    it.pick();
  }

  function move(delta) {
    const els = [...panel.querySelectorAll(".pz-ac-item")];
    if (!els.length) return;
    cursor = (cursor + delta + els.length + 1) % (els.length + 1); // +1 = "no selection"
    els.forEach((el, i) => el.classList.toggle("on", i === cursor));
    if (cursor >= 0) els[cursor].scrollIntoView({ block: "nearest" });
  }

  input.addEventListener("input", () => {
    onQuery(input.value.trim().toLowerCase());
    suggest();
  });
  input.addEventListener("focus", () => { if (input.value.trim()) suggest(); });
  input.addEventListener("blur", () => setTimeout(close, 120));
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
    else if (e.key === "Enter" && cursor >= 0) { e.preventDefault(); choose(cursor); }
    else if (e.key === "Escape") { close(); input.blur(); }
  });

  // "/" focuses search from anywhere, the way every code host does it.
  document.addEventListener("keydown", (e) => {
    if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey) return;
    const t = e.target;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
    e.preventDefault();
    input.focus();
    input.select();
  });

  return { refresh: () => { if (!panel.hidden) suggest(); } };
}
