#!/usr/bin/env bash
# Staleness + acceptance check for the generated wire types and native-
# extension stub (`scripts/gen_wire_types.py`): fails if regenerating would
# change any committed output (`wire.pyi`, `wire.d.ts`, `_quadraui.pyi`), if
# the "good" fixture scripts (or every real Python example under
# `quadraui-py/examples/`) fail to type-check, or if the "bad"
# (misspelled-key) fixture scripts type-check clean when they should not.
# Installs `mypy`/`pyright`/`typescript` into throwaway
# locations under `$CHECK_WIRE_TYPES_TMP` (default: a fresh `mktemp -d`,
# removed on exit) rather than the repo or the user's own environment, so
# a local run never fights an existing venv/node_modules. All three tools
# are pinned to an exact version -- a new release redefining an error code
# or changing strict-mode behavior should not be able to redden this check
# on an unrelated PR; bump the pins deliberately instead.
#
# Needs a Rust toolchain on `PATH`: `gen_wire_types.py --check` shells out
# to `cargo run -p quadraui-binding-core --bin export_wire_schema` to read
# the wire schema straight off `quadraui-binding-core/src/wire_schema.rs`
# (the Rust-side source of truth) rather than a Python transcription of it.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

echo "== regenerating wire types (--check) =="
python3 scripts/gen_wire_types.py --check

MYPY_VERSION="1.13.0"
PYRIGHT_VERSION="1.1.389"
TYPESCRIPT_VERSION="5.6.3"

if [ -z "${CHECK_WIRE_TYPES_TMP:-}" ]; then
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT
else
  tmp_dir="$CHECK_WIRE_TYPES_TMP"
fi
mkdir -p "$tmp_dir"

echo "== mypy --strict (Python) =="
venv="$tmp_dir/venv"
if [ ! -x "$venv/bin/mypy" ]; then
  python3 -m venv "$venv"
  "$venv/bin/pip" install --quiet \
    "mypy==$MYPY_VERSION" "pyright==$PYRIGHT_VERSION" "typing_extensions>=4.0"
fi

echo "-- good fixture must type-check clean --"
MYPYPATH="quadraui-py/python" "$venv/bin/mypy" --strict --ignore-missing-imports \
  scripts/fixtures/wire_types_sample.py

echo "-- every quadraui-py/examples/ example must type-check clean --"
MYPYPATH="quadraui-py/python" "$venv/bin/mypy" --strict --ignore-missing-imports \
  quadraui-py/examples

echo "-- bad (misspelled-key) fixture must fail to type-check --"
if MYPYPATH="quadraui-py/python" "$venv/bin/mypy" --strict --ignore-missing-imports \
  scripts/fixtures/wire_types_sample_bad.py; then
  echo "scripts/fixtures/wire_types_sample_bad.py type-checked clean -- expected it to" \
    "fail (the generated stub should reject its misspelled keys)" >&2
  exit 1
fi

# `pyright` disagrees with `mypy` in places this design rests on (notably
# `in`-narrowing on a `NotRequired` `TypedDict` key) -- a cheap second
# opinion over the same two fixtures, not a replacement for the `mypy`
# gate above. `PYTHONPATH` (rather than `MYPYPATH`, mypy's own env var)
# is how `pyright` is told where to resolve `quadraui.wire` from.
echo "== pyright (Python) =="
echo "-- good fixture must type-check clean --"
PYTHONPATH="quadraui-py/python" "$venv/bin/pyright" scripts/fixtures/wire_types_sample.py

echo "-- every quadraui-py/examples/ example must type-check clean --"
PYTHONPATH="quadraui-py/python" "$venv/bin/pyright" quadraui-py/examples

echo "-- bad (misspelled-key) fixture must fail to type-check --"
if PYTHONPATH="quadraui-py/python" "$venv/bin/pyright" scripts/fixtures/wire_types_sample_bad.py; then
  echo "scripts/fixtures/wire_types_sample_bad.py type-checked clean under pyright --" \
    "expected it to fail (the generated stub should reject its misspelled keys)" >&2
  exit 1
fi

echo "== tsc --strict (TypeScript) =="
node_modules="$tmp_dir/node_modules"
tsc="$tmp_dir/node_modules/.bin/tsc"
if [ ! -x "$tsc" ]; then
  npm install --silent --no-save --prefix "$tmp_dir" "typescript@$TYPESCRIPT_VERSION"
fi

echo "-- good fixture must type-check clean --"
"$tsc" --strict --noEmit --module commonjs --target es2020 scripts/fixtures/wire_types_sample.ts

echo "-- bad (misspelled-key) fixture must fail to type-check --"
if "$tsc" --strict --noEmit --module commonjs --target es2020 scripts/fixtures/wire_types_sample_bad.ts; then
  echo "scripts/fixtures/wire_types_sample_bad.ts type-checked clean -- expected it to" \
    "fail (the generated .d.ts should reject its misspelled keys)" >&2
  exit 1
fi

echo "all wire-types checks passed"
