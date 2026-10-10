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

use crate::event::Rect;
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
    /// present, the label is never underlined (quadraui#625 — no
    /// implicit "underline the first char" fallback), though
    /// [`MenuBar::find_alt_target`]'s keyboard-activation lookup still
    /// falls back to the first character, unrelated to the visual
    /// underline.
    pub label: String,
    /// When true, the item is rendered dimmed and clicks are ignored.
    #[serde(default)]
    pub disabled: bool,
    /// Declarative dropdown items. When `Some`, native menu installers
    /// (macOS `NSMenu` via #184 PR 2; future Win32 / GTK installers)
    /// build the dropdown directly from this list. In-window
    /// rasterisers (TUI / GTK `draw_menu_bar`) ignore the field today;
    /// apps that draw their own dropdown via the `MenuSystem` compose
    /// helper continue to wire that path independently.
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
                id: item.id.clone(),
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
    /// style (#662) — before laying out items.
    ///
    /// This exists because the offset math is the actual regression
    /// risk of a leading slot, not the paint: a consumer that narrows
    /// the rect it hands to a paint call but not the rect it hands to a
    /// click-routing call (or vice versa) gets a menu bar whose visible
    /// items and clickable items silently disagree — the same bug class
    /// #552 found in `TabBar`'s hit-x-offset. Centralizing the shift
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

// ── PaintSurface Phase 4 slice 5/8 (#1081) ─────────────────────────────────
//
// `paint` below is the one shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API — see `crate::primitives::context_menu::native_surface_paint` for
// the same pattern applied three primitives earlier (#1077, slice 1/8 of
// that issue's own numbering; this repo's issue tracker also carries it
// forward as slice 5/8 of the overall `PaintSurface` Phase 4 run).
//
// Pre-migration, `gtk::menu_bar`, `macos::menu_bar` and `win::menu_bar`
// agreed on layout, background/active/disabled colouring, and centred
// label text — but diverged, sometimes substantially, on the Alt-key
// underline:
//
// - **GTK** underlined the activation character via a Pango
//   `AttrList`/`AttrInt::new_underline` range on just that character —
//   real per-glyph underline metrics, but a mechanism [`PaintSurface`]
//   has no verb for (there is no "underline this byte range of a text
//   run" primitive, only whole-run `surface_draw_text_run_styled`, whose
//   `underline` flag would underline the entire label).
// - **Windows** underlined by measuring the prefix before the
//   activation character and the character's own width, then filling a
//   manually-positioned `UNDERLINE_HEIGHT_DIP`-tall rectangle beneath
//   it — reusing the same measure/fill verbs every other rasteriser
//   already has, no font-shaping API involved.
// - **macOS had no underline at all** — its own module doc listed this
//   as a documented "follow-up" scope omission (Core Text's
//   `kCTUnderlineStyleAttributeName` needs attributed-string plumbing
//   `super::text::draw_text` didn't have), not a deliberate design
//   choice.
//
// `paint` adopts Windows' manual-rectangle approach for all three
// backends: it is the only one of the three that maps directly onto
// [`PaintSurface`]'s existing verbs
// ([`crate::paint_surface::PaintSurface::surface_measure_text`] +
// [`crate::paint_surface::PaintSurface::surface_fill_rect`]), and
// adopting it closes macOS's gap instead of leaving a third rendering
// path unported. Visually near-identical to GTK's Pango underline at
// the sizes this bar renders at (a 2px solid bar under one character).
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{MenuBar, MenuBarLayout};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Thickness (surface-native units) of the Alt-key underline
    /// rectangle — mirrors `win::menu_bar`'s pre-migration
    /// `UNDERLINE_HEIGHT_DIP`.
    const UNDERLINE_HEIGHT: f32 = 2.0;

    /// Strip `&` markers from a label for display — mirrors each
    /// pre-migration backend's own `display_text`.
    pub(super) fn display_text(label: &str) -> String {
        label.chars().filter(|&c| c != '&').collect()
    }

    /// The **char index** (not byte index — [`PaintSurface::surface_measure_text`]
    /// works over substrings, not byte ranges) into the display string
    /// of the Alt-activation character (the character immediately after
    /// `&`), or `None` when `label` carries no `&` at all — mirrors
    /// `win::menu_bar`'s pre-migration `alt_char_index`'s "no implicit
    /// fallback" contract (quadraui#625).
    pub(super) fn alt_char_index(label: &str) -> Option<usize> {
        let marker_byte = label.find('&')?;
        Some(label[..marker_byte].chars().count())
    }

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
    /// GTK and Windows both have an Alt-mnemonic convention, so this is
    /// the one every call site but macOS's own wants — see
    /// [`paint_without_mnemonics`] for the macOS twin, which paints
    /// everything here except the underline.
    pub(crate) fn paint(
        bar: &MenuBar,
        layout: &MenuBarLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        paint_with_mnemonics(bar, layout, surface, theme, true);
    }

    /// [`paint`]'s macOS twin: identical in every respect — background,
    /// active/disabled colouring, centred label text — except it never
    /// paints the Alt-underline. macOS has no Alt-mnemonic keyboard
    /// convention (⌘-based shortcuts are the platform idiom instead,
    /// wired through [`crate::accelerator::Accelerator`] /
    /// `NSMenuItem.keyEquivalent`, not a label prefix), so underlining a
    /// character nothing responds to would be a visual lie.
    pub(crate) fn paint_without_mnemonics(
        bar: &MenuBar,
        layout: &MenuBarLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) {
        paint_with_mnemonics(bar, layout, surface, theme, false);
    }

    fn paint_with_mnemonics(
        bar: &MenuBar,
        layout: &MenuBarLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        show_mnemonics: bool,
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

            if is_active {
                surface.surface_fill_rect(vi.bounds, bg);
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

            if !show_mnemonics {
                continue;
            }

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
            // A trailing `&` marks a char index past the display
            // string's end — `paint`'s `text.chars().nth(idx)` guard
            // turns that into "no underline" rather than panicking.
            assert_eq!(alt_char_index("File&"), Some(4));
            assert_eq!(display_text("File&").chars().nth(4), None);
        }

        #[test]
        fn display_text_strips_ampersand() {
            assert_eq!(display_text("&File"), "File");
            assert_eq!(display_text("Sa&ve"), "Save");
            assert_eq!(display_text("File"), "File");
        }

        // ── paint()'s Alt-underline drift fix (#1081) ───────────────────
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
            paint(&bar, &layout, &mut surface, &theme);

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

        /// `paint_without_mnemonics` is `paint`'s macOS twin — same bar,
        /// same layout, but never paints the underline fill the test
        /// above pins for `paint`. Asserted against the exact rect
        /// `paint` fills, so a future change that makes the two converge
        /// (defeating the whole point of the split) fails loudly here
        /// rather than only on a macOS-only pixel test.
        #[test]
        fn paint_without_mnemonics_never_fills_the_underline() {
            let bar = drift_bar();
            let layout = drift_layout(&bar);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint_without_mnemonics(&bar, &layout, &mut surface, &theme);

            let (label_rect, _, _) = surface
                .texts
                .iter()
                .find(|(_, t, _)| t == "File")
                .expect("File label painted");
            let underline_rect = Rect::new(
                label_rect.x,
                label_rect.y + label_rect.height - UNDERLINE_HEIGHT,
                8.0,
                UNDERLINE_HEIGHT,
            );
            assert!(
                !surface
                    .fills
                    .iter()
                    .any(|(r, c)| (r.x - underline_rect.x).abs() < 0.01
                        && (r.y - underline_rect.y).abs() < 0.01
                        && (r.width - underline_rect.width).abs() < 0.01
                        && (r.height - underline_rect.height).abs() < 0.01
                        && *c == theme.tab_active_fg),
                "macOS has no Alt-mnemonic convention — expected no underline fill at \
                 {underline_rect:?}, got fills: {:?}",
                surface.fills,
            );
        }

        /// The other half of `paint_without_mnemonics`'s contract: it is
        /// `paint` *minus the underline*, not a separate, thinner
        /// rasteriser. Dropping the mnemonic must not also drop the bar
        /// background, an item's active/disabled colouring or a label —
        /// so every text run and every non-underline fill has to be
        /// byte-identical between the two, and `fills` has to differ by
        /// exactly the one underline rect.
        #[test]
        fn paint_without_mnemonics_differs_from_paint_only_by_the_underline_fill() {
            let bar = drift_bar();
            let layout = drift_layout(&bar);
            let theme = Theme::default();

            let mut with = RecordingSurface::default();
            paint(&bar, &layout, &mut with, &theme);
            let mut without = RecordingSurface::default();
            paint_without_mnemonics(&bar, &layout, &mut without, &theme);

            assert_eq!(
                with.texts, without.texts,
                "label text runs must be identical — only the underline differs",
            );

            let (label_rect, _, _) = with
                .texts
                .iter()
                .find(|(_, t, _)| t == "File")
                .expect("File label painted");
            let underline = Rect::new(
                label_rect.x,
                label_rect.y + label_rect.height - UNDERLINE_HEIGHT,
                8.0,
                UNDERLINE_HEIGHT,
            );
            let non_underline: Vec<_> = with
                .fills
                .iter()
                .filter(|(r, _)| *r != underline)
                .copied()
                .collect();
            assert_eq!(
                with.fills.len() - 1,
                non_underline.len(),
                "`paint` must emit the underline fill exactly once",
            );
            assert_eq!(
                non_underline, without.fills,
                "every fill but the underline must be identical between the two",
            );
        }

        #[test]
        fn no_ampersand_means_no_underline_fill_beneath_the_label() {
            // quadraui#625's "no implicit fallback" contract, carried
            // forward into the shared paint: "Help" has no `&`, so it
            // must get no thin underline-shaped fill at all.
            let bar = drift_bar();
            let layout = drift_layout(&bar);
            let mut surface = RecordingSurface::default();
            paint(&bar, &layout, &mut surface, &Theme::default());

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(n: usize) -> MenuBar {
        MenuBar {
            id: WidgetId("bar".into()),
            items: (0..n)
                .map(|i| MenuBarItem {
                    id: WidgetId(format!("item{i}")),
                    label: format!("Item{i}"),
                    disabled: false,
                    submenu: None,
                })
                .collect(),
            open_item: None,
            focused_item: None,
        }
    }

    // #662: a leading icon slot (app logo left of the first menu item)
    // must shift every item's x-offset by exactly `leading_width`, and a
    // click that used to land on item 0 pre-shift must land on item 0
    // again post-shift, at its new (shifted) x. This is the regression
    // the module doc on `layout_with_leading` calls out — the paint
    // itself is trivial, keeping paint and hit-test in agreement is not.
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
}
