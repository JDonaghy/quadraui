//! macOS rasteriser for [`crate::Spinner`].
//!
//! Paints a Unicode braille animation glyph + optional label. Same
//! frame table as TUI and GTK for visual consistency.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::spinner::{Spinner, SpinnerLayout, SpinnerMeasure};
use crate::theme::Theme;
use crate::types::Color;

const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn spinner_text(spinner: &Spinner) -> String {
    let glyph = FRAMES[spinner.frame_idx % FRAMES.len()];
    if spinner.label.is_empty() {
        glyph.to_string()
    } else {
        format!("{glyph} {}", spinner.label)
    }
}

/// Compute the macOS pixel-unit layout for a [`Spinner`].
pub fn mac_spinner_layout(spinner: &Spinner, font: &CTFont, x: f64, y: f64) -> SpinnerLayout {
    let text = spinner_text(spinner);
    let (tw, th) = measure_text(font, &text);
    spinner.layout(
        x as f32,
        y as f32,
        SpinnerMeasure::new(tw.max(0.0) as f32, th.max(0.0) as f32),
    )
}

/// Draw a [`Spinner`] onto `ctx`. Returns the layout for hit-testing.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_spinner(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    spinner: &Spinner,
    theme: &Theme,
) -> SpinnerLayout {
    let layout = mac_spinner_layout(spinner, font, x, y);
    let text = spinner_text(spinner);
    let fg = spinner.accent.unwrap_or(theme.foreground);
    draw_text(ctx, font, &text, x, y, color_to_cg(fg));
    layout
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::spinner::SpinnerHit;
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 32;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_spinner() -> Spinner {
        Spinner {
            id: WidgetId::new("sp"),
            label: "Indexing".into(),
            frame_idx: 3,
            accent: None,
        }
    }

    fn paint_via_backend(spinner: &Spinner) -> (BitmapSurface, SpinnerLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_spinner(QRect::new(0.0, 0.0, W as f32, H as f32), spinner);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn layout_width_grows_with_label() {
        let bare = Spinner {
            label: String::new(),
            ..sample_spinner()
        };
        let (_, layout_bare) = paint_via_backend(&bare);
        let (_, layout_labelled) = paint_via_backend(&sample_spinner());
        assert!(
            layout_labelled.bounds.width > layout_bare.bounds.width,
            "label should widen spinner: bare={}, labelled={}",
            layout_bare.bounds.width,
            layout_labelled.bounds.width,
        );
    }

    #[test]
    fn glyph_paints_inside_bounds() {
        let spinner = sample_spinner();
        let (surface, layout) = paint_via_backend(&spinner);
        // Find at least one non-zero pixel inside layout bounds —
        // indicates the glyph was painted.
        let mut painted = false;
        for y in (layout.bounds.y as u32)..((layout.bounds.y + layout.bounds.height) as u32) {
            for x in (layout.bounds.x as u32)..((layout.bounds.x + layout.bounds.width) as u32) {
                if x >= W || y >= H {
                    continue;
                }
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) != (0, 0, 0) {
                    painted = true;
                    break;
                }
            }
            if painted {
                break;
            }
        }
        assert!(
            painted,
            "spinner glyph should produce non-bg pixels inside bounds"
        );
    }

    #[test]
    fn frame_idx_cycles_glyphs() {
        // Different frame indices should produce different glyphs.
        let s0 = Spinner {
            frame_idx: 0,
            ..sample_spinner()
        };
        let s5 = Spinner {
            frame_idx: 5,
            ..sample_spinner()
        };
        assert_ne!(
            spinner_text(&s0),
            spinner_text(&s5),
            "different frame_idx must select different braille glyphs",
        );
    }

    #[test]
    fn hit_test_inside_bounds_returns_body() {
        let spinner = sample_spinner();
        let (_, layout) = paint_via_backend(&spinner);
        let cx = layout.bounds.x + layout.bounds.width * 0.5;
        let cy = layout.bounds.y + layout.bounds.height * 0.5;
        assert_eq!(
            layout.hit_test(cx, cy, &spinner.id),
            SpinnerHit::Body(spinner.id.clone()),
        );
        assert_eq!(layout.hit_test(-1.0, -1.0, &spinner.id), SpinnerHit::Empty,);
    }
}
