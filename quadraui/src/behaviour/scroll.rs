//! Scroll-into-view: the minimum offset change that brings a target row
//! (or, in the pixel variant, a target pixel span) inside a viewport.
//!
//! Every controller that keeps a selected/focused item visible —
//! `TreeNavCore`, `FormController`, `PathPicker`, `DualModePalette`,
//! `Palette`, `IconGrid`, `CellGrid`, `TextInput` — calls into
//! [`ensure_visible`]/[`ensure_visible_px`] instead of re-deriving this
//! arithmetic, so the "scroll the minimum amount to bring the target back
//! into the viewport" rule has exactly one implementation.

/// Scroll `offset` by the minimum amount needed to bring row `target`
/// inside a `viewport`-row-tall window, in row units.
///
/// - `target` above the current window (`target < offset`) scrolls up so
///   `target` becomes the first visible row.
/// - `target` below the current window (`target >= offset + viewport`)
///   scrolls down so `target` becomes the last visible row.
/// - `target` already inside the window, or `viewport == 0` (nothing to
///   measure against), leaves `offset` unchanged.
///
/// Pure function: returns the new offset, does not mutate anything.
pub fn ensure_visible(offset: usize, viewport: usize, target: usize) -> usize {
    if viewport == 0 {
        return offset;
    }
    if target < offset {
        target
    } else if target >= offset + viewport {
        // `target >= offset + viewport` and `offset` is a `usize` (>= 0)
        // together mean `target >= viewport`, so `target + 1 - viewport`
        // never underflows.
        target + 1 - viewport
    } else {
        offset
    }
}

/// Pixel-precise counterpart of [`ensure_visible`]: scroll `offset` by the
/// minimum amount needed to bring the pixel span `[item_start, item_start +
/// item_len)` inside a `viewport`-pixel-tall window.
///
/// Mirrors [`ensure_visible`]'s three cases in pixel units instead of row
/// units: above the window scrolls up so the item's start pixel becomes
/// the window's top edge; below the window (the item's *end* pixel falls
/// past the window's bottom edge) scrolls down so the item's end pixel
/// becomes the window's bottom edge; already inside, or a non-positive
/// `viewport`, leaves `offset` unchanged.
pub fn ensure_visible_px(offset: f32, viewport: f32, item_start: f32, item_len: f32) -> f32 {
    if viewport <= 0.0 {
        return offset;
    }
    let item_end = item_start + item_len;
    if item_start < offset {
        item_start
    } else if item_end > offset + viewport {
        item_end - viewport
    } else {
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ensure_visible ───────────────────────────────────────────────

    #[test]
    fn target_above_window_scrolls_up_to_it() {
        assert_eq!(ensure_visible(10, 5, 3), 3);
    }

    #[test]
    fn target_below_window_scrolls_down_so_it_is_last_visible() {
        // viewport 5, window [10, 15); target 20 -> new offset 16 so the
        // window becomes [16, 21), with 20 as the last visible row.
        assert_eq!(ensure_visible(10, 5, 20), 16);
    }

    #[test]
    fn target_inside_window_is_unchanged() {
        assert_eq!(ensure_visible(10, 5, 12), 10);
    }

    #[test]
    fn target_exactly_at_edges_is_unchanged() {
        assert_eq!(ensure_visible(10, 5, 10), 10);
        assert_eq!(ensure_visible(10, 5, 14), 10);
    }

    #[test]
    fn zero_viewport_is_a_no_op() {
        assert_eq!(ensure_visible(10, 0, 999), 10);
    }

    #[test]
    fn target_zero_scrolls_to_zero() {
        assert_eq!(ensure_visible(10, 5, 0), 0);
    }

    // ── ensure_visible_px ────────────────────────────────────────────

    #[test]
    fn px_item_above_window_scrolls_up_to_its_start() {
        assert_eq!(ensure_visible_px(100.0, 50.0, 40.0, 20.0), 40.0);
    }

    #[test]
    fn px_item_below_window_scrolls_down_to_its_end() {
        // window [100, 150); item [160, 180) -> new offset 130 so the
        // window becomes [130, 180), with 180 as the bottom edge.
        assert_eq!(ensure_visible_px(100.0, 50.0, 160.0, 20.0), 130.0);
    }

    #[test]
    fn px_item_inside_window_is_unchanged() {
        assert_eq!(ensure_visible_px(100.0, 50.0, 110.0, 20.0), 100.0);
    }

    #[test]
    fn px_item_taller_than_viewport_aligns_to_its_end() {
        // An item taller than the viewport, starting inside the current
        // window but extending past its bottom edge: the end-alignment
        // branch wins since the start pixel alone is already visible.
        assert_eq!(ensure_visible_px(0.0, 10.0, 5.0, 50.0), 45.0);
    }

    #[test]
    fn px_zero_viewport_is_a_no_op() {
        assert_eq!(ensure_visible_px(10.0, 0.0, 999.0, 5.0), 10.0);
    }
}
