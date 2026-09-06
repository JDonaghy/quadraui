//! Host-independent paint-geometry helpers (#857).
//!
//! # Why this module exists
//!
//! `src/macos/` is gated `#[cfg(all(feature = "macos", target_os = "macos"))]`
//! (see that arm's comment in `lib.rs`) — the whole module, not per-item the
//! way `src/win/` is. So a `cfg(test)` unit test written *inside*
//! `src/macos/` only compiles and runs on `macos-latest`, the one runner in
//! the fleet. #850 and #857 red-lined CI overnight on exactly that: pure
//! rect/inset arithmetic — "which rect gets `selected_bg` behind it", "where
//! does the translucent-blue selection highlight land" — happened to live
//! behind that gate and so had zero coverage anywhere except a real Mac.
//!
//! `src/win/msg.rs` solved the equivalent problem for Windows: pull the part
//! of the platform-specific paint/bootstrap code that is plain arithmetic
//! out from behind the target gate into its own module, and unit-test it
//! where every host can run it. This module is that pattern applied to
//! macOS's paint geometry — deliberately host-independent (no
//! `cfg(target_os)`, no `objc2`/`core-graphics`/`core-text` import anywhere
//! in it) so `cargo test -p quadraui` runs its tests on this repo's ordinary
//! Linux CI leg, with no `--features macos` and no cross-target needed.
//!
//! The actual toolkit calls — `CGContextFillRect`, the real pixel-blend a
//! translucent fill produces — stay exactly where they were, in
//! `macos::text_selection`/`macos::form`, gated and real-Mac-only as before;
//! this module only owns the *computation* those calls consume. See
//! `mac_apply_selection_highlight_paints_over_the_background` in
//! `macos::backend`'s own tests for the real-pixel half that stays macOS-only
//! (alpha-compositing a translucent fill is genuine CoreGraphics behaviour,
//! not arithmetic this module can absorb).
//!
//! `macos::form`'s `toggle_group_on_item_paints_selected_bg` and
//! `segmented_control_selected_paints_selected_bg` stay macOS-only for a
//! *different* reason than the selection-highlight test above: they are not
//! alpha-compositing assertions (both fill solid `selected_bg`, no
//! translucency involved), they are real-`BitmapSurface`/CoreText pixel
//! probes — `region_has_color` scans for glyph-free pixels because Core
//! Text's own rasteriser, not this crate, decides exactly which pixels an
//! antialiased glyph touches. That is genuinely host-specific (a different
//! font rasteriser could shift the answer), so the *pixel* assertion can't
//! move. The *decision* those two tests are actually guarding — "does this
//! item get a `selected_bg` pill at all" — is not host-specific, and is
//! exactly the class of bug #808 was: the shared `native_surface_paint::
//! paint` painter, ported from the Windows copy, never called *any* fill
//! for an on-toggle/selected-segment. [`selected_item_fill`] below is that
//! decision, extracted so it runs on every host; `native_surface_paint::
//! paint`'s `ToggleGroup`/`SegmentedControl` arms call it directly instead
//! of re-deciding inline. Layered coverage, cheapest tier first:
//! [`selected_item_fill`]'s own tests (this module, every host, no
//! features) → `primitives::form::native_surface_paint::tests::
//! toggle_group_fills_selected_bg_behind_on_items_only` /
//! `segmented_control_fills_selected_bg_behind_the_selected_segment_only`
//! (full `paint()` pipeline via a `RecordingSurface` mock, needs
//! `--features {gtk,win,macos}`) → the two real-pixel `macos::form` tests
//! named above (needs a live Core Text rasteriser, `macos-latest` only).
//!
//! Every item here is `#[allow(dead_code)]`: on a `--features tui` or
//! `--features gtk,tui` build (or a bare, feature-less build), nothing calls
//! in — same shape `native_surface.rs`'s own `#[allow(dead_code)]`
//! documents, one level up the call chain, and the same reason
//! `native_surface_paint` (`primitives::form.rs`) carries the same
//! attribute.

use crate::event::Rect;
use crate::types::Color;

/// Vertical inset applied to a selected `ToggleGroup` / `SegmentedControl`
/// item's background pill, in points.
///
/// Shared with [`crate::primitives::form`]'s `native_surface_paint::paint`,
/// which is the actual call site for pixel backends (GTK/Win/macOS) — see
/// [`form_selection_pill`].
#[allow(dead_code)]
pub(crate) const FORM_PILL_INSET_Y: f32 = 2.0;

/// Shrink an item rect into the "pill" the *on* / *selected* state paints
/// its background into: full item width, inset [`FORM_PILL_INSET_Y`] at top
/// and bottom so consecutive items keep a visible gap between their
/// highlights.
///
/// Ports `macos::form::draw_form`'s pre-#808 `iy + 2.0` / `ih - 4.0` fill —
/// the one selected-state affordance the three deleted per-backend copies
/// disagreed about (macOS painted it, GTK painted nothing, Windows painted
/// `hover_bg` at full row height; #808's `fix(quadraui): restore the
/// selected-state pill the shared Form painter dropped` unified all three
/// on this shape). `macos::form`'s `toggle_group_on_item_paints_selected_bg`
/// and `segmented_control_selected_paints_selected_bg` assert this same
/// shape against real painted pixels, but only on the `macos-latest` CI
/// leg (#857) — the tests in this module's own `mod tests` cover the
/// underlying rect arithmetic everywhere else.
///
/// Degenerate rows (height <= `2 * FORM_PILL_INSET_Y`) fall back to the
/// un-inset rect rather than producing a negative height.
#[allow(dead_code)]
pub(crate) fn form_selection_pill(r: Rect) -> Rect {
    if r.height <= FORM_PILL_INSET_Y * 2.0 {
        return r;
    }
    Rect::new(
        r.x,
        r.y + FORM_PILL_INSET_Y,
        r.width,
        r.height - FORM_PILL_INSET_Y * 2.0,
    )
}

/// The widget-state → "does this item get a `selected_bg` pill"
/// decision, for one `ToggleGroup` toggle or `SegmentedControl` segment
/// whose caller has already computed `selected` (`toggle.value &&
/// !field.disabled`, or `index == selected_idx`) and translated `rect`
/// into the surface's coordinate space.
///
/// Returns `None` — no fill at all — for an unselected item, and
/// `Some((pill, selected_bg))` for a selected one, where `pill` is
/// [`form_selection_pill`]'s inset of `rect`.
///
/// This is the module doc's `#808` regression class made unit-testable:
/// `native_surface_paint::paint`'s `ToggleGroup`/`SegmentedControl` arms
/// call this directly rather than inlining `if selected { fill }`, so a
/// future edit to *this* function — the one place both field kinds
/// decide whether to paint their selection affordance — is covered on
/// every host, not only wherever a pixel-backend's own probe happens to
/// exist.
#[allow(dead_code)]
pub(crate) fn selected_item_fill(
    rect: Rect,
    selected: bool,
    selected_bg: Color,
) -> Option<(Rect, Color)> {
    selected.then(|| (form_selection_pill(rect), selected_bg))
}

/// Compute the fill rects a text-selection highlight paint should issue for
/// one selection's `(row_cell, col_start, col_end)` ranges (see
/// [`crate::text_selection::pixel_selection_ranges`]) — pure geometry, no
/// toolkit call in sight.
///
/// `region_bounds` is the selected `TextRegion`'s bounds in points;
/// `char_w`/`line_h` convert each cell-relative range back to points.
/// Mirrors `macos::text_selection::draw_selection_highlight`'s loop (the
/// call site — see that function), which now delegates the rect math here
/// and only issues the real `CGContextFillRect` call per rect returned.
///
/// Ranges with zero-or-negative width are skipped (matches the GTK/Win
/// twins' guard) — a caret with no selection, or a selection that starts
/// and ends in the same cell, paints nothing.
///
/// Computes in `f64` (matching `char_w`/`line_h`'s precision) but returns
/// [`Rect`], whose fields are `f32` — a round-trip the pre-extraction code
/// didn't have (it passed `f64` straight through to `CGContextFillRect`).
/// `draw_selection_highlight` casts back to `f64` to build the `CGRect`.
/// Immaterial at UI coordinate magnitudes (points, not device pixels at
/// extreme zoom), and intentional: matching [`Rect`]'s field type is what
/// lets every other pixel backend's caller consume this function's output
/// without its own conversion.
#[allow(dead_code)]
pub(crate) fn text_selection_highlight_rects(
    region_bounds: Rect,
    ranges: &[(u16, f32, f32)],
    char_w: f64,
    line_h: f64,
) -> Vec<Rect> {
    ranges
        .iter()
        .filter_map(|&(row_cell, col_start, col_end)| {
            let width = (col_end - col_start) as f64 * char_w;
            if width <= 0.0 {
                return None;
            }
            let x = region_bounds.x as f64 + col_start as f64 * char_w;
            let y = region_bounds.y as f64 + row_cell as f64 * line_h;
            Some(Rect::new(x as f32, y as f32, width as f32, line_h as f32))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row too short to inset falls back to the un-inset rect rather than
    /// producing a negative height (which every pixel backend would either
    /// clamp, drop, or paint as an inverted rect).
    #[test]
    fn form_selection_pill_does_not_invert_on_degenerate_rows() {
        let flat = Rect::new(10.0, 20.0, 30.0, 3.0);
        assert_eq!(form_selection_pill(flat), flat);
    }

    /// The normal case: full width kept, height shrunk by
    /// `2 * FORM_PILL_INSET_Y`, y nudged down by one inset.
    #[test]
    fn form_selection_pill_insets_top_and_bottom_by_two_points() {
        let item = Rect::new(10.0, 20.0, 30.0, 20.0);
        assert_eq!(form_selection_pill(item), Rect::new(10.0, 22.0, 30.0, 16.0));
    }

    /// A row exactly `2 * FORM_PILL_INSET_Y` tall is the boundary — still
    /// falls back rather than producing a zero-height rect that some
    /// pixel backends treat as "invisible" and others as "1px sliver",
    /// an inconsistency not worth relying on.
    #[test]
    fn form_selection_pill_falls_back_at_the_exact_boundary() {
        let boundary = Rect::new(0.0, 0.0, 10.0, FORM_PILL_INSET_Y * 2.0);
        assert_eq!(form_selection_pill(boundary), boundary);
    }

    /// `LOWORD`-style sanity check for the selection-highlight rects: one
    /// single-row range at a known column offset lands at the expected
    /// `(x, y, width, height)`, derived from `region_bounds`/`char_w`/
    /// `line_h` rather than hardcoded — this is the same computation
    /// `mac_apply_selection_highlight_paints_over_the_background`
    /// (macOS-only, real-pixel) exercises, minus the CoreGraphics call.
    #[test]
    fn text_selection_highlight_rects_places_a_single_range() {
        let region_bounds = Rect::new(5.0, 0.0, 200.0, 32.0);
        // Row 0, columns 2..8 (six columns wide).
        let ranges = [(0u16, 2.0f32, 8.0f32)];
        let rects = text_selection_highlight_rects(region_bounds, &ranges, 8.0, 16.0);
        assert_eq!(rects, vec![Rect::new(5.0 + 16.0, 0.0, 48.0, 16.0)]);
    }

    /// Each range's row maps to its own y — the highlight must be clipped
    /// to the selected rows, not flooded across the whole region (the
    /// property the macOS backend.rs test's `unselected_y` probe checks
    /// with real pixels).
    #[test]
    fn text_selection_highlight_rects_places_each_row_independently() {
        let region_bounds = Rect::new(0.0, 0.0, 200.0, 48.0);
        let ranges = [(0u16, 0.0f32, 4.0f32), (1u16, 0.0f32, 2.0f32)];
        let rects = text_selection_highlight_rects(region_bounds, &ranges, 10.0, 16.0);
        assert_eq!(
            rects,
            vec![
                Rect::new(0.0, 0.0, 40.0, 16.0),
                Rect::new(0.0, 16.0, 20.0, 16.0),
            ]
        );
    }

    /// A zero-width range (caret, no selection) produces no rect — matches
    /// the GTK/Win twins' guard, and means `apply_selection_highlight`
    /// paints nothing for an empty selection instead of a zero-width fill.
    #[test]
    fn text_selection_highlight_rects_skips_zero_width_ranges() {
        let region_bounds = Rect::new(0.0, 0.0, 200.0, 16.0);
        let ranges = [(0u16, 3.0f32, 3.0f32)];
        assert!(text_selection_highlight_rects(region_bounds, &ranges, 8.0, 16.0).is_empty());
    }

    /// Same guard for a negative-width range (a defensive case — column
    /// math should never actually produce `col_end < col_start`, but the
    /// rect computation must not panic or emit a rect with a negative
    /// dimension if it ever does).
    #[test]
    fn text_selection_highlight_rects_skips_negative_width_ranges() {
        let region_bounds = Rect::new(0.0, 0.0, 200.0, 16.0);
        let ranges = [(0u16, 5.0f32, 2.0f32)];
        assert!(text_selection_highlight_rects(region_bounds, &ranges, 8.0, 16.0).is_empty());
    }

    /// No ranges in, no rects out.
    #[test]
    fn text_selection_highlight_rects_handles_no_ranges() {
        let region_bounds = Rect::new(0.0, 0.0, 200.0, 16.0);
        assert!(text_selection_highlight_rects(region_bounds, &[], 8.0, 16.0).is_empty());
    }

    /// The #808 regression itself, host-independent: a selected item
    /// (an on `ToggleGroup` toggle, or the chosen `SegmentedControl`
    /// segment — `selected_item_fill` doesn't distinguish the two, the
    /// caller already reduced both to one `selected: bool`) must produce
    /// a fill, not silently paint nothing. `macos::form`'s
    /// `toggle_group_on_item_paints_selected_bg` /
    /// `segmented_control_selected_paints_selected_bg` assert this same
    /// decision against real painted pixels (macOS-only, see this
    /// module's doc); this is the same assertion with no rasteriser in
    /// the loop, so it runs on every host.
    #[test]
    fn selected_item_fill_paints_the_inset_pill_when_selected() {
        let rect = Rect::new(10.0, 20.0, 30.0, 20.0);
        let bg = Color::rgb(10, 20, 30);
        assert_eq!(
            selected_item_fill(rect, true, bg),
            Some((form_selection_pill(rect), bg)),
        );
    }

    /// An unselected item paints nothing at all — not a zero-size rect,
    /// not a differently-coloured rect, no fill call. This is the exact
    /// shape of the #808 bug: the shared painter, ported from the
    /// Windows copy, dropped the fill entirely for the selected case; a
    /// test asserting only "the off item doesn't get `selected_bg`"
    /// wouldn't catch a painter that fills every item unconditionally,
    /// so this asserts `None` — no call at all — rather than probing for
    /// an absence of one particular colour.
    #[test]
    fn selected_item_fill_paints_nothing_when_not_selected() {
        let rect = Rect::new(10.0, 20.0, 30.0, 20.0);
        let bg = Color::rgb(10, 20, 30);
        assert_eq!(selected_item_fill(rect, false, bg), None);
    }
}
