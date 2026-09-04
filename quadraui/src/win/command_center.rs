//! Direct2D / DirectWrite rasteriser for [`crate::CommandCenter`] (#732).
//!
//! Mirrors `gtk::command_center` / `macos::command_center`'s structure:
//! back/forward arrows (`◀` / `▶`) and a bordered search box, centred
//! within the given area. [`CommandCenter::layout`] (the D6 layout API)
//! does every positioning decision via the shared
//! [`CommandCenterMeasure::from_char_width`] formula (#732) — this
//! module only measures (from a plain `char_width` average, not a
//! per-glyph DirectWrite layout — see that constructor's doc for why)
//! and paints (`ID2D1RenderTarget::FillRectangle` / `DrawRectangle` /
//! `DrawText`).
//!
//! ## Scope note — straight-rectangle search box border
//!
//! No rounded-rect helper exists in `win::text` beyond
//! [`super::text::stroke_rect`] (which insets a *plain* rectangle, not a
//! rounded one — see its doc), so the search box border paints as a
//! straight rectangle rather than GTK's rounded-rect pill. Same posture
//! `win::toolbar` takes for its hover/pressed highlight, and matches
//! `macos::command_center`'s own straight-rectangle border (see that
//! module's doc). Hit-test bounds and click routing are unaffected —
//! only the corner treatment differs.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod command_center;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout, CommandCenterMeasure};
use crate::theme::Theme;

/// Compute the Win-GUI pixel/DIP layout for a [`CommandCenter`] without
/// painting — the DirectWrite twin of [`draw_command_center`]'s internal
/// layout call. Needs only `char_width` (no live `DWrite` measurer): the
/// shared [`CommandCenterMeasure::from_char_width`] formula estimates the
/// search box from `char_width` alone, exactly like
/// `GtkBackend::command_center_layout`'s own no-paint query path (#732).
///
/// Coordinate frame: **ABSOLUTE** (`rect.x`/`rect.y` baked into every
/// bounds field), matching [`crate::Backend::command_center_layout`]'s
/// documented contract and the GTK/TUI/macOS twins.
pub fn win_command_center_layout(
    char_width: f32,
    rect: Rect,
    cc: &CommandCenter,
) -> CommandCenterLayout {
    cc.layout(
        rect,
        CommandCenterMeasure::from_char_width(&cc.search_label, char_width, rect.height),
    )
}

/// Draw a [`CommandCenter`] into `rect` (DIPs) on `target`. Returns the
/// resolved layout for host click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_command_center(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    char_width: f32,
    line_height: f32,
    rect: Rect,
    cc: &CommandCenter,
    theme: &Theme,
) -> CommandCenterLayout {
    let layout = win_command_center_layout(char_width, rect, cc);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    let _ = fill_rect(target, rect, theme.tab_bar_bg);

    let enabled_fg = theme.tab_inactive_fg;
    let disabled_fg = theme.muted_fg;
    let text_y = rect.y + (rect.height - line_height) / 2.0;

    if let Some(bb) = layout.back_bounds {
        let fg = if cc.back_enabled {
            enabled_fg
        } else {
            disabled_fg
        };
        let (tw, th) = dwrite.measure_text("◀").unwrap_or((0.0, 0.0));
        let tx = bb.x + (bb.width - tw) / 2.0;
        let _ = dwrite.draw_text(target, "◀", Rect::new(tx, text_y, tw, th), fg);
    }

    if let Some(fb) = layout.forward_bounds {
        let fg = if cc.forward_enabled {
            enabled_fg
        } else {
            disabled_fg
        };
        let (tw, th) = dwrite.measure_text("▶").unwrap_or((0.0, 0.0));
        let tx = fb.x + (fb.width - tw) / 2.0;
        let _ = dwrite.draw_text(target, "▶", Rect::new(tx, text_y, tw, th), fg);
    }

    if let Some(sb) = layout.search_bounds {
        let border = Rect::new(sb.x, sb.y + 2.0, sb.width, (sb.height - 4.0).max(0.0));
        let _ = stroke_rect(target, border, theme.separator, 1.0);

        let (tw, th) = dwrite.measure_text(&cc.search_label).unwrap_or((0.0, 0.0));
        let _ = dwrite.draw_text(
            target,
            &cc.search_label,
            Rect::new(sb.x + 8.0, text_y, tw, th),
            theme.tab_inactive_fg,
        );
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::command_center::CommandCenterHit;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 480.0;
    const H: f32 = 32.0;

    fn sample_cc() -> CommandCenter {
        CommandCenter {
            id: WidgetId::new("cc"),
            back_enabled: true,
            forward_enabled: false,
            search_label: "project".into(),
        }
    }

    fn paint_via_backend_at(cc: &CommandCenter, x: f32, y: f32) -> CommandCenterLayout {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(x, y, W - x, H - y);

        surface
            .paint(|target| {
                draw_command_center(
                    target,
                    &dwrite,
                    char_width,
                    16.0,
                    rect,
                    cc,
                    &Theme::default(),
                );
            })
            .map(|_| win_command_center_layout(char_width, rect, cc))
            .expect("paint command center")
    }

    /// C0 smoke: `draw_command_center` must actually paint + return a
    /// click-routable layout rather than panicking or hitting a
    /// `todo!()` (#732's acceptance bar — "draw_command_center survives
    /// C0 on win").
    #[test]
    fn round_trip_click_hits_back_and_search_box() {
        let cc = sample_cc();
        let layout = paint_via_backend_at(&cc, 0.0, 0.0);

        let back = layout.back_bounds.expect("back bounds present");
        assert_eq!(
            layout.hit_test(back.x + 1.0, back.y + 1.0),
            CommandCenterHit::Back
        );

        let search = layout.search_bounds.expect("search bounds present");
        assert_eq!(
            layout.hit_test(search.x + 1.0, search.y + 1.0),
            CommandCenterHit::SearchBox
        );
    }

    /// Non-zero-origin regression guard (issue #494/#505 — LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends") — mirrors `gtk::command_center`'s and
    /// `mac_command_center_layout`'s own non-zero-origin tests.
    #[test]
    fn round_trip_click_hits_back_at_nonzero_origin() {
        let cc = sample_cc();
        let layout = paint_via_backend_at(&cc, 7.0, 13.0);

        let back = layout.back_bounds.expect("back bounds present");
        assert!(
            back.x >= 7.0,
            "back.x={} must not fall left of the strip's own origin",
            back.x
        );
        assert_eq!(
            layout.hit_test(back.x + 1.0, back.y + 1.0),
            CommandCenterHit::Back
        );
    }

    #[test]
    fn empty_search_label_omits_search_bounds() {
        let cc = CommandCenter {
            search_label: "".into(),
            ..sample_cc()
        };
        let layout = paint_via_backend_at(&cc, 0.0, 0.0);
        assert!(layout.search_bounds.is_none());
        assert!(layout.back_bounds.is_some());
    }

    /// No-paint layout must agree byte-for-byte with what
    /// `draw_command_center` painted — same contract every other `win::`
    /// rasteriser's `no_paint_layout_matches_paint_layout` test proves
    /// (see `win::toolbar`, `win::sidebar_panel`).
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let cc = sample_cc();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_command_center(
                    target,
                    &dwrite,
                    char_width,
                    16.0,
                    rect,
                    &cc,
                    &Theme::default(),
                );
            })
            .map(|_| win_command_center_layout(char_width, rect, &cc))
            .expect("paint");
        let no_paint = win_command_center_layout(char_width, rect, &cc);
        assert_eq!(painted, no_paint);
    }

    /// #732 acceptance bar: `win_command_center_layout` must delegate to
    /// the shared [`CommandCenterMeasure::from_char_width`] formula
    /// rather than re-deriving the search-box width itself — the same
    /// delegation `gtk::backend::GtkBackend::command_center_layout`
    /// proves on its own side (see
    /// `gtk::backend::tests::command_center_layout_delegates_to_shared_char_width_formula`).
    /// Because both call through the identical constructor, this and
    /// that test together establish that gtk and win compute the same
    /// command-center layout for the same char width, even though the
    /// two can't run in one process on this Linux host (this whole
    /// module is `target_os = "windows"`-gated).
    #[test]
    fn win_command_center_layout_delegates_to_shared_char_width_formula() {
        let cc = CommandCenter {
            id: WidgetId::new("cc"),
            back_enabled: true,
            forward_enabled: true,
            search_label: "project-name".into(),
        };
        let char_width = 9.0_f32;
        let rect = Rect::new(3.0, 5.0, 400.0, 24.0);

        let layout = win_command_center_layout(char_width, rect, &cc);

        let expected =
            CommandCenterMeasure::from_char_width(&cc.search_label, char_width, rect.height);
        let search = layout
            .search_bounds
            .expect("non-empty search_label produces search_bounds");
        assert_eq!(search.width, expected.search_box_width);
    }
}
