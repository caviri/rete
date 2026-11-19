"""The catalog: which datasets this server answers for.

Three kinds of entry, merged into one in-memory index:

* **published** — read from ``catalog.json`` sources (URLs or files). The
  default source is the catalog published next to the data on R2; when it
  cannot be read the copy bundled in the package stands in for it. URL
  sources are re-fetched every ``catalog_refresh_s`` with ``If-None-Match``,
  and a failed refresh keeps the last good copy.
* **local** — every ``.rete`` under ``data_dir``, keyed ``local/<path>``. Each
  file describes itself: title and description come from its embedded
  Dataset Card, read in the background so startup never waits on it.
* **generated** — files the authoring tools built, keyed ``generated/<name>``.

Lookups are a dict access. The relay this replaces re-read catalog.json and
re-walked the data folder on every request.
"""
from __future__ import annotations

import json
import logging
import threading
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple
from urllib.parse import urljoin

import httpx

from .settings import BUNDLED_CATALOG, DEFAULT_CATALOG_URL, Settings

log = logging.getLogger("rete_server.catalog")

# Catalog fields kept from a published entry (anything else is dropped).
PUBLISHED_FIELDS = (
    "key", "label", "description", "url", "shards", "kind", "triples", "size",
    "license", "source", "tags", "examples", "shacl_shapes",
)


def _is_url(source: str) -> bool:
    return source.startswith(("http://", "https://"))


def _parse(doc: Any, base: str) -> List[Dict[str, Any]]:
    """Entries of a catalog document (``{"datasets": [...]}`` or a bare list).

    A relative ``url`` resolves against the catalog's own location, so a
    catalog.json can sit beside the files it lists (on a web host or a disk).
    """
    rows = doc.get("datasets") if isinstance(doc, dict) else doc
    if not isinstance(rows, list):
        raise ValueError("a catalog is {\"datasets\": [...]} or a list of entries")
    out = []
    for row in rows:
        if not isinstance(row, dict) or not row.get("key"):
            continue
        entry = {k: row[k] for k in PUBLISHED_FIELDS if row.get(k) is not None}
        url = entry.get("url")
        if url and not _is_url(url):
            if _is_url(base):
                entry["url"] = urljoin(base, url)
            else:
                path = (Path(base).parent / url).resolve()
                entry.pop("url")
                entry["local_path"] = str(path)
        entry.setdefault("kind", "published")
        out.append(entry)
    return out


class Catalog:
    def __init__(self, settings: Settings, http: Optional[httpx.Client] = None):
        self.sources = list(settings.catalogs)
        self.data_dir = settings.data_dir
        self.generated_dir = settings.generated_dir
        self.refresh_s = settings.catalog_refresh_s
        self._http = http or httpx.Client(
            timeout=20.0, follow_redirects=True, headers={"User-Agent": "rete-server"})
        self._lock = threading.RLock()
        # source -> (etag or mtime, entries); the last good read of each source
        self._published: Dict[str, Tuple[Any, List[Dict[str, Any]]]] = {}
        self._status: Dict[str, str] = {}
        self._local: Dict[str, Dict[str, Any]] = {}
        self._generated: Dict[str, Dict[str, Any]] = {}
        # (path, size, mtime) -> card summary, so a rescan never re-reads a card
        self._cards: Dict[Tuple[str, int, float], Dict[str, Any]] = {}
        self._index: Dict[str, Dict[str, Any]] = {}
        self._order: List[str] = []
        self._stop = threading.Event()
        self._thread: Optional[threading.Thread] = None
        self._loaded_at: Optional[float] = None
        self._last_local_scan = 0.0

    # ------------------------------------------------------------------ #
    # Loading
    # ------------------------------------------------------------------ #

    def load(self) -> None:
        """Read every source and scan the folders. Never raises."""
        for source in self.sources:
            self._load_source(source)
        self._scan_local()
        self._scan_generated()
        self._rebuild()
        self._loaded_at = time.time()

    def _load_source(self, source: str) -> None:
        previous = self._published.get(source)
        try:
            if _is_url(source):
                headers = {}
                if previous and previous[0]:
                    headers["If-None-Match"] = previous[0]
                r = self._http.get(source, headers=headers)
                if r.status_code == 304 and previous:
                    self._status[source] = "current"
                    return
                r.raise_for_status()
                entries = _parse(r.json(), str(r.url))
                self._published[source] = (r.headers.get("etag"), entries)
            else:
                path = Path(source)
                mtime = path.stat().st_mtime
                if previous and previous[0] == mtime:
                    return
                entries = _parse(json.loads(path.read_text(encoding="utf-8")), str(path.resolve()))
                self._published[source] = (mtime, entries)
            self._status[source] = f"ok ({len(entries)} datasets)"
        except Exception as e:  # keep serving what we had
            reason = f"{type(e).__name__}: {e}"
            if previous:
                self._status[source] = f"stale, last refresh failed: {reason}"
            elif source == DEFAULT_CATALOG_URL and BUNDLED_CATALOG.is_file():
                entries = _parse(json.loads(BUNDLED_CATALOG.read_text(encoding="utf-8")),
                                 "https://data.graphplaza.com/")
                # No etag: the next refresh retries the published copy.
                self._published[source] = (None, entries)
                self._status[source] = f"bundled copy ({len(entries)} datasets); published one unreadable: {reason}"
            else:
                self._status[source] = f"unreadable: {reason}"
            log.warning("catalog %s: %s", source, self._status[source])

    def _scan_local(self) -> None:
        self._last_local_scan = time.time()
        found: Dict[str, Dict[str, Any]] = {}
        root = self.data_dir
        if root is not None and root.is_dir():
            for path in sorted(root.rglob("*.rete")):
                rel = path.relative_to(root)
                if any(part.startswith(".") for part in rel.parts):
                    continue
                stat = path.stat()
                entry = {
                    "key": f"local/{rel.as_posix()}",
                    "label": rel.stem,
                    "description": "A .rete file in this server's data folder.",
                    "local_path": str(path),
                    "size_bytes": stat.st_size,
                    "kind": "local",
                }
                entry.update(self._cards.get((str(path), stat.st_size, stat.st_mtime), {}))
                found[entry["key"]] = entry
        with self._lock:
            self._local = found

    def _scan_generated(self) -> None:
        found: Dict[str, Dict[str, Any]] = {}
        root = self.generated_dir
        if root.is_dir():
            for path in sorted(root.glob("*.rete")):
                found[f"generated/{path.name}"] = self._generated_entry(path)
        with self._lock:
            self._generated = found

    @staticmethod
    def _generated_entry(path: Path) -> Dict[str, Any]:
        return {
            "key": f"generated/{path.name}",
            "label": path.stem,
            "description": "Built on this server (ephemeral): by build_rete, or opened in a graph view.",
            "url": f"/generated/{path.name}",
            "local_path": str(path),
            "size_bytes": path.stat().st_size,
            "kind": "generated",
        }

    def add_generated(self, path: Path, label: Optional[str] = None) -> Dict[str, Any]:
        entry = self._generated_entry(path)
        if label:
            entry["label"] = label
        with self._lock:
            self._generated[entry["key"]] = entry
            self._rebuild()
        return entry

    def drop_generated(self, path: Path) -> None:
        with self._lock:
            self._generated.pop(f"generated/{path.name}", None)
            self._rebuild()

    def _rebuild(self) -> None:
        index: Dict[str, Dict[str, Any]] = {}
        order: List[str] = []
        with self._lock:
            groups = [entries for _, entries in self._published.values()]
            groups += [list(self._local.values()), list(self._generated.values())]
            for entries in groups:
                for entry in entries:
                    if entry["key"] in index:
                        continue  # the first source to name a key wins
                    index[entry["key"]] = entry
                    order.append(entry["key"])
            self._index, self._order = index, order

    # ------------------------------------------------------------------ #
    # Local cards (background)
    # ------------------------------------------------------------------ #

    def read_local_cards(self) -> None:
        """Fill title/description/counts of local files from their cards."""
        import rete_graph as rete

        for entry in list(self._local.values()):
            path = Path(entry["local_path"])
            try:
                stat = path.stat()
            except OSError:
                continue
            ident = (str(path), stat.st_size, stat.st_mtime)
            if ident in self._cards:
                continue
            summary: Dict[str, Any] = {}
            try:
                card = rete.open(str(path)).card() or {}
                if card.get("title"):
                    summary["label"] = card["title"]
                if card.get("description"):
                    summary["description"] = card["description"]
                for field in ("license", "source"):
                    if card.get(field):
                        summary[field] = card[field]
                if card.get("quad_count") is not None:
                    summary["triples"] = card["quad_count"]
            except Exception as e:
                log.warning("card of %s unreadable: %s", path, e)
            self._cards[ident] = summary
            with self._lock:
                if entry["key"] in self._local:
                    self._local[entry["key"]].update(summary)
        self._rebuild()

    # ------------------------------------------------------------------ #
    # Background refresh
    # ------------------------------------------------------------------ #

    def start(self) -> None:
        self._thread = threading.Thread(target=self._loop, name="catalog-refresh", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()

    def _loop(self) -> None:
        self.read_local_cards()
        while not self._stop.wait(self.refresh_s):
            try:
                self.load()
                self.read_local_cards()
            except Exception:  # the loop must survive anything
                log.exception("catalog refresh failed")

    # ------------------------------------------------------------------ #
    # Lookups
    # ------------------------------------------------------------------ #

    def entries(self) -> List[Dict[str, Any]]:
        index = self._index
        return [index[k] for k in self._order if k in index]

    def get(self, key: str) -> Optional[Dict[str, Any]]:
        entry = self._index.get(key)
        if entry is None and key.startswith("local/") and time.time() - self._last_local_scan > 5:
            # A file dropped into the folder since the last scan (rate-limited:
            # unknown keys must not turn every request into a folder walk).
            self._scan_local()
            self._rebuild()
            entry = self._index.get(key)
        return entry

    def keys(self) -> List[str]:
        return list(self._order)

    def status(self) -> Dict[str, Any]:
        counts: Dict[str, int] = {}
        for entry in self._index.values():
            counts[entry.get("kind") or "published"] = counts.get(entry.get("kind") or "published", 0) + 1
        return {"datasets": len(self._index), "by_kind": counts,
                "sources": dict(self._status),
                "loaded_at": (time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(self._loaded_at))
                              if self._loaded_at else None)}
