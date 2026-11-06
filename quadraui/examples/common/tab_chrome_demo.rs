//! Minimal `AppLogic` painting a `TabBar` whose active tab is enclosed in
//! bracket framing via [`TabChrome::active_frame`] —
//! `[main.rs ×]` rather than the close glyph floating unframed after the
//! label.
//!
//! The point of the demo is the **sidecar + declarative click routing**:
//! the bracket framing is requested via [`Backend::draw_tab_bar_with_chrome`]
//! (never baked into [`TabItem::label`] as a string), and a click still
//! resolves through [`Backend::tab_bar_layout_with_chrome`]'s `close_bounds` /
//! `slot_positions`, never a hand-rolled character scan for `×`.
//!
//! Only the active tab is closable (`is_closable` tracks `is_active`), so
//! its `×` — and, since chrome frames it, its `]` — are the only
//! occurrence of either glyph on screen at any moment. That is what lets
//! the driver test click "×" by text alone and know unambiguously which
//! tab it landed on, and it is also why closing never needs to be undone
//! for the demo to stay exercisable: the *other* tab has no close button
//! to confuse the click routing with.
//!
//! The demo also composes [`TabChrome::focused_index`] with its own
//! keyboard-vs-pointer focus-visible state, the way that field's doc asks
//! every caller to: `Left`/`Right` move keyboard focus one tab via
//! [`TabBar::handle_focus_key`] and the ring stays visible (reverse-video
//! paints on `chrome()`'s composed index); `Enter`/`Space` activate the
//! focused tab; a click focuses+activates a tab without showing the ring
//! — [`Self::chrome`] passes `None` instead of `self.focused` whenever
//! [`Self::focus_visible`] is `false`, same suppression convention
//! [`quadraui::compose::FocusRing::set`] documents for its own click path.

use quadraui::{
    AppLogic, Backend, Color, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, TabBar, TabBarEvent, TabChrome, TabFrame, TabItem, UiEvent, WidgetId,
};

/// Tab labels, index-stable across activation (only `is_active` /
/// `is_closable` move between them).
pub const LABELS: [&str; 2] = ["main.rs", "lib.rs"];

pub struct TabChromeDemo {
    active: usize,
    last_action: String,
    /// Keyboard-focused tab, independent of [`Self::active`] until
    /// `Enter`/`Space` activates it. `None` until the first `Left`/`Right`
    /// — [`TabBar::handle_focus_key`]'s own "nothing focused yet" state.
    focused: Option<usize>,
    /// Whether [`Self::focused`] should paint — true after a keyboard
    /// move, false after a click. See the module doc.
    focus_visible: bool,
}

impl TabChromeDemo {
    pub fn new() -> Self {
        Self {
            active: 0,
            last_action: "ready".to_string(),
            focused: None,
            focus_visible: false,
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

    /// Composes keyboard focus with focus-visible — the one step
    /// [`TabChrome::focused_index`]'s doc delegates to every caller.
    /// `self.focused` only reaches the chrome (and so only paints
    /// reverse-video) when [`Self::focus_visible`] is set; a click
    /// leaves `self.focused` alone but this still passes `None`.
    fn chrome(&self) -> TabChrome {
        let focused_index = self.focus_visible.then_some(self.focused).flatten();
        TabChrome::new(TabFrame::Brackets).with_focused_index(focused_index)
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
        backend.draw_status_bar_interactive(
            Rect::new(0.0, viewport.height - lh, viewport.width, lh),
            &self.hint_bar(),
            &InteractionState::new(),
        );
    }

    #[allow(deprecated)] // `Backend::tab_bar_layout_with_chrome` returns `TabBarHits`.
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
                            // A click always suppresses the focus ring, even
                            // when it lands on the close glyph rather than
                            // the tab body — see the module doc.
                            self.focus_visible = false;
                            self.last_action = format!("closed {}", LABELS[i]);
                            return Reaction::Redraw;
                        }
                    }
                }
                for (i, (start, end)) in hits.slot_positions.iter().enumerate() {
                    if end > start && (*start..*end).contains(&(x as f64)) {
                        self.active = i;
                        self.focused = Some(i);
                        self.focus_visible = false;
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
            // `Left`/`Right` move keyboard focus; `Enter`/`Space` activate
            // it — [`TabBar::handle_focus_key`] decides which, the same
            // helper `quadraui-binding-core`'s `focus_scoped_key_event`
            // calls for a py/node app's `"tabs"` node.
            UiEvent::KeyPressed {
                key: Key::Named(named),
                ..
            } if matches!(named, NamedKey::Left | NamedKey::Right | NamedKey::Enter) => {
                let key_str = match named {
                    NamedKey::Left => "Left",
                    NamedKey::Right => "Right",
                    NamedKey::Enter => "Enter",
                    _ => unreachable!(),
                };
                match TabBar::handle_focus_key(self.focused, LABELS.len(), key_str) {
                    Some(TabBarEvent::FocusMoved { index }) => {
                        self.focused = Some(index);
                        self.focus_visible = true;
                        self.last_action = format!("focus {}", LABELS[index]);
                        Reaction::Redraw
                    }
                    Some(TabBarEvent::TabActivated { index }) => {
                        self.active = index;
                        self.focus_visible = true;
                        self.last_action = format!("activated {}", LABELS[index]);
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }
}
