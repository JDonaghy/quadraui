//! Direct2D / DirectWrite rasteriser for [`crate::primitives::toolbar::Toolbar`]
//! (#730).
//!
//! [`win_toolbar_layout`] stays here: [`Toolbar::layout`] (the D6 layout
//! API) does every positioning decision via the shared
//! [`crate::primitives::toolbar::measure_button`] formula (#730), this
//! module just supplies the DirectWrite measurer. Content painting
//! moved to the shared
//! [`crate::primitives::toolbar::native_surface_paint::paint`] (#1081,
//! `PaintSurface` Phase 4 slice 5/8), which also **closes this
//! backend's own square-corner gap** described below under "Scope for
//! #730" — that scope note is now stale: issue #1073 added
//! [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]
//! to every pixel backend (including this one, via
//! [`super::text::fill_rounded_rect`]), so the hover/pressed/active
//! highlight now paints a real rounded pill instead of a plain
//! rectangle. See that verb's own doc, and
//! `native_surface_paint::paint`'s module doc, for the full drift it
//! resolved. The focus ring stays a **square** stroke on all three
//! backends — [`PaintSurface`] has no rounded-stroke verb.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod toolbar;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! ## Per-state colouring
//!
//! Priority (highest first): pressed → hovered → focused → is_active →
//! enabled — identical to `gtk::toolbar` / `macos::toolbar`. See
//! [`crate::primitives::toolbar::native_surface_paint::paint`]'s module
//! doc for the full table.
//!
//! `bar_bg` is `Toolbar.bg.unwrap_or(theme.header_bg)`. `WinBackend` does
//! not yet carry a live [`Theme`] (see `win::status_bar`'s module doc),
//! so this rasteriser uses [`Theme::default`], the same posture every
//! other `win::` chrome rasteriser takes.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::DWrite;
use crate::event::Rect;
use crate::primitives::toolbar::{
    measure_button, native_surface_paint, Toolbar, ToolbarItemMeasure, ToolbarLayout,
    ToolbarPaintOptions,
};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Compute the Win-GUI pixel/DIP layout for a [`Toolbar`] without
/// painting — the DirectWrite twin of [`draw_toolbar`]'s internal layout
/// call.
///
/// Coordinate frame: **ABSOLUTE** (`rect.x`/`rect.y` baked into every
/// item's `bounds`), matching [`crate::Backend::toolbar_layout`]'s
/// documented contract and `gtk_toolbar_layout` / `mac_toolbar_layout`.
///
/// `dwrite` itself is a [`crate::primitives::layout_metrics::TextMeasure`]
/// (issue #1078) — no wrapper struct needed; see `win::text::DWrite`'s
/// `TextMeasure` impl doc for why the former per-module wrapper
/// (`win::form`/`win::toolbar`'s each having their own `DWriteMeasure`)
/// was deleted.
pub fn win_toolbar_layout(dwrite: &DWrite, rect: Rect, bar: &Toolbar) -> ToolbarLayout {
    bar.layout(rect.x, rect.y, rect.width, rect.height, |btn| {
        ToolbarItemMeasure::new(measure_button(dwrite, btn))
    })
}

/// Draw a [`Toolbar`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`ToolbarLayout`] for host click dispatch.
///
/// Equivalent to [`draw_toolbar_with_options`] with
/// [`ToolbarPaintOptions::default()`] — kept as a separate, **unchanged**
/// function so every existing caller of this re-exported `pub fn` keeps
/// compiling untouched (`CLAUDE.md`'s *Downstream consumers* rule 2).
pub fn draw_toolbar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &Toolbar,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> ToolbarLayout {
    draw_toolbar_with_options(
        target,
        dwrite,
        rect,
        bar,
        hovered_id,
        pressed_id,
        ToolbarPaintOptions::default(),
    )
}

/// [`draw_toolbar`], plus [`ToolbarPaintOptions`]: `options.valign`
/// (issue #260) resolves where button/label text paints within a slot
/// taller than one line.
#[allow(clippy::too_many_arguments)]
pub fn draw_toolbar_with_options(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &Toolbar,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    options: ToolbarPaintOptions,
) -> ToolbarLayout {
    let layout = win_toolbar_layout(dwrite, rect, bar);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    let theme = Theme::default();
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    native_surface_paint::paint(
        bar,
        &layout,
        &mut surface,
        &theme,
        hovered_id,
        pressed_id,
        options,
    );

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{ToolbarButton, ToolbarHit};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 240.0;
    const H: f32 = 40.0;

    fn mk_action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.into(),
            icon: None,
            key_hint: None,
            enabled,
            is_active: false,
            tooltip: String::new(),
        }
    }

    /// Two-button bar, both enabled, no separators/labels — so
    /// `visible_items` order matches `buttons` order 1:1.
    fn sample_bar() -> Toolbar {
        Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("tb:refine", "Refine", true),
                mk_action("tb:drop", "Drop", true),
            ],
            bg: None,
            focused_index: None,
        }
    }

    fn paint_via_backend_at(bar: &Toolbar, x: f32, y: f32) -> ToolbarLayout {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let rect = Rect::new(x, y, W - x, H - y);

        surface
            .paint(|target| {
                draw_toolbar_with_options(
                    target,
                    &dwrite,
                    rect,
                    bar,
                    None,
                    None,
                    ToolbarPaintOptions::default(),
                );
            })
            .map(|_| win_toolbar_layout(&dwrite, rect, bar))
            .expect("paint toolbar")
    }

    /// C0 smoke: `draw_toolbar` must actually paint + return a
    /// click-routable layout rather than panicking or hitting a
    /// `todo!()` (#730's acceptance bar — "draw_toolbar survives C0 on
    /// win").
    #[test]
    fn round_trip_click_hits_enabled_button() {
        let bar = sample_bar();
        let layout = paint_via_backend_at(&bar, 0.0, 0.0);
        assert_eq!(layout.visible_items.len(), 2);

        let refine = &layout.visible_items[0];
        let hit = layout.hit_test(
            refine.bounds.x + refine.bounds.width * 0.5,
            refine.bounds.y + refine.bounds.height * 0.5,
        );
        assert_eq!(
            hit,
            ToolbarHit::Button(WidgetId::new("tb:refine")),
            "expected Refine button hit",
        );
    }

    /// Non-zero-origin regression guard (issue #494 / LESSONS.md "Layout
    /// helpers must return coords in the same frame across backends") —
    /// `Toolbar::layout` bakes `rect.x`/`rect.y` into every item's
    /// `bounds` (ABSOLUTE frame), matching the GTK/macOS/TUI twins.
    #[test]
    fn round_trip_click_hits_enabled_button_at_nonzero_origin() {
        let bar = sample_bar();
        let origin_x = 7.0_f32;
        let origin_y = 13.0_f32;
        let layout = paint_via_backend_at(&bar, origin_x, origin_y);

        let refine = &layout.visible_items[0];
        assert!(
            (refine.bounds.x - origin_x).abs() < 0.01,
            "first button should start at origin_x={}, got {}",
            origin_x,
            refine.bounds.x,
        );
        assert!(
            (refine.bounds.y - origin_y).abs() < 0.01,
            "first button should start at origin_y={}, got {}",
            origin_y,
            refine.bounds.y,
        );

        let hit = layout.hit_test(
            refine.bounds.x + refine.bounds.width * 0.5,
            refine.bounds.y + refine.bounds.height * 0.5,
        );
        assert_eq!(
            hit,
            ToolbarHit::Button(WidgetId::new("tb:refine")),
            "expected Refine button hit at non-zero origin",
        );
    }

    #[test]
    fn disabled_action_not_clickable_after_paint() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "Refine", false)],
            bg: None,
            focused_index: None,
        };
        let layout = paint_via_backend_at(&bar, 0.0, 0.0);
        let r = layout.visible_items[0].bounds;
        assert_eq!(
            layout.hit_test(r.x + 1.0, r.y + 1.0),
            ToolbarHit::Empty,
            "disabled action must not be clickable"
        );
    }

    #[test]
    fn separator_and_label_are_not_clickable() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::Separator,
                ToolbarButton::Label {
                    text: "2 of 5".into(),
                    fg: None,
                },
            ],
            bg: None,
            focused_index: None,
        };
        let layout = paint_via_backend_at(&bar, 0.0, 0.0);
        assert!(!layout.visible_items[0].clickable);
        assert!(!layout.visible_items[1].clickable);
        let r = layout.visible_items[1].bounds;
        assert_eq!(layout.hit_test(r.x, r.y), ToolbarHit::Empty);
    }

    /// No-paint layout must agree byte-for-byte with what `draw_toolbar`
    /// painted — same contract every other `win::` rasteriser's
    /// `no_paint_layout_matches_paint_layout` test proves (see
    /// `win::form`).
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let bar = sample_bar();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_toolbar_with_options(
                    target,
                    &dwrite,
                    rect,
                    &bar,
                    None,
                    None,
                    ToolbarPaintOptions::default(),
                );
            })
            .map(|_| win_toolbar_layout(&dwrite, rect, &bar))
            .expect("paint");
        let no_paint = win_toolbar_layout(&dwrite, rect, &bar);
        assert_eq!(painted, no_paint);
    }

    /// #1081 regression: pre-migration Windows painted the hover/pressed/
    /// active highlight as a plain rectangle (this module's own doc used
    /// to list "no rounded-rect helper exists yet" as a documented scope
    /// gap) — the shared `native_surface_paint::paint` now fills a real
    /// rounded pill via `surface_fill_rounded_rect`. A square fill would
    /// paint the inset rect's own extreme corner pixel; a rounded one
    /// (radius 4) leaves it unpainted, since that pixel sits outside the
    /// corner arc.
    #[test]
    fn active_button_highlight_has_rounded_corners_not_square() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                id: WidgetId::new("tb:on"),
                label: "On".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: true,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
        };
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        surface
            .paint(|target| {
                draw_toolbar_with_options(
                    target,
                    &dwrite,
                    rect,
                    &bar,
                    None,
                    None,
                    ToolbarPaintOptions::default(),
                );
            })
            .expect("paint toolbar");
        let layout = win_toolbar_layout(&dwrite, rect, &bar);

        let theme = Theme::default();
        let item = &layout.visible_items[0].bounds;

        // The inset highlight rect starts at (item.x + 2, item.y + 2);
        // its very corner pixel sits outside a 4px-radius rounded
        // corner arc.
        let corner_x = (item.x + 2.0) as u32;
        let corner_y = (item.y + 2.0) as u32;
        let corner = surface.pixel_at(corner_x, corner_y);
        assert_ne!(
            (corner.r, corner.g, corner.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "highlight corner pixel should NOT be filled — rounded corner (quadraui#1081), not square",
        );

        // The inset rect must still be filled solid in the highlight
        // colour somewhere away from the corners — proves this isn't
        // just "nothing painted". Deliberately NOT the button's exact
        // geometric centre: that's also where the "On" label's own text
        // is centred (`tx = item.x + (item.width - tw) / 2`), and on a
        // real Windows host DirectWrite's ClearType/greyscale
        // anti-aliasing blends glyph ink into the surrounding highlight
        // fill, so the centre pixel is a mix of `theme.foreground` and
        // `theme.selected_bg` — not pure `theme.selected_bg`
        // (quadraui#1081 CI fixup: this exact assertion read
        // `(179, 220, 204)` instead of `theme.selected_bg`'s
        // `(50, 60, 90)` on windows-latest; the macOS twin in
        // `macos::toolbar` needed the same fix for CoreText smoothing).
        // `measure_button`'s `ACTION_H_PAD = 8.0` guarantees the text
        // always starts at `item.x + 8` regardless of font/glyph metrics
        // (`item.width == text_width + 2 * ACTION_H_PAD`, so
        // `tx == item.x + ACTION_H_PAD`), so the strip
        // `[item.x + 2, item.x + 8)` — between the inset's left edge and
        // where the label starts — is always text-free. `+ 5.0` sits in
        // the middle of that strip; the y stays at mid-height,
        // comfortably clear of the top/bottom corner arcs (radius 4 on a
        // 40px-tall bar).
        let cx = (item.x + 5.0) as u32;
        let cy = (item.y + item.height / 2.0) as u32;
        let centre = surface.pixel_at(cx, cy);
        assert_eq!(
            (centre.r, centre.g, centre.b),
            (
                theme.selected_bg.r,
                theme.selected_bg.g,
                theme.selected_bg.b
            ),
            "highlight fill should be solid away from the corners and the label text",
        );
    }

    // ── #260: `ToolbarVAlign` ────────────────────────────────────────────

    /// Multi-row paint test (issue #260's test plan): paint the same bar
    /// into the same tall slot once per [`crate::primitives::toolbar::ToolbarVAlign`]
    /// variant and confirm the row of the first painted (non-background)
    /// pixel moves accordingly — `Top` paints highest, `Bottom` lowest.
    #[test]
    fn multi_row_valign_moves_painted_text_vertically() {
        use crate::primitives::toolbar::ToolbarVAlign;

        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                id: WidgetId::new("tb:go"),
                label: "Go".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
        };
        let theme = Theme::default();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");

        let first_ink_row = |valign: ToolbarVAlign| -> u32 {
            let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
            surface
                .paint(|target| {
                    draw_toolbar_with_options(
                        target,
                        &dwrite,
                        rect,
                        &bar,
                        None,
                        None,
                        ToolbarPaintOptions { valign },
                    );
                })
                .expect("paint toolbar");
            for y in 0..H as u32 {
                for x in 0..W as u32 {
                    let px = surface.pixel_at(x, y);
                    if (px.r, px.g, px.b)
                        != (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b)
                    {
                        return y;
                    }
                }
            }
            panic!("no ink painted for valign {valign:?}");
        };

        let top_row = first_ink_row(ToolbarVAlign::Top);
        let bottom_row = first_ink_row(ToolbarVAlign::Bottom);
        assert!(
            top_row < bottom_row,
            "Top should paint higher than Bottom (top={top_row}, bottom={bottom_row})"
        );
    }
}
