// Test fixtures made in memory, so the tests need no network and no binary files in the repository:
//   png(w, h, rgb)        a solid-colour PNG
//   pmtiles()             a PMTiles v3 archive with one vector tile (z0): layer "shapes", a square "Testland"
//                         (-90..90 lon) and a point "Centre"
import { crc32, deflateSync } from "node:zlib";

export function png(w = 64, h = 48, rgb = [42, 111, 176]) {
  const chunk = (type, data) => {
    const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td) >>> 0);
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 2; // 8-bit RGB
  const row = Buffer.alloc(1 + w * 3);
  for (let x = 0; x < w; x++) row.set(rgb, 1 + x * 3);
  const raw = Buffer.concat(Array.from({ length: h }, () => row));
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

// ---- protobuf, just enough for a Mapbox Vector Tile
const varint = (n) => { const out = []; n = BigInt(n); do { let b = Number(n & 0x7fn); n >>= 7n; if (n) b |= 0x80; out.push(b); } while (n); return Buffer.from(out); };
const key = (field, wire) => varint((field << 3) | wire);
const ld = (field, buf) => Buffer.concat([key(field, 2), varint(buf.length), buf]);
const vi = (field, n) => Buffer.concat([key(field, 0), varint(n)]);
const packed = (field, nums) => ld(field, Buffer.concat(nums.map(varint)));
const zz = (n) => (n << 1) ^ (n >> 31);
const cmd = (id, count) => (id & 7) | (count << 3);

function mvt() {
  const keys = ["name"], values = ["Testland", "Centre"];
  // square, exterior ring clockwise in tile coordinates (y down)
  const square = [cmd(1, 1), zz(1024), zz(1024), cmd(2, 3), zz(2048), zz(0), zz(0), zz(2048), zz(-2048), zz(0), cmd(7, 1)];
  const point = [cmd(1, 1), zz(2048), zz(2048)];
  const feature = (id, tags, type, geom) => ld(2, Buffer.concat([vi(1, id), packed(2, tags), vi(3, type), packed(4, geom)]));
  const layer = Buffer.concat([
    vi(15, 2), ld(1, Buffer.from("shapes")),
    feature(1, [0, 0], 3, square), feature(2, [0, 1], 1, point),
    ...keys.map((k) => ld(3, Buffer.from(k))),
    ...values.map((v) => ld(4, ld(1, Buffer.from(v)))),
    vi(5, 4096),
  ]);
  return ld(3, layer);
}

export function pmtiles() {
  const tile = mvt();
  const meta = Buffer.from(JSON.stringify({ name: "Test shapes", attribution: "test fixture", vector_layers: [{ id: "shapes", fields: { name: "String" }, minzoom: 0, maxzoom: 0 }] }));
  // root directory: one entry, tile id 0 (z0/0/0), run length 1, its length, offset 0 (written as offset + 1)
  const dir = Buffer.concat([varint(1), varint(0), varint(1), varint(tile.length), varint(1)]);
  const H = 127;
  const rootOff = H, metaOff = rootOff + dir.length, dataOff = metaOff + meta.length;
  const h = Buffer.alloc(H);
  h.write("PMTiles", 0, "ascii"); h[7] = 3;
  const u64 = (o, v) => h.writeBigUInt64LE(BigInt(v), o);
  u64(8, rootOff); u64(16, dir.length); u64(24, metaOff); u64(32, meta.length); u64(40, dataOff + tile.length); u64(48, 0);
  u64(56, dataOff); u64(64, tile.length); u64(72, 1); u64(80, 1); u64(88, 1);
  h[96] = 1; h[97] = 1; h[98] = 1; h[99] = 1; h[100] = 0; h[101] = 0; // clustered, no compression, MVT, z0..z0
  const e7 = (v) => Math.round(v * 1e7);
  h.writeInt32LE(e7(-180), 102); h.writeInt32LE(e7(-85), 106); h.writeInt32LE(e7(180), 110); h.writeInt32LE(e7(85), 114);
  h[118] = 0; h.writeInt32LE(0, 119); h.writeInt32LE(0, 123);
  return Buffer.concat([h, dir, meta, tile]);
}
