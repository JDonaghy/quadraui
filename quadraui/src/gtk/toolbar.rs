//! GTK rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! `gtk_toolbar_layout` stays here — it needs Pango's own text
//! measurement to size each item. Content painting (background,
//! per-state colouring, hover/pressed/active highlight, focus ring,
//! separators, labels) moved to the shared
//! [`crate::primitives::toolbar::native_surface_paint::paint`] —
//! see that fn's module doc for the highlight-corner drift it
//! resolved (this backend's rounded pill is now what every backend
//! paints, closing macOS's and Windows' square-corner gap instead of
//! flattening this one down to match them).
//!
//! See that module's own doc for the full per-state colouring table.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::paint_surface::PaintSurface;
use crate::primitives::layout_metrics::TextMeasure;
use crate::primitives::toolbar::{
    measure_button, native_surface_paint, Toolbar, ToolbarItemMeasure, ToolbarLayout,
    ToolbarLayoutOptions, ToolbarOrientation, ToolbarOverflow, ToolbarOverflowPriorities,
    ToolbarPaintOptions,
};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Adapts a live `pango::Layout` (falling back to a `char_width`-based
/// estimate when none is available, e.g. from a layout-only call between
/// paint frames) to the shared [`TextMeasure`] trait so
/// [`crate::primitives::toolbar::measure_button`] never has to name a
/// Pango type — mirrors `macos::toolbar`'s / `win::toolbar`'s twin
/// adapters.
///
/// `pub(crate)` so `gtk::sidebar_panel`'s embedded toolbar header can
/// build the exact same adapter for its own `measure_button` calls,
/// guaranteeing paint and hit-test agree on item positions everywhere a
/// `Toolbar` appears — with no separate per-caller measurer function.
pub(crate) struct PangoMeasure<'a> {
    pub(crate) pango_layout: Option<&'a pango::Layout>,
    pub(crate) char_width: f64,
}

impl TextMeasure for PangoMeasure<'_> {
    fn width_of(&self, text: &str) -> f32 {
        if let Some(pl) = self.pango_layout {
            pl.set_text(text);
            pl.pixel_size().0.max(0) as f32
        } else {
            (text.chars().count() as f64 * self.char_width).ceil() as f32
        }
    }
}

/// Compute the GTK pixel-unit layout for a [`Toolbar`] without painting.
///
/// `pango_layout` is `Some` inside a draw frame (Pango can measure
/// accurately) and `None` from layout-only paths called between
/// frames — in that case a `char_width`-based fallback is used.
pub fn gtk_toolbar_layout(
    bar: &Toolbar,
    pango_layout: Option<&pango::Layout>,
    char_width: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> ToolbarLayout {
    let measure = PangoMeasure {
        pango_layout,
        char_width,
    };
    bar.layout(x as f32, y as f32, w as f32, h as f32, |btn| {
        ToolbarItemMeasure::new(measure_button(&measure, btn))
    })
}

/// Draw a [`Toolbar`] into `(x, y, w, h)` on `cr`. Returns the layout
/// for host click dispatch.
///
/// Equivalent to [`draw_toolbar_with_options`] with
/// [`ToolbarPaintOptions::default()`] — kept as a separate, **unchanged**
/// function so every existing caller of this re-exported `pub fn` keeps
/// compiling untouched (`CLAUDE.md`'s *Downstream consumers* rule 2).
#[allow(clippy::too_many_arguments)]
pub fn draw_toolbar(
    cr: &Context,
    pango_layout: &pango::Layout,
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
        cr,
        pango_layout,
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
#[allow(clippy::too_many_arguments)]
pub fn draw_toolbar_with_options(
    cr: &Context,
    pango_layout: &pango::Layout,
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
    pango_layout.set_attributes(None);
    pango_layout.set_width(-1);
    pango_layout.set_ellipsize(pango::EllipsizeMode::None);

    // Inside a draw frame, prefer Pango measurement; `char_width` is
    // unused. We still pass a positive default so the fallback path
    // (which `draw_toolbar` itself never hits) remains well-defined.
    let toolbar_layout = gtk_toolbar_layout(bar, Some(pango_layout), 8.0, x, y, w, h);

    if w <= 0.0 || h <= 0.0 {
        return toolbar_layout;
    }

    // Clip to the bar's rect so anything painted by mistake doesn't
    // leak past the right edge.
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();

    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: true,
    };
    native_surface_paint::paint(
        bar,
        &toolbar_layout,
        &mut surface,
        theme,
        hovered_id,
        pressed_id,
        options,
    );
    pango_layout.set_attributes(None);

    cr.restore().ok();
    toolbar_layout
}

/// Compute the GTK pixel-unit layout [`draw_toolbar_with_overflow`] would
/// paint, without painting — the layout-only twin
/// [`gtk_toolbar_layout`] is to [`draw_toolbar_with_options`].
///
/// `line_height` is the pixel height of **one** toolbar row — GTK's analog
/// to [`crate::tui::toolbar`]'s hardcoded one-terminal-row constant for
/// [`ToolbarOverflow::Wrap`] (TUI has no sub-row unit to pick; GTK's pixel
/// rows do, so the caller supplies it — typically the backend's own
/// `line_height()`). Ignored for [`ToolbarOverflow::Clip`]/
/// [`ToolbarOverflow::Menu`], which never need more than one line and use
/// `h` for it directly, same as [`gtk_toolbar_layout`].
///
/// For [`ToolbarOverflow::Wrap`], `h` is the **total** area the host has
/// (or is probing with) — [`ToolbarLayout::cross_extent`] on the result
/// reports how many pixels of that the wrapped content actually needs, so
/// a host sizes its container from that before the next paint. Any item
/// whose row starts at or past `y + h` is marked un-clickable (its
/// `Group` children too) while still appearing in `visible_items`, so a
/// host that skips the resize step never gets a hit region for a row
/// [`draw_toolbar_with_overflow`]'s own `(x, y, w, h)` cairo clip didn't
/// actually paint.
#[allow(clippy::too_many_arguments)]
pub fn gtk_toolbar_layout_with_overflow(
    bar: &Toolbar,
    pango_layout: Option<&pango::Layout>,
    char_width: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    let measure = PangoMeasure {
        pango_layout,
        char_width,
    };
    let line_h = if options.overflow == ToolbarOverflow::Wrap {
        line_height
    } else {
        h
    };
    let mut layout = bar.layout_with_overflow(
        x as f32,
        y as f32,
        w as f32,
        line_h as f32,
        options,
        priorities,
        |btn| ToolbarItemMeasure::new(measure_button(&measure, btn)),
    );

    if options.overflow == ToolbarOverflow::Wrap
        && options.orientation == ToolbarOrientation::Horizontal
    {
        let area_bottom = (y + h) as f32;
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
/// `options.overflow` picks how items that don't fit are handled.
///
/// - [`ToolbarOverflow::Clip`] paints identically to
///   [`draw_toolbar_with_options`] (same single-line geometry; `(w, h)`
///   is the one line).
/// - [`ToolbarOverflow::Wrap`] paints each wrapped line `line_height`
///   pixels below the last, clipped to `(x, y, w, h)` — call
///   [`gtk_toolbar_layout_with_overflow`] first and grow `h` to its
///   [`ToolbarLayout::cross_extent`] so every row actually paints.
/// - [`ToolbarOverflow::Menu`] paints a trailing `[ » ]` affordance —
///   [`crate::primitives::toolbar::native_surface_paint::paint`]'s
///   [`crate::primitives::toolbar::ToolbarItemKind::Overflow`] arm reuses
///   the same pill/label `Action` paint every other button gets, through
///   [`crate::primitives::toolbar::overflow_indicator_button`]. A click on
///   it hit-tests to [`crate::primitives::toolbar::ToolbarHit::Overflow`];
///   open a [`crate::ContextMenu`] built from
///   [`ToolbarLayout::overflowed_items`].
///
/// `priorities` is read only when `options.overflow` is
/// [`ToolbarOverflow::Menu`] — pass [`ToolbarOverflowPriorities::new`]
/// for the other two variants.
#[allow(clippy::too_many_arguments)]
pub fn draw_toolbar_with_overflow(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
    bar: &Toolbar,
    theme: &Theme,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
    options: ToolbarLayoutOptions,
    priorities: &ToolbarOverflowPriorities,
) -> ToolbarLayout {
    pango_layout.set_attributes(None);
    pango_layout.set_width(-1);
    pango_layout.set_ellipsize(pango::EllipsizeMode::None);

    let toolbar_layout = gtk_toolbar_layout_with_overflow(
        bar,
        Some(pango_layout),
        8.0,
        x,
        y,
        w,
        h,
        line_height,
        options,
        priorities,
    );

    if w <= 0.0 || h <= 0.0 {
        return toolbar_layout;
    }

    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();

    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: true,
    };
    // `native_surface_paint::paint`'s own background fill only covers
    // `toolbar_layout.bar_bounds`, which for `ToolbarOverflow::Wrap` is
    // exactly one `line_height`-tall line (see `gtk_toolbar_layout_with_overflow`'s
    // own doc) — not every wrapped row. Fill the full `(x, y, w, h)` the
    // caller actually gave us first, so every row this cairo clip can
    // reach gets the bar's background before `paint` re-paints the first
    // line's worth on top of it.
    let bar_bg = bar.bg.unwrap_or(theme.header_bg);
    surface.surface_fill_rect(
        crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32),
        bar_bg,
    );
    native_surface_paint::paint(
        bar,
        &toolbar_layout,
        &mut surface,
        theme,
        hovered_id,
        pressed_id,
        ToolbarPaintOptions::default(),
    );
    pango_layout.set_attributes(None);

    cr.restore().ok();
    toolbar_layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{ToolbarButton, ToolbarHit};
    use crate::types::WidgetId;

    fn test_toolbar() -> Toolbar {
        Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("tb:action"),
                label: "Refine".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
        }
    }

    /// `toolbar_layout` is documented **ABSOLUTE**:
    /// `gtk_toolbar_layout` forwards `x`/`y` straight through to
    /// `Toolbar::layout` as `origin_x`/`origin_y`, which folds them into
    /// every visible item's `bounds` — so a non-zero origin must shift
    /// bounds by exactly `(x, y)`, unlike the LOCAL primitives
    /// (`status_bar_layout`, `data_table_layout`, `form_layout`) that
    /// ignore it entirely. `pango_layout: None` exercises the
    /// `char_width` fallback measurer, the same path a click-time
    /// (outside-frame) hit test uses.
    fn round_trip_at(x: f64, y: f64) {
        let bar = test_toolbar();
        let layout = gtk_toolbar_layout(&bar, None, 8.0, x, y, 100.0, 20.0);

        let vis = &layout.visible_items[0];
        assert_eq!(vis.bounds.x as f64, x);
        assert_eq!(vis.bounds.y as f64, y);

        let ccx = vis.bounds.x + 1.0;
        let ccy = vis.bounds.y + 1.0;
        assert_eq!(
            layout.hit_test(ccx, ccy),
            ToolbarHit::Button(WidgetId::new("tb:action"))
        );
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard: ensures paint and click work
    /// correctly when the toolbar is not at the origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }

    /// Pixel-level multi-row paint test: paint the same bar into the
    /// same tall slot once per [`ToolbarVAlign`] variant and confirm the
    /// row of the first painted (non-background) pixel moves accordingly
    /// — `Top` paints highest, `Bottom` lowest, `Center` strictly between
    /// the two. Exercises the real [`draw_toolbar`] →
    /// [`crate::primitives::toolbar::native_surface_paint::paint`] path
    /// through an in-memory Cairo `ImageSurface`, the same headless
    /// pattern `gtk::command_line`'s pixel tests use — no live GTK window
    /// needed.
    #[test]
    fn multi_row_valign_moves_painted_text_vertically() {
        use crate::primitives::toolbar::ToolbarVAlign;
        use pangocairo::cairo::{Context, Format, ImageSurface};

        const W: i32 = 200;
        const H: i32 = 60;

        let theme = Theme::default();
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

        // Row (from the top) of the first pixel that differs from the
        // bar's own background fill — i.e. the first row any ink lands
        // on, for a given `valign`.
        let first_ink_row = |valign: ToolbarVAlign| -> i32 {
            let mut surface =
                ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
            {
                let cr = Context::new(&surface).expect("Context::new");
                let pango_layout = pangocairo::functions::create_layout(&cr);
                draw_toolbar_with_options(
                    &cr,
                    &pango_layout,
                    0.0,
                    0.0,
                    W as f64,
                    H as f64,
                    &bar,
                    &theme,
                    None,
                    None,
                    ToolbarPaintOptions { valign },
                );
            }
            let stride = surface.stride() as usize;
            let data = surface.data().expect("surface data");
            let bg = theme.header_bg;
            for y in 0..H {
                for x in 0..W {
                    let off = y as usize * stride + x as usize * 4;
                    // Cairo ARGB32 byte order on little-endian is BGRA.
                    let (b, g, r) = (data[off], data[off + 1], data[off + 2]);
                    if (r, g, b) != (bg.r, bg.g, bg.b) {
                        return y;
                    }
                }
            }
            panic!("no ink painted for valign {valign:?}");
        };

        let top_row = first_ink_row(ToolbarVAlign::Top);
        let center_row = first_ink_row(ToolbarVAlign::Center);
        let bottom_row = first_ink_row(ToolbarVAlign::Bottom);

        assert!(
            top_row < center_row,
            "Top should paint higher than Center (top={top_row}, center={center_row})"
        );
        assert!(
            center_row < bottom_row,
            "Center should paint higher than Bottom (center={center_row}, bottom={bottom_row})"
        );
    }

    // ── ToolbarOverflow ─────────────────────────────────────────────────

    fn mk_action(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
            spec: None,
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn three_button_bar() -> Toolbar {
        Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "AAAA"),
                mk_action("b", "BBBB"),
                mk_action("c", "CCCC"),
            ],
            bg: None,
            focused_index: None,
        }
    }

    /// With the `char_width`-fallback measurer, each `[ XXXX ]` button is
    /// `4 * 8.0 + 2 * ACTION_H_PAD(8.0) = 48.0` px wide, so a 50px-wide bar
    /// fits exactly one per row. `ToolbarOverflow::Wrap` places the other
    /// two on additional rows `line_height` apart, and `cross_extent`
    /// reports exactly how tall a host must grow its container for every
    /// row to actually paint — the value a two-pass caller reads between
    /// the probe layout and the real one.
    #[test]
    fn wrap_mode_places_overflow_on_additional_rows() {
        let bar = three_button_bar();
        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Wrap);
        let priorities = ToolbarOverflowPriorities::new();
        const LINE_H: f64 = 20.0;

        let probe = gtk_toolbar_layout_with_overflow(
            &bar,
            None,
            8.0,
            0.0,
            0.0,
            50.0,
            LINE_H,
            LINE_H,
            options,
            &priorities,
        );
        let rows_needed = probe.cross_extent(ToolbarOrientation::Horizontal);
        assert_eq!(rows_needed, 60.0, "one button per row at this width");

        let layout = gtk_toolbar_layout_with_overflow(
            &bar,
            None,
            8.0,
            0.0,
            0.0,
            50.0,
            rows_needed as f64,
            LINE_H,
            options,
            &priorities,
        );
        assert_eq!(
            layout.hit_test(1.0, 1.0),
            ToolbarHit::Button(WidgetId::new("a"))
        );
        assert_eq!(
            layout.hit_test(1.0, LINE_H as f32 + 1.0),
            ToolbarHit::Button(WidgetId::new("b"))
        );
        assert_eq!(
            layout.hit_test(1.0, 2.0 * LINE_H as f32 + 1.0),
            ToolbarHit::Button(WidgetId::new("c"))
        );
    }

    /// A host that skips the resize step and hands `h` smaller than
    /// `cross_extent` reports still gets every wrapped item back in
    /// `visible_items` (so a probing caller can measure the full extent),
    /// but only the first row is clickable — matching the Cairo clip
    /// [`draw_toolbar_with_overflow`] applies at `(x, y, w, h)`, which
    /// never paints past that rect. Guards against the hit-test/paint
    /// drift a `Wrap` layout that ignored `h` entirely would produce.
    #[test]
    fn wrap_mode_marks_rows_past_h_unclickable() {
        let bar = three_button_bar();
        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Wrap);
        let priorities = ToolbarOverflowPriorities::new();
        const LINE_H: f64 = 20.0;

        // 3 buttons need 3 rows at this width, but this host only offers 1.
        let layout = gtk_toolbar_layout_with_overflow(
            &bar,
            None,
            8.0,
            0.0,
            0.0,
            50.0,
            LINE_H,
            LINE_H,
            options,
            &priorities,
        );

        assert_eq!(layout.visible_items.len(), 3, "kept for cross_extent");
        assert_eq!(layout.cross_extent(ToolbarOrientation::Horizontal), 60.0);
        assert_eq!(
            layout.hit_test(1.0, 1.0),
            ToolbarHit::Button(WidgetId::new("a")),
            "row 0 is still clickable"
        );
        assert_eq!(
            layout.hit_test(1.0, LINE_H as f32 + 1.0),
            ToolbarHit::Empty,
            "row 1 was never painted at this h"
        );
        assert_eq!(
            layout.hit_test(1.0, 2.0 * LINE_H as f32 + 1.0),
            ToolbarHit::Empty,
            "row 2 was never painted at this h"
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
        use pangocairo::cairo::{Context, Format, ImageSurface};

        let bar = three_button_bar();
        let options = ToolbarLayoutOptions::new().with_overflow(ToolbarOverflow::Menu);
        let priorities = ToolbarOverflowPriorities::new();
        let theme = Theme::default();

        const W: i32 = 90;
        const H: i32 = 30;
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let layout = {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_toolbar_with_overflow(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                W as f64,
                H as f64,
                H as f64,
                &bar,
                &theme,
                None,
                None,
                options,
                &priorities,
            )
        };

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
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let bg = theme.header_bg;
        let x0 = overflow_vis.bounds.x.round() as i32;
        let x1 = (overflow_vis.bounds.x + overflow_vis.bounds.width).round() as i32;
        let y0 = overflow_vis.bounds.y.round() as i32;
        let y1 = (overflow_vis.bounds.y + overflow_vis.bounds.height).round() as i32;
        let mut painted = false;
        for y in y0.max(0)..y1.min(H) {
            for x in x0.max(0)..x1.min(W) {
                let off = y as usize * stride + x as usize * 4;
                let (b, g, r) = (data[off], data[off + 1], data[off + 2]);
                if (r, g, b) != (bg.r, bg.g, bg.b) {
                    painted = true;
                }
            }
        }
        assert!(
            painted,
            "the `»` affordance must paint ink, not just a hit region"
        );
    }

    // ── Field / MenuButton ──────────────────────────────────────────────

    /// `measure_button`'s `Field`/`MenuButton` arms plus the shared
    /// `native_surface_paint::paint` arms added for them paint and
    /// hit-test correctly through this backend's own entry points — no
    /// GTK-specific code needed for either variant beyond the
    /// `PangoMeasure` adapter every other button already goes through.
    #[test]
    fn field_and_menu_button_paint_and_hit_test() {
        use crate::primitives::context_menu::ContextMenuItem;
        use crate::primitives::text_field::TextField;
        use crate::primitives::toolbar::ToolbarItemWidth;
        use crate::types::StyledText;
        use pangocairo::cairo::{Context, Format, ImageSurface};

        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::MenuButton {
                    id: WidgetId::new("tb:action-menu"),
                    label: "Action".into(),
                    icon: None,
                    menu: vec![ContextMenuItem {
                        label: StyledText::plain("Get Info"),
                        ..Default::default()
                    }],
                },
                ToolbarButton::Field {
                    id: WidgetId::new("tb:search"),
                    field: TextField::new(WidgetId::new("tb:search")).with_placeholder("Search"),
                    width: ToolbarItemWidth::Fixed(120.0),
                },
            ],
            bg: None,
            focused_index: None,
        };

        const W: i32 = 300;
        const H: i32 = 24;
        let theme = Theme::default();
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        let layout = {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_toolbar(
                &cr,
                &pango_layout,
                0.0,
                0.0,
                W as f64,
                H as f64,
                &bar,
                &theme,
                None,
                None,
            )
        };

        let menu_vis = layout
            .visible_items
            .iter()
            .find(|v| v.action_id == Some(WidgetId::new("tb:action-menu")))
            .expect("MenuButton has a visible slot");
        assert_eq!(
            menu_vis.kind,
            crate::primitives::toolbar::ToolbarItemKind::MenuButton
        );
        let mx = menu_vis.bounds.x + menu_vis.bounds.width / 2.0;
        let my = menu_vis.bounds.y + menu_vis.bounds.height / 2.0;
        assert_eq!(
            layout.hit_test(mx, my),
            ToolbarHit::MenuButton(WidgetId::new("tb:action-menu"))
        );

        let field_vis = layout
            .visible_items
            .iter()
            .find(|v| v.action_id == Some(WidgetId::new("tb:search")))
            .expect("Field has a visible slot");
        assert_eq!(field_vis.bounds.width, 120.0);
        let fx = field_vis.bounds.x + field_vis.bounds.width / 2.0;
        let fy = field_vis.bounds.y + field_vis.bounds.height / 2.0;
        assert_eq!(
            layout.hit_test(fx, fy),
            ToolbarHit::Field(WidgetId::new("tb:search"))
        );

        // Painted real ink inside the field's bounds (the framed box),
        // not just a hit-testable hole.
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let bg = theme.header_bg;
        let x0 = field_vis.bounds.x.round() as i32;
        let x1 = (field_vis.bounds.x + field_vis.bounds.width).round() as i32;
        let y0 = field_vis.bounds.y.round() as i32;
        let y1 = (field_vis.bounds.y + field_vis.bounds.height).round() as i32;
        let mut painted = false;
        for y in y0.max(0)..y1.min(H) {
            for x in x0.max(0)..x1.min(W) {
                let off = y as usize * stride + x as usize * 4;
                let (b, g, r) = (data[off], data[off + 1], data[off + 2]);
                if (r, g, b) != (bg.r, bg.g, bg.b) {
                    painted = true;
                }
            }
        }
        assert!(painted, "the field's framed box must paint ink");
    }

    /// A [`ToolbarButton::Field`] with [`ToolbarItemWidth::Flex`] grows
    /// into the room a `Spacer` would otherwise leave blank, up to its
    /// own `max` — the Finder-style trailing search box.
    #[test]
    fn flex_field_grows_into_spare_space_after_a_spacer() {
        use crate::primitives::text_field::TextField;
        use crate::primitives::toolbar::ToolbarItemWidth;

        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                mk_action("a", "AAAA"),
                ToolbarButton::Spacer,
                ToolbarButton::Field {
                    id: WidgetId::new("tb:search"),
                    field: TextField::new(WidgetId::new("tb:search")),
                    width: ToolbarItemWidth::Flex {
                        min: 40.0,
                        max: 200.0,
                    },
                },
            ],
            bg: None,
            focused_index: None,
        };
        let layout = gtk_toolbar_layout(&bar, None, 8.0, 0.0, 0.0, 300.0, 24.0);
        let field_vis = layout
            .visible_items
            .iter()
            .find(|v| v.action_id == Some(WidgetId::new("tb:search")))
            .expect("Field has a visible slot");
        // "AAAA" at `char_width: 8.0` plus padding is well under 100px,
        // so the field grows past its 40px minimum into the rest of the
        // 300px bar, clamped at its own 200px max.
        assert!(
            field_vis.bounds.width > 40.0,
            "flex field should grow past its own min, got {}",
            field_vis.bounds.width
        );
        assert!(
            field_vis.bounds.width <= 200.0,
            "flex field must never exceed its own max, got {}",
            field_vis.bounds.width
        );
    }
}
