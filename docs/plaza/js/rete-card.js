// rete-card.js — read a .rete file's self-description with zero dependencies.
//
// A `.rete` file is self-describing: a fixed header points at every section,
// and a **Dataset Card** — compact JSON — says what the dataset is. So a static
// page can learn what a dataset *is* with one or two small HTTP range requests,
// never touching the index or downloading the file. That is the whole premise of
// the plaza.
//
// Mirrors crates/rete-core/src/header.rs (formats <= 0x05) and
// crates/rete-core/src/v06/mod.rs (0x06, 0x07, 0x08), little-endian on disk.
// Three header layouts are handled:
//   v1/v2   128-byte header, card at [metadata_offset ..+metadata_len), raw JSON.
//   v3..v5  1024-byte core + section directory (24-byte entries from offset 64);
//           the card is the Metadata section (kind 1), raw JSON.
//   v6..v8  128-byte header (docs/SPEC.md 4.1) + root block. The card lives in
//           the card region at 128 + root_len + card_off, `card_len` stored
//           bytes, compressed with the header's internal codec (byte 75:
//           0 = none, 1 = zstd). Since 0x07 a card too big for the hot card
//           keeps its largest fields in a cold annex; the hot card carries an
//           `annex` marker and that is all the plaza shows.
// zstd is decoded with the vendored fzstd (../vendor/fzstd.mjs, MIT, loaded on
// first use) — the gallery never needs the wasm engine to paint.

export const HEADER_LEN = 1024;          // bytes every layout's header fits in
export const FIRST_READ = 32768;         // first request: header + root + usually the card
export const CARD_MAX_FETCH = 1.5e6;     // never fetch a card range larger than this
export const MIN_FORMAT = 3;             // oldest format the plaza reads (0x03)
export const MAX_FORMAT = 8;             // newest (0x08)
const MAGIC = "RETE";

const dec = new TextDecoder();
const hex = (bytes) =>
  Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");

/** "0x08" for 8. */
export const formatLabel = (v) => "0x" + Number(v).toString(16).padStart(2, "0");

/** Decode a .rete header (version-aware) into the offsets/counts we care about. */
export function parseHeader(buf) {
  const b = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (b.length < 24) throw new Error(`header too small: ${b.length}`);
  if (dec.decode(b.subarray(0, 4)) !== MAGIC) throw new Error("not a .rete file (bad magic)");
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const u16 = (o) => dv.getUint16(o, true);
  const u32 = (o) => dv.getUint32(o, true);
  const u64 = (o) => Number(dv.getBigUint64(o, true)); // offsets never exceed 2^53
  const version = b[4], flags = b[5];

  if (version > MAX_FORMAT) {
    throw new Error(`format ${formatLabel(version)} is newer than this page understands (max ${formatLabel(MAX_FORMAT)})`);
  }

  if (version >= 6) {
    // 128-byte header, docs/SPEC.md 4.1.
    if (b.length < 128) throw new Error(`${formatLabel(version)} header truncated: ${b.length}`);
    const rootLen = u32(32), cardOff = u32(80), cardLen = u32(84);
    const graphCount = u64(64);
    return {
      version, flags, hasQuads: (flags & 1) !== 0,
      metadataOffset: cardLen > 0 ? 128 + rootLen + cardOff : 0,
      metadataLen: cardLen,
      cardCodec: b[75],                 // 0 = stored raw, 1 = zstd
      contentHash: hex(b.subarray(8, 24)),
      fileLength: u64(24),
      quadCount: u64(48), termCount: u64(56),
      namedGraphCount: graphCount,
      hasNamedGraphs: graphCount > 0,
      buildOffset: u32(108) > 0 ? 128 + rootLen + u32(104) : 0,
      buildLen: u32(108),
    };
  }

  if (version >= 3) {
    // 1024-byte core (64B) + 24B section-directory entries from offset 64.
    const quadCount = u64(24), termCount = u64(32);
    const contentHash = hex(b.subarray(8, 24));
    const sectionCount = u16(44);
    let metadataOffset = 0, metadataLen = 0, hasNamedGraphs = false;
    for (let i = 0; i < sectionCount; i++) {
      const p = 64 + i * 24;
      if (p + 24 > b.length) break;
      const kind = u16(p), off = u64(p + 8), len = u64(p + 16);
      if (kind === 1) { metadataOffset = off; metadataLen = len; } // Metadata
      else if (kind === 5 && len > 0) hasNamedGraphs = true; // NamedGraphs
    }
    return { version, flags, hasQuads: (flags & 1) !== 0, metadataOffset, metadataLen, cardCodec: 0, contentHash, quadCount, termCount, hasNamedGraphs };
  }

  // Legacy 128-byte header (v1/v2).
  return {
    version, flags, hasQuads: (flags & 1) !== 0,
    metadataOffset: u64(8), metadataLen: u64(16), cardCodec: 0,
    contentHash: hex(b.subarray(92, 108)),
    quadCount: u64(76), termCount: u64(84),
    hasNamedGraphs: u64(116) > 0,
  };
}

/**
 * The first `n` bytes of a response body, cancelling the transfer as soon as
 * they have arrived — so a host that ignores Range and answers 200 with the
 * whole file costs one slice, not the file.
 */
async function bodyUpTo(res, n) {
  if (!res.body || !res.body.getReader) {
    return new Uint8Array(await res.arrayBuffer()).subarray(0, n);
  }
  const reader = res.body.getReader();
  const chunks = [];
  let got = 0;
  while (got < n) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    got += value.length;
  }
  if (got >= n) { try { await reader.cancel(); } catch (_) {} }
  const out = new Uint8Array(Math.min(got, n));
  let o = 0;
  for (const c of chunks) {
    const take = Math.min(c.length, out.length - o);
    if (take <= 0) break;
    out.set(c.subarray(0, take), o);
    o += take;
  }
  return out;
}

/** Total file size from a Content-Range header ("bytes 0-127/12345"), or null. */
function totalFromContentRange(res) {
  let cr = null;
  try { cr = res.headers.get("Content-Range"); } catch (_) {} // hidden by some CORS configs
  const m = cr && cr.match(/\/(\d+)\s*$/);
  return m ? +m[1] : null;
}

/**
 * Fetch [start, end] (inclusive) of a URL via an HTTP Range request. Falls back
 * to slicing a full 200 response if the server ignores Range (the R2 origin the
 * remote datasets live on honours ranges, as the playground relies on).
 */
export async function rangeFetch(url, start, end) {
  const res = await fetch(url, { headers: { Range: `bytes=${start}-${end}` } });
  if (!res.ok && res.status !== 206) throw new Error(`HTTP ${res.status} for ${url}`);
  // 206 -> already the requested slice; 200 -> the body from byte 0, slice it.
  if (res.status === 206) return bodyUpTo(res, end - start + 1);
  return (await bodyUpTo(res, end + 1)).subarray(start, end + 1);
}

let zstdP = null;
function zstd() {
  if (!zstdP) zstdP = import(new URL("../vendor/fzstd.mjs", import.meta.url).href);
  return zstdP;
}

/** Turn the stored card bytes into a JSON object (decompressing if the header says so). */
async function decodeCard(header, stored) {
  let bytes = stored;
  if (header.cardCodec === 1) bytes = (await zstd()).decompress(stored);
  else if (header.cardCodec !== 0) throw new Error(`unknown card codec ${header.cardCodec}`);
  return JSON.parse(dec.decode(bytes));
}

/**
 * Read a dataset's header + embedded card (+ total file size).
 *   source: a URL string, or an already-loaded Uint8Array/ArrayBuffer.
 * Returns { header, card, size, status, bytesFetched, requests, ... }:
 *   card   null when the file carries none, when it is too large to fetch, or
 *          when it could not be decoded (see `status`)
 *   size   total .rete byte length (Content-Range, the 0x06+ header, or the body)
 *   status "full" | "none" (file has no card) | "too-large" | "error" (card
 *          present but unreadable; `cardError` says why)
 *   cardTooLarge  true when the card's byte range exceeds CARD_MAX_FETCH — the
 *          range is NOT fetched (one catalog file has a 117 MB one); callers
 *          paint from the header via liteCardFromHeader().
 *   cardRange     { offset, length } of the stored card
 *   bytesFetched, requests   network cost of this read (0 for in-memory input)
 * A failed header request, a bad magic or a format newer than MAX_FORMAT throws.
 */
export async function readReteCard(source) {
  // Already in memory (e.g. a bundled file fully fetched): read in place.
  if (source instanceof Uint8Array || source instanceof ArrayBuffer) {
    const b = source instanceof Uint8Array ? source : new Uint8Array(source);
    const header = parseHeader(b);
    const out = { header, card: null, size: b.length, status: "none", bytesFetched: 0, requests: 0 };
    return finishCard(out, header, (off, len) => b.subarray(off, off + len));
  }

  // Remote / local-over-http: one first read covers the header, and usually the card.
  const url = source;
  const res = await fetch(url, { headers: { Range: `bytes=0-${FIRST_READ - 1}` } });
  if (!res.ok && res.status !== 206) throw new Error(`HTTP ${res.status} for ${url}`);
  const head = await bodyUpTo(res, FIRST_READ);
  let size = totalFromContentRange(res);
  if (size == null && res.status === 200 && head.length < FIRST_READ) size = head.length; // whole body returned
  const header = parseHeader(head);
  if (size == null && header.fileLength) size = header.fileLength;
  const out = { header, card: null, size, status: "none", bytesFetched: head.length, requests: 1 };

  const want = header.metadataLen;
  if (want > 0 && size != null && header.metadataOffset + want > size) {
    out.cardRange = { offset: header.metadataOffset, length: want };
    out.status = "error"; out.cardError = "card range lies beyond the end of the file";
    return out;
  }
  // The card is often already inside the first read; otherwise one more request
  // (never for a range over CARD_MAX_FETCH — finishCard flags that).
  let fetched = null;
  if (want > 0 && header.metadataOffset + want > head.length && want <= CARD_MAX_FETCH) {
    try {
      fetched = await rangeFetch(url, header.metadataOffset, header.metadataOffset + want - 1);
      out.bytesFetched += fetched.length; out.requests++;
    } catch (e) {
      out.cardRange = { offset: header.metadataOffset, length: want };
      out.status = "error"; out.cardError = String((e && e.message) || e);
      return out;
    }
  }
  return finishCard(out, header, (off, len) =>
    fetched ? fetched : head.subarray(off, off + len));
}

/** Shared tail of readReteCard: guard, slice, decode, fill the status flags. */
async function finishCard(out, header, slice) {
  const len = header.metadataLen;
  if (!(len > 0)) return out; // no card in this file
  out.cardRange = { offset: header.metadataOffset, length: len };
  if (len > CARD_MAX_FETCH) {
    out.status = "too-large"; out.cardTooLarge = true;
    return out;
  }
  try {
    const stored = slice(header.metadataOffset, len);
    if (stored.length < len) throw new Error("card bytes truncated");
    out.card = await decodeCard(header, stored.subarray(0, len));
    out.status = "full";
  } catch (e) {
    out.status = "error"; out.cardError = String((e && e.message) || e);
  }
  return out;
}

/** Human-readable byte size, e.g. 124000000 → "118 MB". */
export function fmtBytes(n) {
  if (n == null) return null;
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return (v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)) + " " + u[i];
}

/**
 * When a file has no embedded card, synthesise a minimal one from the header so
 * every tile still has counts + a stable id to seed its image with. Curated
 * fields come from the manifest entry.
 */
export function liteCardFromHeader(header, entry = {}) {
  return {
    _lite: true, // marks a header-only card (no derived profile inside the file)
    title: entry.title || entry.key,
    description: entry.blurb || null,
    license: entry.license || null,
    source: entry.source || null,
    triple_count: header.quadCount,
    quad_count: header.quadCount,
    named_graph_count: header.namedGraphCount != null ? header.namedGraphCount : (header.hasNamedGraphs ? 1 : 0),
    term_count: header.termCount,
    predicates: [],
    classes: [],
    vocabularies: [],
    queries: [],
    format_version: header.version,
  };
}

// *italic* — the ONE emphasis rule. It is duplicated verbatim in
// web/playground-src/app.js, scripts/preview/card.mjs and
// experiments/plaza/js/rete-card.js, because those three cannot import from one
// another: app.js is concatenated into docs/playground.html as a classic script,
// card.mjs is Node ESM, and rete-card.js is a browser ES module that
// scripts/build_plaza.py copies into docs/plaza/. No bundler in this repo reaches
// all three, so tests/gate/checks/check_md_emphasis.mjs asserts the literal below
// AND this comment are byte-identical in every copy — change one and the gate
// fails until you have changed them all. They had already drifted once: mdLite
// used [^*]+ where the other five used [^*\n]+, so emphasis could cross a
// paragraph break in the playground and nowhere else.
//
// A delimiter only emphasises when it FLANKS its text — CommonMark's left- and
// right-flanking delimiter runs (spec §6.2). Without that, a literal asterisk in
// prose opens a span that swallows the rest of the sentence and eats BOTH
// asterisks: `wdt:* statements … prop/direct/*` rendered as "wdt: statements …
// prop/direct/", and `mc:residedIn*)` as "mc:residedIn)". Clause by clause:
//
//   (?=…|…)      the opener must be LEFT-flanking. Either the character before it
//                is the start of the string, a space or punctuation — or, when a
//                word character precedes it, the character after it must NOT be
//                punctuation. This is what rejects `entity/Q*,` and
//                `mc:residedIn*)`.
//   \*(?!\s)     …and an opener is never followed by a space, which rejects
//                `wdt:* statements` and `orc:* → orcid`.
//   [^*\n]*…     the run to the closer: no asterisk, no newline — emphasis is
//                inline and cannot cross a paragraph.
//   (?:…|…)      the closer must be RIGHT-flanking: the character before it is not
//                a space (so `*foo *bar*` emphasises `bar`, not `foo `), and when
//                that character is punctuation the closer must be followed by a
//                space, punctuation, or the end of the string.
//   \*(?!\*)     and the closer is not the first half of a `**bold**` run.
//
// "Punctuation" is \p{P} + \p{S}, exactly CommonMark's definition — it counts
// symbols, so `→` and `%` flank like punctuation. The `u` flag is what makes those
// classes legal, and it also makes astral characters single code points. HTML
// escaping runs BEFORE this rule at the markup call sites, which is safe: `&`,
// `<`, `>`, `"` and `'` are all punctuation, and so are the `&…;` entities they
// become, so escaping never changes a character's flanking class.
//
// DELIBERATE DEVIATIONS from CommonMark, because this is one regex and not a
// delimiter stack: emphasis may not CONTAIN an asterisk, so `*a.*b*` gives
// `*a.<em>b</em>` where CommonMark gives `<em>a.*b</em>`; there is no nesting; and
// `_underscore_` emphasis is not supported at all. Checked against the reference
// CommonMark implementation over a 3,562-case sweep of flanking neighbourhoods
// (100% agreement, against 51% for the rule this replaced) and over every
// asterisk-bearing string this repo ships (47/47 lines, against 37/47).
const MD_EMPHASIS = /(?=(?:^|[\s\p{P}\p{S}])\*|[^\s\p{P}\p{S}]\*(?![\p{P}\p{S}]))(^|[^*])\*(?!\s)([^*\n]*(?:[^*\s\n\p{P}\p{S}]|[\p{P}\p{S}](?=\*(?:[\s\p{P}\p{S}]|$))))\*(?!\*)/gu;

/**
 * A card `description` reduced to plain prose.
 *
 * A description may be Markdown — headings, bullets, links (see
 * docs/dataset-cards.md; raw HTML is never allowed, in the card or here). The
 * plaza shows it in a gallery blurb and a one-paragraph hero, both of which
 * want text, not structure, so the BLOCK markers are stripped and the inline
 * ones unwrapped. Nothing here emits HTML — every caller still escapes — so
 * this is a readability helper, not a security boundary.
 *
 * On a description with no Markdown in it, this is the identity function.
 */
export function plainDescription(s) {
  // `[ \t]`, never `\s`: these run with /m over the whole text, and `\s` also
  // matches a newline — `^\s{0,3}#` would eat the blank line before a heading
  // and weld it onto the paragraph above.
  return String(s == null ? "" : s)
    .replace(/\r\n?/g, "\n")
    .replace(/^[ \t]*(?:```|~~~)[^\n]*$/gm, "")
    .replace(/^ {0,3}(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$/gm, "")
    .replace(/^[ \t]{0,3}#{1,6}[ \t]+/gm, "")
    .replace(/^[ \t]*>[ \t]?/gm, "")
    .replace(/^[ \t]*(?:[-+*]|\d+[.)])[ \t]+/gm, "• ")
    .replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, "$1")
    .replace(/\*\*([^*\n]+)\*\*/g, "$1")
    .replace(/`([^`\n]+)`/g, "$1")
    .replace(MD_EMPHASIS, "$1$2")
    .replace(/\s*\n+\s*/g, " ")
    .trim();
}
