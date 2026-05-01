//! GTK rasteriser for [`crate::ActivityBar`].
//!
//! Cairo + Pango equivalent of the TUI activity-bar drawing path.
//! Computes the primitive's [`crate::ActivityBarLayout`] internally
//! using fixed `ACTIVITY_ROW_PX` row heights — this matches the
//! pre-quadraui native-button height baked into the App's CSS, and
//! callers don't have a `pango::Layout` to do their own measurement
//! against.
//!
//! Returns per-row hit regions ([`crate::ActivityBarRowHit`]) so the
//! caller can route clicks AND query tooltips against the same
//! frame's painted positions.

use gtk4::cairo::Context;
use gtk4::pango;
use gtk4::pango::FontDescription;

use crate::primitives::activity_bar::{ActivityBar, ActivityBarRowHit};
use crate::theme::Theme;

/// Fixed height (in pixels) of a single activity bar row — matches the
/// native-button `set_height_request: 48` baked into vimcode's GTK CSS.
pub const ACTIVITY_ROW_PX: f64 = 48.0;

/// Draw a [`ActivityBar`] into `(0, 0, width, height)` on `cr`.
///
/// Top items render from the top edge downward at `ACTIVITY_ROW_PX`
/// per row. Bottom items pin to the bottom edge upward in pixels (not
/// rounded down to a row boundary), so the last item ends flush with
/// `height` even when `height` isn't an exact multiple of
/// `ACTIVITY_ROW_PX`.
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
    layout: &pango::Layout,
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

    let saved_font = layout.font_description().unwrap_or_default();
    let icon_font = FontDescription::from_string("Symbols Nerd Font, monospace 20");
    layout.set_font_description(Some(&icon_font));
    layout.set_attributes(None);

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

    let rows_total = ((height / ACTIVITY_ROW_PX).floor() as usize).max(1);
    let bottom_count = bar.bottom_items.len().min(rows_total);
    let top_capacity = rows_total.saturating_sub(bottom_count);
    let mut regions: Vec<ActivityBarRowHit> = Vec::new();

    let draw_row = |y: f64,
                    item: &crate::primitives::activity_bar::ActivityItem,
                    row_idx: usize,
                    regions: &mut Vec<ActivityBarRowHit>| {
        let is_hovered = hovered_idx == Some(row_idx);

        if is_hovered {
            cr.set_source_rgb(hover_bg.0, hover_bg.1, hover_bg.2);
            cr.rectangle(0.0, y, width, ACTIVITY_ROW_PX);
            cr.fill().ok();
        }

        if item.is_active {
            cr.set_source_rgb(accent_col.0, accent_col.1, accent_col.2);
            cr.rectangle(0.0, y, 2.0, ACTIVITY_ROW_PX);
            cr.fill().ok();
        }

        layout.set_text(&item.icon);
        let (iw, ih) = layout.pixel_size();
        let fg = if item.is_active || is_hovered {
            active_fg
        } else {
            inactive_fg
        };
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        cr.move_to(
            (width - iw as f64) / 2.0,
            y + (ACTIVITY_ROW_PX - ih as f64) / 2.0,
        );
        pangocairo::functions::show_layout(cr, layout);

        regions.push(ActivityBarRowHit {
            y_start: y,
            y_end: y + ACTIVITY_ROW_PX,
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
        });
    };

    for (row_idx, item) in bar.top_items.iter().take(top_capacity).enumerate() {
        draw_row(
            row_idx as f64 * ACTIVITY_ROW_PX,
            item,
            row_idx,
            &mut regions,
        );
    }

    for (k, item) in bar.bottom_items.iter().rev().take(bottom_count).enumerate() {
        let y = height - (k + 1) as f64 * ACTIVITY_ROW_PX;
        if y < 0.0 {
            break;
        }
        draw_row(y, item, regions.len(), &mut regions);
    }

    layout.set_font_description(Some(&saved_font));

    regions
}
