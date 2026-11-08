//! `CommandCenter` primitive: a horizontal strip with back/forward nav
//! arrows and a clickable search box. Lives in the menu bar row,
//! centered between the menu labels and any trailing chrome (window
//! controls, etc.).

use crate::event::Rect;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Declarative description of a command center strip.
///
/// Stays a plain, non-`#[non_exhaustive]` struct: a published consumer
/// builds it with an exhaustive struct literal today, so marking it
/// `#[non_exhaustive]` would break that consumer's build. `Default` is
/// still provided for consumers that want the
/// `Default::default()`-plus-field-assignment pattern.
///
/// # Examples
///
/// ```
/// use quadraui::{CommandCenter, CommandCenterHit, CommandCenterMeasure, Rect, WidgetId};
///
/// let mut center = CommandCenter::default();
/// center.id = WidgetId::new("command_center:main");
/// center.back_enabled = true;
/// center.search_label = "my-project".to_string();
///
/// let bounds = Rect::new(0.0, 0.0, 400.0, 1.0);
/// let measure = CommandCenterMeasure::from_char_width(&center.search_label, 8.0, 1.0);
/// let layout = center.layout(bounds, measure);
///
/// let back = layout.back_bounds.expect("back arrow is always reserved");
/// assert_eq!(layout.hit_test(back.x, back.y), CommandCenterHit::Back);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCenter {
    pub id: WidgetId,
    pub back_enabled: bool,
    pub forward_enabled: bool,
    /// Text shown inside the search box (e.g. "🔍 project-name").
    /// Empty string hides the search box entirely.
    #[serde(default)]
    pub search_label: String,
}

impl Default for CommandCenter {
    /// An empty-id command center with both nav arrows disabled and no
    /// search box. Set fields directly afterwards (every field is
    /// `pub`) — see `docs/PRIMITIVE_RULES.md` rule 8.
    fn default() -> Self {
        CommandCenter {
            id: WidgetId::new(""),
            back_enabled: false,
            forward_enabled: false,
            search_label: String::new(),
        }
    }
}

/// Measurement for a `CommandCenter`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandCenterMeasure {
    /// Width of each nav arrow slot.
    pub arrow_width: f32,
    /// Gap between arrows and between arrow group and search box.
    pub gap: f32,
    /// Width of the search box. `0.0` when `search_label` is empty.
    pub search_box_width: f32,
    /// Height of the command center (matches the row).
    pub height: f32,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCenterHit {
    Back,
    Forward,
    SearchBox,
    Bar,
    Outside,
}

/// Fully-resolved command-center layout.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandCenterLayout {
    pub bounds: Rect,
    pub back_bounds: Option<Rect>,
    pub forward_bounds: Option<Rect>,
    pub search_bounds: Option<Rect>,
    pub hit_regions: Vec<(Rect, CommandCenterHit)>,
}

impl CommandCenterLayout {
    /// A layout for an area the rasteriser did not (or could not) paint
    /// into: no hit regions at all, so [`Self::hit_test`] returns
    /// [`CommandCenterHit::Outside`] for every point. Use this instead of
    /// [`CommandCenter::layout`]'s geometric result whenever the paint
    /// loop skips the widget entirely — a `width == 0` / `height == 0`
    /// area, for instance — so the returned layout never describes cells
    /// that were never actually drawn.
    pub fn empty(bounds: Rect) -> Self {
        CommandCenterLayout {
            bounds,
            back_bounds: None,
            forward_bounds: None,
            search_bounds: None,
            hit_regions: Vec::new(),
        }
    }

    pub fn hit_test(&self, x: f32, y: f32) -> CommandCenterHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        CommandCenterHit::Outside
    }
}

impl CommandCenterMeasure {
    /// Standard nav-arrow width (px/DIPs) for every pixel backend that
    /// measures the search box from a plain `char_width` average rather
    /// than a live font layout — see [`Self::from_char_width`]. Matches
    /// the value `gtk_command_center_layout` / `mac_command_center_layout`
    /// use for their own (pixel-exact) measurement, so a command center
    /// looks the same size across backends regardless of which
    /// measurement path produced its layout.
    pub const ARROW_WIDTH_PX: f32 = 24.0;
    /// Gap (px/DIPs) between arrows and between the arrow group and the
    /// search box. See [`Self::ARROW_WIDTH_PX`].
    pub const GAP_PX: f32 = 8.0;
    /// Horizontal padding added on top of the estimated search-label
    /// width, in px/DIPs. See [`Self::from_char_width`].
    const SEARCH_H_PAD_PX: f32 = 16.0;
    /// Minimum search-box width (px/DIPs), regardless of how short the
    /// label is. See [`Self::from_char_width`].
    const SEARCH_MIN_WIDTH_PX: f32 = 280.0;

    /// Build a [`CommandCenterMeasure`] from a plain average `char_width`
    /// rather than a live per-glyph text layout — the shape a backend
    /// needs when it has only a `char_width` number on hand (e.g. a
    /// `*_layout` query made outside a frame, with no live font context
    /// to measure against).
    ///
    /// Search-box width is `search_label.len() as f32 * char_width +
    /// 16.0`, floored at `280.0`, or `0.0` when `search_label` is empty
    /// (hides the search box — see [`CommandCenter::search_label`]'s
    /// doc). This formula is shared by all backends that measure search
    /// width from a plain `char_width` average, so they all compute the
    /// same value for identical inputs.
    ///
    /// Distinct from `gtk_command_center_layout` / `mac_command_center_layout`
    /// (the *pixel-exact* measurement path used when painting, which lays
    /// the label out against a live Pango/Core Text font and reads back
    /// its real width) — this constructor is the cheaper approximation
    /// for callers without one.
    pub fn from_char_width(search_label: &str, char_width: f32, height: f32) -> Self {
        let search_box_width = if search_label.is_empty() {
            0.0
        } else {
            (search_label.len() as f32 * char_width + Self::SEARCH_H_PAD_PX)
                .max(Self::SEARCH_MIN_WIDTH_PX)
        };
        CommandCenterMeasure {
            arrow_width: Self::ARROW_WIDTH_PX,
            gap: Self::GAP_PX,
            search_box_width,
            height,
        }
    }
}

impl CommandCenter {
    /// Compute layout. The entire command center is centered within `bounds`.
    pub fn layout(&self, bounds: Rect, measure: CommandCenterMeasure) -> CommandCenterLayout {
        let content_width = measure.arrow_width * 2.0
            + measure.gap
            + if measure.search_box_width > 0.0 {
                measure.gap + measure.search_box_width
            } else {
                0.0
            };

        let center_x = bounds.x + (bounds.width - content_width).max(0.0) / 2.0;
        let y = bounds.y;
        let h = measure.height;

        let back_rect = Rect::new(center_x, y, measure.arrow_width, h);
        let fwd_rect = Rect::new(
            center_x + measure.arrow_width + measure.gap,
            y,
            measure.arrow_width,
            h,
        );

        let search_rect = if measure.search_box_width > 0.0 {
            Some(Rect::new(
                fwd_rect.x + fwd_rect.width + measure.gap,
                y,
                measure.search_box_width,
                h,
            ))
        } else {
            None
        };

        let mut hit_regions = Vec::new();
        hit_regions.push((back_rect, CommandCenterHit::Back));
        hit_regions.push((fwd_rect, CommandCenterHit::Forward));
        if let Some(sb) = search_rect {
            hit_regions.push((sb, CommandCenterHit::SearchBox));
        }
        hit_regions.push((bounds, CommandCenterHit::Bar));

        CommandCenterLayout {
            bounds,
            back_bounds: Some(back_rect),
            forward_bounds: Some(fwd_rect),
            search_bounds: search_rect,
            hit_regions,
        }
    }
}

// `paint` is the shared [`crate::paint_surface::PaintSurface`]
// implementation of command-center painting (nav chevrons + search
// box). The three per-backend painters disagree on the search box's
// look (GTK: square outline; macOS: rounded fill + icon; Win: square
// inset stroke, no icon); this module's look is the macOS variant —
// VS Code's own title-bar search box, a low-alpha rounded fill +
// border with a leading magnifier glyph — the other two backends
// adopt it in a follow-on issue. `ARROW_WIDTH_PX`/`GAP_PX` come from
// [`CommandCenterMeasure`] itself rather than being redefined here,
// the same constants a follow-on adopter should read off instead of
// declaring its own copy.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{CommandCenter, CommandCenterLayout, CommandCenterMeasure};
    use crate::codicon;
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Corner radius of the search box's rounded rect.
    const SEARCH_CORNER_RADIUS: f32 = 4.0;
    /// Alpha (`0.0..=1.0`) the search box's fill uses, applied to
    /// [`Theme::separator`]. Low enough that the box reads as a faint
    /// tint of the bar background rather than a distinct form control —
    /// VS Code's own title-bar search box has the same barely-there fill.
    const SEARCH_FILL_ALPHA: f64 = 22.0 / 255.0;
    /// Alpha (`0.0..=1.0`) the search box's border uses, applied to
    /// [`Theme::separator`]. Thin and low-contrast rather than the bar's
    /// full-opacity separator colour.
    const SEARCH_BORDER_ALPHA: f64 = 130.0 / 255.0;
    /// Horizontal padding, in surface-native units, between the box's
    /// edge/icon/label — the box holds `pad | icon | pad | label | pad`.
    const SEARCH_PAD_PX: f32 = 8.0;
    /// Vertical inset applied to the search box's top/bottom edge,
    /// leaving a thin margin above/below it within the full-height
    /// search slot — matches every per-backend painter this replaces.
    const SEARCH_V_INSET: f32 = 2.0;

    /// Measure `cc`'s search box from `surface`'s live font, the
    /// pixel-exact twin of [`CommandCenterMeasure::from_char_width`] —
    /// mirrors `mac_command_center_layout`'s "three pads, not two"
    /// accounting (icon + its own trailing pad must be part of the
    /// measurement or the label spills past the rounded rect's right
    /// edge for a label long enough to beat the minimum width).
    fn measure(
        cc: &CommandCenter,
        surface: &dyn PaintSurface,
        height: f32,
    ) -> CommandCenterMeasure {
        let search_box_width = if cc.search_label.is_empty() {
            0.0
        } else {
            let (text_w, _) = surface.surface_measure_text(&cc.search_label);
            let (icon_w, _) = surface.surface_measure_text(&codicon::SEARCH.to_string());
            (text_w + icon_w + SEARCH_PAD_PX * 3.0).max(CommandCenterMeasure::SEARCH_MIN_WIDTH_PX)
        };
        CommandCenterMeasure {
            arrow_width: CommandCenterMeasure::ARROW_WIDTH_PX,
            gap: CommandCenterMeasure::GAP_PX,
            search_box_width,
            height,
        }
    }

    /// Compute the layout for `cc` within `rect` without painting —
    /// callers that only need hit-testing can skip [`paint`] entirely.
    pub(crate) fn layout(
        cc: &CommandCenter,
        surface: &dyn PaintSurface,
        rect: Rect,
    ) -> CommandCenterLayout {
        cc.layout(rect, measure(cc, surface, rect.height))
    }

    /// Paint `cc` into `rect` on `surface`, returning
    /// [`CommandCenterLayout`] for host click dispatch. `line_height`
    /// vertically centres the nav chevrons and search label — an
    /// explicit parameter, same as every per-backend painter.
    pub(crate) fn paint(
        cc: &CommandCenter,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        line_height: f32,
    ) -> CommandCenterLayout {
        let result_layout = cc.layout(rect, measure(cc, surface, rect.height));
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return result_layout;
        }

        surface.surface_fill_rect(rect, theme.tab_bar_bg);

        let enabled_fg = theme.tab_inactive_fg;
        let disabled_fg = theme.muted_fg;
        let text_y = rect.y + ((rect.height - line_height) / 2.0).max(0.0);

        if let Some(bb) = result_layout.back_bounds {
            let fg = if cc.back_enabled {
                enabled_fg
            } else {
                disabled_fg
            };
            draw_centered_chevron(surface, bb, codicon::CHEVRON_LEFT, fg, text_y, line_height);
        }
        if let Some(fb) = result_layout.forward_bounds {
            let fg = if cc.forward_enabled {
                enabled_fg
            } else {
                disabled_fg
            };
            draw_centered_chevron(surface, fb, codicon::CHEVRON_RIGHT, fg, text_y, line_height);
        }

        if let Some(sb) = result_layout.search_bounds {
            let box_rect = Rect::new(
                sb.x,
                sb.y + SEARCH_V_INSET,
                sb.width,
                (sb.height - SEARCH_V_INSET * 2.0).max(0.0),
            );
            let fill = theme.separator.with_alpha(SEARCH_FILL_ALPHA);
            let border = theme.separator.with_alpha(SEARCH_BORDER_ALPHA);
            surface.surface_fill_rounded_rect(box_rect, SEARCH_CORNER_RADIUS, fill);
            surface.surface_stroke_rounded_rect(box_rect, SEARCH_CORNER_RADIUS, border, 1.0);

            let icon_glyph = codicon::SEARCH.to_string();
            let (icon_w, icon_h) = surface.surface_measure_text(&icon_glyph);
            let icon_x = box_rect.x + SEARCH_PAD_PX;
            let icon_y = box_rect.y + ((box_rect.height - icon_h) / 2.0).max(0.0);
            surface.surface_draw_icon_glyph(
                Rect::new(icon_x, icon_y, icon_w, icon_h),
                &icon_glyph,
                disabled_fg,
            );

            let label_x = icon_x + icon_w + SEARCH_PAD_PX;
            surface.surface_draw_text_run(
                Rect::new(
                    label_x,
                    text_y,
                    (box_rect.x + box_rect.width - label_x).max(0.0),
                    line_height,
                ),
                &cc.search_label,
                theme.tab_inactive_fg,
            );
        }

        result_layout
    }

    /// Draw a single codicon chevron glyph centred horizontally within
    /// `slot`, at `text_y`/`line_height` — shared by the back/forward
    /// arrow arms of [`paint`] above.
    fn draw_centered_chevron(
        surface: &mut dyn PaintSurface,
        slot: Rect,
        glyph: char,
        color: crate::types::Color,
        text_y: f32,
        line_height: f32,
    ) {
        let glyph = glyph.to_string();
        let (tw, _) = surface.surface_measure_text(&glyph);
        let tx = slot.x + ((slot.width - tw) / 2.0).max(0.0);
        surface.surface_draw_icon_glyph(Rect::new(tx, text_y, tw, line_height), &glyph, color);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::command_center::CommandCenterHit;
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues — mirrors
        /// `primitives::progress::native_surface_paint`'s own
        /// `RecordingSurface`.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
            icon_glyphs: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(480.0, 32.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, _stroke_width: f32) {
                self.strokes.push((rect, color));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                _radius: f32,
                color: Color,
                _stroke_width: f32,
            ) {
                self.strokes.push((rect, color));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_icon_glyph(&mut self, rect: Rect, text: &str, color: Color) {
                self.icon_glyphs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn sample_cc() -> CommandCenter {
            CommandCenter {
                id: WidgetId::new("cc"),
                back_enabled: true,
                forward_enabled: false,
                search_label: "project".into(),
            }
        }

        #[test]
        fn empty_search_label_omits_search_bounds_and_box() {
            let cc = CommandCenter {
                search_label: "".into(),
                ..sample_cc()
            };
            let mut surface = RecordingSurface::default();
            let l = paint(
                &cc,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 480.0, 32.0),
                16.0,
            );
            assert!(l.search_bounds.is_none());
            assert!(l.back_bounds.is_some());
            assert!(l.forward_bounds.is_some());
            assert!(surface.strokes.is_empty());
        }

        #[test]
        fn search_box_paints_a_low_alpha_fill_and_border() {
            let cc = sample_cc();
            let mut surface = RecordingSurface::default();
            paint(
                &cc,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 480.0, 32.0),
                16.0,
            );
            let theme = Theme::default();
            let (_, fill_color) = surface
                .fills
                .iter()
                .find(|(_, c)| *c != theme.tab_bar_bg)
                .expect("search box fill recorded");
            assert!(
                fill_color.a < theme.separator.a,
                "fill alpha {} should be lower than the opaque separator alpha {}",
                fill_color.a,
                theme.separator.a
            );
            let (_, border_color) = surface.strokes.first().expect("search box border recorded");
            assert_eq!(border_color.r, theme.separator.r);
            assert!(border_color.a > 0 && border_color.a < 255);
        }

        #[test]
        fn search_icon_and_label_are_both_painted() {
            let cc = sample_cc();
            let mut surface = RecordingSurface::default();
            paint(
                &cc,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 480.0, 32.0),
                16.0,
            );
            assert_eq!(surface.icon_glyphs.len(), 3); // back + forward + search icon
            assert!(surface
                .icon_glyphs
                .iter()
                .any(|(_, g, _)| g == &codicon::SEARCH.to_string()));
            assert!(surface.text_runs.iter().any(|(_, t, _)| t == "project"));
        }

        #[test]
        fn back_and_forward_glyphs_use_enabled_or_disabled_colour() {
            let cc = sample_cc(); // back_enabled: true, forward_enabled: false
            let mut surface = RecordingSurface::default();
            paint(
                &cc,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 480.0, 32.0),
                16.0,
            );
            let theme = Theme::default();
            let back_glyph = codicon::CHEVRON_LEFT.to_string();
            let fwd_glyph = codicon::CHEVRON_RIGHT.to_string();
            let (_, _, back_color) = surface
                .icon_glyphs
                .iter()
                .find(|(_, g, _)| g == &back_glyph)
                .expect("back chevron recorded");
            let (_, _, fwd_color) = surface
                .icon_glyphs
                .iter()
                .find(|(_, g, _)| g == &fwd_glyph)
                .expect("forward chevron recorded");
            assert_eq!(*back_color, theme.tab_inactive_fg);
            assert_eq!(*fwd_color, theme.muted_fg);
        }

        #[test]
        fn hit_test_resolves_back_forward_search_at_nonzero_origin() {
            let cc = sample_cc();
            let mut surface = RecordingSurface::default();
            let l = paint(
                &cc,
                &mut surface,
                &Theme::default(),
                Rect::new(7.0, 13.0, 480.0, 32.0),
                16.0,
            );
            let back = l.back_bounds.unwrap();
            let fwd = l.forward_bounds.unwrap();
            let search = l.search_bounds.unwrap();
            assert!(back.x >= 7.0);
            assert_eq!(
                l.hit_test(back.x + 1.0, back.y + 1.0),
                CommandCenterHit::Back
            );
            assert_eq!(
                l.hit_test(fwd.x + 1.0, fwd.y + 1.0),
                CommandCenterHit::Forward
            );
            assert_eq!(
                l.hit_test(search.x + 1.0, search.y + 1.0),
                CommandCenterHit::SearchBox
            );
        }

        #[test]
        fn zero_size_rect_paints_nothing_but_still_returns_a_layout() {
            let cc = sample_cc();
            let mut surface = RecordingSurface::default();
            let l = paint(
                &cc,
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 0.0, 0.0),
                16.0,
            );
            assert!(surface.fills.is_empty());
            assert!(surface.icon_glyphs.is_empty());
            assert_eq!(l.bounds.width, 0.0);
        }

        #[test]
        fn layout_without_painting_matches_paint_search_bounds() {
            let cc = sample_cc();
            let rect = Rect::new(0.0, 0.0, 480.0, 32.0);
            let mut surface = RecordingSurface::default();
            let painted = paint(&cc, &mut surface, &Theme::default(), rect, 16.0);
            let no_paint = layout(&cc, &surface, rect);
            assert_eq!(painted.search_bounds, no_paint.search_bounds);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Shared char-width measure formula ──────────────────────────────

    #[test]
    fn from_char_width_empty_label_hides_search_box() {
        let m = CommandCenterMeasure::from_char_width("", 8.0, 24.0);
        assert_eq!(m.search_box_width, 0.0);
        assert_eq!(m.arrow_width, CommandCenterMeasure::ARROW_WIDTH_PX);
        assert_eq!(m.gap, CommandCenterMeasure::GAP_PX);
        assert_eq!(m.height, 24.0);
    }

    #[test]
    fn from_char_width_short_label_floors_at_min_width() {
        // "hi" is nowhere near wide enough to clear the 280px floor at
        // any sane char width.
        let m = CommandCenterMeasure::from_char_width("hi", 8.0, 24.0);
        assert_eq!(m.search_box_width, 280.0);
    }

    #[test]
    fn from_char_width_long_label_exceeds_min_width() {
        // 40 chars * 8.0 + 16.0 = 336.0, comfortably past the 280 floor —
        // pins the exact formula, not just the floor clamp above.
        let label = "x".repeat(40);
        let m = CommandCenterMeasure::from_char_width(&label, 8.0, 24.0);
        assert_eq!(m.search_box_width, 40.0 * 8.0 + 16.0);
    }

    /// All backends that measure search width from a plain `char_width`
    /// average compute this value for identical inputs through this single
    /// constructor, ensuring they agree on the search-box width.
    #[test]
    fn from_char_width_is_deterministic_for_the_same_inputs() {
        let label = "project-name";
        let a = CommandCenterMeasure::from_char_width(label, 7.5, 20.0);
        let b = CommandCenterMeasure::from_char_width(label, 7.5, 20.0);
        assert_eq!(a, b);
    }

    #[test]
    fn from_char_width_feeds_into_layout_consistently() {
        let cc = CommandCenter {
            id: WidgetId::new("cc"),
            back_enabled: true,
            forward_enabled: true,
            search_label: "project".into(),
        };
        let measure = CommandCenterMeasure::from_char_width(&cc.search_label, 8.0, 24.0);
        let layout = cc.layout(Rect::new(0.0, 0.0, 500.0, 24.0), measure);
        let search = layout
            .search_bounds
            .expect("non-empty label has search bounds");
        assert_eq!(search.width, measure.search_box_width);
    }
}
