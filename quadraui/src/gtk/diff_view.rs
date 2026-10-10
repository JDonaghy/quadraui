//! GTK rasteriser for [`crate::primitives::diff_view::DiffView`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::diff_view::native_surface_paint::paint`] — see that
//! fn's doc for two named divergences (row/header text vertical alignment;
//! header-label ellipsize vs. hard-clip) between platform implementations.
//! Callers reach the paint through [`crate::Backend::draw_diff_view`].