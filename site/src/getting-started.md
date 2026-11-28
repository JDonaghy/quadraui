# Getting started

quadraui has one renderer, written in Rust, and three ways to drive it. Pick
the language you write apps in; each quick start takes about five minutes
and ends with the same counter app running in your terminal, and in a
native window wherever the table below has a native backend for you.

| | Install | Quick start |
|---|---|---|
| **Rust** | `cargo add quadraui --features tui` (0.12; until it ships, from git) | [Rust](quick-start/rust.md) |
| **Python** 3.8+ | `pip install quadraui` | [Python](quick-start/python.md) |
| **Node.js** 22+ / TypeScript | from a clone until 0.12; then `npm install @quadraui/node` | [Node and TypeScript](quick-start/node.md) |

## Which backends you get

The terminal backend works everywhere. A native window needs that
platform's backend compiled in:

| | Terminal | macOS window | Windows window | Linux (GTK4) window |
|---|---|---|---|---|
| Rust | `tui` feature | `macos` feature | `win` feature | `gtk` feature (needs `libgtk-4-dev`) |
| Python wheel from PyPI | yes | yes | yes | yes on x86_64 and aarch64 with glibc 2.39+ and GTK 4.10+; build from source elsewhere |
| Node package from npm | yes | build from source | build from source | build from source |

The Linux Python wheels (0.12 and later) carry a GTK backend that uses the
system's GTK 4, so `run_native()` opens a window on Ubuntu 24.04+,
Fedora 40+ and Debian 13+ once GTK 4.10 or newer is installed. On an older
glibc (Ubuntu 22.04, Debian 12) pip picks a terminal-only wheel, and
`run_native()` raises `UnsupportedError` saying why.

The prebuilt Node addons that `npm install @quadraui/node` downloads are
terminal-only on every platform: `runNative()` raises
`NoNativeBackendError`. A native Node window needs the addon built from
source with `--features macos`, `--features gtk` or `--features win`.
Each quick start shows the source build.

## Try it without writing code

The gallery shows every widget, on whichever backend you build it for:

```sh
git clone https://github.com/JDonaghy/quadraui
cd quadraui
cargo run -p quadraui-gallery --features tui      # or gtk, macos, win
```

Or browse the generated [Widget gallery](gallery/index.md) on this site.
