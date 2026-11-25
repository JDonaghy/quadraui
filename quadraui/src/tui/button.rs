//! TUI entry points for [`crate::Button`] — thin delegates onto the
//! widget's own cell painter, `crate::primitives::button::cell_paint`,
//! which documents the bracket-chrome/icon/intent-colour rules. This
//! module only adapts a raw ratatui [`Buffer`] into the
//! [`crate::CellSurface`] that painter takes, and converts a ratatui
//! cell [`Rect`] into the quadraui one the widget's own cell layout
//! takes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::cell_surface::TuiCellSurface;
use crate::backend::ColorDepth;
use crate::event::Rect as QRect;
use crate::primitive::LayoutCtx;
use crate::primitives::button::{cell_button_layout, cell_paint, Button, ButtonLayout};
use crate::theme::Theme;

/// A [`LayoutCtx`] carrying only `nerd_fonts_enabled` — [`tui_button_layout`]
/// and [`draw_button`]/[`draw_button_interactive`] take that flag as a
/// loose `bool` parameter (their pre-existing, non-contract signature),
/// so this builds the ctx [`cell_button_layout`] needs without a real
/// backend in reach. `line_height`/`char_width` are nominal TUI cell
/// values — [`cell_button_layout`] never reads either.
fn cell_ctx(nerd_fonts_enabled: bool) -> LayoutCtx<'static> {
    LayoutCtx::new(1.0, 1.0, nerd_fonts_enabled)
}

/// Compute the TUI cell-unit layout for a [`Button`] without painting.
/// The control has no internal sub-regions, so `area` maps straight
/// through to [`Button::layout`]'s bounds.
pub fn tui_button_layout(button: &Button, area: Rect) -> ButtonLayout {
    cell_button_layout(
        button,
        &cell_ctx(false),
        QRect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
    )
}

/// Draw a [`Button`] into `area` on `buf`. Returns the layout for host
/// click dispatch.
///
/// Identical to [`draw_button_interactive`] with `is_hovered` /
/// `is_pressed` both `false` — plain, no-hover-state paint. `nerd_fonts_enabled`
/// picks which half of [`Button::icon`] paints — see that field's own doc.
pub fn draw_button(
    buf: &mut Buffer,
    area: Rect,
    button: &Button,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) -> ButtonLayout {
    draw_button_interactive(buf, area, button, theme, false, false, nerd_fonts_enabled)
}

/// [`draw_button`], tinting the background via [`Theme::button_pressed_bg`]
/// / [`Theme::button_hover_bg`] when `is_pressed` / `is_hovered` is set —
/// the TUI side of [`crate::Backend::draw_button_with_state`]. Pressed
/// takes priority over hover, same as the GTK/macOS/Win `native_surface_paint`
/// path. See [`draw_button`] for `nerd_fonts_enabled`.
#[allow(clippy::too_many_arguments)]
pub fn draw_button_interactive(
    buf: &mut Buffer,
    area: Rect,
    button: &Button,
    theme: &Theme,
    is_hovered: bool,
    is_pressed: bool,
    nerd_fonts_enabled: bool,
) -> ButtonLayout {
    let layout = cell_button_layout(
        button,
        &cell_ctx(nerd_fonts_enabled),
        QRect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
    );
    draw_button_layout_interactive(buf, button, &layout, theme, is_hovered, is_pressed);
    layout
}

/// Draw a [`Button`] into `layout.bounds` on `buf`, plain (no hover/
/// pressed tint) — what [`crate::primitive::Primitive::paint_cells`]
/// calls.
///
/// `layout` must be the same [`ButtonLayout`] the host hit-tests against
/// (typically [`tui_button_layout`]'s return value) so paint and
/// click-routing can never disagree.
pub(crate) fn draw_button_layout(
    buf: &mut Buffer,
    button: &Button,
    layout: &ButtonLayout,
    theme: &Theme,
) {
    draw_button_layout_interactive(buf, button, layout, theme, false, false);
}

/// [`draw_button_layout`], tinting the background via
/// `Theme::button_pressed_bg`/`Theme::button_hover_bg` — the cell-grid
/// twin of [`draw_button_interactive`], against an already-resolved
/// `layout` rather than a rect.
fn draw_button_layout_interactive(
    buf: &mut Buffer,
    button: &Button,
    layout: &ButtonLayout,
    theme: &Theme,
    is_hovered: bool,
    is_pressed: bool,
) {
    // The painter never moves the cursor, so the sink is never read.
    let mut cursor_sink = None;
    let mut surface = TuiCellSurface::new(buf, ColorDepth::TrueColor, &mut cursor_sink);
    cell_paint::paint(button, layout, &mut surface, theme, is_hovered, is_pressed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::button::{ButtonHit, ButtonIconPosition, ButtonVariant};
    use crate::tui::ratatui_color;
    use crate::types::WidgetId;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_button(label: &str) -> Button {
        Button::new(WidgetId::new("dialog:ok"), label)
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_button_layout` bakes `area.x`/`area.y` straight into
    // `Button::layout`'s returned bounds (absolute frame), so the
    // regression guard is a full paint+hit_test round trip re-run at a
    // non-zero origin.
    fn primary_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 8, 1);
        let mut buf = Buffer::empty(area);
        let button = mk_button("OK");
        let layout = draw_button(&mut buf, area, &button, &Theme::default(), false);

        assert_eq!(cell_char(&buf, origin_x, origin_y), '[');
        assert_eq!(cell_char(&buf, origin_x + 2, origin_y), 'O');
        assert_eq!(cell_char(&buf, origin_x + 3, origin_y), 'K');
        assert_eq!(cell_char(&buf, origin_x + 5, origin_y), ']');

        let hit = layout.hit_test(Point::new(origin_x as f32 + 1.0, origin_y as f32));
        assert_eq!(hit, ButtonHit::Body);
        let outside = layout.hit_test(Point::new(origin_x as f32 + 50.0, origin_y as f32));
        assert_eq!(outside, ButtonHit::Outside);
    }

    #[test]
    fn primary_paint_and_hit_round_trip() {
        primary_round_trip_at(0, 0);
    }

    #[test]
    fn primary_paint_and_hit_round_trip_at_nonzero_origin() {
        primary_round_trip_at(7, 13);
    }

    #[test]
    fn icon_variant_paints_bare_label_without_brackets() {
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        let mut button = mk_button("X");
        button.variant = ButtonVariant::Icon;
        draw_button(&mut buf, area, &button, &Theme::default(), false);
        assert_eq!(cell_char(&buf, 0, 0), 'X');
        assert_eq!(cell_char(&buf, 1, 0), ' ');
    }

    #[test]
    fn disabled_button_still_paints_but_dims() {
        let area = Rect::new(0, 0, 8, 1);
        let mut buf = Buffer::empty(area);
        let mut button = mk_button("OK");
        button.disabled = true;
        let theme = Theme::default();
        draw_button(&mut buf, area, &button, &theme, false);
        assert_eq!(cell_char(&buf, 2, 0), 'O');
    }

    /// A taller cell centres the label (keeping it off the focus ring's
    /// edge rows) while the hit rect still covers the whole cell.
    #[test]
    fn taller_area_centres_the_label_and_keeps_whole_cell_hit_bounds() {
        let area = Rect::new(0, 0, 8, 3);
        let mut buf = Buffer::empty(area);
        let layout = draw_button(&mut buf, area, &mk_button("OK"), &Theme::default(), false);
        assert_eq!(cell_char(&buf, 0, 1), '[', "label paints on the middle row");
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(cell_char(&buf, 0, 2), ' ');
        assert_eq!(layout.bounds.height.round() as u16, area.height);
        assert_eq!(layout.hit_test(Point::new(0.0, 0.0)), ButtonHit::Body);
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let buf_area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(buf_area);
        let button = mk_button("OK");
        let _layout = draw_button(
            &mut buf,
            Rect::new(0, 0, 0, 0),
            &button,
            &Theme::default(),
            false,
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    fn cell_bg(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf[(x, y)]
            .style()
            .bg
            .unwrap_or(ratatui::style::Color::Reset)
    }

    #[test]
    fn hovered_background_differs_from_resting() {
        let area = Rect::new(0, 0, 8, 1);
        let theme = Theme::default();
        let button = mk_button("OK");

        let mut resting_buf = Buffer::empty(area);
        draw_button_interactive(&mut resting_buf, area, &button, &theme, false, false, false);
        let resting_bg = cell_bg(&resting_buf, 3, 0);

        let mut hovered_buf = Buffer::empty(area);
        draw_button_interactive(&mut hovered_buf, area, &button, &theme, true, false, false);
        let hovered_bg = cell_bg(&hovered_buf, 3, 0);

        assert_ne!(
            resting_bg, hovered_bg,
            "hovering should repaint the button background"
        );
    }

    #[test]
    fn pressed_background_differs_from_hovered_and_resting() {
        let area = Rect::new(0, 0, 8, 1);
        let theme = Theme::default();
        let button = mk_button("OK");

        let mut resting_buf = Buffer::empty(area);
        draw_button_interactive(&mut resting_buf, area, &button, &theme, false, false, false);
        let resting_bg = cell_bg(&resting_buf, 3, 0);

        let mut hovered_buf = Buffer::empty(area);
        draw_button_interactive(&mut hovered_buf, area, &button, &theme, true, false, false);
        let hovered_bg = cell_bg(&hovered_buf, 3, 0);

        let mut pressed_buf = Buffer::empty(area);
        draw_button_interactive(&mut pressed_buf, area, &button, &theme, true, true, false);
        let pressed_bg = cell_bg(&pressed_buf, 3, 0);

        assert_ne!(pressed_bg, resting_bg);
        assert_ne!(pressed_bg, hovered_bg);
    }

    #[test]
    fn disabled_button_ignores_hover_and_pressed() {
        let area = Rect::new(0, 0, 8, 1);
        let theme = Theme::default();
        let mut button = mk_button("OK");
        button.disabled = true;

        let mut resting_buf = Buffer::empty(area);
        draw_button_interactive(&mut resting_buf, area, &button, &theme, false, false, false);
        let resting_bg = cell_bg(&resting_buf, 3, 0);

        let mut hovered_buf = Buffer::empty(area);
        draw_button_interactive(&mut hovered_buf, area, &button, &theme, true, true, false);
        let hovered_bg = cell_bg(&hovered_buf, 3, 0);

        assert_eq!(
            resting_bg, hovered_bg,
            "a disabled button must not paint hover/pressed tints"
        );
    }

    fn cell_fg(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf[(x, y)]
            .style()
            .fg
            .unwrap_or(ratatui::style::Color::Reset)
    }

    /// `Danger`/`Warning` paint intent entirely through the foreground
    /// colour on TUI — both the bracket chrome and the label text — and
    /// leave the background untouched (same `surface_bg` as `Secondary`),
    /// unlike `Primary`'s solid `accent_bg` fill.
    #[test]
    fn danger_variant_paints_error_fg_on_brackets_and_label_with_no_bg_fill() {
        let area = Rect::new(0, 0, 10, 1);
        let theme = Theme::default();
        let mut button = mk_button("Delete");
        button.variant = ButtonVariant::Danger;

        let mut buf = Buffer::empty(area);
        draw_button(&mut buf, area, &button, &theme, false);

        let bracket_fg = cell_fg(&buf, 0, 0);
        let label_fg = cell_fg(&buf, 2, 0);
        assert_eq!(bracket_fg, ratatui_color(theme.error_fg));
        assert_eq!(label_fg, ratatui_color(theme.error_fg));
        assert_eq!(
            cell_bg(&buf, 0, 0),
            ratatui_color(theme.surface_bg),
            "Danger must not fill a solid background on TUI"
        );
    }

    #[test]
    fn warning_variant_paints_warning_fg_on_brackets_and_label() {
        let area = Rect::new(0, 0, 10, 1);
        let theme = Theme::default();
        let mut button = mk_button("Overwrite");
        button.variant = ButtonVariant::Warning;

        let mut buf = Buffer::empty(area);
        draw_button(&mut buf, area, &button, &theme, false);

        assert_eq!(cell_fg(&buf, 0, 0), ratatui_color(theme.warning_fg));
        assert_eq!(cell_fg(&buf, 2, 0), ratatui_color(theme.warning_fg));
    }

    /// A focused `Danger` button must stay visually distinct from an
    /// unfocused one: the bracket chrome switches to `accent_fg` (the
    /// same focus cue `Primary`/`Secondary` already paint) while the
    /// label keeps `error_fg` either way.
    #[test]
    fn focused_danger_button_bracket_differs_from_unfocused_but_label_keeps_intent_colour() {
        let area = Rect::new(0, 0, 10, 1);
        let theme = Theme::default();
        let mut button = mk_button("Delete");
        button.variant = ButtonVariant::Danger;

        let mut unfocused_buf = Buffer::empty(area);
        draw_button(&mut unfocused_buf, area, &button, &theme, false);
        let unfocused_bracket_fg = cell_fg(&unfocused_buf, 0, 0);
        let unfocused_label_fg = cell_fg(&unfocused_buf, 2, 0);

        button.focused = true;
        let mut focused_buf = Buffer::empty(area);
        draw_button(&mut focused_buf, area, &button, &theme, false);
        let focused_bracket_fg = cell_fg(&focused_buf, 0, 0);
        let focused_label_fg = cell_fg(&focused_buf, 2, 0);

        assert_ne!(
            focused_bracket_fg, unfocused_bracket_fg,
            "a focused Danger button's bracket chrome must look different \
             from an unfocused one"
        );
        assert_eq!(
            focused_bracket_fg,
            ratatui_color(theme.accent_fg),
            "focus wins on the bracket chrome ahead of the intent colour"
        );
        assert_eq!(
            unfocused_label_fg,
            ratatui_color(theme.error_fg),
            "the label keeps the intent colour while unfocused"
        );
        assert_eq!(
            focused_label_fg,
            ratatui_color(theme.error_fg),
            "the label keeps the intent colour while focused too"
        );
    }

    #[test]
    fn disabled_danger_button_paints_muted_fg_not_the_intent_colour() {
        let area = Rect::new(0, 0, 10, 1);
        let theme = Theme::default();
        let mut button = mk_button("Delete");
        button.variant = ButtonVariant::Danger;
        button.disabled = true;

        let mut buf = Buffer::empty(area);
        draw_button(&mut buf, area, &button, &theme, false);

        assert_eq!(cell_fg(&buf, 2, 0), ratatui_color(theme.muted_fg));
    }

    /// `[ ⚠ Delete ]`: a Danger button with a leading icon, painted with
    /// Nerd Fonts on (glyph) and off (fallback).
    #[test]
    fn danger_button_with_icon_paints_glyph_or_fallback_per_nerd_fonts_flag() {
        let area = Rect::new(0, 0, 14, 1);
        let theme = Theme::default();
        let mut button = mk_button("Delete");
        button.variant = ButtonVariant::Danger;
        button.icon = Some(crate::types::Icon::new("\u{f071}", "!"));

        let row_text = |nerd_fonts_enabled: bool| -> String {
            let mut buf = Buffer::empty(area);
            draw_button(&mut buf, area, &button, &theme, nerd_fonts_enabled);
            (area.x..area.x + area.width)
                .map(|x| cell_char(&buf, x, area.y))
                .collect()
        };

        let with_nerd_fonts = row_text(true);
        assert!(
            with_nerd_fonts.contains('\u{f071}'),
            "nerd_fonts_enabled: true should paint the glyph half of the icon, got {with_nerd_fonts:?}"
        );
        assert!(with_nerd_fonts.contains("Delete"));

        let without_nerd_fonts = row_text(false);
        assert!(
            without_nerd_fonts.contains('!'),
            "nerd_fonts_enabled: false should paint the fallback half of the icon, got {without_nerd_fonts:?}"
        );
        assert!(!without_nerd_fonts.contains('\u{f071}'));
    }

    /// A huge/non-finite `icon_gap` must neither panic nor allocate
    /// anywhere near its raw size — the unit-level clamp is covered
    /// directly by `crate::primitives::button::cell_paint`'s own tests;
    /// this is the black-box "painting one doesn't blow up" check.
    #[test]
    fn huge_icon_gap_is_clamped_instead_of_overflowing_or_panicking() {
        let area = Rect::new(0, 0, 14, 1);
        let theme = Theme::default();
        let mut button = mk_button("Save");
        button.icon = Some(crate::types::Icon::new("S", "S"));
        button.icon_gap = f32::MAX;

        let mut buf = Buffer::empty(area);
        draw_button(&mut buf, area, &button, &theme, true);
    }

    #[test]
    fn trailing_icon_paints_after_the_label() {
        let area = Rect::new(0, 0, 14, 1);
        let theme = Theme::default();
        let mut button = mk_button("Save");
        button.icon = Some(crate::types::Icon::new("S", "S"));
        button.icon_position = ButtonIconPosition::Trailing;

        let mut buf = Buffer::empty(area);
        draw_button(&mut buf, area, &button, &theme, true);
        let row: String = (area.x..area.x + area.width)
            .map(|x| cell_char(&buf, x, area.y))
            .collect();

        let save_idx = row.find("Save").expect("label must paint");
        let icon_idx = row.rfind('S').expect("icon glyph must paint");
        assert!(
            icon_idx > save_idx,
            "a Trailing icon must paint after the label, got {row:?}"
        );
    }

    #[test]
    fn no_icon_paints_the_label_alone() {
        let area = Rect::new(0, 0, 8, 1);
        let theme = Theme::default();
        let button = mk_button("OK");
        assert_eq!(button.icon, None);

        let mut buf = Buffer::empty(area);
        draw_button(&mut buf, area, &button, &theme, true);
        assert_eq!(cell_char(&buf, 0, 0), '[');
        assert_eq!(cell_char(&buf, 2, 0), 'O');
    }
}
