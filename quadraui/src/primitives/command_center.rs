//! `CommandCenter` primitive: a horizontal strip with back/forward nav
//! arrows and a clickable search box. Lives in the menu bar row,
//! centered between the menu labels and any trailing chrome (window
//! controls, etc.).
//!
//! `CommandCenter` is on the widget contract (`Primitive` +
//! `Interactive`, below): it reaches a frame as a type-erased
//! `crate::frame::Surface::Widget` and paints through
//! `crate::DrawsPrimitives::draw_dyn`. There is no `Backend` method or
//! per-platform rasteriser file of its own — `native_surface_paint`
//! paints it on every pixel backend and `cell_paint` on a cell grid.
//!
//! # Activation
//!
//! A left press on the back arrow, the forward arrow or the search box
//! emits [`crate::UiEvent::CommandCenterActivated`] naming that part
//! ([`CommandCenterTarget`]). A press on a disabled arrow is swallowed
//! without an event; a press on the bar's empty padding is not claimed,
//! so it bubbles to the host (a title-bar drag, for instance).
//!
//! The strip is one keyboard focus stop: `Enter`/`Space` while it holds
//! focus activates the search box, its primary control, and so does an
//! assistive-technology `Click`.

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

/// Which part of a [`CommandCenter`] an activation landed on — the
/// payload of [`crate::UiEvent::CommandCenterActivated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandCenterTarget {
    /// The back (`◀`) nav arrow.
    Back,
    /// The forward (`▶`) nav arrow.
    Forward,
    /// The search box.
    Search,
}

impl CommandCenterTarget {
    /// The [`crate::interactive::SubTarget`] this part is addressed by in
    /// [`crate::interactive::Interactive::sub_regions`] and the frame's
    /// painted-widget record.
    pub fn sub_target(self) -> crate::interactive::SubTarget {
        crate::interactive::SubTarget(match self {
            CommandCenterTarget::Back => 0,
            CommandCenterTarget::Forward => 1,
            CommandCenterTarget::Search => 2,
        })
    }
}

/// The action a [`CommandCenter`] emits through
/// [`crate::interactive::Interactive::handle`]: one activated part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandCenterAction(pub CommandCenterTarget);

impl crate::interactive::WidgetAction for CommandCenterAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::CommandCenterActivated {
            widget: id,
            target: self.0,
        }
    }
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

    /// The rect of `target`'s part, if this layout reserved one.
    pub fn target_bounds(&self, target: CommandCenterTarget) -> Option<Rect> {
        match target {
            CommandCenterTarget::Back => self.back_bounds,
            CommandCenterTarget::Forward => self.forward_bounds,
            CommandCenterTarget::Search => self.search_bounds,
        }
    }
}

impl CommandCenterMeasure {
    /// Standard nav-arrow width (px/DIPs) on a pixel surface — the
    /// value both [`Self::from_char_width`] and the widget contract's
    /// pixel layout use, so a command center looks the same size
    /// regardless of which measurement path produced its layout.
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
    /// rather than a live per-glyph text layout — the shape a caller
    /// needs when it has only a `char_width` number on hand, with no
    /// live font context to measure against.
    ///
    /// Search-box width is `search_label.len() as f32 * char_width +
    /// 16.0`, floored at `280.0`, or `0.0` when `search_label` is empty
    /// (hides the search box — see [`CommandCenter::search_label`]'s
    /// doc).
    ///
    /// Distinct from the widget contract's pixel layout
    /// (`Primitive::layout` on [`CommandCenter`]), which measures the
    /// label and its search icon through the backend's own text measurer
    /// — this constructor is the cheaper approximation for callers
    /// without one.
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
        let content_width = content_width(&measure);

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

    /// The part a left press at absolute `(x, y)` activates, or `None`
    /// when it lands on no part, or on a disabled nav arrow.
    fn target_at(
        &self,
        layout: &CommandCenterLayout,
        x: f32,
        y: f32,
    ) -> Option<CommandCenterTarget> {
        match layout.hit_test(x, y) {
            CommandCenterHit::Back if self.back_enabled => Some(CommandCenterTarget::Back),
            CommandCenterHit::Forward if self.forward_enabled => Some(CommandCenterTarget::Forward),
            CommandCenterHit::SearchBox => Some(CommandCenterTarget::Search),
            _ => None,
        }
    }
}

/// Total width of the arrow group plus (when present) the search box.
fn content_width(measure: &CommandCenterMeasure) -> f32 {
    measure.arrow_width * 2.0
        + measure.gap
        + if measure.search_box_width > 0.0 {
            measure.gap + measure.search_box_width
        } else {
            0.0
        }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────

/// Width, in cells, of each nav-arrow slot on a cell grid.
const CELL_ARROW_WIDTH: f32 = 2.0;
/// Gap, in cells, between the arrows and between the arrow group and the
/// search box on a cell grid.
const CELL_GAP: f32 = 1.0;
/// Horizontal padding, in surface-native units, between the search box's
/// edge/icon/label on a pixel surface — the box holds
/// `pad | icon | pad | label | pad`.
const PIXEL_SEARCH_PAD: f32 = 8.0;

/// The cell-grid measure for `cc` at `height` rows: two-cell arrows, a
/// one-cell gap, and a search box four cells wider than its label.
fn cell_measure(cc: &CommandCenter, height: f32) -> CommandCenterMeasure {
    let search_box_width = if cc.search_label.is_empty() {
        0.0
    } else {
        (cc.search_label.chars().count() + 4) as f32
    };
    CommandCenterMeasure {
        arrow_width: CELL_ARROW_WIDTH,
        gap: CELL_GAP,
        search_box_width,
        height: height.min(1.0),
    }
}

/// Resolve `cc`'s layout on a cell grid.
///
/// `rect` is quantised to whole cells first. A degenerate area yields
/// [`CommandCenterLayout::empty`]. A part that would spill past the
/// area's right edge — only possible when the strip is squeezed narrower
/// than its own content — is dropped from the layout, bounds and hit
/// region both, since the cell painter skips it too.
pub(crate) fn cell_layout(cc: &CommandCenter, rect: Rect) -> CommandCenterLayout {
    let area = Rect::new(
        rect.x.max(0.0).round(),
        rect.y.max(0.0).round(),
        rect.width.max(0.0).round(),
        rect.height.max(0.0).round(),
    );
    if area.width <= 0.0 || area.height <= 0.0 {
        return CommandCenterLayout::empty(area);
    }

    let mut layout = cc.layout(area, cell_measure(cc, area.height));

    let area_right = area.x + area.width;
    let fits = |r: Rect| r.x >= area.x && r.x + r.width <= area_right;
    let back_fits = layout.back_bounds.is_some_and(fits);
    let forward_fits = layout.forward_bounds.is_some_and(fits);
    let search_fits = layout.search_bounds.is_some_and(fits);
    if !back_fits {
        layout.back_bounds = None;
    }
    if !forward_fits {
        layout.forward_bounds = None;
    }
    if !search_fits {
        layout.search_bounds = None;
    }
    layout.hit_regions.retain(|(_, hit)| match hit {
        CommandCenterHit::Back => back_fits,
        CommandCenterHit::Forward => forward_fits,
        CommandCenterHit::SearchBox => search_fits,
        CommandCenterHit::Bar | CommandCenterHit::Outside => true,
    });
    layout
}

/// The pixel measure for `cc` at `height`: [`CommandCenterMeasure::ARROW_WIDTH_PX`]
/// arrows and [`CommandCenterMeasure::GAP_PX`] gaps, and a search box
/// holding the search icon (measured in [`crate::FontRole::Icon`]) and
/// the label (measured in [`crate::FontRole::Chrome`], the role the
/// pixel painter draws it in) with three [`PIXEL_SEARCH_PAD`]s, floored
/// at the shared minimum width.
fn pixel_measure(
    cc: &CommandCenter,
    ctx: &crate::primitive::LayoutCtx,
    height: f32,
) -> CommandCenterMeasure {
    let search_box_width = if cc.search_label.is_empty() {
        0.0
    } else {
        let text_w = ctx.text_width(&cc.search_label, crate::FontRole::Chrome);
        let icon_w = crate::codicon_names::codepoint("search")
            .map(|glyph| ctx.text_width(&glyph.to_string(), crate::FontRole::Icon))
            .unwrap_or(0.0);
        (text_w + icon_w + PIXEL_SEARCH_PAD * 3.0).max(CommandCenterMeasure::SEARCH_MIN_WIDTH_PX)
    };
    CommandCenterMeasure {
        arrow_width: CommandCenterMeasure::ARROW_WIDTH_PX,
        gap: CommandCenterMeasure::GAP_PX,
        search_box_width,
        height,
    }
}

/// Resolve `cc`'s layout on a pixel surface — see [`pixel_measure`].
pub(crate) fn pixel_layout(
    cc: &CommandCenter,
    ctx: &crate::primitive::LayoutCtx,
    rect: Rect,
) -> CommandCenterLayout {
    cc.layout(rect, pixel_measure(cc, ctx, rect.height))
}

impl crate::primitive::Primitive for CommandCenter {
    type Layout = CommandCenterLayout;
    type Event = CommandCenterHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::CommandCenter(self.clone())
    }

    /// The arrow group plus the search box: a content-driven width, one
    /// row tall on a cell grid. A pixel host sizes the strip's height to
    /// its own title-bar row, so no height is reported there.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        match ctx.units() {
            crate::primitive::Units::Cells => {
                crate::NaturalSize::new(Some(content_width(&cell_measure(self, 1.0))), Some(1.0))
            }
            _ => crate::NaturalSize::new(
                Some(content_width(&pixel_measure(self, ctx, ctx.line_height))),
                None,
            ),
        }
    }

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> CommandCenterLayout {
        match ctx.units() {
            crate::primitive::Units::Cells => cell_layout(self, rect),
            _ => pixel_layout(self, ctx, rect),
        }
    }

    /// The shared pixel painter, against the already-resolved `layout`,
    /// with the surface's own line height centring the chevrons and the
    /// search label.
    fn paint(
        &self,
        layout: &CommandCenterLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        let _ = style;
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let line_height = surface.surface_line_height();
            native_surface_paint::paint(self, layout, surface, theme, line_height);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme);
    }

    /// The cell painter over `buf`, adapted through
    /// [`crate::tui::cell_surface::TuiCellSurface`].
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &CommandCenterLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        // The painter never moves the cursor, so the sink is never read.
        let mut cursor_sink = None;
        let mut surface = crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            &mut cursor_sink,
        );
        cell_paint::paint(self, layout, &mut surface, theme);
    }
}

impl crate::interactive::Interactive for CommandCenter {
    type State = ();
    type Action = CommandCenterAction;

    /// One focus stop for the whole strip; the keyboard reaches its
    /// search box through it, while back/forward stay pointer targets a
    /// host also binds to its own accelerators.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    /// `Click` activates the search box, when the strip shows one.
    fn a11y_actions(&self, _state: &()) -> Vec<crate::a11y::A11yAction> {
        if self.search_label.is_empty() {
            Vec::new()
        } else {
            vec![crate::a11y::A11yAction::Click]
        }
    }

    /// The back arrow, forward arrow and search box, each in the strip's
    /// own local space — the three never overlap.
    fn sub_regions(
        &self,
        layout: &CommandCenterLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let origin = layout.bounds;
        [
            CommandCenterTarget::Back,
            CommandCenterTarget::Forward,
            CommandCenterTarget::Search,
        ]
        .into_iter()
        .filter_map(|target| {
            layout.target_bounds(target).map(|r| {
                (
                    Rect::new(r.x - origin.x, r.y - origin.y, r.width, r.height),
                    target.sub_target(),
                )
            })
        })
        .collect()
    }

    /// A left press on an enabled arrow or the search box activates it.
    /// A press on a disabled arrow is consumed without an event, so it
    /// neither activates nor falls through to whatever sits behind the
    /// strip; a press on the bar's empty padding is left unclaimed.
    /// `Enter`/`Space` and an assistive-technology `Click` activate the
    /// search box, when the strip shows one.
    fn handle(
        &self,
        _state: &mut (),
        event: &crate::interactive::RoutedEvent,
        layout: &CommandCenterLayout,
        _cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<CommandCenterAction> {
        use crate::interactive::{PointerKind, RoutedEvent};
        use crate::Handled;

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                ..
            } => {
                let x = layout.bounds.x + local.x;
                let y = layout.bounds.y + local.y;
                match self.target_at(layout, x, y) {
                    Some(target) => Handled::Event(CommandCenterAction(target)),
                    None => match layout.hit_test(x, y) {
                        CommandCenterHit::Back | CommandCenterHit::Forward => Handled::Consumed,
                        _ => Handled::Ignored,
                    },
                }
            }
            RoutedEvent::Key(crate::interactive::KeyEvent {
                key: crate::Key::Named(crate::NamedKey::Enter) | crate::Key::Char(' '),
                ..
            })
            | RoutedEvent::Action(crate::a11y::A11yAction::Click)
                if layout.search_bounds.is_some() =>
            {
                Handled::Event(CommandCenterAction(CommandCenterTarget::Search))
            }
            _ => Handled::Ignored,
        }
    }
}

/// The cell-grid painter for [`CommandCenter`], written against
/// [`crate::CellSurface`]: `◀ ▶ [title]` centred in the strip's first
/// row.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{CommandCenter, CommandCenterLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::event::Rect;
    use crate::theme::Theme;
    use crate::types::Color;

    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `cc` at `layout` on `surface`. `layout` must be the value
    /// `cell_layout` returned for the same strip, so paint and
    /// [`CommandCenterLayout::hit_test`] agree on which parts exist.
    pub(crate) fn paint(
        cc: &CommandCenter,
        layout: &CommandCenterLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let bounds = layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }

        let bg = theme.tab_bar_bg;
        let enabled_fg = theme.tab_inactive_fg;
        let disabled_fg = theme.muted_fg;
        let border_fg = theme.muted_fg;
        let text_fg = theme.tab_inactive_fg;

        let y = bounds.y.round() as u16;
        surface.fill(
            Rect::new(bounds.x.round(), y as f32, bounds.width.round(), 1.0),
            solid(enabled_fg, bg),
        );

        if let Some(bb) = layout.back_bounds {
            let fg = if cc.back_enabled {
                enabled_fg
            } else {
                disabled_fg
            };
            surface.put_char(bb.x.round() as u16, y, '◀', solid(fg, bg));
        }

        if let Some(fb) = layout.forward_bounds {
            let fg = if cc.forward_enabled {
                enabled_fg
            } else {
                disabled_fg
            };
            surface.put_char(fb.x.round() as u16, y, '▶', solid(fg, bg));
        }

        if let Some(sb) = layout.search_bounds {
            let sx = sb.x.round() as u16;
            let sw = sb.width.round() as u16;
            surface.put_char(sx, y, '[', solid(border_fg, bg));
            for (col, ch) in (sx + 1..).zip(cc.search_label.chars()) {
                if col >= sx + sw - 1 {
                    break;
                }
                surface.put_char(col, y, ch, solid(text_fg, bg));
            }
            if sw > 1 {
                surface.put_char(sx + sw - 1, y, ']', solid(border_fg, bg));
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{cell_layout, CommandCenterHit};
        use super::*;
        use crate::tui::cell_surface::TuiCellSurface;
        use crate::types::WidgetId;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
            buf[(x, y)].symbol().chars().next().unwrap_or(' ')
        }

        fn mk_cc(search: &str) -> CommandCenter {
            CommandCenter {
                id: WidgetId::new("cc"),
                back_enabled: true,
                forward_enabled: true,
                search_label: search.into(),
            }
        }

        /// Lay `cc` out for `area` and paint it into `buf`, the way
        /// `Primitive::layout` + `Primitive::paint_cells` do.
        fn draw(buf: &mut Buffer, area: RRect, cc: &CommandCenter) -> CommandCenterLayout {
            let layout = cell_layout(
                cc,
                Rect::new(
                    area.x as f32,
                    area.y as f32,
                    area.width as f32,
                    area.height as f32,
                ),
            );
            let mut cursor_sink = None;
            let mut surface =
                TuiCellSurface::new(buf, crate::backend::ColorDepth::TrueColor, &mut cursor_sink);
            paint(cc, &layout, &mut surface, &Theme::default());
            layout
        }

        /// Paint and click must agree at a non-zero origin too: the
        /// layout carries absolute bounds.
        fn arrows_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
            let area = RRect::new(origin_x, origin_y, 40, 1);
            let mut buf = Buffer::empty(area);
            let layout = draw(&mut buf, area, &mk_cc("Search"));

            let bb = layout.back_bounds.unwrap();
            assert_eq!(cell_char(&buf, bb.x.round() as u16, origin_y), '◀');
            assert_eq!(
                layout.hit_test(bb.x + 0.5, origin_y as f32 + 0.5),
                CommandCenterHit::Back
            );

            let fb = layout.forward_bounds.unwrap();
            assert_eq!(cell_char(&buf, fb.x.round() as u16, origin_y), '▶');
            assert_eq!(
                layout.hit_test(fb.x + 0.5, origin_y as f32 + 0.5),
                CommandCenterHit::Forward
            );
        }

        #[test]
        fn arrows_paint_and_click_round_trip() {
            arrows_paint_and_click_round_trip_at(0, 0);
        }

        #[test]
        fn arrows_paint_and_click_round_trip_at_nonzero_origin() {
            arrows_paint_and_click_round_trip_at(7, 13);
        }

        fn search_box_paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
            let area = RRect::new(origin_x, origin_y, 40, 1);
            let mut buf = Buffer::empty(area);
            let layout = draw(&mut buf, area, &mk_cc("Test"));

            let sb = layout.search_bounds.unwrap();
            let sx = sb.x.round() as u16;
            assert_eq!(cell_char(&buf, sx, origin_y), '[');
            assert_eq!(cell_char(&buf, sx + 1, origin_y), 'T');
            assert_eq!(
                layout.hit_test(sb.x + 2.0, origin_y as f32 + 0.5),
                CommandCenterHit::SearchBox
            );
        }

        #[test]
        fn search_box_paint_and_click_round_trip() {
            search_box_paint_and_click_round_trip_at(0, 0);
        }

        #[test]
        fn search_box_paint_and_click_round_trip_at_nonzero_origin() {
            search_box_paint_and_click_round_trip_at(7, 13);
        }

        #[test]
        fn no_search_box_when_label_empty() {
            let area = RRect::new(0, 0, 40, 1);
            let mut buf = Buffer::empty(area);
            let layout = draw(&mut buf, area, &mk_cc(""));
            assert!(layout.search_bounds.is_none());
        }

        #[test]
        fn outside_hit() {
            let mut buf = Buffer::empty(RRect::new(0, 0, 40, 1));
            let layout = draw(&mut buf, RRect::new(10, 0, 20, 1), &mk_cc("X"));
            assert_eq!(layout.hit_test(0.0, 0.5), CommandCenterHit::Outside);
        }

        #[test]
        fn zero_size_is_a_no_op() {
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 1));
            let layout = draw(&mut buf, RRect::new(0, 0, 0, 0), &mk_cc("X"));
            assert_eq!(cell_char(&buf, 0, 0), ' ');
            assert!(layout.back_bounds.is_none());
            assert!(layout.forward_bounds.is_none());
            assert!(layout.search_bounds.is_none());
            assert_eq!(layout.hit_test(0.0, 0.0), CommandCenterHit::Outside);
        }

        /// A `width == 0` area has no hit regions, matching that nothing
        /// is painted.
        #[test]
        fn zero_width_area_is_hit_testably_empty() {
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 1));
            let layout = draw(&mut buf, RRect::new(0, 0, 0, 1), &mk_cc("X"));
            assert!(layout.back_bounds.is_none());
            assert!(layout.forward_bounds.is_none());
            assert!(layout.search_bounds.is_none());
            for x in 0..10 {
                assert_eq!(
                    layout.hit_test(x as f32 + 0.5, 0.5),
                    CommandCenterHit::Outside
                );
            }
        }

        /// The `height == 0` half of the degenerate-area guard.
        #[test]
        fn zero_height_area_is_hit_testably_empty() {
            let mut buf = Buffer::empty(RRect::new(0, 0, 10, 1));
            let layout = draw(&mut buf, RRect::new(0, 0, 10, 0), &mk_cc("X"));
            assert!(layout.back_bounds.is_none());
            assert!(layout.forward_bounds.is_none());
            assert!(layout.search_bounds.is_none());
            assert_eq!(layout.hit_test(0.0, 0.0), CommandCenterHit::Outside);
        }

        /// Squeezed narrower than its content, the strip skips the search
        /// box rather than spilling past its own edge, and the layout
        /// neither reports nor hit-tests it.
        #[test]
        fn clipped_search_box_does_not_report_search_bounds() {
            // "Search" needs 16 cells (2 arrows * 2 + gap 1 + gap 1 +
            // search box 10); an 8-wide area fits the arrows only.
            let area = RRect::new(0, 0, 8, 1);
            let mut buf = Buffer::empty(area);
            let layout = draw(&mut buf, area, &mk_cc("Search"));

            assert!(
                layout.search_bounds.is_none(),
                "search box doesn't fit in an 8-wide area; its bounds must not be reported"
            );
            assert_eq!(cell_char(&buf, 6, 0), ' ');
            assert_eq!(layout.hit_test(6.5, 0.5), CommandCenterHit::Bar);
        }

        #[test]
        fn disabled_arrow_paints_in_the_muted_colour() {
            let area = RRect::new(0, 0, 40, 1);
            let mut buf = Buffer::empty(area);
            let mut cc = mk_cc("Search");
            cc.back_enabled = false;
            let layout = draw(&mut buf, area, &cc);
            let theme = Theme::default();
            let bx = layout.back_bounds.unwrap().x.round() as u16;
            let fx = layout.forward_bounds.unwrap().x.round() as u16;
            assert_eq!(buf[(bx, 0)].fg, crate::tui::ratatui_color(theme.muted_fg));
            assert_eq!(
                buf[(fx, 0)].fg,
                crate::tui::ratatui_color(theme.tab_inactive_fg)
            );
        }
    }
}

/// The shared [`crate::paint_surface::PaintSurface`] painter for
/// [`CommandCenter`] on every pixel backend: nav chevrons and a search
/// box in VS Code's title-bar look — a low-alpha rounded fill and border
/// with a leading magnifier glyph.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod native_surface_paint {
    use super::{CommandCenter, CommandCenterLayout, PIXEL_SEARCH_PAD};
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
    /// Vertical inset applied to the search box's top/bottom edge,
    /// leaving a thin margin above/below it within the full-height
    /// search slot.
    const SEARCH_V_INSET: f32 = 2.0;

    /// Paint `cc` at the already-resolved `layout` on `surface`.
    /// `line_height` vertically centres the nav chevrons and search
    /// label.
    pub(crate) fn paint(
        cc: &CommandCenter,
        layout: &CommandCenterLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        line_height: f32,
    ) {
        let rect = layout.bounds;
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        surface.surface_fill_rect(rect, theme.tab_bar_bg);

        let enabled_fg = theme.tab_inactive_fg;
        let disabled_fg = theme.muted_fg;
        let text_y = rect.y + ((rect.height - line_height) / 2.0).max(0.0);

        if let Some(bb) = layout.back_bounds {
            let fg = if cc.back_enabled {
                enabled_fg
            } else {
                disabled_fg
            };
            draw_centered_chevron(surface, bb, codicon::CHEVRON_LEFT, fg, text_y, line_height);
        }
        if let Some(fb) = layout.forward_bounds {
            let fg = if cc.forward_enabled {
                enabled_fg
            } else {
                disabled_fg
            };
            draw_centered_chevron(surface, fb, codicon::CHEVRON_RIGHT, fg, text_y, line_height);
        }

        if let Some(sb) = layout.search_bounds {
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
            let icon_x = box_rect.x + PIXEL_SEARCH_PAD;
            let icon_y = box_rect.y + ((box_rect.height - icon_h) / 2.0).max(0.0);
            surface.surface_draw_icon_glyph(
                Rect::new(icon_x, icon_y, icon_w, icon_h),
                &icon_glyph,
                disabled_fg,
            );

            let label_x = icon_x + icon_w + PIXEL_SEARCH_PAD;
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
    }

    /// Draw a single codicon chevron glyph centred horizontally within
    /// `slot`, at `text_y`/`line_height`.
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
        use crate::primitive::LayoutCtx;
        use crate::primitives::command_center::{pixel_layout, CommandCenterHit};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues.
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

        /// Lay `cc` out the way a pixel backend's `LayoutCtx` would (a
        /// 16px row, an 8px-per-char measurer) and paint it.
        fn layout_and_paint(
            cc: &CommandCenter,
            surface: &mut RecordingSurface,
            rect: Rect,
        ) -> CommandCenterLayout {
            let ctx = LayoutCtx::new(16.0, 8.0, false);
            let layout = pixel_layout(cc, &ctx, rect);
            paint(cc, &layout, surface, &Theme::default(), 16.0);
            layout
        }

        #[test]
        fn empty_search_label_omits_search_bounds_and_box() {
            let cc = CommandCenter {
                search_label: "".into(),
                ..sample_cc()
            };
            let mut surface = RecordingSurface::default();
            let l = layout_and_paint(&cc, &mut surface, Rect::new(0.0, 0.0, 480.0, 32.0));
            assert!(l.search_bounds.is_none());
            assert!(l.back_bounds.is_some());
            assert!(l.forward_bounds.is_some());
            assert!(surface.strokes.is_empty());
        }

        #[test]
        fn search_box_paints_a_low_alpha_fill_and_border() {
            let mut surface = RecordingSurface::default();
            layout_and_paint(&sample_cc(), &mut surface, Rect::new(0.0, 0.0, 480.0, 32.0));
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
            let mut surface = RecordingSurface::default();
            layout_and_paint(&sample_cc(), &mut surface, Rect::new(0.0, 0.0, 480.0, 32.0));
            assert_eq!(surface.icon_glyphs.len(), 3); // back + forward + search icon
            assert!(surface
                .icon_glyphs
                .iter()
                .any(|(_, g, _)| g == &codicon::SEARCH.to_string()));
            assert!(surface.text_runs.iter().any(|(_, t, _)| t == "project"));
        }

        #[test]
        fn back_and_forward_glyphs_use_enabled_or_disabled_colour() {
            let mut surface = RecordingSurface::default();
            layout_and_paint(&sample_cc(), &mut surface, Rect::new(0.0, 0.0, 480.0, 32.0));
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
            let mut surface = RecordingSurface::default();
            let l = layout_and_paint(
                &sample_cc(),
                &mut surface,
                Rect::new(7.0, 13.0, 480.0, 32.0),
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
        fn zero_size_rect_paints_nothing() {
            let mut surface = RecordingSurface::default();
            let l = layout_and_paint(&sample_cc(), &mut surface, Rect::new(0.0, 0.0, 0.0, 0.0));
            assert!(surface.fills.is_empty());
            assert!(surface.icon_glyphs.is_empty());
            assert_eq!(l.bounds.width, 0.0);
        }

        /// Once the label outgrows the minimum width, the box the layout
        /// reserved still holds `pad | icon | pad | label | pad`.
        #[test]
        fn long_label_fits_inside_the_reserved_search_box() {
            let cc = CommandCenter {
                search_label: "x".repeat(60),
                ..sample_cc()
            };
            let mut surface = RecordingSurface::default();
            let l = layout_and_paint(&cc, &mut surface, Rect::new(0.0, 0.0, 900.0, 32.0));
            let search = l.search_bounds.unwrap();
            let (label_rect, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t.starts_with('x'))
                .expect("label painted");
            assert!(label_rect.x + 60.0 * 8.0 <= search.x + search.width);
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

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget,
        WidgetAction,
    };
    use crate::primitive::{LayoutCtx, Primitive, Size, Units};
    use crate::{Handled, Modifiers, MouseButton, Point};

    #[derive(Debug, Default)]
    struct MockHost;

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn cc() -> CommandCenter {
        CommandCenter {
            id: WidgetId::new("titlebar:cc"),
            back_enabled: true,
            forward_enabled: false,
            search_label: "project".into(),
        }
    }

    const AREA: Rect = Rect::new(4.0, 2.0, 40.0, 1.0);

    fn cell_ctx() -> LayoutCtx<'static> {
        LayoutCtx::new(1.0, 1.0, false)
    }

    fn press_at(layout: &CommandCenterLayout, x: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(x - layout.bounds.x, 0.0),
            mods: Modifiers::default(),
        }
    }

    fn handle(
        w: &CommandCenter,
        layout: &CommandCenterLayout,
        event: &RoutedEvent,
    ) -> Handled<CommandCenterAction> {
        let mut host = MockHost;
        let mut cx = EventCx::new(w.id, &mut host);
        w.handle(&mut (), event, layout, &mut cx)
    }

    #[test]
    fn is_one_focus_stop() {
        assert_eq!(cc().focus(), FocusSpec::Stop);
    }

    #[test]
    fn enter_and_space_activate_the_search_box() {
        let w = cc();
        let layout = Primitive::layout(&w, &cell_ctx(), AREA);
        for key in [
            crate::Key::Named(crate::NamedKey::Enter),
            crate::Key::Char(' '),
        ] {
            let event = RoutedEvent::Key(crate::interactive::KeyEvent {
                key,
                modifiers: Modifiers::default(),
                repeat: false,
            });
            assert_eq!(
                handle(&w, &layout, &event),
                Handled::Event(CommandCenterAction(CommandCenterTarget::Search))
            );
        }
    }

    #[test]
    fn cell_layout_matches_the_cell_measure() {
        let w = cc();
        let layout = Primitive::layout(&w, &cell_ctx(), AREA);
        // 2 + 1 + 2 + 1 + ("project" + 4)
        let content = 2.0 + 1.0 + 2.0 + 1.0 + 11.0;
        let back = layout.back_bounds.unwrap();
        assert_eq!(back.x, AREA.x + (AREA.width - content) / 2.0);
        assert_eq!(layout.search_bounds.unwrap().width, 11.0);
        let natural = Primitive::measure(&w, &cell_ctx(), Size::new(80.0, 1.0));
        assert_eq!(natural.width, Some(content));
        assert_eq!(natural.height, Some(1.0));
    }

    #[test]
    fn pixel_layout_measures_label_and_icon_through_the_ctx() {
        let w = CommandCenter {
            search_label: "x".repeat(50),
            ..cc()
        };
        let measurer = |text: &str, role: crate::FontRole| match role {
            crate::FontRole::Icon => 20.0,
            _ => text.chars().count() as f32 * 7.0,
        };
        let ctx = LayoutCtx::new(20.0, 7.0, false).with_measurer(&measurer);
        let layout = Primitive::layout(&w, &ctx, Rect::new(0.0, 0.0, 1000.0, 30.0));
        assert_eq!(
            layout.search_bounds.unwrap().width,
            50.0 * 7.0 + 20.0 + PIXEL_SEARCH_PAD * 3.0
        );
        assert_eq!(
            layout.back_bounds.unwrap().width,
            CommandCenterMeasure::ARROW_WIDTH_PX
        );
    }

    #[test]
    fn sub_regions_are_widget_local() {
        let w = cc();
        let layout = Primitive::layout(&w, &cell_ctx(), AREA);
        let subs = w.sub_regions(&layout);
        assert_eq!(subs.len(), 3);
        let back = layout.back_bounds.unwrap();
        assert_eq!(
            subs[0],
            (
                Rect::new(back.x - AREA.x, 0.0, back.width, back.height),
                CommandCenterTarget::Back.sub_target()
            )
        );
        assert_eq!(subs[2].1, CommandCenterTarget::Search.sub_target());
    }

    #[test]
    fn press_on_the_enabled_back_arrow_activates_back() {
        let w = cc();
        let layout = Primitive::layout(&w, &cell_ctx(), AREA);
        let back = layout.back_bounds.unwrap();
        assert_eq!(
            handle(&w, &layout, &press_at(&layout, back.x + 0.5)),
            Handled::Event(CommandCenterAction(CommandCenterTarget::Back))
        );
    }

    #[test]
    fn press_on_the_disabled_forward_arrow_is_swallowed() {
        let w = cc();
        let layout = Primitive::layout(&w, &cell_ctx(), AREA);
        let fwd = layout.forward_bounds.unwrap();
        assert_eq!(
            handle(&w, &layout, &press_at(&layout, fwd.x + 0.5)),
            Handled::Consumed
        );
    }

    #[test]
    fn press_on_the_search_box_activates_search() {
        let w = cc();
        let layout = Primitive::layout(&w, &cell_ctx(), AREA);
        let search = layout.search_bounds.unwrap();
        assert_eq!(
            handle(&w, &layout, &press_at(&layout, search.x + 2.0)),
            Handled::Event(CommandCenterAction(CommandCenterTarget::Search))
        );
    }

    #[test]
    fn press_on_the_bar_padding_is_left_unclaimed() {
        let w = cc();
        let layout = Primitive::layout(&w, &cell_ctx(), AREA);
        assert_eq!(
            handle(&w, &layout, &press_at(&layout, AREA.x + 0.5)),
            Handled::Ignored
        );
    }

    #[test]
    fn a11y_click_needs_a_search_box() {
        let w = CommandCenter {
            search_label: String::new(),
            ..cc()
        };
        assert!(w.a11y_actions(&()).is_empty());
        let layout = Primitive::layout(&w, &cell_ctx(), AREA);
        assert_eq!(
            handle(&w, &layout, &RoutedEvent::Action(A11yAction::Click)),
            Handled::Ignored
        );
        assert_eq!(cc().a11y_actions(&()), vec![A11yAction::Click]);
    }

    #[test]
    fn action_becomes_a_command_center_activated_event() {
        let id = WidgetId::new("titlebar:cc");
        assert_eq!(
            CommandCenterAction(CommandCenterTarget::Forward).into_ui_event(id),
            crate::UiEvent::CommandCenterActivated {
                widget: id,
                target: CommandCenterTarget::Forward,
            }
        );
    }

    /// Event parity's in-crate leg: an [`A11yAction::Click`] routed
    /// through [`crate::a11y::route_widget_action`] produces the exact
    /// [`crate::UiEvent`] a direct [`Interactive::handle`] call with the
    /// same action does. The pointer legs (TuiDriver and the binding
    /// host) live in `tests/widget_contract_command_center.rs` and
    /// `quadraui-binding-core`'s engine tests.
    #[test]
    fn route_widget_action_click_matches_direct_interactive_handle() {
        let w = cc();
        let id = w.id;
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, AREA, Box::new(w.clone()));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let ctx = cell_ctx();

        let events =
            crate::a11y::route_widget_action(&tree, &mut states, &ctx, id, A11yAction::Click);

        let layout = Primitive::layout(&w, &ctx, AREA);
        let expected = match handle(&w, &layout, &RoutedEvent::Action(A11yAction::Click)) {
            Handled::Event(action) => vec![action.into_ui_event(id)],
            _ => vec![],
        };
        assert_eq!(events, expected);
        assert_eq!(
            events,
            vec![crate::UiEvent::CommandCenterActivated {
                widget: id,
                target: CommandCenterTarget::Search,
            }]
        );
    }

    #[cfg(feature = "tui")]
    #[test]
    fn paint_cells_draws_the_strip() {
        let w = cc();
        let layout = Primitive::layout(&w, &cell_ctx(), Rect::new(0.0, 0.0, 30.0, 1.0));
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 30, 1));
        Primitive::paint_cells(&w, &layout, &mut buf, &crate::theme::Theme::default());
        let row: String = (0..30).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(row.contains('◀'), "{row}");
        assert!(row.contains('▶'), "{row}");
        assert!(row.contains("[project"), "{row}");
        assert!(row.contains(']'), "{row}");
    }
}
