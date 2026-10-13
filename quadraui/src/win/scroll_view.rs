//! Windows (Direct2D) rasteriser for [`crate::ScrollView`].
//!
//! [`crate::ScrollView`] owns scroll-offset and visible-range state but
//! never touches a [`crate::Backend`] — painting is the host's job (see
//! that type's module doc). On win that job composes two pieces that are
//! already generic across every pixel backend, so this module adds no new
//! `Backend` trait method:
//!
//! - **scrollbar chrome** — [`crate::Backend::draw_scrollbar`] paints
//!   whatever [`crate::Scrollbar`] [`ScrollView::scrollbar`] computes;
//!   [`draw_scrollbar_for`] skips the call entirely when the content fits
//!   (no scrollbar to draw).
//! - **content clipping** — [`crate::Backend::paint_surface`]'s
//!   `surface_push_clip`/`surface_pop_clip` narrow child painting to the
//!   viewport so content laid out past its edges doesn't bleed outside it;
//!   [`paint_clipped`] wires the push/pop pairing so callers can't
//!   mismatch them.
//!
//! Wheel scrolling and scrollbar-drag *event handling* stay out of this
//! module: [`ScrollView::handle_wheel`] and
//! [`crate::primitives::scrollbar::ThumbDrag`] are already backend-agnostic
//! — a host calls them directly from its [`crate::AppLogic::handle`], the
//! same way `examples/common/scroll_view_demo.rs` does.

use crate::{Backend, Rect, ScrollView};

/// Paint `view`'s scrollable child content clipped to its viewport.
///
/// Pushes a clip region matching `view.viewport` before running
/// `paint_content`, and pops it afterward. On a backend with no
/// [`crate::PaintSurface`] (TUI has no sub-cell clip concept — see that
/// trait's module doc), `paint_content` still runs, just unclipped, since
/// there is nothing to push.
pub fn paint_clipped<B: Backend + ?Sized>(
    backend: &mut B,
    view: &ScrollView,
    paint_content: impl FnOnce(&mut B),
) {
    let pushed = if let Some(surface) = backend.paint_surface() {
        surface.surface_push_clip(view.viewport);
        true
    } else {
        false
    };
    paint_content(backend);
    if pushed {
        if let Some(surface) = backend.paint_surface() {
            surface.surface_pop_clip();
        }
    }
}

/// Paint `view`'s scrollbar along `track`, if its content is longer than
/// its viewport. A no-op when [`ScrollView::scrollbar`] returns `None`
/// (nothing to scroll — e.g. the viewport already fits all the content).
pub fn draw_scrollbar_for<B: Backend + ?Sized>(
    backend: &mut B,
    view: &ScrollView,
    track: Rect,
    min_thumb_len: f32,
) {
    if let Some(scrollbar) = view.scrollbar(track, min_thumb_len) {
        backend.draw_scrollbar(track, &scrollbar);
    }
}
