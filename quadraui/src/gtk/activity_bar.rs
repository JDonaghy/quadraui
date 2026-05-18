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
/// - **Icon glyph:** centred in each row using "Symbols Nerd Font,
///   monospace 20" Pango font; foreground is `theme.foreground` for
///   active/hovered rows, `theme.inactive_fg` otherwise.
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
    let icon_font = FontDescription::from_string("Symbols Nerd Font, monospace 20");
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

        if is_hovered {
            cr.set_source_rgb(hover_bg.0, hover_bg.1, hover_bg.2);
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
        let fg = if item.is_active || is_hovered {
            active_fg
        } else {
            inactive_fg
        };
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        cr.move_to((width - iw as f64) / 2.0, y + (row_h - ih as f64) / 2.0);
        pangocairo::functions::show_layout(cr, pango_layout);

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
