//! `SidebarPanel` primitive: a vertical container with an optional
//! [`Toolbar`] header at the top + a content region beneath that the
//! host paints into.
//!
//! Reserving the header slot, painting the toolbar into it and routing
//! a click to the header before the content are the easy-to-get-wrong
//! parts of a "panel with an action header", so a host gets all three
//! from one descriptor instead of carving the top rect by hand.
//!
//! ## Why "SidebarPanel" and not "Sidebar"
//!
//! The compose layer already exports [`crate::Sidebar`], the
//! multi-section sidebar, and its `SidebarAction`. `SidebarPanel` reads
//! correctly at consumer use sites (each panel IS a sidebar panel) and
//! keeps the names disjoint.
//!
//! ## Shape
//!
//! - `toolbar`:
//!   - `None`: no slot reserved. Content occupies the full rect.
//!   - `Some(bar)`: header slot reserved at `toolbar_height`. Reserved
//!     *even when `bar.buttons.is_empty()`*, so content below doesn't
//!     shift as toolbar items come and go.
//! - `toolbar_height`: optional explicit height in native units. When
//!   `None`, the header is one cell tall on a cell grid and
//!   [`crate::Style::toolbar_height`] (floored at the row height) on a
//!   pixel surface.
//!
//! ## Widget contract
//!
//! [`Primitive::layout`] resolves a [`SidebarPanelLayout`]: the panel,
//! header and content rects in the **absolute** frame the panel was laid
//! out in (the content rect is what a host paints its tree / list / form
//! into), plus the header's own [`ToolbarPrimitiveLayout`] — the exact
//! value [`Toolbar`]'s own contract resolves for the header slot, whose
//! item bounds are header-local. The header slot shares the panel's
//! origin, so header-local coordinates are also panel-local, which is
//! the space [`Interactive::sub_regions`] and
//! [`crate::interactive::RoutedEvent::Pointer`]'s `local` use.
//!
//! The panel paints only its header; the content region is never
//! painted. [`Primitive::paint`] runs this module's shared pixel painter
//! ([`native_surface_paint`]), and [`Primitive::paint_cells`] runs the
//! header toolbar's own cell painter, so a header on a cell grid is
//! byte-for-byte the toolbar it carries.
//!
//! [`Interactive`] is pointer-only ([`FocusSpec::None`]): hover, press
//! and click on the header's buttons go through [`Toolbar`]'s own
//! [`Interactive`] impl, and a click comes back out as the
//! [`crate::UiEvent::Toolbar`] the header toolbar itself would emit. The
//! header's keyboard cursor stays host-driven through
//! [`Toolbar::focused_index`]; an assistive-technology
//! [`A11yAction::Click`] activates it.
//!
//! Hit-test ([`SidebarPanelLayout::hit_test`], absolute coordinates):
//! - Click inside the toolbar slot routes via the nested
//!   [`ToolbarPrimitiveLayout::hit_test`] to a
//!   [`SidebarPanelHit::ToolbarButton`] or, for misses inside the slot,
//!   [`SidebarPanelHit::ToolbarEmpty`].
//! - Click inside the content rect produces
//!   [`SidebarPanelHit::Content`] with content-local coordinates.
//! - Click outside both produces [`SidebarPanelHit::Empty`].

use crate::a11y::A11yAction;
use crate::backend::PointerShape;
use crate::event::{Point, Rect};
use crate::interactive::{
    EventCx, EventHost, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, PaintState, Primitive, Units};
use crate::primitives::toolbar::{
    Toolbar, ToolbarEvent, ToolbarHit, ToolbarPrimitiveLayout, ToolbarState,
};
use crate::types::WidgetId;
use crate::Handled;
use serde::{Deserialize, Serialize};

// ── Data model ───────────────────────────────────────────────────────────────

/// Vertical container: optional header toolbar + content region.
///
/// # Examples
///
/// ```
/// use quadraui::primitive::{LayoutCtx, Primitive};
/// use quadraui::{Rect, SidebarPanel, SidebarPanelHit, WidgetId};
///
/// let panel = SidebarPanel {
///     id: WidgetId::new("sidebar:explorer"),
///     toolbar: None,
///     toolbar_height: None,
/// };
///
/// let bounds = Rect::new(0.0, 0.0, 40.0, 20.0);
/// let layout = panel.layout(&LayoutCtx::new(1.0, 1.0, false), bounds);
///
/// // No toolbar means no slot reserved — content gets the full rect.
/// assert_eq!(layout.content_bounds, bounds);
/// assert_eq!(layout.hit_test(1.0, 1.0), SidebarPanelHit::Content { x: 1.0, y: 1.0 });
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SidebarPanel {
    pub id: WidgetId,
    /// Optional header toolbar. When `None` the slot is **not**
    /// reserved (content gets the full rect). When `Some`, the slot
    /// is always reserved at `toolbar_height` even if `buttons` is
    /// empty — so content below doesn't shift as buttons appear /
    /// disappear.
    #[serde(default)]
    pub toolbar: Option<Toolbar>,
    /// Reserved height for the toolbar slot in native units. When
    /// `None`, the header is one cell on a cell grid and
    /// [`crate::Style::toolbar_height`] on a pixel surface.
    #[serde(default)]
    pub toolbar_height: Option<f32>,
}

// ── Layout + hit-testing ─────────────────────────────────────────────────────

/// [`Primitive::Layout`] for a [`SidebarPanel`].
///
/// The three rects are **absolute** — in the frame the panel was laid
/// out in — so a host paints its content straight into
/// [`Self::content_bounds`]. [`Self::toolbar`] is the header toolbar's
/// own [`ToolbarPrimitiveLayout`]: its `area` is the absolute header
/// slot, its item bounds header-local.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarPanelLayout {
    /// Bounds of the entire panel as given to [`Primitive::layout`].
    pub panel_bounds: Rect,
    /// Bounds of the toolbar slot. `None` when the panel has no
    /// toolbar (slot wasn't reserved). When `Some`, this rect is
    /// reserved even if the toolbar has no buttons.
    pub toolbar_bounds: Option<Rect>,
    /// Bounds of the content area. Always populated — equals the
    /// panel minus the toolbar slot.
    pub content_bounds: Rect,
    /// The header toolbar's resolved layout. `None` mirrors
    /// `toolbar_bounds`.
    pub toolbar: Option<ToolbarPrimitiveLayout>,
}

/// Classification of a hit-test result on a [`SidebarPanel`].
#[derive(Debug, Clone, PartialEq)]
pub enum SidebarPanelHit {
    /// Click landed on a clickable toolbar button — carries its id.
    ToolbarButton(WidgetId),
    /// Click landed inside the toolbar slot but not on a clickable
    /// button (gap, separator, label, or disabled action).
    ToolbarEmpty,
    /// Click landed inside the content area. `(x, y)` are
    /// **content-local** — relative to `content_bounds.x` / `.y`.
    Content { x: f32, y: f32 },
    /// Click missed both the toolbar slot and the content area
    /// (outside the panel entirely).
    Empty,
}

impl SidebarPanelLayout {
    /// Test absolute point `(x, y)` against the layout. The toolbar slot
    /// is checked before the content area.
    pub fn hit_test(&self, x: f32, y: f32) -> SidebarPanelHit {
        if let (Some(tb), Some(toolbar)) = (self.toolbar_bounds, &self.toolbar) {
            if contains(tb, x, y) {
                return match toolbar.hit_test(x - tb.x, y - tb.y) {
                    ToolbarHit::Button(id) => SidebarPanelHit::ToolbarButton(id),
                    ToolbarHit::Empty
                    | ToolbarHit::Overflow
                    | ToolbarHit::Field(_)
                    | ToolbarHit::MenuButton(_) => SidebarPanelHit::ToolbarEmpty,
                };
            }
        }
        if contains(self.content_bounds, x, y) {
            return SidebarPanelHit::Content {
                x: x - self.content_bounds.x,
                y: y - self.content_bounds.y,
            };
        }
        SidebarPanelHit::Empty
    }
}

fn contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// The header height a panel with no explicit `toolbar_height` reserves
/// in `ctx`'s units: one cell on a cell grid, and on a pixel surface the
/// active [`crate::Style::toolbar_height`] floored at `ctx`'s row height,
/// the same chrome metric every pixel rasteriser sizes a toolbar strip
/// with.
fn default_toolbar_height(ctx: &LayoutCtx) -> f32 {
    match ctx.units() {
        Units::Cells => 1.0,
        Units::Pixels => ctx.line_height.max(ctx.style().toolbar_height),
    }
}

// ── Primitive ────────────────────────────────────────────────────────────────

impl Primitive for SidebarPanel {
    type Layout = SidebarPanelLayout;
    type Event = ToolbarEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Round-trips through [`crate::frame::OwnedContent::SidebarPanel`],
    /// which replays through [`crate::DrawsPrimitives::draw_primitive`]
    /// and reports [`crate::frame::FrameZone::Widget`].
    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::SidebarPanel(self.clone())
    }

    /// Carve the header slot off the top of `rect` and resolve the
    /// header toolbar's own [`Primitive::layout`] for it, in whichever
    /// unit `ctx` measures in — see this module's own doc.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let Some(bar) = &self.toolbar else {
            return SidebarPanelLayout {
                panel_bounds: rect,
                toolbar_bounds: None,
                content_bounds: rect,
                toolbar: None,
            };
        };
        let toolbar_height = self
            .toolbar_height
            .unwrap_or_else(|| default_toolbar_height(ctx));
        // Reserved even when `bar.buttons.is_empty()` so content doesn't
        // shift as toolbar items come and go.
        let slot_height = toolbar_height.min(rect.height).max(0.0);
        let toolbar_bounds = Rect::new(rect.x, rect.y, rect.width, slot_height);
        let content_bounds = Rect::new(
            rect.x,
            rect.y + slot_height,
            rect.width,
            (rect.height - slot_height).max(0.0),
        );
        SidebarPanelLayout {
            panel_bounds: rect,
            toolbar_bounds: Some(toolbar_bounds),
            content_bounds,
            toolbar: Some(Primitive::layout(bar, ctx, toolbar_bounds)),
        }
    }

    /// Runs the shared pixel painter at the resolved header geometry,
    /// tinting the header button `env` reports hovered or pressed under
    /// this panel's id (see [`Self::header_interaction`]); the
    /// descriptor's own [`Toolbar::focused_index`] paints its focus ring.
    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let style = env.style();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let (hovered, pressed) = match &self.toolbar {
                Some(bar) => {
                    let interaction = self.header_interaction(bar, env);
                    bar.interaction_ids(&env.with_interaction(&interaction))
                }
                None => (None, None),
            };
            native_surface_paint::paint(
                self,
                layout,
                surface,
                theme,
                hovered.as_ref(),
                pressed.as_ref(),
            );
            let _ = style;
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// Paints the header through the header toolbar's own
    /// [`Primitive::paint_cells`], with the hover and press
    /// [`Self::header_interaction`] re-addresses to it; the content rows
    /// are left untouched for the host.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        if let (Some(bar), Some(toolbar)) = (&self.toolbar, &layout.toolbar) {
            if toolbar.area.width > 0.0 && toolbar.area.height > 0.0 {
                let interaction = self.header_interaction(bar, env);
                Primitive::paint_cells(bar, toolbar, cells, &env.with_interaction(&interaction));
            }
        }
    }
}

impl SidebarPanel {
    /// `env`'s hover and press re-addressed from this panel to its header
    /// `bar`. The painted-widget record names the panel, whose
    /// [`Interactive::sub_regions`] are the header's own, so each
    /// sub-part carries over unchanged.
    #[cfg_attr(
        not(any(
            feature = "tui",
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )),
        allow(dead_code)
    )]
    fn header_interaction(&self, bar: &Toolbar, env: &crate::PaintEnv) -> crate::InteractionState {
        env.child_interaction(&self.id, &bar.id)
    }
}

// ── Interactive ──────────────────────────────────────────────────────────────

/// [`Interactive::State`] for [`SidebarPanel`]: the header toolbar's own
/// [`ToolbarState`] (hovered / pressed button), which every pointer
/// gesture on the header updates.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidebarPanelState {
    /// The header toolbar's interaction state.
    pub toolbar: ToolbarState,
}

impl PaintState for SidebarPanelState {
    fn hovered_idx(&self) -> Option<usize> {
        self.toolbar.hovered_idx()
    }

    fn pressed_idx(&self) -> Option<usize> {
        self.toolbar.pressed_idx()
    }
}

/// [`Interactive::Action`] for [`SidebarPanel`]: an event the header
/// toolbar produced, tagged with that toolbar's own id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarPanelAction {
    /// The header [`Toolbar::id`].
    pub toolbar: WidgetId,
    /// What the header toolbar reported.
    pub event: ToolbarEvent,
}

/// Rides [`crate::UiEvent::Toolbar`] under the header toolbar's own id —
/// the channel a host routing a standalone [`Toolbar`] already handles.
impl WidgetAction for SidebarPanelAction {
    fn into_ui_event(self, _id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Toolbar(self.toolbar, self.event)
    }
}

/// The [`EventHost`] the header toolbar's [`Interactive::handle`] runs
/// against: forwards everything to the panel's own [`EventCx`] except
/// focus, since the panel is not a focus stop.
struct HeaderHost<'a, 'b> {
    panel: &'a mut EventCx<'b>,
}

impl EventHost for HeaderHost<'_, '_> {
    fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
        false
    }
    fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
    fn capture_pointer(&mut self, _id: &WidgetId) {
        self.panel.capture_pointer();
    }
    fn release_pointer(&mut self) {
        self.panel.release_pointer();
    }
    fn set_cursor(&mut self, cursor: PointerShape) {
        self.panel.set_cursor(cursor);
    }
    fn request_redraw(&mut self) {
        self.panel.request_redraw();
    }
    fn emit(&mut self, event: crate::UiEvent) {
        self.panel.emit(event);
    }
    fn units(&self) -> Units {
        self.panel.units()
    }
}

/// Whether panel-local `local` lies inside the header slot `header`
/// (which starts at the panel's own origin).
fn in_header(header: &ToolbarPrimitiveLayout, local: Point) -> bool {
    local.x >= 0.0 && local.y >= 0.0 && local.x < header.area.width && local.y < header.area.height
}

impl Interactive for SidebarPanel {
    type State = SidebarPanelState;
    type Action = SidebarPanelAction;

    /// Pointer-only: the header's keyboard cursor is host-driven through
    /// [`Toolbar::focused_index`], so the panel is never a tab stop.
    fn focus(&self) -> FocusSpec {
        FocusSpec::None
    }

    /// The header toolbar's own sub-parts — header-local, which is
    /// panel-local because the header shares the panel's origin.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, SubTarget)> {
        match (&self.toolbar, &layout.toolbar) {
            (Some(bar), Some(toolbar)) => Interactive::sub_regions(bar, toolbar),
            _ => Vec::new(),
        }
    }

    /// Routes header gestures through the header toolbar's own
    /// [`Interactive::handle`]: a pointer that moves into the content
    /// area reads as leaving the header, and a press in the content area
    /// is not the panel's to claim.
    fn handle(
        &self,
        state: &mut SidebarPanelState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<SidebarPanelAction> {
        let (Some(bar), Some(header)) = (&self.toolbar, &layout.toolbar) else {
            return Handled::Ignored;
        };
        let routed = match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Move | PointerKind::Enter,
                sub,
                local,
                mods,
            } if !in_header(header, *local) => RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                sub: *sub,
                local: *local,
                mods: *mods,
            },
            RoutedEvent::Pointer {
                kind: PointerKind::Down(_) | PointerKind::DoubleClick(_),
                local,
                ..
            } if !in_header(header, *local) => return Handled::Ignored,
            RoutedEvent::Pointer { .. } | RoutedEvent::Action(_) => event.clone(),
            _ => return Handled::Ignored,
        };
        let mut host = HeaderHost { panel: cx };
        let mut header_cx = EventCx::new(bar.id, &mut host);
        Interactive::handle(bar, &mut state.toolbar, &routed, header, &mut header_cx).map(|event| {
            SidebarPanelAction {
                toolbar: bar.id,
                event,
            }
        })
    }

    /// `Click` whenever the header carries something to activate.
    fn a11y_actions(&self, state: &SidebarPanelState) -> Vec<A11yAction> {
        match &self.toolbar {
            Some(bar) => Interactive::a11y_actions(bar, &state.toolbar),
            None => Vec::new(),
        }
    }
}

// Shared pixel painter for all backends, written against
// [`crate::paint_surface::PaintSurface`] instead of individual
// backend drawing APIs.
//
// `SidebarPanel` paints nothing of its own — its entire visible
// surface is the optional [`Toolbar`] header; the content region
// is left to the host. `paint_toolbar_header` below paints the header
// scoped to what a sidebar panel's header needs:
//
// 1. **Plain rect highlights**, not rounded.
// 2. **Clipped** to the header slot.
// 3. **Alpha-channel preserved**: uses `surface_fill_rect`, which
//    respects `Color::a`.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod native_surface_paint {
    use super::{SidebarPanel, SidebarPanelLayout};
    use crate::event::{Point, Rect};
    use crate::paint_surface::PaintSurface;
    use crate::primitives::toolbar::{action_text, Toolbar, ToolbarButton, ToolbarPrimitiveLayout};
    use crate::theme::Theme;
    use crate::types::WidgetId;

    /// Paint `panel`'s header at `layout` (as [`super::Primitive::layout`]
    /// resolved it) on `surface`. The content region is never painted —
    /// the host paints `layout.content_bounds` itself.
    ///
    /// `hovered_toolbar_id` / `pressed_toolbar_id` tint the matching
    /// toolbar action button; the primitive carries no mouse state.
    ///
    /// Non-positive panel width or height short-circuits without
    /// touching `surface`.
    pub(crate) fn paint(
        panel: &SidebarPanel,
        layout: &SidebarPanelLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        hovered_toolbar_id: Option<&WidgetId>,
        pressed_toolbar_id: Option<&WidgetId>,
    ) {
        let bounds = layout.panel_bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }
        if let (Some(bar), Some(header)) = (&panel.toolbar, &layout.toolbar) {
            paint_toolbar_header(
                bar,
                header,
                surface,
                theme,
                hovered_toolbar_id,
                pressed_toolbar_id,
            );
        }
    }

    /// Paint `bar`'s buttons into `header.area` on `surface`, at the item
    /// bounds `header` resolved (header-local, so each is offset by the
    /// header's own origin here).
    fn paint_toolbar_header(
        bar: &Toolbar,
        header: &ToolbarPrimitiveLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) {
        let tb_bounds = header.area;
        surface.surface_push_clip(tb_bounds);

        let bar_bg = bar.bg.unwrap_or(theme.header_bg);
        surface.surface_fill_rect(tb_bounds, bar_bg);

        for vis in &header.layout.visible_items {
            let item = Rect::new(
                vis.bounds.x + tb_bounds.x,
                vis.bounds.y + tb_bounds.y,
                vis.bounds.width,
                vis.bounds.height,
            );
            if item.width <= 0.0 || item.height <= 0.0 {
                continue;
            }
            let Some(btn) = bar.buttons.get(vis.item_idx) else {
                continue;
            };

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
                        surface.surface_fill_rect(inset, bg);
                    }

                    // Focus ring: only when not already visually
                    // dominated by hover/pressed/active.
                    if is_focused && !is_hovered && !is_pressed && !*is_active {
                        let ring = Rect::new(
                            item.x + 1.5,
                            item.y + 1.5,
                            (item.width - 3.0).max(0.0),
                            (item.height - 3.0).max(0.0),
                        );
                        surface.surface_stroke_rect(ring, theme.accent_fg, 1.0);
                    }

                    let fg = if !*enabled {
                        theme.muted_fg
                    } else if is_hovered {
                        theme.hover_fg
                    } else {
                        theme.foreground
                    };

                    let text = action_text(label, icon.as_deref(), key_hint.as_deref());
                    let (tw, th) = surface.surface_measure_text(&text);
                    let tx = item.x + (item.width - tw) / 2.0;
                    let ty = item.y + (item.height - th) / 2.0;
                    surface.surface_draw_text_run(Rect::new(tx, ty, tw, th), &text, fg);
                }
                ToolbarButton::Separator => {
                    let mid_x = item.x + item.width / 2.0;
                    let pad_y = (item.height * 0.2).max(2.0);
                    surface.surface_draw_line(
                        Point::new(mid_x, item.y + pad_y),
                        Point::new(mid_x, item.y + item.height - pad_y),
                        theme.muted_fg,
                        1.0,
                    );
                }
                ToolbarButton::Label { text, fg } => {
                    let color = fg.unwrap_or(theme.muted_fg);
                    let (tw, th) = surface.surface_measure_text(text);
                    let ty = item.y + (item.height - th) / 2.0;
                    surface.surface_draw_text_run(Rect::new(item.x, ty, tw, th), text, color);
                }
                // Invisible by definition — see `ToolbarButton::Spacer`'s
                // own doc.
                ToolbarButton::Spacer => {}
                // A sidebar panel header with a segmented `Group`,
                // embedded `Field`, or `MenuButton` paints only the
                // header background for those slots.
                ToolbarButton::Group { .. }
                | ToolbarButton::Field { .. }
                | ToolbarButton::MenuButton { .. } => {}
            }
        }

        surface.surface_pop_clip();
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitive::{LayoutCtx, Primitive};
        use crate::types::Color;
        use crate::Image;

        /// Records every surface verb the header painter calls, so these
        /// tests run on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            text_runs: Vec<(Rect, String, Color)>,
            lines: Vec<(Point, Point, Color)>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
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
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            /// Records into the same `fills` list as `surface_fill_rect`
            /// (radius dropped); `paint` never calls it.
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
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                from: Point,
                to: Point,
                color: Color,
                _stroke_width: f32,
            ) {
                self.lines.push((from, to, color));
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clip_pushes.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clip_pops += 1;
            }
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn mk_action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
            ToolbarButton::Action {
                spec: None,
                id: WidgetId::new(id),
                label: label.into(),
                icon: None,
                key_hint: None,
                enabled,
                is_active: false,
                tooltip: String::new(),
            }
        }

        fn panel_of(buttons: Vec<ToolbarButton>, focused_index: Option<usize>) -> SidebarPanel {
            SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: Some(Toolbar {
                    id: WidgetId::new("sb:toolbar"),
                    buttons,
                    bg: None,
                    focused_index,
                    ..Default::default()
                }),
                toolbar_height: None,
            }
        }

        fn panel_with_toolbar() -> SidebarPanel {
            panel_of(
                vec![mk_action("a", "Refine", true), mk_action("b", "Drop", true)],
                None,
            )
        }

        /// Pixel metrics matching `RecordingSurface`'s own 8px-per-char
        /// measure, so layout and paint agree on every item width.
        fn paint_at(
            panel: &SidebarPanel,
            bounds: Rect,
            hovered: Option<&WidgetId>,
            pressed: Option<&WidgetId>,
        ) -> (RecordingSurface, SidebarPanelLayout) {
            let layout = Primitive::layout(panel, &LayoutCtx::new(16.0, 8.0, false), bounds);
            let mut surface = RecordingSurface::default();
            paint(
                panel,
                &layout,
                &mut surface,
                &Theme::default(),
                hovered,
                pressed,
            );
            (surface, layout)
        }

        const BOUNDS: Rect = Rect::new(0.0, 0.0, 100.0, 50.0);

        #[test]
        fn no_toolbar_paints_nothing() {
            let panel = SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: None,
                toolbar_height: None,
            };
            let (surface, layout) = paint_at(&panel, BOUNDS, None, None);
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
            assert_eq!(layout.content_bounds, BOUNDS);
        }

        #[test]
        fn zero_size_bounds_paints_nothing() {
            let (surface, _) = paint_at(
                &panel_with_toolbar(),
                Rect::new(0.0, 0.0, 0.0, 0.0),
                None,
                None,
            );
            assert!(surface.fills.is_empty());
            assert!(surface.clip_pushes.is_empty());
        }

        #[test]
        fn toolbar_header_fills_bar_background_and_clips() {
            let theme = Theme::default();
            let (surface, layout) = paint_at(&panel_with_toolbar(), BOUNDS, None, None);
            let tb = layout.toolbar_bounds.expect("toolbar slot reserved");
            assert_eq!(surface.clip_pushes, vec![tb]);
            assert_eq!(surface.clip_pops, 1);
            assert_eq!(surface.fills[0], (tb, theme.header_bg));
        }

        #[test]
        fn hovered_button_gets_hover_bg_and_hover_fg() {
            let theme = Theme::default();
            let (surface, _) = paint_at(
                &panel_with_toolbar(),
                BOUNDS,
                Some(&WidgetId::new("a")),
                None,
            );
            // fills[0] = bar background, fills[1] = hovered button's
            // highlight inset.
            assert_eq!(surface.fills[1].1, theme.hover_bg);
            let (_, _text, color) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "Refine")
                .expect("Refine label drawn");
            assert_eq!(*color, theme.hover_fg);
        }

        #[test]
        fn pressed_button_gets_selected_bg() {
            let (surface, _) = paint_at(
                &panel_with_toolbar(),
                BOUNDS,
                None,
                Some(&WidgetId::new("a")),
            );
            assert_eq!(surface.fills[1].1, Theme::default().selected_bg);
        }

        #[test]
        fn disabled_action_paints_muted_fg_no_highlight() {
            let panel = panel_of(vec![mk_action("a", "Refine", false)], None);
            // Hover on a disabled button is a no-op.
            let (surface, _) = paint_at(&panel, BOUNDS, Some(&WidgetId::new("a")), None);
            assert_eq!(surface.fills.len(), 1);
            let (_, _, color) = &surface.text_runs[0];
            assert_eq!(*color, Theme::default().muted_fg);
        }

        #[test]
        fn focused_button_gets_stroke_ring_when_not_hovered_or_pressed() {
            let panel = panel_of(vec![mk_action("a", "Refine", true)], Some(0));
            let (surface, _) = paint_at(&panel, BOUNDS, None, None);
            assert_eq!(surface.strokes.len(), 1);
            assert_eq!(surface.strokes[0].1, Theme::default().accent_fg);
        }

        #[test]
        fn separator_draws_a_vertical_line_not_a_fill() {
            let panel = panel_of(
                vec![mk_action("a", "Refine", true), ToolbarButton::Separator],
                None,
            );
            let (surface, _) = paint_at(&panel, BOUNDS, None, None);
            assert_eq!(surface.lines.len(), 1);
            assert_eq!(surface.lines[0].2, Theme::default().muted_fg);
        }

        #[test]
        fn label_draws_left_aligned_unhighlighted_text() {
            let panel = panel_of(
                vec![ToolbarButton::Label {
                    text: "2 of 5".into(),
                    fg: None,
                }],
                None,
            );
            let (surface, layout) = paint_at(&panel, BOUNDS, None, None);
            let tb = layout.toolbar_bounds.unwrap();
            assert_eq!(surface.fills.len(), 1);
            let (rect, text, color) = &surface.text_runs[0];
            assert_eq!(text, "2 of 5");
            assert_eq!(rect.x, tb.x);
            assert_eq!(*color, Theme::default().muted_fg);
        }

        /// Item bounds are header-local in the layout; the painter
        /// offsets them by the header's own origin.
        #[test]
        fn a_panel_at_a_nonzero_origin_paints_its_buttons_inside_its_header() {
            let bounds = Rect::new(40.0, 30.0, 100.0, 50.0);
            let (surface, layout) = paint_at(&panel_with_toolbar(), bounds, None, None);
            let tb = layout.toolbar_bounds.unwrap();
            let (rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "Refine")
                .expect("Refine label drawn");
            assert!(rect.x >= tb.x && rect.x + rect.width <= tb.x + tb.width);
            assert!(rect.y >= tb.y && rect.y <= tb.y + tb.height);
        }

        #[test]
        fn content_bounds_still_start_below_toolbar_slot() {
            let (_, layout) = paint_at(&panel_with_toolbar(), BOUNDS, None, None);
            let tb = layout.toolbar_bounds.unwrap();
            assert_eq!(layout.content_bounds.y, tb.y + tb.height);
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, MouseButton, NamedKey};
    use crate::interactive::KeyEvent;
    use crate::primitives::toolbar::{ToolbarButton, ToolbarSubPart};
    use crate::types::Modifiers;

    fn mk_action(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
            spec: None,
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn panel_with_toolbar() -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![mk_action("a", "Refine"), mk_action("b", "Drop")],
                bg: None,
                focused_index: None,
                ..Default::default()
            }),
            toolbar_height: None,
        }
    }

    /// Whole-cell metrics: [`LayoutCtx::units`] reports [`Units::Cells`].
    fn cells() -> LayoutCtx<'static> {
        LayoutCtx::new(1.0, 1.0, false)
    }

    /// Pixel metrics with the default [`crate::Style`].
    fn pixels() -> LayoutCtx<'static> {
        LayoutCtx::new(16.0, 8.0, false)
    }

    /// Header-local bounds of the header item at `idx` in `layout`.
    fn item_bounds(layout: &SidebarPanelLayout, idx: usize) -> Rect {
        layout
            .toolbar
            .as_ref()
            .expect("toolbar laid out")
            .layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == idx)
            .expect("item laid out")
            .bounds
    }

    #[test]
    fn no_toolbar_gives_full_rect_to_content() {
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: None,
            toolbar_height: None,
        };
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        assert!(layout.toolbar_bounds.is_none());
        assert!(layout.toolbar.is_none());
        assert_eq!(layout.content_bounds, Rect::new(0.0, 0.0, 30.0, 10.0));
    }

    #[test]
    fn toolbar_reserves_one_cell_on_a_cell_grid() {
        let layout = panel_with_toolbar().layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        let tb = layout.toolbar_bounds.expect("toolbar slot reserved");
        assert_eq!(tb, Rect::new(0.0, 0.0, 30.0, 1.0));
        assert_eq!(layout.content_bounds, Rect::new(0.0, 1.0, 30.0, 9.0));
        assert_eq!(layout.toolbar.as_ref().map(|t| t.area), Some(tb));
    }

    #[test]
    fn toolbar_reserves_the_style_toolbar_height_on_a_pixel_surface() {
        let style = crate::Style::default().with_toolbar_height(30.0);
        let ctx = pixels().with_style(&style);
        let layout = panel_with_toolbar().layout(&ctx, Rect::new(0.0, 0.0, 200.0, 100.0));
        assert_eq!(layout.toolbar_bounds.unwrap().height, 30.0);
        assert_eq!(layout.content_bounds.y, 30.0);

        // Floored at the row height when the style token is shorter.
        let short = crate::Style::default().with_toolbar_height(4.0);
        let ctx = pixels().with_style(&short);
        let layout = panel_with_toolbar().layout(&ctx, Rect::new(0.0, 0.0, 200.0, 100.0));
        assert_eq!(layout.toolbar_bounds.unwrap().height, 16.0);
    }

    #[test]
    fn empty_toolbar_still_reserves_slot() {
        // Content doesn't shift when the toolbar's buttons vector is
        // temporarily empty.
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![],
                bg: None,
                focused_index: None,
                ..Default::default()
            }),
            toolbar_height: None,
        };
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        assert!(layout.toolbar_bounds.is_some());
        assert_eq!(layout.content_bounds.y, 1.0);
        assert_eq!(layout.content_bounds.height, 9.0);
    }

    #[test]
    fn explicit_toolbar_height_overrides_default() {
        let mut panel = panel_with_toolbar();
        panel.toolbar_height = Some(3.0);
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        assert_eq!(layout.toolbar_bounds.unwrap().height, 3.0);
        assert_eq!(layout.content_bounds.y, 3.0);
        assert_eq!(layout.content_bounds.height, 7.0);
    }

    #[test]
    fn toolbar_height_clamps_to_panel_height() {
        let mut panel = panel_with_toolbar();
        panel.toolbar_height = Some(50.0);
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        assert_eq!(layout.toolbar_bounds.unwrap().height, 10.0);
        assert_eq!(layout.content_bounds.height, 0.0);
    }

    /// Item bounds are header-local whatever the panel's own origin is.
    #[test]
    fn header_items_are_header_local_at_a_nonzero_origin() {
        let layout = panel_with_toolbar().layout(&cells(), Rect::new(7.0, 13.0, 30.0, 10.0));
        let first = item_bounds(&layout, 0);
        assert_eq!((first.x, first.y), (0.0, 0.0));
    }

    fn hit_test_round_trip_at(origin_x: f32, origin_y: f32) {
        for ctx in [cells(), pixels()] {
            let bounds = Rect::new(origin_x, origin_y, 300.0, 200.0);
            let layout = panel_with_toolbar().layout(&ctx, bounds);
            let tb = layout.toolbar_bounds.unwrap();
            let refine = item_bounds(&layout, 0);
            assert_eq!(
                layout.hit_test(tb.x + refine.x + 0.5, tb.y),
                SidebarPanelHit::ToolbarButton(WidgetId::new("a")),
            );
            let drop = item_bounds(&layout, 1);
            assert_eq!(
                layout.hit_test(tb.x + drop.x + 0.5, tb.y),
                SidebarPanelHit::ToolbarButton(WidgetId::new("b")),
            );
            // Past the last button but still inside the slot.
            assert_eq!(
                layout.hit_test(tb.x + tb.width - 1.0, tb.y),
                SidebarPanelHit::ToolbarEmpty,
            );
            // Content-local coordinates below the slot.
            let c = layout.content_bounds;
            assert_eq!(
                layout.hit_test(c.x + 2.0, c.y + 2.0),
                SidebarPanelHit::Content { x: 2.0, y: 2.0 },
            );
            assert_eq!(
                layout.hit_test(origin_x - 1.0, origin_y - 1.0),
                SidebarPanelHit::Empty
            );
        }
    }

    #[test]
    fn hit_test_routes_header_and_content_at_the_origin() {
        hit_test_round_trip_at(0.0, 0.0);
    }

    #[test]
    fn hit_test_routes_header_and_content_at_a_nonzero_origin() {
        hit_test_round_trip_at(7.0, 13.0);
    }

    #[test]
    fn serde_roundtrip() {
        let panel = panel_with_toolbar();
        let json = serde_json::to_string(&panel).unwrap();
        let back: SidebarPanel = serde_json::from_str(&json).unwrap();
        assert_eq!(panel, back);
    }

    // ── Interactive ─────────────────────────────────────────────────

    /// Minimal [`EventHost`] mock recording what a `handle` asked for.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: usize,
        captures: usize,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {
            self.focus_requests += 1;
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {
            self.captures += 1;
        }
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn pointer(kind: PointerKind, x: f32, y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point { x, y },
            mods: Modifiers::default(),
        }
    }

    fn handle(
        panel: &SidebarPanel,
        state: &mut SidebarPanelState,
        layout: &SidebarPanelLayout,
        host: &mut MockHost,
        event: RoutedEvent,
    ) -> Handled<SidebarPanelAction> {
        let mut cx = EventCx::new(panel.id, host);
        Interactive::handle(panel, state, &event, layout, &mut cx)
    }

    fn clicked(id: &str) -> SidebarPanelAction {
        SidebarPanelAction {
            toolbar: WidgetId::new("sb:toolbar"),
            event: ToolbarEvent::ButtonClicked {
                id: WidgetId::new(id),
                is_active: true,
            },
        }
    }

    #[test]
    fn a_header_click_emits_the_header_toolbars_own_event() {
        for ctx in [cells(), pixels()] {
            let panel = panel_with_toolbar();
            let layout = panel.layout(&ctx, Rect::new(5.0, 5.0, 300.0, 200.0));
            let drop = item_bounds(&layout, 1);
            let mut state = SidebarPanelState::default();
            let mut host = MockHost::default();

            let out = handle(
                &panel,
                &mut state,
                &layout,
                &mut host,
                pointer(PointerKind::Down(MouseButton::Left), drop.x + 0.5, 0.0),
            );
            assert_eq!(out, Handled::Event(clicked("b")));
            assert_eq!(state.pressed_idx(), Some(1));
            assert_eq!(host.focus_requests, 0, "the panel is not a focus stop");

            assert!(handle(
                &panel,
                &mut state,
                &layout,
                &mut host,
                pointer(PointerKind::Up(MouseButton::Left), drop.x + 0.5, 0.0),
            )
            .is_consumed());
            assert_eq!(state.pressed_idx(), None);
        }
    }

    #[test]
    fn the_action_rides_ui_event_toolbar_under_the_header_id() {
        assert_eq!(
            clicked("a").into_ui_event(WidgetId::new("sb")),
            crate::UiEvent::Toolbar(
                WidgetId::new("sb:toolbar"),
                ToolbarEvent::ButtonClicked {
                    id: WidgetId::new("a"),
                    is_active: true,
                },
            ),
        );
    }

    #[test]
    fn a_press_in_the_content_area_is_not_claimed() {
        let panel = panel_with_toolbar();
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        let mut state = SidebarPanelState::default();
        let mut host = MockHost::default();
        assert!(handle(
            &panel,
            &mut state,
            &layout,
            &mut host,
            pointer(PointerKind::Down(MouseButton::Left), 1.0, 4.0),
        )
        .is_ignored());
    }

    #[test]
    fn hover_tracks_the_header_and_clears_on_moving_into_the_content() {
        let panel = panel_with_toolbar();
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        let mut state = SidebarPanelState::default();
        let mut host = MockHost::default();

        assert!(handle(
            &panel,
            &mut state,
            &layout,
            &mut host,
            pointer(PointerKind::Enter, 1.0, 0.0)
        )
        .is_consumed());
        assert_eq!(state.hovered_idx(), Some(0));
        assert_eq!(state.toolbar.hovered_id, Some(WidgetId::new("a")));

        assert!(handle(
            &panel,
            &mut state,
            &layout,
            &mut host,
            pointer(PointerKind::Move, 1.0, 5.0)
        )
        .is_consumed());
        assert_eq!(state.hovered_idx(), None);
    }

    #[test]
    fn keys_are_never_the_panels_and_it_is_no_focus_stop() {
        let panel = panel_with_toolbar();
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        let mut state = SidebarPanelState::default();
        let mut host = MockHost::default();
        assert_eq!(Interactive::focus(&panel), FocusSpec::None);
        assert!(handle(
            &panel,
            &mut state,
            &layout,
            &mut host,
            RoutedEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Enter),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
        )
        .is_ignored());
    }

    #[test]
    fn the_a11y_click_activates_the_first_header_button() {
        let panel = panel_with_toolbar();
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        let mut state = SidebarPanelState::default();
        let mut host = MockHost::default();
        assert_eq!(
            Interactive::a11y_actions(&panel, &state),
            vec![A11yAction::Click]
        );
        assert_eq!(
            handle(
                &panel,
                &mut state,
                &layout,
                &mut host,
                RoutedEvent::Action(A11yAction::Click),
            ),
            Handled::Event(clicked("a")),
        );
    }

    /// Three-way event parity's in-crate leg: an [`A11yAction::Click`]
    /// routed through [`crate::a11y::route_widget_action`] produces the
    /// same [`crate::UiEvent`] as a direct [`Interactive::handle`] call
    /// with the same action, and as a mouse click on the first header
    /// button. The `TuiDriver` mouse leg lives in
    /// `tests/widget_contract_sidebar_panel.rs`, the binding-host leg in
    /// `quadraui-binding-core`'s engine tests.
    #[test]
    fn route_widget_action_click_matches_a_direct_handle_and_a_mouse_click() {
        let panel = panel_with_toolbar();
        let area = Rect::new(0.0, 0.0, 30.0, 10.0);
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(panel.id, area, Box::new(panel.clone()));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let via_a11y = crate::a11y::route_widget_action(
            &tree,
            &mut states,
            &cells(),
            panel.id,
            A11yAction::Click,
        );

        let layout = panel.layout(&cells(), area);
        let mut host = MockHost::default();
        let direct = handle(
            &panel,
            &mut SidebarPanelState::default(),
            &layout,
            &mut host,
            RoutedEvent::Action(A11yAction::Click),
        );
        let refine = item_bounds(&layout, 0);
        let via_mouse = handle(
            &panel,
            &mut SidebarPanelState::default(),
            &layout,
            &mut host,
            pointer(PointerKind::Down(MouseButton::Left), refine.x + 0.5, 0.0),
        );

        let expected = vec![clicked("a").into_ui_event(panel.id)];
        assert_eq!(via_a11y, expected);
        for outcome in [direct, via_mouse] {
            match outcome {
                Handled::Event(action) => {
                    assert_eq!(vec![action.into_ui_event(panel.id)], expected)
                }
                other => panic!("expected an event, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_panel_without_a_header_has_no_behaviour() {
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: None,
            toolbar_height: None,
        };
        let layout = panel.layout(&cells(), Rect::new(0.0, 0.0, 30.0, 10.0));
        let mut state = SidebarPanelState::default();
        let mut host = MockHost::default();
        assert!(Interactive::a11y_actions(&panel, &state).is_empty());
        assert!(Interactive::sub_regions(&panel, &layout).is_empty());
        assert!(handle(
            &panel,
            &mut state,
            &layout,
            &mut host,
            pointer(PointerKind::Down(MouseButton::Left), 1.0, 0.0),
        )
        .is_ignored());
    }

    #[test]
    fn sub_regions_are_the_header_buttons_in_panel_local_space() {
        let panel = panel_with_toolbar();
        let layout = panel.layout(&cells(), Rect::new(4.0, 6.0, 30.0, 10.0));
        let regions = Interactive::sub_regions(&panel, &layout);
        let (rect, _) = regions
            .iter()
            .find(|(_, sub)| {
                ToolbarSubPart::from_sub_target(*sub) == Some(ToolbarSubPart::Button(0))
            })
            .expect("the first button is addressable");
        assert_eq!((rect.x, rect.y), (0.0, 0.0));
    }

    #[test]
    fn decodes_through_widget_kinds() {
        let widget = crate::widget_kinds::decode_widget(
            "sidebar_panel",
            serde_json::json!({"id": "sb", "toolbar": null}),
        )
        .unwrap_or_else(|err| panic!("sidebar_panel decodes: {err}"));
        assert_eq!(widget.id().map(|id| id.as_str()), Some("sb"));
    }
}

/// The cell rasteriser: a header paints as its toolbar does on a cell
/// grid, and the content rows are left for the host.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use super::*;
    use crate::primitives::toolbar::ToolbarButton;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn panel_with_toolbar() -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("refine"),
                    label: "Refine".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                }],
                bg: None,
                focused_index: None,
                ..Default::default()
            }),
            toolbar_height: None,
        }
    }

    fn paint(panel: &SidebarPanel, area: Rect, buf: &mut Buffer) -> SidebarPanelLayout {
        let layout = panel.layout(&LayoutCtx::new(1.0, 1.0, false), area);
        panel.paint_cells(
            &layout,
            &mut crate::tui::BufferCellSurface::new(buf),
            &crate::PaintEnv::new(&Theme::default(), &crate::Style::default()),
        );
        layout
    }

    #[test]
    fn paints_toolbar_in_top_row() {
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 10));
        let layout = paint(
            &panel_with_toolbar(),
            Rect::new(0.0, 0.0, 40.0, 10.0),
            &mut buf,
        );
        assert_eq!(cell_char(&buf, 0, 0), '[');
        // Content begins at row 1 and is left blank (host paints).
        assert_eq!(cell_char(&buf, 0, 1), ' ');
        assert_eq!(layout.content_bounds.y, 1.0);
        assert_eq!(layout.content_bounds.height, 9.0);
    }

    /// Hover and press addressed to the panel reach its header toolbar:
    /// the header paints exactly as the toolbar does on its own when the
    /// same sub-part is addressed to the toolbar's id.
    #[test]
    fn panel_addressed_hover_and_press_tint_the_header_button() {
        use crate::primitives::toolbar::ToolbarSubPart;

        let panel = panel_with_toolbar();
        let bar = panel.toolbar.clone().expect("panel has a header toolbar");
        let theme = Theme::default();
        let style = crate::Style::default();
        let area = Rect::new(0.0, 0.0, 40.0, 10.0);
        let layout = panel.layout(&LayoutCtx::new(1.0, 1.0, false), area);
        let header = layout.toolbar.clone().expect("header layout");
        let sub = Some(ToolbarSubPart::Button(0).sub_target());
        let blank = || Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 10));
        let panel_buf = |interaction: crate::InteractionState| {
            let mut buf = blank();
            panel.paint_cells(
                &layout,
                &mut crate::tui::BufferCellSurface::new(&mut buf),
                &crate::PaintEnv::new(&theme, &style).with_interaction(&interaction),
            );
            buf
        };
        let bar_buf = |interaction: crate::InteractionState| {
            let mut buf = blank();
            Primitive::paint_cells(
                &bar,
                &header,
                &mut crate::tui::BufferCellSurface::new(&mut buf),
                &crate::PaintEnv::new(&theme, &style).with_interaction(&interaction),
            );
            buf
        };
        let on_panel = Some((panel.id, sub));
        let on_bar = Some((bar.id, sub));

        let at_rest = panel_buf(crate::InteractionState::new());
        let hovered = panel_buf(crate::InteractionState::from_targets(on_panel, None));
        assert_ne!(hovered, at_rest, "panel hover must tint the header button");
        assert_eq!(
            hovered,
            bar_buf(crate::InteractionState::from_targets(on_bar, None))
        );
        assert_eq!(
            panel_buf(crate::InteractionState::from_targets(on_panel, on_panel)),
            bar_buf(crate::InteractionState::from_targets(on_bar, on_bar))
        );
    }

    /// Paint and click round-trip at a non-zero origin too.
    fn click_in_header_round_trip_at(origin_x: u16, origin_y: u16) {
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 30));
        let layout = paint(
            &panel_with_toolbar(),
            Rect::new(origin_x as f32, origin_y as f32, 40.0, 10.0),
            &mut buf,
        );
        assert_eq!(cell_char(&buf, origin_x, origin_y), '[');
        match layout.hit_test(origin_x as f32 + 2.0, origin_y as f32) {
            SidebarPanelHit::ToolbarButton(id) => assert_eq!(id.as_str(), "refine"),
            other => panic!("expected ToolbarButton, got {other:?}"),
        }
    }

    #[test]
    fn click_in_header_resolves_to_toolbar_button() {
        click_in_header_round_trip_at(0, 0);
    }

    #[test]
    fn click_in_header_resolves_to_toolbar_button_at_nonzero_origin() {
        click_in_header_round_trip_at(7, 13);
    }

    #[test]
    fn no_toolbar_paints_nothing() {
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: None,
            toolbar_height: None,
        };
        let mut buf = Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 10));
        let layout = paint(&panel, Rect::new(0.0, 0.0, 40.0, 10.0), &mut buf);
        assert!(layout.toolbar_bounds.is_none());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert_eq!(layout.content_bounds.height, 10.0);
    }
}

/// The header painted through a real macOS backend via
/// [`crate::DrawsPrimitives::draw_dyn`], read back off a bitmap.
#[cfg(all(test, feature = "macos", target_os = "macos"))]
mod macos_paint_tests {
    use super::*;
    use crate::event::Viewport;
    use crate::macos::headless::BitmapSurface;
    use crate::macos::text::make_font;
    use crate::macos::MacBackend;
    use crate::primitives::toolbar::ToolbarButton;
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 200;

    fn action(id: &str, label: &str, icon: Option<&str>) -> ToolbarButton {
        ToolbarButton::Action {
            spec: None,
            id: WidgetId::new(id),
            label: label.into(),
            icon: icon.map(Into::into),
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn panel_of(button: ToolbarButton) -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![button],
                bg: None,
                focused_index: None,
                ..Default::default()
            }),
            toolbar_height: None,
        }
    }

    fn paint_via_backend(
        backend: &mut MacBackend,
        panel: &SidebarPanel,
    ) -> (BitmapSurface, SidebarPanelLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let resolved = crate::DrawsPrimitives::draw_dyn(
                b,
                Rect::new(0.0, 0.0, W as f32, H as f32),
                panel,
                &(),
            );
            *layout.borrow_mut() = resolved.downcast_ref::<SidebarPanelLayout>().cloned();
        });
        backend.end_frame();
        (surface, layout.into_inner().expect("a SidebarPanelLayout"))
    }

    fn default_backend() -> MacBackend {
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend
    }

    #[test]
    fn toolbar_reserves_header_slot_content_starts_below() {
        let (_surface, layout) = paint_via_backend(
            &mut default_backend(),
            &panel_of(action("refine", "Refine", None)),
        );
        let tb = layout.toolbar_bounds.expect("toolbar bounds present");
        assert_eq!(tb.y, 0.0);
        assert!(layout.content_bounds.y >= tb.y + tb.height);
        match layout.hit_test(2.0, 1.0) {
            SidebarPanelHit::ToolbarButton(id) => assert_eq!(id.as_str(), "refine"),
            other => panic!("expected ToolbarButton, got {other:?}"),
        }
    }

    /// A chrome font smaller than `Style::toolbar_height` gets that fixed
    /// token as its header height; a chrome font taller than it grows the
    /// header, so the header's own icon glyphs are never clipped.
    #[test]
    fn toolbar_header_height_floors_at_style_toolbar_height_and_grows_with_a_huge_chrome_font() {
        fn toolbar_height_for(chrome_desc: &str) -> f32 {
            let mut backend = MacBackend::new();
            Backend::set_editor_font(&mut backend, "Menlo", 9.0);
            Backend::set_ui_font(&mut backend, chrome_desc);
            let (_surface, layout) =
                paint_via_backend(&mut backend, &panel_of(action("refine", "Refine", None)));
            layout
                .toolbar_bounds
                .expect("toolbar bounds present")
                .height
        }

        let h_small_chrome = toolbar_height_for("Menlo 9");
        assert_eq!(h_small_chrome, crate::Style::default().toolbar_height);

        let h_huge_chrome = toolbar_height_for("Menlo 60");
        assert!(
            h_huge_chrome > h_small_chrome,
            "header height must grow with a chrome font taller than the default token: \
             {h_small_chrome} vs {h_huge_chrome}"
        );
    }

    /// A codicon icon glyph — which resolves through the chrome font's
    /// fallback cascade — paints fully inside the header band and sits
    /// vertically centred on it.
    #[test]
    fn toolbar_icon_glyph_is_vertically_centered_in_header_band() {
        let panel = panel_of(action("new-file", "", Some("\u{EAF0}")));
        let (surface, layout) = paint_via_backend(&mut default_backend(), &panel);
        let tb = layout.toolbar_bounds.expect("toolbar bounds present");
        let local = layout
            .toolbar
            .as_ref()
            .expect("toolbar layout present")
            .layout
            .visible_items[0]
            .bounds;
        let item = Rect::new(local.x + tb.x, local.y + tb.y, local.width, local.height);

        // The header band is flat-filled before any glyph is painted, so
        // any pixel that doesn't match that fill is glyph ink.
        let bg = surface.pixel((item.x as u32).min(W - 1), (tb.y as u32).min(H - 1));

        let x0 = item.x.max(0.0) as u32;
        let x1 = ((item.x + item.width).min(W as f32)) as u32;
        let y0 = tb.y.max(0.0) as u32;
        let y1 = ((tb.y + tb.height).min(H as f32)) as u32;

        let mut min_y: Option<u32> = None;
        let mut max_y: Option<u32> = None;
        for y in y0..y1 {
            for x in x0..x1 {
                if surface.pixel(x, y) != bg {
                    min_y = Some(min_y.map_or(y, |m| m.min(y)));
                    max_y = Some(max_y.map_or(y, |m| m.max(y)));
                }
            }
        }
        let min_y = min_y.expect("icon glyph should paint at least one pixel") as f32;
        let max_y = max_y.expect("icon glyph should paint at least one pixel") as f32;

        assert!(min_y >= tb.y - 0.5, "glyph top {min_y} above band {}", tb.y);
        assert!(
            max_y + 1.0 <= tb.y + tb.height + 0.5,
            "glyph bottom {} clipped by band bottom {}",
            max_y + 1.0,
            tb.y + tb.height,
        );
        let glyph_center = (min_y + max_y + 1.0) / 2.0;
        let band_center = tb.y + tb.height / 2.0;
        assert!(
            (glyph_center - band_center).abs() <= 2.0,
            "glyph centre {glyph_center} not within 2px of band centre {band_center}",
        );
    }
}
