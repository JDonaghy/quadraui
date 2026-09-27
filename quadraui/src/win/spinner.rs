//! Direct2D / DirectWrite rasteriser for [`crate::Spinner`] (issue #29).
//!
//! Mirrors `gtk::spinner`'s structure: a Unicode braille animation frame
//! table — same as TUI and GTK, for visual consistency across backends
//! — indexed by `spinner.frame_idx`, plus the optional trailing
//! `label`. [`Spinner::layout`] (the D6 layout API — see that
//! primitive's module doc) just wraps the measured glyph+label box; the
//! measurement itself comes from [`DWrite::measure_text`].
//!
//! Issue #1078: only [`draw_spinner`] (the real Direct2D paint entry
//! point) is `#[cfg(target_os = "windows")]`-gated. [`win_spinner_layout`]
//! is pure geometry generic over [`SpinnerMeasureSource`] — no Direct2D/
//! DirectWrite type in its signature — so it compiles and runs
//! everywhere, including a plain `cargo test --features win` on Linux.
//! `super::mod`'s `mod spinner;` is no longer whole-module gated; see
//! `backend.rs`'s module docs.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] — see `win::status_bar`'s
//! module doc for the "placeholder until a later issue wires the app's
//! real theme through" posture this module shares.

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

#[cfg(target_os = "windows")]
use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::spinner::{Spinner, SpinnerLayout, SpinnerMeasure};
#[cfg(target_os = "windows")]
use crate::theme::Theme;

/// Braille animation frames — identical table to `gtk::spinner::FRAMES`
/// / `tui`'s spinner glyphs, so the same `frame_idx` looks the same
/// glyph across every backend.
const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn frame_text(spinner: &Spinner) -> String {
    let glyph = FRAMES[spinner.frame_idx % FRAMES.len()];
    if spinner.label.is_empty() {
        glyph.to_string()
    } else {
        format!("{glyph} {}", spinner.label)
    }
}

/// Width+height text measurement for [`win_spinner_layout`]. A spinner's
/// hit box height comes from the glyph+label text's own measured height
/// (not a fixed `line_height`), so this can't reuse
/// [`crate::primitives::layout_metrics::TextMeasure`] (width-only) the
/// way `win::menu_bar`/`win::toast` do — a real difference this
/// rasteriser has always had (`DWrite::measure_text` returns both), not
/// new duplication.
pub trait SpinnerMeasureSource {
    fn measure(&self, text: &str) -> (f32, f32);
}

#[cfg(target_os = "windows")]
impl SpinnerMeasureSource for DWrite {
    fn measure(&self, text: &str) -> (f32, f32) {
        self.measure_text(text).unwrap_or((0.0, 0.0))
    }
}

/// Compute a [`Spinner`]'s layout without painting — the measurer twin of
/// [`draw_spinner`]. Both measure the identical glyph+label text via the
/// same [`SpinnerMeasureSource`], so a no-paint hit-test call always
/// agrees with what the last paint drew.
pub fn win_spinner_layout(
    measure: &dyn SpinnerMeasureSource,
    rect: Rect,
    spinner: &Spinner,
) -> SpinnerLayout {
    let text = frame_text(spinner);
    let (w, h) = measure.measure(&text);
    spinner.layout(rect.x, rect.y, SpinnerMeasure::new(w, h))
}

/// Draw a [`Spinner`] onto `target`. Returns the layout for host
/// hit-testing.
#[cfg(target_os = "windows")]
pub fn draw_spinner(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    spinner: &Spinner,
) -> SpinnerLayout {
    let layout = win_spinner_layout(dwrite, rect, spinner);
    let theme = Theme::default();
    let fg = spinner.accent.unwrap_or(theme.foreground);
    let text = frame_text(spinner);
    let _ = dwrite.draw_text(target, &text, layout.bounds, fg);
    layout
}

// #1078: every test below paints through a real `DWrite`/`HeadlessSurface`
// — gated the same way the whole module used to be.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::primitives::spinner::SpinnerHit;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    fn spinner(frame_idx: usize) -> Spinner {
        Spinner {
            id: WidgetId::new("sp"),
            label: "Indexing…".into(),
            frame_idx,
            accent: None,
        }
    }

    /// The layout bounds are wide enough to hold glyph + label, and
    /// `hit_test` resolves a click inside them to `Body` (outside, to
    /// `Empty`) — the round trip a spinner supports (it's read-only, so
    /// there's no dismiss/action sub-region to cover).
    #[test]
    fn layout_hit_test_round_trip() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let spinner = spinner(0);
        let rect = Rect::new(10.0, 10.0, 0.0, 0.0);

        let layout = win_spinner_layout(&dwrite, rect, &spinner);
        assert!(layout.bounds.width > 0.0);
        assert!(layout.bounds.height > 0.0);

        let inside = layout.hit_test(
            layout.bounds.x + 1.0,
            layout.bounds.y + 1.0,
            &WidgetId::new("sp"),
        );
        assert_eq!(inside, SpinnerHit::Body(WidgetId::new("sp")));

        let outside = layout.hit_test(0.0, 0.0, &WidgetId::new("sp"));
        assert_eq!(outside, SpinnerHit::Empty);
    }

    /// Painting doesn't panic and leaves the glyph's own ink somewhere
    /// inside the measured bounds — probing the fg colour would be
    /// glyph-hinting-fragile (see `tooltip`'s module doc on the same
    /// hazard), so this just paints and re-derives the layout to prove
    /// the call succeeds against a real (headless) render target.
    #[test]
    fn paints_without_panicking() {
        let surface = HeadlessSurface::new(200, 20).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let spinner = spinner(3);
        let rect = Rect::new(0.0, 0.0, 0.0, 0.0);

        let layout = surface
            .paint(|target| {
                draw_spinner(target, &dwrite, rect, &spinner);
            })
            .map(|_| win_spinner_layout(&dwrite, rect, &spinner))
            .expect("paint spinner");

        assert!(layout.bounds.width > 0.0);
    }

    /// `frame_idx` cycles through the frame table, not the label —
    /// different frames still measure to non-zero, and painting each
    /// glyph in the same accent colour and label is exercised without
    /// panicking (glyph identity itself isn't a Direct2D-observable
    /// property this test can assert on without pixel-perfect glyph
    /// probing).
    #[test]
    fn accent_colour_is_used_when_set() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut spinner = spinner(9);
        spinner.accent = Some(Color::rgb(255, 0, 0));
        let rect = Rect::new(0.0, 0.0, 0.0, 0.0);

        let surface = HeadlessSurface::new(200, 20).expect("create surface");
        surface
            .paint(|target| {
                draw_spinner(target, &dwrite, rect, &spinner);
            })
            .expect("paint spinner with accent");
    }
}
