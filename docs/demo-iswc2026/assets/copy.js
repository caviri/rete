// One-click copy for the install commands.
//
// A command you have to select by hand is a command you mistype, and these are
// the lines a visitor is most likely to want verbatim. Progressive: without
// JavaScript, or without clipboard permission, the text is still there to
// select — the button just reports that it could not do it for you.
document.addEventListener("click", async (e) => {
  const btn = e.target.closest(".copy");
  if (!btn) return;
  const code = btn.parentElement.querySelector("code");
  if (!code) return;

  const done = (label, ok) => {
    btn.textContent = label;
    btn.classList.toggle("ok", ok);
    setTimeout(() => {
      btn.textContent = "copy";
      btn.classList.remove("ok");
    }, 1600);
  };

  try {
    await navigator.clipboard.writeText(code.textContent.trim());
    done("copied", true);
  } catch {
    // Insecure origin, denied permission, or an old browser: select it instead
    // so the keyboard shortcut still works.
    const range = document.createRange();
    range.selectNodeContents(code);
    const sel = getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
    done("select + ⌘C", false);
  }
});
