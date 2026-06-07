#!/usr/bin/env python3
"""Minimal static HTTP server that honors single-range `Range: bytes=a-b`
requests — enough to serve `.rete` files to the range-reading client.
Python's built-in http.server ignores Range (always 200), which breaks
offset reads, so we add 206 Partial Content support here.

Usage: python3 scripts/range_server.py [PORT] [DIR]   (defaults: 8000 .)
"""
import os
import re
import sys
from http.server import HTTPServer, SimpleHTTPRequestHandler


class RangeHandler(SimpleHTTPRequestHandler):
    def do_GET(self):
        rng = self.headers.get("Range")
        path = self.translate_path(self.path)
        if rng is None or not os.path.isfile(path):
            return super().do_GET()

        m = re.match(r"bytes=(\d+)-(\d*)", rng)
        if not m:
            return super().do_GET()

        size = os.path.getsize(path)
        start = int(m.group(1))
        end = int(m.group(2)) if m.group(2) else size - 1
        end = min(end, size - 1)
        length = end - start + 1

        with open(path, "rb") as f:
            f.seek(start)
            data = f.read(length)

        self.send_response(206)
        self.send_header("Content-Type", self.guess_type(path))
        self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8000
    directory = sys.argv[2] if len(sys.argv) > 2 else "."
    os.chdir(directory)
    HTTPServer(("", port), RangeHandler).serve_forever()
