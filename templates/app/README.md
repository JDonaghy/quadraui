# {{project-name}}

A [quadraui](https://github.com/JDonaghy/quadraui) app, generated from its
`templates/app` starter template. The whole UI lives in `src/lib.rs` as one
[`quadraui::ShellApp`] implementation — no backend-specific code. `src/main.rs`
picks the backend to run it on from the Cargo feature you build with.

## Run it

Terminal (no system dependencies beyond a terminal):

```sh
cargo run --features tui
```

Native desktop window (Linux, needs GTK4 dev packages — `libgtk-4-dev` on
Debian/Ubuntu):

```sh
cargo run --features gtk
```

macOS native window (builds only on a macOS host/target):

```sh
cargo run --features macos
```

Windows native window (builds on any host; only runs on Windows):

```sh
cargo run --features win
```

Press any key to bump the counter in the status bar; `q` or Esc to quit.

## Test it

```sh
cargo test --features tui
```

`tests/tui_driver.rs` drives the real `event → handle → render` path against
quadraui's headless `TestBackend` — no terminal or display required.

## Next steps

- Add primitives (lists, buttons, text inputs, panels, …) to `App::render_content`
  in `src/lib.rs` — see quadraui's `quadraui/examples/` for one example per
  primitive, each paired with a TUI and GTK runner.
- Add sidebar panels or a bottom panel via `App::config()`'s `ShellConfig`.
- Read quadraui's `README.md` and `quadraui/docs/ARCHITECTURE.md` for the
  shape of the toolkit this app is built on.
