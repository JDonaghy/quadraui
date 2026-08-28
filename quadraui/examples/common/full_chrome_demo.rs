//! Full-chrome AppShell demo — proves all four chrome slots
//! (title bar, bottom panel, command line, status bar) plus the
//! existing activity bar / sidebar / main layout.
//!
//! The empty part of the title bar also exercises the CSD drag/maximize
//! escape hatch (#400): left-`MouseDown` calls `Backend::begin_window_drag`,
//! `DoubleClick` calls `Backend::toggle_window_maximize`. On GTK this
//! actually moves/maximizes the window; on TUI (no window to drive) both
//! calls are a documented no-op — the demo's status line shows which
//! happened either way.
//!
//! The outer window border also exercises the edge-resize escape hatch
//! (#406): a left-`MouseDown` within `backend.line_height()` of any edge or
//! corner calls `Backend::begin_window_resize`, and every `MouseMoved`
//! hints the resize cursor via `Backend::set_cursor` while hovering one.
//! Same no-op-on-TUI story as #400 — there's no window to resize, so both
//! calls report back `false` and the status line says so.
//!
//! `Ctrl+T` demonstrates the #532 runtime toggle:
//! `ctx.shell_mut().set_title_bar_visible(!visible)` shows/hides the title
//! bar band without rebuilding the shell, the same `ctx.shell_mut()` path
//! `Ctrl+B` uses for the sidebar in `appshell_demo.rs` (#454). This is the
//! shape vimcode's runtime-toggleable menu bar (vimcode#605) needs: the
//! activity bar and main content must re-derive their top origin from
//! `AppShell::layout()` on the very next frame, not from a cached offset —
//! toggle title bar off and the "TITLE BAR" label disappears while the
//! activity bar/content shift up to reclaim the row; toggle it back on and
//! they shift back down without losing the configured height.

use quadraui::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition};
use quadraui::{
    Backend, Color, Key, Modifiers, MouseButton, NamedKey, PointerShape, Reaction, Rect,
    ShellApp as ShellAppTrait, ShellConfig, ShellContext, StatusBar, StatusBarAction,
    StatusBarInteraction, StatusBarSegment, UiEvent, WidgetId,
};

/// `action_id`s for the CSD title-bar button row (#402). Namespaced per
/// `StatusBarSegment::action_id`'s doc convention.
const TITLE_BAR_MINIMIZE: &str = "titlebar:minimize";
const TITLE_BAR_MAXIMIZE: &str = "titlebar:maximize";
const TITLE_BAR_CLOSE: &str = "titlebar:close";

pub struct FullChromeDemo {
    last_event: String,
    /// Hover/press tracking for the minimize/maximize/close button row
    /// painted into the right side of the title bar (#402). The drag/
    /// double-click-to-maximize gesture on the *empty* part of the title
    /// bar (#400) is handled separately in `handle()` below.
    title_bar_interaction: StatusBarInteraction,
}

impl FullChromeDemo {
    pub fn new() -> Self {
        Self {
            last_event: "click icons | drag dividers | drag/double-click title bar | drag edges to resize | Ctrl+T=toggle title bar | q=quit"
                .into(),
            title_bar_interaction: StatusBarInteraction::new(),
        }
    }

    pub fn config() -> ShellConfig {
        ShellConfig::new(
            "Full Chrome Demo",
            vec![
                PanelDefinition {
                    id: WidgetId::new("panel:explorer"),
                    icon: "E".into(),
                    tooltip: "Explorer".into(),
                    title: "EXPLORER".into(),
                },
                PanelDefinition {
                    id: WidgetId::new("panel:search"),
                    icon: "S".into(),
                    tooltip: "Search".into(),
                    title: "SEARCH".into(),
                },
                PanelDefinition {
                    id: WidgetId::new("panel:git"),
                    icon: "G".into(),
                    tooltip: "Source Control".into(),
                    title: "SOURCE CONTROL".into(),
                },
                PanelDefinition {
                    id: WidgetId::new("panel:debug"),
                    icon: "D".into(),
                    tooltip: "Debug".into(),
                    title: "RUN AND DEBUG".into(),
                },
            ],
        )
        .with_bottom_items(vec![PanelDefinition {
            id: WidgetId::new("panel:settings"),
            icon: "*".into(),
            tooltip: "Settings".into(),
            title: "Settings".into(),
        }])
        .with_title_bar(1.5)
        .with_bottom_panel(8.0)
        .with_bottom_panel_limits(3.0, 25.0)
        .with_command_line()
        .with_status_bar()
    }

    fn draw_label(
        backend: &mut dyn Backend,
        bounds: Rect,
        text: &str,
        fg: Color,
        bg: Color,
        bold: bool,
    ) {
        let lh = backend.line_height();
        let bar = StatusBar {
            id: WidgetId::new("chrome-label"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {text} "),
                fg,
                bg,
                bold,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let rect = Rect::new(bounds.x, bounds.y, bounds.width, lh.min(bounds.height));
        backend.draw_status_bar(rect, &bar, None, None);
    }

    /// Build the title bar's `StatusBar`: the existing plain-text label on
    /// the left, plus a realistic minimize/maximize/close button row on the
    /// right using the same `StatusBarSegment::action_id` mechanism every
    /// other clickable status-bar segment in this codebase uses (#402).
    fn build_title_bar(text: &str, fg: Color, bg: Color) -> StatusBar {
        let button_bg = Color::rgb(70, 70, 76);
        let close_bg = Color::rgb(190, 55, 45);
        StatusBar {
            id: WidgetId::new("chrome-title-bar"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {text} "),
                fg,
                bg,
                bold: true,
                action_id: None,
            }],
            right_segments: vec![
                StatusBarSegment {
                    text: " \u{2500} ".into(),
                    fg,
                    bg: button_bg,
                    bold: false,
                    action_id: Some(WidgetId::new(TITLE_BAR_MINIMIZE)),
                },
                StatusBarSegment {
                    text: " \u{25a1} ".into(),
                    fg,
                    bg: button_bg,
                    bold: false,
                    action_id: Some(WidgetId::new(TITLE_BAR_MAXIMIZE)),
                },
                StatusBarSegment {
                    text: " \u{2715} ".into(),
                    fg,
                    bg: close_bg,
                    bold: false,
                    action_id: Some(WidgetId::new(TITLE_BAR_CLOSE)),
                },
            ],
        }
    }

    /// Dispatch a click on one of the title bar's button segments.
    fn handle_title_bar_button(&mut self, backend: &mut dyn Backend, id: &WidgetId) -> Reaction {
        match id.as_str() {
            TITLE_BAR_CLOSE => Reaction::Exit,
            TITLE_BAR_MAXIMIZE => {
                let toggled = backend.toggle_window_maximize();
                self.last_event = if toggled {
                    "Maximize button: maximize toggled".into()
                } else {
                    "Maximize button (no window)".into()
                };
                Reaction::Redraw
            }
            TITLE_BAR_MINIMIZE => {
                // No `Backend::minimize_window` exists yet — this button
                // demonstrates the click-target/hover mechanism only.
                self.last_event = "Minimize button (no backend hook yet)".into();
                Reaction::Redraw
            }
            _ => Reaction::Redraw,
        }
    }
}

impl Default for FullChromeDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellAppTrait for FullChromeDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let title_fg = Color::rgb(220, 220, 220);
        let title_bg = Color::rgb(50, 50, 55);
        let content_fg = Color::rgb(140, 140, 140);
        let content_bg = Color::rgb(30, 30, 30);
        let status_fg = Color::rgb(200, 200, 220);
        let status_bg = Color::rgb(0, 120, 210);

        if let Some(tb) = layout.title_bar_bounds {
            let lh = backend.line_height();
            let rect = Rect::new(tb.x, tb.y, tb.width, lh.min(tb.height));
            let bar =
                Self::build_title_bar("TITLE BAR  |  File  Edit  View  Help", title_fg, title_bg);
            let bar_layout = backend.draw_status_bar(
                rect,
                &bar,
                self.title_bar_interaction.hovered_id(),
                self.title_bar_interaction.pressed_id(),
            );
            self.title_bar_interaction.set_layout(bar_layout);
        }

        if let Some(content) = layout.sidebar_content_bounds {
            Self::draw_label(
                backend,
                content,
                "(sidebar content)",
                content_fg,
                content_bg,
                false,
            );
        }

        Self::draw_label(
            backend,
            layout.main_content_bounds,
            &self.last_event,
            Color::rgb(200, 200, 200),
            content_bg,
            false,
        );

        if let Some(bp) = layout.bottom_panel_bounds {
            Self::draw_label(
                backend,
                bp,
                "TERMINAL  |  bottom panel (drag top edge to resize)",
                content_fg,
                Color::rgb(25, 25, 28),
                false,
            );
        }

        if let Some(cl) = layout.command_line_bounds {
            Self::draw_label(
                backend,
                cl,
                ":command-line-slot",
                Color::rgb(180, 180, 180),
                Color::rgb(35, 35, 40),
                false,
            );
        }

        if let Some(sb) = layout.status_bar_bounds {
            Self::draw_label(
                backend,
                sb,
                "main  |  UTF-8  |  Ln 1, Col 1",
                status_fg,
                status_bg,
                false,
            );
        }
    }

    fn handle(
        &mut self,
        event: UiEvent,
        backend: &mut dyn Backend,
        ctx: &ShellContext,
    ) -> Reaction {
        // Title bar button row (#402) takes priority over the drag/
        // double-click-to-maximize gesture (#400) on the empty part of the
        // bar: a press that lands on minimize/maximize/close must not also
        // start a window drag. `StatusBarInteraction::handle` only reacts
        // to MouseMoved/MouseDown/MouseUp and is a no-op (`Ignored`) for
        // every other event and for presses outside the button segments,
        // so falling through to the existing title-bar-drag logic below is
        // safe.
        if let Some(tb) = ctx.title_bar_bounds() {
            match self.title_bar_interaction.handle(&event, tb) {
                StatusBarAction::Clicked(id) => return self.handle_title_bar_button(backend, &id),
                StatusBarAction::Redraw => return Reaction::Redraw,
                StatusBarAction::Ignored => {}
            }
        }

        match &event {
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            // `Ctrl+T` = the #532 runtime toggle: `ctx.shell_mut()` reaches
            // the same real `AppShell` `ShellAdapter` renders (the #454
            // pattern `Ctrl+B` uses for the sidebar in `appshell_demo.rs`),
            // so this app can show/hide the title-bar row without
            // rebuilding the shell. `compute_layout` re-derives the
            // activity bar and main content origins from `has_title_bar`
            // on the very next `layout()` call, so no other state needs to
            // change in lockstep.
            UiEvent::KeyPressed {
                key: Key::Char('t'),
                modifiers: Modifiers { ctrl: true, .. },
                ..
            } => {
                let now_visible = {
                    let mut shell = ctx.shell_mut();
                    let visible = !shell.title_bar_visible();
                    shell.set_title_bar_visible(visible);
                    visible
                };
                self.last_event = if now_visible {
                    "Title bar shown (Ctrl+T via ctx.shell_mut())".into()
                } else {
                    "Title bar hidden (Ctrl+T via ctx.shell_mut())".into()
                };
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                position, button, ..
            } => {
                if ctx.in_title_bar(position.x, position.y) {
                    // #400: the empty part of the title bar drags the
                    // window on a left-button press, same as native CSD
                    // chrome. `begin_window_drag` is a documented no-op
                    // (returns `false`) on backends with no window (TUI).
                    //
                    // Checked before #406's window-edge resize below: a
                    // full-width title bar owns the entire top band, same
                    // as native GTK `HeaderBar` CSD (which has no top-edge
                    // resize handle either — the header bar IS the top
                    // edge). Resize is still reachable from the left/right/
                    // bottom borders and bottom corners, which the title
                    // bar doesn't cover.
                    if *button == MouseButton::Left {
                        let dragging = backend.begin_window_drag();
                        self.last_event = if dragging {
                            "Title bar drag started".into()
                        } else {
                            "Title bar click (no window to drag)".into()
                        };
                    } else {
                        self.last_event = "Title bar click".into();
                    }
                    Reaction::Redraw
                } else if let Some(edge) =
                    ctx.window_edge(position.x, position.y, backend.line_height())
                {
                    // #406: the outer window border resizes on a left-
                    // button press, same as native CSD chrome. Margin
                    // derives from `line_height()` (portable across
                    // cells/pixels per LESSONS.md) rather than a hardcoded
                    // constant. `begin_window_resize` is a documented
                    // no-op (returns `false`) on backends with no window
                    // (TUI).
                    if *button == MouseButton::Left {
                        let resizing = backend.begin_window_resize(edge);
                        self.last_event = if resizing {
                            format!("Edge resize started ({edge:?})")
                        } else {
                            format!("Edge resize (no window) ({edge:?})")
                        };
                    } else {
                        self.last_event = format!("Edge click ({edge:?})");
                    }
                    Reaction::Redraw
                } else if ctx.in_sidebar(position.x, position.y) {
                    self.last_event = format!(
                        "Sidebar click (panel: {})",
                        ctx.active_panel_id.map(|id| id.as_str()).unwrap_or("none")
                    );
                    Reaction::Redraw
                } else if ctx.in_main(position.x, position.y) {
                    self.last_event = "Main area click".into();
                    Reaction::Redraw
                } else if ctx.in_bottom_panel(position.x, position.y) {
                    self.last_event = "Bottom panel click".into();
                    Reaction::Redraw
                } else if ctx.in_command_line(position.x, position.y) {
                    self.last_event = "Command line click".into();
                    Reaction::Redraw
                } else if ctx.in_status_bar(position.x, position.y) {
                    self.last_event = "Status bar click".into();
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            UiEvent::MouseMoved { position, .. } => {
                // #406: hint the resize cursor while hovering the outer
                // window border, restore the default pointer everywhere
                // else. No repaint needed — this only changes the OS
                // pointer glyph, not anything the app itself draws.
                //
                // Skip the title bar band first, same priority as the
                // MouseDown handling above — the title bar isn't resizable
                // (a full-width CSD header bar owns the top edge), so it
                // must never show a resize cursor even though it's within
                // `window_edge`'s top margin.
                let shape = if ctx.in_title_bar(position.x, position.y) {
                    PointerShape::Default
                } else {
                    match ctx.window_edge(position.x, position.y, backend.line_height()) {
                        Some(edge) => PointerShape::Resize(edge),
                        None => PointerShape::Default,
                    }
                };
                backend.set_cursor(shape);
                Reaction::Continue
            }
            UiEvent::DoubleClick { position, .. } => {
                if ctx.in_title_bar(position.x, position.y) {
                    // #400: double-click on the empty title bar toggles
                    // maximize/restore, same as native CSD chrome.
                    // `toggle_window_maximize` is a documented no-op on
                    // backends with no window (TUI).
                    let toggled = backend.toggle_window_maximize();
                    self.last_event = if toggled {
                        "Title bar double-click: maximize toggled".into()
                    } else {
                        "Title bar double-click (no window)".into()
                    };
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn on_shell_event(&mut self, event: &AppShellEvent, _ctx: &ShellContext) {
        self.last_event = match event {
            AppShellEvent::PanelChanged { panel_id } => {
                format!("Panel: {}", panel_id.as_str())
            }
            AppShellEvent::SidebarHidden => "Sidebar hidden".into(),
            AppShellEvent::SidebarResized { new_width } => {
                format!("Sidebar resized: {new_width:.0}px")
            }
            AppShellEvent::BottomPanelResized { new_height } => {
                format!("Bottom panel resized: {new_height:.0}px")
            }
            AppShellEvent::BottomPanelHidden => "Bottom panel hidden".into(),
            AppShellEvent::BottomItemClicked { id } => {
                format!("Bottom item: {}", id.as_str())
            }
            _ => return,
        };
    }
}
