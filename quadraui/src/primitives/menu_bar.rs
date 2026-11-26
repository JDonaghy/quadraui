//! `MenuBar` primitive: a horizontal strip of top-level menu labels
//! (File / Edit / View / ...). Each top-level item opens a dropdown
//! menu — represented in vimcode's rendering path by a
//! `ContextMenu`-style popup. The menu bar itself is just the
//! navigation strip; the dropdown is a separate concern the app
//! composes when a menu is open.
//!
//! Used for the top-of-window menu on Linux / Windows (macOS uses the
//! global menu bar, which this primitive maps to identically — the
//! backend decides whether to actually draw the strip or defer to
//! NSMenu).
//!
//! # Backend contract
//!
//! **Declarative.** Render the menu-bar row with each top-level item
//! as a clickable label. Click / keyboard-navigation resolves to
//! [`MenuBarHit::Item`]; the app opens a dropdown next to the item
//! using the returned `hit_regions` position. Keyboard Alt+key
//! activates the item whose label starts with that character.

use crate::a11y::A11yAction;
use crate::backend::{Backend, DrawsPrimitives, ResolvedMenuStyle};
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::compose::Handled;
use crate::event::{Key, MouseButton, NamedKey, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Declarative description of a menu bar.
///
/// # Examples
///
/// ```
/// use quadraui::{MenuBar, MenuBarHit, MenuBarItem, MenuBarItemMeasure, Rect, WidgetId};
///
/// let bar = MenuBar {
///     id: WidgetId::new("menubar:main"),
///     items: vec![MenuBarItem {
///         id: WidgetId::new("menu:file"),
///         label: "&File".to_string(),
///         disabled: false,
///         submenu: None,
///     }],
///     open_item: None,
///     focused_item: None,
/// };
///
/// let layout = bar.layout(Rect::new(0.0, 0.0, 80.0, 1.0), |_| MenuBarItemMeasure::new(6.0));
/// assert_eq!(layout.hit_test(2.0, 0.0), MenuBarHit::Item(0));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuBar {
    pub id: WidgetId,
    pub items: Vec<MenuBarItem>,
    /// Index of the currently-open menu (if any). Backends use this to
    /// render the "pressed" visual on the active item.
    #[serde(default)]
    pub open_item: Option<usize>,
    /// Keyboard-focused item (for Alt+navigation) — may differ from
    /// `open_item` during arrow-key traversal.
    #[serde(default)]
    pub focused_item: Option<usize>,
}

/// One top-level menu entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuBarItem {
    pub id: WidgetId,
    /// Display label, e.g. `"&File"` (with the `&` marking the
    /// Alt-activation character — backends render the following char
    /// underlined and map Alt+that-char to this item). If no `&` is
    /// present, the label is never underlined, though
    /// [`MenuBar::find_alt_target`]'s keyboard-activation lookup still
    /// falls back to the first character, unrelated to the visual
    /// underline.
    pub label: String,
    /// When true, the item is rendered dimmed and clicks are ignored.
    #[serde(default)]
    pub disabled: bool,
    /// Declarative dropdown items. When `Some`, native menu installers
    /// build the dropdown directly from this list. The in-window
    /// painters ignore the field; apps that draw their own dropdown via
    /// the `MenuSystem` compose helper wire that path independently.
    #[serde(default)]
    pub submenu: Option<Vec<crate::primitives::context_menu::ContextMenuItem>>,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Per-item measurement (width in the backend's unit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuBarItemMeasure {
    pub width: f32,
}

impl MenuBarItemMeasure {
    pub fn new(width: f32) -> Self {
        Self { width }
    }
}

/// Minimum height (px) of the in-canvas menu-bar strip on a pixel backend
/// (GTK/macOS/Win) — a VS-Code-style custom title bar row, floored above
/// (not replacing) the backend's own chrome line height. [`resolve_bar_height`]
/// is the single place that turns a candidate line height into this metric.
pub const BAR_HEIGHT: f32 = 30.0;

/// Resolve the menu-bar strip height a caller should use for
/// [`MenuBar::layout`]'s (or [`MenuBar::layout_with_leading`]'s)
/// `bounds.height`, given a candidate line height.
///
/// On a pixel backend (`candidate_line_height > 1.0`) this returns
/// `candidate_line_height.max(`[`BAR_HEIGHT`]`)` — the in-canvas bar is
/// floored at the fixed metric, same as every other resolver of this
/// shape in the crate (`GtkBackend::resolved_row_height`,
/// `palette::item_row_height`), so a chrome
/// font taller than the token still gets the room it measured instead of
/// being clipped. On TUI (`candidate_line_height <= 1.0`, the cell-grid
/// convention where one row is `1.0` "unit" tall) the candidate passes
/// through unchanged: a terminal cell has no sub-cell height to refine.
pub fn resolve_bar_height(candidate_line_height: f32) -> f32 {
    if candidate_line_height > 1.0 {
        candidate_line_height.max(BAR_HEIGHT)
    } else {
        candidate_line_height
    }
}

/// Resolved position of one visible menu-bar item.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleMenuBarItem {
    pub item_idx: usize,
    pub id: WidgetId,
    pub bounds: Rect,
    /// `true` iff the item is clickable (not disabled).
    pub clickable: bool,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuBarHit {
    /// Click landed on a top-level item.
    Item(usize),
    /// Click landed on the bar (not on any item) — apps may swallow.
    Bar,
    /// Click landed outside the bar — apps may dismiss the open menu.
    Outside,
}

/// Fully-resolved menu-bar layout.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuBarLayout {
    /// Full bar bounds.
    pub bounds: Rect,
    pub visible_items: Vec<VisibleMenuBarItem>,
    pub hit_regions: Vec<(Rect, MenuBarHit)>,
}

impl MenuBarLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> MenuBarHit {
        let inside = x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height;
        if !inside {
            return MenuBarHit::Outside;
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        MenuBarHit::Bar
    }
}

impl MenuBar {
    /// Compute item positions along the bar.
    ///
    /// # Arguments
    ///
    /// - `bounds` — menu-bar row.
    /// - `measure_item(i)` — width of item `i`.
    pub fn layout<F>(&self, bounds: Rect, measure_item: F) -> MenuBarLayout
    where
        F: Fn(usize) -> MenuBarItemMeasure,
    {
        let mut visible_items: Vec<VisibleMenuBarItem> = Vec::new();
        let mut hit_regions: Vec<(Rect, MenuBarHit)> = Vec::new();

        let mut cursor_x = bounds.x;
        for (i, item) in self.items.iter().enumerate() {
            let w = measure_item(i).width;
            if cursor_x + w > bounds.x + bounds.width {
                break;
            }
            let item_bounds = Rect::new(cursor_x, bounds.y, w, bounds.height);
            let clickable = !item.disabled;
            visible_items.push(VisibleMenuBarItem {
                item_idx: i,
                id: item.id,
                bounds: item_bounds,
                clickable,
            });
            if clickable {
                hit_regions.push((item_bounds, MenuBarHit::Item(i)));
            }
            cursor_x += w;
        }

        MenuBarLayout {
            bounds,
            visible_items,
            hit_regions,
        }
    }

    /// Like [`Self::layout`], but reserves `leading_width` device units
    /// at the start of `bounds` for a fixed-size leading element — an
    /// app-logo [`crate::Image`] left of the first menu item, VS-Code
    /// style — before laying out items.
    ///
    /// The offset math is the key concern: a consumer that narrows
    /// the rect it hands to a paint call but not the rect it hands to a
    /// click-routing call (or vice versa) gets a menu bar whose visible
    /// items and clickable items silently disagree. Centralizing the shift
    /// here means both call sites can pass the *same* `bounds` +
    /// `leading_width` pair instead of each independently computing a
    /// narrowed rect.
    ///
    /// `leading_width <= 0.0` behaves exactly like [`Self::layout`]
    /// called with `bounds` unchanged. [`MenuBarLayout::bounds`] on the
    /// result still covers the *full* `bounds` (including the reserved
    /// leading region) so [`MenuBarLayout::hit_test`]'s "inside the bar"
    /// check keeps treating a click over the icon as `MenuBarHit::Bar`
    /// rather than `MenuBarHit::Outside` — callers that want to
    /// special-case a click on the icon itself compare the click x
    /// against `bounds.x + leading_width` themselves, since the icon's
    /// own geometry isn't a `MenuBar` concern.
    pub fn layout_with_leading<F>(
        &self,
        bounds: Rect,
        leading_width: f32,
        measure_item: F,
    ) -> MenuBarLayout
    where
        F: Fn(usize) -> MenuBarItemMeasure,
    {
        let leading_width = leading_width.max(0.0).min(bounds.width);
        let items_bounds = Rect::new(
            bounds.x + leading_width,
            bounds.y,
            (bounds.width - leading_width).max(0.0),
            bounds.height,
        );
        let mut layout = self.layout(items_bounds, measure_item);
        layout.bounds = bounds;
        layout
    }

    /// Find the index of the item whose label contains the Alt-key
    /// character `ch` (case-insensitive). The label's `&` prefix marks
    /// the activation character; if no `&`, the first character is
    /// used.
    pub fn find_alt_target(&self, ch: char) -> Option<usize> {
        let target = ch.to_ascii_lowercase();
        for (i, item) in self.items.iter().enumerate() {
            if item.disabled {
                continue;
            }
            let marker = item.label.find('&').map(|p| p + 1);
            let trigger = match marker {
                Some(idx) => item.label.chars().nth(idx),
                None => item.label.chars().next(),
            };
            if let Some(c) = trigger {
                if c.to_ascii_lowercase() == target {
                    return Some(i);
                }
            }
        }
        None
    }
}

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────

/// Total horizontal padding a pixel surface reserves around one
/// top-level item's label — half either side, so an item's hit slot is
/// its measured label plus this. One metric for GTK, macOS and Windows:
/// all three reserved 8 units a side before the strip became one
/// [`Primitive`].
pub const ITEM_H_PADDING: f32 = 16.0;

/// Columns a cell grid reserves around one item's label — one blank
/// column either side of the glyphs.
pub const CELL_ITEM_PADDING: f32 = 2.0;

/// Strip `&` markers from a label for display: the text a rasteriser
/// actually paints, and the string [`MenuBar::pixel_layout`] measures.
pub fn display_text(label: &str) -> String {
    label.chars().filter(|&c| c != '&').collect()
}

/// The **char index** into [`display_text`]'s output of the Alt-activation
/// character (the one immediately after `&`), or `None` when `label`
/// carries no `&` at all — there is no implicit "underline the first
/// char" fallback, unrelated to [`MenuBar::find_alt_target`]'s keyboard
/// lookup, which does fall back.
///
/// A char index, not a byte index: both painters address the display
/// string by glyph (`chars().nth`, and a cell grid's own column
/// accounting), never by byte range.
pub fn alt_char_index(label: &str) -> Option<usize> {
    let marker_byte = label.find('&')?;
    Some(label[..marker_byte].chars().count())
}

/// Columns one item occupies on a cell grid: its display glyphs — wide
/// (CJK, emoji) glyphs claiming two columns each, per
/// [`crate::text_util::display_width`] — plus [`CELL_ITEM_PADDING`].
pub fn cell_item_width(label: &str) -> f32 {
    crate::text_util::display_width(&display_text(label)) as f32 + CELL_ITEM_PADDING
}

impl MenuBar {
    /// Lay the strip out on a pixel surface: each item's slot is
    /// `measure(label)` (its display text in the surface's own chrome
    /// font) plus [`ITEM_H_PADDING`].
    ///
    /// The one measurement rule GTK, macOS and Windows share, so a
    /// backend's `*_menu_bar_layout` twin passes its own text measurer
    /// here instead of re-deriving the padding.
    pub fn pixel_layout(&self, bounds: Rect, measure: impl Fn(&str) -> f32) -> MenuBarLayout {
        self.layout(bounds, |i| {
            MenuBarItemMeasure::new(measure(&display_text(&self.items[i].label)) + ITEM_H_PADDING)
        })
    }

    /// Lay the strip out on a cell grid: each item's slot is
    /// [`cell_item_width`] columns.
    pub fn cell_layout(&self, bounds: Rect) -> MenuBarLayout {
        self.layout(bounds, |i| {
            MenuBarItemMeasure::new(cell_item_width(&self.items[i].label))
        })
    }

    /// The item a keyboard move starts from: [`Self::focused_item`], else
    /// [`Self::open_item`], else nothing (so a first `Right` seeds at the
    /// leading item and a first `Left` at the trailing one).
    fn nav_cursor(&self) -> Option<usize> {
        self.focused_item.or(self.open_item)
    }

    /// Resolve one keystroke against the strip — the single key→meaning
    /// rule [`Interactive::handle`] dispatches through, so every host
    /// driving a menu bar agrees on what a key does.
    fn handle_key(&self, key: &KeyEvent) -> Handled<MenuBarEvent> {
        let enabled = |i: usize| !self.items[i].disabled;
        match key.key {
            // Alt+<mnemonic> opens that item's menu wherever the strip's
            // own focus currently sits — the convention the painted
            // underline advertises.
            Key::Char(ch) if key.modifiers.alt => match self.find_alt_target(ch) {
                Some(idx) => self.open_event(idx),
                None => Handled::Ignored,
            },
            // F10 is the keyboard route *into* the strip: it parks the
            // cursor on the leading enabled item without opening it.
            Key::Named(NamedKey::F(10)) => {
                match FlatNav::mv(self.items.len(), None, NavKey::Down, false, enabled) {
                    Some(idx) if Some(idx) != self.focused_item => {
                        Handled::Event(MenuBarEvent::FocusMoved { idx })
                    }
                    Some(_) => Handled::Consumed,
                    None => Handled::Ignored,
                }
            }
            Key::Named(NamedKey::Escape) => {
                if self.open_item.is_some() || self.focused_item.is_some() {
                    Handled::Event(MenuBarEvent::Closed)
                } else {
                    Handled::Ignored
                }
            }
            // Down opens the focused item's menu; Enter does the same, so
            // a strip reached by F10 opens without the user guessing
            // which of the two this widget wanted.
            Key::Named(NamedKey::Down) | Key::Named(NamedKey::Enter) => match self.nav_cursor() {
                Some(idx) if idx < self.items.len() && !self.items[idx].disabled => {
                    self.open_event(idx)
                }
                _ => Handled::Ignored,
            },
            Key::Named(NamedKey::Left) | Key::Named(NamedKey::Right) => {
                let nav = if matches!(key.key, Key::Named(NamedKey::Left)) {
                    NavKey::Up
                } else {
                    NavKey::Down
                };
                // Wrapping: a strip is a short, closed ring of menus, so
                // Right past the last menu lands back on the first —
                // the same traversal every desktop menu bar offers.
                match FlatNav::mv(self.items.len(), self.nav_cursor(), nav, true, enabled) {
                    // A move while a menu is open carries the open state
                    // with it, which is what makes Left/Right feel like
                    // "walk the menus" rather than "walk the labels".
                    Some(idx) if self.open_item.is_some() => self.open_event(idx),
                    Some(idx) if Some(idx) != self.focused_item => {
                        Handled::Event(MenuBarEvent::FocusMoved { idx })
                    }
                    Some(_) => Handled::Consumed,
                    None => Handled::Ignored,
                }
            }
            _ => Handled::Ignored,
        }
    }

    /// Ask the host to open item `idx`'s menu, or report nothing when it
    /// is already the open one — a second Enter on an open menu is the
    /// keystroke the dropdown itself should see, not a re-open.
    fn open_event(&self, idx: usize) -> Handled<MenuBarEvent> {
        if self.open_item == Some(idx) {
            return Handled::Consumed;
        }
        match self.items.get(idx) {
            Some(item) if !item.disabled => {
                Handled::Event(MenuBarEvent::MenuOpened { idx, item: item.id })
            }
            _ => Handled::Ignored,
        }
    }
}

/// The semantic outcomes a [`MenuBar`] strip reports — its
/// [`Interactive::Action`].
///
/// [`MenuBar`] stays purely declarative: no variant below mutates the
/// strip. Each names the state the host writes back onto its own
/// `MenuBar` before the next frame — the same contract
/// [`crate::ListViewEvent`] carries for a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuBarEvent {
    /// The keyboard cursor (or the pointer, which the strip tracks the
    /// same way) moved to item `idx` without opening it. The host writes
    /// it onto [`MenuBar::focused_item`].
    FocusMoved { idx: usize },
    /// Item `idx` asked for its menu: the host writes
    /// `open_item = Some(idx)` (and, conventionally,
    /// `focused_item = Some(idx)`) and paints the dropdown —
    /// `item` is that top-level entry's own [`MenuBarItem::id`], which
    /// a native installer reports under the same identity.
    MenuOpened { idx: usize, item: WidgetId },
    /// Escape left the strip: the host clears both
    /// [`MenuBar::open_item`] and [`MenuBar::focused_item`].
    Closed,
}

impl MenuBar {
    /// Shows this bar in `rect` the way `backend`'s menu style asks for,
    /// returning the layout clicks resolve against.
    ///
    /// Under [`ResolvedMenuStyle::Native`] (macOS by default) the bar is
    /// installed as the platform's own menu bar through
    /// [`Backend::install_menu_bar`], which skips an unchanged bar, so
    /// calling this every frame is cheap. Nothing paints in the window,
    /// and the layout is [`Self::presented_layout`]'s zero-height one.
    /// Otherwise the strip paints through
    /// [`DrawsPrimitives::draw_dyn`] and the layout is the widget
    /// contract's own.
    ///
    /// Every caller that shows a standalone menu bar goes through this
    /// (frame replay, the binding layer, [`crate::compose::MenuSystem`]),
    /// so the native-or-painted decision lives in one place.
    pub fn present(&self, backend: &mut dyn Backend, rect: Rect) -> MenuBarLayout {
        if backend.effective_menu_style() == ResolvedMenuStyle::Native {
            backend.install_menu_bar(self);
            return native_layout(rect);
        }
        let _ = backend.draw_dyn(rect, self, &());
        backend.layout_primitive(rect, self).layout
    }

    /// The layout [`Self::present`] returns for `rect`, without painting
    /// or installing anything: a zero-height layout with no items under
    /// [`ResolvedMenuStyle::Native`], where the platform's menu bar lives
    /// outside the window and nothing in it can be hit; otherwise the
    /// widget contract's own layout.
    pub fn presented_layout(&self, backend: &dyn Backend, rect: Rect) -> MenuBarLayout {
        if backend.effective_menu_style() == ResolvedMenuStyle::Native {
            return native_layout(rect);
        }
        backend.layout_primitive(rect, self).layout
    }
}

/// The layout of a bar installed as the platform's own menu bar: its
/// origin and width, no height and no items.
fn native_layout(rect: Rect) -> MenuBarLayout {
    MenuBarLayout {
        bounds: Rect::new(rect.x, rect.y, rect.width, 0.0),
        visible_items: Vec::new(),
        hit_regions: Vec::new(),
    }
}

impl WidgetAction for MenuBarEvent {
    /// [`Self::MenuOpened`] is the one strip outcome the crate's
    /// [`crate::UiEvent`] vocabulary names directly
    /// ([`crate::UiEvent::MenuActivated`], under the opened item's own
    /// id — the same identity a native menu installer reports, so an app
    /// routes both through one handler).
    ///
    /// The other two ride [`crate::UiEvent::List`] under the strip's own
    /// [`MenuBar::id`], the channel [`crate::Completions`] already uses
    /// for the same two shapes: a moved cursor is exactly
    /// [`crate::ListViewEvent::SelectionChanged`]'s `{ idx }`, and a
    /// close is the Escape keystroke the strip saw but did not act on
    /// itself, which [`crate::ListViewEvent::KeyPressed`] carries.
    /// Reusing that channel keeps both outcomes loss-free under one id
    /// rather than mapping them onto an unrelated event an existing
    /// handler could act on.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            MenuBarEvent::MenuOpened { item, .. } => crate::UiEvent::MenuActivated(item),
            MenuBarEvent::FocusMoved { idx } => {
                crate::UiEvent::List(id, crate::ListViewEvent::SelectionChanged { idx })
            }
            MenuBarEvent::Closed => crate::UiEvent::List(
                id,
                crate::ListViewEvent::KeyPressed {
                    key: NamedKey::Escape.name().into_owned(),
                    modifiers: crate::Modifiers::default(),
                },
            ),
        }
    }
}

/// [`MenuBar`]'s fully-resolved widget-contract layout: the absolute
/// `area` the strip was laid out in, the row height its items were
/// resolved at, and the strip-local [`MenuBarLayout`] itself.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer via struct literal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MenuBarPrimitiveLayout {
    /// The rect [`Primitive::layout`] was called with.
    pub area: Rect,
    /// Row height the items were resolved at: [`Self::area`]'s own
    /// height, since every item claims one row of the strip's full
    /// height. A host that wants the strip's *natural* height asks
    /// [`Primitive::measure`], which applies [`resolve_bar_height`] to
    /// its line height.
    pub bar_height: f32,
    /// The resolved strip layout. Its `bounds`/`visible_items`/
    /// `hit_regions` are in the same absolute space as [`Self::area`].
    pub layout: MenuBarLayout,
}

impl MenuBarPrimitiveLayout {
    /// Hit-test at a position relative to [`Self::area`]'s own origin —
    /// the space [`crate::interactive::RoutedEvent::Pointer`]'s `local`
    /// field uses.
    pub fn hit_test_local(&self, x: f32, y: f32) -> MenuBarHit {
        self.layout.hit_test(self.area.x + x, self.area.y + y)
    }
}

impl Primitive for MenuBar {
    type Layout = MenuBarPrimitiveLayout;
    type Event = MenuBarEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Under [`ResolvedMenuStyle::Native`] the bar is installed as the
    /// platform's own menu bar ([`Self::present`]) and nothing paints in
    /// the window.
    fn native_presentation(&self, backend: &mut dyn Backend, rect: Rect) -> bool {
        if backend.effective_menu_style() != ResolvedMenuStyle::Native {
            return false;
        }
        let _ = self.present(backend, rect);
        true
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::MenuBar(self.clone())
    }

    /// The strip's own content width — every item's slot laid end to end
    /// — and one [`resolve_bar_height`] row tall. A strip wider than its
    /// container simply drops the items that do not fit (see
    /// [`MenuBar::layout`]), so this is a natural size, not a minimum.
    ///
    /// The line height is floored at one unit first, so a ctx carrying
    /// none still reports a one-row strip rather than a zero-height one
    /// — the same floor [`crate::ContextMenu`]'s own `measure` applies
    /// to its row pitch.
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> crate::NaturalSize {
        let width: f32 = match ctx.units() {
            Units::Cells => self
                .items
                .iter()
                .map(|item| cell_item_width(&item.label))
                .sum(),
            _ => self
                .items
                .iter()
                .map(|item| {
                    ctx.text_width(&display_text(&item.label), crate::FontRole::Chrome)
                        + ITEM_H_PADDING
                })
                .sum(),
        };
        let line_height = if ctx.line_height > 0.0 {
            ctx.line_height
        } else {
            1.0
        };
        crate::NaturalSize::new(Some(width), Some(resolve_bar_height(line_height)))
    }

    /// `rect` is the whole strip: items start at its leading edge and
    /// each claims one row of its full height, through
    /// [`MenuBar::cell_layout`] on a cell grid and
    /// [`MenuBar::pixel_layout`] (measuring through
    /// [`LayoutCtx::text_width`]) on a pixel surface — so paint and
    /// hit-testing resolve one derivation, in `rect`'s own absolute
    /// space.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> MenuBarPrimitiveLayout {
        let layout = match ctx.units() {
            Units::Cells => self.cell_layout(rect),
            _ => self.pixel_layout(rect, |text| ctx.text_width(text, crate::FontRole::Chrome)),
        };
        MenuBarPrimitiveLayout {
            area: rect,
            bar_height: rect.height,
            layout,
        }
    }

    /// The shared pixel painter, `native_surface_paint::paint`, tinting
    /// the item the pointer hovers or presses from `env`. A painted strip
    /// is an in-window menu bar on every backend, so it underlines each
    /// label's `&` mnemonic: [`Self::find_alt_target`] opens that menu
    /// wherever the strip paints, and a native menu bar paints nothing.
    fn paint(
        &self,
        layout: &MenuBarPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let pointer = crate::primitives::context_menu::PointerTint::from_env(&self.id, env);
            native_surface_paint::paint(self, &layout.layout, surface, theme, pointer);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme, env);
    }

    /// [`cell_paint::paint`] over `cells`, with the pointer's tint from
    /// `env`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &MenuBarPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let pointer = crate::primitives::context_menu::PointerTint::from_env(&self.id, env);
        cell_paint::paint(self, &layout.layout, cells, theme, pointer);
    }
}

impl Interactive for MenuBar {
    /// Nothing: the cursor lives on [`MenuBar::focused_item`] and the
    /// open menu on [`MenuBar::open_item`], both of which the host writes
    /// from [`MenuBarEvent`] — including the pointer's own hover, which
    /// this strip reports as a cursor move rather than keeping a second,
    /// paint-only hover field the descriptor would contradict.
    type State = ();
    type Action = MenuBarEvent;

    /// One focus stop as a whole: the strip navigates internally through
    /// [`Self::handle`]'s own key dispatch rather than declaring a stop
    /// per menu. [`FocusSpec::None`] for an empty strip.
    fn focus(&self) -> FocusSpec {
        if self.items.is_empty() {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn handle(
        &self,
        _state: &mut (),
        event: &RoutedEvent,
        layout: &MenuBarPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<MenuBarEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => match layout.hit_test_local(local.x, local.y) {
                MenuBarHit::Item(idx) => {
                    cx.request_focus(None);
                    // Click to open, click again to close: the toggle
                    // every desktop menu bar offers on its own label.
                    if self.open_item == Some(idx) {
                        Handled::Event(MenuBarEvent::Closed)
                    } else {
                        self.open_event(idx)
                    }
                }
                // The strip's own background: a click there belongs to
                // the strip (it must not fall through to whatever is
                // behind the chrome row), but changes nothing.
                MenuBarHit::Bar => {
                    cx.request_focus(None);
                    Handled::Consumed
                }
                MenuBarHit::Outside => Handled::Ignored,
            },
            // Hover tracks the cursor, and — while a menu is open —
            // drags the open menu with it, the "slide along the bar"
            // behaviour a mouse-driven menu bar has.
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } => match layout.hit_test_local(local.x, local.y) {
                MenuBarHit::Item(idx) if self.open_item.is_some() => self.open_event(idx),
                MenuBarHit::Item(idx) if Some(idx) != self.focused_item => {
                    Handled::Event(MenuBarEvent::FocusMoved { idx })
                }
                MenuBarHit::Item(_) => Handled::Consumed,
                MenuBarHit::Bar | MenuBarHit::Outside => Handled::Ignored,
            },
            RoutedEvent::Key(key) => self.handle_key(key),
            // `A11yAction` carries no item index through this generic
            // seam, so `Click` opens whichever menu the cursor sits on —
            // the same one Enter opens, matching the assistive-technology
            // default-action convention.
            RoutedEvent::Action(A11yAction::Click) => match self.nav_cursor() {
                Some(idx) => self.open_event(idx),
                None => Handled::Ignored,
            },
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &()) -> Vec<A11yAction> {
        if self.items.is_empty() {
            Vec::new()
        } else {
            vec![A11yAction::Click]
        }
    }

    /// One sub-part per visible item, addressed by its index into
    /// [`MenuBar::items`], in [`Self::handle`]'s own local space.
    fn sub_regions(&self, layout: &MenuBarPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        layout
            .layout
            .visible_items
            .iter()
            .map(|vi| {
                (
                    Rect::new(
                        vi.bounds.x - layout.area.x,
                        vi.bounds.y - layout.area.y,
                        vi.bounds.width,
                        vi.bounds.height,
                    ),
                    SubTarget(vi.item_idx as u64),
                )
            })
            .collect()
    }
}

// Shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API. Manual-rectangle underline approach for the Alt-activation
// character: measure the prefix and character width, then fill a
// thin rectangle beneath it. This maps to [`PaintSurface`]'s existing
// verbs ([`crate::paint_surface::PaintSurface::surface_measure_text`] +
// [`crate::paint_surface::PaintSurface::surface_fill_rect`]) and works
// across all backends.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{alt_char_index, display_text, MenuBar, MenuBarLayout};
    use crate::event::Rect;
    use crate::interactive::SubTarget;
    use crate::paint_surface::PaintSurface;
    use crate::primitives::context_menu::PointerTint;
    use crate::theme::Theme;

    /// Thickness (surface-native units) of the Alt-key underline
    /// rectangle.
    const UNDERLINE_HEIGHT: f32 = 2.0;

    /// Paint a [`MenuBar`] at its caller-resolved `layout` onto
    /// `surface`.
    ///
    /// # Visual contract
    ///
    /// - **Background:** filled with `theme.tab_bar_bg`.
    /// - **Open/focused item:** `theme.tab_active_bg` fill,
    ///   `theme.tab_active_fg` label.
    /// - **Disabled item:** `theme.muted_fg` label, no fill.
    /// - **Alt-underline:** a [`UNDERLINE_HEIGHT`]-tall bar under the
    ///   character following `&` in the raw label, in the label's own
    ///   foreground colour. No `&` in the label ⇒ no underline at all.
    ///
    /// `pointer` names the items the pointer hovers and presses, by item
    /// index, and tints them under [`PointerTint::row_bg`]'s rule: an
    /// open or focused item keeps [`Theme::tab_active_bg`] under hover,
    /// a pressed item takes [`Theme::menu_item_pressed_bg`], and any
    /// other hovered item takes [`Theme::menu_item_hover_bg`]. A disabled
    /// item never tints.
    pub(crate) fn paint(
        bar: &MenuBar,
        layout: &MenuBarLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        pointer: PointerTint,
    ) {
        surface.surface_fill_rect(layout.bounds, theme.tab_bar_bg);

        for vi in &layout.visible_items {
            let item = &bar.items[vi.item_idx];
            let is_active =
                bar.open_item == Some(vi.item_idx) || bar.focused_item == Some(vi.item_idx);
            let (fg, bg) = if is_active {
                (theme.tab_active_fg, theme.tab_active_bg)
            } else if item.disabled {
                (theme.muted_fg, theme.tab_bar_bg)
            } else {
                (theme.tab_inactive_fg, theme.tab_bar_bg)
            };
            // `is_active` (open or keyboard-focused) is the emphasis
            // hover must not weaken: `focused_item` is also how the
            // closed bar follows the mouse, so the focused item is often
            // the hovered one too.
            let row_bg = pointer.row_bg(
                SubTarget(vi.item_idx as u64),
                !item.disabled,
                is_active.then_some(bg),
                theme.tab_bar_bg,
                theme,
            );

            if let Some(row_bg) = row_bg {
                surface.surface_fill_rect(vi.bounds, row_bg);
            }

            let text = display_text(&item.label);
            let (text_w, text_h) = surface.surface_measure_text(&text);
            let text_x = vi.bounds.x + (vi.bounds.width - text_w) / 2.0;
            let text_y = vi.bounds.y + (vi.bounds.height - text_h) / 2.0;
            surface.surface_draw_text_run(
                Rect::new(text_x, text_y, text_w.max(0.0), text_h.max(0.0)),
                &text,
                fg,
            );

            if let Some(idx) = alt_char_index(&item.label) {
                if let Some(ch) = text.chars().nth(idx) {
                    let prefix: String = text.chars().take(idx).collect();
                    let (prefix_w, _) = surface.surface_measure_text(&prefix);
                    let (char_w, _) = surface.surface_measure_text(&ch.to_string());
                    let underline_rect = Rect::new(
                        text_x + prefix_w,
                        text_y + text_h - UNDERLINE_HEIGHT,
                        char_w.max(1.0),
                        UNDERLINE_HEIGHT,
                    );
                    surface.surface_fill_rect(underline_rect, fg);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // ── paint()'s Alt-underline drift fix ───────────────────────────
        //
        // These run through `paint` itself against a `RecordingSurface`
        // test double (mirrors `primitives::palette::native_surface_paint`'s
        // pattern) rather than any one platform's real Cairo/Core
        // Graphics/Direct2D calls — deterministic on every host, and the
        // one test that proves the behaviour every backend now shares,
        // including macOS, which painted no underline at all before this
        // port (see this module's own doc above).
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::primitives::menu_bar::{MenuBarItem, MenuBarItemMeasure};
        use crate::types::{Color, WidgetId};
        use crate::Image;

        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            texts: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(300.0, 20.0, 1.0)
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
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
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

        fn drift_bar() -> MenuBar {
            MenuBar {
                id: WidgetId::new("bar"),
                items: vec![
                    MenuBarItem {
                        id: WidgetId::new("file"),
                        label: "&File".into(),
                        disabled: false,
                        submenu: None,
                    },
                    MenuBarItem {
                        id: WidgetId::new("help"),
                        label: "Help".into(),
                        disabled: false,
                        submenu: None,
                    },
                ],
                open_item: Some(0),
                focused_item: None,
            }
        }

        fn drift_layout(bar: &MenuBar) -> MenuBarLayout {
            let bounds = Rect::new(0.0, 0.0, 300.0, 20.0);
            bar.layout(bounds, |i| {
                let text = display_text(&bar.items[i].label);
                MenuBarItemMeasure::new(text.chars().count() as f32 * 8.0 + 16.0)
            })
        }

        #[test]
        fn paints_alt_underline_as_a_filled_rect_under_the_activation_char() {
            let bar = drift_bar();
            let layout = drift_layout(&bar);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(&bar, &layout, &mut surface, &theme, PointerTint::default());

            let (label_rect, _, _) = surface
                .texts
                .iter()
                .find(|(_, t, _)| t == "File")
                .expect("File label painted");
            // 'F' is the activation char (index 0, empty prefix) — its
            // underline rect starts flush with the label's own x/bottom
            // edge and is one char's measured width
            // (`RecordingSurface::surface_measure_text`'s `8.0`/char).
            let expected = Rect::new(
                label_rect.x,
                label_rect.y + label_rect.height - UNDERLINE_HEIGHT,
                8.0,
                UNDERLINE_HEIGHT,
            );
            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| (r.x - expected.x).abs() < 0.01
                        && (r.y - expected.y).abs() < 0.01
                        && (r.width - expected.width).abs() < 0.01
                        && (r.height - expected.height).abs() < 0.01
                        && *c == theme.tab_active_fg),
                "expected an underline fill at {expected:?} in {:?}, got fills: {:?}",
                theme.tab_active_fg,
                surface.fills,
            );
        }

        #[test]
        fn no_ampersand_means_no_underline_fill_beneath_the_label() {
            // No `&` in the label means no underline-shaped fill beneath it.
            let bar = drift_bar();
            let layout = drift_layout(&bar);
            let mut surface = RecordingSurface::default();
            paint(
                &bar,
                &layout,
                &mut surface,
                &Theme::default(),
                PointerTint::default(),
            );

            let help_item = layout
                .visible_items
                .iter()
                .find(|vi| bar.items[vi.item_idx].label == "Help")
                .expect("Help item visible");
            let thin_fills_under_help = surface
                .fills
                .iter()
                .filter(|(r, _)| {
                    r.height <= UNDERLINE_HEIGHT
                        && r.x >= help_item.bounds.x
                        && r.x < help_item.bounds.x + help_item.bounds.width
                })
                .count();
            assert_eq!(
                thin_fills_under_help, 0,
                "Help has no '&' and must get no underline fill"
            );
        }

        /// The tint `bar`'s painter receives with the pointer on item
        /// `hovered` and pressing item `pressed`, read back through a
        /// [`crate::PaintEnv`] the way the widget's own paint reads it.
        fn tint(bar: &MenuBar, (hovered, pressed): (Option<usize>, Option<usize>)) -> PointerTint {
            let theme = Theme::default();
            let style = crate::Style::default();
            let at = |idx: Option<usize>| idx.map(|i| (bar.id, Some(SubTarget(i as u64))));
            let interaction = crate::InteractionState::from_targets(at(hovered), at(pressed));
            let env = crate::PaintEnv::new(&theme, &style).with_interaction(&interaction);
            PointerTint::from_env(&bar.id, &env)
        }

        fn idle_bar() -> MenuBar {
            let mut bar = drift_bar();
            bar.open_item = None;
            bar.focused_item = None;
            bar
        }

        #[test]
        fn hovered_item_paints_menu_item_hover_bg() {
            let bar = idle_bar();
            let layout = drift_layout(&bar);
            let theme = Theme::default();
            let interaction = (Some(0), None);
            let mut surface = RecordingSurface::default();
            paint(&bar, &layout, &mut surface, &theme, tint(&bar, interaction));

            let expected = theme.menu_item_hover_bg(theme.tab_bar_bg);
            assert!(
                surface.fills.iter().any(|(_, c)| *c == expected),
                "hovered menu-bar item must paint Theme::menu_item_hover_bg: {:?}",
                surface.fills
            );
        }

        #[test]
        fn hovering_the_closed_bars_focused_item_keeps_active_bg_not_hover_bg() {
            // On a closed bar a pointer move sets `focused_item` to the
            // hovered item, so that item is both active and hovered at
            // once. The stronger `tab_active_bg` must win, not the
            // fainter hover tint.
            let mut bar = drift_bar();
            bar.open_item = None;
            bar.focused_item = Some(0);
            let layout = drift_layout(&bar);
            let theme = Theme::default();
            let interaction = (Some(0), None);
            let mut surface = RecordingSurface::default();
            paint(&bar, &layout, &mut surface, &theme, tint(&bar, interaction));

            let hover_bg = theme.menu_item_hover_bg(theme.tab_bar_bg);
            assert!(
                !surface.fills.iter().any(|(_, c)| *c == hover_bg),
                "the focused (hovered) item must not downgrade to the hover tint: {:?}",
                surface.fills
            );
            assert!(
                surface.fills.iter().any(|(_, c)| *c == theme.tab_active_bg),
                "the focused (hovered) item must keep theme.tab_active_bg: {:?}",
                surface.fills
            );
        }

        #[test]
        fn leaving_the_item_clears_the_hover_fill() {
            let bar = idle_bar();
            let layout = drift_layout(&bar);
            let theme = Theme::default();
            let interaction = (None, None); // nothing hovered
            let mut surface = RecordingSurface::default();
            paint(&bar, &layout, &mut surface, &theme, tint(&bar, interaction));

            let hover_bg = theme.menu_item_hover_bg(theme.tab_bar_bg);
            assert!(
                !surface.fills.iter().any(|(_, c)| *c == hover_bg),
                "no item is hovered, so no item should paint the hover tint: {:?}",
                surface.fills
            );
        }

        #[test]
        fn disabled_item_is_never_tinted_even_when_fed_as_hovered() {
            // The pointer can rest on a disabled item, which must not
            // tint as though it could be clicked.
            let mut bar = idle_bar();
            bar.items[0].disabled = true;
            let layout = drift_layout(&bar);
            let theme = Theme::default();
            let interaction = (Some(0), Some(0));
            let mut surface = RecordingSurface::default();
            paint(&bar, &layout, &mut surface, &theme, tint(&bar, interaction));

            let hover_bg = theme.menu_item_hover_bg(theme.tab_bar_bg);
            let pressed_bg = theme.menu_item_pressed_bg(theme.tab_bar_bg);
            assert!(
                !surface
                    .fills
                    .iter()
                    .any(|(_, c)| *c == hover_bg || *c == pressed_bg),
                "a disabled item fed as hovered/pressed must not tint: {:?}",
                surface.fills
            );
        }

        #[test]
        fn pressed_item_paints_menu_item_pressed_bg_not_hover_bg() {
            let bar = idle_bar();
            let layout = drift_layout(&bar);
            let theme = Theme::default();
            let interaction = (Some(0), Some(0));
            let mut surface = RecordingSurface::default();
            paint(&bar, &layout, &mut surface, &theme, tint(&bar, interaction));

            let pressed = theme.menu_item_pressed_bg(theme.tab_bar_bg);
            let hover = theme.menu_item_hover_bg(theme.tab_bar_bg);
            assert!(
                surface.fills.iter().any(|(_, c)| *c == pressed),
                "pressed menu-bar item must paint Theme::menu_item_pressed_bg: {:?}",
                surface.fills
            );
            assert!(
                !surface.fills.iter().any(|(_, c)| *c == hover),
                "a pressed item must not also paint the plain hover tint: {:?}",
                surface.fills
            );
        }
    }
}

/// The cell-grid painter for [`MenuBar`], written against
/// [`crate::CellSurface`].
///
/// One row: the strip's background in [`crate::Theme::tab_bar_bg`], then
/// each visible item's label painted one column in from its own slot.
/// An open or keyboard-focused item fills its whole slot with
/// [`crate::Theme::tab_active_bg`]/`tab_active_fg`; a disabled item's
/// label is [`crate::Theme::muted_fg`]; every other label is
/// [`crate::Theme::tab_inactive_fg`]. The Alt-activation character — the
/// one after `&` in the raw label, per [`alt_char_index`] — is
/// underlined, and a label with no `&` is never underlined. Wide (CJK,
/// emoji) glyphs claim two columns, so a label's painted columns match
/// the width [`cell_item_width`] measured.
///
/// The same content `native_surface_paint::paint` puts on a pixel
/// surface, degraded onto the cell grid (see [`crate::paint_surface`]'s
/// module doc, "Why TUI stays out").
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{alt_char_index, MenuBar, MenuBarLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::interactive::SubTarget;
    use crate::primitives::context_menu::PointerTint;
    use crate::theme::Theme;
    use crate::types::Color;

    /// An opaque single-cell style — colours only, no modifier.
    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `bar` at its caller-resolved `layout`. `layout.bounds` is
    /// the strip's own row in the surface's absolute cell space, so the
    /// painted glyph columns and the columns
    /// [`MenuBarLayout::hit_test`] reports are the same. `pointer` tints
    /// the hovered or pressed item under [`PointerTint::row_bg`]'s rule,
    /// the same one the pixel painter follows.
    pub(crate) fn paint(
        bar: &MenuBar,
        layout: &MenuBarLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        pointer: PointerTint,
    ) {
        let bounds = layout.bounds;
        let width = bounds.width.round() as u16;
        let height = bounds.height.round() as u16;
        if width == 0 || height == 0 {
            return;
        }
        let x0 = bounds.x.round() as u16;
        let y = bounds.y.round() as u16;
        let right = x0.saturating_add(width);

        let bar_bg = theme.tab_bar_bg;
        for col in 0..width {
            surface.put_char(x0 + col, y, ' ', solid(bar_bg, bar_bg));
        }

        for vi in &layout.visible_items {
            let item = &bar.items[vi.item_idx];
            let is_active =
                bar.open_item == Some(vi.item_idx) || bar.focused_item == Some(vi.item_idx);

            let (fg, bg) = if is_active {
                (theme.tab_active_fg, theme.tab_active_bg)
            } else if item.disabled {
                (theme.muted_fg, bar_bg)
            } else {
                (theme.tab_inactive_fg, bar_bg)
            };
            let tint = pointer.row_bg(
                SubTarget(vi.item_idx as u64),
                !item.disabled,
                is_active.then_some(bg),
                bar_bg,
                theme,
            );
            let bg = tint.unwrap_or(bg);

            // `vi.bounds.x` is absolute — `MenuBar::layout` starts its
            // cursor at the bounds' own origin — so the painted glyph
            // position and the hit-test bounds share one frame.
            let start_x = vi.bounds.x.round() as u16;
            let end_x = start_x + vi.bounds.width.round() as u16;

            if tint.is_some() {
                for col in start_x..end_x.min(right) {
                    surface.put_char(col, y, ' ', solid(fg, bg));
                }
            }

            let underline_pos = alt_char_index(&item.label);
            let mut cx = start_x + 1; // one column of left padding
            let mut char_idx: usize = 0;
            for ch in item.label.chars() {
                if ch == '&' {
                    continue;
                }
                if cx >= right {
                    break;
                }
                let w = crate::text_util::char_cell_width(ch);
                let mut style = solid(fg, bg);
                if underline_pos == Some(char_idx) {
                    // The underline takes the glyph's own colour: a
                    // `CellStyle` has no "underline in the default
                    // colour" state, and matching the foreground is what
                    // every terminal draws for a plain SGR 4 anyway.
                    style.underline = Some(fg);
                }
                if w == 2 && cx + 1 < right {
                    surface.put_wide(cx, y, ch, style);
                } else {
                    surface.put_char(cx, y, ch, style);
                }
                cx += w;
                char_idx += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `display_text` / `alt_char_index` are the label-parsing rule both
    // `cell_paint` and `native_surface_paint` resolve an item's
    // Alt-mnemonic through, so their coverage is unconditional rather
    // than tied to a GUI feature.

    #[test]
    fn alt_char_index_none_without_ampersand() {
        assert_eq!(alt_char_index("File"), None);
    }

    #[test]
    fn alt_char_index_marks_char_after_ampersand() {
        assert_eq!(alt_char_index("&File"), Some(0));
        // '&' isn't necessarily the first char — "Sa&ve" underlines
        // 'v' (char index 2 in "Save").
        assert_eq!(alt_char_index("Sa&ve"), Some(2));
    }

    #[test]
    fn alt_char_index_handles_empty_and_trailing_marker() {
        assert_eq!(alt_char_index(""), None);
        // A trailing `&` marks a char index past the display string's
        // end — a painter's `text.chars().nth(idx)` guard turns that
        // into "no underline" rather than panicking.
        assert_eq!(alt_char_index("File&"), Some(4));
        assert_eq!(display_text("File&").chars().nth(4), None);
    }

    #[test]
    fn display_text_strips_ampersand() {
        assert_eq!(display_text("&File"), "File");
        assert_eq!(display_text("Sa&ve"), "Save");
        assert_eq!(display_text("File"), "File");
    }

    fn bar(n: usize) -> MenuBar {
        MenuBar {
            id: WidgetId::new("bar"),
            items: (0..n)
                .map(|i| MenuBarItem {
                    id: WidgetId::new(format!("item{i}")),
                    label: format!("Item{i}"),
                    disabled: false,
                    submenu: None,
                })
                .collect(),
            open_item: None,
            focused_item: None,
        }
    }

    // A leading icon slot (app logo left of the first menu item)
    // must shift every item's x-offset by exactly `leading_width`, and a
    // click that used to land on item 0 pre-shift must land on item 0
    // again post-shift, at its new (shifted) x.
    #[test]
    fn leading_icon_shifts_item_x_offsets_by_its_width() {
        let bounds = Rect::new(0.0, 0.0, 200.0, 1.0);
        let measure = |_: usize| MenuBarItemMeasure::new(20.0);
        let leading_width = 32.0;

        let plain = bar(3).layout(bounds, measure);
        let with_icon = bar(3).layout_with_leading(bounds, leading_width, measure);

        assert_eq!(plain.visible_items.len(), with_icon.visible_items.len());
        for (p, w) in plain.visible_items.iter().zip(&with_icon.visible_items) {
            assert_eq!(w.bounds.x, p.bounds.x + leading_width);
            assert_eq!(w.bounds.width, p.bounds.width);
        }
    }

    #[test]
    fn leading_icon_click_on_first_item_still_hits_it_at_its_shifted_x() {
        let bounds = Rect::new(0.0, 0.0, 200.0, 1.0);
        let measure = |_: usize| MenuBarItemMeasure::new(20.0);
        let leading_width = 32.0;

        let layout = bar(3).layout_with_leading(bounds, leading_width, measure);
        let item0 = &layout.visible_items[0];
        assert_eq!(item0.bounds.x, leading_width);

        // Click in the middle of item 0's shifted bounds.
        let hit = layout.hit_test(item0.bounds.x + 1.0, 0.0);
        assert_eq!(hit, MenuBarHit::Item(0));

        // Click over the reserved icon region (left of the shift) is
        // still "inside the bar" — `bounds` covers the full width —
        // but doesn't land on any item.
        let icon_hit = layout.hit_test(leading_width / 2.0, 0.0);
        assert_eq!(icon_hit, MenuBarHit::Bar);
    }

    #[test]
    fn zero_leading_width_matches_plain_layout() {
        let bounds = Rect::new(0.0, 0.0, 200.0, 1.0);
        let measure = |_: usize| MenuBarItemMeasure::new(20.0);

        let plain = bar(3).layout(bounds, measure);
        let zero_leading = bar(3).layout_with_leading(bounds, 0.0, measure);

        assert_eq!(plain, zero_leading);
    }

    #[test]
    fn leading_width_is_clamped_to_bounds_width() {
        let bounds = Rect::new(0.0, 0.0, 50.0, 1.0);
        let measure = |_: usize| MenuBarItemMeasure::new(20.0);

        // Absurdly large leading_width must not push items_bounds.width
        // negative (which would panic or wrap in a naive `width -
        // leading_width` subtraction).
        let layout = bar(2).layout_with_leading(bounds, 10_000.0, measure);
        assert!(layout.visible_items.is_empty());
    }

    // ── MenuBar primitive tests ───────────────────────────────────────

    fn mk_menu_item(id: &str, label: &str) -> MenuBarItem {
        MenuBarItem {
            id: WidgetId::new(id),
            label: label.to_string(),
            disabled: false,
            submenu: None,
        }
    }

    #[test]
    fn menu_bar_layout_flat_items() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                mk_menu_item("edit", "&Edit"),
                mk_menu_item("view", "&View"),
            ],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 800.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(60.0));
        assert_eq!(layout.visible_items.len(), 3);
        assert_eq!(layout.visible_items[0].bounds.x, 0.0);
        assert_eq!(layout.visible_items[1].bounds.x, 60.0);
        assert_eq!(layout.visible_items[2].bounds.x, 120.0);
        // Click on Edit.
        match layout.hit_test(70.0, 10.0) {
            MenuBarHit::Item(1) => {}
            other => panic!("expected Item(1), got {other:?}"),
        }
    }

    #[test]
    fn menu_bar_alt_target_resolution() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                mk_menu_item("edit", "&Edit"),
                mk_menu_item("view", "&View"),
            ],
            open_item: None,
            focused_item: None,
        };
        assert_eq!(bar.find_alt_target('f'), Some(0));
        assert_eq!(bar.find_alt_target('E'), Some(1));
        assert_eq!(bar.find_alt_target('v'), Some(2));
        assert_eq!(bar.find_alt_target('x'), None);
    }

    #[test]
    fn menu_bar_disabled_items_not_clickable() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                MenuBarItem {
                    id: WidgetId::new("tools"),
                    label: "&Tools".to_string(),
                    disabled: true,
                    submenu: None,
                },
            ],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 800.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(60.0));
        assert!(!layout.visible_items[1].clickable);
        // Click on the disabled Tools item → Bar (not Item).
        assert_eq!(layout.hit_test(70.0, 10.0), MenuBarHit::Bar);
        // Alt+t skips disabled.
        assert_eq!(bar.find_alt_target('t'), None);
    }

    #[test]
    fn menu_bar_click_outside() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![mk_menu_item("file", "File")],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(50.0));
        assert_eq!(layout.hit_test(100.0, 50.0), MenuBarHit::Outside);
    }

    // ── resolve_bar_height ────────────────────────────────────────────

    #[test]
    fn resolve_bar_height_floors_pixel_backends_at_30px() {
        // GTK/macOS/Win chrome line heights vary (16-22px typical) and all
        // sit below the 30px token, so every one of them is floored up to
        // the same fixed row.
        assert_eq!(resolve_bar_height(16.0), BAR_HEIGHT);
        assert_eq!(resolve_bar_height(18.0), BAR_HEIGHT);
        assert_eq!(resolve_bar_height(22.0), BAR_HEIGHT);
        assert_eq!(BAR_HEIGHT, 30.0);
    }

    #[test]
    fn resolve_bar_height_keeps_a_taller_chrome_font_uncapped() {
        // A chrome font/high-DPI setup taller than the 30px token must not
        // get clipped: the floor only raises a short candidate, it never
        // lowers a tall one.
        assert_eq!(resolve_bar_height(34.0), 34.0);
    }

    #[test]
    fn resolve_bar_height_leaves_tui_cell_grid_unchanged() {
        // TUI's cell-grid convention: one row is `1.0` "unit" tall, with
        // no sub-cell height to refine.
        assert_eq!(resolve_bar_height(1.0), 1.0);
        assert_eq!(resolve_bar_height(0.0), 0.0);
    }

    #[test]
    fn resolved_bar_height_feeds_layout_straight_through() {
        let bounds = Rect::new(0.0, 0.0, 200.0, resolve_bar_height(18.0));
        let layout = bar(2).layout(bounds, |_| MenuBarItemMeasure::new(50.0));
        assert_eq!(layout.bounds.height, BAR_HEIGHT);
        assert_eq!(layout.visible_items[0].bounds.height, BAR_HEIGHT);
    }
}

/// The cell painter's own paint↔click round trips, run against a real
/// ratatui buffer through the test-local [`draw_menu_bar`], which adapts
/// a `Buffer` into the [`crate::CellSurface`] [`cell_paint::paint`]
/// takes.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use super::*;
    use crate::primitives::context_menu::PointerTint;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as TuiRect;
    use ratatui::style::Modifier;

    use crate::theme::Theme;
    use crate::tui::ratatui_color;

    /// Lays `bar` out on the cell grid at `area` and paints it, returning
    /// the layout a click is hit-tested against.
    fn draw_menu_bar(
        buf: &mut Buffer,
        area: TuiRect,
        bar: &MenuBar,
        theme: &Theme,
    ) -> MenuBarLayout {
        let layout = bar.cell_layout(Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ));
        let mut cursor_sink = None;
        let mut surface = crate::tui::cell_surface::TuiCellSurface::over(
            buf,
            crate::backend::ColorDepth::TrueColor,
            &mut cursor_sink,
        );
        cell_paint::paint(bar, &layout, &mut surface, theme, PointerTint::default());
        layout
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn make_bar() -> MenuBar {
        MenuBar {
            id: WidgetId::new("menu"),
            items: ["&File", "&Edit", "&View"]
                .iter()
                .map(|label| MenuBarItem {
                    id: WidgetId::new(label.trim_start_matches('&').to_lowercase()),
                    label: (*label).into(),
                    disabled: false,
                    submenu: None,
                })
                .collect(),
            open_item: None,
            focused_item: None,
        }
    }

    /// Shared body for the paint↔click round trip, run at both the origin
    /// and a non-zero `area` origin: the layout bakes `area.x`/`area.y`
    /// straight into the returned bounds (absolute frame), so both the
    /// painted glyph position and the `hit_test` coordinates shift
    /// together with the origin.
    fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let area = TuiRect::new(origin_x, origin_y, 40, 1);
        let mut buf = Buffer::empty(TuiRect::new(0, 0, origin_x + 40, origin_y + 1));
        let bar = make_bar();
        let layout = draw_menu_bar(&mut buf, area, &bar, &Theme::default());

        for vi in &layout.visible_items {
            let mid_x = vi.bounds.x + vi.bounds.width / 2.0;
            let mid_y = vi.bounds.y + 0.5;
            assert_eq!(
                layout.hit_test(mid_x, mid_y),
                MenuBarHit::Item(vi.item_idx),
                "hit_test at painted item {}'s centre should return Item({})",
                vi.item_idx,
                vi.item_idx,
            );

            let label = &bar.items[vi.item_idx].label;
            let first_display_char = label.chars().find(|&c| c != '&').unwrap();
            let start_col = vi.bounds.x.round() as u16 + 1; // after padding
            assert_eq!(
                cell_char(&buf, start_col, area.y),
                first_display_char,
                "first display char of item {} should be '{first_display_char}' at col {start_col}",
                vi.item_idx,
            );
        }
    }

    #[test]
    fn paint_and_click_round_trip() {
        paint_and_click_round_trip_at(0, 0);
    }

    /// Non-zero-origin guard: same round trip at a shifted `area`
    /// origin, proving `hit_test` resolves against the absolute (not
    /// local) bounds the painter wrote into.
    /// Paints `bar` through [`Primitive::paint_cells`] with the pointer
    /// on item `hovered` and pressing item `pressed`, and returns the
    /// background of each visible item's first cell, by item index.
    fn item_bgs_with_pointer(
        bar: &MenuBar,
        theme: &Theme,
        hovered: Option<usize>,
        pressed: Option<usize>,
    ) -> Vec<ratatui::style::Color> {
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 40, 1));
        let ctx = crate::primitive::LayoutCtx::new(1.0, 1.0, false);
        let layout = Primitive::layout(bar, &ctx, Rect::new(0.0, 0.0, 40.0, 1.0));
        let at = |idx: Option<usize>| idx.map(|i| (bar.id, Some(SubTarget(i as u64))));
        let interaction = crate::InteractionState::from_targets(at(hovered), at(pressed));
        let style = crate::Style::default();
        let env = crate::PaintEnv::new(theme, &style).with_interaction(&interaction);
        Primitive::paint_cells(
            bar,
            &layout,
            &mut crate::tui::BufferCellSurface::new(&mut buf),
            &env,
        );
        layout
            .layout
            .visible_items
            .iter()
            .map(|vi| buf[(vi.bounds.x.round() as u16, 0)].bg)
            .collect()
    }

    #[test]
    fn hovered_item_takes_the_menu_hover_tint_on_cells() {
        let theme = Theme::default();
        let bgs = item_bgs_with_pointer(&make_bar(), &theme, Some(1), None);
        let rest = ratatui_color(theme.tab_bar_bg);
        assert_eq!(
            bgs,
            vec![
                rest,
                ratatui_color(theme.menu_item_hover_bg(theme.tab_bar_bg)),
                rest
            ]
        );
    }

    #[test]
    fn pressed_item_takes_the_menu_pressed_tint_on_cells() {
        let theme = Theme::default();
        let bgs = item_bgs_with_pointer(&make_bar(), &theme, Some(2), Some(2));
        assert_eq!(
            bgs[2],
            ratatui_color(theme.menu_item_pressed_bg(theme.tab_bar_bg))
        );
    }

    #[test]
    fn hovering_the_open_item_keeps_the_active_fill_on_cells() {
        let theme = Theme::default();
        let mut bar = make_bar();
        bar.open_item = Some(0);
        let bgs = item_bgs_with_pointer(&bar, &theme, Some(0), None);
        assert_eq!(bgs[0], ratatui_color(theme.tab_active_bg));
    }

    #[test]
    fn a_disabled_item_never_tints_on_cells() {
        let theme = Theme::default();
        let mut bar = make_bar();
        bar.items[1].disabled = true;
        let bgs = item_bgs_with_pointer(&bar, &theme, Some(1), Some(1));
        assert_eq!(bgs[1], ratatui_color(theme.tab_bar_bg));
    }

    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7, 13);
    }

    #[test]
    fn disabled_item_not_clickable() {
        let area = TuiRect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let mut bar = make_bar();
        bar.items[1].disabled = true;
        let layout = draw_menu_bar(&mut buf, area, &bar, &Theme::default());

        let vi = &layout.visible_items[1];
        let mid_x = vi.bounds.x + vi.bounds.width / 2.0;
        assert_eq!(layout.hit_test(mid_x, 0.5), MenuBarHit::Bar);
    }

    #[test]
    fn open_item_has_active_bg() {
        let area = TuiRect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut bar = make_bar();
        bar.open_item = Some(1);
        let layout = draw_menu_bar(&mut buf, area, &bar, &theme);

        let vi = &layout.visible_items[1];
        let col = vi.bounds.x.round() as u16 + 1;
        assert_eq!(buf[(col, 0)].bg, ratatui_color(theme.tab_active_bg));

        let vi0 = &layout.visible_items[0];
        let col0 = vi0.bounds.x.round() as u16 + 1;
        assert_eq!(buf[(col0, 0)].bg, ratatui_color(theme.tab_bar_bg));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 10, 1));
        let area = TuiRect::new(0, 0, 0, 1);
        let _layout = draw_menu_bar(&mut buf, area, &make_bar(), &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ', "buffer should be untouched");
    }

    #[test]
    fn alt_char_underlined() {
        let area = TuiRect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        draw_menu_bar(&mut buf, area, &make_bar(), &Theme::default());

        // 'F' in "&File" is at col 1 (after one cell of left padding).
        assert!(
            buf[(1, 0)].modifier.contains(Modifier::UNDERLINED),
            "alt-key char 'F' should be underlined"
        );
        assert_eq!(cell_char(&buf, 1, 0), 'F');
        assert!(
            !buf[(2, 0)].modifier.contains(Modifier::UNDERLINED),
            "non-alt char 'i' should not be underlined"
        );
    }

    /// A label with no `&` is never underlined — no cell in the item's
    /// slot carries the modifier, and the label still paints.
    #[test]
    fn no_ampersand_label_is_never_underlined() {
        let area = TuiRect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let mut bar = make_bar();
        bar.items[0].label = "File".into();
        let layout = draw_menu_bar(&mut buf, area, &bar, &Theme::default());

        let vi = &layout.visible_items[0];
        let start_x = vi.bounds.x.round() as u16;
        let end_x = start_x + vi.bounds.width.round() as u16;
        for x in start_x..end_x {
            assert!(
                !buf[(x, area.y)].modifier.contains(Modifier::UNDERLINED),
                "label with no '&' must not underline any cell (col {x})"
            );
        }
        assert_eq!(cell_char(&buf, start_x + 1, area.y), 'F');
    }

    /// A wide (CJK) label glyph leaves its continuation cell as the
    /// wide-char marker (empty symbol), not a stale space.
    #[test]
    fn wide_glyph_label_leaves_continuation_cell_empty() {
        let area = TuiRect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let mut bar = make_bar();
        bar.items[0].label = "&文件".into();
        let layout = draw_menu_bar(&mut buf, area, &bar, &Theme::default());

        let vi = &layout.visible_items[0];
        let start_x = vi.bounds.x.round() as u16;
        assert_eq!(cell_char(&buf, start_x + 1, area.y), '文');
        assert_eq!(
            buf[(start_x + 2, area.y)].symbol(),
            "",
            "the cell after a wide glyph must be the empty continuation marker"
        );
        assert_eq!(cell_char(&buf, start_x + 3, area.y), '件');
    }

    #[test]
    fn cell_item_width_measures_cjk_as_double_width_not_char_count() {
        // "&文件" strips to "文件" — two CJK chars, 2 cells each, plus
        // the one-column padding either side.
        assert_eq!(cell_item_width("&文件"), 6.0);
    }
}

/// The widget contract's own unit coverage: [`Primitive::layout`]'s two
/// unit branches, [`Primitive::measure`], and every key/pointer gesture
/// [`Interactive::handle`] resolves.
#[cfg(test)]
mod widget_contract_tests {
    use super::*;
    use crate::interactive::{EventHost, SubTarget};
    use crate::types::Modifiers;
    use crate::{Point, PointerShape, UiEvent};

    /// Records what `handle` asked the host for, so a test can assert on
    /// the focus request a click makes.
    #[derive(Default)]
    struct RecordingHost {
        focused: Vec<Option<SubTarget>>,
        emitted: Vec<UiEvent>,
    }

    impl EventHost for RecordingHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focused.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, event: UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn bar() -> MenuBar {
        MenuBar {
            id: WidgetId::new("menubar:main"),
            items: vec![
                MenuBarItem {
                    id: WidgetId::new("menu:file"),
                    label: "&File".into(),
                    disabled: false,
                    submenu: None,
                },
                MenuBarItem {
                    id: WidgetId::new("menu:edit"),
                    label: "&Edit".into(),
                    disabled: false,
                    submenu: None,
                },
                MenuBarItem {
                    id: WidgetId::new("menu:view"),
                    label: "&View".into(),
                    disabled: false,
                    submenu: None,
                },
            ],
            open_item: None,
            focused_item: None,
        }
    }

    const AREA: Rect = Rect::new(0.0, 0.0, 40.0, 1.0);

    fn cell_ctx() -> LayoutCtx<'static> {
        LayoutCtx::new(1.0, 1.0, false)
    }

    fn resolved(bar: &MenuBar) -> MenuBarPrimitiveLayout {
        Primitive::layout(bar, &cell_ctx(), AREA)
    }

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    fn press(bar: &MenuBar, event: KeyEvent) -> Handled<MenuBarEvent> {
        let layout = resolved(bar);
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);
        Interactive::handle(bar, &mut (), &RoutedEvent::Key(event), &layout, &mut cx)
    }

    fn pointer(bar: &MenuBar, kind: PointerKind, x: f32, y: f32) -> Handled<MenuBarEvent> {
        let layout = resolved(bar);
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);
        Interactive::handle(
            bar,
            &mut (),
            &RoutedEvent::Pointer {
                kind,
                sub: None,
                local: Point::new(x, y),
                mods: Modifiers::default(),
            },
            &layout,
            &mut cx,
        )
    }

    #[test]
    fn cell_layout_gives_every_item_its_label_plus_one_column_a_side() {
        let layout = resolved(&bar());
        assert_eq!(layout.layout.visible_items.len(), 3);
        // "File" is four columns wide, plus one padding column a side.
        assert_eq!(layout.layout.visible_items[0].bounds.width, 6.0);
        assert_eq!(layout.layout.visible_items[1].bounds.x, 6.0);
    }

    #[test]
    fn pixel_layout_measures_through_the_ctx_and_adds_the_shared_padding() {
        // No measurer attached: `text_width` falls back to
        // `char_width * chars`, so "File" is 4 * 7.0 wide.
        let ctx = LayoutCtx::new(18.0, 7.0, false);
        let layout = Primitive::layout(&bar(), &ctx, Rect::new(0.0, 0.0, 400.0, 30.0));
        assert_eq!(
            layout.layout.visible_items[0].bounds.width,
            4.0 * 7.0 + ITEM_H_PADDING
        );
    }

    #[test]
    fn measure_reports_the_whole_strips_content_width_and_one_bar_row() {
        let natural = Primitive::measure(&bar(), &cell_ctx(), Size::new(80.0, 24.0));
        // "File" + "Edit" + "View", six columns each.
        assert_eq!(natural.width, Some(18.0));
        assert_eq!(natural.height, Some(1.0));
    }

    /// A ctx carrying no line height still reports a one-row strip, the
    /// same floor `ContextMenu::measure` applies to its own row pitch.
    #[test]
    fn measure_floors_a_missing_line_height_at_one_row() {
        let natural = Primitive::measure(
            &bar(),
            &LayoutCtx::new(0.0, 1.0, false),
            Size::new(80.0, 24.0),
        );
        assert_eq!(natural.height, Some(1.0));
    }

    #[test]
    fn a_pixel_measure_floors_the_bar_at_the_fixed_strip_height() {
        let natural = Primitive::measure(
            &bar(),
            &LayoutCtx::new(18.0, 7.0, false),
            Size::new(400.0, 40.0),
        );
        assert_eq!(natural.height, Some(BAR_HEIGHT));
    }

    #[test]
    fn hit_test_local_resolves_against_the_areas_own_origin() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), Rect::new(10.0, 4.0, 40.0, 1.0));
        assert_eq!(layout.hit_test_local(1.0, 0.0), MenuBarHit::Item(0));
        assert_eq!(layout.hit_test_local(7.0, 0.0), MenuBarHit::Item(1));
    }

    #[test]
    fn a_click_on_an_item_asks_the_host_to_open_that_menu() {
        let bar = bar();
        assert_eq!(
            pointer(&bar, PointerKind::Down(MouseButton::Left), 7.0, 0.0),
            Handled::Event(MenuBarEvent::MenuOpened {
                idx: 1,
                item: WidgetId::new("menu:edit"),
            })
        );
    }

    #[test]
    fn a_click_on_the_already_open_menu_closes_it() {
        let mut bar = bar();
        bar.open_item = Some(1);
        assert_eq!(
            pointer(&bar, PointerKind::Down(MouseButton::Left), 7.0, 0.0),
            Handled::Event(MenuBarEvent::Closed)
        );
    }

    #[test]
    fn a_click_on_the_strips_own_background_is_swallowed_not_ignored() {
        let bar = bar();
        assert_eq!(
            pointer(&bar, PointerKind::Down(MouseButton::Left), 30.0, 0.0),
            Handled::Consumed
        );
    }

    #[test]
    fn a_click_claims_focus_for_the_strip() {
        let bar = bar();
        let layout = resolved(&bar);
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);
        let _ = Interactive::handle(
            &bar,
            &mut (),
            &RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(1.0, 0.0),
                mods: Modifiers::default(),
            },
            &layout,
            &mut cx,
        );
        assert_eq!(host.focused, vec![None]);
    }

    #[test]
    fn hover_moves_the_cursor_and_drags_an_open_menu_with_it() {
        let bar = bar();
        assert_eq!(
            pointer(&bar, PointerKind::Move, 7.0, 0.0),
            Handled::Event(MenuBarEvent::FocusMoved { idx: 1 })
        );

        let mut open = bar.clone();
        open.open_item = Some(0);
        open.focused_item = Some(0);
        assert_eq!(
            pointer(&open, PointerKind::Move, 7.0, 0.0),
            Handled::Event(MenuBarEvent::MenuOpened {
                idx: 1,
                item: WidgetId::new("menu:edit"),
            })
        );
    }

    #[test]
    fn right_and_left_walk_the_menus_and_wrap() {
        let mut bar = bar();
        assert_eq!(
            press(&bar, key(Key::Named(NamedKey::Right))),
            Handled::Event(MenuBarEvent::FocusMoved { idx: 0 })
        );
        bar.focused_item = Some(2);
        assert_eq!(
            press(&bar, key(Key::Named(NamedKey::Right))),
            Handled::Event(MenuBarEvent::FocusMoved { idx: 0 })
        );
        bar.focused_item = Some(0);
        assert_eq!(
            press(&bar, key(Key::Named(NamedKey::Left))),
            Handled::Event(MenuBarEvent::FocusMoved { idx: 2 })
        );
    }

    #[test]
    fn arrow_keys_skip_a_disabled_menu() {
        let mut bar = bar();
        bar.items[1].disabled = true;
        bar.focused_item = Some(0);
        assert_eq!(
            press(&bar, key(Key::Named(NamedKey::Right))),
            Handled::Event(MenuBarEvent::FocusMoved { idx: 2 })
        );
    }

    #[test]
    fn walking_with_a_menu_open_opens_the_one_moved_onto() {
        let mut bar = bar();
        bar.open_item = Some(0);
        bar.focused_item = Some(0);
        assert_eq!(
            press(&bar, key(Key::Named(NamedKey::Right))),
            Handled::Event(MenuBarEvent::MenuOpened {
                idx: 1,
                item: WidgetId::new("menu:edit"),
            })
        );
    }

    #[test]
    fn down_and_enter_open_the_focused_menu() {
        let mut bar = bar();
        bar.focused_item = Some(2);
        let expected = Handled::Event(MenuBarEvent::MenuOpened {
            idx: 2,
            item: WidgetId::new("menu:view"),
        });
        assert_eq!(press(&bar, key(Key::Named(NamedKey::Down))), expected);
        assert_eq!(press(&bar, key(Key::Named(NamedKey::Enter))), expected);
    }

    #[test]
    fn down_with_nothing_focused_is_left_for_the_host() {
        assert_eq!(
            press(&bar(), key(Key::Named(NamedKey::Down))),
            Handled::Ignored
        );
    }

    #[test]
    fn f10_parks_the_cursor_on_the_leading_menu_without_opening_it() {
        assert_eq!(
            press(&bar(), key(Key::Named(NamedKey::F(10)))),
            Handled::Event(MenuBarEvent::FocusMoved { idx: 0 })
        );
    }

    #[test]
    fn alt_plus_a_mnemonic_opens_that_menu_from_anywhere() {
        let event = KeyEvent {
            key: Key::Char('v'),
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::default()
            },
            repeat: false,
        };
        assert_eq!(
            press(&bar(), event),
            Handled::Event(MenuBarEvent::MenuOpened {
                idx: 2,
                item: WidgetId::new("menu:view"),
            })
        );
    }

    #[test]
    fn a_plain_character_is_not_a_mnemonic() {
        assert_eq!(press(&bar(), key(Key::Char('v'))), Handled::Ignored);
    }

    #[test]
    fn escape_closes_an_open_or_focused_strip_and_is_otherwise_ignored() {
        let mut bar = bar();
        assert_eq!(
            press(&bar, key(Key::Named(NamedKey::Escape))),
            Handled::Ignored
        );
        bar.open_item = Some(1);
        assert_eq!(
            press(&bar, key(Key::Named(NamedKey::Escape))),
            Handled::Event(MenuBarEvent::Closed)
        );
    }

    #[test]
    fn an_a11y_click_opens_the_menu_the_cursor_sits_on() {
        let mut bar = bar();
        bar.focused_item = Some(1);
        let layout = resolved(&bar);
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &bar,
                &mut (),
                &RoutedEvent::Action(A11yAction::Click),
                &layout,
                &mut cx,
            ),
            Handled::Event(MenuBarEvent::MenuOpened {
                idx: 1,
                item: WidgetId::new("menu:edit"),
            })
        );
        assert_eq!(
            Interactive::a11y_actions(&bar, &()),
            vec![A11yAction::Click]
        );
    }

    #[test]
    fn an_empty_strip_is_no_focus_stop_and_offers_no_a11y_action() {
        let empty = MenuBar {
            id: WidgetId::new("menubar:empty"),
            items: Vec::new(),
            open_item: None,
            focused_item: None,
        };
        assert_eq!(Interactive::focus(&empty), FocusSpec::None);
        assert!(Interactive::a11y_actions(&empty, &()).is_empty());
        assert_eq!(Interactive::focus(&bar()), FocusSpec::Stop);
    }

    #[test]
    fn sub_regions_name_every_visible_item_in_local_space() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), Rect::new(10.0, 4.0, 40.0, 1.0));
        let regions = Interactive::sub_regions(&bar, &layout);
        assert_eq!(regions.len(), 3);
        assert_eq!(regions[0].0.x, 0.0);
        assert_eq!(regions[0].1, SubTarget(0));
        assert_eq!(regions[1].0.x, 6.0);
    }

    #[test]
    fn each_event_rides_a_ui_event_under_the_right_id() {
        let id = WidgetId::new("menubar:main");
        assert_eq!(
            MenuBarEvent::MenuOpened {
                idx: 1,
                item: WidgetId::new("menu:edit"),
            }
            .into_ui_event(id),
            UiEvent::MenuActivated(WidgetId::new("menu:edit"))
        );
        assert_eq!(
            MenuBarEvent::FocusMoved { idx: 2 }.into_ui_event(id),
            UiEvent::List(id, crate::ListViewEvent::SelectionChanged { idx: 2 })
        );
        assert_eq!(
            MenuBarEvent::Closed.into_ui_event(id),
            UiEvent::List(
                id,
                crate::ListViewEvent::KeyPressed {
                    key: "Escape".to_string(),
                    modifiers: Modifiers::default(),
                }
            )
        );
    }

    #[test]
    fn capture_round_trips_the_strip_as_its_own_owned_content() {
        let bar = bar();
        match Primitive::capture(&bar) {
            crate::frame::OwnedContent::MenuBar(captured) => assert_eq!(captured, bar),
            other => panic!("expected OwnedContent::MenuBar, got {other:?}"),
        }
    }

    /// Event parity's in-crate leg: an [`A11yAction::Click`] routed
    /// through [`crate::a11y::route_widget_action`] opens the menu the
    /// cursor sits on — the same `UiEvent::MenuActivated` a pointer click
    /// on that item produces. The pointer leg (TuiDriver) asserts the
    /// same event in `tests/widget_contract_menus_event_parity.rs`.
    #[test]
    fn route_widget_action_click_matches_the_parity_event() {
        let bar = MenuBar {
            focused_item: Some(0),
            ..bar()
        };
        let id = bar.id;
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, AREA, Box::new(bar));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let events = crate::a11y::route_widget_action(
            &tree,
            &mut states,
            &cell_ctx(),
            id,
            A11yAction::Click,
        );
        assert_eq!(
            events,
            vec![UiEvent::MenuActivated(WidgetId::new("menu:file"))]
        );
    }
}
