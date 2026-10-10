#!/usr/bin/env python3
"""Prove a Rust diff changed comments only.

Strips every comment (`//`, `///`, `//!`, `/* */`) from each changed `.rs`
file at BASE and at HEAD, collapses whitespace outside string/char
literals, and requires the two token streams to be identical. Also requires
that doc-comment code blocks (doctests) and `SAFETY:` notes are unchanged in
number, so a cleanup can't silently drop a doctest or a safety argument.

Usage: tools/comment_only_check.py [BASE] [HEAD]   (default origin/develop HEAD)
Exit 0 when every changed .rs file differs only in comments.
"""
from __future__ import annotations

import re
import subprocess
import sys


def strip(src: str) -> tuple[str, list[str], int]:
    out, doc_blocks, safety = [], [], 0
    i, n = 0, len(src)
    pending_space = False

    def emit(tok: str) -> None:
        nonlocal pending_space
        if pending_space and out:
            out.append(" ")
        pending_space = False
        out.append(tok)

    doc_lines: list[str] = []
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j == -1 else j
            text = src[i:j]
            if "SAFETY:" in text:
                safety += 1
            if text.startswith(("///", "//!")):
                doc_lines.append(text[3:].strip())
            i = j
            pending_space = True
            continue
        if src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            if "SAFETY:" in src[i:j]:
                safety += 1
            i = j
            pending_space = True
            continue
        if c.isspace():
            pending_space = True
            i += 1
            continue
        m = re.match(r'b?r(#*)"', src[i:])
        if m:
            end = src.find('"' + m.group(1), i + len(m.group(0)))
            end = n if end == -1 else end + 1 + len(m.group(1))
            emit(src[i:end])
            i = end
            continue
        if c == '"' or src.startswith('b"', i):
            j = i + (2 if c == "b" else 1)
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            emit(src[i : j + 1])
            i = j + 1
            continue
        if c == "'":
            m = re.match(r"'(\\.[^']*|[^'\\])'", src[i:])
            if m:  # char literal; otherwise a lifetime, handled as plain text
                emit(m.group(0))
                i += len(m.group(0))
                continue
        j = i + 1
        while j < n and not src[j].isspace() and src[j] not in "\"'/" :
            j += 1
        emit(src[i:j])
        i = j
    # Doctest blocks: fenced code inside doc comments.
    block, inside = [], False
    for line in doc_lines:
        if line.startswith("```"):
            if inside:
                doc_blocks.append("\n".join(block))
                block = []
            inside = not inside
        elif inside:
            block.append(line)
    return "".join(out), doc_blocks, safety


def show(rev: str, path: str) -> str:
    r = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else ""


def main() -> int:
    base = sys.argv[1] if len(sys.argv) > 1 else "origin/develop"
    head = sys.argv[2] if len(sys.argv) > 2 else "HEAD"
    files = subprocess.run(
        ["git", "diff", "--name-only", f"{base}...{head}"], capture_output=True, text=True, check=True
    ).stdout.split()
    bad = 0
    for f in files:
        if not f.endswith(".rs"):
            print(f"NON-RUST  {f}")
            bad += 1
            continue
        a, b = strip(show(base, f)), strip(show(head, f))
        problems = []
        if a[0] != b[0]:
            problems.append("code changed")
        if sorted(a[1]) != sorted(b[1]):
            problems.append(f"doctest blocks changed ({len(a[1])} -> {len(b[1])})")
        if b[2] < a[2]:
            problems.append(f"SAFETY notes dropped ({a[2]} -> {b[2]})")
        print(("FAIL" if problems else "ok  "), f, "; ".join(problems))
        bad += bool(problems)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
