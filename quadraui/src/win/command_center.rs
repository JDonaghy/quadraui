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

/// Horizontal padding added on top of the real measured search-label
/// width, in DIPs — [`draw_command_center`]'s real-measurement twin of
/// [`CommandCenterMeasure::from_char_width`]'s private
/// `SEARCH_H_PAD_PX`. Kept as its own local constant rather than
/// reaching into that private const, same posture
/// `gtk::command_center`'s `GTK_SEARCH_PAD_PX` and
/// `macos::command_center`'s `SEARCH_PAD_PX` already take — pinned to
/// the identical value (`16.0` total, matching both siblings) by
/// `search_pad_and_min_width_match_the_shared_estimate_formula` below.
const SEARCH_H_PAD_PX: f32 = 16.0;
/// Minimum search-box width (DIPs) regardless of how short the real
/// measured label is — mirrors `from_char_width`'s private
/// `SEARCH_MIN_WIDTH_PX`. See [`SEARCH_H_PAD_PX`]'s doc.
const SEARCH_MIN_WIDTH_PX: f32 = 280.0;

/// Compute the Win-GUI pixel/DIP layout for a [`CommandCenter`] without
/// painting — the DirectWrite twin of [`draw_command_center`]'s internal
/// layout call. Needs only `char_width` (no live `DWrite` measurer): the
/// shared [`CommandCenterMeasure::from_char_width`] formula estimates the
/// search box from `char_width` alone, exactly like
/// `GtkBackend::command_center_layout`'s own no-paint query path (#732).
///
/// This is deliberately the cheap *estimate* used only when no live
/// `DWrite` measurer is on hand (the `WinBackend::command_center_layout`
/// trait method's "no surface attached yet" fallback, #924). Once a
/// surface is live, [`draw_command_center`] sizes the search box from
/// [`win_command_center_layout_measured`] instead — see that function's
/// doc for why the two must not be conflated (issue #1260).
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

/// Compute the Win-GUI layout for a [`CommandCenter`] from `dwrite`'s
/// real measured search-label width, rather than the plain-`char_width`
/// estimate [`win_command_center_layout`] uses. Mirrors
/// `gtk::command_center::gtk_command_center_layout` (real Pango
/// `pixel_size`) and `macos::command_center::mac_command_center_layout`
/// (real Core Text `measure_text`) — both of those size their search
/// box from a live font-layout measurement of the actual label, not an
/// average-char-width guess.
///
/// Before this existed, [`draw_command_center`] called
/// [`win_command_center_layout`] even though it already had `dwrite` on
/// hand and called [`DWrite::measure_text`] on the very same label a few
/// lines later (just to position the drawn glyphs) — the measured width
/// was computed and then discarded, never fed back into the box's own
/// size. On a real Win-GUI host, `char_width` is the *editor* font's
/// average glyph width (see `WinBackend::attach_surface`'s doc: only the
/// editor `DWrite::new` call's `char_width` is kept; the chrome-font
/// one is discarded), which can disagree sharply with how wide the
/// label actually renders in the *chrome* font `dwrite` here was built
/// from — exactly the Command Center/`current_char_width` font-mismatch
/// issue #1260's "leading hypothesis" named as the likely root cause of
/// the registered search-box zone swallowing most of the title-bar
/// band's blank strip. Measuring the real label removes that mismatch
/// entirely, the same way the GTK/macOS rasterisers already do.
fn win_command_center_layout_measured(
    dwrite: &DWrite,
    rect: Rect,
    cc: &CommandCenter,
) -> CommandCenterLayout {
    let search_box_width = if cc.search_label.is_empty() {
        0.0
    } else {
        let (text_w, _) = dwrite.measure_text(&cc.search_label).unwrap_or((0.0, 0.0));
        (text_w + SEARCH_H_PAD_PX).max(SEARCH_MIN_WIDTH_PX)
    };
    cc.layout(
        rect,
        CommandCenterMeasure {
            arrow_width: CommandCenterMeasure::ARROW_WIDTH_PX,
            gap: CommandCenterMeasure::GAP_PX,
            search_box_width,
            height: rect.height,
        },
    )
}

/// Draw a [`CommandCenter`] into `rect` (DIPs) on `target`. Returns the
/// resolved layout for host click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_command_center(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    // #1260: no longer used to size the search box (see
    // `win_command_center_layout_measured`'s doc) — kept in the
    // signature, just unused, rather than removed, since this function
    // is re-exported (`win::mod`'s `pub use command_center::{
    // draw_command_center, ..}`) and dropping a parameter would be a
    // breaking signature change for any direct caller outside this
    // crate (rule 8). `WinBackend::draw_command_center`, the only
    // in-tree caller, still has `self.current_char_width` on hand for
    // free, so it costs nothing to keep passing it.
    _char_width: f32,
    line_height: f32,
    rect: Rect,
    cc: &CommandCenter,
    theme: &Theme,
) -> CommandCenterLayout {
    let layout = win_command_center_layout_measured(dwrite, rect, cc);

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
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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

    /// No-paint *measured* layout must agree byte-for-byte with what
    /// `draw_command_center` painted — same contract every other `win::`
    /// rasteriser's `no_paint_layout_matches_paint_layout` test proves
    /// (see `win::toolbar`, `win::sidebar_panel`), updated for #1260: the
    /// paint path now sizes the search box from
    /// `win_command_center_layout_measured` (real `DWrite` measurement),
    /// not the plain-`char_width` estimate [`win_command_center_layout`]
    /// still returns — see that function's doc. Comparing against the
    /// estimate here would make this test fail by design after the fix,
    /// not catch a real regression.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let cc = sample_cc();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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
            .map(|_| win_command_center_layout_measured(&dwrite, rect, &cc))
            .expect("paint");
        let no_paint = win_command_center_layout_measured(&dwrite, rect, &cc);
        assert_eq!(painted, no_paint);
    }

    /// #1260 regression: `draw_command_center`'s returned layout (and
    /// therefore the zone `WinBackend::register_command_center_zones`
    /// registers for `nc_hit_test`) must be sized from `dwrite`'s real
    /// measured label width, not the plain-`char_width` estimate — this
    /// is the bug the issue's "leading hypothesis" named: on a real host
    /// `char_width` is the *editor* font's average glyph width, which
    /// can disagree sharply with how wide the label renders in the
    /// *chrome* font actually painted, so a search box sized from the
    /// estimate can end up a different width than the box actually
    /// drawn and hit-tested against. Pin the two measurements apart with
    /// a label whose real glyph widths are nothing like a flat average
    /// (a run of `"i"` is much narrower per-char than `"0"`, the glyph
    /// [`super::text::DWrite::new`] measures `char_width` from) so a
    /// regression back to the estimate is caught instead of silently
    /// agreeing by coincidence.
    #[test]
    fn draw_command_center_sizes_search_box_from_real_measured_width_not_char_width_estimate() {
        let cc = CommandCenter {
            search_label: "i".repeat(60),
            ..sample_cc()
        };
        let rect = Rect::new(0.0, 0.0, 900.0, H);
        let (dwrite, _, char_width) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(900, H as u32).expect("create surface");

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
            .map(|_| win_command_center_layout_measured(&dwrite, rect, &cc))
            .expect("paint");

        let estimate = win_command_center_layout(char_width, rect, &cc);

        let painted_width = painted
            .search_bounds
            .expect("non-empty search_label produces search_bounds")
            .width;
        let estimate_width = estimate
            .search_bounds
            .expect("non-empty search_label produces search_bounds")
            .width;

        assert_ne!(
            painted_width, estimate_width,
            "a run of narrow glyphs should measure differently than the flat \
             char_width estimate — if this starts passing by equality, the \
             fixture no longer exercises the real-vs-estimate gap #1260 fixed"
        );
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
