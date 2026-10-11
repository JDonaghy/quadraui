//! Backend-agnostic `AppLogic` for the `TextDisplay` word-wrap demo
//! ([`tui_text_display_wrap`] / [`gtk_text_display_wrap`]).
//!
//! A single full-viewport [`quadraui::TextDisplay`] with one line that is
//! deliberately much wider than the demo's viewport. The line wraps onto
//! continuation rows (prefixed with the "↳ " marker — see
//! [`quadraui::primitives::text_display`]'s `wrap` module), so the tail word
//! [`TAIL_MARKER`] is visible on a later row. This demo demonstrates that
//! `TextDisplay` word-wraps over-long lines rather than truncating them.
//!
//! Controls:
//! - `q` / `Esc` — quit

use quadraui::{
    AppLogic, Backend, Rect, StyledSpan, TextDisplay, TextDisplayLine, UiEvent, WidgetId,
};

/// Distinctive word placed at the very end of the long line. Never painted
/// anywhere on screen if `TextDisplay` truncates instead of wrapping —
/// that is exactly the regression the conformance scenario and the driver
/// test both assert against via `AssertScreenHas` / `screen_contains`.
pub const TAIL_MARKER: &str = "TAILMARKER";

/// Well past any reasonable terminal/window width, so the line reliably
/// needs several wrapped rows regardless of the exact column budget a
/// given backend computes.
fn long_line_text() -> String {
    let mut s = String::from(
        "This one line of prose is deliberately far wider than the viewport \
         so that it cannot possibly fit on a single row no matter how the \
         backend measures character width, which is exactly the scenario \
         quadraui#905 was filed against: a pane that silently truncated \
         long lines at its right edge instead of wrapping them onto \
         continuation rows the reader could actually scroll to and read, ",
    );
    s.push_str(TAIL_MARKER);
    s
}

pub struct TextDisplayWrapDemo {
    display: TextDisplay,
}

impl TextDisplayWrapDemo {
    pub fn new() -> Self {
        let long_line = TextDisplayLine {
            spans: vec![StyledSpan::plain(long_line_text())],
            decoration: Default::default(),
            timestamp: None,
        };
        // Deliberately short — well inside every backend's wrap budget for
        // this viewport, even GTK's coarser pixel-to-column approximation
        // (`Backend::char_width`), so it never wraps and always paints as
        // one run. That's what lets `assert_screen_has` match the whole
        // sentence verbatim in the conformance scenario and driver test.
        let short_line = TextDisplayLine {
            spans: vec![StyledSpan::plain("A short line follows.")],
            decoration: Default::default(),
            timestamp: None,
        };
        Self {
            display: TextDisplay {
                lines: vec![long_line, short_line],
                ..TextDisplay::new(WidgetId::new("text-display-wrap-demo"))
            },
        }
    }
}

impl Default for TextDisplayWrapDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for TextDisplayWrapDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Rect::new(0.0, 0.0, vp.width, vp.height);
        backend.draw_text_display(rect, &self.display);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> quadraui::Reaction {
        use quadraui::{Key, NamedKey, Reaction};
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            _ => Reaction::Continue,
        }
    }
}
