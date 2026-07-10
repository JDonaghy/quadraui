//! Frame-level rendering: declarative surface list + unified hit-test.
//!
//! Apps build a [`ScreenLayout`] by pushing [`Surface`] entries in
//! back-to-front z-order, then call [`ScreenLayout::draw`] to render
//! everything and get back a [`FrameHitMap`] for click dispatch.
//!
//! This moves draw orchestration out of per-app backend code and into
//! quadraui — apps describe the frame, quadraui executes it.
//!
//! Apps that already paint each surface at its natural call site
//! (rather than in one batch immediately followed by `.draw()`) can
//! instead push the same [`Surface`] entries into a `ScreenLayout`
//! purely for hit-testing and call [`ScreenLayout::hit_map`], which
//! registers every zone without invoking any `backend.draw_*()` call.

use crate::event::Rect;
use crate::primitives::activity_bar::ActivityBar;
use crate::primitives::chart::Chart;
use crate::primitives::command_line::CommandLine;
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::primitives::data_table::DataTable;
use crate::primitives::dialog::{Dialog, DialogLayout};
use crate::primitives::editor::Editor;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::form::Form;
use crate::primitives::list::ListView;
use crate::primitives::menu_bar::MenuBar;
use crate::primitives::multi_section_view::MultiSectionView;
use crate::primitives::palette::Palette;
use crate::primitives::panel::Panel;
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::split::Split;
use crate::primitives::status_bar::StatusBar;
use crate::primitives::tab_bar::TabBar;
use crate::primitives::terminal::Terminal;
use crate::primitives::text_display::TextDisplay;
use crate::primitives::toast::ToastStack;
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::primitives::tree::TreeView;
use crate::types::WidgetId;
use crate::Backend;

/// A surface to render in a single frame. Entries are pushed in
/// back-to-front z-order; [`ScreenLayout::draw`] renders them
/// sequentially and the resulting [`FrameHitMap`] checks the
/// highest-z surface first.
#[allow(clippy::large_enum_variant)]
pub enum Surface<'a> {
    Editor {
        rect: Rect,
        editor: &'a Editor,
    },
    TabBar {
        rect: Rect,
        bar: &'a TabBar,
        hovered_close: Option<usize>,
    },
    StatusBar {
        rect: Rect,
        bar: &'a StatusBar,
        hovered: Option<&'a WidgetId>,
        pressed: Option<&'a WidgetId>,
    },
    ActivityBar {
        rect: Rect,
        bar: &'a ActivityBar,
        hovered: Option<usize>,
    },
    CommandLine {
        rect: Rect,
        cmd: &'a CommandLine,
    },
    Terminal {
        rect: Rect,
        term: &'a Terminal,
    },
    TextDisplay {
        rect: Rect,
        td: &'a TextDisplay,
    },
    MultiSectionView {
        rect: Rect,
        view: &'a MultiSectionView,
    },
    Tree {
        rect: Rect,
        tree: &'a TreeView,
    },
    List {
        rect: Rect,
        list: &'a ListView,
    },
    Form {
        rect: Rect,
        form: &'a Form,
    },
    MenuBar {
        rect: Rect,
        bar: &'a MenuBar,
    },
    Split {
        rect: Rect,
        split: &'a Split,
    },
    Panel {
        rect: Rect,
        panel: &'a Panel,
    },
    Scrollbar {
        rect: Rect,
        sb: &'a Scrollbar,
    },
    Palette {
        rect: Rect,
        palette: &'a Palette,
    },
    Tooltip {
        tooltip: &'a Tooltip,
        layout: &'a TooltipLayout,
    },
    ContextMenu {
        menu: &'a ContextMenu,
        layout: &'a ContextMenuLayout,
    },
    Dialog {
        dialog: &'a Dialog,
        layout: &'a DialogLayout,
    },
    Completions {
        completions: &'a Completions,
        layout: &'a CompletionsLayout,
    },
    FindReplace {
        rect: Rect,
        panel: &'a FindReplacePanel,
    },
    RichTextPopup {
        popup: &'a RichTextPopup,
        layout: &'a RichTextPopupLayout,
    },
    Toast {
        rect: Rect,
        stack: &'a ToastStack,
    },
    DataTable {
        rect: Rect,
        table: &'a DataTable,
        hovered: Option<usize>,
    },
    Chart {
        rect: Rect,
        chart: &'a Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    },
}

/// Identifies which surface zone a point landed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameZone {
    Editor { idx: usize },
    TabBar { idx: usize },
    StatusBar { idx: usize },
    ActivityBar { idx: usize },
    CommandLine { idx: usize },
    Terminal { idx: usize },
    TextDisplay { idx: usize },
    MultiSectionView { idx: usize },
    Tree { idx: usize },
    List { idx: usize },
    Form { idx: usize },
    MenuBar { idx: usize },
    Split { idx: usize },
    Panel { idx: usize },
    Scrollbar { idx: usize },
    Palette { idx: usize },
    Tooltip { idx: usize },
    ContextMenu { idx: usize },
    Dialog { idx: usize },
    Completions { idx: usize },
    FindReplace { idx: usize },
    RichTextPopup { idx: usize },
    Toast { idx: usize },
    DataTable { idx: usize },
    Chart { idx: usize },
    Empty,
}

/// Hit regions collected during [`ScreenLayout::draw`]. Resolves
/// absolute coordinates to the highest-z surface that contains them.
#[derive(Default)]
pub struct FrameHitMap {
    zones: Vec<(Rect, FrameZone)>,
}

impl FrameHitMap {
    fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, rect: Rect, zone: FrameZone) {
        self.zones.push((rect, zone));
    }

    /// Find which zone contains `(x, y)`. Returns the highest-z match
    /// (last-drawn surface wins). Returns `FrameZone::Empty` when no
    /// surface contains the point.
    pub fn hit_test(&self, x: f32, y: f32) -> FrameZone {
        for (rect, zone) in self.zones.iter().rev() {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return zone.clone();
            }
        }
        FrameZone::Empty
    }

    /// Number of registered zones.
    pub fn len(&self) -> usize {
        self.zones.len()
    }

    /// Whether the hit map is empty.
    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }
}

/// Declarative frame description. Push surfaces in back-to-front
/// z-order, then call [`Self::draw`] to render and get a hit map.
pub struct ScreenLayout<'a> {
    surfaces: Vec<Surface<'a>>,
}

impl<'a> ScreenLayout<'a> {
    pub fn new() -> Self {
        Self {
            surfaces: Vec::new(),
        }
    }

    pub fn push(&mut self, surface: Surface<'a>) {
        self.surfaces.push(surface);
    }

    /// Render all surfaces via `backend` in z-order and return a
    /// [`FrameHitMap`] for unified click dispatch. Each surface's
    /// bounding rect is registered as a hit zone; apps then use
    /// per-surface `hit_test()` methods for fine-grained resolution.
    ///
    /// If you're accumulating surfaces that are painted elsewhere
    /// (e.g. at their natural call sites across an existing paint
    /// pass) purely to recover a hit map, use [`Self::hit_map`]
    /// instead — it skips every `backend.draw_*()` call.
    pub fn draw(&self, backend: &mut dyn Backend) -> FrameHitMap {
        let mut hit_map = FrameHitMap::new();

        for (idx, surface) in self.surfaces.iter().enumerate() {
            match surface {
                Surface::Editor { rect, editor } => {
                    backend.draw_editor(*rect, editor);
                }
                Surface::TabBar {
                    rect,
                    bar,
                    hovered_close,
                } => {
                    backend.draw_tab_bar(*rect, bar, *hovered_close);
                }
                Surface::StatusBar {
                    rect,
                    bar,
                    hovered,
                    pressed,
                } => {
                    backend.draw_status_bar(*rect, bar, *hovered, *pressed);
                }
                Surface::ActivityBar { rect, bar, hovered } => {
                    backend.draw_activity_bar(*rect, bar, *hovered);
                }
                Surface::CommandLine { rect, cmd } => {
                    backend.draw_command_line(*rect, cmd);
                }
                Surface::Terminal { rect, term } => {
                    backend.draw_terminal(*rect, term);
                }
                Surface::TextDisplay { rect, td } => {
                    backend.draw_text_display(*rect, td);
                }
                Surface::MultiSectionView { rect, view } => {
                    backend.draw_multi_section_view(*rect, view);
                }
                Surface::Tree { rect, tree } => {
                    backend.draw_tree(*rect, tree);
                }
                Surface::List { rect, list } => {
                    backend.draw_list(*rect, list);
                }
                Surface::Form { rect, form } => {
                    backend.draw_form(*rect, form);
                }
                Surface::MenuBar { rect, bar } => {
                    backend.draw_menu_bar(*rect, bar);
                }
                Surface::Split { rect, split } => {
                    backend.draw_split(*rect, split);
                }
                Surface::Panel { rect, panel } => {
                    backend.draw_panel(*rect, panel);
                }
                Surface::Scrollbar { rect, sb } => {
                    backend.draw_scrollbar(*rect, sb);
                }
                Surface::Palette { rect, palette } => {
                    backend.draw_palette(*rect, palette);
                }
                Surface::Tooltip { tooltip, layout } => {
                    backend.draw_tooltip(tooltip, layout);
                }
                Surface::ContextMenu { menu, layout } => {
                    backend.draw_context_menu(menu, layout);
                }
                Surface::Dialog { dialog, layout } => {
                    backend.draw_dialog(dialog, layout);
                }
                Surface::Completions {
                    completions,
                    layout,
                } => {
                    backend.draw_completions(completions, layout);
                }
                Surface::FindReplace { rect, panel } => {
                    backend.draw_find_replace(*rect, panel);
                }
                Surface::RichTextPopup { popup, layout } => {
                    backend.draw_rich_text_popup(popup, layout);
                }
                Surface::Toast { rect, stack } => {
                    backend.draw_toast_stack(*rect, stack);
                }
                Surface::DataTable {
                    rect,
                    table,
                    hovered,
                } => {
                    backend.draw_data_table(*rect, table, *hovered);
                }
                Surface::Chart {
                    rect,
                    chart,
                    hovered_point,
                    crosshair_x,
                } => {
                    backend.draw_chart(*rect, chart, *hovered_point, *crosshair_x);
                }
            }

            let (rect, zone) = Self::zone_for(idx, surface);
            hit_map.push(rect, zone);
        }

        hit_map
    }

    /// Return the [`FrameHitMap`] that [`Self::draw`] would produce,
    /// without invoking any `backend.draw_*()` call. For apps that
    /// already painted these same surfaces elsewhere in the frame and
    /// only need the hit-test result — pushing surfaces here re-invokes
    /// no rendering, so it's safe to build this after (or interleaved
    /// with) the app's real paint sequencing.
    pub fn hit_map(&self) -> FrameHitMap {
        let mut hit_map = FrameHitMap::new();

        for (idx, surface) in self.surfaces.iter().enumerate() {
            let (rect, zone) = Self::zone_for(idx, surface);
            hit_map.push(rect, zone);
        }

        hit_map
    }

    /// Compute the hit-test rect/zone for a single surface at index `idx`.
    /// Shared by [`Self::draw`] and [`Self::hit_map`] so the two stay in
    /// lock-step by construction.
    fn zone_for(idx: usize, surface: &Surface<'a>) -> (Rect, FrameZone) {
        match surface {
            Surface::Editor { rect, .. } => (*rect, FrameZone::Editor { idx }),
            Surface::TabBar { rect, .. } => (*rect, FrameZone::TabBar { idx }),
            Surface::StatusBar { rect, .. } => (*rect, FrameZone::StatusBar { idx }),
            Surface::ActivityBar { rect, .. } => (*rect, FrameZone::ActivityBar { idx }),
            Surface::CommandLine { rect, .. } => (*rect, FrameZone::CommandLine { idx }),
            Surface::Terminal { rect, .. } => (*rect, FrameZone::Terminal { idx }),
            Surface::TextDisplay { rect, .. } => (*rect, FrameZone::TextDisplay { idx }),
            Surface::MultiSectionView { rect, .. } => (*rect, FrameZone::MultiSectionView { idx }),
            Surface::Tree { rect, .. } => (*rect, FrameZone::Tree { idx }),
            Surface::List { rect, .. } => (*rect, FrameZone::List { idx }),
            Surface::Form { rect, .. } => (*rect, FrameZone::Form { idx }),
            Surface::MenuBar { rect, .. } => (*rect, FrameZone::MenuBar { idx }),
            Surface::Split { rect, .. } => (*rect, FrameZone::Split { idx }),
            Surface::Panel { rect, .. } => (*rect, FrameZone::Panel { idx }),
            Surface::Scrollbar { rect, .. } => (*rect, FrameZone::Scrollbar { idx }),
            Surface::Palette { rect, .. } => (*rect, FrameZone::Palette { idx }),
            Surface::Tooltip { layout, .. } => (layout.bounds, FrameZone::Tooltip { idx }),
            Surface::ContextMenu { layout, .. } => (layout.bounds, FrameZone::ContextMenu { idx }),
            Surface::Dialog { layout, .. } => (layout.bounds, FrameZone::Dialog { idx }),
            Surface::Completions { layout, .. } => (layout.bounds, FrameZone::Completions { idx }),
            Surface::FindReplace { rect, .. } => (*rect, FrameZone::FindReplace { idx }),
            Surface::RichTextPopup { layout, .. } => {
                (layout.bounds, FrameZone::RichTextPopup { idx })
            }
            Surface::Toast { rect, .. } => (*rect, FrameZone::Toast { idx }),
            Surface::DataTable { rect, .. } => (*rect, FrameZone::DataTable { idx }),
            Surface::Chart { rect, .. } => (*rect, FrameZone::Chart { idx }),
        }
    }
}

impl<'a> Default for ScreenLayout<'a> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_returns_highest_z_surface() {
        let mut map = FrameHitMap::new();
        // Background surface covers entire area.
        map.push(
            Rect::new(0.0, 0.0, 100.0, 100.0),
            FrameZone::Editor { idx: 0 },
        );
        // Overlay covers top-left quadrant.
        map.push(
            Rect::new(0.0, 0.0, 50.0, 50.0),
            FrameZone::Palette { idx: 1 },
        );

        // Point in the overlay → highest-z wins.
        assert_eq!(map.hit_test(25.0, 25.0), FrameZone::Palette { idx: 1 });
        // Point outside the overlay but inside editor.
        assert_eq!(map.hit_test(75.0, 75.0), FrameZone::Editor { idx: 0 });
        // Point outside everything.
        assert_eq!(map.hit_test(150.0, 150.0), FrameZone::Empty);
    }

    #[test]
    fn hit_test_empty_map() {
        let map = FrameHitMap::new();
        assert_eq!(map.hit_test(10.0, 10.0), FrameZone::Empty);
        assert!(map.is_empty());
    }

    #[test]
    fn multiple_overlapping_zones_last_wins() {
        let mut map = FrameHitMap::new();
        map.push(
            Rect::new(0.0, 0.0, 80.0, 24.0),
            FrameZone::Editor { idx: 0 },
        );
        map.push(Rect::new(0.0, 0.0, 80.0, 1.0), FrameZone::TabBar { idx: 1 });
        map.push(
            Rect::new(0.0, 23.0, 80.0, 1.0),
            FrameZone::StatusBar { idx: 2 },
        );

        assert_eq!(map.hit_test(40.0, 0.5), FrameZone::TabBar { idx: 1 });
        assert_eq!(map.hit_test(40.0, 12.0), FrameZone::Editor { idx: 0 });
        assert_eq!(map.hit_test(40.0, 23.5), FrameZone::StatusBar { idx: 2 });
    }

    #[test]
    fn screen_layout_default_is_empty() {
        let layout: ScreenLayout<'_> = ScreenLayout::default();
        assert_eq!(layout.surfaces.len(), 0);
    }

    #[test]
    fn hit_map_registers_zones_without_a_backend() {
        let editor = Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            lines: Vec::new(),
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 0,
            max_col: 0,
            gutter_char_width: 0,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: std::collections::HashMap::new(),
            code_action_lines: std::collections::HashSet::new(),
            bracket_match_positions: Vec::new(),
            active_indent_col: None,
            tabstop: 4,
            cursorline: false,
            lightbulb_glyph: '\0',
        };
        let palette = Palette {
            id: "pal".into(),
            title: String::new(),
            query: String::new(),
            query_cursor: 0,
            items: Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: false,
            show_query: true,
            create_label: None,
            preview: None,
            mode: crate::primitives::palette::PaletteMode::List,
        };

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Editor {
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            editor: &editor,
        });
        layout.push(Surface::Palette {
            rect: Rect::new(0.0, 0.0, 50.0, 50.0),
            palette: &palette,
        });

        // No `&mut dyn Backend` in scope anywhere in this test — proves
        // `hit_map()` needs no backend to recover the same zones `draw()`
        // would have registered.
        let hit_map = layout.hit_map();

        assert_eq!(hit_map.len(), 2);
        // Point in the overlay → highest-z (last-pushed) wins.
        assert_eq!(hit_map.hit_test(25.0, 25.0), FrameZone::Palette { idx: 1 });
        // Point outside the overlay but inside the editor.
        assert_eq!(hit_map.hit_test(75.0, 75.0), FrameZone::Editor { idx: 0 });
        // Point outside everything.
        assert_eq!(hit_map.hit_test(150.0, 150.0), FrameZone::Empty);
    }
}
