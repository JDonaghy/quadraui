//! `Sidebar` demo — adapted from
//! `quadraui/examples/common/sidebar_reveal_demo.rs`,
//! `sidebar_panel_app.rs`, `sidebar_search.rs` and
//! `sidebar_panel_body_demo.rs`.
//!
//! Four variants cover the sidebar-family primitives: a plain
//! [`SidebarSystem`] tree with programmatic `reveal` (quadraui#595), a
//! [`SidebarPanel`] with a toolbar header above a task list (quadraui#259),
//! a [`SidebarSystem`] tree with `Decoration::Header` group rows, and a
//! [`compose::sidebar_panel_body::SidebarPanelBody`] composing background +
//! chrome + a raw `TreeView` body + scrollbar (quadraui#1041).

use quadraui::compose::sidebar_panel_body::{SidebarPanelBody, SidebarPanelChrome};
use quadraui::{
    Backend, BackendCaps, BackendWidget, Color, Decoration, InteractionState, NavigationMode,
    Reaction, Rect, Scrollbar, SidebarEvent, SidebarPanel, SidebarPanelHit, SidebarSectionDef,
    SidebarSystem, StatusBar, StatusBarSegment, StyledText, Toolbar, ToolbarButton,
    ToolbarHoverTracker, TreeRow, TreeStyle, TreeView, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("sidebar.rs");

// gallery:begin
const REVEAL_ROW_COUNT: usize = 30;
const BODY_ROW_COUNT: usize = 40;

fn flat_rows(n: usize) -> Vec<TreeRow> {
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

/// Rows for the "Header groups" variant: two file-header rows, each
/// followed by two match rows — `Decoration::Header` vs. `Normal`.
fn grouped_rows() -> Vec<TreeRow> {
    let mut rows = Vec::new();
    for (file_idx, file) in ["src/main.rs", "src/lib.rs"].iter().enumerate() {
        rows.push(TreeRow {
            path: vec![file_idx as u16],
            indent: 0,
            icon: None,
            text: StyledText::plain(*file),
            badge: Some(quadraui::Badge::plain("2")),
            is_expanded: None,
            decoration: Decoration::Header,
            edit: None,
        });
        for match_idx in 0..2 {
            rows.push(TreeRow {
                path: vec![file_idx as u16, match_idx as u16],
                indent: 1,
                icon: None,
                text: StyledText::plain(format!("  match {match_idx} in {file}")),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            });
        }
    }
    rows
}

struct TreeBody(TreeView);

impl BackendWidget for TreeBody {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        backend.draw_tree(rect, &self.0);
    }
}

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
}

pub struct SidebarDemo {
    // Variant 0: tree + reveal.
    reveal_sidebar: SidebarSystem,
    reveal_last: String,
    // Variant 1: SidebarPanel + toolbar + tasks.
    tasks: Vec<String>,
    tasks_selected: Option<usize>,
    filter_on: bool,
    tasks_last: String,
    hover: ToolbarHoverTracker,
    pressed: Option<WidgetId>,
    next_task_id: usize,
    // Variant 2: header groups.
    groups_sidebar: SidebarSystem,
    groups_last: String,
    // Variant 3: SidebarPanelBody + chrome cycle.
    body_rows: Vec<TreeRow>,
    body_scroll: usize,
    chrome_mode: ChromeMode,
    body_last: String,
}

impl SidebarDemo {
    pub fn new() -> Self {
        let mut reveal_sidebar = SidebarSystem::new(vec![SidebarSectionDef::new("items", "ITEMS")]);
        reveal_sidebar.set_rows(0, flat_rows(REVEAL_ROW_COUNT));
        reveal_sidebar.set_navigation_mode(NavigationMode::Selection);
        reveal_sidebar.set_active_section(Some(0));

        let mut groups_sidebar =
            SidebarSystem::new(vec![SidebarSectionDef::new("results", "RESULTS")]);
        groups_sidebar.set_rows(0, grouped_rows());
        groups_sidebar.set_navigation_mode(NavigationMode::Selection);
        groups_sidebar.set_active_section(Some(0));

        Self {
            reveal_sidebar,
            reveal_last: "—".into(),
            tasks: vec![
                "Review PR #257".into(),
                "Write toolbar tests".into(),
                "Land sidebar panel".into(),
            ],
            tasks_selected: Some(0),
            filter_on: false,
            tasks_last: "click toolbar buttons, then list rows".into(),
            hover: ToolbarHoverTracker::new(),
            pressed: None,
            next_task_id: 4,
            groups_sidebar,
            groups_last: "—".into(),
            body_rows: flat_rows(BODY_ROW_COUNT),
            body_scroll: 0,
            chrome_mode: ChromeMode::HeaderAndSearch,
            body_last: "—".into(),
        }
    }

    // ── Variant 1 helpers ────────────────────────────────────────────

    fn toolbar(&self) -> Toolbar {
        Toolbar::new(WidgetId::new("gallery:sidebar:toolbar")).with_buttons(vec![
            ToolbarButton::Action {
                id: WidgetId::new("gallery:sidebar:add"),
                label: "".into(),
                icon: Some("+".into()),
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: "Add task".into(),
            },
            ToolbarButton::Separator,
            ToolbarButton::Action {
                id: WidgetId::new("gallery:sidebar:filter"),
                label: "Filter".into(),
                icon: Some("⚙".into()),
                key_hint: None,
                enabled: true,
                is_active: self.filter_on,
                tooltip: "Toggle filter".into(),
            },
            ToolbarButton::Action {
                id: WidgetId::new("gallery:sidebar:clear"),
                label: "Clear".into(),
                icon: None,
                key_hint: None,
                enabled: !self.tasks.is_empty(),
                is_active: false,
                tooltip: "Remove all tasks".into(),
            },
        ])
    }

    fn panel(&self) -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("gallery:sidebar:panel"),
            toolbar: Some(self.toolbar()),
            toolbar_height: Some(2.0),
        }
    }

    fn dispatch_toolbar(&mut self, id: &WidgetId) {
        match id.as_str() {
            "gallery:sidebar:add" => {
                self.tasks.push(format!("Task #{}", self.next_task_id));
                self.next_task_id += 1;
                self.tasks_last = "Added a task".into();
            }
            "gallery:sidebar:filter" => {
                self.filter_on = !self.filter_on;
                self.tasks_last = format!("Filter {}", if self.filter_on { "on" } else { "off" });
            }
            "gallery:sidebar:clear" => {
                self.tasks.clear();
                self.tasks_selected = None;
                self.tasks_last = "Cleared all tasks".into();
            }
            _ => {}
        }
    }

    // ── Variant 3 helpers ────────────────────────────────────────────

    fn body_chrome(&self) -> SidebarPanelChrome {
        match self.chrome_mode {
            ChromeMode::None => SidebarPanelChrome::None,
            ChromeMode::Header => SidebarPanelChrome::Header("ITEMS".into()),
            ChromeMode::HeaderAndSearch => SidebarPanelChrome::HeaderAndSearch {
                header: "ITEMS".into(),
                query: String::new(),
                placeholder: "Filter items".into(),
                active: false,
            },
        }
    }
}

impl Default for SidebarDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for SidebarDemo {
    fn name(&self) -> &'static str {
        "Sidebar"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &[
            "Tree + reveal",
            "Toolbar header",
            "Header groups",
            "Body chrome",
        ]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        match variant {
            0 => {
                let sidebar_rect =
                    Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                self.reveal_sidebar.render(backend, sidebar_rect);
                let hint = Rect::new(area.x, area.y + sidebar_rect.height, area.width, lh);
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:reveal-hint"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" last: {} ", self.reveal_last),
                        fg: Color::rgb(220, 220, 220),
                        bg: Color::rgb(40, 40, 60),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![StatusBarSegment {
                        text: " ↑↓ select · z collapse · g reveal ".into(),
                        fg: Color::rgb(220, 220, 220),
                        bg: Color::rgb(40, 40, 60),
                        bold: false,
                        action_id: None,
                    }],
                };
                let _ = backend.draw_status_bar_interactive(hint, &bar, &InteractionState::new());
            }
            1 => {
                let panel_rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                let layout = backend.draw_sidebar_panel_interactive(
                    panel_rect,
                    &self.panel(),
                    &InteractionState::from_parts(self.hover.current(), self.pressed.clone()),
                );
                let content = layout.content_bounds;
                let header_text = if self.filter_on {
                    "  Tasks (filtered)  "
                } else {
                    "  Tasks  "
                };
                let _ = backend.draw_status_bar_interactive(
                    Rect::new(content.x, content.y, content.width, lh),
                    &StatusBar {
                        id: WidgetId::new("gallery:sidebar:tasks-header"),
                        left_segments: vec![StatusBarSegment {
                            text: header_text.into(),
                            fg: Color::rgb(200, 200, 200),
                            bg: Color::rgb(20, 20, 20),
                            bold: true,
                            action_id: None,
                        }],
                        right_segments: vec![],
                    },
                    &InteractionState::new(),
                );
                for (i, task) in self.tasks.iter().enumerate() {
                    let row_y = content.y + lh * (1.0 + i as f32);
                    if row_y + lh > content.y + content.height {
                        break;
                    }
                    let (fg, bg) = if Some(i) == self.tasks_selected {
                        (Color::rgb(255, 255, 255), Color::rgb(60, 100, 160))
                    } else {
                        (Color::rgb(220, 220, 220), Color::rgb(20, 20, 20))
                    };
                    let _ = backend.draw_status_bar_interactive(
                        Rect::new(content.x, row_y, content.width, lh),
                        &StatusBar {
                            id: WidgetId::new(format!("gallery:sidebar:task-row:{i}")),
                            left_segments: vec![StatusBarSegment {
                                text: format!("  {task}  "),
                                fg,
                                bg,
                                bold: false,
                                action_id: None,
                            }],
                            right_segments: vec![],
                        },
                        &InteractionState::new(),
                    );
                }
                let hint = Rect::new(area.x, area.y + panel_rect.height, area.width, lh);
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:tasks-hint"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" {} ", self.tasks_last),
                        fg: Color::rgb(220, 220, 220),
                        bg: Color::rgb(40, 40, 60),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                };
                let _ = backend.draw_status_bar_interactive(hint, &bar, &InteractionState::new());
            }
            2 => {
                let sidebar_rect =
                    Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                self.groups_sidebar.render(backend, sidebar_rect);
                let hint = Rect::new(area.x, area.y + sidebar_rect.height, area.width, lh);
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:groups-hint"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" last: {} ", self.groups_last),
                        fg: Color::rgb(220, 220, 220),
                        bg: Color::rgb(40, 40, 60),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                };
                let _ = backend.draw_status_bar_interactive(hint, &bar, &InteractionState::new());
            }
            _ => {
                let panel_rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                let panel = SidebarPanelBody {
                    background: Some(Color::rgb(24, 24, 28)),
                    chrome: self.body_chrome(),
                    scrollbar_gutter: Some(backend.char_width().max(1.0)),
                };
                let tree = TreeView {
                    id: WidgetId::new("gallery:sidebar:body-tree"),
                    rows: self.body_rows.clone(),
                    selection_mode: quadraui::SelectionMode::Single,
                    selected_path: None,
                    scroll_offset: self.body_scroll,
                    style: TreeStyle::default(),
                    has_focus: true,
                };
                let layout = panel.render(backend, panel_rect, &TreeBody(tree));
                if let Some(sb_rect) = layout.scrollbar_rect {
                    let line_h = lh.max(1.0);
                    let visible = (sb_rect.height / line_h).max(1.0);
                    let sb = Scrollbar::vertical(
                        WidgetId::new("gallery:sidebar:body-scrollbar"),
                        sb_rect,
                        self.body_scroll as f32,
                        self.body_rows.len() as f32,
                        visible,
                        line_h,
                    );
                    backend.draw_scrollbar(sb_rect, &sb);
                }
                let hint = Rect::new(area.x, area.y + panel_rect.height, area.width, lh);
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:body-hint"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" chrome={:?} last: {} ", self.chrome_mode, self.body_last),
                        fg: Color::rgb(220, 220, 220),
                        bg: Color::rgb(40, 40, 60),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![StatusBarSegment {
                        text: " ↑↓ scroll · c cycle chrome ".into(),
                        fg: Color::rgb(220, 220, 220),
                        bg: Color::rgb(40, 40, 60),
                        bold: false,
                        action_id: None,
                    }],
                };
                let _ = backend.draw_status_bar_interactive(hint, &bar, &InteractionState::new());
            }
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let lh = backend.line_height();
        match variant {
            0 => {
                self.reveal_sidebar
                    .set_backend_info(backend.line_height(), backend.msv_metrics());
                let rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                match self.reveal_sidebar.handle(event, backend, rect) {
                    SidebarEvent::RowSelected { section, path } => {
                        self.reveal_last = format!("sel→{section} {path:?}");
                        Reaction::Redraw
                    }
                    SidebarEvent::StateChanged
                    | SidebarEvent::Consumed
                    | SidebarEvent::ScrollChanged { .. } => Reaction::Redraw,
                    SidebarEvent::Ignored => match event {
                        UiEvent::KeyPressed {
                            key: quadraui::Key::Char('z'),
                            ..
                        } => {
                            let collapsed = !self.reveal_sidebar.is_collapsed(0);
                            self.reveal_sidebar.set_collapsed(0, collapsed);
                            self.reveal_last =
                                format!("{}→0", if collapsed { "collapsed" } else { "expanded" });
                            Reaction::Redraw
                        }
                        UiEvent::KeyPressed {
                            key: quadraui::Key::Char('g'),
                            ..
                        } => {
                            let target = vec![(REVEAL_ROW_COUNT - 1) as u16];
                            self.reveal_sidebar.reveal(0, &target, rect);
                            self.reveal_last = format!("reveal→0 {target:?}");
                            Reaction::Redraw
                        }
                        _ => Reaction::Continue,
                    },
                    _ => Reaction::Redraw,
                }
            }
            1 => match event {
                UiEvent::MouseMoved { position, .. } => {
                    let panel_rect =
                        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                    let layout = backend.sidebar_panel_layout(panel_rect, &self.panel());
                    let mut new_hover = self.hover.clone();
                    if let Some(tlayout) = &layout.toolbar_layout {
                        new_hover.update(tlayout, position.x, position.y);
                    } else {
                        new_hover.clear();
                    }
                    if new_hover.current() != self.hover.current() {
                        self.hover = new_hover;
                        Reaction::Redraw
                    } else {
                        Reaction::Continue
                    }
                }
                UiEvent::MouseDown { position, .. } => {
                    let panel_rect =
                        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                    let layout = backend.sidebar_panel_layout(panel_rect, &self.panel());
                    match layout.hit_test(position.x, position.y) {
                        SidebarPanelHit::ToolbarButton(id) => {
                            self.pressed = Some(id);
                            Reaction::Redraw
                        }
                        SidebarPanelHit::Content { y, .. } => {
                            if y >= lh {
                                let idx = ((y - lh) / lh).floor() as usize;
                                if idx < self.tasks.len() {
                                    self.tasks_selected = Some(idx);
                                    self.tasks_last = format!("Selected: {}", self.tasks[idx]);
                                    return Reaction::Redraw;
                                }
                            }
                            Reaction::Continue
                        }
                        _ => Reaction::Continue,
                    }
                }
                UiEvent::MouseUp { position, .. } => {
                    let panel_rect =
                        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                    let layout = backend.sidebar_panel_layout(panel_rect, &self.panel());
                    let pressed = self.pressed.take();
                    if let (Some(pressed_id), SidebarPanelHit::ToolbarButton(release_id)) =
                        (pressed, layout.hit_test(position.x, position.y))
                    {
                        if pressed_id == release_id {
                            self.dispatch_toolbar(&release_id);
                        }
                        return Reaction::Redraw;
                    }
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            2 => {
                let rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                match self.groups_sidebar.handle(event, backend, rect) {
                    SidebarEvent::RowSelected { section, path } => {
                        self.groups_last = format!("sel→{section} {path:?}");
                        Reaction::Redraw
                    }
                    SidebarEvent::HeaderActivated { section } => {
                        self.groups_last = format!("header→{section}");
                        Reaction::Redraw
                    }
                    SidebarEvent::StateChanged
                    | SidebarEvent::Consumed
                    | SidebarEvent::ScrollChanged { .. } => Reaction::Redraw,
                    SidebarEvent::Ignored => Reaction::Continue,
                    _ => Reaction::Redraw,
                }
            }
            _ => match event {
                UiEvent::KeyPressed {
                    key: quadraui::Key::Char('c'),
                    ..
                } => {
                    self.chrome_mode = self.chrome_mode.next();
                    self.body_last = format!("chrome→{:?}", self.chrome_mode);
                    Reaction::Redraw
                }
                UiEvent::KeyPressed {
                    key: quadraui::Key::Named(quadraui::NamedKey::Up),
                    ..
                } => {
                    self.body_scroll = self.body_scroll.saturating_sub(1);
                    Reaction::Redraw
                }
                UiEvent::KeyPressed {
                    key: quadraui::Key::Named(quadraui::NamedKey::Down),
                    ..
                } => {
                    let max = self.body_rows.len().saturating_sub(1);
                    self.body_scroll = (self.body_scroll + 1).min(max);
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        match variant {
            0 => serde_json::json!({ "last": self.reveal_last, "row_count": REVEAL_ROW_COUNT }),
            1 => serde_json::json!({ "tasks": self.tasks, "selected": self.tasks_selected }),
            2 => serde_json::json!({ "last": self.groups_last }),
            _ => serde_json::json!({
                "chrome_mode": format!("{:?}", self.chrome_mode),
                "row_count": self.body_rows.len(),
            }),
        }
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_toolbar_adds_a_task() {
        let mut demo = SidebarDemo::new();
        let before = demo.tasks.len();
        demo.dispatch_toolbar(&WidgetId::new("gallery:sidebar:add"));
        assert_eq!(demo.tasks.len(), before + 1);
    }

    #[test]
    fn chrome_mode_cycles_through_all_three() {
        let mut demo = SidebarDemo::new();
        let start = demo.chrome_mode;
        demo.chrome_mode = demo.chrome_mode.next();
        demo.chrome_mode = demo.chrome_mode.next();
        demo.chrome_mode = demo.chrome_mode.next();
        assert_eq!(demo.chrome_mode, start);
    }
}
