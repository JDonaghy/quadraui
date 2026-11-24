//! Minimal `AppLogic` painting a `TabBar` whose tabs carry per-tab
//! [`TabIcon`] glyphs — VS Code's coloured
//! language/file-type badge ahead of each tab label.
//!
//! The point of the demo is the **sidecar**: icons ride on
//! [`TabBar::icons`] as a list parallel to `bar.tabs` rather than as a
//! [`TabItem`] field, so the same tabs paint with or without decoration.
//! Pressing `i` toggles the sidecar between the real icon list and an
//! empty one, which is exactly the comparison a human wants to eyeball:
//! label positions shift right by the icon reservation and nothing else
//! moves.
//!
//! Glyphs are ASCII on purpose (`R`, `T`, `M`) — a Nerd Font codepoint
//! renders as a blank box in a terminal without a patched font, which
//! would make the demo look broken on a stock machine and would give the
//! driver test nothing to assert on. The colours are what carry the
//! "identity colour survives an inactive tab" half of the contract.
//!
//! Every tab is closable: a click on its `×` removes it from the bar
//! (the icon sidecar is filtered to match). If the closed tab was
//! active, its **right-hand neighbour** becomes active — the same
//! close-neighbour rule as `compose::WorkspaceController::close` —
//! falling back to the new last tab when the closed tab was the
//! rightmost one. Close hit-testing is checked before tab-body
//! activation, so a click on the glyph never doubles as an activation
//! click first.

use quadraui::{
    AppLogic, Backend, Color, DrawsPrimitives, InteractionState, Key, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, TabBar, TabBarHit, TabIcon, TabItem, UiEvent, WidgetId,
};

/// Tab labels, paired index-for-index with [`TabIconsDemo::icons`].
pub const LABELS: [&str; 3] = [" main.rs ", " Cargo.toml ", " README.md "];

/// The key that toggles the icon sidecar on/off.
pub const TOGGLE_KEY: char = 'i';

pub struct TabIconsDemo {
    active: usize,
    icons_on: bool,
    /// Parallel to [`LABELS`] — `true` once that tab has been closed.
    closed: [bool; LABELS.len()],
}

impl TabIconsDemo {
    pub fn new() -> Self {
        Self {
            active: 0,
            icons_on: true,
            closed: [false; LABELS.len()],
        }
    }

    /// [`LABELS`] indices still open, in display order.
    fn open_indices(&self) -> Vec<usize> {
        (0..LABELS.len()).filter(|&i| !self.closed[i]).collect()
    }

    /// The icon sidecar, filtered and reordered to match `bar().tabs`.
    fn visible_icons(&self) -> Vec<Option<TabIcon>> {
        if !self.icons_on {
            return vec![];
        }
        let icons = self.icons();
        self.open_indices()
            .into_iter()
            .map(|i| icons[i].clone())
            .collect()
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
            tabs: self
                .open_indices()
                .into_iter()
                .map(|i| TabItem {
                    label: LABELS[i].to_string(),
                    is_active: i == self.active,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
            icons: self.visible_icons(),
            ..Default::default()
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
        // The bar carries its icons, or none while they're toggled off.
        backend.draw_dyn(Rect::new(0.0, 0.0, viewport.width, lh), &self.bar(), &());
        backend.draw_status_bar_interactive(
            Rect::new(0.0, viewport.height - lh, viewport.width, lh),
            &self.hint_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            // Clicks resolve against the same `Primitive::layout` the
            // paint used, icons included: a layout without them would
            // report every slot shifted left by the icon reservation, so
            // clicking the third tab would activate the second.
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let (x, y) = (position.x, position.y);
                if y <= lh {
                    let strip = backend
                        .layout_primitive(Rect::new(0.0, 0.0, viewport.width, lh), &self.bar());
                    let open = self.open_indices();
                    match strip.layout.hit_test(x, y) {
                        TabBarHit::TabClose(vis_i) => {
                            let orig = open[vis_i];
                            self.closed[orig] = true;
                            if self.active == orig {
                                // Close-neighbour rule (see
                                // `compose::WorkspaceController::close`'s
                                // doc): the tab that slides into the
                                // closed one's visible slot — its right
                                // neighbour — takes over; when the closed
                                // tab was last, clamp onto the new last
                                // tab instead.
                                let new_open = self.open_indices();
                                if let Some(last) = new_open.len().checked_sub(1) {
                                    self.active = new_open[vis_i.min(last)];
                                }
                            }
                            return Reaction::Redraw;
                        }
                        TabBarHit::Tab(vis_i) => {
                            self.active = open[vis_i];
                            return Reaction::Redraw;
                        }
                        _ => {}
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
                let open = self.open_indices();
                if !open.is_empty() {
                    let pos = open.iter().position(|&i| i == self.active).unwrap_or(0);
                    self.active = open[(pos + 1) % open.len()];
                }
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
