#!/usr/bin/env bash
# Staleness + acceptance check for the generated wire types
# (`scripts/gen_wire_types.py`): fails if regenerating would change either
# committed output, if the "good" fixture scripts fail to type-check, or if
# the "bad" (misspelled-key) fixture scripts type-check clean when they
# should not. Installs `mypy`/`typescript` into throwaway locations under
# `$CHECK_WIRE_TYPES_TMP` (default: a fresh `mktemp -d`) rather than the
# repo or the user's own environment, so a local run never fights an
# existing venv/node_modules.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

echo "== regenerating wire types (--check) =="
python3 scripts/gen_wire_types.py --check

tmp_dir="${CHECK_WIRE_TYPES_TMP:-$(mktemp -d)}"
mkdir -p "$tmp_dir"

echo "== mypy --strict (Python) =="
venv="$tmp_dir/venv"
if [ ! -x "$venv/bin/mypy" ]; then
  python3 -m venv "$venv"
  "$venv/bin/pip" install --quiet "mypy>=1.8" "typing_extensions>=4.0"
fi

echo "-- good fixture must type-check clean --"
MYPYPATH="quadraui-py/python" "$venv/bin/mypy" --strict --ignore-missing-imports \
  scripts/wire_types_sample.py

echo "-- bad (misspelled-key) fixture must fail to type-check --"
if MYPYPATH="quadraui-py/python" "$venv/bin/mypy" --strict --ignore-missing-imports \
  scripts/wire_types_sample_bad.py; then
  echo "scripts/wire_types_sample_bad.py type-checked clean -- expected it to" \
    "fail (the generated stub should reject its misspelled keys)" >&2
  exit 1
fi

echo "== tsc --strict (TypeScript) =="
node_modules="$tmp_dir/node_modules"
tsc="$tmp_dir/node_modules/.bin/tsc"
if [ ! -x "$tsc" ]; then
  npm install --silent --no-save --prefix "$tmp_dir" typescript
fi

echo "-- good fixture must type-check clean --"
"$tsc" --strict --noEmit --module commonjs --target es2020 scripts/wire_types_sample.ts

echo "-- bad (misspelled-key) fixture must fail to type-check --"
if "$tsc" --strict --noEmit --module commonjs --target es2020 scripts/wire_types_sample_bad.ts; then
  echo "scripts/wire_types_sample_bad.ts type-checked clean -- expected it to" \
    "fail (the generated .d.ts should reject its misspelled keys)" >&2
  exit 1
fi

echo "all wire-types checks passed"
