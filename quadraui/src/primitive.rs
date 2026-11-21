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
    /// [`crate::Backend::chrome_line_height`]. Already run through
    /// [`Self::resolve_row_height`] against the calling backend's
    /// [`crate::Backend::style`] by
    /// [`crate::DrawsPrimitives::draw_primitive`], so a `Primitive` impl
    /// reads this field directly rather than re-applying that floor
    /// itself.
    pub line_height: f32,
    /// Approximate character advance, in the backend's native units —
    /// see [`crate::Backend::char_width`].
    pub char_width: f32,
    /// Whether the backend has a Nerd Font icon face registered — see
    /// [`crate::Backend::nerd_fonts_enabled`].
    pub nerd_fonts_enabled: bool,
}

impl LayoutCtx {
    /// Floor `line_height` at `style.row_height`, the same row-pitch
    /// resolution every pixel backend's own private `resolved_row_height`
    /// helper already applies on its bespoke `draw_list`/`draw_tree`/
    /// `draw_data_table` paths (e.g. `GtkBackend::resolved_row_height`,
    /// `MacBackend::resolved_row_height`) before laying out rows — a
    /// chrome font taller than the theme's `row_height` token must never
    /// be clipped.
    ///
    /// [`crate::backend::DrawsPrimitives::draw_primitive`] applies this to
    /// the raw [`crate::Backend::chrome_line_height`] it measures before
    /// building the [`LayoutCtx`] a [`Primitive::layout`] call receives,
    /// so a `Primitive` impl's own `ctx.line_height` is already
    /// row-pitch-resolved and never has to call this itself — it exists
    /// as a `pub` function anyway so a caller building a [`LayoutCtx`] by
    /// hand (every in-crate `*PrimitiveLayout` test, and
    /// `quadraui-binding-core`'s script-widget seam) can match that
    /// resolution exactly instead of re-deriving the formula.
    ///
    /// `line_height <= 1.0` is passed through unchanged — TUI's
    /// cell-grid convention (one whole cell, not one pixel;
    /// [`crate::Backend::chrome_line_height`] reports `1.0` there) has no
    /// pixel font metric for a `style.row_height` token to floor against.
    pub fn resolve_row_height(line_height: f32, style: &Style) -> f32 {
        if line_height > 1.0 {
            line_height.max(style.row_height)
        } else {
            line_height
        }
    }

    /// Bundle this ctx's metrics plus `units` into one [`LayoutMeasure`]
    /// — the named, by-value form a `Primitive` impl that wants to store
    /// or compare "this frame's measurements" as a single value, rather
    /// than reaching for three loose [`LayoutCtx`] fields, can use
    /// instead.
    pub fn measure(&self, units: Units) -> LayoutMeasure {
        LayoutMeasure {
            line_height: self.line_height,
            char_width: self.char_width,
            units,
        }
    }
}

/// Which unit system a measurement is expressed in — sub-pixel-precise
/// float units for a pixel backend (GTK, macOS, Win), or whole terminal
/// cells for TUI.
///
/// Mirrors [`crate::Backend::cell_grid`]'s existing boolean (`true` on
/// [`crate::tui::backend::TuiBackend`], `false` on every pixel backend)
/// as a named two-variant type rather than a bare `bool`, for a
/// [`LayoutMeasure`] that wants to say which regime its numbers live in
/// without a caller having to remember which boolean value means which
/// regime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    /// Sub-pixel-precise float units — a GTK/macOS/Win backend's native
    /// coordinate space.
    Pixels,
    /// Whole terminal cells — [`crate::tui::backend::TuiBackend`]'s
    /// native coordinate space.
    Cells,
}

/// A named bundle of the chrome metrics [`LayoutCtx`] carries, plus the
/// [`Units`] they're expressed in — see [`LayoutCtx::measure`], which
/// builds one of these from a live `LayoutCtx`.
///
/// Not consumed by [`Primitive::layout`]/[`Primitive::paint`] themselves
/// today — those still take the three loose [`LayoutCtx`] fields they
/// always have, so this lands as a pure addition with no widget
/// re-port. It exists as the named value a future revision of this
/// contract can thread through instead, without every existing
/// `Primitive` implementor changing in the same change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutMeasure {
    /// See [`LayoutCtx::line_height`].
    pub line_height: f32,
    /// See [`LayoutCtx::char_width`].
    pub char_width: f32,
    /// Which unit system [`Self::line_height`]/[`Self::char_width`] are
    /// expressed in.
    pub units: Units,
}

/// Generic pairing of a [`Primitive::Layout`]'s absolute paint area with
/// its primitive-local layout — the shape every built-in primitive's own
/// hand-rolled `*PrimitiveLayout` (e.g.
/// [`crate::primitives::list::ListViewPrimitiveLayout`],
/// [`crate::primitives::tree::TreeViewPrimitiveLayout`]) already repeats:
/// an `area: Rect` plus a primitive-local `layout` value, with `hit_test`
/// subtracting `area`'s origin before delegating to the inner layout's
/// own hit-testing.
///
/// A future `Primitive` implementor can reach for this directly instead
/// of writing that same two-field struct from scratch; no existing
/// implementor is re-ported onto it by this change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrimitiveLayout<L> {
    /// The absolute rect the primitive was laid out into — what
    /// [`Primitive::layout`] received as `rect`.
    pub area: Rect,
    /// The primitive-local layout [`Primitive::layout`] resolved, in
    /// `area`-relative coordinates.
    pub layout: L,
}

impl<L> PrimitiveLayout<L> {
    /// Pair `area` with the primitive-local `layout` resolved for it.
    pub fn new(area: Rect, layout: L) -> Self {
        Self { area, layout }
    }
}

/// Which paint destination [`crate::backend::DrawsPrimitives::draw_primitive`]
/// resolved for a call, without the borrow itself — the tag half of
/// [`PaintTarget`]. Lets code branch on "pixel surface or cell grid"
/// before it has a live `&mut dyn PaintSurface`/`&mut
/// ratatui::buffer::Buffer` in hand to match on, e.g. for logging or a
/// capability check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubTarget {
    /// A pixel backend's [`PaintSurface`] — GTK, macOS, Win. Reached via
    /// [`crate::Backend::paint_surface`].
    Surface,
    /// TUI's cell grid. Reached via
    /// [`crate::Backend::cell_surface`]/[`crate::CellSurface::ratatui_buffer`].
    #[cfg(feature = "tui")]
    Cells,
}

/// Which concrete paint destination a call resolved, carrying the borrow
/// itself — see [`SubTarget`] for the borrow-less tag this exposes via
/// [`Self::kind`].
///
/// Mirrors [`crate::backend::DrawsPrimitives::draw_primitive`]'s own
/// two-way choice between [`crate::Backend::paint_surface`] and
/// [`crate::Backend::cell_surface`], named as one value instead of two
/// `Option`s a caller has to check in sequence.
pub enum PaintTarget<'a> {
    /// A pixel backend's [`PaintSurface`].
    Surface(&'a mut dyn PaintSurface),
    /// TUI's cell grid.
    #[cfg(feature = "tui")]
    Cells(&'a mut ratatui::buffer::Buffer),
}

impl PaintTarget<'_> {
    /// This target's borrow-less [`SubTarget`] tag.
    pub fn kind(&self) -> SubTarget {
        match self {
            PaintTarget::Surface(_) => SubTarget::Surface,
            #[cfg(feature = "tui")]
            PaintTarget::Cells(_) => SubTarget::Cells,
        }
    }
}

/// The non-geometry inputs every [`Primitive::paint`] call needs besides
/// its own `Layout` and paint destination: [`crate::Theme`] +
/// [`crate::Style`], bundled by reference so a future paint entry point
/// can take one value instead of two. Not required by [`Primitive::paint`]
/// today — that still takes `theme`/`style` as the two loose parameters
/// it always has.
#[derive(Debug, Clone, Copy)]
pub struct PaintServices<'a> {
    /// The active theme — see [`crate::Backend::theme`].
    pub theme: &'a Theme,
    /// The active style — see [`crate::Backend::style`].
    pub style: &'a Style,
}

/// Everything a [`Primitive::paint`]/[`Primitive::paint_cells`] call
/// needs beyond its own `Layout`: which [`PaintTarget`] to paint onto,
/// the [`PaintServices`] (theme + style), and the row pitch
/// [`LayoutCtx::resolve_row_height`] resolved for this frame.
///
/// Like [`LayoutMeasure`] and [`PrimitiveLayout`], this is a pure
/// addition: [`crate::backend::DrawsPrimitives::draw_primitive`]'s
/// default body does not build one today, since doing so for real would
/// mean changing [`Primitive::paint`]'s signature to accept it — a
/// widget re-port this change deliberately does not make. It is defined
/// now as the named shape that re-port has to converge on, instead of
/// each primitive inventing its own paint-environment bundle piecemeal.
pub struct PaintEnv<'a> {
    /// Where to paint.
    pub target: PaintTarget<'a>,
    /// What to paint with.
    pub services: PaintServices<'a>,
    /// This frame's resolved row pitch — see
    /// [`LayoutCtx::resolve_row_height`].
    pub row_height: f32,
}

/// Per-frame paint-time state a [`Primitive`] impl may want layered on
/// top of its own `Layout` — hover/press index, a continuous pixel
/// scroll offset, zebra striping — the same shape every built-in
/// widget's own `*PaintState`
/// ([`crate::primitives::row_stack::RowPaintState`] and its
/// `ListPaintState`/`TreePaintState`/`DataTablePaintState` aliases)
/// already repeats structurally.
///
/// Not required by [`Primitive::paint`] today (adopting it crate-wide is
/// a future widget re-port, not this change) — defined now so that
/// re-port has a named trait to implement against instead of inventing
/// one per widget again. Every method defaults to "no such state", so
/// implementing this trait for a type that doesn't track one of these
/// concepts costs nothing.
pub trait PaintState {
    /// Index of the row currently hovered, if any.
    fn hovered_idx(&self) -> Option<usize> {
        None
    }
    /// Index of the row currently pressed (mouse-down, not yet
    /// released), if any.
    fn pressed_idx(&self) -> Option<usize> {
        None
    }
    /// Continuous pixel scroll offset, if the primitive tracks one finer
    /// than a whole-row `Layout` scroll index.
    fn scroll_px(&self) -> Option<f32> {
        None
    }
    /// Whether every other row should be tinted ("zebra striping").
    fn striped(&self) -> bool {
        false
    }
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

    /// Paint `layout` onto a TUI cell buffer —
    /// [`crate::backend::DrawsPrimitives::draw_primitive`]'s default body
    /// calls this instead of [`Self::paint`] whenever
    /// [`crate::backend::Backend::cell_surface`] answers `Some`
    /// (`TuiBackend`'s case), since TUI has no [`PaintSurface`] to hand
    /// it (see that trait's module doc, "Why TUI stays out").
    ///
    /// # Why a second method instead of a cell-quantised `PaintSurface`
    ///
    /// A real prototype (`crate::primitives::list::ListView`'s pilot
    /// impl) ruled out a `TuiBackend: PaintSurface` impl that reproduces
    /// `crate::tui::draw_list`'s output byte-for-byte: `ListView::bordered`'s
    /// frame is painted as literal box-drawing glyphs (`╭─╮│╰─╯`) chosen
    /// per-corner — a choice [`PaintSurface`]'s verbs have no way to
    /// express, since none of them picks a glyph, only a rect or a path
    /// shape. A `PaintSurface` verb general enough to pick "this exact
    /// glyph at this exact corner" stops being a backend-agnostic
    /// drawing primitive and becomes a terminal-box-drawing API wearing
    /// a `PaintSurface` costume. Row text also carries its own cell-count
    /// accounting
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
    /// Default: no-op — correct for a `Primitive` implementor with no
    /// TUI story at all (every built-in primitive that has a TUI
    /// rasteriser overrides it); such an implementor painted through
    /// [`crate::backend::DrawsPrimitives::draw_primitive`] on TUI still
    /// gets a real, hit-testable [`Self::Layout`] back, just no pixels
    /// on screen, the same "resolve layout, maybe paint" contract that
    /// default already has for a `PaintSurface`-less backend.
    #[cfg(feature = "tui")]
    fn paint_cells(&self, layout: &Self::Layout, buf: &mut ratatui::buffer::Buffer, theme: &Theme) {
        let _ = (layout, buf, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `line_height` above `1.0` (a pixel backend's real font metric) is
    /// floored at `style.row_height` — a chrome font taller than the
    /// theme's row-pitch token must widen the resolved pitch, not get
    /// clipped to the font metric.
    #[test]
    fn resolve_row_height_floors_a_short_pixel_metric_at_the_style_token() {
        let style = Style::default().with_row_height(22.0);
        assert_eq!(LayoutCtx::resolve_row_height(10.0, &style), 22.0);
    }

    /// A pixel font metric already taller than the style token wins —
    /// the floor never shrinks `line_height`.
    #[test]
    fn resolve_row_height_keeps_a_tall_pixel_metric_unchanged() {
        let style = Style::default().with_row_height(14.0);
        assert_eq!(LayoutCtx::resolve_row_height(30.0, &style), 30.0);
    }

    /// TUI's cell-grid convention (`line_height == 1.0`, one whole cell)
    /// has no pixel font metric to floor — it must pass through
    /// unchanged regardless of the style's pixel-oriented `row_height`.
    #[test]
    fn resolve_row_height_passes_through_the_tui_cell_convention() {
        let style = Style::default().with_row_height(22.0);
        assert_eq!(LayoutCtx::resolve_row_height(1.0, &style), 1.0);
    }

    /// [`LayoutCtx::measure`] bundles the ctx's own metrics plus the
    /// caller-supplied [`Units`] into a [`LayoutMeasure`] — no implicit
    /// recomputation of either field.
    #[test]
    fn layout_ctx_measure_bundles_its_own_fields_verbatim() {
        let ctx = LayoutCtx {
            line_height: 22.0,
            char_width: 8.0,
            nerd_fonts_enabled: true,
        };
        let measure = ctx.measure(Units::Pixels);
        assert_eq!(
            measure,
            LayoutMeasure {
                line_height: 22.0,
                char_width: 8.0,
                units: Units::Pixels,
            }
        );
    }

    #[test]
    fn units_pixels_and_cells_are_distinct() {
        assert_ne!(Units::Pixels, Units::Cells);
    }

    /// [`PrimitiveLayout::new`] pairs `area` with the primitive-local
    /// layout verbatim — no coordinate transform of either field, the
    /// same contract every hand-rolled `*PrimitiveLayout` already
    /// follows.
    #[test]
    fn primitive_layout_new_pairs_area_and_layout_verbatim() {
        let area = Rect::new(1.0, 2.0, 3.0, 4.0);
        let layout = PrimitiveLayout::new(area, 42_u32);
        assert_eq!(layout.area, area);
        assert_eq!(layout.layout, 42);
    }

    /// A no-op [`PaintSurface`] mock, just to construct a real
    /// [`PaintTarget::Surface`] borrow for [`paint_target_kind_reports_surface`].
    #[derive(Default)]
    struct NullSurface;

    impl PaintSurface for NullSurface {
        fn surface_begin_frame(&mut self, _v: crate::Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> crate::Viewport {
            crate::Viewport::new(0.0, 0.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            16.0
        }
        fn surface_char_width(&self) -> f32 {
            8.0
        }
        fn surface_measure_text(&self, _t: &str) -> (f32, f32) {
            (0.0, 0.0)
        }
        fn surface_fill_rect(&mut self, _r: Rect, _c: crate::Color) {}
        fn surface_fill_rounded_rect(&mut self, _r: Rect, _radius: f32, _c: crate::Color) {}
        fn surface_stroke_rect(&mut self, _r: Rect, _c: crate::Color, _w: f32) {}
        fn surface_draw_text_run(&mut self, _r: Rect, _t: &str, _c: crate::Color) {}
        fn surface_draw_line(
            &mut self,
            _from: crate::Point,
            _to: crate::Point,
            _c: crate::Color,
            _w: f32,
        ) {
        }
        fn surface_push_clip(&mut self, _r: Rect) {}
        fn surface_pop_clip(&mut self) {}
        fn surface_draw_image(
            &mut self,
            _r: Rect,
            _i: &crate::Image,
        ) -> crate::backend::ImagePaintResult {
            crate::backend::ImagePaintResult::Unsupported
        }
    }

    #[test]
    fn paint_target_kind_reports_surface() {
        let mut surface = NullSurface;
        let target = PaintTarget::Surface(&mut surface);
        assert_eq!(target.kind(), SubTarget::Surface);
    }

    #[cfg(feature = "tui")]
    #[test]
    fn paint_target_kind_reports_cells() {
        let area = ratatui::layout::Rect::new(0, 0, 4, 4);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        let target = PaintTarget::Cells(&mut buf);
        assert_eq!(target.kind(), SubTarget::Cells);
    }

    #[test]
    fn paint_services_exposes_the_theme_and_style_it_was_built_with() {
        let theme = Theme::default();
        let style = Style::default();
        let services = PaintServices {
            theme: &theme,
            style: &style,
        };
        assert_eq!(*services.theme, theme);
        assert_eq!(*services.style, style);
    }

    #[test]
    fn paint_env_bundles_target_services_and_row_height() {
        let mut surface = NullSurface;
        let theme = Theme::default();
        let style = Style::default();
        let env = PaintEnv {
            target: PaintTarget::Surface(&mut surface),
            services: PaintServices {
                theme: &theme,
                style: &style,
            },
            row_height: 22.0,
        };
        assert_eq!(env.target.kind(), SubTarget::Surface);
        assert_eq!(env.row_height, 22.0);
    }

    /// A type implementing [`PaintState`] with no overrides gets every
    /// default — "no such state" for all four concepts.
    struct NoPaintState;
    impl PaintState for NoPaintState {}

    #[test]
    fn paint_state_default_methods_report_no_state() {
        let state = NoPaintState;
        assert_eq!(state.hovered_idx(), None);
        assert_eq!(state.pressed_idx(), None);
        assert_eq!(state.scroll_px(), None);
        assert!(!state.striped());
    }

    /// A real override only changes the one method it defines — proves
    /// the trait's defaults are independent per-method, not a single
    /// all-or-nothing impl.
    struct HoveredOnly;
    impl PaintState for HoveredOnly {
        fn hovered_idx(&self) -> Option<usize> {
            Some(3)
        }
    }

    #[test]
    fn paint_state_override_leaves_other_defaults_untouched() {
        let state = HoveredOnly;
        assert_eq!(state.hovered_idx(), Some(3));
        assert_eq!(state.scroll_px(), None);
        assert!(!state.striped());
    }
}
