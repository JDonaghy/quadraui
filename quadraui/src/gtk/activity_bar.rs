//! GTK rasteriser for [`crate::ActivityBar`].
//!
//! Cairo + Pango equivalent of the TUI activity-bar drawing path.
//! Calls [`ActivityBar::layout`] with `ACTIVITY_ROW_PX` as the
//! item height, then paints from the resulting
//! [`crate::ActivityBarLayout`]. Paint and hit-test consume one
//! layout — no independent geometry derivation.
//!
//! Returns per-row hit regions ([`crate::ActivityBarRowHit`]) so the
//! caller can route clicks AND query tooltips against the same
//! frame's painted positions.

use gtk4::cairo::Context;
use gtk4::pango;
use gtk4::pango::FontDescription;

use crate::primitives::activity_bar::{
    ActivityBar, ActivityBarRowHit, ActivityBarStyle, ActivitySide,
};
use crate::theme::Theme;

/// Fixed height (in pixels) of a single activity bar row — matches the
/// native-button `set_height_request: 48` baked into vimcode's GTK CSS.
pub const ACTIVITY_ROW_PX: f64 = 48.0;

/// Pango font description for activity-bar icon glyphs. 18pt renders at
/// ≈ 24px at the standard 96 dpi (`18 * 96 / 72 = 24`), matching VS
/// Code's 24px codicons — the pre-#620 "… 20" size rendered ≈ 26.7px,
/// visibly oversized against the unchanged 48px row (`ACTIVITY_ROW_PX`).
///
/// The family (`"Symbols Nerd Font"`) is the same one
/// [`super::NERD_FONT_FALLBACK_FAMILY`] names for every other GTK
/// chrome glyph path (#416) — kept as a literal here rather than built
/// from the constant because `pub const` string concatenation isn't
/// expressible on stable Rust; if the fallback family ever changes,
/// update both.
pub const ICON_FONT_DESC: &str = "Symbols Nerd Font, monospace 18";

/// Draw an [`ActivityBar`] into `(0, 0, width, height)` on `cr`.
///
/// Computes the layout via [`ActivityBar::layout`] with
/// `ACTIVITY_ROW_PX` item height, then paints from the resolved
/// `visible_items`. Returns per-row hit regions for click + tooltip
/// dispatch.
///
/// Equivalent to [`draw_activity_bar_with_style`] with
/// `ActivityBarStyle::default()`, i.e. no active-row fill — see that
/// function for the full visual contract and #658's reasoning for why the
/// fill lives in a separate style value rather than a field here.
///
/// `nerd_fonts_enabled` picks which half of each item's [`crate::Icon`]
/// paints — `glyph` when `true`, `fallback` when `false` (issue #683).
/// Pass the backend's own flag (`Backend::set_nerd_fonts`).
#[allow(clippy::too_many_arguments)]
pub fn draw_activity_bar(
    cr: &Context,
    pango_layout: &pango::Layout,
    width: f64,
    height: f64,
    bar: &ActivityBar,
    theme: &Theme,
    hovered_idx: Option<usize>,
    nerd_fonts_enabled: bool,
) -> Vec<ActivityBarRowHit> {
    draw_activity_bar_with_style(
        cr,
        pango_layout,
        width,
        height,
        bar,
        &ActivityBarStyle::default(),
        theme,
        hovered_idx,
        nerd_fonts_enabled,
    )
}

/// [`draw_activity_bar`] with an explicit [`ActivityBarStyle`] request
/// (#658). `ActivityBarStyle::default()` reproduces [`draw_activity_bar`]
/// pixel for pixel.
///
/// # Visual contract
///
/// - **Background:** filled with `theme.tab_bar_bg`.
/// - **Right-edge separator:** 1 px column in `theme.separator`.
/// - **Active row:** two independent, opt-in indicators (#658), each
///   producing zero pixels unless requested:
///   - `style.active_bg` fills the whole row (VS Code style).
///   - `bar.active_accent` paints a 2 px left-edge line (JetBrains style).
///
///   Set either, both, or neither — there is no theme fallback for
///   either knob.
/// - **Hovered row:** subtle background tint
///   (`theme.tab_bar_bg.lighten(0.10)`).
/// - **Icon glyph:** centred in each row using [`ICON_FONT_DESC`]
///   ("Symbols Nerd Font, monospace 18" — 18pt ≈ 24px at 96 dpi,
///   matching VS Code's 24px codicons; #620); foreground is
///   `theme.foreground` for active/hovered rows, `theme.inactive_fg`
///   otherwise. `ACTIVITY_ROW_PX` (the 48px row) is unrelated and
///   unchanged — only the glyph shrank.
#[allow(clippy::too_many_arguments)]
pub fn draw_activity_bar_with_style(
    cr: &Context,
    pango_layout: &pango::Layout,
    width: f64,
    height: f64,
    bar: &ActivityBar,
    style: &ActivityBarStyle,
    theme: &Theme,
    hovered_idx: Option<usize>,
    nerd_fonts_enabled: bool,
) -> Vec<ActivityBarRowHit> {
    // Background.
    let (br, bgc, bb) = (
        theme.tab_bar_bg.r as f64 / 255.0,
        theme.tab_bar_bg.g as f64 / 255.0,
        theme.tab_bar_bg.b as f64 / 255.0,
    );
    cr.set_source_rgb(br, bgc, bb);
    cr.rectangle(0.0, 0.0, width, height);
    cr.fill().ok();

    // Right-edge separator.
    let (sr, sg, sb) = (
        theme.separator.r as f64 / 255.0,
        theme.separator.g as f64 / 255.0,
        theme.separator.b as f64 / 255.0,
    );
    cr.set_source_rgb(sr, sg, sb);
    cr.rectangle(width - 1.0, 0.0, 1.0, height);
    cr.fill().ok();

    let saved_font = pango_layout.font_description().unwrap_or_default();
    let icon_font = FontDescription::from_string(ICON_FONT_DESC);
    pango_layout.set_font_description(Some(&icon_font));
    pango_layout.set_attributes(None);

    // #658: no theme fallback for either knob — `None` genuinely means
    // "don't paint this". `Some` colours pass straight through.
    let accent_col = bar
        .active_accent
        .map(|c| (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0));
    let active_bg_col = style
        .active_bg
        .map(|c| (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0));
    let inactive_fg = (
        theme.inactive_fg.r as f64 / 255.0,
        theme.inactive_fg.g as f64 / 255.0,
        theme.inactive_fg.b as f64 / 255.0,
    );
    let active_fg = (
        theme.foreground.r as f64 / 255.0,
        theme.foreground.g as f64 / 255.0,
        theme.foreground.b as f64 / 255.0,
    );
    let hover_bg = {
        let c = theme.tab_bar_bg.lighten(0.10);
        (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0)
    };

    // Compute layout from the primitive — one derivation for both paint
    // and hit-test.
    let layout = bar.layout(width as f32, height as f32, ACTIVITY_ROW_PX as f32);

    let mut regions: Vec<ActivityBarRowHit> = Vec::new();

    for (flat_idx, vi) in layout.visible_items.iter().enumerate() {
        let y = vi.bounds.y as f64;
        let row_h = vi.bounds.height as f64;

        let item = match vi.side {
            ActivitySide::Top => &bar.top_items[vi.item_idx],
            ActivitySide::Bottom => &bar.bottom_items[vi.item_idx],
        };

        let is_hovered = hovered_idx == Some(flat_idx);

        // Active-row fill (VS Code style). Lowest-priority layer — painted
        // first so hover/keyboard-selection tints below still take visual
        // precedence over it when they also apply to this row. `None`
        // (the default) paints nothing here (#658).
        if item.is_active {
            if let Some((r, g, b)) = active_bg_col {
                cr.set_source_rgb(r, g, b);
                cr.rectangle(0.0, y, width, row_h);
                cr.fill().ok();
            }
        }

        // Hover tint (lower priority: painted first so selection can win).
        if is_hovered {
            cr.set_source_rgb(hover_bg.0, hover_bg.1, hover_bg.2);
            cr.rectangle(0.0, y, width, row_h);
            cr.fill().ok();
        }

        // Keyboard-selection highlight: painted *after* hover so the brighter
        // selection tint (lighten 0.20, or `bar.selection_bg`) always wins over
        // the dimmer hover tint (lighten 0.10) when the cursor sits on a hovered
        // row.
        if item.is_keyboard_selected {
            let sel_bg = bar
                .selection_bg
                .map(|c| (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0))
                .unwrap_or_else(|| {
                    let c = theme.tab_bar_bg.lighten(0.20);
                    (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0)
                });
            cr.set_source_rgb(sel_bg.0, sel_bg.1, sel_bg.2);
            cr.rectangle(0.0, y, width, row_h);
            cr.fill().ok();
        }

        if item.is_active {
            if let Some((r, g, b)) = accent_col {
                cr.set_source_rgb(r, g, b);
                cr.rectangle(0.0, y, 2.0, row_h);
                cr.fill().ok();
            }
        }

        let icon_str = if nerd_fonts_enabled {
            item.icon.glyph.as_str()
        } else {
            item.icon.fallback.as_str()
        };
        pango_layout.set_text(icon_str);
        let (iw, ih) = pango_layout.pixel_size();
        let fg = if item.is_active || is_hovered || item.is_keyboard_selected {
            active_fg
        } else {
            inactive_fg
        };
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        cr.move_to((width - iw as f64) / 2.0, y + (row_h - ih as f64) / 2.0);
        super::painted_text::show_layout(cr, pango_layout);

        regions.push(ActivityBarRowHit {
            y_start: y,
            y_end: y + row_h,
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
        });
    }

    pango_layout.set_font_description(Some(&saved_font));

    regions
}

// ── Tests ──────────────────────────────────────────────────────────────────
//
// Headless (no display required) pixel-metric test using a Cairo
// `ImageSurface`, mirroring `gtk::tab_bar`'s test style. Gated on the
// `gtk` feature so it only runs under `cargo test --features gtk`.

#[cfg(test)]
mod tests {
    use super::*;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    /// #620: the activity-bar icon glyph must render distinctly smaller
    /// than the pre-fix 20pt size (≈ 26.7px @ 96 dpi) while
    /// `ACTIVITY_ROW_PX` — the 48px row itself — stays untouched. The
    /// glyph used for measurement doesn't need the real Nerd Font
    /// installed: point-size scaling is monotonic for any font Pango
    /// falls back to, so the relative shrink still holds headless.
    #[test]
    fn icon_glyph_shrinks_while_row_height_is_unchanged() {
        assert_eq!(
            ACTIVITY_ROW_PX, 48.0,
            "the 48px row metric must not move — only the glyph size (#620)"
        );
        assert!(
            ICON_FONT_DESC.ends_with(" 18"),
            "icon font point size should be 18 (≈ 24px @ 96dpi), got {ICON_FONT_DESC:?}"
        );

        let surface = ImageSurface::create(Format::ARgb32, 64, 64).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);

        let measure = |desc: &str| -> (i32, i32) {
            let font = FontDescription::from_string(desc);
            pango_layout.set_font_description(Some(&font));
            pango_layout.set_text("\u{f07b}");
            pango_layout.pixel_size()
        };

        let (old_w, old_h) = measure("Symbols Nerd Font, monospace 20");
        let (new_w, new_h) = measure(ICON_FONT_DESC);

        assert!(
            new_h < old_h,
            "new icon glyph height ({new_h}) should be smaller than the pre-#620 \
             20pt height ({old_h})"
        );
        assert!(
            new_w <= old_w,
            "new icon glyph width ({new_w}) should not exceed the pre-#620 20pt \
             width ({old_w})"
        );
    }

    // ── #658: active_bg / active_accent independence ────────────────────

    use crate::primitives::activity_bar::ActivityItem;
    use crate::types::{Color, WidgetId};

    const ROW_W: i32 = 48;

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y).
    ///
    /// Cairo's `ARgb32` stores each pixel as four bytes in native
    /// (little-endian) byte order: [B, G, R, A].
    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    fn one_item_bar(active_accent: Option<Color>) -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("bar"),
            top_items: vec![ActivityItem {
                id: WidgetId::new("activity:explorer"),
                icon: "E".into(),
                tooltip: String::new(),
                is_active: true,
                is_keyboard_selected: false,
            }],
            bottom_items: vec![],
            active_accent,
            selection_bg: None,
            is_keyboard_focused: false,
        }
    }

    fn paint_one_row(bar: &ActivityBar, style: &ActivityBarStyle) -> ImageSurface {
        let surface = ImageSurface::create(Format::ARgb32, ROW_W, ACTIVITY_ROW_PX as i32)
            .expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let pango_layout = pangocairo::functions::create_layout(&cr);
            draw_activity_bar_with_style(
                &cr,
                &pango_layout,
                ROW_W as f64,
                ACTIVITY_ROW_PX,
                bar,
                style,
                &Theme::default(),
                None,
                false,
            );
        }
        surface.flush();
        surface
    }

    /// #658 acceptance: `style.active_bg: Some(..)` + `active_accent: None`
    /// paints a filled active row with **zero** accent-line pixels — the
    /// legacy 2px left-edge column (x ∈ [0, 2)) must show the fill colour,
    /// not a leftover accent tint (the pre-#658 rasteriser fell back to
    /// `theme.accent_fg` there regardless of `active_accent`).
    #[test]
    fn active_bg_fills_row_with_zero_accent_pixels_when_accent_is_none() {
        let active_bg = Color::rgb(49, 50, 51);
        let bar = one_item_bar(None);
        let style = ActivityBarStyle::new().with_active_bg(active_bg);
        let mut surface = paint_one_row(&bar, &style);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let mid_y = (ACTIVITY_ROW_PX as i32) / 2;
        for x in 0..2 {
            let px = pixel(&data, stride, x, mid_y);
            assert_eq!(
                px,
                (active_bg.r, active_bg.g, active_bg.b),
                "x={x} is inside the legacy accent column; with active_accent \
                 None it must show the active_bg fill, not any accent tint \
                 (zero accent-line pixels)"
            );
        }
        // Deep in the row, away from the glyph, should also be filled.
        let px = pixel(&data, stride, ROW_W - 4, mid_y);
        assert_eq!(px, (active_bg.r, active_bg.g, active_bg.b));
    }

    /// The flip side: `active_accent: Some(..)` with no `active_bg` still
    /// paints the traditional 2px line, and nothing past it.
    #[test]
    fn active_accent_paints_two_px_line_when_set() {
        let accent = Color::rgb(80, 140, 255);
        let bar = one_item_bar(Some(accent));
        let mut surface = paint_one_row(&bar, &ActivityBarStyle::default());
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");

        let mid_y = (ACTIVITY_ROW_PX as i32) / 2;
        assert_eq!(
            pixel(&data, stride, 0, mid_y),
            (accent.r, accent.g, accent.b)
        );
        assert_eq!(
            pixel(&data, stride, 1, mid_y),
            (accent.r, accent.g, accent.b)
        );

        let theme = Theme::default();
        let bg_px = pixel(&data, stride, 2, 2);
        assert_eq!(
            bg_px,
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "x=2 is past the 2px accent strip and should be tab_bar_bg"
        );
    }

    /// Neither knob set: the active row paints exactly like an inactive
    /// row — no fill, no line. This is "today's behaviour" the doc on
    /// both fields promises stays the default.
    #[test]
    fn neither_knob_set_paints_plain_row() {
        let bar = one_item_bar(None);
        let mut surface = paint_one_row(&bar, &ActivityBarStyle::default());
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let theme = Theme::default();

        let mid_y = (ACTIVITY_ROW_PX as i32) / 2;
        for x in [0, 1, ROW_W - 4] {
            let px = pixel(&data, stride, x, mid_y);
            assert_eq!(
                px,
                (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
                "x={x} should be plain tab_bar_bg when neither active_bg nor \
                 active_accent is set"
            );
        }
    }

    /// #683: `nerd_fonts_enabled` selects `Icon::glyph` vs `Icon::fallback`.
    /// Uses two ASCII strings of clearly different width (`"WWWW"` vs
    /// `"E"`) rather than a real Nerd Font codepoint, so the assertion
    /// holds headless even without the Symbols Nerd Font installed —
    /// same reasoning as `icon_glyph_shrinks_while_row_height_is_unchanged`
    /// above. Measures the painted glyph's pixel bounding-box width via
    /// foreground-vs-background scanning: whichever half of the `Icon`
    /// paints, a wider string produces a wider non-background run.
    #[test]
    fn nerd_fonts_flag_selects_glyph_or_fallback() {
        use crate::types::Icon;

        let bar = ActivityBar {
            id: WidgetId::new("bar"),
            top_items: vec![ActivityItem {
                id: WidgetId::new("activity:explorer"),
                icon: Icon::new("WWWW", "E"),
                tooltip: String::new(),
                is_active: false,
                is_keyboard_selected: false,
            }],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
        };

        let painted_width = |nerd_fonts_enabled: bool| -> i32 {
            let mut surface =
                ImageSurface::create(Format::ARgb32, ROW_W, ACTIVITY_ROW_PX as i32).unwrap();
            {
                let cr = Context::new(&surface).unwrap();
                let pango_layout = pangocairo::functions::create_layout(&cr);
                draw_activity_bar_with_style(
                    &cr,
                    &pango_layout,
                    ROW_W as f64,
                    ACTIVITY_ROW_PX,
                    &bar,
                    &ActivityBarStyle::default(),
                    &Theme::default(),
                    None,
                    nerd_fonts_enabled,
                );
            }
            surface.flush();
            let stride = surface.stride() as usize;
            let data = surface.data().unwrap();
            let theme = Theme::default();
            let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);
            let mid_y = (ACTIVITY_ROW_PX as i32) / 2;
            let mut left = None;
            let mut right = None;
            for x in 0..ROW_W {
                if pixel(&data, stride, x, mid_y) != bg {
                    left.get_or_insert(x);
                    right = Some(x);
                }
            }
            match (left, right) {
                (Some(l), Some(r)) => r - l + 1,
                _ => 0,
            }
        };

        let glyph_width = painted_width(true);
        let fallback_width = painted_width(false);
        assert!(
            glyph_width > fallback_width,
            "nerd_fonts_enabled: true should paint the wider glyph half \
             (\"WWWW\", measured {glyph_width}px) vs the narrower fallback \
             half (\"E\", measured {fallback_width}px) painted when false"
        );
    }
}
