#!/usr/bin/env bash
# Builds the quadraui-ffi crate, compiles smoke.c against its generated C
# header and static library, and runs the resulting binary — the one check
# that proves the ABI actually links and runs from a real C translation
# unit, not just from Rust-side tests calling the `extern "C"` functions
# directly in-process.
#
# Set SMOKE_VALGRIND=1 to run the binary under valgrind's leak checker
# instead, which is what turns smoke.c's `qui_free` discipline into an
# actual proof that no callee-allocated string leaks.
#
# Linux-only: it links the `.a` produced by the `debug` profile directly and
# assumes glibc's `-lpthread -ldl -lm`. Not run by CI (nothing here builds or
# executes it automatically); a cross-platform version is follow-on work.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cargo build -p quadraui-ffi --manifest-path "$CRATE_DIR/Cargo.toml"

# `-I`: this crate directory is not untrusted input, but `-I` keeps the
# one-off `python3 -c` from picking up a same-named module (e.g. a stray
# `json.py`) from the current working directory instead of the stdlib.
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 --manifest-path "$CRATE_DIR/Cargo.toml" \
    | python3 -I -c "import json,sys; print(json.load(sys.stdin)['target_directory'])")"

OUT_BIN="$SCRIPT_DIR/smoke_bin"
CC="${CC:-cc}"

"$CC" -std=c11 -Wall -Wextra -Wpedantic -Werror -g \
    -I "$CRATE_DIR/include" \
    "$SCRIPT_DIR/smoke.c" \
    "$TARGET_DIR/debug/libquadraui_ffi.a" \
    -lpthread -ldl -lm \
    -o "$OUT_BIN"

if [[ "${SMOKE_VALGRIND:-0}" == "1" ]]; then
    exec valgrind --error-exitcode=1 --leak-check=full --errors-for-leak-kinds=definite "$OUT_BIN"
fi

"$OUT_BIN"
