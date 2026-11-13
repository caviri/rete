// The CARD tier on malformed or hostile headers: every length and offset in a
// header comes from the file, so none of them may size a read on its own.
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { CardTier } from "../src/capabilities.mjs";

const HEAD = 1024;
const tierOf = (source) => new CardTier({ stamp: async () => "1" }).of(source);

/** A 0x08 header in a 1 KiB file, with the given fields (SPEC.md §4.1). */
function header08({ fileLen = HEAD, rootLen = 0, regionLen = 0, dataOffset = 128, cardAt = 0, cardLen = 0, buildAt = 0, buildLen = 0 } = {}) {
  const b = new Uint8Array(HEAD);
  const v = new DataView(b.buffer);
  b.set([0x52, 0x45, 0x54, 0x45]);
  b[4] = 0x08;
  v.setUint16(6, 128, true);
  v.setBigUint64(24, BigInt(fileLen), true);
  v.setUint32(32, rootLen, true);
  v.setUint32(36, regionLen, true);
  v.setBigUint64(40, BigInt(dataOffset), true);
  v.setUint32(80, cardAt, true);
  v.setUint32(84, cardLen, true);
  v.setUint32(104, buildAt, true);
  v.setUint32(108, buildLen, true);
  return b;
}

/** A 0x05 header whose directory lists one metadata section (SPEC.md §4.1, generation 1). */
function header05({ offset = HEAD, length }) {
  const b = new Uint8Array(HEAD);
  const v = new DataView(b.buffer);
  b.set([0x52, 0x45, 0x54, 0x45]);
  b[4] = 0x05;
  v.setUint16(6, HEAD, true);
  v.setUint16(44, 1, true);
  v.setUint16(64, 1, true); // kind 1: metadata
  v.setBigUint64(64 + 8, BigInt(offset), true);
  v.setBigUint64(64 + 16, BigInt(length), true);
  return b;
}

// A card length of 0xFFFFFFFF that the header's other fields agree with.
const hugeCard = () => {
  const regionLen = 0xffff_ffff;
  const dataOffset = 128 + regionLen;
  return { bytes: header08({ fileLen: dataOffset, regionLen, dataOffset, cardLen: 0xffff_ffff }), claims: dataOffset };
};

/**
 * Serve `bytes` over HTTP Range as a file of `claims` bytes (zeros past the
 * real ones), recording every range asked for. `mode` makes it misbehave.
 */
async function serve(t, bytes, { claims = bytes.length, mode = "honest" } = {}) {
  const ranges = [];
  const closed = [];
  const server = createServer((req, res) => {
    res.on("close", () => closed.push(Date.now()));
    if (mode === "ignores-range") {
      // The whole "file", streamed for as long as the client keeps reading.
      res.writeHead(200, { "content-length": String(claims) });
      const chunk = Buffer.alloc(64 * 1024);
      let sent = 0;
      const pump = () => {
        while (sent < claims) {
          sent += chunk.length;
          if (!res.write(chunk)) return void res.once("drain", pump);
        }
        res.end();
      };
      return pump();
    }
    const [, a, b] = /bytes=(\d+)-(\d+)/.exec(req.headers.range ?? "") ?? [];
    const start = Number(a);
    const end = Number(b);
    ranges.push(end - start + 1);
    const body = Buffer.alloc(mode === "long-206" ? (end - start + 1) * 4 : end - start + 1);
    Buffer.from(bytes.subarray(start, end + 1)).copy(body);
    res.writeHead(206, { "content-range": `bytes ${start}-${end}/${claims}`, "content-length": String(body.length) });
    res.end(body);
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  t.after(() => new Promise((r) => server.close(r)));
  return { url: `http://127.0.0.1:${server.address().port}/x.rete`, ranges, closed };
}

const published = (url) => ({ kind: "published", url, cacheKey: url });

test("a card length of 0xFFFFFFFF is refused after the header read alone", async (t) => {
  const { bytes, claims } = hugeCard();
  const { url, ranges } = await serve(t, bytes, { claims });
  const tier = await tierOf(published(url));
  assert.equal(tier.readable, false);
  assert.deepEqual(ranges, [HEAD]);
});

test("a 0x05 metadata length of 2^40 is refused after the header read alone", async (t) => {
  const { url, ranges } = await serve(t, header05({ length: 2 ** 40 }), { claims: 2 ** 41 });
  const tier = await tierOf(published(url));
  assert.equal(tier.readable, false);
  assert.deepEqual(ranges, [HEAD]);
});

test("local files: huge lengths, a section past the end, a header that disagrees with the file", async (t) => {
  const dir = await mkdtemp(join(tmpdir(), "rete-tier-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const cases = {
    "card-ffffffff": hugeCard().bytes,
    "meta-2e40": header05({ length: 2 ** 40 }),
    "meta-past-end": header05({ offset: HEAD, length: 4096 }), // small, but the file is 1 KiB
    "card-outside-region": header08({ regionLen: 64, dataOffset: 192, cardLen: 4096 }),
    "file-length-lies": header08({ fileLen: 1 << 30, regionLen: 64, dataOffset: 192, cardLen: 32 }),
  };
  for (const [name, bytes] of Object.entries(cases)) {
    const path = join(dir, `${name}.rete`);
    await writeFile(path, bytes);
    const tier = await tierOf({ kind: "local", path, cacheKey: path });
    assert.equal(tier.readable, false, name);
  }
});

test("a host that ignores Range is cut off, not read to the end", async (t) => {
  const { url, closed } = await serve(t, new Uint8Array(0), { claims: 1 << 30, mode: "ignores-range" });
  await assert.rejects(tierOf(published(url)), /HTTP 200/);
  // The response was cancelled: the server sees the connection close, well
  // before it could have sent its gigabyte.
  for (let i = 0; i < 40 && !closed.length; i++) await new Promise((r) => setTimeout(r, 50));
  assert.equal(closed.length, 1);
});

test("a range response longer than the range is cut at the range", async (t) => {
  const { url } = await serve(t, header08({ regionLen: 0, dataOffset: 128 }), { mode: "long-206" });
  const tier = await tierOf(published(url));
  assert.equal(tier.readable, true); // a header with no card: readable, and read once
  assert.equal(tier.stats.bytesRead, HEAD);
});
