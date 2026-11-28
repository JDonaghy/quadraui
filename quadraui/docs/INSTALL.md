# Installing quadraui

Covers what to put in `Cargo.toml`, which feature turns on which backend, the system packages
each backend needs, and which platform combinations CI actually verifies.

## Add the dependency

quadraui is on crates.io. No feature is on by default, so you must pick at least one
backend. A bare `quadraui = "0.1"` compiles the widget descriptors and layout code but
has no backend to run them on.

```toml
[dependencies]
quadraui = { version = "0.11", features = ["tui"] }
```

A typical app ships the terminal backend plus the native one for each desktop it targets.
Cargo features are additive, so turn on every backend you might use and choose at runtime or
per-target:

```toml
[dependencies]
quadraui = { version = "0.11", features = ["tui"] }

[target.'cfg(target_os = "linux")'.dependencies]
quadraui = { version = "0.11", features = ["tui", "gtk"] }

[target.'cfg(target_os = "macos")'.dependencies]
quadraui = { version = "0.11", features = ["tui", "macos"] }

[target.'cfg(target_os = "windows")'.dependencies]
quadraui = { version = "0.11", features = ["tui", "win"] }
```

To follow unreleased work, pin a git revision rather than tracking `develop`:
`quadraui = { git = "https://github.com/JDonaghy/quadraui", rev = "<commit-sha>", features = [...] }`.
That is what the in-house consumers (vimcode, coord-tui) do.

**Rust version.** The crate declares `rust-version = "1.92.0"` — the real floor, held up
by the `gtk` feature's glib-rs 0.22 / gtk4 0.11 dependency family. That is older than
`rust-toolchain.toml`'s pin (which this repo's own CI and devs build with, so a new stable
release's clippy lints can't turn CI red with zero code changes); the two are deliberately
not kept in lockstep, since they answer different questions. `ci.yml`'s `msrv` job builds
every backend feature at exactly 1.92.0 on every PR, so this floor is tested, not just
declared.

## Features

| Feature | Gives you | Builds on |
|---|---|---|
| `tui` | `quadraui::tui`: the terminal backend (ratatui) | Linux, macOS, Windows |
| `gtk` | `quadraui::gtk`: GTK4 native backend (Cairo + Pango) | Linux. Also builds on macOS against Homebrew GTK |
| `macos` | `quadraui::macos`: AppKit native backend (Core Graphics + Core Text) | macOS only. A no-op on other targets |
| `win` | `quadraui::win`: Win32 native backend (Direct2D + DirectWrite) | Windows (MSVC). Type-checks elsewhere, runs only on Windows |
| `terminal` | `quadraui::terminal_engine`: PTY + vt100 + scrollback, for embedding a terminal in your app | Any. No renderer of its own; pairs with any backend |

Platform services (clipboard, trash, secret store) come with each backend feature; you do
not enable them separately.

## System packages

### Linux

- **`tui`**: nothing. Every dependency is pure Rust, including the clipboard, the
  Secret Service keyring client (`zbus`, no `libdbus`) and trash. CI proves that by building a
  fully static musl binary (`ci.yml`'s `musl-static` job).
- **`gtk`**: GTK **4.10 or newer**, Pango (including `pangoft2`), Cairo and Fontconfig
  development packages, found through `pkg-config`.

  ```sh
  # Debian / Ubuntu (CI installs the first three -dev packages; the rest come with them)
  sudo apt-get install -y pkg-config libgtk-4-dev libpango1.0-dev libcairo2-dev libfontconfig1-dev

  # Fedora
  sudo dnf install -y pkgconf-pkg-config gtk4-devel pango-devel cairo-devel fontconfig-devel

  # Arch
  sudo pacman -S --needed pkgconf gtk4 pango cairo fontconfig
  ```

  Only the Debian/Ubuntu line is exercised by CI; the others are the equivalent packages.

### Linux: the Python wheel (`pip install quadraui`)

The `quadraui` package on PyPI needs no Rust toolchain and no `-dev` packages. On Linux,
pip picks between two wheels by the host's glibc:

| Wheel | Hosts | `run_tui()` | `run_native()` |
|---|---|---|---|
| `manylinux_2_39` (x86_64 only) | glibc 2.39+: Ubuntu 24.04+, Fedora 40+, Debian 13+, Arch | yes | GTK window when the system GTK 4 (4.10+) runtime is installed |
| `manylinux_2_17` (x86_64 only) | older glibc: Ubuntu 22.04, Debian 12 | yes | raises `UnsupportedError` (these distributions ship GTK older than 4.10) |

The `manylinux_2_39` wheel carries two builds of the extension and links the **system** GTK
libraries rather than bundling them, so GTK is a runtime package you install, not a build
dependency:

```sh
sudo apt install libgtk-4-1    # Ubuntu / Debian
sudo dnf install gtk4          # Fedora
sudo pacman -S gtk4            # Arch
```

Without GTK, `import quadraui` and `run_tui()` still work, and `run_native()` raises
`UnsupportedError` naming `libgtk-4.so.1` and the command above, so an app can catch it and
fall back to `run_tui()`. `QUADRAUI_DISABLE_GTK=1` skips loading GTK in a process that never
opens a window. Both wheels are x86_64 only; on aarch64 pip builds the sdist, which needs a
Rust toolchain and is TUI-only by default. On aarch64, an older glibc or a source install,
build the GTK backend
yourself: with the GTK 4 `-dev`/`-devel` package and a Rust toolchain installed, run
`MATURIN_PEP517_ARGS="--features gtk" pip install --no-binary quadraui quadraui`.

On every PR that touches `quadraui-py`, the wheels workflow installs from a directory
holding both wheels into clean containers: `ubuntu:24.04`, `fedora:40` and `fedora:42`
with GTK (a window must paint), `ubuntu:24.04` and `fedora:42` without GTK (`run_native()`
must raise naming `libgtk-4.so.1`), and `debian:12` (pip must pick the TUI-only wheel). The
reasoning is in
[`quadraui-py/docs/LINUX_NATIVE_WHEELS.md`](https://github.com/JDonaghy/quadraui/blob/develop/quadraui-py/docs/LINUX_NATIVE_WHEELS.md).

### macOS

- **`macos`**, **`tui`**: the Xcode Command Line Tools (`xcode-select --install`). No other
  packages.
- **`gtk` on macOS** (optional, mostly for cross-backend testing): `brew install gtk4 pkg-config`.

A GUI app launched from a terminal works as is. To ship it, you need an `.app` bundle,
signing and notarisation, which quadraui does not do for you yet (quadraui#1122).

### Windows

- **`tui`**, **`win`**: the MSVC toolchain (`x86_64-pc-windows-msvc`, from Visual Studio
  Build Tools with the "Desktop development with C++" workload). The `*-windows-gnu` targets
  are not tested.
- **`win` requires an application manifest in *your* binary.** The backend's message dialogs
  use `TaskDialogIndirect`, which exists only in Common Controls v6. quadraui's own
  `build.rs` embeds the manifest for its examples and tests, but build-script link arguments
  do **not** reach crates that depend on quadraui. Without the manifest your executable is
  killed by the loader **before `main` runs, with no output at all**. Add one of:

  - the [`embed-manifest`](https://crates.io/crates/embed-manifest) crate in your own
    `build.rs`, declaring a dependency on `Microsoft.Windows.Common-Controls` 6.0;
  - a `.rc` resource carrying the same manifest;
  - or the two linker flags quadraui's `build.rs` uses, in your own `build.rs`:

    ```rust
    // build.rs
    fn main() {
        let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
            && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
        if windows_msvc {
            println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
        }
    }
    ```

  This is a general Win32 requirement for visual styles and task dialogs, not something
  specific to quadraui.

## What CI verifies

These are the combinations a pull request has to keep green. Anything not listed may well
work but is not checked.

| Platform (runner) | Features | What runs |
|---|---|---|
| Linux (`ubuntu-latest`) | `tui` | build, tests, clippy, benches (smoke) |
| Linux (`ubuntu-latest`) | `gtk` + `tui` | build, tests, clippy |
| Linux (`ubuntu-latest`) | `tui` on `x86_64-unknown-linux-musl` | fully static build of an example |
| Linux (`ubuntu-latest`) | `win`, `macos` | type-check only (no real windowing) |
| Windows (`windows-latest`) | `tui`, `win` | build, tests, clippy, cross-backend conformance matrix |
| macOS (`macos-latest`) | `macos` | build, tests, clippy, conformance matrix. Runs on PRs that touch `quadraui/src`, and weekly |
| macOS (`macos-latest`) | `gtk`, `macos,tui,gtk` | tests against Homebrew GTK, cross-backend parity |

## Next

- [`GUIDE.md`](GUIDE.md): your first app, from `cargo new` to something you can click.
- [`COMPOSE.md`](COMPOSE.md): the controllers a larger app is assembled from.
- [`PRIMITIVE_RULES.md`](PRIMITIVE_RULES.md): writing a widget, and how an app paints, routes and tests one.
- [`decisions/DECISIONS.md`](decisions/DECISIONS.md) D-019: what may change between releases
  before 1.0.
