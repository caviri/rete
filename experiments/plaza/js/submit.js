// submit.js — the "submit a dataset" composer.
//
// The visitor types once, here, and the two buttons carry that text somewhere it
// can be answered: a prefilled GitHub issue, or a prefilled email. Both are pure
// URL construction — no backend, no form endpoint, nothing to keep running,
// which is the same constraint the rest of this site lives under.
//
//   GitHub:  /issues/new?title=…&body=…      (GitHub's own prefill parameters)
//   Email:   mailto:…?subject=…&body=…       (RFC 6068)
//
// Deliberately no `labels=` on the GitHub URL: that parameter fails the whole
// prefill if the label does not exist in the repository, so the marker lives in
// the body instead where it can never break the link.

const REPO = "https://github.com/caviri/rete";
const EMAIL = "hi@h4ck1ng.science";

// Practical ceilings. Browsers and mail clients both truncate silently, which
// would lose the visitor's words without telling them — so we tell them.
const MAILTO_SAFE = 1800;
const GITHUB_SAFE = 7000;

const $ = (sel) => document.querySelector(sel);

const modal = $("#submitModal");
const form = $("#submitForm");
const ghBtn = $("#ghBtn");
const mailBtn = $("#mailBtn");
const note = $("#submitNote");

if (modal && form) {
  const open = (e) => {
    if (e) e.preventDefault();
    modal.hidden = false;
    compose();
    setTimeout(() => form.elements.name?.focus(), 30);
  };
  const close = () => { modal.hidden = true; };

  $("#submitBtn")?.addEventListener("click", open);
  $("#submitBtn2")?.addEventListener("click", open);
  $("#submitX")?.addEventListener("click", close);
  modal.addEventListener("click", (e) => { if (e.target === modal) close(); });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !modal.hidden) close();
  });

  form.addEventListener("input", compose);
  compose();
}

/** Build the shared title/body, then hang both URLs off it. */
function compose() {
  const v = (n) => (form.elements[n]?.value || "").trim();
  const name = v("name");
  const url = v("url");
  const license = v("license");
  const who = v("who");
  const about = v("about");

  const title = name ? `Dataset submission: ${name}` : "Dataset submission";

  const body = [
    `**Dataset**: ${name || "—"}`,
    `**URL**: ${url || "—"}`,
    `**Licence**: ${license || "—"}`,
    who ? `**Submitted by**: ${who}` : null,
    "",
    about || "_(no description given)_",
    "",
    "---",
    "Sent from the rete plaza submission form — https://graphplaza.com",
  ]
    .filter((line) => line !== null)
    .join("\n");

  const gh = `${REPO}/issues/new?title=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;
  const mail = `mailto:${EMAIL}?subject=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;

  const ready = Boolean(name && about);
  for (const [el, href] of [[ghBtn, gh], [mailBtn, mail]]) {
    if (!el) continue;
    el.href = ready ? href : "#";
    el.classList.toggle("pz-disabled", !ready);
    el.setAttribute("aria-disabled", String(!ready));
  }

  if (!note) return;
  if (!ready) {
    note.textContent = "A name and a description are enough to send it.";
  } else if (mail.length > MAILTO_SAFE) {
    note.textContent =
      "Long descriptions can be truncated by some mail clients — the GitHub route " +
      "carries the full text.";
  } else if (gh.length > GITHUB_SAFE) {
    note.textContent = "That is a long description; consider trimming it before sending.";
  } else {
    note.textContent = "Opens with everything already filled in — you only press send.";
  }
}

// A disabled-looking anchor still navigates; stop it at the source.
for (const el of [ghBtn, mailBtn]) {
  el?.addEventListener("click", (e) => {
    if (el.classList.contains("pz-disabled")) {
      e.preventDefault();
      form?.reportValidity();
    }
  });
}
