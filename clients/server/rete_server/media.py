"""``media_preview``: look at one picture a graph points to.

The URL is an IIIF Presentation manifest (v2 or v3), an IIIF Image API
info.json or service base, or a plain image. The answer is one small JPEG plus
a few facts about it. Nothing else is fetched or kept: no PDFs, no video, no
HTML pages.

The public server is open, so this fetches URLs for anonymous callers. The
rules, all in this file:

* http and https only, no credentials in the URL.
* The host is resolved once and every address must be public: loopback,
  private, link-local, multicast, reserved, unspecified, CGNAT, IPv6
  unique-local / site-local and the IPv4-in-IPv6 forms are refused. The
  connection then goes to that vetted address (the Host header and the TLS
  server name stay the original host), so a second DNS answer cannot move it.
* Redirects are followed by hand, at most 3, and each one is vetted the same way.
* At most 4 HTTP requests and about 20 s per call. A manifest or info.json is
  read up to 2 MB, an image up to 15 MB, counted as it streams.

Residual risk: the vetted address is one the resolver gave us at that moment,
so a name that points at a public host stays trusted for the length of the
call; and ``RETE_MEDIA_ALLOW_PRIVATE=1`` switches every address check off.
Proxies from the environment are ignored on purpose (they would see, and
could redirect, the pinned connection).
"""
from __future__ import annotations

import base64
import io
import ipaddress
import json
import socket
import time
import warnings
from typing import Any, Callable, Dict, List, Optional, Tuple
from urllib.parse import urljoin, urlsplit

import httpx
from PIL import Image

MAX_JSON = 2 * 1024 * 1024
MAX_IMAGE = 15 * 1024 * 1024
MAX_REQUESTS = 4
MAX_REDIRECTS = 3
TOTAL_TIMEOUT_S = 20.0
MAX_PIXELS = 60_000_000
B64_CAP = 120_000
MIN_PX, MAX_PX = 128, 1024
USER_AGENT = "rete-server-media-preview/1.0"

Image.MAX_IMAGE_PIXELS = MAX_PIXELS

_BLOCKED_NETS = [ipaddress.ip_network(n) for n in (
    "0.0.0.0/8", "100.64.0.0/10", "192.0.0.0/24", "198.18.0.0/15",
    "64:ff9b::/96", "2002::/16", "2001::/32", "fec0::/10")]


class MediaError(Exception):
    """A problem to tell the caller about, in plain words."""


# --------------------------------------------------------------------------- #
# Which addresses may be fetched
# --------------------------------------------------------------------------- #

def _address_ok(text: str) -> bool:
    try:
        ip = ipaddress.ip_address(text.split("%", 1)[0])
    except ValueError:
        return False
    if isinstance(ip, ipaddress.IPv6Address) and ip.ipv4_mapped is not None:
        ip = ip.ipv4_mapped
    if (ip.is_private or ip.is_loopback or ip.is_link_local or ip.is_multicast
            or ip.is_reserved or ip.is_unspecified or not ip.is_global):
        return False
    if getattr(ip, "is_site_local", False):
        return False
    return not any(ip in net for net in _BLOCKED_NETS if net.version == ip.version)


def _resolve(host: str, port: int) -> List[str]:
    """Every address the name resolves to (tests replace this)."""
    try:
        infos = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    except socket.gaierror:
        raise MediaError(f"could not find the host {host!r}") from None
    seen: List[str] = []
    for info in infos:
        addr = info[4][0]
        if addr not in seen:
            seen.append(addr)
    return seen


class _Fetcher:
    """HTTP GET with the rules above, one per call (it owns the request budget)."""

    def __init__(self, allow_private: bool):
        self.allow_private = allow_private
        self.requests = 0
        self.deadline = time.monotonic() + TOTAL_TIMEOUT_S
        self.client = httpx.Client(follow_redirects=False, trust_env=False,
                                   headers={"User-Agent": USER_AGENT, "Accept": "*/*"})

    def close(self) -> None:
        self.client.close()

    def _vet(self, url: str) -> Tuple[httpx.URL, str, Optional[str]]:
        """(URL pointing at the vetted address, Host header, TLS server name)."""
        parts = urlsplit(url)
        if parts.scheme not in ("http", "https"):
            raise MediaError("only http and https URLs can be previewed")
        if parts.username or parts.password or "@" in parts.netloc:
            raise MediaError("a URL with a user name or password is not allowed")
        host = parts.hostname
        if not host:
            raise MediaError("the URL has no host")
        try:
            port = parts.port or (443 if parts.scheme == "https" else 80)
        except ValueError:
            raise MediaError("the URL has a bad port") from None
        addrs = _resolve(host, port)
        if not addrs:
            raise MediaError(f"could not find the host {host!r}")
        if not self.allow_private and not all(_address_ok(a) for a in addrs):
            raise MediaError("host not allowed: it points at a private or internal address")
        pinned = httpx.URL(url).copy_with(host=addrs[0])
        netloc_host = f"[{host}]" if ":" in host else host
        default = 443 if parts.scheme == "https" else 80
        hostheader = netloc_host if (parts.port in (None, default)) else f"{netloc_host}:{parts.port}"
        try:
            ipaddress.ip_address(host)
            sni = None
        except ValueError:
            sni = host
        return pinned, hostheader, sni

    def get(self, url: str, cap_for: Callable[[bytes], int]) -> Tuple[bytes, str, str]:
        """(body, content type, final URL). ``cap_for`` picks the size cap from
        the first bytes, so a manifest and an image get different limits."""
        for _hop in range(MAX_REDIRECTS + 1):
            if self.requests >= MAX_REQUESTS:
                raise MediaError("too many requests for one preview")
            left = self.deadline - time.monotonic()
            if left <= 0:
                raise MediaError("timed out")
            pinned, hostheader, sni = self._vet(url)
            self.requests += 1
            ext = {"sni_hostname": sni} if sni and pinned.scheme == "https" else {}
            req = self.client.build_request(
                "GET", pinned, headers={"Host": hostheader}, extensions=ext,
                timeout=httpx.Timeout(min(left, 10.0)))
            try:
                resp = self.client.send(req, stream=True)
            except httpx.TimeoutException:
                raise MediaError("timed out") from None
            except httpx.HTTPError as e:
                raise MediaError(f"could not fetch it ({type(e).__name__})") from None
            try:
                if resp.status_code in (301, 302, 303, 307, 308):
                    loc = resp.headers.get("location")
                    if not loc:
                        raise MediaError("the server redirected without saying where")
                    url = urljoin(url, loc)
                    continue
                if resp.status_code != 200:
                    raise MediaError(f"the server answered {resp.status_code}")
                return self._read(resp, cap_for) + (url,)
            except httpx.TimeoutException:
                raise MediaError("timed out") from None
            except httpx.HTTPError as e:
                raise MediaError(f"could not fetch it ({type(e).__name__})") from None
            finally:
                resp.close()
        raise MediaError("too many redirects")

    def _read(self, resp: httpx.Response, cap_for: Callable[[bytes], int]) -> Tuple[bytes, str]:
        ctype = (resp.headers.get("content-type") or "").split(";")[0].strip().lower()
        declared = resp.headers.get("content-length")
        chunks: List[bytes] = []
        total = 0
        cap: Optional[int] = None
        for chunk in resp.iter_bytes(1 << 16):
            if cap is None:
                cap = cap_for(chunk)
                if declared and declared.isdigit() and int(declared) > cap:
                    raise MediaError(f"too big: the file is {int(declared) // 1024} KB and the limit is {cap // 1024} KB")
            total += len(chunk)
            if total > cap:
                raise MediaError(f"too big: the file is over the {cap // 1024} KB limit")
            if time.monotonic() > self.deadline:
                raise MediaError("timed out")
            chunks.append(chunk)
        return b"".join(chunks), ctype


# --------------------------------------------------------------------------- #
# What the bytes are
# --------------------------------------------------------------------------- #

def _is_image(head: bytes) -> bool:
    return (head.startswith((b"\xff\xd8\xff", b"\x89PNG\r\n\x1a\n", b"GIF87a", b"GIF89a", b"II*\x00", b"MM\x00*"))
            or (head[:4] == b"RIFF" and head[8:12] == b"WEBP"))


def _looks_json(head: bytes) -> bool:
    return head.lstrip(b"\xef\xbb\xbf \t\r\n")[:1] == b"{"


def _cap_for(head: bytes) -> int:
    return MAX_JSON if _looks_json(head) else MAX_IMAGE


# --------------------------------------------------------------------------- #
# IIIF
# --------------------------------------------------------------------------- #

def _first(x: Any) -> Any:
    return x[0] if isinstance(x, list) and x else x


def _label(x: Any) -> str:
    """A IIIF label: a string, {"@value"}, a list, or a v3 language map."""
    if x is None:
        return ""
    if isinstance(x, str):
        return x
    if isinstance(x, list):
        return next((s for s in (_label(i) for i in x) if s), "")
    if isinstance(x, dict):
        if "@value" in x:
            return _label(x["@value"])
        for v in x.values():
            s = _label(v)
            if s:
                return s
    return ""


def _id(x: Any) -> str:
    return (x.get("@id") or x.get("id") or "") if isinstance(x, dict) else (x if isinstance(x, str) else "")


def _types(x: Any) -> List[str]:
    t = x.get("@type") or x.get("type") if isinstance(x, dict) else None
    return [str(i) for i in (t if isinstance(t, list) else [t])] if t else []


def _service_base(node: Any) -> Optional[str]:
    """The IIIF Image service base on a resource, if it names one."""
    svc = node.get("service") if isinstance(node, dict) else None
    for s in (svc if isinstance(svc, list) else [svc]):
        if not isinstance(s, dict):
            continue
        profile = json.dumps(s.get("profile") or "")
        if any("ImageService" in t for t in _types(s)) or "iiif.io/api/image" in profile \
                or "iiif.io/api/image" in str(s.get("@context") or ""):
            sid = _id(s)
            if sid:
                sid = sid.rstrip("/")
                return sid[:-len("/info.json")] if sid.endswith("/info.json") else sid
    return None


def _sized(base: str, px: int) -> str:
    return f"{base}/full/!{px},{px}/0/default.jpg"


def _bodies(canvas: Dict[str, Any]) -> List[Dict[str, Any]]:
    """The image resources painted on a canvas (v2 images, v3 painting annotations)."""
    out: List[Dict[str, Any]] = []
    for img in canvas.get("images") or []:           # v2
        res = img.get("resource") if isinstance(img, dict) else None
        if isinstance(res, dict):
            out.append(res)
    for page in canvas.get("items") or []:           # v3
        for anno in (page.get("items") or []) if isinstance(page, dict) else []:
            body = anno.get("body") if isinstance(anno, dict) else None
            for b in (body if isinstance(body, list) else [body]):
                if isinstance(b, dict) and isinstance(b.get("items"), list):   # a Choice
                    out.extend(i for i in b["items"] if isinstance(i, dict))
                elif isinstance(b, dict):
                    out.append(b)
    return out


def _thumbs(node: Dict[str, Any]) -> List[Dict[str, Any]]:
    t = node.get("thumbnail")
    items = t if isinstance(t, list) else [t]
    return [{"@id": i} if isinstance(i, str) else i for i in items if i]


def _canvas_candidates(canvas: Dict[str, Any], manifest: Dict[str, Any], px: int) -> List[str]:
    """Where to get this canvas's picture, cheapest first: the image service at
    the wanted size, the image resource itself, then a thumbnail."""
    urls: List[str] = []
    bodies = _bodies(canvas)
    for b in bodies:
        base = _service_base(b)
        if base:
            urls.append(_sized(base, px))
    for b in bodies:
        if "Image" in " ".join(_types(b)) or b.get("format", "").startswith("image/") or _id(b):
            urls.append(_id(b))
    for holder in (canvas, manifest):
        for t in _thumbs(holder):
            base = _service_base(t)
            if base:
                urls.append(_sized(base, px))
            urls.append(_id(t))
    seen, out = set(), []
    for u in urls:
        if u and u not in seen:
            seen.add(u)
            out.append(u)
    return out


def _pick_canvas(canvases: List[Dict[str, Any]], which: Any) -> Tuple[int, Dict[str, Any]]:
    if not canvases:
        raise MediaError("the manifest has no canvases")
    text = str(which).strip()
    if text.lstrip("-").isdigit():
        i = int(text)
        if not 0 <= i < len(canvases):
            raise MediaError(f"canvas {i} does not exist; the manifest has {len(canvases)} (0 to {len(canvases) - 1})")
        return i, canvases[i]
    for i, c in enumerate(canvases):
        if _id(c) == text or _id(c).split("#")[0] == text.split("#")[0]:
            return i, c
    raise MediaError(f"no canvas with the id {text!r}; the manifest has {len(canvases)} canvases")


# --------------------------------------------------------------------------- #
# The picture
# --------------------------------------------------------------------------- #

def _encode(data: bytes, px: int) -> Tuple[bytes, int, int]:
    """Downscale and re-encode as JPEG; step quality and size down until the
    base64 fits."""
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("error", Image.DecompressionBombWarning)
            img = Image.open(io.BytesIO(data))
            if img.width * img.height > MAX_PIXELS * 2:
                raise Image.DecompressionBombError("too many pixels")
            if img.format == "JPEG":
                img.draft("RGB", (px * 2, px * 2))
            img.load()
    except (Image.DecompressionBombError, Image.DecompressionBombWarning):
        raise MediaError("too big: the picture has too many pixels to open") from None
    except Exception:
        raise MediaError("not an image: the bytes could not be decoded") from None
    if img.mode in ("RGBA", "LA", "P"):
        img = img.convert("RGBA")
        flat = Image.new("RGB", img.size, (255, 255, 255))
        flat.paste(img, mask=img.getchannel("A"))
        img = flat
    elif img.mode != "RGB":
        img = img.convert("RGB")
    size = px
    while True:
        work = img.copy()
        work.thumbnail((size, size), Image.LANCZOS)
        for quality in (80, 70, 60, 50, 40):
            buf = io.BytesIO()
            work.save(buf, "JPEG", quality=quality, optimize=True)
            if len(buf.getvalue()) * 4 // 3 + 4 <= B64_CAP:
                return buf.getvalue(), work.width, work.height
        if size <= 64:
            return buf.getvalue(), work.width, work.height
        size = int(size * 0.8)


def _json_doc(body: bytes) -> Dict[str, Any]:
    try:
        doc = json.loads(body.decode("utf-8-sig"))
    except Exception:
        raise MediaError("not a IIIF manifest: the answer is not valid JSON") from None
    if not isinstance(doc, dict):
        raise MediaError("not a IIIF manifest: expected a JSON object")
    return doc


def preview(url: str, canvas: Any = 0, max_px: int = 512, allow_private: bool = False) -> Tuple[bytes, Dict[str, Any]]:
    """(JPEG bytes, facts). Blocking; raises :class:`MediaError`."""
    px = min(max(int(max_px), MIN_PX), MAX_PX)
    if not isinstance(url, str) or not url.strip():
        raise MediaError("give a URL")
    url = url.strip()
    f = _Fetcher(allow_private)
    try:
        body, ctype, final = f.get(url, _cap_for)
        facts: Dict[str, Any] = {"source": url, "kind": "image", "label": "", "canvas": None,
                                 "canvases": 0, "canvas_labels": []}
        candidates: List[str] = []
        if _is_image(body[:16]):
            image_url, data = final, body
        elif _looks_json(body[:16]):
            doc = _json_doc(body)
            types = " ".join(_types(doc))
            ctx = json.dumps(doc.get("@context") or "")
            if "Manifest" in types:
                facts["kind"] = "iiif-manifest"
                facts["label"] = _label(doc.get("label"))
                seqs = doc.get("sequences")
                raw = (seqs[0].get("canvases") if seqs and isinstance(seqs[0], dict) else doc.get("items")) or []
                canvases = [c for c in raw if isinstance(c, dict)]
                i, cv = _pick_canvas(canvases, canvas)
                facts["canvases"] = len(canvases)
                facts["canvas_labels"] = [_label(c.get("label")) for c in canvases[:20]]
                facts["canvas"] = {"index": i, "id": _id(cv), "label": _label(cv.get("label"))}
                candidates = _canvas_candidates(cv, doc, px)
                if not candidates:
                    raise MediaError("that canvas carries no image")
            elif "Collection" in types:
                raise MediaError("not a IIIF manifest: this is a collection; give one of its manifests")
            elif "iiif.io/api/image" in ctx or ("protocol" in doc and "width" in doc):
                facts["kind"] = "iiif-image"
                base = final[:-len("/info.json")] if final.endswith("/info.json") else (_id(doc) or final)
                base = base.rstrip("/")
                candidates = [_sized(base, px), f"{base}/full/max/0/default.jpg"]
            else:
                raise MediaError("not a IIIF manifest or image service")
            image_url, data = "", b""
            last: Optional[MediaError] = None
            for cand in candidates:
                if f.requests >= MAX_REQUESTS:
                    break
                try:
                    got, _ct, got_final = f.get(urljoin(final, cand), lambda _h: MAX_IMAGE)
                except MediaError as e:
                    last = e
                    continue
                if _is_image(got[:16]):
                    image_url, data = got_final, got
                    break
                last = MediaError("not an image: the address did not return a picture")
            if not data:
                raise last or MediaError("could not get a picture for it")
        else:
            raise MediaError("not an image or a IIIF manifest")
    finally:
        f.close()
    jpeg, w, h = _encode(data, px)
    facts.update({"width": w, "height": h, "image_url": image_url, "bytes": len(jpeg)})
    return jpeg, facts


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")
