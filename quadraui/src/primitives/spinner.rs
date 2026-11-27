//! `Spinner` primitive: an indeterminate activity indicator. Used for
//! ongoing operations whose duration isn't known ahead of time —
//! LSP boot, git clone, Mason installs, project search.
//!
//! The spinner carries a `frame_idx` (app-incremented per tick) and a
//! label. Backends pick the glyph from their own frame table using
//! `frame_idx % frame_count`. TUI commonly uses braille frames
//! (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`), GUI backends may use a rotating arc or native
//! equivalent.
//!
//! # Backend contract
//!
//! **Declarative snapshot.** The app advances `frame_idx` on its own
//! animation ticker (~100 ms for braille, faster for arcs). The
//! primitive has no built-in timer — it's a paint description only.
//! Backends rasterise the current glyph + label.
//!
//! # No [`crate::Style`] tokens
//!
//! `Toast`/`Tooltip`/`ContextMenu`/`Palette`/`Dialog` each read
//! [`crate::Style::corner_radius`]/`border_width`/`shadow_elevation`,
//! and `ProgressBar`'s track reads `corner_radius`. `Spinner` has no
//! bordered box at all — every backend paints a bare glyph-plus-label,
//! matching VS Code's own activity spinner (a rotating icon, no card)
//! — so there is no rectangle for a radius to round or a shadow to sit
//! behind. The spinner painters stay untouched by that token set.

use crate::event::Rect;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Braille animation frames for every backend's spinner rasteriser —
/// the one table a backend indexes instead of carrying its own copy.
/// A backend picks `FRAMES[frame_idx % FRAMES.len()]` for the current
/// glyph; see [`frame_glyph`].
///
/// The animation lives in the shared painters, not in a backend: both
/// [`cell_paint`] and [`native_surface_paint`] index this table through
/// [`frame_glyph`], and so does the widget's own
/// [`crate::primitive::Primitive::layout`], which must agree with them
/// on how wide the glyph-plus-label text is.
pub(crate) const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// `FRAMES[frame_idx % FRAMES.len()]` — the glyph painted for the given
/// animation frame.
pub(crate) fn frame_glyph(frame_idx: usize) -> char {
    FRAMES[frame_idx % FRAMES.len()]
}

/// The exact text a pixel surface measures and paints for `spinner`:
/// the current animation glyph alone, or `"{glyph} {label}"` when
/// [`Spinner::label`] is non-empty. The one shared builder every pixel
/// measurer and [`native_surface_paint::paint`] both call, instead of
/// each carrying its own byte-for-byte-identical copy.
///
/// The cell painter puts the glyph and the label in separate cells
/// (see [`cell_paint`]) and measures in whole columns instead, so it
/// uses [`Spinner::inline_cols`] rather than this.
pub(crate) fn spinner_text(spinner: &Spinner) -> String {
    let glyph = frame_glyph(spinner.frame_idx);
    if spinner.label.is_empty() {
        glyph.to_string()
    } else {
        format!("{glyph} {}", spinner.label)
    }
}

/// Declarative description of an indeterminate spinner.
///
/// # Examples
///
/// ```
/// use quadraui::{Spinner, SpinnerHit, SpinnerMeasure, WidgetId};
///
/// let spinner = Spinner {
///     id: WidgetId::new("spinner:lsp_boot"),
///     label: "Indexing…".to_string(),
///     frame_idx: 3,
///     accent: None,
/// };
///
/// let layout = spinner.layout(0.0, 0.0, SpinnerMeasure::new(80.0, 1.0));
/// assert_eq!(layout.hit_test(5.0, 0.0, &spinner.id), SpinnerHit::Body(spinner.id.clone()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Spinner {
    pub id: WidgetId,
    /// Label shown next to the spinner, e.g. "Indexing…". Empty =
    /// glyph only.
    #[serde(default)]
    pub label: String,
    /// Animation frame index. Apps increment this on their ticker;
    /// backends render `frame_idx % frame_count` of their own frame
    /// table.
    #[serde(default)]
    pub frame_idx: usize,
    /// Optional accent colour for the glyph. `None` = backend decides.
    #[serde(default)]
    pub accent: Option<Color>,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Measurement for a `Spinner`. Backends report the inline width of
/// the glyph + label at current font metrics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinnerMeasure {
    pub width: f32,
    pub height: f32,
}

impl SpinnerMeasure {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// Classification of a hit-test result. Spinners are not interactive —
/// only `Body` / `Empty`. `Body` is returned so apps can still use a
/// spinner click as "focus the originating operation" if they want.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpinnerHit {
    Body(WidgetId),
    Empty,
}

/// Fully-resolved spinner layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinnerLayout {
    pub bounds: Rect,
}

impl SpinnerLayout {
    pub fn hit_test(&self, x: f32, y: f32, id: &WidgetId) -> SpinnerHit {
        if x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height
        {
            SpinnerHit::Body(*id)
        } else {
            SpinnerHit::Empty
        }
    }
}

impl Spinner {
    /// Compute the spinner's bounds given backend measurement.
    ///
    /// # Arguments
    ///
    /// - `origin_x`, `origin_y` — top-left position in the parent
    ///   surface's coordinate space.
    /// - `measure` — width/height for the current `label` at font
    ///   metrics (TUI: glyph + space + label chars; GTK: Pango).
    pub fn layout(&self, origin_x: f32, origin_y: f32, measure: SpinnerMeasure) -> SpinnerLayout {
        SpinnerLayout {
            bounds: Rect::new(origin_x, origin_y, measure.width, measure.height),
        }
    }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────
//
// `Spinner` reaches the frame type-erased as
// [`crate::frame::Surface::Widget`]: `layout` sizes the glyph-plus-label
// run in the host's units, `paint` runs the shared pixel painter in
// [`native_surface_paint`], and `paint_cells` runs the cell painter in
// [`cell_paint`]. The animation is a pure function of
// [`Spinner::frame_idx`] in both painters — the host advances that field
// on its own ticker, so neither painter keeps frame state of its own.
//
// A spinner is purely decorative: it reacts to no input and has no
// affordance, so it is [`crate::non_interactive!`].

impl Spinner {
    /// How many terminal columns the glyph-plus-label run occupies: one
    /// for the glyph, plus a separator column and the label's own
    /// display width when [`Self::label`] is non-empty.
    ///
    /// The cell-unit counterpart of measuring [`spinner_text`] — the
    /// label's display width, not its `char` count, so a double-width
    /// glyph in the label is accounted for the way the cell painter
    /// writes it.
    pub(crate) fn inline_cols(&self) -> usize {
        if self.label.is_empty() {
            1
        } else {
            2 + crate::text_util::display_width(&self.label)
        }
    }
}

/// Resolve `spinner`'s layout for a cell grid: a single row, as wide as
/// [`Spinner::inline_cols`] but never wider than `rect`.
///
/// Both extents clamp to `rect`, so a spinner given a zero-height area
/// resolves zero-height bounds and the cell painter draws nothing —
/// a glyph row in an area with no rows is worse than no spinner.
///
/// Bounds are in the absolute frame `rect` is expressed in, matching
/// what the pixel path reports, so a
/// [`SpinnerLayout::hit_test`] against either takes the same
/// coordinates the host painted at.
pub(crate) fn cell_spinner_layout(spinner: &Spinner, rect: Rect) -> SpinnerLayout {
    let cols = (spinner.inline_cols() as f32).min(rect.width.max(0.0));
    let rows = rect.height.clamp(0.0, 1.0);
    spinner.layout(rect.x, rect.y, SpinnerMeasure::new(cols, rows))
}

impl crate::primitive::Primitive for Spinner {
    type Layout = SpinnerLayout;
    type Event = SpinnerHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Spinner(self.clone())
    }

    /// The glyph-plus-label run's own width, one row tall — a spinner
    /// is an inline indicator, so a container sizes it to its content
    /// on both axes rather than stretching it.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        match ctx.units() {
            crate::primitive::Units::Cells => {
                crate::NaturalSize::new(Some(self.inline_cols() as f32), Some(1.0))
            }
            _ => crate::NaturalSize::new(
                Some(ctx.text_width(&spinner_text(self), self.font_role())),
                Some(ctx.line_height),
            ),
        }
    }

    /// On a cell grid, [`cell_spinner_layout`]. On a pixel surface, the
    /// glyph-plus-label text measured in the backend's own chrome font
    /// — the same text [`native_surface_paint::paint`] draws.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> SpinnerLayout {
        match ctx.units() {
            crate::primitive::Units::Cells => cell_spinner_layout(self, rect),
            _ => {
                let width = ctx.text_width(&spinner_text(self), self.font_role());
                self.layout(
                    rect.x,
                    rect.y,
                    SpinnerMeasure::new(width.max(0.0), ctx.line_height),
                )
            }
        }
    }

    /// The shared pixel painter, against the already-resolved `layout`.
    fn paint(
        &self,
        layout: &SpinnerLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        native_surface_paint::paint_resolved(self, layout, surface, theme);
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme);
    }

    /// [`cell_paint::paint`] into the layout's bounds.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &SpinnerLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        cell_paint::paint(self, layout, cells, env.theme());
    }
}

crate::non_interactive!(Spinner);

/// The cell-grid painter for [`Spinner`], written against
/// [`crate::CellSurface`].
///
/// One row: [`frame_glyph`] in the leading column, then the label from
/// the third column on (one separator column between them), clipped to
/// `layout.bounds`.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{frame_glyph, Spinner, SpinnerLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;

    /// An opaque single-cell style — every glyph this painter writes
    /// sets both colours and no modifier.
    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `spinner` into `layout.bounds` on `surface`.
    ///
    /// `layout` must be the value [`super::cell_spinner_layout`]
    /// returned for the same spinner and rect, so paint and
    /// [`SpinnerLayout::hit_test`] can never disagree. A layout with no
    /// painted row (zero width or height) is a no-op.
    pub(crate) fn paint(
        spinner: &Spinner,
        layout: &SpinnerLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        if layout.bounds.width < 1.0 || layout.bounds.height < 1.0 {
            return;
        }

        let x = layout.bounds.x.round() as u16;
        let y = layout.bounds.y.round() as u16;
        let end = x + layout.bounds.width.round() as u16;

        let glyph_fg = spinner.accent.unwrap_or(theme.accent_fg);
        let bg = theme.background;
        surface.put_char(x, y, frame_glyph(spinner.frame_idx), solid(glyph_fg, bg));

        if !spinner.label.is_empty() {
            let label_x = x + 2;
            let max_cols = end.saturating_sub(label_x);
            surface.put_str_clipped(
                label_x,
                y,
                &spinner.label,
                max_cols,
                solid(theme.foreground, bg),
            );
        }
    }
}

// `paint` is the shared [`crate::paint_surface::PaintSurface`]
// implementation of spinner painting (glyph + optional label) every
// pixel backend reaches through `Spinner`'s `Primitive::paint`.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{spinner_text, Spinner, SpinnerLayout, SpinnerMeasure};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint `spinner` at `(x, y)` on `surface`, returning
    /// [`SpinnerLayout`] for host hit-testing: no fixed width/height, the
    /// glyph + label text's own measured size is the spinner's bounds.
    pub(crate) fn paint(
        spinner: &Spinner,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        x: f32,
        y: f32,
    ) -> SpinnerLayout {
        let text = spinner_text(spinner);
        let (w, h) = surface.surface_measure_text(&text);
        let layout = spinner.layout(x, y, SpinnerMeasure::new(w.max(0.0), h.max(0.0)));
        paint_resolved(spinner, &layout, surface, theme);
        layout
    }

    /// The same paint as [`paint`], against an already-resolved
    /// `layout` — what [`crate::primitive::Primitive::paint`] calls,
    /// since the widget contract hands a painter the layout rather than
    /// an origin.
    ///
    /// `layout.bounds` is the glyph-plus-label run's own rect, origin
    /// included, so this needs no separate `x`/`y`.
    pub(crate) fn paint_resolved(
        spinner: &Spinner,
        layout: &SpinnerLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        let fg = spinner.accent.unwrap_or(theme.foreground);
        surface.surface_draw_text_run(layout.bounds, &spinner_text(spinner), fg);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Rect, Viewport};
        use crate::primitives::spinner::SpinnerHit;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues — mirrors
        /// `primitives::progress::native_surface_paint`'s own
        /// `RecordingSurface`.
        #[derive(Default)]
        struct RecordingSurface {
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 20.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn spinner(label: &str, frame_idx: usize) -> Spinner {
            Spinner {
                id: WidgetId::new("sp"),
                label: label.into(),
                frame_idx,
                accent: None,
            }
        }

        #[test]
        fn glyph_and_label_paint_as_one_text_run() {
            let s = spinner("Indexing", 3);
            let mut surface = RecordingSurface::default();
            paint(&s, &mut surface, &Theme::default(), 0.0, 0.0);
            assert_eq!(surface.text_runs.len(), 1);
            let (_, text, _) = &surface.text_runs[0];
            assert_eq!(text, &format!("{} Indexing", super::super::frame_glyph(3)));
        }

        #[test]
        fn empty_label_paints_the_glyph_alone() {
            let s = spinner("", 0);
            let mut surface = RecordingSurface::default();
            paint(&s, &mut surface, &Theme::default(), 0.0, 0.0);
            let (_, text, _) = &surface.text_runs[0];
            assert_eq!(text, &super::super::frame_glyph(0).to_string());
        }

        #[test]
        fn accent_overrides_theme_foreground() {
            let mut s = spinner("Loading", 0);
            let accent = Color::rgb(200, 10, 10);
            s.accent = Some(accent);
            let mut surface = RecordingSurface::default();
            paint(&s, &mut surface, &Theme::default(), 0.0, 0.0);
            let (_, _, color) = &surface.text_runs[0];
            assert_eq!(*color, accent);
        }

        #[test]
        fn no_accent_uses_theme_foreground() {
            let s = spinner("Loading", 0);
            let theme = Theme {
                foreground: Color::rgb(1, 2, 3),
                ..Theme::default()
            };
            let mut surface = RecordingSurface::default();
            paint(&s, &mut surface, &theme, 0.0, 0.0);
            let (_, _, color) = &surface.text_runs[0];
            assert_eq!(*color, theme.foreground);
        }

        /// `paint`'s returned layout bakes `x`/`y` straight into
        /// `bounds` — the same absolute-frame contract every
        /// per-backend `*_spinner_layout` twin already upholds (see
        /// `macos::spinner`'s non-zero-origin regression test).
        #[test]
        fn layout_bounds_use_the_given_origin() {
            let s = spinner("Indexing", 0);
            let mut surface = RecordingSurface::default();
            let layout = paint(&s, &mut surface, &Theme::default(), 7.0, 13.0);
            assert_eq!(layout.bounds.x, 7.0);
            assert_eq!(layout.bounds.y, 13.0);
            let cx = layout.bounds.x + layout.bounds.width / 2.0;
            let cy = layout.bounds.y + layout.bounds.height / 2.0;
            assert_eq!(layout.hit_test(cx, cy, &s.id), SpinnerHit::Body(s.id));
        }

        #[test]
        fn different_frames_paint_different_glyphs() {
            let s0 = spinner("", 0);
            let s5 = spinner("", 5);
            let mut surface0 = RecordingSurface::default();
            let mut surface5 = RecordingSurface::default();
            paint(&s0, &mut surface0, &Theme::default(), 0.0, 0.0);
            paint(&s5, &mut surface5, &Theme::default(), 0.0, 0.0);
            assert_ne!(surface0.text_runs[0].1, surface5.text_runs[0].1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spinner + ProgressBar primitive tests

    #[test]
    fn spinner_layout_bounds() {
        let s = Spinner {
            id: WidgetId::new("installing"),
            label: "Installing rust-analyzer…".to_string(),
            frame_idx: 42,
            accent: None,
        };
        let layout = s.layout(10.0, 20.0, SpinnerMeasure::new(200.0, 16.0));
        assert_eq!(layout.bounds.x, 10.0);
        assert_eq!(layout.bounds.y, 20.0);
        assert_eq!(layout.bounds.width, 200.0);
        // Hit-test on bounds returns Body(id).
        match layout.hit_test(100.0, 28.0, &s.id) {
            SpinnerHit::Body(id) => assert_eq!(id.as_str(), "installing"),
            _ => panic!(),
        }
        assert_eq!(layout.hit_test(500.0, 28.0, &s.id), SpinnerHit::Empty);
    }
}

/// Cell-grid paint and hit-test round trips through the widget's own
/// cell painter onto a plain buffer.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use crate::primitives::spinner::{cell_paint, cell_spinner_layout, Spinner, SpinnerLayout};
    use crate::theme::Theme;
    use crate::tui::BufferCellSurface;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// The cell layout for `area`, painted onto `buf` through the
    /// widget's own cell painter.
    fn draw_spinner(
        buf: &mut Buffer,
        area: Rect,
        spinner: &Spinner,
        theme: &Theme,
    ) -> SpinnerLayout {
        let layout = cell_spinner_layout(
            spinner,
            crate::event::Rect::new(
                area.x as f32,
                area.y as f32,
                area.width as f32,
                area.height as f32,
            ),
        );
        cell_paint::paint(spinner, &layout, &mut BufferCellSurface::new(buf), theme);
        layout
    }
    use crate::primitives::spinner::SpinnerHit;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_spinner(label: &str, frame: usize) -> Spinner {
        Spinner {
            id: WidgetId::new("spin"),
            label: label.into(),
            frame_idx: frame,
            accent: None,
        }
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // The cell layout bakes `area.x`/`area.y`
    // straight into `spinner.layout`'s returned bounds (absolute
    // frame), so the regression guard is a full paint+hit_test round
    // trip re-run at a non-zero origin.
    fn glyph_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 30, 1);
        let mut buf = Buffer::empty(area);
        let spinner = mk_spinner("Loading", 0);
        let layout = draw_spinner(&mut buf, area, &spinner, &Theme::default());

        assert_eq!(cell_char(&buf, origin_x, origin_y), '⠋');
        assert_eq!(cell_char(&buf, origin_x + 2, origin_y), 'L');

        let hit = layout.hit_test(origin_x as f32 + 0.5, origin_y as f32 + 0.5, &spinner.id);
        assert_eq!(hit, SpinnerHit::Body(WidgetId::new("spin")));
    }

    #[test]
    fn glyph_paint_and_hit_round_trip() {
        glyph_round_trip_at(0, 0);
    }

    /// The same glyph paint + hit_test round trip must hold when the
    /// spinner isn't painted at the screen origin.
    #[test]
    fn glyph_paint_and_hit_round_trip_at_nonzero_origin() {
        glyph_round_trip_at(7, 13);
    }

    #[test]
    fn frame_advances_glyph() {
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        let spinner = mk_spinner("", 3);
        draw_spinner(&mut buf, area, &spinner, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), '⠸');
    }

    #[test]
    fn outside_returns_empty() {
        let area = Rect::new(0, 0, 30, 1);
        let mut buf = Buffer::empty(area);
        let spinner = mk_spinner("Test", 0);
        let layout = draw_spinner(&mut buf, area, &spinner, &Theme::default());
        assert_eq!(layout.hit_test(50.0, 0.5, &spinner.id), SpinnerHit::Empty);
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let spinner = mk_spinner("Test", 0);
        let _layout = draw_spinner(&mut buf, Rect::new(0, 0, 0, 0), &spinner, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    /// An area with columns but no rows has nowhere to put the glyph
    /// row, so it paints nothing — the height half of the no-op above,
    /// which a width-only clamp would have let through.
    #[test]
    fn zero_height_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let spinner = mk_spinner("Test", 0);
        let layout = draw_spinner(
            &mut buf,
            Rect::new(0, 0, 10, 0),
            &spinner,
            &Theme::default(),
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(layout.bounds.height, 0.0);
    }
}
