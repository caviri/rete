#!/usr/bin/env python3
"""Tiny static server with HTTP Range (206) support — PMTiles reads the archive
by byte range, which the stdlib SimpleHTTPRequestHandler does not honor. Serves
this directory so `viewer.html` can range-read `out/graphmap.pmtiles`.

    python experiments/graph-map/serve.py [port]   # default 8000
"""
import os
import re
import sys
from functools import partial
from http.server import HTTPServer, SimpleHTTPRequestHandler

DIR = os.path.dirname(os.path.abspath(__file__))


class RangeHandler(SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()

    def do_GET(self):
        rng = self.headers.get("Range")
        path = self.translate_path(self.path)
        if not rng or not os.path.isfile(path):
            return super().do_GET()
        m = re.match(r"bytes=(\d+)-(\d*)", rng)
        if not m:
            return super().do_GET()
        size = os.path.getsize(path)
        start = int(m.group(1))
        end = int(m.group(2)) if m.group(2) else size - 1
        end = min(end, size - 1)
        length = end - start + 1
        ctype = self.guess_type(path)
        self.send_response(206)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Content-Length", str(length))
        self.end_headers()
        with open(path, "rb") as f:
            f.seek(start)
            self.wfile.write(f.read(length))


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8000
    httpd = HTTPServer(("0.0.0.0", port), partial(RangeHandler, directory=DIR))
    print(f"serving {DIR} on http://localhost:{port}  (open /viewer.html)", flush=True)
    httpd.serve_forever()


if __name__ == "__main__":
    main()
