# Theming

All colours come from one **`Theme`**: named tokens such as `background`,
`foreground`, `selected_bg`, `border_fg` and `tab_active_bg`. Widgets paint
with tokens, never with literal colours, so changing the theme recolours
every widget on every backend at once.

- **Built in:** `Theme::default()` (dark) and `Theme::light_default()`.
- **From VS Code:** `Theme::from_vscode_json(path)` loads a VS Code colour
  theme file.
- **Per backend:** each native backend can report the system's light or
  dark preference.

## Setting it

Rust apps hand the backend a theme before painting. The gallery does it in
its `ShellApp::pre_render` hook (`quadraui-gallery/src/app.rs`):

<!-- sample: quadraui-gallery/src/app.rs -->
```rust,noplayground
    fn pre_render(&self, backend: &mut dyn Backend) {
        backend.set_theme(self.theme);
    }
```

Python and Node override tokens by name, with hex strings or RGBA values.
From `quadraui-py/tests/test_theme.py`, on the underlying `QuiApp` (an app
layer `App` exposes it as `app.qui`):

<!-- sample: quadraui-py/tests/test_theme.py -->
```python
    app.set_theme({"background": "#112233"})
```

## Scoped styles

To recolour one part of the screen, give a container a `"style"` with token
overrides, or define a named style once with `define_style(name,
overrides)` and refer to it by name, like a CSS class.
`quadraui-py/examples/style_override_demo.py` and `style_classes_demo.py`
show both.

## If you know...

- **CSS:** tokens are custom properties (`--background`); a scoped style is
  a class that overrides some of them.
- **Textual:** like Textual's design variables (`$background`,
  `$primary`), set from code instead of a `.tcss` file.
- **iced:** like `Theme` plus a custom `Palette`, but with one token per
  widget part, so a VS Code theme maps onto it directly.
