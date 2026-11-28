"""media_preview: IIIF manifests, image services and plain images, and the rules
that keep an open server from being used to reach internal hosts.

Fixtures come from a small HTTP server on 127.0.0.1 in a thread. Tests that
need to fetch from it switch the private-address check off (the setting meant
for tests); the safety tests leave it on.
"""
from __future__ import annotations

import asyncio
import base64
import io
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from fastmcp import Client
from PIL import Image

from conftest import settings
from rete_server import media
from rete_server.mcp import build_mcp

BIG = (1600, 1200)


def _jpeg(size=BIG, fmt="JPEG", color=(200, 30, 30)) -> bytes:
    img = Image.effect_noise(size, 60).convert("RGB")
    img.paste(color, (0, 0, size[0] // 2, size[1]))
    buf = io.BytesIO()
    img.save(buf, fmt)
    return buf.getvalue()


JPEG = _jpeg()
PNG = _jpeg((900, 600), "PNG")
GREY = _jpeg((500, 500), "JPEG", (10, 10, 10))


class Handler(BaseHTTPRequestHandler):
    routes: dict = {}
    hits: list = []

    def log_message(self, *a):
        pass

    def do_GET(self):
        Handler.hits.append((self.path, self.headers.get("Host")))
        path = self.path
        if path.startswith("/svc/"):                    # /svc/<name>/full/!N,N/0/default.jpg
            parts = path.split("/")
            if len(parts) >= 7 and parts[3] == "full":
                size = parts[4].lstrip("!").split(",")[0]
                img = Image.open(io.BytesIO(JPEG))
                if size.isdigit():
                    img.thumbnail((int(size), int(size)))
                buf = io.BytesIO()
                img.save(buf, "JPEG")
                return self._send(200, buf.getvalue(), "image/jpeg")
        if path in self.routes:
            body, ctype, *rest = self.routes[path]
            if ctype == "redirect":
                self.send_response(302)
                self.send_header("Location", body)
                self.end_headers()
                return
            return self._send(200, body, ctype)
        self._send(404, b"no", "text/plain")

    def _send(self, code, body, ctype):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


@pytest.fixture(scope="module")
def origin():
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    base = f"http://127.0.0.1:{srv.server_port}"
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    def j(doc):
        return json.dumps(doc).encode(), "application/json"

    v2 = {"@context": "http://iiif.io/api/presentation/2/context.json", "@id": f"{base}/m/v2.json",
          "@type": "sc:Manifest", "label": "A folio book",
          "sequences": [{"canvases": [
              {"@id": f"{base}/c/{i}", "@type": "sc:Canvas", "label": f"f. {i}",
               "images": [{"resource": {"@id": f"{base}/svc/p{i}/full/full/0/default.jpg", "@type": "dctypes:Image",
                                        "service": {"@context": "http://iiif.io/api/image/2/context.json",
                                                    "@id": f"{base}/svc/p{i}", "profile": "http://iiif.io/api/image/2/level1.json"}}}]}
              for i in range(3)]}]}
    v3 = {"@context": "http://iiif.io/api/presentation/3/context.json", "id": f"{base}/m/v3.json",
          "type": "Manifest", "label": {"en": ["Picture set"]},
          "items": [{"id": f"{base}/c3/{i}", "type": "Canvas", "label": {"none": [f"pic {i}"]},
                     "items": [{"type": "AnnotationPage", "items": [{
                         "type": "Annotation", "motivation": "painting",
                         "body": {"id": f"{base}/svc/q{i}/full/max/0/default.jpg", "type": "Image",
                                  "service": [{"id": f"{base}/svc/q{i}", "type": "ImageService3", "profile": "level1"}]}}]}]}
                    for i in range(2)]}
    thumb_only = {"@context": "http://iiif.io/api/presentation/3/context.json", "id": f"{base}/m/t.json",
                  "type": "Manifest", "label": {"en": ["Thumb"]},
                  "items": [{"id": f"{base}/ct/0", "type": "Canvas", "label": {"en": ["only"]},
                             "thumbnail": [{"id": f"{base}/img/grey.jpg", "type": "Image"}], "items": []}]}
    info = {"@context": "http://iiif.io/api/image/3/context.json", "id": f"{base}/svc/z1", "type": "ImageService3",
            "protocol": "http://iiif.io/api/image", "width": 1600, "height": 1200, "profile": "level1"}
    Handler.routes = {
        "/m/v2.json": j(v2), "/m/v3.json": j(v3), "/m/t.json": j(thumb_only),
        "/svc/z1/info.json": j(info), "/svc/q0/info.json": j({**info, "id": f"{base}/svc/q0"}),
        "/img/big.jpg": (JPEG, "image/jpeg"), "/img/big.png": (PNG, "application/octet-stream"),
        "/img/grey.jpg": (GREY, "image/jpeg"),
        "/img/fake.jpg": (b"<html>hello</html>", "image/jpeg"),
        "/m/other.json": j({"hello": "world"}),
        "/m/coll.json": j({"@id": "x", "@type": "sc:Collection", "label": "c"}),
        "/img/huge.bin": (b"\xff\xd8\xff" + b"0" * (media.MAX_IMAGE + 10), "image/jpeg"),
        "/m/huge.json": (b'{"a": "' + b"0" * (media.MAX_JSON + 10) + b'"}', "application/json"),
        "/go/image": (f"{base}/img/big.jpg", "redirect"),
        "/loop": (f"{base}/loop", "redirect"),
    }
    yield base
    srv.shutdown()


def call(url, allow_private=True, **args):
    async def go():
        mcp = build_mcp(False, settings(media_allow_private=allow_private))
        async with Client(mcp) as c:
            return await c.call_tool("media_preview", {"url": url, **args})
    return asyncio.run(go())


def look(url, **args):
    r = call(url, **args)
    img = next(c for c in r.content if c.type == "image")
    facts = json.loads(next(c for c in r.content if c.type == "text").text)
    return img, facts


def error_of(url, allow_private=False, **args) -> str:
    from fastmcp.exceptions import ToolError
    with pytest.raises(ToolError) as e:
        call(url, allow_private=allow_private, **args)
    return str(e.value).lower()


def check_image(img, facts, max_px):
    assert img.mime_type == "image/jpeg"
    raw = base64.b64decode(img.data)
    assert raw[:3] == b"\xff\xd8\xff"
    assert len(img.data) <= media.B64_CAP
    pic = Image.open(io.BytesIO(raw))
    assert max(pic.size) <= max_px
    assert (facts["width"], facts["height"]) == pic.size
    assert facts["bytes"] == len(raw)


# --------------------------------------------------------------------------- #
# What it can read
# --------------------------------------------------------------------------- #

def test_v2_manifest(origin):
    img, f = look(f"{origin}/m/v2.json")
    check_image(img, f, 512)
    assert f["kind"] == "iiif-manifest" and f["label"] == "A folio book"
    assert f["canvases"] == 3 and f["canvas_labels"] == ["f. 0", "f. 1", "f. 2"]
    assert f["canvas"] == {"index": 0, "id": f"{origin}/c/0", "label": "f. 0"}
    assert "/full/!512,512/0/default.jpg" in f["image_url"]       # the service, not the master


def test_v2_canvas_by_index_and_id(origin):
    _, f = look(f"{origin}/m/v2.json", canvas=2, max_px=256)
    assert f["canvas"]["index"] == 2 and "p2" in f["image_url"] and "!256,256" in f["image_url"]
    _, f = look(f"{origin}/m/v2.json", canvas=f"{origin}/c/1")
    assert f["canvas"]["index"] == 1
    _, f = look(f"{origin}/m/v2.json", canvas="2")                 # an index sent as text
    assert f["canvas"]["index"] == 2


def test_v3_manifest(origin):
    img, f = look(f"{origin}/m/v3.json", canvas=1, max_px=300)
    check_image(img, f, 300)
    assert f["label"] == "Picture set" and f["canvases"] == 2
    assert f["canvas"]["label"] == "pic 1" and "q1" in f["image_url"]


def test_manifest_thumbnail_fallback(origin):
    img, f = look(f"{origin}/m/t.json")
    check_image(img, f, 512)
    assert f["image_url"].endswith("/img/grey.jpg")


def test_info_json_and_service_base(origin):
    img, f = look(f"{origin}/svc/z1/info.json", max_px=200)
    check_image(img, f, 200)
    assert f["kind"] == "iiif-image" and "!200,200" in f["image_url"]
    _, f = look(f"{origin}/svc/q0/info.json")
    assert "q0" in f["image_url"]


@pytest.mark.parametrize("path,size", [("/img/big.jpg", BIG), ("/img/big.png", (900, 600))])
def test_direct_image(origin, path, size):
    img, f = look(f"{origin}{path}")
    check_image(img, f, 512)
    assert f["kind"] == "image" and f["canvases"] == 0


def test_max_px_is_clamped(origin):
    img, f = look(f"{origin}/img/big.jpg", max_px=5000)
    check_image(img, f, 1024)
    img, f = look(f"{origin}/img/big.jpg", max_px=10)
    check_image(img, f, 128)
    assert max(f["width"], f["height"]) == 128


def test_redirect_to_an_image_is_followed(origin):
    img, f = look(f"{origin}/go/image")
    check_image(img, f, 512)


def test_noisy_picture_fits_the_cap(origin):
    img, f = look(f"{origin}/img/big.jpg", max_px=1024)
    assert len(img.data) <= media.B64_CAP


def test_host_header_is_the_original_host(origin):
    Handler.hits.clear()
    look(f"{origin}/img/big.jpg")
    assert Handler.hits[0][1] == origin.split("//")[1]


# --------------------------------------------------------------------------- #
# What it refuses
# --------------------------------------------------------------------------- #

@pytest.mark.parametrize("url", [
    "http://localhost/x.jpg", "http://127.0.0.1/x.jpg", "http://[::1]/x.jpg",
    "http://10.1.2.3/x.jpg", "http://169.254.169.254/latest/meta-data/",
    "http://192.168.0.1/", "http://172.16.0.9/", "http://0.0.0.0/", "http://[fd00::1]/",
    "http://[::ffff:10.0.0.1]/", "http://100.64.0.1/", "http://224.0.0.1/"])
def test_private_hosts_refused(url):
    assert "host not allowed" in error_of(url)


def test_private_host_refused_even_on_a_real_server(origin):
    assert "host not allowed" in error_of(f"{origin}/img/big.jpg")


@pytest.mark.parametrize("url,words", [
    ("file:///etc/passwd", "http"), ("ftp://example.org/a.jpg", "http"),
    ("gopher://example.org/", "http"), ("http://user:pw@example.org/a.jpg", "user name"),
    ("http:///nohost", "host")])
def test_bad_urls_refused(url, words):
    assert words in error_of(url, allow_private=True)


def test_address_checks():
    for ok in ("8.8.8.8", "93.184.216.34", "2606:4700:4700::1111"):
        assert media._address_ok(ok), ok
    for bad in ("127.0.0.1", "::1", "10.0.0.1", "169.254.169.254", "fe80::1", "fe80::1%eth0", "fc00::1",
                "fec0::1", "0.0.0.0", "::", "240.0.0.1", "224.0.0.1", "ff02::1", "100.64.0.1",
                "::ffff:127.0.0.1", "64:ff9b::7f00:1", "not an ip"):
        assert not media._address_ok(bad), bad


def test_a_name_with_one_private_address_is_refused(monkeypatch):
    monkeypatch.setattr(media, "_resolve", lambda host, port: ["93.184.216.34", "10.0.0.5"])
    assert "host not allowed" in error_of("http://mixed.test/a.jpg")


def test_redirect_to_a_private_host_is_refused(origin, monkeypatch):
    """allowed.test answers from the fixture server; its redirect names a host that resolves to 10.x."""
    port = int(origin.rsplit(":", 1)[1])
    names = {"allowed.test": "127.0.0.1", "internal.test": "10.0.0.5"}
    monkeypatch.setattr(media, "_resolve", lambda host, p: [names[host]])
    monkeypatch.setattr(media, "_address_ok", lambda a: a == "127.0.0.1")
    Handler.routes["/to-private"] = ("http://internal.test/secret.jpg", "redirect")
    Handler.hits.clear()
    assert "host not allowed" in error_of(f"http://allowed.test:{port}/to-private")
    assert [h[0] for h in Handler.hits] == ["/to-private"]         # the private host was never asked
    # and the same name without the redirect works, over the vetted address
    img, f = look(f"http://allowed.test:{port}/img/big.jpg", allow_private=False)
    check_image(img, f, 512)


def test_redirect_loop_stops(origin):
    msg = error_of(f"{origin}/loop", allow_private=True)
    assert "redirect" in msg or "too many" in msg


def test_oversized_image_refused(origin):
    assert "too big" in error_of(f"{origin}/img/huge.bin", allow_private=True)


def test_oversized_manifest_refused(origin):
    assert "too big" in error_of(f"{origin}/m/huge.json", allow_private=True)


def test_not_an_image(origin):
    assert "not an image" in error_of(f"{origin}/img/fake.jpg", allow_private=True)


def test_not_a_manifest(origin):
    assert "not a iiif" in error_of(f"{origin}/m/other.json", allow_private=True)
    assert "collection" in error_of(f"{origin}/m/coll.json", allow_private=True)


def test_missing_canvas_and_404(origin):
    assert "canvas 9 does not exist" in error_of(f"{origin}/m/v2.json", allow_private=True, canvas=9)
    assert "no canvas with the id" in error_of(f"{origin}/m/v2.json", allow_private=True, canvas="nope")
    assert "404" in error_of(f"{origin}/missing.jpg", allow_private=True)


def test_too_many_pixels(monkeypatch):
    monkeypatch.setattr(media, "MAX_PIXELS", 100)
    monkeypatch.setattr(Image, "MAX_IMAGE_PIXELS", 100)
    with pytest.raises(media.MediaError, match="too big"):
        media._encode(JPEG, 512)


def test_request_budget(origin, monkeypatch):
    """A manifest whose pictures all fail stops after four requests."""
    Handler.hits.clear()
    base = origin
    doc = {"@type": "sc:Manifest", "label": "x", "sequences": [{"canvases": [
        {"@id": "c", "label": "c", "images": [{"resource": {"@id": f"{base}/nothing/{i}.jpg"}}
                                              for i in range(1)],
         "thumbnail": [f"{base}/n/{i}.jpg" for i in range(6)]}]}]}
    Handler.routes["/m/many.json"] = (json.dumps(doc).encode(), "application/json")
    error_of(f"{base}/m/many.json", allow_private=True)
    assert len(Handler.hits) <= media.MAX_REQUESTS


# --------------------------------------------------------------------------- #
# The switch
# --------------------------------------------------------------------------- #

def test_tool_absent_when_switched_off():
    async def names(**kw):
        mcp = build_mcp(False, settings(**kw))
        async with Client(mcp) as c:
            return {t.name for t in await c.list_tools()}, mcp.instructions

    on, text_on = asyncio.run(names())
    off, text_off = asyncio.run(names(media_preview=False))
    assert "media_preview" in on and "media_preview" not in off
    assert "media_preview" in text_on and "media_preview" not in text_off
    assert len(text_on) < 4096


def test_settings_and_flag():
    from rete_server.cli import settings_from
    from rete_server.settings import Settings

    s = Settings.from_env({})
    assert s.media_preview and not s.media_allow_private
    s = Settings.from_env({"RETE_MEDIA_PREVIEW": "0", "RETE_MEDIA_ALLOW_PRIVATE": "1"})
    assert not s.media_preview and s.media_allow_private
    assert not settings_from(["--no-media-preview"]).media_preview


def test_tool_is_marked_read_only_and_open_world():
    async def go():
        async with Client(build_mcp(False, settings())) as c:
            return next(t for t in await c.list_tools() if t.name == "media_preview")
    t = asyncio.run(go())
    assert t.annotations.read_only_hint and t.annotations.open_world_hint
