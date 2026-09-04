//! Shared text-selection state machine (#741).
//!
//! `GtkTextSelection`/`TuiTextSelection` were byte-identical structs, and
//! the region registry plus `active_text_selection`/`set_active_text_selection`/
//! `clear_text_selection`/`clear_selection_display`/`cancel_text_selection_drag`/
//! `select_all_text_region` were ~190 lines duplicated verbatim between
//! `gtk/backend.rs` and `tui/backend.rs`. `PRIMITIVE_RULES.md`'s
//! primitive-first rule (#713) forbids a third copy for Win-GUI — this
//! module is the one implementation every pixel- and cell-based backend
//! embeds as a field, mirrors `desktop.rs`'s extraction of
//! `WindowDragArm`/`ModalPumpDepth` (#498): backend-neutral state plus
//! pure helpers, not a trait, so each backend still owns its own storage
//! location and decides when to call in (from its `Backend` trait
//! overrides and its runner's mouse/keyboard pre-processing).
//!
//! ## What stays backend-specific
//!
//! Painting the highlight and extracting the copied text read from two
//! different sources depending on the backend:
//!
//! - **TUI** reads the live `ratatui::buffer::Buffer` cell-by-cell —
//!   only available inside `terminal.draw`'s closure, so `TuiBackend`
//!   keeps its own `apply_selection_highlight`/`extract_selection_text`.
//! - **GTK, Win-GUI, and (eventually) macOS** are pixel-based and store
//!   selectable source text on the `TextRegion` itself (`TextRegion::lines`
//!   — see that field's doc). [`pixel_selection_ranges`] and
//!   [`extract_lines_pixel`] are the row/column math and text-slicing
//!   those three share; each backend still owns the actual paint call
//!   (Cairo `fill()` for GTK, Direct2D `FillRectangle` for Win-GUI)
//!   since that's real toolkit code with no portable shape.

#[cfg(any(feature = "gtk", feature = "win"))]
use crate::dispatch::text_selection_line_range;
use crate::dispatch::{DragState, DragTarget, TextRegion};
use crate::event::Point;
#[cfg(any(feature = "gtk", feature = "win"))]
use crate::event::Rect;
use crate::types::WidgetId;

/// A finalised or in-progress text selection: a region id plus anchor/focus
/// in the backend's native coordinate space (TUI: cells; GTK/Win-GUI/macOS:
/// pixels). Persists after the mouse button is released until Ctrl-C
/// copies the text or a new mouse-down clears it.
///
/// Lifted from the byte-identical `GtkTextSelection`/`TuiTextSelection`
/// (#741) — every field and its meaning is unchanged from those two.
#[derive(Debug, Clone)]
pub(crate) struct TextSelection {
    pub region: WidgetId,
    pub anchor: Point,
    pub focus: Point,
}

/// Per-frame `TextRegion` registry plus the active-selection state a
/// `Backend::register_text_region`/`cancel_text_selection_drag` override
/// needs. Embedded as a field (not a blanket trait impl) by every backend
/// that declares `BackendCaps::text_selection = true` — `GtkBackend`,
/// `TuiBackend`, and (#741) `WinBackend`.
#[derive(Default)]
pub(crate) struct TextSelectionState {
    /// Selectable text regions registered during the current frame via
    /// `Backend::register_text_region`. Cleared at the start of each
    /// frame by [`Self::begin_frame`].
    pub text_regions: Vec<TextRegion>,
    /// Finalised selection (may persist after mouse-up). `None` when no
    /// selection is active.
    active_selection: Option<TextSelection>,
    /// The id of the most-recently focused/hovered `TextRegion` — used by
    /// [`Self::select_all_text_region`] to resolve the Ctrl-A target.
    /// Updated by [`Self::set_active_text_selection`] (a drag produced a
    /// `TextSelectionChanged` event) and [`Self::track_focused_text_region`]
    /// (a runner's mouse-down started a `TextSelection` drag before the
    /// first move). Intentionally NOT cleared by
    /// [`Self::clear_text_selection`]/[`Self::clear_selection_display`] so
    /// Ctrl-A still targets the right region after Ctrl-C or a plain click.
    last_text_region_id: Option<WidgetId>,
}

impl TextSelectionState {
    /// Register `region` for the current frame. Backs the
    /// `Backend::register_text_region` override.
    pub fn register_text_region(&mut self, region: TextRegion) {
        self.text_regions.push(region);
    }

    /// Clear the per-frame region registry. Called by the backend's own
    /// `begin_frame` (same lifecycle as its other per-frame caches).
    pub fn begin_frame(&mut self) {
        self.text_regions.clear();
    }

    /// Look up a registered region by id.
    pub fn find_region(&self, id: &WidgetId) -> Option<&TextRegion> {
        self.text_regions.iter().find(|r| &r.id == id)
    }

    /// Return the current active text selection, if any.
    pub fn active_text_selection(&self) -> Option<&TextSelection> {
        self.active_selection.as_ref()
    }

    /// Update (or start) the active text selection. Called by the runner
    /// when a `TextSelectionChanged` event arrives, and by
    /// [`Self::select_all_text_region`]. Also updates
    /// [`Self::last_text_region_id`] so Ctrl-A can resolve the correct
    /// target even after the drag has ended.
    pub fn set_active_text_selection(&mut self, region: WidgetId, anchor: Point, focus: Point) {
        self.last_text_region_id = Some(region.clone());
        self.active_selection = Some(TextSelection {
            region,
            anchor,
            focus,
        });
    }

    /// Clear the active text selection highlight only (does NOT end an
    /// in-progress `TextSelection` drag). Called before dispatching a new
    /// mouse-down so the old highlight disappears without interrupting the
    /// drag that is about to start.
    pub fn clear_selection_display(&mut self) {
        self.active_selection = None;
    }

    /// Clear the active text selection and end any in-progress
    /// `TextSelection` drag. Called after Ctrl-C copies the selection or
    /// on a plain click outside any text region.
    pub fn clear_text_selection(&mut self, drag_state: &mut DragState) {
        self.active_selection = None;
        if matches!(drag_state.target(), Some(DragTarget::TextSelection { .. })) {
            drag_state.end();
        }
    }

    /// End any in-progress `TextSelection` drag without clearing the
    /// displayed `active_selection`. Mirrors [`Self::clear_text_selection`]
    /// but preserves the highlight — backs the `Backend` trait's
    /// `cancel_text_selection_drag` override, which apps hosting an
    /// embedded terminal call to abort a speculative drag before
    /// forwarding a click to a PTY.
    pub fn cancel_text_selection_drag(&mut self, drag_state: &mut DragState) {
        if matches!(drag_state.target(), Some(DragTarget::TextSelection { .. })) {
            drag_state.end();
        }
    }

    /// Record that `id` is the most-recently focused/clicked `TextRegion`.
    /// Called by the runner's mouse-down handling after a `TextSelection`
    /// drag begins, so [`Self::select_all_text_region`] can resolve the
    /// correct target even before the first drag-move fires a
    /// `TextSelectionChanged` event.
    pub fn track_focused_text_region(&mut self, id: WidgetId) {
        self.last_text_region_id = Some(id);
    }

    /// Set the active selection to cover the entire visible content of the
    /// most-recently focused `TextRegion`. Returns `true` when a region was
    /// found and the selection was set; `false` when no region can be
    /// resolved (zero registered regions, or multiple with no prior
    /// interaction).
    ///
    /// Target resolution order:
    /// 1. [`Self::last_text_region_id`] if the region is still registered
    ///    this frame.
    /// 2. The sole registered region (if exactly one exists).
    /// 3. Returns `false` — caller should fall through to the app.
    ///
    /// # Viewport-only limitation
    ///
    /// `TextRegion.bounds` is the painted viewport. For scrolled panels
    /// (e.g. long issue bodies) only the on-screen rows are selected.
    /// Full-document select-all requires `TextRegion` to carry
    /// total-content rows; a follow-up issue tracks this.
    pub fn select_all_text_region(&mut self) -> bool {
        // Resolve the target region id.
        let region_id = if let Some(ref id) = self.last_text_region_id {
            if self.text_regions.iter().any(|r| &r.id == id) {
                id.clone()
            } else if self.text_regions.len() == 1 {
                self.text_regions[0].id.clone()
            } else {
                return false;
            }
        } else if self.text_regions.len() == 1 {
            self.text_regions[0].id.clone()
        } else {
            return false;
        };

        // Clone the bounds to release the borrow before calling
        // `set_active_text_selection`, which needs `&mut self`.
        let bounds = match self.text_regions.iter().find(|r| r.id == region_id) {
            Some(r) => r.bounds,
            None => return false,
        };

        // Anchor at top-left; focus just past the bottom-right so that
        // `text_selection_line_range` covers every row. The focus is
        // clamped to the region bounds by that function, so placing it one
        // unit outside is harmless.
        let anchor = Point::new(bounds.x, bounds.y);
        let focus = Point::new(bounds.x + bounds.width, bounds.y + bounds.height);
        self.set_active_text_selection(region_id, anchor, focus);
        true
    }
}

/// Convert a pixel-space selection (`anchor`/`focus`, plus the selected
/// region's `bounds`) into cell-relative `(row, col_start, col_end)` ranges
/// via [`text_selection_line_range`] — the row/column math
/// `GtkBackend`'s/`WinBackend`'s `apply_selection_highlight`/
/// `extract_selection_text` share (both pixel-based backends storing
/// source text on the `TextRegion`, unlike TUI which reads the live cell
/// buffer).
///
/// `None` when `line_height`/`char_width` aren't positive (metrics not
/// known yet, e.g. before the first real frame).
///
/// `#[cfg(any(feature = "gtk", feature = "win"))]`: the only two adopters
/// today — a `tui`-only build (the `--features tui` clippy leg) would
/// otherwise flag this as dead code.
#[cfg(any(feature = "gtk", feature = "win"))]
pub(crate) fn pixel_selection_ranges(
    region_bounds: Rect,
    anchor: Point,
    focus: Point,
    line_height: f32,
    char_width: f32,
) -> Option<Vec<(u16, f32, f32)>> {
    if line_height <= 0.0 || char_width <= 0.0 {
        return None;
    }
    let bx = region_bounds.x;
    let by = region_bounds.y;
    let bw = region_bounds.width / char_width;
    let bh = region_bounds.height / line_height;
    let cell_bounds = Rect::new(0.0, 0.0, bw, bh);
    let anchor_cell = Point {
        x: (anchor.x - bx) / char_width,
        y: (anchor.y - by) / line_height,
    };
    let focus_cell = Point {
        x: (focus.x - bx) / char_width,
        y: (focus.y - by) / line_height,
    };
    Some(text_selection_line_range(
        anchor_cell,
        focus_cell,
        cell_bounds,
    ))
}

/// Extract the selected text from `region.lines` (pixel-based backends
/// only — see the module doc) using pixel `anchor`/`focus`. Returns an
/// empty string when there is no `lines` content or the metrics aren't
/// known yet. Shared by GTK's and Win-GUI's `extract_selection_text`.
#[cfg(any(feature = "gtk", feature = "win"))]
pub(crate) fn extract_lines_pixel(
    region: &TextRegion,
    anchor: Point,
    focus: Point,
    line_height: f32,
    char_width: f32,
) -> String {
    if region.lines.is_empty() {
        return String::new();
    }
    let Some(ranges) =
        pixel_selection_ranges(region.bounds, anchor, focus, line_height, char_width)
    else {
        return String::new();
    };

    let mut lines: Vec<String> = Vec::with_capacity(ranges.len());
    for (row_cell, col_start, col_end) in ranges {
        let line_idx = row_cell as usize;
        let Some(src) = region.lines.get(line_idx) else {
            continue;
        };
        let col_start = col_start as usize;
        let col_end = col_end as usize;
        // Extract by character index (col_start..col_end).
        let chars: Vec<char> = src.chars().collect();
        let s: String = chars
            .get(col_start.min(chars.len())..col_end.min(chars.len()))
            .unwrap_or(&[])
            .iter()
            .collect();
        lines.push(s.trim_end().to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    //! Coverage for the state machine + pixel math lifted from
    //! `gtk/backend.rs`'s pre-#741 tests — see that file's (now removed)
    //! `gtk_*_text_selection*`/`gtk_extract_selection_text_*` suite for the
    //! scenarios these generalise. `TuiBackend`/`GtkBackend`/`WinBackend`
    //! each keep their own thinner tests proving they delegate here
    //! correctly; this module owns the actual behaviour under test.
    use super::*;
    use crate::event::Rect;

    fn region(id: &str, x: f32, y: f32, w: f32, h: f32, lines: Vec<&str>) -> TextRegion {
        TextRegion {
            id: WidgetId::new(id),
            bounds: Rect::new(x, y, w, h),
            lines: lines.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn register_and_clear_round_trip() {
        let mut state = TextSelectionState::default();
        state.register_text_region(region("r1", 0.0, 0.0, 100.0, 50.0, vec![]));
        state.register_text_region(region("r2", 0.0, 50.0, 100.0, 50.0, vec![]));
        assert_eq!(state.text_regions.len(), 2);
        state.begin_frame();
        assert!(state.text_regions.is_empty());
    }

    #[test]
    fn set_and_clear_active_selection() {
        let mut state = TextSelectionState::default();
        state.register_text_region(region("r", 0.0, 0.0, 200.0, 100.0, vec![]));
        assert!(state.active_text_selection().is_none());
        state.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(50.0, 20.0),
        );
        assert!(state.active_text_selection().is_some());
        state.clear_selection_display();
        assert!(state.active_text_selection().is_none());
    }

    #[test]
    fn clear_text_selection_also_ends_a_text_selection_drag() {
        let mut state = TextSelectionState::default();
        let mut drag = DragState::new();
        drag.begin(DragTarget::TextSelection {
            region: WidgetId::new("r"),
            anchor: Point::new(0.0, 0.0),
        });
        state.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
        );
        state.clear_text_selection(&mut drag);
        assert!(state.active_text_selection().is_none());
        assert!(
            drag.target().is_none(),
            "clear_text_selection must also end an in-progress TextSelection drag"
        );
    }

    #[test]
    fn clear_text_selection_leaves_an_unrelated_drag_alone() {
        let mut state = TextSelectionState::default();
        let mut drag = DragState::new();
        drag.begin(DragTarget::ScrollbarY {
            widget: WidgetId::new("sb"),
            track_start: 0.0,
            track_length: 100.0,
            thumb_length: 20.0,
            max_scroll: 10,
            grab_offset: 0.0,
            inverted: false,
        });
        state.clear_text_selection(&mut drag);
        assert!(
            drag.target().is_some(),
            "clear_text_selection must not touch a non-TextSelection drag"
        );
    }

    #[test]
    fn cancel_text_selection_drag_preserves_the_displayed_selection() {
        let mut state = TextSelectionState::default();
        let mut drag = DragState::new();
        drag.begin(DragTarget::TextSelection {
            region: WidgetId::new("r"),
            anchor: Point::new(0.0, 0.0),
        });
        state.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
        );
        state.cancel_text_selection_drag(&mut drag);
        assert!(drag.target().is_none());
        assert!(
            state.active_text_selection().is_some(),
            "cancel_text_selection_drag must not clear the displayed selection"
        );
    }

    #[test]
    fn select_all_targets_the_sole_region() {
        let mut state = TextSelectionState::default();
        state.register_text_region(region("body", 0.0, 0.0, 10.0, 5.0, vec![]));
        assert!(state.select_all_text_region());
        let sel = state
            .active_text_selection()
            .expect("selection should be active");
        assert_eq!(sel.anchor, Point::new(0.0, 0.0));
        assert_eq!(sel.focus, Point::new(10.0, 5.0));
    }

    #[test]
    fn select_all_returns_false_with_no_regions() {
        let mut state = TextSelectionState::default();
        assert!(!state.select_all_text_region());
        assert!(state.active_text_selection().is_none());
    }

    #[test]
    fn select_all_prefers_the_tracked_region_over_an_ambiguous_pair() {
        let mut state = TextSelectionState::default();
        state.register_text_region(region("r1", 0.0, 0.0, 10.0, 5.0, vec![]));
        state.register_text_region(region("r2", 0.0, 5.0, 10.0, 5.0, vec![]));
        state.track_focused_text_region(WidgetId::new("r2"));
        assert!(state.select_all_text_region());
        assert_eq!(
            state.active_text_selection().unwrap().region,
            WidgetId::new("r2")
        );
    }

    #[cfg(any(feature = "gtk", feature = "win"))]
    #[test]
    fn extract_lines_pixel_single_row() {
        let region = region(
            "body",
            0.0,
            0.0,
            200.0,
            100.0,
            vec!["The quick brown fox jumps over the lazy dog."],
        );
        // char_w = 10, line_h = 20 → columns 0..20 covers "The quick brown fox "
        let text = extract_lines_pixel(
            &region,
            Point::new(0.0, 0.0),
            Point::new(200.0, 10.0),
            20.0,
            10.0,
        );
        assert_eq!(text, "The quick brown fox");
    }

    #[cfg(any(feature = "gtk", feature = "win"))]
    #[test]
    fn extract_lines_pixel_multi_row() {
        let region = region(
            "body",
            0.0,
            0.0,
            200.0,
            100.0,
            vec!["first line here", "second line here"],
        );
        let text = extract_lines_pixel(
            &region,
            Point::new(0.0, 0.0),
            Point::new(200.0, 30.0),
            20.0,
            10.0,
        );
        assert_eq!(text, "first line here\nsecond line here");
    }

    #[cfg(any(feature = "gtk", feature = "win"))]
    #[test]
    fn extract_lines_pixel_empty_when_no_lines_content() {
        let region = region("body", 0.0, 0.0, 200.0, 32.0, vec![]);
        let text = extract_lines_pixel(
            &region,
            Point::new(0.0, 0.0),
            Point::new(200.0, 32.0),
            16.0,
            8.0,
        );
        assert_eq!(text, "");
    }

    #[cfg(any(feature = "gtk", feature = "win"))]
    #[test]
    fn extract_lines_pixel_empty_when_metrics_unknown() {
        let region = region("body", 0.0, 0.0, 200.0, 32.0, vec!["hello"]);
        let text = extract_lines_pixel(
            &region,
            Point::new(0.0, 0.0),
            Point::new(200.0, 32.0),
            0.0,
            0.0,
        );
        assert_eq!(text, "");
    }
}
