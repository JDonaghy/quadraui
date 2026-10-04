//! GTK rasteriser for [`crate::primitives::toolbar::Toolbar`].
//!
//! `gtk_toolbar_layout` stays here — it needs Pango's own text
//! measurement to size each item. Content painting (background,
//! per-state colouring, hover/pressed/active highlight, focus ring,
//! separators, labels) moved to the shared
//! [`crate::primitives::toolbar::native_surface_paint::paint`] (#1081,
//! `PaintSurface` Phase 4 slice 5/8) — see that fn's module doc for
//! the highlight-corner drift it resolved (this backend's rounded pill
//! is now what every backend paints, closing macOS's and Windows'
//! square-corner gap instead of flattening this one down to match
//! them).
//!
//! See that module's own doc for the full per-state colouring table.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::layout_metrics::TextMeasure;
use crate::primitives::toolbar::{
    measure_button, native_surface_paint, Toolbar, ToolbarItemMeasure, ToolbarLayout,
    ToolbarPaintOptions,
};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Adapts a live `pango::Layout` (falling back to a `char_width`-based
/// estimate when none is available, e.g. from a layout-only call between
/// paint frames) to the shared [`TextMeasure`] trait so
/// [`crate::primitives::toolbar::measure_button`] never has to name a
/// Pango type — mirrors `macos::toolbar`'s / `win::toolbar`'s twin
/// adapters (#730).
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
/// (issue #260) resolves where button/label text paints within a slot
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{ToolbarButton, ToolbarHit};
    use crate::types::WidgetId;

    fn test_toolbar() -> Toolbar {
        Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
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

    /// `toolbar_layout` is documented **ABSOLUTE** (issue #505):
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

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }

    // ── #260: `ToolbarVAlign` ────────────────────────────────────────────

    /// Pixel-level multi-row paint test (issue #260's test plan): paint
    /// the same bar into the same tall slot once per [`ToolbarVAlign`]
    /// variant and confirm the row of the first painted (non-background)
    /// pixel moves accordingly — `Top` paints highest, `Bottom` lowest,
    /// `Center` strictly between the two. Exercises the real
    /// [`draw_toolbar`] → [`crate::primitives::toolbar::native_surface_paint::paint`]
    /// path through an in-memory Cairo `ImageSurface`, the same headless
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
}
