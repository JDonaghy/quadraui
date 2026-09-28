//! TUI rasteriser for [`crate::ToastStack`].
//!
//! Paints toast notification boxes stacked in a viewport corner.
//! Each toast is a small box with title, optional body, severity
//! tint, dismiss `×`, and optional action button label.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::toast::{
    truncate_line, wrap_text_lines, ToastItem, ToastMeasure, ToastSeverity, ToastStack,
    ToastStackLayout, VisibleToast, MAX_BODY_LINES,
};
use crate::theme::Theme;

/// Max toast width (#1182): was a fixed `40.0` too narrow for a title
/// with an action button (e.g. vimcode's "Install Markdown Language
/// Server?" + "Install"), which got cut off mid-word. Now a per-content
/// floor/ceiling — see [`tui_toast_width`] — with this as the ceiling.
const TUI_TOAST_MAX_WIDTH: f32 = 60.0;
/// Width floor: keeps a short "Saved"-style toast from shrinking below a
/// sensible box, and leaves room for the 2-cell border + dismiss glyph.
const TUI_TOAST_MIN_WIDTH: f32 = 12.0;
const TUI_TOAST_MARGIN: f32 = 1.0;
const TUI_TOAST_GAP: f32 = 1.0;
const TUI_DISMISS_WIDTH: f32 = 3.0;
const TUI_ACTION_PADDING: f32 = 2.0;
/// Rows reserved for the top+bottom border (#1182 — TUI toasts previously
/// had no border at all).
const TUI_BORDER_ROWS: f32 = 2.0;
/// Columns reserved for the left+right border. Drawn in the 1-cell
/// padding columns every toast already reserved before text/dismiss, so
/// this doesn't change the box's overall width formula.
const TUI_BORDER_COLS: f32 = 2.0;

/// Resolve a toast's box width from its content (#1182): wide enough for
/// the whole title plus its action/dismiss buttons, clamped to a sensible
/// [`TUI_TOAST_MAX_WIDTH`] ceiling and [`TUI_TOAST_MIN_WIDTH`] floor, and
/// never wider than the viewport allows. The body doesn't drive width —
/// it wraps instead (see [`tui_body_lines`]) — so a very long body alone
/// doesn't blow the box out sideways.
fn tui_toast_width(
    toast: &ToastItem,
    dismiss_width: f32,
    action_width: f32,
    viewport_width: f32,
) -> f32 {
    let title_needed =
        toast.title.chars().count() as f32 + TUI_BORDER_COLS + dismiss_width + action_width;
    title_needed
        .clamp(TUI_TOAST_MIN_WIDTH, TUI_TOAST_MAX_WIDTH)
        .min((viewport_width - TUI_TOAST_MARGIN * 2.0).max(0.0))
}

/// Word-wrap `toast`'s body to fit a box of `width` columns (already
/// resolved by [`tui_toast_width`]), capped at [`MAX_BODY_LINES`] —
/// shared by the measure closure (for height) and the paint routine (for
/// the actual text), so they always agree (#1182).
fn tui_body_lines(toast: &ToastItem, width: f32) -> Vec<String> {
    if toast.body.is_empty() {
        return Vec::new();
    }
    let avail = (width - TUI_BORDER_COLS).max(0.0);
    wrap_text_lines(&toast.body, avail, MAX_BODY_LINES, &|s| {
        s.chars().count() as f32
    })
}

fn toast_height(toast: &ToastItem, width: f32) -> f32 {
    let body_lines = tui_body_lines(toast, width).len();
    // 1 title row + wrapped body rows + top/bottom border rows.
    1.0 + body_lines as f32 + TUI_BORDER_ROWS
}

fn severity_bg(severity: ToastSeverity, theme: &Theme) -> crate::types::Color {
    match severity {
        ToastSeverity::Info => theme.surface_bg,
        ToastSeverity::Success => crate::types::Color::rgb(30, 80, 30),
        ToastSeverity::Warning => crate::types::Color::rgb(100, 80, 20),
        ToastSeverity::Error => theme.error_fg,
    }
}

/// Compute the TUI cell-unit layout for a [`ToastStack`] without painting.
///
/// `area`'s origin is baked into the returned bounds (absolute buffer
/// coordinates, matching `tui_menu_bar_layout` / `tui_panel_layout`) —
/// hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
pub fn tui_toast_stack_layout(stack: &ToastStack, area: Rect) -> ToastStackLayout {
    let viewport_width = area.width as f32;
    stack.layout(
        area.x as f32,
        area.y as f32,
        viewport_width,
        area.height as f32,
        TUI_TOAST_MARGIN,
        TUI_TOAST_GAP,
        |i| {
            let toast = &stack.toasts[i];
            let action_w = toast
                .action
                .as_ref()
                .map(|a| a.label.chars().count() as f32 + TUI_ACTION_PADDING)
                .unwrap_or(0.0);
            let width = tui_toast_width(toast, TUI_DISMISS_WIDTH, action_w, viewport_width);
            ToastMeasure {
                width,
                height: toast_height(toast, width),
                dismiss_width: TUI_DISMISS_WIDTH,
                action_width: action_w,
            }
        },
    )
}

/// Draw a [`ToastStack`] overlay onto `buf`. Returns the layout for
/// host click dispatch.
pub fn draw_toast_stack(
    buf: &mut Buffer,
    area: Rect,
    stack: &ToastStack,
    theme: &Theme,
) -> ToastStackLayout {
    let layout = tui_toast_stack_layout(stack, area);

    for vt in &layout.visible_toasts {
        let toast = &stack.toasts[vt.toast_idx];
        paint_toast(buf, area, vt, toast, theme);
    }

    layout
}

fn paint_toast(buf: &mut Buffer, area: Rect, vt: &VisibleToast, toast: &ToastItem, theme: &Theme) {
    let bg_color = toast
        .accent
        .unwrap_or_else(|| severity_bg(toast.severity, theme));
    let bg = ratatui_color(bg_color);
    let fg = ratatui_color(theme.foreground);
    let border_fg = ratatui_color(theme.border_fg);

    let bx = vt.bounds.x.round() as u16;
    let by = vt.bounds.y.round() as u16;
    let bw = vt.bounds.width.round() as u16;
    let bh = vt.bounds.height.round() as u16;

    let in_area = |x: u16, y: u16| x < area.x + area.width && y < area.y + area.height;

    // Fill background.
    for dy in 0..bh {
        for dx in 0..bw {
            let x = bx + dx;
            let y = by + dy;
            if in_area(x, y) {
                set_cell(buf, x, y, ' ', fg, bg);
            }
        }
    }

    // Border (#1182 — previously absent, so an Info toast's `surface_bg`
    // fill could blend into a light theme). Top/bottom rows get a full
    // box-drawing line; the left/right columns of the interior rows
    // reuse the 1-cell padding every toast already reserved before
    // text/dismiss, so this doesn't change the box's width.
    let bottom_y = by + bh.saturating_sub(1);
    if bh >= 2 && bw >= 2 {
        for dx in 0..bw {
            let x = bx + dx;
            let top_ch = if dx == 0 {
                '┌'
            } else if dx == bw - 1 {
                '┐'
            } else {
                '─'
            };
            if in_area(x, by) {
                set_cell(buf, x, by, top_ch, border_fg, bg);
            }
            let bottom_ch = if dx == 0 {
                '└'
            } else if dx == bw - 1 {
                '┘'
            } else {
                '─'
            };
            if in_area(x, bottom_y) {
                set_cell(buf, x, bottom_y, bottom_ch, border_fg, bg);
            }
        }
        for dy in 1..bh.saturating_sub(1) {
            let y = by + dy;
            if in_area(bx, y) {
                set_cell(buf, bx, y, '│', border_fg, bg);
            }
            if in_area(bx + bw - 1, y) {
                set_cell(buf, bx + bw - 1, y, '│', border_fg, bg);
            }
        }
    }

    // Title on the first interior row (below the top border),
    // left-aligned with 1-cell padding, truncated/ellipsized to the
    // space left of the action/dismiss buttons — never drawn under them
    // (#1182).
    let title_y = by + 1;
    let text_end = vt
        .action_bounds
        .map(|ab| ab.x.round() as u16)
        .or_else(|| vt.dismiss_bounds.map(|db| db.x.round() as u16))
        .unwrap_or(bx + bw.saturating_sub(1));
    let dismiss_w = vt.dismiss_bounds.map(|d| d.width).unwrap_or(0.0);
    let action_w = vt.action_bounds.map(|a| a.width).unwrap_or(0.0);
    let title_avail = (vt.bounds.width - TUI_BORDER_COLS - dismiss_w - action_w).max(0.0);
    let title_line = truncate_line(&toast.title, title_avail, &|s| s.chars().count() as f32);
    for (col, ch) in (bx + 1..).zip(title_line.chars()) {
        if col >= text_end {
            break;
        }
        set_cell(buf, col, title_y, ch, fg, bg);
    }

    // Body wraps across up to `MAX_BODY_LINES` rows below the title
    // (#1182 — previously a single unwrapped row that ran past the box).
    // Body rows have no action/dismiss to avoid, so they use the full
    // interior width (up to the right border column).
    if !toast.body.is_empty() {
        let body_text_end = bx + bw.saturating_sub(1);
        for (i, line) in tui_body_lines(toast, vt.bounds.width).iter().enumerate() {
            let body_y = title_y + 1 + i as u16;
            for (col, ch) in (bx + 1..).zip(line.chars()) {
                if col >= body_text_end {
                    break;
                }
                set_cell(buf, col, body_y, ch, fg, bg);
            }
        }
    }

    // Dismiss × at right edge of the title row.
    if let Some(db) = vt.dismiss_bounds {
        let dx = db.x.round() as u16 + 1;
        let dy = title_y;
        if in_area(dx, dy) {
            set_cell(buf, dx, dy, '×', fg, bg);
        }
    }

    // Action button label before dismiss on the title row.
    if let Some(ab) = vt.action_bounds {
        if let Some(ref action) = toast.action {
            let ax = ab.x.round() as u16 + 1;
            let ay = title_y;
            let action_fg = ratatui_color(theme.accent_fg);
            for (c, ch) in (ax..).zip(action.label.chars()) {
                if c >= bx + bw {
                    break;
                }
                set_cell(buf, c, ay, ch, action_fg, bg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::{
        ToastAction, ToastCorner, ToastHit, ToastItem, ToastSeverity, ToastStack,
    };
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn info_toast(id: &str, title: &str) -> ToastItem {
        ToastItem {
            id: WidgetId::new(id),
            title: title.into(),
            body: String::new(),
            severity: ToastSeverity::Info,
            action: None,
            accent: None,
        }
    }

    fn stack_br(toasts: Vec<ToastItem>) -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts,
        }
    }

    /// Shared body for `single_toast_paint_and_click_round_trip`, run at
    /// both the origin and a non-zero `area` origin (quadraui#494 /
    /// LESSONS.md "Layout helpers must return coords in the same frame
    /// across backends"). `tui_toast_stack_layout` bakes `area.x`/`area.y`
    /// straight into the returned bounds (absolute frame, matching
    /// `tui_menu_bar_layout`/`tui_panel_layout`), so both the painted
    /// glyph position AND the `hit_test` coordinates must shift together
    /// with the origin.
    fn single_toast_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 60, 20);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 60, origin_y + 20));
        let stack = stack_br(vec![info_toast("t1", "Hello world")]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        assert_eq!(layout.visible_toasts.len(), 1);
        let vt = &layout.visible_toasts[0];

        // Title should be painted inside the toast bounds, on the first
        // interior row below the top border (#1182).
        let tx = vt.bounds.x.round() as u16 + 1;
        let ty = vt.bounds.y.round() as u16 + 1;
        assert_eq!(cell_char(&buf, tx, ty), 'H');

        // Hit-test on the title text → Body.
        let hit = layout.hit_test(tx as f32 + 0.5, ty as f32 + 0.5);
        assert_eq!(hit, ToastHit::Body(WidgetId::new("t1")));
    }

    #[test]
    fn single_toast_paint_and_click_round_trip() {
        single_toast_paint_and_click_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard (quadraui#494): same round trip,
    /// painted at a shifted `area` origin.
    #[test]
    fn single_toast_paint_and_click_round_trip_at_nonzero_origin() {
        single_toast_paint_and_click_round_trip_at(7, 13);
    }

    /// Shared body for `dismiss_glyph_paint_and_click_round_trip` —
    /// see [`single_toast_paint_and_click_round_trip_at`] for the
    /// quadraui#494 non-zero-origin rationale.
    fn dismiss_glyph_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 60, 20);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 60, origin_y + 20));
        let stack = stack_br(vec![info_toast("t1", "Test")]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        let db = vt.dismiss_bounds.expect("dismiss bounds present");
        let dx = db.x.round() as u16 + 1;
        // Dismiss glyph paints on the title row, below the top border.
        let dy = db.y.round() as u16 + 1;
        assert_eq!(cell_char(&buf, dx, dy), '×');

        let hit = layout.hit_test(dx as f32 + 0.5, dy as f32 + 0.5);
        assert_eq!(hit, ToastHit::Dismiss(WidgetId::new("t1")));
    }

    #[test]
    fn dismiss_glyph_paint_and_click_round_trip() {
        dismiss_glyph_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn dismiss_glyph_paint_and_click_round_trip_at_nonzero_origin() {
        dismiss_glyph_paint_and_click_round_trip_at(7, 13);
    }

    /// Shared body for `action_button_paint_and_click_round_trip` —
    /// see [`single_toast_paint_and_click_round_trip_at`] for the
    /// quadraui#494 non-zero-origin rationale.
    fn action_button_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 60, 20);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 60, origin_y + 20));
        let mut toast = info_toast("t1", "Error occurred");
        toast.action = Some(ToastAction {
            id: WidgetId::new("retry"),
            label: "Retry".into(),
        });
        let stack = stack_br(vec![toast]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        let ab = vt.action_bounds.expect("action bounds present");
        let ax = ab.x.round() as u16 + 1;
        // Action label paints on the title row, below the top border.
        let ay = ab.y.round() as u16 + 1;
        assert_eq!(cell_char(&buf, ax, ay), 'R');

        let hit = layout.hit_test(ax as f32 + 0.5, ay as f32 + 0.5);
        assert_eq!(hit, ToastHit::Action(WidgetId::new("retry")));
    }

    #[test]
    fn action_button_paint_and_click_round_trip() {
        action_button_paint_and_click_round_trip_at(0, 0);
    }

    #[test]
    fn action_button_paint_and_click_round_trip_at_nonzero_origin() {
        action_button_paint_and_click_round_trip_at(7, 13);
    }

    #[test]
    fn body_text_paints_on_second_row() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Title");
        toast.body = "Body text here".into();
        let stack = stack_br(vec![toast]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        let bx = vt.bounds.x.round() as u16 + 1;
        // Body paints below the title row, which itself sits below the
        // top border (#1182): bounds.y (border) + 1 (title) + 1 (body).
        let body_y = vt.bounds.y.round() as u16 + 2;
        assert_eq!(cell_char(&buf, bx, body_y), 'B');
    }

    /// Acceptance regression for #1182 (vimcode's "Install Markdown
    /// Language Server?" prompt): the box widens to fit the whole title
    /// plus its action/dismiss buttons instead of the old fixed 40-col
    /// width that cut the title off mid-word, the title never overlaps
    /// the action column, and the box has a visible border.
    #[test]
    fn long_title_with_action_widens_box_and_avoids_overlap() {
        let area = Rect::new(0, 0, 90, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Install Markdown Language Server?");
        toast.action = Some(ToastAction {
            id: WidgetId::new("install"),
            label: "Install".into(),
        });
        toast.body = "N: don't ask again · :ExtInstall markdown-language-server".into();
        let stack = stack_br(vec![toast.clone()]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        let ab = vt.action_bounds.expect("action bounds present");

        // Box widened to fit the whole title + action + dismiss, up to
        // the configured max.
        let title_len = toast.title.chars().count() as f32;
        let action_w =
            toast.action.as_ref().unwrap().label.chars().count() as f32 + TUI_ACTION_PADDING;
        let expected_width =
            (title_len + 2.0 + TUI_DISMISS_WIDTH + action_w).min(TUI_TOAST_MAX_WIDTH);
        assert_eq!(vt.bounds.width, expected_width);

        // The whole title painted, ending strictly before the action
        // column — never overlapping it.
        let bx = vt.bounds.x.round() as u16;
        let ty = vt.bounds.y.round() as u16 + 1;
        let ax = ab.x.round() as u16;
        let painted_title: String = (bx + 1..ax).map(|x| cell_char(&buf, x, ty)).collect();
        assert_eq!(painted_title.trim_end(), toast.title);

        // Border corners are visible (previously no border existed).
        let by = vt.bounds.y.round() as u16;
        let bottom_y = by + vt.bounds.height.round() as u16 - 1;
        assert_eq!(cell_char(&buf, bx, by), '┌');
        assert_eq!(cell_char(&buf, bx, bottom_y), '└');

        // Body wrapped onto a row inside the box, above the bottom border.
        let body_y1 = ty + 1;
        assert_ne!(cell_char(&buf, bx + 1, body_y1), ' ');
        assert!(
            bottom_y > body_y1,
            "box should have room for the wrapped body before its border"
        );
    }

    #[test]
    fn multiple_toasts_stack_upward_from_bottom() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let stack = stack_br(vec![
            info_toast("first", "First"),
            info_toast("second", "Second"),
        ]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        assert_eq!(layout.visible_toasts.len(), 2);
        // Newest (second) is nearest the bottom corner.
        assert_eq!(layout.visible_toasts[0].id.as_str(), "second");
        assert_eq!(layout.visible_toasts[1].id.as_str(), "first");
        // Second toast should be above the first.
        assert!(layout.visible_toasts[1].bounds.y < layout.visible_toasts[0].bounds.y);
    }

    #[test]
    fn empty_stack_paints_nothing() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let stack = stack_br(vec![]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());
        assert!(layout.visible_toasts.is_empty());
        assert_eq!(layout.hit_test(30.0, 10.0), ToastHit::Empty);
    }

    #[test]
    fn outside_toast_returns_empty() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let stack = stack_br(vec![info_toast("t1", "Test")]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());
        // Top-left corner is far from bottom-right toast.
        assert_eq!(layout.hit_test(0.0, 0.0), ToastHit::Empty);
    }
}
