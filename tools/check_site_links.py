#!/usr/bin/env python3
"""Fail on a broken relative link in the built site (`site/book/`).

mdBook only warns about some missing chapters and never checks image paths
or links between pages, so a renamed page or a missing screenshot would
ship silently. This walks every built HTML page and checks that each
relative `href`/`src` points at a file that exists, and that a
`page.html#anchor` link's anchor is an `id` on that page. External links
(`http:`, `https:`, `mailto:`) are not fetched.

Usage, after `mdbook build site`:

    python3 tools/check_site_links.py site/book
"""
from __future__ import annotations

import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit


class _Collector(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.links: list[str] = []
        self.ids: set[str] = set()

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        for name, value in attrs:
            if value is None:
                continue
            if name == "id":
                self.ids.add(value)
            elif name in ("href", "src") and tag in ("a", "img"):
                self.links.append(value)


def parse(page: Path) -> _Collector:
    c = _Collector()
    c.feed(page.read_text(errors="replace"))
    return c


def broken_links(book: Path) -> list[str]:
    pages = sorted(book.rglob("*.html"))
    parsed = {p.resolve(): parse(p) for p in pages}
    problems = []
    for page, info in parsed.items():
        for link in info.links:
            parts = urlsplit(link)
            if parts.scheme or link.startswith("//") or link.startswith("javascript:"):
                continue
            target = page if not parts.path else (page.parent / unquote(parts.path)).resolve()
            if parts.path.endswith("/"):
                target = target / "index.html"
            rel = page.relative_to(book.resolve())
            if not target.exists():
                problems.append(f"{rel}: {link} -> missing {target.relative_to(book.resolve()) if book.resolve() in target.parents else target}")
                continue
            if parts.fragment and target.suffix == ".html":
                ids = parsed.get(target, parse(target)).ids
                if unquote(parts.fragment) not in ids:
                    problems.append(f"{rel}: {link} -> no id {parts.fragment!r} on that page")
    return problems


def main(argv: list[str]) -> int:
    book = Path(argv[1] if len(argv) > 1 else "site/book")
    if not book.is_dir():
        print(f"check_site_links: {book} is not a directory; run `mdbook build site` first", file=sys.stderr)
        return 2
    problems = broken_links(book)
    for p in problems:
        print(p)
    print(f"check_site_links: {len(problems)} broken link(s) in {book}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
