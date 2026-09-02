//! Minimal `SidebarSystem` demo for `SidebarSystem::reveal` (#595) — a
//! *programmatic* selection (no click, no arrow key) that must select
//! the target row, expand its section if collapsed, and scroll it into
//! view.
//!
//! One Tree section with more rows than fit on screen at once, so `g`
//! (reveal the section's last row) has to actually move the viewport to
//! prove anything happened. This demo relies on `PerSection` scroll mode
//! — `SidebarSystem`'s default. `ScrollMode::WholePanel` (see
//! `multi_tree.rs`'s `DebugSidebar`) drives the viewport from a single
//! panel-level scroll offset instead of each section's own
//! `TreeController::scroll_offset`, so `reveal`'s
//! `TreeController::scroll_to_visible` call — correct as it is — has no
//! visible effect under that mode. This demo intentionally stays in the
//! default mode so `reveal` actually moves the screen.
//!
//! Controls:
//! - `↑` / `↓`     move selection (interactive nav — already scrolls)
//! - `z`           toggle collapse of the section
//! - `g`           reveal (#595): select + expand + scroll the
//!   section's last row into view, without going through interactive
//!   nav — simulates a caller restoring a saved selection or jumping
//!   to a search hit
//! - `q` / `Esc`   quit

use quadraui::{
    AppLogic, Backend, Color, Decoration, Key, NamedKey, NavigationMode, Reaction, Rect,
    SidebarEvent, SidebarSectionDef, SidebarSystem, StatusBar, StatusBarSegment, StyledText,
    TreeRow, UiEvent, WidgetId,
};

const STATUS_BAR_LINES: f32 = 1.0;
/// Row count for the single "ITEMS" section — kept alongside the
/// `fake_rows` call in [`SidebarRevealDemo::new`] so the `g` handler
/// knows the last row's path without needing a public "row count"
/// getter on `SidebarSystem`.
const ROW_COUNT: usize = 30;

pub struct SidebarRevealDemo {
    sidebar: SidebarSystem,
    last_action: String,
}

impl SidebarRevealDemo {
    pub fn new() -> Self {
        let mut sidebar = SidebarSystem::new(vec![SidebarSectionDef::new("items", "ITEMS")]);
        sidebar.set_rows(0, fake_rows(ROW_COUNT));
        sidebar.set_navigation_mode(NavigationMode::Selection);
        sidebar.set_active_section(Some(0));
        Self {
            sidebar,
            last_action: "—".into(),
        }
    }

    fn sidebar_rect(backend: &dyn Backend) -> Rect {
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

    fn build_status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("reveal-status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" last: {} ", self.last_action),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " ↑↓ / z=collapse / g=reveal / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for SidebarRevealDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for SidebarRevealDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let sidebar = Self::sidebar_rect(backend);
        let status = Self::status_rect(backend);
        self.sidebar.render(backend, sidebar);
        let _ = backend.draw_status_bar(status, &self.build_status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        // `reveal` is backend-free and needs this cached first — see
        // `SidebarSystem::set_backend_info`.
        self.sidebar
            .set_backend_info(backend.line_height(), backend.msv_metrics());

        let rect = Self::sidebar_rect(backend);
        match self.sidebar.handle(&event, backend, rect) {
            SidebarEvent::RowSelected { section, path } => {
                self.last_action = format!("sel→{section} {path:?}");
                Reaction::Redraw
            }
            SidebarEvent::HeaderActivated { section } => {
                self.last_action = format!("header→{section}");
                Reaction::Redraw
            }
            SidebarEvent::StateChanged
            | SidebarEvent::Consumed
            | SidebarEvent::ScrollChanged { .. } => Reaction::Redraw,
            SidebarEvent::Ignored => match event {
                // #595: toggle collapse of the section — lets `g` (below)
                // exercise "reveal on a collapsed section expands it
                // first, then scrolls".
                UiEvent::KeyPressed {
                    key: Key::Char('z'),
                    ..
                } => {
                    let collapsed = !self.sidebar.is_collapsed(0);
                    self.sidebar.set_collapsed(0, collapsed);
                    self.last_action =
                        format!("{}→0", if collapsed { "collapsed" } else { "expanded" });
                    Reaction::Redraw
                }
                // #595: `reveal` — a *programmatic* selection (no click,
                // no arrow key) of the section's last row. Proves
                // `reveal` selects the row, expands the section if it
                // was collapsed via `z`, and scrolls it into view.
                UiEvent::KeyPressed {
                    key: Key::Char('g'),
                    ..
                } => {
                    let target = vec![(ROW_COUNT - 1) as u16];
                    let rect = Self::sidebar_rect(backend);
                    self.sidebar.reveal(0, &target, rect);
                    self.last_action = format!("reveal→0 {target:?}");
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
            },
            _ => Reaction::Redraw,
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
