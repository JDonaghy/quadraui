#!/usr/bin/env bash
# Live-app headless smoke for a quadraui GTK example (quadraui#450, GD-5).
#
# Runs the *real* `quadraui::gtk::run()` runner — real `Application` +
# `ApplicationWindow` + `GdkDisplay` — under Xvfb, with software (Cairo)
# rendering so no GPU is required. This is the class of bug the offscreen
# `GtkDriver` (quadraui#446..#448) structurally cannot catch: quadraui#437
# ("gtk_terminal opens with tiny/broken window, paste doesn't work") only
# reproduced against a live window.
#
# Operator-run (not wired into CI — the `gtk` CI job is deliberately
# Xvfb-free, see .github/workflows/ci.yml): requires `xvfb-run` on PATH
# (`apt install xvfb` — quadraui#450's operator-side step, not done by
# this script). Route this to a box that has it (e.g. dellserver, once
# it declares a `gtk-headless` coordinator capability per #450).
#
# Usage:
#   quadraui/scripts/gtk_smoke.sh [example-name] [feature,list]
#
# Examples:
#   quadraui/scripts/gtk_smoke.sh                       # gtk_terminal, gtk+terminal
#   quadraui/scripts/gtk_smoke.sh gtk_data_table gtk    # any other gtk_* example
#
# Env overrides:
#   QUADRAUI_GTK_SMOKE_MS     — ms before the scripted check fires and the
#                               window closes (default: 800)
#   QUADRAUI_GTK_SMOKE_PASTE  — text round-tripped through the real OS
#                               clipboard + replayed as a synthetic Ctrl-V
#                               (default: a short CJK/emoji mix, matching
#                               quadraui#437's original repro content)
#
# Exit code: 0 if the window opened at a sane size and (when
# QUADRAUI_GTK_SMOKE_PASTE is set) the clipboard round-tripped; non-zero
# otherwise. See `quadraui::gtk::run`'s "Headless smoke mode" module doc
# for exactly what's checked.

set -euo pipefail

EXAMPLE="${1:-gtk_terminal}"
FEATURES="${2:-gtk,terminal}"

if ! command -v xvfb-run >/dev/null 2>&1; then
    echo "gtk_smoke.sh: 'xvfb-run' not found on PATH." >&2
    echo "  This is the quadraui#450 operator-side prerequisite:" >&2
    echo "  install it with 'apt install xvfb' on the target box." >&2
    exit 127
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

export QUADRAUI_GTK_SMOKE_MS="${QUADRAUI_GTK_SMOKE_MS:-800}"
export QUADRAUI_GTK_SMOKE_PASTE="${QUADRAUI_GTK_SMOKE_PASTE:-quadraui smoke 你好 🎉}"
export GSK_RENDERER="${GSK_RENDERER:-cairo}"

echo "gtk_smoke.sh: xvfb-run -a env GSK_RENDERER=$GSK_RENDERER" \
    "cargo run --example $EXAMPLE --features $FEATURES" \
    "(QUADRAUI_GTK_SMOKE_MS=$QUADRAUI_GTK_SMOKE_MS)"

xvfb-run -a env GSK_RENDERER="$GSK_RENDERER" \
    cargo run --quiet --manifest-path "$REPO_ROOT/Cargo.toml" \
    --example "$EXAMPLE" --features "$FEATURES"
