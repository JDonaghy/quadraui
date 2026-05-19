//! TUI rasteriser for [`crate::StatusBar`].
//!
//! Per D6: this function consumes a pre-computed
//! [`crate::StatusBarLayout`] (built by the caller via
//! [`crate::StatusBar::layout`] with its native cell-width measurer)
//! and paints the resolved `visible_segments` verbatim. No layout
//! decisions live here; any policy change (priority drop, gap rules,
//! …) belongs in the primitive.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

use super::{cell_width, ratatui_color, set_cell, set_cell_wide};
use crate::primitives::status_bar::{StatusBar, StatusBarLayout, StatusSegmentSide};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Draw a [`StatusBar`] into `area` on `buf`. Returns hit regions in
/// **bar-local cell coordinates** (relative to `area.x`) for each
/// segment carrying an `action_id`. Caller dispatches clicks against
/// the returned list.
///
/// The bar is filled with the first segment's `bg` (so the bar looks
/// continuous even when gaps exist between left and right halves).
/// When the bar has no segments, [`Theme::background`] is used as the
/// fill colour. Each visible segment's `text` is painted at the
/// `bounds.x` position the layout assigned, in the segment's `fg` /
/// `bg` colours, with `bold` honoured via [`Modifier::BOLD`].
///
/// # Arguments
///
/// - `buf`, `area` — ratatui buffer + bar rect (single row).
/// - `bar` — the primitive description.
/// - `layout` — the resolved layout, computed by the caller via
///   `bar.layout(area.width as f32, 1.0, MIN_GAP_CELLS, |seg|
///   StatusSegmentMeasure::new(seg.text.chars().count() as f32))`.
///   The measurer's unit is character cells.
/// - `theme` — used only for the fallback fill colour when the bar
///   has no segments at all.
pub fn draw_status_bar(
    buf: &mut Buffer,
    area: Rect,
    bar: &StatusBar,
    layout: &StatusBarLayout,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> StatusBarLayout {
    if area.width == 0 || area.height == 0 {
        return StatusBarLayout {
            bar_width: 0.0,
            bar_height: 0.0,
            visible_segments: Vec::new(),
            hit_regions: Vec::new(),
            resolved_right_start: 0,
        };
    }
    let y = area.y;

    let fill_bg = bar
        .left_segments
        .first()
        .or(bar.right_segments.first())
        .map(|s| ratatui_color(s.bg))
        .unwrap_or_else(|| ratatui_color(theme.background));
    for col in 0..area.width {
        set_cell(buf, area.x + col, y, ' ', fill_bg, fill_bg);
    }

    for vs in &layout.visible_segments {
        let seg = match vs.side {
            StatusSegmentSide::Left => &bar.left_segments[vs.segment_idx],
            StatusSegmentSide::Right => &bar.right_segments[vs.segment_idx],
        };
        let fg = ratatui_color(seg.fg);
        let effective_bg = if seg
            .action_id
            .as_ref()
            .is_some_and(|id| Some(id) == pressed_id)
        {
            seg.bg.darken(0.05)
        } else if seg
            .action_id
            .as_ref()
            .is_some_and(|id| Some(id) == hovered_id)
        {
            seg.bg.lighten(0.05)
        } else {
            seg.bg
        };
        let bg = ratatui_color(effective_bg);
        let start_x = area.x + vs.bounds.x.round() as u16;
        let bar_end = area.x + area.width;
        let mut cx = start_x;
        for ch in seg.text.chars() {
            if cx >= bar_end {
                break;
            }
            let w = cell_width(ch);
            if w == 2 && cx + 1 < bar_end {
                set_cell_wide(buf, cx, y, ch, fg, bg);
            } else {
                set_cell(buf, cx, y, ch, fg, bg);
            }
            if seg.bold {
                if let Some(cell) = buf.cell_mut(Position::new(cx, y)) {
                    cell.set_style(Style::default().add_modifier(Modifier::BOLD));
                }
            }
            cx += w;
        }
    }
    layout.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::{StatusBar, StatusBarSegment, StatusSegmentMeasure};
    use crate::types::{Color, WidgetId};

    fn make_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("test-bar"),
            left_segments: vec![
                StatusBarSegment {
                    text: "NORMAL".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(40, 80, 120),
                    bold: true,
                    action_id: None,
                },
                StatusBarSegment {
                    text: " main.rs ".into(),
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: None,
                },
            ],
            right_segments: vec![StatusBarSegment {
                text: " 1:1 ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_left_then_right_segments() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let bar = make_bar();
        let layout = bar.layout(40.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );

        // Left side starts at column 0: "NORMAL main.rs "
        let row: String = (0..15).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(row, "NORMAL main.rs ");

        // Right side ends at column 40: last cell is the space after "1:1 ".
        let right: String = (35..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(right, " 1:1 ");
    }

    #[test]
    fn empty_bar_falls_back_to_theme_background() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let bar = StatusBar {
            id: WidgetId::new("empty"),
            left_segments: vec![],
            right_segments: vec![],
        };
        let layout = bar.layout(10.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let theme = Theme {
            background: Color::rgb(1, 2, 3),
            foreground: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 10, 1),
            &bar,
            &layout,
            &theme,
            None,
            None,
        );

        // Whole bar painted with theme.background as bg.
        for x in 0..10 {
            let bg = buf[(x, 0)].bg;
            assert_eq!(
                bg,
                ratatui::style::Color::Rgb(1, 2, 3),
                "expected theme.background at column {x}, got {bg:?}"
            );
        }
    }

    #[test]
    fn bold_segment_sets_bold_modifier() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let bar = make_bar();
        let layout = bar.layout(20.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );

        // "NORMAL" was the bold segment — first 6 cells should carry BOLD.
        for x in 0..6 {
            assert!(
                buf[(x, 0)].modifier.contains(Modifier::BOLD),
                "expected BOLD at column {x}",
            );
        }
        // " main.rs " (non-bold) should not carry BOLD.
        for x in 6..15 {
            assert!(
                !buf[(x, 0)].modifier.contains(Modifier::BOLD),
                "did not expect BOLD at column {x}",
            );
        }
    }

    #[test]
    fn zero_size_area_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let bar = make_bar();
        let layout = bar.layout(10.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        // Zero-width area: function must return without panicking, without
        // touching the buffer, and with an empty layout.
        let result = draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 0, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert!(result.hit_regions.is_empty());
    }

    #[test]
    fn returns_hit_regions_for_action_segments() {
        // Bar with an action_id'd segment in the middle of the left half.
        let bar = StatusBar {
            id: WidgetId::new("test-bar"),
            left_segments: vec![
                StatusBarSegment {
                    text: "AAA".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(0, 0, 0),
                    bold: false,
                    action_id: None,
                },
                StatusBarSegment {
                    text: "BBBB".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(0, 0, 0),
                    bold: false,
                    action_id: Some(WidgetId::new("middle-action")),
                },
            ],
            right_segments: vec![],
        };
        let layout = bar.layout(20.0, 1.0, 0.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let result = draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );

        // One hit region for the segment with action_id.
        assert_eq!(result.hit_regions.len(), 1);
        use crate::primitives::status_bar::StatusBarHit;
        match &result.hit_regions[0].1 {
            StatusBarHit::Segment(id) => assert_eq!(id.as_str(), "middle-action"),
            other => panic!("expected Segment, got {:?}", other),
        }
        // Region starts after "AAA" (x=3.0) and is 4 cells wide ("BBBB").
        let r = &result.hit_regions[0].0;
        assert_eq!(r.x, 3.0);
        assert_eq!(r.width, 4.0);
    }

    fn make_clickable_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("click-bar"),
            left_segments: vec![StatusBarSegment {
                text: "BTN".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: Some(WidgetId::new("btn")),
            }],
            right_segments: vec![],
        }
    }

    #[test]
    fn hovered_segment_uses_lightened_bg() {
        let bar = make_clickable_bar();
        let base_bg = bar.left_segments[0].bg;
        let hover_id = WidgetId::new("btn");
        let layout = bar.layout(20.0, 1.0, 0.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            Some(&hover_id),
            None,
        );

        let expected = ratatui_color(base_bg.lighten(0.05));
        assert_eq!(
            buf[(0, 0)].bg,
            expected,
            "hovered segment bg should be lightened"
        );
    }

    #[test]
    fn pressed_segment_uses_darkened_bg() {
        let bar = make_clickable_bar();
        let base_bg = bar.left_segments[0].bg;
        let press_id = WidgetId::new("btn");
        let layout = bar.layout(20.0, 1.0, 0.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            Some(&press_id),
        );

        let expected = ratatui_color(base_bg.darken(0.05));
        assert_eq!(
            buf[(0, 0)].bg,
            expected,
            "pressed segment bg should be darkened"
        );
    }

    #[test]
    fn pressed_takes_precedence_over_hovered() {
        let bar = make_clickable_bar();
        let base_bg = bar.left_segments[0].bg;
        let id = WidgetId::new("btn");
        let layout = bar.layout(20.0, 1.0, 0.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            Some(&id),
            Some(&id),
        );

        let expected = ratatui_color(base_bg.darken(0.05));
        assert_eq!(buf[(0, 0)].bg, expected, "pressed should win over hovered");
    }

    #[test]
    fn non_clickable_segment_ignores_hover() {
        let bar = make_bar();
        let base_bg = bar.left_segments[0].bg;
        let fake_id = WidgetId::new("no-match");
        let layout = bar.layout(40.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &bar,
            &layout,
            &Theme::default(),
            Some(&fake_id),
            None,
        );

        let expected = ratatui_color(base_bg);
        assert_eq!(
            buf[(0, 0)].bg,
            expected,
            "non-clickable segment bg should be unchanged"
        );
    }
}
