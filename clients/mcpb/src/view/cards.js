// The card view's decisions, apart from the page that draws them, so they are
// tested in Node (test/cards.test.mjs): which kind of value a field holds, which
// addresses may be loaded, whether the host's policy lets one load, and how many
// cards are drawn at once.
//
// The field types and the way a value is recognised follow the playground's Cards
// output (web/playground-src/app.js: COL_TYPES, autoCell, looksImageUrl,
// looksAudioUrl, looksVideoUrl, looksMeshUrl): the same names, the same file
// extensions. IIIF manifests, WKT geometries (a small map), Markdown and PMTiles
// map layers are drawn by the Retelens cells (clients/chrome/lib/cells.js, bundled
// into the page): this file only recognises them. What is left out is what needs
// a library the page does not carry (molecular structures, PDF.js, Page preview)
// and what names a dataset (the playground's per-host image rules).

/** [type, label]: the playground's column types that make sense in an offline page. */
export const FIELD_TYPES = [
  ["auto", "Auto"],
  ["text", "Text"],
  ["link", "Link"],
  ["button", "Button"],
  ["image", "Image"],
  ["iiif", "IIIF"],
  ["geo", "Map"],
  ["audio", "Audio"],
  ["video", "Video"],
  ["model3d", "3D"],
  ["pdf", "PDF"],
  ["markdown", "Markdown"],
  ["layer", "Map layer"],
  ["number", "Number"],
];
export const FIELD_TYPE_NAMES = FIELD_TYPES.map(([t]) => t);

export const CARDS_FIRST = 24; // cards drawn before "Show more"
export const CARDS_STEP = 48; // cards each "Show more" adds
export const CARDS_MAX = 500; // cards in all, however many rows there are
export const LIVE_3D = 6; // 3D viewers alive at once: each one holds a WebGL context

const IMAGE_EXT = /\.(jpe?g|png|gif|svg|webp|avif)$/i;
const AUDIO_EXT = /\.(mp3|wav|ogg|oga|flac|m4a|aac|opus)$/i;
const VIDEO_EXT = /\.(mp4|webm|ogv|m4v|mov)$/i;
const MESH_EXT = /\.(glb|gltf)$/i; // what <model-viewer> reads; the playground also lists ply/splat
const PDF_EXT = /\.pdf$/i;
const COMMONS_FILE = /^https?:\/\/commons\.wikimedia\.org\/wiki\/Special:FilePath\//i;
const DATA_IMAGE = /^data:image\/(png|jpe?g|gif|webp|avif|svg\+xml)[;,]/i;
// A WKT geometry: the value starts with one (after an optional CRS IRI, as GeoSPARQL writes it).
const WKT = /^\s*(<[^>\s]*>\s*)?(POINT|LINESTRING|POLYGON|MULTIPOINT|MULTILINESTRING|MULTIPOLYGON|GEOMETRYCOLLECTION)\s*[ZM]*\s*\(/i;
// A IIIF manifest or collection, by its path (Retelens' looksIiif).
const IIIF = (path) => /\/manifest(\.json(ld)?)?$/i.test(path) || /\/iiif\//i.test(path) || /iiif.*\/collection(\.json)?$/i.test(path);
const PMTILES_EXT = /\.pmtiles$/i;

/** The path of a URL without query and fragment, for the extension test. */
const pathOf = (url) => String(url).split(/[?#]/)[0];

/**
 * An address a card may load or open, or null. http and https only (an http
 * address is upgraded for media: the page is served over https), and a data:
 * address only when `data` says an image may be one. Anything with a space or
 * a control character is not an address: it is text.
 */
export function safeUrl(value, { data = false, upgrade = false } = {}) {
  const s = String(value ?? "").trim();
  if (!s || s.length > (data ? 1_000_000 : 8192) || /[\s\u0000-\u001f\u007f<>"]/.test(s.replace(/^(data:[^,]*,).*$/s, "$1"))) return null;
  if (data && DATA_IMAGE.test(s)) return s;
  let url;
  try {
    url = new URL(s);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (!url.hostname) return null;
  if (upgrade && url.protocol === "http:") url.protocol = "https:";
  return url.href;
}

/** A web address worth linking: a dotted host, so a toy namespace like http://ex/a stays text. */
export const isWebUrl = (value) => {
  const url = safeUrl(value);
  return !!url && new URL(url).hostname.includes(".");
};

/**
 * What a value is, from the value alone: "image", "iiif", "geo", "layer", "audio",
 * "video", "model3d", "pdf", "link" or "text". No dataset names, no predicates.
 */
export function detect(value) {
  const s = String(value ?? "");
  if (DATA_IMAGE.test(s) && safeUrl(s, { data: true })) return "image";
  if (WKT.test(s)) return "geo";
  if (!isWebUrl(s)) return "text";
  const path = pathOf(s);
  if (COMMONS_FILE.test(s) || IMAGE_EXT.test(path)) return "image";
  if (PMTILES_EXT.test(path)) return "layer";
  if (IIIF(path)) return "iiif";
  if (MESH_EXT.test(path)) return "model3d";
  if (AUDIO_EXT.test(path)) return "audio";
  if (VIDEO_EXT.test(path)) return "video";
  if (PDF_EXT.test(path)) return "pdf";
  return "link";
}

/** A Commons file address with a width, so the server sends a thumbnail and not the original. */
export function thumbnail(url, width = 480) {
  return COMMONS_FILE.test(url) ? `${url}${url.includes("?") ? "&" : "?"}width=${width}` : url;
}

const MEDIA = new Set(["image", "audio", "video", "model3d"]);
/** What the card does with a type: "media" loads something, "link" opens it, "text" shows it. */
export const isMedia = (type) => MEDIA.has(type);
const RICH = new Set(["iiif", "geo", "markdown", "layer"]);
/** Types the Retelens cells draw: a IIIF manifest's pictures, a geometry's map, Markdown, a map layer. */
export const isRich = (type) => RICH.has(type);

/**
 * One field of one card: { name, value, type, src, link }. `type` is what it is
 * drawn as (a type forced on a value that is no address is drawn as text, so a
 * forced Image on a label does not make a broken picture). `src` is the address
 * to load for a media field; `link` the address to open.
 */
export function field(name, value, forced = "auto") {
  const type0 = forced && forced !== "auto" && FIELD_TYPE_NAMES.includes(forced) ? forced : detect(value);
  const image = type0 === "image";
  const src = safeUrl(value, { data: image, upgrade: true });
  const link = safeUrl(value);
  let type = type0;
  // the Retelens cells take these from the value itself; an address type on a value that is no address is text
  if (type === "geo" || type === "markdown") return { name, value: String(value), type, src: null, link: null };
  if ((type === "iiif" || type === "layer") && link) return { name, value: String(value), type, src: null, link };
  if (type !== "text" && type !== "number" && !src) type = "text";
  if (type === "number" && !Number.isFinite(Number(value))) type = "text";
  return { name, value: String(value), type, src: isMedia(type) ? src : null, link: type === "text" || type === "number" ? null : link };
}

/** A card's fields: those the row has a value for, in the order of `vars`. */
export function cardFields(vars, row, types = {}) {
  const out = [];
  for (const v of vars) {
    const value = row[v];
    if (value === undefined || value === null || value === "") continue;
    out.push(field(v, value, types[v]));
  }
  return out;
}

/** The label of an "Open … ↗" link under a media field, as the playground words them. */
export const OPEN_LABEL = { image: "Open image ↗", audio: "Open audio ↗", video: "Open video ↗", model3d: "Open 3D ↗", pdf: "Open PDF ↗", link: "Open ↗", button: "Open ↗" };

/** How many cards are drawn: `shown` so far, `total` rows, a step more on request, never more than the cap. */
export function cardsShown(total, shown = CARDS_FIRST, more = false) {
  const cap = Math.min(total, CARDS_MAX);
  return Math.min(cap, more ? shown + CARDS_STEP : shown);
}

/** What the "Show more" button says, or "" when everything is drawn. */
export function moreLabel(total, shown) {
  const cap = Math.min(total, CARDS_MAX);
  const left = cap - shown;
  return left > 0 ? `Show ${Math.min(left, CARDS_STEP)} more (${left} hidden)` : "";
}

// --- what the host lets the page load ---------------------------------------------------

/** Whether a CSP source expression (https://host, https://*.host, https:, *) covers an address. */
export function sourceCovers(source, url) {
  let u;
  try {
    u = new URL(url);
  } catch {
    return false;
  }
  const s = String(source).trim();
  if (s === "*") return true;
  if (/^[a-z][a-z0-9+.-]*:$/i.test(s)) return u.protocol === s.toLowerCase();
  const m = /^(?:([a-z][a-z0-9+.-]*):\/\/)?(\*\.)?([^/:*]+)(?::(\d+|\*))?(\/.*)?$/i.exec(s);
  if (!m) return false;
  const [, scheme, wild, host, port] = m;
  if (scheme && u.protocol !== `${scheme.toLowerCase()}:`) return false;
  if (!scheme && u.protocol !== "https:") return false;
  const h = u.hostname.toLowerCase();
  const want = host.toLowerCase();
  if (wild ? !h.endsWith(`.${want}`) : h !== want) return false;
  return !port || port === "*" || port === (u.port || (u.protocol === "https:" ? "443" : "80"));
}

/**
 * Whether the host's content security policy lets the page load `url` as `kind`:
 * true or false when the host said what it approved (hostCapabilities.sandbox.csp:
 * resourceDomains for images, audio and video; connectDomains for the fetch a 3D
 * model needs), and null when it said nothing, in which case the page tries and
 * shows what happened. Pictures given as data: addresses need no one's leave.
 */
export function hostAllows(csp, url, kind) {
  if (/^data:/i.test(url)) return true;
  if (!csp || typeof csp !== "object") return null;
  const list = kind === "model3d" ? csp.connectDomains : csp.resourceDomains;
  return (list ?? []).some((s) => sourceCovers(s, url));
}
