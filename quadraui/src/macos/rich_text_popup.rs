//! macOS rasteriser for [`crate::RichTextPopup`].
//!
//! Content painting moved to the shared
//! [`crate::primitives::rich_text_popup::native_surface_paint::paint`]
//! (#1077, `PaintSurface` Phase 4 slice 4/8 — GTK is deliberately not
//! part of this migration, see that fn's module doc). macOS gains
//! several capabilities its own pre-migration "Scope omissions" doc
//! (kept below for the historical record) listed as missing: selection
//! background + inverted fg, a focused-link underline, and
//! content-area clipping. Bold span styling is still not rendered here
//! — `CgSurface::surface_draw_text_run_styled` takes the trait's
//! default, which drops style entirely (see that default's own doc) —
//! so this is a real gap that stays a gap, not a claimed fix.
//!
//! ## Scope omissions (historical, pre-#1077; superseded above except
//! ## where noted)
//!
//! - **Per-line font scale** (markdown heading rows) — needs
//!   `CTFontCreateCopyWithSymbolicTraits` or per-line CTFont swap. Still
//!   not supported; see the shared `paint`'s module doc.
//! - **Bold / italic span attributes** — bold still isn't rendered
//!   (see above); italic was never supported and remains unsupported.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::rich_text_popup::{
    native_surface_paint, RichTextPopup, RichTextPopupLayout,
};
use crate::theme::Theme;

pub const RICH_TEXT_POPUP_SB_WIDTH: f64 = native_surface_paint::SB_WIDTH as f64;
pub const RICH_TEXT_POPUP_SB_INSET: f64 = native_surface_paint::SB_INSET as f64;

/// Draw a [`RichTextPopup`] at its resolved layout.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_rich_text_popup(
    ctx: CGContextRef,
    font: &CTFont,
    popup: &RichTextPopup,
    layout: &RichTextPopupLayout,
    theme: &Theme,
) {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    let _ = native_surface_paint::paint(popup, layout, &mut surface, theme);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::rich_text_popup::PopupPlacement;
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 200;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_popup() -> RichTextPopup {
        RichTextPopup {
            id: WidgetId::new("rtp"),
            lines: vec![
                StyledText::plain("fn map<U, F>("),
                StyledText::plain("    self,"),
                StyledText::plain("    f: F,"),
                StyledText::plain(") -> Option<U>"),
            ],
            line_text: vec![
                "fn map<U, F>(".into(),
                "    self,".into(),
                "    f: F,".into(),
                ") -> Option<U>".into(),
            ],
            line_scales: vec![],
            scroll_top: 0,
            max_visible_rows: 8,
            has_focus: false,
            selection: None,
            links: vec![],
            focused_link: None,
            placement: PopupPlacement::Above,
            padding: 1.0,
            fg: None,
            bg: None,
        }
    }

    fn layout_for(
        popup: &RichTextPopup,
        viewport: QRect,
        line_height: f32,
        char_width: f32,
    ) -> RichTextPopupLayout {
        let measure = crate::primitives::rich_text_popup::RichTextPopupMeasure::new(
            char_width * 30.0,
            line_height,
        );
        popup.layout(100.0, 150.0, viewport, measure, |_, _, _| 0.0)
    }

    fn paint(popup: &RichTextPopup, layout: &RichTextPopupLayout) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_rich_text_popup(popup, layout);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn popup_paints_hover_bg() {
        let popup = sample_popup();
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&popup, viewport, 16.0, 8.4);
        let surface = paint(&popup, &layout);
        let theme = Theme::default();
        // Probe inside the content area, away from any line glyphs
        // (right edge of the popup).
        let b = layout.bounds;
        let (r, g, bp, _) =
            surface.pixel((b.x + b.width - 4.0) as u32, (b.y + b.height - 4.0) as u32);
        assert_eq!(
            (r, g, bp),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
        );
    }

    #[test]
    fn focused_popup_border_paints_differently_than_unfocused() {
        // When `has_focus` is true the border should be drawn in
        // `theme.link_fg` instead of `theme.hover_border`. We probe
        // the same edge pixel in both states and assert the values
        // differ — sufficient to prove the conditional fires without
        // needing to disambiguate AA-blended pixel values.
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let mut unfocused = sample_popup();
        unfocused.has_focus = false;
        let mut focused = sample_popup();
        focused.has_focus = true;
        let layout_u = layout_for(&unfocused, viewport, 16.0, 8.4);
        let layout_f = layout_for(&focused, viewport, 16.0, 8.4);
        let surface_u = paint(&unfocused, &layout_u);
        let surface_f = paint(&focused, &layout_f);
        let bx = layout_u.bounds.x as u32;
        let by = layout_u.bounds.y as u32;
        let bw = layout_u.bounds.width as u32;
        // Right edge of the top border, away from line glyphs.
        let pu = surface_u.pixel(bx + bw - 4, by);
        let pf = surface_f.pixel(bx + bw - 4, by);
        assert_ne!(
            (pu.0, pu.1, pu.2),
            (pf.0, pf.1, pf.2),
            "focused vs unfocused border should paint different colours",
        );
    }

    /// #1077: selection background is now painted on macOS — pre-migration
    /// this backend's own doc listed it as a "Scope omission." Regression:
    /// a selected character's row must contain at least one pixel in the
    /// popup's own foreground colour (the selection-bg fill), which no
    /// unselected popup ever paints.
    #[test]
    fn selection_paints_a_background_fill() {
        use crate::primitives::rich_text_popup::TextSelection;

        let mut popup = sample_popup();
        popup.selection = Some(TextSelection {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 5,
        });
        let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
        let layout = layout_for(&popup, viewport, 16.0, 8.4);
        let surface = paint(&popup, &layout);
        let theme = Theme::default();
        let row = layout.visible_lines[0].bounds;
        let sel_fg = popup.fg.unwrap_or(theme.foreground);
        let y = (row.y + row.height / 2.0) as u32;
        let found = (row.x as u32..(row.x + 60.0) as u32)
            .map(|x| surface.pixel(x, y))
            .any(|(r, g, b, _)| (r, g, b) == (sel_fg.r, sel_fg.g, sel_fg.b));
        assert!(
            found,
            "selected row should paint at least one pixel in the selection-bg colour"
        );
    }

    /// [`crate::Backend::draw_rich_text_popup_with_font_role`] must
    /// select a genuinely different live `CTFont`, not a documented
    /// no-op: with the chrome font set far larger than the editor font,
    /// requesting the default (plain `draw_rich_text_popup`, i.e.
    /// `FontRole::Chrome`) paints a visibly wider glyph than requesting
    /// `FontRole::Editor`, and both must paint real ink.
    #[test]
    fn font_role_editor_paints_with_the_editor_fonts_metrics() {
        fn ink_right_edge(surface: &BitmapSurface, bounds: crate::event::Rect) -> i32 {
            let mut last = 0i32;
            for y in bounds.y as u32..(bounds.y + bounds.height) as u32 {
                for x in bounds.x as u32..(bounds.x + bounds.width) as u32 {
                    let (r, g, b, _) = surface.pixel(x, y);
                    if (r, g, b) != (0, 0, 0) {
                        last = last.max(x as i32 - bounds.x as i32);
                    }
                }
            }
            last
        }

        fn single_char_popup() -> RichTextPopup {
            let mut p = sample_popup();
            p.lines = vec![StyledText::plain("M")];
            p.line_text = vec!["M".to_string()];
            p.fg = Some(crate::types::Color::rgb(255, 255, 255));
            p.bg = Some(crate::types::Color::rgb(0, 0, 0));
            p
        }

        fn paint_with_role(role: Option<crate::FontRole>) -> (BitmapSurface, RichTextPopupLayout) {
            let popup = single_char_popup();
            let viewport = QRect::new(0.0, 0.0, W as f32, H as f32);
            let measure =
                crate::primitives::rich_text_popup::RichTextPopupMeasure::new(200.0, 80.0);
            let layout = popup.layout(10.0, 10.0, viewport, measure, |_, _, _| 0.0);
            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = MacBackend::new();
            // Chrome font far larger than the editor font — if the role
            // really selects a different live `CTFont`, the two runs
            // paint visibly different glyph widths.
            backend.set_ui_font("Menlo 60");
            backend.set_current_font(font());
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| match role {
                Some(role) => b.draw_rich_text_popup_with_font_role(&popup, &layout, role),
                None => b.draw_rich_text_popup(&popup, &layout),
            });
            backend.end_frame();
            (surface, layout)
        }

        // `None` = plain `draw_rich_text_popup` (today's call site, no
        // role argument at all) — must still paint the chrome font.
        let (chrome_surface, chrome_layout) = paint_with_role(None);
        let (editor_surface, editor_layout) = paint_with_role(Some(crate::FontRole::Editor));

        let chrome_right = ink_right_edge(&chrome_surface, chrome_layout.content_bounds);
        let editor_right = ink_right_edge(&editor_surface, editor_layout.content_bounds);

        assert!(
            editor_right > 0,
            "FontRole::Editor must still paint real ink, got {editor_right}"
        );
        assert!(
            chrome_right > editor_right * 2,
            "plain draw_rich_text_popup (no role argument) painting the 60pt chrome font \
             should paint a far wider glyph than FontRole::Editor's 14pt glyph: \
             chrome_right={chrome_right}, editor_right={editor_right}"
        );
    }
}
