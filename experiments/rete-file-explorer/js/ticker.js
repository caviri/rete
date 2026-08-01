// ticker.js — the right-to-left marquee, mirroring the plaza's.
//
// Two jobs the CSS cannot do alone:
//
// 1. Seamless looping needs the message duplicated until it overflows the
//    window twice, so translating by exactly one copy's width lands where it
//    started. How many copies that takes depends on the text and the window
//    size, so it is measured rather than guessed.
// 2. A marquee nobody can stop is hostile. It pauses on hover and focus,
//    honours prefers-reduced-motion (where it becomes a static centred line),
//    and can be dismissed for good.

const KEY = "rete-explorer-ticker-dismissed";

export function initTicker() {
  const bar = document.getElementById("ticker");
  const track = document.getElementById("tickerTrack");
  if (!bar || !track) return;

  try {
    if (localStorage.getItem(KEY) === "1") {
      bar.remove();
      return;
    }
  } catch (_) {
    // A webview with storage disabled just keeps the ticker.
  }

  const seed = track.firstElementChild;
  if (!seed) return;

  /** Fill the track with enough copies to cover 2× the window, then animate. */
  const fill = () => {
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
    track.style.setProperty("--shift", `-${one}px`);
    // Constant speed whatever the message length: ~55 px per second.
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

  const x = document.getElementById("tickerX");
  if (x) {
    x.addEventListener("click", () => {
      bar.remove();
      try {
        localStorage.setItem(KEY, "1");
      } catch (_) {}
    });
  }
}
