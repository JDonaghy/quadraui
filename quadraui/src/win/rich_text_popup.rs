//! Direct2D / DirectWrite rasteriser for [`crate::RichTextPopup`]
//!
//! Content painting is shared with other backends via
//! [`crate::primitives::rich_text_popup::native_surface_paint::paint`].
//! This backend implements selection background, bold text, focused-link
//! underline, and link hit regions. The shared paint function also handles
//! content-area clipping and scrollbar geometry that match other backends.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod rich_text_popup;` and
//! `backend.rs`'s module docs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::rich_text_popup::{
    native_surface_paint, RichTextPopup, RichTextPopupLayout,
};
use crate::theme::Theme;

/// Draw a [`RichTextPopup`] at its resolved `layout`. Returns per-link
/// hit regions `(Rect, url)`.
pub fn draw_rich_text_popup(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    popup: &RichTextPopup,
    layout: &RichTextPopupLayout,
) -> Vec<(Rect, String)> {
    let theme = Theme::default();
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    native_surface_paint::paint(popup, layout, &mut surface, &theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::rich_text_popup::{PopupPlacement, RichTextLink, RichTextPopupMeasure};
    use crate::types::{StyledText, WidgetId};
    use crate::win::testing::HeadlessSurface;

    fn popup() -> RichTextPopup {
        RichTextPopup {
            id: WidgetId::new("rtp"),
            lines: vec![StyledText::plain("see docs")],
            line_text: vec!["see docs".into()],
            line_scales: vec![],
            scroll_top: 0,
            max_visible_rows: 10,
            has_focus: false,
            selection: None,
            links: vec![RichTextLink {
                line: 0,
                start_byte: 4,
                end_byte: 8,
                url: "https://example.com".into(),
            }],
            focused_link: None,
            placement: PopupPlacement::Below,
            padding: 2.0,
            fg: None,
            bg: None,
        }
    }

    #[test]
    fn paints_and_returns_link_hit_regions() {
        let surface = HeadlessSurface::new(300, 200).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let p = popup();
        let viewport = Rect::new(0.0, 0.0, 300.0, 200.0);
        let measure = RichTextPopupMeasure::new(200.0, 16.0);
        let layout = p.layout(20.0, 100.0, viewport, measure, |_, s, e| (e - s) as f32);

        let mut links = Vec::new();
        surface
            .paint(|target| {
                links = draw_rich_text_popup(target, &dwrite, &p, &layout);
            })
            .expect("paint rich text popup");

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].1, "https://example.com");

        let theme = Theme::default();
        let b = layout.bounds;
        let inner = surface.pixel_at((b.x + b.width - 3.0) as u32, (b.y + b.height - 3.0) as u32);
        assert_eq!(
            (inner.r, inner.g, inner.b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b)
        );
    }

    /// The scrollbar now paints the same wider,
    /// [`native_surface_paint::SB_WIDTH`]-wide translucent bar GTK/macOS
    /// paint — pre-migration this rasteriser painted the primitive's
    /// bare 1-unit `layout.scrollbar.track` verbatim, fully opaque.
    #[test]
    fn scrollbar_track_paints_wider_than_the_layouts_bare_track() {
        let surface = HeadlessSurface::new(300, 300).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        // Many short lines force a scrollbar (max_visible_rows < total).
        let p = RichTextPopup {
            id: WidgetId::new("rtp"),
            lines: (0..30)
                .map(|i| StyledText::plain(format!("l{i}")))
                .collect(),
            line_text: (0..30).map(|i| format!("l{i}")).collect(),
            line_scales: vec![],
            scroll_top: 0,
            max_visible_rows: 5,
            has_focus: false,
            selection: None,
            links: vec![],
            focused_link: None,
            placement: PopupPlacement::Below,
            padding: 2.0,
            fg: None,
            bg: None,
        };
        let viewport = Rect::new(0.0, 0.0, 300.0, 300.0);
        let measure = RichTextPopupMeasure::new(100.0, 16.0);
        let layout = p.layout(20.0, 20.0, viewport, measure, |_, s, e| (e - s) as f32);
        let sb = layout
            .scrollbar
            .expect("scrollbar present for overflowing content");

        surface
            .paint(|target| {
                let _ = draw_rich_text_popup(target, &dwrite, &p, &layout);
            })
            .expect("paint rich text popup");

        let theme = Theme::default();
        // The bare `layout.scrollbar.track` is only 1 unit wide, sitting
        // flush against the popup's right border. The painted (wider)
        // bar starts `SB_WIDTH + SB_INSET` in from that edge — probe a
        // point inside the wider bar but well outside the bare 1-unit
        // track to confirm the painted geometry is really wider.
        let wide_x = (sb.track.x - native_surface_paint::SB_WIDTH / 2.0) as u32;
        let y = (sb.track.y + 2.0) as u32;
        let px = surface.pixel_at(wide_x, y);
        // Translucent muted_fg over the popup bg — must differ from
        // plain hover_bg (i.e. the track actually painted something
        // here, not just background).
        assert_ne!(
            (px.r, px.g, px.b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
            "scrollbar track should paint wider than the layout's bare 1-unit track"
        );
    }
}
