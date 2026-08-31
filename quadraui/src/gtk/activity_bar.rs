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

use crate::primitives::activity_bar::{ActivityBar, ActivityBarRowHit, ActivitySide};
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
/// # Visual contract
///
/// - **Background:** filled with `theme.tab_bar_bg`.
/// - **Right-edge separator:** 1 px column in `theme.separator`.
/// - **Active row:** 2 px left-edge accent bar in
///   `theme.accent_fg` (or `bar.active_accent` if the bar overrides).
/// - **Hovered row:** subtle background tint
///   (`theme.tab_bar_bg.lighten(0.10)`).
/// - **Icon glyph:** centred in each row using [`ICON_FONT_DESC`]
///   ("Symbols Nerd Font, monospace 18" — 18pt ≈ 24px at 96 dpi,
///   matching VS Code's 24px codicons; #620); foreground is
///   `theme.foreground` for active/hovered rows, `theme.inactive_fg`
///   otherwise. `ACTIVITY_ROW_PX` (the 48px row) is unrelated and
///   unchanged — only the glyph shrank.
pub fn draw_activity_bar(
    cr: &Context,
    pango_layout: &pango::Layout,
    width: f64,
    height: f64,
    bar: &ActivityBar,
    theme: &Theme,
    hovered_idx: Option<usize>,
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

    let accent_col = bar
        .active_accent
        .map(|c| (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0))
        .unwrap_or_else(|| {
            (
                theme.accent_fg.r as f64 / 255.0,
                theme.accent_fg.g as f64 / 255.0,
                theme.accent_fg.b as f64 / 255.0,
            )
        });
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
            cr.set_source_rgb(accent_col.0, accent_col.1, accent_col.2);
            cr.rectangle(0.0, y, 2.0, row_h);
            cr.fill().ok();
        }

        pango_layout.set_text(&item.icon);
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
}
