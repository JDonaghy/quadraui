//! Proof that `PaintSurface` is genuinely public API.
//!
//! This file is a separate compilation unit, so anything it can name
//! and call is real public API, not an accidentally-`pub(crate)` item.
//!
//! Three things are asserted, one per backend family:
//!
//! 1. A free function generic over `&mut dyn quadraui::PaintSurface`
//!    type-checks here at all.
//! 2. [`quadraui::Backend::paint_surface`] answers `Some` on every pixel
//!    backend compiled in (GTK, Win-GUI), and the trait object it hands
//!    back dispatches to the same backend instance `Backend`'s own methods
//!    see (`surface_viewport` agrees with `Backend::viewport`).
//! 3. `Backend::paint_surface` answers `None` on `TuiBackend` — the
//!    structural absence `PaintSurface`'s own module doc names ("Why TUI
//!    stays out"), not a missing override.
//!
//! Calling an actual paint verb (`surface_fill_rect` et al.) needs a live
//! Cairo/Direct2D render target wired up through each backend's own
//! `attach_surface`-equivalent machinery — that round trip is covered by
//! `src/gtk/backend.rs` and `src/win/backend.rs` tests. This file's job is
//! narrower: prove the trait and its accessor are public.

#[cfg(any(feature = "tui", feature = "gtk", feature = "win"))]
use quadraui::Backend;
#[cfg(any(feature = "gtk", feature = "win"))]
use quadraui::{PaintSurface, Viewport};

/// Code outside this crate can write a function generic over
/// `&mut dyn PaintSurface`, the same shape every in-tree
/// `primitives::*::native_surface_paint::paint` helper uses.
/// `surface_viewport` is read-only and safe to call on any
/// backend in any state (no live Cairo/Direct2D context required).
#[cfg(any(feature = "gtk", feature = "win"))]
fn read_viewport_through_the_seam(surface: &mut dyn PaintSurface) -> Viewport {
    surface.surface_viewport()
}

#[cfg(feature = "tui")]
#[test]
fn tui_backend_has_no_paint_surface_by_structural_design() {
    use quadraui::tui::backend::TuiBackend;

    let mut backend = TuiBackend::new();
    assert!(
        backend.paint_surface().is_none(),
        "TuiBackend must answer None from Backend::paint_surface — a cell grid has no \
         sub-cell Rect to paint into (PaintSurface's module doc, \"Why TUI stays out\"); \
         see tests/conformance/caps.rs's ACCEPTED_DEFAULTS entry for the same claim enforced \
         against silent regressions"
    );
}

#[cfg(feature = "gtk")]
#[test]
fn gtk_backend_paint_surface_is_public_and_reachable() {
    use quadraui::gtk::backend::GtkBackend;

    let mut backend = GtkBackend::new();
    let expected = Backend::viewport(&backend);
    let surface = backend
        .paint_surface()
        .expect("GtkBackend implements PaintSurface directly, so this must be Some");
    assert_eq!(
        read_viewport_through_the_seam(surface),
        expected,
        "the PaintSurface trait object Backend::paint_surface hands back must be the same \
         GtkBackend instance Backend's own viewport() reads from"
    );
}

#[cfg(feature = "win")]
#[test]
fn win_backend_paint_surface_is_public_and_reachable() {
    use quadraui::win::backend::WinBackend;

    let mut backend = WinBackend::new();
    let expected = Backend::viewport(&backend);
    let surface = backend
        .paint_surface()
        .expect("WinBackend implements PaintSurface directly, so this must be Some");
    assert_eq!(
        read_viewport_through_the_seam(surface),
        expected,
        "the PaintSurface trait object Backend::paint_surface hands back must be the same \
         WinBackend instance Backend's own viewport() reads from"
    );
}

/// An out-of-crate `PaintSurface` that implements only the required
/// verbs, recording every styled text run it is asked to draw.
mod out_of_crate_text_line {
    use quadraui::backend::ImagePaintResult;
    use quadraui::{Color, FontRole, Image, PaintSurface, Point, Rect, TextRunStyle, Viewport};

    const CHAR_W: f32 = 8.0;

    #[derive(Default)]
    struct Recorder {
        runs: Vec<(Rect, String, Color, bool, bool)>,
    }

    impl PaintSurface for Recorder {
        fn surface_begin_frame(&mut self, _v: Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> Viewport {
            Viewport::new(100.0, 100.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            16.0
        }
        fn surface_char_width(&self) -> f32 {
            CHAR_W
        }
        fn surface_measure_text(&self, t: &str) -> (f32, f32) {
            (t.chars().count() as f32 * CHAR_W, 16.0)
        }
        fn surface_fill_rect(&mut self, _r: Rect, _c: Color) {}
        fn surface_fill_rounded_rect(&mut self, _r: Rect, _radius: f32, _c: Color) {}
        fn surface_stroke_rect(&mut self, _r: Rect, _c: Color, _w: f32) {}
        fn surface_draw_text_run(&mut self, r: Rect, t: &str, c: Color) {
            self.runs.push((r, t.to_string(), c, false, false));
        }
        #[allow(clippy::too_many_arguments)]
        fn surface_draw_text_run_styled(
            &mut self,
            r: Rect,
            t: &str,
            c: Color,
            bold: bool,
            italic: bool,
            _underline: bool,
            _scale_x: f32,
        ) {
            self.runs.push((r, t.to_string(), c, bold, italic));
        }
        fn surface_draw_line(&mut self, _from: Point, _to: Point, _c: Color, _w: f32) {}
        fn surface_push_clip(&mut self, _r: Rect) {}
        fn surface_pop_clip(&mut self) {}
        fn surface_draw_image(&mut self, _r: Rect, _i: &Image) -> ImagePaintResult {
            ImagePaintResult::Unsupported
        }
    }

    #[test]
    fn the_text_line_defaults_place_runs_on_a_monospace_grid() {
        let mut s = Recorder::default();
        // "日" is two cells wide; "e\u{301}" is one cell.
        let text = "a日e\u{301}b";
        assert_eq!(
            s.surface_text_x_at_index(text, &[], FontRole::Editor, 0),
            0.0
        );
        assert_eq!(
            s.surface_text_x_at_index(text, &[], FontRole::Editor, 1),
            CHAR_W
        );
        assert_eq!(
            s.surface_text_x_at_index(text, &[], FontRole::Editor, 4),
            3.0 * CHAR_W
        );
        // An index inside the wide glyph snaps back to its start.
        assert_eq!(
            s.surface_text_x_at_index(text, &[], FontRole::Editor, 2),
            CHAR_W
        );

        // Nearest boundary, never between `e` and its combining mark.
        assert_eq!(
            s.surface_text_index_at_x(text, &[], FontRole::Editor, 9.0),
            1
        );
        assert_eq!(
            s.surface_text_index_at_x(text, &[], FontRole::Editor, 26.0),
            4
        );
        assert_eq!(
            s.surface_text_index_at_x(text, &[], FontRole::Editor, 31.0),
            7
        );
        assert_eq!(
            s.surface_text_index_at_x(text, &[], FontRole::Editor, 99.0),
            8
        );

        let red = Color::rgb(200, 0, 0);
        let fg = Color::rgb(220, 220, 220);
        s.surface_draw_text_line(
            Point::new(10.0, 4.0),
            text,
            &[
                TextRunStyle::new(0..1, fg),
                TextRunStyle::new(1..4, red).with_bold(true),
                TextRunStyle::new(7..8, fg).with_italic(true),
            ],
            FontRole::Editor,
        );
        let placed: Vec<(f32, &str, bool, bool)> = s
            .runs
            .iter()
            .map(|(r, t, _, b, i)| (r.x, t.as_str(), *b, *i))
            .collect();
        assert_eq!(
            placed,
            vec![
                (10.0, "a", false, false),
                (10.0 + CHAR_W, "日", true, false),
                (10.0 + 4.0 * CHAR_W, "b", false, true),
            ],
            "the uncovered combining cluster is left unpainted"
        );
    }
}
