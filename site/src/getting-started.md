# Getting Started

## Add the dependency

quadraui is `publish = false` today (version `0.0.1`), so it isn't on
crates.io yet — add it as a git or path dependency:

```toml
[dependencies]
quadraui = { git = "https://github.com/JDonaghy/quadraui", features = ["tui"] }
```

Pick the backend feature(s) your app needs:

| Feature | Backend |
|---|---|
| `tui` | Terminal, via ratatui |
| `gtk` | GTK4, via Cairo + Pango (needs `libgtk-4-dev` / `pkg-config` to build) |
| `macos` | Core Graphics + Core Text (only compiles on `target_os = "macos"`) |
| `win` | Direct2D + DirectWrite |

## Run the example

```sh
git clone https://github.com/JDonaghy/quadraui
cd quadraui
cargo run --example hello --features tui
```

Press any key to bump the counter; `q` to quit. Swap `--features tui` for
`--features gtk` (needs GTK4 installed) to run the exact same `Hello` app
as a native window instead.

## Explore the gallery

```sh
cargo run -p quadraui-gallery --features tui
```

Every primitive this site's [Gallery](gallery/index.md) documents lives in
`quadraui-gallery/src/demos/` — one `Demo` implementation per primitive,
with a Code tab showing its own source and a Data tab showing the plain
JSON it renders.

## Where to go next

- [Gallery](gallery/index.md) — every primitive, with its code sample and
  a backend-support grid.
- [docs.rs](https://docs.rs/quadraui) — API reference (once published;
  until then, `cargo doc -p quadraui --features tui,gtk --open` from a
  checkout).
- [crates.io](https://crates.io/crates/quadraui) — the published crate
  (once released).
- [GitHub repository](https://github.com/JDonaghy/quadraui) — source,
  issues, and `CLAUDE.md`/`CONTRIBUTING.md` for contributing.
