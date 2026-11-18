//! `Canvas::regions`/`Canvas::focusable` `AppLogic` ([`tui_canvas_regions`]).
//!
//! One `Canvas` with two named [`CanvasRegion`]s — `A` and `B` — each a
//! toggle button painted entirely through `DrawOp`s. The canvas carries
//! no retained scene graph of its own (`Canvas`'s module doc), so this
//! demo keeps the toggle state itself and rebuilds both the paint ops
//! and the region list from it every frame, exactly like
//! `examples/common/canvas_app.rs`'s gauge does for its single value.
//!
//! Three things this demo exercises, all resolved through
//! `Backend::canvas_layout`/[`CanvasLayout::region_hit_test`] — the same
//! seam `canvas_app.rs` already resolves plain clicks through:
//!
//! 1. **Click-per-region** — clicking inside `A` or `B` toggles that
//!    region's own on/off state; a click elsewhere, or on neither
//!    region, changes nothing (`CanvasHit::Inside`/`Outside` are both
//!    no-ops here, same as `canvas_app.rs`'s `CanvasHit::Outside`).
//! 2. **Hover cursor** — moving the mouse over a region reads back
//!    [`CanvasRegion::cursor`] and forwards it to `Backend::set_cursor`
//!    itself; nothing in `quadraui` calls `set_cursor` on this demo's
//!    behalf (`CanvasRegion::cursor`'s own doc). On TUI this is a
//!    silent no-op (`Backend::set_cursor`'s default body), but the call
//!    still happens on every `MouseMoved`.
//! 3. **Tab-to-focus + a key reaching the canvas** — `Canvas::focusable`
//!    is `true`, and this `AppLogic` hand-rolls a single-entry
//!    `tab_stops` list (no `ScreenLayout` involved — unlike
//!    `examples/common/focus_demo.rs`, there is only one tab stop here,
//!    so there is no layout to derive it from). Once the canvas is
//!    focused (`UiEvent::FocusChanged`), Left/Right moves which region
//!    is "selected" and Enter/Space toggles the selected region — both
//!    delivered to `handle` as ordinary `KeyPressed` events, proving a
//!    keystroke really does reach a focused `Canvas`.
//!
//! Controls:
//! - click region A / B    toggle that region on/off
//! - Tab                   focus the canvas
//! - Left / Right (focused)   move the keyboard selection between A/B
//! - Enter / Space (focused)  toggle the selected region
//! - q / Esc               quit

use std::cell::RefCell;

use quadraui::{
    AppLogic, Backend, Canvas, CanvasCursor, CanvasHit, CanvasRegion, Color, DrawOp,
    InteractionState, Key, MouseButton, NamedKey, PointerShape, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

/// The two toggleable regions, in declaration order — also their index
/// into [`CanvasRegionsDemo::active`].
const REGION_IDS: [&str; 2] = ["region:a", "region:b"];
const REGION_LABELS: [&str; 2] = ["A", "B"];

pub struct CanvasRegionsDemo {
    /// Whether each region (indexed by [`REGION_IDS`]) is toggled on.
    active: [bool; 2],
    /// Which region Left/Right currently selects for Enter/Space to
    /// toggle — independent of mouse hover, same way a focused `List`'s
    /// `selected_idx` is independent of where the mouse last was.
    selected: usize,
    /// Last `UiEvent::FocusChanged` payload — `Some(canvas_id)` once the
    /// canvas has been Tab'd to, `None` before that and after the canvas
    /// loses focus (there is nowhere else to Tab to in this demo, so
    /// that only happens before the very first Tab).
    focused: Option<WidgetId>,
    /// The canvas's own rect, cached at paint time for `tab_stops` to
    /// read back — `AppLogic::tab_stops` is `&self`-only and gets no
    /// `Backend`, so the geometry has to be computed where a `Backend`
    /// *is* available (`render`) and read back here, same pattern
    /// `examples/common/focus_demo.rs`'s `cached_tab_stops` uses.
    cached_canvas_rect: RefCell<Rect>,
}

impl CanvasRegionsDemo {
    pub fn new() -> Self {
        Self {
            active: [false, false],
            selected: 0,
            focused: None,
            cached_canvas_rect: RefCell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
        }
    }

    fn canvas_id(&self) -> WidgetId {
        WidgetId::new("canvas:regions")
    }

    /// The canvas's own rect, in the active backend's native units —
    /// same "size off `line_height`" convention as
    /// `examples/common/canvas_app.rs`'s `gauge_rect`.
    fn canvas_rect(&self, backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(lh, lh * 2.0, (viewport.width - lh * 2.0).max(0.0), lh * 3.0)
    }

    /// Build the canvas's `DrawOp`s and `regions`, in LOCAL coordinates
    /// — `(0, 0)` is `rect`'s own top-left corner (`Canvas`'s module
    /// doc's "Coordinate frame" section). Painting and hit-testing both
    /// go through this one method, so they can never disagree, the same
    /// contract `canvas_app.rs`'s `gauge_canvas` documents.
    fn canvas(&self, rect: Rect) -> Canvas {
        let gap = 2.0_f32.min(rect.width / 4.0);
        let box_w = ((rect.width - gap) / 2.0).max(1.0);
        let box_h = rect.height;
        let mut ops = Vec::new();
        let mut regions = Vec::new();
        for (i, label) in REGION_LABELS.iter().enumerate() {
            let box_rect = Rect::new(i as f32 * (box_w + gap), 0.0, box_w, box_h);
            let fill = if self.active[i] {
                Color::rgb(40, 160, 90)
            } else {
                Color::rgb(60, 60, 60)
            };
            ops.push(DrawOp::Rect {
                rect: box_rect,
                color: fill,
            });
            // The keyboard-selected region gets a border so the
            // selection is visible without relying on color alone.
            if i == self.selected {
                ops.push(DrawOp::StrokeRect {
                    rect: box_rect,
                    color: Color::rgb(255, 255, 255),
                    stroke_width: 1.0,
                });
            }
            let marker = if i == self.selected { ">" } else { " " };
            let state = if self.active[i] { "ON" } else { "OFF" };
            ops.push(DrawOp::TextRun {
                rect: Rect::new(box_rect.x + 1.0, box_rect.y + 1.0, box_w.max(1.0), 1.0),
                text: format!("{marker}{label}:{state}"),
                color: Color::rgb(255, 255, 255),
            });
            regions.push(CanvasRegion {
                id: WidgetId::new(REGION_IDS[i]),
                rect: box_rect,
                cursor: Some(CanvasCursor::Pointer),
            });
        }
        Canvas {
            id: self.canvas_id(),
            ops,
            regions,
            focusable: true,
        }
    }

    fn region_index(&self, id: WidgetId) -> Option<usize> {
        REGION_IDS
            .iter()
            .position(|candidate| WidgetId::new(*candidate) == id)
    }

    fn status_bar(&self) -> StatusBar {
        let focus_label = match &self.focused {
            Some(id) if *id == self.canvas_id() => "canvas: focused".to_string(),
            _ => "canvas: unfocused".to_string(),
        };
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " {focus_label} — click A/B to toggle, Tab to focus, \
                     Left/Right + Enter once focused, q to quit "
                ),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for CanvasRegionsDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for CanvasRegionsDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let rect = self.canvas_rect(backend);
        *self.cached_canvas_rect.borrow_mut() = rect;
        let _ = backend.draw_canvas(rect, &self.canvas(rect));

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        // One tab stop: the canvas itself. No `ScreenLayout` involved —
        // unlike `focus_demo.rs`'s multi-widget order, there is nothing
        // to derive here beyond the one rect `render` already cached.
        vec![(self.canvas_id(), *self.cached_canvas_rect.borrow())]
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
            UiEvent::FocusChanged(id) => {
                self.focused = id;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Left),
                ..
            } if self.focused == Some(self.canvas_id()) => {
                self.selected = (self.selected + REGION_IDS.len() - 1) % REGION_IDS.len();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Right),
                ..
            } if self.focused == Some(self.canvas_id()) => {
                self.selected = (self.selected + 1) % REGION_IDS.len();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter) | Key::Char(' '),
                ..
            } if self.focused == Some(self.canvas_id()) => {
                self.active[self.selected] = !self.active[self.selected];
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = self.canvas_rect(backend);
                let canvas = self.canvas(rect);
                // Same rect/`canvas()` both `render` and this handler
                // build from, so paint and click-routing can never
                // disagree — `canvas_app.rs`'s own doc comment.
                let layout = backend.canvas_layout(rect, &canvas);
                if let CanvasHit::Region { id } =
                    layout.region_hit_test(&canvas, position.x, position.y)
                {
                    if let Some(idx) = self.region_index(id) {
                        self.active[idx] = !self.active[idx];
                        self.selected = idx;
                        return Reaction::Redraw;
                    }
                }
                Reaction::Continue
            }
            UiEvent::MouseMoved { position, .. } => {
                let rect = self.canvas_rect(backend);
                let canvas = self.canvas(rect);
                let layout = backend.canvas_layout(rect, &canvas);
                let shape = match layout.region_hit_test(&canvas, position.x, position.y) {
                    CanvasHit::Region { id } => self
                        .region_index(id)
                        .and_then(|idx| canvas.regions[idx].cursor)
                        .map(CanvasCursor::to_pointer_shape)
                        .unwrap_or(PointerShape::Default),
                    _ => PointerShape::Default,
                };
                let _ = backend.set_cursor(shape);
                Reaction::Continue
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
