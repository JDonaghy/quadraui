//! TUI rasteriser for [`crate::ToastOverlay`].
//!
//! Paints toast notification boxes stacked in a viewport corner.
//! Each toast is a small box with title, optional body, severity
//! tint, dismiss `×`, and optional action button label.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::toast::{
    toast_button_rects, truncate_line, wrap_text_lines, Toast, ToastFocusTarget, ToastMeasure,
    ToastOverlay, ToastSeverity, ToastStackLayout, VisibleToast, MAX_BODY_LINES,
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
/// Horizontal gap (cells) between two adjacent action buttons on the
/// button row (#1185).
const TUI_ACTION_GAP: f32 = 1.0;
/// Rows reserved for the top+bottom border (#1182 — TUI toasts previously
/// had no border at all).
const TUI_BORDER_ROWS: f32 = 2.0;
/// Columns reserved for the left+right border. Drawn in the 1-cell
/// padding columns every toast already reserved before text/dismiss, so
/// this doesn't change the box's overall width formula.
const TUI_BORDER_COLS: f32 = 2.0;
/// Toast-local inset from the box edge for the dismiss/action rows —
/// keeps both clear of the 1-cell border (#1185).
const TUI_INSET: f32 = 1.0;

/// Per-action button width in cells: label length plus
/// [`TUI_ACTION_PADDING`] breathing room either side.
fn tui_action_width(action: &crate::primitives::toast::ToastButton) -> f32 {
    action.label.chars().count() as f32 + TUI_ACTION_PADDING
}

/// Resolve a toast's box width from its content (#1182, extended #1185):
/// wide enough for the whole title plus its dismiss button, *and* wide
/// enough for the action row (if any) to fit without its buttons
/// overlapping — clamped to a sensible [`TUI_TOAST_MAX_WIDTH`] ceiling
/// and [`TUI_TOAST_MIN_WIDTH`] floor, and never wider than the viewport
/// allows. The body doesn't drive width — it wraps instead (see
/// [`tui_body_lines`]) — so a very long body alone doesn't blow the box
/// out sideways.
fn tui_toast_width(toast: &Toast, viewport_width: f32) -> f32 {
    let title_needed = toast.title.chars().count() as f32 + TUI_BORDER_COLS + TUI_DISMISS_WIDTH;
    let action_widths: Vec<f32> = toast.actions.iter().map(tui_action_width).collect();
    let actions_needed = if action_widths.is_empty() {
        0.0
    } else {
        TUI_BORDER_COLS
            + action_widths.iter().sum::<f32>()
            + TUI_ACTION_GAP * (action_widths.len() as f32 - 1.0)
    };
    title_needed
        .max(actions_needed)
        .clamp(TUI_TOAST_MIN_WIDTH, TUI_TOAST_MAX_WIDTH)
        .min((viewport_width - TUI_TOAST_MARGIN * 2.0).max(0.0))
}

/// Word-wrap `toast`'s body to fit a box of `width` columns (already
/// resolved by [`tui_toast_width`]), capped at [`MAX_BODY_LINES`] —
/// shared by the measure closure (for height) and the paint routine (for
/// the actual text), so they always agree (#1182).
fn tui_body_lines(toast: &Toast, width: f32) -> Vec<String> {
    if toast.body.is_empty() {
        return Vec::new();
    }
    let avail = (width - TUI_BORDER_COLS).max(0.0);
    wrap_text_lines(&toast.body, avail, MAX_BODY_LINES, &|s| {
        s.chars().count() as f32
    })
}

fn toast_height(toast: &Toast, width: f32) -> f32 {
    let body_lines = tui_body_lines(toast, width).len();
    // 1 title row + wrapped body rows + 1 button row (if the toast has
    // actions, #1185) + top/bottom border rows.
    let button_row = if toast.actions.is_empty() { 0.0 } else { 1.0 };
    1.0 + body_lines as f32 + button_row + TUI_BORDER_ROWS
}

fn severity_bg(severity: ToastSeverity, theme: &Theme) -> crate::types::Color {
    match severity {
        ToastSeverity::Info => theme.surface_bg,
        ToastSeverity::Success => crate::types::Color::rgb(30, 80, 30),
        ToastSeverity::Warning => crate::types::Color::rgb(100, 80, 20),
        ToastSeverity::Error => theme.error_fg,
    }
}

/// Compute the TUI cell-unit layout for a [`ToastOverlay`] without painting.
///
/// `area`'s origin is baked into the returned bounds (absolute buffer
/// coordinates, matching `tui_menu_bar_layout` / `tui_panel_layout`) —
/// hosts call `layout.hit_test(x, y)` with raw click coordinates, no
/// localisation needed.
pub fn tui_toast_stack_layout(stack: &ToastOverlay, area: Rect) -> ToastStackLayout {
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
            let width = tui_toast_width(toast, viewport_width);
            let height = toast_height(toast, width);
            let action_widths: Vec<f32> = toast.actions.iter().map(tui_action_width).collect();
            let (dismiss_rect, action_rects) = toast_button_rects(
                width,
                height,
                TUI_INSET,
                TUI_DISMISS_WIDTH,
                1.0,
                &action_widths,
                1.0,
                TUI_ACTION_GAP,
            );
            ToastMeasure {
                width,
                height,
                dismiss_rect,
                action_rects,
            }
        },
    )
}

/// Draw a [`ToastOverlay`] overlay onto `buf`. Returns the layout for
/// host click dispatch.
pub fn draw_toast_stack(
    buf: &mut Buffer,
    area: Rect,
    stack: &ToastOverlay,
    theme: &Theme,
) -> ToastStackLayout {
    let layout = tui_toast_stack_layout(stack, area);

    for vt in &layout.visible_toasts {
        let toast = &stack.toasts[vt.toast_idx];
        let focus = stack.focus.as_ref().filter(|f| f.toast_id == toast.id);
        paint_toast(buf, area, vt, toast, theme, focus);
    }

    layout
}

fn paint_toast(
    buf: &mut Buffer,
    area: Rect,
    vt: &VisibleToast,
    toast: &Toast,
    theme: &Theme,
    focus: Option<&crate::primitives::toast::ToastFocus>,
) {
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
    // space left of the dismiss button — never drawn under it (#1182).
    // #1185: the action row moved to its own line at the bottom, so the
    // title only needs to dodge dismiss now.
    let title_y = by + 1;
    let text_end = vt
        .dismiss_bounds
        .map(|db| db.x.round() as u16)
        .unwrap_or(bx + bw.saturating_sub(1));
    let dismiss_w = vt.dismiss_bounds.map(|d| d.width).unwrap_or(0.0);
    let title_avail = (vt.bounds.width - TUI_BORDER_COLS - dismiss_w).max(0.0);
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

    // Dismiss × near the top-right (#1185 — was a full-height trailing
    // column before this; now a single-row square near the title row).
    let dismiss_focused = matches!(focus, Some(f) if f.target == ToastFocusTarget::Dismiss);
    if let Some(db) = vt.dismiss_bounds {
        let dx = db.x.round() as u16 + (db.width as u16).saturating_sub(1) / 2;
        let dy = db.y.round() as u16;
        if in_area(dx, dy) {
            // #1185: visible focus indicator — invert fg/bg on the
            // dismiss cell when it's the controller's focus target.
            let (dfg, dbg) = if dismiss_focused {
                (bg, ratatui_color(theme.link_fg))
            } else {
                (fg, bg)
            };
            set_cell(buf, dx, dy, '×', dfg, dbg);
        }
    }

    // Action buttons on their own row at the bottom-right (#1185 — was
    // inline with the title, at most one, before this). The action
    // marked `primary` fills its row with `theme.accent_bg`; the rest
    // paint plain text in `theme.link_fg` (secondary/ghost-button look).
    for (i, (ab, action)) in vt.action_rects.iter().zip(toast.actions.iter()).enumerate() {
        let ay = ab.y.round() as u16;
        let ax0 = ab.x.round() as u16;
        let aw = ab.width.round() as u16;
        let action_focused = matches!(focus, Some(f) if f.target == ToastFocusTarget::Action(i));
        let (mut row_bg, mut row_fg) = if action.primary {
            (theme.accent_bg, theme.foreground)
        } else {
            (bg_color, theme.link_fg)
        };
        if action_focused {
            // #1185: visible focus indicator — invert to a
            // `theme.link_fg` filled row, same treatment as dismiss.
            row_fg = row_bg;
            row_bg = theme.link_fg;
        }
        let row_bg = ratatui_color(row_bg);
        let row_fg = ratatui_color(row_fg);
        // Fill the button's own row so a primary action reads as a
        // filled "chip".
        for dx in 0..aw {
            let x = ax0 + dx;
            if in_area(x, ay) {
                set_cell(buf, x, ay, ' ', row_fg, row_bg);
            }
        }
        let label_start = ax0 + (aw.saturating_sub(action.label.chars().count() as u16)) / 2;
        for (c, ch) in (label_start..).zip(action.label.chars()) {
            if c >= ax0 + aw {
                break;
            }
            if in_area(c, ay) {
                set_cell(buf, c, ay, ch, row_fg, row_bg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::{
        Toast, ToastButton, ToastCorner, ToastFocus, ToastFocusTarget, ToastHit, ToastOverlay,
        ToastSeverity,
    };
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn info_toast(id: &str, title: &str) -> Toast {
        Toast {
            id: WidgetId::new(id),
            title: title.into(),
            body: String::new(),
            severity: ToastSeverity::Info,
            actions: Vec::new(),
            accent: None,
        }
    }

    fn stack_br(toasts: Vec<Toast>) -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts,
            focus: None,
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
        let dx = db.x.round() as u16 + (db.width as u16).saturating_sub(1) / 2;
        // Dismiss glyph paints on its own inset row near the top-right
        // (#1185).
        let dy = db.y.round() as u16;
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
        toast.actions = vec![ToastButton {
            id: WidgetId::new("retry"),
            label: "Retry".into(),
            primary: false,
        }];
        let stack = stack_br(vec![toast]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        assert_eq!(vt.action_rects.len(), 1);
        let ab = vt.action_rects[0];
        let ax0 = ab.x.round() as u16;
        let aw = ab.width.round() as u16;
        let ax = ax0 + (aw.saturating_sub("Retry".chars().count() as u16)) / 2;
        // Action label paints on its own row at the bottom-right (#1185).
        let ay = ab.y.round() as u16;
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

    /// Acceptance regression for #1182/#1185 (vimcode's "Install Markdown
    /// Language Server?" prompt): the box widens to fit the whole title
    /// plus its dismiss button instead of the old fixed 40-col width
    /// that cut the title off mid-word, the title never overlaps the
    /// dismiss column, the action row sits on its own line below the
    /// title/body (never inline with it), and the box has a visible
    /// border.
    #[test]
    fn long_title_with_action_widens_box_and_avoids_overlap() {
        let area = Rect::new(0, 0, 90, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Install Markdown Language Server?");
        toast.actions = vec![ToastButton {
            id: WidgetId::new("install"),
            label: "Install".into(),
            primary: true,
        }];
        toast.body = "N: don't ask again · :ExtInstall markdown-language-server".into();
        let stack = stack_br(vec![toast.clone()]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        assert_eq!(vt.action_rects.len(), 1);
        let ab = vt.action_rects[0];
        let db = vt.dismiss_bounds.expect("dismiss bounds present");

        // Box widened to fit the whole title + dismiss, up to the
        // configured max.
        let title_len = toast.title.chars().count() as f32;
        let expected_width = (title_len + TUI_BORDER_COLS + TUI_DISMISS_WIDTH)
            .max(TUI_TOAST_MIN_WIDTH)
            .min(TUI_TOAST_MAX_WIDTH);
        assert_eq!(vt.bounds.width, expected_width);

        // The whole title painted, ending strictly before the dismiss
        // column — never overlapping it.
        let bx = vt.bounds.x.round() as u16;
        let ty = vt.bounds.y.round() as u16 + 1;
        let dx = db.x.round() as u16;
        let painted_title: String = (bx + 1..dx).map(|x| cell_char(&buf, x, ty)).collect();
        assert_eq!(painted_title.trim_end(), toast.title);

        // The action row is on its own line, strictly below the title
        // row and the dismiss row — never inline with either.
        assert!(ab.y > db.y);
        assert!(ab.y > vt.bounds.y + 1.0);

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

    /// #1185: two actions produce two distinct, side-by-side button
    /// rects on the same row, with the primary action's row filled with
    /// `theme.accent_bg`.
    #[test]
    fn two_actions_paint_side_by_side_with_primary_filled() {
        let area = Rect::new(0, 0, 90, 20);
        let mut buf = Buffer::empty(area);
        let mut toast = info_toast("t1", "Install?");
        toast.actions = vec![
            ToastButton {
                id: WidgetId::new("install"),
                label: "Install".into(),
                primary: true,
            },
            ToastButton {
                id: WidgetId::new("skip"),
                label: "Skip".into(),
                primary: false,
            },
        ];
        let stack = stack_br(vec![toast]);
        let theme = Theme::default();
        let layout = draw_toast_stack(&mut buf, area, &stack, &theme);

        let vt = &layout.visible_toasts[0];
        assert_eq!(vt.action_rects.len(), 2);
        let install = vt.action_rects[0];
        let skip = vt.action_rects[1];
        assert_eq!(install.y, skip.y);
        assert!(install.x + install.width <= skip.x);

        // Primary's row is filled with `theme.accent_bg`.
        let ax = install.x.round() as u16;
        let ay = install.y.round() as u16;
        let cell = &buf[(ax, ay)];
        assert_eq!(cell.bg, ratatui_color(theme.accent_bg));
    }

    /// #1185: the controller's `ToastFocus` gets a visible focus
    /// indicator (inverted colours) on the dismiss cell.
    #[test]
    fn focused_dismiss_paints_inverted() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let mut stack = stack_br(vec![info_toast("t1", "Saved")]);
        stack.focus = Some(ToastFocus {
            toast_id: WidgetId::new("t1"),
            target: ToastFocusTarget::Dismiss,
        });
        let theme = Theme::default();
        let layout = draw_toast_stack(&mut buf, area, &stack, &theme);
        let db = layout.visible_toasts[0]
            .dismiss_bounds
            .expect("dismiss bounds present");
        let dx = db.x.round() as u16 + (db.width as u16).saturating_sub(1) / 2;
        let dy = db.y.round() as u16;
        let cell = &buf[(dx, dy)];
        assert_eq!(cell.bg, ratatui_color(theme.link_fg));
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
