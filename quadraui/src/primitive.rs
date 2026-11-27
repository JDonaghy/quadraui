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
//! `list_layout`, and so on — because
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
//! use quadraui::primitive::{LayoutCtx, PaintEnv, Primitive};
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
//!     fn paint(&self, layout: &Rect, surface: &mut dyn PaintSurface, _env: &PaintEnv) {
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
//! let ctx = LayoutCtx::new(16.0, 8.0, false);
//! let mut surface = NullSurface::default();
//! let layout = swatch.layout(&ctx, Rect::new(0.0, 0.0, 10.0, 10.0));
//! let (theme, style) = (Theme::default(), Style::default());
//! swatch.paint(&layout, &mut surface, &PaintEnv::new(&theme, &style));
//! ```

use crate::a11y::{A11yAction, A11yPrimitive};
#[cfg(feature = "tui")]
use crate::cell_surface::CellSurface;
use crate::event::Rect;
use crate::frame::OwnedContent;
use crate::interaction::InteractionState;
use crate::interactive::{EventCx, FocusSpec, Interactive, RoutedEvent, WidgetAction};
use crate::paint_surface::PaintSurface;
use crate::style::Style;
use crate::theme::Theme;
use crate::types::WidgetId;
use crate::Handled;

/// The text measurer [`LayoutCtx`] carries — a borrowed closure over
/// the calling backend's [`crate::Backend::measure_text`], spelled once
/// here rather than inline on the field and on
/// [`LayoutCtx::with_measurer`]'s parameter.
type TextMeasurer<'a> = &'a dyn Fn(&str, crate::FontRole) -> f32;

/// Backend-measured geometry a [`Primitive::layout`] call needs that a
/// bare [`Rect`] doesn't carry.
///
/// Mirrors the handful of [`crate::Backend`] measurement methods every
/// pixel backend already exposes (`chrome_line_height`, `char_width`,
/// `nerd_fonts_enabled`, `measure_text`) —
/// [`crate::DrawsPrimitives::draw_primitive`]'s default body builds one of
/// these from `self` before calling [`Primitive::layout`], so a
/// `Primitive` impl never has to reach for `&dyn Backend` itself.
///
/// Built with [`Self::new`], never a struct literal: the text measurer
/// behind [`Self::text_width`] is a private field, so a caller with a
/// live backend attaches it with [`Self::with_measurer`] and a caller
/// without one (a script-widget seam, a unit test) gets the
/// `char_width` approximation by construction.
#[derive(Clone, Copy)]
pub struct LayoutCtx<'a> {
    /// One standard row's height, in the backend's native units — see
    /// [`crate::Backend::chrome_line_height`]. Already run through
    /// [`Self::resolve_row_height`] against the calling backend's
    /// [`crate::Backend::style`] by
    /// [`crate::DrawsPrimitives::draw_primitive`], so a `Primitive` impl
    /// reads this field directly rather than re-applying that floor
    /// itself.
    pub line_height: f32,
    /// Approximate character advance, in the backend's native units —
    /// see [`crate::Backend::char_width`]. One nominal advance for every
    /// glyph; [`Self::text_width`] is the proportional-font-correct
    /// measurement a layout that positions text should ask for instead.
    pub char_width: f32,
    /// Whether the backend has a Nerd Font icon face registered — see
    /// [`crate::Backend::nerd_fonts_enabled`].
    pub nerd_fonts_enabled: bool,
    /// The calling backend's [`crate::Backend::measure_text`], bound to
    /// that backend — see [`Self::with_measurer`] and
    /// [`Self::text_width`]. `None` when no backend is in reach.
    measurer: Option<TextMeasurer<'a>>,
    /// The calling backend's [`crate::Backend::style`] — see
    /// [`Self::with_style`] and [`Self::style`]. `None` when no backend is
    /// in reach.
    style: Option<&'a crate::Style>,
    /// The calling backend's editor-font row height — see
    /// [`Self::with_editor_line_height`] and [`Self::editor_line_height`].
    editor_line_height: Option<f32>,
    /// The calling backend's chrome-font text line height — see
    /// [`Self::with_chrome_text_line_height`] and
    /// [`Self::text_line_height`].
    chrome_text_line_height: Option<f32>,
    /// The backend's chrome line height before the row-height floor — see
    /// [`Self::with_chrome_line_height`].
    chrome_line_height: Option<f32>,
    /// Whether the calling backend paints per-row text scale — see
    /// [`Self::with_scaled_text_rows`] and [`Self::scales_text_rows`].
    scales_text_rows: bool,
    /// The paint state the widget is painted with — see
    /// [`Self::with_paint_state`] and [`Self::paint_state`].
    paint_state: Option<&'a dyn PaintState>,
}

/// Prints the three scalar metrics plus whether a text measurer is
/// attached — a `&dyn Fn` has no useful `Debug`, and the closure's
/// identity is not part of this value's meaning.
impl std::fmt::Debug for LayoutCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutCtx")
            .field("line_height", &self.line_height)
            .field("char_width", &self.char_width)
            .field("nerd_fonts_enabled", &self.nerd_fonts_enabled)
            .field("measurer", &self.measurer.map(|_| "<fn>"))
            .field("style", &self.style.is_some())
            .field("scroll_px", &self.paint_state().scroll_px())
            .finish()
    }
}

/// Compares the three scalar metrics and the attached paint state's
/// [`PaintState::scroll_px`], the one paint-state answer a layout
/// resolves geometry from. The measurer is excluded: two function
/// pointers to the same backend's [`crate::Backend::measure_text`] are
/// not reliably equal, and a `Primitive` caching "did this frame's
/// metrics change?" asks about the metrics, not about the closure that
/// carried them.
impl PartialEq for LayoutCtx<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.line_height == other.line_height
            && self.char_width == other.char_width
            && self.nerd_fonts_enabled == other.nerd_fonts_enabled
            && self.paint_state().scroll_px() == other.paint_state().scroll_px()
    }
}

impl LayoutCtx<'static> {
    /// A ctx with no text measurer attached: [`Self::text_width`] falls
    /// back to `char_width * chars`.
    ///
    /// [`crate::DrawsPrimitives::draw_primitive`] builds its ctx this way
    /// and then attaches the calling backend's own measurer with
    /// [`Self::with_measurer`]; a caller with no backend in reach
    /// (`quadraui-binding-core`'s script-widget seam, a `Primitive`
    /// layout unit test) uses this directly and lays out against the
    /// approximation.
    pub fn new(line_height: f32, char_width: f32, nerd_fonts_enabled: bool) -> Self {
        Self {
            line_height,
            char_width,
            nerd_fonts_enabled,
            measurer: None,
            style: None,
            editor_line_height: None,
            chrome_text_line_height: None,
            chrome_line_height: None,
            scales_text_rows: false,
            paint_state: None,
        }
    }
}

impl<'a> LayoutCtx<'a> {
    /// This ctx with `m` attached as its text measurer, so
    /// [`Self::text_width`] reports real glyph widths.
    ///
    /// `m` is a closure rather than a `&dyn Backend`:
    /// [`crate::DrawsPrimitives`] is blanket-implemented over
    /// `B: Backend + ?Sized`, so a generic `&B` cannot be coerced to
    /// another trait object there — `|t, r| backend.measure_text(t, r).0`
    /// can, and keeps this type free of any `Backend` bound.
    pub fn with_measurer<'b>(self, m: &'b dyn Fn(&str, crate::FontRole) -> f32) -> LayoutCtx<'b>
    where
        'a: 'b,
    {
        LayoutCtx {
            measurer: Some(m),
            ..self
        }
    }

    /// This ctx with `style` attached, so a layout that sizes itself off a
    /// [`crate::Style`] token (a tab's [`crate::Style::tab_padding_x`])
    /// reads the calling backend's live value — the one its own
    /// rasteriser paints with.
    pub fn with_style<'b>(self, style: &'b crate::Style) -> LayoutCtx<'b>
    where
        'a: 'b,
    {
        LayoutCtx {
            style: Some(style),
            ..self
        }
    }

    /// This ctx with `line_height` as the editor font's row height — the
    /// row pitch an editor-class primitive (`crate::font_role::EditorClassPrimitive`)
    /// lays its rows out on, which [`Self::line_height`] (the chrome row)
    /// does not describe once the two fonts differ.
    pub fn with_editor_line_height(self, line_height: f32) -> Self {
        Self {
            editor_line_height: Some(line_height),
            ..self
        }
    }

    /// The editor font's row height attached with
    /// [`Self::with_editor_line_height`] — [`crate::Backend::line_height`]
    /// on a ctx [`crate::DrawsPrimitives`] built — else
    /// [`Self::line_height`].
    pub fn editor_line_height(&self) -> f32 {
        self.editor_line_height.unwrap_or(self.line_height)
    }

    /// Attach the backend's own chrome line height
    /// ([`crate::Backend::chrome_line_height`]) before
    /// [`Self::resolve_row_height`]'s row-height floor is applied to
    /// [`Self::line_height`] — the unit a widget sized in line-height
    /// multiples (an app shell's sidebar width, its bottom-panel height)
    /// scales by, so its geometry matches a caller that reads the metric
    /// straight off the backend.
    pub fn with_chrome_line_height(self, line_height: f32) -> Self {
        Self {
            chrome_line_height: Some(line_height),
            ..self
        }
    }

    /// The backend's chrome line height as attached by
    /// [`Self::with_chrome_line_height`], without the row-height floor;
    /// [`Self::line_height`] when none was attached.
    pub fn chrome_line_height(&self) -> f32 {
        self.chrome_line_height.unwrap_or(self.line_height)
    }

    /// This ctx with `line_height` as the chrome (UI) font's own text
    /// line height — the pitch of one line of text painted in that font,
    /// without the [`crate::Style::row_height`] floor [`Self::line_height`]
    /// carries for chrome rows. [`crate::Backend::chrome_line_height`]
    /// reports the editor font's height on a backend with no separate
    /// chrome metric, so a ctx [`crate::DrawsPrimitives`] builds measures
    /// this through [`crate::Backend::measure_text`] in
    /// [`crate::FontRole::Chrome`] instead.
    pub fn with_chrome_text_line_height(self, line_height: f32) -> Self {
        Self {
            chrome_text_line_height: Some(line_height),
            ..self
        }
    }

    /// The height of one line of text painted in `role`'s font:
    /// [`Self::editor_line_height`] for [`crate::FontRole::Editor`], and
    /// the height attached with [`Self::with_chrome_text_line_height`]
    /// (else [`Self::line_height`]) for [`crate::FontRole::Chrome`],
    /// [`crate::FontRole::ChromeBold`] and [`crate::FontRole::Icon`], which
    /// paint in the chrome font.
    pub fn text_line_height(&self, role: crate::FontRole) -> f32 {
        match role {
            crate::FontRole::Editor => self.editor_line_height(),
            crate::FontRole::Chrome | crate::FontRole::ChromeBold | crate::FontRole::Icon => {
                self.chrome_text_line_height.unwrap_or(self.line_height)
            }
        }
    }

    /// This ctx with `scales` as [`crate::Backend::scales_text_rows`], so
    /// a layout that reserves taller rows for scaled text (a markdown
    /// heading) agrees with a host that sized the same box from that
    /// backend capability.
    pub fn with_scaled_text_rows(self, scales: bool) -> Self {
        Self {
            scales_text_rows: scales,
            ..self
        }
    }

    /// The capability attached with [`Self::with_scaled_text_rows`] —
    /// [`crate::Backend::scales_text_rows`] on a ctx
    /// [`crate::DrawsPrimitives`] built — else `false`, every row one
    /// [`Self::line_height`] tall.
    pub fn scales_text_rows(&self) -> bool {
        self.scales_text_rows
    }

    /// This ctx with `state` as the paint state the widget is painted
    /// with — the same value [`PaintEnv::state`] carries to its paint —
    /// so a layout that depends on paint-time state (a row widget's
    /// [`PaintState::scroll_px`]) resolves the geometry that is painted.
    /// [`crate::DrawsPrimitives::draw_dyn`] attaches the caller's state,
    /// and event routing attaches what that paint recorded
    /// ([`PaintedWidgets::painted_state`]), so a hit-test resolves rows
    /// exactly where they were drawn.
    pub fn with_paint_state<'b>(self, state: &'b dyn PaintState) -> LayoutCtx<'b>
    where
        'a: 'b,
    {
        LayoutCtx {
            paint_state: Some(state),
            ..self
        }
    }

    /// The paint state attached with [`Self::with_paint_state`], else
    /// `()` — no paint-time state, every answer its default.
    pub fn paint_state(&self) -> &dyn PaintState {
        self.paint_state.unwrap_or(&())
    }

    /// The style attached with [`Self::with_style`], else
    /// [`crate::Style::default`].
    pub fn style(&self) -> &crate::Style {
        static DEFAULT: std::sync::LazyLock<crate::Style> =
            std::sync::LazyLock::new(crate::Style::default);
        self.style.unwrap_or(&DEFAULT)
    }

    /// Width of `text` in `role`'s font, in the same units as
    /// [`Self::char_width`] — the calling backend's real
    /// [`crate::Backend::measure_text`] when a measurer is attached (see
    /// [`Self::with_measurer`]), else the `char_width * chars`
    /// approximation.
    ///
    /// A `Primitive::layout` that positions text must measure through
    /// this rather than scaling [`Self::char_width`] itself: on a
    /// proportional chrome font the error runs in both directions (GTK's
    /// Pango measures `alpha` and `beta` *narrower* than
    /// `chars * char_width` and `gamma` *wider*), so no scalar
    /// correction closes it and a shared layout built on the
    /// approximation ellipsises labels the rasteriser fits.
    pub fn text_width(&self, text: &str, role: crate::FontRole) -> f32 {
        match self.measurer {
            Some(m) => m(text, role),
            None => self.char_width * text.chars().count() as f32,
        }
    }

    /// Floor `line_height` at `style.row_height`, the same row-pitch
    /// resolution every pixel backend's own private `resolved_row_height`
    /// helper already applies on its bespoke `draw_list` path (e.g. `GtkBackend::resolved_row_height`,
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

    /// Which [`Units`] this ctx's metrics are expressed in, derived from
    /// [`Self::line_height`] — `<= 1.0` is TUI's whole-cell convention
    /// ([`Units::Cells`]), anything taller is a real pixel font metric
    /// ([`Units::Pixels`]). The same runtime discriminator every
    /// `Primitive::layout` impl's TUI-vs-pixel-backend branch already
    /// checks by hand (`ctx.line_height <= 1.0`); this names it instead
    /// of repeating the bare float comparison at each call site.
    pub fn units(&self) -> Units {
        if self.line_height <= 1.0 {
            Units::Cells
        } else {
            Units::Pixels
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
#[non_exhaustive]
pub enum Units {
    /// Sub-pixel-precise float units — a GTK/macOS/Win backend's native
    /// coordinate space.
    Pixels,
    /// Whole terminal cells — [`crate::tui::backend::TuiBackend`]'s
    /// native coordinate space.
    Cells,
}

/// Quantise `rect` to the whole cells a cell-grid backend actually paints
/// it into — the same rounding [`crate::tui::backend::TuiBackend`] applies
/// when it converts a [`Rect`] to its native grid before painting a cell.
///
/// A container can hand a leaf a fractional rect (a ratio-split
/// [`crate::SplitTree`] pane), but only whole cells of it are painted. A `Primitive::layout` that resolves its
/// cell-grid geometry against the raw rect hit-tests one cell wider than
/// what painted, claiming a neighbour's cell (the split divider beside the
/// pane) as its own body. Resolving against this rect instead keeps
/// hit-testing on the painted grid.
pub(crate) fn cell_grid_rect(rect: Rect) -> Rect {
    Rect::new(
        rect.x.max(0.0).round(),
        rect.y.max(0.0).round(),
        rect.width.max(0.0).round(),
        rect.height.max(0.0).round(),
    )
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
#[non_exhaustive]
pub struct LayoutMeasure {
    /// See [`LayoutCtx::line_height`].
    pub line_height: f32,
    /// See [`LayoutCtx::char_width`].
    pub char_width: f32,
    /// Which unit system [`Self::line_height`]/[`Self::char_width`] are
    /// expressed in.
    pub units: Units,
}

/// Available space a [`DynWidget::measure`] call sizes against, in
/// [`LayoutCtx`]'s own units (pixels or whole cells — see
/// [`LayoutCtx::units`]) — a plain width/height pair with no origin,
/// since measuring never needs a position the way [`Primitive::layout`]'s
/// `rect` does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    /// Available width.
    pub width: f32,
    /// Available height.
    pub height: f32,
}

impl Size {
    /// Pair `width` and `height` into a [`Size`].
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
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
/// implementor is re-ported onto it by this change. Named `AreaLayout`
/// rather than `PrimitiveLayout` so that name is free for the
/// object-safe [`PrimitiveLayout`] trait [`DynWidget::layout`] returns.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct AreaLayout<L> {
    /// The absolute rect the primitive was laid out into — what
    /// [`Primitive::layout`] received as `rect`.
    pub area: Rect,
    /// The primitive-local layout [`Primitive::layout`] resolved, in
    /// `area`-relative coordinates.
    pub layout: L,
}

impl<L> AreaLayout<L> {
    /// Pair `area` with the primitive-local `layout` resolved for it.
    pub fn new(area: Rect, layout: L) -> Self {
        Self { area, layout }
    }
}

/// Object-safe view of any `'static` [`Primitive::Layout`]/
/// [`Interactive::Layout`][crate::interactive::Interactive::Layout] value
/// — what [`DynWidget::layout`] returns in place of an associated type, so
/// a type-erased widget can still hand its layout to [`DynWidget::paint`]/
/// [`DynWidget::handle`] for the blanket [`DynWidget`] impl to downcast
/// back to the concrete layout type via [`Self::as_any`].
///
/// Blanket-implemented for every `'static` type, mirroring how
/// `std::any::Any` itself is usable through `&dyn Any` for any `'static`
/// value — a concrete `*Layout` type never implements this by hand.
pub trait PrimitiveLayout: std::any::Any {
    /// This value's `&dyn Any` view, for downcasting back to the
    /// concrete layout type.
    fn as_any(&self) -> &dyn std::any::Any;
}

impl<L: std::any::Any> PrimitiveLayout for L {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl dyn PrimitiveLayout {
    /// Downcast back to a concrete layout type `T` — `self.as_any()`
    /// plus [`std::any::Any::downcast_ref`] in one call.
    ///
    /// Prefer this over calling [`Self::as_any`] directly on a
    /// `Box<dyn PrimitiveLayout>`: because [`PrimitiveLayout`] is
    /// blanket-implemented for every `'static` type (this module's own
    /// `impl<L: Any> PrimitiveLayout for L` above), `Box<dyn
    /// PrimitiveLayout>` — an owned, `'static` value in its own right —
    /// *also* matches that blanket impl. Rust's method resolution finds
    /// that impl on the `Box` itself before it ever autoderefs to the
    /// trait object underneath, so `boxed_layout.as_any()` erases the
    /// `Box` rather than the layout inside it, and the downcast that
    /// follows silently fails. This method is inherent to `dyn
    /// PrimitiveLayout` itself (not part of the [`PrimitiveLayout`]
    /// trait), so it has no such competing impl at the `Box` level —
    /// `boxed_layout.downcast_ref::<T>()` resolves through exactly one
    /// autoderef step, to the trait object's own vtable.
    pub fn downcast_ref<T: std::any::Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
}

/// Which paint destination [`crate::backend::DrawsPrimitives::draw_primitive`]
/// resolved for a call, without the borrow itself — the tag half of
/// [`PaintTarget`]. Lets code branch on "pixel surface or cell grid"
/// before it has a live `&mut dyn PaintSurface`/`&mut dyn CellSurface`
/// in hand to match on, e.g. for logging or a
/// capability check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SubTarget {
    /// A pixel backend's [`PaintSurface`] — GTK, macOS, Win. Reached via
    /// [`crate::Backend::paint_surface`].
    Surface,
    /// TUI's cell grid. Reached via [`crate::Backend::cell_surface`].
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
#[non_exhaustive]
pub enum PaintTarget<'a> {
    /// A pixel backend's [`PaintSurface`].
    Surface(&'a mut dyn PaintSurface),
    /// TUI's cell grid.
    #[cfg(feature = "tui")]
    Cells(&'a mut dyn CellSurface),
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
/// [`crate::Style`], bundled by reference inside [`PaintEnv`].
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct PaintServices<'a> {
    /// The active theme — see [`crate::Backend::theme`].
    pub theme: &'a Theme,
    /// The active style — see [`crate::Backend::style`].
    pub style: &'a Style,
}

/// Everything a [`Primitive::paint`] / [`Primitive::paint_cells`] /
/// [`DynWidget::paint`] call needs beyond its own `Layout` and paint
/// destination: the [`PaintServices`] (theme + style), the row pitch
/// [`LayoutCtx::resolve_row_height`] resolved for this frame, the
/// caller's per-call [`PaintState`], and the hover/press
/// [`InteractionState`] the widget paints with.
///
/// [`crate::backend::DrawsPrimitives::draw_dyn`] and
/// [`crate::backend::DrawsPrimitives::draw_primitive`] build one of these
/// per call. `interaction` is already resolved: the caller's own state
/// when its [`PaintState::interaction`] answers `Some`, otherwise the
/// ambient pointer state ([`crate::interaction::ambient_interaction`]).
/// A caller painting without a backend (a unit test, a headless
/// renderer) builds one with [`Self::new`] and the `with_*` builders.
///
/// `#[non_exhaustive]`, so a field added later reaches every widget
/// without another signature change; construct it through [`Self::new`].
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct PaintEnv<'a> {
    /// What to paint with.
    pub services: PaintServices<'a>,
    /// This frame's resolved row pitch — see
    /// [`LayoutCtx::resolve_row_height`].
    pub row_height: f32,
    /// The caller's per-call paint state.
    pub state: &'a dyn PaintState,
    /// The resolved hover/press state this paint renders.
    pub interaction: &'a InteractionState,
    /// The painted widget's own persistent
    /// [`Interactive::State`], type-erased; read it with
    /// [`Self::widget_state_as`]. [`crate::DrawsPrimitives::draw_dyn`]
    /// fills it from the runtime's per-thread store
    /// ([`crate::widget_state::with_widget_states`]) for an identified
    /// widget that has received an event; `None` otherwise, which a
    /// widget paints as `State::default()`.
    pub widget_state: Option<&'a dyn std::any::Any>,
}

/// The panic message for a cell painter that delegates to a `ratatui`
/// rasteriser when its [`CellSurface`] has no `ratatui` buffer behind it.
#[cfg(feature = "tui")]
#[doc(hidden)]
pub const CELL_BUFFER_REQUIRED: &str =
    "this cell painter needs a ratatui-backed CellSurface (TuiBackend inside \
     enter_frame_scope, or quadraui::tui::BufferCellSurface)";

/// The empty interaction [`PaintEnv::new`] starts from.
static NO_INTERACTION: InteractionState = InteractionState::new();

impl std::fmt::Debug for PaintEnv<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaintEnv")
            .field("services", &self.services)
            .field("row_height", &self.row_height)
            .field("interaction", self.interaction)
            .field("widget_state", &self.widget_state.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> PaintEnv<'a> {
    /// An env over `theme` and `style`, with `row_height` taken from
    /// [`Style::row_height`], no per-call state, and nothing hovered or
    /// pressed.
    pub fn new(theme: &'a Theme, style: &'a Style) -> Self {
        Self {
            services: PaintServices { theme, style },
            row_height: style.row_height,
            state: &(),
            interaction: &NO_INTERACTION,
            widget_state: None,
        }
    }

    /// This env with the widget's persistent state replaced.
    pub fn with_widget_state(mut self, widget_state: Option<&'a dyn std::any::Any>) -> Self {
        self.widget_state = widget_state;
        self
    }

    /// The painted widget's persistent state as its concrete
    /// [`Interactive::State`] type `T`, or `None` when there is none or
    /// it is another type.
    ///
    /// ```
    /// use quadraui::{PaintEnv, Style, Theme};
    ///
    /// #[derive(Default)]
    /// struct DragState {
    ///     grabbed: Option<usize>,
    /// }
    ///
    /// let (theme, style) = (Theme::default(), Style::default());
    /// let state = DragState { grabbed: Some(2) };
    /// let env = PaintEnv::new(&theme, &style).with_widget_state(Some(&state));
    /// assert_eq!(env.widget_state_as::<DragState>().unwrap().grabbed, Some(2));
    /// assert!(env.widget_state_as::<u32>().is_none());
    /// ```
    pub fn widget_state_as<T: std::any::Any>(&self) -> Option<&'a T> {
        self.widget_state?.downcast_ref::<T>()
    }

    /// This env with `row_height` replaced.
    pub fn with_row_height(mut self, row_height: f32) -> Self {
        self.row_height = row_height;
        self
    }

    /// This env with the per-call paint state replaced.
    pub fn with_state(mut self, state: &'a dyn PaintState) -> Self {
        self.state = state;
        self
    }

    /// This env with the hover/press state replaced.
    pub fn with_interaction(mut self, interaction: &'a InteractionState) -> Self {
        self.interaction = interaction;
        self
    }

    /// The theme to paint with.
    pub fn theme(&self) -> &'a Theme {
        self.services.theme
    }

    /// The style to paint with.
    pub fn style(&self) -> &'a Style {
        self.services.style
    }

    /// Whether `id` is hovered: `None` when it is not, `Some(None)` when
    /// the pointer is on the widget but on no sub-part, `Some(Some(sub))`
    /// when it is on sub-part `sub`.
    pub fn hovered(&self, id: &WidgetId) -> Option<Option<crate::interactive::SubTarget>> {
        match self.interaction.hovered_target() {
            Some((hovered, sub)) if hovered == *id => Some(sub),
            _ => None,
        }
    }

    /// Whether `id` paints pressed, in the same shape as
    /// [`Self::hovered`]. A press counts only while the pointer is still
    /// on the address it pressed: dragging off a pressed button shows it
    /// at rest, and dragging back shows it pressed again.
    pub fn pressed(&self, id: &WidgetId) -> Option<Option<crate::interactive::SubTarget>> {
        match self.interaction.pressed_target() {
            Some((pressed, sub))
                if pressed == *id && self.interaction.hovered_target() == Some((pressed, sub)) =>
            {
                Some(sub)
            }
            _ => None,
        }
    }

    /// The hovered sub-part of `id`, if the pointer is on one.
    pub fn hovered_sub(&self, id: &WidgetId) -> Option<crate::interactive::SubTarget> {
        self.hovered(id).flatten()
    }

    /// The pressed sub-part of `id`, under [`Self::pressed`]'s rule.
    pub fn pressed_sub(&self, id: &WidgetId) -> Option<crate::interactive::SubTarget> {
        self.pressed(id).flatten()
    }

    /// Whether the pointer is anywhere on `id`.
    pub fn is_hovered(&self, id: &WidgetId) -> bool {
        self.hovered(id).is_some()
    }

    /// Whether `id` paints pressed, under [`Self::pressed`]'s rule.
    pub fn is_pressed(&self, id: &WidgetId) -> bool {
        self.pressed(id).is_some()
    }

    /// The interaction state re-addressed from `parent` to `child`: each
    /// slot that names `parent` names `child` instead, on the same
    /// sub-part, and every other slot is dropped. For a composite widget
    /// that paints a child under its own id — the child reads its hover
    /// through the parent's address.
    pub fn child_interaction(&self, parent: &WidgetId, child: &WidgetId) -> InteractionState {
        let readdress = |target: Option<crate::focus::FocusAddress>| match target {
            Some((id, sub)) if id == *parent => Some((*child, sub)),
            _ => None,
        };
        InteractionState::from_targets(
            readdress(self.interaction.hovered_target()),
            readdress(self.interaction.pressed_target()),
        )
    }
}

/// Per-frame paint-time state a [`Primitive`] impl may want layered on
/// top of its own `Layout` — hover/press index, a continuous pixel
/// scroll offset, zebra striping — the same shape every built-in
/// widget's own `*PaintState`
/// ([`crate::primitives::row_stack::RowPaintState`] and its
/// `ListPaintState`/`TreePaintState`/`DataTablePaintState` aliases)
/// already repeats structurally.
///
/// A caller hands one to [`crate::backend::DrawsPrimitives::draw_dyn`],
/// and the widget reads it back as [`PaintEnv::state`]. Every method
/// defaults to "no such state", so implementing this trait for a type
/// that doesn't track one of these concepts costs nothing.
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
    /// Whether a focus ring should be stroked around the focused row.
    ///
    /// Unlike the other methods on this trait, no built-in `*PaintState`
    /// stores this as a field — it is a single widget-level flag a
    /// focus-ring controller supplies, not per-row/scroll/stripe state —
    /// so every implementor gets `false` ("no ring") from this default
    /// unless it overrides the method to read its own controller.
    fn focus_ring_visible(&self) -> bool {
        false
    }
    /// The hover/press state this call paints with, overriding the
    /// ambient pointer state. Default: `None` — paint with the ambient
    /// state ([`crate::interaction::ambient_interaction`]).
    fn interaction(&self) -> Option<&InteractionState> {
        None
    }
}

/// `()` carries no paint-time state at all — the value
/// [`crate::backend::DrawsPrimitives::draw_dyn`] callers pass when the
/// widget being drawn has nothing [`PaintState`] would describe (e.g.
/// [`crate::primitives::cell_grid::CellGrid`] through
/// [`crate::frame::Surface::Widget`], which already keeps its
/// hover/selection/cursor state as persistent fields on itself — see
/// that widget's own [`Primitive::paint`] doc). Paints with the ambient
/// pointer state.
impl PaintState for () {}

/// An explicit hover/press state: the paint uses exactly this, not the
/// ambient pointer state.
impl PaintState for InteractionState {
    fn interaction(&self) -> Option<&InteractionState> {
        Some(self)
    }
}

/// An owned copy of a [`PaintState`]'s row answers — everything but
/// [`PaintState::interaction`], which a paint reads live rather than
/// keeping.
///
/// [`crate::DrawsPrimitives::draw_dyn`] records one per identified
/// widget ([`PaintedWidget::paint_state`]): the caller's state is a
/// borrow that ends with the paint, and event routing, which runs after
/// it, lays the widget out again with this copy attached
/// ([`LayoutCtx::with_paint_state`]) so it resolves the geometry that was
/// painted.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PaintStateSnapshot {
    /// [`PaintState::hovered_idx`].
    pub hovered_idx: Option<usize>,
    /// [`PaintState::pressed_idx`].
    pub pressed_idx: Option<usize>,
    /// [`PaintState::scroll_px`].
    pub scroll_px: Option<f32>,
    /// [`PaintState::striped`].
    pub striped: bool,
    /// [`PaintState::focus_ring_visible`].
    pub focus_ring_visible: bool,
}

impl PaintStateSnapshot {
    /// Copy `state`'s answers.
    pub fn of(state: &dyn PaintState) -> Self {
        Self {
            hovered_idx: state.hovered_idx(),
            pressed_idx: state.pressed_idx(),
            scroll_px: state.scroll_px(),
            striped: state.striped(),
            focus_ring_visible: state.focus_ring_visible(),
        }
    }
}

impl PaintState for PaintStateSnapshot {
    fn hovered_idx(&self) -> Option<usize> {
        self.hovered_idx
    }
    fn pressed_idx(&self) -> Option<usize> {
        self.pressed_idx
    }
    fn scroll_px(&self) -> Option<f32> {
        self.scroll_px
    }
    fn striped(&self) -> bool {
        self.striped
    }
    fn focus_ring_visible(&self) -> bool {
        self.focus_ring_visible
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

    /// This instance's own [`crate::types::WidgetId`], when it carries
    /// one. Default: `None` — most `Primitive` implementors are purely
    /// local values (a `Swatch`, a one-off `Divider`) with no identity a
    /// host needs to address; a widget that does carry an id (every
    /// built-in primitive has one as a plain field) overrides this to
    /// return it, which is what lets [`DynWidget`]'s blanket impl (and,
    /// through it, [`crate::frame::Surface::Widget`]) recover the id
    /// generically instead of needing a bespoke accessor per widget.
    fn id(&self) -> Option<&crate::types::WidgetId> {
        None
    }

    /// Which font the implementor paints in — see [`crate::FontRole`]'s
    /// module doc for the chrome-by-default rule this mirrors. Default:
    /// [`crate::FontRole::Chrome`], correct for every `Primitive`
    /// implementor outside [`crate::EditorClassPrimitive::ALL`]'s
    /// opt-out list.
    fn font_role(&self) -> crate::FontRole {
        crate::FontRole::Chrome
    }

    /// The implementor's best-effort intrinsic size within `avail`, for
    /// a layout container that sizes a child to its own content on a
    /// non-stretch cross axis. Default: [`crate::NaturalSize::NONE`] —
    /// "no cheap measurement, stretch to fill", the same always-stretch
    /// behaviour every `Primitive` implementor has without overriding
    /// this method.
    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        let _ = (ctx, avail);
        crate::NaturalSize::NONE
    }

    /// Clone the implementor's content into a serializable
    /// [`OwnedContent`] — see [`DynWidget::capture`]. Default:
    /// [`OwnedContent::Opaque`]; a widget whose content type implements
    /// `serde::{Serialize, Deserialize}` overrides this to return its own
    /// [`OwnedContent`] variant.
    fn capture(&self) -> OwnedContent {
        OwnedContent::Opaque
    }

    /// Resolve this primitive's layout for `rect`.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout;

    /// How far past its `rect` this primitive paints on a pixel surface,
    /// on every side: an overlay's drop shadow, or a border stroke
    /// centred on its edge. [`crate::DrawsPrimitives::draw_primitive`]
    /// and [`crate::DrawsPrimitives::draw_dyn`] widen their paint clip by
    /// this much, so that chrome is not cut off at the widget's own edge.
    /// Default: `0.0`, a primitive that paints only inside its `rect`.
    fn paint_bleed(&self, style: &Style) -> f32 {
        let _ = style;
        0.0
    }

    /// The rect the primitive's paint occupies for `layout`, in `units`
    /// (the painted surface's own coordinate space), when it is a surface
    /// a structural observer must be able to see. After painting an
    /// identified primitive ([`Self::id`]),
    /// [`crate::DrawsPrimitives::draw_primitive`] and
    /// [`crate::DrawsPrimitives::draw_dyn`] report a `Some` rect as that
    /// id's zone via [`crate::Backend::register_zone`], so it appears in
    /// [`crate::testing::FrameInventory::zones`]. A cell-grid widget
    /// returns the whole-cell rect it actually painted, not its raw float
    /// bounds. Default: `None` — the primitive reports no zone of its
    /// own, and nothing is registered.
    fn paint_zone(&self, layout: &Self::Layout, units: Units) -> Option<Rect> {
        let _ = (layout, units);
        None
    }

    /// Present the primitive through the platform itself, without
    /// painting it, when `backend` presents it natively — the macOS menu
    /// bar installed as the system one. Returns whether it did; on
    /// `false` the caller paints it as usual.
    /// [`crate::frame::ScreenLayout::draw`] asks every widget surface
    /// before painting it. Default: `false`, a primitive that is always
    /// painted.
    fn native_presentation(&self, backend: &mut dyn crate::Backend, rect: Rect) -> bool {
        let _ = (backend, rect);
        false
    }

    /// Paint `layout` (as produced by [`Self::layout`]) onto `surface`,
    /// with the theme, style and hover/press state `env` carries.
    fn paint(&self, layout: &Self::Layout, surface: &mut dyn PaintSurface, env: &PaintEnv);

    /// Paint `layout` onto a TUI cell grid —
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
    /// Takes the backend-neutral [`CellSurface`], so a widget reaches the
    /// cursor ([`CellSurface::set_cursor`]) and inline graphics
    /// ([`CellSurface::emit_graphics`]) verbs, and no `ratatui` type
    /// appears in this trait. A painter that still delegates to a
    /// `crate::tui` rasteriser over a `ratatui` buffer reaches it through
    /// [`CellSurface::ratatui_buffer`], marked `// CSR:` at the call. A
    /// caller with a bare buffer and no backend wraps it in
    /// `quadraui::tui::BufferCellSurface`.
    ///
    /// `#[cfg(feature = "tui")]`: unlike [`Self::paint`], which exists
    /// unconditionally because every pixel backend needs it, only a
    /// `tui`-enabled build has a cell backend to call this.
    ///
    /// Default: no-op — correct for a `Primitive` implementor with no
    /// TUI story at all (every built-in primitive that has a TUI
    /// rasteriser overrides it); such an implementor painted through
    /// [`crate::backend::DrawsPrimitives::draw_primitive`] on TUI still
    /// gets a real, hit-testable [`Self::Layout`] back, just no pixels
    /// on screen, the same "resolve layout, maybe paint" contract that
    /// default already has for a `PaintSurface`-less backend.
    #[cfg(feature = "tui")]
    fn paint_cells(&self, layout: &Self::Layout, cells: &mut dyn CellSurface, env: &PaintEnv) {
        let _ = (layout, cells, env);
    }
}

/// Type-erased view of any [`Interactive`] widget — the one shape
/// [`crate::frame::Surface::Widget`] and
/// [`crate::backend::DrawsPrimitives::draw_dyn`] paint, hit-test and
/// route events to, so the frame/runtime/binding layers each need
/// exactly one code path for every widget instead of one hand-written
/// `Surface` variant (and matching `Backend` paint/layout pair) per
/// primitive. See `crate::interactive`'s module doc for the `Interactive`
/// half this erases, and this module's own doc for why `Primitive` alone
/// — generic `Layout`/`Event` associated types — cannot be a trait
/// object.
///
/// Never implemented by hand: the blanket `impl<W: Interactive> DynWidget
/// for W` below covers every [`Interactive`] widget automatically,
/// downcasting [`PrimitiveLayout`]/`dyn Any` state back to `W::Layout`/
/// `W::State` via [`PrimitiveLayout::as_any`]/[`std::any::Any::downcast_mut`]
/// and mapping `W::Action` to a [`crate::UiEvent`] via
/// [`WidgetAction::into_ui_event`].
pub trait DynWidget: A11yPrimitive + std::any::Any {
    /// This widget's `&dyn Any` view, for downcasting back to the
    /// concrete widget type — the [`DynWidget`] counterpart of
    /// [`PrimitiveLayout::as_any`], for a caller that needs a method this
    /// trait doesn't erase (e.g. `quadraui::CellGrid::cells_requested`,
    /// which a binding layer has no generic hook for yet). Prefer
    /// [`downcast_ref`][dyn@DynWidget::downcast_ref] over calling this
    /// directly — same `Box`-vs-trait-object pitfall
    /// [`PrimitiveLayout::as_any`]'s own doc describes.
    fn as_any(&self) -> &dyn std::any::Any;

    /// This widget's own id, if it carries one — see [`Primitive::id`].
    fn id(&self) -> Option<&WidgetId>;

    /// Which font this widget paints in — see [`Primitive::font_role`].
    fn font_role(&self) -> crate::FontRole;

    /// This widget's best-effort intrinsic size — see [`Primitive::measure`].
    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize;

    /// Resolve this widget's layout for `rect`, erased behind
    /// [`PrimitiveLayout`] — see [`Primitive::layout`].
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Box<dyn PrimitiveLayout>;

    /// How far past its `rect` this widget paints — see
    /// [`Primitive::paint_bleed`].
    fn paint_bleed(&self, style: &Style) -> f32;

    /// The rect this widget's paint occupies for `layout`, if it reports
    /// one — see [`Primitive::paint_zone`]. `None` when `layout` isn't
    /// this widget's own layout type.
    fn paint_zone(&self, layout: &dyn PrimitiveLayout, units: Units) -> Option<Rect>;

    /// Present this widget through the platform, without painting it —
    /// see [`Primitive::native_presentation`].
    fn native_presentation(&self, backend: &mut dyn crate::Backend, rect: Rect) -> bool;

    /// Paint `layout` (as produced by [`Self::layout`]) onto `target`.
    /// Replaces [`Primitive::paint`]/[`Primitive::paint_cells`]'s split
    /// with a single method that matches on [`PaintTarget`] itself, since
    /// a type-erased caller (e.g.
    /// [`crate::backend::DrawsPrimitives::draw_dyn`]) has exactly one
    /// paint call to make regardless of which target it resolved. The
    /// caller's [`PaintState`] and the resolved hover/press state travel
    /// in `env`.
    fn paint(&self, layout: &dyn PrimitiveLayout, target: PaintTarget<'_>, env: &PaintEnv);

    /// How this widget participates in keyboard tab order — see
    /// [`Interactive::focus`].
    fn focus(&self) -> FocusSpec;

    /// This widget's addressable sub-parts for `layout`, in widget-local
    /// coordinates — see [`Interactive::sub_regions`]. Returns an empty
    /// `Vec` when `layout` isn't this widget's own layout type, matching
    /// [`Self::handle`]'s "wrong pairing is a programmer error reported
    /// as nothing, not a panic" convention.
    fn sub_regions(
        &self,
        layout: &dyn PrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)>;

    /// [`Interactive::HOVER_ON_SUB_REGIONS_ONLY`].
    fn hover_on_sub_regions_only(&self) -> bool;

    /// [`Interactive::RECEIVES_HOVER_MOVES`].
    fn receives_hover_moves(&self) -> bool;

    /// The frame-inventory name of sub-part `sub` — see
    /// [`Interactive::sub_region_name`].
    fn sub_region_name(&self, sub: crate::interactive::SubTarget) -> Option<&'static str>;

    /// A fresh, default-valued interaction state for this widget — the
    /// type-erased counterpart of [`Interactive::State::default`].
    fn new_state(&self) -> Box<dyn std::any::Any>;

    /// Handle one [`RoutedEvent`] — the type-erased counterpart of
    /// [`Interactive::handle`], with `state`/`layout` erased behind `dyn
    /// Any`/[`PrimitiveLayout`] and the returned action already turned
    /// into a [`crate::UiEvent`] via [`WidgetAction::into_ui_event`]. A
    /// `state`/`layout` that fails to downcast back to this widget's own
    /// `W::State`/`W::Layout` (a caller handing in the wrong pairing) is
    /// a programmer error reported as [`Handled::Ignored`] rather than a
    /// panic, mirroring every other "not mine" outcome this method can
    /// return.
    fn handle(
        &self,
        state: &mut dyn std::any::Any,
        event: &RoutedEvent,
        layout: &dyn PrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<crate::UiEvent>;

    /// Assistive-technology actions this widget currently supports — see
    /// [`Interactive::a11y_actions`].
    fn a11y_actions(&self, state: &dyn std::any::Any) -> Vec<A11yAction>;

    /// [`Interactive::OPENS_POPUPS`]: whether [`Self::popups`] can ever
    /// answer anything.
    fn opens_popups(&self) -> bool;

    /// The popups this widget has open — see [`Interactive::popups`].
    /// `state` `None` (no stored state yet) or of the wrong type answers
    /// as `State::default()`; a `layout` of the wrong type answers no
    /// popups.
    fn popups(
        &self,
        state: Option<&dyn std::any::Any>,
        layout: &dyn PrimitiveLayout,
    ) -> Vec<crate::popup::Popup>;

    /// Clone this widget's content into a serializable [`OwnedContent`]
    /// for [`crate::frame::OwnedFrame::from_layout`]. Default:
    /// [`OwnedContent::Opaque`] — correct for a widget with no
    /// serializable capture of its own; a widget that does implement
    /// `serde::{Serialize, Deserialize}` (every built-in primitive)
    /// overrides this to return its own [`OwnedContent`] variant, the
    /// same way [`crate::frame::capture_surface`] already does per
    /// `Surface` variant.
    fn capture(&self) -> OwnedContent {
        OwnedContent::Opaque
    }
}

impl<W> DynWidget for W
where
    W: Interactive + 'static,
    W::Layout: 'static,
    W::State: 'static,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn id(&self) -> Option<&WidgetId> {
        Primitive::id(self)
    }

    fn font_role(&self) -> crate::FontRole {
        Primitive::font_role(self)
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        Primitive::measure(self, ctx, avail)
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Box<dyn PrimitiveLayout> {
        Box::new(Primitive::layout(self, ctx, rect))
    }

    fn paint_bleed(&self, style: &Style) -> f32 {
        Primitive::paint_bleed(self, style)
    }

    fn paint_zone(&self, layout: &dyn PrimitiveLayout, units: Units) -> Option<Rect> {
        let layout = layout.downcast_ref::<W::Layout>()?;
        Primitive::paint_zone(self, layout, units)
    }

    fn native_presentation(&self, backend: &mut dyn crate::Backend, rect: Rect) -> bool {
        Primitive::native_presentation(self, backend, rect)
    }

    fn paint(&self, layout: &dyn PrimitiveLayout, target: PaintTarget<'_>, env: &PaintEnv) {
        let Some(layout) = layout.downcast_ref::<W::Layout>() else {
            return;
        };
        match target {
            PaintTarget::Surface(surface) => Primitive::paint(self, layout, surface, env),
            #[cfg(feature = "tui")]
            PaintTarget::Cells(cells) => Primitive::paint_cells(self, layout, cells, env),
        }
    }

    fn focus(&self) -> FocusSpec {
        Interactive::focus(self)
    }

    fn sub_regions(
        &self,
        layout: &dyn PrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        match layout.downcast_ref::<W::Layout>() {
            Some(layout) => Interactive::sub_regions(self, layout),
            None => Vec::new(),
        }
    }

    fn sub_region_name(&self, sub: crate::interactive::SubTarget) -> Option<&'static str> {
        Interactive::sub_region_name(self, sub)
    }

    fn hover_on_sub_regions_only(&self) -> bool {
        W::HOVER_ON_SUB_REGIONS_ONLY
    }

    fn receives_hover_moves(&self) -> bool {
        W::RECEIVES_HOVER_MOVES
    }

    fn new_state(&self) -> Box<dyn std::any::Any> {
        Box::new(W::State::default())
    }

    fn handle(
        &self,
        state: &mut dyn std::any::Any,
        event: &RoutedEvent,
        layout: &dyn PrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<crate::UiEvent> {
        let id = cx.id;
        let (Some(state), Some(layout)) = (
            state.downcast_mut::<W::State>(),
            layout.downcast_ref::<W::Layout>(),
        ) else {
            return Handled::Ignored;
        };
        Interactive::handle(self, state, event, layout, cx).map(|action| action.into_ui_event(id))
    }

    fn a11y_actions(&self, state: &dyn std::any::Any) -> Vec<A11yAction> {
        match state.downcast_ref::<W::State>() {
            Some(state) => Interactive::a11y_actions(self, state),
            None => Vec::new(),
        }
    }

    fn opens_popups(&self) -> bool {
        W::OPENS_POPUPS
    }

    fn popups(
        &self,
        state: Option<&dyn std::any::Any>,
        layout: &dyn PrimitiveLayout,
    ) -> Vec<crate::popup::Popup> {
        if !W::OPENS_POPUPS {
            return Vec::new();
        }
        let Some(layout) = layout.downcast_ref::<W::Layout>() else {
            return Vec::new();
        };
        match state.and_then(|s| s.downcast_ref::<W::State>()) {
            Some(state) => Interactive::popups(self, state, layout),
            None => Interactive::popups(self, &W::State::default(), layout),
        }
    }

    fn capture(&self) -> OwnedContent {
        Primitive::capture(self)
    }
}

/// A widget painted at a layout its host already resolved, in place of
/// the one the widget's own [`Primitive::layout`] would resolve.
///
/// A popup host often places a widget itself: it flips a tooltip above
/// its anchor, sizes a dialog box to content it measured, or positions a
/// completion list against a cursor. Wrapping the widget and that layout
/// in a `Placed` hands it to [`crate::DrawsPrimitives::draw_dyn`] /
/// [`crate::DrawsPrimitives::draw_primitive`] unchanged: [`Self::layout`]
/// returns the held layout whatever `rect` it is given, and every other
/// part of the widget contract — paint, bleed, zone, focus, sub-regions,
/// event handling, popups, accessibility — is the widget's own. Painting
/// and hit-testing therefore agree with the host's placement.
///
/// The held layout must be in the same coordinate frame the widget's own
/// [`Primitive::layout`] would return for the `rect` passed to the draw
/// call — absolute for a widget whose layout bakes in its area's origin.
/// A host whose widget's layout type is `#[non_exhaustive]` starts from
/// the widget's own resolution and overrides the fields it owns:
///
/// ```ignore
/// let mut layout = backend.layout_primitive(viewport, &dialog);
/// layout.layout = host_dialog_layout;
/// backend.draw_dyn(viewport, &quadraui::Placed::new(dialog, layout), &state);
/// ```
pub struct Placed<W: Primitive> {
    /// The widget painted.
    pub widget: W,
    /// The layout it paints at.
    pub layout: W::Layout,
}

impl<W: Primitive> Placed<W> {
    /// Pair `widget` with the `layout` it paints at.
    pub fn new(widget: W, layout: W::Layout) -> Self {
        Self { widget, layout }
    }
}

impl<W: Primitive> A11yPrimitive for Placed<W> {
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        self.widget.a11y_info()
    }

    fn a11y_children(&self) -> Vec<crate::a11y::A11yChild> {
        self.widget.a11y_children()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        self.widget.a11y_focus_key()
    }
}

impl<W> Primitive for Placed<W>
where
    W: Primitive,
    W::Layout: Clone,
{
    type Layout = W::Layout;
    type Event = W::Event;

    fn id(&self) -> Option<&WidgetId> {
        self.widget.id()
    }

    fn font_role(&self) -> crate::FontRole {
        self.widget.font_role()
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        self.widget.measure(ctx, avail)
    }

    fn capture(&self) -> OwnedContent {
        self.widget.capture()
    }

    /// The held layout; `rect` is not consulted.
    fn layout(&self, _ctx: &LayoutCtx, _rect: Rect) -> W::Layout {
        self.layout.clone()
    }

    fn paint_bleed(&self, style: &Style) -> f32 {
        self.widget.paint_bleed(style)
    }

    fn paint_zone(&self, layout: &W::Layout, units: Units) -> Option<Rect> {
        self.widget.paint_zone(layout, units)
    }

    fn native_presentation(&self, backend: &mut dyn crate::Backend, rect: Rect) -> bool {
        self.widget.native_presentation(backend, rect)
    }

    fn paint(&self, layout: &W::Layout, surface: &mut dyn PaintSurface, env: &PaintEnv) {
        self.widget.paint(layout, surface, env)
    }

    #[cfg(feature = "tui")]
    fn paint_cells(&self, layout: &W::Layout, cells: &mut dyn CellSurface, env: &PaintEnv) {
        self.widget.paint_cells(layout, cells, env)
    }
}

impl<W> Interactive for Placed<W>
where
    W: Interactive,
    W::Layout: Clone,
{
    type State = W::State;
    type Action = W::Action;

    const HOVER_ON_SUB_REGIONS_ONLY: bool = W::HOVER_ON_SUB_REGIONS_ONLY;
    const RECEIVES_HOVER_MOVES: bool = W::RECEIVES_HOVER_MOVES;
    const OPENS_POPUPS: bool = W::OPENS_POPUPS;

    fn focus(&self) -> FocusSpec {
        self.widget.focus()
    }

    fn handle(
        &self,
        state: &mut W::State,
        event: &RoutedEvent,
        layout: &W::Layout,
        cx: &mut EventCx,
    ) -> Handled<W::Action> {
        self.widget.handle(state, event, layout, cx)
    }

    fn a11y_actions(&self, state: &W::State) -> Vec<A11yAction> {
        self.widget.a11y_actions(state)
    }

    fn sub_regions(&self, layout: &W::Layout) -> Vec<(Rect, crate::interactive::SubTarget)> {
        self.widget.sub_regions(layout)
    }

    fn sub_region_name(&self, sub: crate::interactive::SubTarget) -> Option<&'static str> {
        self.widget.sub_region_name(sub)
    }

    fn popups(&self, state: &W::State, layout: &W::Layout) -> Vec<crate::popup::Popup> {
        self.widget.popups(state, layout)
    }
}

impl dyn DynWidget {
    /// Downcast back to a concrete widget type `T` — `self.as_any()` plus
    /// [`std::any::Any::downcast_ref`] in one call. Inherent to `dyn
    /// DynWidget` itself, not part of the [`DynWidget`] trait, for the
    /// same `Box`-erases-the-`Box`-not-the-widget reason
    /// [`dyn PrimitiveLayout`]'s own `downcast_ref` is inherent — see that
    /// method's doc.
    pub fn downcast_ref<T: std::any::Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }
}

// ── Painted-layout record ───────────────────────────────────────────────

/// One widget's geometry as [`crate::DrawsPrimitives::draw_dyn`] painted
/// it this frame — the entry [`PaintedWidgets`] stores per identified
/// widget.
///
/// `rect` and `sub_regions` are **target-surface absolute** (the same
/// space a [`crate::UiEvent::MouseDown`] position arrives in), so a
/// driver or binding layer hit-tests them without knowing where the
/// widget sits. `layout` stays exactly as [`DynWidget::layout`] produced
/// it — in the widget's own local space — and downcasts back to the
/// widget's concrete layout type through
/// [`dyn PrimitiveLayout::downcast_ref`][dyn@PrimitiveLayout::downcast_ref].
pub struct PaintedWidget {
    /// The widget's own id — [`DynWidget::id`], which is `Some` for every
    /// recorded entry (an anonymous widget is never recorded, since
    /// nothing could address it afterwards).
    pub id: WidgetId,
    /// The absolute rect the widget was painted into.
    pub rect: Rect,
    /// The layout that painted, erased behind [`PrimitiveLayout`].
    pub layout: Box<dyn PrimitiveLayout>,
    /// This widget's addressable sub-parts, most-specific first — see
    /// [`Interactive::sub_regions`], whose widget-local rects are
    /// translated into absolute space here.
    pub sub_regions: Vec<(Rect, crate::interactive::SubTarget)>,
    /// The sub-parts the pointer counts as on this widget over, when the
    /// widget hovers only on some of them (see
    /// [`Interactive::HOVER_ON_SUB_REGIONS_ONLY`]); everywhere else the
    /// pointer passes through to whatever is painted beneath. `None`: the
    /// whole of [`Self::rect`].
    pub hover_targets: Option<Vec<crate::interactive::SubTarget>>,
    /// The paint state the widget painted with, which its layout was
    /// resolved against.
    pub paint_state: PaintStateSnapshot,
}

/// Every widget [`crate::DrawsPrimitives::draw_dyn`] painted in the
/// current frame, in paint order.
///
/// # Why this exists
///
/// A widget painted through the widget contract has no per-backend layout
/// cache to read back: `draw_dyn` resolves `w.layout(&ctx, rect)`, paints
/// it and drops it. Drivers that need to click one *sub-part* of a widget
/// (`TuiDriver::tab_center`, the GTK equivalent) and binding layers that
/// need the sub-part under a point therefore had nothing generic to ask.
/// This is that record: one store, filled by `draw_dyn` for every widget
/// whose [`DynWidget::id`] is `Some`, cleared at the start of every frame.
///
/// Deliberately widget-agnostic — a widget names its own sub-parts
/// through [`Interactive::sub_regions`], so adding a widget to the
/// contract never adds a field here.
///
/// [`crate::DrawsPrimitives::draw_primitive`] does not fill this: it
/// paints a bare [`Primitive`], which carries no [`Interactive`] half to
/// name sub-parts with, and its caller already holds the concrete
/// `P::Layout` it returned. The record is for the type-erased path, where
/// the layout is dropped as soon as the frame is painted.
#[derive(Default)]
pub struct PaintedWidgets {
    entries: Vec<PaintedWidget>,
}

impl PaintedWidgets {
    /// Drop every entry — called once per frame, before anything paints.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Append one painted widget, in paint order.
    pub fn record(&mut self, widget: PaintedWidget) {
        self.entries.push(widget);
    }

    /// Every entry, in paint order (back to front).
    pub fn iter(&self) -> impl Iterator<Item = &PaintedWidget> {
        self.entries.iter()
    }

    /// How many widgets painted this frame.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing painted through the widget contract this frame.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The absolute rect and layout `id` painted with this frame, or
    /// `None` if it didn't paint.
    ///
    /// The *last* entry wins when one id painted twice in a frame (a
    /// two-pass paint that corrects its own scroll offset, as
    /// [`crate::compose::WorkspaceController::render`] does), so a caller
    /// always reads the geometry that is actually on screen.
    pub fn painted_layout(&self, id: &WidgetId) -> Option<(Rect, &dyn PrimitiveLayout)> {
        self.entries
            .iter()
            .rev()
            .find(|e| &e.id == id)
            .map(|e| (e.rect, e.layout.as_ref()))
    }

    /// The paint state `id` painted with this frame, or `None` if it
    /// didn't paint. Same last-entry-wins rule as
    /// [`Self::painted_layout`].
    pub fn painted_state(&self, id: &WidgetId) -> Option<PaintStateSnapshot> {
        self.entries
            .iter()
            .rev()
            .find(|e| &e.id == id)
            .map(|e| e.paint_state)
    }

    /// The widget under `(x, y)` and, when its layout names one, the
    /// sub-part under that same point.
    ///
    /// Topmost wins: entries are searched in reverse paint order, so a
    /// widget painted over another one answers first. `Some((id, None))`
    /// means the point landed inside the widget but on no addressable
    /// sub-part (dead space, or a widget that names no sub-parts at all).
    pub fn painted_hit(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(WidgetId, Option<crate::interactive::SubTarget>)> {
        self.entries.iter().rev().find_map(|e| {
            if !contains(e.rect, x, y) {
                return None;
            }
            let sub = e
                .sub_regions
                .iter()
                .find(|(r, _)| contains(*r, x, y))
                .map(|(_, sub)| *sub);
            match &e.hover_targets {
                Some(targets) if !sub.is_some_and(|sub| targets.contains(&sub)) => None,
                _ => Some((e.id, sub)),
            }
        })
    }

    /// The absolute rect of `id`'s `sub` sub-part, as painted this frame.
    ///
    /// Same last-entry-wins rule as [`Self::painted_layout`].
    pub fn painted_sub_rect(
        &self,
        id: &WidgetId,
        sub: crate::interactive::SubTarget,
    ) -> Option<Rect> {
        self.entries
            .iter()
            .rev()
            .find(|e| &e.id == id)?
            .sub_regions
            .iter()
            .find(|(_, s)| *s == sub)
            .map(|(r, _)| *r)
    }

    /// The center of [`Self::painted_sub_rect`], the point a driver
    /// clicks to land on that sub-part.
    pub fn painted_sub_center(
        &self,
        id: &WidgetId,
        sub: crate::interactive::SubTarget,
    ) -> Option<(f32, f32)> {
        self.painted_sub_rect(id, sub)
            .map(|r| (r.x + r.width / 2.0, r.y + r.height / 2.0))
    }
}

/// Half-open containment, matching [`crate::TabBarLayout::hit_test`]'s own
/// convention: the right/bottom edge belongs to the next rect along.
pub(crate) fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

thread_local! {
    /// The current frame's [`PaintedWidgets`].
    ///
    /// A `thread_local!` rather than a [`crate::Backend`] field reached
    /// through a trait method, for two reasons. The store has to be
    /// writable from [`crate::DrawsPrimitives::draw_dyn`]'s blanket impl
    /// (which only ever has `&mut dyn Backend`-shaped access, so it can
    /// reach backend state through `Backend` methods alone) *and*
    /// readable from a shared `&TuiBackend`/`&GtkBackend` in a driver —
    /// two accessors, on a trait whose method count this repo ratchets
    /// strictly downwards (`tests/backend_method_ratchet.rs`). And the
    /// record is per-frame, per-painting-thread state with exactly one
    /// writer: every backend paints its own frames on the thread that
    /// owns its event loop, so thread-local storage is the same scope the
    /// data already has. [`crate::gtk`]'s own font-override store and
    /// [`crate::testing`]'s scenario hooks use the same shape.
    static PAINTED_WIDGETS: std::cell::RefCell<PaintedWidgets> =
        std::cell::RefCell::new(PaintedWidgets::default());
}

/// Drop the previous frame's painted-layout record.
///
/// Called once per frame from `BackendCore::begin_frame` — the shared
/// begin-frame step every backend already runs — so a widget that stops
/// painting stops being addressable, exactly like each backend's own
/// per-frame caches.
pub fn clear_painted_widgets() {
    PAINTED_WIDGETS.with(|p| p.borrow_mut().clear());
}

/// Record one widget painted by [`crate::DrawsPrimitives::draw_dyn`].
pub fn record_painted_widget(widget: PaintedWidget) {
    PAINTED_WIDGETS.with(|p| p.borrow_mut().record(widget));
}

/// Read this frame's [`PaintedWidgets`].
///
/// A closure rather than a returned reference: the store lives in
/// thread-local storage, so a borrow cannot outlive the call. Every query
/// helper ([`PaintedWidgets::painted_layout`],
/// [`PaintedWidgets::painted_hit`], …) is a method on the store itself, so
/// a caller reads one of them inside `f`.
///
/// `f` reads geometry; it must not paint. The store is borrowed for the
/// duration of the call, so a [`crate::DrawsPrimitives::draw_dyn`] call
/// from inside `f` — which records into that same store — panics on the
/// conflicting borrow. Copy out what is needed (a rect, a point) and
/// paint after the call returns.
pub fn with_painted_widgets<R>(f: impl FnOnce(&PaintedWidgets) -> R) -> R {
    PAINTED_WIDGETS.with(|p| f(&p.borrow()))
}

/// The widget-contract widget under `(x, y)` this frame, and the sub-part
/// under that same point — [`PaintedWidgets::painted_hit`] against the
/// live record.
pub fn painted_hit(x: f32, y: f32) -> Option<(WidgetId, Option<crate::interactive::SubTarget>)> {
    with_painted_widgets(|p| p.painted_hit(x, y))
}

/// The paint state `id` painted with this frame —
/// [`PaintedWidgets::painted_state`] against the live record — or the
/// default (no paint-time state) when it didn't paint or the record is
/// borrowed for writing.
pub fn painted_state(id: &WidgetId) -> PaintStateSnapshot {
    PAINTED_WIDGETS
        .with(|p| p.try_borrow().ok().and_then(|p| p.painted_state(id)))
        .unwrap_or_default()
}

/// The absolute center of `id`'s `sub` sub-part this frame —
/// [`PaintedWidgets::painted_sub_center`] against the live record.
pub fn painted_sub_center(id: &WidgetId, sub: crate::interactive::SubTarget) -> Option<(f32, f32)> {
    with_painted_widgets(|p| p.painted_sub_center(id, sub))
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

    /// A ctx with no paint state attached answers every paint-state
    /// question with its default; an attached state answers through
    /// [`LayoutCtx::paint_state`], and `scroll_px` takes part in
    /// equality since a layout resolves geometry from it.
    #[test]
    fn layout_ctx_carries_the_attached_paint_state() {
        let bare = LayoutCtx::new(16.0, 8.0, false);
        assert_eq!(bare.paint_state().scroll_px(), None);
        let state = PaintStateSnapshot {
            scroll_px: Some(12.5),
            striped: true,
            ..PaintStateSnapshot::default()
        };
        let scrolled = bare.with_paint_state(&state);
        assert_eq!(scrolled.paint_state().scroll_px(), Some(12.5));
        assert!(scrolled.paint_state().striped());
        assert_ne!(scrolled, bare);
    }

    /// The record keeps a copy of the state each widget painted with,
    /// and the last paint of an id wins.
    #[test]
    fn painted_state_reads_back_the_last_paint_of_an_id() {
        let id = WidgetId::new("rows");
        let mut painted = PaintedWidgets::default();
        for scroll_px in [Some(4.0), Some(9.5)] {
            painted.record(PaintedWidget {
                id,
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                layout: Box::new(()),
                sub_regions: Vec::new(),
                hover_targets: None,
                paint_state: PaintStateSnapshot::of(
                    &crate::primitives::list::ListPaintState::default().with_scroll_px(scroll_px),
                ),
            });
        }
        assert_eq!(
            painted.painted_state(&id).and_then(|s| s.scroll_px),
            Some(9.5)
        );
        assert_eq!(painted.painted_state(&WidgetId::new("other")), None);
    }

    /// [`LayoutCtx::units`] reports [`Units::Cells`] for TUI's whole-cell
    /// `line_height` convention and [`Units::Pixels`] for a real pixel
    /// font metric, matching [`Units::Pixels`]/[`Units::Cells`]'s own
    /// doc.
    #[test]
    fn layout_ctx_units_discriminates_cells_from_pixels() {
        let cells = LayoutCtx::new(1.0, 1.0, false);
        let pixels = LayoutCtx::new(22.0, 8.0, false);
        assert_eq!(cells.units(), Units::Cells);
        assert_eq!(pixels.units(), Units::Pixels);
    }

    /// [`LayoutCtx::measure`] bundles the ctx's own metrics plus the
    /// caller-supplied [`Units`] into a [`LayoutMeasure`] — no implicit
    /// recomputation of either field.
    #[test]
    fn layout_ctx_measure_bundles_its_own_fields_verbatim() {
        let ctx = LayoutCtx::new(22.0, 8.0, true);
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

    /// `line_height <= 1.0` (TUI's whole-cell convention) reports
    /// [`Units::Cells`]; anything above it (a pixel backend's real font
    /// metric) reports [`Units::Pixels`].
    #[test]
    fn layout_ctx_units_follows_the_line_height_convention() {
        let cells = LayoutCtx::new(1.0, 8.0, false);
        let pixels = LayoutCtx::new(22.0, 8.0, false);
        assert_eq!(cells.units(), Units::Cells);
        assert_eq!(pixels.units(), Units::Pixels);
    }

    /// [`AreaLayout::new`] pairs `area` with the primitive-local layout
    /// verbatim — no coordinate transform of either field, the same
    /// contract every hand-rolled `*PrimitiveLayout` already follows.
    #[test]
    fn area_layout_new_pairs_area_and_layout_verbatim() {
        let area = Rect::new(1.0, 2.0, 3.0, 4.0);
        let layout = AreaLayout::new(area, 42_u32);
        assert_eq!(layout.area, area);
        assert_eq!(layout.layout, 42);
    }

    /// The blanket [`PrimitiveLayout`] impl lets any `'static` value
    /// (here a plain [`AreaLayout`]) be reached through `&dyn
    /// PrimitiveLayout` and downcast back to its concrete type — the
    /// round-trip [`DynWidget`]'s blanket impl relies on.
    #[test]
    fn primitive_layout_as_any_downcasts_back_to_the_concrete_type() {
        let area = Rect::new(0.0, 0.0, 1.0, 1.0);
        let layout = AreaLayout::new(area, 7_u32);
        let erased: Box<dyn PrimitiveLayout> = Box::new(layout);
        let back = erased
            .downcast_ref::<AreaLayout<u32>>()
            .expect("downcasts back to AreaLayout<u32>");
        assert_eq!(*back, layout);
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
        let mut cells = crate::tui::BufferCellSurface::new(&mut buf);
        let target = PaintTarget::Cells(&mut cells);
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
    fn paint_env_bundles_services_and_row_height() {
        let theme = Theme::default();
        let style = Style::default();
        let env = PaintEnv::new(&theme, &style).with_row_height(22.0);
        assert_eq!(*env.services.theme, theme);
        assert_eq!(*env.style(), style);
        assert_eq!(env.row_height, 22.0);
        assert_eq!(*env.interaction, InteractionState::new());
    }

    fn address(id: &str, sub: Option<u64>) -> crate::focus::FocusAddress {
        (WidgetId::new(id), sub.map(crate::interactive::SubTarget))
    }

    #[test]
    fn paint_env_hovered_reports_the_widget_and_its_sub_part() {
        let (theme, style) = (Theme::default(), Style::default());
        let state = InteractionState::from_targets(Some(address("bar", Some(2))), None);
        let env = PaintEnv::new(&theme, &style).with_interaction(&state);
        let bar = WidgetId::new("bar");
        assert_eq!(
            env.hovered(&bar),
            Some(Some(crate::interactive::SubTarget(2)))
        );
        assert_eq!(
            env.hovered_sub(&bar),
            Some(crate::interactive::SubTarget(2))
        );
        assert!(env.is_hovered(&bar));
        assert!(!env.is_hovered(&WidgetId::new("other")));
        assert_eq!(env.hovered(&WidgetId::new("other")), None);

        let on_widget = InteractionState::from_targets(Some(address("bar", None)), None);
        let env = env.with_interaction(&on_widget);
        assert_eq!(env.hovered(&bar), Some(None));
        assert_eq!(env.hovered_sub(&bar), None);
    }

    #[test]
    fn paint_env_pressed_requires_the_pointer_on_the_pressed_address() {
        let (theme, style) = (Theme::default(), Style::default());
        let bar = WidgetId::new("bar");
        let held = InteractionState::from_targets(
            Some(address("bar", Some(1))),
            Some(address("bar", Some(1))),
        );
        let env = PaintEnv::new(&theme, &style).with_interaction(&held);
        assert!(env.is_pressed(&bar));
        assert_eq!(
            env.pressed_sub(&bar),
            Some(crate::interactive::SubTarget(1))
        );

        let dragged_off = InteractionState::from_targets(
            Some(address("bar", Some(2))),
            Some(address("bar", Some(1))),
        );
        let env = env.with_interaction(&dragged_off);
        assert!(!env.is_pressed(&bar));
        assert_eq!(env.pressed(&bar), None);

        let pointer_gone = InteractionState::from_targets(None, Some(address("bar", Some(1))));
        let env = env.with_interaction(&pointer_gone);
        assert!(!env.is_pressed(&bar));
    }

    #[test]
    fn paint_env_child_interaction_readdresses_parent_slots_to_the_child() {
        let (theme, style) = (Theme::default(), Style::default());
        let state = InteractionState::from_targets(
            Some(address("panel", Some(3))),
            Some(address("elsewhere", None)),
        );
        let env = PaintEnv::new(&theme, &style).with_interaction(&state);
        let child = env.child_interaction(&WidgetId::new("panel"), &WidgetId::new("header"));
        assert_eq!(child.hovered_target(), Some(address("header", Some(3))));
        assert_eq!(child.pressed_target(), None);
    }

    #[test]
    fn interaction_state_overrides_and_unit_defers_to_ambient() {
        let state = InteractionState::from_parts(Some(WidgetId::new("w")), None);
        assert_eq!(PaintState::interaction(&state), Some(&state));
        assert_eq!(PaintState::interaction(&()), None);
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
        assert!(!state.focus_ring_visible());
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

    /// `dyn DynWidget::downcast_ref` recovers the concrete widget behind a
    /// type-erased `&dyn DynWidget` — the seam a caller with a method
    /// `DynWidget` itself doesn't erase (e.g.
    /// `quadraui::CellGrid::cells_requested`) reaches for instead of
    /// extending the trait's own vtable.
    #[test]
    fn dyn_widget_downcast_ref_recovers_the_concrete_type() {
        use crate::CellGrid;

        let grid = CellGrid {
            id: WidgetId::new("sheet"),
            row_count: 1,
            col_count: 1,
            frozen_rows: 0,
            frozen_cols: 0,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 8.0,
            row_heights: Default::default(),
            col_widths: Default::default(),
            show_row_headers: false,
            show_col_headers: false,
            row_labels: Default::default(),
            col_labels: Default::default(),
            cursor: (0, 0),
            selection_anchor: None,
            cells: Vec::new(),
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: false,
            show_scrollbar: false,
            editing: None,
            column_separators: false,
            striped: false,
        };
        let erased: Box<dyn DynWidget> = Box::new(grid);
        let recovered = erased
            .as_ref()
            .downcast_ref::<CellGrid>()
            .expect("CellGrid should downcast back out of its own DynWidget erasure");
        assert_eq!(recovered.id.as_str(), "sheet");

        // A mismatched concrete type downcasts to `None` rather than
        // panicking.
        assert!(erased.as_ref().downcast_ref::<crate::Spinner>().is_none());
    }

    /// Two overlapping widgets: the one painted last is the one on
    /// screen, so a point inside both resolves to it.
    #[test]
    fn painted_hit_prefers_the_widget_painted_last() {
        let under = WidgetId::new("under");
        let over = WidgetId::new("over");
        let mut painted = PaintedWidgets::default();
        painted.record(PaintedWidget {
            id: under,
            rect: Rect::new(0.0, 0.0, 20.0, 10.0),
            layout: Box::new(()),
            sub_regions: Vec::new(),
            hover_targets: None,
            paint_state: Default::default(),
        });
        painted.record(PaintedWidget {
            id: over,
            rect: Rect::new(5.0, 0.0, 20.0, 10.0),
            layout: Box::new(()),
            sub_regions: Vec::new(),
            hover_targets: None,
            paint_state: Default::default(),
        });

        assert_eq!(painted.painted_hit(6.0, 1.0), Some((over, None)));
        assert_eq!(painted.painted_hit(1.0, 1.0), Some((under, None)));
        assert_eq!(painted.painted_hit(40.0, 1.0), None);
    }

    /// Sub-regions are answered in the order the widget listed them, so a
    /// widget that lists its most specific part first wins the overlap —
    /// and a point on no sub-region still resolves the widget itself.
    #[test]
    fn painted_hit_answers_the_first_matching_sub_region() {
        let id = WidgetId::new("composite");
        let specific = crate::interactive::SubTarget(1);
        let broad = crate::interactive::SubTarget(2);
        let mut painted = PaintedWidgets::default();
        painted.record(PaintedWidget {
            id,
            rect: Rect::new(0.0, 0.0, 20.0, 2.0),
            layout: Box::new(()),
            sub_regions: vec![
                (Rect::new(8.0, 0.0, 2.0, 2.0), specific),
                (Rect::new(0.0, 0.0, 10.0, 2.0), broad),
            ],
            hover_targets: None,
            paint_state: Default::default(),
        });

        assert_eq!(painted.painted_hit(8.5, 1.0), Some((id, Some(specific))));
        assert_eq!(painted.painted_hit(1.0, 1.0), Some((id, Some(broad))));
        assert_eq!(painted.painted_hit(15.0, 1.0), Some((id, None)));
        assert_eq!(
            painted.painted_sub_center(&id, specific),
            Some((9.0, 1.0)),
            "a sub-part's center is what a driver clicks"
        );
        assert_eq!(
            painted.painted_sub_center(&WidgetId::new("other"), specific),
            None
        );
    }

    /// The right/bottom edge belongs to whatever sits next along, so a
    /// point exactly on a widget's far edge is outside it.
    #[test]
    fn painted_hit_is_half_open_on_the_far_edges() {
        let id = WidgetId::new("edges");
        let mut painted = PaintedWidgets::default();
        painted.record(PaintedWidget {
            id,
            rect: Rect::new(0.0, 0.0, 4.0, 2.0),
            layout: Box::new(()),
            sub_regions: Vec::new(),
            hover_targets: None,
            paint_state: Default::default(),
        });

        assert_eq!(painted.painted_hit(0.0, 0.0), Some((id, None)));
        assert_eq!(painted.painted_hit(4.0, 0.0), None);
        assert_eq!(painted.painted_hit(0.0, 2.0), None);
    }
}
