//! TUI rasteriser for [`crate::Button`].
//!
//! Paints `[ Label ]` bracket chrome for [`ButtonVariant::Primary`] /
//! [`ButtonVariant::Secondary`] / [`ButtonVariant::Danger`] /
//! [`ButtonVariant::Warning`], and the bare label for
//! [`ButtonVariant::Icon`] (no bracket chrome — an icon button typically
//! carries a single glyph). The whole cell range is one hit target, same
//! as [`crate::Button::layout`].
//!
//! [`ButtonVariant::Danger`]/[`ButtonVariant::Warning`] paint **no**
//! background fill on TUI — unlike [`ButtonVariant::Primary`]'s solid
//! `accent_bg`, intent here is carried entirely by the foreground colour
//! ([`Theme::error_fg`]/[`Theme::warning_fg`]) on both the bracket chrome
//! and the label text, e.g. `[ ⚠ Delete ]`. [`Button::icon`] paints
//! [`crate::Icon::glyph`] when the backend's Nerd Fonts flag is on and
//! [`crate::Icon::fallback`] otherwise, same convention as
//! [`crate::tui::activity_bar::draw_activity_bar`].
//!
//! A focused button's bracket chrome always paints [`Theme::accent_fg`],
//! the same cue [`crate::tui::switch`]/[`crate::tui::radio_group`] paint
//! for their own focus state — this wins over the intent colour on the
//! brackets so a focused `Danger`/`Warning` button stays visibly
//! distinct from an unfocused one. The label text keeps its intent
//! colour either way.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{ratatui_color, set_cell};
use crate::primitives::button::{Button, ButtonIconPosition, ButtonLayout, ButtonVariant};
use crate::primitives::layout_metrics::pixel::NOMINAL_CELL_PX_WIDTH;
use crate::theme::Theme;

/// Quantise [`Button::icon_gap`] (a surface-native pixel count) to whole
/// TUI cells, floored at one cell — see that field's own "TUI story" doc
/// for why a zero-cell gap isn't honoured. Clamped at `max_cells` (the
/// button's own paint width) so an unvalidated, arbitrarily large
/// `icon_gap` (a stray `f32::MAX`/`1e9`) can neither overflow the
/// `as usize` cast nor make `" ".repeat(..)` allocate past what the
/// paint loop could ever use.
fn icon_gap_cells(icon_gap: f32, max_cells: usize) -> usize {
    if !icon_gap.is_finite() {
        return max_cells.max(1);
    }
    ((icon_gap / NOMINAL_CELL_PX_WIDTH).ceil() as usize).clamp(1, max_cells.max(1))
}

/// Compute the TUI cell-unit layout for a [`Button`] without painting.
/// The control has no internal sub-regions, so `area` maps straight
/// through to [`Button::layout`]'s bounds.
pub fn tui_button_layout(button: &Button, area: Rect) -> ButtonLayout {
    button.layout(crate::event::Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    ))
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
    let layout = tui_button_layout(button, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let y = layout.bounds.y.round() as u16;
    let x0 = layout.bounds.x.round() as u16;
    let w = layout.bounds.width.round() as u16;
    let end = x0.saturating_add(w);

    // `Danger`/`Warning` intent is carried entirely by foreground colour
    // on TUI — no background fill of their own, unlike `Primary`'s solid
    // `accent_bg` — see the module doc.
    let intent_fg = match button.variant {
        ButtonVariant::Danger => Some(theme.error_fg),
        ButtonVariant::Warning => Some(theme.warning_fg),
        ButtonVariant::Primary | ButtonVariant::Secondary | ButtonVariant::Icon => None,
    };

    let bg = if button.disabled {
        theme.surface_bg
    } else {
        match button.variant {
            ButtonVariant::Primary => theme.accent_bg,
            ButtonVariant::Secondary | ButtonVariant::Danger | ButtonVariant::Warning => {
                theme.surface_bg
            }
            ButtonVariant::Icon => theme.background,
        }
    };
    let bg = if button.disabled {
        bg
    } else if is_pressed {
        theme.button_pressed_bg(bg)
    } else if is_hovered {
        theme.button_hover_bg(bg)
    } else {
        bg
    };
    let bg = ratatui_color(bg);
    let fg = if button.disabled {
        ratatui_color(theme.muted_fg)
    } else if let Some(intent) = intent_fg {
        ratatui_color(intent)
    } else {
        ratatui_color(theme.foreground)
    };
    // `focused` wins on the bracket chrome ahead of `intent_fg` so a
    // focused `Danger`/`Warning` button stays visually distinct from an
    // unfocused one — same `accent_fg` focus cue `Primary`/`Secondary`
    // already paint here, and `tui/switch.rs`/`tui/radio_group.rs` paint
    // elsewhere. The label (`fg`, above) keeps the intent colour either
    // way, so a focused Danger button still reads as `[ Delete ]` in
    // `error_fg` text inside an `accent_fg` bracket, not a plain button.
    let brk_fg = if button.disabled {
        ratatui_color(theme.muted_fg)
    } else if button.focused {
        ratatui_color(theme.accent_fg)
    } else if let Some(intent) = intent_fg {
        ratatui_color(intent)
    } else {
        ratatui_color(theme.border_fg)
    };

    let is_icon = matches!(button.variant, ButtonVariant::Icon);
    let icon_ch: Option<char> = button.icon.as_ref().map(|icon| {
        icon.resolve(nerd_fonts_enabled)
            .chars()
            .next()
            .unwrap_or(' ')
    });
    let inner = match icon_ch {
        None => button.label.clone(),
        Some(ch) => {
            let gap = " ".repeat(icon_gap_cells(button.icon_gap, w as usize));
            match button.icon_position {
                ButtonIconPosition::Leading => format!("{ch}{gap}{}", button.label),
                ButtonIconPosition::Trailing => format!("{}{gap}{ch}", button.label),
            }
        }
    };
    let text: String = if is_icon {
        inner
    } else {
        format!("[ {inner} ]")
    };
    let last_idx = text.chars().count().saturating_sub(1);

    let mut col = x0;
    for (i, ch) in text.chars().enumerate() {
        if col >= end {
            break;
        }
        let is_bracket = !is_icon && (i == 0 || i == last_idx);
        let ch_fg = if is_bracket { brk_fg } else { fg };
        set_cell(buf, col, y, ch, ch_fg, bg);
        col += 1;
    }
    while col < end {
        set_cell(buf, col, y, ' ', fg, bg);
        col += 1;
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::button::ButtonHit;
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

    /// `icon_gap_cells` must clamp against the button's own paint width
    /// rather than growing the gap string without bound — an
    /// unvalidated `f32::MAX`/non-finite `icon_gap` must neither panic
    /// nor allocate anywhere near that size.
    #[test]
    fn huge_icon_gap_is_clamped_instead_of_overflowing_or_panicking() {
        let area = Rect::new(0, 0, 14, 1);
        let theme = Theme::default();
        let mut button = mk_button("Save");
        button.icon = Some(crate::types::Icon::new("S", "S"));
        button.icon_gap = f32::MAX;

        let mut buf = Buffer::empty(area);
        draw_button(&mut buf, area, &button, &theme, true);

        assert_eq!(icon_gap_cells(f32::MAX, 10), 10);
        assert_eq!(icon_gap_cells(f32::INFINITY, 10), 10);
        assert_eq!(icon_gap_cells(f32::NAN, 10), 10);
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
