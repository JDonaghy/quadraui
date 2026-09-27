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
//! [`super::backend::MacBackend::apply_selection_highlight`] resolves the
//! active selection all the way to pixel-space `Rect`s via
//! [`crate::backend_core::BackendCore::selection_highlight_rects`]
//! (#1090, folding in what used to be a separate
//! [`crate::paint_geometry::text_selection_highlight_rects`] call here)
//! and hands them, plus a colour resolved from
//! [`crate::theme::Theme::text_selection_highlight`], to
//! [`draw_selection_highlight`] — mirroring every other `draw_*` method
//! on `MacBackend`, which resolves geometry itself and delegates the
//! actual CG calls to a per-primitive submodule. This module now owns
//! only the real `CGContextFillRect` calls, which have no portable
//! shape.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::CGContextRef;

use crate::event::Rect;
use crate::types::Color;

/// Paint one translucent-blue rectangle per already-resolved pixel-space
/// `rect` — mirrors `WinBackend::apply_selection_highlight`'s identical
/// loop over Direct2D `FillRectangle` calls. The row/column → pixel-rect
/// resolution itself (`region_bounds` + cell ranges → `rects`) now lives
/// in [`crate::backend_core::BackendCore::selection_highlight_rects`]
/// (#1090), shared with the GTK/Win-GUI twins — this function only owns
/// the real `CGContextFillRect` calls, which have no portable shape.
///
/// `color` comes from [`crate::theme::Theme::text_selection_highlight`]
/// (#1090) rather than a hard-coded literal, but CG's fill-colour API
/// takes floats directly, so it's converted from `Color`'s 0-255 channels
/// here rather than round-tripping through a separate float constant the
/// way this function did before #1090.
///
/// # Safety
///
/// `ctx` must be a valid, non-null `CGContextRef` borrowed for the
/// duration of this call — i.e. called from inside
/// [`super::backend::MacBackend::enter_frame_scope`], same contract as
/// every other CG-calling function in `macos/`.
pub(crate) unsafe fn draw_selection_highlight(ctx: CGContextRef, rects: &[Rect], color: Color) {
    CGContextSetRGBFillColor(
        ctx,
        color.r as f64 / 255.0,
        color.g as f64 / 255.0,
        color.b as f64 / 255.0,
        color.a as f64 / 255.0,
    );
    for rect in rects {
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
    //! `crate::backend_core::BackendCore::selection_highlight_rects`, has
    //! its own host-independent tests in that module (#1090) — this
    //! module's own `mod tests` only covers what's genuinely local to it
    //! (the highlight colour's alpha, now sourced from `Theme`).

    /// [`crate::theme::Theme::text_selection_highlight`]'s alpha must
    /// stay translucent — a fully opaque highlight would hide the
    /// selected text underneath, unlike every other adopter of this
    /// shared look.
    #[test]
    fn highlight_alpha_is_translucent() {
        let a = crate::theme::Theme::default().text_selection_highlight().a;
        assert!(a > 0 && a < 255);
    }
}
