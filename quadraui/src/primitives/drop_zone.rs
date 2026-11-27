//! Drop-zone computation for tab drag-and-drop.
//!
//! Given a set of group rectangles (each with per-tab slot positions)
//! and a cursor position, [`compute_drop_zone`] determines where a
//! dragged tab would land: center of a group, an edge split, or a
//! reorder position within the tab bar.
//!
//! [`DropOverlay`] translates a [`DropZone`] result into visual
//! overlay geometry (highlight rect, insertion bar, ghost label
//! position). It is a widget-contract [`crate::primitive::Primitive`]:
//! a host paints it on top of the normal frame with
//! [`crate::backend::DrawsPrimitives::draw_dyn`] at
//! [`DropOverlay::bounds`], or pushes it as a
//! [`crate::frame::Surface::Widget`] at that rect.

use crate::event::Rect;
use serde::{Deserialize, Serialize};

/// A group's bounds and per-tab slot positions, supplied by the
/// consumer's layout system. `tab_slots` contains `(start_x, end_x)`
/// pairs in the same coordinate system as `bounds`.
#[derive(Debug, Clone)]
pub struct DropGroupRect {
    pub bounds: Rect,
    pub tab_slots: Vec<(f32, f32)>,
}

/// Cardinal direction for a split drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum DropEdge {
    Left,
    Right,
    Top,
    Bottom,
}

/// What kind of drop zone the cursor is over.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum DropZoneKind {
    /// Drop onto the center — tab joins this group.
    #[default]
    Center,
    /// Drop on an edge — split the group in this direction.
    Split(DropEdge),
    /// Drop between tabs — reorder within the tab bar.
    /// The `usize` is the insertion index (0 = before first tab,
    /// `tab_count` = after last tab).
    TabReorder(usize),
}

/// Result of [`compute_drop_zone`].
///
/// # Examples
///
/// ```
/// use quadraui::{compute_drop_zone, DropGroupRect, DropZoneKind, Rect};
///
/// let groups = vec![DropGroupRect {
///     bounds: Rect::new(0.0, 0.0, 100.0, 50.0),
///     tab_slots: vec![],
/// }];
///
/// // Cursor over the group's content area, away from any edge, lands
/// // on `Center` — the tab would join this group.
/// let zone = compute_drop_zone(50.0, 30.0, &groups, 2.0).expect("cursor is over a group");
/// assert_eq!(zone.group_idx, 0);
/// assert_eq!(zone.kind, DropZoneKind::Center);
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct DropZone {
    pub kind: DropZoneKind,
    /// Which group (index into the `groups` slice) this targets.
    pub group_idx: usize,
}

/// Compute which drop zone a cursor position falls into.
///
/// `tab_bar_height` is the height of the tab bar region at the top of
/// each group. Cursor positions within the tab bar region produce
/// `TabReorder`; positions in the content area below produce `Center`
/// or `Split` depending on edge proximity.
///
/// Returns `None` if the cursor is outside all groups.
pub fn compute_drop_zone(
    cursor_x: f32,
    cursor_y: f32,
    groups: &[DropGroupRect],
    tab_bar_height: f32,
) -> Option<DropZone> {
    for (gi, group) in groups.iter().enumerate() {
        let b = &group.bounds;
        if cursor_x < b.x
            || cursor_x >= b.x + b.width
            || cursor_y < b.y
            || cursor_y >= b.y + b.height
        {
            continue;
        }

        let in_tab_bar = cursor_y < b.y + tab_bar_height && tab_bar_height > 0.0;

        if in_tab_bar && !group.tab_slots.is_empty() {
            let insert_idx = tab_reorder_index(cursor_x, &group.tab_slots);
            return Some(DropZone {
                kind: DropZoneKind::TabReorder(insert_idx),
                group_idx: gi,
            });
        }

        let content_y = b.y + tab_bar_height;
        let content_h = (b.height - tab_bar_height).max(0.0);
        if content_h <= 0.0 {
            return Some(DropZone {
                kind: DropZoneKind::Center,
                group_idx: gi,
            });
        }

        let edge_w = edge_zone_size(b.width);
        let edge_h = edge_zone_size(content_h);

        let rel_x = cursor_x - b.x;
        let rel_y = cursor_y - content_y;

        if rel_x < edge_w {
            return Some(DropZone {
                kind: DropZoneKind::Split(DropEdge::Left),
                group_idx: gi,
            });
        }
        if rel_x >= b.width - edge_w {
            return Some(DropZone {
                kind: DropZoneKind::Split(DropEdge::Right),
                group_idx: gi,
            });
        }
        if rel_y < edge_h {
            return Some(DropZone {
                kind: DropZoneKind::Split(DropEdge::Top),
                group_idx: gi,
            });
        }
        if rel_y >= content_h - edge_h {
            return Some(DropZone {
                kind: DropZoneKind::Split(DropEdge::Bottom),
                group_idx: gi,
            });
        }

        return Some(DropZone {
            kind: DropZoneKind::Center,
            group_idx: gi,
        });
    }
    None
}

/// Hit-test classification for a cursor position against a set of
/// [`DropGroupRect`]s.
#[derive(Debug, Clone, PartialEq)]
pub enum DropZoneHit {
    /// Cursor is over a live drop zone.
    Zone(DropZone),
    /// Cursor is outside every group.
    Empty,
}

/// Hit-test a cursor position against `groups`, following this
/// primitive's `Empty`-variant convention on top of [`compute_drop_zone`].
/// Identical arguments and behaviour to
/// [`compute_drop_zone`] — this only wraps its `Option` result in a
/// [`DropZoneHit`] so `DropZone`, like every other primitive, has a
/// `hit_test` entry point with a named miss case instead of `None`.
pub fn drop_zone_hit_test(
    cursor_x: f32,
    cursor_y: f32,
    groups: &[DropGroupRect],
    tab_bar_height: f32,
) -> DropZoneHit {
    match compute_drop_zone(cursor_x, cursor_y, groups, tab_bar_height) {
        Some(zone) => DropZoneHit::Zone(zone),
        None => DropZoneHit::Empty,
    }
}

/// Compute the insertion index from cursor x and tab slot positions.
/// Finds the midpoint of each tab; cursor left of midpoint inserts
/// before, right inserts after.
fn tab_reorder_index(cursor_x: f32, slots: &[(f32, f32)]) -> usize {
    for (i, (start, end)) in slots.iter().enumerate() {
        let mid = (*start + *end) / 2.0;
        if cursor_x < mid {
            return i;
        }
    }
    slots.len()
}

/// Edge zone size: 20% of dimension, at least 3 but never more than
/// half the dimension (so a dimension under 6 splits evenly into its two
/// edges).
fn edge_zone_size(dimension: f32) -> f32 {
    (dimension * 0.2).max(3.0).min(dimension / 2.0)
}

// ── Drop overlay ────────────────────────────────────────────────────────────

/// Visual overlay components for rendering a drop zone indicator,
/// painted on top of the normal frame during a tab drag.
#[derive(Debug, Clone, PartialEq)]
pub struct DropOverlay {
    /// Tinted rectangle highlighting the target zone.
    pub highlight: Option<Rect>,
    /// Vertical or horizontal insertion bar for tab reorder.
    pub insertion_bar: Option<Rect>,
    /// Ghost label position: `(x, y)` near the cursor.
    pub ghost_position: Option<(f32, f32)>,
}

impl DropOverlay {
    /// Alpha applied to the highlight tint by the shared pixel painter
    /// every pixel backend paints the overlay through.
    pub const HIGHLIGHT_ALPHA: f32 = 0.15;

    /// Minimum insertion-bar thickness (px on GTK/macOS, DIPs on
    /// Win-GUI), applied when [`DropOverlay::insertion_bar`]'s computed
    /// width would otherwise be thinner (e.g. a zero-width bar from
    /// [`drop_zone_overlay`]).
    pub const MIN_BAR_THICKNESS: f32 = 2.0;

    /// The rect this overlay paints inside: the union of
    /// [`Self::highlight`] and [`Self::insertion_bar`] (the bar widened to
    /// [`Self::MIN_BAR_THICKNESS`], as the pixel painter widens it), or a
    /// zero-sized [`Rect::default`] when the overlay carries neither.
    ///
    /// The overlay's geometry is absolute, so this is the rect a host
    /// pushes it at as a [`crate::frame::Surface::Widget`] or hands to
    /// [`crate::backend::DrawsPrimitives::draw_dyn`]: a pixel backend clips
    /// a widget's paint to that rect, and this is the smallest one that
    /// clips nothing.
    pub fn bounds(&self) -> Rect {
        let bar = self
            .insertion_bar
            .map(|b| Rect::new(b.x, b.y, b.width.max(Self::MIN_BAR_THICKNESS), b.height));
        match (self.highlight, bar) {
            (Some(h), Some(b)) => {
                let x0 = h.x.min(b.x);
                let y0 = h.y.min(b.y);
                let x1 = (h.x + h.width).max(b.x + b.width);
                let y1 = (h.y + h.height).max(b.y + b.height);
                Rect::new(x0, y0, x1 - x0, y1 - y0)
            }
            (Some(h), None) => h,
            (None, Some(b)) => b,
            (None, None) => Rect::default(),
        }
    }
}

/// Compute overlay geometry for a drop zone.
///
/// `bar_height` is the thickness of the insertion bar (typically 2–3
/// pixels in GTK, 1 cell in TUI). `ghost_offset` is how far the ghost
/// label floats from the cursor (typically one line height).
pub fn drop_zone_overlay(
    zone: &DropZone,
    groups: &[DropGroupRect],
    cursor_x: f32,
    cursor_y: f32,
    tab_bar_height: f32,
    bar_thickness: f32,
    ghost_offset: f32,
) -> DropOverlay {
    let b = groups[zone.group_idx].bounds;
    let content_y = b.y + tab_bar_height;
    let content_h = (b.height - tab_bar_height).max(0.0);

    match &zone.kind {
        DropZoneKind::Center => DropOverlay {
            highlight: Some(Rect::new(b.x, content_y, b.width, content_h)),
            insertion_bar: None,
            ghost_position: Some((cursor_x + ghost_offset, cursor_y)),
        },
        DropZoneKind::Split(dir) => {
            let half_w = b.width / 2.0;
            let half_h = content_h / 2.0;
            let highlight = match dir {
                DropEdge::Left => Rect::new(b.x, content_y, half_w, content_h),
                DropEdge::Right => Rect::new(b.x + half_w, content_y, half_w, content_h),
                DropEdge::Top => Rect::new(b.x, content_y, b.width, half_h),
                DropEdge::Bottom => Rect::new(b.x, content_y + half_h, b.width, half_h),
            };
            DropOverlay {
                highlight: Some(highlight),
                insertion_bar: None,
                ghost_position: Some((cursor_x + ghost_offset, cursor_y)),
            }
        }
        DropZoneKind::TabReorder(idx) => {
            let slots = &groups[zone.group_idx].tab_slots;
            let bar_x = if *idx == 0 {
                slots.first().map_or(b.x, |(s, _)| *s)
            } else if *idx >= slots.len() {
                slots.last().map_or(b.x, |(_, e)| *e)
            } else {
                let (_, prev_end) = slots[*idx - 1];
                let (next_start, _) = slots[*idx];
                (prev_end + next_start) / 2.0
            };
            DropOverlay {
                highlight: None,
                insertion_bar: Some(Rect::new(
                    bar_x - bar_thickness / 2.0,
                    b.y,
                    bar_thickness,
                    tab_bar_height,
                )),
                ghost_position: Some((cursor_x + ghost_offset, cursor_y)),
            }
        }
    }
}

// ── Widget contract ─────────────────────────────────────────────────────────
//
// `DropOverlay` reaches a backend through `DrawsPrimitives::draw_dyn` (or
// a `Surface::Widget`) at the rect `DropOverlay::bounds` reports, and the
// host registers `DROP_OVERLAY_ZONE_ID` at that rect. Its
// geometry is absolute, so the resolved layout is the descriptor's own
// rects rather than anything derived from the host's `rect`; `paint` runs
// the shared pixel painter and `paint_cells` the cell painter in
// `cell_paint`. A drop overlay reacts to no input of its own: the drag it
// visualises belongs to the host (`compose::TabGroup`), so the widget is
// `non_interactive!`.

/// The zone id a host registers with [`crate::Backend::register_zone`]
/// at [`DropOverlay::bounds`] after painting an overlay. The overlay
/// paints no text, so this zone is what attributes it in a frame's
/// inventory. At most one overlay is visible at a time, so one fixed
/// chrome id serves every overlay.
///
/// The overlay itself carries no [`crate::types::WidgetId`]: it is
/// painted over the tab bars a drag is hovering, and an id would make it
/// the widget-contract hit for every point it covers.
pub const DROP_OVERLAY_ZONE_ID: &str = "chrome:drop-overlay";

/// The resolved layout of a [`DropOverlay`]: its paint rects, in the same
/// absolute frame the descriptor's own geometry is expressed in.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct DropOverlayLayout {
    /// [`DropOverlay::bounds`] — the union of everything below.
    pub bounds: Rect,
    /// The tinted target-zone rect, when the overlay has one.
    pub highlight: Option<Rect>,
    /// The tab-reorder insertion bar, when the overlay has one.
    pub insertion_bar: Option<Rect>,
}

impl DropOverlay {
    fn resolved_layout(&self) -> DropOverlayLayout {
        DropOverlayLayout {
            bounds: self.bounds(),
            highlight: self.highlight,
            insertion_bar: self.insertion_bar,
        }
    }
}

impl crate::a11y::A11yPrimitive for DropOverlay {
    /// Presentational: the overlay previews where a dragged tab lands,
    /// and the drag itself is announced by the tab bar being dragged
    /// from.
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        crate::a11y::A11yInfo::new()
            .with_role(crate::a11y::A11yRole::Presentation)
            .with_label("Drop overlay")
    }
}

impl crate::primitive::Primitive for DropOverlay {
    type Layout = DropOverlayLayout;
    type Event = std::convert::Infallible;

    /// The size of [`DropOverlay::bounds`], in whatever units the
    /// descriptor's geometry was computed in — the overlay is sized by
    /// the drop zone it previews, never by the container.
    fn measure(
        &self,
        _ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let b = self.bounds();
        crate::NaturalSize::new(Some(b.width), Some(b.height))
    }

    /// The descriptor's own absolute rects. `rect` is only the clip the
    /// host paints inside, normally [`DropOverlay::bounds`].
    fn layout(&self, _ctx: &crate::primitive::LayoutCtx, _rect: Rect) -> DropOverlayLayout {
        self.resolved_layout()
    }

    /// The shared pixel painter, against the already-resolved `layout`.
    fn paint(
        &self,
        layout: &DropOverlayLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        native_surface_paint::paint_resolved(layout, surface, theme);
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme);
    }

    /// [`cell_paint::paint`] over `buf`, adapted through
    /// [`crate::tui::cell_surface::TuiCellSurface`].
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &DropOverlayLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        cell_paint::paint(layout, cells, theme);
    }
}

crate::non_interactive!(DropOverlay);

/// The cell-grid painter for [`DropOverlay`], written against
/// [`crate::CellSurface`].
///
/// The highlight re-tints the background of every cell it covers with a
/// lightened `theme.accent_fg`, keeping each cell's glyph and foreground;
/// the insertion bar is a column of `│` in `theme.accent_fg` on
/// `theme.background`.
///
/// The background-only re-tint goes through
/// [`crate::CellSurface::ratatui_buffer`]: every glyph-writing verb
/// replaces the cell's symbol, and a read-back-then-rewrite through
/// [`crate::CellSurface::get`] loses multi-codepoint symbols and the
/// trailing half of a double-width glyph.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::DropOverlayLayout;
    use crate::cell_surface::{solid_style, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;

    /// The highlight's background tint: `theme.accent_fg` lightened so
    /// glyphs painted on top of it stay legible.
    pub(crate) fn highlight_tint(theme: &Theme) -> Color {
        let accent = theme.accent_fg;
        Color::rgb(
            accent.r.saturating_add(30),
            accent.g.saturating_add(30),
            accent.b.saturating_add(60),
        )
    }

    /// Paint `layout` onto `surface`.
    pub(crate) fn paint(layout: &DropOverlayLayout, surface: &mut dyn CellSurface, theme: &Theme) {
        if let Some(h) = layout.highlight {
            let x0 = h.x.round() as u16;
            let y0 = h.y.round() as u16;
            let x1 = (h.x + h.width).round() as u16;
            let y1 = (h.y + h.height).round() as u16;
            let tint_bg = crate::tui::ratatui_color(highlight_tint(theme));
            // CSR: ratatui_buffer escape hatch
            if let Some(buf) = surface.ratatui_buffer() {
                let area = buf.area;
                for y in y0..y1 {
                    for x in x0..x1 {
                        if x >= area.x
                            && y >= area.y
                            && x < area.x + area.width
                            && y < area.y + area.height
                        {
                            buf[(x, y)].set_bg(tint_bg);
                        }
                    }
                }
            }
        }

        if let Some(bar) = layout.insertion_bar {
            let x = bar.x.round() as u16;
            let y0 = bar.y.round() as u16;
            let y1 = (bar.y + bar.height).round() as u16;
            let style = solid_style(theme.accent_fg, theme.background);
            for y in y0..y1 {
                surface.put_char(x, y, '│', style);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::event::Rect;
        use crate::primitives::drop_zone::DropOverlay;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn paint_into(overlay: &DropOverlay, buf: &mut Buffer, theme: &Theme) {
            let mut cursor_sink = None;
            let mut surface = crate::tui::cell_surface::TuiCellSurface::over(
                buf,
                crate::backend::ColorDepth::TrueColor,
                &mut cursor_sink,
            );
            paint(&overlay.resolved_layout(), &mut surface, theme);
        }

        #[test]
        fn highlight_tints_background_and_keeps_glyphs() {
            let theme = Theme::default();
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 4));
            buf[(2, 1)].set_char('x');
            let overlay = DropOverlay {
                highlight: Some(Rect::new(1.0, 1.0, 3.0, 2.0)),
                insertion_bar: None,
                ghost_position: None,
            };
            paint_into(&overlay, &mut buf, &theme);

            let tint = crate::tui::ratatui_color(highlight_tint(&theme));
            for y in 1..3 {
                for x in 1..4 {
                    assert_eq!(buf[(x, y)].bg, tint, "cell ({x},{y}) is tinted");
                }
            }
            assert_eq!(
                buf[(2, 1)].symbol(),
                "x",
                "the glyph under the tint survives"
            );
            assert_ne!(
                buf[(0, 0)].bg,
                tint,
                "cells outside the highlight are untouched"
            );
            assert_ne!(
                buf[(4, 1)].bg,
                tint,
                "the highlight's right edge is exclusive"
            );
        }

        #[test]
        fn highlight_past_the_buffer_edge_is_clipped() {
            let theme = Theme::default();
            let mut buf = Buffer::empty(RRect::new(0, 0, 4, 2));
            let overlay = DropOverlay {
                highlight: Some(Rect::new(2.0, 0.0, 10.0, 10.0)),
                insertion_bar: None,
                ghost_position: None,
            };
            paint_into(&overlay, &mut buf, &theme);
            let tint = crate::tui::ratatui_color(highlight_tint(&theme));
            assert_eq!(buf[(3, 1)].bg, tint);
        }

        #[test]
        fn insertion_bar_paints_a_vertical_rule_in_the_accent() {
            let theme = Theme::default();
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 4));
            let overlay = DropOverlay {
                highlight: None,
                insertion_bar: Some(Rect::new(5.0, 0.0, 1.0, 3.0)),
                ghost_position: None,
            };
            paint_into(&overlay, &mut buf, &theme);
            for y in 0..3 {
                let cell = &buf[(5, y)];
                assert_eq!(cell.symbol(), "│");
                assert_eq!(cell.fg, crate::tui::ratatui_color(theme.accent_fg));
                assert_eq!(cell.bg, crate::tui::ratatui_color(theme.background));
            }
            assert_eq!(
                buf[(5, 3)].symbol(),
                " ",
                "the bar's bottom edge is exclusive"
            );
        }
    }
}

// Shared pixel painter for drop overlays, against
// [`crate::paint_surface::PaintSurface`] on GTK, macOS and Windows. The
// highlight is a real alpha blend through `surface_fill_rect` on every
// backend.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod native_surface_paint {
    use super::{DropOverlay, DropOverlayLayout};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint a resolved [`DropOverlayLayout`] onto `surface`: a
    /// translucent highlight rect at [`DropOverlay::HIGHLIGHT_ALPHA`]
    /// and/or a solid insertion bar widened to at least
    /// [`DropOverlay::MIN_BAR_THICKNESS`], both in `theme.accent_fg`.
    /// `ghost_position` is not painted here; a host that wants a ghost
    /// label draws it itself on top.
    pub(crate) fn paint_resolved(
        layout: &DropOverlayLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        if let Some(h) = layout.highlight {
            if h.width > 0.0 && h.height > 0.0 {
                surface.surface_fill_rect(
                    h,
                    theme
                        .accent_fg
                        .with_alpha(DropOverlay::HIGHLIGHT_ALPHA as f64),
                );
            }
        }

        if let Some(bar) = layout.insertion_bar {
            if bar.height > 0.0 {
                let widened = crate::event::Rect::new(
                    bar.x,
                    bar.y,
                    bar.width.max(DropOverlay::MIN_BAR_THICKNESS),
                    bar.height,
                );
                surface.surface_fill_rect(widened, theme.accent_fg);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Rect, Viewport};
        use crate::types::Color;
        use crate::Image;

        fn paint(overlay: &DropOverlay, surface: &mut dyn PaintSurface, theme: &Theme) {
            paint_resolved(&overlay.resolved_layout(), surface, theme);
        }

        /// Records every `surface_fill_rect` call — mirrors
        /// `primitives::scrollbar`'s `RecordingSurface` test double,
        /// scoped to just the verb this primitive uses, so this test
        /// runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
                (0.0, 0.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            /// Test-only recorder — the painter never calls this verb;
            /// it exists only so `RecordingSurface` satisfies the trait.
            /// Records into the same `fills` list as `surface_fill_rect`
            /// (radius dropped).
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
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

        /// The highlight carries real alpha (`Color::a < 255`) rather
        /// than a tint pre-mixed against the theme background.
        #[test]
        fn highlight_paints_with_real_alpha_not_opaque() {
            let overlay = DropOverlay {
                highlight: Some(Rect::new(10.0, 10.0, 50.0, 30.0)),
                insertion_bar: None,
                ghost_position: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &theme);

            assert_eq!(
                surface.fills.len(),
                1,
                "expected exactly the highlight fill"
            );
            let (rect, color) = surface.fills[0];
            assert_eq!(rect, Rect::new(10.0, 10.0, 50.0, 30.0));
            assert!(
                color.a < 255,
                "highlight fill must carry real alpha, got opaque a={}",
                color.a
            );
        }

        #[test]
        fn insertion_bar_paints_opaque() {
            let overlay = DropOverlay {
                highlight: None,
                insertion_bar: Some(Rect::new(40.0, 0.0, 2.0, 20.0)),
                ghost_position: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &theme);

            assert_eq!(surface.fills.len(), 1, "expected exactly the bar fill");
            let (_, color) = surface.fills[0];
            assert_eq!(
                color, theme.accent_fg,
                "insertion bar is solid theme.accent_fg"
            );
        }

        #[test]
        fn insertion_bar_widens_to_min_thickness() {
            let overlay = DropOverlay {
                highlight: None,
                insertion_bar: Some(Rect::new(40.0, 0.0, 0.0, 20.0)),
                ghost_position: None,
            };
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &Theme::default());

            let (rect, _) = surface.fills[0];
            assert_eq!(rect.width, DropOverlay::MIN_BAR_THICKNESS);
        }

        #[test]
        fn zero_size_highlight_paints_nothing() {
            let overlay = DropOverlay {
                highlight: Some(Rect::new(10.0, 10.0, 0.0, 0.0)),
                insertion_bar: None,
                ghost_position: None,
            };
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &Theme::default());
            assert!(surface.fills.is_empty());
        }

        #[test]
        fn empty_overlay_paints_nothing() {
            let overlay = DropOverlay {
                highlight: None,
                insertion_bar: None,
                ghost_position: None,
            };
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &Theme::default());
            assert!(surface.fills.is_empty());
        }

        #[test]
        fn both_highlight_and_bar_paint_two_fills() {
            let overlay = DropOverlay {
                highlight: Some(Rect::new(0.0, 0.0, 20.0, 20.0)),
                insertion_bar: Some(Rect::new(40.0, 0.0, 2.0, 20.0)),
                ghost_position: None,
            };
            let mut surface = RecordingSurface::default();
            paint(&overlay, &mut surface, &Theme::default());
            assert_eq!(surface.fills.len(), 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(x: f32, y: f32, w: f32, h: f32, slots: &[(f32, f32)]) -> DropGroupRect {
        DropGroupRect {
            bounds: Rect::new(x, y, w, h),
            tab_slots: slots.to_vec(),
        }
    }

    #[test]
    fn a_pane_under_six_units_tall_resolves_to_its_edges() {
        let groups = vec![group(0.0, 0.0, 40.0, 5.0, &[])];
        let top = compute_drop_zone(20.0, 2.0, &groups, 1.0).unwrap();
        assert_eq!(top.kind, DropZoneKind::Split(DropEdge::Top));
        let bottom = compute_drop_zone(20.0, 4.0, &groups, 1.0).unwrap();
        assert_eq!(bottom.kind, DropZoneKind::Split(DropEdge::Bottom));
    }

    #[test]
    fn center_drop() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = compute_drop_zone(50.0, 50.0, &groups, 20.0).unwrap();
        assert_eq!(zone.group_idx, 0);
        assert_eq!(zone.kind, DropZoneKind::Center);
    }

    #[test]
    fn left_edge_split() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = compute_drop_zone(5.0, 50.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::Split(DropEdge::Left));
    }

    #[test]
    fn right_edge_split() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = compute_drop_zone(95.0, 50.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::Split(DropEdge::Right));
    }

    #[test]
    fn top_edge_split() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        // tab_bar_height=20, content starts at y=20, edge_h = 60*0.2 = 12
        let zone = compute_drop_zone(50.0, 25.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::Split(DropEdge::Top));
    }

    #[test]
    fn bottom_edge_split() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = compute_drop_zone(50.0, 75.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::Split(DropEdge::Bottom));
    }

    #[test]
    fn tab_reorder_before_first() {
        let slots = vec![(10.0, 40.0), (40.0, 70.0)];
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &slots)];
        // Cursor at x=15, left of first tab's midpoint (25)
        let zone = compute_drop_zone(15.0, 5.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::TabReorder(0));
    }

    #[test]
    fn tab_reorder_between_tabs() {
        let slots = vec![(10.0, 40.0), (40.0, 70.0)];
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &slots)];
        // Cursor at x=35, right of first midpoint (25), left of second (55)
        let zone = compute_drop_zone(35.0, 5.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::TabReorder(1));
    }

    #[test]
    fn tab_reorder_after_last() {
        let slots = vec![(10.0, 40.0), (40.0, 70.0)];
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &slots)];
        // Cursor at x=60, right of second midpoint (55)
        let zone = compute_drop_zone(60.0, 5.0, &groups, 20.0).unwrap();
        assert_eq!(zone.kind, DropZoneKind::TabReorder(2));
    }

    #[test]
    fn cursor_outside_all_groups() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        assert!(compute_drop_zone(200.0, 200.0, &groups, 20.0).is_none());
    }

    #[test]
    fn hit_test_returns_zone_for_center_drop() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        match drop_zone_hit_test(50.0, 50.0, &groups, 20.0) {
            DropZoneHit::Zone(zone) => {
                assert_eq!(zone.kind, DropZoneKind::Center);
                assert_eq!(zone.group_idx, 0);
            }
            DropZoneHit::Empty => panic!("expected a zone hit"),
        }
    }

    #[test]
    fn hit_test_outside_all_groups_is_empty() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        assert_eq!(
            drop_zone_hit_test(200.0, 200.0, &groups, 20.0),
            DropZoneHit::Empty
        );
    }

    #[test]
    fn multiple_groups_selects_correct() {
        let groups = vec![
            group(0.0, 0.0, 50.0, 80.0, &[]),
            group(50.0, 0.0, 50.0, 80.0, &[]),
        ];
        let zone = compute_drop_zone(75.0, 50.0, &groups, 20.0).unwrap();
        assert_eq!(zone.group_idx, 1);
    }

    #[test]
    fn overlay_center_covers_content_area() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = DropZone {
            kind: DropZoneKind::Center,
            group_idx: 0,
        };
        let ov = drop_zone_overlay(&zone, &groups, 50.0, 50.0, 20.0, 2.0, 10.0);
        assert_eq!(ov.highlight, Some(Rect::new(0.0, 20.0, 100.0, 60.0)));
        assert!(ov.insertion_bar.is_none());
    }

    #[test]
    fn overlay_split_left_covers_left_half() {
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &[])];
        let zone = DropZone {
            kind: DropZoneKind::Split(DropEdge::Left),
            group_idx: 0,
        };
        let ov = drop_zone_overlay(&zone, &groups, 5.0, 50.0, 20.0, 2.0, 10.0);
        assert_eq!(ov.highlight, Some(Rect::new(0.0, 20.0, 50.0, 60.0)));
    }

    #[test]
    fn overlay_tab_reorder_insertion_bar() {
        let slots = vec![(10.0, 40.0), (40.0, 70.0)];
        let groups = vec![group(0.0, 0.0, 100.0, 80.0, &slots)];
        let zone = DropZone {
            kind: DropZoneKind::TabReorder(1),
            group_idx: 0,
        };
        let ov = drop_zone_overlay(&zone, &groups, 35.0, 5.0, 20.0, 2.0, 10.0);
        assert!(ov.highlight.is_none());
        let bar = ov.insertion_bar.unwrap();
        assert_eq!(bar.x, 39.0); // midpoint of prev_end(40) and next_start(40) = 40, minus half thickness
        assert_eq!(bar.width, 2.0);
        assert_eq!(bar.height, 20.0);
    }

    #[test]
    fn bounds_is_the_union_of_highlight_and_widened_insertion_bar() {
        let highlight = Rect::new(1.0, 2.0, 3.0, 4.0);
        let bar = Rect::new(5.0, 6.0, 7.0, 8.0);

        let both = DropOverlay {
            highlight: Some(highlight),
            insertion_bar: Some(bar),
            ghost_position: None,
        };
        assert_eq!(both.bounds(), Rect::new(1.0, 2.0, 11.0, 12.0));

        let highlight_only = DropOverlay {
            highlight: Some(highlight),
            insertion_bar: None,
            ghost_position: None,
        };
        assert_eq!(highlight_only.bounds(), highlight);

        let bar_only = DropOverlay {
            highlight: None,
            insertion_bar: Some(bar),
            ghost_position: None,
        };
        assert_eq!(bar_only.bounds(), bar);

        let hairline_bar = DropOverlay {
            highlight: None,
            insertion_bar: Some(Rect::new(40.0, 0.0, 0.0, 20.0)),
            ghost_position: None,
        };
        assert_eq!(
            hairline_bar.bounds(),
            Rect::new(40.0, 0.0, DropOverlay::MIN_BAR_THICKNESS, 20.0),
            "bounds covers the bar as the pixel painter widens it"
        );

        let neither = DropOverlay {
            highlight: None,
            insertion_bar: None,
            ghost_position: None,
        };
        assert_eq!(neither.bounds(), Rect::default());
    }

    #[test]
    fn layout_and_measure_follow_bounds_whatever_rect_the_host_passes() {
        use crate::primitive::{LayoutCtx, Primitive, Size};

        let overlay = DropOverlay {
            highlight: Some(Rect::new(4.0, 2.0, 10.0, 6.0)),
            insertion_bar: None,
            ghost_position: None,
        };
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let layout = overlay.layout(&ctx, Rect::new(0.0, 0.0, 80.0, 24.0));
        assert_eq!(layout.bounds, overlay.bounds());
        assert_eq!(layout.highlight, overlay.highlight);
        assert_eq!(layout.insertion_bar, None);

        let natural = overlay.measure(&ctx, Size::new(80.0, 24.0));
        assert_eq!(natural.width, Some(10.0));
        assert_eq!(natural.height, Some(6.0));
    }

    #[test]
    fn an_overlay_carries_no_widget_id() {
        use crate::primitive::Primitive;

        let overlay = DropOverlay {
            highlight: Some(Rect::new(0.0, 0.0, 4.0, 4.0)),
            insertion_bar: None,
            ghost_position: None,
        };
        assert!(overlay.id().is_none());
    }
}
