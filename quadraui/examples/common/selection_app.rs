//! Mouse-drag text-selection + Ctrl-C copy demo for `run_with_shell`.
//!
//! [`SelectionDemo`] implements [`ShellApp`] and proves that the selection
//! pipeline — drag to select, Ctrl-A select-all, Ctrl-C copy — works
//! identically when an app is driven by [`quadraui::tui::shell_runner::run_with_shell`]
//! (the path used by coord-tui) as it does with the plain `tui::run` path.
//!
//! The pipeline lives in `quadraui::tui::run::dispatch_event`; both runners
//! converge on it. This demo is the acceptance smoke-test for issue #283.
//!
//! ## Controls
//!
//! | Input                     | Action                                  |
//! |---------------------------|-----------------------------------------|
//! | Click-drag content lines  | Start / extend text selection           |
//! | Ctrl-A                    | Select all lines in the content area    |
//! | Ctrl-C (with selection)   | Copy selection; shows a preview         |
//! | q / Esc                   | Quit                                    |
//!
//! ## #454 composability: mouse-reporting terminals
//!
//! When an embedded terminal has mouse reporting enabled (vim, tmux, …)
//! the app should call `backend.cancel_text_selection_drag()` immediately
//! after `TerminalSession::forward_mouse` returns `true`. This cancels the
//! speculative `DragTarget::TextSelection` drag that `apply_dispatch`
//! starts *before* `handle()` is called, preventing spurious
//! `TextSelectionChanged` events on subsequent mouse moves. See the
//! [`Backend::cancel_text_selection_drag`] doc for the full code pattern.
//!
//! This demo does not embed a PTY, so the call is not needed here. The
//! canonical example is `tui_terminal.rs` + `common/terminal_app.rs`.

use quadraui::compose::app_shell::AppShellLayout;
use quadraui::{
    Backend, Color, Key, NamedKey, Reaction, Rect, ShellApp, ShellConfig, ShellContext, StatusBar,
    StatusBarSegment, TextRegion, UiEvent, WidgetId,
};

// ── Content ───────────────────────────────────────────────────────────────────

/// Sample lines displayed in the selectable content area.
pub const CONTENT_LINES: &[&str] = &[
    "The quick brown fox jumps over the lazy dog.",
    "Pack my box with five dozen liquor jugs.",
    "How vexingly quick daft zebras jump!",
    "The five boxing wizards jump quickly.",
    "Sphinx of black quartz, judge my vow.",
    "Mr. Jock, TV quiz PhD, bags few lynx.",
    "Waltz, bad nymph, for quick jigs vex.",
];

/// Widget id for the selectable text region (stable across frames).
pub const CONTENT_ID: &str = "selection-demo-content";

// ── App ───────────────────────────────────────────────────────────────────────

/// Demo app: selectable text panel driven by `run_with_shell`.
pub struct SelectionDemo {
    /// Message shown in the bottom status bar.
    status: String,
}

impl SelectionDemo {
    pub fn new() -> Self {
        Self { status: hint() }
    }

    /// Build the [`ShellConfig`] (no sidebar panels — main-area only).
    pub fn config() -> ShellConfig {
        ShellConfig::new("Selection Demo", vec![])
    }

    // ── Render helpers ────────────────────────────────────────────────────────

    /// Paint `CONTENT_LINES` row by row into `bounds`. Returns the actual
    /// rendered bounds (may be shorter than `bounds` when fewer lines fit).
    fn fill_content(&self, backend: &mut dyn Backend, bounds: Rect) -> Rect {
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return Rect::new(bounds.x, bounds.y, bounds.width, 0.0);
        }
        let lh = backend.line_height();
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(20, 20, 35);
        let mut rendered_rows: usize = 0;
        for (i, &line) in CONTENT_LINES.iter().enumerate() {
            let row_y = bounds.y + i as f32 * lh;
            if row_y + lh > bounds.y + bounds.height {
                break;
            }
            let row_rect = Rect::new(bounds.x, row_y, bounds.width, lh);
            let bar = StatusBar {
                id: WidgetId::new(format!("{CONTENT_ID}-row-{i}")),
                left_segments: vec![StatusBarSegment {
                    text: format!(" {line}"),
                    fg,
                    bg,
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            backend.draw_status_bar(row_rect, &bar, None, None);
            rendered_rows = i + 1;
        }
        Rect::new(bounds.x, bounds.y, bounds.width, rendered_rows as f32 * lh)
    }
}

impl Default for SelectionDemo {
    fn default() -> Self {
        Self::new()
    }
}

// ── ShellApp impl ─────────────────────────────────────────────────────────────

impl ShellApp for SelectionDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let lh = backend.line_height();
        let main = layout.main_content_bounds;

        // Reserve one row at the bottom for the status bar.
        let content_area = Rect::new(main.x, main.y, main.width, (main.height - lh).max(0.0));
        let rendered = self.fill_content(backend, content_area);

        // Register the selectable text region. Pixel-based backends (GTK,
        // macOS) use `lines` for text extraction; TUI reads ratatui cells.
        backend.register_text_region(TextRegion {
            id: WidgetId::new(CONTENT_ID),
            bounds: rendered,
            lines: CONTENT_LINES.iter().map(|s| s.to_string()).collect(),
        });

        // Status bar at the bottom of the main content area.
        let status_rect = Rect::new(main.x, main.y + main.height - lh, main.width, lh);
        let status_bar = StatusBar {
            id: WidgetId::new("selection-demo-status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar(status_rect, &status_bar, None, None);
    }

    fn handle(
        &mut self,
        event: UiEvent,
        backend: &mut dyn Backend,
        _ctx: &ShellContext,
    ) -> Reaction {
        match event {
            // ── Quit ─────────────────────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,

            // ── Copy confirmation ─────────────────────────────────────────────
            UiEvent::TextCopied(text) => {
                let preview: String = text.chars().take(40).collect();
                let ellipsis = if text.chars().count() > 40 { "…" } else { "" };
                self.status =
                    format!("Copied: \"{preview}{ellipsis}\" — drag or Ctrl-A to select again");
                Reaction::Redraw
            }

            // ── Selection-in-progress feedback ────────────────────────────────
            UiEvent::TextSelectionChanged { anchor, focus, .. } => {
                let lh = backend.line_height();
                let a_row = (anchor.y / lh).floor() as usize + 1;
                let f_row = (focus.y / lh).floor() as usize + 1;
                let (start, end) = if a_row <= f_row {
                    (a_row, f_row)
                } else {
                    (f_row, a_row)
                };
                self.status = if start == end {
                    format!("Selecting row {start} — Ctrl-C to copy")
                } else {
                    format!("Selecting rows {start}–{end} — Ctrl-C to copy")
                };
                Reaction::Redraw
            }

            // ── Click resets hint ─────────────────────────────────────────────
            UiEvent::MouseDown { .. } => {
                self.status = hint();
                Reaction::Redraw
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

fn hint() -> String {
    "drag to select · Ctrl-A select all · Ctrl-C copy · q quit".into()
}
