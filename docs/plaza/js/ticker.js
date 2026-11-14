// ticker.js — the line above the header: a marquee that scrolls right to left.
// It pauses under the pointer or keyboard focus, stands still for visitors who
// ask for reduced motion (styles.css), and can be dismissed for good.
const KEY = "plaza-fund-dismissed";
const bar = document.getElementById("fund");

if (bar) {
  let gone = false;
  try { gone = localStorage.getItem(KEY) === "1"; } catch (_) {}
  if (gone) bar.remove();
  else {
    // A second, hidden copy of the text makes the loop seamless.
    const group = bar.querySelector(".pz-marquee-group");
    if (group) {
      // On a very wide screen one pass of the text is shorter than the bar:
      // repeat it until it is not, so the loop never shows a gap.
      const items = [...group.children];
      for (let i = 0; i < 6 && group.scrollWidth < Math.max(screen.width, innerWidth); i++) {
        for (const it of items) {
          const c = it.cloneNode(true);
          c.setAttribute("aria-hidden", "true");
          c.querySelectorAll("a").forEach((a) => { a.tabIndex = -1; a.removeAttribute("id"); });
          group.append(c);
        }
      }
      const copy = group.cloneNode(true);
      copy.setAttribute("aria-hidden", "true");
      copy.querySelectorAll("a").forEach((a) => { a.tabIndex = -1; a.removeAttribute("id"); });
      group.after(copy);
      // "Submit a dataset" opens the same dialog as the header button.
      bar.addEventListener("click", (e) => {
        if (!e.target.closest('a[href="#submit"]')) return;
        e.preventDefault();
        document.getElementById("submitBtn")?.click();
      });
    }
    document.getElementById("fundX")?.addEventListener("click", () => {
      bar.remove();
      try { localStorage.setItem(KEY, "1"); } catch (_) {}
    });
  }
}
