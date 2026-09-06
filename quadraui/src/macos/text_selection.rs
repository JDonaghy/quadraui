//! macOS text-selection highlight painting (#803).
//!
//! [`draw_selection_highlight`] is the CoreGraphics twin of
//! `GtkBackend::apply_selection_highlight`'s Cairo `fill()` call and
//! `WinBackend::apply_selection_highlight`'s Direct2D `FillRectangle`
//! call — the one piece of the shared `crate::text_selection` adoption
//! pattern that has no portable shape (real toolkit paint code). See
//! that module's doc for what stays shared (the region registry, the
//! active-selection state machine, the pixel↔cell row/column math) vs.
//! what each pixel-based backend owns itself.
//!
//! [`super::backend::MacBackend::apply_selection_highlight`] resolves
//! the active selection into `(row, col_start, col_end)` ranges via the
//! shared [`crate::text_selection::pixel_selection_ranges`] and hands
//! them to [`draw_selection_highlight`] here — mirroring every other
//! `draw_*` method on `MacBackend`, which resolves geometry itself and
//! delegates the actual CG calls to a per-primitive submodule.
//!
//! The range→rect arithmetic itself lives in
//! [`crate::paint_geometry::text_selection_highlight_rects`] (#857):
//! that module has no `target_os` gate, unlike this one (`mod macos` is
//! gated whole — see `lib.rs`'s comment on that `pub mod macos` arm), so
//! its tests run on every host including this repo's ordinary Linux CI,
//! rather than only on `macos-latest`. [`draw_selection_highlight`]
//! below just turns each computed rect into one real
//! `CGContextFillRect` call.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::CGContextRef;

use crate::event::Rect;

/// Same translucent-blue highlight `GtkBackend::apply_selection_highlight`
/// paints (`rgba(0.39, 0.58, 1.0, 0.30)`) and `WinBackend`'s Direct2D twin
/// approximates (`Color::rgba(100, 148, 255, 77)`) — kept as the exact
/// float form here since CG's fill-colour API takes floats directly, with
/// no 0-255 `Color` round-trip in between.
const HIGHLIGHT_RGBA: (f64, f64, f64, f64) = (0.39, 0.58, 1.0, 0.30);

/// Paint one translucent-blue rectangle per `(row_cell, col_start, col_end)`
/// range (see [`crate::text_selection::pixel_selection_ranges`]).
/// `region_bounds` is the selected `TextRegion`'s bounds in points;
/// `char_w`/`line_h` convert the cell-relative ranges back to points —
/// mirrors `WinBackend::apply_selection_highlight`'s identical loop over
/// Direct2D `FillRectangle` calls.
///
/// Skips zero-or-negative-width ranges (matches the GTK/Win twins' guard).
///
/// # Safety
///
/// `ctx` must be a valid, non-null `CGContextRef` borrowed for the
/// duration of this call — i.e. called from inside
/// [`super::backend::MacBackend::enter_frame_scope`], same contract as
/// every other CG-calling function in `macos/`.
pub(crate) unsafe fn draw_selection_highlight(
    ctx: CGContextRef,
    region_bounds: Rect,
    ranges: &[(u16, f32, f32)],
    char_w: f64,
    line_h: f64,
) {
    let (r, g, b, a) = HIGHLIGHT_RGBA;
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    for rect in
        crate::paint_geometry::text_selection_highlight_rects(region_bounds, ranges, char_w, line_h)
    {
        CGContextFillRect(
            ctx,
            CGRect::new(
                &CGPoint::new(rect.x as f64, rect.y as f64),
                &CGSize::new(rect.width as f64, rect.height as f64),
            ),
        );
    }
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    //! `draw_selection_highlight` itself needs a live `CGContextRef` (real
    //! CoreGraphics FFI), so it's exercised via
    //! `MacBackend::apply_selection_highlight` against a
    //! `super::super::headless::BitmapSurface` in `backend.rs`'s own test
    //! module (pixel-readback assertion) rather than here. The rect math
    //! it now delegates to,
    //! `crate::paint_geometry::text_selection_highlight_rects`, has its
    //! own host-independent tests in that module (#857) — this module's
    //! own `mod tests` only covers what's genuinely local to it (the
    //! highlight colour's alpha).
    use super::*;

    /// `HIGHLIGHT_RGBA`'s alpha must stay translucent — a fully opaque
    /// highlight would hide the selected text underneath, unlike every
    /// other adopter of this shared look.
    #[test]
    fn highlight_alpha_is_translucent() {
        assert!(HIGHLIGHT_RGBA.3 > 0.0 && HIGHLIGHT_RGBA.3 < 1.0);
    }
}
