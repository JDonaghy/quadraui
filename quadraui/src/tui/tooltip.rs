//! TUI rasteriser for [`crate::Tooltip`].
//!
//! Border chrome is chosen by the [`TooltipChrome`] argument (#541 —
//! [`crate::TooltipBorder`]), not hardcoded here — see
//! `primitives::tooltip`'s module doc for why the vocabulary is a
//! sidecar value rather than a field on [`Tooltip`] or [`TooltipLayout`].
//! [`draw_tooltip`] keeps its pre-#541 signature and renders
//! `TooltipChrome::default()`; [`draw_tooltip_with_chrome`] takes the
//! request explicitly:
//!
//! - [`TooltipBorder::Full`] (the default) renders a closed box —
//!   `┌─┐`/`└─┘` top and bottom rows plus `│` sides — whenever the
//!   measured height leaves room for both border rows and at least one
//!   content row (`height >= 3`, `width >= 2`). This matches
//!   `quadraui::gtk::draw_tooltip`, which has always stroked a full
//!   rectangle. Below that room threshold, `Full` falls back to the
//!   `Sides` rendering rather than an empty box — a rendering detail of
//!   this variant, not a mode a consumer selects. An optional
//!   `chrome.title` is centred into the top row when the box closes.
//! - [`TooltipBorder::Sides`] always renders `│` on the first/last column
//!   only, regardless of height — the pre-#542 TUI look, now available
//!   by explicit request rather than as the only option. No title (no
//!   top rule to embed it in).
//! - [`TooltipBorder::None`] renders no border chrome at all.
//!
//! Before #541 gave backends an explicit vocabulary, the choice was
//! hardcoded per backend: TUI painted side bars only, GTK stroked a full
//! box, so the same primitive produced materially different chrome while
//! `screen_has("Keybindings")` stayed `true` on both — the divergence
//! #542's structural-parity tier exists to catch (see that tier's tests
//! for the historical case, still pinned there against `Full`, today's
//! default).
//!
//! When `tooltip.styled_lines` is `Some`, each entry renders as one
//! row of styled spans (multi-line styled path used by signature help
//! and diff peek). Otherwise `tooltip.text` is split on `\n` and each
//! line is rendered plain (LSP hover popup path). Lines that exceed the
//! box width or the available content rows are truncated.
//!
//! `layout.bounds.height` is the *total* box height, not a content-row
//! count — see the contract note on [`crate::TooltipMeasure`]. Callers
//! that want `N` content lines visible once a bordered box is drawn
//! (`height >= 3`) must measure `N + 2`, or their last two lines are
//! silently dropped by [`draw_tooltip`]'s `.take(content_rows)`.

use ratatui::buffer::Buffer;

use super::{ratatui_color, set_cell};
use crate::event::Rect as QRect;
use crate::primitives::tooltip::{Tooltip, TooltipBorder, TooltipChrome, TooltipLayout};
use crate::theme::Theme;
use crate::types::Color;

fn qc(c: Color) -> ratatui::style::Color {
    ratatui_color(c)
}

/// The whole-cell bounds [`draw_tooltip`] actually paints into —
/// `layout.bounds` rounded to integer terminal columns/rows exactly the
/// way `draw_tooltip` does internally.
///
/// Backends must register *this* (not the raw float `layout.bounds`) as
/// the tooltip's zone via `Backend::register_zone`. Registering the
/// unrounded bounds would let a structural-parity observer see a surface
/// that doesn't match the cells actually painted — a latent precision
/// mismatch (#542 review) that today's 0.35 ratio tolerance in the
/// structural-parity acceptance slice happens to absorb, but which should
/// stay aligned regardless.
pub fn painted_bounds(layout: &TooltipLayout) -> QRect {
    QRect::new(
        layout.bounds.x.round(),
        layout.bounds.y.round(),
        layout.bounds.width.round(),
        layout.bounds.height.round(),
    )
}

/// Draw a [`Tooltip`] into `layout.bounds` on `buf` with the default
/// chrome — a [`TooltipBorder::Full`] box, no title, i.e. exactly what
/// this rasteriser drew before #541 added a choice.
///
/// Per-tooltip `tooltip.fg` / `tooltip.bg` overrides win over the
/// theme defaults. The frame border always uses [`Theme::hover_border`].
///
/// To ask for different chrome, call [`draw_tooltip_with_chrome`].
pub fn draw_tooltip(buf: &mut Buffer, tooltip: &Tooltip, layout: &TooltipLayout, theme: &Theme) {
    draw_tooltip_with_chrome(buf, tooltip, layout, &TooltipChrome::default(), theme);
}

/// Draw a [`Tooltip`] into `layout.bounds` on `buf`, with the border and
/// optional title requested by `chrome` (#541).
///
/// Per-tooltip `tooltip.fg` / `tooltip.bg` overrides win over the
/// theme defaults. The frame border always uses [`Theme::hover_border`].
pub fn draw_tooltip_with_chrome(
    buf: &mut Buffer,
    tooltip: &Tooltip,
    layout: &TooltipLayout,
    chrome: &TooltipChrome,
    theme: &Theme,
) {
    let painted = painted_bounds(layout);
    let x = painted.x as u16;
    let y = painted.y as u16;
    let w = painted.width as u16;
    let h = painted.height as u16;
    if w == 0 || h == 0 {
        return;
    }

    let fg = tooltip
        .fg
        .map(qc)
        .unwrap_or_else(|| ratatui_color(theme.hover_fg));
    let bg = tooltip
        .bg
        .map(qc)
        .unwrap_or_else(|| ratatui_color(theme.hover_bg));
    let border = ratatui_color(theme.hover_border);

    // `Sides` never draws top/bottom, regardless of height. `Full` draws
    // a closed box whenever there's room for both border rows and at
    // least one content row (`h - 2` for content once both are paid
    // for); below that there's no room for both rows and any content, so
    // `Full` falls back to the same side-bars-only chrome `Sides` always
    // uses rather than rendering an empty box. `w >= 2` is a separate,
    // narrower guard: `paint_border_row`'s `col == 0` and `col == w - 1`
    // branches pick the left/right corner glyph, and at `w < 2` those two
    // indices collide (`w - 1 == 0`), so the `col == 0` arm would win and
    // every corner would render as a left corner.
    let has_horizontal_border = matches!(chrome.border, TooltipBorder::Full) && h >= 3 && w >= 2;
    // `None` draws no chrome at all — not even the side bars `Sides` and
    // (as a fallback) `Full` paint.
    let draw_side_bars = !matches!(chrome.border, TooltipBorder::None);
    let content_row0: u16 = if has_horizontal_border { 1 } else { 0 };
    let content_rows: u16 = if has_horizontal_border { h - 2 } else { h };
    // Content starts one cell past the side border when one is drawn
    // (border column + 1 pad), or just one cell of padding when it isn't.
    let text_col_offset: u16 = if draw_side_bars { 2 } else { 1 };

    if has_horizontal_border {
        // `title`, framed with a single space either side, centred among
        // the interior columns (excluding the two corners). A title that
        // doesn't fit the interior width is dropped rather than clipped —
        // a truncated title reads as a rendering bug, plain dashes don't.
        let paint_border_row = |buf: &mut Buffer, row: u16, top: bool, title: Option<&str>| {
            let interior = w.saturating_sub(2);
            let framed_title = title.and_then(|t| {
                let framed = format!(" {t} ");
                let fits = framed.chars().count() as u16 <= interior;
                fits.then_some(framed)
            });
            let title_chars: Vec<char> = framed_title
                .as_deref()
                .map(|s| s.chars().collect())
                .unwrap_or_default();
            let title_len = title_chars.len() as u16;
            // Integer divide rounds down, so when `interior - title_len` is
            // odd the title sits one column left of dead-centre (the extra
            // column goes to the right). Deliberate and stable rather than
            // arbitrary: cells are indivisible, so *some* side gets the odd
            // column, and biasing left matches how the rest of this crate
            // centres odd remainders (`tui::panel`'s title bar, `tui::
            // dialog`'s buttons). Flagged as a cosmetic nit in the #541
            // review; documented rather than "fixed", since rounding the
            // other way is equally off-centre.
            let title_start = 1 + interior.saturating_sub(title_len) / 2;

            for col in 0..w {
                if title_len > 0 && col >= title_start && col < title_start + title_len {
                    set_cell(
                        buf,
                        x + col,
                        row,
                        title_chars[(col - title_start) as usize],
                        fg,
                        bg,
                    );
                    continue;
                }
                let ch = if col == 0 {
                    if top {
                        '┌'
                    } else {
                        '└'
                    }
                } else if col == w - 1 {
                    if top {
                        '┐'
                    } else {
                        '┘'
                    }
                } else {
                    '─'
                };
                set_cell(buf, x + col, row, ch, border, bg);
            }
        };
        paint_border_row(buf, y, true, chrome.title.as_deref());
        paint_border_row(buf, y + h - 1, false, None);
    }

    let paint_row_background = |buf: &mut Buffer, row: u16| {
        for col in 0..w {
            let is_border_col = draw_side_bars && (col == 0 || col == w - 1);
            let ch = if is_border_col { '│' } else { ' ' };
            let cell_fg = if is_border_col { border } else { fg };
            set_cell(buf, x + col, row, ch, cell_fg, bg);
        }
    };

    if let Some(ref styled_lines) = tooltip.styled_lines {
        for (i, styled) in styled_lines.iter().enumerate().take(content_rows as usize) {
            let row = y + content_row0 + i as u16;
            paint_row_background(buf, row);
            let mut col_off: u16 = text_col_offset;
            for span in &styled.spans {
                let span_fg = span.fg.map(qc).unwrap_or(fg);
                let span_bg = span.bg.map(qc).unwrap_or(bg);
                for ch in span.text.chars() {
                    let col = x + col_off;
                    if col + 1 >= x + w {
                        break;
                    }
                    set_cell(buf, col, row, ch, span_fg, span_bg);
                    col_off += 1;
                }
            }
        }
        return;
    }

    let lines: Vec<&str> = tooltip.text.lines().collect();
    for (i, text_line) in lines.iter().enumerate().take(content_rows as usize) {
        let row = y + content_row0 + i as u16;
        paint_row_background(buf, row);
        for (j, ch) in text_line.chars().enumerate() {
            let col = x + text_col_offset + j as u16;
            if col + 1 >= x + w {
                break;
            }
            set_cell(buf, col, row, ch, fg, bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::tooltip::{ResolvedPlacement, Tooltip, TooltipLayout};
    use crate::types::{StyledSpan, StyledText, WidgetId};
    use ratatui::layout::Rect;

    fn make_layout(x: f32, y: f32, w: f32, h: f32) -> TooltipLayout {
        TooltipLayout {
            bounds: QRect::new(x, y, w, h),
            resolved_placement: ResolvedPlacement::Bottom,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    /// #542 review (non-blocking): the zone a backend registers for a
    /// tooltip must match the cells `draw_tooltip` actually painted, not
    /// the raw float layout bounds — otherwise a structural-parity
    /// observer sees a surface offset from what's really on screen.
    #[test]
    fn painted_bounds_rounds_like_draw_tooltip_does() {
        let layout = make_layout(0.4, 1.6, 9.6, 2.5);
        let painted = painted_bounds(&layout);
        assert_eq!(painted, QRect::new(0.0, 2.0, 10.0, 3.0));
    }

    #[test]
    fn paints_side_borders_and_plain_text() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = Tooltip {
            id: WidgetId::new("hover"),
            text: "hello".into(),
            styled_lines: None,
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        };
        let layout = make_layout(0.0, 0.0, 10.0, 1.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

        // Borders at col 0 and col 9.
        assert_eq!(cell_char(&buf, 0, 0), '│');
        assert_eq!(cell_char(&buf, 9, 0), '│');
        // Text starts at col 2.
        let row: String = (2..7).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(row, "hello");
    }

    /// #541/#542: once there's room for both border rows (`height >= 3`),
    /// the box must be closed on every side — not just left/right — so a
    /// structural-parity observer sees the same chrome shape GTK has
    /// always drawn.
    #[test]
    fn paints_a_full_box_when_height_allows_it() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = Tooltip {
            id: WidgetId::new("hover"),
            text: "hello".into(),
            styled_lines: None,
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        };
        let layout = make_layout(0.0, 0.0, 10.0, 3.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

        // Top row: corners + a solid horizontal rule.
        assert_eq!(cell_char(&buf, 0, 0), '┌');
        assert_eq!(cell_char(&buf, 9, 0), '┐');
        assert_eq!(cell_char(&buf, 4, 0), '─');
        // Bottom row: corners + a solid horizontal rule.
        assert_eq!(cell_char(&buf, 0, 2), '└');
        assert_eq!(cell_char(&buf, 9, 2), '┘');
        assert_eq!(cell_char(&buf, 4, 2), '─');
        // Content moved down one row, side borders unchanged.
        assert_eq!(cell_char(&buf, 0, 1), '│');
        assert_eq!(cell_char(&buf, 9, 1), '│');
        let row: String = (2..7).map(|x| cell_char(&buf, x, 1)).collect();
        assert_eq!(row, "hello");
    }

    #[test]
    fn styled_lines_paint_each_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = Tooltip {
            id: WidgetId::new("sig"),
            text: String::new(),
            styled_lines: Some(vec![
                StyledText {
                    spans: vec![StyledSpan::plain("line1")],
                },
                StyledText {
                    spans: vec![StyledSpan::plain("line2")],
                },
            ]),
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        };
        let layout = make_layout(0.0, 0.0, 12.0, 2.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());

        let r0: String = (2..7).map(|x| cell_char(&buf, x, 0)).collect();
        let r1: String = (2..7).map(|x| cell_char(&buf, x, 1)).collect();
        assert_eq!(r0, "line1");
        assert_eq!(r1, "line2");
    }

    #[test]
    fn per_tooltip_bg_overrides_theme() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = Tooltip {
            id: WidgetId::new("hover"),
            text: "x".into(),
            styled_lines: None,
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: Some(Color::rgb(100, 0, 0)),
        };
        let layout = make_layout(0.0, 0.0, 10.0, 1.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());
        // Cell 2 should have bg = (100, 0, 0).
        let bg = buf[(2u16, 0u16)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(100, 0, 0));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let tt = Tooltip {
            id: WidgetId::new("hover"),
            text: "x".into(),
            styled_lines: None,
            placement: crate::primitives::tooltip::TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        };
        let layout = make_layout(0.0, 0.0, 0.0, 1.0);
        draw_tooltip(&mut buf, &tt, &layout, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // ── #541: explicit border vocabulary ─────────────────────────────────

    fn tooltip_with() -> Tooltip {
        Tooltip::new(WidgetId::new("hover"), "hi")
    }

    /// `make_layout` paired with the [`TooltipChrome`] (#541) a test
    /// wants — the in-crate equivalent of `tooltip.layout(...)` plus a
    /// `TooltipChrome::new(..).with_title(..)` sidecar.
    fn layout_with(
        w: f32,
        h: f32,
        border: TooltipBorder,
        title: Option<&str>,
    ) -> (TooltipLayout, TooltipChrome) {
        let mut chrome = TooltipChrome::new(border);
        if let Some(t) = title {
            chrome = chrome.with_title(t);
        }
        (make_layout(0.0, 0.0, w, h), chrome)
    }

    /// `Sides` must never draw top/bottom rules, even at a height that
    /// would give `Full` plenty of room for a closed box — the whole
    /// point of separating the two is that a consumer can *ask* for the
    /// narrow look regardless of how tall its content is.
    #[test]
    fn sides_border_never_closes_even_when_height_allows_it() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
        // 5 lines so every one of the 5 rows the layout reserves actually
        // gets painted (a row with no corresponding text line is left
        // untouched by `draw_tooltip`, which would make an empty row's
        // "no border here" look identical to a genuinely-suppressed one).
        let mut tt = tooltip_with();
        tt.text = "hi\nhi\nhi\nhi\nhi".into();
        let (layout, chrome) = layout_with(10.0, 5.0, TooltipBorder::Sides, None);
        draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

        for row in 0..5u16 {
            assert_eq!(
                cell_char(&buf, 0, row),
                '│',
                "row {row}: Sides must paint the left bar on every row"
            );
            assert_eq!(
                cell_char(&buf, 9, row),
                '│',
                "row {row}: Sides must paint the right bar on every row"
            );
        }
        // No corners anywhere — a top/bottom rule would introduce one.
        for row in 0..5u16 {
            for col in 0..10u16 {
                let ch = cell_char(&buf, col, row);
                assert!(
                    !['┌', '┐', '└', '┘', '─'].contains(&ch),
                    "Sides must never paint box-drawing corner/rule glyphs \
                     (found {ch:?} at ({col}, {row}))"
                );
            }
        }
    }

    /// `None` paints no border chrome at all — not even the side bars
    /// `Sides` (and `Full`'s too-short fallback) always paint.
    #[test]
    fn none_border_paints_no_chrome() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
        let tt = tooltip_with();
        let (layout, chrome) = layout_with(10.0, 3.0, TooltipBorder::None, None);
        draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

        for row in 0..3u16 {
            for col in 0..10u16 {
                let ch = cell_char(&buf, col, row);
                assert!(
                    !['┌', '┐', '└', '┘', '─', '│'].contains(&ch),
                    "None must paint no border glyph at all (found {ch:?} at ({col}, {row}))"
                );
            }
        }
        // Content starts one column earlier than `Full`/`Sides` (padding
        // only, no border column to clear first).
        let row: String = (1..3).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(row, "hi");
    }

    /// #541 ask 2: an optional title is centred into the top border row
    /// when `Full` actually closes the box.
    #[test]
    fn full_border_centers_title_in_top_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = tooltip_with();
        let (layout, chrome) = layout_with(12.0, 3.0, TooltipBorder::default(), Some("Hi"));
        draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

        // Interior columns are 1..=10 (corners at 0 and 11); " Hi " (4
        // chars) centred among 10 interior columns starts at col 1 + 3 = 4.
        let top_row: String = (0..12).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(top_row, "┌─── Hi ───┐");
        // Bottom row carries no title — plain rule.
        let bottom_row: String = (0..12).map(|x| cell_char(&buf, x, 2)).collect();
        assert_eq!(bottom_row, "└──────────┘");
    }

    /// A title that doesn't fit the interior width is dropped rather than
    /// clipped mid-glyph.
    #[test]
    fn oversized_title_is_dropped_not_truncated() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = tooltip_with();
        let (layout, chrome) = layout_with(
            12.0,
            3.0,
            TooltipBorder::default(),
            Some("This title is far too long for the box"),
        );
        draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

        let top_row: String = (0..12).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(
            top_row, "┌──────────┐",
            "an oversized title must fall back to a plain rule, not a clipped label"
        );
    }

    /// `title` has no top rule to embed into on `Sides` or `None`, so it
    /// must not leak into the content area either.
    #[test]
    fn title_is_ignored_when_border_is_not_full() {
        for border in [TooltipBorder::Sides, TooltipBorder::None] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
            let tt = tooltip_with();
            let (layout, chrome) = layout_with(12.0, 3.0, border, Some("Hi"));
            draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

            let screen: String = (0..3)
                .flat_map(|row| (0..12).map(move |col| (col, row)))
                .map(|(col, row)| cell_char(&buf, col, row))
                .collect();
            assert!(
                !screen.contains("Hi"),
                "{border:?}: title must not appear anywhere when there's no top rule to \
                 embed it in (screen contents: {screen:?})"
            );
        }
    }

    /// `Full`'s degrade-to-`Sides` fallback (too short for a box) must
    /// also drop the title — there's no top row to put it in once the
    /// box doesn't close.
    #[test]
    fn full_border_drops_title_when_too_short_to_close() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let tt = tooltip_with();
        let (layout, chrome) = layout_with(12.0, 1.0, TooltipBorder::default(), Some("Hi"));
        draw_tooltip_with_chrome(&mut buf, &tt, &layout, &chrome, &Theme::default());

        assert_eq!(cell_char(&buf, 0, 0), '│');
        let row: String = (0..12).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            !row.contains("Hi"),
            "a too-short box has no top row to embed the title in: {row:?}"
        );
    }
}
