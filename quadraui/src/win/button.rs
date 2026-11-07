//! Direct2D / DirectWrite rasteriser for [`crate::Button`].
//!
//! Painting is delegated to the shared
//! [`crate::primitives::button::native_surface_paint::paint`] function — see
//! that fn's doc for details. The `Backend`-trait entry point
//! [`super::backend::WinBackend::draw_button`] calls the shared `paint`
//! directly with `self.current_theme`, ensuring the live theme is used
//! for all painting. This module carries only [`win_button_layout`], which
//! provides backend-specific geometry calculations without performing
//! painting.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod button;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.

use crate::event::Rect;
use crate::primitives::button::{Button, ButtonLayout};

/// Compute a [`Button`]'s layout without painting — the twin of
/// `crate::Backend::draw_button`'s paint path. A `Button` has no
/// internal sub-regions, so this maps straight through to
/// [`Button::layout`], the same way `win_progress_layout` wraps
/// [`crate::primitives::progress::ProgressBar`]'s layout.
pub fn win_button_layout(rect: Rect, button: &Button) -> ButtonLayout {
    button.layout(rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Viewport;
    use crate::interaction::InteractionState;
    use crate::primitives::button::{ButtonEvent, ButtonHit, ButtonIconPosition, ButtonVariant};
    use crate::theme::Theme;
    use crate::types::{Icon, WidgetId};
    use crate::win::backend::WinBackend;
    use crate::win::testing::HeadlessSurface;
    use crate::win::text::DWrite;
    use crate::Backend;

    const W: u32 = 120;
    const H: u32 = 20;

    /// Pixel offset from the left edge, at mid-height, that always
    /// lands on the fill rather than the centred label/icon text:
    /// `Style::default().padding == 8.0` is the inset
    /// `native_surface_paint::paint` never draws label/icon content
    /// inside of, and `6 < 8` sits inside that padding band — see
    /// `crate::macos::button`'s matching probe for the same reasoning
    /// on the other GUI backend that shares this paint helper.
    const FILL_PROBE_X: u32 = 6;

    fn mk_button(label: &str) -> Button {
        Button::new(WidgetId::new("dialog:ok"), label)
    }

    /// Paint `button` via the shared
    /// [`crate::primitives::button::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `surface`'s
    /// headless target — the same adapter `WinBackend::draw_button`
    /// uses, exercised here directly.
    fn paint(
        surface: &HeadlessSurface,
        dwrite: &DWrite,
        rect: Rect,
        button: &Button,
        theme: &Theme,
    ) -> ButtonLayout {
        surface
            .paint(|target| {
                let mut raw = super::super::surface::D2dSurface {
                    target,
                    dwrite: Some(dwrite),
                };
                crate::primitives::button::native_surface_paint::paint(
                    button,
                    &mut raw,
                    theme,
                    rect,
                    &crate::style::Style::default(),
                    false,
                    false,
                );
            })
            .map(|_| win_button_layout(rect, button))
            .expect("paint button")
    }

    #[test]
    fn primary_variant_fills_accent_bg() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let button = mk_button("OK");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &button, &theme);

        let px = surface.pixel_at(FILL_PROBE_X, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b)
        );
    }

    #[test]
    fn secondary_variant_fills_surface_bg() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut button = mk_button("Cancel");
        button.variant = ButtonVariant::Secondary;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &button, &theme);

        let px = surface.pixel_at(FILL_PROBE_X, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b)
        );
    }

    #[test]
    fn disabled_button_fills_surface_bg_regardless_of_variant() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let mut button = mk_button("OK");
        button.disabled = true;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        paint(&surface, &dwrite, rect, &button, &theme);

        let px = surface.pixel_at(FILL_PROBE_X, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b)
        );
    }

    /// Regression guard at a non-zero origin — layout helpers must
    /// return coords in the same frame across backends.
    fn hit_test_round_trip_at(x: f32, y: f32) {
        let rect = Rect::new(x, y, 40.0, 16.0);
        let layout = win_button_layout(rect, &mk_button("OK"));
        let inside = layout.hit_test(crate::event::Point::new(x + 4.0, y + 4.0));
        assert_eq!(inside, ButtonHit::Body);
        let outside = layout.hit_test(crate::event::Point::new(x + 400.0, y));
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
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let button = mk_button("OK");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let theme = Theme::default();

        let layout = paint(&surface, &dwrite, rect, &button, &theme);
        assert_eq!(
            layout.hit_test(crate::event::Point::new(2.0, 2.0)),
            ButtonHit::Body
        );
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

    /// `win_button_layout` must produce byte-identical layout to the painted result.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let button = mk_button("OK");
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let theme = Theme::default();
        let painted = paint(&surface, &dwrite, rect, &button, &theme);
        let no_paint = win_button_layout(rect, &button);

        assert_eq!(painted, no_paint);
    }

    /// Paint via the real `Backend::draw_button` path (a live `WinBackend`
    /// attached to a headless Direct2D target), so the dispatch
    /// [`super::super::backend::WinBackend::draw_button`] does gets
    /// exercised too, not just the shared `native_surface_paint::paint`
    /// helper — mirrors `crate::macos::button`'s `paint_via_backend`.
    fn paint_via_backend(button: &Button) -> (HeadlessSurface, ButtonLayout) {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = backend.draw_button(Rect::new(0.0, 0.0, W as f32, H as f32), button);
        backend.end_frame();
        (surface, layout)
    }

    /// Paint via `Backend::draw_button_with_state`, so `interaction`'s
    /// resolved hover/pressed state actually reaches the shared
    /// `native_surface_paint::paint` — the real path a live click router
    /// drives.
    fn paint_with_state(button: &Button, interaction: &InteractionState) -> HeadlessSurface {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), W, H)
            .expect("attach headless surface");
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.draw_button_with_state(
            Rect::new(0.0, 0.0, W as f32, H as f32),
            button,
            interaction,
        );
        backend.end_frame();
        surface
    }

    /// Paint via `Backend::draw_button` with text-run recording on, so
    /// the icon/label glyph runs' own paint `x` positions are observable
    /// without reading raw pixels.
    fn paint_recording_runs_at(button: &Button, rect_width: f32) -> Vec<crate::testing::TextRun> {
        let surface = HeadlessSurface::new(rect_width.ceil() as u32, H).expect("create surface");
        let mut backend = WinBackend::new();
        backend
            .attach_headless(surface.target().clone(), rect_width.ceil() as u32, H)
            .expect("attach headless surface");
        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(rect_width, H as f32, 1.0));
        backend.draw_button(Rect::new(0.0, 0.0, rect_width, H as f32), button);
        backend.end_frame();
        backend.text_runs().to_vec()
    }

    /// [`paint_recording_runs_at`] into the fixed `W`-wide button every
    /// other test here uses.
    fn paint_recording_runs(button: &Button) -> Vec<crate::testing::TextRun> {
        paint_recording_runs_at(button, W as f32)
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
        let px = danger_surface.pixel_at(FILL_PROBE_X, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.button_danger_bg().r,
                theme.button_danger_bg().g,
                theme.button_danger_bg().b,
            ),
        );
        assert_ne!(
            danger_surface.pixel_at(FILL_PROBE_X, H / 2),
            primary_surface.pixel_at(FILL_PROBE_X, H / 2),
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
        let px = warning_surface.pixel_at(FILL_PROBE_X, H / 2);
        assert_eq!(
            (px.r, px.g, px.b),
            (
                theme.button_warning_bg().r,
                theme.button_warning_bg().g,
                theme.button_warning_bg().b,
            ),
        );
        assert_ne!(
            warning_surface.pixel_at(FILL_PROBE_X, H / 2),
            primary_surface.pixel_at(FILL_PROBE_X, H / 2),
            "a Warning button must not paint the same fill as Primary",
        );
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
            resting.pixel_at(FILL_PROBE_X, H / 2),
            hovered.pixel_at(FILL_PROBE_X, H / 2),
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
            resting.pixel_at(FILL_PROBE_X, H / 2),
            pressed.pixel_at(FILL_PROBE_X, H / 2),
            "pressing a button must repaint its fill",
        );
        assert_ne!(
            hovered.pixel_at(FILL_PROBE_X, H / 2),
            pressed.pixel_at(FILL_PROBE_X, H / 2),
            "a pressed button must paint Theme::button_pressed_bg, not stay at the hover tint",
        );
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
}
