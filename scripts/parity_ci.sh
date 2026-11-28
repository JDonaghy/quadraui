#!/usr/bin/env bash
# Build everything the cross-language parity check needs, then run it:
# the `quadraui` Python extension (maturin develop into a fresh venv), the
# Node package (`npm ci` + `npm run build:debug` + `npm run build:dist`),
# and the Rust tracer (`cargo build -p quadraui-parity`). CI's `parity` job
# and a local run share this script.
#
# Usage:
#   scripts/parity_ci.sh                 # build all three + run the check
#   scripts/parity_ci.sh --keep-going    # args forwarded to scripts/parity_check.py
#
# Prerequisites: a Rust toolchain, `python3` with the `venv` module, and
# Node >= 22.6 (the tracers run `.ts` files directly) with npm.

set -euo pipefail
[ -n "${CI:-}" ] && set -x
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}" PYTHONUNBUFFERED=1

phase() { echo "::parity:: ===== $* ====="; }

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
# Outside `target/` on CI for the reason scripts/test_quadraui_py.sh gives:
# a venv restored from the rust cache points at another runner's Python.
venv_dir="${QUADRAUI_PARITY_VENV:-${RUNNER_TEMP:-$repo_root/target}/quadraui-parity-venv}"

phase "python extension (maturin develop, $venv_dir)"
rm -rf "$venv_dir"
python3 -m venv "$venv_dir"
venv_python="$venv_dir/bin/python"
"$venv_python" -m pip install --quiet --upgrade pip maturin
(cd quadraui-py && VIRTUAL_ENV="$venv_dir" "$venv_python" -m maturin develop)

phase "node addon (npm ci + build:debug + build:dist)"
(cd quadraui-node && npm ci && npm run build:debug && npm run build:dist)

phase "rust tracer"
cargo build -p quadraui-parity --features tui --bin parity-trace

phase "parity check"
exec "$venv_python" scripts/parity_check.py --rust-bin "$target_dir/debug/parity-trace" "$@"
