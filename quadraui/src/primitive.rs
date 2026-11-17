//! The [`Primitive`] trait — a single `layout()` + `paint()` pair a
//! widget outside this crate implements to compose with every pixel
//! [`crate::Backend`] through one generic entry point,
//! [`crate::DrawsPrimitives::draw_primitive`], instead of needing a bespoke
//! `draw_*`/`*_layout` pair added to the sealed [`crate::Backend`]
//! trait itself.
//!
//! # Why this exists
//!
//! Every built-in primitive (`ListView`, `TreeView`, `DataTable`, …)
//! gets its own pair of [`crate::Backend`] methods — `draw_list` /
//! `list_layout`, `draw_tree` / `tree_layout`, and so on — because
//! `Backend` is sealed and each pair is a deliberate, versioned
//! addition reviewed per primitive (see `docs/decisions/
//! BACKEND_TRAIT_PROPOSAL.md` §4). A consumer that wants to paint its
//! own widget through the same pixel backends GTK/macOS/Win already
//! implement has had no portable way to do that without one of those
//! three backends growing a method for it.
//!
//! `Primitive` is that seam, one level up from [`crate::PaintSurface`]
//! ("the unsealed paint seam"): a widget built against `Primitive`
//! composes `layout()` + `paint()` the same way every built-in
//! primitive's `native_surface_paint::paint` helper already does
//! internally, and [`crate::DrawsPrimitives::draw_primitive`]'s provided
//! default runs both against [`crate::Backend::paint_surface`] for any
//! backend that implements [`crate::PaintSurface`] (today: GTK, macOS,
//! Win) — zero new `Backend` methods, zero per-backend opt-in.
//!
//! # Design — `layout()` bakes in what `paint()` needs
//!
//! Per Decision D6, a primitive's `layout()` call resolves every
//! geometry decision up front and returns a fully-resolved `Layout` a
//! backend rasterises verbatim. `Primitive` carries that one step
//! further: [`Primitive::paint`] takes only `Self::Layout`, `&mut dyn
//! PaintSurface`, `&Theme` and `&Style` — no `Rect`, no font metrics —
//! so a `Layout` type must bake in whatever absolute geometry (the
//! painted area's origin, the resolved row height, …) its own
//! `paint()` needs. [`crate::ListView`]'s impl does this via
//! [`crate::primitives::list::ListViewPrimitiveLayout`], which pairs
//! its existing list-local [`crate::ListViewLayout`] with the absolute
//! area it was laid out into.
//!
//! # Hit-testing lives on `Layout`, not on this trait
//!
//! Every built-in `*Layout` type already carries its own `hit_test`
//! (e.g. [`crate::ListViewLayout::hit_test`]) because the right
//! hit-testing *shape* (a flat index, a tree path, a column/row pair,
//! …) differs per primitive — forcing one signature onto this trait
//! would either lose information or force every implementor through a
//! lossy common type. A `Primitive` impl's `Layout` is expected to
//! expose its own `hit_test`-shaped method the same way; see
//! [`crate::primitives::list::ListViewPrimitiveLayout::hit_test`].
//!
//! # `A11yPrimitive` supertrait
//!
//! [`Primitive`] requires [`A11yPrimitive`] — a widget painted through
//! this trait reports accessibility info through the same
//! [`crate::a11y`] machinery every built-in primitive already does,
//! rather than a parallel opt-in a consumer could forget to implement.
//!
//! # Example — a third-party widget
//!
//! A type defined entirely outside this crate, with no change to
//! `quadraui::Backend`/`quadraui::gtk`/`quadraui::macos`/`quadraui::win`:
//!
//! ```
//! use quadraui::primitive::{LayoutCtx, Primitive};
//! use quadraui::{A11yInfo, A11yPrimitive, Color, PaintSurface, Rect, Style, Theme};
//!
//! /// A single filled swatch.
//! struct Swatch {
//!     color: Color,
//! }
//!
//! impl A11yPrimitive for Swatch {
//!     fn a11y_info(&self) -> A11yInfo {
//!         A11yInfo::default()
//!     }
//! }
//!
//! impl Primitive for Swatch {
//!     type Layout = Rect;
//!     type Event = ();
//!
//!     fn layout(&self, _ctx: &LayoutCtx, rect: Rect) -> Rect {
//!         rect
//!     }
//!
//!     fn paint(
//!         &self,
//!         layout: &Rect,
//!         surface: &mut dyn PaintSurface,
//!         _theme: &Theme,
//!         _style: &Style,
//!     ) {
//!         surface.surface_fill_rect(*layout, self.color);
//!     }
//! }
//!
//! # use quadraui::{Image, Point, Viewport};
//! # use quadraui::backend::ImagePaintResult;
//! # #[derive(Default)]
//! # struct NullSurface;
//! # impl PaintSurface for NullSurface {
//! #     fn surface_begin_frame(&mut self, _v: Viewport) {}
//! #     fn surface_end_frame(&mut self) {}
//! #     fn surface_viewport(&self) -> Viewport { Viewport::new(0.0, 0.0, 1.0) }
//! #     fn surface_line_height(&self) -> f32 { 16.0 }
//! #     fn surface_char_width(&self) -> f32 { 8.0 }
//! #     fn surface_measure_text(&self, _t: &str) -> (f32, f32) { (0.0, 0.0) }
//! #     fn surface_fill_rect(&mut self, _r: Rect, _c: Color) {}
//! #     fn surface_fill_rounded_rect(&mut self, _r: Rect, _radius: f32, _c: Color) {}
//! #     fn surface_stroke_rect(&mut self, _r: Rect, _c: Color, _w: f32) {}
//! #     fn surface_draw_text_run(&mut self, _r: Rect, _t: &str, _c: Color) {}
//! #     fn surface_draw_line(&mut self, _from: Point, _to: Point, _c: Color, _w: f32) {}
//! #     fn surface_push_clip(&mut self, _r: Rect) {}
//! #     fn surface_pop_clip(&mut self) {}
//! #     fn surface_draw_image(&mut self, _r: Rect, _i: &Image) -> ImagePaintResult {
//! #         ImagePaintResult::Unsupported
//! #     }
//! # }
//! // Any `&mut dyn PaintSurface` paints it — this is what
//! // `DrawsPrimitives::draw_primitive`'s default body does internally, against
//! // whatever `Backend::paint_surface` hands back.
//! let swatch = Swatch { color: Color::rgb(255, 0, 0) };
//! let ctx = LayoutCtx {
//!     line_height: 16.0,
//!     char_width: 8.0,
//!     nerd_fonts_enabled: false,
//! };
//! let mut surface = NullSurface::default();
//! let layout = swatch.layout(&ctx, Rect::new(0.0, 0.0, 10.0, 10.0));
//! swatch.paint(&layout, &mut surface, &Theme::default(), &Style::default());
//! ```

use crate::a11y::A11yPrimitive;
use crate::event::Rect;
use crate::paint_surface::PaintSurface;
use crate::style::Style;
use crate::theme::Theme;

/// Backend-measured geometry a [`Primitive::layout`] call needs that a
/// bare [`Rect`] doesn't carry.
///
/// Mirrors the handful of [`crate::Backend`] measurement methods every
/// pixel backend already exposes (`chrome_line_height`, `char_width`,
/// `nerd_fonts_enabled`) — [`crate::DrawsPrimitives::draw_primitive`]'s default
/// body builds one of these from `self` before calling
/// [`Primitive::layout`], so a `Primitive` impl never has to reach for
/// `&dyn Backend` itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutCtx {
    /// One standard row's height, in the backend's native units — see
    /// [`crate::Backend::chrome_line_height`].
    pub line_height: f32,
    /// Approximate character advance, in the backend's native units —
    /// see [`crate::Backend::char_width`].
    pub char_width: f32,
    /// Whether the backend has a Nerd Font icon face registered — see
    /// [`crate::Backend::nerd_fonts_enabled`].
    pub nerd_fonts_enabled: bool,
}

/// A widget a pixel [`crate::Backend`] can paint without a bespoke
/// `draw_*`/`*_layout` method pair. See the module doc for the full
/// contract.
pub trait Primitive: A11yPrimitive {
    /// This primitive's fully-resolved layout — bakes in every
    /// geometry decision [`Self::paint`] needs (see the module doc's
    /// "Design" section), and typically exposes its own
    /// `hit_test`-shaped method.
    type Layout;

    /// The event(s) this primitive emits in response to input. Not
    /// produced by [`Self::layout`]/[`Self::paint`] themselves — a host
    /// derives these from [`crate::UiEvent`] plus `Self::Layout`'s own
    /// hit-testing, the same way every built-in primitive's
    /// compose-layer controller already does.
    type Event;

    /// Resolve this primitive's layout for `rect`.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout;

    /// Paint `layout` (as produced by [`Self::layout`]) onto `surface`.
    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    );

    /// Paint `layout` onto a TUI cell buffer — [`crate::tui::backend::TuiBackend`]'s
    /// paint entry point ([`crate::tui::backend::TuiBackend::draw_primitive`])
    /// calls this instead of [`Self::paint`], since TUI has no
    /// [`PaintSurface`] to hand it (see that trait's module doc, "Why TUI
    /// stays out").
    ///
    /// # Why a second method instead of a cell-quantised `PaintSurface`
    ///
    /// A real prototype (`crate::primitives::list::ListView`'s pilot
    /// impl) ruled out a `TuiBackend: PaintSurface` impl that reproduces
    /// `crate::tui::draw_list`'s output byte-for-byte: `ListView::bordered`'s
    /// frame is painted as literal box-drawing glyphs (`╭─╮│╰─╯`) chosen
    /// per-corner, which [`PaintSurface::surface_stroke_rect`]/
    /// [`PaintSurface::surface_draw_path`]'s axis-aligned-rect and
    /// straight-segment verbs have no way to express — a `PaintSurface`
    /// verb general enough to pick "this exact glyph at this exact
    /// corner" stops being a backend-agnostic drawing primitive and
    /// becomes a terminal-box-drawing API wearing a `PaintSurface`
    /// costume. Row text also carries its own cell-count accounting
    /// (`crate::text_util::display_width`/`char_cell_width`, double-width
    /// CJK glyphs occupying two cells) that a pixel-shaped
    /// `surface_draw_text_run` has no parallel for — a font-shaped text
    /// run has no "how many terminal columns wide" concept to quantise
    /// to. Both gaps are structural, not missing verbs that a future
    /// `PaintSurface` addition could close, so TUI gets this second,
    /// cell-buffer-flavored method instead — mirroring how
    /// [`crate::tui::draw_list`] itself already exists as a parallel
    /// rasteriser to the shared `native_surface_paint` helper, not a
    /// `PaintSurface` implementor.
    ///
    /// Takes `&mut ratatui::buffer::Buffer` directly (the same type
    /// [`crate::tui::draw_list`] itself takes) rather than inventing a
    /// quadraui-local cell-surface abstraction — every `tui`-feature
    /// consumer already depends on the same `ratatui` version to call
    /// [`crate::tui::draw_list`] and friends, so this adds no new
    /// dependency surface.
    ///
    /// `#[cfg(feature = "tui")]`: unlike [`Self::paint`], which exists
    /// unconditionally because every pixel backend needs it, nothing
    /// outside a `tui`-enabled build can construct a `ratatui::buffer::Buffer`
    /// to call this with.
    ///
    /// Default: no-op — correct for the common case of a `Primitive`
    /// implementor with no TUI story at all (every built-in primitive
    /// except [`crate::ListView`]'s pilot today); such an implementor
    /// painted through [`crate::tui::backend::TuiBackend::draw_primitive`]
    /// still gets a real, hit-testable [`Self::Layout`] back, just no
    /// pixels on screen, the same "resolve layout, maybe paint" contract
    /// [`crate::backend::DrawsPrimitives::draw_primitive`]'s own default
    /// already has for a `PaintSurface`-less backend.
    #[cfg(feature = "tui")]
    fn paint_cells(&self, layout: &Self::Layout, buf: &mut ratatui::buffer::Buffer, theme: &Theme) {
        let _ = (layout, buf, theme);
    }
}
