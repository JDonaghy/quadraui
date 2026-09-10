//! Direct2D / DirectWrite rasteriser for
//! [`crate::primitives::sidebar_panel::SidebarPanel`] (#731).
//!
//! Painting moved to the shared
//! [`crate::primitives::sidebar_panel::native_surface_paint::paint`]
//! (#862, `NativeSurface` Phase 2d slice 5/9) — see that fn's module doc
//! for the four named divergences found while unifying
//! `gtk::draw_sidebar_panel`, `macos::sidebar_panel::draw_sidebar_panel`
//! and `win::sidebar_panel::draw_sidebar_panel` into one implementation
//! (including divergence 4: this backend never wired a live `Theme`
//! through to the embedded toolbar — preserved as-is, not fixed here).
//! This module now only carries [`win_sidebar_panel_layout`] (pure
//! layout, still needed by `WinBackend::sidebar_panel_layout` for
//! no-paint hit-test queries), [`RawSidebarPanelSurface`] and the
//! deprecated [`draw_sidebar_panel`] compatibility shim over it,
//! mirroring `win::panel`'s identical #859 shape.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod sidebar_panel;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use super::toolbar::DWriteMeasure;
use crate::event::Rect;
use crate::native_surface::NativeSurface;
use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
use crate::primitives::toolbar::{measure_button, ToolbarItemMeasure};
use crate::theme::Theme;
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
///
/// `nerd_fonts_enabled` (issue #913 review fix) resolves the embedded
/// toolbar's per-button [`crate::types::Icon`] overrides
/// (`Toolbar::with_icon_override`) the same way
/// [`crate::primitives::sidebar_panel::native_surface_paint::paint`]
/// does, so this no-paint layout's hit regions always agree with what
/// actually painted. Before this fix this function ignored the flag
/// entirely — a registered override was measured at its raw, unresolved
/// width no matter what actually painted. A `Toolbar` with no overrides
/// is unaffected by the flag.
pub fn win_sidebar_panel_layout(
    dwrite: &DWrite,
    line_height: f32,
    rect: Rect,
    panel: &SidebarPanel,
    nerd_fonts_enabled: bool,
) -> SidebarPanelLayout {
    let measure = DWriteMeasure(dwrite);
    panel.layout(rect, SidebarPanelMeasure::new(line_height, 0.0), |btn| {
        let resolved = match &panel.toolbar {
            Some(bar) => bar.resolve_button_icon(btn, nerd_fonts_enabled),
            None => std::borrow::Cow::Borrowed(btn),
        };
        ToolbarItemMeasure::new(measure_button(&measure, &resolved))
    })
}

/// Minimal [`NativeSurface`] adapter over a bare `(&ID2D1RenderTarget,
/// &DWrite)` pair, used only by the deprecated [`draw_sidebar_panel`]
/// shim below. The shared paint calls `surface_fill_rect`,
/// `surface_stroke_rect`, `surface_measure_text`,
/// `surface_draw_text_run`, `surface_draw_line` and
/// `surface_push_clip`/`surface_pop_clip`; every other method is
/// `unreachable!()`. Mirrors `win::panel::RawPanelSurface`'s identical
/// #859 pattern.
pub(crate) struct RawSidebarPanelSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
    pub(crate) dwrite: &'a DWrite,
}

impl NativeSurface for RawSidebarPanelSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawSidebarPanelSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawSidebarPanelSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawSidebarPanelSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawSidebarPanelSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawSidebarPanelSurface has no backend char width")
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        self.dwrite.measure_text(text).unwrap_or((0.0, 0.0))
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        let _ = super::text::fill_rect(self.target, rect, color);
    }

    fn surface_stroke_rect(&mut self, rect: crate::Rect, color: crate::Color, stroke_width: f32) {
        let _ = super::text::stroke_rect(self.target, rect, color, stroke_width);
    }

    fn surface_draw_text_run(&mut self, rect: crate::Rect, text: &str, color: crate::Color) {
        let _ = self.dwrite.draw_text(self.target, text, rect, color);
    }

    fn surface_draw_line(
        &mut self,
        from: crate::Point,
        to: crate::Point,
        color: crate::Color,
        stroke_width: f32,
    ) {
        let _ =
            super::text::draw_line(self.target, from.x, from.y, to.x, to.y, color, stroke_width);
    }

    fn surface_push_clip(&mut self, rect: crate::Rect) {
        super::text::push_clip(self.target, rect);
    }

    fn surface_pop_clip(&mut self) {
        super::text::pop_clip(self.target);
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("SidebarPanel::paint never draws an image")
    }
}

/// Deprecated free-function shim (#862, CLAUDE.md rule 8): reproduces
/// the pre-#862 signature exactly (no `Theme` param — this backend never
/// took one for `SidebarPanel`'s embedded toolbar; see this module's
/// doc, divergence 4) for any external caller that held a direct
/// `quadraui::win::draw_sidebar_panel` reference rather than going
/// through [`crate::Backend::draw_sidebar_panel`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_sidebar_panel` instead — this free function is a compatibility shim over the shared #862 implementation"
)]
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
    let theme = Theme::default();
    let mut surface = RawSidebarPanelSurface { target, dwrite };
    // `false`: this deprecated shim reproduces the pre-#862 signature
    // exactly (see its doc above), which predates `nerd_fonts_enabled`
    // entirely — there's no flag for a caller of this shim to have
    // passed. `false` matches the fallback-only behaviour every such
    // caller already observed.
    crate::primitives::sidebar_panel::native_surface_paint::paint(
        panel,
        &mut surface,
        &theme,
        rect,
        line_height,
        hovered_toolbar_id,
        pressed_toolbar_id,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::sidebar_panel::SidebarPanelHit;
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 240.0;
    const H: f32 = 120.0;

    /// Paint `panel` via the shared
    /// [`crate::primitives::sidebar_panel::native_surface_paint::paint`]
    /// through a [`RawSidebarPanelSurface`] over `surface`'s headless
    /// target — the same adapter the deprecated [`draw_sidebar_panel`]
    /// shim uses, exercised here directly so these tests don't trip the
    /// `-D warnings`-denied `deprecated` lint (CLAUDE.md rule 3; mirrors
    /// `win::panel`'s and `win::scrollbar`'s identical test-migration
    /// note).
    ///
    /// `nerd_fonts_enabled` (issue #913 review fix) is forwarded
    /// unchanged to the shared paint — see that fn's doc for its
    /// contract.
    #[allow(clippy::too_many_arguments)]
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        line_height: f32,
        rect: Rect,
        panel: &SidebarPanel,
        hovered_toolbar_id: Option<&WidgetId>,
        pressed_toolbar_id: Option<&WidgetId>,
        nerd_fonts_enabled: bool,
    ) -> SidebarPanelLayout {
        let theme = Theme::default();
        surface
            .paint(|target| {
                let mut raw = RawSidebarPanelSurface { target, dwrite };
                crate::primitives::sidebar_panel::native_surface_paint::paint(
                    panel,
                    &mut raw,
                    &theme,
                    rect,
                    line_height,
                    hovered_toolbar_id,
                    pressed_toolbar_id,
                    nerd_fonts_enabled,
                );
            })
            .map(|_| win_sidebar_panel_layout(dwrite, line_height, rect, panel, nerd_fonts_enabled))
            .expect("paint sidebar panel")
    }

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
                icon_overrides: Vec::new(),
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

        let layout = paint(
            &surface,
            &dwrite,
            line_height,
            rect,
            &panel,
            None,
            None,
            false,
        );

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

        let layout = paint(
            &surface,
            &dwrite,
            line_height,
            rect,
            &panel,
            None,
            None,
            false,
        );

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

        let painted = paint(
            &surface,
            &dwrite,
            line_height,
            rect,
            &panel,
            None,
            None,
            false,
        );
        let no_paint = win_sidebar_panel_layout(&dwrite, line_height, rect, &panel, false);
        assert_eq!(painted, no_paint);
    }

    /// Second-round review fix: `no_paint_layout_matches_paint_layout`
    /// above only ever compared the two layouts at `nerd_fonts_enabled:
    /// false`, which can never observe a width divergence — an override
    /// resolves to `Icon::fallback` on both paths regardless. This test
    /// re-runs the identical byte-for-byte comparison at `true`, with a
    /// registered override whose glyph and fallback measure to
    /// different widths, closing the gap: `win_sidebar_panel_layout`
    /// used to ignore `nerd_fonts_enabled` entirely (it wasn't even a
    /// parameter), so `painted` and `no_paint` disagreed the instant an
    /// override was registered and the flag was on — clicks past the
    /// overridden button would have landed on the wrong hit region.
    #[test]
    fn no_paint_layout_matches_paint_layout_with_glyph_override() {
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(
                Toolbar::new(
                    WidgetId::new("sb:toolbar"),
                    vec![mk_action("a", "Refine"), mk_action("b", "Drop")],
                )
                .with_icon_override(
                    WidgetId::new("a"),
                    crate::types::Icon::new("\u{f021}\u{f021}\u{f021}", "R"),
                ),
            ),
            toolbar_height: None,
        };
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, line_height) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = paint(
            &surface,
            &dwrite,
            line_height,
            rect,
            &panel,
            None,
            None,
            true,
        );
        let no_paint = win_sidebar_panel_layout(&dwrite, line_height, rect, &panel, true);
        assert_eq!(painted, no_paint);
    }

    /// Issue #913 review fix: closes the "no test exercises an embedded
    /// toolbar with a registered override" gap for Win-GUI — the shared
    /// `native_surface_paint::paint` this module delegates to measures
    /// and paints via `Toolbar::resolve_button_icon`, so a registered
    /// override must change the measured button width once
    /// `nerd_fonts_enabled` flips, exactly like every other backend's
    /// `nerd_fonts_flag_selects_glyph_or_fallback`-style test.
    #[test]
    fn nerd_fonts_enabled_changes_measured_toolbar_button_width() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let measure = DWriteMeasure(&dwrite);
        let bar = Toolbar::new(WidgetId::new("tb"), vec![mk_action("a", "Refine")])
            .with_icon_override(
                WidgetId::new("a"),
                crate::types::Icon::new("\u{f021}\u{f021}\u{f021}", "R"),
            );
        let btn = &bar.buttons[0];

        let glyph_btn = bar.resolve_button_icon(btn, true);
        let fallback_btn = bar.resolve_button_icon(btn, false);

        let glyph_w = measure_button(&measure, &glyph_btn);
        let fallback_w = measure_button(&measure, &fallback_btn);

        assert!(
            glyph_w > fallback_w,
            "a 3-glyph Nerd-Font icon should measure wider than the 1-char \
             fallback: glyph_w={glyph_w}, fallback_w={fallback_w}"
        );
    }
}
