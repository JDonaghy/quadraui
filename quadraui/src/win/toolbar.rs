//! Direct2D / DirectWrite rasteriser for [`crate::primitives::toolbar::Toolbar`]
//! (#730).
//!
//! Mirrors `gtk::toolbar` / `macos::toolbar`'s structure and visual
//! contract: [`Toolbar::layout`] (the D6 layout API) does every
//! positioning decision via the shared
//! [`crate::primitives::toolbar::measure_button`] formula (#730); this
//! module only measures (via [`DWriteMeasure`], a thin adapter over
//! [`DWrite`]) and paints (`ID2D1RenderTarget::FillRectangle` /
//! `DrawLine` / `DrawText`). Paint and hit-test both derive from one
//! `Toolbar::layout` call, so they can't drift apart.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod toolbar;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! ## Per-state colouring
//!
//! Priority (highest first): pressed → hovered → focused → is_active →
//! enabled — identical to `gtk::toolbar` / `macos::toolbar`.
//!
//! | State              | Foreground             | Background           |
//! |--------------------|------------------------|----------------------|
//! | Action, enabled    | `theme.foreground`     | `bar_bg`             |
//! | Action, disabled   | `theme.muted_fg`       | `bar_bg`             |
//! | Action, is_active  | `theme.foreground`     | `theme.selected_bg`  |
//! | Action, focused    | `theme.foreground`     | `bar_bg` + ring      |
//! | Action, hovered    | `theme.hover_fg`       | `theme.hover_bg`     |
//! | Action, pressed    | `theme.foreground`     | `theme.selected_bg`  |
//! | Separator          | `theme.muted_fg`       | `bar_bg`             |
//! | Label              | `Label.fg` or `muted`  | `bar_bg`             |
//!
//! `bar_bg` is `Toolbar.bg.unwrap_or(theme.header_bg)`. `WinBackend` does
//! not yet carry a live [`Theme`] (see `win::status_bar`'s module doc),
//! so this rasteriser uses [`Theme::default`], the same posture every
//! other `win::` chrome rasteriser takes.
//!
//! ## Scope for #730
//!
//! No rounded-rect / stroke-inset helper exists yet in `win::text`
//! beyond [`stroke_rect`] (which insets a plain rectangle, not a
//! rounded one — see its doc), so the hover/pressed/active highlight
//! and focus ring paint as plain rectangles rather than GTK's
//! rounded-rect pills. Hit-test bounds and click routing are unaffected
//! — only the highlight's corner treatment differs.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{draw_line, fill_rect, stroke_rect, DWrite};
use crate::event::Rect;
use crate::primitives::layout_metrics::TextMeasure;
use crate::primitives::toolbar::{
    action_text, measure_button, Toolbar, ToolbarButton, ToolbarItemMeasure, ToolbarLayout,
};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Adapts a live [`DWrite`] handle to the shared [`TextMeasure`] trait
/// so [`crate::primitives::toolbar::measure_button`] never has to name a
/// DirectWrite type — mirrors `macos::toolbar::CtFontMeasure` /
/// `gtk::toolbar::PangoMeasure` / `win::form::DWriteMeasure`, which all
/// exist for exactly this reason (#730).
struct DWriteMeasure<'a>(&'a DWrite);

impl TextMeasure for DWriteMeasure<'_> {
    fn width_of(&self, text: &str) -> f32 {
        self.0.measure_text(text).map(|(w, _)| w).unwrap_or(0.0)
    }
}

/// Compute the Win-GUI pixel/DIP layout for a [`Toolbar`] without
/// painting — the DirectWrite twin of [`draw_toolbar`]'s internal layout
/// call.
///
/// Coordinate frame: **ABSOLUTE** (`rect.x`/`rect.y` baked into every
/// item's `bounds`), matching [`crate::Backend::toolbar_layout`]'s
/// documented contract and `gtk_toolbar_layout` / `mac_toolbar_layout`.
pub fn win_toolbar_layout(dwrite: &DWrite, rect: Rect, bar: &Toolbar) -> ToolbarLayout {
    let measure = DWriteMeasure(dwrite);
    bar.layout(rect.x, rect.y, rect.width, rect.height, |btn| {
        ToolbarItemMeasure::new(measure_button(&measure, btn))
    })
}

/// Draw a [`Toolbar`] into `rect` (DIPs) on `target`. Returns the
/// resolved [`ToolbarLayout`] for host click dispatch.
pub fn draw_toolbar(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    bar: &Toolbar,
    hovered_id: Option<&WidgetId>,
    pressed_id: Option<&WidgetId>,
) -> ToolbarLayout {
    let layout = win_toolbar_layout(dwrite, rect, bar);

    if rect.width <= 0.0 || rect.height <= 0.0 {
        return layout;
    }

    let theme = Theme::default();
    let bar_bg = bar.bg.unwrap_or(theme.header_bg);
    let _ = fill_rect(target, rect, bar_bg);

    for vis in &layout.visible_items {
        let item = vis.bounds;
        if item.width <= 0.0 || item.height <= 0.0 {
            continue;
        }

        let btn = &bar.buttons[vis.item_idx];
        match btn {
            ToolbarButton::Action {
                id,
                label,
                icon,
                key_hint,
                enabled,
                is_active,
                ..
            } => {
                let is_hovered = *enabled && hovered_id == Some(id);
                let is_pressed = *enabled && pressed_id == Some(id);
                let is_focused = *enabled && bar.focused_index == Some(vis.item_idx);

                // Highlight background: pressed/active > hovered > none.
                let highlight = if is_pressed || *is_active {
                    Some(theme.selected_bg)
                } else if is_hovered {
                    Some(theme.hover_bg)
                } else {
                    None
                };
                if let Some(bg) = highlight {
                    let inset = Rect::new(
                        item.x + 2.0,
                        item.y + 2.0,
                        (item.width - 4.0).max(0.0),
                        (item.height - 4.0).max(0.0),
                    );
                    let _ = fill_rect(target, inset, bg);
                }

                // Focus ring: only when not already visually dominated
                // by hover / pressed / active.
                if is_focused && !is_hovered && !is_pressed && !*is_active {
                    let ring = Rect::new(
                        item.x + 1.5,
                        item.y + 1.5,
                        (item.width - 3.0).max(0.0),
                        (item.height - 3.0).max(0.0),
                    );
                    let _ = stroke_rect(target, ring, theme.accent_fg, 1.0);
                }

                let fg = if !*enabled {
                    theme.muted_fg
                } else if is_hovered {
                    theme.hover_fg
                } else {
                    theme.foreground
                };

                let text = action_text(label, icon.as_deref(), key_hint.as_deref());
                let (tw, th) = dwrite.measure_text(&text).unwrap_or((0.0, 0.0));
                let tx = item.x + (item.width - tw) / 2.0;
                let ty = item.y + (item.height - th) / 2.0;
                let _ = dwrite.draw_text(target, &text, Rect::new(tx, ty, tw, th), fg);
            }
            ToolbarButton::Separator => {
                let mid_x = item.x + item.width / 2.0;
                let pad_y = (item.height * 0.2).max(2.0);
                let _ = draw_line(
                    target,
                    mid_x,
                    item.y + pad_y,
                    mid_x,
                    item.y + item.height - pad_y,
                    theme.muted_fg,
                    1.0,
                );
            }
            ToolbarButton::Label { text, fg } => {
                let color = fg.unwrap_or(theme.muted_fg);
                let (tw, th) = dwrite.measure_text(text).unwrap_or((0.0, 0.0));
                let ty = item.y + (item.height - th) / 2.0;
                let _ = dwrite.draw_text(target, text, Rect::new(item.x, ty, tw, th), color);
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::ToolbarHit;
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 240.0;
    const H: f32 = 40.0;

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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let rect = Rect::new(x, y, W - x, H - y);

        surface
            .paint(|target| {
                draw_toolbar(target, &dwrite, rect, bar, None, None);
            })
            .map(|_| win_toolbar_layout(&dwrite, rect, bar))
            .expect("paint toolbar")
    }

    /// C0 smoke: `draw_toolbar` must actually paint + return a
    /// click-routable layout rather than panicking or hitting a
    /// `todo!()` (#730's acceptance bar — "draw_toolbar survives C0 on
    /// win").
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

    /// Non-zero-origin regression guard (issue #494 / LESSONS.md "Layout
    /// helpers must return coords in the same frame across backends") —
    /// `Toolbar::layout` bakes `rect.x`/`rect.y` into every item's
    /// `bounds` (ABSOLUTE frame), matching the GTK/macOS/TUI twins.
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
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0).expect("create DWrite");
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");

        let painted = surface
            .paint(|target| {
                draw_toolbar(target, &dwrite, rect, &bar, None, None);
            })
            .map(|_| win_toolbar_layout(&dwrite, rect, &bar))
            .expect("paint");
        let no_paint = win_toolbar_layout(&dwrite, rect, &bar);
        assert_eq!(painted, no_paint);
    }
}
