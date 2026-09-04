//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::sidebar_panel::SidebarPanel`] (#731).
//!
//! Mirrors `gtk::sidebar_panel` / `tui::sidebar_panel`'s structure and
//! visual contract: [`SidebarPanel::layout`] (the D6 layout API) does
//! every positioning decision — this module only measures (via the
//! shared [`super::toolbar::DWriteMeasure`] adapter) and paints, by
//! delegating the toolbar slot to [`super::toolbar::draw_toolbar`]. The
//! content region is **not** painted here — it is returned in
//! `SidebarPanelLayout.content_bounds` for the host to draw into
//! (mirrors the existing `draw_panel` contract, and the GTK/TUI/macOS
//! `sidebar_panel` rasterisers).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod sidebar_panel;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use super::toolbar::DWriteMeasure;
use crate::event::Rect;
use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::{measure_button, ToolbarItemMeasure};
use crate::types::WidgetId;

/// Compute the Win-GUI pixel/DIP layout for a [`SidebarPanel`] without
/// painting — the DirectWrite twin of [`draw_sidebar_panel`]'s internal
/// layout call. No geometry is re-derived here: this delegates entirely
/// to [`SidebarPanel::layout`] (#731's acceptance bar).
///
/// Coordinate frame: **ABSOLUTE** (`rect.x`/`rect.y` baked into
/// `content_bounds` / `toolbar_bounds`), matching
/// [`crate::Backend::sidebar_panel_layout`]'s documented contract and
/// `gtk_sidebar_panel_layout` / the TUI/macOS twins.
pub fn win_sidebar_panel_layout(
    dwrite: &DWrite,
    line_height: f32,
    rect: Rect,
    panel: &SidebarPanel,
) -> SidebarPanelLayout {
    let measure = DWriteMeasure(dwrite);
    panel.layout(rect, SidebarPanelMeasure::new(line_height, 0.0), |btn| {
        ToolbarItemMeasure::new(measure_button(&measure, btn))
    })
}

/// Draw a [`SidebarPanel`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`SidebarPanelLayout`] for the host to paint content into
/// `content_bounds` and route clicks via `hit_test`.
#[allow(clippy::too_many_arguments)]
pub fn draw_sidebar_panel(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    line_height: f32,
    rect: Rect,
    panel: &SidebarPanel,
    hovered_toolbar_id: Option<&WidgetId>,
    pressed_toolbar_id: Option<&WidgetId>,
) -> SidebarPanelLayout {
    let layout = win_sidebar_panel_layout(dwrite, line_height, rect, panel);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    if let (Some(bar), Some(tb_bounds)) = (&panel.toolbar, layout.toolbar_bounds) {
        let _ = super::toolbar::draw_toolbar(
            target,
            dwrite,
            tb_bounds,
            bar,
            hovered_toolbar_id,
            pressed_toolbar_id,
        );
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::sidebar_panel::SidebarPanelHit;
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 240.0;
    const H: f32 = 120.0;

    fn mk_action(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.into(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn panel_with_toolbar() -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![mk_action("a", "Refine"), mk_action("b", "Drop")],
                bg: None,
                focused_index: None,
            }),
            toolbar_height: None,
        }
    }

    fn panel_without_toolbar() -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: None,
            toolbar_height: None,
        }
    }

    /// C0 smoke: `draw_sidebar_panel` must actually paint + return a
    /// click-routable layout rather than panicking or hitting a
    /// `todo!()` (#731's acceptance bar — "draw_sidebar_panel survives
    /// C0 with text_ok on win").
    #[test]
    fn text_ok_round_trip_click_hits_toolbar_button() {
        let panel = panel_with_toolbar();
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, line_height) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, W, H);

        let layout = surface
            .paint(|target| {
                draw_sidebar_panel(target, &dwrite, line_height, rect, &panel, None, None);
            })
            .map(|_| win_sidebar_panel_layout(&dwrite, line_height, rect, &panel))
            .expect("paint sidebar panel");

        let tb = layout.toolbar_bounds.expect("toolbar slot reserved");
        let hit = layout.hit_test(tb.x + 2.0, tb.y + tb.height / 2.0);
        assert_eq!(
            hit,
            SidebarPanelHit::ToolbarButton(WidgetId::new("a")),
            "expected first toolbar button hit"
        );
    }

    /// No-toolbar case: content gets the full rect, and a click inside
    /// it round-trips to content-local coordinates.
    #[test]
    fn no_toolbar_click_hits_content_local_coords() {
        let panel = panel_without_toolbar();
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, line_height) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(10.0, 5.0, W - 10.0, H - 5.0);

        let layout = surface
            .paint(|target| {
                draw_sidebar_panel(target, &dwrite, line_height, rect, &panel, None, None);
            })
            .map(|_| win_sidebar_panel_layout(&dwrite, line_height, rect, &panel))
            .expect("paint sidebar panel");

        assert!(layout.toolbar_bounds.is_none());
        assert_eq!(layout.content_bounds, rect);

        match layout.hit_test(rect.x + 3.0, rect.y + 4.0) {
            SidebarPanelHit::Content { x, y } => {
                assert!((x - 3.0).abs() < 0.01 && (y - 4.0).abs() < 0.01);
            }
            other => panic!("expected Content hit, got {other:?}"),
        }
    }

    /// No-paint layout must agree byte-for-byte with what
    /// `draw_sidebar_panel` painted — same contract every other `win::`
    /// rasteriser's `no_paint_layout_matches_paint_layout` test proves
    /// (see `win::toolbar`, `win::form`).
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let panel = panel_with_toolbar();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, line_height) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_sidebar_panel(target, &dwrite, line_height, rect, &panel, None, None);
            })
            .map(|_| win_sidebar_panel_layout(&dwrite, line_height, rect, &panel))
            .expect("paint");
        let no_paint = win_sidebar_panel_layout(&dwrite, line_height, rect, &panel);
        assert_eq!(painted, no_paint);
    }
}
