#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""Mirror the IIIF of every digitized Fuero Juzgo witness into our R2 bucket.

For each of the 12 digitized witnesses we (a) stream every full-res page image
from its source IIIF server straight into R2 at
  fuero_juzgo/iiif/<wkey>/<NNNN>.jpg
and (b) write a SELF-HOSTED IIIF Presentation v2 manifest that points at those
R2 images (service dropped -> static image resource) at
  fuero_juzgo/iiif/<wkey>/manifest.json
served publicly at https://data.graphplaza.com/fuero_juzgo/iiif/<wkey>/...

The converter then points fjo:iiifManifest at our R2 manifest (renders as a
viewer + downloadable) and keeps the original as fjo:sourceManifest.

Resumable (ledger of uploaded keys), polite (bounded workers + backoff on
429/503, honors Retry-After). Reads R2 creds from .env (S3_API_ENDPOINT --
the account r2.cloudflarestorage.com endpoint, NOT BUCKET_ENDPOINT).

Usage:
  python scripts/fuero_juzgo_iiif_mirror.py all           # everything
  python scripts/fuero_juzgo_iiif_mirror.py P-II-17       # one witness
  python scripts/fuero_juzgo_iiif_mirror.py P-II-17 --limit 5   # smoke test
"""
import os, sys, json, time, copy, threading, subprocess, urllib.request, urllib.error
from concurrent.futures import ThreadPoolExecutor, as_completed
import boto3

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUCKET = "rete"
PUB = "https://data.graphplaza.com"          # public read domain (bucket root)
LEDGER = os.path.join(ROOT, "data", "fuero_juzgo", "iiif_mirror.ledger")
UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/120.0 Safari/537.36")

# witnesskey -> source IIIF manifest URL. Escorial = RBME/patrimonionacional,
# BSB = MDZ, BnF = Gallica, Bodleian = Digital Bodleian. All IIIF Presentation v2.
WITNESSES = [
    ("Z-III-6",  "https://rbdigital.realbiblioteca.es/files/manifests/esc_Z-III-6.json"),
    ("Z-III-21", "https://rbdigital.realbiblioteca.es/files/manifests/esc_Z-III-21.json"),
    ("Z-III-18", "https://rbdigital.realbiblioteca.es/files/manifests/esc_Z-III-18.json"),
    ("P-II-17",  "https://rbdigital.realbiblioteca.es/files/manifests/esc_P-II-17.json"),
    ("M-II-18",  "https://rbdigital.realbiblioteca.es/files/manifests/esc_M-II-18.json"),
    ("M-III-5",  "https://rbdigital.realbiblioteca.es/files/manifests/esc_M-III-5.json"),
    ("Z-II-9",   "https://rbdigital.realbiblioteca.es/files/manifests/esc_Z-II-9.json"),
    ("d-III-18", "https://rbdigital.realbiblioteca.es/files/manifests/esc_d-III-18.json"),
    ("bsb00160754", "https://api.digitale-sammlungen.de/iiif/presentation/v2/bsb00160754/manifest"),
    ("bsb00094631", "https://api.digitale-sammlungen.de/iiif/presentation/v2/bsb00094631/manifest"),
    ("bnf-esp-256", "https://gallica.bnf.fr/iiif/ark:/12148/btv1b10033228s/manifest.json"),
    ("bodleian-holkham-46",
     "https://iiif.bodleian.ox.ac.uk/iiif/manifest/aa0dadc7-b2e9-44fa-9b1c-4d25760d5297.json"),
]


def load_env():
    env = {}
    with open(os.path.join(ROOT, ".env"), encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                env[k] = v.strip().strip('"').strip("'")
    return env


ENV = load_env()
s3 = boto3.client(
    "s3", endpoint_url=ENV["S3_API_ENDPOINT"],          # the correct write endpoint
    aws_access_key_id=ENV["ACCESS_KEY_ID"],
    aws_secret_access_key=ENV["SECRET_ACCESS_KEY"], region_name="auto")

_lock = threading.Lock()
_done = set()
if os.path.exists(LEDGER):
    _done = set(open(LEDGER, encoding="utf-8").read().split("\n"))
_led = None


def mark(key):
    with _lock:
        _led.write(key + "\n"); _led.flush(); _done.add(key)


def referer(url):
    if "gallica.bnf.fr" in url:
        return "https://gallica.bnf.fr/"
    if "patrimonionacional" in url:
        return "https://rbme.patrimonionacional.es/"
    return None


def fetch_curl(url, tries=7):
    """GET via curl -- Gallica blocks urllib's request fingerprint (403) but
    serves curl fine. `-w %{http_code}` appends the 3-digit status to stdout
    after the (binary) body, so body = out[:-3], code = out[-3:]."""
    ref = referer(url)
    cmd = ["curl", "-s", "--max-time", "120", "-A", UA]
    if ref:
        cmd += ["-e", ref]
    cmd += ["-w", "%{http_code}", url]
    last = ""
    for i in range(tries):
        out = subprocess.run(cmd, capture_output=True).stdout
        if len(out) >= 3 and out[-3:].isdigit():
            code, body = out[-3:].decode(), out[:-3]
            if code == "200":
                return body
            last = "HTTP " + code
            if code in ("403", "429", "500", "502", "503", "504"):
                time.sleep(min(120, 5 * (2 ** i))); continue
            raise RuntimeError("curl %s for %s" % (code, url))
        else:
            last = "curl empty"
        time.sleep(min(60, 2 ** i))
    raise RuntimeError("curl fetch failed after %d tries: %s (%s)" % (tries, url, last))


def fetch(url, tries=7):
    """GET bytes with UA/referer, backoff on 403/429/5xx (honors Retry-After).
    Gallica is routed through curl (urllib gets fingerprint-blocked)."""
    if "gallica.bnf.fr" in url:
        return fetch_curl(url, tries)
    last = None
    for i in range(tries):
        try:
            hdrs = {"User-Agent": UA}
            ref = referer(url)
            if ref:
                hdrs["Referer"] = ref
            req = urllib.request.Request(url, headers=hdrs)
            with urllib.request.urlopen(req, timeout=90) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            last = e
            # 403 from Gallica/Bodleian under load = rate limiting, not a hard
            # block -> back off and retry (bounded).
            if e.code in (403, 429, 500, 502, 503, 504):
                ra = e.headers.get("Retry-After")
                wait = int(ra) if (ra and ra.isdigit()) else min(120, 5 * (2 ** i))
                time.sleep(wait); continue
            raise
        except Exception as e:
            last = e
            time.sleep(min(60, 2 ** i))
    raise RuntimeError("fetch failed after %d tries: %s (%s)" % (tries, url, last))


def canvases(m):
    """[(resource, w, h)] from a IIIF v2 (sequences) or v3 (items) manifest, in
    canvas order. `resource` is the image resource dict (not a URL) -- build the
    actual image URL with image_url(), which works whether resource.@id is a
    full-image URL (Escorial/BSB/Gallica) or a bare Image-API service base
    (Bodleian)."""
    out = []
    for s in m.get("sequences", []):
        for c in s.get("canvases", []):
            r = ((c.get("images") or [{}])[0]).get("resource", {}) or {}
            if r:
                out.append((r, c.get("width") or r.get("width"),
                            c.get("height") or r.get("height")))
    if not out:                                     # v3 fallback
        for c in m.get("items", []):
            try:
                body = c["items"][0]["items"][0]["body"]
                body = body[0] if isinstance(body, list) else body
                out.append((body, c.get("width") or body.get("width"),
                            c.get("height") or body.get("height")))
            except Exception:
                pass
    return out


def image_url(res, size="full"):
    """A IIIF Image-API URL for the resource at the requested `size`. Prefers the
    resource's service @id (the resizable base); else strips any size path off
    resource.@id; Gallica serves native.jpg, everyone else default.jpg."""
    svc = res.get("service")
    svc = svc[0] if isinstance(svc, list) else svc
    base = (svc or {}).get("@id") or (svc or {}).get("id")
    if not base:
        rid = res.get("@id") or res.get("id") or ""
        base = rid.split("/full/")[0] if "/full/" in rid else rid
    base = base.rstrip("/")
    suffix = "native.jpg" if "gallica" in base else "default.jpg"
    return "%s/full/%s/0/%s" % (base, size, suffix)


def mirror_witness(wkey, src_url, workers=4, limit=None, delay=0.0):
    print("[%s] fetching source manifest ..." % wkey, flush=True)
    m = json.loads(fetch(src_url))
    pages = canvases(m)
    if limit:
        pages = pages[:limit]
    n = len(pages)
    print("[%s] %d pages -> R2 (workers=%d)" % (wkey, n, workers), flush=True)

    def one(i_res):
        i, (res, w, h) = i_res
        key = "fuero_juzgo/iiif/%s/%04d.jpg" % (wkey, i + 1)
        if key in _done:
            return ("skip", i)
        data = fetch(image_url(res, "full"))
        s3.put_object(Bucket=BUCKET, Key=key, Body=data, ContentType="image/jpeg")
        mark(key)
        if delay:
            time.sleep(delay)                       # rate-limit courtesy (Gallica)
        return ("ok", i)

    ok = skip = 0
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futs = [ex.submit(one, iu) for iu in enumerate(pages)]
        for k, f in enumerate(as_completed(futs)):
            st, _ = f.result()
            ok += (st == "ok"); skip += (st == "skip")
            if (k + 1) % 20 == 0:
                print("[%s]   %d/%d (%d new, %d cached)" % (wkey, k + 1, n, ok, skip), flush=True)

    # ---- self-hosted manifest: swap image resource @id -> R2, drop service ----
    base = "%s/fuero_juzgo/iiif/%s" % (PUB, wkey)
    out = copy.deepcopy(m)
    i = 0
    for s in out.get("sequences", []):
        for c in s.get("canvases", []):
            i += 1
            r2 = "%s/%04d.jpg" % (base, i)
            for im in c.get("images", []):
                res = im.setdefault("resource", {})
                res.pop("service", None)
                res["@id"] = r2
                res["format"] = "image/jpeg"
            c["@id"] = "%s/canvas/%04d" % (base, i)
    if not out.get("sequences") and out.get("items"):   # v3 -> rewrite bodies
        i = 0
        for c in out.get("items", []):
            i += 1
            r2 = "%s/%04d.jpg" % (base, i)
            try:
                body = c["items"][0]["items"][0]["body"]
                body = body[0] if isinstance(body, list) else body
                body.pop("service", None); body["id"] = r2; body["format"] = "image/jpeg"
            except Exception:
                pass
    out["@id"] = base + "/manifest.json"
    out.setdefault("attribution",
                   "Facsimile mirrored to R2 for the rete Fuero Juzgo dataset; "
                   "source: %s" % src_url)
    s3.put_object(Bucket=BUCKET, Key="fuero_juzgo/iiif/%s/manifest.json" % wkey,
                  Body=json.dumps(out, ensure_ascii=False).encode("utf-8"),
                  ContentType="application/json")
    print("[%s] DONE: %d new, %d cached; manifest -> %s/manifest.json"
          % (wkey, ok, skip, base), flush=True)
    return n


def make_thumb(wkey, src_url=None, page="0030"):
    """Upload fuero_juzgo/iiif/<wkey>/thumb.jpg: a width-900 JPEG resized (Pillow)
    from OUR already-mirrored R2 full-res page -- no source server involved, so
    it dodges every hotlink/rate-limit quirk. Page 30 is a text folio present in
    all witnesses (all have >=126 pages)."""
    import io
    from PIL import Image
    key = "fuero_juzgo/iiif/%s/%s.jpg" % (wkey, page)
    try:
        raw = s3.get_object(Bucket=BUCKET, Key=key)["Body"].read()
    except Exception as e:
        print("[%s] thumb: source page %s not on R2 yet (%s)" % (wkey, key, e)); return
    im = Image.open(io.BytesIO(raw)).convert("RGB")
    w, h = im.size
    if w > 900:
        im = im.resize((900, round(h * 900 / w)), Image.LANCZOS)
    buf = io.BytesIO(); im.save(buf, "JPEG", quality=82)
    s3.put_object(Bucket=BUCKET, Key="fuero_juzgo/iiif/%s/thumb.jpg" % wkey,
                  Body=buf.getvalue(), ContentType="image/jpeg")
    print("[%s] thumb <- R2 %s.jpg -> %d b" % (wkey, page, buf.tell()), flush=True)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if args and args[0] == "thumbs":
        for wkey, src in WITNESSES:
            try:
                make_thumb(wkey, src)
            except Exception as e:
                print("[%s] thumb FAILED: %s" % (wkey, e), flush=True)
        print("THUMBS DONE"); return
    limit = None
    for a in sys.argv[1:]:
        if a.startswith("--limit"):
            limit = int(a.split("=")[1]) if "=" in a else int(sys.argv[sys.argv.index(a) + 1])
    target = args[0] if args else "all"
    sel = WITNESSES if target == "all" else [(k, u) for k, u in WITNESSES if k == target]
    if not sel:
        print("unknown witness key:", target); sys.exit(2)
    global _led
    os.makedirs(os.path.dirname(LEDGER), exist_ok=True)
    _led = open(LEDGER, "a", encoding="utf-8")
    total = 0
    for wkey, src in sel:
        # MDZ tolerates more; Gallica rate-limits hardest -> 1 worker + a delay.
        if wkey == "bnf-esp-256":
            workers, delay = 2, 0.2
        elif wkey == "bodleian-holkham-46":
            workers, delay = 2, 0.0
        elif wkey.startswith("bsb"):
            workers, delay = 6, 0.0
        else:
            workers, delay = 4, 0.0
        total += mirror_witness(wkey, src, workers=workers, limit=limit, delay=delay)
    _led.close()
    print("ALL DONE: %d pages across %d witness(es)" % (total, len(sel)), flush=True)


if __name__ == "__main__":
    main()
