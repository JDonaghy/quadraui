# Getting started

quadraui has one renderer, written in Rust, and three ways to drive it. Pick
the language you write apps in; each quick start takes about five minutes
and ends with the same counter app running in your terminal and then in a
native window.

| | Install | Quick start |
|---|---|---|
| **Rust** | `cargo add quadraui --features tui` | [Rust](quick-start/rust.md) |
| **Python** 3.8+ | `pip install quadraui` | [Python](quick-start/python.md) |
| **Node.js** 22+ / TypeScript | `npm install @quadraui/node` | [Node and TypeScript](quick-start/node.md) |

## Which backends you get

The terminal backend works everywhere. A native window needs that
platform's backend compiled in:

| | Terminal | macOS window | Windows window | Linux (GTK4) window |
|---|---|---|---|---|
| Rust | `tui` feature | `macos` feature | `win` feature | `gtk` feature (needs `libgtk-4-dev`) |
| Python wheel from PyPI | yes | yes | yes | build from source |
| Node package from npm | yes | build from source | build from source | build from source |

The Linux Python wheel and the npm packages leave the native backends out
because GTK cannot be bundled into a portable wheel and the prebuilt Node
addons are built headless. Each quick start shows the source build.

## Try it without writing code

The gallery shows every widget, on whichever backend you build it for:

```sh
git clone https://github.com/JDonaghy/quadraui
cd quadraui
cargo run -p quadraui-gallery --features tui      # or gtk, macos, win
```

Or browse the generated [Widget gallery](gallery/index.md) on this site.
