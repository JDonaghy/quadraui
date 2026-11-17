//! Direct2D / DirectWrite rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! [`win_toolbar_layout`] stays here: [`Toolbar::layout`] (the D6 layout
//! API) does every positioning decision via the shared
//! [`crate::primitives::toolbar::measure_button`] formula, this
//! module just supplies the DirectWrite measurer. Content painting
//! moved to the shared
//! [`crate::primitives::toolbar::native_surface_paint::paint`], which
//! paints rounded highlight pills via
//! [`super::text::fill_rounded_rect`]. The focus ring stays a
//! **square** stroke on all three backends — [`PaintSurface`] has no
//! rounded-stroke verb.
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

use super::text::{pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::paint_surface::PaintSurface;
use crate::primitives::toolbar::{
    measure_button, native_surface_paint, Toolbar, ToolbarItemMeasure, ToolbarLayout,
    ToolbarLayoutOptions, ToolbarOverflowPriorities, ToolbarPaintOptions,
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
/// — no wrapper struct needed; see `win::text::DWrite`'s
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
/// resolves where button/label text paints within a slot
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

/// Compute the Win-GUI layout [`draw_toolbar_with_overflow`] would paint,
/// without painting — the layout-only twin
/// [`win_toolbar_layout`] is to [`draw_toolbar_with_options`].
///
/// `line_height` is the DIP height of **one** toolbar row — this
/// backend's analog to [`crate::gtk::toolbar::gtk_toolbar_layout_with_overflow`]'s
/// own `line_height` parameter. Ignored for [`ToolbarOverflow::Clip`](crate::primitives::toolbar::ToolbarOverflow::Clip)/
/// [`ToolbarOverflow::Menu`](crate::primitives::toolbar::ToolbarOverflow::Menu), which never need more than one line and use
/// `rect.height` directly, same as [`win_toolbar_layout`].
///
/// For [`ToolbarOverflow::Wrap`](crate::primitives::toolbar::ToolbarOverflow::Wrap), `rect.height` is the **total** area the
/// host has (or is probing with) — [`ToolbarLayout::cross_extent`] on the
/// result reports how many DIPs of that the wrapped content actually
/// needs, so a host sizes its container from that before the next paint.
/// Any item whose row starts at or past `rect.y + rect.height` is marked
/// un-clickable (its `Group` children too) while still appearing in
/// `visible_items`, so a host that skips the resize step never gets a hit
/// region for a row [`draw_toolbar_with_overflow`] never actually paints.
///
/// Known gap: the un-clickable clamp above only applies to
/// [`ToolbarOrientation::Horizontal`](crate::primitives::toolbar::ToolbarOrientation::Horizontal), and `line_height` is substituted
/// for the toolbar's main-axis length regardless of orientation, so
/// [`ToolbarOrientation::Vertical`](crate::primitives::toolbar::ToolbarOrientation::Vertical) + [`ToolbarOverflow::Wrap`](crate::primitives::toolbar::ToolbarOverflow::Wrap) gets a
/// one-row-tall main axis and every item wraps into its own column
/// instead of filling `rect.height` first — matching
/// [`crate::gtk::toolbar::gtk_toolbar_layout_with_overflow`]'s own gap,
/// not yet fixed on either backend.
pub fn win_toolbar_layout_with_overflow(
    dwrite: &DWrite,
    rect: Rect,
    bar: &Toolbar,
    line_height: f32,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    super::backend::toolbar_layout_with_overflow_via(
        dwrite,
        rect,
        bar,
        line_height,
        options,
        priorities,
    )
}

/// [`draw_toolbar_with_options`], plus [`ToolbarLayoutOptions`]:
/// `options.overflow` picks how items that don't fit are handled.
///
/// - [`ToolbarOverflow::Clip`](crate::primitives::toolbar::ToolbarOverflow::Clip) uses the same single-line geometry as
///   [`draw_toolbar_with_options`] (`rect` is the one line) — visually
///   equivalent for a `rect`-sized bar, though this function additionally
///   pre-fills the whole `rect` and pushes a clip at `rect` first, which
///   [`draw_toolbar_with_options`] does not.
/// - [`ToolbarOverflow::Wrap`](crate::primitives::toolbar::ToolbarOverflow::Wrap) paints each wrapped line `line_height` DIPs
///   below the last, clipped to `rect` — call
///   [`win_toolbar_layout_with_overflow`] first and grow `rect.height` to
///   its [`ToolbarLayout::cross_extent`] so every row actually paints.
/// - [`ToolbarOverflow::Menu`](crate::primitives::toolbar::ToolbarOverflow::Menu) paints a trailing `[ » ]` affordance —
///   [`crate::primitives::toolbar::native_surface_paint::paint`]'s
///   [`crate::primitives::toolbar::ToolbarItemKind::Overflow`] arm reuses
///   the same pill/label `Action` paint every other button gets, through
///   [`crate::primitives::toolbar::overflow_indicator_button`]. A click on
///   it hit-tests to [`crate::primitives::toolbar::ToolbarHit::Overflow`];
///   open a [`crate::ContextMenu`] built from
///   [`ToolbarLayout::overflowed_items`].
///
/// `priorities` is read only when `options.overflow` is
/// [`ToolbarOverflow::Menu`](crate::primitives::toolbar::ToolbarOverflow::Menu) — pass [`ToolbarOverflowPriorities::new`]
/// for the other two variants.
///
/// Always paints with [`ToolbarPaintOptions::default`] — a host cannot
/// combine [`crate::primitives::toolbar::ToolbarVAlign::Bottom`] with a
/// non-`Clip` overflow mode here; matches
/// [`crate::gtk::toolbar::draw_toolbar_with_overflow`].
#[allow(clippy::too_many_arguments)]
pub fn draw_toolbar_with_overflow(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    line_height: f32,
    bar: &Toolbar,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    let layout =
        win_toolbar_layout_with_overflow(dwrite, rect, bar, line_height, options, priorities);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    let theme = Theme::default();
    push_clip(target, rect);

    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    // `native_surface_paint::paint`'s own background fill only covers
    // `layout.bar_bounds`, which for `ToolbarOverflow::Wrap` is exactly
    // one `line_height`-tall line (see `win_toolbar_layout_with_overflow`'s
    // own doc) — not every wrapped row. Fill the full `rect` the caller
    // actually gave us first, so every row this clip can reach gets the
    // bar's background before `paint` re-paints the first line's worth on
    // top of it.
    let bar_bg = bar.bg.unwrap_or(theme.header_bg);
    surface.surface_fill_rect(rect, bar_bg);
    native_surface_paint::paint(
        bar,
        &layout,
        &mut surface,
        &theme,
        hovered_id,
        pressed_id,
        ToolbarPaintOptions::default(),
    );

    pop_clip(target);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{
        ToolbarButton, ToolbarHit, ToolbarOrientation, ToolbarOverflow,
    };
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 240.0;
    const H: f32 = 40.0;

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

    /// `draw_toolbar` must paint and return a click-routable layout.
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

    /// Non-zero-origin layout: `Toolbar::layout` bakes `rect.x`/`rect.y`
    /// into every item's `bounds` (ABSOLUTE frame), matching the GTK/macOS/TUI twins.
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

    /// Pre-migration Windows painted the hover/pressed/
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

        // The inset rect must be filled solid in the highlight colour away
        // from the corners — proves this isn't just "nothing painted". Not the
        // button's exact geometric centre: that's where the "On" label's text
        // is centred (`tx = item.x + (item.width - tw) / 2`), and DirectWrite's
        // ClearType/greyscale anti-aliasing blends glyph ink into the surrounding
        // highlight fill, so the centre pixel is a mix of `theme.foreground` and
        // `theme.selected_bg`. `measure_button`'s `ACTION_H_PAD = 8.0` guarantees
        // the text always starts at `item.x + 8` regardless of font/glyph metrics
        // (`item.width == text_width + 2 * ACTION_H_PAD`, so
        // `tx == item.x + ACTION_H_PAD`), so the strip `[item.x + 2, item.x + 8)`
        // — between the inset's left edge and where the label starts — is always
        // text-free. `+ 5.0` sits in the middle of that strip; the y stays at
        // mid-height, comfortably clear of the top/bottom corner arcs (radius 4
        // on a 40px-tall bar).
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

    // ── ToolbarOverflow ─────────────────────────────────────────────────

    fn three_button_bar() -> Toolbar {
        Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "AAAA", true),
                mk_action("b", "BBBB", true),
                mk_action("c", "CCCC", true),
            ],
            bg: None,
            focused_index: None,
        }
    }

    /// `ToolbarOverflow::Wrap` places overflow on additional rows
    /// `line_height` DIPs apart, and `cross_extent` reports exactly how
    /// tall a host must grow its container for every row to actually
    /// paint — the value a two-pass caller reads between the probe layout
    /// and the real one. `bar_w` is sized from the real DWrite-measured
    /// button width so exactly one button fits per row.
    #[test]
    fn wrap_mode_places_overflow_on_additional_rows() {
        let bar = three_button_bar();
        let (dwrite, line_h, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let btn_w = measure_button(&dwrite, &bar.buttons[0]);
        let bar_w = btn_w + 1.0;
        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Wrap);
        let priorities = ToolbarOverflowPriorities::new();

        let probe = win_toolbar_layout_with_overflow(
            &dwrite,
            Rect::new(0.0, 0.0, bar_w, line_h),
            &bar,
            line_h,
            options,
            &priorities,
        );
        let rows_needed = probe.cross_extent(ToolbarOrientation::Horizontal);
        assert_eq!(
            rows_needed,
            3.0 * line_h,
            "one button per row at this width"
        );

        let layout = win_toolbar_layout_with_overflow(
            &dwrite,
            Rect::new(0.0, 0.0, bar_w, rows_needed),
            &bar,
            line_h,
            options,
            &priorities,
        );
        assert_eq!(
            layout.hit_test(1.0, 1.0),
            ToolbarHit::Button(WidgetId::new("a"))
        );
        assert_eq!(
            layout.hit_test(1.0, line_h + 1.0),
            ToolbarHit::Button(WidgetId::new("b"))
        );
        assert_eq!(
            layout.hit_test(1.0, 2.0 * line_h + 1.0),
            ToolbarHit::Button(WidgetId::new("c"))
        );
    }

    /// A host that skips the resize step and hands a height smaller than
    /// `cross_extent` reports still gets every wrapped item back in
    /// `visible_items` (so a probing caller can measure the full extent),
    /// but only the first row is clickable — matching the clip
    /// [`draw_toolbar_with_overflow`] applies at `rect`, which never
    /// paints past it. Guards against the hit-test/paint drift a `Wrap`
    /// layout that ignored `rect.height` entirely would produce.
    #[test]
    fn wrap_mode_marks_rows_past_height_unclickable() {
        let bar = three_button_bar();
        let (dwrite, line_h, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let btn_w = measure_button(&dwrite, &bar.buttons[0]);
        let bar_w = btn_w + 1.0;
        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Wrap);
        let priorities = ToolbarOverflowPriorities::new();

        // 3 buttons need 3 rows at this width, but this host only offers 1.
        let layout = win_toolbar_layout_with_overflow(
            &dwrite,
            Rect::new(0.0, 0.0, bar_w, line_h),
            &bar,
            line_h,
            options,
            &priorities,
        );

        assert_eq!(layout.visible_items.len(), 3, "kept for cross_extent");
        assert_eq!(
            layout.cross_extent(ToolbarOrientation::Horizontal),
            3.0 * line_h
        );
        assert_eq!(
            layout.hit_test(1.0, 1.0),
            ToolbarHit::Button(WidgetId::new("a")),
            "row 0 is still clickable"
        );
        assert_eq!(
            layout.hit_test(1.0, line_h + 1.0),
            ToolbarHit::Empty,
            "row 1 was never painted at this height"
        );
        assert_eq!(
            layout.hit_test(1.0, 2.0 * line_h + 1.0),
            ToolbarHit::Empty,
            "row 2 was never painted at this height"
        );
    }

    /// `ToolbarOverflow::Menu` on a bar too narrow for every button paints
    /// a trailing `[ » ]` affordance in place of whatever it dropped —
    /// through [`crate::primitives::toolbar::native_surface_paint::paint`]'s
    /// `Overflow` arm, the same pill/label path every other `Action`
    /// button gets — and a click there hit-tests to
    /// [`crate::primitives::toolbar::ToolbarHit::Overflow`], ready to open
    /// a [`crate::ContextMenu`] from `overflowed_items()`.
    #[test]
    fn menu_mode_paints_affordance_and_hit_tests_overflow() {
        let bar = three_button_bar();
        let (dwrite, line_h, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let btn_w = measure_button(&dwrite, &bar.buttons[0]);
        // Wide enough for one button plus the `»` affordance, not all three.
        let bar_w = btn_w * 1.5;
        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Menu);
        let priorities = ToolbarOverflowPriorities::new();
        let theme = Theme::default();
        let rect = Rect::new(0.0, 0.0, bar_w, line_h);

        let surface = HeadlessSurface::new(bar_w.ceil() as u32, line_h.ceil() as u32)
            .expect("create surface");
        let layout = surface
            .paint(|target| {
                draw_toolbar_with_overflow(
                    target,
                    &dwrite,
                    rect,
                    line_h,
                    &bar,
                    None,
                    None,
                    options,
                    &priorities,
                );
            })
            .map(|_| {
                win_toolbar_layout_with_overflow(&dwrite, rect, &bar, line_h, options, &priorities)
            })
            .expect("paint toolbar");

        let overflowed = layout.overflowed_items();
        assert!(!overflowed.is_empty(), "narrow bar must drop something");

        let overflow_vis = layout
            .visible_items
            .iter()
            .find(|vis| vis.kind == crate::primitives::toolbar::ToolbarItemKind::Overflow)
            .expect("Menu layout paints a trailing `»` entry when it drops items");
        let mid_x = overflow_vis.bounds.x + overflow_vis.bounds.width / 2.0;
        let mid_y = overflow_vis.bounds.y + overflow_vis.bounds.height / 2.0;
        assert_eq!(layout.hit_test(mid_x, mid_y), ToolbarHit::Overflow);

        // The affordance actually painted ink somewhere inside its own
        // bounds — not just a hit-testable hole.
        let bg = theme.header_bg;
        let x0 = overflow_vis.bounds.x.round() as u32;
        let x1 = (overflow_vis.bounds.x + overflow_vis.bounds.width).round() as u32;
        let y0 = overflow_vis.bounds.y.round() as u32;
        let y1 = (overflow_vis.bounds.y + overflow_vis.bounds.height).round() as u32;
        let mut painted = false;
        'scan: for y in y0..y1.min(line_h.ceil() as u32) {
            for x in x0..x1.min(bar_w.ceil() as u32) {
                let px = surface.pixel_at(x, y);
                if (px.r, px.g, px.b) != (bg.r, bg.g, bg.b) {
                    painted = true;
                    break 'scan;
                }
            }
        }
        assert!(
            painted,
            "overflow affordance should paint ink inside its own bounds"
        );
    }
}
