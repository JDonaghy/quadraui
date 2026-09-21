//! `SidebarPanelBody` (#1041) demo — background fill + optional
//! header/search chrome + a raw `TreeView` body + a hand-built
//! scrollbar, all composed through one `SidebarPanelBody` value instead
//! of the per-backend hand-rolled row-slicing the issue's `panels.rs`
//! audit found in `vimcode`.
//!
//! `TreeView`'s own `draw_tree` doesn't paint a scrollbar (see
//! `SidebarPanelBody`'s module doc) — this demo deliberately uses a raw
//! `TreeView`, not `TreeController`, so the composer's
//! `scrollbar_gutter` reservation + `scrollbar_rect` output are the
//! thing actually exercised, not hidden behind a controller that
//! already manages its own gutter.
//!
//! Controls:
//! - `↑` / `↓`     scroll the list
//! - `c`           cycle chrome: none → header → header+search → none
//! - `/`           toggle search-input focus (header+search mode only)
//! - typed chars / `Backspace` — edit the search query while focused
//! - `q` / `Esc`   quit

use quadraui::compose::sidebar_panel_body::{SidebarPanelBody, SidebarPanelChrome};
use quadraui::{
    AppLogic, Backend, BackendWidget, Color, Decoration, InteractionState, Key, NamedKey, Reaction,
    Rect, Scrollbar, StatusBar, StatusBarSegment, StyledText, TreeRow, TreeStyle, TreeView,
    UiEvent, WidgetId,
};

const STATUS_BAR_LINES: f32 = 1.0;
const ROW_COUNT: usize = 40;

/// Cycle order for `c`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChromeMode {
    None,
    Header,
    HeaderAndSearch,
}

impl ChromeMode {
    fn next(self) -> Self {
        match self {
            ChromeMode::None => ChromeMode::Header,
            ChromeMode::Header => ChromeMode::HeaderAndSearch,
            ChromeMode::HeaderAndSearch => ChromeMode::None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ChromeMode::None => "none",
            ChromeMode::Header => "header",
            ChromeMode::HeaderAndSearch => "header+search",
        }
    }
}

/// Owned [`TreeView`] wrapper so it can be handed to
/// [`SidebarPanelBody::render`] as a `&dyn BackendWidget` — the same
/// "adapter owns the widget value, `render` just paints it" shape
/// `BottomPanelTab::content` already uses.
struct TreeBody(TreeView);

impl BackendWidget for TreeBody {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        backend.draw_tree(rect, &self.0);
    }
}

pub struct SidebarPanelBodyDemo {
    rows: Vec<TreeRow>,
    scroll_offset: usize,
    chrome_mode: ChromeMode,
    search_query: String,
    search_active: bool,
    last_action: String,
}

impl SidebarPanelBodyDemo {
    pub fn new() -> Self {
        Self {
            rows: fake_rows(ROW_COUNT),
            scroll_offset: 0,
            chrome_mode: ChromeMode::HeaderAndSearch,
            search_query: String::new(),
            search_active: false,
            last_action: "—".into(),
        }
    }

    fn panel_rect(backend: &dyn Backend) -> Rect {
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

    fn filtered_rows(&self) -> Vec<TreeRow> {
        if self.search_query.is_empty() {
            self.rows.clone()
        } else {
            self.rows
                .iter()
                .filter(|r| {
                    r.text
                        .spans
                        .iter()
                        .any(|s| s.text.contains(&self.search_query))
                })
                .cloned()
                .collect()
        }
    }

    fn chrome(&self) -> SidebarPanelChrome {
        match self.chrome_mode {
            ChromeMode::None => SidebarPanelChrome::None,
            ChromeMode::Header => SidebarPanelChrome::Header("ITEMS".into()),
            ChromeMode::HeaderAndSearch => SidebarPanelChrome::HeaderAndSearch {
                header: "ITEMS".into(),
                query: self.search_query.clone(),
                placeholder: "Filter items".into(),
                active: self.search_active,
            },
        }
    }

    fn build_status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("panel-body-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " chrome={} last: {} ",
                    self.chrome_mode.label(),
                    self.last_action
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " ↑↓ / c=chrome / /=search / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for SidebarPanelBodyDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for SidebarPanelBodyDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let panel_rect = Self::panel_rect(backend);
        let status = Self::status_rect(backend);

        let rows = self.filtered_rows();
        let total_rows = rows.len();
        let panel = SidebarPanelBody {
            background: Some(Color::rgb(24, 24, 28)),
            chrome: self.chrome(),
            scrollbar_gutter: Some(backend.char_width().max(1.0)),
        };

        let tree = TreeView {
            id: WidgetId::new("panel-body-demo:tree"),
            rows,
            selection_mode: quadraui::SelectionMode::Single,
            selected_path: None,
            scroll_offset: self.scroll_offset,
            style: TreeStyle::default(),
            has_focus: true,
        };
        let layout = panel.render(backend, panel_rect, &TreeBody(tree));

        if let Some(sb_rect) = layout.scrollbar_rect {
            let line_h = backend.line_height().max(1.0);
            let visible = (sb_rect.height / line_h).max(1.0);
            let sb = Scrollbar::vertical(
                WidgetId::new("panel-body-demo:scrollbar"),
                sb_rect,
                self.scroll_offset as f32,
                total_rows as f32,
                visible,
                line_h,
            );
            backend.draw_scrollbar(sb_rect, &sb);
        }

        let _ = backend.draw_status_bar_interactive(
            status,
            &self.build_status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('c'),
                ..
            } if !self.search_active => {
                self.chrome_mode = self.chrome_mode.next();
                self.last_action = format!("chrome→{}", self.chrome_mode.label());
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('/'),
                ..
            } if self.chrome_mode == ChromeMode::HeaderAndSearch => {
                self.search_active = !self.search_active;
                self.last_action = if self.search_active {
                    "search focused".into()
                } else {
                    "search unfocused".into()
                };
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                let max = self.rows.len().saturating_sub(1);
                self.scroll_offset = (self.scroll_offset + 1).min(max);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Backspace),
                ..
            } if self.search_active => {
                self.search_query.pop();
                self.scroll_offset = 0;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char(c), ..
            } if self.search_active => {
                self.search_query.push(c);
                self.scroll_offset = 0;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if !self.search_active => Reaction::Exit,
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

fn fake_rows(n: usize) -> Vec<TreeRow> {
    (0..n)
        .map(|i| TreeRow {
            path: vec![i as u16],
            indent: 0,
            icon: None,
            text: StyledText::plain(format!("item{i}")),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        })
        .collect()
}
