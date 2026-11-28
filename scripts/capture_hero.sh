#!/usr/bin/env bash
# Regenerate the README / site hero screenshots: the counter from
# examples/hero/rust, painted off screen by each backend's headless test
# driver (no window opens), written to site/src/images/hero-<backend>.png.
#
# Usage, from the repo root, on each host that can render a backend:
#   scripts/capture_hero.sh tui,gtk     # Linux: terminal + GTK4
#   scripts/capture_hero.sh tui,macos   # macOS: Core Graphics
#   scripts/capture_hero.sh tui,win     # Windows: Direct2D (run under Git Bash)
# Only the backends in the feature list are (re)written; commit the PNGs.
#
# The TUI image is an SVG of the cell grid, rasterised by tui_svg2png.py with
# Pango (needs python3-gi, gir1.2-pango-1.0 and python3-gi-cairo); without
# them the SVG is kept and the PNG step is skipped with a warning.
set -euo pipefail

FEATURES=${1:-tui,gtk}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=$ROOT/site/src/images
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

cargo run -q -p quadraui-hero --bin hero-capture --features "capture,$FEATURES" -- "$TMP"

for f in "$TMP"/hero-*.png; do
  [ -e "$f" ] && cp "$f" "$OUT/"
done
if [ -e "$TMP/hero-tui.svg" ]; then
  mkdir -p "$TMP/svg" && mv "$TMP/hero-tui.svg" "$TMP/svg/"
  PY=/usr/bin/python3; [ -x "$PY" ] || PY=python3
  if "$PY" -c 'import gi; gi.require_version("PangoCairo", "1.0")' 2>/dev/null; then
    "$PY" "$ROOT/scripts/tui_svg2png.py" "$TMP/svg" "$TMP/png" "DejaVu Sans Mono"
    cp "$TMP/png/hero-tui.png" "$OUT/"
  else
    echo "capture_hero: no PyGObject/Pango; keeping site/src/images/hero-tui.svg only" >&2
    cp "$TMP/svg/hero-tui.svg" "$OUT/"
  fi
fi
ls -l "$OUT"/hero-*
