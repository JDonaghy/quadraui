//! Row windowing: a list, tree, table or text display whose descriptor
//! carries only a slice of a much longer collection.
//!
//! A descriptor sets `items_window: Some(`[`ItemsWindow`]`)` to say its
//! rows are the loaded slice `[start, start + len)` of `total` real rows.
//! Every index it reports or accepts (`scroll_offset`, the selection, the
//! rows in its events) is then absolute — an index into the real
//! collection, not into the loaded slice.
//!
//! The widget's own `layout` paints such a descriptor through a *band*: a
//! clone holding only the rows one viewport (plus [`BAND_PAD_ROWS`]) shows,
//! with an empty placeholder for every band position the loaded slice does
//! not cover. Allocation is therefore bounded by the viewport, never by
//! `total` — a wire payload claiming ten billion rows costs a few dozen
//! placeholder rows, not an allocation failure.
//!
//! When the visible range moves outside the loaded slice, the widget's
//! [`crate::interactive::Interactive::data_requested`] answers a
//! [`crate::UiEvent::RowsRequested`] asking the app for the rows around
//! it; [`RowsRequestLatch`] keeps it from asking twice for the same range.
//! The widget asks only after input, never while painting: the app sends
//! the rows its first viewport shows (and the rows a viewport that grows
//! on resize shows) without being asked, or those rows paint as
//! placeholders until the next input.
//!
//! [`crate::CellGrid`] windows its rows the same way under other names:
//! its descriptor's `loaded_row_start` and `row_count` are this module's
//! `items_window.start` and `items_window.total`, and its
//! `CellGridEvent::CellsRequested { row_start, rows, .. }` (routed as
//! `UiEvent::CellGrid`) is [`crate::UiEvent::RowsRequested`]'s
//! `{ start, count }` plus a column range.
//!
//! [`HostLatch`] is the uncontrolled-by-default rule the four row widgets
//! (and `TabBar`) resolve their scroll, selection and active-tab state by:
//! the descriptor's value wins only when it changed since the widget last
//! saw it, so an app that never echoes a move still sees the move, and an
//! app that does echo it sees its own value.

use serde::{Deserialize, Serialize};

use crate::types::WidgetId;

/// A windowed collection: the descriptor's rows/items/lines are the loaded
/// slice starting at absolute index `start` of `total` real rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ItemsWindow {
    /// The absolute index of the descriptor's first loaded row.
    pub start: usize,
    /// How many rows the real collection has.
    pub total: usize,
}

impl ItemsWindow {
    /// A window of `total` rows whose loaded slice starts at `start`.
    pub fn new(start: usize, total: usize) -> Self {
        Self { start, total }
    }

    /// The real extent: `total`, widened to cover a loaded slice a stale
    /// total understates, so a bad total never truncates loaded rows.
    pub fn row_count(self, loaded_len: usize) -> usize {
        self.total.max(self.start.saturating_add(loaded_len))
    }

    /// The position in the loaded slice of absolute row `abs`, or `None`
    /// when that row is not loaded.
    pub fn loaded_index(self, loaded_len: usize, abs: usize) -> Option<usize> {
        abs.checked_sub(self.start).filter(|i| *i < loaded_len)
    }
}

/// Rows a band materialises beyond its viewport's measured capacity — a
/// margin so a partially visible last row or a sub-row pixel shift always
/// has a row to land on.
pub const BAND_PAD_ROWS: usize = 8;

/// How many rows a band painted into a `height`-tall rect needs: the rows
/// `height` holds at `line_height` each, rounded up, plus
/// [`BAND_PAD_ROWS`].
pub fn band_rows_for(height: f32, line_height: f32) -> usize {
    let rows = if line_height > 0.0 {
        (height.max(0.0) / line_height).ceil() as usize
    } else {
        0
    };
    rows.saturating_add(BAND_PAD_ROWS)
}

/// The contiguous slice of a windowed collection one frame materialises:
/// `len` rows from absolute index `start`, the row painted at the top of
/// the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Band {
    pub start: usize,
    pub len: usize,
}

impl Band {
    /// `abs` rebased onto the band, or `None` when it is outside it.
    pub fn relative(self, abs: usize) -> Option<usize> {
        abs.checked_sub(self.start).filter(|rel| *rel < self.len)
    }

    /// The absolute index of band row `rel`.
    pub fn absolute(self, rel: usize) -> usize {
        self.start.saturating_add(rel)
    }
}

/// The `band_rows`-wide band that paints `window` scrolled to absolute
/// row `scroll`, which is clamped into the real extent the same way every
/// row widget clamps its own `scroll_offset`.
pub fn band(window: ItemsWindow, loaded_len: usize, scroll: usize, band_rows: usize) -> Band {
    let real_total = window.row_count(loaded_len);
    let start = scroll.min(real_total.saturating_sub(1));
    let len = band_rows.max(1).min(real_total.saturating_sub(start));
    Band { start, len }
}

/// The absolute scroll row and the sub-row pixel remainder a windowed
/// descriptor paints at: `scroll_px` (whole rows plus remainder) when set,
/// else `descriptor_offset` with no remainder.
pub fn split_scroll_px(
    descriptor_offset: usize,
    scroll_px: Option<f32>,
    line_height: f32,
) -> (usize, Option<f32>) {
    match scroll_px {
        Some(px) if line_height > 0.0 => {
            let rows = (px / line_height).floor().max(0.0);
            (rows as usize, Some(px - rows * line_height))
        }
        other => (descriptor_offset, other),
    }
}

/// `band` filled from `loaded` — the payload covering `[window.start,
/// window.start + loaded.len())` — with `placeholder(abs)` at every band
/// position the payload has no row for. Exactly `band.len` long.
pub fn materialise<T: Clone>(
    loaded: &[T],
    window: ItemsWindow,
    band: Band,
    mut placeholder: impl FnMut(usize) -> T,
) -> Vec<T> {
    (0..band.len)
        .map(|i| {
            let abs = band.absolute(i);
            match window
                .loaded_index(loaded.len(), abs)
                .and_then(|j| loaded.get(j))
            {
                Some(row) => row.clone(),
                None => placeholder(abs),
            }
        })
        .collect()
}

/// `idx` rebased from an absolute index onto `band`, or `None` when it is
/// not inside the band.
pub fn band_relative(idx: Option<usize>, band: Band) -> Option<usize> {
    band.relative(idx?)
}

/// The `(start, count)` a windowed collection needs loaded next, or `None`
/// when the loaded slice already covers the visible range `[scroll, scroll
/// + viewport_rows)`.
///
/// The request covers the visible range plus one viewport of margin on
/// each side, clamped to `[0, window.total)` — bounded by a small multiple
/// of `viewport_rows`, never by `total`. A clamp that leaves nothing to
/// ask for (a scroll past the end) answers `None`, never a zero count.
pub fn rows_requested(
    window: ItemsWindow,
    loaded_len: usize,
    scroll: usize,
    viewport_rows: usize,
) -> Option<(usize, usize)> {
    let visible_start = scroll.min(window.total);
    let visible_end = visible_start
        .saturating_add(viewport_rows)
        .min(window.total);
    let loaded_end = window.start.saturating_add(loaded_len);
    if window.start <= visible_start && loaded_end >= visible_end {
        return None;
    }
    let margin = viewport_rows.max(1);
    let start = visible_start.saturating_sub(margin);
    let end = visible_end.saturating_add(margin).min(window.total);
    let count = end.saturating_sub(start);
    (count > 0).then_some((start, count))
}

/// A value the widget owns unless the descriptor changes it: the
/// descriptor wins only when it differs from the descriptor value the
/// widget saw when it last moved, otherwise the widget's own value wins.
///
/// An app that never echoes a move keeps sending the same descriptor
/// value, which keeps matching [`Self::seen`], so the widget's move
/// stands. An app that echoes it sends a changed value, which wins — and
/// equals the widget's value anyway. An app that sets a new value of its
/// own (a programmatic jump) sends a changed value too, which wins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostLatch<T> {
    seen: Option<T>,
    value: Option<T>,
}

impl<T: Clone + PartialEq> HostLatch<T> {
    /// The resolved value against this frame's `descriptor` value.
    pub fn get(&self, descriptor: &T) -> T {
        match (&self.seen, &self.value) {
            (Some(seen), Some(value)) if seen == descriptor => value.clone(),
            _ => descriptor.clone(),
        }
    }

    /// Record a move to `value`, made while the descriptor said
    /// `descriptor`.
    pub fn set(&mut self, descriptor: &T, value: T) {
        self.seen = Some(descriptor.clone());
        self.value = Some(value);
    }

    /// Rewrite both the remembered descriptor value and the widget's own
    /// value through `f` — for a renumbering both sides undergo at once
    /// (a closed tab shifting every later tab's index down by one).
    pub fn map(&mut self, f: impl Fn(&T) -> T) {
        self.seen = self.seen.as_ref().map(&f);
        self.value = self.value.as_ref().map(&f);
    }

    /// The descriptor value the last move was made against, if any.
    pub fn seen(&self) -> Option<&T> {
        self.seen.as_ref()
    }

    /// Whether the widget holds a value of its own.
    pub fn is_set(&self) -> bool {
        self.value.is_some()
    }
}

impl HostLatch<usize> {
    /// Scroll `row` — a selection the app just moved — into a viewport of
    /// `viewport_rows`, for a scroll the widget owns. The scroll is the
    /// app's when its descriptor value changed since the widget last saw
    /// it, or, before the widget has seen one, when it is non-zero; that
    /// scroll stands as sent. Returns whether the scroll moved.
    pub fn follow_selection(
        &mut self,
        descriptor: usize,
        viewport_rows: usize,
        row: usize,
    ) -> bool {
        let app_scrolls = match self.seen() {
            Some(seen) => *seen != descriptor,
            None => descriptor != 0,
        };
        if app_scrolls {
            return false;
        }
        let scroll = self.get(&descriptor);
        let follow = crate::behaviour::scroll::ensure_visible(scroll, viewport_rows, row);
        if follow == scroll {
            return false;
        }
        self.set(&descriptor, follow);
        true
    }
}

/// Per-widget repeat suppression for [`crate::UiEvent::RowsRequested`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowsRequestLatch {
    last: Option<(ItemsWindow, usize, usize, usize)>,
}

impl RowsRequestLatch {
    /// The `RowsRequested` widget `id` should emit, or `None` when the
    /// loaded slice covers the visible range or this exact request
    /// (window, loaded length, start, count) was already made. A covered
    /// range clears the latch, so the next miss asks again.
    pub fn request(
        &mut self,
        id: WidgetId,
        window: ItemsWindow,
        loaded_len: usize,
        scroll: usize,
        viewport_rows: usize,
    ) -> Option<crate::UiEvent> {
        let Some((start, count)) = rows_requested(window, loaded_len, scroll, viewport_rows) else {
            self.last = None;
            return None;
        };
        let key = (window, loaded_len, start, count);
        if self.last == Some(key) {
            return None;
        }
        self.last = Some(key);
        Some(crate::UiEvent::RowsRequested { id, start, count })
    }
}

/// What a windowed widget's layout records besides its band-relative inner
/// layout: the band it painted, the collection's real extent, and the band
/// descriptor itself, which its paint draws.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowedLayout<W> {
    /// The band this layout materialised.
    pub band: Band,
    /// The real extent ([`ItemsWindow::row_count`]).
    pub total: usize,
    /// The loaded slice, `(start, len)`.
    pub loaded: (usize, usize),
    /// The absolute scroll row the band paints from.
    pub scroll: usize,
    /// How many rows the viewport holds at a zero offset — the page size
    /// and the visible range a request is computed against.
    pub viewport_rows: usize,
    /// The sub-row pixel remainder the band paints at.
    pub scroll_px: Option<f32>,
    /// The band descriptor: the band's rows, scroll offset `0`, the
    /// selection rebased onto the band, no window, its own scrollbar off.
    pub live: Box<W>,
}

impl<W> WindowedLayout<W> {
    /// The absolute index of band row `rel`.
    pub fn to_abs(&self, rel: usize) -> usize {
        self.band.absolute(rel)
    }

    /// The position in the descriptor's loaded slice of absolute row
    /// `abs`, or `None` when that row is a placeholder.
    pub fn loaded_index(&self, abs: usize) -> Option<usize> {
        abs.checked_sub(self.loaded.0)
            .filter(|i| *i < self.loaded.1)
    }

    /// The window this layout was resolved against.
    pub fn window(&self) -> ItemsWindow {
        ItemsWindow::new(self.loaded.0, self.total)
    }
}

/// A vertical scrollbar for a windowed collection, sized from the real
/// extent rather than the band: `total` rows scrolled to absolute row
/// `scroll` (plus `scroll_px` of a row), `viewport_rows` visible, along
/// `track`. `None` when every row fits.
pub fn extent_scrollbar(
    id: WidgetId,
    track: crate::Rect,
    total: usize,
    scroll: usize,
    scroll_px: Option<f32>,
    viewport_rows: usize,
    row_height: f32,
) -> Option<crate::Scrollbar> {
    if total <= viewport_rows || track.height <= 0.0 || track.width <= 0.0 {
        return None;
    }
    let frac = match scroll_px {
        Some(px) if row_height > 0.0 => px / row_height,
        _ => 0.0,
    };
    Some(crate::Scrollbar::vertical(
        id,
        track,
        scroll as f32 + frac,
        total as f32,
        viewport_rows as f32,
        row_height.max(1.0).min(track.height),
    ))
}

/// The paint state a band paints with: the caller's, with
/// [`crate::primitive::PaintState::scroll_px`] replaced by `scroll_px` and
/// the hovered/pressed row rebased from absolute onto the band.
pub fn band_paint_state(
    state: &dyn crate::primitive::PaintState,
    band: Band,
    scroll_px: Option<f32>,
) -> crate::primitive::PaintStateSnapshot {
    let snap = crate::primitive::PaintStateSnapshot::of(state);
    crate::primitive::PaintStateSnapshot {
        hovered_idx: snap.hovered_idx.and_then(|i| band.relative(i)),
        pressed_idx: snap.pressed_idx.and_then(|i| band.relative(i)),
        scroll_px,
        ..snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_selection_scrolls_an_owned_scroll_and_leaves_an_app_scroll() {
        let mut owned = HostLatch::default();
        assert!(owned.follow_selection(0, 5, 20));
        assert_eq!(owned.get(&0), 16);
        assert!(!owned.follow_selection(0, 5, 18), "already visible");
        // A first frame that pins a scroll keeps it.
        let mut pinned = HostLatch::default();
        assert!(!pinned.follow_selection(40, 5, 0));
        assert_eq!(pinned.get(&40), 40);
        // A scroll the app changed this frame wins over the selection.
        let mut moved = HostLatch::default();
        moved.set(&0, 3);
        assert!(!moved.follow_selection(30, 5, 0));
        assert_eq!(moved.get(&30), 30);
    }

    #[test]
    fn row_count_widens_a_stale_total_to_cover_the_loaded_slice() {
        assert_eq!(ItemsWindow::new(0, 100).row_count(10), 100);
        assert_eq!(ItemsWindow::new(95, 100).row_count(10), 105);
    }

    #[test]
    fn band_is_bounded_by_its_rows_never_by_the_total() {
        let w = ItemsWindow::new(0, 10_000_000_000);
        let b = band(w, 0, 5_000, band_rows_for(20.0, 1.0));
        assert_eq!(
            b,
            Band {
                start: 5_000,
                len: 28
            }
        );
    }

    #[test]
    fn band_near_the_end_paints_the_short_tail() {
        let b = band(ItemsWindow::new(0, 100), 0, 95, 20);
        assert_eq!(b, Band { start: 95, len: 5 });
        let past = band(ItemsWindow::new(0, 100), 0, 500, 20);
        assert_eq!(past, Band { start: 99, len: 1 });
    }

    #[test]
    fn materialise_fills_unloaded_positions_with_placeholders() {
        let loaded = vec![10, 11, 12];
        let rows = materialise(
            &loaded,
            ItemsWindow::new(10, 100),
            Band { start: 9, len: 5 },
            |abs| abs as i32 * -1,
        );
        assert_eq!(rows, vec![-9, 10, 11, 12, -13]);
    }

    #[test]
    fn split_scroll_px_resolves_whole_rows_and_the_remainder() {
        assert_eq!(split_scroll_px(3, None, 20.0), (3, None));
        assert_eq!(split_scroll_px(3, Some(45.0), 20.0), (2, Some(5.0)));
    }

    #[test]
    fn rows_requested_is_none_while_the_slice_covers_the_viewport() {
        let w = ItemsWindow::new(0, 100_000);
        assert_eq!(rows_requested(w, 50, 10, 20), None);
        assert_eq!(rows_requested(w, 50, 40, 20), Some((20, 60)));
        assert_eq!(
            rows_requested(w, 50, 99_990, 20),
            Some((99_970, 30)),
            "the request is clamped to the total"
        );
        assert_eq!(rows_requested(ItemsWindow::new(0, 0), 0, 0, 20), None);
    }

    #[test]
    fn host_latch_lets_the_descriptor_win_only_when_it_changes() {
        let mut latch = HostLatch::default();
        assert_eq!(latch.get(&0), 0);
        latch.set(&0, 5);
        assert_eq!(latch.get(&0), 5, "an unchanged descriptor keeps the move");
        assert_eq!(latch.get(&7), 7, "a changed descriptor wins");
        latch.map(|v| v + 1);
        assert_eq!(latch.get(&1), 6, "both sides shift");
        assert_eq!(latch.get(&0), 0);
    }

    #[test]
    fn rows_request_latch_asks_once_per_range_and_rearms_when_covered() {
        let id = WidgetId::new("t");
        let w = ItemsWindow::new(0, 1_000);
        let mut latch = RowsRequestLatch::default();
        assert_eq!(latch.request(id, w, 50, 10, 20), None);
        let first = latch.request(id, w, 50, 100, 20);
        assert_eq!(
            first,
            Some(crate::UiEvent::RowsRequested {
                id,
                start: 80,
                count: 60
            })
        );
        assert_eq!(latch.request(id, w, 50, 100, 20), None, "no repeat");
        assert!(
            latch.request(id, w, 50, 101, 20).is_some(),
            "a new range asks"
        );
        assert_eq!(latch.request(id, w, 50, 0, 20), None);
        assert!(latch.request(id, w, 50, 101, 20).is_some(), "re-armed");
    }
}
