//! macOS rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! `mac_toolbar_layout` stays here — it needs Core Text's own
//! measurement to size each item. Content painting uses the shared
//! [`crate::primitives::toolbar::native_surface_paint::paint`], which
//! fills hover/pressed/active highlights via
//! [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`].
//! Per D6: layout policy lives in [`crate::primitives::toolbar::Toolbar::layout`];
//! this rasteriser now only builds the [`super::surface::CgSurface`]
//! adapter and delegates paint.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::toolbar::{
    measure_button, native_surface_paint, Toolbar, ToolbarItemMeasure, ToolbarLayout,
    ToolbarLayoutOptions, ToolbarOrientation, ToolbarOverflow, ToolbarOverflowPriorities,
    ToolbarPaintOptions,
};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Compute the macOS pixel-unit layout for a [`Toolbar`] without
/// painting. `font` is required for accurate text measurement.
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
/// resolves where button/label text paints within a slot
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

/// Compute the macOS pixel-unit layout for a [`Toolbar`] without painting,
/// honouring [`ToolbarLayoutOptions::overflow`]. `h` is the **slot** a host
/// allocated the whole bar — same contract as [`mac_toolbar_layout`]'s own
/// `h` — not a per-row height: [`ToolbarOverflow::Wrap`] always stacks
/// wrapped lines at `row_h` each (a pixel backend has no terminal-cell
/// grid to pin a line to, so `row_h` plays the role
/// [`crate::tui::toolbar::tui_toolbar_layout_with_overflow`]'s hardcoded
/// `1.0` line height plays there), and any item whose row starts at or
/// past `y + h` is marked un-clickable so hit-testing can't claim a
/// button [`draw_toolbar_with_overflow`] never actually paints inside the
/// slot. Read [`ToolbarLayout::cross_extent`] on the result to learn how
/// tall the full wrapped stack actually needs were a host to grow `h` to
/// fit every row. [`ToolbarOverflow::Clip`]/[`ToolbarOverflow::Menu`] keep
/// `h` as the single row's height (`row_h` is unused), same as
/// [`mac_toolbar_layout`].
#[allow(clippy::too_many_arguments)]
pub fn mac_toolbar_layout_with_overflow(
    bar: &Toolbar,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    row_h: f64,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    let line_height = match options.overflow {
        ToolbarOverflow::Wrap => row_h.max(1.0),
        _ => h,
    } as f32;

    let mut layout = bar.layout_with_overflow(
        x as f32,
        y as f32,
        w as f32,
        line_height,
        options,
        priorities,
        |btn| ToolbarItemMeasure::new(measure_button(font, btn)),
    );

    // Mirrors `tui_toolbar_layout_with_overflow`'s own demotion: a row
    // stacked at or past the slot's bottom edge was never going to be
    // painted (`draw_toolbar_with_overflow` clips to `(x, y, w, h)`), so
    // hit-testing must not report a hit on it either. `Vertical` wrapping
    // has no analogous "ran past the bottom" edge along this axis — same
    // scope TUI's own demotion keeps.
    if options.overflow == ToolbarOverflow::Wrap
        && options.orientation == ToolbarOrientation::Horizontal
    {
        let area_bottom = y as f32 + h as f32;
        for vis in &mut layout.visible_items {
            if vis.bounds.y >= area_bottom {
                vis.clickable = false;
                for child in &mut vis.group_children {
                    child.clickable = false;
                }
            }
        }
    }

    layout
}

/// [`draw_toolbar_with_options`], plus [`ToolbarLayoutOptions`]:
/// `options.overflow` picks how items that don't fit are handled — mirrors
/// `tui::toolbar::draw_toolbar_with_overflow`'s own per-variant contract:
///
/// - [`ToolbarOverflow::Clip`] paints the same single-row geometry
///   [`draw_toolbar_with_options`] would for `options.orientation`'s
///   default (`Horizontal`) — for `Vertical` it honours the orientation
///   `draw_toolbar_with_options` always ignores.
/// - [`ToolbarOverflow::Wrap`] paints each wrapped line on its own
///   `row_h`-tall row below `y`, clipped to the slot `(x, y, w, h)` the
///   caller allocated — same clip a terminal's `Buffer` imposes for
///   free, re-created here explicitly since a `CGContextRef` has nothing
///   to run out of on its own. A row (or part of one) that would land
///   past `y + h` is clipped away and its items are marked un-clickable
///   by [`mac_toolbar_layout_with_overflow`] — grow `h` first (sized from
///   that function's [`ToolbarLayout::cross_extent`]) if every row must
///   paint.
/// - [`ToolbarOverflow::Menu`] paints a trailing `[ » ]` cell through the
///   same shared [`native_surface_paint::paint`] every other item uses — a
///   click on it hit-tests to
///   [`crate::primitives::toolbar::ToolbarHit::Overflow`]; the host opens a
///   [`crate::ContextMenu`]/native menu built from
///   [`ToolbarLayout::overflowed_items`].
///
/// `row_h` is the per-row height [`ToolbarOverflow::Wrap`] stacks lines
/// at — see [`mac_toolbar_layout_with_overflow`]'s doc — and is ignored by
/// the other two variants. `priorities` is read only when
/// `options.overflow` is [`ToolbarOverflow::Menu`].
///
/// # Safety
///
/// Same contract as [`draw_toolbar`] — `ctx` must be a valid
/// `CGContextRef` borrowed for the duration of the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_toolbar_with_overflow(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    row_h: f64,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    let layout =
        mac_toolbar_layout_with_overflow(bar, font, x, y, w, h, row_h, options, priorities);

    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    // Clip to the slot the caller allocated — never grow past it, so an
    // out-of-slot `Wrap` row can't paint over whatever the host placed
    // below the bar (the un-clickable items `mac_toolbar_layout_with_overflow`
    // marks past this same edge agree with what's actually visible here).
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
        ToolbarPaintOptions::default(),
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
    use crate::primitives::toolbar::{
        ToolbarButton, ToolbarHit, ToolbarItemKind, TOOLBAR_OVERFLOW_ITEM_IDX,
    };
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 40;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed on every macOS host")
    }

    fn mk_action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
        ToolbarButton::Action {
            spec: None,
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
    /// [`crate::primitives::toolbar::ToolbarPaintOptions`] to test
    /// each [`crate::primitives::toolbar::ToolbarVAlign`] variant.
    fn paint_via_backend_with_options(
        bar: &Toolbar,
        x: f32,
        y: f32,
        options: crate::primitives::toolbar::ToolbarPaintOptions,
    ) -> (BitmapSurface, ToolbarLayout) {
        paint_via_backend_full(bar, x, y, &crate::InteractionState::new(), options).0
    }

    /// Like [`paint_via_backend_with_options`] but reuses one `MacBackend`
    /// across calls (so its per-widget animation state, e.g.
    /// [`super::super::MacBackend`]'s hover-fade map, survives between
    /// paints) and takes an explicit [`crate::InteractionState`]. Returns
    /// the backend alongside the surface/layout so a caller can paint
    /// again — immediately, or after a real sleep — to observe a
    /// transition progress.
    fn paint_via_backend_full(
        bar: &Toolbar,
        x: f32,
        y: f32,
        interaction: &crate::InteractionState,
        options: crate::primitives::toolbar::ToolbarPaintOptions,
    ) -> ((BitmapSurface, ToolbarLayout), MacBackend) {
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let (surface, layout) = paint_once(&mut backend, bar, x, y, interaction, options);
        backend.end_frame();
        ((surface, layout), backend)
    }

    /// Repaint `bar` through an already-begun `backend`, for sampling a
    /// second frame (e.g. after a real sleep) against the same backend's
    /// retained animation state.
    fn repaint(
        backend: &mut MacBackend,
        bar: &Toolbar,
        x: f32,
        y: f32,
        interaction: &crate::InteractionState,
        options: crate::primitives::toolbar::ToolbarPaintOptions,
    ) -> (BitmapSurface, ToolbarLayout) {
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let (surface, layout) = paint_once(backend, bar, x, y, interaction, options);
        backend.end_frame();
        (surface, layout)
    }

    fn paint_once(
        backend: &mut MacBackend,
        bar: &Toolbar,
        x: f32,
        y: f32,
        interaction: &crate::InteractionState,
        options: crate::primitives::toolbar::ToolbarPaintOptions,
    ) -> (BitmapSurface, ToolbarLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_toolbar_with_options(
                QRect::new(x, y, W as f32 - x, H as f32 - y),
                bar,
                interaction,
                options,
            );
            *layout.borrow_mut() = Some(l);
        });
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
        // Test that layout returns coordinates in the same frame as paint:
        // both the painted button position and the `hit_test` round trip
        // must agree on the same absolute frame at a non-zero origin.
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

    /// Pre-migration macOS painted the hover/pressed/
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
                spec: None,
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

        // The inset rect must be filled solid in the highlight colour
        // somewhere away from the corners — proves something was painted.
        // Avoid the button's exact centre (text is centred there too).
        // `measure_button`'s `ACTION_H_PAD = 8.0` means the strip
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

    /// Multi-row paint test: paint the same bar into the same tall slot
    /// once per [`crate::primitives::toolbar::ToolbarVAlign`] variant and
    /// confirm the row of the first painted (non-background) pixel moves
    /// accordingly — `Top` paints highest, `Bottom` lowest.
    #[test]
    fn multi_row_valign_moves_painted_text_vertically() {
        use crate::primitives::toolbar::ToolbarVAlign;

        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                spec: None,
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

    /// `MacBackend::draw_toolbar_with_options` fades a hovered button's
    /// highlight in over `HOVER_FADE_DURATION` instead of snapping it to
    /// full opacity on the first hovered frame: the very first paint of a
    /// newly-hovered button should be visibly dimmer than
    /// `theme.hover_bg`, since the fade has only just started.
    #[test]
    fn hover_highlight_is_dim_on_first_hovered_frame() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("tb:refine", "Refine", true)],
            bg: None,
            focused_index: None,
        };
        let theme = Theme::default();
        let interaction =
            crate::InteractionState::from_parts(Some(WidgetId::new("tb:refine")), None);

        let ((surface, layout), _backend) =
            paint_via_backend_full(&bar, 0.0, 0.0, &interaction, Default::default());
        let item = &layout.visible_items[0];

        // Sample the highlight fill away from the corners/label, matching
        // `active_button_highlight_has_rounded_corners_not_square` above.
        let cx = (item.bounds.x + 5.0) as u32;
        let cy = (item.bounds.y + item.bounds.height / 2.0) as u32;
        let (r, _g, _b, _a) = surface.pixel(cx, cy);
        assert_ne!(
            r, theme.hover_bg.r,
            "first hovered frame should paint a partially faded-in highlight, not the full-opacity colour",
        );
    }

    /// Once a button has been hovered for longer than the fade's
    /// duration, the highlight should match the shared, static
    /// `theme.hover_bg` fill exactly — the animation hands off to the
    /// ordinary binary highlight once it finishes.
    #[test]
    fn hover_highlight_reaches_full_opacity_after_fade_duration() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("tb:refine", "Refine", true)],
            bg: None,
            focused_index: None,
        };
        let theme = Theme::default();
        let interaction =
            crate::InteractionState::from_parts(Some(WidgetId::new("tb:refine")), None);

        let (_first, mut backend) =
            paint_via_backend_full(&bar, 0.0, 0.0, &interaction, Default::default());

        // `HOVER_FADE_DURATION` (120ms) plus generous margin for scheduler
        // jitter — mirrors the sleep-based timing tests elsewhere in this
        // crate (e.g. `tui::backend`, `win::run`).
        std::thread::sleep(std::time::Duration::from_millis(200));

        let (surface, layout) = repaint(
            &mut backend,
            &bar,
            0.0,
            0.0,
            &interaction,
            Default::default(),
        );
        let item = &layout.visible_items[0];
        let cx = (item.bounds.x + 5.0) as u32;
        let cy = (item.bounds.y + item.bounds.height / 2.0) as u32;
        let (r, g, b, _a) = surface.pixel(cx, cy);
        assert_eq!(
            (r, g, b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
            "highlight should reach the static theme colour once the fade-in finishes",
        );
    }

    /// `ToolbarOverflow::Wrap` on a bar too narrow for every button stacks
    /// each one onto its own `row_h`-tall row below `y`; when the slot
    /// `h` the caller allocated covers every row, `draw_toolbar_with_overflow`
    /// paints every one of them (not just the first) — both the per-row
    /// background fill and the button it holds.
    #[test]
    fn wrap_mode_paints_every_row_not_just_the_first() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "AAAA", true),
                mk_action("b", "BBBB", true),
                mk_action("c", "CCCC", true),
            ],
            bg: None,
            focused_index: None,
        };
        let f = font();
        let row_h = 20.0_f64;

        // One button's width at this font, measured via the ordinary
        // Clip-mode layout — a bar exactly that wide forces each of the
        // three buttons onto its own `Wrap` row.
        let clip = mac_toolbar_layout(&bar, &f, 0.0, 0.0, 1000.0, row_h);
        let w = clip.visible_items[0].bounds.width as f64 + 2.0;

        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Wrap);
        let priorities = ToolbarOverflowPriorities::new();

        // The slot covers all three rows, so nothing is demoted.
        let total_h = row_h * 3.0;
        let probe = mac_toolbar_layout_with_overflow(
            &bar,
            &f,
            0.0,
            0.0,
            w,
            total_h,
            row_h,
            options,
            &priorities,
        );
        assert_eq!(
            probe.visible_items.len(),
            3,
            "every button kept, just re-rowed"
        );
        assert!(
            probe.visible_items.iter().all(|v| v.clickable),
            "every row fits inside the slot, so every button stays clickable",
        );
        assert_eq!(
            probe.cross_extent(ToolbarOrientation::Horizontal),
            row_h as f32 * 3.0,
            "three rows of height row_h",
        );

        let surface = BitmapSurface::new(w.ceil() as u32, total_h.ceil() as u32);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let theme = Theme::default();
        // SAFETY: `surface.context_ptr()` is a freshly created, valid
        // `CGContextRef` held alive by `surface` for this call.
        let layout = unsafe {
            draw_toolbar_with_overflow(
                surface.context_ptr(),
                &f,
                0.0,
                0.0,
                w,
                total_h,
                row_h,
                &bar,
                &theme,
                None,
                None,
                options,
                &priorities,
            )
        };

        let bar_bg = (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b);
        for (row, vis) in layout.visible_items.iter().enumerate() {
            // Sample inside the row's own background fill, left of the
            // label text (same text-free strip the hover-highlight tests
            // above sample) — proving the row got a real background fill,
            // not just whatever the untouched bitmap started as.
            let cx = (vis.bounds.x + 5.0) as u32;
            let cy = (vis.bounds.y + vis.bounds.height / 2.0) as u32;
            let (r, g, b, a) = surface.pixel(cx, cy);
            assert_eq!(
                (r, g, b, a),
                (bar_bg.0, bar_bg.1, bar_bg.2, 255),
                "row {row} should be filled with the bar background, not left untouched",
            );
        }

        match layout.hit_test(
            layout.visible_items[2].bounds.x + 1.0,
            layout.visible_items[2].bounds.y + 1.0,
        ) {
            ToolbarHit::Button(id) => assert_eq!(id, WidgetId::new("c")),
            other => panic!("expected the third button on row 2, got {other:?}"),
        }
    }

    /// A `Wrap` slot shorter than every row needs clips to exactly that
    /// slot — never past it — and demotes any row stacked at or beyond the
    /// slot's bottom edge to un-clickable, so `ToolbarLayout::hit_test`
    /// can't claim a button that was never actually painted. Mirrors
    /// `tui::toolbar`'s own `overflow_wrap_hit_test_agrees_with_the_rows_actually_painted`.
    #[test]
    fn wrap_mode_clips_to_the_slot_and_demotes_rows_past_it() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "AAAA", true),
                mk_action("b", "BBBB", true),
                mk_action("c", "CCCC", true),
            ],
            bg: None,
            focused_index: None,
        };
        let f = font();
        let row_h = 20.0_f64;

        let clip = mac_toolbar_layout(&bar, &f, 0.0, 0.0, 1000.0, row_h);
        let w = clip.visible_items[0].bounds.width as f64 + 2.0;

        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Wrap);
        let priorities = ToolbarOverflowPriorities::new();

        // The slot only covers the first two rows — the third button's
        // row starts exactly at the slot's bottom edge.
        let slot_h = row_h * 2.0;
        let probe = mac_toolbar_layout_with_overflow(
            &bar,
            &f,
            0.0,
            0.0,
            w,
            slot_h,
            row_h,
            options,
            &priorities,
        );
        assert_eq!(
            probe.visible_items.len(),
            3,
            "every button kept, just re-rowed"
        );
        assert!(probe.visible_items[0].clickable, "row 0 is inside the slot");
        assert!(probe.visible_items[1].clickable, "row 1 is inside the slot");
        assert!(
            !probe.visible_items[2].clickable,
            "row 2 starts at the slot's bottom edge, never painted, so not clickable",
        );
        assert_eq!(
            probe.hit_test(
                probe.visible_items[2].bounds.x + 1.0,
                probe.visible_items[2].bounds.y + 1.0,
            ),
            ToolbarHit::Empty,
            "a point over the un-clickable row must not resolve to a button",
        );

        // Painting into a surface sized exactly to the slot must not
        // panic or write past its own bounds — proving the clip really
        // stops at `slot_h`, not wherever `cross_extent` says the wrapped
        // content would ideally like to go.
        let surface = BitmapSurface::new(w.ceil() as u32, slot_h.ceil() as u32);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let theme = Theme::default();
        // SAFETY: `surface.context_ptr()` is a freshly created, valid
        // `CGContextRef` held alive by `surface` for this call.
        let layout = unsafe {
            draw_toolbar_with_overflow(
                surface.context_ptr(),
                &f,
                0.0,
                0.0,
                w,
                slot_h,
                row_h,
                &bar,
                &theme,
                None,
                None,
                options,
                &priorities,
            )
        };
        assert!(
            !layout.visible_items[2].clickable,
            "the paint call's own returned layout agrees with the probe",
        );
    }

    /// `ToolbarOverflow::Menu` on a bar too narrow for every button drops
    /// the lowest-priority ones and paints a trailing `[ » ]` affordance in
    /// their place; a click there hit-tests to `ToolbarHit::Overflow`, and
    /// the affordance itself is painted rather than skipped — the shared
    /// paint code resolves a synthetic `TOOLBAR_OVERFLOW_ITEM_IDX` entry to
    /// the `[ » ]` button instead of ignoring it.
    #[test]
    fn menu_mode_paints_overflow_affordance_and_hit_tests_overflow() {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "AAAA", true),
                mk_action("b", "BBBB", true),
                mk_action("c", "CCCC", true),
            ],
            bg: None,
            focused_index: None,
        };
        let f = font();
        let h = 20.0_f64;

        let mut backend = MacBackend::new();
        backend.set_current_font(f);

        // One button's width, so a bar just wide enough for one button
        // plus the `»` affordance forces the other two to drop. Sized
        // through `backend.toolbar_layout` (measures with `chrome_font`,
        // the font this test's own paint call below resolves through)
        // rather than `mac_toolbar_layout` with a plain `Menlo` `CTFont`,
        // so `w` stays correct regardless of how the two fonts' advance
        // widths compare.
        let clip = backend.toolbar_layout(QRect::new(0.0, 0.0, 1000.0, h as f32), &bar);
        let item_w = clip.visible_items[0].bounds.width as f64;
        let w = item_w * 1.6;

        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Menu);
        let priorities = ToolbarOverflowPriorities::new();

        let surface = BitmapSurface::new(w.ceil() as u32, h.ceil() as u32);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        backend.begin_frame(Viewport::new(w as f32, h as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_toolbar_with_overflow(
                QRect::new(0.0, 0.0, w as f32, h as f32),
                &bar,
                &crate::InteractionState::new(),
                options,
                &priorities,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        let layout = layout.into_inner().unwrap();

        let overflow_vis = layout
            .visible_items
            .iter()
            .find(|v| v.kind == ToolbarItemKind::Overflow)
            .expect("overflow affordance present");
        assert_eq!(overflow_vis.item_idx, TOOLBAR_OVERFLOW_ITEM_IDX);
        assert!(
            !layout.overflowed_items().is_empty(),
            "at least one button should have been dropped",
        );
        assert_eq!(
            layout.hit_test(overflow_vis.bounds.x + 1.0, overflow_vis.bounds.y + 1.0,),
            ToolbarHit::Overflow,
        );

        // The affordance's own pill must actually be painted — not just
        // the row background around it — proving the `»` button resolves
        // through the ordinary `Action` paint arm instead of being
        // silently skipped as an unknown `bar.buttons` index.
        let theme = Theme::default();
        let mut any_non_bg = false;
        for dx in 0..(overflow_vis.bounds.width as u32).max(1) {
            for dy in 0..(overflow_vis.bounds.height as u32).max(1) {
                let (r, g, b, a) = surface.pixel(
                    overflow_vis.bounds.x as u32 + dx,
                    overflow_vis.bounds.y as u32 + dy,
                );
                if a > 0 && (r, g, b) != (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b) {
                    any_non_bg = true;
                }
            }
        }
        assert!(
            any_non_bg,
            "overflow affordance bounds should contain glyph ink, not just the bar background",
        );
    }
}
