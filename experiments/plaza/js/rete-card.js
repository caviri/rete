// rete-card.js — read a .rete file's self-description with zero dependencies.
//
// A `.rete` file is self-describing: its first 128 bytes are a fixed header
// (SPEC.md §4.1) that points at every section, and an optional **Dataset Card**
// — compact JSON — sits in the metadata section at [metadata_offset ..
// metadata_offset+metadata_len). So a static page can learn what a dataset *is*
// with two tiny HTTP range requests (header, then card), never touching the
// index or downloading the file. That is the whole premise of the plaza.
//
// Mirrors crates/rete-core/src/header.rs (little-endian on disk). Reads the
// formats the bundled engine reads, 0x05 to 0x08: 0x05 is a 1024-byte header
// with a section directory and a plain JSON card; 0x06 to 0x08 are a 128-byte
// header that places the card in its region, stored with zstd (docs/SPEC.md §4).
// The file explorer's reader (experiments/rete-file-explorer/js/rete-fs.js)
// makes the same checks and also keeps every section.

import { decompress as zstdDecompress } from "../vendor/fzstd.mjs";

export const HEADER_LEN = 1024; // fetch this many header bytes (covers both layouts)
const MAGIC = "RETE";
const OLDEST = 0x05;
const NEWEST = 0x08;
const V2_HEADER = 128;
const CODEC_ZSTD = 1;
const MAX_CARD = 16 << 20; // a stored card; rete-core caps it at 64 MiB decoded

const dec = new TextDecoder();
const hex = (bytes) =>
  Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
const formatLabel = (v) => `0x${Number(v).toString(16).padStart(2, "0")}`;

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
  if (version > NEWEST) throw new Error(`format ${formatLabel(version)} is newer than the plaza reads (up to ${formatLabel(NEWEST)})`);
  if (version < OLDEST) throw new Error(`format ${formatLabel(version)} is older than the plaza reads (from ${formatLabel(OLDEST)})`);

  if (version >= 6) {
    // 128-byte header: file length, root and region lengths, data offset, counts,
    // and the card's place inside the region that follows the root block.
    if (b.length < V2_HEADER) throw new Error(`${formatLabel(version)} header truncated: ${b.length} bytes`);
    const fileLength = u64(24), rootLen = u32(32), regionLen = u32(36), dataOffset = u64(40);
    const cardAt = u32(80), cardLen = u32(84);
    if (u16(6) !== V2_HEADER || V2_HEADER + rootLen + regionLen > dataOffset || dataOffset > fileLength) {
      throw new Error("the header's regions are not in order — a damaged or truncated file");
    }
    if (cardLen && cardAt + cardLen > regionLen) throw new Error("the header puts the card outside its region — a damaged file");
    const namedGraphCount = u64(64);
    return {
      version, flags, hasQuads: (flags & 1) !== 0,
      metadataOffset: cardLen ? V2_HEADER + rootLen + cardAt : 0,
      metadataLen: cardLen,
      cardCodec: b[75],
      fileLength,
      contentHash: hex(b.subarray(8, 24)),
      quadCount: u64(48), termCount: u64(56),
      namedGraphCount,
      hasNamedGraphs: namedGraphCount > 0,
    };
  }

  {
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
    return { version, flags, hasQuads: (flags & 1) !== 0, metadataOffset, metadataLen, cardCodec: 0, fileLength: null, contentHash, quadCount, termCount, hasNamedGraphs };
  }
}

/** The card's JSON from its stored bytes: zstd from 0x06 on, plain in 0x05. */
function decodeCard(raw, codec) {
  let bytes = raw;
  if (codec === CODEC_ZSTD) bytes = zstdDecompress(raw);
  else if (codec !== 0) throw new Error(`card codec ${codec} is not one the plaza reads`);
  return JSON.parse(dec.decode(bytes));
}

/**
 * Fetch [start, end] (inclusive) of a URL via an HTTP Range request. Falls back
 * to slicing a full 200 response if the server ignores Range (the R2 origin the
 * remote datasets live on honours ranges, as the playground relies on).
 */
export async function rangeFetch(url, start, end) {
  const res = await fetch(url, { headers: { Range: `bytes=${start}-${end}` } });
  if (!res.ok && res.status !== 206) throw new Error(`HTTP ${res.status} for ${url}`);
  const full = new Uint8Array(await res.arrayBuffer());
  // 206 → already the requested slice; 200 → the whole body, slice it.
  return res.status === 206 ? full : full.subarray(start, end + 1);
}

/**
 * Read a dataset's header + embedded card (+ total file size).
 *   source: a URL string, or an already-loaded Uint8Array/ArrayBuffer.
 * Returns { header, card, size } — card is null when the file carries none;
 * size is the total .rete byte length (from Content-Range, or the full body).
 */
export async function readReteCard(source) {
  // Already in memory (e.g. a bundled file fully fetched): read in place.
  if (source instanceof Uint8Array || source instanceof ArrayBuffer) {
    const b = source instanceof Uint8Array ? source : new Uint8Array(source);
    const header = parseHeader(b);
    let card = null;
    if (header.metadataLen > 0 && header.metadataOffset + header.metadataLen <= b.length) {
      const slice = b.subarray(header.metadataOffset, header.metadataOffset + header.metadataLen);
      card = decodeCard(slice, header.cardCodec);
    }
    return { header, card, size: b.length };
  }

  // Remote / local-over-http: header request first (captures total size).
  const url = source;
  const res = await fetch(url, { headers: { Range: `bytes=0-${HEADER_LEN - 1}` } });
  if (!res.ok && res.status !== 206) throw new Error(`HTTP ${res.status} for ${url}`);
  const full = new Uint8Array(await res.arrayBuffer());
  let size = null;
  const cr = res.headers.get("Content-Range"); // "bytes 0-127/12345678"
  if (cr) { const m = cr.match(/\/(\d+)\s*$/); if (m) size = +m[1]; }
  if (size == null && res.status === 200) size = full.length; // whole body returned
  const head = res.status === 206 ? full : full.subarray(0, HEADER_LEN);
  const header = parseHeader(head);
  if (header.fileLength) size = header.fileLength; // 0x06 on: the header says
  let card = null;
  // A card past MAX_CARD is not read: the tile falls back to the header's counts.
  if (header.metadataLen > 0 && header.metadataLen <= MAX_CARD) {
    const start = header.metadataOffset;
    const end = start + header.metadataLen - 1;
    const metaBytes = await rangeFetch(url, start, end);
    card = decodeCard(metaBytes.subarray(0, header.metadataLen), header.cardCodec);
  }
  return { header, card, size };
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
    named_graph_count: header.namedGraphCount ?? (header.hasNamedGraphs ? 1 : 0),
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
