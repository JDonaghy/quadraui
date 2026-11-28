//! `Sidebar` demo — adapted from
//! `quadraui/examples/common/sidebar_reveal_demo.rs`,
//! `sidebar_panel_app.rs`, `sidebar_search.rs` and
//! `sidebar_panel_body_demo.rs`.
//!
//! Four variants cover the sidebar-family primitives: a plain
//! [`Sidebar`] tree with programmatic `reveal`, a [`SidebarPanel`]
//! with a toolbar header above a task list, a [`Sidebar`] tree
//! with `Decoration::Header` group rows, and a
//! [`compose::sidebar_panel_body::SidebarPanelBody`] composing background +
//! chrome + a raw `TreeView` body + scrollbar.

use quadraui::compose::sidebar_panel_body::{SidebarPanelBody, SidebarPanelChrome};
use quadraui::{
    Backend, BackendCaps, BackendWidget, Canvas, Decoration, DrawOp, DrawsPrimitives, Handled,
    NavigationMode, Reaction, Rect, Scrollbar, Sidebar, SidebarAction, SidebarPanel,
    SidebarPanelHit, SidebarPanelLayout, SidebarSectionDef, SidebarState, StatusBar,
    StatusBarSegment, Style, StyledText, Toolbar, ToolbarButton, TreeRow, TreeStyle, TreeView,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("sidebar.rs");

/// `Style`'s chrome-metric `token`, floored at the chrome line height on
/// a pixel backend — never shrunk below it, so a chrome font taller than
/// the design token isn't clipped — or the plain chrome line height on
/// TUI's fixed cell grid, where every `Style` chrome metric is a
/// documented no-op. Mirrors `ShellView`'s own token-vs-line-height carve
/// (`compose::shell_layout::compute_layout`'s `header_h`/`status_h`).
fn chrome_strip_height(backend: &dyn Backend, token: impl Fn(&Style) -> f32) -> f32 {
    let lh = backend.chrome_line_height();
    if lh <= 1.0 {
        backend.snap_height(lh)
    } else {
        backend.snap_height(lh.max(token(&backend.style())))
    }
}

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
        let _ = backend.draw_dyn(rect, &self.0, &());
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
    reveal_sidebar: Sidebar,
    reveal_state: SidebarState,
    reveal_last: String,
    // Variant 1: SidebarPanel + toolbar + tasks.
    tasks: Vec<String>,
    tasks_selected: Option<usize>,
    filter_on: bool,
    tasks_last: String,
    pressed: Option<WidgetId>,
    next_task_id: usize,
    // Variant 2: header groups.
    groups_sidebar: Sidebar,
    groups_state: SidebarState,
    groups_last: String,
    // Variant 3: SidebarPanelBody + chrome cycle.
    body_rows: Vec<TreeRow>,
    body_scroll: usize,
    chrome_mode: ChromeMode,
    body_last: String,
}

impl SidebarDemo {
    pub fn new() -> Self {
        let mut reveal_sidebar = Sidebar::from_defs(
            WidgetId::new("gallery:sidebar:reveal"),
            &[SidebarSectionDef::new("items", "ITEMS")],
        );
        reveal_sidebar.set_rows(0, flat_rows(REVEAL_ROW_COUNT));
        reveal_sidebar.navigation_mode = NavigationMode::Selection;
        reveal_sidebar.active_section = Some(0);

        let mut groups_sidebar = Sidebar::from_defs(
            WidgetId::new("gallery:sidebar:groups"),
            &[SidebarSectionDef::new("results", "RESULTS")],
        );
        groups_sidebar.set_rows(0, grouped_rows());
        groups_sidebar.navigation_mode = NavigationMode::Selection;
        groups_sidebar.active_section = Some(0);

        Self {
            reveal_sidebar,
            reveal_state: SidebarState::default(),
            reveal_last: "—".into(),
            tasks: vec![
                "Review PR #257".into(),
                "Write toolbar tests".into(),
                "Land sidebar panel".into(),
            ],
            tasks_selected: Some(0),
            filter_on: false,
            tasks_last: "click toolbar buttons, then list rows".into(),
            pressed: None,
            next_task_id: 4,
            groups_sidebar,
            groups_state: SidebarState::default(),
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
                spec: None,
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
                spec: None,
                id: WidgetId::new("gallery:sidebar:filter"),
                label: "Filter".into(),
                icon: Some("⚙".into()),
                key_hint: None,
                enabled: true,
                is_active: self.filter_on,
                tooltip: "Toggle filter".into(),
            },
            ToolbarButton::Action {
                spec: None,
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
        let lh = chrome_strip_height(backend, |s| s.status_bar_height);
        let theme = backend.theme();
        match variant {
            0 => {
                let sidebar_rect =
                    Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                let _ = backend.draw_dyn(sidebar_rect, &self.reveal_sidebar, &());
                let hint = Rect::new(area.x, area.y + sidebar_rect.height, area.width, lh);
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:reveal-hint"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" last: {} ", self.reveal_last),
                        fg: theme.muted_fg,
                        bg: theme.header_bg,
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![StatusBarSegment {
                        text: " ↑↓ select · z collapse · g reveal ".into(),
                        fg: theme.muted_fg,
                        bg: theme.header_bg,
                        bold: false,
                        action_id: None,
                    }],
                };
                let _ = backend.draw_dyn(hint, &bar, &());
            }
            1 => {
                let panel_rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                let layout = backend
                    .draw_dyn(panel_rect, &self.panel(), &())
                    .downcast_ref::<SidebarPanelLayout>()
                    .cloned()
                    .expect("a SidebarPanel resolves a SidebarPanelLayout");
                let content = layout.content_bounds;
                // `SidebarPanel` paints its toolbar slot but deliberately
                // leaves `content_bounds` to the host (see that
                // primitive's own doc) — fill it with the surface colour
                // first, so the window's base clear colour (a stray
                // light-grey strip under `Theme::light_default()`) never
                // shows through below the last task row.
                let backdrop = Canvas {
                    id: WidgetId::new("sidebar:backdrop"),
                    ops: vec![DrawOp::Rect {
                        rect: Rect::new(0.0, 0.0, content.width, content.height),
                        color: theme.surface_bg,
                    }],
                    regions: Vec::new(),
                    focusable: false,
                };
                let _ = backend.draw_dyn(content, &backdrop, &());
                let header_text = if self.filter_on {
                    "  Tasks (filtered)  "
                } else {
                    "  Tasks  "
                };
                let _ = backend.draw_dyn(
                    Rect::new(content.x, content.y, content.width, lh),
                    &StatusBar {
                        id: WidgetId::new("gallery:sidebar:tasks-header"),
                        left_segments: vec![StatusBarSegment {
                            text: header_text.into(),
                            fg: theme.header_fg,
                            bg: theme.header_bg,
                            bold: true,
                            action_id: None,
                        }],
                        right_segments: vec![],
                    },
                    &(),
                );
                for (i, task) in self.tasks.iter().enumerate() {
                    let row_y = content.y + lh * (1.0 + i as f32);
                    if row_y + lh > content.y + content.height {
                        break;
                    }
                    let (fg, bg) = if Some(i) == self.tasks_selected {
                        (theme.header_fg, theme.selected_bg)
                    } else {
                        (theme.surface_fg, theme.surface_bg)
                    };
                    let _ = backend.draw_dyn(
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
                        &(),
                    );
                }
                let hint = Rect::new(area.x, area.y + panel_rect.height, area.width, lh);
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:tasks-hint"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" {} ", self.tasks_last),
                        fg: theme.muted_fg,
                        bg: theme.header_bg,
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                };
                let _ = backend.draw_dyn(hint, &bar, &());
            }
            2 => {
                let sidebar_rect =
                    Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                let _ = backend.draw_dyn(sidebar_rect, &self.groups_sidebar, &());
                let hint = Rect::new(area.x, area.y + sidebar_rect.height, area.width, lh);
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:groups-hint"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" last: {} ", self.groups_last),
                        fg: theme.muted_fg,
                        bg: theme.header_bg,
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                };
                let _ = backend.draw_dyn(hint, &bar, &());
            }
            _ => {
                let panel_rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                let panel = SidebarPanelBody {
                    background: Some(theme.surface_bg),
                    chrome: self.body_chrome(),
                    scrollbar_gutter: Some(backend.char_width().max(1.0)),
                };
                let tree = TreeView {
                    id: WidgetId::new("gallery:sidebar:body-tree"),
                    rows: self.body_rows.clone(),
                    selection_mode: quadraui::SelectionMode::Single,
                    selected_path: None,
                    scroll_offset: self.body_scroll,
                    items_window: None,
                    scroll_px: None,
                    style: TreeStyle {
                        show_scrollbar: false,
                        ..TreeStyle::default()
                    },
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
                    let _ = backend.draw_dyn(sb.track, &sb, &());
                    backend.register_zone(sb.id, sb.track);
                }
                let hint = Rect::new(area.x, area.y + panel_rect.height, area.width, lh);
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:body-hint"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" chrome={:?} last: {} ", self.chrome_mode, self.body_last),
                        fg: theme.muted_fg,
                        bg: theme.header_bg,
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![StatusBarSegment {
                        text: " ↑↓ scroll · c cycle chrome ".into(),
                        fg: theme.muted_fg,
                        bg: theme.header_bg,
                        bold: false,
                        action_id: None,
                    }],
                };
                let _ = backend.draw_dyn(hint, &bar, &());
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
        let lh = chrome_strip_height(backend, |s| s.status_bar_height);
        match variant {
            0 => {
                let rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                let handled =
                    self.reveal_sidebar
                        .handle_event(&mut self.reveal_state, event, backend, rect);
                match handled {
                    Handled::Event(actions) => {
                        for action in actions {
                            if let SidebarAction::RowSelected { section, path } = &action {
                                self.reveal_last = format!("sel→{section} {path:?}");
                            }
                            self.reveal_sidebar.apply(&action);
                        }
                        Reaction::Redraw
                    }
                    Handled::Consumed => Reaction::Redraw,
                    Handled::Ignored => match event {
                        UiEvent::KeyPressed {
                            key: quadraui::Key::Char('z'),
                            ..
                        } => {
                            let collapsed = !self.reveal_sidebar.sections[0].collapsed;
                            self.reveal_sidebar.sections[0].collapsed = collapsed;
                            self.reveal_last =
                                format!("{}→0", if collapsed { "collapsed" } else { "expanded" });
                            Reaction::Redraw
                        }
                        UiEvent::KeyPressed {
                            key: quadraui::Key::Char('g'),
                            ..
                        } => {
                            let target = vec![(REVEAL_ROW_COUNT - 1) as u16];
                            self.reveal_sidebar.reveal(0, &target, backend, rect);
                            self.reveal_last = format!("reveal→0 {target:?}");
                            Reaction::Redraw
                        }
                        _ => Reaction::Continue,
                    },
                }
            }
            1 => match event {
                UiEvent::MouseDown { position, .. } => {
                    let panel_rect =
                        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                    let layout = backend.layout_primitive(panel_rect, &self.panel());
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
                    let layout = backend.layout_primitive(panel_rect, &self.panel());
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
                let handled =
                    self.groups_sidebar
                        .handle_event(&mut self.groups_state, event, backend, rect);
                match handled {
                    Handled::Event(actions) => {
                        for action in actions {
                            match &action {
                                SidebarAction::RowSelected { section, path } => {
                                    self.groups_last = format!("sel→{section} {path:?}");
                                }
                                SidebarAction::HeaderActivated { section } => {
                                    self.groups_last = format!("header→{section}");
                                }
                                _ => {}
                            }
                            self.groups_sidebar.apply(&action);
                        }
                        Reaction::Redraw
                    }
                    Handled::Consumed => Reaction::Redraw,
                    Handled::Ignored => Reaction::Continue,
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
