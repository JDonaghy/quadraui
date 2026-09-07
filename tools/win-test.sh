#!/usr/bin/env bash
# Run quadraui's Windows (MSVC) test suite on the real Windows host, driven
# from a Linux/WSL shell. See CLAUDE.md, "Win-GUI: building and testing for
# real".
#
# WHY THIS SCRIPT EXISTS (#832 smoke): the invocation needs three environment
# variables, none of which produces an error that names its own cause when it
# is missing, and *all three* have now been forgotten at least once by someone
# copying the command out of a doc:
#
#   RUSTFLAGS=-C target-feature=+crt-static
#       The host has no vcruntime140.dll, so a dynamically-linked test .exe
#       exits 53 with completely empty output — indistinguishable from a
#       program that ran and printed nothing.
#
#   RUSTDOCFLAGS=-C target-feature=+crt-static
#       rustdoc compiles each doctest itself and does NOT inherit RUSTFLAGS.
#       Miss this one and all 20 integration-test targets pass while the
#       `Doc-tests quadraui` leg fails 9-of-11 with exit 53, blaming the
#       library docs rather than the missing flag.
#
#   CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env
#       cargo-xwin hard-codes a `wine` runner for `test`/`run` (there is no
#       flag to turn it off) and there is no wine on this host. Regular test
#       targets die with `could not execute process 'wine ...'`; doctests fail
#       differently and much more quietly — rustdoc reports
#       `Couldn't run the test: No such file or directory (os error 2)`,
#       again 9-of-11, again pointing at the docs. Overriding the runner lets
#       binfmt_misc (registered for the PE `MZ` magic) exec the .exe directly;
#       stdout and exit codes round-trip intact.
#
# A .cargo/config.toml cannot replace this script: cargo-xwin injects the wine
# runner into cargo's *environment*, and env beats config, so a `runner` key in
# config is silently ignored (verified on dell64, 2026-09-07).
#
# Usage:
#   tools/win-test.sh                      # whole win suite: lib + integration + doctests
#   tools/win-test.sh --doc                # doctests only
#   tools/win-test.sh --test win_backend   # one integration target
#   tools/win-test.sh -- --nocapture       # args for the test binary
#
# Any arguments are appended verbatim to the `cargo xwin test` line.
#
# Prerequisites: cargo-xwin, the x86_64-pc-windows-msvc rustup target, and a
# Windows host reachable through WSL interop (binfmt_misc). This is a
# Linux/WSL-host cross-compile helper by construction; on a native Windows host
# run `cargo test -p quadraui --features win` directly instead — none of the
# three variables above applies there.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Note: these must be *set*, not appended to whatever the caller exported.
# A RUSTFLAGS env var replaces `target.*.rustflags` from any cargo config
# rather than merging with it, so the flag has to be present in the value we
# hand to cargo, not merely somewhere in the config chain.
export RUSTFLAGS="-C target-feature=+crt-static"
export RUSTDOCFLAGS="-C target-feature=+crt-static"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=env

cd "$repo_root"
exec cargo xwin test --target x86_64-pc-windows-msvc -p quadraui --features win "$@"
