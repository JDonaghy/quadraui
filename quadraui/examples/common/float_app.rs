//! Backend-agnostic app code for the `Float` primitive demo
//! ([`tui_float`] / [`gtk_float`]).
//!
//! Three floats, exercising placement, edge clamping, z-order, focus
//! vs. non-focus key routing and opaque vs. transparent chrome:
//!
//! - **Placement + edge clamping**: the actions menu anchors to the
//!   currently selected command row and prefers to open below it
//!   (`Side::Bottom`). Navigate to one of the last rows and the float
//!   automatically flips to open above instead — [`Anchor::resolve`]'s
//!   fallback, driven by real state, not a hand-picked example.
//! - **Z-order**: both floats can be open at the same time. Opening the
//!   hint while the menu is already open pushes the hint *on top* of
//!   it in [`crate::ModalStack`] — the menu doesn't move, doesn't
//!   close, and (see the next point) doesn't lose keyboard focus just
//!   because something painted over part of it.
//! - **Focus vs. non-focus**: the hint float
//!   (`focusable: false`) is a which-key-style popup — it must never
//!   steal keys. The actions menu (`focusable: true`) is interactive —
//!   arrow keys and Enter must reach it while it's open. `handle` below
//!   never hand-rolls an "is a float open" bool chain to decide which:
//!   it asks `backend.modal_stack_handle().borrow().top_focusable()`.
//! - **Opaque vs. transparent**: the overlay banner is pinned over the
//!   middle of the command list and `t` cycles it through transparent
//!   → opaque → closed. Transparent (`Float::transparent`) paints the
//!   border and the banner's own label line but no background fill, so
//!   the command rows it covers stay readable underneath it; opaque
//!   paints the fill and hides them. Same float, same anchor, same
//!   `Backend::draw_float` call — one `bool` apart.
//!
//! Controls:
//! - Up / Down: move the command list cursor, or (when the menu is
//!   open) the menu's own cursor
//! - Enter: run the selected command, or (menu open) pick the
//!   selected menu item
//! - m: toggle the actions menu for the selected command
//! - ?: toggle the which-key hint (never steals focus)
//! - t: cycle the overlay banner: transparent → opaque → closed
//!   (never steals focus either; clicking it cycles it too)
//! - Esc: close the menu if open, else the hint, else the overlay
//!   banner, else quit
//! - q: quit

use quadraui::{
    Anchor, AppLogic, Backend, Color, Float, FloatLayout, FloatMeasure, InteractionState, Key,
    NamedKey, Reaction, Rect, Side, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

const HINT_ID: &str = "float:hint";
const MENU_ID: &str = "float:menu";
const OVERLAY_ID: &str = "float:overlay";

const COMMANDS: &[&str] = &["Open File", "Save", "Rename", "Duplicate", "Close", "Quit"];
const MENU_ITEMS: &[&str] = &["Run now", "Run later", "Cancel"];

/// Overlay banner size, in character cells / line heights — wide
/// enough for its longest label and tall enough to cover several
/// command rows, so "shows through" vs. "hidden" is visible at a
/// glance on either backend.
const OVERLAY_COLS: f32 = 26.0;
const OVERLAY_ROWS: f32 = 5.0;

/// The overlay banner's chrome state, cycled by `t`. `Transparent` and
/// `Opaque` differ only in the banner float's `transparent` flag — see
/// this module's doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayMode {
    Closed,
    Transparent,
    Opaque,
}

impl OverlayMode {
    fn next(self) -> Self {
        match self {
            Self::Closed => Self::Transparent,
            Self::Transparent => Self::Opaque,
            Self::Opaque => Self::Closed,
        }
    }

    /// Label painted into the banner's `content_bounds`, naming the
    /// mode on screen so the demo is readable without the doc.
    fn label(self) -> &'static str {
        match self {
            Self::Closed => "",
            Self::Transparent => "overlay: transparent",
            Self::Opaque => "overlay: opaque",
        }
    }
}

pub struct FloatApp {
    selected: usize,
    hint_open: bool,
    menu_open: bool,
    menu_selected: usize,
    overlay: OverlayMode,
    last_action: String,
}

impl FloatApp {
    pub fn new() -> Self {
        Self {
            selected: 0,
            hint_open: false,
            menu_open: false,
            menu_selected: 0,
            overlay: OverlayMode::Closed,
            last_action: String::new(),
        }
    }

    /// The selected command row's rect, in the active backend's native
    /// units — one `line_height` tall, full width, starting at row 0.
    fn row_rect(&self, backend: &dyn Backend, idx: usize) -> Rect {
        let lh = backend.line_height();
        let width = backend.viewport().width;
        Rect::new(0.0, idx as f32 * lh, width, lh)
    }

    /// Anchors the actions menu to the selected row, preferring to open
    /// below it. Near the bottom of the list this overflows the
    /// viewport and [`Anchor::resolve`] flips it to open above instead
    /// — see this module's doc.
    fn menu_anchor(&self, backend: &dyn Backend) -> Anchor {
        Anchor::new(self.row_rect(backend, self.selected), Side::Bottom)
    }

    fn menu_float(&self, backend: &dyn Backend) -> Float {
        Float::new(WidgetId::new(MENU_ID), self.menu_anchor(backend))
    }

    fn menu_layout(&self, backend: &dyn Backend) -> FloatLayout {
        let cw = backend.char_width();
        let lh = backend.line_height();
        let float = self.menu_float(backend);
        // N content lines + 2 border rows.
        let measure = FloatMeasure::new(cw * 16.0, lh * (MENU_ITEMS.len() as f32 + 2.0));
        float.layout(
            Rect::new(
                0.0,
                0.0,
                backend.viewport().width,
                backend.viewport().height,
            ),
            measure,
        )
    }

    /// Anchors the which-key hint to a fixed help affordance in the
    /// bottom-right corner, opening upward (`Side::Top`) so it never
    /// covers the status bar it's anchored to. The flip/clamp fallback
    /// itself is exercised by [`Self::menu_anchor`] below, whose anchor
    /// rect moves with `self.selected` — see that method's doc.
    fn hint_anchor(&self, backend: &dyn Backend) -> Anchor {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let cw = backend.char_width();
        let w = cw * 7.0; // "?=help "
        Anchor::new(
            Rect::new(viewport.width - w, viewport.height - lh, w, lh),
            Side::Top,
        )
    }

    fn hint_float(&self, backend: &dyn Backend) -> Float {
        let mut f = Float::new(WidgetId::new(HINT_ID), self.hint_anchor(backend));
        f.focusable = false;
        f
    }

    fn hint_layout(&self, backend: &dyn Backend) -> FloatLayout {
        let cw = backend.char_width();
        let lh = backend.line_height();
        let float = self.hint_float(backend);
        // 3 content lines + 2 border rows.
        let measure = FloatMeasure::new(cw * 22.0, lh * 5.0);
        float.layout(
            Rect::new(
                0.0,
                0.0,
                backend.viewport().width,
                backend.viewport().height,
            ),
            measure,
        )
    }

    /// The overlay banner is a fixed band pinned over the middle of the
    /// command list: [`Side::AtPoint`] opens the box at the anchor
    /// rect's own origin (clamped to the viewport) instead of beside
    /// it, which is what a banner wants — it is not attached to a row.
    fn overlay_anchor(&self, backend: &dyn Backend) -> Anchor {
        let cw = backend.char_width();
        let lh = backend.line_height();
        Anchor::new(
            Rect::new(0.0, lh, cw * OVERLAY_COLS, lh * OVERLAY_ROWS),
            Side::AtPoint,
        )
    }

    /// The banner float, or `None` when it is closed. `transparent` is
    /// the only difference between the two open modes: with it set,
    /// `Backend::draw_float` skips the background fill and the command
    /// rows beneath the banner stay readable through it.
    fn overlay_float(&self, backend: &dyn Backend) -> Option<Float> {
        if self.overlay == OverlayMode::Closed {
            return None;
        }
        let mut f = Float::new(WidgetId::new(OVERLAY_ID), self.overlay_anchor(backend));
        // A banner is informational: it must never take the keyboard
        // away from the list it sits on top of.
        f.focusable = false;
        f.transparent = self.overlay == OverlayMode::Transparent;
        Some(f)
    }

    fn overlay_layout(&self, backend: &dyn Backend) -> FloatLayout {
        let cw = backend.char_width();
        let lh = backend.line_height();
        let anchor = self.overlay_anchor(backend);
        let mut float = Float::new(WidgetId::new(OVERLAY_ID), anchor);
        float.focusable = false;
        let measure = FloatMeasure::new(cw * OVERLAY_COLS, lh * OVERLAY_ROWS);
        float.layout(
            Rect::new(
                0.0,
                0.0,
                backend.viewport().width,
                backend.viewport().height,
            ),
            measure,
        )
    }

    /// The banner's single label line, painted into the first row of
    /// `layout.content_bounds`. The remaining content rows are left
    /// deliberately unpainted: when the banner is transparent that is
    /// where the command rows underneath show through, and when it is
    /// opaque that is where the background fill covers them.
    fn paint_overlay_content(&self, backend: &mut dyn Backend, layout: &FloatLayout) {
        let lh = backend.line_height();
        let cb = layout.content_bounds;
        self.line_at(
            backend,
            Rect::new(cb.x, cb.y, cb.width, lh),
            "overlay-label",
            format!(" {} ", self.overlay.label()),
            false,
        );
    }

    /// Advance the banner one step through transparent → opaque →
    /// closed.
    fn cycle_overlay(&mut self, backend: &mut dyn Backend) {
        self.overlay = self.overlay.next();
        self.sync_overlay(backend);
    }

    fn close_overlay(&mut self, backend: &mut dyn Backend) {
        self.overlay = OverlayMode::Closed;
        self.sync_overlay(backend);
    }

    /// Bring [`crate::ModalStack`] in line with the current mode:
    /// `push_float` re-registers the same id with the current bounds,
    /// so the two open modes share one entry and closing pops it.
    fn sync_overlay(&mut self, backend: &mut dyn Backend) {
        match self.overlay_float(backend) {
            Some(float) => {
                let bounds = self.overlay_layout(backend).bounds;
                backend
                    .modal_stack_handle()
                    .borrow_mut()
                    .push_float(&float, bounds);
            }
            None => {
                backend
                    .modal_stack_handle()
                    .borrow_mut()
                    .pop(&WidgetId::new(OVERLAY_ID));
            }
        }
    }

    fn status_bar(&self) -> StatusBar {
        let last = if self.last_action.is_empty() {
            "nothing yet".to_string()
        } else {
            self.last_action.clone()
        };
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" last: {last} "),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " m=menu ?=help q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn line(&self, backend: &mut dyn Backend, y: f32, id: &str, text: String, selected: bool) {
        let width = backend.viewport().width;
        let lh = backend.line_height();
        self.line_at(backend, Rect::new(0.0, y, width, lh), id, text, selected);
    }

    /// Same as [`Self::line`], but at an explicit `row` rect rather than
    /// always spanning the full viewport from `x = 0` — needed to paint
    /// a float's own content into `layout.content_bounds`, which starts
    /// wherever `Float::layout` resolved the box to, not at the origin.
    fn line_at(
        &self,
        backend: &mut dyn Backend,
        row: Rect,
        id: &str,
        text: String,
        selected: bool,
    ) {
        let bg = if selected {
            Color::rgb(60, 60, 90)
        } else {
            Color::rgb(24, 24, 32)
        };
        let bar = StatusBar {
            id: WidgetId::new(id),
            left_segments: vec![StatusBarSegment {
                text,
                fg: Color::rgb(220, 220, 220),
                bg,
                bold: selected,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar_interactive(row, &bar, &InteractionState::new());
    }

    /// Paint the menu's own items into `layout.content_bounds` — `Float`
    /// never paints content itself (see its module doc); this is the
    /// host's job, same contract as `SidebarPanelLayout::content_bounds`.
    fn paint_menu_content(&self, backend: &mut dyn Backend, layout: &FloatLayout) {
        let lh = backend.line_height();
        let cb = layout.content_bounds;
        for (i, item) in MENU_ITEMS.iter().enumerate() {
            let y = cb.y + i as f32 * lh;
            if y + lh > cb.y + cb.height {
                break;
            }
            self.line_at(
                backend,
                Rect::new(cb.x, y, cb.width, lh),
                &format!("menu-item-{i}"),
                format!(" {item} "),
                i == self.menu_selected,
            );
        }
    }

    fn paint_hint_content(&self, backend: &mut dyn Backend, layout: &FloatLayout) {
        let lh = backend.line_height();
        let cb = layout.content_bounds;
        let lines = ["Up/Down move", "Enter runs", "m = menu"];
        for (i, text) in lines.iter().enumerate() {
            let y = cb.y + i as f32 * lh;
            if y + lh > cb.y + cb.height {
                break;
            }
            self.line_at(
                backend,
                Rect::new(cb.x, y, cb.width, lh),
                &format!("hint-line-{i}"),
                format!(" {text} "),
                false,
            );
        }
    }

    fn open_menu(&mut self, backend: &mut dyn Backend) {
        self.menu_open = true;
        self.menu_selected = 0;
        let bounds = self.menu_layout(backend).bounds;
        let float = self.menu_float(backend);
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push_float(&float, bounds);
    }

    fn close_menu(&mut self, backend: &mut dyn Backend) {
        self.menu_open = false;
        backend
            .modal_stack_handle()
            .borrow_mut()
            .pop(&WidgetId::new(MENU_ID));
    }

    fn toggle_hint(&mut self, backend: &mut dyn Backend) {
        if self.hint_open {
            self.hint_open = false;
            backend
                .modal_stack_handle()
                .borrow_mut()
                .pop(&WidgetId::new(HINT_ID));
        } else {
            self.hint_open = true;
            let bounds = self.hint_layout(backend).bounds;
            let float = self.hint_float(backend);
            // `push_float` forwards `float.focusable` (`false` here), so
            // the hint popup never steals keys — see `Float`'s module
            // doc's "Focus vs. non-focus floats" section.
            backend
                .modal_stack_handle()
                .borrow_mut()
                .push_float(&float, bounds);
        }
    }
}

impl Default for FloatApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FloatApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let lh = backend.line_height();
        let viewport = backend.viewport();

        for (i, cmd) in COMMANDS.iter().enumerate() {
            self.line(
                backend,
                i as f32 * lh,
                &format!("cmd-{i}"),
                format!(" {cmd} "),
                i == self.selected,
            );
        }

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );

        // Floats paint last (highest z) — `ModalStack` has no opinion on
        // draw order, only on hit-test precedence (see its module doc).
        // The menu paints before the hint so that, when both are open,
        // the hint visually sits on top — exercising z-order without
        // the hint ever being able to steal the menu's keyboard focus
        // (see `handle` below).
        //
        // The overlay banner paints first of the three, i.e. directly on
        // top of the command rows it is meant to show through: a
        // transparent float only reveals what was already painted
        // beneath it this frame.
        if let Some(float) = self.overlay_float(backend) {
            let layout = self.overlay_layout(backend);
            backend.draw_float(&float, &layout);
            self.paint_overlay_content(backend, &layout);
        }
        if self.menu_open {
            let layout = self.menu_layout(backend);
            let float = self.menu_float(backend);
            backend.draw_float(&float, &layout);
            self.paint_menu_content(backend, &layout);
        }
        if self.hint_open {
            let layout = self.hint_layout(backend);
            let float = self.hint_float(backend);
            backend.draw_float(&float, &layout);
            self.paint_hint_content(backend, &layout);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => {
                if self.menu_open {
                    self.close_menu(backend);
                    Reaction::Redraw
                } else if self.hint_open {
                    self.toggle_hint(backend);
                    Reaction::Redraw
                } else if self.overlay != OverlayMode::Closed {
                    self.close_overlay(backend);
                    Reaction::Redraw
                } else {
                    Reaction::Exit
                }
            }
            UiEvent::KeyPressed {
                key: Key::Char('?'),
                ..
            } => {
                self.toggle_hint(backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('t'),
                ..
            } => {
                self.cycle_overlay(backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                if self.menu_open {
                    self.close_menu(backend);
                } else {
                    self.open_menu(backend);
                }
                Reaction::Redraw
            }

            // Route by asking who currently owns the keyboard —
            // never a hand-rolled "is a float open" bool chain. A
            // non-focusable hint on top of the stack is skipped
            // automatically; `top_focusable()` only ever names the menu
            // (when open) or nothing (base layer).
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                let focused = backend
                    .modal_stack_handle()
                    .borrow()
                    .top_focusable()
                    .map(|e| e.id);
                if focused == Some(WidgetId::new(MENU_ID)) {
                    self.menu_selected = self.menu_selected.saturating_sub(1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                let focused = backend
                    .modal_stack_handle()
                    .borrow()
                    .top_focusable()
                    .map(|e| e.id);
                if focused == Some(WidgetId::new(MENU_ID)) {
                    self.menu_selected = (self.menu_selected + 1).min(MENU_ITEMS.len() - 1);
                } else {
                    self.selected = (self.selected + 1).min(COMMANDS.len() - 1);
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            } => {
                let focused = backend
                    .modal_stack_handle()
                    .borrow()
                    .top_focusable()
                    .map(|e| e.id);
                if focused == Some(WidgetId::new(MENU_ID)) {
                    self.last_action = format!(
                        "{} / {}",
                        COMMANDS[self.selected], MENU_ITEMS[self.menu_selected]
                    );
                    self.close_menu(backend);
                } else if COMMANDS[self.selected] == "Quit" {
                    return Reaction::Exit;
                } else {
                    self.last_action = COMMANDS[self.selected].to_string();
                }
                Reaction::Redraw
            }

            // ── Clicks inside an open float — pre-tagged by `ModalStack`
            // (see `examples/common/modal_occlusion_demo.rs` for the
            // established no-click-guard pattern) ─────────────────────
            UiEvent::MouseDown {
                widget: Some(ref id),
                position,
                ..
            } if id.as_str() == MENU_ID => {
                let layout = self.menu_layout(backend);
                let lh = backend.line_height();
                let rel_y = position.y - layout.content_bounds.y;
                if rel_y >= 0.0 {
                    let idx = (rel_y / lh).floor() as usize;
                    if idx < MENU_ITEMS.len() {
                        self.last_action =
                            format!("{} / {}", COMMANDS[self.selected], MENU_ITEMS[idx]);
                        self.close_menu(backend);
                    }
                }
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                widget: Some(ref id),
                ..
            } if id.as_str() == HINT_ID => {
                // The hint swallows the click (it's registered on the
                // modal stack so it can) but has nothing to activate —
                // click it away.
                self.toggle_hint(backend);
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                widget: Some(ref id),
                ..
            } if id.as_str() == OVERLAY_ID => {
                // Registered on the modal stack like the other two, so
                // a click lands on the banner rather than the row it
                // covers — cycling it is the way out.
                self.cycle_overlay(backend);
                Reaction::Redraw
            }

            UiEvent::MouseDown {
                widget: None,
                position,
                ..
            } => {
                let lh = backend.line_height();
                let idx = (position.y / lh).floor() as usize;
                if idx < COMMANDS.len() {
                    self.selected = idx;
                }
                Reaction::Redraw
            }

            UiEvent::WindowResized { .. } => {
                // Both anchors are viewport-derived, so a resize moves
                // the resolved bounds; re-push onto `ModalStack` so its
                // hit-test rects stay in sync with what `render` repaints
                // next frame.
                if self.menu_open {
                    let bounds = self.menu_layout(backend).bounds;
                    let float = self.menu_float(backend);
                    backend
                        .modal_stack_handle()
                        .borrow_mut()
                        .push_float(&float, bounds);
                }
                if self.hint_open {
                    let bounds = self.hint_layout(backend).bounds;
                    let float = self.hint_float(backend);
                    backend
                        .modal_stack_handle()
                        .borrow_mut()
                        .push_float(&float, bounds);
                }
                self.sync_overlay(backend);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
