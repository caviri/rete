#!/usr/bin/env python3
"""Cross-origin-isolated static server with HTTP Range support — for the
EXPERIMENTAL threaded playground (web/playground-threads.html).

Browser WASM threads need `SharedArrayBuffer`, which the browser only exposes on
a *cross-origin isolated* page. That requires two response headers:
    Cross-Origin-Opener-Policy: same-origin
    Cross-Origin-Embedder-Policy: require-corp
A plain static host (or file://) does not send these, so the threaded build will
NOT get a thread pool there. This server adds them (plus Range support, reused
from range_server.py) so you can run the experiment locally:

    python3 scripts/serve_coi.py 8080 web
    -> open http://localhost:8080/playground-threads.html

The normal offline playground (docs/playground.html) does NOT need this server.
"""
import os
import re
import sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
ROOT = sys.argv[2] if len(sys.argv) > 2 else "."


class COIRangeHandler(SimpleHTTPRequestHandler):
    def end_headers(self):
        # The cross-origin isolation headers that unlock SharedArrayBuffer.
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def do_GET(self):
        rng = self.headers.get("Range")
        if not rng:
            return super().do_GET()
        m = re.match(r"bytes=(\d+)-(\d*)", rng)
        path = self.translate_path(self.path)
        if not m or not os.path.isfile(path):
            return super().do_GET()
        size = os.path.getsize(path)
        start = int(m.group(1))
        end = int(m.group(2)) if m.group(2) else size - 1
        end = min(end, size - 1)
        if start > end:
            self.send_error(416, "Requested Range Not Satisfiable")
            return
        self.send_response(206)
        self.send_header("Content-Type", self.guess_type(path))
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Content-Length", str(end - start + 1))
        self.end_headers()
        with open(path, "rb") as f:
            f.seek(start)
            self.wfile.write(f.read(end - start + 1))


os.chdir(ROOT)
print(f"cross-origin-isolated server on http://localhost:{PORT}/  (root: {ROOT})", file=sys.stderr)
print(f"  open http://localhost:{PORT}/playground-threads.html", file=sys.stderr)
ThreadingHTTPServer(("", PORT), COIRangeHandler).serve_forever()
