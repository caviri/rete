// What a .rete can answer, read from its header and its Dataset Card alone —
// the index-free CARD tier: two small reads, however large the file.
//
// The discovery tools ask this before they open a graph. Not every file has
// what dataset_schema and find_entities read: the low-memory builder writes no
// stats + schema section and no label index, and only --text-index writes a
// full-text one. Without this check those tools opened the graph to answer
// with nothing, and find_entities' "nothing matched" read as a spelling
// problem. The header says which sections are present (SPEC.md §4.1).
import { open as openFile } from "node:fs/promises";
import { init, wasm } from "rete-graph";

// Every file is at least 1 KiB (SPEC.md §4.4), and the first KiB holds either
// header: so the first read is that, and the engine's card reader asks for it too.
const HEAD = 1024;
const KEEP = 200; // tiers held, per bytes

// Format 0x06 and later: a 128-byte header with presence bits (SPEC.md §4.1).
const HEADER_LEN = 128;
const VERSIONS = new Set([0x06, 0x07, 0x08]);
const FLAG_QUOTED = 0b10; // flags bit1: RDF-star quoted triples
const P_TEXT = 1; // the presence bits, header bytes 78-79
const P_TEXT_PARTIAL = 2;
const P_LABELS = 4;
const P_SCHEMA = 8;

// Format 0x05 (most of the published catalogue until it is converted): a
// 1024-byte header whose section directory says what is present (SPEC.md §4.1,
// generation 1). Its card is plain JSON.
const V05 = 0x05;
const V05_SECTION_COUNT_AT = 44;
const V05_SECTIONS_AT = 64;
const V05_SECTION_LEN = 24;
const V05_FLAG_QUOTED = 0b100; // flags bit2
const K05 = { metadata: 1, pyramidMeta: 4, textIndex: 6, labelIndex: 8, schema: 9, textIndexSome: 11 };

/** The CARD tier of resolved sources, read once per file version. */
export class CardTier {
  #cache = new Map(); // source cache key + stamp -> Promise<tier>

  constructor(store) {
    this.store = store;
  }

  /** The tier; read again only when the file changed. A tier already held cost no reads this time. */
  async of(source) {
    const key = `${source.cacheKey}#${await this.store.stamp(source)}`;
    const held = this.#cache.get(key);
    if (held) return held.then((t) => (t.readable ? { ...t, stats: { bytesRead: 0, requests: 0 } } : t));
    const tier = readTier(source).catch((error) => {
      this.#cache.delete(key);
      throw error;
    });
    this.#cache.set(key, tier);
    if (this.#cache.size > KEEP) this.#cache.delete(this.#cache.keys().next().value);
    return tier;
  }
}

/** Range reads for each kind of source: HTTP Range, a file, or a chat graph's bytes. */
function rangeReader(source) {
  if (source.kind === "chat") {
    const bytes = source.bytes;
    return { size: async () => bytes.length, read: async (at, n) => new Uint8Array(bytes.subarray(at, at + n)) };
  }
  if (source.kind === "local") {
    const read = async (at, n) => {
      const file = await openFile(source.path, "r");
      try {
        const buf = new Uint8Array(n);
        const { bytesRead } = await file.read(buf, 0, n, at);
        return buf.subarray(0, bytesRead);
      } finally {
        await file.close();
      }
    };
    const size = async () => {
      const file = await openFile(source.path, "r");
      try {
        return (await file.stat()).size;
      } finally {
        await file.close();
      }
    };
    return { size, read };
  }
  let total = null;
  const read = async (at, n) => {
    const res = await fetch(source.url, { headers: { Range: `bytes=${at}-${at + n - 1}` }, signal: AbortSignal.timeout(20_000) });
    if (res.status !== 206) throw new Error(`HTTP ${res.status} for a range of ${source.url}`);
    total ??= Number(res.headers.get("content-range")?.split("/")[1]) || null;
    return new Uint8Array(await res.arrayBuffer());
  };
  return { size: async () => total, read };
}

/**
 * What the first KiB says: which layout, its flags, what is present and where
 * the card is. Null when it is not a header of a version this engine reads.
 */
function parseHead(b) {
  if (b.length < HEADER_LEN || String.fromCharCode(...b.subarray(0, 4)) !== "RETE") return null;
  return b[4] === V05 ? parseHead05(b) : parseHead06(b);
}

/** A 0x05 header: the section directory. */
function parseHead05(b) {
  if (b.length < HEAD) return null;
  const view = new DataView(b.buffer, b.byteOffset, b.byteLength);
  if (view.getUint16(6, true) !== HEAD) return null;
  const count = view.getUint16(V05_SECTION_COUNT_AT, true);
  if (V05_SECTIONS_AT + count * V05_SECTION_LEN > HEAD) return null;
  const sections = [];
  for (let i = 0; i < count; i++) {
    const at = V05_SECTIONS_AT + i * V05_SECTION_LEN;
    const length = Number(view.getBigUint64(at + 16, true));
    if (length > 0) sections.push({ kind: view.getUint16(at, true), offset: Number(view.getBigUint64(at + 8, true)), length });
  }
  const has = (kind) => sections.some((s) => s.kind === kind);
  // Older builds keep the labels and the schema inside the pyramid section;
  // newer 0x05 builds give each its own.
  const pyramid = has(K05.pyramidMeta);
  return {
    version: V05,
    quoted: (b[5] & V05_FLAG_QUOTED) !== 0,
    fileBytes: 0, // not in a 0x05 header; the range reader's Content-Range says it
    schema: pyramid || has(K05.schema),
    labels: pyramid || has(K05.labelIndex),
    text: has(K05.textIndex) || has(K05.textIndexSome),
    textPartial: !has(K05.textIndex) && has(K05.textIndexSome),
    metadata: sections.find((s) => s.kind === K05.metadata) ?? null,
  };
}

/** A 0x06–0x08 header: its presence bits and the card region (SPEC.md §4.1, §4.4). */
function parseHead06(b) {
  const view = new DataView(b.buffer, b.byteOffset, b.byteLength);
  if (!VERSIONS.has(b[4]) || view.getUint16(6, true) !== HEADER_LEN) return null;
  const rootLen = view.getUint32(32, true);
  const cardAt = view.getUint32(80, true);
  const cardLen = view.getUint32(84, true);
  const buildAt = view.getUint32(104, true);
  const buildLen = view.getUint32(108, true);
  // The card region starts after the header and the root block; the engine
  // reads the card and the build record in one span of it.
  const region = HEADER_LEN + rootLen;
  const cardEnd = cardLen || buildLen ? region + Math.max(cardLen ? cardAt + cardLen : 0, buildLen ? buildAt + buildLen : 0) : 0;
  const presence = view.getUint16(78, true);
  const has = (bit) => (presence & bit) !== 0;
  return {
    version: b[4],
    quoted: (b[5] & FLAG_QUOTED) !== 0,
    fileBytes: Number(view.getBigUint64(24, true)),
    // dataset_schema reads the stats + schema section; name search the label index.
    schema: has(P_SCHEMA),
    labels: has(P_LABELS),
    text: has(P_TEXT),
    textPartial: has(P_TEXT_PARTIAL),
    cardLen,
    cardEnd,
  };
}

/** The card's JSON, decoded by the engine (its codec, its digest check) from a prefix of the file. */
async function decodeCard(prefix) {
  await init();
  const text = wasm.card(prefix);
  return text ? JSON.parse(text) : null;
}

async function readTier(source) {
  const reader = rangeReader(source);
  let prefix = await reader.read(0, HEAD);
  const head = parseHead(prefix);
  if (!head) return { readable: false };
  const stats = { bytesRead: prefix.length, requests: 1 };
  let card = null;
  if (head.version === V05) {
    if (head.metadata) {
      const bytes = await reader.read(head.metadata.offset, head.metadata.length);
      stats.bytesRead += bytes.length;
      stats.requests += 1;
      card = JSON.parse(new TextDecoder().decode(bytes));
    }
  } else if (head.cardLen > 0) {
    if (head.cardEnd > prefix.length) {
      const rest = await reader.read(prefix.length, head.cardEnd - prefix.length);
      stats.bytesRead += rest.length;
      stats.requests += 1;
      const both = new Uint8Array(prefix.length + rest.length);
      both.set(prefix);
      both.set(rest, prefix.length);
      prefix = both;
    }
    card = await decodeCard(prefix);
  }
  if (card) {
    // Measured from the header, as the client's card() does, so every file
    // answers it — even one built before the signal existed.
    if (!card.signals || typeof card.signals !== "object") card.signals = {};
    card.signals.quoted_triples = head.quoted
      ? { present: true, export_surfaces: ["rdf12", "rdf-star"], export_default: "rdf12" }
      : { present: false };
  }
  return {
    readable: true,
    card,
    fileBytes: head.fileBytes || (await reader.size()),
    stats,
    capabilities: {
      card: Boolean(card),
      schema: head.schema,
      name_search: head.labels,
      full_text_search: head.text,
      ...(head.textPartial ? { full_text_search_partial: true } : {}),
      starter_queries: examplesOf(card).length,
      label_predicate: card?.signals?.label_predicate ?? null,
    },
  };
}

/** The runnable queries a card ships, as the client's examples() lists them. */
export const examplesOf = (card) => [
  ...(card?.queries ?? []).map((q) => ({ ...q })),
  ...(card?.example_queries ?? []).map((sparql) => ({ sparql })),
];

/** One plain line on what a graph supports, for the tools' notes. */
export function supportLine(caps) {
  const yes = (b) => (b ? "yes" : "no");
  return (
    `Supports — card: ${yes(caps.card)}; schema (dataset_schema): ${yes(caps.schema)}; ` +
    `name search (find_entities): ${yes(caps.name_search)}; full-text search: ${yes(caps.full_text_search)}` +
    (caps.label_predicate ? `; labels are ${caps.label_predicate}` : "") +
    "."
  );
}
