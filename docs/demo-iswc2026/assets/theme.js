// Light/dark: an explicit choice wins and is remembered; otherwise the OS
// decides. Loaded on every page of the demonstration site.
(function () {
  var root = document.documentElement;
  try {
    var saved = localStorage.getItem("rete-demo-theme");
    if (saved) root.setAttribute("data-theme", saved);
  } catch (e) {}
  document.addEventListener("DOMContentLoaded", function () {
    var btn = document.getElementById("theme");
    if (!btn) return;
    btn.addEventListener("click", function () {
      var dark =
        root.getAttribute("data-theme") === "dark" ||
        (!root.getAttribute("data-theme") &&
          matchMedia("(prefers-color-scheme: dark)").matches);
      var next = dark ? "light" : "dark";
      root.setAttribute("data-theme", next);
      try {
        localStorage.setItem("rete-demo-theme", next);
      } catch (e) {}
    });
  });
})();
