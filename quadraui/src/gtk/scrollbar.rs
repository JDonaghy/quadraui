//! GTK rasteriser for [`crate::Scrollbar`].
//!
//! Painting moved to the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] (#811,
//! `NativeSurface` Phase 2d) — see that fn's doc for the one named
//! divergence (quadraui#791) re-verified (already fixed) while unifying
//! `gtk::draw_scrollbar`, `macos::scrollbar::draw_scrollbar` and
//! `win::scrollbar::draw_scrollbar` into one implementation. The
//! deprecated `draw_scrollbar` compatibility shim over the shared
//! [`super::surface::CairoSurface`] adapter (#1072 — consolidated from
//! this module's own private `RawScrollbarSurface`, used by any
//! free-function rasteriser too (`gtk::data_table`, `gtk::list`) that
//! paints an embedded scrollbar from only a `cr: &Context` — not a live
//! `GtkBackend`) was removed in issue #1109 (zero uses in coord-tui's
//! `main` and vimcode's `develop`); callers reach the same paint
//! through [`crate::Backend::draw_scrollbar`].
