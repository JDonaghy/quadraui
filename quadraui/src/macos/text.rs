//! Core Text infrastructure — font creation, metrics, text measurement
//! and rendering against a borrowed `CGContextRef`.
//!
//! Issue #34 in the macOS backend milestone. Free-standing helpers that
//! `MacBackend` (landing in #35) will glue to the `Backend` trait
//! `current_line_height` / `current_char_width` stash. For #34 they
//! live as standalone functions so the smoke harness in
//! [`super::run`] can paint text without an enclosing backend.
//!
//! ## Coordinate convention for `draw_text`
//!
//! Callers pass `(x, y)` in **view-local points with top-left origin**
//! — matching the rest of quadraui. `draw_text` flips the text matrix
//! internally so glyphs render right-side up inside our flipped
//! `QuadraView`, and offsets by ascent so `y` corresponds to the top
//! of the glyph cell rather than the baseline.
//!
//! ## Why direct CoreGraphics FFI for the draw call
//!
//! The high-level `CTLine::draw(context: &CGContext)` wrapper expects
//! an owned `core_graphics::context::CGContext`, which would release
//! the pointer when dropped — fatal for the borrowed pointer AppKit
//! hands us inside `drawRect:`. We use the wrapper for line / font
//! construction and drop to `extern "C"` for `CTLineDraw` +
//! `CGContextSetTextPosition` + matrix manipulation.

use core_foundation::attributed_string::{CFAttributedString, CFAttributedStringRef};
use core_foundation::base::{CFAllocatorRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::base::CGFloat;
use core_graphics::geometry::CGAffineTransform;
use core_graphics::sys::CGContextRef;
use core_text::font::{self, CTFont};
use core_text::line::CTLine;
use core_text::string_attributes::{
    kCTFontAttributeName, kCTForegroundColorFromContextAttributeName,
};

/// Aggregate font measurements in points.
///
/// `line_height` follows the standard macOS convention
/// (`ascent + descent + leading`). `char_width` is the advance width
/// of the capital letter `M` — for monospace fonts every glyph shares
/// that advance; for proportional fonts it serves as a reasonable
/// "average glyph width" baseline used by primitives that lay out by
/// cell count (terminal, status bar, etc.).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    pub ascent: f64,
    pub descent: f64,
    pub leading: f64,
    pub line_height: f64,
    pub char_width: f64,
}

/// Create a [`CTFont`] for the named family at the given point size.
/// Returns `None` if the family is unknown to the system. Core Text
/// applies its own fallback chain for missing glyphs at render time;
/// this only flags "the family itself doesn't exist."
pub fn make_font(family: &str, size_pt: f64) -> Option<CTFont> {
    font::new_from_name(family, size_pt).ok()
}

/// Sample a font's typographic metrics. The returned `char_width`
/// is computed via `measure_text(font, "M")` — measuring an empty
/// string is meaningless for char-width, and `M` is the conventional
/// fixed-width gauge.
pub fn font_metrics(font: &CTFont) -> FontMetrics {
    let ascent = font.ascent();
    let descent = font.descent();
    let leading = font.leading();
    let (char_width, _) = measure_text(font, "M");
    FontMetrics {
        ascent,
        descent,
        leading,
        line_height: ascent + descent + leading,
        char_width,
    }
}

/// Measure the rendered footprint of `text` in `font`. Returns
/// `(width, height)` in points — height is the font's full line height
/// regardless of string content (matches how primitives reserve
/// vertical space).
pub fn measure_text(font: &CTFont, text: &str) -> (f64, f64) {
    if text.is_empty() {
        return (0.0, font.ascent() + font.descent() + font.leading());
    }
    let line = build_ctline(font, text);
    let bounds = line.get_typographic_bounds();
    (
        bounds.width,
        bounds.ascent + bounds.descent + bounds.leading,
    )
}

/// Paint `text` at `(x, y)` (view-local points, top-left origin) using
/// `font` and `color` (rgba 0.0–1.0 each). The CG context's clip
/// region is respected automatically by Core Text — callers that want
/// to clip to a rect call [`CGContextClipToRect`] before this.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// this call (typical: passed straight from `drawRect:`). Calling
/// with a freed or null pointer is UB.
pub unsafe fn draw_text(
    ctx: CGContextRef,
    font: &CTFont,
    text: &str,
    x: f64,
    y: f64,
    color: (f64, f64, f64, f64),
) {
    if text.is_empty() {
        return;
    }

    CGContextSaveGState(ctx);

    // Set the *fill* colour rather than embedding a foreground-colour
    // attribute on the CFAttributedString — Core Text falls back to
    // the context's current fill colour for unattributed glyphs.
    CGContextSetRGBFillColor(ctx, color.0, color.1, color.2, color.3);

    // Flip the text matrix so glyphs render right-side up inside
    // QuadraView (which has `isFlipped = YES`). Without this the
    // glyphs would draw upside-down because CG's intrinsic text
    // origin is at the baseline with ascent rising in +y, but our
    // view's +y points downward.
    let flip = CGAffineTransform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: -1.0,
        tx: 0.0,
        ty: 0.0,
    };
    CGContextSetTextMatrix(ctx, flip);

    // `y` is the requested **top** of the glyph in view coords;
    // CT positions the baseline. Shift down by ascent so the glyph
    // cell's top edge lands at the requested `y`.
    let ascent = font.ascent();
    CGContextSetTextPosition(ctx, x, y + ascent);

    let line = build_ctline(font, text);
    CTLineDraw(line.as_concrete_TypeRef(), ctx);

    CGContextRestoreGState(ctx);
}

/// Build a `CTLine` carrying just the font attribute. Foreground
/// colour is left to the context's fill (see [`draw_text`]).
///
/// `core_foundation::CFAttributedString::new` doesn't take attributes,
/// so we call `CFAttributedStringCreate` directly via FFI and wrap
/// the resulting `+1`-retained ref in our own CFAttributedString.
fn build_ctline(font: &CTFont, text: &str) -> CTLine {
    let font_key = unsafe { CFString::wrap_under_get_rule(kCTFontAttributeName) };
    // Tell Core Text to honour the graphics context's current fill
    // colour for glyph rendering. Without this attribute CT defaults
    // to **black**, regardless of what `CGContextSetRGBFillColor`
    // was set to right before `CTLineDraw` — Core Text reads its
    // foreground from the attributed string, not the context state,
    // unless this flag is explicitly true.
    let from_ctx_key =
        unsafe { CFString::wrap_under_get_rule(kCTForegroundColorFromContextAttributeName) };
    let attributes = CFDictionary::from_CFType_pairs(&[
        (font_key, font.as_CFType()),
        (from_ctx_key, CFBoolean::true_value().as_CFType()),
    ]);
    let cf_text = CFString::new(text);
    // SAFETY: `cf_text` and `attributes` outlive the call. The create
    // function returns a `+1`-retained ref which `wrap_under_create_rule`
    // takes ownership of (matching CF's release-on-Drop semantics).
    let attr_ref: CFAttributedStringRef = unsafe {
        CFAttributedStringCreate(
            std::ptr::null(),
            cf_text.as_concrete_TypeRef(),
            attributes.as_concrete_TypeRef(),
        )
    };
    let attr_string = unsafe { CFAttributedString::wrap_under_create_rule(attr_ref) };
    CTLine::new_with_attributed_string(attr_string.as_concrete_TypeRef())
}

// ── Direct CoreGraphics / CoreText FFI ──────────────────────────────────────
//
// All these are linked transitively via core-graphics + core-text. The
// wrappers in those crates either take ownership of the `CGContextRef`
// (which we can't grant — AppKit owns it for the duration of
// `drawRect:`) or hide the text matrix / position calls behind types
// that don't compose with our smoke flow.

#[link(name = "CoreText", kind = "framework")]
extern "C" {
    fn CTLineDraw(line: core_text::line::CTLineRef, context: CGContextRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFAttributedStringCreate(
        alloc: CFAllocatorRef,
        str: CFStringRef,
        attributes: CFDictionaryRef,
    ) -> CFAttributedStringRef;
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: CGFloat,
        green: CGFloat,
        blue: CGFloat,
        alpha: CGFloat,
    );
    fn CGContextSetTextMatrix(c: CGContextRef, t: CGAffineTransform);
    fn CGContextSetTextPosition(c: CGContextRef, x: CGFloat, y: CGFloat);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// System-installed monospace font — present on every macOS install
    /// since 10.6. Picking a known-resident family makes the tests
    /// deterministic without reading the user's font preferences.
    const TEST_FONT: &str = "Menlo";
    const TEST_SIZE: f64 = 14.0;

    fn font() -> CTFont {
        make_font(TEST_FONT, TEST_SIZE).expect("Menlo should be installed on every macOS host")
    }

    #[test]
    fn make_font_existing_returns_some() {
        assert!(make_font("Menlo", 12.0).is_some());
    }

    #[test]
    fn font_metrics_structure_consistent() {
        let m = font_metrics(&font());
        assert!(
            m.ascent > 0.0,
            "ascent should be positive, got {}",
            m.ascent
        );
        assert!(
            m.descent >= 0.0,
            "descent should be non-negative, got {}",
            m.descent
        );
        assert!(
            m.leading >= 0.0,
            "leading should be non-negative, got {}",
            m.leading
        );
        // line_height ≡ ascent + descent + leading by definition
        let expected = m.ascent + m.descent + m.leading;
        assert!(
            (m.line_height - expected).abs() < 1e-9,
            "line_height {} should equal ascent + descent + leading = {}",
            m.line_height,
            expected,
        );
        // Plausibility: a 14pt font should produce a line height in
        // roughly the 14–28pt range (varies a bit by font).
        assert!(
            m.line_height > 12.0 && m.line_height < 30.0,
            "line_height {} out of plausible range for 14pt Menlo",
            m.line_height,
        );
    }

    #[test]
    fn font_metrics_char_width_positive() {
        let m = font_metrics(&font());
        assert!(
            m.char_width > 0.0 && m.char_width < TEST_SIZE * 2.0,
            "char_width {} out of expected range for 14pt Menlo",
            m.char_width,
        );
    }

    #[test]
    fn measure_text_empty_string() {
        let (w, h) = measure_text(&font(), "");
        assert_eq!(w, 0.0);
        assert!(
            h > 0.0,
            "empty-string height should still report line height"
        );
    }

    #[test]
    fn measure_text_single_char_matches_char_width_for_monospace() {
        let f = font();
        let m = font_metrics(&f);
        let (w, _) = measure_text(&f, "M");
        // Menlo is monospace — width of "M" should equal `char_width`
        // exactly (modulo floating-point round-trip).
        assert!(
            (w - m.char_width).abs() < 1e-6,
            "single-char width {} should equal char_width {}",
            w,
            m.char_width,
        );
    }

    #[test]
    fn measure_text_scales_linearly_for_monospace() {
        let f = font();
        let (w1, _) = measure_text(&f, "x");
        let (w10, _) = measure_text(&f, "xxxxxxxxxx");
        // Monospace: 10 chars should be 10× one char (within rounding).
        let ratio = w10 / w1;
        assert!(
            (ratio - 10.0).abs() < 0.05,
            "10× width ratio was {}, expected ~10 for monospace",
            ratio,
        );
    }
}
