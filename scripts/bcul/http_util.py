"""Polite, dependency-free HTTP fetcher for the BCU Lausanne digital-twin harvest.

stdlib only (urllib). Handles gzip, retries with backoff, and a global rate limit
so we stay a good citizen against patrinum.ch / Alma SRU / e-codices.
"""
from __future__ import annotations

import gzip
import time
import urllib.error
import urllib.request

UA = (
    "BCUL-DigitalTwin-Harvester/1.0 "
    "(rete-project; digital-twin research; contact carlosvivarrios@gmail.com)"
)

RETRY_STATUS = {429, 500, 502, 503, 504}


BROWSER_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
)


class Fetcher:
    def __init__(self, rate: float = 1.5, retries: int = 6, timeout: int = 90, ua: str | None = None):
        # rate = max requests/second (approx). None/0 disables throttling.
        self.min_interval = (1.0 / rate) if rate else 0.0
        self.retries = retries
        self.timeout = timeout
        self.ua = ua or UA
        self._last = 0.0
        self.n_requests = 0

    def _throttle(self) -> None:
        if self.min_interval:
            dt = time.time() - self._last
            if dt < self.min_interval:
                time.sleep(self.min_interval - dt)
        self._last = time.time()

    def get(self, url: str, accept: str | None = None):
        """Return (bytes, content_type, status). status 404 -> (None, None, 404)."""
        self._throttle()
        headers = {"User-Agent": self.ua, "Accept-Encoding": "gzip"}
        if accept:
            headers["Accept"] = accept
        last_err = None
        for attempt in range(self.retries):
            try:
                req = urllib.request.Request(url, headers=headers)
                with urllib.request.urlopen(req, timeout=self.timeout) as r:
                    data = r.read()
                    if r.headers.get("Content-Encoding") == "gzip":
                        data = gzip.decompress(data)
                    self.n_requests += 1
                    return data, r.headers.get_content_type(), r.status
            except urllib.error.HTTPError as e:
                last_err = e
                if e.code == 404:
                    return None, None, 404
                if e.code in RETRY_STATUS:
                    time.sleep(min(90, (2 ** attempt) * 2))
                    continue
                raise
            except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
                last_err = e
                time.sleep(min(45, 2 ** attempt))
        raise RuntimeError(f"GET failed after {self.retries} attempts: {url}\n  last error: {last_err}")

    def get_text(self, url: str, accept: str | None = None) -> str | None:
        data, _ctype, status = self.get(url, accept=accept)
        if data is None:
            return None
        return data.decode("utf-8", "replace")
