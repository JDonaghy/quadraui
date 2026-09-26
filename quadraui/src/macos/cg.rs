//! Shared CoreGraphics FFI bindings and small paint helpers used by
//! (most of) the per-primitive rasterisers in [`super`].
//!
//! Before this module existed, 28 files under `src/macos/` each
//! declared their own private `extern "C"` block for the handful of
//! `CGContext*` entry points every rasteriser needs (save/restore
//! state, clip, set fill/stroke colour, fill/stroke a rect, stroke a
//! line), and 24 of them carried their own byte-identical
//! `color_to_cg`/`fill_rect`/`stroke_rect` copies plus (11 of them) a
//! `CGRectExt::new_xywh` trait shim for building a `CGRect` from
//! `(x, y, w, h)` (quadraui#1071). This module is the single source
//! of truth for all of that; per-file copies are deleted in favour of
//! a glob import (`use super::cg::*;`) so call sites are unchanged.
//!
//! Not every `src/macos/` file that touches CoreGraphics draws through
//! here — `backend.rs`, `board.rs`, `headless.rs`, `image.rs`,
//! `pipeline_view.rs`, `text.rs`, `toolbar.rs`, and a handful of other
//! small rasterisers (`activity_bar.rs`, `command_center.rs`,
//! `completions.rs`, `form.rs`, `list.rs`, `menu_bar.rs`, `minimap.rs`,
//! `progress.rs`, `run.rs`, `text_selection.rs`, `message_list.rs`,
//! `spinner.rs`) still carry their own copies of some or all of this
//! (some also need extra entry points — `CGContextTranslateCTM`,
//! path-construction for rounded rects, image drawing, font/text
//! layout — that this module intentionally does not take on). Folding
//! those in is tracked as a follow-up once a macOS host can verify
//! each one; this module covers the rasterisers that were already
//! byte-identical, which is what quadraui#1071 asks for.
//!
//! ## A divergence found while consolidating (reported per #1071's
//! ## "report any divergence" instruction, not silently resolved)
//!
//! `macos/minimap.rs`'s own `fill_rect` (out of scope here, still
//! local to that file) additionally early-returns on `w <= 0.0 || h <=
//! 0.0`, which none of the other 20 `fill_rect` copies did. This
//! module's [`fill_rect`] intentionally matches the *majority* shape
//! (unguarded) rather than picking `minimap.rs`'s; [`fill_rect_alpha`]
//! below *does* carry that guard because both of its two source copies
//! (`editor.rs`, `minimap.rs`) agreed on it.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::CGContextRef;

use crate::types::Color;

extern "C" {
    pub(crate) fn CGContextSaveGState(c: CGContextRef);
    pub(crate) fn CGContextRestoreGState(c: CGContextRef);
    pub(crate) fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    pub(crate) fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    pub(crate) fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    pub(crate) fn CGContextSetLineWidth(c: CGContextRef, width: core_graphics::base::CGFloat);
    pub(crate) fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    pub(crate) fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
    pub(crate) fn CGContextMoveToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    pub(crate) fn CGContextAddLineToPoint(
        c: CGContextRef,
        x: core_graphics::base::CGFloat,
        y: core_graphics::base::CGFloat,
    );
    pub(crate) fn CGContextStrokePath(c: CGContextRef);
}

/// Build a `CGRect` from `(x, y, w, h)` — replaces the `CGRectExt::new_xywh`
/// trait shim that used to be copy-pasted into 11 files.
pub(crate) fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
}

/// Decompose a [`Color`] into the `(r, g, b, a)` `f64` tuple
/// `CGContextSetRGB{Fill,Stroke}Color` expects, each channel scaled to
/// `0.0..=1.0`.
pub(crate) fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

/// Fill the rect `(x, y, w, h)` with `c` (using `c`'s own alpha).
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub(crate) unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, rect(x, y, w, h));
}

/// Like [`fill_rect`], but the alpha channel is `alpha` instead of the
/// colour's own — mirrors GTK's `cr.set_source_rgba(r, g, b, alpha)`
/// for selection/overlay tints, where the opacity is carried separately
/// from the colour's own (always-opaque) `a` channel.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub(crate) unsafe fn fill_rect_alpha(
    ctx: CGContextRef,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    c: Color,
    alpha: f64,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (r, g, b, _) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, alpha);
    CGContextFillRect(ctx, rect(x, y, w, h));
}

/// Stroke the rect `(x, y, w, h)`'s outline with `c` at `line_width`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub(crate) unsafe fn stroke_rect(
    ctx: CGContextRef,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    c: Color,
    line_width: f64,
) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    CGContextStrokeRect(ctx, rect(x, y, w, h));
}

/// Stroke a single line segment from `(x0, y0)` to `(x1, y1)` with `c`
/// at `line_width`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub(crate) unsafe fn stroke_line(
    ctx: CGContextRef,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    c: Color,
    line_width: f64,
) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    CGContextMoveToPoint(ctx, x0, y0);
    CGContextAddLineToPoint(ctx, x1, y1);
    CGContextStrokePath(ctx);
}
