//! Direct2D / DirectWrite rasteriser for [`crate::Dialog`] (issue #28).
//!
//! Content painting moved to the shared
//! [`crate::primitives::dialog::native_surface_paint::paint`] (#1077,
//! `NativeSurface` Phase 4 slice 4/8) — see that fn's module doc for the
//! drift it resolved, several of which were Windows-only gaps this
//! backend gains here for the first time: per-button border stroke,
//! `DialogButton::tint`, button-label/text-input vertical centring, and
//! `DialogTable`'s `column_widths` hint + correctly-sized header
//! separator.
//!
//! **Font role**: the caller now passes whichever [`DWrite`] handle it
//! wants the whole dialog painted in — see
//! [`crate::win::backend::WinBackend::draw_dialog`], which passes
//! `chrome_dwrite` (falling back to the editor `dwrite` handle if no
//! live chrome one exists yet), giving `chrome_dwrite` its first real
//! caller (see that field's own doc).
//!
//! `DialogInput::Toolbar` still renders inline here, unchanged — Windows
//! never delegated to a shared toolbar rasteriser (there is no
//! `win::toolbar`-via-`NativeSurface` equivalent yet; out of #1077's
//! scope, see the shared `paint`'s module doc).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod dialog;` and `backend.rs`'s module
//! docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::dialog::{native_surface_paint, Dialog, DialogInput, DialogLayout};
use crate::primitives::toolbar::ToolbarItemKind;
use crate::theme::Theme;

/// Draw a [`Dialog`] at its resolved `dialog_layout`. Returns
/// `Vec<Rect>` per visible button, in `dialog_layout.visible_buttons`
/// order — same contract as [`crate::Backend::draw_dialog`].
///
/// `dwrite` is the font the whole dialog paints in — see this module's
/// doc for why the caller now passes the chrome handle.
pub fn draw_dialog(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    dialog: &Dialog,
    dialog_layout: &DialogLayout,
    line_height: f32,
) -> Vec<Rect> {
    let theme = Theme::default();
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    let button_rects =
        native_surface_paint::paint(dialog, dialog_layout, line_height, &mut surface, &theme);

    // DialogInput::Toolbar isn't painted by the shared `paint` — see
    // that fn's module doc — so render it here, exactly as before this
    // migration (Windows has never delegated this to a shared toolbar
    // rasteriser).
    if let (Some(input_b), Some(DialogInput::Toolbar(toolbar))) =
        (dialog_layout.input_bounds, dialog.input.as_ref())
    {
        let fg = theme.surface_fg;
        let sel = theme.selected_bg;
        let border = theme.border_fg;
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

    button_rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::dialog::{DialogButton, DialogMeasure};
    use crate::primitives::toolbar::ToolbarItemMeasure;
    use crate::types::{Color, StyledText, WidgetId};
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
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

    /// #1077: `DialogButton::tint` continues to be honoured post-port —
    /// Windows already implemented this pre-migration (it was the one
    /// backend that did), and this test pins it against a regression now
    /// that the label paint moved into the shared `paint`.
    #[test]
    fn tinted_button_label_paints_in_the_tint_colour() {
        let surface = HeadlessSurface::new(300, 300).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut d = dialog();
        let tint = Color::rgb(220, 40, 40);
        d.buttons[0].tint = Some(tint); // "OK" — the default button
        let viewport = QRect::new(0.0, 0.0, 300.0, 300.0);
        let layout = d.layout(viewport, measure(), |_| ToolbarItemMeasure::new(0.0));

        surface
            .paint(|target| {
                let _ = draw_dialog(target, &dwrite, &d, &layout, 16.0);
            })
            .expect("paint dialog");

        let btn = layout.visible_buttons[0].bounds;
        let y = (btn.y + btn.height / 2.0) as u32;
        let found = (btn.x as u32..(btn.x + btn.width) as u32)
            .map(|x| surface.pixel_at(x, y))
            .any(|px| (px.r, px.g, px.b) == (tint.r, tint.g, tint.b));
        assert!(
            found,
            "tinted button label should paint at least one pixel in the tint colour \
             somewhere in its row"
        );
    }

    /// #1077: `DialogTable::column_widths` is now honoured on every
    /// backend — pre-migration Windows's table painter never read the
    /// field at all. Regression: a column with an explicit width hint
    /// much wider than its content must place the next column's text
    /// further right than the content-only width would.
    #[test]
    fn table_column_widths_hint_widens_the_column() {
        use crate::primitives::dialog::DialogTable;

        let table = DialogTable {
            headers: None,
            rows: vec![vec!["a".into(), "b".into()]],
            column_widths: Some(vec![40, 0]),
        };
        let mut d = dialog();
        d.table = Some(table);

        let measure_with_table = DialogMeasure {
            table_height: 16.0,
            ..measure()
        };
        let viewport = QRect::new(0.0, 0.0, 300.0, 300.0);
        let layout = d.layout(viewport, measure_with_table, |_| {
            ToolbarItemMeasure::new(0.0)
        });
        let table_b = layout.table_bounds.expect("table bounds present");

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");

        // Text-run sink records every `draw_text` call's rect — use it
        // to find where "b" (the second column's only cell) actually
        // landed.
        let previous = crate::testing::install_text_run_sink();
        let surface = HeadlessSurface::new(300, 300).expect("create surface");
        surface
            .paint(|target| {
                let _ = draw_dialog(target, &dwrite, &d, &layout, 16.0);
            })
            .expect("paint dialog");
        let runs = crate::testing::take_text_run_sink(previous);
        let b_rect = runs
            .iter()
            .find(|r| r.text == "b")
            .map(|r| r.bounds)
            .expect("second column cell painted");

        // A bare "a" is far narrower than the 40-char-cell hint
        // (`40 * 16.0 * 0.6` = 384 DIPs) — without honouring
        // `column_widths`, "b" would land just past "a"'s own narrow
        // measured width instead.
        assert!(
            b_rect.x > table_b.x + 100.0,
            "second column should be pushed right by the first column's explicit width \
             hint (b_rect.x={}, table_b.x={})",
            b_rect.x,
            table_b.x
        );
    }
}
