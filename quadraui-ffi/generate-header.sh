#!/usr/bin/env bash
# Regenerates the checked-in include/quadraui_ffi.h from this crate's
# extern "C" surface. Run this after any change to src/lib.rs's public FFI
# items and commit the result.
#
# build.rs itself only writes a header into OUT_DIR (it never mutates the
# source tree); this script is the one place that updates the committed
# copy, and the `header_up_to_date` test (run plain, with no env var) fails
# the build if the two ever drift.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

QUADRAUI_FFI_UPDATE_HEADER=1 cargo test -p quadraui-ffi \
    --manifest-path "$SCRIPT_DIR/Cargo.toml" \
    --test header_up_to_date -- --nocapture
