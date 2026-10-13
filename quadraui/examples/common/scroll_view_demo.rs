//! `ScrollView` + GTK painter demo: a tall list of rows that overflows
//! its viewport, scrolled by mouse wheel or by dragging the scrollbar
//! thumb.
//!
//! `ScrollView` itself (in `quadraui::primitives::scroll_view`) only
//! tracks scroll-offset and visible-range state; this `AppLogic`
//! supplies the actual child content (the row list) and wires wheel +
//! drag input onto it, the same composition pattern every other
//! `*Controller` in `quadraui::compose` uses for its own scrollbar
//! (see `quadraui::compose::tree_controller`). Painting goes through
//! `quadraui::gtk::scroll_view::{paint_clipped, draw_scrollbar_for}`,
//! GTK-only since there is no TUI rasteriser for this primitive yet.

use quadraui::primitives::scrollbar::ThumbDrag;
use quadraui::{
    AppLogic, Backend, Color, MouseButton, PressZone, Reaction, Rect, ScrollView, UiEvent, WidgetId,
};

/// Number of rows in the list; the last one is `ROW_COUNT - 1`.
pub const ROW_COUNT: usize = 40;
/// Width of the scrollbar track along the window's right edge. Drivers
/// derive the track's centre from this plus the backend viewport, so no
/// test needs a literal pixel column.
pub const SCROLLBAR_WIDTH: f32 = 14.0;
/// Shortest thumb the scrollbar ever paints. A press within this
/// distance of the thumb's own end of the track always lands on the
/// thumb, whatever the window height.
pub const MIN_THUMB_LEN: f32 = 20.0;

const ROW_HEIGHT: f32 = 24.0;
const WHEEL_STEP: f32 = ROW_HEIGHT * 3.0;

/// Wheel-scrollable, scrollbar-draggable list of `ROW_COUNT` rows.
pub struct ScrollViewDemo {
    scroll_offset: f32,
    drag: Option<ThumbDrag>,
}

impl ScrollViewDemo {
    pub fn new() -> Self {
        Self {
            scroll_offset: 0.0,
            drag: None,
        }
    }

    fn content_len(&self) -> f32 {
        ROW_COUNT as f32 * ROW_HEIGHT
    }

    /// The row list's own viewport — the full window minus the
    /// scrollbar track along the right edge.
    fn viewport_rect(&self, backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        Rect::new(0.0, 0.0, (vp.width - SCROLLBAR_WIDTH).max(0.0), vp.height)
    }

    /// The scrollbar's own track, along the window's right edge.
    fn track_rect(&self, backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        Rect::new(
            (vp.width - SCROLLBAR_WIDTH).max(0.0),
            0.0,
            SCROLLBAR_WIDTH,
            vp.height,
        )
    }

    /// Build a fresh `ScrollView` from the current window size and the
    /// persisted `scroll_offset`, rather than caching one — the
    /// viewport rect would otherwise go stale across a window resize.
    fn view(&self, backend: &dyn Backend) -> ScrollView {
        let mut view = ScrollView::vertical(
            WidgetId::new("scroll-view-demo"),
            self.viewport_rect(backend),
            self.content_len(),
        );
        view.scroll_offset = self.scroll_offset;
        view.clamp_offset();
        view
    }

    fn row_color(row: usize) -> Color {
        if row.is_multiple_of(2) {
            Color::rgb(40, 60, 90)
        } else {
            Color::rgb(55, 80, 115)
        }
    }

    fn apply_scroll(&mut self, view: &ScrollView, delta: f32) -> Reaction {
        match view.scroll_by(delta) {
            Some(event) => {
                self.scroll_offset = match event {
                    quadraui::ScrollViewEvent::Scrolled { offset } => offset,
                };
                Reaction::Redraw
            }
            None => Reaction::Continue,
        }
    }
}

impl Default for ScrollViewDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ScrollViewDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let view = self.view(backend);
        let (start, end) = view.visible_range();
        let first_row = (start / ROW_HEIGHT).floor().max(0.0) as usize;
        let last_row = ((end / ROW_HEIGHT).ceil() as usize).min(ROW_COUNT);

        quadraui::gtk::scroll_view::paint_clipped(backend, &view, |backend| {
            for row in first_row..last_row {
                let row_top = row as f32 * ROW_HEIGHT - view.clamped_offset();
                let rect = Rect::new(view.viewport.x, row_top, view.viewport.width, ROW_HEIGHT);
                if let Some(surface) = backend.paint_surface() {
                    surface.surface_fill_rect(rect, Self::row_color(row));
                    surface.surface_draw_text_run(
                        rect,
                        &format!(" row {row}"),
                        Color::rgb(230, 230, 230),
                    );
                }
            }
        });

        let track = self.track_rect(backend);
        quadraui::gtk::scroll_view::draw_scrollbar_for(backend, &view, track, MIN_THUMB_LEN);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let view = self.view(backend);
        match event {
            UiEvent::Scroll { delta, .. } => match view.handle_wheel(delta, WHEEL_STEP) {
                Some(quadraui::ScrollViewEvent::Scrolled { offset }) => {
                    self.scroll_offset = offset;
                    Reaction::Redraw
                }
                None => Reaction::Continue,
            },
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let track = self.track_rect(backend);
                let Some(scrollbar) = view.scrollbar(track, MIN_THUMB_LEN) else {
                    return Reaction::Continue;
                };
                if !track.contains(position) {
                    return Reaction::Continue;
                }
                match scrollbar.press_zone(position.y) {
                    PressZone::Thumb => {
                        self.drag = Some(ThumbDrag::begin(
                            position.y,
                            view.clamped_offset(),
                            scrollbar.travel(),
                            view.max_offset(),
                        ));
                        Reaction::Redraw
                    }
                    PressZone::Before => self.apply_scroll(&view, -WHEEL_STEP),
                    PressZone::After => self.apply_scroll(&view, WHEEL_STEP),
                }
            }
            UiEvent::MouseMoved { position, buttons } => {
                if !buttons.left {
                    return Reaction::Continue;
                }
                let Some(drag) = &self.drag else {
                    return Reaction::Continue;
                };
                let Some(new_offset) = drag.offset_at(position.y) else {
                    return Reaction::Continue;
                };
                if new_offset == self.scroll_offset {
                    return Reaction::Continue;
                }
                self.scroll_offset = new_offset;
                Reaction::Redraw
            }
            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                if self.drag.take().is_some() {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }
}
