//! macOS rasteriser for [`crate::Tooltip`].
//!
//! Content painting uses the shared
//! [`crate::primitives::tooltip::native_surface_paint::paint`].
//!
//! [`draw_tooltip`] renders `TooltipChrome::default()`; [`draw_tooltip_with_chrome`]
//! takes the chrome request explicitly.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::tooltip::{native_surface_paint, Tooltip, TooltipChrome, TooltipLayout};
use crate::theme::Theme;

/// Draw a [`Tooltip`] at its resolved layout position with the default
/// chrome — a [`crate::TooltipBorder::Full`] box, no title, i.e. exactly
/// what this rasteriser drew before #541 added a choice.
///
/// `padding_x` is the horizontal padding from the left border to the
/// start of text — consumers typically pass `char_width`.
///
/// To ask for different chrome, call [`draw_tooltip_with_chrome`].
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_tooltip(
    ctx: CGContextRef,
    font: &CTFont,
    tooltip: &Tooltip,
    tooltip_layout: &TooltipLayout,
    line_height: f64,
    padding_x: f64,
    theme: &Theme,
) {
    // SAFETY: forwarded unchanged — `ctx` validity is the caller's
    // contract, documented above and identical for both entry points.
    unsafe {
        draw_tooltip_with_chrome(
            ctx,
            font,
            tooltip,
            tooltip_layout,
            &TooltipChrome::default(),
            line_height,
            padding_x,
            theme,
        );
    }
}

/// Draw a [`Tooltip`] at its resolved layout position, with the border
/// and optional title requested by `chrome`.
///
/// `padding_x` is the horizontal padding from the left border to the
/// start of text — consumers typically pass `char_width`. Halved when
/// `chrome.border` is [`crate::TooltipBorder::None`], since there is no
/// border column to clear first.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_tooltip_with_chrome(
    ctx: CGContextRef,
    font: &CTFont,
    tooltip: &Tooltip,
    tooltip_layout: &TooltipLayout,
    chrome: &TooltipChrome,
    line_height: f64,
    padding_x: f64,
    theme: &Theme,
) {
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    native_surface_paint::paint(
        tooltip,
        tooltip_layout,
        chrome,
        line_height as f32,
        padding_x as f32,
        &mut surface,
        theme,
        &crate::style::Style::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::tooltip::{
        ResolvedPlacement, Tooltip, TooltipBorder, TooltipLayout, TooltipPlacement,
    };
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 200;
    const H: u32 = 60;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_tooltip() -> Tooltip {
        Tooltip {
            id: WidgetId::new("tip"),
            text: "Hover hint".into(),
            styled_lines: None,
            placement: TooltipPlacement::Bottom,
            fg: None,
            bg: None,
        }
    }

    fn sample_layout(border: TooltipBorder, title: Option<&str>) -> (TooltipLayout, TooltipChrome) {
        let layout = TooltipLayout {
            bounds: QRect::new(10.0, 10.0, 120.0, 24.0),
            resolved_placement: ResolvedPlacement::Bottom,
        };
        let mut chrome = TooltipChrome::new(border);
        chrome.title = title.map(str::to_string);
        (layout, chrome)
    }

    fn paint(tip: &Tooltip, layout: &TooltipLayout, chrome: &TooltipChrome) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_tooltip_with_chrome(tip, layout, chrome);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn tooltip_paints_hover_bg() {
        let tip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::default(), None);
        let surface = paint(&tip, &layout, &chrome);
        let theme = Theme::default();
        // Probe near right edge of bounds — glyph-free zone.
        let bx = layout.bounds.x as u32;
        let by = layout.bounds.y as u32;
        let bw = layout.bounds.width as u32;
        let bh = layout.bounds.height as u32;
        let (r, g, b, _) = surface.pixel(bx + bw - 4, by + bh / 2);
        assert_eq!(
            (r, g, b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
        );
    }

    #[test]
    fn tooltip_border_paints_at_edge() {
        let tip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::default(), None);
        let surface = paint(&tip, &layout, &chrome);
        let theme = Theme::default();
        // The 1pt border stroke centres on the rect's top edge, so
        // the edge pixel is anti-aliased ~50/50 between border ink
        // and tooltip bg. Verify the edge pixel differs from the
        // pure bg fill (probed at +2 px below the top edge, well
        // inside the bg region).
        // Probe near the right edge, away from "Hover hint" glyphs.
        let bx = layout.bounds.x as u32;
        let by = layout.bounds.y as u32;
        let bw = layout.bounds.width as u32;
        let (edge_r, edge_g, edge_b, _) = surface.pixel(bx + bw - 4, by);
        let (inner_r, inner_g, inner_b, _) = surface.pixel(bx + bw - 4, by + 4);
        assert_eq!(
            (inner_r, inner_g, inner_b),
            (theme.hover_bg.r, theme.hover_bg.g, theme.hover_bg.b),
            "inner pixel should be pure bg",
        );
        assert_ne!(
            (edge_r, edge_g, edge_b),
            (inner_r, inner_g, inner_b),
            "edge pixel should differ from bg (border ink present)",
        );
    }

    #[test]
    fn tooltip_with_custom_bg_overrides_theme() {
        let mut tip = sample_tooltip();
        tip.bg = Some(crate::types::Color::rgb(50, 100, 150));
        let (layout, chrome) = sample_layout(TooltipBorder::default(), None);
        let surface = paint(&tip, &layout, &chrome);
        let bx = layout.bounds.x as u32;
        let by = layout.bounds.y as u32;
        let bw = layout.bounds.width as u32;
        let bh = layout.bounds.height as u32;
        let (r, g, b, _) = surface.pixel(bx + bw - 4, by + bh / 2);
        assert_eq!((r, g, b), (50, 100, 150));
    }

    #[test]
    fn empty_bounds_no_op() {
        let tip = sample_tooltip();
        let layout = TooltipLayout {
            bounds: QRect::new(10.0, 10.0, 0.0, 0.0),
            resolved_placement: ResolvedPlacement::Bottom,
        };
        let surface = paint(&tip, &layout, &TooltipChrome::default());
        // Surface stays all-zero.
        let (r, g, b, _) = surface.pixel(10, 10);
        assert_eq!((r, g, b), (0, 0, 0));
    }

    // Explicit border vocabulary tests, matching `gtk::tooltip::tests`' coverage.

    /// A pure-background reference pixel: bottom-right corner, a few
    /// pixels in from both edges — clear of any border stroke (which
    /// hugs the very edge) and of `sample_tooltip`'s short, top-aligned
    /// "Hover hint" body text.
    fn bg_reference(surface: &BitmapSurface, bounds: QRect) -> (u8, u8, u8) {
        let (r, g, b, _) = surface.pixel(
            bounds.x as u32 + bounds.width as u32 - 4,
            bounds.y as u32 + bounds.height as u32 - 4,
        );
        (r, g, b)
    }

    #[test]
    fn sides_border_only_strokes_left_and_right() {
        let tip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::Sides, None);
        let surface = paint(&tip, &layout, &chrome);
        let b = layout.bounds;
        let (bx, by, bw, bh) = (b.x as u32, b.y as u32, b.width as u32, b.height as u32);

        let inner = bg_reference(&surface, b);
        let (top_r, top_g, top_b, _) = surface.pixel(bx + bw / 2, by);
        let (bottom_r, bottom_g, bottom_b, _) = surface.pixel(bx + bw / 2, by + bh - 1);
        let (left_r, left_g, left_b, _) = surface.pixel(bx, by + bh / 2);
        let (right_r, right_g, right_b, _) = surface.pixel(bx + bw - 1, by + bh / 2);

        assert_eq!(
            (top_r, top_g, top_b),
            inner,
            "TooltipBorder::Sides must not stroke the top edge — no top/bottom rule, \
             regardless of box height"
        );
        assert_eq!(
            (bottom_r, bottom_g, bottom_b),
            inner,
            "TooltipBorder::Sides must not stroke the bottom edge"
        );
        assert_ne!(
            (left_r, left_g, left_b),
            inner,
            "TooltipBorder::Sides must still stroke the left edge"
        );
        assert_ne!(
            (right_r, right_g, right_b),
            inner,
            "TooltipBorder::Sides must still stroke the right edge"
        );
    }

    #[test]
    fn none_border_strokes_nothing() {
        let tip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::None, None);
        let surface = paint(&tip, &layout, &chrome);
        let b = layout.bounds;
        let (bx, by, bw, bh) = (b.x as u32, b.y as u32, b.width as u32, b.height as u32);

        let inner = bg_reference(&surface, b);
        for (name, (r, g, bl, _)) in [
            ("top", surface.pixel(bx + bw / 2, by)),
            ("bottom", surface.pixel(bx + bw / 2, by + bh - 1)),
            ("left", surface.pixel(bx, by + bh / 2)),
            ("right", surface.pixel(bx + bw - 1, by + bh / 2)),
        ] {
            assert_eq!(
                (r, g, bl),
                inner,
                "TooltipBorder::None must not stroke {name} — no chrome at all"
            );
        }
    }

    /// A title punches a background-coloured gap through the top stroke so it
    /// reads as embedded in the border, not a content row. Scans the top edge
    /// rather than probing one fixed x — the title is horizontally centred, so a
    /// single sample can land on a glyph's ink instead of the cleared pad around it.
    #[test]
    fn full_border_title_punches_a_gap_but_leaves_the_rest_of_the_top_edge_stroked() {
        let tip = sample_tooltip();
        let (layout, chrome) = sample_layout(TooltipBorder::Full, Some("Hi"));
        let surface = paint(&tip, &layout, &chrome);
        let b = layout.bounds;
        let (bx, by, bw) = (b.x as u32, b.y as u32, b.width as u32);

        let inner = bg_reference(&surface, b);
        let margin = 3u32;
        let interior: Vec<(u8, u8, u8)> = (bx + margin..bx + bw - margin)
            .map(|x| {
                let (r, g, bl, _) = surface.pixel(x, by);
                (r, g, bl)
            })
            .collect();

        assert!(
            interior.contains(&inner),
            "the title's short label + padding should punch at least one background-\
             coloured pixel into the top edge somewhere in its interior (inner={inner:?}, \
             top-edge interior samples={interior:?})"
        );

        let (corner_r, corner_g, corner_b, _) = surface.pixel(bx + 1, by);
        assert_ne!(
            (corner_r, corner_g, corner_b),
            inner,
            "the top edge right at the corner should still show the plain border stroke"
        );
    }

    /// Regression test: callers following the `TooltipMeasure` contract
    /// (see that type's doc) size `Sides`/`None` tooltips to exactly one
    /// content row — no border row to spare, unlike `Full`'s 3 — so
    /// `paint`'s text placement must not assume a top border those two
    /// variants never draw. Before that placement accounted for the
    /// border mode, a one-row `Sides`/`None` box failed the
    /// `row_y + line_height > bounds.y + bounds.height` guard and drew no
    /// text at all, i.e. an empty box on every pixel backend.
    #[test]
    fn text_paints_inside_bounds_for_every_border_at_contract_height() {
        let mut probe = MacBackend::new();
        probe.set_current_font(font());
        let line_h = probe.line_height();

        for (border, rows) in [
            (TooltipBorder::Sides, 1.0),
            (TooltipBorder::Full, 3.0),
            (TooltipBorder::None, 1.0),
        ] {
            let tip = sample_tooltip();
            let layout = TooltipLayout {
                bounds: QRect::new(10.0, 10.0, 120.0, line_h * rows),
                resolved_placement: ResolvedPlacement::Bottom,
            };
            let chrome = TooltipChrome::new(border);
            let surface = paint(&tip, &layout, &chrome);
            let b = layout.bounds;
            let bg = bg_reference(&surface, b);

            let has_ink = (b.y as u32..(b.y + b.height) as u32).any(|y| {
                (b.x as u32..(b.x + b.width) as u32).any(|x| {
                    (
                        surface.pixel(x, y).0,
                        surface.pixel(x, y).1,
                        surface.pixel(x, y).2,
                    ) != bg
                })
            });
            assert!(
                has_ink,
                "{border:?}: expected glyph ink somewhere inside a {rows}-row-tall box \
                 sized per the `TooltipMeasure` contract, found none"
            );
        }
    }

    #[test]
    fn title_is_ignored_when_border_is_sides_or_none() {
        for border in [TooltipBorder::Sides, TooltipBorder::None] {
            let with_title = sample_tooltip();
            let without_title = sample_tooltip();
            let (with_layout, with_chrome) = sample_layout(border, Some("Ignored"));
            let (without_layout, without_chrome) = sample_layout(border, None);
            let with_surface = paint(&with_title, &with_layout, &with_chrome);
            let without_surface = paint(&without_title, &without_layout, &without_chrome);
            let b = with_layout.bounds;
            let (bx, by, bw) = (b.x as u32, b.y as u32, b.width as u32);
            let top_with = with_surface.pixel(bx + bw / 2, by);
            let top_without = without_surface.pixel(bx + bw / 2, by);
            assert_eq!(
                top_with, top_without,
                "{border:?}: setting `title` must not change what's painted at the top edge"
            );
        }
    }

    // ── hover-driven tooltip, end to end through real pointer events ────

    /// Drives [`crate::compose::HoverTooltipTracker`] through real
    /// [`UiEvent::MouseMoved`] dispatch + [`super::super::testing::MacDriver::tick`]
    /// — the same `dispatch_event`/`AppLogic::tick` path the live
    /// `macos::run` runner uses for its `request_frame_in`-scheduled
    /// repaint (see [`MacDriver::tick`]'s own doc for why the headless
    /// driver needs this seam instead of a real `NSTimer`). Not a test of
    /// `HoverTooltipTracker` itself (that lives in its own module) — this
    /// proves the macOS dispatch + tick + `draw_tooltip` wiring actually
    /// shows and hides a tooltip in response to pointer movement.
    mod hover_driven {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        use super::super::super::testing::MacDriver;
        use super::*;
        use crate::compose::HoverTooltipTracker;
        use crate::runner::Reaction;
        use crate::{AppLogic, ButtonMask, Point, UiEvent};

        const DELAY_MS: u64 = 500;

        /// Anchor rect the tooltip is shown for — matches both the
        /// hover hit-test target and the tooltip's own bounds, so a
        /// `MouseMoved` anywhere inside it hovers the one tracked item.
        fn anchor() -> QRect {
            QRect::new(10.0, 10.0, 60.0, 20.0)
        }

        /// Minimal [`AppLogic`] wiring a single-widget
        /// [`HoverTooltipTracker`] to real pointer input: `handle` feeds
        /// `MouseMoved`'s position through a plain [`QRect::contains`]
        /// hit-test, `tick` advances the delay, `render` paints the
        /// tooltip only while [`HoverTooltipTracker::visible_item`] is
        /// `Some`. `clock_ms` is a caller-driven millisecond counter
        /// (the tracker's own "headless-clock contract" — see its module
        /// doc), not a wall clock, so the test controls elapsed time
        /// exactly without sleeping.
        struct HoverApp {
            tracker: RefCell<HoverTooltipTracker<WidgetId>>,
            clock_ms: Rc<Cell<u64>>,
        }

        impl AppLogic for HoverApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                if self.tracker.borrow().visible_item().is_some() {
                    let tooltip = sample_tooltip();
                    let layout = TooltipLayout {
                        bounds: anchor(),
                        resolved_placement: ResolvedPlacement::Bottom,
                    };
                    backend.draw_tooltip(&tooltip, &layout);
                }
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                match event {
                    UiEvent::MouseMoved { position, .. } => {
                        let hovered = anchor().contains(position).then(|| WidgetId::new("w"));
                        let now = self.clock_ms.get();
                        let changed = self.tracker.borrow_mut().on_hover(hovered, now);
                        if hovered.is_some() {
                            backend.request_frame_in(std::time::Duration::from_millis(DELAY_MS));
                        }
                        if changed {
                            Reaction::Redraw
                        } else {
                            Reaction::Continue
                        }
                    }
                    _ => Reaction::Continue,
                }
            }

            fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
                let now = self.clock_ms.get();
                if self.tracker.borrow_mut().tick(DELAY_MS, now) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
        }

        fn hovering_move(driver: &mut MacDriver<HoverApp>, x: f32, y: f32) {
            driver.dispatch(UiEvent::MouseMoved {
                position: Point::new(x, y),
                buttons: ButtonMask::default(),
            });
        }

        #[test]
        fn tooltip_appears_after_the_delay_and_hides_on_pointer_leave() {
            let clock = Rc::new(Cell::new(0u64));
            let app = HoverApp {
                tracker: RefCell::new(HoverTooltipTracker::new()),
                clock_ms: clock.clone(),
            };
            let mut driver = MacDriver::new(app, 100, 50);

            // Pointer lands on the anchor — still pending, no tooltip yet.
            hovering_move(&mut driver, 40.0, 20.0);
            assert!(
                !driver.screen_contains("Hover hint"),
                "the tooltip must not show the instant the pointer lands"
            );
            // `HoverApp::handle` asked the real `MacBackend::request_frame_in`
            // to schedule the redraw this delay needs — not just this test's
            // hand-driven `clock`/`tick()` below. Without this, nothing here
            // would fail if `request_frame_in` stopped being called at all.
            assert_eq!(
                driver.backend().frame_requests(),
                vec![std::time::Duration::from_millis(DELAY_MS)],
                "hovering the anchor must request a tick after the hover delay"
            );

            // The delay elapses with no further pointer movement — the
            // `tick()` the real runner's `request_frame_in` timer would
            // have fired now shows the tooltip.
            clock.set(DELAY_MS);
            driver.tick();
            assert!(
                driver.screen_contains("Hover hint"),
                "the tooltip must show once the hover delay has elapsed"
            );

            // The pointer leaves the anchor entirely — the tooltip must
            // disappear immediately, no further delay.
            hovering_move(&mut driver, 90.0, 40.0);
            assert!(
                !driver.screen_contains("Hover hint"),
                "the tooltip must hide as soon as the pointer leaves its anchor"
            );
        }
    }
}
