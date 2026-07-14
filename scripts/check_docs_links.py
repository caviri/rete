#!/usr/bin/env python3
"""Validate local files and anchors in generated documentation HTML."""

from __future__ import annotations

import argparse
from collections import Counter
from html.parser import HTMLParser
from pathlib import Path
import re
from urllib.parse import unquote, urlsplit


IGNORED_SCHEMES = {"data", "http", "https", "javascript", "mailto", "tel"}


class Page:
    def __init__(self) -> None:
        self.ids: list[str] = []
        self.links: list[str] = []
        self.headings: list[tuple[str | None, str]] = []


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.page = Page()
        self.heading_id: str | None = None
        self.heading_text: list[str] | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = dict(attrs)
        if anchor := values.get("id"):
            self.page.ids.append(anchor)
        for name in ("href", "src"):
            if link := values.get(name):
                self.page.links.append(link)
        if tag in {"h2", "h3"}:
            self.heading_id = values.get("id")
            self.heading_text = []

    def handle_data(self, data: str) -> None:
        if self.heading_text is not None:
            self.heading_text.append(data)

    def handle_endtag(self, tag: str) -> None:
        if tag in {"h2", "h3"} and self.heading_text is not None:
            self.page.headings.append((self.heading_id, "".join(self.heading_text)))
            self.heading_id = None
            self.heading_text = None


def add_runtime_heading_ids(page: Page) -> None:
    """Mirror docgen's client-side TOC slugger for h2/h3 anchors."""
    used: set[str] = set()
    for existing, text in page.headings:
        if existing:
            continue
        base = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-") or "section"
        anchor = base
        suffix = 2
        while anchor in used:
            anchor = f"{base}-{suffix}"
            suffix += 1
        used.add(anchor)
        page.ids.append(anchor)


def parse_page(path: Path) -> Page:
    parser = PageParser()
    parser.feed(path.read_text(encoding="utf-8"))
    parser.close()
    add_runtime_heading_ids(parser.page)
    return parser.page


def display_path(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()


def resolve_target(source: Path, raw_path: str, root: Path) -> Path | None:
    path_text = unquote(raw_path)
    target = source if not path_text else source.parent / path_text
    target = target.resolve()
    try:
        target.relative_to(root)
    except ValueError:
        return None
    if raw_path.endswith("/") or target.is_dir():
        target /= "index.html"
    return target


def is_playground_state(target: Path, fragment: str) -> bool:
    return target.name == "playground.html" and "=" in fragment


def check_docs(root: Path) -> list[str]:
    root = root.resolve()
    html_files = sorted(root.rglob("*.html"))
    pages = {path: parse_page(path) for path in html_files}
    errors: list[str] = []

    for path, page in pages.items():
        source_name = display_path(path, root)
        for anchor, count in sorted(Counter(page.ids).items()):
            if count > 1:
                errors.append(f"{source_name} -> duplicate anchor #{anchor}")

        for link in page.links:
            parts = urlsplit(link)
            if parts.scheme.lower() in IGNORED_SCHEMES or parts.netloc:
                continue
            target = resolve_target(path, parts.path, root)
            if target is None or not target.is_file():
                errors.append(f"{source_name} -> {link}")
                continue
            if not parts.fragment or is_playground_state(target, parts.fragment):
                continue
            fragment = unquote(parts.fragment)
            target_page = pages.get(target)
            if target_page is None:
                target_page = parse_page(target)
            if fragment not in target_page.ids:
                errors.append(f"{source_name} -> {link}")

    return sorted(set(errors))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "docs",
        help="documentation root (default: repository docs/)",
    )
    args = parser.parse_args()
    errors = check_docs(args.root)
    if errors:
        print("Broken documentation links:")
        for error in errors:
            print(f"  {error}")
        return 1
    print(f"Documentation links OK ({len(list(args.root.rglob('*.html')))} HTML files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
