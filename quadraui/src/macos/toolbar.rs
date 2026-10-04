//! macOS rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! `mac_toolbar_layout` stays here — it needs Core Text's own
//! measurement to size each item. Content painting moved to the shared
//! [`crate::primitives::toolbar::native_surface_paint::paint`] (#1081,
//! `PaintSurface` Phase 4 slice 5/8), which also **closes this
//! backend's own square-corner gap**: pre-migration macOS painted the
//! hover/pressed/active highlight as a plain rectangle (Core Graphics
//! has no rounded-rect-fill convenience this file used to reach for).
//! The shared `paint` now fills a real rounded pill via
//! [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]
//! (#1073) — see that fn's module doc for the full drift it resolved.
//! Per D6: layout policy lives in [`crate::primitives::toolbar::Toolbar::layout`];
//! this rasteriser now only builds the [`super::surface::CgSurface`]
//! adapter and delegates paint.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::toolbar::{
    measure_button, native_surface_paint, Toolbar, ToolbarItemMeasure, ToolbarLayout,
    ToolbarPaintOptions,
};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Compute the macOS pixel-unit layout for a [`Toolbar`] without
/// painting. `font` is required for accurate text measurement — it's a
/// [`crate::primitives::layout_metrics::TextMeasure`] itself (issue
/// #1078), so no wrapper struct is needed the way `macos::form`/
/// `macos::toolbar` each used to define their own `CtFontMeasure`.
pub fn mac_toolbar_layout(
    bar: &Toolbar,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> ToolbarLayout {
    bar.layout(x as f32, y as f32, w as f32, h as f32, |btn| {
        ToolbarItemMeasure::new(measure_button(font, btn))
    })
}

/// Paint `bar` into `(x, y, w, h)` on `ctx`. Returns the resolved
/// layout for host click dispatch.
///
/// Equivalent to [`draw_toolbar_with_options`] with
/// [`ToolbarPaintOptions::default()`] — kept as a separate, **unchanged**
/// function so every existing caller of this re-exported `pub fn` keeps
/// compiling untouched (`CLAUDE.md`'s *Downstream consumers* rule 2).
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call (typical: the frame-scope pointer stashed on
/// [`super::MacBackend`]).
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_toolbar(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> ToolbarLayout {
    draw_toolbar_with_options(
        ctx,
        font,
        x,
        y,
        w,
        h,
        bar,
        theme,
        hovered_id,
        pressed_id,
        ToolbarPaintOptions::default(),
    )
}

/// [`draw_toolbar`], plus [`ToolbarPaintOptions`]: `options.valign`
/// (issue #260) resolves where button/label text paints within a slot
/// taller than one line.
///
/// # Safety
///
/// Same contract as [`draw_toolbar`] — `ctx` must be a valid
/// `CGContextRef` borrowed for the duration of the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_toolbar_with_options(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    options: ToolbarPaintOptions,
) -> ToolbarLayout {
    let layout = mac_toolbar_layout(bar, font, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    // `ns_push_clip`/`ns_pop_clip` bracket a `CGContextSaveGState`/
    // `CGContextClipToRect`/`CGContextRestoreGState` triple — see their
    // own docs. Matches this fn's pre-migration single save/clip/restore
    // exactly.
    super::backend::ns_push_clip(
        ctx,
        crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32),
    );

    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    native_surface_paint::paint(
        bar,
        &layout,
        &mut surface,
        theme,
        hovered_id,
        pressed_id,
        options,
    );

    super::backend::ns_pop_clip(ctx);
    layout
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::toolbar::{ToolbarButton, ToolbarHit};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 40;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed on every macOS host")
    }

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
    /// `visible_items` order matches `buttons` order 1:1 and index 0
    /// is always the "Refine" action.
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

    /// Paint a bar through the full `MacBackend::draw_toolbar` path at
    /// origin `(0, 0)` and return both the surface and the resolved
    /// layout (for hit_test). Establishes the harness shape every
    /// chrome rasteriser test in this crate follows (see e.g.
    /// `macos::panel::tests::paint_via_backend`).
    fn paint_via_backend(bar: &Toolbar) -> (BitmapSurface, ToolbarLayout) {
        paint_via_backend_at(bar, 0.0, 0.0)
    }

    /// Like [`paint_via_backend`] but paints at an arbitrary `(x, y)`
    /// origin. Unlike `StatusBar`/`TextDisplay` in this same module
    /// family (bar/body-LOCAL layouts — see their doc comments),
    /// `Toolbar::layout` bakes `x`/`y` straight into each item's
    /// `bounds` (ABSOLUTE frame, matching `mac_toolbar_layout`'s doc
    /// comment). This lets tests confirm painted buttons and the
    /// returned layout still agree at a non-zero origin.
    fn paint_via_backend_at(bar: &Toolbar, x: f32, y: f32) -> (BitmapSurface, ToolbarLayout) {
        paint_via_backend_with_options(
            bar,
            x,
            y,
            crate::primitives::toolbar::ToolbarPaintOptions::default(),
        )
    }

    /// Like [`paint_via_backend_at`] but with an explicit
    /// [`crate::primitives::toolbar::ToolbarPaintOptions`] — issue #260's
    /// `valign` multi-row paint tests use this to paint the same bar at
    /// the same slot once per [`crate::primitives::toolbar::ToolbarVAlign`]
    /// variant.
    fn paint_via_backend_with_options(
        bar: &Toolbar,
        x: f32,
        y: f32,
        options: crate::primitives::toolbar::ToolbarPaintOptions,
    ) -> (BitmapSurface, ToolbarLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_toolbar_with_options(
                QRect::new(x, y, W as f32 - x, H as f32 - y),
                bar,
                &crate::InteractionState::new(),
                options,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn round_trip_click_hits_enabled_button() {
        // Paint at origin (0, 0), then hit-test the centre of the
        // first visible ("Refine") button's painted bounds. Assert the
        // layout reports a hit on that button's id.
        let bar = sample_bar();
        let (_surface, layout) = paint_via_backend(&bar);
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

    #[test]
    fn round_trip_click_hits_enabled_button_at_nonzero_origin() {
        // Regression for quadraui#494 / LESSONS.md "Layout helpers must
        // return coords in the same frame across backends": the
        // origin-(0,0) test above can't distinguish a `Toolbar::layout`
        // that (correctly) bakes `x`/`y` into `bounds` from one that
        // (incorrectly) ignores them — both look identical when
        // x == y == 0. Paint at a non-zero origin and confirm both the
        // painted button position and the `hit_test` round trip agree
        // on the same absolute frame. This file has no `#[cfg(test)]`
        // module prior to quadraui#494; unlike sibling files' nonzero-
        // origin regressions this isn't a refactor of a pre-existing
        // test, so it's written fresh per LESSONS.md's guidance and is
        // unverified by the compiler in this sandbox (no macOS target).
        let bar = sample_bar();
        let origin_x = 7.0_f32;
        let origin_y = 13.0_f32;
        let (_surface, layout) = paint_via_backend_at(&bar, origin_x, origin_y);

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

    /// #1081 regression: pre-migration macOS painted the hover/pressed/
    /// active highlight as a plain square rect (Core Graphics had no
    /// rounded-fill convenience this file reached for) — the shared
    /// `native_surface_paint::paint` now fills a real rounded pill via
    /// `surface_fill_rounded_rect`. A square fill would paint the inset
    /// rect's own extreme corner pixel; a rounded one (radius 4) leaves
    /// it unpainted, since that pixel sits outside the corner arc.
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
        let (surface, layout) = paint_via_backend(&bar);
        let theme = Theme::default();
        let item = &layout.visible_items[0];

        // The inset highlight rect starts at (item.x + 2, item.y + 2);
        // its very corner pixel sits outside a 4px-radius rounded
        // corner arc.
        let corner_x = (item.bounds.x + 2.0) as u32;
        let corner_y = (item.bounds.y + 2.0) as u32;
        let (r, g, b, _) = surface.pixel(corner_x, corner_y);
        assert_ne!(
            (r, g, b),
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
        // geometric centre: that's also where the "On" label's own
        // text is centred (`tx = item.x + (item.width - tw) / 2`), and
        // on a real macOS host CoreText's font smoothing anti-aliases
        // glyph ink into the surrounding highlight fill, so the centre
        // pixel is a blend of `theme.foreground` and `theme.selected_bg`
        // — not pure `theme.selected_bg` (quadraui#1081 smoke-test
        // fixup: this exact assertion previously read `(195, 196, 201)`
        // instead of `theme.selected_bg`'s `(50, 60, 90)` on a real Mac).
        // `measure_button`'s `ACTION_H_PAD = 8.0` guarantees the text
        // always starts `item.x + 8` regardless of font/glyph metrics
        // (`item.width == text_width + 2 * ACTION_H_PAD`, so
        // `tx == item.x + ACTION_H_PAD`), so the strip
        // `[item.x + 2, item.x + 8)` — between the inset's left edge
        // and where the label starts — is always text-free. `+ 5.0`
        // sits in the middle of that strip; the y stays at mid-height,
        // comfortably clear of the top/bottom corner arcs (radius 4 on
        // a 40px-tall bar).
        let cx = (item.bounds.x + 5.0) as u32;
        let cy = (item.bounds.y + item.bounds.height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(cx, cy);
        assert_eq!(
            (r, g, b),
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

        let first_ink_row = |valign: ToolbarVAlign| -> u32 {
            let (surface, _layout) = paint_via_backend_with_options(
                &bar,
                0.0,
                0.0,
                crate::primitives::toolbar::ToolbarPaintOptions { valign },
            );
            for y in 0..H {
                for x in 0..W {
                    let (r, g, b, _) = surface.pixel(x, y);
                    if (r, g, b) != (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b) {
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
