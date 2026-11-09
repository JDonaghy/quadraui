//! Backend-agnostic app code for the panel example
//! ([`tui_panel`] / [`gtk_panel`]).
//!
//! [`PanelApp`] demonstrates a [`SidebarPanel`] with a [`Toolbar`] header
//! (title label, close and maximize action buttons) and a content area
//! showing selectable text — the `Panel` title-bar shape, reproduced
//! with `SidebarPanel` + `Toolbar`.
//!
//! Controls:
//! - click-drag content area   select text (TUI: line-wise highlight)
//! - Ctrl-A                    select all content in the panel
//! - Ctrl-C (with selection)   copy selection to clipboard (OSC52 + native)
//! - click close button        quit
//! - click maximize button     toggle collapsed
//! - click title bar           log "title clicked"
//! - c                         toggle collapsed
//! - q / Esc                   quit

use quadraui::{
    AppLogic, Backend, Color, InteractionState, Key, NamedKey, Reaction, Rect, SidebarPanel,
    SidebarPanelHit, StatusBar, StatusBarSegment, TextRegion, Toolbar, ToolbarButton, UiEvent,
    WidgetId,
};

const CONTENT_LINES: &[&str] = &[
    "The quick brown fox jumps over the lazy dog.",
    "Pack my box with five dozen liquor jugs.",
    "How vexingly quick daft zebras jump!",
    "The five boxing wizards jump quickly.",
    "Sphinx of black quartz, judge my vow.",
];

const CONTENT_ID: &str = "panel-content";

pub struct PanelApp {
    collapsed: bool,
    last_message: String,
}

impl PanelApp {
    pub fn new() -> Self {
        Self {
            collapsed: false,
            last_message: "Click-drag or Ctrl-A to select, Ctrl-C to copy".into(),
        }
    }

    /// The panel's header toolbar: a title label, a spacer, then the
    /// maximize and close actions — close last so it lands rightmost,
    /// matching the old `Panel::actions` convention (first action is
    /// rightmost).
    fn toolbar(&self) -> Toolbar {
        Toolbar::new(WidgetId::new("demo-panel:toolbar"))
            .with_buttons(vec![
                ToolbarButton::Label {
                    text: " Demo Panel".into(),
                    fg: None,
                },
                ToolbarButton::Spacer,
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("maximize"),
                    label: if self.collapsed { "+" } else { "□" }.into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: self.collapsed,
                    tooltip: if self.collapsed { "Expand" } else { "Maximize" }.into(),
                },
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("close"),
                    label: "×".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: "Close".into(),
                },
            ])
            .with_bg(Color::rgb(40, 80, 120))
    }

    fn panel(&self) -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("demo-panel"),
            toolbar: Some(self.toolbar()),
            toolbar_height: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " Ctrl-A=select all | c=collapse | q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    /// Renders the selectable text block and returns the bounds covering all
    /// rendered lines (used by `render` to register the `TextRegion`).
    fn fill_content(&self, backend: &mut dyn Backend, bounds: Rect) -> Rect {
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return bounds;
        }
        let lh = backend.line_height();
        let bg = Color::rgb(20, 20, 35);
        let fg = Color::rgb(210, 210, 210);
        let mut rendered_height = 0.0_f32;
        for (i, &line) in CONTENT_LINES.iter().enumerate() {
            let row_y = bounds.y + i as f32 * lh;
            if row_y + lh > bounds.y + bounds.height {
                break;
            }
            let row_rect = Rect::new(bounds.x, row_y, bounds.width, lh);
            let bar = StatusBar {
                id: WidgetId::new(format!("{CONTENT_ID}-line-{i}")),
                left_segments: vec![StatusBarSegment {
                    text: format!(" {line} "),
                    fg,
                    bg,
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let _ = backend.draw_status_bar_interactive(row_rect, &bar, &InteractionState::new());
            rendered_height = (i + 1) as f32 * lh;
        }
        Rect::new(bounds.x, bounds.y, bounds.width, rendered_height)
    }
}

impl Default for PanelApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for PanelApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let panel_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
        let panel = self.panel();
        let layout =
            backend.draw_sidebar_panel_interactive(panel_rect, &panel, &InteractionState::new());

        let content_bounds = if self.collapsed {
            Rect::new(layout.content_bounds.x, layout.content_bounds.y, 0.0, 0.0)
        } else {
            self.fill_content(backend, layout.content_bounds)
        };
        // Register the selectable text region. Pass `lines` so pixel-based
        // backends (GTK, macOS) can extract the selected substring at Ctrl-C
        // time. TUI ignores `lines` and reads cells from its ratatui buffer.
        backend.register_text_region(TextRegion {
            id: WidgetId::new(CONTENT_ID),
            bounds: content_bounds,
            lines: CONTENT_LINES.iter().map(|s| s.to_string()).collect(),
        });

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('c'),
                ..
            } => {
                self.collapsed = !self.collapsed;
                self.last_message = if self.collapsed {
                    "Collapsed".into()
                } else {
                    "Expanded".into()
                };
                Reaction::Redraw
            }
            UiEvent::TextCopied(text) => {
                let chars: Vec<char> = text.chars().take(40).collect();
                let preview: String = chars.iter().collect();
                let suffix = if text.chars().count() > 40 { "…" } else { "" };
                self.last_message = format!("Copied: \"{preview}{suffix}\"");
                Reaction::Redraw
            }
            UiEvent::TextSelectionChanged { anchor, focus, .. } => {
                let lh = backend.line_height();
                let anchor_row = (anchor.y / lh).floor() as usize;
                let focus_row = (focus.y / lh).floor() as usize;
                let (start, end) = if anchor_row <= focus_row {
                    (anchor_row, focus_row)
                } else {
                    (focus_row, anchor_row)
                };
                self.last_message = if start == end {
                    format!("Selecting row {} — Ctrl-C to copy", start + 1)
                } else {
                    format!("Selecting rows {}–{} — Ctrl-C to copy", start + 1, end + 1)
                };
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                self.last_message = "Click-drag or Ctrl-A to select, Ctrl-C to copy".into();
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let panel_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
                let panel = self.panel();
                let layout = backend.sidebar_panel_layout(panel_rect, &panel);
                match layout.hit_test(position.x, position.y) {
                    SidebarPanelHit::ToolbarButton(id) if id.as_str() == "close" => {
                        return Reaction::Exit;
                    }
                    SidebarPanelHit::ToolbarButton(id) if id.as_str() == "maximize" => {
                        self.collapsed = !self.collapsed;
                        self.last_message = if self.collapsed {
                            "Collapsed".into()
                        } else {
                            "Expanded".into()
                        };
                    }
                    SidebarPanelHit::ToolbarEmpty => {
                        self.last_message = "Title bar clicked".into();
                    }
                    _ => {}
                }
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
