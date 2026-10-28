// What a .rete can answer, read from its header and its Dataset Card alone —
// the index-free CARD tier: two small reads, however large the file.
//
// The discovery tools ask this before they open a graph. Opening a large
// published file reads its index directories first (106 MB and 17 s for the
// 66 GB crossref.rete), and the files built by the low-memory builder carry no
// schema pyramid — so no schema, and no label index to search names in — nor,
// unless built with --text-index, a full-text index. Without this check
// dataset_schema and find_entities paid that open to answer with nothing, and
// find_entities' "nothing matched" read as a spelling problem.
import { open as openFile } from "node:fs/promises";

const HEAD = 1024; // the header and its section directory (SPEC.md 4.1)
const SECTION_COUNT_AT = 44;
const SECTIONS_AT = 64;
const SECTION_LEN = 24;
const FLAGS_AT = 5;
const FLAG_QUOTED_TRIPLES = 0b100;
const KIND = { metadata: 1, pyramidMeta: 4, textIndex: 6 };
const KEEP = 200; // tiers held, per bytes

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

/** The fixed header: version, flags and the section directory; null when it is not one this reads. */
function parseHead(b) {
  if (b.length < HEAD || String.fromCharCode(...b.subarray(0, 4)) !== "RETE") return null;
  const view = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const count = view.getUint16(SECTION_COUNT_AT, true);
  if (SECTIONS_AT + count * SECTION_LEN > HEAD) return null;
  const sections = [];
  for (let i = 0; i < count; i++) {
    const at = SECTIONS_AT + i * SECTION_LEN;
    sections.push({ kind: view.getUint16(at, true), offset: Number(view.getBigUint64(at + 8, true)), length: Number(view.getBigUint64(at + 16, true)) });
  }
  return { version: b[4], flags: b[FLAGS_AT], sections };
}


async function readTier(source) {
  const reader = rangeReader(source);
  const head = parseHead(await reader.read(0, HEAD));
  if (!head) return { readable: false };
  const stats = { bytesRead: HEAD, requests: 1 };
  const section = (kind) => head.sections.find((s) => s.kind === kind && s.length > 0);
  const meta = section(KIND.metadata);
  let card = null;
  if (meta) {
    const bytes = await reader.read(meta.offset, meta.length);
    stats.bytesRead += bytes.length;
    stats.requests += 1;
    card = JSON.parse(new TextDecoder().decode(bytes));
    // Measured from the header, as the client's card() does, so every file
    // answers it — even one built before the signal existed.
    if (!card.signals || typeof card.signals !== "object") card.signals = {};
    card.signals.quoted_triples =
      head.flags & FLAG_QUOTED_TRIPLES
        ? { present: true, export_surfaces: ["rdf12", "rdf-star"], export_default: "rdf12" }
        : { present: false };
  }
  // The schema and the label index that name search reads both live in the
  // pyramid section, which the low-memory builder does not write.
  const pyramid = Boolean(section(KIND.pyramidMeta));
  return {
    readable: true,
    card,
    fileBytes: await reader.size(),
    stats,
    capabilities: {
      card: Boolean(card),
      schema: pyramid,
      name_search: pyramid,
      full_text_search: Boolean(section(KIND.textIndex)),
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
