//! Minimal `AppLogic` painting a `TabBar` with a double-width (CJK) tab
//! label, routed entirely through the widget contract:
//! [`quadraui::Surface::Widget`] paints it through `draw_dyn`, and
//! `AppLogic::frame_tree` hands the same widget to the shared routing
//! engine, which delivers clicks and arrow keys to `TabBar`'s own
//! `Interactive` impl and reports the outcome back as
//! [`quadraui::UiEvent::TabBar`].
//!
//! Deliberately independent of two other things that look similar:
//!
//! - [`super::tab_group_demo::TabGroupDemo`] — its `TabGroup` tab
//!   labels are hardcoded ASCII and not settable from outside, so it can't
//!   host a wide-char label at all.
//! - `tests/acceptance/ms-11/wide_tab_labels.rs`'s sealed `WideTabFixture`
//!   — a separate conformance-suite-owned construction that exercises
//!   a `TabBar` whose label contains double-width glyphs via `quadraui`'s
//!   public API.

use std::cell::Cell;

use quadraui::{
    AppLogic, Backend, Color, FrameTree, Reaction, Rect, ScreenLayout, StatusBar, StatusBarSegment,
    Surface, TabBar, TabBarEvent, TabItem, UiEvent, WidgetId,
};

/// Tab 0's label — contains double-width CJK characters mid-string.
pub const WIDE_LABEL: &str = " 1: 日本語.rs ";
/// Tab 1's label — an ASCII control, unaffected by wide-char handling.
pub const ASCII_LABEL: &str = " 2: main.rs ";

pub struct WideTabBarDemo {
    /// Index of the tab marked active in the descriptor this demo
    /// rebuilds each frame. Advanced by whichever `TabBarEvent` the
    /// routing engine reports — a click on a tab body, or Enter/Space on
    /// a keyboard-focused one.
    active: usize,
    /// Strip rect the last `render` painted into. `frame_tree` gets no
    /// `&dyn Backend` of its own (see `AppLogic::frame_tree`'s doc), so
    /// it reads the rect back from here rather than re-deriving a
    /// viewport-dependent one that could drift from what was painted —
    /// the same render-then-read-back pattern `frame_demo`'s cached hit
    /// map uses.
    strip: Cell<Rect>,
}

impl WideTabBarDemo {
    pub fn new() -> Self {
        Self {
            active: 0,
            strip: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
        }
    }

    /// The bar's own widget id — also the key the runtime's widget-state
    /// store persists `TabBarState` under across frames.
    fn id() -> WidgetId {
        WidgetId::new("wide-tab-bar-demo:tabs")
    }

    /// Readout of which tab the routing engine last activated, so the
    /// outcome of a click or an Enter press is observable as text rather
    /// than only as the active tab's own styling. Reports the index, not
    /// the label: this demo's whole point is that a double-width label
    /// paints intact in the bar, and a second copy of that same label
    /// elsewhere on screen makes "which run is the tab's?" ambiguous for
    /// any observer reading the painted text back.
    fn status(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("wide-tab-bar-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" active: tab {} ", self.active),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " ←/→ + Enter · q quit ".to_string(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    /// This frame's `TabBar` descriptor, rebuilt from `self.active`. The
    /// keyboard-focus index lives in the runtime's own `TabBarState`,
    /// not here.
    fn bar(&self) -> TabBar {
        let labels = [WIDE_LABEL, ASCII_LABEL];
        TabBar {
            id: Self::id(),
            tabs: labels
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == self.active,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: true,
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: Some(Color::rgb(80, 160, 240)),
            show_tab_close: true,
            compact: false,
            ..Default::default()
        }
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
        let strip = Rect::new(0.0, 0.0, viewport.width, backend.chrome_line_height());
        self.strip.set(strip);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: strip,
            widget: Box::new(self.bar()),
        });
        frame.push(Surface::Widget {
            rect: Rect::new(
                0.0,
                viewport.height - strip.height,
                viewport.width,
                strip.height,
            ),
            widget: Box::new(self.status()),
        });
        let _ = frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(Self::id(), self.strip.get(), Box::new(self.bar()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::TabBar(_, TabBarEvent::TabActivated { index }) if index < 2 => {
                self.active = index;
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
