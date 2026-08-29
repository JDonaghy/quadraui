//! Minimal `AppLogic` painting a `TabBar` with a double-width (CJK) tab
//! label — the conformance-matrix fixture for quadraui#555's TUI vt100
//! observer.
//!
//! Deliberately independent of two other things that look similar:
//!
//! - [`super::tab_group_demo::TabGroupDemo`] — its `TabGroupController`
//!   labels are hardcoded ASCII and not settable from outside, so it can't
//!   host a wide-char label at all.
//! - `tests/acceptance/ms-11/wide_tab_labels.rs`'s sealed `WideTabFixture`
//!   — oracle-owned acceptance fixture for issue #554 itself; a worker may
//!   not import from or depend on a sealed acceptance slice. This fixture
//!   is a separate, conformance-suite-owned construction that happens to
//!   exercise the same class of content (a `TabBar` whose label contains
//!   double-width glyphs), via nothing but `quadraui`'s public API.

use quadraui::{AppLogic, Backend, Color, Reaction, Rect, TabBar, TabItem, UiEvent, WidgetId};

/// Tab 0's label — a CJK filename mid-string, matching quadraui#555's own
/// worked example verbatim.
pub const WIDE_LABEL: &str = " 1: 日本語.rs ";
/// Tab 1's label — an ASCII control, unaffected by wide-char handling.
pub const ASCII_LABEL: &str = " 2: main.rs ";

pub struct WideTabBarDemo;

impl WideTabBarDemo {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WideTabBarDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for WideTabBarDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        backend.draw_tab_bar(
            Rect::new(0.0, 0.0, viewport.width, 1.0),
            &TabBar {
                id: WidgetId::new("wide-tab-bar-demo:tabs"),
                tabs: vec![
                    TabItem {
                        label: WIDE_LABEL.to_string(),
                        is_active: true,
                        is_dirty: false,
                        is_preview: false,
                        is_closable: true,
                    },
                    TabItem {
                        label: ASCII_LABEL.to_string(),
                        is_active: false,
                        is_dirty: false,
                        is_preview: false,
                        is_closable: true,
                    },
                ],
                scroll_offset: 0,
                right_segments: vec![],
                active_accent: Some(Color::rgb(80, 160, 240)),
                show_tab_close: true,
                compact: false,
            },
            None,
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::KeyPressed {
            key: quadraui::Key::Char('q'),
            ..
        } = event
        {
            return Reaction::Exit;
        }
        Reaction::Continue
    }
}
