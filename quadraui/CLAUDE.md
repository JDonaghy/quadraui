# quadraui

Cross-platform UI primitives for keyboard-driven desktop and terminal apps. Four rendering backends share a single declarative API: GTK4 (Linux), TUI/ratatui (everywhere), macOS (Core Graphics), Windows (Direct2D).

## Build commands

```bash
# TUI only (fast, no system deps)
cargo build --features tui
cargo test --features tui

# GTK (requires GTK4 dev libraries)
cargo build --features tui --features gtk
cargo test --features tui --features gtk

# Run a specific example
cargo run --example tui_pipeline --features tui
cargo run --example gtk_pipeline --features gtk
```

## Project structure

```
src/
  primitives/     — Stateless descriptors (ListView, TreeView, Form, PipelineView, …)
  compose/        — Stateful controllers that wrap primitives (SidebarSystem, ChatController, …)
  tui/            — TUI rasterisers: one file per primitive (list.rs, pipeline_view.rs, …)
  gtk/            — GTK rasterisers: one file per primitive
  event.rs        — UiEvent enum: the single input model across all backends
  dispatch.rs     — Routes UiEvents to the right handler
  theme.rs        — Theme struct: all colour tokens
  lib.rs          — Re-exports everything public
examples/
  tui_<name>.rs   — One runnable TUI demo per primitive/feature
  gtk_<name>.rs   — One runnable GTK demo per primitive/feature
  common/         — Shared demo helpers (AppLogic impls, etc.)
```

## Demos are mandatory for visual features

**Any new primitive, new interaction, or visual behaviour change must ship with a runnable demo.**

- New primitive → new `examples/tui_<name>.rs` (and `gtk_<name>.rs` if GTK is in scope)
- New interaction on an existing primitive → extend the relevant existing example or add a new one
- The demo must exercise the changed code path visually — not just compile

Name demos after the feature, not the ticket: `tui_list_hscroll.rs`, not `tui_issue276.rs`.

Examples must be registered in `Cargo.toml` or picked up by the glob — check that `cargo run --example <name> --features tui` works before declaring done.

## Demo conventions

Look at `examples/tui_pipeline.rs` and `examples/gtk_panel.rs` as reference patterns. Every demo:

1. Implements `AppLogic` (or `GtkAppLogic`)
2. Has a `main()` that calls `quadraui::tui::run(app)` or the GTK equivalent
3. Shows the feature in an immediately obvious state on launch — no hidden setup steps
4. Prints brief key hints in the status bar or terminal so the reviewer knows what to try

## Primitive layer rules

- Primitives in `src/primitives/` are **stateless descriptors** — no `mut`, no side effects
- Rasterisers in `src/tui/` and `src/gtk/` take `&Primitive` + `&mut Buffer/Context` and paint, nothing else
- State machines live in `src/compose/` controllers or in the caller's `AppLogic`

## Event model

All input arrives as `UiEvent` (see `src/event.rs`). Key variants:

| Event | Meaning |
|---|---|
| `MouseDown / MouseUp / MouseMove` | Pointer with position |
| `KeyDown { key, modifiers }` | Keyboard |
| `ClipboardPaste(String)` | Bracketed paste / text pasted into an input |
| `TextCopied(String)` | Broadcast after text is copied to clipboard (not paste) |
| `Resize { width, height }` | Terminal or window resize |

`ClipboardPaste` is for inserting text into inputs. `TextCopied` is the confirmation that a copy operation succeeded — do not conflate them.

## Theme

All colours come from `&Theme`. Never hardcode RGB values in rasterisers. Use:

```rust
let fg = ratatui_color(theme.foreground);
let muted = ratatui_color(theme.muted_fg);
let accent = ratatui_color(theme.accent_bg);
```

## Adding a new primitive

1. `src/primitives/<name>.rs` — descriptor struct + layout types
2. `src/tui/<name>.rs` — `draw_<name>(buf, area, &primitive, theme)`
3. `src/gtk/<name>.rs` — GTK equivalent (if in scope)
4. Re-export from `src/lib.rs`
5. `examples/tui_<name>.rs` + `examples/gtk_<name>.rs` — runnable demos
6. Unit tests inline in the rasteriser file

## Tests

Unit tests live in the same file as the code they test (inline `#[cfg(test)]`). For rasterisers, test that specific cells contain expected characters or colours given known input — see `src/tui/pipeline_view.rs` for the pattern.

Do not add integration tests that require a display or running process.

## Files workers must not touch

- `CHANGELOG.md` — coordinator updates this at session end
- `README.md` — coordinator updates this at session end
