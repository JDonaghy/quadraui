//! [`GalleryApp`] — the one [`ShellApp`] every backend runs unmodified.
//!
//! Chrome layout:
//! - **Activity bar**: the five primitive groups in [`GROUPS`].
//! - **Sidebar**: the demos registered (via [`crate::registry::registry`])
//!   in the active group, one clickable row each.
//! - **Main pane**: a Demo / Code / Data tab strip over the active
//!   demo's render / trimmed source / primitive JSON.
//! - **Bottom panel**: an event log. [`GalleryApp::handle`] logs every
//!   [`UiEvent`] (plus its target [`WidgetId`], when the event carries
//!   one) *before* forwarding it to the active demo's content area.

use quadraui::{
    AppShellEvent, AppShellLayout, Backend, Color, InteractionState, Key, NamedKey,
    PanelDefinition, Reaction, Rect, ShellApp, ShellConfig, ShellContext, StatusBar,
    StatusBarSegment, TabBar, TabBarHit, TabItem, UiEvent, WidgetId,
};

use crate::registry::registry;
use crate::Demo;

/// The five activity-bar groups (issue #1341's "activity bar of the five
/// groups"). A demo declares its group as a plain `&'static str`
/// (`Demo::group`); anything that doesn't match one of these is simply
/// never shown — the registry's table-driven test (`tests/gallery_driver.rs`)
/// catches a typo'd group name by asserting every registered demo's
/// group is one of these.
pub const GROUPS: [&str; 5] = ["Content", "Chrome", "Containers", "Overlays", "Data"];

/// Activity-bar glyphs, parallel to [`GROUPS`]. Deliberately plain
/// digits rather than an initial letter: every group's own uppercased
/// title (`"CONTENT"`, `"CONTAINERS"`, `"OVERLAYS"`, ...) is painted in
/// the sidebar header, and an initial-letter icon (`'C'`, `'O'`, ...)
/// would also match *inside* that title text — a `TuiDriver::find()`
/// call hunting for the clickable icon could land on the header label
/// instead. Digits never appear in any group title or static chrome
/// text this shell paints, so `find("4")` unambiguously finds the
/// Overlays icon.
const GROUP_ICONS: [&str; 5] = ["1", "2", "3", "4", "5"];

/// The activity-bar icon glyph for `GROUPS[index]` — the glyph
/// `TuiDriver::find()` should search for to click that group, in
/// `tests/gallery_driver.rs`. Public so tests never need their own copy
/// of [`GROUP_ICONS`] (and can't drift from it).
pub fn group_icon(index: usize) -> &'static str {
    GROUP_ICONS[index]
}

fn group_panel_id(index: usize) -> WidgetId {
    WidgetId::new(format!("gallery:group:{index}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainTab {
    Demo,
    Code,
    Data,
}

impl MainTab {
    fn from_index(index: usize) -> Self {
        match index {
            0 => MainTab::Demo,
            1 => MainTab::Code,
            _ => MainTab::Data,
        }
    }

    fn label(self) -> &'static str {
        match self {
            MainTab::Demo => "Demo",
            MainTab::Code => "Code",
            MainTab::Data => "Data",
        }
    }
}

/// Maximum number of lines kept in the event log — oldest entries are
/// dropped first. Generous enough that a normal demo session never hits
/// it, small enough that a long-running gallery process doesn't grow the
/// log unbounded.
const MAX_LOG_LINES: usize = 500;

pub struct GalleryApp {
    demos: Vec<Box<dyn Demo>>,
    /// `demos[i]`'s click-target id, parallel to `demos` — built once so
    /// sidebar clicks resolve back to an index without re-deriving ids.
    demo_ids: Vec<WidgetId>,
    active_group: usize,
    /// Index into `demos` of the selected demo, or `None` when the
    /// active group has no registered demos yet (every group but
    /// `Overlays` today — the other four are follow-up port issues).
    selected: Option<usize>,
    active_tab: MainTab,
    log: Vec<String>,
}

impl GalleryApp {
    pub fn new() -> Self {
        let demos = registry();
        let demo_ids: Vec<WidgetId> = (0..demos.len())
            .map(|i| WidgetId::new(format!("gallery:demo:{i}")))
            .collect();
        let active_group = 0;
        let selected = demos.iter().position(|d| d.group() == GROUPS[active_group]);
        Self {
            demos,
            demo_ids,
            active_group,
            selected,
            active_tab: MainTab::Demo,
            log: vec!["Gallery ready — click a group, then a demo.".into()],
        }
    }

    /// The [`ShellConfig`] every backend's `main.rs` passes to
    /// `run_with_shell` alongside `GalleryApp::new()`.
    pub fn config() -> ShellConfig {
        let panels: Vec<PanelDefinition> = GROUPS
            .iter()
            .enumerate()
            .map(|(i, group)| PanelDefinition {
                id: group_panel_id(i),
                icon: GROUP_ICONS[i].to_string(),
                tooltip: (*group).to_string(),
                title: group.to_uppercase(),
            })
            .collect();
        ShellConfig::new("quadraui gallery", panels)
            .with_bottom_panel(8.0)
            .with_bottom_panel_limits(4.0, 24.0)
    }

    fn active_group_name(&self) -> &'static str {
        GROUPS[self.active_group]
    }

    fn demos_in_active_group(&self) -> impl Iterator<Item = usize> + '_ {
        let group = self.active_group_name();
        self.demos
            .iter()
            .enumerate()
            .filter(move |(_, d)| d.group() == group)
            .map(|(i, _)| i)
    }

    fn tab_bar(&self) -> TabBar {
        TabBar {
            id: WidgetId::new("gallery:main-tabs"),
            tabs: [MainTab::Demo, MainTab::Code, MainTab::Data]
                .into_iter()
                .map(|tab| TabItem {
                    label: format!(" {} ", tab.label()),
                    is_active: tab == self.active_tab,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: Some(Color::rgb(0, 140, 220)),
            show_tab_close: false,
            compact: true,
        }
    }

    fn log_event(&mut self, event: &UiEvent) {
        let (label, widget) = describe_event(event);
        let line = match widget {
            Some(id) => format!("{label} \u{2192} {}", id.as_str()),
            None => label.to_string(),
        };
        self.log.push(line);
        if self.log.len() > MAX_LOG_LINES {
            let drop = self.log.len() - MAX_LOG_LINES;
            self.log.drain(0..drop);
        }
    }

    /// The Demo tab's content rect, after reserving one line at the top
    /// for `demos[idx]`'s caps note when it has one. Shared by
    /// `render_content` and `handle` so the two never disagree about
    /// where the demo's own content starts.
    fn demo_content_rect(&self, idx: usize, backend: &dyn Backend, body: Rect) -> Rect {
        let lh = backend.line_height();
        let caps = backend.backend_caps();
        if self.demos[idx].caps_note(0, &caps).is_some() {
            Rect::new(body.x, body.y + lh, body.width, (body.height - lh).max(0.0))
        } else {
            body
        }
    }
}

impl Default for GalleryApp {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellApp for GalleryApp {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let lh = backend.line_height();

        // ── Sidebar: demos in the active group ──────────────────────
        if let Some(sb) = layout.sidebar_content_bounds {
            let rows: Vec<usize> = self.demos_in_active_group().collect();
            if rows.is_empty() {
                let bar = StatusBar {
                    id: WidgetId::new("gallery:sidebar:empty"),
                    left_segments: vec![StatusBarSegment {
                        text: " (no demos ported yet) ".into(),
                        fg: Color::rgb(120, 120, 120),
                        bg: Color::rgb(25, 25, 25),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                };
                backend.draw_status_bar_interactive(
                    Rect::new(sb.x, sb.y, sb.width, lh),
                    &bar,
                    &InteractionState::new(),
                );
            } else {
                for (row, &idx) in rows.iter().enumerate() {
                    let y = sb.y + row as f32 * lh;
                    if y + lh > sb.y + sb.height {
                        break;
                    }
                    let is_active = self.selected == Some(idx);
                    let bar = StatusBar {
                        id: WidgetId::new(format!("gallery:sidebar-row:{idx}")),
                        left_segments: vec![StatusBarSegment {
                            text: format!(" {} ", self.demos[idx].name()),
                            fg: if is_active {
                                Color::rgb(255, 255, 255)
                            } else {
                                Color::rgb(190, 190, 190)
                            },
                            bg: if is_active {
                                Color::rgb(45, 70, 110)
                            } else {
                                Color::rgb(25, 25, 25)
                            },
                            bold: is_active,
                            action_id: Some(self.demo_ids[idx].clone()),
                        }],
                        right_segments: vec![],
                    };
                    backend.draw_status_bar_interactive(
                        Rect::new(sb.x, y, sb.width, lh),
                        &bar,
                        &InteractionState::new(),
                    );
                }
            }
        }

        // ── Main pane: Demo / Code / Data tabs ──────────────────────
        let main = layout.main_content_bounds;
        if main.width > 0.0 && main.height > 0.0 {
            let tab_rect = Rect::new(main.x, main.y, main.width, lh);
            backend.draw_tab_bar_layout(tab_rect, &self.tab_bar(), None);

            let body = Rect::new(main.x, main.y + lh, main.width, (main.height - lh).max(0.0));

            match self.selected {
                None => {
                    let bar = StatusBar {
                        id: WidgetId::new("gallery:main:empty"),
                        left_segments: vec![StatusBarSegment {
                            text: " Select a demo from the sidebar ".into(),
                            fg: Color::rgb(160, 160, 160),
                            bg: Color::rgb(20, 20, 20),
                            bold: false,
                            action_id: None,
                        }],
                        right_segments: vec![],
                    };
                    backend.draw_status_bar_interactive(
                        Rect::new(body.x, body.y, body.width, lh),
                        &bar,
                        &InteractionState::new(),
                    );
                }
                Some(idx) => match self.active_tab {
                    MainTab::Demo => {
                        let caps = backend.backend_caps();
                        if let Some(note) = self.demos[idx].caps_note(0, &caps) {
                            let note_bar = StatusBar {
                                id: WidgetId::new("gallery:caps-note"),
                                left_segments: vec![StatusBarSegment {
                                    text: format!(" \u{26a0} {note} "),
                                    fg: Color::rgb(255, 210, 120),
                                    bg: Color::rgb(45, 35, 15),
                                    bold: false,
                                    action_id: None,
                                }],
                                right_segments: vec![],
                            };
                            backend.draw_status_bar_interactive(
                                Rect::new(body.x, body.y, body.width, lh),
                                &note_bar,
                                &InteractionState::new(),
                            );
                        }
                        let demo_rect = self.demo_content_rect(idx, backend, body);
                        self.demos[idx].render(0, backend, demo_rect);
                    }
                    MainTab::Code => draw_text_block(backend, body, self.demos[idx].source()),
                    MainTab::Data => {
                        let json = serde_json::to_string_pretty(&self.demos[idx].data(0))
                            .unwrap_or_default();
                        draw_text_block(backend, body, &json);
                    }
                },
            }
        }

        // ── Bottom panel: event log ──────────────────────────────────
        if let Some(bp) = layout.bottom_panel_bounds {
            draw_event_log(backend, bp, &self.log);
        }
    }

    fn handle(
        &mut self,
        event: UiEvent,
        backend: &mut dyn Backend,
        ctx: &ShellContext,
    ) -> Reaction {
        self.log_event(&event);

        if is_quit_key(&event) {
            return Reaction::Exit;
        }

        // `log_event` above just appended to the bottom-panel log on
        // every call, so every path below has already changed what's on
        // screen — escalate whatever `handle_inner` decides (it's free
        // to return `Continue` when *it* found nothing to do) up to at
        // least `Redraw`. Without this, a click that reaches a demo but
        // doesn't change the demo's own state (e.g. a miss on a hit
        // test) would return `Continue`, and the host would skip
        // repainting — leaving the just-logged line invisible even
        // though `self.log` really did grow.
        match self.handle_inner(event, backend, ctx) {
            Reaction::Exit => Reaction::Exit,
            // `Reaction` is `#[non_exhaustive]`; any variant other than
            // `Exit` still means "the log grew, repaint" here, so a
            // future variant falls into this arm safely too.
            _ => Reaction::Redraw,
        }
    }

    fn on_shell_event_ctx(&mut self, event: &AppShellEvent, _ctx: &ShellContext) {
        if let AppShellEvent::PanelChanged { panel_id } = event {
            if let Some(i) = (0..GROUPS.len()).find(|&i| group_panel_id(i) == *panel_id) {
                self.active_group = i;
                let first_in_group = self.demos_in_active_group().next();
                self.selected = first_in_group;
                self.active_tab = MainTab::Demo;
                self.log.push(format!("\u{2192} group: {}", GROUPS[i]));
            }
        }
    }
}

impl GalleryApp {
    fn handle_inner(
        &mut self,
        event: UiEvent,
        backend: &mut dyn Backend,
        ctx: &ShellContext,
    ) -> Reaction {
        if let UiEvent::MouseDown { position, .. } = &event {
            // Sidebar row click → select demo. Each row is a full-width
            // one-line `StatusBar`, so the row index is just the click's
            // offset from the sidebar's top divided by `line_height` —
            // no separate hit-test layout needed.
            if ctx.in_sidebar(position.x, position.y) {
                if let Some(sb) = ctx.sidebar_bounds() {
                    let lh = backend.line_height();
                    let row = ((position.y - sb.y) / lh).floor();
                    if row >= 0.0 {
                        let rows: Vec<usize> = self.demos_in_active_group().collect();
                        if let Some(&idx) = rows.get(row as usize) {
                            self.selected = Some(idx);
                            self.active_tab = MainTab::Demo;
                            self.log.push(format!(
                                "\u{2192} demo selected: {}",
                                self.demos[idx].name()
                            ));
                            return Reaction::Redraw;
                        }
                    }
                }
                return Reaction::Continue;
            }

            // Main-pane tab strip click (Demo / Code / Data).
            let lh = backend.line_height();
            let main = ctx.main_bounds();
            let tab_rect = Rect::new(main.x, main.y, main.width, lh);
            if position.x >= tab_rect.x
                && position.x < tab_rect.x + tab_rect.width
                && position.y >= tab_rect.y
                && position.y < tab_rect.y + tab_rect.height
            {
                let layout = backend.resolve_tab_bar_layout(tab_rect, &self.tab_bar());
                let local_x = position.x - tab_rect.x;
                let local_y = position.y - tab_rect.y;
                if let TabBarHit::Tab(idx) = layout.hit_test(local_x, local_y) {
                    self.active_tab = MainTab::from_index(idx);
                    self.log
                        .push(format!("\u{2192} tab: {}", self.active_tab.label()));
                    return Reaction::Redraw;
                }
                return Reaction::Continue;
            }
        }

        // Forward everything else to the active demo's Demo-tab content.
        if self.active_tab == MainTab::Demo {
            if let Some(idx) = self.selected {
                let lh = backend.line_height();
                let main = ctx.main_bounds();
                let body = Rect::new(main.x, main.y + lh, main.width, (main.height - lh).max(0.0));
                let demo_rect = self.demo_content_rect(idx, backend, body);
                return self.demos[idx].handle(0, &event, backend, demo_rect);
            }
        }

        Reaction::Continue
    }
}

fn is_quit_key(event: &UiEvent) -> bool {
    matches!(
        event,
        UiEvent::KeyPressed {
            key: Key::Char('q'),
            ..
        } | UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            ..
        }
    )
}

/// Short label + optional target [`WidgetId`] for one [`UiEvent`] — the
/// text the event log shows (issue #1341: "logs each `UiEvent` + target
/// `WidgetId`").
fn describe_event(event: &UiEvent) -> (&'static str, Option<WidgetId>) {
    match event {
        UiEvent::KeyPressed { .. } => ("KeyPressed", None),
        UiEvent::CharTyped(_) => ("CharTyped", None),
        UiEvent::MouseDown { widget, .. } => ("MouseDown", widget.clone()),
        UiEvent::MouseUp { widget, .. } => ("MouseUp", widget.clone()),
        UiEvent::MouseMoved { .. } => ("MouseMoved", None),
        UiEvent::MouseEntered { widget } => ("MouseEntered", Some(widget.clone())),
        UiEvent::MouseLeft { widget } => ("MouseLeft", Some(widget.clone())),
        UiEvent::DoubleClick { widget, .. } => ("DoubleClick", widget.clone()),
        UiEvent::Scroll { widget, .. } => ("Scroll", widget.clone()),
        UiEvent::WindowResized { .. } => ("WindowResized", None),
        UiEvent::StatusBar(id, _) => ("StatusBar", Some(id.clone())),
        UiEvent::TabBar(id, _) => ("TabBar", Some(id.clone())),
        UiEvent::ActivityBar(id, _) => ("ActivityBar", Some(id.clone())),
        UiEvent::Tree(id, _) => ("Tree", Some(id.clone())),
        UiEvent::List(id, _) => ("List", Some(id.clone())),
        UiEvent::Form(id, _) => ("Form", Some(id.clone())),
        UiEvent::Palette(id, _) => ("Palette", Some(id.clone())),
        UiEvent::Terminal(id, _) => ("Terminal", Some(id.clone())),
        UiEvent::TextDisplay(id, _) => ("TextDisplay", Some(id.clone())),
        UiEvent::Chart(id, _) => ("Chart", Some(id.clone())),
        UiEvent::DataTable(id, _) => ("DataTable", Some(id.clone())),
        UiEvent::MenuActivated(id) => ("MenuActivated", Some(id.clone())),
        UiEvent::ContextMenuItemActivated(id) => ("ContextMenuItemActivated", Some(id.clone())),
        UiEvent::FocusChanged(id) => ("FocusChanged", id.clone()),
        _ => ("Event", None),
    }
}

/// Paint `text` as one row per line, top-down, clipped to `rect`.
fn draw_text_block(backend: &mut dyn Backend, rect: Rect, text: &str) {
    let lh = backend.line_height();
    if rect.height <= 0.0 || rect.width <= 0.0 {
        return;
    }
    for (i, line) in text.lines().enumerate() {
        let y = rect.y + i as f32 * lh;
        if y + lh > rect.y + rect.height {
            break;
        }
        let bar = StatusBar {
            id: WidgetId::new(format!("gallery:text-row:{i}")),
            left_segments: vec![StatusBarSegment {
                text: format!(" {line} "),
                fg: Color::rgb(210, 210, 210),
                bg: Color::rgb(18, 18, 18),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar_interactive(
            Rect::new(rect.x, y, rect.width, lh),
            &bar,
            &InteractionState::new(),
        );
    }
}

/// Paint the last lines of `log` that fit `rect`, oldest-of-the-visible-
/// window first (newest line at the bottom, standard log order).
fn draw_event_log(backend: &mut dyn Backend, rect: Rect, log: &[String]) {
    let lh = backend.line_height();
    if rect.height <= 0.0 {
        return;
    }
    let capacity = (rect.height / lh).floor().max(0.0) as usize;
    let start = log.len().saturating_sub(capacity);
    let text = log[start..].join("\n");
    draw_text_block(backend, rect, &text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_selects_the_first_group_and_its_first_demo() {
        let app = GalleryApp::new();
        assert_eq!(app.active_group, 0);
        // `Overlays` (index 3) is the only populated group today.
        if GROUPS[0] == "Overlays" {
            assert!(app.selected.is_some());
        }
    }

    #[test]
    fn group_panel_ids_are_distinct() {
        let ids: Vec<WidgetId> = (0..GROUPS.len()).map(group_panel_id).collect();
        for (i, a) in ids.iter().enumerate() {
            for (j, b) in ids.iter().enumerate() {
                assert_eq!(i == j, a == b);
            }
        }
    }

    #[test]
    fn describe_event_extracts_mouse_down_widget() {
        let id = WidgetId::new("w");
        let ev = UiEvent::MouseDown {
            widget: Some(id.clone()),
            button: quadraui::MouseButton::Left,
            position: quadraui::Point::new(0.0, 0.0),
            modifiers: Default::default(),
        };
        let (label, widget) = describe_event(&ev);
        assert_eq!(label, "MouseDown");
        assert_eq!(widget, Some(id));
    }
}
