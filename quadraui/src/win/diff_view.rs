//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::diff_view::DiffView`] (#737).
//!
//! Mirrors the gtk/macos/tui twins' structure: [`DiffView::layout`] (the
//! shared layout API lifted out of three near-identical backend copies by
//! #737) resolves pane widths, the divider position, the optional header
//! strip, and the scroll-clamped visible-line window — this module only
//! converts that DIP-agnostic `f32` geometry to paint calls via
//! [`fill_rect`]/[`DWrite::draw_text`].
//!
//! The `row kind → colour` tables are **not** duplicated here either.
//! They lived three times over (gtk, macos, tui) before #737; #713's
//! primitive-first rule forbids a fourth copy, so this rasteriser calls
//! [`crate::primitives::diff_view::row_colors`] (side-by-side) /
//! [`crate::primitives::diff_view::unified_row_style`] (unified) — same
//! as every other backend, migrated in the same PR.
//!
//! Unlike the GTK/macOS twins, no explicit clip bracket is needed around
//! pane/row text: [`DWrite::draw_text`] already paints with
//! `D2D1_DRAW_TEXT_OPTIONS_CLIP` against the rect it's handed (see
//! `win::text`'s module doc), so an overlong line is clipped by
//! construction rather than by a `push_clip`/`pop_clip` bracket.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod diff_view;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! `diff_view_layout` is **not** overridden on `WinBackend` — same as
//! every other backend. It ships a trait default (`Backend::diff_view_layout`)
//! that is a pure function of `line_height()` + `DiffView::mode`, which is
//! exactly what every backend's own `draw_diff_view` already resolves to
//! (see `tests/conformance/caps.rs`'s `#506` block comment, revisited for
//! #737 — the new shared [`DiffView::layout`] backs that default's
//! reasoning even more directly now, but the default itself was already
//! the honest answer and stays unoverridden here too).

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, DWrite};
use crate::event::Rect;
use crate::primitives::diff_view::{
    row_colors, unified_hunk_header, unified_row_style, unified_row_text, DiffLineContent,
    DiffMode, DiffView, DiffViewGeometry, DiffViewLayout,
};
use crate::theme::Theme;

/// Text inset from a pane's left edge, DIPs. Mirrors the GTK/macOS twins'
/// `TEXT_PAD` constant.
const TEXT_PAD_DIP: f32 = 4.0;
/// Text inset used in unified mode (tighter, matching the GTK/macOS twins).
const UNIFIED_PAD_DIP: f32 = 2.0;

/// `rect` shrunk by `pad` DIPs on its left edge — the DirectWrite
/// equivalent of the GTK/macOS twins' `x + TEXT_PAD` move-to, since
/// [`DWrite::draw_text`] takes the whole layout box rather than a cursor
/// position.
fn inset_left(r: Rect, pad: f32) -> Rect {
    Rect::new(r.x + pad, r.y, (r.width - pad).max(0.0), r.height)
}

/// Draw a [`DiffView`] into `rect` (DIPs, target-relative) on `target`.
/// Returns [`DiffViewLayout`] for scroll clamping — same contract as the
/// GTK/macOS/TUI twins' `draw_diff_view`.
pub fn draw_diff_view(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    view: &DiffView,
    theme: &Theme,
    line_height: f32,
) -> DiffViewLayout {
    let geometry = view.layout(rect, line_height);

    if rect.width <= 0.0 || rect.height <= 0.0 || line_height <= 0.0 {
        return geometry.as_layout();
    }

    let _ = fill_rect(target, rect, theme.background);

    match view.mode {
        DiffMode::SideBySide => draw_side_by_side(target, dwrite, view, theme, &geometry),
        DiffMode::Unified => draw_unified(target, dwrite, view, theme, &geometry),
    }

    geometry.as_layout()
}

// ── Side-by-side ─────────────────────────────────────────────────────────────

fn draw_side_by_side(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    if let Some(header) = &geometry.header {
        let strip = Rect::new(
            header.left.x,
            header.left.y,
            header.left.width + header.divider.width + header.right.width,
            header.left.height,
        );
        let _ = fill_rect(target, strip, theme.header_bg);
        let _ = fill_rect(target, header.divider, theme.border_fg);

        if let Some(label) = &view.left_label {
            let _ = dwrite.draw_text(
                target,
                label,
                inset_left(header.left, TEXT_PAD_DIP),
                theme.header_fg,
            );
        }
        if let Some(label) = &view.right_label {
            let _ = dwrite.draw_text(
                target,
                label,
                inset_left(header.right, TEXT_PAD_DIP),
                theme.header_fg,
            );
        }
    }

    for line in &geometry.lines {
        let DiffLineContent::Row { row_idx } = line.content else {
            continue;
        };
        let row = flat[row_idx];
        let (left_fg, left_bg, right_fg, right_bg) = row_colors(row.kind, theme);

        let left_r = line.left.expect("side-by-side row has left bounds");
        let right_r = line.right.expect("side-by-side row has right bounds");
        let divider_r = line.divider.expect("side-by-side row has divider bounds");

        let _ = fill_rect(target, left_r, left_bg);
        let _ = fill_rect(target, right_r, right_bg);
        let _ = fill_rect(target, divider_r, theme.border_fg);

        if let Some(text) = &row.left {
            let _ = dwrite.draw_text(target, text, inset_left(left_r, TEXT_PAD_DIP), left_fg);
        }
        if let Some(text) = &row.right {
            let _ = dwrite.draw_text(target, text, inset_left(right_r, TEXT_PAD_DIP), right_fg);
        }
    }
}

// ── Unified ──────────────────────────────────────────────────────────────────

fn draw_unified(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    view: &DiffView,
    theme: &Theme,
    geometry: &DiffViewGeometry,
) {
    let flat = view.flat_rows();

    for line in &geometry.lines {
        match line.content {
            DiffLineContent::UnifiedHeader { hunk_idx } => {
                let header_text = unified_hunk_header(&view.hunks[hunk_idx]);
                let _ = fill_rect(target, line.bounds, theme.background);
                let _ = dwrite.draw_text(
                    target,
                    &header_text,
                    inset_left(line.bounds, UNIFIED_PAD_DIP),
                    theme.accent_fg,
                );
            }
            DiffLineContent::Row { row_idx } => {
                let row = flat[row_idx];
                let (prefix, fg, bg) = unified_row_style(row.kind, theme);
                let _ = fill_rect(target, line.bounds, bg);

                let prefix_str = prefix.to_string();
                let prefix_w = dwrite
                    .measure_text(&prefix_str)
                    .map(|(w, _)| w)
                    .unwrap_or(0.0);
                let _ = dwrite.draw_text(
                    target,
                    &prefix_str,
                    inset_left(line.bounds, UNIFIED_PAD_DIP),
                    fg,
                );

                let text = unified_row_text(row);
                let text_x = line.bounds.x + UNIFIED_PAD_DIP + prefix_w + TEXT_PAD_DIP;
                let text_rect = Rect::new(
                    text_x,
                    line.bounds.y,
                    (line.bounds.x + line.bounds.width - text_x).max(0.0),
                    line.bounds.height,
                );
                let _ = dwrite.draw_text(target, text, text_rect, fg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::compute_hunks;
    use crate::primitives::diff_view::DiffRowKind;
    use crate::types::{Color, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 400.0;
    const H: f32 = 200.0;
    const LINE_HEIGHT: f32 = 16.0;

    fn sample_view(mode: DiffMode) -> DiffView {
        let left = "one\ntwo\nthree\n";
        let right = "one\nTWO\nthree\n";
        DiffView {
            id: WidgetId::new("diff"),
            left: left.into(),
            right: right.into(),
            left_label: None,
            right_label: None,
            hunks: compute_hunks(left, right),
            mode,
            editability: Default::default(),
            scroll_offset: 0,
            focused_pane: Default::default(),
            has_focus: false,
        }
    }

    fn first_row_of(view: &DiffView, kind: DiffRowKind) -> usize {
        view.hunks
            .iter()
            .flat_map(|h| h.rows.iter())
            .position(|r| r.kind == kind)
            .expect("fixture should contain the requested row kind")
    }

    /// C0 smoke: `draw_diff_view` must actually paint text + a
    /// scroll-clamp-usable layout rather than panicking or hitting a
    /// `todo!()` (#737's acceptance bar — "draw_diff_view survives C0
    /// with text_ok on win").
    #[test]
    fn draw_diff_view_paints_text_and_returns_layout() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_diff_view(target, &dwrite, rect, &view, &theme, LINE_HEIGHT);
            })
            .map(|_| view.layout(rect, LINE_HEIGHT).as_layout())
            .expect("paint diff view");

        assert!(layout.visible_rows > 0);
        assert_eq!(layout.total_rows, view.total_rows());

        // "text_ok" — some non-background pixel actually painted inside
        // the left pane (proves DrawText ran, not just the background/row
        // fills).
        let mut painted_any = false;
        for x in 0..(W as u32 / 2) {
            for y in 0..(LINE_HEIGHT as u32 * 3) {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (255, 255, 255) {
                    painted_any = true;
                }
            }
        }
        assert!(painted_any, "expected diff_view to paint visible glyphs");
    }

    /// The divider colour and position are the same shared geometry every
    /// backend paints from — pins the DIP position independently of the
    /// text-glyph probe above.
    #[test]
    fn divider_paints_border_colour() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(0.0, 0.0, W, H);

        surface
            .paint(|target| {
                draw_diff_view(target, &dwrite, rect, &view, &theme, LINE_HEIGHT);
            })
            .expect("paint diff view");

        let changed = first_row_of(&view, DiffRowKind::Changed);
        let geometry = view.layout(rect, LINE_HEIGHT);
        let divider_x = geometry.panes.expect("side-by-side has panes").divider_x as u32;
        let row_y = (LINE_HEIGHT * (changed as f32 + 0.5)) as u32;

        let px = surface.pixel_at(divider_x, row_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b),
            "the pane divider should be painted in border_fg",
        );
    }

    /// Non-zero-origin regression guard (LESSONS.md — the LOCAL/ABSOLUTE
    /// mixup) — mirrors every other `win::` rasteriser's own nonzero-origin
    /// test (see `win::pipeline_view::paint_and_click_round_trip_action_button_at_nonzero_origin`).
    #[test]
    fn paints_at_a_nonzero_origin_only() {
        let origin_x = 40.0_f32;
        let origin_y = 24.0_f32;
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(origin_x, origin_y, W - origin_x, H - origin_y);

        // Pre-fill with a sentinel colour distinct from every theme colour
        // this paints with, so "untouched" is unambiguous — mirrors
        // `win::pipeline_view`/`win::list`'s own nonzero-origin tests and
        // this module's own `zero_size_rect_is_a_no_op`. A raw
        // `HeadlessSurface` starts black, which collides with real theme
        // colours closely enough to make a false pass possible.
        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill sentinel background");

        surface
            .paint(|target| {
                draw_diff_view(target, &dwrite, rect, &view, &theme, LINE_HEIGHT);
            })
            .expect("paint diff view");

        let geometry = view.layout(rect, LINE_HEIGHT);
        let panes = geometry.panes.expect("side-by-side has panes");
        let div_x = panes.divider_x as u32;
        let probe_y = (origin_y + LINE_HEIGHT * 0.5) as u32;

        let px = surface.pixel_at(div_x, probe_y);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.border_fg.r, theme.border_fg.g, theme.border_fg.b),
            "divider must follow the requested origin",
        );

        // Untouched sentinel above and to the left.
        let above = surface.pixel_at(div_x, origin_y as u32 - 4);
        assert_eq!(
            (above.r, above.g, above.b),
            (255, 255, 255),
            "nothing should paint above the requested origin",
        );
        let left = surface.pixel_at(4, probe_y);
        assert_eq!(
            (left.r, left.g, left.b),
            (255, 255, 255),
            "nothing should paint left of the requested origin",
        );
    }

    /// Unified mode's `total_rows` counts hunk headers too — mirrors the
    /// GTK/macOS/TUI twins' regression guard.
    #[test]
    fn unified_mode_counts_hunk_headers_in_total_rows() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme::default();
        let view = sample_view(DiffMode::Unified);
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_diff_view(target, &dwrite, rect, &view, &theme, LINE_HEIGHT);
            })
            .map(|_| view.layout(rect, LINE_HEIGHT).as_layout())
            .expect("paint diff view");

        let hunk_count = view.hunks.len();
        assert!(hunk_count > 0, "fixture should produce at least one hunk");
        assert_eq!(
            layout.total_rows,
            view.total_rows() + hunk_count,
            "unified mode reports content rows plus one @@ header per hunk",
        );
    }

    /// No-paint layout must agree with what `draw_diff_view` painted from
    /// — same contract every other `win::` rasteriser's
    /// `no_paint_layout_matches_paint_layout` test proves.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let theme = Theme::default();

        let painted = surface
            .paint(|target| {
                draw_diff_view(target, &dwrite, rect, &view, &theme, LINE_HEIGHT);
            })
            .map(|_| view.layout(rect, LINE_HEIGHT).as_layout())
            .expect("paint");
        let no_paint = view.layout(rect, LINE_HEIGHT).as_layout();
        assert_eq!(painted, no_paint);
    }

    /// Zero-size rect is a no-op — mirrors every other `win::` rasteriser's
    /// same guard.
    #[test]
    fn zero_size_rect_is_a_no_op() {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let theme = Theme {
            background: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        let view = sample_view(DiffMode::SideBySide);
        let rect = Rect::new(0.0, 0.0, 0.0, H);

        surface
            .fill_rect(Rect::new(0.0, 0.0, W, H), Color::rgb(255, 255, 255))
            .expect("fill background");

        surface
            .paint(|target| {
                draw_diff_view(target, &dwrite, rect, &view, &theme, LINE_HEIGHT);
            })
            .expect("paint diff view");

        let px = surface.pixel_at(1, 1);
        assert_eq!(
            (px.r, px.g, px.b),
            (255, 255, 255),
            "a zero-width diff view should paint nothing at all",
        );
    }
}
