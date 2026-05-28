//! TUI rasteriser for [`crate::ToastStack`].
//!
//! Paints toast notification boxes stacked in a viewport corner.
//! Each toast is a small box with title, optional body, severity
//! tint, dismiss `×`, and optional action button label.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::toast::{
    ToastMeasure, ToastSeverity, ToastStack, ToastStackLayout, VisibleToast,
};
use crate::theme::Theme;

const TUI_TOAST_WIDTH: f32 = 40.0;
const TUI_TOAST_MARGIN: f32 = 1.0;
const TUI_TOAST_GAP: f32 = 1.0;
const TUI_DISMISS_WIDTH: f32 = 3.0;
const TUI_ACTION_PADDING: f32 = 2.0;

fn toast_height(toast: &crate::primitives::toast::ToastItem) -> f32 {
    if toast.body.is_empty() {
        1.0
    } else {
        2.0
    }
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
pub fn tui_toast_stack_layout(
    stack: &ToastStack,
    viewport_width: f32,
    viewport_height: f32,
) -> ToastStackLayout {
    stack.layout(
        viewport_width,
        viewport_height,
        TUI_TOAST_MARGIN,
        TUI_TOAST_GAP,
        |i| {
            let toast = &stack.toasts[i];
            let action_w = toast
                .action
                .as_ref()
                .map(|a| a.label.chars().count() as f32 + TUI_ACTION_PADDING)
                .unwrap_or(0.0);
            ToastMeasure {
                width: TUI_TOAST_WIDTH.min(viewport_width - TUI_TOAST_MARGIN * 2.0),
                height: toast_height(toast),
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
    let layout = tui_toast_stack_layout(stack, area.width as f32, area.height as f32);

    for vt in &layout.visible_toasts {
        let toast = &stack.toasts[vt.toast_idx];
        paint_toast(buf, area, vt, toast, theme);
    }

    layout
}

fn paint_toast(
    buf: &mut Buffer,
    area: Rect,
    vt: &VisibleToast,
    toast: &crate::primitives::toast::ToastItem,
    theme: &Theme,
) {
    let bg_color = toast
        .accent
        .unwrap_or_else(|| severity_bg(toast.severity, theme));
    let bg = ratatui_color(bg_color);
    let fg = ratatui_color(theme.foreground);

    let bx = vt.bounds.x.round() as u16;
    let by = vt.bounds.y.round() as u16;
    let bw = vt.bounds.width.round() as u16;
    let bh = vt.bounds.height.round() as u16;

    // Fill background.
    for dy in 0..bh {
        for dx in 0..bw {
            let x = bx + dx;
            let y = by + dy;
            if x < area.x + area.width && y < area.y + area.height {
                set_cell(buf, x, y, ' ', fg, bg);
            }
        }
    }

    // Title on first row, left-aligned with 1-cell padding.
    let title_y = by;
    let text_end = vt
        .action_bounds
        .map(|ab| ab.x.round() as u16)
        .or_else(|| vt.dismiss_bounds.map(|db| db.x.round() as u16))
        .unwrap_or(bx + bw);
    for (col, ch) in (bx + 1..).zip(toast.title.chars()) {
        if col >= text_end {
            break;
        }
        set_cell(buf, col, title_y, ch, fg, bg);
    }

    // Body on second row (if present).
    if !toast.body.is_empty() && bh > 1 {
        let body_y = by + 1;
        for (col, ch) in (bx + 1..).zip(toast.body.chars()) {
            if col >= text_end {
                break;
            }
            set_cell(buf, col, body_y, ch, fg, bg);
        }
    }

    // Dismiss × at right edge of first row.
    if let Some(db) = vt.dismiss_bounds {
        let dx = db.x.round() as u16 + 1;
        let dy = db.y.round() as u16;
        if dx < area.x + area.width && dy < area.y + area.height {
            set_cell(buf, dx, dy, '×', fg, bg);
        }
    }

    // Action button label before dismiss on first row.
    if let Some(ab) = vt.action_bounds {
        if let Some(ref action) = toast.action {
            let ax = ab.x.round() as u16 + 1;
            let ay = ab.y.round() as u16;
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

    #[test]
    fn single_toast_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let stack = stack_br(vec![info_toast("t1", "Hello world")]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        assert_eq!(layout.visible_toasts.len(), 1);
        let vt = &layout.visible_toasts[0];

        // Title should be painted inside the toast bounds.
        let tx = vt.bounds.x.round() as u16 + 1;
        let ty = vt.bounds.y.round() as u16;
        assert_eq!(cell_char(&buf, tx, ty), 'H');

        // Hit-test on the title text → Body.
        let hit = layout.hit_test(tx as f32 + 0.5, ty as f32 + 0.5);
        assert_eq!(hit, ToastHit::Body(WidgetId::new("t1")));
    }

    #[test]
    fn dismiss_glyph_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let stack = stack_br(vec![info_toast("t1", "Test")]);
        let layout = draw_toast_stack(&mut buf, area, &stack, &Theme::default());

        let vt = &layout.visible_toasts[0];
        let db = vt.dismiss_bounds.expect("dismiss bounds present");
        let dx = db.x.round() as u16 + 1;
        let dy = db.y.round() as u16;
        assert_eq!(cell_char(&buf, dx, dy), '×');

        let hit = layout.hit_test(dx as f32 + 0.5, dy as f32 + 0.5);
        assert_eq!(hit, ToastHit::Dismiss(WidgetId::new("t1")));
    }

    #[test]
    fn action_button_paint_and_click_round_trip() {
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
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
        let ay = ab.y.round() as u16;
        assert_eq!(cell_char(&buf, ax, ay), 'R');

        let hit = layout.hit_test(ax as f32 + 0.5, ay as f32 + 0.5);
        assert_eq!(hit, ToastHit::Action(WidgetId::new("retry")));
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
        let body_y = vt.bounds.y.round() as u16 + 1;
        assert_eq!(cell_char(&buf, bx, body_y), 'B');
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
