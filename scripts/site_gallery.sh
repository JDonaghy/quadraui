#!/usr/bin/env bash
# Regenerate the site's widget gallery and build the site.
#
# Every gallery page and screenshot comes from quadraui-gallery itself: its
# registry (each demo's name, group, variants and code region) and its
# --capture mode, which paints every demo through each compiled-in backend's
# headless test driver (no window opens) and writes one image per
# (demo, variant, backend) plus a manifest. tools/site_gen.py turns those
# into site/src/gallery/ and site/src/SUMMARY.md (from
# site/SUMMARY.template.md); both are generated, never edited by hand.
#
# Usage, from anywhere:
#   scripts/site_gallery.sh                  # terminal only
#   scripts/site_gallery.sh tui,gtk          # Linux: terminal + GTK4 columns
#   scripts/site_gallery.sh tui,macos        # on a Mac: terminal + macOS
#   EXTRA_CAPTURES="dir1 dir2" scripts/site_gallery.sh tui,gtk
#       # merge captures made elsewhere (each dir has a manifest.json),
#       # e.g. a Windows run of `quadraui-gallery --capture` or CI's
#       # gallery-capture-* artifacts
#
# Then `mdbook build site` runs if mdbook is on PATH (`cargo install mdbook`).
# .github/workflows/pages.yml runs the same steps on every push to develop,
# with the GTK, macOS and Windows columns from CI's capture jobs.
set -euo pipefail

FEATURES=${1:-tui}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=${SITE_WORK:-$ROOT/target/site}
cd "$ROOT"

rm -rf "$WORK/capture"
mkdir -p "$WORK/capture"
cargo build -q -p quadraui-gallery --bin quadraui-gallery --features "$FEATURES"
BIN=${CARGO_TARGET_DIR:-$ROOT/target}/debug/quadraui-gallery
"$BIN" --dump-registry "$WORK/registry.json"
# stdout carries terminal escape sequences from the caret demo; keep it off
# the console.
"$BIN" --capture "$WORK/capture" >/dev/null

captures=(--capture "$WORK/capture")
for dir in ${EXTRA_CAPTURES:-}; do
  captures+=(--capture "$dir")
done
python3 tools/site_gen.py \
  --registry "$WORK/registry.json" \
  "${captures[@]}" \
  --summary-template site/SUMMARY.template.md \
  --out site/src

if command -v mdbook >/dev/null; then
  mdbook build site
  python3 tools/check_site_links.py site/book
  echo "site_gallery: built site/book/ (open site/book/index.html, or: mdbook serve site)"
else
  echo "site_gallery: generated site/src/gallery/; install mdbook to build the site" >&2
fi
