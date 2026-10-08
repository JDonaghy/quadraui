# Installing quadraui

Covers what to put in `Cargo.toml`, which feature turns on which backend, the system packages
each backend needs, and which platform combinations CI actually verifies.

## Add the dependency

quadraui is on crates.io. No feature is on by default, so you must pick at least one
backend. A bare `quadraui = "0.1"` compiles the widget descriptors and layout code but
has no backend to run them on.

```toml
[dependencies]
quadraui = { version = "0.1", features = ["tui"] }
```

A typical app ships the terminal backend plus the native one for each desktop it targets.
Cargo features are additive, so turn on every backend you might use and choose at runtime or
per-target:

```toml
[dependencies]
quadraui = { version = "0.1", features = ["tui"] }

[target.'cfg(target_os = "linux")'.dependencies]
quadraui = { version = "0.1", features = ["tui", "gtk"] }

[target.'cfg(target_os = "macos")'.dependencies]
quadraui = { version = "0.1", features = ["tui", "macos"] }

[target.'cfg(target_os = "windows")'.dependencies]
quadraui = { version = "0.1", features = ["tui", "win"] }
```

To follow unreleased work, pin a git revision rather than tracking `develop`:
`quadraui = { git = "https://github.com/JDonaghy/quadraui", rev = "<commit-sha>", features = [...] }`.
That is what the in-house consumers (vimcode, coord-tui) do.

**Rust version.** The crate currently declares `rust-version = "1.97.1"`, the same toolchain
its CI pins. Lowering that to the real minimum is tracked in quadraui#1350.

## Features

| Feature | Gives you | Builds on |
|---|---|---|
| `tui` | `quadraui::tui`: the terminal backend (ratatui) | Linux, macOS, Windows |
| `gtk` | `quadraui::gtk`: GTK4 native backend (Cairo + Pango) | Linux. Also builds on macOS against Homebrew GTK |
| `macos` | `quadraui::macos`: AppKit native backend (Core Graphics + Core Text) | macOS only. A no-op on other targets |
| `win` | `quadraui::win`: Win32 native backend (Direct2D + DirectWrite) | Windows (MSVC). Type-checks elsewhere, runs only on Windows |
| `terminal` | `quadraui::terminal_engine`: PTY + vt100 + scrollback, for embedding a terminal in your app | Any. No renderer of its own; pairs with any backend |
| `layout` | `quadraui::flex`: flex/grid layout via Taffy | Any. Pure Rust |

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
- [`APP_ARCHITECTURE.md`](APP_ARCHITECTURE.md): how a larger app is put together.
- [`decisions/DECISIONS.md`](decisions/DECISIONS.md) D-019: what may change between releases
  before 1.0.
