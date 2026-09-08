//! GTK rasteriser + layout helper for [`crate::TextDisplay`].
//!
//! Field-kind *painting* moved to the shared
//! [`crate::primitives::text_display::paint`] (#810, NativeSurface Phase
//! 2c) — see that fn's doc for the divergence (Windows's bold-span
//! support, dropped since `NativeSurface` has no weight parameter)
//! resolved while unifying `gtk::text_display::draw_text_display`,
//! `macos::text_display::draw_text_display` and
//! `win::text_display::draw_text_display` into one implementation. This
//! module's own [`draw_text_display`] is now a thin wrapper over that
//! shared `paint`, via [`super::form::RawFormSurface`] — kept (not
//! deleted, unlike the `macos`/`win` twins) because `kubeui-gtk`'s
//! `paint()` calls it directly on a raw `(&Context, &pango::Layout)`
//! pair with no live [`super::backend::GtkBackend`] on hand, the same
//! shape [`crate::gtk::multi_section_view`]'s embedded-`Chart` section
//! body uses `RawFormSurface` for. [`gtk_text_display_layout`] is the
//! pixel-unit layout query used for both hit-testing
//! (`GtkBackend::text_display_layout`) and painting
//! (`GtkBackend::draw_text_display`, via `Backend::text_display_layout`).

use crate::primitives::text_display::{TextDisplay, TextDisplayLayout, TextDisplayLineMeasure};
use crate::theme::Theme;

/// Draw a [`TextDisplay`] into `(x, y, w, h)` on `cr`.
///
/// Thin wrapper over [`crate::primitives::text_display::paint`] via
/// [`super::form::RawFormSurface`] — see this module's doc for why it's
/// kept rather than deleted like the `macos`/`win` equivalents. `theme`
/// is threaded through unchanged from the pre-#810 signature.
///
/// `char_width` is the approximate average character width (pixels) —
/// see [`gtk_text_display_layout`]'s doc for why paint and layout must
/// use the same value (quadraui#905).
#[allow(clippy::too_many_arguments)]
pub fn draw_text_display(
    cr: &gtk4::cairo::Context,
    layout: &gtk4::pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    display: &TextDisplay,
    theme: &Theme,
    line_height: f64,
    char_width: f64,
) {
    let rect = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    let mut surface = super::form::RawFormSurface { cr, layout };
    crate::primitives::text_display::paint(
        display,
        rect,
        &mut surface,
        theme,
        line_height as f32,
        char_width as f32,
    );
}

/// Compute the text-display layout using GTK-native metrics (pixel
/// line_height, 12px scrollbar gutter, 8px min thumb). Consumers call
/// this to drive hit-testing for scrollbar drag interaction without
/// re-deriving metrics. Runs outside the frame scope (no cairo/pango
/// handles needed).
///
/// `char_width` is the backend's approximate average character width
/// (`Backend::char_width`) — see quadraui#905: since this pure function
/// has no live Pango layout to measure real glyph widths with, it
/// approximates the wrap column budget from pixels the same way
/// [`draw_text_display`] does, so a click always resolves against the
/// same row boundaries that were actually painted.
pub fn gtk_text_display_layout(
    display: &TextDisplay,
    rect: crate::event::Rect,
    line_height: f64,
    char_width: f64,
) -> TextDisplayLayout {
    let body_h = if display.title.is_some() {
        (rect.height as f64 - line_height).max(0.0)
    } else {
        rect.height as f64
    };
    if body_h <= 0.0 {
        return display.layout(0.0, 0.0, |_| {
            TextDisplayLineMeasure::new(line_height as f32)
        });
    }
    let gutter_px = 12.0_f32;
    let body_width_px = if display.show_scrollbar {
        (rect.width - gutter_px).max(0.0)
    } else {
        rect.width
    };
    let col_budget = crate::primitives::text_display::px_to_cols(body_width_px, char_width as f32);
    let measure = |i: usize| {
        let rows = crate::primitives::text_display::wrap_row_count(&display.lines[i], col_budget);
        TextDisplayLineMeasure::new(rows as f32 * line_height as f32)
    };
    if display.show_scrollbar {
        display.layout_with_scrollbar(rect.width, body_h as f32, gutter_px, 8.0, measure)
    } else {
        display.layout(rect.width, body_h as f32, measure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::text_display::{TextDisplayHit, TextDisplayLine};
    use crate::types::{Decoration, StyledSpan, WidgetId};

    fn display(show_scrollbar: bool) -> TextDisplay {
        TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..20)
                .map(|i| TextDisplayLine {
                    spans: vec![StyledSpan::plain(format!("line{i}"))],
                    decoration: Decoration::Normal,
                    timestamp: None,
                })
                .collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar,
        }
    }

    /// `text_display_layout` is documented **LOCAL** (issue #505):
    /// `gtk_text_display_layout` never reads `rect.x`/`rect.y`, so the
    /// returned `scrollbar_bounds` / `hit_test` results must be
    /// identical regardless of where `rect` is positioned — the
    /// opposite regression from the ABSOLUTE methods (a LOCAL method
    /// that accidentally starts folding in the origin), but the same
    /// "must agree across origins" contract.
    fn scrollbar_thumb_round_trip_at(x: f32, y: f32) {
        let rect = crate::event::Rect::new(x, y, 20.0, 5.0);
        let d = display(true);
        let layout = gtk_text_display_layout(&d, rect, 1.0, 1.0);

        let thumb = layout.thumb_bounds.expect("thumb bounds present");
        // LOCAL frame: thumb bounds must not have absorbed rect.x/rect.y.
        assert!(
            thumb.x < rect.width,
            "thumb.x={} must stay inside the local frame",
            thumb.x
        );

        let hit = layout.hit_test(thumb.x + 0.5, thumb.y + 0.5);
        assert_eq!(hit, TextDisplayHit::ScrollbarThumb);
    }

    #[test]
    fn scrollbar_thumb_paint_and_click_round_trip() {
        scrollbar_thumb_round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn scrollbar_thumb_paint_and_click_round_trip_at_nonzero_origin() {
        scrollbar_thumb_round_trip_at(7.0, 13.0);
    }

    // ── quadraui#905: wrap column budget from pixels ───────────────────

    #[test]
    fn wrap_reports_multi_row_height_for_a_long_line() {
        let mut d = display(false);
        d.lines = vec![TextDisplayLine {
            spans: vec![StyledSpan::plain(
                "the quick brown fox jumps over the lazy dog",
            )],
            decoration: Decoration::Normal,
            timestamp: None,
        }];
        // 80px wide at an 8px average char width is a 10-cell budget —
        // far narrower than the 44-char line, so it must wrap onto more
        // than one row's worth of height.
        let rect = crate::event::Rect::new(0.0, 0.0, 80.0, 100.0);
        let layout = gtk_text_display_layout(&d, rect, 16.0, 8.0);
        let vis = layout.visible_lines.first().expect("line 0 visible");
        assert!(
            vis.bounds.height > 16.0,
            "expected wrapped height > one line_height, got {}",
            vis.bounds.height
        );
    }

    #[test]
    fn draw_and_layout_agree_on_wrapped_row_count() {
        // Paint (`draw_text_display`, via the shared `paint`) and the
        // pure hit-testing layout must compute the identical wrapped
        // height for the same inputs — see `gtk_text_display_layout`'s
        // doc on why (quadraui#494/#905).
        let mut d = display(false);
        d.lines = vec![TextDisplayLine {
            spans: vec![StyledSpan::plain(
                "the quick brown fox jumps over the lazy dog",
            )],
            decoration: Decoration::Normal,
            timestamp: None,
        }];
        // Tall enough that the wrapped line's full height isn't clipped
        // by the viewport (`TextDisplay::layout` clamps a row's bounds to
        // whatever vertical space remains) — this test is about the wrap
        // row-count math agreeing, not about viewport clipping.
        let rect = crate::event::Rect::new(0.0, 0.0, 80.0, 300.0);
        let from_layout = gtk_text_display_layout(&d, rect, 16.0, 8.0).visible_lines[0]
            .bounds
            .height;

        let col_budget = crate::primitives::text_display::px_to_cols(80.0, 8.0);
        let from_paint_math =
            crate::primitives::text_display::wrap_row_count(&d.lines[0], col_budget) as f32 * 16.0;
        assert_eq!(from_layout, from_paint_math);
    }
}
