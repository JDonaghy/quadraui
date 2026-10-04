//! GTK rasteriser for [`crate::primitives::diff_view::DiffView`].
//!
//! Painting moved to the shared
//! [`crate::primitives::diff_view::native_surface_paint::paint`] (#866,
//! `PaintSurface` Phase 2d slice 9/9) — see that fn's doc for the two
//! named divergences (row/header text vertical alignment; header-label
//! ellipsize vs. hard-clip) found while unifying
//! `gtk::diff_view::draw_diff_view`, `macos::diff_view::draw_diff_view`
//! and `win::diff_view::draw_diff_view` into one implementation. The
//! deprecated `draw_diff_view` compatibility shim over the shared
//! [`super::surface::CairoSurface`] adapter was removed in issue #1109
//! (zero uses in coord-tui's `main` and vimcode's `develop`); callers
//! reach the same paint through [`crate::Backend::draw_diff_view`].
