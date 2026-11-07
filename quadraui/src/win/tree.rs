//! Direct2D / DirectWrite rasteriser for [`crate::TreeView`].
//!
//! Content painting (background, rows, chevron/icon/badge/text or
//! inline-edit, vertical scrollbar) is shared with other backends via
//! [`crate::primitives::tree::native_surface_paint::paint`] — see that
//! fn's module doc for what's shared. This rasteriser now paints
//! `TreeRow::edit` with full caret + selection-highlight + placeholder
//! treatment like every other backend.
//!
//! [`TreeView::layout`] (the D6 layout API) does every positioning and
//! row-clipping decision; this module measures chevron width with a
//! real `DWrite::measure_text` call via [`win_tree_layout_measured`] —
//! see that fn's doc. [`win_tree_layout`]'s `line_height * 0.65` estimate
//! only runs when no live DWrite handle is attached yet. Paint and
//! hit-test both derive from the same layout call for whatever handle
//! is live, so they can't drift apart.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod tree;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! Takes the live theme as a `&Theme` parameter, matching every other
//! Win-GUI rasteriser's convention — the caller
//! ([`crate::win::WinBackend::draw_tree`]) passes `&self.current_theme`,
//! the same field `Backend::set_theme` writes. Takes the live
//! [`crate::style::Style`] the same way — `&self.current_style`,
//! written by `Backend::set_style` — so a host-set
//! [`crate::style::Style::row_padding_x`] reaches both the chevron paint
//! and the `Chevron` hit region [`win_tree_layout_measured`] computes.
//!
//! `WinBackend` tracks a `nerd_fonts_enabled` flag the same way
//! `TuiBackend`/`GtkBackend`/`MacBackend` do, and this rasteriser picks
//! [`crate::types::Icon::glyph`] vs [`crate::types::Icon::fallback`] the
//! same way theirs do. `draw_tree` is the only Win-GUI rasteriser wired
//! to the flag so far — `win::activity_bar` still always paints `fallback`
//! (see that module's doc). The chosen glyph paints in
//! [`crate::types::Icon::color`] when set, else the row's `def_fg`.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::layout_metrics::{tree_row_pitch, TextMeasure};
use crate::primitives::tree::{
    tree_chevron_glyph, TreePaintState, TreeRowMeasure, TreeView, TreeViewLayout,
};
use crate::theme::Theme;
use crate::types::Decoration;

/// Compute a [`TreeView`]'s layout without painting and without a real
/// measurer — the `line_height * 0.65` chevron-width *estimate*, for
/// callers with no live `&dyn TextMeasure` (no surface/DWrite attached
/// yet). Prefer [`win_tree_layout_measured`] whenever a `DWrite` handle
/// is available — it measures the real chevron glyph instead.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::tree_layout`]
/// — the row-pitch/chevron math is identical across every pixel backend,
/// so it lives there once instead of once per backend.
pub fn win_tree_layout(
    tree: &TreeView,
    rect: Rect,
    line_height: f32,
    row_padding_x: f32,
) -> TreeViewLayout {
    crate::primitives::layout_metrics::tree_layout(tree, rect, line_height as f64, row_padding_x)
}

/// Like [`win_tree_layout`], but measures the chevron glyph actually
/// configured on `tree.style` (`chevron_expanded` / `chevron_collapsed`)
/// through `measure` instead of estimating its pixel width as
/// `line_height * 0.65`. `DWrite` implements
/// [`TextMeasure`](crate::primitives::layout_metrics::TextMeasure)
/// directly (see `win::text`'s impl), so callers pass a live `&DWrite`
/// straight through — no adapter type needed, same shape
/// `macos::tree::mac_tree_layout` takes with `&CTFont`.
///
/// [`draw_tree`] and [`crate::win::WinBackend::tree_layout`] both go
/// through this so paint and hit-test never disagree about where the
/// chevron ends.
pub fn win_tree_layout_measured(
    measure: &dyn TextMeasure,
    rect: Rect,
    tree: &TreeView,
    line_height: f32,
    row_padding_x: f32,
) -> TreeViewLayout {
    crate::primitives::layout_metrics::tree_layout_measured(
        tree,
        rect,
        line_height as f64,
        measure,
        row_padding_x,
    )
}

/// Like [`win_tree_layout_measured`], but threads an explicit
/// pixel-precise scroll position (`scroll_px`) through to
/// [`TreeView::layout_with_scroll_px`] instead of only
/// [`TreeView::scroll_offset`]'s whole-row value. The row-measurement
/// closure mirrors [`crate::primitives::layout_metrics::tree_layout_measured`]'s
/// (chevron glyph width measured via `measure`, same as
/// [`win_tree_layout_measured`]) — that shared fn calls
/// [`TreeView::layout`], which can't reach the scroll_px-aware method,
/// so this duplicates its small measurement closure rather than
/// widening that fn's signature. `scroll_px: None` produces
/// byte-identical output to [`win_tree_layout_measured`].
pub fn win_tree_layout_measured_with_scroll_px(
    measure: &dyn TextMeasure,
    rect: Rect,
    tree: &TreeView,
    line_height: f32,
    row_padding_x: f32,
    scroll_px: Option<f32>,
) -> TreeViewLayout {
    let header_height = (line_height * 1.2).round();
    let item_height = tree_row_pitch(tree, line_height as f64) as f32;
    let indent_px = (line_height * 0.9).round();
    let show_chevrons = tree.style.show_chevrons;
    tree.layout_with_scroll_px(rect.width, rect.height, scroll_px, |i| {
        let row = &tree.rows[i];
        let is_header = matches!(row.decoration, Decoration::Header);
        let row_h = if is_header {
            header_height
        } else {
            item_height
        };
        let chevron_end_x = if let Some(expanded) = row.is_expanded.filter(|_| show_chevrons) {
            let glyph_w = measure.width_of(&tree_chevron_glyph(&tree.style, expanded));
            Some(row_padding_x + row.indent as f32 * indent_px + glyph_w + 4.0)
        } else {
            None
        };
        TreeRowMeasure {
            height: row_h,
            chevron_end_x,
        }
    })
}

/// Draw a [`TreeView`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`TreeViewLayout`] for host click dispatch — hit regions
/// are tree-local (relative to `rect.x` / `rect.y`), matching every
/// other backend's `draw_tree` contract.
///
/// # Visual contract
///
/// - **Background:** `Theme::tab_bar_bg`.
/// - **Header rows** (`Decoration::Header`): `header_bg` / `header_fg`.
/// - **Selected row** (when `tree.has_focus`): `selected_bg` /
///   `header_fg`.
/// - **Inactive-selected row** (selected but `!tree.has_focus`):
///   `inactive_selected_bg` / `foreground`.
/// - **Muted / Error / Warning rows**: `muted_fg` / `error_fg` /
///   `warning_fg` on the row's own background.
/// - **Indent:** `(line_height * 0.9).round()` DIPs per depth level.
/// - **Chevrons:** `tree.style.chevron_expanded` /
///   `chevron_collapsed` when `tree.style.show_chevrons`, *unless* the
///   field is still holding `TreeStyle::default()`'s `"▾"`/`"▸"`, in
///   which case the shared paint path (`primitives::tree::paint`)
///   substitutes the sharper `codicon::CHEVRON_DOWN`/`CHEVRON_RIGHT`
///   glyph instead; a non-default override is always painted verbatim.
///   Leaves get a `line_height * 0.8` leading offset for alignment.
/// - **Badge** (right-aligned): `badge.fg`/`badge.bg`, falling back to
///   `muted_fg` / the row's own background.
///
/// `nerd_fonts_enabled` picks `row.icon.glyph` when `true` and
/// `row.icon.fallback` when `false` — pass `WinBackend`'s own
/// flag (set via [`crate::Backend::set_nerd_fonts`]), same convention
/// as the TUI/GTK/macOS `draw_tree` rasterisers.
///
/// `style` supplies [`crate::style::Style::row_padding_x`]; the caller
/// ([`crate::win::WinBackend::draw_tree`]) passes `&self.current_style`,
/// the same field `Backend::set_style` writes, matching `theme`'s own
/// `&self.current_theme` convention.
#[allow(clippy::too_many_arguments)]
pub fn draw_tree(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    tree: &TreeView,
    line_height: f32,
    nerd_fonts_enabled: bool,
    theme: &Theme,
    style: &crate::style::Style,
) -> TreeViewLayout {
    let layout = win_tree_layout_measured(dwrite, rect, tree, line_height, style.row_padding_x);

    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::tree::native_surface_paint::paint(
        tree,
        rect,
        &layout,
        line_height,
        nerd_fonts_enabled,
        &mut surface,
        theme,
        style,
        // `hovered_idx` / `pressed_idx`: this base painter never tints
        // rows — that's `draw_tree_with_state`'s job, which composes both
        // indices (and the focus-visible ring below) from
        // `TreePaintState`. `None`/`false` here keep `draw_tree`
        // pixel-identical to the contract above.
        None,
        None,
        false,
    );

    layout
}

/// [`draw_tree`], plus hover/press row tinting, a focus-visible ring,
/// and — when `state.scroll_px` is `Some` — a pixel-precise scroll
/// position instead of only [`TreeView::scroll_offset`]'s whole-row
/// value: the first visible row is shifted up by the sub-row remainder
/// (via [`win_tree_layout_measured_with_scroll_px`]) and the vertical
/// scrollbar thumb tracks the same position (via
/// [`crate::primitives::tree::native_surface_paint::paint_with_scroll_px`]).
/// `state.scroll_px: None` paints pixel-identically to [`draw_tree`].
#[allow(clippy::too_many_arguments)]
pub fn draw_tree_with_state(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    tree: &TreeView,
    line_height: f32,
    nerd_fonts_enabled: bool,
    theme: &Theme,
    style: &crate::style::Style,
    state: &TreePaintState,
    focus_ring_visible: bool,
) -> TreeViewLayout {
    let layout = win_tree_layout_measured_with_scroll_px(
        dwrite,
        rect,
        tree,
        line_height,
        style.row_padding_x,
        state.scroll_px,
    );

    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::tree::native_surface_paint::paint_with_scroll_px(
        tree,
        rect,
        &layout,
        line_height,
        nerd_fonts_enabled,
        &mut surface,
        theme,
        style,
        state.hovered_idx,
        state.pressed_idx,
        focus_ring_visible,
        state.scroll_px,
    );

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tree::{TreeRow, TreeViewHit};
    use crate::style::Style;
    use crate::types::{
        Badge, Color, Decoration, Icon, SelectionMode, StyledText, TreeStyle, WidgetId,
    };
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 200.0;
    const H: f32 = 200.0;
    const LINE_HEIGHT: f32 = 14.0;

    fn leaf(idx: u16, label: &str) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label.to_string()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn branch(idx: u16, label: &str, expanded: bool) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: Some(Icon::new("", "D")),
            text: StyledText::plain(label.to_string()),
            badge: Some(Badge::plain("3")),
            is_expanded: Some(expanded),
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn make_tree(rows: Vec<TreeRow>) -> TreeView {
        TreeView {
            id: WidgetId::new("tree"),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    /// Paint↔click round trip: painted row backgrounds and the
    /// independently-computed `win_tree_layout` must agree on which
    /// row a click lands on, including after a scroll offset.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut tree = make_tree(vec![
            branch(0, "src", true),
            leaf(1, "main.rs"),
            leaf(2, "lib.rs"),
            leaf(3, "util.rs"),
        ]);
        tree.selected_path = Some(vec![2]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                    &Style::default(),
                );
            })
            .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT, 8.0))
            .expect("paint tree");

        assert_eq!(layout.visible_rows.len(), 4, "all four rows should fit");

        for vis in &layout.visible_rows {
            let cx = vis.bounds.x + vis.bounds.width / 2.0;
            let cy = vis.bounds.y + vis.bounds.height / 2.0;
            let hit = layout.hit_test(cx, cy);
            match hit {
                TreeViewHit::Row(idx) | TreeViewHit::Chevron(idx) => {
                    assert_eq!(
                        idx, vis.row_idx,
                        "row centre should hit-test back to itself"
                    );
                }
                other => panic!("expected a row hit at row {}, got {:?}", vis.row_idx, other),
            }
        }

        // Selected row (index 2) painted `selected_bg` at its own bounds.
        let theme = Theme::default();
        let sel_bounds = layout.visible_rows[2].bounds;
        let px = surface.pixel_at((sel_bounds.x + 1.0) as u32, (sel_bounds.y + 1.0) as u32);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "selected row should paint selected_bg at its own bounds"
        );
    }

    /// A click on a branch row's chevron zone resolves to `Chevron`,
    /// and to the right of it resolves to `Row` — mirrors
    /// `gtk::tree`'s chevron split tests.
    #[test]
    fn chevron_and_body_hit_split() {
        let tree = make_tree(vec![branch(0, "src", true)]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_tree_layout(&tree, rect, LINE_HEIGHT, 8.0);

        let hit = layout.hit_test(1.0, layout.visible_rows[0].bounds.y + 1.0);
        assert!(matches!(hit, TreeViewHit::Chevron(0)), "got {:?}", hit);

        let hit = layout.hit_test(100.0, layout.visible_rows[0].bounds.y + 1.0);
        assert!(matches!(hit, TreeViewHit::Row(0)), "got {:?}", hit);
    }

    /// Scroll-offset round trip: after scrolling 2 rows down, the
    /// topmost visible row is `rows[2]` and a click on its painted
    /// bounds resolves to `Row(2)` — catches paint/hit-test scroll
    /// drift.
    #[test]
    fn scroll_offset_paint_and_click_agree() {
        let mut tree = make_tree((0..8).map(|i| leaf(i, &format!("file-{i}.rs"))).collect());
        tree.scroll_offset = 2;
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let layout = surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                    &Style::default(),
                );
            })
            .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT, 8.0))
            .expect("paint");

        let first = layout.visible_rows.first().expect("has visible rows");
        assert_eq!(
            first.row_idx, 2,
            "scroll_offset=2 should put rows[2] at top"
        );
        let hit = layout.hit_test(
            first.bounds.x + 5.0,
            first.bounds.y + first.bounds.height / 2.0,
        );
        assert!(matches!(hit, TreeViewHit::Row(2)), "got {:?}", hit);
    }

    /// A click below the last row returns `Empty`.
    #[test]
    fn click_below_last_row_returns_empty() {
        let tree = make_tree(vec![leaf(0, "a"), leaf(1, "b")]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_tree_layout(&tree, rect, LINE_HEIGHT, 8.0);
        let last = layout.visible_rows.last().expect("has rows");
        let hit = layout.hit_test(10.0, last.bounds.y + last.bounds.height + 5.0);
        assert!(matches!(hit, TreeViewHit::Empty), "got {:?}", hit);
    }

    /// No-paint layout must agree byte-for-byte with what `draw_tree`
    /// painted.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let tree = make_tree(vec![branch(0, "src", true), leaf(1, "main.rs")]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                    &Style::default(),
                );
            })
            .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT, 8.0))
            .expect("paint");
        let no_paint = win_tree_layout(&tree, rect, LINE_HEIGHT, 8.0);
        assert_eq!(painted, no_paint);
    }

    /// `scroll_px: None` must produce byte-identical output to
    /// [`win_tree_layout_measured`] — the "paints exactly as before"
    /// contract [`draw_tree_with_state`] documents.
    #[test]
    fn win_tree_layout_measured_with_scroll_px_none_matches_win_tree_layout_measured() {
        let tree = make_tree((0..6).map(|i| leaf(i, &format!("row-{i}"))).collect());
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");

        let via_plain = win_tree_layout_measured(&dwrite, rect, &tree, LINE_HEIGHT, 8.0);
        let via_scroll_px =
            win_tree_layout_measured_with_scroll_px(&dwrite, rect, &tree, LINE_HEIGHT, 8.0, None);
        assert_eq!(via_plain, via_scroll_px);
    }

    /// A fractional `scroll_px` shifts the first visible row's bounds up
    /// by the sub-row remainder and reports it back as `scroll_sub_px`,
    /// matching `mac_tree_layout_with_scroll_px`'s contract.
    #[test]
    fn win_tree_layout_measured_with_scroll_px_shifts_first_row_by_sub_row_remainder() {
        let tree = make_tree((0..6).map(|i| leaf(i, &format!("row-{i}"))).collect());
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");

        let item_height =
            crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT as f64) as f32;
        let half_row = item_height / 2.0;
        let layout = win_tree_layout_measured_with_scroll_px(
            &dwrite,
            rect,
            &tree,
            LINE_HEIGHT,
            8.0,
            Some(half_row),
        );

        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.scroll_sub_px, half_row);
        assert_eq!(
            layout.visible_rows[0].bounds.y, -half_row,
            "the first visible row must shift up by the sub-row remainder"
        );
    }

    /// `draw_tree_with_state` with `scroll_px: None` must paint
    /// pixel-identically to [`draw_tree`]; a fractional `scroll_px` must
    /// paint a visibly different first row — the clipped partial-row
    /// look of pixel-smooth scrolling.
    #[test]
    fn draw_tree_with_state_scroll_px_shifts_first_row_pixels() {
        let tree = make_tree((0..6).map(|i| leaf(i, &format!("row-{i}"))).collect());
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let item_height =
            crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT as f64) as f32;

        let paint_at = |scroll_px: Option<f32>| -> HeadlessSurface {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let state =
                crate::primitives::tree::TreePaintState::default().with_scroll_px(scroll_px);
            surface
                .paint(|target| {
                    draw_tree_with_state(
                        target,
                        &dwrite,
                        rect,
                        &tree,
                        LINE_HEIGHT,
                        false,
                        &Theme::default(),
                        &Style::default(),
                        &state,
                        false,
                    );
                })
                .expect("paint tree");
            surface
        };

        let unscrolled = paint_at(None);
        let half_row_scrolled = paint_at(Some(item_height / 2.0));

        let reference = {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            surface
                .paint(|target| {
                    draw_tree(
                        target,
                        &dwrite,
                        rect,
                        &tree,
                        LINE_HEIGHT,
                        false,
                        &Theme::default(),
                        &Style::default(),
                    );
                })
                .expect("paint tree");
            surface
        };
        for y in 0..(H as u32) {
            for x in 0..(W as u32) {
                assert_eq!(
                    unscrolled.pixel_at(x, y),
                    reference.pixel_at(x, y),
                    "scroll_px: None must paint pixel-identically to draw_tree at ({x}, {y})"
                );
            }
        }

        let mut any_diff = false;
        for y in 0..(item_height as u32) {
            for x in 0..(W as u32) {
                if unscrolled.pixel_at(x, y) != half_row_scrolled.pixel_at(x, y) {
                    any_diff = true;
                }
            }
        }
        assert!(
            any_diff,
            "a half-row scroll_px must paint a visibly different first row \
             than scroll_px: None"
        );
    }

    /// `nerd_fonts_enabled` selects `row.icon.glyph` vs
    /// `row.icon.fallback`. Uses two ASCII strings of clearly different
    /// width (`"WWWW"` vs `"E"`) rather than a real Nerd Font codepoint,
    /// so the assertion holds headless without a Nerd Font installed.
    /// Measures the painted icon's pixel bounding-box width via
    /// foreground-vs-background scanning — asserted on pixels, not screen
    /// text.
    #[test]
    fn nerd_fonts_flag_selects_glyph_or_fallback() {
        let row = TreeRow {
            path: vec![0],
            indent: 0,
            icon: Some(Icon::new("WWWW", "E")),
            text: StyledText::plain(String::new()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        };
        let tree = make_tree(vec![row]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let theme = Theme::default();
        let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);

        let painted_width = |nerd_fonts_enabled: bool| -> u32 {
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            surface
                .paint(|target| {
                    draw_tree(
                        target,
                        &dwrite,
                        rect,
                        &tree,
                        LINE_HEIGHT,
                        nerd_fonts_enabled,
                        &Theme::default(),
                        &Style::default(),
                    );
                })
                .expect("paint");

            // Single row's band starts at y = 0; probe its vertical
            // mid-point.
            let mid_y = (LINE_HEIGHT * 1.4 / 2.0) as u32;
            let mut left = None;
            let mut right = None;
            for x in 0..(W as u32) {
                let px = surface.pixel_at(x, mid_y);
                if (px.r, px.g, px.b) != bg {
                    left.get_or_insert(x);
                    right = Some(x);
                }
            }
            match (left, right) {
                (Some(l), Some(r)) => r - l + 1,
                _ => 0,
            }
        };

        let glyph_width = painted_width(true);
        let fallback_width = painted_width(false);
        assert!(
            glyph_width > fallback_width,
            "nerd_fonts_enabled: true should paint the wider glyph icon \
             (\"WWWW\", measured {glyph_width}px) vs the narrower fallback \
             (\"E\", measured {fallback_width}px) painted when false"
        );
    }

    /// A row with `Icon::color` set paints its icon glyph in that colour;
    /// a row without one paints it in the default row fg. Empty row text
    /// (as in `nerd_fonts_flag_selects_glyph_or_fallback` above) means the
    /// icon glyph is the only ink in the row, so the most-inked pixel on
    /// the row's mid-line is unambiguously the icon. This test passes
    /// `&Theme::default()` explicitly and reads its expected colours from
    /// the same instance, so it stays correct regardless of what
    /// `draw_tree`'s caller wires through in production.
    #[test]
    fn icon_color_paints_icon_glyph_in_that_color_else_default_fg() {
        let icon_color = crate::types::Color::rgb(220, 80, 20);
        let theme = Theme::default();
        let default_fg = theme.foreground;
        let bg = theme.tab_bar_bg;

        let dist = |a: (u8, u8, u8), c: crate::types::Color| {
            let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
            d(a.0, c.r) + d(a.1, c.g) + d(a.2, c.b)
        };

        let most_inked_on_row = |icon: Icon| -> (u8, u8, u8) {
            let row = TreeRow {
                path: vec![0],
                indent: 0,
                icon: Some(icon),
                text: StyledText::plain(String::new()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            };
            let tree = make_tree(vec![row]);
            let rect = Rect::new(0.0, 0.0, W, H);
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            surface
                .paint(|target| {
                    draw_tree(
                        target,
                        &dwrite,
                        rect,
                        &tree,
                        LINE_HEIGHT,
                        false,
                        &Theme::default(),
                        &Style::default(),
                    );
                })
                .expect("paint");

            let mid_y = (LINE_HEIGHT * 1.4 / 2.0) as u32;
            let mut best = (bg.r, bg.g, bg.b);
            let mut best_d = -1i32;
            for x in 0..(W as u32) {
                let px = surface.pixel_at(x, mid_y);
                let c = (px.r, px.g, px.b);
                let d = dist(c, bg);
                if d > best_d {
                    best_d = d;
                    best = c;
                }
            }
            best
        };

        let colored = most_inked_on_row(Icon::new("R", "R").with_color(icon_color));
        assert!(
            dist(colored, icon_color) < dist(colored, default_fg),
            "row with Icon::color set: most-inked pixel {colored:?} should be \
             closer to Icon::color {icon_color:?} than default fg {default_fg:?}"
        );

        let uncolored = most_inked_on_row(Icon::new("R", "R"));
        assert!(
            dist(uncolored, default_fg) < dist(uncolored, icon_color),
            "row without Icon::color: most-inked pixel {uncolored:?} should be \
             closer to default fg {default_fg:?} than the unrelated colour \
             {icon_color:?}"
        );
    }

    /// `win::tree::draw_tree` now paints `TreeRow::edit` with full
    /// edit-state affordance. This test paints a row with `edit: Some(_)`
    /// and asserts its interior contains painted (non-background) pixels
    /// distinct from the plain label — the row's `text` is a single narrow
    /// glyph ("x"), while its `edit.text` is a much wider string, so the
    /// rightmost painted column tells the two apart.
    #[test]
    fn paints_row_being_edited() {
        // Differential design: the row's `text` is a single narrow glyph
        // ("x"), while its `edit.text` is a much wider string, so the
        // rightmost painted column tells the two apart. The edited tree
        // paints substantially further right than the label alone.
        let rightmost_painted_x = |edit: Option<crate::primitives::tree::TreeRowEditState>| -> u32 {
            let row = TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("x".to_string()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit,
            };
            let tree = make_tree(vec![row]);
            let rect = Rect::new(0.0, 0.0, W, H);
            let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            let layout = surface
                .paint(|target| {
                    draw_tree(
                        target,
                        &dwrite,
                        rect,
                        &tree,
                        LINE_HEIGHT,
                        false,
                        &Theme::default(),
                        &Style::default(),
                    );
                })
                .map(|_| win_tree_layout(&tree, rect, LINE_HEIGHT, 8.0))
                .expect("paint tree");

            let theme = Theme::default();
            let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);
            let row0 = &layout.visible_rows[0];
            let y = (row0.bounds.y + row0.bounds.height / 2.0) as u32;
            let mut rightmost = 0u32;
            for x in (row0.bounds.x as u32)..(row0.bounds.x + row0.bounds.width) as u32 {
                let px = surface.pixel_at(x.min(W as u32 - 1), y.min(H as u32 - 1));
                if (px.r, px.g, px.b) != bg {
                    rightmost = x;
                }
            }
            rightmost
        };

        let plain_label_x = rightmost_painted_x(None);
        let editing_x = rightmost_painted_x(Some(crate::primitives::tree::TreeRowEditState {
            text: "a much wider inline-rename string".into(),
            cursor: 3,
            selection_anchor: None,
            placeholder: None,
        }));

        assert!(
            editing_x > plain_label_x + 20,
            "a row with `edit: Some(_)` carrying much wider text must paint \
             noticeably further right ({editing_x}) than the same row's plain \
             label ({plain_label_x}) — this is the #1075 fix for Win never \
             painting TreeRow::edit at all (pre-fix, both values were equal)"
        );
    }

    /// Before this migration, `draw_tree` never
    /// painted a vertical scrollbar even though `Backend::tree_vscrollbar`
    /// already returned real geometry for hit-testing.
    #[test]
    fn paints_vertical_scrollbar_track_when_overflowing() {
        let tree = make_tree((0..40).map(|i| leaf(i, &format!("row-{i}"))).collect());
        let rect = Rect::new(0.0, 0.0, W, H);
        let item_height =
            crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT as f64) as f32;
        let expected = tree
            .vscrollbar_with_track_width(
                rect,
                item_height,
                crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH,
            )
            .expect("40 rows in this viewport must need a v-scrollbar");

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                    &Style::default(),
                );
            })
            .expect("paint tree");

        let theme = Theme::default();
        let probe_x = (expected.track.x + expected.track.width / 2.0) as u32;
        let probe_y = (expected.track.y + expected.track.height / 2.0) as u32;
        assert!(
            probe_x < W as u32 && probe_y < H as u32,
            "probe ({probe_x}, {probe_y}) fell outside the {W}x{H} surface — \
             the v-scrollbar track geometry is off-rect"
        );
        let px = surface.pixel_at(probe_x, probe_y);
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "expected the v-scrollbar track at ({probe_x}, {probe_y}) to be painted \
             (non-background)"
        );
    }

    /// A tree whose rows all fit the viewport must paint no v-scrollbar
    /// track at all — `TreeView::vscrollbar_with_track_width` returns
    /// `None` for a non-overflowing tree, and the shared painter skips
    /// the fill entirely in that case (see `native_surface_paint::paint`'s
    /// doc). Probes the full column the track would occupy if painting
    /// one unconditionally, not just the single row's own bounds.
    #[test]
    fn no_scrollbar_track_painted_when_content_fits() {
        let tree = make_tree(vec![leaf(0, "only-row")]);
        let rect = Rect::new(0.0, 0.0, W, H);

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                    &Style::default(),
                );
            })
            .expect("paint tree");

        let theme = Theme::default();
        // Well below the single row, inside the column a v-scrollbar
        // track would occupy if painted unconditionally.
        let probe_x = (rect.x + rect.width - 2.0) as u32;
        let probe_y = (rect.y + rect.height / 2.0) as u32;
        let px = surface.pixel_at(probe_x, probe_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "a tree whose single row fits the viewport must not paint a \
             v-scrollbar track at ({probe_x}, {probe_y})"
        );
    }

    /// Reference alpha blend, independent of production code, for the
    /// expected pixel colour in
    /// `scrollbar_track_width_follows_style_override` — mirrors
    /// `win::scrollbar`'s own `expected_alpha_blend` helper.
    fn expected_alpha_blend(base: Color, over: Color, alpha: f32) -> Color {
        let mix =
            |b: u8, o: u8| -> u8 { (b as f32 * (1.0 - alpha) + o as f32 * alpha).round() as u8 };
        Color::rgb(
            mix(base.r, over.r),
            mix(base.g, over.g),
            mix(base.b, over.b),
        )
    }

    /// The painted v-scrollbar track follows a host-set
    /// `Style::scrollbar_width`, not the fixed `TREE_VSCROLLBAR_WIDTH`
    /// literal — mirrors `primitives::tree`'s own
    /// `vertical_scrollbar_track_width_follows_style_token` test, but
    /// exercises this backend's own `draw_tree` entry point. Probes just
    /// inside the widened track's leading edge — a region the fixed
    /// 10px-wide track would have left as plain background — against the
    /// exact expected blend (`tab_bar_bg` at 20% toward `scrollbar_track`,
    /// the not-hovered/not-dragging `track_alpha` `native_surface_paint`
    /// uses), so a future paint that merely touches this pixel without
    /// being the real track (a hover tint, a long row label) can't
    /// satisfy the assertion vacuously.
    #[test]
    fn scrollbar_track_width_follows_style_override() {
        let tree = make_tree((0..40).map(|i| leaf(i, &format!("row-{i}"))).collect());
        let rect = Rect::new(0.0, 0.0, W, H);
        let item_height =
            crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT as f64) as f32;
        let style = Style::default().with_scrollbar_width(24.0);
        let expected = tree
            .vscrollbar_with_track_width(rect, item_height, style.scrollbar_width)
            .expect("40 rows in this viewport must need a v-scrollbar");

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                    &style,
                );
            })
            .expect("paint tree");

        let theme = Theme::default();
        let probe_x = (expected.track.x + 2.0) as u32;
        let probe_y = (expected.track.y + expected.track.height / 2.0) as u32;
        assert!(
            probe_x < W as u32 && probe_y < H as u32,
            "probe ({probe_x}, {probe_y}) fell outside the {W}x{H} surface — \
             the overridden-width v-scrollbar track geometry is off-rect"
        );
        let px = surface.pixel_at(probe_x, probe_y);
        // Not hovered/dragging: track_alpha = 0.20 (see the shared `paint`).
        let expected_color = expected_alpha_blend(theme.tab_bar_bg, theme.scrollbar_track, 0.20);
        // Direct2D's blend quantises to 8-bit channels with its own
        // rounding, so a channel may land one level off the f32 reference
        // (e.g. 26.4 → 27). One level is far inside the 4–5 level gap
        // between the blended track and plain `tab_bar_bg`, so the probe
        // still distinguishes "track" from "background" unambiguously.
        const BLEND_TOLERANCE: i32 = 1;
        let within = |got: u8, want: u8| (got as i32 - want as i32).abs() <= BLEND_TOLERANCE;
        assert!(
            within(px.r, expected_color.r)
                && within(px.g, expected_color.g)
                && within(px.b, expected_color.b),
            "expected the 24px-wide v-scrollbar track's leading edge at \
             ({probe_x}, {probe_y}) to be painted the track colour blended \
             over tab_bar_bg (~{:?}), got {:?} — a fixed 10px-wide track \
             would have left this region as plain background instead",
            (expected_color.r, expected_color.g, expected_color.b),
            (px.r, px.g, px.b),
        );
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "the probe at ({probe_x}, {probe_y}) is plain tab_bar_bg — no \
             v-scrollbar track was painted there"
        );
    }

    /// `TreeController::render` must not double-paint the v-scrollbar:
    /// `WinBackend` reports `tree_paints_own_scrollbar() == true`, meaning
    /// its `draw_tree` already paints the tree's overlay scrollbar over
    /// the tree's own content, so `TreeController::render` passes the
    /// full rect to `draw_tree` and skips a separate `draw_scrollbar`
    /// call. Rendering the same rows through `TreeController::render` and
    /// through a bare `Backend::draw_tree` call must produce
    /// pixel-identical frames; any stray pixel would be an extra,
    /// separately painted scrollbar. Mirrors
    /// `macos::tree::tree_controller_render_does_not_double_paint_scrollbar`.
    /// Uses a real `attach_headless` Direct2D handle — this whole file is
    /// already `#[cfg(target_os = "windows")]`-gated at the `mod tree;`
    /// declaration in `win::mod`, so every test here is Windows-only
    /// regardless of any per-test attribute.
    #[test]
    fn tree_controller_render_does_not_double_paint_scrollbar() {
        use crate::event::Viewport;
        use crate::win::backend::WinBackend;
        use crate::Backend;

        const PW: u32 = 200;
        const PH: u32 = 200;

        let rows: Vec<TreeRow> = (0..40).map(|i| leaf(i, &format!("row{i}"))).collect();
        let rect = Rect::new(0.0, 0.0, PW as f32, PH as f32);

        let reference_surface = HeadlessSurface::new(PW, PH).expect("create surface");
        let mut reference = WinBackend::new();
        reference
            .attach_headless(reference_surface.target().clone(), PW, PH)
            .expect("attach headless surface");
        let tree = make_tree(rows.clone());
        reference.begin_frame(Viewport::new(PW as f32, PH as f32, 1.0));
        reference.draw_tree(rect, &tree);
        reference.end_frame();

        let surface = HeadlessSurface::new(PW, PH).expect("create surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), PW, PH)
            .expect("attach headless surface");
        let mut controller = crate::compose::TreeController::new("tree");
        controller.set_rows(rows);
        backend.begin_frame(Viewport::new(PW as f32, PH as f32, 1.0));
        assert!(
            backend.tree_paints_own_scrollbar(),
            "Win owns the tree scrollbar paint"
        );
        controller.render(&mut backend, rect);
        backend.end_frame();

        for y in 0..PH {
            for x in 0..PW {
                assert_eq!(
                    surface.pixel_at(x, y),
                    reference_surface.pixel_at(x, y),
                    "pixel ({x}, {y}) differs between TreeController::render and a \
                     draw_tree-only reference frame — TreeController painted \
                     something extra, i.e. a second scrollbar"
                );
            }
        }
    }

    /// [`win_tree_layout_measured`] must size the chevron end-x from the
    /// real DirectWrite glyph width of the glyph paint actually draws
    /// (the codicon substitute for the default style, an app override
    /// verbatim otherwise — see `tree::tree_chevron_glyph`), not
    /// [`win_tree_layout`]'s `line_height * 0.65` estimate — and the
    /// resulting boundary must still resolve to `TreeViewHit::Chevron`.
    #[test]
    fn win_tree_layout_measured_uses_real_chevron_glyph_width_not_the_estimate() {
        let tree = make_tree(vec![branch(0, "alpha", true)]);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");

        let estimated = win_tree_layout(&tree, rect, LINE_HEIGHT, 8.0);
        let measured = win_tree_layout_measured(&dwrite, rect, &tree, LINE_HEIGHT, 8.0);

        // The Chevron hit-region's width *is* the row's chevron_end_x
        // (see `TreeView::layout`'s hit-region split).
        let est_x = estimated.hit_regions[0].0.width;
        let measured_x = measured.hit_regions[0].0.width;
        assert_ne!(
            est_x, measured_x,
            "measured chevron end-x must use the real DirectWrite glyph width \
             ({measured_x}), not the line_height * 0.65 estimate ({est_x})",
        );

        // The default `chevron_expanded` is swapped for the codicon
        // `CHEVRON_DOWN` glyph before painting/measuring (see
        // `tree::tree_chevron_glyph`), so that is what must be measured here.
        let glyph_w = dwrite.width_of(&crate::primitives::tree::tree_chevron_glyph(
            &tree.style,
            true,
        ));
        let indent_px = (LINE_HEIGHT * 0.9).round();
        // `row_padding_x` left margin (8.0, passed above) + indent + glyph + 4px gap.
        let expected = 8.0 + tree.rows[0].indent as f32 * indent_px + glyph_w + 4.0;
        assert!(
            (measured_x - expected).abs() < 0.01,
            "measured chevron end-x {measured_x} must equal the DirectWrite-measured \
             geometry {expected} (glyph advance {glyph_w})",
        );
        assert_eq!(
            measured.hit_test(measured_x - 1.0, 4.0),
            TreeViewHit::Chevron(0),
            "the measured boundary itself must still resolve to the chevron hit region",
        );
    }

    /// An app-supplied `TreeStyle` chevron override is painted verbatim;
    /// leaving `TreeStyle::default()`'s `"▾"`/`"▸"` paints the sharper
    /// codicon substitute instead — see `draw_tree`'s "Chevrons" doc and
    /// `tree::tree_chevron_glyph`.
    #[test]
    fn chevron_override_paints_app_glyph_default_paints_codicon() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, W, H);

        let mut overridden = make_tree(vec![branch(0, "src", true)]);
        overridden.style.chevron_expanded = "V".to_string();

        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let previous = crate::testing::install_text_run_sink();
        surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &overridden,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                    &Style::default(),
                );
            })
            .expect("paint with chevron override");
        let runs = crate::testing::take_text_run_sink(previous);
        assert!(
            runs.iter().any(|r| r.text == "V"),
            "expected the overridden chevron glyph among painted runs, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
        assert!(!runs
            .iter()
            .any(|r| r.text.contains(crate::codicon::CHEVRON_DOWN)));

        let default_tree = make_tree(vec![branch(0, "src", true)]);
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let previous = crate::testing::install_text_run_sink();
        surface
            .paint(|target| {
                draw_tree(
                    target,
                    &dwrite,
                    rect,
                    &default_tree,
                    LINE_HEIGHT,
                    false,
                    &Theme::default(),
                    &Style::default(),
                );
            })
            .expect("paint with default style");
        let runs = crate::testing::take_text_run_sink(previous);
        assert!(
            runs.iter()
                .any(|r| r.text.contains(crate::codicon::CHEVRON_DOWN)),
            "expected the default codicon chevron glyph among painted runs, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }
}

/// Multi-select click wiring over [`crate::win::testing::WinDriver`].
///
/// `TreeView` stays purely declarative (no selection-set field of its
/// own — see `crate::types::apply_selection_gesture`'s doc), so the app
/// owns the selected-index set and the gesture anchor, resolves each
/// `MouseDown` to a flattened row via `TreeViewLayout::hit_test`, derives
/// the gesture from the event's `Modifiers`, and maps the resulting
/// indices back to `TreePath`s via
/// [`crate::primitives::tree::selected_paths_for_indices`] before
/// emitting `TreeEvent::SelectionSet`. Mirrors `gtk::tree`'s
/// `multi_select_driver_tests`, except `WinDriver` has no
/// `set_double_click_folding` toggle, so consecutive clicks that must stay
/// independent are spaced more than `WIN_DOUBLE_CLICK_RADIUS` DIPs apart
/// on the x axis — row hit testing only depends on y, and these leaf rows
/// have no chevron, so an x offset never lands in a chevron zone.
///
/// This module's own file is already `#[cfg(target_os = "windows")]`-gated
/// at the `mod tree;` declaration in `win::mod`, so `#[cfg(test)]` alone
/// is enough here — unlike `win::list`/`win::data_table`, whose modules
/// compile everywhere and need the extra `target_os` gate on this block.
#[cfg(test)]
mod multi_select_driver_tests {
    use super::super::testing::WinDriver;
    use crate::primitives::tree::{selected_paths_for_indices, TreeEvent, TreeRow, TreeRowMeasure};
    use crate::runner::{AppLogic, Reaction};
    use crate::{
        apply_selection_gesture, Backend, Decoration, Modifiers, MouseButton, Point, Rect,
        SelectionGesture, SelectionMode, StyledText, TreeView, TreeViewHit, UiEvent, WidgetId,
    };

    const VIEWPORT_W: f32 = 40.0;
    const VIEWPORT_H: f32 = 10.0;
    const ROW_H: f32 = 1.0;

    fn leaf(idx: u16, label: &str) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label.to_string()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn sample_tree() -> TreeView {
        TreeView {
            id: WidgetId::new("t"),
            rows: (0..5).map(|i| leaf(i, &format!("row {i}"))).collect(),
            selection_mode: SelectionMode::Multi,
            selected_path: None,
            scroll_offset: 0,
            style: Default::default(),
            has_focus: true,
        }
    }

    struct TreeApp {
        tree: TreeView,
        selected: Vec<usize>,
        anchor: usize,
        events: Vec<TreeEvent>,
    }

    impl TreeApp {
        fn new() -> Self {
            Self {
                tree: sample_tree(),
                selected: Vec::new(),
                anchor: 0,
                events: Vec::new(),
            }
        }
    }

    impl AppLogic for TreeApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_tree(Rect::new(0.0, 0.0, VIEWPORT_W, VIEWPORT_H), &self.tree);
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            let UiEvent::MouseDown {
                position,
                modifiers,
                ..
            } = event
            else {
                return Reaction::Continue;
            };
            let layout = self
                .tree
                .layout(VIEWPORT_W, VIEWPORT_H, |_| TreeRowMeasure::new(ROW_H));
            let hit = layout.hit_test(position.x, position.y);
            let idx = match hit {
                TreeViewHit::Row(idx) | TreeViewHit::Chevron(idx) => idx,
                TreeViewHit::Empty => return Reaction::Continue,
            };
            let gesture = SelectionGesture::from_modifiers(modifiers);
            let indices = apply_selection_gesture(&self.selected, self.anchor, idx, gesture);
            self.selected = indices.clone();
            if !matches!(gesture, SelectionGesture::Range) {
                self.anchor = idx;
            }
            let paths = selected_paths_for_indices(&self.tree.rows, &indices);
            self.events.push(TreeEvent::SelectionSet { paths });
            Reaction::Redraw
        }
    }

    fn mouse_down(x: f32, y: f32, modifiers: Modifiers) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers,
        }
    }

    #[test]
    fn plain_click_replaces_selection() {
        let mut driver = WinDriver::new(TreeApp::new(), 80, 40);
        driver.dispatch(mouse_down(2.0, 2.0, Modifiers::default()));
        assert_eq!(driver.app().selected, vec![2]);
    }

    #[test]
    fn ctrl_click_toggles_rows_into_a_multi_selection() {
        let mut driver = WinDriver::new(TreeApp::new(), 80, 40);
        driver.dispatch(mouse_down(2.0, 0.0, Modifiers::default()));
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        // x is spaced > WIN_DOUBLE_CLICK_RADIUS DIPs from the previous
        // click so the shared `DoubleClickDetector` doesn't fold these
        // back-to-back single clicks into a `DoubleClick`; y alone picks
        // the row.
        driver.dispatch(mouse_down(10.0, 2.0, ctrl));
        assert_eq!(driver.app().selected, vec![0, 2]);
        // Ctrl-click the same row again toggles it back off.
        driver.dispatch(mouse_down(18.0, 2.0, ctrl));
        assert_eq!(driver.app().selected, vec![0]);
        assert_eq!(
            driver.app().events.last(),
            Some(&TreeEvent::SelectionSet {
                paths: vec![vec![0]]
            })
        );
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor() {
        let mut driver = WinDriver::new(TreeApp::new(), 80, 40);
        driver.dispatch(mouse_down(2.0, 0.0, Modifiers::default()));
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        driver.dispatch(mouse_down(10.0, 3.0, shift));
        assert_eq!(driver.app().selected, vec![0, 1, 2, 3]);
        assert_eq!(
            driver.app().events.last(),
            Some(&TreeEvent::SelectionSet {
                paths: vec![vec![0], vec![1], vec![2], vec![3]]
            })
        );
    }

    #[test]
    fn cmd_click_toggles_the_same_as_ctrl_click() {
        let mut driver = WinDriver::new(TreeApp::new(), 80, 40);
        driver.dispatch(mouse_down(2.0, 1.0, Modifiers::default()));
        driver.dispatch(mouse_down(
            10.0,
            4.0,
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        ));
        assert_eq!(driver.app().selected, vec![1, 4]);
    }
}
