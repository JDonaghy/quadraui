//! GTK rasteriser for [`crate::Scrollbar`].
//!
//! Cairo overlay-style scrollbar: a thin coloured track with a slightly
//! brighter thumb on top, both painted with alpha so the bar hovers
//! over the editor content without obscuring it. Hover and drag states
//! modulate the alpha (track gets brighter on hover; thumb gets
//! progressively brighter on hover → drag).

use gtk4::cairo::Context;

use super::cairo_rgb;
use crate::primitives::scrollbar::Scrollbar;
use crate::theme::Theme;

/// Draw a [`Scrollbar`] onto a Cairo context.
///
/// Track and thumb are read from `scrollbar` directly — no math happens
/// here. The track rect is filled with `theme.scrollbar_track` at low
/// alpha; the thumb rect (positioned by `scrollbar.thumb_start` along
/// `scrollbar.axis`) is filled with `theme.scrollbar_thumb` at higher
/// alpha. Hover / drag state on the primitive bumps the alphas so the
/// bar is more visible while the user is interacting with it.
///
/// Both axes share this implementation — the `axis` field of the
/// primitive determines whether `thumb_start` / `thumb_len` are applied
/// vertically or horizontally.
pub fn draw_scrollbar(cr: &Context, scrollbar: &Scrollbar, theme: &Theme) {
    let track = scrollbar.track;
    if track.width <= 0.0 || track.height <= 0.0 {
        return;
    }

    let track_alpha = if scrollbar.hovered || scrollbar.dragging {
        0.35
    } else {
        0.20
    };
    let thumb_alpha = if scrollbar.dragging {
        0.85
    } else if scrollbar.hovered {
        0.70
    } else {
        0.50
    };

    let (tr, tg, tb) = cairo_rgb(theme.scrollbar_track);
    cr.set_source_rgba(tr, tg, tb, track_alpha);
    cr.rectangle(
        track.x as f64,
        track.y as f64,
        track.width as f64,
        track.height as f64,
    );
    cr.fill().ok();

    let (thr, thg, thb) = cairo_rgb(theme.scrollbar_thumb);
    cr.set_source_rgba(thr, thg, thb, thumb_alpha);
    let (tx, ty, tw, th) = match scrollbar.axis {
        crate::primitives::scrollbar::ScrollAxis::Vertical => (
            track.x as f64,
            track.y as f64 + scrollbar.thumb_start as f64,
            track.width as f64,
            scrollbar.thumb_len as f64,
        ),
        crate::primitives::scrollbar::ScrollAxis::Horizontal => (
            track.x as f64 + scrollbar.thumb_start as f64,
            track.y as f64,
            scrollbar.thumb_len as f64,
            track.height as f64,
        ),
    };
    cr.rectangle(tx, ty, tw, th);
    cr.fill().ok();
}
