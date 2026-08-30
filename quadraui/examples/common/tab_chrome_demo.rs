//! Minimal `AppLogic` painting a `TabBar` whose active tab is enclosed in
//! bracket framing via [`TabChrome::active_frame`] (quadraui#631) —
//! `[main.rs ×]` rather than the close glyph floating unframed after the
//! label.
//!
//! The point of the demo is the **sidecar + declarative click routing**:
//! the bracket framing is requested via [`Backend::draw_tab_bar_with_chrome`]
//! (never baked into [`TabItem::label`] as a string, which is exactly the
//! workaround #631 exists to make unnecessary — see coord-tui's
//! `doc_tab_label`), and a click still resolves through
//! [`Backend::tab_bar_layout_with_chrome`]'s `close_bounds` /
//! `slot_positions`, never a hand-rolled character scan for `×`.
//!
//! Only the active tab is closable (`is_closable` tracks `is_active`), so
//! its `×` — and, since chrome frames it, its `]` — are the only
//! occurrence of either glyph on screen at any moment. That is what lets
//! the driver test click "×" by text alone and know unambiguously which
//! tab it landed on, and it is also why closing never needs to be undone
//! for the demo to stay exercisable: the *other* tab has no close button
//! to confuse the click routing with.

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, TabBar,
    TabChrome, TabFrame, TabItem, UiEvent, WidgetId,
};

/// Tab labels, index-stable across activation (only `is_active` /
/// `is_closable` move between them).
pub const LABELS: [&str; 2] = ["main.rs", "lib.rs"];

pub struct TabChromeDemo {
    active: usize,
    last_action: String,
}

impl TabChromeDemo {
    pub fn new() -> Self {
        Self {
            active: 0,
            last_action: "ready".to_string(),
        }
    }

    /// Only the active tab shows a close button — see the module doc for
    /// why that keeps `×`/`]` unambiguous on screen.
    fn bar(&self) -> TabBar {
        TabBar {
            id: WidgetId::new("tab-chrome-demo:tabs"),
            tabs: LABELS
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == self.active,
                    is_closable: i == self.active,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    fn chrome(&self) -> TabChrome {
        TabChrome::new(TabFrame::Brackets)
    }

    /// Bottom hint line — the driver test's window onto "closed" vs.
    /// "activated", since the tab strip's glyphs alone can't say which
    /// happened.
    fn hint_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("tab-chrome-demo:hint"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_action),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " tab next · q quit ".to_string(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for TabChromeDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for TabChromeDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        backend.draw_tab_bar_with_chrome(
            Rect::new(0.0, 0.0, viewport.width, lh),
            &self.bar(),
            None,
            &self.chrome(),
        );
        backend.draw_status_bar(
            Rect::new(0.0, viewport.height - lh, viewport.width, lh),
            &self.hint_bar(),
            None,
            None,
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            // Close buttons take precedence over tab-body activation —
            // mirrors `compose::tab_group::TabGroupController`'s own
            // click-routing order.
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let (x, y) = (position.x, position.y);
                if y > lh {
                    return Reaction::Continue;
                }
                let hits = backend.tab_bar_layout_with_chrome(
                    Rect::new(0.0, 0.0, viewport.width, lh),
                    &self.bar(),
                    &self.chrome(),
                );
                for (i, close) in hits.close_bounds.iter().enumerate() {
                    if let Some((start, end)) = close {
                        if end > start && (*start..*end).contains(&(x as f64)) {
                            self.last_action = format!("closed {}", LABELS[i]);
                            return Reaction::Redraw;
                        }
                    }
                }
                for (i, (start, end)) in hits.slot_positions.iter().enumerate() {
                    if end > start && (*start..*end).contains(&(x as f64)) {
                        self.active = i;
                        self.last_action = format!("activated {}", LABELS[i]);
                        return Reaction::Redraw;
                    }
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.active = (self.active + 1) % LABELS.len();
                self.last_action = format!("activated {}", LABELS[self.active]);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
