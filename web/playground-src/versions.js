(function (root) {
  "use strict";

  const REPO = "caviri/rete";
  const API = `https://api.github.com/repos/${REPO}/pulls?state=open&per_page=100`;
  const PREVIEW = "https://preview.graphplaza.com";
  const PRODUCTION = "https://caviri.github.io/rete/playground.html";
  const CACHE_KEY = "retePreviewVersionsV1";
  const CACHE_MS = 5 * 60 * 1000;
  const SHA_RE = /^[0-9a-f]{40}$/i;

  function eligiblePull(pr) {
    return Boolean(
      pr && Number.isInteger(pr.number) && pr.number > 0
      && pr.head && SHA_RE.test(pr.head.sha || "")
      && pr.head.repo && pr.head.repo.full_name === REPO,
    );
  }

  function previewUrl(pr) {
    if (!eligiblePull(pr)) throw new Error("preview requires a same-repository PR with a full SHA");
    return `${PREVIEW}/pr-${pr.number}/${pr.head.sha}/playground.html`;
  }

  function versionHref(url, hash) {
    return `${url}${hash || ""}`;
  }

  function cachedPreviews(storage, now) {
    if (!storage) return null;
    const cached = JSON.parse(storage.getItem(CACHE_KEY) || "null");
    return cached && cached.expires > now && Array.isArray(cached.previews)
      ? cached.previews
      : null;
  }

  async function discoverPreviews(options = {}) {
    const fetcher = options.fetch || (root.fetch && root.fetch.bind(root));
    const storage = options.storage === undefined ? root.sessionStorage : options.storage;
    const now = options.now ? options.now() : Date.now();
    if (!fetcher) return [];

    try {
      const cached = cachedPreviews(storage, now);
      if (cached) return cached;

      const response = await fetcher(API, {
        headers: { Accept: "application/vnd.github+json" },
      });
      if (!response.ok) return [];
      const pulls = await response.json();
      if (!Array.isArray(pulls)) return [];

      const previews = (await Promise.all(pulls.filter(eligiblePull).map(async (pr) => {
        const url = previewUrl(pr);
        try {
          const probe = await fetcher(url, { method: "HEAD", cache: "no-store" });
          if (!probe.ok) return null;
          return {
            number: pr.number,
            title: String(pr.title || ""),
            sha: pr.head.sha,
            url,
          };
        } catch (_error) {
          return null;
        }
      }))).filter(Boolean);

      if (storage) {
        storage.setItem(CACHE_KEY, JSON.stringify({
          expires: now + CACHE_MS,
          previews,
        }));
      }
      return previews;
    } catch (_error) {
      return [];
    }
  }

  function currentPreview() {
    const value = root.RETE_PREVIEW;
    if (!value || !Number.isInteger(value.number) || !SHA_RE.test(value.headSha || "")) {
      return null;
    }
    const pull = {
      number: value.number,
      title: String(value.title || ""),
      head: { sha: value.headSha, repo: { full_name: REPO } },
    };
    return {
      number: pull.number,
      title: pull.title,
      sha: pull.head.sha,
      url: previewUrl(pull),
    };
  }

  function addOption(doc, select, preview) {
    const option = doc.createElement("option");
    option.value = preview.url;
    option.textContent = `PR #${preview.number} · ${preview.title} · ${preview.sha.slice(0, 7)}`;
    select.appendChild(option);
  }

  async function initVersionPicker(options = {}) {
    const doc = options.document || root.document;
    const location = options.location || root.location;
    const select = doc && doc.getElementById("versionSelect");
    if (!select || !location) return [];

    const metadata = root.RETE_PREVIEW;
    const productionBuild = metadata && metadata.baseSha
      ? metadata.baseSha
      : root.RETE_BUILD;
    select.options[0].value = PRODUCTION;
    select.options[0].textContent = `Production${productionBuild ? ` · ${String(productionBuild).slice(0, 7)}` : ""}`;

    const current = currentPreview();
    if (current) addOption(doc, select, current);

    const discovered = await discoverPreviews(options);
    const seen = new Set(current ? [current.url] : []);
    for (const preview of discovered) {
      if (seen.has(preview.url)) continue;
      seen.add(preview.url);
      addOption(doc, select, preview);
    }

    if (current) select.value = current.url;
    select.onchange = () => location.assign(versionHref(select.value, location.hash));

    const badge = doc.getElementById("previewBadge");
    if (badge && current) badge.classList.remove("hidden");
    return discovered;
  }

  root.RETE_PLAYGROUND_VERSIONS = {
    eligiblePull,
    previewUrl,
    versionHref,
    discoverPreviews,
    initVersionPicker,
  };
})(window);
