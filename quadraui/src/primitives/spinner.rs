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
//! behind. Each backend's `draw_spinner` stays untouched by that token
//! set.

use crate::event::Rect;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Braille animation frames for every backend's spinner rasteriser —
/// the one table a backend indexes instead of carrying its own copy.
/// A backend picks `FRAMES[frame_idx % FRAMES.len()]` for the current
/// glyph; see [`frame_glyph`]. The TUI rasteriser and
/// `native_surface_paint` read it today; the pixel backends' own
/// `draw_spinner` copies collapse onto it as each adopts the shared
/// painter.
///
/// Gated on the union of the rasteriser features: a backend-less
/// `quadraui` build compiles the primitive types but nothing that
/// paints, so an ungated table is `dead_code` — a hard error under
/// warnings-as-errors. The gate must stay a superset of every backend
/// that paints a spinner; adding a rasteriser feature means adding it
/// here.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// `FRAMES[frame_idx % FRAMES.len()]` — the glyph a backend paints for
/// the given animation frame.
///
/// Gated with [`FRAMES`], for the same reason.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) fn frame_glyph(frame_idx: usize) -> char {
    FRAMES[frame_idx % FRAMES.len()]
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

// `paint` is the shared [`crate::paint_surface::PaintSurface`]
// implementation of spinner painting (glyph + optional label), the
// single rasteriser behind GTK/macOS/Win's three independent (but
// visually identical) `draw_spinner` copies. Those backends still
// carry their own paint path today — adopting this one is a follow-on
// issue — but this module is where a future adopter's `draw_spinner`
// forwards to, the same way `button`/`progress` already route their
// backends through `native_surface_paint::paint`.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{frame_glyph, Spinner, SpinnerLayout, SpinnerMeasure};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// The exact text a backend measures and paints for `spinner`: the
    /// current animation glyph alone, or `"{glyph} {label}"` when
    /// [`Spinner::label`] is non-empty. Every pixel backend built this
    /// string independently (`spinner_text`/`frame_text`, byte-for-byte
    /// identical) before routing through this shared painter.
    fn spinner_text(spinner: &Spinner) -> String {
        let glyph = frame_glyph(spinner.frame_idx);
        if spinner.label.is_empty() {
            glyph.to_string()
        } else {
            format!("{glyph} {}", spinner.label)
        }
    }

    /// Paint `spinner` at `(x, y)` on `surface`, returning
    /// [`SpinnerLayout`] for host hit-testing — same contract as every
    /// per-backend `draw_spinner`: no fixed width/height, the glyph +
    /// label text's own measured size is the spinner's bounds.
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

        let fg = spinner.accent.unwrap_or(theme.foreground);
        surface.surface_draw_text_run(Rect::new(x, y, w.max(0.0), h.max(0.0)), &text, fg);

        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
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
