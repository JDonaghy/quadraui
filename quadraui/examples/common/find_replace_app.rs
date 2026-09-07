//! `FindReplaceApp` — minimal visual smoke test for the `FindReplacePanel`
//! primitive's [`FindReplacePanel::hit_test`] (quadraui#818).
//!
//! Renders the find/replace overlay anchored at the top-right of a
//! full-screen "editor" area. Clicking a toggle/nav/close button reports
//! which [`quadraui::FindReplaceClickTarget`] was hit in the status bar
//! — the primitive's first click-routed consumer (before #818,
//! `FindReplacePanel` had no `hit_test` at all).
//!
//! ## Key bindings
//!
//! | Key       | Action                          |
//! |-----------|----------------------------------|
//! | `r`       | Toggle the replace row           |
//! | `q` / Esc | Quit                              |

use quadraui::{
    compute_find_replace_hit_regions, AppLogic, Backend, Color, FindReplaceClickTarget,
    FindReplaceHit, FindReplacePanel, InteractionState, MouseButton, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, UiEvent, WidgetId, FR_PANEL_WIDTH,
};

pub struct FindReplaceApp {
    panel: FindReplacePanel,
    last_click: Option<String>,
}

impl FindReplaceApp {
    pub fn new() -> Self {
        let mut app = Self {
            panel: FindReplacePanel {
                query: "needle".into(),
                replacement: String::new(),
                show_replace: false,
                focus: 0,
                cursor: 6,
                sel_anchor: None,
                match_info: "1 of 3".into(),
                case_sensitive: false,
                whole_word: false,
                use_regex: false,
                preserve_case: false,
                in_selection: false,
                group_bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
                panel_width: FR_PANEL_WIDTH,
                replace_one_glyph: "R1".into(),
                replace_all_glyph: "R*".into(),
                hit_regions: Vec::new(),
            },
            last_click: None,
        };
        app.rebuild_hit_regions();
        app
    }

    /// Recompute `hit_regions` — must be called whenever `show_replace`
    /// or `match_info` changes, since [`compute_find_replace_hit_regions`]
    /// depends on both (matches [`FindReplacePanel::hit_regions`]'s own
    /// doc: "computed once ... at panel construction", re-run on every
    /// state change that would move a region).
    fn rebuild_hit_regions(&mut self) {
        let (regions, _input_w) = compute_find_replace_hit_regions(
            self.panel.panel_width,
            self.panel.show_replace,
            &self.panel.match_info,
            self.panel.replace_one_glyph.chars().count() as u16,
            self.panel.replace_all_glyph.chars().count() as u16,
        );
        self.panel.hit_regions = regions;
    }

    /// Same `rect` `render` paints the panel into (the "editor" area),
    /// used to keep `panel.group_bounds` and click routing in agreement.
    fn editor_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, 0.0, vp.width, (vp.height - lh).max(0.0))
    }

    /// The panel's content corner (inside its 1-cell border), in the
    /// same absolute coordinates `tui::draw_find_replace` paints at.
    /// Mirrors that function's `x`/`y` derivation exactly — see its own
    /// doc — so click routing can't drift from where the panel is
    /// actually painted (quadraui#818).
    fn content_origin(&self, rect: Rect) -> (f32, f32) {
        let editor_left = rect.x;
        let panel_w = (self.panel.panel_width as f32).min((rect.width - 2.0).max(0.0));
        let gb = &self.panel.group_bounds;
        let gb_right = editor_left + gb.x + gb.width;
        let x = (gb_right - (panel_w + 1.0)).max(editor_left);
        let y = gb.y.max(1.0);
        (x + 1.0, y + 1.0)
    }

    fn status_bar(&self) -> StatusBar {
        let msg = match &self.last_click {
            Some(m) => format!(" {m} "),
            None => " click a button — r toggles replace row, q quits ".into(),
        };
        StatusBar {
            id: WidgetId::new("find-replace-status"),
            left_segments: vec![StatusBarSegment {
                text: msg,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for FindReplaceApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FindReplaceApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Self::editor_rect(backend);
        // `group_bounds` fills the editor rect — content-relative, per
        // the primitive's doc (`editor_left` carries the absolute shift).
        let mut panel = self.panel.clone();
        panel.group_bounds = Rect::new(0.0, 0.0, rect.width, rect.height);
        backend.draw_find_replace(rect, &panel);

        let status_rect = Rect::new(0.0, rect.height, vp.width, vp.height - rect.height);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = Self::editor_rect(backend);
                self.panel.group_bounds = Rect::new(0.0, 0.0, rect.width, rect.height);
                let origin = self.content_origin(rect);
                let hit = self
                    .panel
                    .hit_test(position.x, position.y, origin, 1.0, 1.0);
                self.last_click = Some(match hit {
                    FindReplaceHit::Target(target) => {
                        match target {
                            FindReplaceClickTarget::ToggleCase => {
                                self.panel.case_sensitive = !self.panel.case_sensitive
                            }
                            FindReplaceClickTarget::ToggleWholeWord => {
                                self.panel.whole_word = !self.panel.whole_word
                            }
                            FindReplaceClickTarget::ToggleRegex => {
                                self.panel.use_regex = !self.panel.use_regex
                            }
                            FindReplaceClickTarget::TogglePreserveCase => {
                                self.panel.preserve_case = !self.panel.preserve_case
                            }
                            FindReplaceClickTarget::ToggleInSelection => {
                                self.panel.in_selection = !self.panel.in_selection
                            }
                            FindReplaceClickTarget::Chevron => {
                                self.panel.show_replace = !self.panel.show_replace;
                                self.rebuild_hit_regions();
                            }
                            _ => {}
                        }
                        format!("clicked {target:?}")
                    }
                    FindReplaceHit::Empty => "clicked empty area".into(),
                });
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('r'),
                ..
            } => {
                self.panel.show_replace = !self.panel.show_replace;
                self.rebuild_hit_regions();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: quadraui::Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
