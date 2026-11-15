// The explorer's header reader (both layouts) and its catalogs.
//
//   node --test experiments/rete-file-explorer/test/*.test.mjs
//
// The header tests build a generation-2 file by hand from docs/SPEC.md §4.1–4.2:
// a 128-byte header, a root block whose section table lists two sections, and a
// card region holding a zstd-framed card — so the reader's offsets are checked
// against the spec, not against itself.
import { test } from "node:test";
import assert from "node:assert/strict";

import { cardFromBytes, decodeCard, parseHeader } from "../js/rete-fs.js";
import { normalizeCatalog } from "../js/library.js";

/** A zstd frame holding `bytes` in one raw block (RFC 8878 §3.1): what fzstd must read. */
function zstdRaw(bytes) {
  const n = bytes.length;
  if (n > 255) throw new Error("keep the test card small");
  const frame = new Uint8Array(4 + 1 + 1 + 3 + n);
  frame.set([0x28, 0xb5, 0x2f, 0xfd]); // magic, little-endian
  frame[4] = 0x20; // single segment, 1-byte content size, no checksum
  frame[5] = n;
  const block = 1 | (0 << 1) | (n << 3); // last block, raw, size n
  frame.set([block & 0xff, (block >> 8) & 0xff, (block >> 16) & 0xff], 6);
  frame.set(bytes, 9);
  return frame;
}

const CARD = { title: "A test graph", triple_count: 3, classes: [["http://ex/C", 2]] };

/** A generation-2 file: header, root (two sections), card region, front, data, footer. */
function file08() {
  const card = zstdRaw(new TextEncoder().encode(JSON.stringify(CARD)));
  const record = new TextEncoder().encode('{"builder":"test"}');
  const sections = [
    { kind: 1, offset: 0, length: 40 }, // schema, front region
    { kind: 17, offset: 16, length: 100 }, // index, data region
  ];
  const rootLen = 2 + sections.length * 38 + 4;
  const regionLen = card.length + record.length + 10;
  const frontBase = 128 + rootLen + regionLen;
  const dataOffset = frontBase + 64;
  const fileLen = dataOffset + 200 + 8;

  const b = new Uint8Array(fileLen);
  const v = new DataView(b.buffer);
  b.set([0x52, 0x45, 0x54, 0x45]);
  b[4] = 0x08;
  b[5] = 0b11; // named graphs, quoted triples
  v.setUint16(6, 128, true);
  b.set(Array.from({ length: 16 }, (_, i) => i), 8); // content hash
  v.setBigUint64(24, BigInt(fileLen), true);
  v.setUint32(32, rootLen, true);
  v.setUint32(36, regionLen, true);
  v.setBigUint64(40, BigInt(dataOffset), true);
  v.setBigUint64(48, 1234n, true); // statements
  v.setBigUint64(56, 567n, true); // terms
  v.setBigUint64(64, 2n, true); // named graphs
  b[75] = 1; // internal codec: zstd
  v.setUint16(78, 0b1101, true); // text, labels, schema
  v.setUint32(80, 0, true); // card at the region's start
  v.setUint32(84, card.length, true);
  v.setUint32(104, card.length, true); // the record right after it
  v.setUint32(108, record.length, true);

  b[128] = 1; // root version
  b[129] = sections.length;
  sections.forEach((s, i) => {
    const p = 130 + i * 38;
    b[p] = s.kind;
    v.setBigUint64(p + 6, BigInt(s.offset), true);
    v.setBigUint64(p + 14, BigInt(s.length), true);
    b.fill(0xab, p + 22, p + 38); // digest
  });
  b.set(card, 128 + rootLen);
  b.set(record, 128 + rootLen + card.length);
  b.set([0x07, 0, 0, 0, 0x52, 0x45, 0x54, 0x45], fileLen - 8);
  return { b, rootLen, regionLen, frontBase, dataOffset, fileLen, card };
}

test("a 0x08 header: counts, presence, and every section at its absolute offset", () => {
  const { b, rootLen, frontBase, dataOffset, fileLen, card } = file08();
  const h = parseHeader(b);
  assert.equal(h.generation, 2);
  assert.equal(h.version, 8);
  assert.equal(h.fileLength, fileLen);
  assert.equal(h.quadCount, 1234);
  assert.equal(h.termCount, 567);
  assert.equal(h.namedGraphCount, 2);
  assert.equal(h.hasQuads, true);
  assert.equal(h.quoted, true);
  assert.deepEqual(h.presence, { text: true, textPartial: false, labels: true, schema: true, pyramid: false, quads: false });
  assert.equal(h.contentHash, "000102030405060708090a0b0c0d0e0f");
  assert.deepEqual(h.card, { offset: 128 + rootLen, length: card.length, codec: 1 });
  assert.equal(h.sectionsComplete, true);

  const byName = Object.fromEntries(h.sections.map((s) => [s.name, s]));
  assert.equal(byName["ROOT BLOCK"].offset, 128);
  assert.equal(byName.SCHEMA.offset, frontBase, "a front section is relative to the front");
  assert.equal(byName.INDEX.offset, dataOffset + 16, "a data section is relative to the data offset");
  assert.equal(byName.INDEX.length, 100);
  assert.equal(byName.INDEX.digest, "ab".repeat(16));
  assert.equal(byName["FRONT SECTIONS"], undefined, "the table replaces the coarse regions");
});

test("the card is read from the bytes and decoded with zstd", () => {
  const { b } = file08();
  assert.deepEqual(cardFromBytes(b, parseHeader(b)), CARD);
  // Generation 1 stores it plain.
  assert.deepEqual(decodeCard(new TextEncoder().encode('{"title":"plain"}'), 0), { title: "plain" });
});

test("a 0x08 header read alone gives the file's regions, and says the table is missing", () => {
  const { b, frontBase, dataOffset } = file08();
  const h = parseHeader(b.subarray(0, 128));
  assert.equal(h.sectionsComplete, false);
  const names = h.sections.map((s) => s.name);
  assert.deepEqual(names, ["ROOT BLOCK", "CARD", "BUILD RECORD", "FRONT SECTIONS", "DATA SECTIONS"]);
  const front = h.sections.find((s) => s.name === "FRONT SECTIONS");
  assert.deepEqual([front.offset, front.length], [frontBase, dataOffset - frontBase]);
});

test("a header whose regions are out of order, or whose card leaves its region, is refused", () => {
  const { b } = file08();
  const bad = b.slice();
  new DataView(bad.buffer).setBigUint64(40, BigInt(bad.length + 1), true); // data past the end
  assert.throws(() => parseHeader(bad), /not in order/);
  const bad2 = b.slice();
  new DataView(bad2.buffer).setUint32(84, 0xffff_ffff, true); // a card longer than its region
  assert.throws(() => parseHeader(bad2), /outside its region/);
  // A section table that runs past the file is not trusted: regions only.
  const bad3 = b.slice();
  new DataView(bad3.buffer).setBigUint64(130 + 38 + 14, BigInt(1) << 40n, true);
  assert.equal(parseHeader(bad3).sectionsComplete, false);
});

test("a 0x05 header still reads as the 1 KB directory it is", () => {
  const b = new Uint8Array(1024);
  const v = new DataView(b.buffer);
  b.set([0x52, 0x45, 0x54, 0x45]);
  b[4] = 0x05;
  v.setUint16(6, 1024, true);
  v.setBigUint64(24, 99n, true);
  v.setBigUint64(32, 42n, true);
  v.setUint16(44, 2, true);
  v.setUint16(64, 1, true); // metadata
  v.setBigUint64(72, 1024n, true);
  v.setBigUint64(80, 17n, true);
  v.setUint16(88, 3, true); // index
  v.setBigUint64(96, 2048n, true);
  v.setBigUint64(104, 4096n, true);
  const h = parseHeader(b);
  assert.equal(h.generation, 1);
  assert.equal(h.quadCount, 99);
  assert.equal(h.termCount, 42);
  assert.deepEqual(h.card, { offset: 1024, length: 17, codec: 0 });
  assert.deepEqual(h.sections.map((s) => [s.name, s.offset, s.length]), [["METADATA", 1024, 17], ["INDEX", 2048, 4096]]);
});

test("formats outside 0x05–0x08, the engine's, are named, not misread", () => {
  const b = new Uint8Array(1024);
  b.set([0x52, 0x45, 0x54, 0x45]);
  b[4] = 0x09;
  assert.throws(() => parseHeader(b), /0x09 is newer/);
  b[4] = 0x04;
  assert.throws(() => parseHeader(b), /0x04 is older/);
  assert.throws(() => parseHeader(new TextEncoder().encode("PK\x03\x04".padEnd(1024))), /not a \.rete/);
});

test("rete's catalog: names and lines from its labels, a URL from remoteBase when there is none, shards left out", () => {
  const cat = normalizeCatalog(
    {
      remoteBase: "https://data.example.org/",
      datasets: [
        { key: "boe", label: "boe.rete — Spanish law: 12,330 laws (remote, lazy)", url: "https://data.example.org/boe/boe-2026.rete", size: "8.9 MB", triples: "1.2 M", license: "CC-BY-4.0 (BOE)", tags: ["law"] },
        { key: "mini", label: "Mini", description: "A **small** [graph](https://x)." },
        { key: "split", label: "Split", shards: ["https://a/1.rete", "https://a/2.rete"] },
      ],
    },
    "https://raw.example.org/catalog.json",
  );
  assert.equal(cat.name, "raw.example.org");
  assert.equal(cat.titled, false);
  assert.deepEqual(
    cat.datasets.map((d) => [d.key, d.name, d.blurb, d.url, d.sizeText, d.license]),
    [
      ["boe", "boe", "Spanish law: 12,330 laws", "https://data.example.org/boe/boe-2026.rete", "8.9 MB", "CC-BY-4.0"],
      ["mini", "Mini", "A small graph.", "https://data.example.org/mini/mini.rete", null, null],
    ],
  );
});

test("the plaza's shape, relative links, a bare list of links; nothing but http(s)", () => {
  const plaza = normalizeCatalog(
    { title: "rete plaza", tagline: "a gathering place", datasets: [{ key: "scholar", title: "scholar — synthetic world", rete: "data/scholar.rete", icon: "🎓", blurb: "250 papers" }] },
    "https://example.org/plaza/plaza.json",
  );
  assert.equal(plaza.name, "rete plaza");
  assert.equal(plaza.titled, true);
  assert.equal(plaza.description, "a gathering place");
  assert.deepEqual(
    [plaza.datasets[0].url, plaza.datasets[0].name, plaza.datasets[0].blurb, plaza.datasets[0].icon],
    ["https://example.org/plaza/data/scholar.rete", "scholar", "250 papers", "🎓"],
  );

  const list = normalizeCatalog(["https://h/a/x.rete", "https://h/a/x.rete", "file:///etc/passwd", "javascript:alert(1)"], "https://h/list.json");
  assert.deepEqual(list.datasets.map((d) => [d.key, d.url]), [["x", "https://h/a/x.rete"]]);

  assert.throws(() => normalizeCatalog({ hello: 1 }, "https://h/"), /no dataset list/);
});
