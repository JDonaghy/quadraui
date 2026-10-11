//! GTK rasteriser for [`crate::Scrollbar`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] — see that
//! function's documentation for details. Callers reach the paint through
//! [`crate::Backend::draw_scrollbar`].
