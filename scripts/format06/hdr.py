"""Parse a 0x05 .rete header (first 1 KiB) from a local path or a URL.
usage: python hdr.py SRC... ; prints version, flags, size and the section directory."""
import struct
import sys
import urllib.request

KINDS = {1: "card", 2: "dict", 3: "index", 4: "pyramid", 5: "named", 6: "text", 7: "build",
         8: "labels", 9: "schema", 10: "quad", 11: "text-r"}


def fetch(src, off, n):
    if src.startswith("http"):
        req = urllib.request.Request(src, headers={"Range": f"bytes={off}-{off + n - 1}", "User-Agent": "rete-ws20-measure/1"})
        with urllib.request.urlopen(req, timeout=60) as r:
            cr = r.headers.get("Content-Range", "")
            total = int(cr.rsplit("/", 1)[1]) if "/" in cr else None
            return r.read(), total
    with open(src, "rb") as f:
        f.seek(0, 2)
        total = f.tell()
        f.seek(off)
        return f.read(n), total


def parse(b):
    magic, ver, flags, hlen = b[:4], b[4], b[5], struct.unpack_from("<H", b, 6)[0]
    h = {"magic": magic, "version": ver, "flags": flags, "hlen": hlen,
         "hash": b[8:24].hex(), "quads": struct.unpack_from("<Q", b, 24)[0],
         "terms": struct.unpack_from("<Q", b, 32)[0], "levels": struct.unpack_from("<H", b, 40)[0],
         "nsec": struct.unpack_from("<H", b, 44)[0], "schema_len": struct.unpack_from("<I", b, 46)[0],
         "perm_mask": b[50]}
    secs = []
    for i in range(h["nsec"]):
        k, fl, _r, off, ln = struct.unpack_from("<HHIQQ", b, 64 + 24 * i)
        secs.append((KINDS.get(k, str(k)), k, off, ln))
    h["sections"] = secs
    return h


def main():
    for src in sys.argv[1:]:
        try:
            b, total = fetch(src, 0, 1024)
            h = parse(b)
        except Exception as e:  # noqa: BLE001
            print(f"{src}\tERR {e}")
            continue
        s = " ".join(f"{n}@{o}+{l}" for n, k, o, l in h["sections"])
        print(f"{src}\tv{h['version']}\tflags={h['flags']:#x}\tsize={total}\tquads={h['quads']}\tterms={h['terms']}\t{s}")


if __name__ == "__main__":
    main()
