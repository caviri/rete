"""Latency-injecting HTTP range proxy (copied from rete-datasets bench/latency_proxy.py) that also LOGS each
request's arrival time, so round trips can be counted: every request waits RTT_MS before it is forwarded to the
upstream (nginx); requests are served concurrently, so N ranges fanned out at once pay one RTT.
usage: python3 lat_proxy.py PORT UPSTREAM_HOST RTT_MS LOGFILE
"""
import http.client
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT, UP, RTT, LOG = int(sys.argv[1]), sys.argv[2], float(sys.argv[3]) / 1000.0, sys.argv[4]
LOCK = threading.Lock()
LOGF = open(LOG, "a", buffering=1)


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _fwd(self, method):
        t = time.time()
        with LOCK:
            LOGF.write(f"{t:.6f}\t{method}\t{self.path}\t{self.headers.get('Range', '')}\n")
        if self.path.startswith("/__mark__"):
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        time.sleep(RTT)
        c = http.client.HTTPSConnection(UP, 443, timeout=600)
        hdr = {k: v for k, v in self.headers.items() if k.lower() in ("range", "accept")}; hdr["User-Agent"] = "rete-ws20-measure/1"
        c.request(method, self.path, headers=hdr)
        r = c.getresponse()
        body = r.read()
        self.send_response(r.status)
        for k, v in r.getheaders():
            if k.lower() in ("content-range", "accept-ranges", "content-type", "etag", "last-modified"):
                self.send_header(k, v)
        self.send_header("Content-Length", str(len(body) if method == "GET" else r.getheader("Content-Length", "0")))
        self.end_headers()
        if method == "GET":
            self.wfile.write(body)
        c.close()

    def do_GET(self):
        self._fwd("GET")

    def do_HEAD(self):
        self._fwd("HEAD")


ThreadingHTTPServer.daemon_threads = True
ThreadingHTTPServer(("0.0.0.0", PORT), H).serve_forever()
