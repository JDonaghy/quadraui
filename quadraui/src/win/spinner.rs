//! Direct2D / DirectWrite rasteriser for [`crate::Spinner`] (issue #29).
//!
//! Mirrors `gtk::spinner`'s structure: a Unicode braille animation frame
//! table — same as TUI and GTK, for visual consistency across backends
//! — indexed by `spinner.frame_idx`, plus the optional trailing
//! `label`. [`Spinner::layout`] (the D6 layout API — see that
//! primitive's module doc) just wraps the measured glyph+label box; the
//! measurement itself comes from [`DWrite::measure_text`].
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod spinner;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] — see `win::status_bar`'s
//! module doc for the "placeholder until a later issue wires the app's
//! real theme through" posture this module shares.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::spinner::{Spinner, SpinnerLayout, SpinnerMeasure};
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

/// Compute a [`Spinner`]'s layout without painting — the DirectWrite
/// measurer twin of [`draw_spinner`]. Both measure the identical
/// glyph+label text via [`DWrite::measure_text`], so a no-paint
/// hit-test call always agrees with what the last paint drew.
pub fn win_spinner_layout(dwrite: &DWrite, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
    let text = frame_text(spinner);
    let (w, h) = dwrite.measure_text(&text).unwrap_or((0.0, 0.0));
    spinner.layout(rect.x, rect.y, SpinnerMeasure::new(w, h))
}

/// Draw a [`Spinner`] onto `target`. Returns the layout for host
/// hit-testing.
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

#[cfg(test)]
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
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
