//! `Completions` primitive: an LSP-style autocomplete popup anchored to
//! the cursor position. Similar to `ListView` but tailored for
//! completion semantics — per-item `kind` icon (function / variable /
//! snippet / etc.), right-aligned type detail, optional
//! documentation preview.
//!
//! Unlike `Palette`, the query isn't inside the primitive — it's
//! implicit in the cursor position. The app filters its source against
//! the current word-under-cursor and passes the visible `items`.
//!
//! # Backend contract
//!
//! **Declarative + anchor-positioned.** Apps pass the cursor position
//! and the primitive's `layout()` picks a position that keeps the
//! popup inside the viewport (prefer below the cursor, flip to above
//! if it would overflow). Click on item → `ItemActivated { idx }`;
//! keyboard navigation emits the respective events; Escape / Enter /
//! Tab follow their typical IDE semantics.

use crate::event::Rect;
use crate::layout::{Anchor, ResolvedSide, Side};
use crate::types::{Icon, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a completion popup.
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
/// use quadraui::{
///     CompletionItem, CompletionItemMeasure, Completions, CompletionsHit, Rect, StyledText,
///     WidgetId,
/// };
///
/// let mut popup = Completions::default();
/// popup.id = WidgetId::new("completions:editor");
/// popup.items = vec![CompletionItem {
///     label: StyledText::plain("map"),
///     detail: None,
///     documentation: None,
///     kind: Default::default(),
///     icon: None,
/// }];
/// popup.has_focus = true;
///
/// let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
/// let layout = popup.layout(10.0, 5.0, 1.0, viewport, 20.0, 10.0, |_| {
///     CompletionItemMeasure::new(1.0)
/// });
///
/// assert_eq!(layout.hit_test(10.0, 6.0), CompletionsHit::Item(0));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completions {
    pub id: WidgetId,
    pub items: Vec<CompletionItem>,
    pub selected_idx: usize,
    #[serde(default)]
    pub scroll_offset: usize,
    /// When true, the primitive is "live" and receiving focus for
    /// keyboard nav. When false it's suppressed (rendered but
    /// non-interactive) — apps set this based on their own state.
    #[serde(default)]
    pub has_focus: bool,
}

impl Default for Completions {
    /// An empty, unfocused completion popup with no items. Set fields
    /// directly afterwards (every field is `pub`) — see
    /// `docs/PRIMITIVE_RULES.md` rule 8.
    fn default() -> Self {
        Completions {
            id: WidgetId::new(""),
            items: Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: false,
        }
    }
}

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    /// Display label (what the user sees in the popup). Usually the
    /// same as the insertion text, but LSP can differ (e.g. label
    /// `"map(..)"` inserts `"map"`).
    pub label: StyledText,
    /// Right-aligned type / detail text, e.g. `"fn(T) -> U"` or
    /// `"crate::foo"`.
    #[serde(default)]
    pub detail: Option<StyledText>,
    /// Optional documentation snippet shown in a preview panel. The
    /// primitive doesn't render this — apps display it separately when
    /// they care.
    #[serde(default)]
    pub documentation: Option<StyledText>,
    /// Kind icon (LSP CompletionItemKind mapping).
    #[serde(default)]
    pub kind: CompletionKind,
    /// Icon override. Backends default to a theme-defined glyph per
    /// `kind` if this is `None`.
    #[serde(default)]
    pub icon: Option<Icon>,
}

impl CompletionItem {
    /// The icon a rasteriser actually paints: [`Self::icon`] when set,
    /// else [`CompletionKind::default_icon`] for [`Self::kind`]. Every
    /// completion paints *some* icon — `kind` always has a default —
    /// so callers never need their own `Option` fallback.
    pub fn resolved_icon(&self) -> Icon {
        self.icon
            .clone()
            .unwrap_or_else(|| self.kind.default_icon())
    }
}

/// Kind of completion — drives the default icon + colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompletionKind {
    #[default]
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

impl CompletionKind {
    /// Two-letter ASCII mnemonic painted ahead of the label when
    /// [`CompletionItem::icon`] is `None`.
    ///
    /// Not a bundled-codicon glyph: codicon only ships a handful of
    /// `symbol-*` names in [`crate::Icon::codicon`]'s curated table (see
    /// `codicon_names.rs`), nowhere near this enum's 25 variants, and
    /// guessing the rest of the upstream PUA codepoints by hand risks
    /// painting the wrong glyph with nothing to catch it (every real
    /// entry in that table is independently verified against the
    /// bundled font's own `cmap`/`post` tables). An ASCII mnemonic has
    /// no such risk and, with [`Icon::glyph`] equal to [`Icon::fallback`],
    /// paints identically on every backend with no font dependency —
    /// extending the curated codicon table to cover every kind is
    /// follow-on work, not a precondition for painting *an* icon here.
    pub fn default_icon(&self) -> Icon {
        let mnemonic = match self {
            CompletionKind::Text => "tx",
            CompletionKind::Method => "me",
            CompletionKind::Function => "fn",
            CompletionKind::Constructor => "ct",
            CompletionKind::Field => "fd",
            CompletionKind::Variable => "va",
            CompletionKind::Class => "cl",
            CompletionKind::Interface => "if",
            CompletionKind::Module => "mo",
            CompletionKind::Property => "pr",
            CompletionKind::Unit => "un",
            CompletionKind::Value => "vl",
            CompletionKind::Enum => "en",
            CompletionKind::Keyword => "kw",
            CompletionKind::Snippet => "sn",
            CompletionKind::Color => "co",
            CompletionKind::File => "fi",
            CompletionKind::Reference => "rf",
            CompletionKind::Folder => "fo",
            CompletionKind::EnumMember => "em",
            CompletionKind::Constant => "cn",
            CompletionKind::Struct => "st",
            CompletionKind::Event => "ev",
            CompletionKind::Operator => "op",
            CompletionKind::TypeParameter => "tp",
        };
        Icon::new(mnemonic, mnemonic)
    }
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Per-item measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompletionItemMeasure {
    pub height: f32,
}

impl CompletionItemMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

/// Resolved position of one visible completion item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleCompletion {
    pub item_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionsHit {
    Item(usize),
    /// Click inside the popup but not on an item (border padding).
    Inert,
    /// Click outside the popup — typically dismisses.
    Empty,
}

/// Resolved placement direction for the popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionsPlacement {
    /// Popup rendered below the cursor anchor.
    Below,
    /// Popup rendered above the cursor anchor (flipped to avoid
    /// overflow).
    Above,
}

/// Fully-resolved completions-popup layout.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionsLayout {
    pub bounds: Rect,
    pub placement: CompletionsPlacement,
    pub visible_items: Vec<VisibleCompletion>,
    pub hit_regions: Vec<(Rect, CompletionsHit)>,
    pub resolved_scroll_offset: usize,
}

impl CompletionsLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> CompletionsHit {
        let inside = x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height;
        if !inside {
            return CompletionsHit::Empty;
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        CompletionsHit::Inert
    }
}

impl Completions {
    /// Compute popup placement + per-item bounds.
    ///
    /// # Arguments
    ///
    /// - `cursor_x`, `cursor_y` — caret position in viewport coords.
    /// - `line_height` — height of the line containing the cursor
    ///   (used for the gap between cursor and popup).
    /// - `viewport` — parent surface bounds.
    /// - `popup_width`, `max_popup_height` — size constraints for the
    ///   popup.
    /// - `measure_item(i)` — height for item `i`.
    ///
    /// # Placement
    ///
    /// Prefer below the cursor; flip above if below would overflow.
    /// Shift left if the popup would extend past the right edge.
    #[allow(clippy::too_many_arguments)]
    pub fn layout<F>(
        &self,
        cursor_x: f32,
        cursor_y: f32,
        line_height: f32,
        viewport: Rect,
        popup_width: f32,
        max_popup_height: f32,
        measure_item: F,
    ) -> CompletionsLayout
    where
        F: Fn(usize) -> CompletionItemMeasure,
    {
        // Clamp scroll offset.
        let resolved_scroll_offset =
            crate::primitives::scrollbar::clamp_scroll_offset(self.scroll_offset, self.items.len());

        // Determine the popup's height (bounded by max and total content).
        let total_content: f32 = (resolved_scroll_offset..self.items.len())
            .map(|i| measure_item(i).height)
            .sum();
        let desired_height = total_content.min(max_popup_height);

        // Vertical placement: below cursor if it fits, else above, via
        // the shared `Anchor` flip/clamp resolver. The anchor rect's
        // width is set to `popup_width` so `Anchor::resolve`'s cross-axis
        // centering (`(rect.width - w) * 0.5`) collapses to zero and `x`
        // stays left-aligned with `cursor_x` rather than centered; its
        // height is `line_height` so the `Below` candidate opens flush
        // under the cursor's line, matching this popup's pre-`Anchor`
        // geometry exactly in every case that fits either side.
        //
        // When *neither* side fits, `Anchor::resolve` pins to the
        // preferred side (`Below`) rather than picking whichever side
        // has more room — see the `completions_layout_pins_below_when_neither_fits`
        // test for the case that differs from the pre-`Anchor`
        // implementation.
        let anchor_rect = Rect::new(cursor_x, cursor_y, popup_width, line_height);
        let (origin, resolved_side) =
            Anchor::new(anchor_rect, Side::Bottom).resolve((popup_width, desired_height), viewport);
        let (x, y) = (origin.x, origin.y);
        let placement = match resolved_side {
            ResolvedSide::Bottom => CompletionsPlacement::Below,
            ResolvedSide::Top => CompletionsPlacement::Above,
            _ => unreachable!("Completions only ever prefers Side::Bottom"),
        };

        // Clip popup to the viewport.
        let clipped_h = (viewport.y + viewport.height - y)
            .min(desired_height)
            .max(0.0);
        let bounds = Rect::new(x, y, popup_width, clipped_h);

        let mut visible_items: Vec<VisibleCompletion> = Vec::new();
        let mut hit_regions: Vec<(Rect, CompletionsHit)> = Vec::new();

        let mut cursor_row = y;
        for i in resolved_scroll_offset..self.items.len() {
            if cursor_row >= y + clipped_h {
                break;
            }
            let m = measure_item(i);
            let remaining = y + clipped_h - cursor_row;
            let draw_h = m.height.min(remaining).max(0.0);
            if draw_h <= 0.0 {
                break;
            }
            let item_bounds = Rect::new(x, cursor_row, popup_width, draw_h);
            visible_items.push(VisibleCompletion {
                item_idx: i,
                bounds: item_bounds,
            });
            hit_regions.push((item_bounds, CompletionsHit::Item(i)));
            cursor_row += m.height;
        }

        CompletionsLayout {
            bounds,
            placement,
            visible_items,
            hit_regions,
            resolved_scroll_offset,
        }
    }
}

/// Shared pixel-surface paint for [`Completions`] — the single place a
/// GUI backend's popup paint logic lives, instead of each pixel backend
/// (GTK/Cairo, macOS/CoreGraphics, Windows/Direct2D) carrying its own
/// copy that can silently drift from the other two. TUI is not here —
/// see `crate::paint_surface`'s module doc ("Why TUI stays out") — its
/// painter lives in `crate::tui::completions` and degrades the same
/// content (kind icon, every label span, detail) onto a cell grid
/// instead of this trait's pixel-`Rect` verbs.
///
/// Wiring GTK's/macOS's/Windows' own `draw_completions` through this is
/// follow-on, per-backend work (this issue is backend-neutral); until
/// then this module is reachable only from its own tests, same as any
/// shared painter added ahead of its backend adoption.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Completions, CompletionsLayout};
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::Rect;

    /// Paint a [`Completions`] popup at its resolved [`CompletionsLayout`]:
    /// background + border, then each visible row's selection fill, kind
    /// icon ([`super::CompletionItem::resolved_icon`]), every label span,
    /// and right-aligned `detail`.
    ///
    /// - `nerd_fonts_enabled` selects `Icon::glyph` vs. `Icon::fallback`
    ///   for the kind icon, same as every other icon-painting rasteriser
    ///   in this crate.
    /// - `style` supplies [`Style::row_padding_x`], the inset a row's
    ///   icon starts at.
    pub(crate) fn paint(
        completions: &Completions,
        layout: &CompletionsLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        nerd_fonts_enabled: bool,
    ) {
        let bounds = layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        surface.surface_fill_rect(bounds, theme.completion_bg);
        surface.surface_stroke_rect(bounds, theme.completion_border, 1.0);

        for vis in &layout.visible_items {
            let Some(item) = completions.items.get(vis.item_idx) else {
                continue;
            };
            let row = vis.bounds;
            let is_selected = vis.item_idx == completions.selected_idx;
            let row_bg = if is_selected {
                theme.completion_selected_bg
            } else {
                theme.completion_bg
            };
            if is_selected {
                surface.surface_fill_rect(row, row_bg);
            }

            surface.surface_push_clip(row);
            let mut cursor_x = row.x + style.row_padding_x;

            let icon = item.resolved_icon();
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            if !glyph.is_empty() {
                let (iw, ih) = surface.surface_measure_text(glyph);
                surface.surface_draw_text_run(
                    Rect::new(cursor_x, row.y + (row.height - ih) / 2.0, iw, ih),
                    glyph,
                    theme.completion_fg,
                );
                cursor_x += iw + 6.0;
            }

            let detail_info = item.detail.as_ref().map(|detail| {
                let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
                let (dw, _) = surface.surface_measure_text(&detail_text);
                (detail_text, dw)
            });
            let detail_reserve = detail_info.as_ref().map(|(_, dw)| *dw + 8.0).unwrap_or(0.0);
            let text_right_limit = row.x + row.width - detail_reserve - 4.0;

            // A span that overflows `text_right_limit` is ellipsis-
            // truncated via the shared `fit_text_prefix` rather than
            // hard-clipped mid-glyph — same treatment as `ListView`'s
            // and `TreeView`'s rasterisers.
            for span in &item.label.spans {
                let remaining = text_right_limit - cursor_x;
                if remaining <= 0.0 {
                    break;
                }
                let span_fg = span.fg.unwrap_or(theme.completion_fg);
                let (sw, sh) = surface.surface_measure_text_styled(&span.text, span.bold);
                let (paint_text, paint_w, truncated) = if sw <= remaining {
                    (span.text.clone(), sw, false)
                } else {
                    let (t, w) = crate::paint_surface::fit_text_prefix(
                        surface, &span.text, span.bold, remaining,
                    );
                    (t, w, true)
                };
                if paint_w <= 0.0 {
                    break;
                }
                if let Some(sbg) = span.bg {
                    surface.surface_fill_rect(Rect::new(cursor_x, row.y, paint_w, row.height), sbg);
                }
                surface.surface_draw_text_run_styled(
                    Rect::new(cursor_x, row.y + (row.height - sh) / 2.0, paint_w, sh),
                    &paint_text,
                    span_fg,
                    span.bold,
                    false,
                    false,
                    1.0,
                );
                cursor_x += paint_w;
                if truncated {
                    break;
                }
            }

            surface.surface_pop_clip();

            // Detail text is pinned to the row's right edge, not the
            // h-scroll-shifted label — there is no h-scroll here, but
            // this keeps the same "measure outside the label clip"
            // shape `ListView`'s rasteriser uses.
            if let Some((detail_text, dw)) = detail_info {
                let dx = row.x + row.width - dw - 4.0;
                if dx > cursor_x {
                    let (_, dh) = surface.surface_measure_text(&detail_text);
                    surface.surface_draw_text_run(
                        Rect::new(dx, row.y + (row.height - dh) / 2.0, dw, dh),
                        &detail_text,
                        theme.muted_fg,
                    );
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::completions::{
            CompletionItem, CompletionItemMeasure, CompletionKind,
        };
        use crate::types::{Color, StyledSpan, StyledText, WidgetId};
        use crate::Image;

        /// Records every fill + stroke + styled text-run call — mirrors
        /// `primitives::list`'s `RecordingSurface` test double, scoped to
        /// the verbs [`paint`] uses, so this test runs on any host
        /// without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            texts: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(400.0, 200.0, 1.0)
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
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                _radius: f32,
                color: Color,
                stroke_width: f32,
            ) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.texts.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        use crate::event::Point;

        fn completions_with(items: Vec<CompletionItem>) -> Completions {
            Completions {
                id: WidgetId::new("c"),
                items,
                selected_idx: 1,
                scroll_offset: 0,
                has_focus: true,
            }
        }

        fn layout_for(c: &Completions) -> CompletionsLayout {
            let viewport = Rect::new(0.0, 0.0, 400.0, 200.0);
            c.layout(10.0, 10.0, 16.0, viewport, 300.0, 100.0, |_| {
                CompletionItemMeasure::new(20.0)
            })
        }

        #[test]
        fn paints_kind_icon_ahead_of_the_label() {
            let items = vec![CompletionItem {
                label: StyledText::plain("map"),
                detail: None,
                documentation: None,
                kind: CompletionKind::Function,
                icon: None,
            }];
            let c = completions_with(items);
            let l = layout_for(&c);
            let mut surface = RecordingSurface::default();
            paint(
                &c,
                &l,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                true,
            );
            let icon = CompletionKind::Function.default_icon();
            assert!(
                surface.texts.iter().any(|(_, text, _)| text == &icon.glyph),
                "kind icon {:?} not painted: {:?}",
                icon.glyph,
                surface.texts
            );
        }

        #[test]
        fn paints_every_label_span() {
            let items = vec![CompletionItem {
                label: StyledText {
                    spans: vec![StyledSpan::plain("fn "), StyledSpan::plain("main")],
                },
                detail: None,
                documentation: None,
                kind: CompletionKind::Text,
                icon: None,
            }];
            let c = completions_with(items);
            let l = layout_for(&c);
            let mut surface = RecordingSurface::default();
            paint(
                &c,
                &l,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                false,
            );
            assert!(surface.texts.iter().any(|(_, text, _)| text == "fn "));
            assert!(surface.texts.iter().any(|(_, text, _)| text == "main"));
        }

        #[test]
        fn paints_detail_text() {
            let items = vec![CompletionItem {
                label: StyledText::plain("map"),
                detail: Some(StyledText::plain("fn(T) -> U")),
                documentation: None,
                kind: CompletionKind::Method,
                icon: None,
            }];
            let c = completions_with(items);
            let l = layout_for(&c);
            let mut surface = RecordingSurface::default();
            paint(
                &c,
                &l,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                false,
            );
            assert!(surface
                .texts
                .iter()
                .any(|(_, text, _)| text == "fn(T) -> U"));
        }

        #[test]
        fn fills_the_selected_row_with_completion_selected_bg() {
            let items = vec![
                CompletionItem {
                    label: StyledText::plain("a"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Text,
                    icon: None,
                },
                CompletionItem {
                    label: StyledText::plain("b"),
                    detail: None,
                    documentation: None,
                    kind: CompletionKind::Text,
                    icon: None,
                },
            ];
            let c = completions_with(items); // selected_idx: 1
            let l = layout_for(&c);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(&c, &l, &mut surface, &theme, &Style::default(), false);
            let selected_row = l.visible_items[1].bounds;
            assert!(surface.fills.iter().any(
                |(rect, color)| *rect == selected_row && *color == theme.completion_selected_bg
            ));
            // The unselected row must not get that fill.
            let other_row = l.visible_items[0].bounds;
            assert!(!surface
                .fills
                .iter()
                .any(|(rect, color)| *rect == other_row && *color == theme.completion_selected_bg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Completions primitive tests (D6) ──────────────────────────────

    fn make_completion(label: &str, kind: CompletionKind) -> CompletionItem {
        CompletionItem {
            label: StyledText::plain(label),
            detail: None,
            documentation: None,
            kind,
            icon: None,
        }
    }

    #[test]
    fn completions_layout_below_cursor() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: vec![
                make_completion("fn main", CompletionKind::Function),
                make_completion("fn map", CompletionKind::Function),
                make_completion("Vec", CompletionKind::Struct),
            ],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = c.layout(100.0, 50.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Below);
        assert_eq!(layout.bounds.x, 100.0);
        // Popup y = cursor_y + line_height = 50 + 18 = 68
        assert_eq!(layout.bounds.y, 68.0);
        assert_eq!(layout.visible_items.len(), 3);
        assert_eq!(layout.visible_items[0].bounds.y, 68.0);
        // Click on 2nd item.
        match layout.hit_test(150.0, 90.0) {
            CompletionsHit::Item(idx) => assert_eq!(idx, 1),
            _ => panic!(),
        }
    }

    #[test]
    fn completions_layout_flips_above_when_below_overflows() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..5)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        // Cursor near the bottom of a small viewport.
        let viewport = Rect::new(0.0, 0.0, 800.0, 150.0);
        // Cursor at y=120, line_height=18 → below_y = 138, need 100 px (5 × 20)
        // → bottom edge = 238, > 150 viewport. Flip above.
        let layout = c.layout(100.0, 120.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Above);
        // Above: y = cursor - content_h = 120 - 100 = 20
        assert_eq!(layout.bounds.y, 20.0);
    }

    #[test]
    fn completions_layout_shifts_left_when_right_overflows() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: vec![make_completion("item", CompletionKind::Text)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 600.0);
        // Cursor near right edge — popup_width 300, cursor_x 200 → right would be 500 > 400.
        let layout = c.layout(200.0, 50.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 100.0); // 400 - 300
    }

    #[test]
    fn completions_layout_scroll_offset_applies() {
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..10)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 5,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = c.layout(0.0, 0.0, 18.0, viewport, 200.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        // Visible items start at index 5.
        assert_eq!(layout.visible_items[0].item_idx, 5);
    }

    #[test]
    fn completions_layout_pins_below_when_neither_side_fully_fits() {
        // 3 items * 20 = 60 desired height; viewport only 100 tall with
        // the cursor near its middle, so neither the `below_y`/`above_y`
        // candidate fits without clamping.
        //
        // Behaviour change from the pre-`Anchor` implementation: that
        // code picked whichever side had more *room* (here, above:
        // 55 > 40), landing the popup pinned to the viewport's top edge
        // (y=0). `Anchor::resolve`'s shared fallback instead pins to the
        // *preferred* side (`Below`, matching `Tooltip`/`Float`/
        // `ContextMenu`) and clamps that — landing pinned to the bottom
        // edge instead (y=40, i.e. `bounds.y + bounds.height == 100`).
        // Both place the (unclipped) 60-tall popup fully inside the
        // 100-tall viewport; only which edge it is pinned to differs.
        let c = Completions {
            id: WidgetId::new("c"),
            items: (0..3)
                .map(|i| make_completion(&format!("x{i}"), CompletionKind::Variable))
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 100.0);
        let layout = c.layout(100.0, 55.0, 5.0, viewport, 200.0, 60.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.placement, CompletionsPlacement::Below);
        assert_eq!(layout.bounds.y, 40.0);
        assert_eq!(layout.bounds.height, 60.0);
    }

    #[test]
    fn completions_layout_clamps_x_to_viewport_left_edge() {
        // Cursor at x=0 with `popup_width` larger than it — `x` must
        // still clamp to the viewport's own left edge rather than go
        // negative.
        let c = Completions {
            id: WidgetId::new("c"),
            items: vec![make_completion("item", CompletionKind::Text)],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let viewport = Rect::new(10.0, 0.0, 400.0, 600.0);
        let layout = c.layout(0.0, 50.0, 18.0, viewport, 300.0, 200.0, |_| {
            CompletionItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 10.0);
    }
}
