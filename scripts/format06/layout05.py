"""Map every byte of a format-0x05 .rete file to (section, part, class), reading only framing and directories.

Classes (the WS20 split):
  root   - header, container framing, section preambles, label root, quad preamble + P0, schema
  dir    - directories: dictionary chunk directories, permutation tile directories + synopsis trailers,
           text-index token table, quad tile directories, quad graph-directory blocks, label directory nodes
  card   - the Dataset Card (kind 1)
  meta   - legacy pyramid-meta (kind 4: superedges, stats, legacy label block), named graphs (kind 5)
  build  - the build record (kind 7)
  dead   - bytes no lazy reader reads (dictionary restart tables)
  cold   - payload: dictionary chunks, index tiles, postings, label data blocks, quad tiles

usage: python layout05.py SRC [--json OUT]   (SRC = local path or https URL; a URL is read by range only)
"""
import json
import struct
import sys
import urllib.request

import zstandard

KINDS = {1: "card", 2: "dict", 3: "index", 4: "pyramid", 5: "named", 6: "text", 7: "build",
         8: "labels", 9: "schema", 10: "quad", 11: "text"}
PERMS = ["SPO", "POS", "OSP", "SOP", "PSO", "OPS"]


class Src:
    """Range reader with a page cache; counts the bytes it fetched (remote reads only what parsing needs)."""

    PAGE = 64 << 10

    def __init__(self, src):
        self.src, self.pages, self.fetched, self.requests = src, {}, 0, 0
        if src.startswith("http"):
            b, self.size = self._get(0, 1024, raw=True)
            self.pages_from(0, b)
        else:
            self.f = open(src, "rb")
            self.f.seek(0, 2)
            self.size = self.f.tell()

    def _get(self, off, n, raw=False):
        req = urllib.request.Request(self.src, headers={"Range": f"bytes={off}-{off + n - 1}",
                                                        "User-Agent": "rete-ws20-measure/1"})
        with urllib.request.urlopen(req, timeout=120) as r:
            body = r.read()
            total = int(r.headers["Content-Range"].rsplit("/", 1)[1])
        self.fetched += len(body)
        self.requests += 1
        return (body, total) if raw else body

    def pages_from(self, off, b):
        pass  # the header read is not page aligned; keep it simple: re-read through pages

    def read(self, off, n):
        if n <= 0:
            return b""
        n = min(n, self.size - off)
        if not self.src.startswith("http"):
            self.f.seek(off)
            return self.f.read(n)
        first, last = off // self.PAGE, (off + n - 1) // self.PAGE
        missing = [p for p in range(first, last + 1) if p not in self.pages]
        # fetch missing pages in contiguous runs
        i = 0
        while i < len(missing):
            j = i
            while j + 1 < len(missing) and missing[j + 1] == missing[j] + 1:
                j += 1
            a = missing[i] * self.PAGE
            b = min((missing[j] + 1) * self.PAGE, self.size)
            body = self._get(a, b - a)
            for k, p in enumerate(range(missing[i], missing[j] + 1)):
                self.pages[p] = body[k * self.PAGE:(k + 1) * self.PAGE]
            i = j + 1
        buf = b"".join(self.pages[p] for p in range(first, last + 1))
        s = off - first * self.PAGE
        return buf[s:s + n]


def uvarint(b, pos):
    x = shift = 0
    while True:
        c = b[pos]
        pos += 1
        x |= (c & 0x7F) << shift
        if c < 0x80:
            return x, pos
        shift += 7


class Stream:
    """Sequential varint reader over a file region, refilling from the source in growing windows."""

    def __init__(self, src, off, end, window=64 << 10):
        self.src, self.base, self.end, self.window = src, off, end, window
        self.buf, self.pos = b"", 0

    def _need(self, k):
        while self.pos + k > len(self.buf):
            want = min(max(self.window, len(self.buf)), self.end - self.base - len(self.buf))
            if want <= 0:
                raise EOFError
            self.buf += self.src.read(self.base + len(self.buf), want)

    def varint(self):
        x = shift = 0
        while True:
            self._need(1)
            c = self.buf[self.pos]
            self.pos += 1
            x |= (c & 0x7F) << shift
            if c < 0x80:
                return x
            shift += 7

    def bytes(self, n):
        self._need(n)
        v = self.buf[self.pos:self.pos + n]
        self.pos += n
        return v

    def skip(self, n):
        self.pos += n  # may run past the buffer; later reads refill from base+len(buf), so only skip at the end

    @property
    def at(self):
        return self.base + self.pos


class Layout:
    def __init__(self, src):
        self.s = Src(src)
        self.regions = []  # (start, end, section, part, cls)
        self.stats = {}
        h = self.s.read(0, 1024)
        assert h[:4] == b"RETE", "not a .rete file"
        self.version, self.flags = h[4], h[5]
        self.quads, self.terms = struct.unpack_from("<QQ", h, 24)
        nsec = struct.unpack_from("<H", h, 44)[0]
        self.perm_mask = h[50] or 0b111111
        self.perms = [p for i, p in enumerate(PERMS) if self.perm_mask >> i & 1]
        self.sections = []
        for i in range(nsec):
            k, _fl, _r, off, ln = struct.unpack_from("<HHIQQ", h, 64 + 24 * i)
            self.sections.append((k, off, ln))
        self.add(0, 1024, "header", "header", "root")
        for k, off, ln in self.sections:
            if ln == 0:
                continue
            name = KINDS.get(k, f"kind{k}")
            getattr(self, f"do_{name}", self.do_other)(k, off, ln, name)
        self.regions.sort()
        end = max(e for _, e, *_ in self.regions)
        if end < self.s.size:
            self.add(end, self.s.size, "footer", "footer", "root")

    def add(self, a, b, sec, part, cls):
        if b > a:
            self.regions.append((a, b, sec, part, cls))

    # ---- sections -------------------------------------------------------------------------
    def do_other(self, k, off, ln, name):
        cls = {"card": "card", "build": "build", "pyramid": "meta", "named": "meta", "schema": "root"}.get(name, "cold")
        self.add(off, off + ln, name, name, cls)

    def container(self, off, ln, sec):
        """Container framing: varint count, then per section varint len + payload. Returns payload ranges."""
        b = self.s.read(off, min(ln, 64))
        n, pos = uvarint(b, 0)
        self.add(off, off + pos, sec, "framing", "root")
        out, cur = [], off + pos
        for _ in range(n):
            vb = self.s.read(cur, 10)
            plen, used = uvarint(vb, 0)
            self.add(cur, cur + used, sec, "framing", "root")
            out.append((cur + used, plen))
            cur += used + plen
        return out

    def do_dict(self, k, off, ln, name):
        st = {"sections": []}
        for si, (p, plen) in enumerate(self.container(off, ln, "dict")):
            s = Stream(self.s, p, p + plen)
            header_len = s.varint()
            hstart = s.at
            term_count, interval, nrest = s.varint(), s.varint(), s.varint()
            pre_end = s.at
            self.add(p, pre_end, "dict", f"s{si}.header", "root")
            self.add(pre_end, hstart + header_len, "dict", f"s{si}.restart-table", "dead")
            d = Stream(self.s, hstart + header_len, p + plen)
            nchunks = d.varint()
            clens = []
            keybytes = 0
            for _ in range(nchunks):
                d.varint()
                kl = d.varint()
                d.bytes(kl)
                keybytes += kl
                clens.append(d.varint())
            dir_end = d.at
            self.add(hstart + header_len, dir_end, "dict", f"s{si}.chunk-dir", "dir")
            self.add(dir_end, dir_end + sum(clens), "dict", f"s{si}.chunks", "cold")
            st["sections"].append({"terms": term_count, "chunks": nchunks, "dir_bytes": dir_end - hstart - header_len,
                                   "key_bytes": keybytes, "restart_table": hstart + header_len - pre_end,
                                   "chunk_bytes": sum(clens), "payload": plen})
        self.stats["dict"] = st

    def tile_dir(self, p, plen, sec, label, synopsis):
        d = Stream(self.s, p, p + plen)
        n = d.varint()
        clens = []
        for _ in range(n):
            d.varint()
            d.varint()
            clens.append(d.varint())
        dir_end = d.at
        tiles_end = dir_end + sum(clens)
        self.add(p, dir_end, sec, f"{label}.tile-dir", "dir")
        self.add(dir_end, tiles_end, sec, f"{label}.tiles", "cold")
        if tiles_end < p + plen:
            self.add(tiles_end, p + plen, sec, f"{label}.synopsis", "dir")
        return {"tiles": n, "dir_bytes": dir_end - p, "tile_bytes": sum(clens), "synopsis_bytes": p + plen - tiles_end}

    def do_index(self, k, off, ln, name):
        parts = self.container(off, ln, "index")
        st = {}
        syn = bool(self.flags & 0b10)
        for i, (p, plen) in enumerate(parts):
            if i < len(self.perms):
                st[self.perms[i]] = self.tile_dir(p, plen, "index", self.perms[i], syn)
            else:
                self.add(p, p + plen, "index", "quad-seal", "root")
        self.stats["index"] = st

    def do_text(self, k, off, ln, name):
        p = off + (16 if k == 11 else 0)
        if k == 11:
            self.add(off, p, "text", "digest", "root")
        b = self.s.read(p, 10)
        ttl, used = uvarint(b, 0)
        self.add(p, p + used, "text", "token-table-len", "root")
        self.add(p + used, p + used + ttl, "text", "token-table", "dir")
        self.add(p + used + ttl, off + ln, "text", "postings", "cold")
        self.stats["text"] = {"kind": k, "token_table": ttl, "postings": off + ln - (p + used + ttl)}

    def do_schema(self, k, off, ln, name):
        self.add(off, off + ln, "schema", "schema", "root")
        self.stats["schema"] = {"bytes": ln}

    def do_labels(self, k, off, ln, name):
        b = self.s.read(off, 32)
        levels = b[17]
        entries = struct.unpack_from("<Q", b, 20)[0]
        root_len = struct.unpack_from("<I", b, 28)[0]
        body = off + 32 + root_len
        self.add(off, body, "labels", "preamble+root", "root")
        dir_start = off + ln  # no directory nodes when levels == 0
        node_off = None
        dz = zstandard.ZstdDecompressor()
        raw = self.s.read(off + 32, root_len)
        nodes_seen = 0
        for _lvl in range(levels):
            try:
                node = dz.decompress(raw, max_output_size=64 << 20)
            except zstandard.ZstdError:
                node = raw
            n, pos = uvarint(node, 0)
            first = None
            for _ in range(n):
                _sh, pos = uvarint(node, pos)
                sl, pos = uvarint(node, pos)
                pos += sl
                o, pos = uvarint(node, pos)
                l_, pos = uvarint(node, pos)
                _e, pos = uvarint(node, pos)
                if first is None:
                    first = (o, l_)
            node_off = body + first[0]
            dir_start = min(dir_start, node_off)
            raw = self.s.read(node_off, first[1])
            nodes_seen += 1
        self.add(body, dir_start, "labels", "data-blocks", "cold")
        self.add(dir_start, off + ln, "labels", "dir-nodes", "dir")
        self.stats["labels"] = {"levels": levels, "entries": entries, "root_len": root_len,
                                "dir_nodes_bytes": off + ln - dir_start, "blocks_bytes": dir_start - body}

    def do_quad(self, k, off, ln, name):
        s = Stream(self.s, off, off + ln)
        magic = s.bytes(4)
        assert magic == b"RQIX", magic
        _ver, mask = s.bytes(1)[0], s.bytes(1)[0]
        s.bytes(2)
        graphs, quads, block_graphs, nparts = s.varint(), s.varint(), s.varint(), s.varint()
        plens = [s.varint() for _ in range(nparts)]
        pre_end = s.at
        n = (nparts - 2) // 2
        orders = [p for i, p in enumerate(PERMS) if mask >> i & 1]
        self.add(off, pre_end, "quad", "preamble", "root")
        cur = pre_end
        st = {"graphs": graphs, "quads": quads, "orders": orders, "parts": {}}
        for i, pl in enumerate(plens):
            if i == 0:
                part, cls = "P0.graph-dir-index", "root"
            elif i <= n:
                part, cls = f"G+{orders[i - 1]}.tile-dir", "dir"
            elif i == n + 1:
                part, cls = "graph-dir-blocks", "dir"
            else:
                part, cls = f"G+{orders[i - n - 2]}.tiles", "cold"
            self.add(cur, cur + pl, "quad", part, cls)
            st["parts"][part] = pl
            cur += pl
        self.stats["quad"] = st

    # ---- reports --------------------------------------------------------------------------
    def summary(self):
        by_cls, by_part = {}, {}
        for a, b, sec, part, cls in self.regions:
            by_cls[cls] = by_cls.get(cls, 0) + b - a
            key = (sec, part.split(".")[-1] if "." in part else part, cls)
            by_part[key] = by_part.get(key, 0) + b - a
        return by_cls, by_part


def main():
    src = sys.argv[1]
    L = Layout(src)
    by_cls, by_part = L.summary()
    out = {"source": src, "size": L.s.size, "version": L.version, "flags": L.flags, "quads": L.quads,
           "terms": L.terms, "perms": L.perms, "sections": L.sections, "by_class": by_cls,
           "by_part": [[s, p, c, n] for (s, p, c), n in sorted(by_part.items())], "stats": L.stats,
           "regions": L.regions, "fetched": L.s.fetched, "requests": L.s.requests}
    if "--json" in sys.argv:
        json.dump(out, open(sys.argv[sys.argv.index("--json") + 1], "w"))
    print(f"{src}: {L.s.size:,} B, {L.quads:,} quads, {L.terms:,} terms, perms {''.join(p[0] for p in L.perms)}"
          f" (layout read: {L.s.fetched:,} B in {L.s.requests} requests)")
    for c in ("root", "dir", "card", "meta", "build", "dead", "cold"):
        print(f"  {c:6} {by_cls.get(c, 0):>16,}")
    for (s, p, c), n in sorted(by_part.items()):
        if c != "cold":
            print(f"    {s:7} {p:22} {c:6} {n:>14,}")
    print("  stats:", json.dumps(L.stats)[:3000])


if __name__ == "__main__":
    main()
