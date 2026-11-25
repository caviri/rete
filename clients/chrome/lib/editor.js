// The SPARQL console's editor: a textarea over a coloured copy of its text, with line numbers beside it.
// The textarea keeps everything an editor needs — the text, the caret, the selection, undo, every key — and is
// transparent; under it, the same text coloured by RetelensQuery.highlight (keywords, IRIs, literals, variables,
// comments: the colours of the query block), redrawn on each change and scrolled with it. A value set from code (an
// example chosen, Edit in console) redraws too. No library and no markup: the copy is text nodes and spans.
(function () {
  const { h } = window.RetelensFmt;
  const VALUE = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value");

  // opts: {label} -> {el, textarea, refresh}
  function create(opts = {}) {
    const ta = h("textarea", { class: "rl-ed-ta", spellcheck: "false", wrap: "off", autocapitalize: "off", autocomplete: "off", "aria-label": opts.label || "SPARQL query" });
    const code = h("code");
    const pre = h("pre", { class: "rl-ed-hl", "aria-hidden": "true" }, code);
    const gutter = h("div", { class: "rl-ed-gutter", "aria-hidden": "true" });
    const el = h("div", { class: "rl-ed", "data-rl": "editor" }, gutter, h("div", { class: "rl-ed-body" }, pre, ta));
    let lines = 0;
    const sync = () => { pre.scrollTop = ta.scrollTop; pre.scrollLeft = ta.scrollLeft; gutter.scrollTop = ta.scrollTop; };
    const draw = () => {
      const text = VALUE.get.call(ta);
      code.textContent = "";
      code.append(...window.RetelensQuery.highlight(text), "\n"); // the newline gives an empty last line its height
      const n = text.split("\n").length;
      if (n !== lines) { lines = n; gutter.textContent = Array.from({ length: n }, (_, i) => String(i + 1)).join("\n"); }
      sync();
    };
    // drawn at once (not on the next frame): the textarea's own text is transparent, a late copy would blink
    ta.addEventListener("input", draw);
    ta.addEventListener("scroll", sync);
    Object.defineProperty(ta, "value", { configurable: true, get() { return VALUE.get.call(this); }, set(v) { VALUE.set.call(this, v); draw(); } });
    draw();
    return { el, textarea: ta, refresh: draw };
  }

  window.RetelensEditor = { create };
})();
