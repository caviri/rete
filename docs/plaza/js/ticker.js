// ticker.js — the funding marquee.
//
// Two jobs the CSS cannot do on its own:
//
// 1. Seamless looping needs the message duplicated until it overflows the
//    viewport twice, so the -50% translate lands exactly where it started. How
//    many copies that takes depends on the text and the window, so it is
//    measured rather than guessed.
// 2. A marquee nobody can stop is hostile. Dismissal sticks, and the animation
//    respects prefers-reduced-motion (where the strip becomes a static line).
const KEY = "plaza-ticker-dismissed";

const bar = document.getElementById("ticker");
const track = document.getElementById("tickerTrack");

if (bar && track) {
  try {
    if (localStorage.getItem(KEY) === "1") bar.remove();
  } catch (_) {}
}

if (bar && track && bar.isConnected) {
  const seed = track.firstElementChild;

  /** Fill the track with enough copies to cover 2× the viewport, then animate. */
  const fill = () => {
    // Reset to one copy so a resize recomputes from a known state.
    while (track.children.length > 1) track.lastElementChild.remove();
    const one = seed.getBoundingClientRect().width;
    if (!one) return;
    const need = Math.max(2, Math.ceil((window.innerWidth * 2) / one));
    for (let i = 1; i < need; i++) {
      const clone = seed.cloneNode(true);
      clone.setAttribute("aria-hidden", "true");
      // Only the first copy should be reachable by keyboard or a screen reader.
      for (const a of clone.querySelectorAll("a")) a.tabIndex = -1;
      track.appendChild(clone);
    }
    // The track holds `need` copies; shifting by one copy's width and looping
    // is indistinguishable from endless scrolling.
    track.style.setProperty("--shift", `-${one}px`);
    // Constant speed regardless of message length: ~55 px per second.
    track.style.setProperty("--dur", `${Math.max(8, one / 55)}s`);
    bar.classList.add("run");
  };

  // Fonts land after first paint and change the measured width.
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(fill);
  fill();

  let t;
  window.addEventListener("resize", () => {
    clearTimeout(t);
    t = setTimeout(fill, 150);
  });

  document.getElementById("tickerX")?.addEventListener("click", () => {
    bar.remove();
    try {
      localStorage.setItem(KEY, "1");
    } catch (_) {}
  });
}
