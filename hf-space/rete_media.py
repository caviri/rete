"""Media utilities for the MCP/REST planes.

Two capabilities, both returning base64 ``data:`` URIs ready to inline into
generated HTML:

* :func:`embed_urls` — fetch a list of URLs and return each as base64.
  Images are recompressed to WebP with a size cap (``max_dimension``), which
  typically shrinks them 3-10x — the difference between an HTML report that
  embeds twenty photos and one that cannot.
* :func:`preview` — one *representative image* for a piece of media: first
  page of a PDF, a frame of a video (read lazily over HTTP Range — the whole
  file is never downloaded), a IIIF resource (info.json or Presentation
  manifest v2/v3), an HTML page (og:image / twitter:image), or the image
  itself.

Detection is content-first (magic bytes), falling back to Content-Type —
object stores routinely serve everything as octet-stream.
"""
from __future__ import annotations

import base64
import io
import json
import os
import re
from typing import Any, Dict, List, Optional
from urllib.parse import urljoin

import httpx

MAX_FETCH = int(os.environ.get("MEDIA_MAX_MB") or 32) * (1 << 20)
MAX_URLS = int(os.environ.get("MEDIA_MAX_URLS") or 20)

_http = httpx.Client(follow_redirects=True, timeout=60.0,
                     headers={"User-Agent": "rete-space-media/1.0"})


def _fetch(url: str, cap: int = MAX_FETCH) -> tuple:
    """GET with a hard size cap. Returns (bytes, content_type)."""
    with _http.stream("GET", url) as r:
        r.raise_for_status()
        declared = int(r.headers.get("content-length") or 0)
        if declared > cap:
            raise ValueError(f"file is {declared} bytes; cap is {cap}")
        chunks, total = [], 0
        for chunk in r.iter_bytes(1 << 18):
            total += len(chunk)
            if total > cap:
                raise ValueError(f"file exceeds the {cap}-byte cap")
            chunks.append(chunk)
        return b"".join(chunks), (r.headers.get("content-type") or "").split(";")[0].strip()


def _sniff(body: bytes, ctype: str) -> str:
    """Media kind from magic bytes first, Content-Type second."""
    head = body[:512]
    if head.startswith(b"%PDF"):
        return "pdf"
    if head[4:12] in (b"ftypisom", b"ftypmp42", b"ftypMSNV", b"ftypM4V ", b"ftypmp41") or head[4:8] == b"ftyp":
        return "video"
    if head.startswith(b"\x1a\x45\xdf\xa3"):  # Matroska/WebM
        return "video"
    if head.startswith((b"\x89PNG", b"\xff\xd8\xff", b"GIF8", b"RIFF", b"II*\x00", b"MM\x00*", b"BM")):
        return "image"
    stripped = head.lstrip()
    if stripped[:1] in (b"{", b"["):
        return "json"
    if stripped[:1] == b"<" and (b"<svg" in head or ctype == "image/svg+xml"):
        return "svg"
    if stripped[:1] == b"<":
        return "html"
    if ctype.startswith("image/"):
        return "image"
    if ctype.startswith("video/"):
        return "video"
    if ctype in ("text/html", "application/xhtml+xml"):
        return "html"
    if ctype in ("application/json", "application/ld+json"):
        return "json"
    if ctype == "application/pdf":
        return "pdf"
    return "binary"


def _image_to_webp(body: bytes, max_dimension: int, quality: int) -> Dict[str, Any]:
    from PIL import Image

    img = Image.open(io.BytesIO(body))
    img.load()
    if img.mode not in ("RGB", "RGBA"):
        img = img.convert("RGBA" if "A" in img.mode or img.mode == "P" else "RGB")
    if max(img.size) > max_dimension:
        img.thumbnail((max_dimension, max_dimension))
    out = io.BytesIO()
    img.save(out, "WEBP", quality=quality)
    data = out.getvalue()
    return {"mime": "image/webp", "width": img.size[0], "height": img.size[1],
            "bytes": len(data), "data_uri": "data:image/webp;base64," + base64.b64encode(data).decode()}


def _pil_frame_to_webp(img, max_dimension: int, quality: int) -> Dict[str, Any]:
    out = io.BytesIO()
    img.save(out, "WEBP", quality=quality)
    return _image_to_webp(out.getvalue(), max_dimension, quality)


# --------------------------------------------------------------------------- #
# embed
# --------------------------------------------------------------------------- #

def embed_urls(urls: List[str], max_dimension: int = 1024, webp_quality: int = 80) -> List[Dict[str, Any]]:
    """Fetch each URL and return it as a base64 ``data:`` URI.

    Images are recompressed to WebP (downscaled to ``max_dimension`` on the
    long side); everything else is passed through verbatim with its MIME.
    """
    if len(urls) > MAX_URLS:
        raise ValueError(f"at most {MAX_URLS} urls per call (got {len(urls)})")
    results = []
    for url in urls:
        try:
            body, ctype = _fetch(url)
            kind = _sniff(body, ctype)
            if kind == "image":
                doc = _image_to_webp(body, max_dimension, webp_quality)
                doc["original_bytes"] = len(body)
            else:
                # Trust the sniffed kind over a generic header — object
                # stores routinely label everything octet-stream.
                sniffed = {"pdf": "application/pdf", "svg": "image/svg+xml",
                           "video": "video/mp4", "html": "text/html",
                           "json": "application/json"}.get(kind)
                mime = sniffed or ctype or "application/octet-stream"
                doc = {"mime": mime, "bytes": len(body),
                       "data_uri": f"data:{mime};base64," + base64.b64encode(body).decode()}
            doc.update({"url": url, "ok": True, "kind": kind})
            results.append(doc)
        except Exception as e:
            results.append({"url": url, "ok": False, "error": f"{type(e).__name__}: {e}"})
    return results


# --------------------------------------------------------------------------- #
# preview
# --------------------------------------------------------------------------- #

class _HttpRangeFile(io.RawIOBase):
    """A seekable read-only file over HTTP Range — lets PyAV pull just the
    boxes/frames it needs from a remote video instead of the whole file."""

    def __init__(self, url: str, chunk: int = 1 << 18):
        r = _http.head(url)
        length = int(r.headers.get("content-length") or 0)
        if not length:
            g = _http.get(url, headers={"Range": "bytes=0-0"})
            length = int(g.headers["content-range"].rsplit("/", 1)[1]) if g.status_code == 206 else 0
        if not length:
            raise ValueError("host reports no length; cannot range-read")
        self.url, self.length, self.chunk = url, length, chunk
        self.pos = 0
        self.fetched = 0

    def readable(self):  # pragma: no cover - io protocol
        return True

    def seekable(self):  # pragma: no cover - io protocol
        return True

    def seek(self, offset, whence=io.SEEK_SET):
        self.pos = {io.SEEK_SET: offset, io.SEEK_CUR: self.pos + offset,
                    io.SEEK_END: self.length + offset}[whence]
        return self.pos

    def tell(self):
        return self.pos

    def read(self, n=-1):
        if n is None or n < 0:
            n = self.length - self.pos
        n = min(n, self.length - self.pos)
        if n <= 0:
            return b""
        r = _http.get(self.url, headers={"Range": f"bytes={self.pos}-{self.pos + n - 1}"})
        if r.status_code != 206:
            raise IOError(f"range read failed ({r.status_code})")
        self.pos += len(r.content)
        self.fetched += len(r.content)
        return r.content


def _preview_pdf(body: bytes, max_dimension: int, quality: int) -> Dict[str, Any]:
    import pypdfium2 as pdfium

    doc = pdfium.PdfDocument(body)
    try:
        page = doc[0]
        scale = max_dimension / max(page.get_size())
        bitmap = page.render(scale=min(scale, 2.0))
        img = bitmap.to_pil()
    finally:
        doc.close()
    out = _pil_frame_to_webp(img, max_dimension, quality)
    out["pages"] = None  # cheap render only touches page 1
    return out


def _preview_video(url: str, body: Optional[bytes], max_dimension: int, quality: int) -> Dict[str, Any]:
    import av

    if body is not None:
        container = av.open(io.BytesIO(body))
        fetched = len(body)
    else:
        rf = _HttpRangeFile(url)
        container = av.open(rf)
        fetched = None
    try:
        stream = container.streams.video[0]
        # A frame ~10% in beats the black first frame of most encodes.
        if container.duration:
            container.seek(int(container.duration * 0.1))
        frame = next(container.decode(stream))
        img = frame.to_image()
    finally:
        if fetched is None:
            fetched = rf.fetched
        container.close()
    out = _pil_frame_to_webp(img, max_dimension, quality)
    out["video_bytes_fetched"] = fetched
    return out


def _iiif_image_url(doc: Any, base_url: str, dim: int) -> Optional[str]:
    """A concrete image URL out of a IIIF info.json or manifest (v2/v3)."""
    if isinstance(doc, dict):
        ctx = str(doc.get("@context") or "")
        ident = doc.get("@id") or doc.get("id") or ""
        if "image/2" in ctx or "image/3" in ctx or doc.get("protocol") == "http://iiif.io/api/image":
            return f"{ident.rstrip('/')}/full/!{dim},{dim}/0/default.jpg"
        # Manifest: prefer an explicit thumbnail, else the first image service.
        thumb = doc.get("thumbnail")
        if isinstance(thumb, list) and thumb:
            thumb = thumb[0]
        if isinstance(thumb, dict):
            t = thumb.get("@id") or thumb.get("id")
            if t:
                return t
        if isinstance(thumb, str):
            return thumb
        service = doc.get("service")
        if isinstance(service, list) and service:
            service = service[0]
        if isinstance(service, dict):
            sid = service.get("@id") or service.get("id")
            if sid:
                return f"{sid.rstrip('/')}/full/!{dim},{dim}/0/default.jpg"
        for key in ("sequences", "canvases", "images", "items", "body", "resource"):
            if key in doc:
                found = _iiif_image_url(doc[key], base_url, dim)
                if found:
                    return found
        ident = doc.get("@id") or doc.get("id")
        if isinstance(ident, str) and re.search(r"\.(jpe?g|png|webp|tif+)$", ident, re.I):
            return ident
    if isinstance(doc, list):
        for item in doc:
            found = _iiif_image_url(item, base_url, dim)
            if found:
                return found
    return None


_META_IMG = [
    re.compile(r'<meta[^>]+property=["\']og:image(?::secure_url)?["\'][^>]+content=["\']([^"\']+)', re.I),
    re.compile(r'<meta[^>]+content=["\']([^"\']+)["\'][^>]+property=["\']og:image(?::secure_url)?["\']', re.I),
    re.compile(r'<meta[^>]+name=["\']twitter:image(?::src)?["\'][^>]+content=["\']([^"\']+)', re.I),
    re.compile(r'<meta[^>]+content=["\']([^"\']+)["\'][^>]+name=["\']twitter:image(?::src)?["\']', re.I),
    re.compile(r'<link[^>]+rel=["\']image_src["\'][^>]+href=["\']([^"\']+)', re.I),
]


def _preview_html(body: bytes, base_url: str, max_dimension: int, quality: int) -> Dict[str, Any]:
    text = body.decode("utf-8", "replace")
    for pattern in _META_IMG:
        m = pattern.search(text)
        if m:
            img_url = urljoin(base_url, m.group(1))
            img_body, _ = _fetch(img_url)
            out = _image_to_webp(img_body, max_dimension, quality)
            out["preview_source"] = img_url
            return out
    raise ValueError("page declares no og:image / twitter:image / image_src")


def preview(url: str, max_dimension: int = 512, webp_quality: int = 80) -> Dict[str, Any]:
    """One representative WebP image (as a data URI) for a media URL.

    Handles: images (recompressed), PDFs (first page), videos (a frame,
    fetched lazily over HTTP Range), IIIF info.json / Presentation manifests
    (v2/v3), and HTML pages (og:image / twitter:image). Raises ValueError
    when nothing representable is found.
    """
    head = _http.head(url)
    ctype = (head.headers.get("content-type") or "").split(";")[0].strip()
    length = int(head.headers.get("content-length") or 0)

    # Videos go straight to the lazy range path when the host supports it.
    if ctype.startswith("video/") and length > MAX_FETCH:
        doc = _preview_video(url, None, max_dimension, webp_quality)
        doc.update({"url": url, "kind": "video"})
        return doc

    body, ctype = _fetch(url)
    kind = _sniff(body, ctype)

    if kind == "image":
        doc = _image_to_webp(body, max_dimension, webp_quality)
    elif kind == "svg":
        mime = "image/svg+xml"
        doc = {"mime": mime, "bytes": len(body),
               "data_uri": f"data:{mime};base64," + base64.b64encode(body).decode()}
    elif kind == "pdf":
        doc = _preview_pdf(body, max_dimension, webp_quality)
    elif kind == "video":
        doc = _preview_video(url, body, max_dimension, webp_quality)
    elif kind == "json":
        parsed = json.loads(body)
        img_url = _iiif_image_url(parsed, url, max_dimension)
        if not img_url:
            raise ValueError("JSON is not a recognizable IIIF info.json/manifest")
        img_body, ictype = _fetch(urljoin(url, img_url))
        doc = _image_to_webp(img_body, max_dimension, webp_quality)
        doc["preview_source"] = urljoin(url, img_url)
    elif kind == "html":
        doc = _preview_html(body, url, max_dimension, webp_quality)
    else:
        raise ValueError(f"no preview strategy for {kind!r} ({ctype or 'unknown type'})")

    doc.update({"url": url, "kind": kind})
    return doc
