"""Shared helpers for the WikiArt harvest. Stdlib only (runs on python:3.12-slim).

WikiArt exposes two keyless JSON surfaces:

  /en/api/2/*   the documented v2 API. Records are keyed by 24-hex Mongo ids and
                carry the rich fields (biography, description, tags, galleries,
                styles/genres/media as arrays). Paginated via `paginationToken`
                + `hasMore`, 60 records per page.
  /en/App/*     the endpoints the site itself calls. Keyed by numeric `contentId`
                and returns whole lists in one shot. Thinner, but it is the only
                place `contentId` appears -- the id used by the WikiArt image
                dumps (ArtGAN / huggan) and by /en/App/Painting/ImageJson.

Both are fetched so the two id systems can be joined on (artistUrl, url).
"""

import gzip
import io
import json
import os
import random
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

BASE = "https://www.wikiart.org"
UA = "rete-dataset-harvest/1.0 (+https://github.com/caviri/rete; contact via repo issues)"

# WikiArt sits behind Cloudflare. 24 concurrent requests measured clean, but the
# harvest is long-running so we stay well under that and back off on any 429/5xx.
DEFAULT_WORKERS = int(os.environ.get("WIKIART_WORKERS", "12"))
RETRIES = int(os.environ.get("WIKIART_RETRIES", "6"))

_throttle = threading.Semaphore(DEFAULT_WORKERS)


def raw_dir():
    here = os.path.dirname(os.path.abspath(__file__))
    return os.path.abspath(os.path.join(here, "..", "raw"))


def get(path, params=None, retries=RETRIES, timeout=60, raw_query=None):
    """GET a WikiArt JSON endpoint, with retry + exponential backoff.

    `path` may be an absolute URL or a site-relative path.
    `raw_query` is appended verbatim, bypassing urlencode -- needed for
    pagination tokens, which arrive already percent-encoded (see `paged`).
    Returns the decoded JSON, or raises after `retries` attempts.
    """
    url = path if path.startswith("http") else BASE + path
    if params:
        url += ("&" if "?" in url else "?") + urllib.parse.urlencode(params)
    if raw_query:
        url += ("&" if "?" in url else "?") + raw_query

    last = None
    for attempt in range(retries):
        try:
            with _throttle:
                req = urllib.request.Request(
                    url, headers={"User-Agent": UA, "Accept-Encoding": "gzip"}
                )
                with urllib.request.urlopen(req, timeout=timeout) as r:
                    body = r.read()
                    if r.headers.get("Content-Encoding") == "gzip":
                        body = gzip.GzipFile(fileobj=io.BytesIO(body)).read()
            try:
                return json.loads(body.decode("utf-8"))
            except (ValueError, UnicodeDecodeError):
                # Followed a redirect to the HTML error page -- deterministic.
                raise NotJson(f"non-JSON body ({len(body)}B) from {url}")
        except urllib.error.HTTPError as e:
            last = e
            if e.code == 404:
                raise           # a real "not found" -- caller decides
            if e.code == 500:
                # Distinguish the quota wall from a transient 500: both are 500,
                # only the body differs. Retrying a quota 500 is pure waste.
                try:
                    msg = json.loads(e.read().decode("utf-8", "replace"))
                    msg = (msg.get("Exception") or {}).get("Message") or ""
                except Exception:
                    msg = ""
                if "limit exceeded" in msg.lower():
                    raise QuotaExceeded(msg)
            if e.code not in (403, 408, 429, 500, 502, 503, 504):
                raise
        except NotJson:
            raise                   # deterministic -- retrying cannot help
        except Exception as e:      # timeouts, connection resets
            last = e
        if attempt == retries - 1:
            break                   # no point sleeping before giving up
        # jittered exponential backoff: 1s, 2s, 4s, 8s, 16s, 32s
        time.sleep(min(2 ** attempt, 32) + random.random())
    raise RuntimeError(f"GET failed after {retries} attempts: {url} ({last})")


class NotJson(Exception):
    """The server answered, but with something that is not JSON.

    WikiArt does this by 302-ing a handful of ids to an HTML error page --
    records that exist in an artist's oeuvre listing but have no detail document
    (e.g. contentId 197943, "The Death of Marat"). The redirect is deterministic,
    so this is raised immediately rather than retried, and callers record the id
    as a permanent miss.
    """


class QuotaExceeded(Exception):
    """The keyless /en/api/2/ quota is spent.

    WikiArt meters anonymous use of the documented v2 API and then answers every
    v2 request with HTTP 500 {"Exception":{"Message":"Free API limit exceeded"}}.
    Retrying does not help and only deepens the hole, so this is raised
    immediately and callers stop.

    The /en/App/ endpoints are NOT metered -- they are the site's own AJAX layer
    and keep serving normally. Everything essential is reachable through them.
    """


class PagedWall(Exception):
    """A paginated chain hit a deterministic server-side 500 and cannot continue.

    Carries the records collected before the wall. See `paged(on_error="stop")`.
    """

    def __init__(self, records, token, cause):
        super().__init__(f"pagination walled after {len(records)} records: {cause}")
        self.records, self.token, self.cause = records, token, cause


def paged(path, params=None, limit_pages=None):
    """Yield every record from a paginated /en/api/2/ endpoint.

    The v2 API returns {"data": [...], "paginationToken": str, "hasMore": bool}.

    GOTCHA: the token arrives ALREADY percent-encoded (base64 with '+', '/' and
    '=' written as %2b, %2f, %3d). Passing it through urlencode re-encodes the
    '%' to '%25' and the next request 500s. It must go on the URL verbatim.
    """
    for rec in paged_list(path, params, limit_pages):
        yield rec


def paged_list(path, params=None, limit_pages=None, on_error="raise"):
    """Collect every record from a paginated /en/api/2/ endpoint.

    on_error="stop" returns what was collected instead of raising when the chain
    hits a wall -- see the UpdatedArtists note in harvest_artists.py. The caller
    gets (records, walled_token) via PagedWall only when on_error="raise".
    """
    params = dict(params or {})
    out, token, pages = [], None, 0
    while True:
        try:
            page = get(path, params,
                       raw_query=("paginationToken=" + token) if token else None)
        except Exception as e:
            if on_error == "stop":
                return out, token, e
            raise PagedWall(out, token, e)
        out.extend(page.get("data", []))
        pages += 1
        token = page.get("paginationToken")
        if not page.get("hasMore") or not token:
            return (out, None, None) if on_error == "stop" else out
        if limit_pages and pages >= limit_pages:
            return (out, token, None) if on_error == "stop" else out


class JsonlSink:
    """Append-only JSONL writer that is safe to call from many threads.

    Resumable: `seen` is preloaded from the existing file, so re-running the
    harvest only fetches what is missing. Writes go through one lock and are
    flushed per batch so a killed run loses at most the OS buffer.
    """

    def __init__(self, path, key="id"):
        self.path = path
        self.key = key
        self.seen = set()
        self.lock = threading.Lock()
        self.n = 0
        os.makedirs(os.path.dirname(path), exist_ok=True)
        if os.path.exists(path):
            with open(path, "r", encoding="utf-8") as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        self.seen.add(json.loads(line)[self.key])
                    except Exception:
                        continue        # tolerate a truncated final line
            self.n = len(self.seen)
        self.fh = open(path, "a", encoding="utf-8")

    def write(self, rec):
        k = rec.get(self.key)
        with self.lock:
            if k in self.seen:
                return False
            self.seen.add(k)
            self.fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
            self.n += 1
            if self.n % 500 == 0:
                self.fh.flush()
            return True

    def close(self):
        self.fh.flush()
        self.fh.close()


def progress(done, total, label, t0):
    el = time.time() - t0
    rate = done / el if el > 0 else 0
    eta = (total - done) / rate if rate > 0 and total else 0
    sys.stderr.write(
        f"\r  {label}: {done:,}/{total:,}  {rate:6.1f}/s  elapsed {el/60:5.1f}m  eta {eta/60:5.1f}m   "
    )
    sys.stderr.flush()
