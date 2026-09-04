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
//!
//! ## `Surface` vs. calling `Backend::draw_*` directly (issue #456)
//!
//! `ScreenLayout` + [`Surface`] is the **canonical path for a consumer
//! assembling a top-level screen out of multiple primitives**: pushing
//! `Surface` entries and calling [`ScreenLayout::draw`] routes both
//! backends through the same call site, so they cannot each paint the
//! identical primitive a different way — the drift #456 found in
//! vimcode, where the TUI palette painted via `backend.draw_palette(..)`
//! and GTK's identical call site instead pushed a `Surface::Palette`,
//! for no reason but that both APIs existed. `Backend::draw_<name>`
//! remains public, low-level API — `ScreenLayout::draw` calls it
//! internally, some primitives have no `Surface` variant yet, and
//! rasteriser tests / compose helpers call it directly by design. See
//! `quadraui/docs/DECISIONS.md` D-006 for the full decision and
//! `quadraui/docs/PRIMITIVE_RULES.md` "One primitive, one canonical
//! paint path" for the authoring rule this implies for new primitives.

//! ## Presence gating + the paint/hit-test order invariant (issue #774)
//!
//! `Surface`/`ScreenLayout` above answer "what does a frame look
//! like" for the primitives quadraui already knows about. They don't
//! answer a question every non-trivial app also has: "which of *my*
//! app-specific rungs (an editor pane, a bottom panel, an overlay
//! stack) exist this frame, and in what back-to-front order do they
//! have to be visited — identically — whether you're painting them or
//! resolving a click?"
//!
//! Left unanswered, that question gets reinvented per app, per
//! backend, as two independently-maintained lists that are supposed
//! to agree: vimcode's `FRAME_Z_ORDER` (paint) and
//! `MOUSE_ARBITRATION_ORDER` (hit-test) drifted exactly once —
//! vimcode#587/#592 — and the fix was a `debug_assert!` comparing the
//! two lists after the fact, not a structural guarantee.
//!
//! [`FrameRung`] + [`FramePresence`] + [`check_frame_order`] make that
//! drift impossible instead of merely detectable:
//!
//! - An app defines one small `enum` for its rungs and implements
//!   [`FrameRung::z_order`] **once** — the single source of truth for
//!   "what's on top of what."
//! - [`FramePresence::from_fn`] evaluates a presence predicate against
//!   every rung **once per frame**, so "is the palette even open right
//!   now" is computed a single time and consulted by both painting and
//!   hit-testing, rather than re-derived (and potentially
//!   re-diverging) at each call site.
//! - [`compose_frame`] walks present rungs in canonical order with a
//!   single non-branching callback — no `match` over paint-operation
//!   variants, because the order comes from `z_order()`, not from the
//!   shape of the walk.
//! - [`check_frame_order`] asserts that some *other*, independently
//!   produced sequence (e.g. a hit-test walk, or a legacy per-backend
//!   paint routine mid-migration to [`compose_frame`]) is consistent
//!   with `z_order()`. This is the guard that would have caught
//!   vimcode#587/#592 as a compile-time-adjacent test failure instead
//!   of a months-later bug report.
//!
//! This machinery is deliberately independent of [`Surface`]/
//! [`ScreenLayout`]: the rungs it orders are app-defined (an app's own
//! enum), not quadraui's primitive set, so it composes with either the
//! `Surface` path or a bespoke per-backend paint routine equally well.
//!
//! ```
//! use quadraui::frame::{check_frame_order, compose_frame, FramePresence, FrameRung};
//!
//! #[derive(Debug, Clone, Copy, PartialEq, Eq)]
//! enum Rung {
//!     Editor,
//!     BottomPanel,
//!     Palette,
//! }
//!
//! impl FrameRung for Rung {
//!     fn z_order() -> &'static [Self] {
//!         // Back-to-front: Editor paints first, Palette paints (and
//!         // hit-tests) last, i.e. "on top".
//!         &[Rung::Editor, Rung::BottomPanel, Rung::Palette]
//!     }
//! }
//!
//! // Computed once: bottom panel is closed, palette is open.
//! let presence = FramePresence::from_fn(|r| !matches!(r, Rung::BottomPanel));
//!
//! let mut painted = Vec::new();
//! compose_frame(&presence, |r| painted.push(r));
//! assert_eq!(painted, vec![Rung::Editor, Rung::Palette]);
//!
//! // A hit-test walk built the same order independently — still valid.
//! assert!(check_frame_order(&[Rung::Editor, Rung::Palette]).is_ok());
//!
//! // A hit-test walk that regressed to checking the palette before the
//! // editor is caught immediately, not months later.
//! assert!(check_frame_order(&[Rung::Palette, Rung::Editor]).is_err());
//! ```

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

/// A typed, app-defined z-order rung (issue #774). Implement this once
/// per app-specific "what can appear in a frame" enum (an editor pane,
/// a bottom panel, an overlay stack, ...) to get a single declared
/// back-to-front order that both painting ([`compose_frame`]) and
/// hit-testing can consult — see the module docs above for the
/// vimcode#587/#592 drift this exists to make structurally impossible.
pub trait FrameRung: Copy + Eq + std::fmt::Debug + 'static {
    /// Every variant, back-to-front (bottom of the stack first, i.e.
    /// painted first / hit-tested last). The single source of truth —
    /// define it once, consult it from both paint and hit-test code.
    fn z_order() -> &'static [Self]
    where
        Self: Sized;

    /// This rung's position in [`Self::z_order`]. Panics if the rung
    /// is missing from `z_order()` — that is always a programmer
    /// error (an unlisted variant), not a runtime condition to handle.
    fn rank(&self) -> usize
    where
        Self: Sized,
    {
        Self::z_order()
            .iter()
            .position(|r| r == self)
            .unwrap_or_else(|| panic!("{self:?} is missing from FrameRung::z_order()"))
    }
}

/// Presence gate for a frame: which rungs of `R` exist right now,
/// evaluated once and reused by both the paint walk ([`compose_frame`])
/// and hit-testing, so "is this even open" can't be answered two
/// different ways in the same frame.
pub struct FramePresence<R> {
    present: Vec<(R, bool)>,
}

impl<R: FrameRung> FramePresence<R> {
    /// Evaluate `predicate` once per rung in [`FrameRung::z_order`].
    pub fn from_fn(predicate: impl Fn(R) -> bool) -> Self {
        let present = R::z_order().iter().map(|&r| (r, predicate(r))).collect();
        Self { present }
    }

    /// Whether `rung` is present this frame. Rungs not covered by
    /// [`Self::from_fn`]'s `z_order()` scan (impossible in practice,
    /// since it's derived from `z_order()` itself) are absent.
    pub fn is_present(&self, rung: R) -> bool {
        self.present
            .iter()
            .any(|(r, present)| *r == rung && *present)
    }

    /// Rungs that are present this frame, in canonical [`FrameRung::z_order`] order.
    pub fn present_rungs(&self) -> impl Iterator<Item = R> + '_ {
        self.present.iter().filter(|(_, p)| *p).map(|(r, _)| *r)
    }
}

/// Walk every present rung of `R`, back-to-front, invoking `paint`
/// once per rung. This is the non-branching frame composer: an app
/// supplies `presence` (from [`FramePresence::from_fn`]) and a
/// callback keyed by rung — no `match` over paint-operation variants,
/// and the walk order is [`FrameRung::z_order`] by construction, so it
/// cannot drift from what a `z_order()`-derived hit-test walk expects.
///
/// Returns the visited rungs in the order `paint` was called, e.g. to
/// feed to [`check_frame_order`] when composing the same order a
/// second time via an independent path (see module docs).
pub fn compose_frame<R: FrameRung>(
    presence: &FramePresence<R>,
    mut paint: impl FnMut(R),
) -> Vec<R> {
    let mut visited = Vec::new();
    for rung in presence.present_rungs() {
        paint(rung);
        visited.push(rung);
    }
    visited
}

/// Two rungs visited out of the order [`FrameRung::z_order`] declares:
/// `after` was visited following `before`, despite ranking lower (i.e.
/// further from "on top") in the canonical order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameOrderViolation<R> {
    pub before: R,
    pub after: R,
}

/// Assert that `visited` — some sequence of rungs actually painted or
/// hit-tested, produced however the caller likes — is consistent with
/// [`FrameRung::z_order`]: no rung may appear after a rung that ranks
/// higher has already appeared. Duplicates and gaps (rungs absent this
/// frame) are fine; the invariant is purely the *relative* order among
/// whatever was visited.
///
/// This is the guard that turns a `FRAME_Z_ORDER` vs.
/// `MOUSE_ARBITRATION_ORDER`-style drift (vimcode#587/#592) into a
/// failing assertion instead of a silent, months-later bug.
pub fn check_frame_order<R: FrameRung>(visited: &[R]) -> Result<(), FrameOrderViolation<R>> {
    let mut last: Option<(R, usize)> = None;
    for &rung in visited {
        let rank = rung.rank();
        if let Some((before, before_rank)) = last {
            if rank < before_rank {
                return Err(FrameOrderViolation {
                    before,
                    after: rung,
                });
            }
        }
        last = Some((rung, rank));
    }
    Ok(())
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestRung {
        Editor,
        BottomPanel,
        StatusBar,
        Palette,
    }

    impl FrameRung for TestRung {
        fn z_order() -> &'static [Self] {
            &[
                TestRung::Editor,
                TestRung::BottomPanel,
                TestRung::StatusBar,
                TestRung::Palette,
            ]
        }
    }

    #[test]
    fn frame_rung_rank_matches_z_order_position() {
        assert_eq!(TestRung::Editor.rank(), 0);
        assert_eq!(TestRung::BottomPanel.rank(), 1);
        assert_eq!(TestRung::StatusBar.rank(), 2);
        assert_eq!(TestRung::Palette.rank(), 3);
    }

    #[test]
    #[should_panic(expected = "missing from FrameRung::z_order()")]
    fn frame_rung_rank_panics_for_unlisted_variant() {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        struct Rogue;
        impl FrameRung for Rogue {
            fn z_order() -> &'static [Self] {
                &[]
            }
        }
        Rogue.rank();
    }

    #[test]
    fn frame_presence_gates_rungs_computed_once() {
        let presence = FramePresence::from_fn(|r| r != TestRung::BottomPanel);

        assert!(presence.is_present(TestRung::Editor));
        assert!(!presence.is_present(TestRung::BottomPanel));
        assert!(presence.is_present(TestRung::StatusBar));
        assert!(presence.is_present(TestRung::Palette));

        assert_eq!(
            presence.present_rungs().collect::<Vec<_>>(),
            vec![TestRung::Editor, TestRung::StatusBar, TestRung::Palette]
        );
    }

    #[test]
    fn compose_frame_visits_only_present_rungs_in_z_order() {
        let presence = FramePresence::<TestRung>::from_fn(|r| r != TestRung::Palette);

        let mut painted = Vec::new();
        let visited = compose_frame(&presence, |r| painted.push(r));

        let expected = vec![TestRung::Editor, TestRung::BottomPanel, TestRung::StatusBar];
        assert_eq!(painted, expected);
        assert_eq!(visited, expected);
    }

    #[test]
    fn check_frame_order_accepts_canonical_and_gapped_sequences() {
        // Exact canonical order.
        assert!(check_frame_order(&[
            TestRung::Editor,
            TestRung::BottomPanel,
            TestRung::StatusBar,
            TestRung::Palette,
        ])
        .is_ok());

        // A rung absent this frame just leaves a gap — still fine.
        assert!(check_frame_order(&[TestRung::Editor, TestRung::Palette]).is_ok());

        // Repeated visits to the same rung don't violate order.
        assert!(
            check_frame_order(&[TestRung::Editor, TestRung::Editor, TestRung::StatusBar]).is_ok()
        );

        // Empty and single-element sequences are trivially ordered.
        assert!(check_frame_order::<TestRung>(&[]).is_ok());
        assert!(check_frame_order(&[TestRung::Palette]).is_ok());
    }

    #[test]
    fn check_frame_order_rejects_out_of_order_visits() {
        // This is the vimcode#587/#592-shaped bug: a hit-test walk
        // (or a paint routine mid-migration to `compose_frame`) that
        // visits the palette before the editor, when `z_order()` says
        // the palette is on top.
        let err = check_frame_order(&[TestRung::Palette, TestRung::Editor]).unwrap_err();
        assert_eq!(
            err,
            FrameOrderViolation {
                before: TestRung::Palette,
                after: TestRung::Editor,
            }
        );
    }

    #[test]
    fn compose_frame_output_always_satisfies_check_frame_order() {
        // By construction: compose_frame walks z_order() directly, so
        // whatever it visits must already be in-order. This is the
        // structural half of the #774 guarantee — the assertion in
        // `check_frame_order` is the guard for code paths that don't
        // go through `compose_frame`.
        let presence = FramePresence::<TestRung>::from_fn(|r| r != TestRung::StatusBar);
        let visited = compose_frame(&presence, |_| {});
        assert!(check_frame_order(&visited).is_ok());
    }
}
