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
    native_surface_paint, ActivityBar, ActivityBarRowHit, ActivityBarStyle,
};
use crate::theme::Theme;

/// Fixed height (in pixels) of a single activity bar row — matches the
/// native-button `set_height_request: 48` baked into vimcode's GTK CSS.
pub const ACTIVITY_ROW_PX: f64 = 48.0;

/// Pango font description for activity-bar icon glyphs, at the
/// pre-#1157 fixed 18pt (≈ 24px at the standard 96 dpi,
/// `18 * 96 / 72 = 24`) size — matches
/// [`crate::primitives::activity_bar::DEFAULT_ACTIVITY_ICON_SIZE_PX`]'s
/// 24px default converted through [`activity_bar_icon_font`]'s own
/// pt/px ratio; the pre-#620 "… 20" size rendered ≈ 26.7px, visibly
/// oversized against the unchanged 48px row (`ACTIVITY_ROW_PX`).
///
/// The family (`"Symbols Nerd Font"`) is the same default
/// [`super::NERD_FONT_FALLBACK_FAMILY`] names for every other GTK
/// chrome glyph path (#416) — kept as a literal here rather than built
/// from the constant because `pub const` string concatenation isn't
/// expressible on stable Rust; if the default fallback family ever
/// changes, update both. Only a *default*, unlike the family
/// [`activity_bar_icon_font`] actually paints with: this constant does
/// not track a `Backend::set_nerd_font_fallback` override
/// (`GtkBackend::set_nerd_font_fallback`, issue #929) — it exists for
/// its point size (`draw_activity_bar`'s size-assertion tests read
/// `ICON_FONT_DESC.ends_with(" 18")`) and as a byte-for-byte pin for
/// [`activity_bar_icon_font`]'s default-size output (modulo family
/// substitution).
///
/// `#[allow(dead_code)]`: `activity_bar_icon_font` builds its own
/// description at whatever size the caller resolves (#1157) rather than
/// parsing this literal, so nothing outside `#[cfg(test)]` reads this
/// constant any more — it's kept `pub` (and non-`#[cfg(test)]`) purely as
/// the size the two tests above pin against, matching this module's
/// private (non-`pub`) enclosing `gtk::activity_bar` — `pub` alone
/// doesn't make it externally reachable, so `dead_code` still fires
/// outside test builds.
#[allow(dead_code)]
pub const ICON_FONT_DESC: &str = "Symbols Nerd Font, monospace 18";

/// Build the Pango font description [`draw_activity_bar_with_style`]
/// paints the icon glyph with, at `size_px` device-independent pixels
/// (issue #1157 — [`crate::ActivityBarStyle::icon_size_px`]).
///
/// Converts `size_px` to Pango's point-size convention via the 96/72 dpi
/// ratio [`ICON_FONT_DESC`]'s own doc explains, so
/// `activity_bar_icon_font(24.0)` — the
/// [`crate::primitives::activity_bar::DEFAULT_ACTIVITY_ICON_SIZE_PX`]
/// default — reproduces [`ICON_FONT_DESC`]'s `18` byte-for-byte (see
/// `icon_font_matches_the_pinned_default_description` below). The family
/// always comes from [`super::current_nerd_font_fallback_family`] —
/// [`NERD_FONT_FALLBACK_FAMILY`][super::NERD_FONT_FALLBACK_FAMILY] until a
/// `Backend::set_nerd_font_fallback` call overrides it — never from
/// [`ICON_FONT_DESC`]'s own literal family.
fn activity_bar_icon_font(size_px: f32) -> FontDescription {
    let pt = size_px * 72.0 / 96.0;
    let mut f = FontDescription::from_string(&format!("monospace {pt}"));
    f.set_family(&format!(
        "{}, monospace",
        super::current_nerd_font_fallback_family()
    ));
    f
}

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
/// - **Icon glyph:** centred in each row at `style.resolved_icon_size_px()`
///   (issue #1157; defaults to
///   [`crate::primitives::activity_bar::DEFAULT_ACTIVITY_ICON_SIZE_PX`] —
///   24px, matching VS Code's codicons; see [`activity_bar_icon_font`]'s
///   own doc for the pt/px conversion and #620's original fixed-18pt
///   fix this generalises); foreground is `theme.foreground` for
///   active/hovered rows, `theme.inactive_fg` otherwise.
///   `ACTIVITY_ROW_PX` (the 48px row) is unrelated and independent of
///   the icon's own size.
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
    let saved_font = pango_layout.font_description().unwrap_or_default();
    let icon_font = activity_bar_icon_font(style.resolved_icon_size_px());
    pango_layout.set_font_description(Some(&icon_font));
    pango_layout.set_attributes(None);

    // Compute layout from the primitive — one derivation for both paint
    // and hit-test.
    let layout = bar.layout(width as f32, height as f32, ACTIVITY_ROW_PX as f32);

    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: true,
    };
    let regions = native_surface_paint::paint(
        bar,
        &layout,
        style,
        &mut surface,
        theme,
        hovered_idx,
        nerd_fonts_enabled,
    );

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

    /// #1157: [`activity_bar_icon_font`]'s default-size output (24px, the
    /// crate's `DEFAULT_ACTIVITY_ICON_SIZE_PX`) must reproduce
    /// [`ICON_FONT_DESC`]'s pinned `18` point size byte-for-byte (modulo
    /// family, which always comes from
    /// `super::current_nerd_font_fallback_family` on both sides here).
    #[test]
    fn icon_font_matches_the_pinned_default_description() {
        use crate::primitives::activity_bar::DEFAULT_ACTIVITY_ICON_SIZE_PX;

        let desc = activity_bar_icon_font(DEFAULT_ACTIVITY_ICON_SIZE_PX);
        let pinned = FontDescription::from_string(ICON_FONT_DESC);
        assert_eq!(
            desc.size(),
            pinned.size(),
            "activity_bar_icon_font({DEFAULT_ACTIVITY_ICON_SIZE_PX}) should match \
             ICON_FONT_DESC's pinned point size"
        );
    }

    /// #1157: the icon glyph's *ink* size follows
    /// `ActivityBarStyle::icon_size_px`, not whatever font size the
    /// caller's `pango::Layout` happened to carry when `draw_activity_bar_with_style`
    /// was called — the regression this issue reports ("icon size follows
    /// the active font").
    #[test]
    fn icon_ink_height_follows_style_not_the_callers_font() {
        let bar = one_item_bar(None);

        // Bounding-box height (in device px) of every non-background
        // pixel in a freshly painted single-row surface.
        let ink_height = |style: &ActivityBarStyle, callers_font_pt: f64| -> i32 {
            let mut surface =
                ImageSurface::create(Format::ARgb32, ROW_W, ACTIVITY_ROW_PX as i32).unwrap();
            {
                let cr = Context::new(&surface).unwrap();
                let pango_layout = pangocairo::functions::create_layout(&cr);
                // Simulate a caller whose own (editor/chrome) font is set
                // to something wildly different before painting — the
                // fix must not let this leak into the icon glyph size.
                pango_layout.set_font_description(Some(&FontDescription::from_string(&format!(
                    "monospace {callers_font_pt}"
                ))));
                draw_activity_bar_with_style(
                    &cr,
                    &pango_layout,
                    ROW_W as f64,
                    ACTIVITY_ROW_PX,
                    &bar,
                    style,
                    &Theme::default(),
                    None,
                    false,
                );
            }
            surface.flush();
            let stride = surface.stride() as usize;
            let data = surface.data().unwrap();
            let theme = Theme::default();
            let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);
            let (mut min_y, mut max_y) = (i32::MAX, i32::MIN);
            for y in 0..ACTIVITY_ROW_PX as i32 {
                // Exclude the last column: it's the bar's own 1px
                // right-edge separator (always painted, regardless of
                // icon size), not glyph ink.
                for x in 0..ROW_W - 1 {
                    if pixel(&data, stride, x, y) != bg {
                        min_y = min_y.min(y);
                        max_y = max_y.max(y);
                    }
                }
            }
            if min_y > max_y {
                0
            } else {
                max_y - min_y + 1
            }
        };

        let default_at_small_font = ink_height(&ActivityBarStyle::default(), 6.0);
        let default_at_huge_font = ink_height(&ActivityBarStyle::default(), 60.0);
        assert_eq!(
            default_at_small_font, default_at_huge_font,
            "default icon ink height must not track the caller's font size \
             (6pt vs 60pt should paint identically)"
        );

        let small_icon = ink_height(&ActivityBarStyle::new().with_icon_size_px(10.0), 14.0);
        assert!(
            small_icon < default_at_huge_font,
            "icon_size_px(10.0) ({small_icon}px ink) should paint smaller than \
             the 24px default ({default_at_huge_font}px ink)"
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
