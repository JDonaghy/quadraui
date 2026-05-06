# Architecture

**Workspace members:**

| Crate | Purpose |
|---|---|
| `quadraui/` | Core library: primitives, types, theme, backend traits, TUI + GTK rasterisers. |
| `kubeui-core/` | Domain logic for a Kubernetes dashboard demo. No rendering deps — testable in isolation. |
| `kubeui/` | TUI Kubernetes dashboard. Real consumer of every TUI rasteriser. |
| `kubeui-gtk/` | GTK Kubernetes dashboard. Same domain logic as `kubeui`. |

**Two-layer split** inside `quadraui/`:

- `quadraui/src/primitives/` — backend-agnostic widget descriptions +
  layout functions + hit_test. **Must NOT depend on `ratatui`, `gtk4`,
  `cairo`, or any backend crate.** Tests live as inline `#[cfg(test)]
  mod tests` blocks at the bottom of each primitive file.
- `quadraui/src/tui/` and `quadraui/src/gtk/` — backend rasterisers.
  Each consumes the primitive's `layout()` and paints verbatim. **Must
  NOT contain layout decisions** that the primitive could express.

**Compose helpers** in `quadraui/src/compose/` sit above primitives
and below apps. They own interaction state machines for common
multi-primitive compositions so consumers don't reimplement them:

- `FocusRing` — Tab/Shift+Tab cycling through widget IDs.
- `MenuSystem` — MenuBar + ContextMenu dropdown interaction.
- `SidebarSystem` — MSV + TreeView sidebar panel interaction.

**GTK hosting helpers** in `quadraui/src/gtk/`:

- `MenuOverlay` — encapsulates the DrawingArea boilerplate for
  MenuSystem dropdown popups when the menu bar lives in a separate
  titlebar widget. Handles the negative-y coordinate transform,
  `set_focusable(false)`, `can_target` toggling, surface clearing,
  and draw/click/motion wiring. Apps that use the single-DA runner
  don't need this — the dropdown paints on the same surface.

**Backend trait** in `quadraui/src/backend.rs` plumbs frame state and
the `set_current_theme` / `set_nerd_fonts` setters that hosts call once
per frame. Per-primitive `draw_*` functions are free functions in the
backend modules; the trait is for cross-cutting state.

**`set_cell` / `cairo_rgb` / `ratatui_color` / etc.** are private
backend helpers — never call them from primitives.
