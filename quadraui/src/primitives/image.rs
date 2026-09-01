//! `Image` primitive: a raster image (SVG/PNG/JPEG) painted at a target
//! rect (#662).
//!
//! Motivated by vimcode wanting its app logo left of the `File` menu the
//! way VS Code does — that needs actual pixels, not a codepoint in an
//! icon font. Every other "icon" in quadraui ([`crate::types::Icon`],
//! the per-tab icon sidecar, `ActivityItem::icon`) is a font glyph with
//! an ASCII fallback; this primitive is the one place quadraui paints
//! bytes instead of a character.
//!
//! # Scope guard
//!
//! This is **chrome** — a fixed-size decorative element (an app logo, a
//! toolbar button glyph that has no font-icon equivalent). It is
//! deliberately **not**:
//! - an image-viewer widget (no zoom, pan, or multi-image gallery state),
//! - animation (no GIF/APNG frame stepping),
//! - a caching or asset-management layer (callers own the bytes/path
//!   they hand in; the backend decodes once per paint call, same as
//!   every other rasteriser in this crate).
//!
//! If a future need grows past this list, that is a new primitive or a
//! deliberate scope expansion — not a quiet addition here.
//!
//! # Decoding is a backend concern
//!
//! [`Image`] only carries a source (bytes or a path) plus layout
//! metadata; it never decodes pixels itself. Each backend's
//! `Backend::draw_image` decodes through its native stack — GTK via
//! `gdk_pixbuf`, macOS via `NSImage` — so the primitive stays free of
//! image-format dependencies. See that trait method's doc comment for
//! the per-backend contract, including why TUI is a legitimate
//! `Unsupported` rather than a silent no-op (#507).

use crate::event::Rect;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Where the image's encoded bytes come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageSource {
    /// Raw encoded image bytes (PNG/JPEG/SVG/...) — the backend's
    /// decoder sniffs the format from content, so no separate MIME hint
    /// is carried here.
    Bytes(Vec<u8>),
    /// Filesystem path to an image file, decoded lazily by the backend
    /// on each paint (see the module docs — no caching layer here).
    Path(std::path::PathBuf),
}

/// How an image's intrinsic size maps onto its target rect. Named after
/// the equivalent CSS `object-fit` keywords since the semantics are
/// identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ImageFit {
    /// Scale to fit entirely within the target rect, preserving aspect
    /// ratio. May letterbox (leave empty space on one axis).
    #[default]
    Contain,
    /// Scale to fill the target rect, preserving aspect ratio. May crop
    /// (overflow one axis, clipped to the rect).
    Cover,
    /// Stretch to exactly fill the target rect, ignoring aspect ratio.
    Fill,
    /// Paint at intrinsic size, anchored at the rect's top-left corner —
    /// no scaling. Backends clip to the caller's rect if the image is
    /// larger.
    None,
}

/// Declarative description of an `Image` widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub id: WidgetId,
    pub source: ImageSource,
    /// The image's natural pixel dimensions, if known up front. Required
    /// for [`ImageFit::Contain`] / [`ImageFit::Cover`] / [`ImageFit::None`]
    /// to compute a meaningful target rect via [`Image::layout`] — when
    /// `None`, [`Image::layout`] falls back to filling the whole target
    /// rect (equivalent to [`ImageFit::Fill`]) regardless of `fit`, since
    /// there is no aspect ratio to preserve. Backends that decode the
    /// image themselves (GTK/macOS) still learn the *real* intrinsic size
    /// from the decoded asset and may use that in preference to this
    /// hint; this field exists so [`Image::layout`] can be computed
    /// without decoding.
    pub intrinsic_size: Option<(u32, u32)>,
    pub fit: ImageFit,
    /// Required, not optional: TUI cannot paint raster pixels at all, so
    /// this is what actually renders there (see
    /// `Backend::draw_image`'s doc comment) — and it's what GTK/macOS
    /// paint nothing in place of on a decode failure (bad path, corrupt
    /// bytes), so a host can still show *something* on every backend and
    /// every failure mode. An empty string means "leave the rect blank",
    /// a legitimate choice for a purely decorative image.
    pub fallback_text: String,
}

/// Resolved target rect for painting an [`Image`] — the result of
/// applying `fit` against a bounding rect. See [`Image::layout`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageLayout {
    /// Where the image should actually be painted, in the same
    /// coordinate space as the `bounds` passed to [`Image::layout`].
    /// Always contained within `bounds` for [`ImageFit::Contain`] and
    /// [`ImageFit::Fill`]; may extend past `bounds` on one axis for
    /// [`ImageFit::Cover`] and [`ImageFit::None`] — backends are
    /// expected to clip painting to `bounds` in that case.
    pub bounds: Rect,
}

impl Image {
    /// Compute where this image should paint within `bounds`, honoring
    /// `fit`. Pure geometry — no decoding, so it works identically
    /// whether or not the backend has actually loaded `source` yet, and
    /// hosts can call it from `AppLogic::handle` (no `&mut Backend`
    /// needed) to reserve space before a click-routing pass, the same
    /// way every other primitive's `layout` works.
    pub fn layout(&self, bounds: Rect) -> ImageLayout {
        let Some((iw, ih)) = self.intrinsic_size else {
            // No intrinsic size to preserve an aspect ratio against —
            // filling the rect is the only sensible default.
            return ImageLayout { bounds };
        };
        if iw == 0 || ih == 0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return ImageLayout { bounds };
        }
        let iw = iw as f32;
        let ih = ih as f32;
        let target = match self.fit {
            ImageFit::Fill => bounds,
            ImageFit::None => Rect::new(bounds.x, bounds.y, iw, ih),
            ImageFit::Contain => fit_within(bounds, iw, ih, true),
            ImageFit::Cover => fit_within(bounds, iw, ih, false),
        };
        ImageLayout { bounds: target }
    }
}

/// Scale `(iw, ih)` to fit `bounds` — `contain` picks the smaller of the
/// two axis scales (never overflows `bounds`), `!contain` (cover) picks
/// the larger (never underflows `bounds`, may overflow one axis). Result
/// is centered within `bounds` on both axes, matching CSS
/// `object-fit: contain|cover` + `object-position: center`.
fn fit_within(bounds: Rect, iw: f32, ih: f32, contain: bool) -> Rect {
    let scale_x = bounds.width / iw;
    let scale_y = bounds.height / ih;
    let scale = if contain {
        scale_x.min(scale_y)
    } else {
        scale_x.max(scale_y)
    };
    let w = iw * scale;
    let h = ih * scale;
    let x = bounds.x + (bounds.width - w) / 2.0;
    let y = bounds.y + (bounds.height - h) / 2.0;
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(fit: ImageFit, intrinsic_size: Option<(u32, u32)>) -> Image {
        Image {
            id: WidgetId::new("img"),
            source: ImageSource::Path("/tmp/logo.png".into()),
            intrinsic_size,
            fit,
            fallback_text: "[LOGO]".into(),
        }
    }

    #[test]
    fn no_intrinsic_size_fills_bounds_regardless_of_fit() {
        let bounds = Rect::new(0.0, 0.0, 40.0, 20.0);
        let layout = image(ImageFit::Contain, None).layout(bounds);
        assert_eq!(layout.bounds, bounds);
    }

    #[test]
    fn fill_always_matches_bounds() {
        let bounds = Rect::new(5.0, 5.0, 40.0, 20.0);
        let layout = image(ImageFit::Fill, Some((100, 50))).layout(bounds);
        assert_eq!(layout.bounds, bounds);
    }

    #[test]
    fn none_paints_at_intrinsic_size_anchored_top_left() {
        let bounds = Rect::new(5.0, 5.0, 40.0, 20.0);
        let layout = image(ImageFit::None, Some((100, 50))).layout(bounds);
        assert_eq!(layout.bounds, Rect::new(5.0, 5.0, 100.0, 50.0));
    }

    #[test]
    fn contain_never_overflows_bounds_and_preserves_aspect_ratio() {
        // Intrinsic 200x100 (2:1) into a 40x40 square — contain must
        // shrink to 40x20 (limited by height... wait width) — width
        // scale = 40/200 = 0.2, height scale = 40/100 = 0.4, contain
        // picks the smaller (0.2) => 40x20.
        let bounds = Rect::new(0.0, 0.0, 40.0, 40.0);
        let layout = image(ImageFit::Contain, Some((200, 100))).layout(bounds);
        assert_eq!(layout.bounds.width, 40.0);
        assert_eq!(layout.bounds.height, 20.0);
        // Centered vertically within the 40-tall bounds.
        assert_eq!(layout.bounds.y, 10.0);
        assert_eq!(layout.bounds.x, 0.0);
        assert!(layout.bounds.width <= bounds.width);
        assert!(layout.bounds.height <= bounds.height);
    }

    #[test]
    fn cover_fills_bounds_on_the_shorter_axis_and_overflows_the_other() {
        // Same 200x100 image into the same 40x40 square — cover picks
        // the larger scale (0.4) => 80x40, overflowing width.
        let bounds = Rect::new(0.0, 0.0, 40.0, 40.0);
        let layout = image(ImageFit::Cover, Some((200, 100))).layout(bounds);
        assert_eq!(layout.bounds.width, 80.0);
        assert_eq!(layout.bounds.height, 40.0);
        // Centered horizontally, so it overflows both left and right.
        assert_eq!(layout.bounds.x, -20.0);
        assert_eq!(layout.bounds.y, 0.0);
    }

    #[test]
    fn zero_size_intrinsic_falls_back_to_bounds() {
        let bounds = Rect::new(0.0, 0.0, 40.0, 20.0);
        let layout = image(ImageFit::Contain, Some((0, 100))).layout(bounds);
        assert_eq!(layout.bounds, bounds);
    }
}
