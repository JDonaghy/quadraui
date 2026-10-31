#!/usr/bin/env bash
# Build the `quadraui-py` extension module into a venv with `maturin
# develop`, then run its `pytest` suite against the real interpreter that
# produced. CI's `quadraui-py-tests` job and a local/dell64-style run both
# call this one script so the two never drift apart.
#
# Every other CI leg (`tui`, `gtk`, `msrv`, ...) runs `--exclude quadraui-py`
# because the crate's default `cdylib` build links against libpython, which
# those jobs' runners don't provision, and a `pyo3/extension-module`-only
# `cargo check`/`clippy` proves it compiles but never loads it into Python.
# This script is the one path that actually imports the built extension and
# exercises its behaviour.
#
# Usage:
#   scripts/test_quadraui_py.sh              # build + run the full suite
#   scripts/test_quadraui_py.sh -k foo        # args forwarded to pytest
#
# Prerequisites: a Rust toolchain able to build `quadraui-py` (see
# `quadraui-py/Cargo.toml`), and a `python3` on PATH with the `venv` module
# (>= 3.8, matching `quadraui-py/pyproject.toml`'s `requires-python`).

set -euo pipefail

# Echo each command under CI so an admin-gated Actions log pinpoints the exact
# failing step; stays quiet for a local/dell64 run.
[ -n "${CI:-}" ] && set -x

# Surface a real, scrapeable error whatever fails. A combined "build +
# pytest" step that dies silently (an extension panic on import, a killed
# build, a pip resolver error) leaves a CI log the diagnostic scraper can't
# match. `RUST_BACKTRACE=full` turns any panic crossing the extension's FFI
# boundary into a printed backtrace; `PYTHONUNBUFFERED` stops the last lines
# before a crash being swallowed in a buffer; the phase markers localise a
# failure to pip / maturin / pytest; and `python -VV` + `rustc -vV` pin the
# exact interpreter and toolchain the run actually used.
export RUST_BACKTRACE="${RUST_BACKTRACE:-full}"
export PYTHONUNBUFFERED=1

phase() { echo "::quadraui-py:: ===== $* ====="; }

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The venv must not live under `target/`: that directory is saved and restored
# by `Swatinem/rust-cache@v2`, and a restored cache carries a venv skeleton
# whose interpreter symlinks and `pyvenv.cfg` point at a different runner's
# Python, which makes `python3 -m venv` abort. On CI a fresh `$RUNNER_TEMP`
# sidesteps the cache entirely; a local run (no `RUNNER_TEMP`) still defaults
# under `target/`, and `QUADRAUI_PY_VENV` overrides both.
default_venv_parent="${RUNNER_TEMP:-$repo_root/target}"
venv_dir="${QUADRAUI_PY_VENV:-$default_venv_parent/quadraui-py-venv}"

cd "$repo_root"

phase "toolchain"
rustc -vV || true

phase "create venv ($venv_dir)"
# Recreate from scratch so no leftover or restored-from-cache venv state can
# break creation or shadow a rebuilt interpreter.
rm -rf "$venv_dir"
python3 -m venv "$venv_dir"
if [ -x "$venv_dir/bin/python" ]; then
  venv_python="$venv_dir/bin/python"
else
  venv_python="$venv_dir/Scripts/python.exe"
fi

phase "interpreter"
"$venv_python" -VV

phase "pip install (maturin + pytest)"
"$venv_python" -m pip install --upgrade pip
"$venv_python" -m pip install maturin pytest

# `maturin develop` looks for an *activated* virtualenv (`VIRTUAL_ENV`) or
# `.venv`, not just a python interpreter path — it never sees one since this
# script calls the venv's `python` directly rather than sourcing its
# `activate` script.
export VIRTUAL_ENV="$venv_dir"
phase "maturin develop --release"
(cd quadraui-py && "$venv_python" -m maturin develop --release)

phase "pytest"
# `-v -rA` prints a line per test (so a silent collection error or crash is
# visible) and a short reason summary for every outcome, not just failures.
exec "$venv_python" -m pytest quadraui-py/tests -v -rA "$@"
