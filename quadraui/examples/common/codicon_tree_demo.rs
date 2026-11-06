//! Backend-agnostic app code for the codicon-icon tree demo
//! ([`tui_codicon_tree`] / [`gtk_codicon_tree`]).
//!
//! [`CodiconTreeDemo`] paints a small file-explorer-shaped [`TreeView`]
//! whose rows use [`quadraui::Icon::codicon`] — `"root-folder"`,
//! `"folder"`, `"file-code"`, `"markdown"` — instead of a raw glyph
//! string. `n` toggles `set_nerd_fonts`, which flips the same rows
//! between the codicon glyph and each icon's plain-text fallback on
//! every backend: [`quadraui::Backend::set_nerd_fonts`] is a real,
//! backend-agnostic flag, not a TUI-only concept, and the GTK rasteriser
//! honours it the same way the TUI one does (see
//! [`quadraui::Icon::codicon`]'s doc for why no backend-specific wiring
//! was needed to paint the glyph side in the first place).
//!
//! Controls:
//! - `n` toggle nerd fonts on/off (codicon glyph vs. plain-text fallback,
//!   on every backend)
//! - `q` / `Esc` quit

use quadraui::{
    AppLogic, Backend, Color, Decoration, Icon, InteractionState, Key, NamedKey, Reaction, Rect,
    SelectionMode, StatusBar, StatusBarSegment, StyledText, TreeRow, TreeStyle, TreeView, UiEvent,
    WidgetId,
};

const STATUS_BAR_LINES: f32 = 1.0;

pub struct CodiconTreeDemo {
    rows: Vec<TreeRow>,
    nerd_fonts_enabled: bool,
}

impl CodiconTreeDemo {
    pub fn new() -> Self {
        Self {
            rows: Self::demo_rows(),
            nerd_fonts_enabled: true,
        }
    }

    /// Current nerd-fonts flag, for test assertions
    /// (`driver.app().nerd_fonts_enabled()`).
    pub fn nerd_fonts_enabled(&self) -> bool {
        self.nerd_fonts_enabled
    }

    /// A small project-shaped tree: a root folder, one subfolder, and a
    /// few files — enough to show `"folder"` and `"file*"` codicons side
    /// by side without a scrollbar.
    fn demo_rows() -> Vec<TreeRow> {
        let row =
            |path: &[u16], indent: u16, codicon: &str, fallback: &str, label: &str, expanded| {
                TreeRow {
                    path: path.to_vec(),
                    indent,
                    icon: Icon::codicon(codicon, fallback),
                    text: StyledText::plain(label),
                    badge: None,
                    is_expanded: expanded,
                    decoration: Decoration::Normal,
                    edit: None,
                }
            };
        vec![
            row(&[0], 0, "root-folder", "DIR", "quadraui", Some(true)),
            row(&[0, 0], 1, "folder", "DIR", "src", Some(true)),
            row(&[0, 0, 0], 2, "file-code", "RS", "types.rs", None),
            row(&[0, 0, 1], 2, "file-code", "RS", "codicon.rs", None),
            row(&[0, 1], 1, "file", "TOML", "Cargo.toml", None),
            row(&[0, 2], 1, "markdown", "MD", "README.md", None),
        ]
    }

    fn tree_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let status_h = backend.line_height() * STATUS_BAR_LINES;
        Rect::new(
            0.0,
            0.0,
            viewport.width,
            (viewport.height - status_h).max(0.0),
        )
    }

    fn status_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let status_h = backend.line_height() * STATUS_BAR_LINES;
        Rect::new(
            0.0,
            (viewport.height - status_h).max(0.0),
            viewport.width,
            status_h,
        )
    }

    fn status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("codicon-tree-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " Icon::codicon demo — nerd_fonts={} ",
                    self.nerd_fonts_enabled
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " n=toggle nerd fonts | q=quit ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for CodiconTreeDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for CodiconTreeDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.set_nerd_fonts(self.nerd_fonts_enabled);

        let tree = TreeView {
            id: WidgetId::new("codicon-tree-demo:tree"),
            rows: self.rows.clone(),
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            scroll_px: None,
            style: TreeStyle::default(),
            has_focus: true,
        };
        backend.draw_tree(Self::tree_rect(backend), &tree);

        let _ = backend.draw_status_bar_interactive(
            Self::status_rect(backend),
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('n'),
                ..
            } => {
                self.nerd_fonts_enabled = !self.nerd_fonts_enabled;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
