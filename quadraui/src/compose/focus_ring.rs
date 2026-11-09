//! `FocusRing` — a compose helper for Tab/Shift+Tab focus cycling
//! through a list of [`WidgetId`]s.
//!
//! Eliminates repeated modulo arithmetic and ID-array management.
//! Apps register focusable widget IDs once, then call [`advance`],
//! [`retreat`], or [`set`] from their event handler.
//!
//! ```ignore
//! let mut ring = FocusRing::new(vec!["search", "toggles", "replace", "buttons"]);
//! // Tab:
//! ring.advance();
//! // Shift+Tab:
//! ring.retreat();
//! // Click-to-focus:
//! ring.set(&WidgetId::new("replace"));
//! // Query:
//! assert_eq!(ring.current(), Some(&WidgetId::new("replace")));
//! ```
//!
//! # Relationship to [`FocusManager`] and [`FocusGroup`](super::focus_group::FocusGroup)
//!
//! `FocusRing` is a thin wrapper over [`FocusManager`] — the same
//! `WidgetId`-keyed focus owner every backend's Tab/Shift+Tab intercept
//! drives — adding only [`Self::is_focus_visible`]'s `:focus-visible`
//! bookkeeping on top. The index-wrapping arithmetic itself lives in one
//! place, [`crate::focus::cycle_index`], which `FocusManager::step` calls
//! too, so there is exactly one cycling implementation under both.
//!
//! [`FocusGroup`](super::focus_group::FocusGroup) exists separately for call sites with no natural
//! `WidgetId` to key by: a dynamically-resized run of anonymous regions
//! indexed `0..N` (sidebar sections, tab-group panes — see
//! [`crate::compose::sidebar_system`] / [`crate::compose::tab_group`]),
//! where the count changes at runtime as sections/panes are added or
//! removed. It is also a thin wrapper over [`crate::focus::cycle_index`].

use crate::event::Rect;
use crate::focus::FocusManager;
use crate::types::WidgetId;

/// Manages Tab/Shift+Tab cycling through a fixed list of focusable widgets.
///
/// # Focus-visible
///
/// Alongside "what is focused" ([`Self::current`]), `FocusRing` tracks
/// *how* it got there, in [`Self::is_focus_visible`] — VS Code's
/// `:focus-visible` convention: a keyboard move ([`Self::advance`],
/// [`Self::retreat`], [`Self::focus_first`]) shows the ring; a pointer
/// move ([`Self::set`], the click-to-focus path) focuses the widget but
/// suppresses it. A shared painter never sees this distinction — it only
/// receives whatever `bool` the host passes as `has_focus`/`focused` on
/// the primitive's own descriptor — so a host that wants the suppression
/// computes that bool as `ring.is_focused(id) && ring.is_focus_visible()`
/// rather than `ring.is_focused(id)` alone. [`crate::Button::focused`]
/// and [`crate::TextInput::has_focus`] both follow this convention.
/// [`crate::ListView`]/[`crate::TreeView`] don't: their `has_focus`
/// field is also used by some hosts as a persistent "currently
/// showing" indicator unrelated to real keyboard focus (see
/// [`crate::ListView::has_focus`]'s doc), so their GUI focus-visible
/// ring reads a separate `focus_ring_visible` parameter on
/// [`crate::Backend::draw_list_with_state`]/
/// [`crate::Backend::draw_tree_with_state`] instead of this field.
#[derive(Debug, Clone)]
pub struct FocusRing {
    items: Vec<WidgetId>,
    manager: FocusManager,
    focus_visible: bool,
}

impl FocusRing {
    /// Create a new ring from anything convertible to `WidgetId`.
    ///
    /// The first item is focused initially, with the ring visible (as if
    /// reached by keyboard). Pass an empty vec for no focus.
    pub fn new(ids: Vec<impl Into<WidgetId>>) -> Self {
        let items: Vec<WidgetId> = ids.into_iter().map(Into::into).collect();
        let mut manager = FocusManager::new();
        // `FocusManager::sync_tab_order` only reads the id half of each
        // pair (see its doc) — the rects exist purely to satisfy its
        // signature, which is built for real on-screen tab stops; a ring
        // has no geometry of its own.
        let stops: Vec<(WidgetId, Rect)> = items
            .iter()
            .map(|id| (*id, Rect::new(0.0, 0.0, 0.0, 0.0)))
            .collect();
        manager.sync_tab_order(&stops);
        if let Some(first) = items.first() {
            manager.set_focus(Some(*first));
        }
        Self {
            items,
            manager,
            focus_visible: true,
        }
    }

    /// Move focus to the next item (wraps around), and mark the ring
    /// focus-visible. No-op if nothing is currently focused (empty ring,
    /// or focus was [`Self::clear`]ed).
    pub fn advance(&mut self) {
        if self.manager.focused().is_some() {
            self.manager.focus_next();
            self.focus_visible = true;
        }
    }

    /// Move focus to the previous item (wraps around), and mark the ring
    /// focus-visible. No-op if nothing is currently focused (empty ring,
    /// or focus was [`Self::clear`]ed).
    pub fn retreat(&mut self) {
        if self.manager.focused().is_some() {
            self.manager.focus_prev();
            self.focus_visible = true;
        }
    }

    /// Set focus to a specific widget by ID — the click-to-focus path.
    /// No-op if the ID isn't in the ring. Marks the ring **not**
    /// focus-visible: a mouse click focuses the widget without showing
    /// the ring, matching VS Code. A host that wants the ring shown
    /// after a programmatic `set` anyway (keyboard-equivalent focus,
    /// e.g. restoring focus after a dialog closes) should call
    /// [`Self::set_focus_visible`] immediately after.
    pub fn set(&mut self, id: &WidgetId) {
        if self.items.iter().any(|item| item == id) {
            self.manager.set_focus(Some(*id));
            self.focus_visible = false;
        }
    }

    /// Clear focus (nothing focused).
    pub fn clear(&mut self) {
        self.manager.clear();
    }

    /// Focus the first item, and mark the ring focus-visible. No-op if
    /// empty.
    pub fn focus_first(&mut self) {
        if let Some(first) = self.items.first() {
            self.manager.set_focus(Some(*first));
            self.focus_visible = true;
        }
    }

    /// Whether the focus ring should currently be painted — `true` after
    /// a keyboard move, `false` after [`Self::set`] (a mouse click). See
    /// the type doc's "Focus-visible" section for how a host applies
    /// this to a primitive's `has_focus`/`focused` descriptor field.
    pub fn is_focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// Force the focus-visible flag directly, independent of how focus
    /// last moved — e.g. a host that wants the ring shown immediately
    /// after a programmatic [`Self::set`] (focus restored by code, not a
    /// click).
    pub fn set_focus_visible(&mut self, visible: bool) {
        self.focus_visible = visible;
    }

    /// The currently focused widget ID, or `None` if nothing is focused.
    pub fn current(&self) -> Option<&WidgetId> {
        self.manager.focused()
    }

    /// The index of the currently focused item.
    pub fn current_index(&self) -> Option<usize> {
        self.current()
            .and_then(|id| self.items.iter().position(|item| item == id))
    }

    /// Whether the given ID is currently focused.
    pub fn is_focused(&self, id: &WidgetId) -> bool {
        self.manager.is_focused(id)
    }

    /// The number of items in the ring.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the ring is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The list of widget IDs in order.
    pub fn items(&self) -> &[WidgetId] {
        &self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_focuses_first() {
        let ring = FocusRing::new(vec!["a", "b", "c"]);
        assert_eq!(ring.current(), Some(&WidgetId::new("a")));
        assert_eq!(ring.current_index(), Some(0));
    }

    #[test]
    fn empty_ring() {
        let ring = FocusRing::new(Vec::<&str>::new());
        assert_eq!(ring.current(), None);
        assert!(ring.is_empty());
    }

    #[test]
    fn advance_wraps() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("b")));
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("c")));
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn retreat_wraps() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        ring.retreat();
        assert_eq!(ring.current(), Some(&WidgetId::new("c")));
        ring.retreat();
        assert_eq!(ring.current(), Some(&WidgetId::new("b")));
    }

    #[test]
    fn set_by_id() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        ring.set(&WidgetId::new("c"));
        assert_eq!(ring.current_index(), Some(2));
        assert!(ring.is_focused(&WidgetId::new("c")));
        assert!(!ring.is_focused(&WidgetId::new("a")));
    }

    #[test]
    fn set_unknown_id_is_noop() {
        let mut ring = FocusRing::new(vec!["a", "b"]);
        ring.set(&WidgetId::new("z"));
        assert_eq!(ring.current(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn clear_and_refocus() {
        let mut ring = FocusRing::new(vec!["a", "b"]);
        ring.clear();
        assert_eq!(ring.current(), None);
        ring.advance(); // no-op when cleared
        assert_eq!(ring.current(), None);
        ring.focus_first();
        assert_eq!(ring.current(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn advance_after_set() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        ring.set(&WidgetId::new("b"));
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("c")));
    }

    #[test]
    fn new_ring_starts_focus_visible() {
        let ring = FocusRing::new(vec!["a", "b"]);
        assert!(ring.is_focus_visible());
    }

    /// The acceptance scenario for VS Code's `:focus-visible` convention:
    /// a Tab-reached widget shows the ring, and clicking a different one
    /// suppresses it even though focus did move.
    #[test]
    fn click_to_focus_suppresses_the_ring_but_keyboard_move_restores_it() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        assert!(ring.is_focus_visible(), "Tab landed here, so it's visible");

        ring.set(&WidgetId::new("b"));
        assert!(ring.is_focused(&WidgetId::new("b")), "click did move focus");
        assert!(
            !ring.is_focus_visible(),
            "a mouse click must suppress the ring"
        );

        ring.advance();
        assert!(
            ring.is_focus_visible(),
            "a subsequent keyboard move restores the ring"
        );
    }

    #[test]
    fn retreat_and_focus_first_also_mark_focus_visible() {
        let mut ring = FocusRing::new(vec!["a", "b"]);
        ring.set(&WidgetId::new("b"));
        assert!(!ring.is_focus_visible());

        ring.retreat();
        assert!(ring.is_focus_visible(), "Shift+Tab restores visibility");

        ring.set(&WidgetId::new("a"));
        assert!(!ring.is_focus_visible());
        ring.focus_first();
        assert!(ring.is_focus_visible());
    }

    #[test]
    fn set_focus_visible_overrides_directly() {
        let mut ring = FocusRing::new(vec!["a"]);
        ring.set(&WidgetId::new("a"));
        assert!(!ring.is_focus_visible());

        ring.set_focus_visible(true);
        assert!(ring.is_focus_visible());
    }

    /// Clicking an id that isn't in the ring is already a no-op for
    /// focus; it must be a no-op for visibility too.
    #[test]
    fn set_unknown_id_does_not_change_focus_visible() {
        let mut ring = FocusRing::new(vec!["a", "b"]);
        assert!(ring.is_focus_visible());
        ring.set(&WidgetId::new("z"));
        assert!(ring.is_focus_visible());
    }

    #[test]
    fn single_item_ring() {
        let mut ring = FocusRing::new(vec!["only"]);
        assert_eq!(ring.current(), Some(&WidgetId::new("only")));
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("only")));
        ring.retreat();
        assert_eq!(ring.current(), Some(&WidgetId::new("only")));
    }

    /// A headless, end-to-end proof that a real rasteriser — TUI's
    /// `draw_text_input`, whose border-tint-on-`has_focus` treatment is
    /// [`crate::tui::text_input`]'s existing focus story — paints a
    /// visibly different border depending on how focus last moved
    /// through [`FocusRing`], via the composed
    /// `ring.is_focused(id) && ring.is_focus_visible()` bool
    /// [`FocusRing::is_focus_visible`]'s type doc describes. Unlike a
    /// test that only re-reads the expression it just assigned, this
    /// one paints both states into a `Buffer` and asserts the actual
    /// cell colour, so it fails if either `FocusRing`'s composition or
    /// `draw_text_input`'s border-tint logic regresses.
    #[cfg(feature = "tui")]
    #[test]
    fn focused_text_input_paints_a_different_border_after_tab_than_after_click() {
        use crate::tui::draw_text_input;
        use crate::{TextInput, Theme};
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let id = WidgetId::new("search:input");
        let mut ring = FocusRing::new(vec![id.clone()]);
        let mut input = TextInput::new(id.clone()).with_lines(vec!["hi".to_string()]);
        let theme = Theme::default();
        let rect = Rect::new(0, 0, 10, 3);
        // Sample the top border edge, not a corner (paints the same
        // colour) and not the content row.
        let border_fg = |buf: &Buffer| buf[(1, 0)].fg;

        // Tab: `advance()` is the real keyboard-move path, not just
        // `FocusRing::new`'s initial state.
        ring.advance();
        input.has_focus = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            input.has_focus,
            "a Tab-reached text input must report focus-visible"
        );
        let mut buf_tab = Buffer::empty(rect);
        draw_text_input(&mut buf_tab, rect, &input, &theme);

        // Click: `set` re-focuses the same widget via the pointer path.
        ring.set(&id);
        input.has_focus = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            ring.is_focused(&id),
            "the click did move focus to this input"
        );
        assert!(
            !input.has_focus,
            "a mouse click must suppress the text input's focus-visible ring"
        );
        let mut buf_click = Buffer::empty(rect);
        draw_text_input(&mut buf_click, rect, &input, &theme);

        assert_ne!(
            border_fg(&buf_tab),
            border_fg(&buf_click),
            "the painted border colour must differ between the Tab-reached \
             and clicked states"
        );

        // Shift+Tab afterwards restores both the composed bool and the
        // painted border.
        ring.retreat();
        input.has_focus = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            input.has_focus,
            "a subsequent keyboard move restores focus-visible"
        );
        let mut buf_retreat = Buffer::empty(rect);
        draw_text_input(&mut buf_retreat, rect, &input, &theme);
        assert_eq!(
            border_fg(&buf_tab),
            border_fg(&buf_retreat),
            "Shift+Tab repaints the same focus-visible border Tab did"
        );
    }

    /// The same end-to-end proof as
    /// `focused_text_input_paints_a_different_border_after_tab_than_after_click`,
    /// for [`crate::ListView`]'s shared GUI painter
    /// ([`crate::primitives::list::native_surface_paint::paint`]):
    /// `ring.advance()` (the real Tab path) composes a visible ring,
    /// `ring.set()` (the real click path) suppresses it, and this test
    /// asserts the actual recorded stroke rather than re-reading the
    /// composed bool it just assigned.
    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[test]
    fn focused_list_row_paints_a_ring_after_tab_than_after_click() {
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::paint_surface::PaintSurface;
        use crate::primitives::list::{ListItem, ListItemMeasure, ListView};
        use crate::style::Style;
        use crate::types::{Color, StyledText};
        use crate::{Rect, Theme};

        #[derive(Default)]
        struct RecordingSurface {
            strokes: Vec<(Rect, Color, f32)>,
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
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(
                &mut self,
                _from: crate::event::Point,
                _to: crate::event::Point,
                _color: Color,
                _sw: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(
                &mut self,
                _rect: Rect,
                _image: &crate::Image,
            ) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        let id = WidgetId::new("sidebar:list");
        let mut ring = FocusRing::new(vec![id.clone()]);
        let theme = Theme::default();
        let style = Style::default();
        let area = Rect::new(0.0, 0.0, 100.0, 80.0);
        const LINE_HEIGHT: f32 = 16.0;

        // `has_focus: true` throughout: it's the "is this the active
        // list" signal (and gates the pre-existing `selected_bg` fill,
        // unchanged by this issue), *not* the focus-visible signal the
        // ring reads — see `ListView::has_focus`'s own doc for why the
        // two are kept separate. The ring reads `focus_ring_visible`,
        // composed from `ring` below exactly like
        // `Backend::draw_list_with_state`'s own doc describes.
        let list = ListView {
            id: id.clone(),
            title: None,
            items: vec![ListItem {
                text: StyledText::plain("alpha"),
                icon: None,
                detail: None,
                decoration: Default::default(),
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };

        let paint_and_count_strokes = |focus_ring_visible: bool| -> usize {
            let layout = list.layout(area.width, area.height, 0.0, |_| {
                ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            crate::primitives::list::native_surface_paint::paint(
                &list,
                area,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &theme,
                &style,
                None,
                None,
                focus_ring_visible,
            );
            surface.strokes.len()
        };

        // Tab: `advance()` is the real keyboard-move path.
        ring.advance();
        let focus_ring_visible = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            focus_ring_visible,
            "a Tab-reached list must report focus-visible"
        );
        assert_eq!(
            paint_and_count_strokes(focus_ring_visible),
            1,
            "a Tab-reached list's selected row must paint a ring"
        );

        // Click: `set` re-focuses the same widget via the pointer path.
        ring.set(&id);
        let focus_ring_visible = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            ring.is_focused(&id),
            "the click did move focus to this list"
        );
        assert!(
            !focus_ring_visible,
            "a mouse click must suppress the list's focus-visible ring"
        );
        assert_eq!(
            paint_and_count_strokes(focus_ring_visible),
            0,
            "a clicked list's selected row must paint no ring"
        );

        // Shift+Tab afterwards restores the ring.
        ring.retreat();
        let focus_ring_visible = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            focus_ring_visible,
            "a subsequent keyboard move restores focus-visible"
        );
        assert_eq!(
            paint_and_count_strokes(focus_ring_visible),
            1,
            "Shift+Tab repaints the same ring Tab did"
        );
    }

    /// Same proof as `focused_list_row_paints_a_ring_after_tab_than_after_click`,
    /// for [`crate::TreeView`]'s shared GUI painter
    /// ([`crate::primitives::tree::native_surface_paint::paint`]).
    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[test]
    fn focused_tree_row_paints_a_ring_after_tab_than_after_click() {
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::paint_surface::PaintSurface;
        use crate::primitives::tree::{TreeRow, TreeView};
        use crate::style::Style;
        use crate::types::{Color, Decoration, SelectionMode, StyledText, TreeStyle};
        use crate::{Rect, Theme};

        #[derive(Default)]
        struct RecordingSurface {
            strokes: Vec<(Rect, Color, f32)>,
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
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(
                &mut self,
                _from: crate::event::Point,
                _to: crate::event::Point,
                _color: Color,
                _sw: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(
                &mut self,
                _rect: Rect,
                _image: &crate::Image,
            ) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        let id = WidgetId::new("sidebar:tree");
        let mut ring = FocusRing::new(vec![id.clone()]);
        let theme = Theme::default();
        let style = Style::default();
        let area = Rect::new(0.0, 0.0, 100.0, 80.0);
        const LINE_HEIGHT: f32 = 16.0;

        // `has_focus: true` throughout — see
        // `focused_list_row_paints_a_ring_after_tab_than_after_click`'s
        // matching comment for why.
        let tree = TreeView {
            id: id.clone(),
            rows: vec![TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("alpha"),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            }],
            selection_mode: SelectionMode::Single,
            selected_path: Some(vec![0]),
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: true,
        };

        let paint_and_count_strokes = |focus_ring_visible: bool| -> usize {
            let layout = crate::primitives::layout_metrics::tree_layout(
                &tree,
                area,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            crate::primitives::tree::native_surface_paint::paint(
                &tree,
                area,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &style,
                None,
                None,
                focus_ring_visible,
            );
            surface.strokes.len()
        };

        // Tab: `advance()` is the real keyboard-move path.
        ring.advance();
        let focus_ring_visible = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            focus_ring_visible,
            "a Tab-reached tree must report focus-visible"
        );
        assert_eq!(
            paint_and_count_strokes(focus_ring_visible),
            1,
            "a Tab-reached tree's selected row must paint a ring"
        );

        // Click: `set` re-focuses the same widget via the pointer path.
        ring.set(&id);
        let focus_ring_visible = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            ring.is_focused(&id),
            "the click did move focus to this tree"
        );
        assert!(
            !focus_ring_visible,
            "a mouse click must suppress the tree's focus-visible ring"
        );
        assert_eq!(
            paint_and_count_strokes(focus_ring_visible),
            0,
            "a clicked tree's selected row must paint no ring"
        );

        // Shift+Tab afterwards restores the ring.
        ring.retreat();
        let focus_ring_visible = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            focus_ring_visible,
            "a subsequent keyboard move restores focus-visible"
        );
        assert_eq!(
            paint_and_count_strokes(focus_ring_visible),
            1,
            "Shift+Tab repaints the same ring Tab did"
        );
    }
}
