//! macOS rasteriser for [`crate::Button`].
//!
//! Painting is handled by the shared
//! [`crate::primitives::button::native_surface_paint::paint`], same as
//! [`crate::macos::progress`]'s `ProgressBar`. This module provides
//! [`mac_button_layout`], the backend-specific geometry calculation.

use crate::primitives::button::{Button, ButtonLayout};

/// Compute the macOS pixel-unit layout for a [`Button`] without
/// painting. A `Button` has no internal sub-regions, so this maps
/// straight through to [`Button::layout`].
pub fn mac_button_layout(button: &Button, x: f64, y: f64, w: f64, h: f64) -> ButtonLayout {
    button.layout(crate::event::Rect::new(
        x as f32, y as f32, w as f32, h as f32,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Point, Rect as QRect, Viewport};
    use crate::interaction::InteractionState;
    use crate::primitives::button::{ButtonEvent, ButtonHit, ButtonIconPosition, ButtonVariant};
    use crate::theme::Theme;
    use crate::types::{Icon, WidgetId};
    use crate::Backend;
    use core_text::font::CTFont;

    const W: u32 = 120;
    const H: u32 = 20;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn mk_button(label: &str) -> Button {
        Button::new(WidgetId::new("dialog:ok"), label)
    }

    /// Paint via the real `Backend::draw_button` path — routes through
    /// the shared `native_surface_paint::paint`.
    fn paint_via_backend(button: &Button) -> (BitmapSurface, ButtonLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_button(QRect::new(0.0, 0.0, W as f32, H as f32), button);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// Pixel offset from the left edge, at mid-height, that always lands
    /// on the fill rather than the centred label/icon text:
    /// `Style::default().padding == 8.0` is the inset
    /// `native_surface_paint::paint` never draws label/icon content
    /// inside of, and `6 < 8` sits inside that padding band — see
    /// `crate::win::button`'s matching probe for the same reasoning on
    /// the other GUI backend that shares this paint helper.
    const FILL_PROBE_X: u32 = 6;

    #[test]
    fn primary_variant_fills_accent_bg() {
        let button = mk_button("OK");
        let (surface, _layout) = paint_via_backend(&button);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(FILL_PROBE_X, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b),
        );
    }

    #[test]
    fn secondary_variant_fills_surface_bg() {
        let mut button = mk_button("Cancel");
        button.variant = ButtonVariant::Secondary;
        let (surface, _layout) = paint_via_backend(&button);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(FILL_PROBE_X, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    #[test]
    fn disabled_button_fills_surface_bg_regardless_of_variant() {
        let mut button = mk_button("OK");
        button.disabled = true;
        let (surface, _layout) = paint_via_backend(&button);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(FILL_PROBE_X, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    /// Regression guard at a non-zero origin — LESSONS.md "Layout
    /// helpers must return coords in the same frame across backends".
    fn hit_test_round_trip_at(origin_x: f64, origin_y: f64) {
        let layout = mac_button_layout(&mk_button("OK"), origin_x, origin_y, 40.0, 16.0);
        let inside = layout.hit_test(Point::new(origin_x as f32 + 4.0, origin_y as f32 + 4.0));
        assert_eq!(inside, ButtonHit::Body);
        let outside = layout.hit_test(Point::new(origin_x as f32 + 400.0, origin_y as f32));
        assert_eq!(outside, ButtonHit::Outside);
    }

    #[test]
    fn hit_test_round_trip() {
        hit_test_round_trip_at(0.0, 0.0);
    }

    #[test]
    fn hit_test_round_trip_at_nonzero_origin() {
        hit_test_round_trip_at(7.0, 13.0);
    }

    #[test]
    fn click_activation_still_works_after_painting() {
        let button = mk_button("OK");
        let (_surface, layout) = paint_via_backend(&button);
        assert_eq!(layout.hit_test(Point::new(2.0, 2.0)), ButtonHit::Body);
        assert_eq!(
            button.click(),
            Some(ButtonEvent::Activated { id: button.id })
        );
    }

    #[test]
    fn keyboard_activation_requires_focus() {
        let mut button = mk_button("OK");
        button.focused = true;
        let event = button.handle_key(
            &crate::event::Key::Named(crate::event::NamedKey::Enter),
            &crate::types::Modifiers::default(),
        );
        assert_eq!(event, Some(ButtonEvent::Activated { id: button.id }));
    }

    /// `ButtonVariant::Danger`/`Warning` paint through
    /// `Theme::button_danger_bg`/`button_warning_bg`, a solid fill
    /// distinct from `Primary`'s `accent_bg` — same intent colouring
    /// `crate::tui::button` carries through foreground only.
    #[test]
    fn danger_variant_fill_differs_from_primary() {
        let primary = mk_button("OK");
        let (primary_surface, _) = paint_via_backend(&primary);
        let mut danger = mk_button("Delete");
        danger.variant = ButtonVariant::Danger;
        let (danger_surface, _) = paint_via_backend(&danger);

        let theme = Theme::default();
        let (r, g, b, _) = danger_surface.pixel(FILL_PROBE_X, H / 2);
        assert_eq!(
            (r, g, b),
            (
                theme.button_danger_bg().r,
                theme.button_danger_bg().g,
                theme.button_danger_bg().b,
            ),
        );
        assert_ne!(
            danger_surface.pixel(FILL_PROBE_X, H / 2),
            primary_surface.pixel(FILL_PROBE_X, H / 2),
            "a Danger button must not paint the same fill as Primary",
        );
    }

    #[test]
    fn warning_variant_fill_differs_from_primary() {
        let primary = mk_button("OK");
        let (primary_surface, _) = paint_via_backend(&primary);
        let mut warning = mk_button("Overwrite");
        warning.variant = ButtonVariant::Warning;
        let (warning_surface, _) = paint_via_backend(&warning);

        let theme = Theme::default();
        let (r, g, b, _) = warning_surface.pixel(FILL_PROBE_X, H / 2);
        assert_eq!(
            (r, g, b),
            (
                theme.button_warning_bg().r,
                theme.button_warning_bg().g,
                theme.button_warning_bg().b,
            ),
        );
        assert_ne!(
            warning_surface.pixel(FILL_PROBE_X, H / 2),
            primary_surface.pixel(FILL_PROBE_X, H / 2),
            "a Warning button must not paint the same fill as Primary",
        );
    }

    /// Paint via `Backend::draw_button_with_state`, so `interaction`'s
    /// resolved hover/pressed state actually reaches the shared
    /// `native_surface_paint::paint` — the real path
    /// `examples/common/controls.rs`'s `ControlsApp` drives.
    fn paint_with_state(button: &Button, interaction: &InteractionState) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_button_with_state(
                QRect::new(0.0, 0.0, W as f32, H as f32),
                button,
                interaction,
            );
        });
        backend.end_frame();
        surface
    }

    /// `Backend::draw_button_with_state` tints the fill via
    /// `Theme::button_hover_bg` while `InteractionState` reports the
    /// button hovered — the pointer-enter story this issue's acceptance
    /// names explicitly.
    #[test]
    fn hover_state_changes_the_fill_colour() {
        let button = mk_button("OK");
        let resting = paint_with_state(&button, &InteractionState::new());
        let hovered = paint_with_state(
            &button,
            &InteractionState::from_parts(Some(button.id), None),
        );

        assert_ne!(
            resting.pixel(FILL_PROBE_X, H / 2),
            hovered.pixel(FILL_PROBE_X, H / 2),
            "hovering a button must repaint its fill",
        );
    }

    /// `Backend::draw_button_with_state` tints the fill via
    /// `Theme::button_pressed_bg` while `InteractionState` reports the
    /// button pressed, and pressed wins over hover when both are
    /// reported for the same id — the same priority
    /// `native_surface_paint::paint`'s own doc states.
    #[test]
    fn pressed_state_wins_over_hover_and_changes_the_fill_colour() {
        let button = mk_button("OK");
        let resting = paint_with_state(&button, &InteractionState::new());
        let hovered = paint_with_state(
            &button,
            &InteractionState::from_parts(Some(button.id), None),
        );
        let pressed = paint_with_state(
            &button,
            &InteractionState::from_parts(Some(button.id), Some(button.id)),
        );

        assert_ne!(
            resting.pixel(FILL_PROBE_X, H / 2),
            pressed.pixel(FILL_PROBE_X, H / 2),
            "pressing a button must repaint its fill",
        );
        assert_ne!(
            hovered.pixel(FILL_PROBE_X, H / 2),
            pressed.pixel(FILL_PROBE_X, H / 2),
            "a pressed button must paint Theme::button_pressed_bg, not stay at the hover tint",
        );
    }

    /// Paint via the real `Backend::draw_button` path with text-run
    /// recording on, so the icon/label glyph runs' own paint `x`
    /// positions are observable without reading raw pixels, into a
    /// `rect_width`-wide button (`H` tall) rather than the fixed `W`
    /// every other test here uses — lets a narrow-button test exercise
    /// the `.max(style.padding)` clamp in
    /// `native_surface_paint::paint`'s centring arithmetic, which a
    /// `rect_width` as wide as `W` never binds.
    fn paint_recording_runs_at(button: &Button, rect_width: f32) -> Vec<crate::testing::TextRun> {
        let surface = BitmapSurface::new(rect_width.ceil() as u32, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(rect_width, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_button(QRect::new(0.0, 0.0, rect_width, H as f32), button);
        });
        backend.end_frame();
        backend.text_runs().to_vec()
    }

    /// Paint via the real `Backend::draw_button` path with text-run
    /// recording on, into the fixed `W`-wide button every other test
    /// here uses.
    fn paint_recording_runs(button: &Button) -> Vec<crate::testing::TextRun> {
        paint_recording_runs_at(button, W as f32)
    }

    /// A `Leading` icon paints before the label, `icon_gap` clear of it —
    /// the ordering and spacing this issue's acceptance names explicitly.
    #[test]
    fn leading_icon_paints_before_the_label_icon_gap_apart() {
        let mut button = mk_button("Save");
        let icon = Icon::codicon("save", "S").expect("\"save\" is a real codicon name");
        let icon_glyph = icon.glyph.clone();
        button.icon = Some(icon);
        button.icon_position = ButtonIconPosition::Leading;
        button.icon_gap = 10.0;

        let runs = paint_recording_runs(&button);
        let icon_run = runs
            .iter()
            .find(|r| r.text == icon_glyph)
            .expect("icon glyph must paint a text run");
        let label_run = runs
            .iter()
            .find(|r| r.text == "Save")
            .expect("label must paint a text run");

        assert!(
            icon_run.bounds.x < label_run.bounds.x,
            "a Leading icon must paint before the label: icon.x={}, label.x={}",
            icon_run.bounds.x,
            label_run.bounds.x,
        );
        let gap = label_run.bounds.x - (icon_run.bounds.x + icon_run.bounds.width);
        assert!(
            (gap - button.icon_gap).abs() < 0.5,
            "expected icon_gap ({}) clear of the icon, got {gap}",
            button.icon_gap,
        );
    }

    /// `Trailing` flips the order: the label paints first, the icon
    /// after it.
    #[test]
    fn trailing_icon_paints_after_the_label() {
        let mut button = mk_button("Save");
        let icon = Icon::codicon("save", "S").expect("\"save\" is a real codicon name");
        let icon_glyph = icon.glyph.clone();
        button.icon = Some(icon);
        button.icon_position = ButtonIconPosition::Trailing;

        let runs = paint_recording_runs(&button);
        let icon_run = runs
            .iter()
            .find(|r| r.text == icon_glyph)
            .expect("icon glyph must paint a text run");
        let label_run = runs
            .iter()
            .find(|r| r.text == "Save")
            .expect("label must paint a text run");

        assert!(
            label_run.bounds.x < icon_run.bounds.x,
            "a Trailing icon must paint after the label: label.x={}, icon.x={}",
            label_run.bounds.x,
            icon_run.bounds.x,
        );
    }

    /// The icon+label block is inset from `rect`'s own edge by at least
    /// `Style::default().padding` — `ButtonMetrics::padding_x`'s own
    /// source — not flush against it.
    #[test]
    fn label_is_inset_by_at_least_the_style_padding() {
        let button = mk_button("OK");
        let runs = paint_recording_runs(&button);
        let label_run = runs
            .iter()
            .find(|r| r.text == "OK")
            .expect("label must paint a text run");

        let style = crate::style::Style::default();
        assert!(
            label_run.bounds.x >= style.padding - 0.5,
            "expected the label inset by at least style.padding ({}), got x={}",
            style.padding,
            label_run.bounds.x,
        );
    }

    /// At `W` (120px), plain centring alone already clears
    /// `style.padding` for a two-character label, so the test above
    /// never actually exercises the `.max(style.padding)` clamp in
    /// `native_surface_paint::paint`'s centring arithmetic. This test
    /// narrows the button to just `label_w + style.padding` wide, where
    /// plain centring would put the label at half of `style.padding` —
    /// below the floor — so the assertion only holds if the clamp binds.
    #[test]
    fn label_clamps_to_style_padding_when_plain_centering_would_not() {
        let button = mk_button("OK");
        let style = crate::style::Style::default();

        let wide_runs = paint_recording_runs(&button);
        let label_w = wide_runs
            .iter()
            .find(|r| r.text == "OK")
            .expect("label must paint a text run")
            .bounds
            .width;

        let rect_width = label_w + style.padding;
        let narrow_runs = paint_recording_runs_at(&button, rect_width);
        let label_run = narrow_runs
            .iter()
            .find(|r| r.text == "OK")
            .expect("label must paint a text run");

        assert!(
            label_run.bounds.x >= style.padding - 0.5,
            "expected the clamp to hold the label at style.padding ({}), got x={}",
            style.padding,
            label_run.bounds.x,
        );
    }
}
