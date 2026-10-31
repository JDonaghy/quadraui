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

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
venv_dir="${QUADRAUI_PY_VENV:-$repo_root/target/quadraui-py-venv}"

cd "$repo_root"

python3 -m venv "$venv_dir"
if [ -x "$venv_dir/bin/python" ]; then
  venv_python="$venv_dir/bin/python"
else
  venv_python="$venv_dir/Scripts/python.exe"
fi

"$venv_python" -m pip install --upgrade pip
"$venv_python" -m pip install maturin pytest

# `maturin develop` looks for an *activated* virtualenv (`VIRTUAL_ENV`) or
# `.venv`, not just a python interpreter path — it never sees one since this
# script calls the venv's `python` directly rather than sourcing its
# `activate` script.
export VIRTUAL_ENV="$venv_dir"
(cd quadraui-py && "$venv_python" -m maturin develop --release)

exec "$venv_python" -m pytest quadraui-py/tests "$@"
