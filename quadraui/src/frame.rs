//! Frame-level rendering: declarative surface list + unified hit-test.
//!
//! Apps build a [`ScreenLayout`] by pushing [`Surface`] entries in
//! back-to-front z-order, then call [`ScreenLayout::draw`] to render
//! everything and get back a [`FrameHitMap`] for click dispatch.
//!
//! Apps that already paint each surface at its natural call site
//! can instead push the same [`Surface`] entries into a `ScreenLayout`
//! purely for hit-testing and call [`ScreenLayout::hit_map`], which
//! registers every zone without invoking any `backend.draw_*()` call.
//!
//! ## `Surface` vs. calling `Backend::draw_*` directly
//!
//! `ScreenLayout` + [`Surface`] is the **canonical path for a consumer
//! assembling a top-level screen out of multiple primitives**: pushing
//! `Surface` entries and calling [`ScreenLayout::draw`] routes both
//! backends through the same call site, so they cannot each paint the
//! identical primitive a different way. `Backend::draw_<name>` remains
//! public, low-level API — `ScreenLayout::draw` calls it internally,
//! and rasteriser tests / compose helpers call it directly by design.
//! See `quadraui/docs/decisions/DECISIONS.md` D-006 for the full
//! decision and `quadraui/docs/PRIMITIVE_RULES.md` "One primitive,
//! one canonical paint path" for the authoring rule this implies for
//! new primitives.
//!
//! ## `Surface` coverage
//!
//! Every primitive that has a `Backend::draw_*` method has a matching
//! [`Surface`] variant (`Board`, `CommandCenter`, `DiffView`,
//! `DropOverlay`, `Image`, `MessageList`, `Minimap`, `PipelineView`,
//! `Progress`, `SidebarPanel`, `Spinner`, `SplitTree`, `TextInput`,
//! `Toolbar` included). `Backend::draw_<name>` stays reachable directly
//! for the reasons above; there is no primitive that is categorically
//! unreachable through the declarative path.
//!
//! ## `FrameHitMap` is serializable; [`OwnedFrame`] is the owned `Frame`
//!
//! [`Surface<'a>`] borrows every primitive it paints — correct for a
//! same-frame paint/hit-test pass, useless for anything that needs to
//! outlive the frame: an accessibility tree snapshot (AccessKit), a
//! web backend serializing a frame description across a WASM/JS
//! boundary, or a record/replay harness persisting what was on screen
//! at a given tick. [`FrameHitMap`] — already returned by
//! [`ScreenLayout::draw`]/[`ScreenLayout::hit_map`] — is that shape's
//! owned, lifetime-free counterpart for hit-testing purposes: it stores
//! `(Rect, FrameZone)` pairs by value. Both halves of that pair —
//! [`Rect`] and [`FrameZone`] — implement `serde::{Serialize,
//! Deserialize}`, so [`FrameHitMap`] itself does, and
//! [`FrameHitMap::zones`] / [`FrameHitMap::from_zones`] make the owned
//! data readable and round-trippable instead of only probeable via
//! [`FrameHitMap::hit_test`].
//!
//! A `FrameZone` alone is a bare tag (e.g. `FrameZone::Tree { idx: 3
//! }`) plus a frame-local surface index: it carries no primitive
//! content, no accessibility role, no label, no value, and cannot by
//! itself feed an AccessKit `TreeUpdate` node, be re-painted by a web
//! backend, or replay a frame. [`OwnedFrame`] is the owned, lifetime-free
//! counterpart that closes that gap: [`OwnedFrame::from_layout`] clones
//! every primitive a [`ScreenLayout`] was about to borrow into an
//! [`OwnedNode`] (rect, optional [`crate::types::WidgetId`], optional
//! [`crate::a11y::A11yInfo`], and the primitive itself as [`OwnedContent`]),
//! so the resulting [`OwnedFrame`] can be serialized, sent across a
//! process/wire boundary, or inspected long after the `ScreenLayout` that
//! produced it has gone out of scope. [`OwnedFrame::draw`] repaints it
//! through the same `Backend` calls [`ScreenLayout::draw`] would have
//! made, and [`OwnedFrame::hit_map`] derives a [`FrameHitMap`] without a
//! backend at all — both are lock-step with the original push order
//! because the index each [`OwnedNode`] carries is captured once, at
//! `from_layout` time.

//! ## Presence gating + the paint/hit-test order invariant
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
//! And the fix was a `debug_assert!` comparing the
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
use crate::primitives::board::BoardModel;
use crate::primitives::chart::Chart;
use crate::primitives::command_center::CommandCenter;
use crate::primitives::command_line::CommandLine;
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::primitives::data_table::DataTable;
use crate::primitives::dialog::{Dialog, DialogLayout};
use crate::primitives::diff_view::DiffView;
use crate::primitives::drop_zone::DropOverlay;
use crate::primitives::editor::Editor;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::float::{Float, FloatLayout};
use crate::primitives::form::Form;
use crate::primitives::image::Image;
use crate::primitives::list::ListView;
use crate::primitives::menu_bar::MenuBar;
use crate::primitives::message_list::MessageList;
use crate::primitives::minimap::Minimap;
use crate::primitives::multi_section_view::MultiSectionView;
use crate::primitives::palette::Palette;
use crate::primitives::panel::Panel;
use crate::primitives::pipeline_view::PipelineView;
use crate::primitives::progress::ProgressBar;
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::sidebar_panel::SidebarPanel;
use crate::primitives::spinner::Spinner;
use crate::primitives::split::Split;
use crate::primitives::split_tree::SplitTree;
use crate::primitives::status_bar::StatusBar;
use crate::primitives::tab_bar::TabBar;
use crate::primitives::terminal::Terminal;
use crate::primitives::text_display::TextDisplay;
use crate::primitives::text_input::TextInput;
use crate::primitives::toast::ToastOverlay;
use crate::primitives::toolbar::Toolbar;
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::primitives::tree::TreeView;
use crate::types::WidgetId;
use crate::Backend;

/// A surface to render in a single frame. Entries are pushed in
/// back-to-front z-order; [`ScreenLayout::draw`] renders them
/// sequentially and the resulting [`FrameHitMap`] checks the
/// highest-z surface first.
///
/// `#[non_exhaustive]`: a new primitive adds a new variant here, and
/// per CLAUDE.md's *Downstream consumers* rule 2, a variant addition
/// should be additive rather than a potential downstream `E0004`.
/// Verified safe to add today: neither `coord-tui` nor `vimcode`
/// exhaustively matches `Surface` without a wildcard arm (grep in the
/// PR body).
#[allow(clippy::large_enum_variant)]
#[non_exhaustive]
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
    /// Like `Tooltip`/`ContextMenu`/`Dialog` above, carries its own
    /// resolved `layout` rather than a plain `rect` — `Float`
    /// is an overlay-with-caller-anchor primitive, so there's no bare
    /// `rect` the host would otherwise have on hand.
    Float {
        float: &'a Float,
        layout: &'a FloatLayout,
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
        stack: &'a ToastOverlay,
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
    Board {
        rect: Rect,
        model: &'a BoardModel,
    },
    CommandCenter {
        rect: Rect,
        cc: &'a CommandCenter,
    },
    DiffView {
        rect: Rect,
        view: &'a DiffView,
    },
    /// Tab-drag highlight overlay. Unlike every other
    /// transient-overlay variant above (`Tooltip`, `ContextMenu`,
    /// `Dialog`, `Completions`, `RichTextPopup`), there is no `rect`
    /// or `*Layout.bounds` to carry — [`DropOverlay::bounds`] derives
    /// the zone rect from `overlay` itself.
    DropOverlay {
        overlay: &'a DropOverlay,
    },
    Image {
        rect: Rect,
        image: &'a Image,
    },
    MessageList {
        rect: Rect,
        list: &'a MessageList,
    },
    Minimap {
        rect: Rect,
        minimap: &'a Minimap,
    },
    PipelineView {
        rect: Rect,
        view: &'a PipelineView,
    },
    Progress {
        rect: Rect,
        bar: &'a ProgressBar,
    },
    /// Mirrors [`Surface::StatusBar`]'s `hovered`/`pressed` shape: a
    /// `Surface` is a per-frame description, not owned
    /// [`crate::interaction::InteractionState`], so [`ScreenLayout::draw`]
    /// rebuilds one from these two borrowed ids at the call boundary.
    SidebarPanel {
        rect: Rect,
        panel: &'a SidebarPanel,
        hovered: Option<&'a WidgetId>,
        pressed: Option<&'a WidgetId>,
    },
    Spinner {
        rect: Rect,
        spinner: &'a Spinner,
    },
    /// Structural chrome, like [`Surface::Split`] — a divider set with
    /// no independent focusable identity, so excluded from
    /// [`ScreenLayout::tab_stops`].
    SplitTree {
        rect: Rect,
        tree: &'a SplitTree,
    },
    TextInput {
        rect: Rect,
        ti: &'a TextInput,
    },
    /// See [`Surface::SidebarPanel`]'s doc for why `hovered`/`pressed`
    /// are borrowed ids rather than an owned `InteractionState`.
    Toolbar {
        rect: Rect,
        bar: &'a Toolbar,
        hovered: Option<&'a WidgetId>,
        pressed: Option<&'a WidgetId>,
    },
}

/// Identifies which surface zone a point landed in.
///
/// Implements `Serialize`/`Deserialize` so this type — and, with
/// [`Rect`] (already serializable), [`FrameHitMap`] as a whole — can
/// be persisted or sent across a process/wire boundary. See the module
/// doc's "`FrameHitMap` is serializable" section.
///
/// Every variant's `idx` is a **frame-local surface index**: its
/// meaning depends entirely on the push order of the
/// [`ScreenLayout`] that produced it, not on any stable identity (see
/// vimcode's `click.rs:323-327`, which documents the same sharp edge —
/// "global surface index, NOT a per-tab-bar position"). Serializing a
/// `FrameZone` and handing it to a different process is only
/// meaningful if that process reproduces the exact same push order;
/// [`FrameHitMap::from_zones`] does not and cannot validate that an
/// `idx` it is handed actually corresponds to anything on the
/// receiving end.
///
/// `#[non_exhaustive]` for the same reason as [`Surface`]: a new
/// `Surface` variant implies a new `FrameZone` twin, and that pairing
/// should stay additive downstream. Verified safe today: neither
/// `coord-tui` nor `vimcode` exhaustively matches `FrameZone` without
/// a wildcard arm (vimcode's one match, `click.rs:331-355`, ends in
/// `_ => {}`; grep in the PR body).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
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
    Float { idx: usize },
    Completions { idx: usize },
    FindReplace { idx: usize },
    RichTextPopup { idx: usize },
    Toast { idx: usize },
    DataTable { idx: usize },
    Chart { idx: usize },
    Board { idx: usize },
    CommandCenter { idx: usize },
    DiffView { idx: usize },
    DropOverlay { idx: usize },
    Image { idx: usize },
    MessageList { idx: usize },
    Minimap { idx: usize },
    PipelineView { idx: usize },
    Progress { idx: usize },
    SidebarPanel { idx: usize },
    Spinner { idx: usize },
    SplitTree { idx: usize },
    TextInput { idx: usize },
    Toolbar { idx: usize },
    Empty,
}

/// Hit regions collected during [`ScreenLayout::draw`]. Resolves
/// absolute coordinates to the highest-z surface that contains them.
///
/// Every `(Rect, FrameZone)` pair is stored by value, so unlike
/// [`Surface<'a>`] a `FrameHitMap` carries no lifetime and can outlive
/// the frame it was built from. `Rect` and `FrameZone` both implement
/// `serde::{Serialize, Deserialize}`, so this struct derives them too
/// — it can be persisted or sent across a process/wire boundary.
/// **This is a step toward an owned `Frame`, not the owned frame
/// itself**: a `FrameZone` carries no primitive content, role, label
/// or value, only a bare tag and a frame-local index — see the module
/// doc's "`FrameHitMap` is serializable" section for what that means
/// for a receiver on the other end of a wire, and for what is still
/// missing.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
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

    /// Build a [`FrameHitMap`] directly from owned `(Rect, FrameZone)`
    /// pairs — the inverse of [`Self::zones`]. Lets a
    /// consumer that deserialized a frame shape (or otherwise built one
    /// independently of [`ScreenLayout::draw`]/[`ScreenLayout::hit_map`])
    /// hand it back in for [`Self::hit_test`] to resolve clicks against,
    /// in the same back-to-front z-order `ScreenLayout` would have
    /// produced (last entry wins ties, matching [`Self::hit_test`]'s
    /// "highest-z match" contract). Does **not** validate that each
    /// `FrameZone`'s `idx` corresponds to anything meaningful — see
    /// [`FrameZone`]'s doc for why that index is push-order-dependent
    /// and cannot be checked for provenance here.
    pub fn from_zones(zones: Vec<(Rect, FrameZone)>) -> Self {
        Self { zones }
    }

    /// The owned `(Rect, FrameZone)` pairs this hit map was built from,
    /// in the same back-to-front order they were pushed — the read
    /// half of the owned/serializable contract described in the module
    /// doc. A consumer that wants more than point
    /// containment (enumerate every zone to build an accessibility
    /// tree, serialize the whole frame, diff two frames for
    /// record/replay) reads this directly instead of probing
    /// [`Self::hit_test`] pixel by pixel.
    pub fn zones(&self) -> &[(Rect, FrameZone)] {
        &self.zones
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
                    // The trait's `WidgetId`-keyed hover/pressed
                    // store. `Surface::StatusBar` still carries the two
                    // borrowed ids because a `Surface` is a per-frame
                    // *description*, not owned state — rebuild the
                    // store at the call boundary.
                    let interaction = crate::interaction::InteractionState::from_parts(
                        hovered.cloned(),
                        pressed.cloned(),
                    );
                    backend.draw_status_bar_interactive(*rect, bar, &interaction);
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
                Surface::Float { float, layout } => {
                    backend.draw_float(float, layout);
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
                    backend.draw_toast_overlay(*rect, stack);
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
                Surface::Board { rect, model } => {
                    backend.draw_board(*rect, model);
                }
                Surface::CommandCenter { rect, cc } => {
                    backend.draw_command_center(*rect, cc);
                }
                Surface::DiffView { rect, view } => {
                    backend.draw_diff_view(*rect, view);
                }
                Surface::DropOverlay { overlay } => {
                    backend.draw_drop_overlay(overlay);
                }
                Surface::Image { rect, image } => {
                    backend.draw_image(*rect, image);
                }
                Surface::MessageList { rect, list } => {
                    backend.draw_message_list(*rect, list);
                }
                Surface::Minimap { rect, minimap } => {
                    backend.draw_minimap(*rect, minimap);
                }
                Surface::PipelineView { rect, view } => {
                    backend.draw_pipeline_view(*rect, view);
                }
                Surface::Progress { rect, bar } => {
                    backend.draw_progress(*rect, bar);
                }
                Surface::SidebarPanel {
                    rect,
                    panel,
                    hovered,
                    pressed,
                } => {
                    let interaction = crate::interaction::InteractionState::from_parts(
                        hovered.cloned(),
                        pressed.cloned(),
                    );
                    backend.draw_sidebar_panel_interactive(*rect, panel, &interaction);
                }
                Surface::Spinner { rect, spinner } => {
                    backend.draw_spinner(*rect, spinner);
                }
                Surface::SplitTree { rect, tree } => {
                    backend.draw_split_tree(*rect, tree);
                }
                Surface::TextInput { rect, ti } => {
                    backend.draw_text_input(*rect, ti);
                }
                Surface::Toolbar {
                    rect,
                    bar,
                    hovered,
                    pressed,
                } => {
                    let interaction = crate::interaction::InteractionState::from_parts(
                        hovered.cloned(),
                        pressed.cloned(),
                    );
                    backend.draw_toolbar_interactive(*rect, bar, &interaction);
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

    /// Tab order for this frame, as `(WidgetId, Rect)` pairs in reading
    /// order (top-to-bottom, then left-to-right by rect origin) — the
    /// shared source of truth [`crate::focus::FocusManager::sync_tab_order`]
    /// cycles through. An app that builds its screen via
    /// `ScreenLayout` hands this straight to
    /// [`crate::runner::AppLogic::tab_stops`]; see that method's doc for
    /// the opt-in contract.
    ///
    /// Only surfaces that carry both a plain `rect` and an owning
    /// [`WidgetId`] participate. Excluded, deliberately:
    /// - Transient overlays (`Tooltip`, `ContextMenu`, `Dialog`,
    ///   `Float`, `Completions`, `RichTextPopup`, `FindReplace`,
    ///   `Toast`, `DropOverlay`) — these already own their own internal
    ///   focus/dismiss handling while open, and being modal, nothing
    ///   behind them should be Tab-reachable anyway. `Float` in
    ///   particular already has its own, finer-grained focus story —
    ///   [`crate::ModalStack::top_focusable`] — rather than
    ///   participating in `Tab`/`Shift+Tab` cycling.
    /// - Structural chrome (`Split`, `Scrollbar`, `SplitTree`) —
    ///   containers/controls with no independent focusable identity of
    ///   their own.
    /// - `Image` and `Spinner` — purely decorative, never focusable,
    ///   regardless of the "carries a `rect` plus an owning `WidgetId`"
    ///   rule: a consumer that adopts them would otherwise get dead
    ///   stops in its Tab order.
    /// - `Progress` **unless** `ProgressBar::cancellable` is set — a
    ///   bare progress readout has nothing to activate, but a
    ///   cancellable one has a real affordance ([`ProgressBarHit::Cancel`])
    ///   worth reaching by keyboard. `Minimap` stays included even
    ///   though it is also mostly visual, because it is click-to-scroll
    ///   and so has a real interaction to land on.
    pub fn tab_stops(&self) -> Vec<(WidgetId, Rect)> {
        let mut stops: Vec<(WidgetId, Rect)> = self
            .surfaces
            .iter()
            .filter_map(|s| match s {
                Surface::Editor { rect, editor } => Some((editor.id, *rect)),
                Surface::TabBar { rect, bar, .. } => Some((bar.id, *rect)),
                Surface::StatusBar { rect, bar, .. } => Some((bar.id, *rect)),
                Surface::ActivityBar { rect, bar, .. } => Some((bar.id, *rect)),
                Surface::CommandLine { rect, cmd } => Some((cmd.id, *rect)),
                Surface::Terminal { rect, term } => Some((term.id, *rect)),
                Surface::TextDisplay { rect, td } => Some((td.id, *rect)),
                Surface::MultiSectionView { rect, view } => Some((view.id, *rect)),
                Surface::Tree { rect, tree } => Some((tree.id, *rect)),
                Surface::List { rect, list } => Some((list.id, *rect)),
                Surface::Form { rect, form } => Some((form.id, *rect)),
                Surface::MenuBar { rect, bar } => Some((bar.id, *rect)),
                Surface::Panel { rect, panel } => Some((panel.id, *rect)),
                Surface::Palette { rect, palette } => Some((palette.id, *rect)),
                Surface::DataTable { rect, table, .. } => Some((table.id, *rect)),
                Surface::Chart { rect, chart, .. } => Some((chart.id, *rect)),
                Surface::Board { rect, model } => Some((model.id, *rect)),
                Surface::CommandCenter { rect, cc } => Some((cc.id, *rect)),
                Surface::DiffView { rect, view } => Some((view.id, *rect)),
                Surface::MessageList { rect, list } => Some((list.id, *rect)),
                Surface::Minimap { rect, minimap } => Some((minimap.id, *rect)),
                Surface::PipelineView { rect, view } => Some((view.id, *rect)),
                Surface::Progress { rect, bar } if bar.cancellable => Some((bar.id, *rect)),
                Surface::SidebarPanel { rect, panel, .. } => Some((panel.id, *rect)),
                Surface::TextInput { rect, ti } => Some((ti.id, *rect)),
                Surface::Toolbar { rect, bar, .. } => Some((bar.id, *rect)),
                Surface::Image { .. }
                | Surface::Spinner { .. }
                | Surface::Progress { .. }
                | Surface::Split { .. }
                | Surface::Scrollbar { .. }
                | Surface::Tooltip { .. }
                | Surface::ContextMenu { .. }
                | Surface::Dialog { .. }
                | Surface::Float { .. }
                | Surface::Completions { .. }
                | Surface::FindReplace { .. }
                | Surface::RichTextPopup { .. }
                | Surface::DropOverlay { .. }
                | Surface::SplitTree { .. }
                | Surface::Toast { .. } => None,
            })
            .collect();
        stops.sort_by(|(_, a), (_, b)| {
            a.y.partial_cmp(&b.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
        });
        stops
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
            Surface::Float { layout, .. } => (layout.bounds, FrameZone::Float { idx }),
            Surface::Completions { layout, .. } => (layout.bounds, FrameZone::Completions { idx }),
            Surface::FindReplace { rect, .. } => (*rect, FrameZone::FindReplace { idx }),
            Surface::RichTextPopup { layout, .. } => {
                (layout.bounds, FrameZone::RichTextPopup { idx })
            }
            Surface::Toast { rect, .. } => (*rect, FrameZone::Toast { idx }),
            Surface::DataTable { rect, .. } => (*rect, FrameZone::DataTable { idx }),
            Surface::Chart { rect, .. } => (*rect, FrameZone::Chart { idx }),
            Surface::Board { rect, .. } => (*rect, FrameZone::Board { idx }),
            Surface::CommandCenter { rect, .. } => (*rect, FrameZone::CommandCenter { idx }),
            Surface::DiffView { rect, .. } => (*rect, FrameZone::DiffView { idx }),
            Surface::DropOverlay { overlay } => (overlay.bounds(), FrameZone::DropOverlay { idx }),
            Surface::Image { rect, .. } => (*rect, FrameZone::Image { idx }),
            Surface::MessageList { rect, .. } => (*rect, FrameZone::MessageList { idx }),
            Surface::Minimap { rect, .. } => (*rect, FrameZone::Minimap { idx }),
            Surface::PipelineView { rect, .. } => (*rect, FrameZone::PipelineView { idx }),
            Surface::Progress { rect, .. } => (*rect, FrameZone::Progress { idx }),
            Surface::SidebarPanel { rect, .. } => (*rect, FrameZone::SidebarPanel { idx }),
            Surface::Spinner { rect, .. } => (*rect, FrameZone::Spinner { idx }),
            Surface::SplitTree { rect, .. } => (*rect, FrameZone::SplitTree { idx }),
            Surface::TextInput { rect, .. } => (*rect, FrameZone::TextInput { idx }),
            Surface::Toolbar { rect, .. } => (*rect, FrameZone::Toolbar { idx }),
        }
    }
}

impl<'a> Default for ScreenLayout<'a> {
    fn default() -> Self {
        Self::new()
    }
}

/// A single primitive's content, owned rather than borrowed — the
/// payload half of an [`OwnedNode`]. One variant per [`Surface`]
/// variant that carries a `serde`-serializable primitive.
///
/// `#[non_exhaustive]` for the same reason as [`Surface`] and
/// [`FrameZone`]: a new primitive adds a new variant here additively.
///
/// [`Self::DropOverlay`] and [`Self::FindReplace`] are the fallbacks for
/// the two [`Surface`] variants whose primitive does not (yet) implement
/// `serde::{Serialize, Deserialize}` ([`DropOverlay`], [`FindReplacePanel`]
/// respectively) — the node still gets a rect and its own distinct
/// [`FrameZone`] tag via [`OwnedFrame::hit_map`], but carries no content
/// to redraw or inspect. [`Self::Opaque`] is the generic fallback for any
/// future primitive that likewise can't be captured yet.
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum OwnedContent {
    Editor(Editor),
    TabBar(TabBar),
    StatusBar(StatusBar),
    ActivityBar(ActivityBar),
    CommandLine(CommandLine),
    Terminal(Terminal),
    TextDisplay(TextDisplay),
    MultiSectionView(MultiSectionView),
    Tree(TreeView),
    List(ListView),
    Form(Form),
    MenuBar(MenuBar),
    Split(Split),
    Panel(Panel),
    Scrollbar(Scrollbar),
    Palette(Palette),
    Tooltip(Tooltip),
    ContextMenu(ContextMenu),
    Dialog(Dialog),
    Float(Float),
    Completions(Completions),
    RichTextPopup(RichTextPopup),
    Toast(ToastOverlay),
    DataTable(DataTable),
    Chart(Chart),
    Board(BoardModel),
    CommandCenter(CommandCenter),
    DiffView(DiffView),
    Image(Image),
    MessageList(MessageList),
    Minimap(Minimap),
    PipelineView(PipelineView),
    Progress(ProgressBar),
    SidebarPanel(SidebarPanel),
    Spinner(Spinner),
    SplitTree(SplitTree),
    TextInput(TextInput),
    Toolbar(Toolbar),
    /// No content captured — the source [`Surface`] was [`Surface::DropOverlay`].
    DropOverlay,
    /// No content captured — the source [`Surface`] was [`Surface::FindReplace`].
    FindReplace,
    /// No content captured — a generic fallback for any future
    /// non-serializable primitive. See this enum's doc for which
    /// [`Surface`] variants use the more specific fallbacks instead.
    Opaque,
}

impl OwnedContent {
    /// Repaint this content through the matching `Backend::draw_*`
    /// call, mirroring [`ScreenLayout::draw`]'s match arm for the
    /// [`Surface`] variant this content was captured from. Transient,
    /// per-frame interaction state (hover/press ids, hovered row
    /// indices) is not captured by [`OwnedFrame::from_layout`], so
    /// every such parameter is passed as `None`/absent here.
    ///
    /// The six overlay variants that need a non-serializable `*Layout`
    /// companion to redraw (`Tooltip`, `ContextMenu`, `Dialog`, `Float`,
    /// `Completions`, `RichTextPopup`), plus [`Self::DropOverlay`],
    /// [`Self::FindReplace`] and [`Self::Opaque`], are no-ops: their
    /// hit-test zone still round-trips, but repainting them needs either
    /// a layout or primitive content this owned snapshot does not keep.
    fn draw(&self, rect: Rect, backend: &mut dyn Backend) {
        match self {
            OwnedContent::Editor(editor) => {
                backend.draw_editor(rect, editor);
            }
            OwnedContent::TabBar(bar) => {
                backend.draw_tab_bar(rect, bar, None);
            }
            OwnedContent::StatusBar(bar) => {
                let interaction = crate::interaction::InteractionState::from_parts(None, None);
                backend.draw_status_bar_interactive(rect, bar, &interaction);
            }
            OwnedContent::ActivityBar(bar) => {
                backend.draw_activity_bar(rect, bar, None);
            }
            OwnedContent::CommandLine(cmd) => backend.draw_command_line(rect, cmd),
            OwnedContent::Terminal(term) => backend.draw_terminal(rect, term),
            OwnedContent::TextDisplay(td) => backend.draw_text_display(rect, td),
            OwnedContent::MultiSectionView(view) => backend.draw_multi_section_view(rect, view),
            OwnedContent::Tree(tree) => backend.draw_tree(rect, tree),
            OwnedContent::List(list) => backend.draw_list(rect, list),
            OwnedContent::Form(form) => backend.draw_form(rect, form),
            OwnedContent::MenuBar(bar) => {
                backend.draw_menu_bar(rect, bar);
            }
            OwnedContent::Split(split) => {
                backend.draw_split(rect, split);
            }
            OwnedContent::Panel(panel) => {
                backend.draw_panel(rect, panel);
            }
            OwnedContent::Scrollbar(sb) => backend.draw_scrollbar(rect, sb),
            OwnedContent::Palette(palette) => backend.draw_palette(rect, palette),
            OwnedContent::Toast(stack) => {
                backend.draw_toast_overlay(rect, stack);
            }
            OwnedContent::DataTable(table) => {
                backend.draw_data_table(rect, table, None);
            }
            OwnedContent::Chart(chart) => {
                backend.draw_chart(rect, chart, None, None);
            }
            OwnedContent::Board(model) => {
                backend.draw_board(rect, model);
            }
            OwnedContent::CommandCenter(cc) => {
                backend.draw_command_center(rect, cc);
            }
            OwnedContent::DiffView(view) => {
                backend.draw_diff_view(rect, view);
            }
            OwnedContent::Image(image) => {
                backend.draw_image(rect, image);
            }
            OwnedContent::MessageList(list) => backend.draw_message_list(rect, list),
            OwnedContent::Minimap(minimap) => {
                backend.draw_minimap(rect, minimap);
            }
            OwnedContent::PipelineView(view) => {
                backend.draw_pipeline_view(rect, view);
            }
            OwnedContent::Progress(bar) => {
                backend.draw_progress(rect, bar);
            }
            OwnedContent::SidebarPanel(panel) => {
                let interaction = crate::interaction::InteractionState::from_parts(None, None);
                backend.draw_sidebar_panel_interactive(rect, panel, &interaction);
            }
            OwnedContent::Spinner(spinner) => {
                backend.draw_spinner(rect, spinner);
            }
            OwnedContent::SplitTree(tree) => {
                backend.draw_split_tree(rect, tree);
            }
            OwnedContent::TextInput(ti) => {
                backend.draw_text_input(rect, ti);
            }
            OwnedContent::Toolbar(bar) => {
                let interaction = crate::interaction::InteractionState::from_parts(None, None);
                backend.draw_toolbar_interactive(rect, bar, &interaction);
            }
            OwnedContent::Tooltip(_)
            | OwnedContent::ContextMenu(_)
            | OwnedContent::Dialog(_)
            | OwnedContent::Float(_)
            | OwnedContent::Completions(_)
            | OwnedContent::RichTextPopup(_)
            | OwnedContent::DropOverlay
            | OwnedContent::FindReplace
            | OwnedContent::Opaque => {}
        }
    }

    /// The [`FrameZone`] this content contributes at frame-local index
    /// `idx`, mirroring [`ScreenLayout::zone_for`]'s tag-per-variant
    /// mapping so a `ScreenLayout` and the [`OwnedFrame`] captured from
    /// it produce identical zones.
    fn zone(&self, idx: usize) -> FrameZone {
        match self {
            OwnedContent::Editor(_) => FrameZone::Editor { idx },
            OwnedContent::TabBar(_) => FrameZone::TabBar { idx },
            OwnedContent::StatusBar(_) => FrameZone::StatusBar { idx },
            OwnedContent::ActivityBar(_) => FrameZone::ActivityBar { idx },
            OwnedContent::CommandLine(_) => FrameZone::CommandLine { idx },
            OwnedContent::Terminal(_) => FrameZone::Terminal { idx },
            OwnedContent::TextDisplay(_) => FrameZone::TextDisplay { idx },
            OwnedContent::MultiSectionView(_) => FrameZone::MultiSectionView { idx },
            OwnedContent::Tree(_) => FrameZone::Tree { idx },
            OwnedContent::List(_) => FrameZone::List { idx },
            OwnedContent::Form(_) => FrameZone::Form { idx },
            OwnedContent::MenuBar(_) => FrameZone::MenuBar { idx },
            OwnedContent::Split(_) => FrameZone::Split { idx },
            OwnedContent::Panel(_) => FrameZone::Panel { idx },
            OwnedContent::Scrollbar(_) => FrameZone::Scrollbar { idx },
            OwnedContent::Palette(_) => FrameZone::Palette { idx },
            OwnedContent::Tooltip(_) => FrameZone::Tooltip { idx },
            OwnedContent::ContextMenu(_) => FrameZone::ContextMenu { idx },
            OwnedContent::Dialog(_) => FrameZone::Dialog { idx },
            OwnedContent::Float(_) => FrameZone::Float { idx },
            OwnedContent::Completions(_) => FrameZone::Completions { idx },
            OwnedContent::RichTextPopup(_) => FrameZone::RichTextPopup { idx },
            OwnedContent::Toast(_) => FrameZone::Toast { idx },
            OwnedContent::DataTable(_) => FrameZone::DataTable { idx },
            OwnedContent::Chart(_) => FrameZone::Chart { idx },
            OwnedContent::Board(_) => FrameZone::Board { idx },
            OwnedContent::CommandCenter(_) => FrameZone::CommandCenter { idx },
            OwnedContent::DiffView(_) => FrameZone::DiffView { idx },
            OwnedContent::Image(_) => FrameZone::Image { idx },
            OwnedContent::MessageList(_) => FrameZone::MessageList { idx },
            OwnedContent::Minimap(_) => FrameZone::Minimap { idx },
            OwnedContent::PipelineView(_) => FrameZone::PipelineView { idx },
            OwnedContent::Progress(_) => FrameZone::Progress { idx },
            OwnedContent::SidebarPanel(_) => FrameZone::SidebarPanel { idx },
            OwnedContent::Spinner(_) => FrameZone::Spinner { idx },
            OwnedContent::SplitTree(_) => FrameZone::SplitTree { idx },
            OwnedContent::TextInput(_) => FrameZone::TextInput { idx },
            OwnedContent::Toolbar(_) => FrameZone::Toolbar { idx },
            OwnedContent::DropOverlay => FrameZone::DropOverlay { idx },
            OwnedContent::FindReplace => FrameZone::FindReplace { idx },
            OwnedContent::Opaque => FrameZone::Empty,
        }
    }
}

/// One captured surface in an [`OwnedFrame`]: the owned counterpart of
/// a single [`Surface`] entry. Every field is owned and `serde`-capable,
/// so an `OwnedNode` outlives the `ScreenLayout` it was captured from.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OwnedNode {
    /// Frame-local surface index, matching [`FrameZone`]'s `idx` —
    /// see that type's doc for the push-order-dependent caveat.
    pub idx: usize,
    /// Bounding rect this node paints/hit-tests at — the same rect
    /// [`ScreenLayout::zone_for`] would have used for the source
    /// [`Surface`] (plain `rect` for most variants, `layout.bounds` for
    /// the overlay variants that carry one, [`DropOverlay::bounds`] for
    /// [`Surface::DropOverlay`]).
    pub rect: Rect,
    /// This node's owning widget, when the source [`Surface`] variant
    /// carries one — mirrors [`ScreenLayout::tab_stops`]'s inclusion
    /// list exactly (structural chrome, transient overlays, and purely
    /// decorative primitives have no widget id here, matching that
    /// method's exclusions).
    pub widget_id: Option<WidgetId>,
    /// Accessibility sidecar data for this node. Always `None` when
    /// produced by [`OwnedFrame::from_layout`] — no primitive today
    /// carries [`crate::a11y::A11yInfo`] to capture — but present on the
    /// type so a consumer can attach it after the fact.
    pub a11y: Option<crate::a11y::A11yInfo>,
    /// The captured primitive content.
    pub content: OwnedContent,
}

/// An owned, lifetime-free frame tree: the serializable counterpart of
/// a [`ScreenLayout`]. Build one with [`Self::from_layout`], then call
/// [`Self::draw`] to repaint it or [`Self::hit_map`] to derive a
/// [`FrameHitMap`] without a backend. See the module doc's "`FrameHitMap`
/// is serializable; `OwnedFrame` is the owned `Frame`" section for the
/// full rationale.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OwnedFrame {
    nodes: Vec<OwnedNode>,
}

impl OwnedFrame {
    /// An empty `OwnedFrame`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture every surface in `layout` into an owned, serializable
    /// [`OwnedFrame`]. Clones each primitive rather than borrowing it,
    /// so the result outlives `layout` and the frame it described.
    pub fn from_layout(layout: &ScreenLayout<'_>) -> Self {
        let nodes = layout
            .surfaces
            .iter()
            .enumerate()
            .map(|(idx, surface)| {
                let (rect, widget_id, content) = capture_surface(surface);
                OwnedNode {
                    idx,
                    rect,
                    widget_id,
                    a11y: None,
                    content,
                }
            })
            .collect();
        Self { nodes }
    }

    /// The captured nodes, in the same back-to-front order they were
    /// pushed into the source [`ScreenLayout`].
    pub fn nodes(&self) -> &[OwnedNode] {
        &self.nodes
    }

    /// Repaint every node through `backend` and return the resulting
    /// [`FrameHitMap`] — the owned-data counterpart of
    /// [`ScreenLayout::draw`]. See [`OwnedContent::draw`] for which
    /// variants are no-ops here (they still register a hit zone).
    pub fn draw(&self, backend: &mut dyn Backend) -> FrameHitMap {
        let mut hit_map = FrameHitMap::new();
        for node in &self.nodes {
            node.content.draw(node.rect, backend);
            hit_map.push(node.rect, node.content.zone(node.idx));
        }
        hit_map
    }

    /// Derive the [`FrameHitMap`] this frame would produce, without
    /// invoking any `backend.draw_*()` call — the owned-data counterpart
    /// of [`ScreenLayout::hit_map`].
    pub fn hit_map(&self) -> FrameHitMap {
        let mut hit_map = FrameHitMap::new();
        for node in &self.nodes {
            hit_map.push(node.rect, node.content.zone(node.idx));
        }
        hit_map
    }
}

/// Capture one borrowed [`Surface`] into its owned `(rect, widget_id,
/// content)` parts. The widget-id rule mirrors
/// [`ScreenLayout::tab_stops`]'s inclusion list exactly; the rect rule
/// mirrors [`ScreenLayout::zone_for`]'s exactly. [`Surface::DropOverlay`]
/// falls back to [`OwnedContent::DropOverlay`] and [`Surface::FindReplace`]
/// falls back to [`OwnedContent::FindReplace`], because [`DropOverlay`]
/// and [`FindReplacePanel`] do not implement `serde::{Serialize,
/// Deserialize}` — both still keep their own distinct [`FrameZone`] tag.
fn capture_surface(surface: &Surface<'_>) -> (Rect, Option<WidgetId>, OwnedContent) {
    match surface {
        Surface::Editor { rect, editor } => (
            *rect,
            Some(editor.id),
            OwnedContent::Editor((*editor).clone()),
        ),
        Surface::TabBar { rect, bar, .. } => {
            (*rect, Some(bar.id), OwnedContent::TabBar((*bar).clone()))
        }
        Surface::StatusBar { rect, bar, .. } => {
            (*rect, Some(bar.id), OwnedContent::StatusBar((*bar).clone()))
        }
        Surface::ActivityBar { rect, bar, .. } => (
            *rect,
            Some(bar.id),
            OwnedContent::ActivityBar((*bar).clone()),
        ),
        Surface::CommandLine { rect, cmd } => (
            *rect,
            Some(cmd.id),
            OwnedContent::CommandLine((*cmd).clone()),
        ),
        Surface::Terminal { rect, term } => (
            *rect,
            Some(term.id),
            OwnedContent::Terminal((*term).clone()),
        ),
        Surface::TextDisplay { rect, td } => {
            (*rect, Some(td.id), OwnedContent::TextDisplay((*td).clone()))
        }
        Surface::MultiSectionView { rect, view } => (
            *rect,
            Some(view.id),
            OwnedContent::MultiSectionView((*view).clone()),
        ),
        Surface::Tree { rect, tree } => (*rect, Some(tree.id), OwnedContent::Tree((*tree).clone())),
        Surface::List { rect, list } => (*rect, Some(list.id), OwnedContent::List((*list).clone())),
        Surface::Form { rect, form } => (*rect, Some(form.id), OwnedContent::Form((*form).clone())),
        Surface::MenuBar { rect, bar } => {
            (*rect, Some(bar.id), OwnedContent::MenuBar((*bar).clone()))
        }
        Surface::Split { rect, split } => (*rect, None, OwnedContent::Split((*split).clone())),
        Surface::Panel { rect, panel } => {
            (*rect, Some(panel.id), OwnedContent::Panel((*panel).clone()))
        }
        Surface::Scrollbar { rect, sb } => (*rect, None, OwnedContent::Scrollbar((*sb).clone())),
        Surface::Palette { rect, palette } => (
            *rect,
            Some(palette.id),
            OwnedContent::Palette((*palette).clone()),
        ),
        Surface::Tooltip { tooltip, layout } => (
            layout.bounds,
            None,
            OwnedContent::Tooltip((*tooltip).clone()),
        ),
        Surface::ContextMenu { menu, layout } => (
            layout.bounds,
            None,
            OwnedContent::ContextMenu((*menu).clone()),
        ),
        Surface::Dialog { dialog, layout } => {
            (layout.bounds, None, OwnedContent::Dialog((*dialog).clone()))
        }
        Surface::Float { float, layout } => {
            (layout.bounds, None, OwnedContent::Float((*float).clone()))
        }
        Surface::Completions {
            completions,
            layout,
        } => (
            layout.bounds,
            None,
            OwnedContent::Completions((*completions).clone()),
        ),
        Surface::FindReplace { rect, .. } => (*rect, None, OwnedContent::FindReplace),
        Surface::RichTextPopup { popup, layout } => (
            layout.bounds,
            None,
            OwnedContent::RichTextPopup((*popup).clone()),
        ),
        Surface::Toast { rect, stack } => (*rect, None, OwnedContent::Toast((*stack).clone())),
        Surface::DataTable { rect, table, .. } => (
            *rect,
            Some(table.id),
            OwnedContent::DataTable((*table).clone()),
        ),
        Surface::Chart { rect, chart, .. } => {
            (*rect, Some(chart.id), OwnedContent::Chart((*chart).clone()))
        }
        Surface::Board { rect, model } => {
            (*rect, Some(model.id), OwnedContent::Board((*model).clone()))
        }
        Surface::CommandCenter { rect, cc } => (
            *rect,
            Some(cc.id),
            OwnedContent::CommandCenter((*cc).clone()),
        ),
        Surface::DiffView { rect, view } => (
            *rect,
            Some(view.id),
            OwnedContent::DiffView((*view).clone()),
        ),
        Surface::DropOverlay { overlay } => (overlay.bounds(), None, OwnedContent::DropOverlay),
        Surface::Image { rect, image } => (*rect, None, OwnedContent::Image((*image).clone())),
        Surface::MessageList { rect, list } => (
            *rect,
            Some(list.id),
            OwnedContent::MessageList((*list).clone()),
        ),
        Surface::Minimap { rect, minimap } => (
            *rect,
            Some(minimap.id),
            OwnedContent::Minimap((*minimap).clone()),
        ),
        Surface::PipelineView { rect, view } => (
            *rect,
            Some(view.id),
            OwnedContent::PipelineView((*view).clone()),
        ),
        Surface::Progress { rect, bar } => (
            *rect,
            if bar.cancellable { Some(bar.id) } else { None },
            OwnedContent::Progress((*bar).clone()),
        ),
        Surface::SidebarPanel { rect, panel, .. } => (
            *rect,
            Some(panel.id),
            OwnedContent::SidebarPanel((*panel).clone()),
        ),
        Surface::Spinner { rect, spinner } => {
            (*rect, None, OwnedContent::Spinner((*spinner).clone()))
        }
        Surface::SplitTree { rect, tree } => {
            (*rect, None, OwnedContent::SplitTree((*tree).clone()))
        }
        Surface::TextInput { rect, ti } => {
            (*rect, Some(ti.id), OwnedContent::TextInput((*ti).clone()))
        }
        Surface::Toolbar { rect, bar, .. } => {
            (*rect, Some(bar.id), OwnedContent::Toolbar((*bar).clone()))
        }
    }
}

/// A typed, app-defined z-order rung. Implement this once
/// per app-specific "what can appear in a frame" enum (an editor pane,
/// a bottom panel, an overlay stack, ...) to get a single declared
/// back-to-front order that both painting ([`compose_frame`]) and
/// hit-testing can consult.
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
/// `MOUSE_ARBITRATION_ORDER`-style drift into a
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
            scroll_top: 0.0,
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
            scroll_offset: 0.0,
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

    /// `tab_stops` derives Tab order from geometry (reading order),
    /// not push order — the whole point of deriving it from
    /// `ScreenLayout` rather than an app hand-maintaining a list.
    #[test]
    fn tab_stops_sorts_by_reading_order_not_push_order() {
        let editor = Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 10.0, 100.0, 20.0),
            lines: Vec::new(),
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0.0,
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
            scroll_offset: 0.0,
            total_count: 0,
            has_focus: false,
            show_query: true,
            create_label: None,
            preview: None,
            mode: crate::primitives::palette::PaletteMode::List,
        };
        let split = crate::primitives::split::Split {
            id: "split".into(),
            direction: crate::primitives::split::SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        };

        let mut layout = ScreenLayout::new();
        // Pushed in the "wrong" (bottom-to-top) order — `tab_stops` must
        // still resolve reading order (top-to-bottom) from the rects.
        layout.push(Surface::Editor {
            rect: editor.rect,
            editor: &editor,
        });
        layout.push(Surface::Palette {
            rect: Rect::new(0.0, 0.0, 50.0, 5.0),
            palette: &palette,
        });
        // Structural chrome: must not appear in tab order at all.
        layout.push(Surface::Split {
            rect: Rect::new(0.0, 30.0, 100.0, 1.0),
            split: &split,
        });

        let stops = layout.tab_stops();

        assert_eq!(
            stops.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["pal", "ed"],
            "palette (y=0) must sort before the editor (y=10), regardless of push order"
        );
        assert!(
            !stops.iter().any(|(id, _)| id.as_str() == "split"),
            "Split is structural chrome, not a tab stop"
        );
    }

    /// Every primitive with a `Backend::draw_*` method has a matching
    /// `Surface` variant. Builds one of each variant, pushes them all,
    /// and checks `hit_map()` registers every one under the right
    /// `FrameZone` tag.
    #[test]
    fn surface_gap_closed_for_all_fourteen_primitives() {
        let board = crate::primitives::board::BoardModel {
            id: "board".into(),
            columns: Vec::new(),
            selected_card_id: None,
            col_scroll_offset: 0.0,
        };
        let cc = CommandCenter {
            id: "cc".into(),
            back_enabled: false,
            forward_enabled: false,
            search_label: String::new(),
        };
        let diff = DiffView {
            id: "diff".into(),
            left: String::new(),
            right: String::new(),
            left_label: None,
            right_label: None,
            hunks: Vec::new(),
            mode: Default::default(),
            editability: Default::default(),
            scroll_offset: 0.0,
            focused_pane: Default::default(),
            has_focus: false,
        };
        let overlay = DropOverlay {
            highlight: Some(Rect::new(1.0, 1.0, 2.0, 2.0)),
            insertion_bar: None,
            ghost_position: None,
        };
        let image = Image {
            id: "img".into(),
            source: crate::primitives::image::ImageSource::Path(std::path::PathBuf::new()),
            intrinsic_size: None,
            fit: Default::default(),
            fallback_text: String::new(),
        };
        let message_list = MessageList {
            id: "ml".into(),
            rows: Vec::new(),
            scroll_top: 0.0,
        };
        let minimap = Minimap {
            id: "mm".into(),
            lines: Vec::new(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 0,
            total_buffer_lines: 0,
        };
        let pipeline = PipelineView {
            id: "pv".into(),
            stages: Vec::new(),
            focused_stage: None,
        };
        let progress = ProgressBar {
            id: "pb".into(),
            label: String::new(),
            value: None,
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let sidebar = SidebarPanel {
            id: "sp".into(),
            toolbar: None,
            toolbar_height: None,
        };
        let spinner = Spinner {
            id: "sn".into(),
            label: String::new(),
            frame_idx: 0,
            accent: None,
        };
        let split_tree = SplitTree::Leaf("leaf".into());
        let text_input = TextInput {
            id: "ti".into(),
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            placeholder: None,
            scroll_offset: 0.0,
            scroll_col: 0,
            has_focus: false,
        };
        let toolbar = Toolbar {
            id: "tb".into(),
            buttons: Vec::new(),
            bg: None,
            focused_index: None,
        };

        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let mut layout = ScreenLayout::new();
        layout.push(Surface::Board {
            rect,
            model: &board,
        });
        layout.push(Surface::CommandCenter { rect, cc: &cc });
        layout.push(Surface::DiffView { rect, view: &diff });
        layout.push(Surface::DropOverlay { overlay: &overlay });
        layout.push(Surface::Image {
            rect,
            image: &image,
        });
        layout.push(Surface::MessageList {
            rect,
            list: &message_list,
        });
        layout.push(Surface::Minimap {
            rect,
            minimap: &minimap,
        });
        layout.push(Surface::PipelineView {
            rect,
            view: &pipeline,
        });
        layout.push(Surface::Progress {
            rect,
            bar: &progress,
        });
        layout.push(Surface::SidebarPanel {
            rect,
            panel: &sidebar,
            hovered: None,
            pressed: None,
        });
        layout.push(Surface::Spinner {
            rect,
            spinner: &spinner,
        });
        layout.push(Surface::SplitTree {
            rect,
            tree: &split_tree,
        });
        layout.push(Surface::TextInput {
            rect,
            ti: &text_input,
        });
        layout.push(Surface::Toolbar {
            rect,
            bar: &toolbar,
            hovered: None,
            pressed: None,
        });

        let hit_map = layout.hit_map();
        assert_eq!(hit_map.len(), 14);

        let zones: Vec<&FrameZone> = hit_map.zones().iter().map(|(_, z)| z).collect();
        assert_eq!(zones[0], &FrameZone::Board { idx: 0 });
        assert_eq!(zones[1], &FrameZone::CommandCenter { idx: 1 });
        assert_eq!(zones[2], &FrameZone::DiffView { idx: 2 });
        assert_eq!(zones[3], &FrameZone::DropOverlay { idx: 3 });
        assert_eq!(zones[4], &FrameZone::Image { idx: 4 });
        assert_eq!(zones[5], &FrameZone::MessageList { idx: 5 });
        assert_eq!(zones[6], &FrameZone::Minimap { idx: 6 });
        assert_eq!(zones[7], &FrameZone::PipelineView { idx: 7 });
        assert_eq!(zones[8], &FrameZone::Progress { idx: 8 });
        assert_eq!(zones[9], &FrameZone::SidebarPanel { idx: 9 });
        assert_eq!(zones[10], &FrameZone::Spinner { idx: 10 });
        assert_eq!(zones[11], &FrameZone::SplitTree { idx: 11 });
        assert_eq!(zones[12], &FrameZone::TextInput { idx: 12 });
        assert_eq!(zones[13], &FrameZone::Toolbar { idx: 13 });

        // `DropOverlay`'s zone rect comes from `DropOverlay::bounds()`,
        // not a `rect` field it doesn't have.
        assert_eq!(hit_map.zones()[3].0, overlay.bounds());

        // Tab order: `DropOverlay` (transient overlay) and `SplitTree`
        // (structural chrome, like `Split`) don't participate, and
        // neither do the purely-decorative `Image`/`Spinner` or this
        // non-cancellable `Progress` (no activatable affordance).
        let stops = layout.tab_stops();
        let stop_ids: Vec<&str> = stops.iter().map(|(id, _)| id.as_str()).collect();
        for id in ["board", "cc", "diff", "ml", "mm", "pv", "sp", "ti", "tb"] {
            assert!(stop_ids.contains(&id), "{id} should be a tab stop");
        }
        for id in ["img", "sn", "pb", "leaf"] {
            assert!(!stop_ids.contains(&id), "{id} should not be a tab stop");
        }
        assert_eq!(stop_ids.len(), 9);
    }

    /// A cancellable `Progress` bar *does* get a tab stop — the cancel
    /// affordance (`ProgressBarHit::Cancel`) is a real interaction, unlike
    /// a bare progress readout.
    #[test]
    fn cancellable_progress_is_a_tab_stop() {
        let progress = ProgressBar {
            id: "pb".into(),
            label: String::new(),
            value: None,
            frame_idx: 0,
            cancellable: true,
            accent: None,
        };
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let mut layout = ScreenLayout::new();
        layout.push(Surface::Progress {
            rect,
            bar: &progress,
        });
        let stops = layout.tab_stops();
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].0.as_str(), "pb");
    }

    /// `Surface::Float` dispatches to `Backend::draw_float` and is
    /// excluded from Tab order — same transient-overlay treatment as
    /// `Tooltip`/`ContextMenu`/`Dialog` (see this module's
    /// `tab_stops` doc).
    #[test]
    fn surface_float_dispatches_to_draw_float_and_is_excluded_from_tab_stops() {
        use crate::layout::{Anchor, Side};
        use crate::testing::RecordingBackend;
        use crate::{Float, FloatMeasure};

        let editor = Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            lines: Vec::new(),
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0.0,
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
        let float = Float::new(
            WidgetId::new("f"),
            Anchor::new(Rect::new(0.0, 0.0, 5.0, 1.0), Side::Bottom),
        );
        let float_layout = float.layout(
            Rect::new(0.0, 0.0, 80.0, 24.0),
            FloatMeasure::new(10.0, 4.0),
        );

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Editor {
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            editor: &editor,
        });
        layout.push(Surface::Float {
            float: &float,
            layout: &float_layout,
        });

        let mut backend = RecordingBackend::new();
        layout.draw(&mut backend);
        assert!(backend.calls.contains(&"draw_float"));

        let stops = layout.tab_stops();
        assert_eq!(
            stops.len(),
            1,
            "the Float surface must not become a Tab stop — it owns its \
             own focus story via ModalStack::top_focusable"
        );
        assert_eq!(stops[0].0.as_str(), "ed");
    }

    /// `FrameHitMap` is serializable: round-trips through JSON
    /// (exercising the new `FrameZone` variants too) and reconstructs
    /// via `from_zones`/`zones`.
    #[test]
    fn frame_hit_map_serde_round_trip() {
        let mut map = FrameHitMap::new();
        map.push(Rect::new(0.0, 0.0, 10.0, 10.0), FrameZone::Board { idx: 0 });
        map.push(Rect::new(0.0, 0.0, 5.0, 5.0), FrameZone::Toolbar { idx: 1 });

        let json = serde_json::to_string(&map).expect("FrameHitMap serializes");
        let restored: FrameHitMap = serde_json::from_str(&json).expect("FrameHitMap deserializes");
        assert_eq!(restored, map);
        assert_eq!(restored.hit_test(1.0, 1.0), FrameZone::Toolbar { idx: 1 });

        // `from_zones` is the inverse of `zones()` — a map rebuilt from
        // its own owned data behaves identically.
        let rebuilt = FrameHitMap::from_zones(map.zones().to_vec());
        assert_eq!(rebuilt, map);
    }

    /// [`OwnedFrame`] round-trips through JSON with no backend in
    /// scope, and [`OwnedFrame::hit_map`] on the deserialized copy
    /// matches what the original [`ScreenLayout`] would have produced.
    #[test]
    fn owned_frame_serde_round_trip_preserves_hit_map() {
        let editor = Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            lines: Vec::new(),
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0.0,
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
            scroll_offset: 0.0,
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

        let captured = OwnedFrame::from_layout(&layout);
        assert_eq!(captured.nodes().len(), 2);

        let json = serde_json::to_string(&captured).expect("OwnedFrame serializes");
        let restored: OwnedFrame = serde_json::from_str(&json).expect("OwnedFrame deserializes");

        let hit_map = restored.hit_map();
        assert!(!hit_map.is_empty());
        assert_eq!(hit_map.len(), 2);
        // Point in the overlay → highest-z (last-pushed) wins, same as
        // `ScreenLayout::hit_map` would have reported for the source.
        assert_eq!(hit_map.hit_test(25.0, 25.0), FrameZone::Palette { idx: 1 });
        // Point outside the overlay but inside the editor.
        assert_eq!(hit_map.hit_test(75.0, 75.0), FrameZone::Editor { idx: 0 });
        // Point outside everything.
        assert_eq!(hit_map.hit_test(150.0, 150.0), FrameZone::Empty);
    }

    /// The literal acceptance shape: build a frame, serialize it,
    /// deserialize it, `draw()` the deserialized copy through a real
    /// [`crate::tui::TuiBackend`] (the production `Backend` call path,
    /// not just `hit_map()`'s backend-free shortcut), and check the
    /// result is non-empty and correct.
    #[cfg(feature = "tui")]
    #[test]
    fn owned_frame_serde_round_trip_draws_through_tui_backend() {
        use crate::runner::{AppLogic, Reaction};
        use crate::tui::testing::TuiDriver;
        use std::cell::RefCell;

        let editor = Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 0.0, 20.0, 5.0),
            lines: Vec::new(),
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0.0,
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

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Editor {
            rect: Rect::new(0.0, 0.0, 20.0, 5.0),
            editor: &editor,
        });

        let captured = OwnedFrame::from_layout(&layout);
        let json = serde_json::to_string(&captured).expect("OwnedFrame serializes");
        let restored: OwnedFrame = serde_json::from_str(&json).expect("OwnedFrame deserializes");

        /// Drives the deserialized `OwnedFrame` through the real
        /// `Backend` trait on every render, exactly as a long-running
        /// app's render hook would, and stashes the resulting
        /// `FrameHitMap` for the test to inspect afterward.
        struct DrawRestoredFrame(OwnedFrame, RefCell<Option<FrameHitMap>>);
        impl AppLogic for DrawRestoredFrame {
            type AreaId = ();
            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let hit_map = self.0.draw(backend);
                *self.1.borrow_mut() = Some(hit_map);
            }
            fn handle(&mut self, _event: crate::UiEvent, _backend: &mut dyn Backend) -> Reaction {
                Reaction::Continue
            }
        }

        let app = DrawRestoredFrame(restored, RefCell::new(None));
        let driver = TuiDriver::new(app, 20, 5);
        let hit_map = driver
            .app()
            .1
            .borrow()
            .clone()
            .expect("render ran at least once during TuiDriver::new");

        assert!(!hit_map.is_empty());
        assert_eq!(hit_map.hit_test(1.0, 1.0), FrameZone::Editor { idx: 0 });
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
