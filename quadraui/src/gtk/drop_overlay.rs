//! GTK rasteriser for [`crate::DropOverlay`].
//!
//! Painting moved to the shared
//! [`crate::primitives::drop_zone::native_surface_paint::paint`] (#865,
//! `PaintSurface` Phase 2d slice 8/9) — see that fn's doc for the one
//! named divergence (Windows's pre-migration CPU-premixed highlight,
//! unlike GTK/macOS's real alpha blend) found and reported while
//! unifying `gtk::draw_drop_overlay`, `macos::drop_overlay::
//! draw_drop_overlay` and `win::drop_overlay::draw_drop_overlay` into
//! one implementation. The deprecated `draw_drop_overlay` compatibility
//! shim over the shared [`super::surface::CairoSurface`] adapter was
//! removed in issue #1109 (zero uses in coord-tui's `main` and
//! vimcode's `develop`); callers reach the same paint through
//! [`crate::Backend::draw_drop_overlay`].
