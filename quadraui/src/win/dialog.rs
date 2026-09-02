//! Direct2D / DirectWrite rasteriser for [`crate::Dialog`] (issue #28).
//!
//! Mirrors `gtk::dialog`'s structure: `dialog_layout` is fully resolved
//! upstream (host calls [`crate::primitives::dialog::Dialog::layout`]);
//! this module only paints it and returns the per-button hit rectangles
//! in `dialog_layout.visible_buttons` order, matching
//! [`crate::Backend::draw_dialog`]'s contract.
//!
//! Unlike GTK (which swaps between an editor-mono Pango layout and a
//! separate UI `FontDescription`), `WinBackend` only carries one
//! [`DWrite`] font today (see `win::status_bar`'s module doc on the
//! backend's theme/font posture), so title/body/buttons all render in
//! that single font.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod dialog;` and `backend.rs`'s module
//! docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::dialog::{Dialog, DialogInput, DialogLayout, DialogTable};
use crate::primitives::toolbar::ToolbarItemKind;
use crate::theme::Theme;
use crate::types::StyledText;

fn flatten(text: &StyledText) -> String {
    text.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Draw a [`DialogTable`] at `bounds`, top-left anchored. Column widths
/// are auto-computed from content via [`DWrite::measure_text`]. A header
/// row (when present) is followed by a plain dash separator row — same
/// simplification `gtk::dialog`'s table painter makes (no `┼` junction).
#[allow(clippy::too_many_arguments)]
fn draw_table(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    table: &DialogTable,
    bounds: Rect,
    line_height: f32,
    fg: crate::Color,
    border: crate::Color,
) {
    let ncols = table.num_cols();
    if ncols == 0 {
        return;
    }

    let measure = |s: &str| dwrite.measure_text(s).map(|(w, _)| w).unwrap_or(0.0);

    let mut col_w = vec![0.0f32; ncols];
    if let Some(headers) = &table.headers {
        for (j, h) in headers.iter().enumerate().take(ncols) {
            col_w[j] = col_w[j].max(measure(h));
        }
    }
    for row in &table.rows {
        for (j, cell) in row.iter().enumerate().take(ncols) {
            col_w[j] = col_w[j].max(measure(cell));
        }
    }

    let sep_w = measure(" \u{2502} ");
    let mut col_x = vec![0.0f32; ncols];
    let mut cursor_x = bounds.x;
    for j in 0..ncols {
        col_x[j] = cursor_x;
        cursor_x += col_w[j];
        if j + 1 < ncols {
            cursor_x += sep_w;
        }
    }

    let mut row_y = bounds.y;
    let row_rect = |x: f32, y: f32, w: f32| Rect::new(x, y, w.max(1.0), line_height);

    if let Some(headers) = &table.headers {
        for (j, h) in headers.iter().enumerate().take(ncols) {
            let _ = dwrite.draw_text(target, h, row_rect(col_x[j], row_y, col_w[j]), fg);
        }
        for j in 0..ncols.saturating_sub(1) {
            let sep_x = col_x[j] + col_w[j];
            let _ = dwrite.draw_text(target, " \u{2502} ", row_rect(sep_x, row_y, sep_w), border);
        }
        row_y += line_height;

        let total_w = col_x[ncols - 1] + col_w[ncols - 1] - bounds.x;
        let dashes =
            "\u{2500}".repeat(((total_w / line_height.max(1.0)).ceil() as usize + 4).max(1));
        let _ = dwrite.draw_text(target, &dashes, row_rect(bounds.x, row_y, total_w), border);
        row_y += line_height;
    }

    for row in &table.rows {
        for (j, cell) in row.iter().enumerate().take(ncols) {
            let _ = dwrite.draw_text(target, cell, row_rect(col_x[j], row_y, col_w[j]), fg);
        }
        for j in 0..ncols.saturating_sub(1) {
            let sep_x = col_x[j] + col_w[j];
            let _ = dwrite.draw_text(target, " \u{2502} ", row_rect(sep_x, row_y, sep_w), border);
        }
        row_y += line_height;
    }
}

/// Draw a [`Dialog`] at its resolved `dialog_layout`. Returns
/// `Vec<Rect>` per visible button, in `dialog_layout.visible_buttons`
/// order — same contract as [`crate::Backend::draw_dialog`].
pub fn draw_dialog(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    dialog: &Dialog,
    dialog_layout: &DialogLayout,
    line_height: f32,
) -> Vec<Rect> {
    let bounds = dialog_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    let theme = Theme::default();
    let bg = theme.surface_bg;
    let fg = theme.surface_fg;
    let border = theme.border_fg;
    let sel = theme.selected_bg;
    let input_bg = theme.input_bg;
    let title_fg = theme.title_fg;

    let _ = fill_rect(target, bounds, bg);
    let _ = stroke_rect(target, bounds, border, 1.0);

    if let Some(title_rect) = dialog_layout.title_bounds {
        let _ = dwrite.draw_text(target, &flatten(&dialog.title), title_rect, title_fg);
    }

    let body_b = dialog_layout.body_bounds;
    for (i, line) in dialog.body.iter().enumerate() {
        let row_y = body_b.y + i as f32 * line_height;
        if row_y + line_height > body_b.y + body_b.height {
            break;
        }
        let row_rect = Rect::new(body_b.x, row_y, body_b.width, line_height);
        let _ = dwrite.draw_text(target, &flatten(line), row_rect, fg);
    }

    if let (Some(table_b), Some(table)) = (dialog_layout.table_bounds, dialog.table.as_ref()) {
        draw_table(target, dwrite, table, table_b, line_height, fg, border);
    }

    if let (Some(input_b), Some(input_kind)) = (dialog_layout.input_bounds, dialog.input.as_ref()) {
        match input_kind {
            DialogInput::TextInput(input) => {
                let _ = fill_rect(target, input_b, input_bg);
                let _ = stroke_rect(target, input_b, border, 1.0);
                let display = if input.value.is_empty() {
                    format!(" {}", input.placeholder)
                } else {
                    format!(" {}", input.value)
                };
                let text_rect = Rect::new(
                    input_b.x + 2.0,
                    input_b.y,
                    (input_b.width - 2.0).max(0.0),
                    input_b.height,
                );
                let _ = dwrite.draw_text(target, &display, text_rect, fg);
            }
            DialogInput::Toolbar(toolbar) => {
                let _ = fill_rect(target, input_b, theme.background);
                let _ = stroke_rect(target, input_b, border, 1.0);
                if let Some(tl) = &dialog_layout.body_toolbar_layout {
                    for vis in &tl.visible_items {
                        match vis.kind {
                            ToolbarItemKind::Separator => {
                                let mid_x = vis.bounds.x + vis.bounds.width / 2.0;
                                let _ = super::text::draw_line(
                                    target,
                                    mid_x,
                                    vis.bounds.y,
                                    mid_x,
                                    vis.bounds.y + vis.bounds.height,
                                    border,
                                    1.0,
                                );
                            }
                            ToolbarItemKind::Action | ToolbarItemKind::Label => {
                                if let Some(crate::primitives::toolbar::ToolbarButton::Action {
                                    label,
                                    enabled,
                                    is_active,
                                    ..
                                }) = toolbar.buttons.get(vis.item_idx)
                                {
                                    if *is_active {
                                        let _ = fill_rect(target, vis.bounds, sel);
                                    }
                                    let label_fg = if *enabled { fg } else { theme.muted_fg };
                                    let _ = dwrite.draw_text(target, label, vis.bounds, label_fg);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut rects = Vec::with_capacity(dialog_layout.visible_buttons.len());
    for vis in &dialog_layout.visible_buttons {
        let btn = &dialog.buttons[vis.button_idx];
        rects.push(vis.bounds);

        if btn.is_default {
            let _ = fill_rect(target, vis.bounds, sel);
        }
        let _ = stroke_rect(target, vis.bounds, border, 1.0);

        let label = if dialog.vertical_buttons {
            let prefix = if btn.is_default { "\u{25b8} " } else { "  " };
            format!("{prefix}{}", btn.label)
        } else {
            format!("  {}  ", btn.label)
        };
        let label_fg = btn.tint.unwrap_or(fg);
        let _ = dwrite.draw_text(target, &label, vis.bounds, label_fg);
    }

    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::dialog::{DialogButton, DialogMeasure};
    use crate::primitives::toolbar::ToolbarItemMeasure;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    fn dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("d"),
            title: StyledText::plain("Title"),
            body: vec![StyledText::plain("Body")],
            buttons: vec![
                DialogButton {
                    id: WidgetId::new("ok"),
                    label: "OK".into(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                },
                DialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                },
            ],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: None,
        }
    }

    fn measure() -> DialogMeasure {
        DialogMeasure {
            width: 200.0,
            title_height: 20.0,
            body_height: 20.0,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: 20.0,
            button_width: 60.0,
            button_gap: 8.0,
            padding: 10.0,
        }
    }

    #[test]
    fn paints_and_returns_per_button_hit_rects() {
        let surface = HeadlessSurface::new(300, 300).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let d = dialog();
        let viewport = QRect::new(0.0, 0.0, 300.0, 300.0);
        let layout = d.layout(viewport, measure(), |_| ToolbarItemMeasure::new(0.0));

        let mut rects = Vec::new();
        surface
            .paint(|target| {
                rects = draw_dialog(target, &dwrite, &d, &layout, 16.0);
            })
            .expect("paint dialog");

        assert_eq!(rects.len(), 2, "one hit rect per visible button");
        assert_eq!(
            rects,
            layout
                .visible_buttons
                .iter()
                .map(|v| v.bounds)
                .collect::<Vec<_>>()
        );

        // Default (first) button's bg is the selected-bg tint.
        let sel = Theme::default().selected_bg;
        let r0 = rects[0];
        let px = surface.pixel_at((r0.x + 2.0) as u32, (r0.y + r0.height / 2.0) as u32);
        assert_eq!((px.r, px.g, px.b), (sel.r, sel.g, sel.b));

        // Dialog background is painted at a corner clear of any chrome.
        let bg = Theme::default().surface_bg;
        let b = layout.bounds;
        let corner = surface.pixel_at((b.x + 2.0) as u32, (b.y + 2.0) as u32);
        assert_eq!((corner.r, corner.g, corner.b), (bg.r, bg.g, bg.b));
    }
}
