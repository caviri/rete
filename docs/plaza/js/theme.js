// theme.js — light/dark toggle, persisted in localStorage. The actual theme is
// set by a tiny inline <head> script (to avoid a flash); this module just wires
// the #themeBtn button and keeps its label in sync. Loaded on both pages.
const KEY = "plaza-theme";
const root = document.documentElement;

if (!root.dataset.theme) {
  // Fallback if the inline head script didn't run.
  try {
    root.dataset.theme =
      localStorage.getItem(KEY) ||
      (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark");
  } catch (_) {
    root.dataset.theme = "dark";
  }
}

const btn = document.getElementById("themeBtn");
if (btn) {
  const cur = () => root.dataset.theme;
  const paint = () => {
    // Show the theme you'd switch TO (no emoji).
    btn.textContent = cur() === "light" ? "Dark" : "Light";
    btn.title = `Switch to ${cur() === "light" ? "dark" : "light"} theme`;
  };
  paint();
  btn.addEventListener("click", () => {
    root.dataset.theme = cur() === "light" ? "dark" : "light";
    try { localStorage.setItem(KEY, root.dataset.theme); } catch (_) {}
    paint();
    // Let the pages re-render their theme-aware procedural images.
    window.dispatchEvent(new CustomEvent("plaza-theme", { detail: root.dataset.theme }));
  });
}
