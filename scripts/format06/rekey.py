"""Estimate a 0x05 file's dictionary chunk directories rebuilt with shortest separator keys (what `rete convert`
would write): decode every chunk's first and last term, recompute keys, re-encode, zstd.
usage: python rekey.py LAYOUT.json FILE"""
import json, sys, zstandard
L = json.load(open(sys.argv[1])); f = open(sys.argv[2], "rb")
dz, zc = zstandard.ZstdDecompressor(), zstandard.ZstdCompressor(level=9)

def uv(b, p):
    x = s = 0
    while True:
        c = b[p]; p += 1; x |= (c & 0x7F) << s
        if c < 0x80: return x, p
        s += 7

def wv(out, x):
    while x >= 0x80: out.append(x & 0x7F | 0x80); x >>= 7
    out.append(x)

def sep(prev_last, first):
    i = 0
    while i < len(prev_last) and i < len(first) and prev_last[i] == first[i]: i += 1
    return first[:i + 1]

tot_old = tot_new = tot_new_z = tot_old_z = 0
regs = [r for r in L["regions"] if r[2] == "dict"]
for si in range(4):
    dirr = [r for r in regs if r[3] == f"s{si}.chunk-dir"]
    ch = [r for r in regs if r[3] == f"s{si}.chunks"]
    if not dirr: continue
    a, b = dirr[0][0], dirr[0][1]
    f.seek(a); d = f.read(b - a)
    n, p = uv(d, 0); ents = []
    for _ in range(n):
        dr, p = uv(d, p); kl, p = uv(d, p); key = d[p:p + kl]; p += kl; cl, p = uv(d, p); ents.append((dr, key, cl))
    pos = ch[0][0]; firsts, lasts = [], []
    for dr, key, cl in ents:
        f.seek(pos); body = dz.decompress(f.read(cl), max_output_size=64 << 20); pos += cl
        q = 0; term = b""; first = None
        while q < len(body):
            sh, q = uv(body, q); sl, q = uv(body, q); term = term[:sh] + body[q:q + sl]; q += sl
            if first is None: first = term
        firsts.append(first or b""); lasts.append(term)
    out = bytearray(); wv(out, n)
    for i, (dr, key, cl) in enumerate(ents):
        k = b"" if i == 0 else sep(lasts[i - 1], firsts[i])
        wv(out, dr); wv(out, len(k)); out += k; wv(out, cl)
    tot_old += len(d); tot_new += len(out); tot_old_z += len(zc.compress(d)); tot_new_z += len(zc.compress(bytes(out)))
print(f"{sys.argv[2]}: chunk dirs raw {tot_old:,} -> {tot_new:,}; zstd {tot_old_z:,} -> {tot_new_z:,}")
