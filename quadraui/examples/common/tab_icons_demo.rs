//! Minimal `AppLogic` painting a `TabBar` whose tabs carry per-tab
//! [`TabIcon`] glyphs (quadraui#620) — VS Code's coloured
//! language/file-type badge ahead of each tab label.
//!
//! The point of the demo is the **sidecar**: icons are passed to
//! [`Backend::draw_tab_bar_icons`] as a slice parallel to `bar.tabs`
//! rather than as a `TabItem` field, so the same `TabBar` value paints
//! with or without decoration. Pressing `i` toggles the sidecar between
//! the real icon list and `&[]`, which is exactly the comparison a human
//! wants to eyeball: label positions shift right by the icon reservation
//! and nothing else moves.
//!
//! Glyphs are ASCII on purpose (`R`, `T`, `M`) — a Nerd Font codepoint
//! renders as a blank box in a terminal without a patched font, which
//! would make the demo look broken on a stock machine and would give the
//! driver test nothing to assert on. The colours are what carry the
//! "identity colour survives an inactive tab" half of the contract.

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, TabBar,
    TabIcon, TabItem, UiEvent, WidgetId,
};

/// Tab labels, paired index-for-index with [`TabIconsDemo::icons`].
pub const LABELS: [&str; 3] = [" main.rs ", " Cargo.toml ", " README.md "];

/// The key that toggles the icon sidecar on/off.
pub const TOGGLE_KEY: char = 'i';

pub struct TabIconsDemo {
    active: usize,
    icons_on: bool,
}

impl TabIconsDemo {
    pub fn new() -> Self {
        Self {
            active: 0,
            icons_on: true,
        }
    }

    /// The icon sidecar: entry `i` decorates tab `i`. Rust orange, TOML
    /// grey-blue, Markdown blue — each independent of the tab's
    /// active/inactive foreground.
    pub fn icons(&self) -> Vec<Option<TabIcon>> {
        vec![
            Some(TabIcon {
                glyph: "R".to_string(),
                color: Color::rgb(222, 165, 132),
            }),
            Some(TabIcon {
                glyph: "T".to_string(),
                color: Color::rgb(160, 190, 210),
            }),
            Some(TabIcon {
                glyph: "M".to_string(),
                color: Color::rgb(120, 170, 240),
            }),
        ]
    }

    /// Bottom hint line — also the driver test's window onto which mode
    /// the demo is in, since the icon glyphs alone can't say "off".
    fn hint_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("tab-icons-demo:hint"),
            left_segments: vec![StatusBarSegment {
                text: if self.icons_on {
                    " icons: on ".to_string()
                } else {
                    " icons: off ".to_string()
                },
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " i toggle · tab next · q quit ".to_string(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn bar(&self) -> TabBar {
        TabBar {
            id: WidgetId::new("tab-icons-demo:tabs"),
            tabs: LABELS
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == self.active,
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
}

impl Default for TabIconsDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for TabIconsDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let bar = self.bar();
        // The same `TabBar`, painted with or without the sidecar — the
        // whole point of #620's shape.
        let icons = if self.icons_on { self.icons() } else { vec![] };
        backend.draw_tab_bar_icons(Rect::new(0.0, 0.0, viewport.width, lh), &bar, &icons, None);
        backend.draw_status_bar(
            Rect::new(0.0, viewport.height - lh, viewport.width, lh),
            &self.hint_bar(),
            None,
            None,
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            // Click routing goes through the **no-paint twin**
            // (`tab_bar_layout_icons`) with the same sidecar the paint
            // used, which is the whole reason that twin exists: the
            // icon-less `tab_bar_layout` would report every slot shifted
            // left by the icon reservation, so clicking the third tab
            // would activate the second.
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let (x, y) = (position.x, position.y);
                if y <= lh {
                    let icons = if self.icons_on { self.icons() } else { vec![] };
                    let hits = backend.tab_bar_layout_icons(
                        Rect::new(0.0, 0.0, viewport.width, lh),
                        &self.bar(),
                        &icons,
                    );
                    for (i, (start, end)) in hits.slot_positions.iter().enumerate() {
                        if (*start..*end).contains(&(x as f64)) && end > start {
                            self.active = i;
                            return Reaction::Redraw;
                        }
                    }
                }
                Reaction::Continue
            }
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char(TOGGLE_KEY),
                ..
            } => {
                self.icons_on = !self.icons_on;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.active = (self.active + 1) % LABELS.len();
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
