//! `FindReplaceApp` — minimal visual smoke test for the `FindReplacePanel`
//! widget.
//!
//! Renders the find/replace overlay anchored at the top-right of a
//! full-screen "editor" area, routed through the app's `FrameTree`. Every
//! `FindReplaceEvent` the panel reports is written back onto it and
//! named in the status bar.
//!
//! ## Key bindings
//!
//! | Key       | Action                                          |
//! |-----------|-------------------------------------------------|
//! | `r`       | Toggle the replace row (panel not focused)      |
//! | `q` / Esc | Quit (Esc closes the panel while it is focused) |

use std::cell::Cell;

use quadraui::{
    AppLogic, Backend, Color, FindReplaceEvent, FindReplacePanel, FindReplaceToggle, FrameTree,
    NamedKey, Reaction, Rect, ScreenLayout, StatusBar, StatusBarSegment, Surface, UiEvent,
    WidgetId, FR_PANEL_WIDTH,
};

pub struct FindReplaceApp {
    pub panel: FindReplacePanel,
    last_event: Option<String>,
    /// The box `render` last painted the panel in — the rect the frame
    /// tree routes it at.
    panel_rect: Cell<Rect>,
}

impl FindReplaceApp {
    pub fn new() -> Self {
        let mut panel = FindReplacePanel {
            id: WidgetId::new("find-replace"),
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
        };
        panel.rebuild_hit_regions();
        Self {
            panel,
            last_event: None,
            panel_rect: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
        }
    }

    /// The "editor" area the panel anchors in: everything above the
    /// status bar.
    fn editor_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, 0.0, vp.width, (vp.height - lh).max(0.0))
    }

    fn status_bar(&self) -> StatusBar {
        let msg = match &self.last_event {
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

/// The status-bar wording for one panel event.
fn describe(event: &FindReplaceEvent) -> String {
    match event {
        FindReplaceEvent::FieldEdited {
            field,
            text,
            cursor,
            ..
        } => format!("{field:?} = {text:?} @ {cursor}"),
        FindReplaceEvent::Toggled { toggle, on } => {
            format!("{toggle:?} {}", if *on { "on" } else { "off" })
        }
        other => format!("{other:?}"),
    }
}

impl AppLogic for FindReplaceApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Self::editor_rect(backend);
        // `group_bounds` is the editor rect the panel anchors at the
        // top-right of.
        let mut panel = self.panel.clone();
        panel.group_bounds = rect;
        let panel_rect = panel.anchored_rect(backend, rect);
        self.panel_rect.set(panel_rect);

        let status_rect = Rect::new(0.0, rect.height, vp.width, vp.height - rect.height);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: panel_rect,
            widget: Box::new(panel),
        });
        frame.push(Surface::Widget {
            rect: status_rect,
            widget: Box::new(self.status_bar()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(
            self.panel.id,
            self.panel_rect.get(),
            Box::new(self.panel.clone()),
        );
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::FindReplace(id, event) if id == self.panel.id => {
                if event == FindReplaceEvent::Close {
                    return Reaction::Exit;
                }
                self.panel.apply_event(&event);
                self.last_event = Some(describe(&event));
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('r'),
                ..
            } => {
                let on = !self.panel.show_replace;
                self.panel.apply_event(&FindReplaceEvent::Toggled {
                    toggle: FindReplaceToggle::ShowReplace,
                    on,
                });
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
