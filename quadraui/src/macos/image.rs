//! macOS stand-in for the [`Image`] primitive (#802).
//!
//! #662 scoped the `Image` rasteriser to GTK only for its first pass —
//! macOS's natural decoder is `NSImage`, not `gdk_pixbuf`, and wiring
//! that up is real, backend-specific work, not a shim over shared logic
//! (the same shape of gap `minimap.rs` describes for `Minimap`'s paint
//! calls). Leaving `Backend::draw_image` as a reachable `todo!()` here
//! was a correctness bug, not a scope decision, though: an app that
//! calls it on macOS took the whole host down with a panic instead of
//! degrading the way [`ImagePaintResult::Unsupported`] already lets TUI
//! degrade for its own categorical "no pixel grid" case. This module is
//! that same honest answer for macOS, until a real `NSImage` decoder
//! lands here.
//!
//! [`Image`]: crate::primitives::image::Image

use crate::backend::ImagePaintResult;

/// Report [`ImagePaintResult::Unsupported`] and paint nothing — macOS has
/// no `NSImage` decode path yet (#662's first pass scoped GTK only;
/// #802 closes the panic gap without attempting that decoder). Mirrors
/// `Backend::draw_image`'s TUI behaviour: a host asking for an image on
/// macOS gets a clean, inspectable "nothing painted" answer rather than
/// a crash or a silent no-op.
pub fn mac_draw_image() -> ImagePaintResult {
    ImagePaintResult::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_draw_image_reports_unsupported() {
        assert_eq!(mac_draw_image(), ImagePaintResult::Unsupported);
    }
}
