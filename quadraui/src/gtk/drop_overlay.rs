//! GTK rasteriser for [`crate::DropOverlay`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::drop_zone::native_surface_paint::paint`] — see that
//! function's documentation for details on platform divergences (Windows's
//! pre-multiplied alpha versus GTK/macOS's real alpha blend).
