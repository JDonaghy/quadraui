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

/// The five activity-bar groups. A demo declares its group as a plain `&'static str`
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
/// instead. Digits are only collision-free with every *registered demo's*
/// own rendered content by accident of timing, though — nothing stops a
/// future demo in `GROUPS[0]` from painting a `"1"` of its own once it's
/// selected by default. A test that must visit every group regardless of
/// what's registered there (`tests/gallery_driver.rs`'s registry-driven
/// test) should resolve the icon's real painted bounds via
/// `quadraui::testing::ConformanceDriver::inventory()`'s zone for
/// [`group_panel_id`] and click its center, rather than `driver.find(..)`
/// scraping for this glyph.
const GROUP_ICONS: [&str; 5] = ["1", "2", "3", "4", "5"];

/// The activity-bar icon glyph for `GROUPS[index]` — the glyph
/// `TuiDriver::find()` should search for to click that group, in
/// `tests/gallery_driver.rs`. Public so tests never need their own copy
/// of [`GROUP_ICONS`] (and can't drift from it).
pub fn group_icon(index: usize) -> &'static str {
    GROUP_ICONS[index]
}

/// The [`WidgetId`] `GROUPS[index]`'s activity-bar item is registered
/// under — both as its `PanelDefinition::id` (see [`GalleryApp::config`])
/// and as the zone id `AppShell::render` records for it, so
/// `quadraui::testing::ConformanceDriver::inventory()` reports this same
/// id's real painted bounds. Public so a test that must visit every group
/// (`tests/gallery_driver.rs`'s registry-driven test) can resolve a
/// group's real click target from its zone rather than scraping
/// [`GROUP_ICONS`]' glyph (see that constant's doc for why).
pub fn group_panel_id(index: usize) -> WidgetId {
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
    /// Index into `self.demos[selected].variants()` — the variant every
    /// `Demo` method call below addresses. Reset to `0` whenever
    /// `selected` changes, since a variant index from the previous demo
    /// has no meaning for the new one.
    active_variant: usize,
    active_tab: MainTab,
    log: Vec<String>,
}

impl GalleryApp {
    pub fn new() -> Self {
        Self::from_demos(registry())
    }

    /// Shared by [`Self::new`] (the real `registry()`) and this module's
    /// own tests (a hand-built `Vec` exercising a shape `registry()`
    /// doesn't today, e.g. a demo with more than one variant).
    fn from_demos(demos: Vec<Box<dyn Demo>>) -> Self {
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
            active_variant: 0,
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

    /// Append one line to the event log, trimming the oldest entries once
    /// `MAX_LOG_LINES` is exceeded. The single place every log mutation
    /// goes through, so the cap can never be bypassed by a call site that
    /// pushes directly.
    fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > MAX_LOG_LINES {
            let drop = self.log.len() - MAX_LOG_LINES;
            self.log.drain(0..drop);
        }
    }

    fn log_event(&mut self, event: &UiEvent) {
        let (label, widget) = describe_event(event);
        let line = match widget {
            Some(id) => format!("{label} \u{2192} {}", id.as_str()),
            None => label.to_string(),
        };
        self.push_log(line);
    }

    /// Number of chrome rows (variant picker, then caps note — in that
    /// paint order) reserved above `demos[idx]`'s own content in the Demo
    /// tab. Shared by `demo_content_rect`, `render_content` and
    /// `handle_inner` so all three agree on where the demo's own content
    /// starts and where the variant-picker row (when there is one) sits.
    fn demo_header_rows(&self, idx: usize, backend: &dyn Backend) -> usize {
        let mut rows = 0;
        if self.demos[idx].variants().len() > 1 {
            rows += 1;
        }
        let caps = backend.backend_caps();
        if self.demos[idx]
            .caps_note(self.active_variant, &caps)
            .is_some()
        {
            rows += 1;
        }
        rows
    }

    /// The Demo tab's content rect, after reserving
    /// `demo_header_rows(idx, backend)` lines at the top. Shared by
    /// `render_content` and `handle` so the two never disagree about
    /// where the demo's own content starts.
    fn demo_content_rect(&self, idx: usize, backend: &dyn Backend, body: Rect) -> Rect {
        let lh = backend.line_height();
        let top = body.y + self.demo_header_rows(idx, backend) as f32 * lh;
        Rect::new(
            body.x,
            top,
            body.width,
            (body.y + body.height - top).max(0.0),
        )
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
                        let mut row = 0usize;
                        let variants = self.demos[idx].variants();
                        if variants.len() > 1 {
                            let rect = Rect::new(body.x, body.y, body.width, lh);
                            draw_variant_picker(backend, rect, variants, self.active_variant);
                            row += 1;
                        }
                        let caps = backend.backend_caps();
                        if let Some(note) = self.demos[idx].caps_note(self.active_variant, &caps) {
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
                                Rect::new(body.x, body.y + row as f32 * lh, body.width, lh),
                                &note_bar,
                                &InteractionState::new(),
                            );
                        }
                        let demo_rect = self.demo_content_rect(idx, backend, body);
                        self.demos[idx].render(self.active_variant, backend, demo_rect);
                    }
                    MainTab::Code => {
                        draw_text_block(backend, body, "code", self.demos[idx].source())
                    }
                    MainTab::Data => {
                        let json = serde_json::to_string_pretty(
                            &self.demos[idx].data(self.active_variant),
                        )
                        .unwrap_or_default();
                        draw_text_block(backend, body, "data", &json);
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

        // Forward to `handle_inner` *before* deciding whether this is a
        // quit key: an overlay demo (Palette / Dialog / ContextMenu) needs
        // `Escape` to dismiss itself, and a demo that consumes the key
        // returns something other than `Continue` here. Only fall back to
        // quitting the gallery when nothing downstream wanted the key.
        let reaction = self.handle_inner(event.clone(), backend, ctx);

        match reaction {
            Reaction::Exit => Reaction::Exit,
            Reaction::Continue if is_quit_key(&event) => Reaction::Exit,
            // Pass the requested wake-up through unchanged — collapsing it
            // to `Redraw` would drop the demo's "nothing to paint now, but
            // check back in `Duration`" request (see
            // `quadraui::runner::Reaction::RedrawAfter`'s doc).
            r @ Reaction::RedrawAfter(_) => r,
            // `log_event` above just appended to the bottom-panel log on
            // every call, so every remaining path has already changed
            // what's on screen — escalate `Continue` (when `handle_inner`
            // found nothing else to do) up to `Redraw`. Without this, a
            // click that reaches a demo but doesn't change the demo's own
            // state (e.g. a miss on a hit test) would return `Continue`,
            // and the host would skip repainting — leaving the
            // just-logged line invisible even though `self.log` really
            // did grow. `Reaction` is `#[non_exhaustive]`; any future
            // variant not named above falls into this arm the same way.
            _ => Reaction::Redraw,
        }
    }

    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        if self.active_tab != MainTab::Demo {
            return Reaction::Continue;
        }
        match self.selected {
            Some(idx) => self.demos[idx].tick(self.active_variant, backend),
            None => Reaction::Continue,
        }
    }

    fn on_shell_event_ctx(&mut self, event: &AppShellEvent, _ctx: &ShellContext) {
        if let AppShellEvent::PanelChanged { panel_id } = event {
            if let Some(i) = (0..GROUPS.len()).find(|&i| group_panel_id(i) == *panel_id) {
                self.active_group = i;
                let first_in_group = self.demos_in_active_group().next();
                self.selected = first_in_group;
                self.active_variant = 0;
                self.active_tab = MainTab::Demo;
                self.push_log(format!("\u{2192} group: {}", GROUPS[i]));
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
                            self.active_variant = 0;
                            self.active_tab = MainTab::Demo;
                            self.push_log(format!(
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
                    self.push_log(format!("\u{2192} tab: {}", self.active_tab.label()));
                    return Reaction::Redraw;
                }
                return Reaction::Continue;
            }

            // Variant-picker row click (only present on the Demo tab, and
            // only when the selected demo has more than one variant) —
            // same equal-width column math `draw_variant_picker` painted
            // with, via the shared `variant_col_at` helper.
            if self.active_tab == MainTab::Demo {
                if let Some(idx) = self.selected {
                    let variants = self.demos[idx].variants();
                    if variants.len() > 1 {
                        let row_rect = Rect::new(main.x, main.y + lh, main.width, lh);
                        if position.y >= row_rect.y && position.y < row_rect.y + row_rect.height {
                            if let Some(col) = variant_col_at(row_rect, variants.len(), position.x)
                            {
                                self.active_variant = col;
                                self.push_log(format!("\u{2192} variant: {}", variants[col]));
                                return Reaction::Redraw;
                            }
                            return Reaction::Continue;
                        }
                    }
                }
            }
        }

        // Forward everything else to the active demo's Demo-tab content.
        // A positional mouse event outside the main pane (a click in the
        // bottom-panel event log, the title bar, or the sidebar handled
        // above) is never meant for the demo — gate on `ctx.in_main` so a
        // demo that treats any `MouseDown`/`MouseUp`/etc. as meaningful
        // doesn't react to clicks happening elsewhere on screen.
        if self.active_tab == MainTab::Demo {
            if let Some(idx) = self.selected {
                let outside_main = event_position(&event)
                    .map(|p| !ctx.in_main(p.x, p.y))
                    .unwrap_or(false);
                if !outside_main {
                    let lh = backend.line_height();
                    let main = ctx.main_bounds();
                    let body =
                        Rect::new(main.x, main.y + lh, main.width, (main.height - lh).max(0.0));
                    let demo_rect = self.demo_content_rect(idx, backend, body);
                    return self.demos[idx].handle(self.active_variant, &event, backend, demo_rect);
                }
            }
        }

        Reaction::Continue
    }
}

/// The cursor position carried by `event`, for every `UiEvent` variant
/// that has one — `None` for events with no screen position (key events,
/// window events, ...), which are never gated on `ctx.in_main`.
fn event_position(event: &UiEvent) -> Option<quadraui::Point> {
    match event {
        UiEvent::MouseDown { position, .. }
        | UiEvent::MouseUp { position, .. }
        | UiEvent::MouseMoved { position, .. }
        | UiEvent::DoubleClick { position, .. }
        | UiEvent::Scroll { position, .. } => Some(*position),
        _ => None,
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
/// text the event log shows: every `UiEvent` logged alongside its target
/// `WidgetId`, when the event carries one.
fn describe_event(event: &UiEvent) -> (&'static str, Option<WidgetId>) {
    match event {
        UiEvent::Accelerator(..) => ("Accelerator", None),
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
        UiEvent::WindowFocused(_) => ("WindowFocused", None),
        UiEvent::DpiChanged(_) => ("DpiChanged", None),
        UiEvent::WindowStateChanged { .. } => ("WindowStateChanged", None),
        UiEvent::FilesDropped { .. } => ("FilesDropped", None),
        UiEvent::ClipboardPaste(_) => ("ClipboardPaste", None),
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

/// Paint `text` as one row per line, top-down, clipped to `rect`. `prefix`
/// scopes each row's [`WidgetId`] (`gallery:text-row:{prefix}:{i}`) so two
/// calls painting in the same frame (the Code/Data body and the event
/// log both go through this) never mint the same id twice.
fn draw_text_block(backend: &mut dyn Backend, rect: Rect, prefix: &str, text: &str) {
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
            id: WidgetId::new(format!("gallery:text-row:{prefix}:{i}")),
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
    draw_text_block(backend, rect, "log", &text);
}

/// Which variant column (if any) contains `x`, for an equal-width picker
/// row of `num_variants` columns spanning `row_rect` — the inverse of
/// `draw_variant_picker`'s own column math, shared so paint and hit test
/// can never disagree on where a column boundary falls.
fn variant_col_at(row_rect: Rect, num_variants: usize, x: f32) -> Option<usize> {
    if num_variants == 0 || row_rect.width <= 0.0 {
        return None;
    }
    if x < row_rect.x || x >= row_rect.x + row_rect.width {
        return None;
    }
    let col_width = row_rect.width / num_variants as f32;
    let col = ((x - row_rect.x) / col_width).floor().max(0.0) as usize;
    if col < num_variants {
        Some(col)
    } else {
        None
    }
}

/// Paint one row of clickable variant names, the active one highlighted,
/// as `variants.len()` equal-width adjacent one-line bars (the same
/// per-row-its-own-`StatusBar` pattern the sidebar uses above). Equal
/// columns, computed the same way here and in `handle_inner`'s variant-row
/// hit test (via [`variant_col_at`]), so the two never disagree on which
/// column a click landed in.
fn draw_variant_picker(backend: &mut dyn Backend, rect: Rect, variants: &[&str], active: usize) {
    if variants.is_empty() || rect.width <= 0.0 {
        return;
    }
    let col_width = rect.width / variants.len() as f32;
    for (i, name) in variants.iter().enumerate() {
        let is_active = i == active;
        let id = WidgetId::new(format!("gallery:variant:{i}"));
        let bar = StatusBar {
            id: id.clone(),
            left_segments: vec![StatusBarSegment {
                text: format!(" {name} "),
                fg: if is_active {
                    Color::rgb(255, 255, 255)
                } else {
                    Color::rgb(170, 170, 170)
                },
                bg: if is_active {
                    Color::rgb(45, 70, 110)
                } else {
                    Color::rgb(30, 30, 30)
                },
                bold: is_active,
                action_id: Some(id),
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar_interactive(
            Rect::new(
                rect.x + i as f32 * col_width,
                rect.y,
                col_width,
                rect.height,
            ),
            &bar,
            &InteractionState::new(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_selects_the_first_group_and_its_first_demo() {
        let app = GalleryApp::new();
        assert_eq!(app.active_group, 0);
        let expected = registry().iter().position(|d| d.group() == GROUPS[0]);
        assert_eq!(
            app.selected, expected,
            "GalleryApp::new should select the first registered demo whose \
             group is GROUPS[0], or None when that group has no demos yet"
        );
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

    /// Fake two-variant demo — `registry()` has no such demo today (the
    /// seed `ToastDemo` has exactly one), so this is what exercises the
    /// variant-picker row's render path at all.
    struct TwoVariantDemo;

    impl Demo for TwoVariantDemo {
        fn name(&self) -> &'static str {
            "Two"
        }

        fn group(&self) -> &'static str {
            GROUPS[0]
        }

        fn variants(&self) -> &'static [&'static str] {
            &["a", "b"]
        }

        fn render(&self, _variant: usize, _backend: &mut dyn Backend, _area: Rect) {}

        fn handle(
            &mut self,
            _variant: usize,
            _event: &UiEvent,
            _backend: &mut dyn Backend,
            _area: Rect,
        ) -> quadraui::Reaction {
            quadraui::Reaction::Continue
        }

        fn source(&self) -> &'static str {
            "fn two_variant_demo() {}"
        }

        fn data(&self, _variant: usize) -> serde_json::Value {
            serde_json::Value::Null
        }
    }

    #[test]
    fn render_content_paints_a_variant_picker_row_for_a_multi_variant_demo() {
        let app = GalleryApp::from_demos(vec![Box::new(TwoVariantDemo)]);
        assert_eq!(app.selected, Some(0));

        let mut backend = quadraui::testing::RecordingBackend::new();
        let layout = AppShellLayout {
            window_bounds: Rect::new(0.0, 0.0, 80.0, 24.0),
            title_bar_bounds: None,
            activity_bar_bounds: Rect::new(0.0, 0.0, 3.0, 24.0),
            sidebar_header_bounds: None,
            sidebar_content_bounds: None,
            divider_bounds: None,
            main_content_bounds: Rect::new(3.0, 0.0, 77.0, 20.0),
            bottom_panel_bounds: None,
            command_line_bounds: None,
            status_bar_bounds: None,
        };

        // Must not panic, and must actually paint something for the
        // picker row (the two variant columns) plus the demo's own empty
        // body — `draw_status_bar_interactive` (recorded as
        // `"draw_status_bar"`, see `RecordingBackend`'s impl) is the one
        // call both go through. At least 2 calls: one per variant column
        // (`draw_variant_picker` loops `variants.len()` times).
        app.render_content(&mut backend, &layout);
        let status_bar_calls = backend
            .calls
            .iter()
            .filter(|c| **c == "draw_status_bar")
            .count();
        assert!(
            status_bar_calls >= 2,
            "expected at least 2 draw_status_bar calls (one per variant column), got {status_bar_calls}: {:?}",
            backend.calls
        );
    }

    #[test]
    fn variant_col_at_divides_the_row_into_equal_columns() {
        let row = Rect::new(0.0, 0.0, 30.0, 1.0);
        assert_eq!(variant_col_at(row, 3, 0.0), Some(0));
        assert_eq!(variant_col_at(row, 3, 9.9), Some(0));
        assert_eq!(variant_col_at(row, 3, 10.0), Some(1));
        assert_eq!(variant_col_at(row, 3, 19.9), Some(1));
        assert_eq!(variant_col_at(row, 3, 20.0), Some(2));
        assert_eq!(variant_col_at(row, 3, 29.9), Some(2));
    }

    #[test]
    fn variant_col_at_rejects_points_outside_the_row() {
        let row = Rect::new(10.0, 0.0, 30.0, 1.0);
        assert_eq!(variant_col_at(row, 3, 9.9), None);
        assert_eq!(variant_col_at(row, 3, 40.0), None);
        assert_eq!(variant_col_at(row, 0, 15.0), None);
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
