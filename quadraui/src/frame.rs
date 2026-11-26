//! Frame-level rendering: declarative surface list + unified hit-test.
//!
//! Apps build a [`ScreenLayout`] by pushing [`Surface`] entries in
//! back-to-front z-order, then call [`ScreenLayout::draw`] to render
//! everything. [`ScreenLayout::draw`]'s [`FrameHitMap`] return value and
//! [`ScreenLayout::hit_map`] are both `#[deprecated]` — route
//! [`Surface::Widget`] clicks through [`ScreenLayout::into_frame_tree`]
//! and [`crate::frame_tree::FrameTree`] instead; every other `Surface`
//! variant's app recomputes its own rects for hit-testing the way
//! `examples/common/frame_demo.rs` does, since there is no
//! [`crate::interactive::Interactive`] widget behind it for a
//! [`crate::frame_tree::FrameTree`] node to hold.
//!
//! Apps that already paint each surface at its natural call site can
//! instead push the same [`Surface`] entries into a `ScreenLayout` purely
//! for hit-testing and call the deprecated [`ScreenLayout::hit_map`],
//! which registers every zone without invoking any `backend.draw_*()`
//! call.
//!
//! `ScreenLayout::tab_stops` derives Tab order from the same pushed
//! surfaces, and `ScreenLayout::tree_frame` (see its own doc) is an
//! in-progress, not-yet-consumed handle-based alternative to
//! re-walking every pushed `Surface::Tree` each frame.
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
//! [`Surface`] variant (`Board`, `DiffView`, `Image`, `MessageList`,
//! `Minimap`, `PipelineView`, `Progress`, `Spinner`, `SplitTree`
//! included). `Backend::draw_<name>` stays reachable directly for the
//! reasons above; there is no primitive that is categorically
//! unreachable through the declarative path.
//!
//! ## `FrameHitMap` is serializable; [`OwnedFrame`] is the owned `Frame`
//!
//! [`Surface`] owns every primitive it paints, so it carries no
//! lifetime and a [`ScreenLayout`] can be built in one place and drawn
//! in another. What a `Surface` is not is *serializable*: it
//! implements no `serde` traits, so it cannot by itself serve the
//! cases that need a frame description to leave the process — an
//! accessibility tree snapshot (AccessKit), a web backend serializing
//! a frame description across a WASM/JS boundary, or a record/replay
//! harness persisting what was on screen at a given tick.
//! [`FrameHitMap`] — already returned by
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
//! backend, or replay a frame. [`OwnedFrame`] is the `serde`-capable
//! counterpart that closes that gap: [`OwnedFrame::from_layout`] clones
//! every primitive a [`ScreenLayout`] holds into an
//! [`OwnedNode`] (rect, optional [`crate::types::WidgetId`], optional
//! [`crate::a11y::A11yInfo`], and the primitive itself as [`OwnedContent`]),
//! so the resulting [`OwnedFrame`] can be serialized, sent across a
//! process/wire boundary, or inspected entirely independently of the
//! `ScreenLayout` that produced it. [`OwnedFrame::draw`] repaints it
//! through the same `Backend` calls [`ScreenLayout::draw`] would have
//! made, and [`OwnedFrame::hit_map`] derives a [`FrameHitMap`] without a
//! backend at all — both are lock-step with the original push order
//! because the index each [`OwnedNode`] carries is captured once, at
//! `from_layout` time.
//!
//! That `serde` capability is the whole reason both types exist:
//! [`Surface`] is owned-but-not-serde and takes any primitive quadraui
//! can paint, while [`OwnedFrame`] is the serializable snapshot and is
//! therefore limited to primitives that are themselves `serde`-capable.
//! [`FindReplacePanel`] is not, which is why [`OwnedContent`] has the
//! contentless [`OwnedContent::FindReplace`] fallback (and
//! [`OwnedContent::Opaque`] as the generic one). Reach
//! for `Surface`/`ScreenLayout` to describe and paint a frame; reach
//! for `OwnedFrame` when that description has to be serialized.
//!
//! ## Live-frame capture for accessibility
//!
//! [`crate::testing::a11y::A11yScenario`] builds an [`OwnedFrame`]
//! straight from a test's own [`ScreenLayout`], with no backend
//! involved. A live runner has no such scenario to ask — it only ever
//! calls [`ScreenLayout::draw`] (or [`OwnedFrame::draw`]) against a real
//! [`Backend`]. Both of those methods, right after painting, ask the
//! backend for its [`FrameCapture`] slot via [`Backend::frame_capture`]
//! and, when [`FrameCapture::is_enabled`] says so, record the just-drawn
//! [`OwnedFrame`] in it — so [`Backend::last_frame`] can answer with
//! whatever the app actually had on screen. Capture is opt-in via
//! [`FrameCapture::set_enabled`] (or [`set_frame_capture`]) and costs
//! nothing when left off (the default): the check is one virtual call
//! and a `bool`, not a clone of every surface.
//!
//! A backend joins in by owning one [`FrameCapture`] field and returning
//! `Some(&mut …)` from [`Backend::frame_capture`] plus its
//! [`FrameCapture::last_frame`] from [`Backend::last_frame`]; both trait
//! methods default to `None`, so a backend that hasn't opted in reports
//! no frames rather than wrong ones.
//!
//! This only works through the canonical [`ScreenLayout`] path (D-006,
//! `docs/decisions/DECISIONS.md`) — an app that calls `backend.draw_*`
//! methods directly, bypassing `ScreenLayout`/`OwnedFrame`, never
//! populates `last_frame`, so it stays invisible to an accessibility
//! tree or an MCP agent built on top of one.

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

use std::collections::{HashMap, HashSet};

use crate::backend::DrawsPrimitives;
use crate::descriptor_store::{DescriptorStore, Handle};
use crate::event::Rect;
use crate::primitive::DynWidget;
use crate::primitives::activity_bar::ActivityBar;
use crate::primitives::board::BoardModel;
use crate::primitives::button::Button;
use crate::primitives::canvas::Canvas;
use crate::primitives::cell_grid::CellGrid;
use crate::primitives::chart::Chart;
use crate::primitives::command_center::CommandCenter;
use crate::primitives::command_line::CommandLine;
use crate::primitives::completions::Completions;
use crate::primitives::context_menu::ContextMenu;
use crate::primitives::data_table::DataTable;
use crate::primitives::dialog::Dialog;
use crate::primitives::diff_view::DiffView;
use crate::primitives::editor::Editor;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::float::Float;
use crate::primitives::form::Form;
use crate::primitives::image::Image;
use crate::primitives::list::ListView;
use crate::primitives::menu_bar::MenuBar;
use crate::primitives::message_list::MessageList;
use crate::primitives::minimap::Minimap;
use crate::primitives::multi_section_view::MultiSectionView;
use crate::primitives::palette::Palette;
use crate::primitives::pipeline_view::PipelineView;
use crate::primitives::progress::ProgressBar;
use crate::primitives::radio_group::RadioGroup;
use crate::primitives::rich_text_popup::RichTextPopup;
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::sidebar_panel::SidebarPanel;
use crate::primitives::spinner::Spinner;
use crate::primitives::split::Split;
use crate::primitives::split_tree::SplitTree;
use crate::primitives::status_bar::StatusBar;
use crate::primitives::switch::Switch;
use crate::primitives::tab_bar::TabBar;
use crate::primitives::terminal::Terminal;
use crate::primitives::text_display::TextDisplay;
use crate::primitives::text_field::TextField;
use crate::primitives::text_input::TextInput;
use crate::primitives::toast::ToastOverlay;
use crate::primitives::toolbar::Toolbar;
use crate::primitives::tooltip::Tooltip;
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
pub enum Surface {
    Editor {
        rect: Rect,
        editor: Editor,
    },
    MultiSectionView {
        rect: Rect,
        view: MultiSectionView,
    },
    Tree {
        rect: Rect,
        tree: TreeView,
    },
    List {
        rect: Rect,
        list: ListView,
    },
    Split {
        rect: Rect,
        split: Split,
    },
    Scrollbar {
        rect: Rect,
        sb: Scrollbar,
    },
    FindReplace {
        rect: Rect,
        panel: FindReplacePanel,
    },
    DataTable {
        rect: Rect,
        table: DataTable,
        hovered: Option<usize>,
    },
    /// Any type-erased [`DynWidget`] — paints through
    /// [`crate::backend::DrawsPrimitives::draw_dyn`] rather than a
    /// bespoke `Backend::draw_*` method, on every backend including a
    /// zero-feature build. [`crate::primitives::cell_grid::CellGrid`]
    /// paints this way (it has no bespoke `Backend::draw_cell_grid`
    /// method at all), and any future widget built against
    /// [`crate::interactive::Interactive`] gets this variant for free via
    /// [`DynWidget`]'s blanket impl, with no new `Surface` variant of its
    /// own.
    Widget {
        rect: Rect,
        widget: Box<dyn DynWidget>,
    },
    Chart {
        rect: Rect,
        chart: Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    },
    /// App-defined escape-hatch drawing: a declarative list of
    /// [`crate::primitives::canvas::DrawOp`]s painted in order. See
    /// [`Canvas`]'s module doc for the coordinate frame and the
    /// per-backend degrade table.
    Canvas {
        rect: Rect,
        canvas: Canvas,
    },
    Board {
        rect: Rect,
        model: BoardModel,
    },
    DiffView {
        rect: Rect,
        view: DiffView,
    },
    Image {
        rect: Rect,
        image: Image,
    },
    MessageList {
        rect: Rect,
        list: MessageList,
    },
    Minimap {
        rect: Rect,
        minimap: Minimap,
    },
    PipelineView {
        rect: Rect,
        view: PipelineView,
    },
    Progress {
        rect: Rect,
        bar: ProgressBar,
    },
    Spinner {
        rect: Rect,
        spinner: Spinner,
    },
    /// Structural chrome, like [`Surface::Split`] — a divider set with
    /// no independent focusable identity, so excluded from
    /// [`ScreenLayout::tab_stops`].
    SplitTree {
        rect: Rect,
        tree: SplitTree,
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
#[deprecated(
    since = "0.12.0",
    note = "use `crate::frame_tree::FrameTree` (via `ScreenLayout::into_frame_tree`/`AppLogic::frame_tree`) for click/hit-test routing instead"
)]
pub enum FrameZone {
    Editor {
        idx: usize,
    },
    TabBar {
        idx: usize,
    },
    MultiSectionView {
        idx: usize,
    },
    Tree {
        idx: usize,
    },
    List {
        idx: usize,
    },
    Split {
        idx: usize,
    },
    Scrollbar {
        idx: usize,
    },
    FindReplace {
        idx: usize,
    },
    DataTable {
        idx: usize,
    },
    /// The zone for a [`Surface::Widget`] — carries the widget's own id
    /// (from [`DynWidget::id`]), since there is no per-variant field name
    /// (`grid.id`, `bar.id`, ...) for a caller to read it off `Surface`
    /// itself the way every other variant's id-bearing field still
    /// allows.
    Widget {
        idx: usize,
        id: Option<WidgetId>,
    },
    Chart {
        idx: usize,
    },
    Canvas {
        idx: usize,
    },
    Board {
        idx: usize,
    },
    DiffView {
        idx: usize,
    },
    Image {
        idx: usize,
    },
    MessageList {
        idx: usize,
    },
    Minimap {
        idx: usize,
    },
    PipelineView {
        idx: usize,
    },
    Progress {
        idx: usize,
    },
    Spinner {
        idx: usize,
    },
    SplitTree {
        idx: usize,
    },
    Empty,
}

/// Hit regions collected during [`ScreenLayout::draw`]. Resolves
/// absolute coordinates to the highest-z surface that contains them.
///
/// Every `(Rect, FrameZone)` pair is stored by value, so a
/// `FrameHitMap` carries no lifetime and can outlive the frame it was
/// built from. `Rect` and `FrameZone` both implement
/// `serde::{Serialize, Deserialize}`, so this struct derives them too
/// — it can be persisted or sent across a process/wire boundary.
/// **This is a step toward an owned `Frame`, not the owned frame
/// itself**: a `FrameZone` carries no primitive content, role, label
/// or value, only a bare tag and a frame-local index — see the module
/// doc's "`FrameHitMap` is serializable" section for what that means
/// for a receiver on the other end of a wire, and for what is still
/// missing.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[deprecated(
    since = "0.12.0",
    note = "use `crate::frame_tree::FrameTree` (via `ScreenLayout::into_frame_tree`/`AppLogic::frame_tree`) for click/hit-test routing instead"
)]
#[allow(deprecated)]
pub struct FrameHitMap {
    zones: Vec<(Rect, FrameZone)>,
}

#[allow(deprecated)]
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
pub struct ScreenLayout {
    surfaces: Vec<Surface>,
    tree_store: DescriptorStore<TreeView>,
    tree_handles: HashMap<WidgetId, Handle<TreeView>>,
}

impl ScreenLayout {
    pub fn new() -> Self {
        Self {
            surfaces: Vec::new(),
            tree_store: DescriptorStore::new(),
            tree_handles: HashMap::new(),
        }
    }

    pub fn push(&mut self, surface: Surface) {
        self.surfaces.push(surface);
    }

    /// The pushed surfaces, in z-order.
    pub fn surfaces(&self) -> &[Surface] {
        &self.surfaces
    }

    /// The pushed surfaces, mutably — for a caller that keeps a layout
    /// across frames and edits a surface in place instead of rebuilding
    /// the whole screen. Surfaces can be replaced, not added or
    /// removed, so each surface's index (and therefore the `idx` its
    /// [`FrameZone`] carries) is stable.
    pub fn surfaces_mut(&mut self) -> &mut [Surface] {
        &mut self.surfaces
    }

    /// Geometry for every pushed [`Surface::Tree`] as `(Handle, Rect)`
    /// pairs instead of cloning each tree's rows into the returned
    /// `Vec`.
    ///
    /// Each tree is kept in an internal [`DescriptorStore`] keyed by its
    /// [`WidgetId`]. A tree whose id was already seen — e.g. the same
    /// logical tree replaced in place via [`Self::surfaces_mut`] on a
    /// *retained* `ScreenLayout` — reuses its existing [`Handle`] and
    /// store slot rather than being removed and re-inserted. An id that
    /// stops being pushed (the surface was replaced with a different
    /// id, or dropped) has its handle and store slot evicted by this
    /// same call, so a retained layout's store stays bounded by the
    /// number of tree ids pushed *this* frame, not by how many have
    /// ever been pushed.
    ///
    /// This only pays off for a `ScreenLayout` an app keeps across
    /// frames and mutates via [`Self::surfaces_mut`]; a fresh
    /// `ScreenLayout::new()` built every frame (every in-repo example
    /// and the `templates/app` scaffold do this today) still inserts —
    /// and clones — every tree from an empty store each call. And even
    /// on a retained layout, this still compares the *whole* tree
    /// (`PartialEq` over every row) to decide whether to skip the
    /// clone, so a changed tree costs a full compare-then-clone rather
    /// than the zero-diff in-place [`DescriptorStore::update`] the
    /// store is designed around — a caller that wants that property
    /// has to mutate through the returned [`Handle`] itself instead of
    /// calling this on a freshly diffed `Surface::Tree`.
    ///
    /// Pushing two [`Surface::Tree`] entries with the same
    /// [`WidgetId`] is not rejected, but only the last one survives in
    /// the store: both resolve to the same [`Handle`], so the earlier
    /// tree's content is overwritten by the later one before either is
    /// returned.
    ///
    /// This leaves [`Self::draw`], [`Self::hit_map`] and
    /// [`Self::tab_stops`] untouched; they still paint and hit-test
    /// directly off [`Self::surfaces`]. No in-crate caller consumes
    /// this yet — `draw`/`hit_map`/`tab_stops`/[`OwnedFrame`] still
    /// read [`Self::surfaces`] directly — so treat this as an early,
    /// additive building block rather than a finished migration.
    pub fn tree_frame(&mut self) -> Vec<(Handle<TreeView>, Rect)> {
        let mut frame = Vec::new();
        let mut seen = HashSet::new();
        for surface in &self.surfaces {
            if let Surface::Tree { rect, tree } = surface {
                seen.insert(tree.id);
                let handle = match self.tree_handles.get(&tree.id) {
                    Some(&handle) => {
                        if self.tree_store.get(handle) != Some(tree) {
                            self.tree_store.update(handle, |slot| *slot = tree.clone());
                        }
                        handle
                    }
                    None => {
                        let handle = self.tree_store.insert(tree.clone());
                        self.tree_handles.insert(tree.id, handle);
                        handle
                    }
                };
                frame.push((handle, *rect));
            }
        }

        // Evict any id that was tracked before but wasn't pushed this
        // call, so a replaced or removed `Surface::Tree` doesn't keep
        // its old slot (and clone) alive forever.
        let stale: Vec<WidgetId> = self
            .tree_handles
            .keys()
            .filter(|id| !seen.contains(*id))
            .copied()
            .collect();
        for id in stale {
            if let Some(handle) = self.tree_handles.remove(&id) {
                self.tree_store.remove(handle);
            }
        }

        frame
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
    ///
    /// After painting, records an [`OwnedFrame`] of `self` in `backend`'s
    /// [`FrameCapture`] slot when that slot exists and is enabled — see
    /// the module doc's "Live-frame capture for accessibility" section.
    ///
    /// The returned [`FrameHitMap`] is deprecated; route [`Surface::Widget`]
    /// clicks through [`Self::into_frame_tree`] instead and ignore this
    /// return value for every other surface.
    #[allow(deprecated)]
    pub fn draw(&self, backend: &mut dyn Backend) -> FrameHitMap {
        let mut hit_map = FrameHitMap::new();

        for (idx, surface) in self.surfaces.iter().enumerate() {
            match surface {
                Surface::Editor { rect, editor } => {
                    backend.draw_editor(*rect, editor);
                }
                Surface::MultiSectionView { rect, view } => {
                    backend.draw_multi_section_view(*rect, view);
                }
                #[allow(deprecated)]
                Surface::Tree { rect, tree } => {
                    backend.draw_tree(*rect, tree);
                }
                #[allow(deprecated)]
                Surface::List { rect, list } => {
                    backend.draw_list(*rect, list);
                }
                Surface::Split { rect, split } => {
                    backend.draw_split(*rect, split);
                }
                Surface::Scrollbar { rect, sb } => {
                    backend.draw_scrollbar(*rect, sb);
                }
                Surface::FindReplace { rect, panel } => {
                    backend.draw_find_replace(*rect, panel);
                }
                #[allow(deprecated)]
                Surface::DataTable {
                    rect,
                    table,
                    hovered,
                } => {
                    backend.draw_data_table(*rect, table, *hovered);
                }
                Surface::Widget { rect, widget } => {
                    // A widget the platform presents itself (a native menu
                    // bar) is not painted in the window.
                    if !widget.native_presentation(backend, *rect) {
                        backend.draw_dyn(*rect, widget.as_ref(), &());
                    }
                }
                Surface::Chart {
                    rect,
                    chart,
                    hovered_point,
                    crosshair_x,
                } => {
                    backend.draw_chart(*rect, chart, *hovered_point, *crosshair_x);
                }
                Surface::Canvas { rect, canvas } => {
                    backend.draw_canvas(*rect, canvas);
                }
                Surface::Board { rect, model } => {
                    backend.draw_board(*rect, model);
                }
                Surface::DiffView { rect, view } => {
                    backend.draw_diff_view(*rect, view);
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
                Surface::Spinner { rect, spinner } => {
                    backend.draw_spinner(*rect, spinner);
                }
                Surface::SplitTree { rect, tree } => {
                    backend.draw_split_tree(*rect, tree);
                }
            }

            let (rect, zone) = Self::zone_for(idx, surface);
            hit_map.push(rect, zone);
        }

        record_captured_frame(backend, || OwnedFrame::from_layout(self));

        hit_map
    }

    /// Return the [`FrameHitMap`] that [`Self::draw`] would produce,
    /// without invoking any `backend.draw_*()` call. For apps that
    /// already painted these same surfaces elsewhere in the frame and
    /// only need the hit-test result — pushing surfaces here re-invokes
    /// no rendering, so it's safe to build this after (or interleaved
    /// with) the app's real paint sequencing.
    #[deprecated(
        since = "0.12.0",
        note = "use `Self::into_frame_tree` for `Surface::Widget` click/hit-test routing instead"
    )]
    #[allow(deprecated)]
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
    /// shared source of truth [`crate::focus::FocusTree::sync_tab_order`]
    /// cycles through. An app that builds its screen via
    /// `ScreenLayout` hands this straight to
    /// [`crate::runner::AppLogic::extra_tab_stops`]; see that method's doc for
    /// the opt-in contract.
    ///
    /// Only surfaces that carry both a plain `rect` and an owning
    /// [`WidgetId`] participate. Excluded, deliberately:
    /// - Transient overlays (`FindReplace`) — these already own their
    ///   own internal focus/dismiss handling while open, and being
    ///   modal, nothing behind them should be Tab-reachable anyway.
    /// - Structural chrome (`Split`, `Scrollbar`, `SplitTree`) —
    ///   containers/controls with no independent focusable identity of
    ///   their own.
    /// - `Image`, `Spinner` and `Canvas` — purely decorative, never
    ///   focusable, regardless of the "carries a `rect` plus an owning
    ///   `WidgetId`" rule: a consumer that adopts them would otherwise
    ///   get dead stops in its Tab order. `Canvas`'s own hit-test
    ///   ([`crate::primitives::canvas::CanvasHit`]) only ever answers
    ///   "inside"/"outside" of its bounds — it has no activation
    ///   affordance to reach by keyboard.
    /// - `Progress` **unless** `ProgressBar::cancellable` is set — a
    ///   bare progress readout has nothing to activate, but a
    ///   cancellable one has a real affordance ([`ProgressBarHit::Cancel`])
    ///   worth reaching by keyboard. `Minimap` stays included even
    ///   though it is also mostly visual, because it is click-to-scroll
    ///   and so has a real interaction to land on.
    ///
    /// `Widget` doesn't fit the per-variant allow/exclude list above — it
    /// consults [`DynWidget::focus`] directly instead; see
    /// [`Self::widget_tab_stops`].
    pub fn tab_stops(&self) -> Vec<(WidgetId, Rect)> {
        let mut stops: Vec<(WidgetId, Rect)> = self
            .surfaces
            .iter()
            .flat_map(|s| match s {
                Surface::Editor { rect, editor } => vec![(editor.id, *rect)],
                Surface::MultiSectionView { rect, view } => vec![(view.id, *rect)],
                Surface::Tree { rect, tree } => vec![(tree.id, *rect)],
                Surface::List { rect, list } => vec![(list.id, *rect)],
                Surface::DataTable { rect, table, .. } => vec![(table.id, *rect)],
                Surface::Widget { rect, widget } => Self::widget_tab_stops(*rect, widget.as_ref()),
                Surface::Chart { rect, chart, .. } => vec![(chart.id, *rect)],
                Surface::Board { rect, model } => vec![(model.id, *rect)],
                Surface::DiffView { rect, view } => vec![(view.id, *rect)],
                Surface::MessageList { rect, list } => vec![(list.id, *rect)],
                Surface::Minimap { rect, minimap } => vec![(minimap.id, *rect)],
                Surface::PipelineView { rect, view } => vec![(view.id, *rect)],
                Surface::Progress { rect, bar } if bar.cancellable => vec![(bar.id, *rect)],
                Surface::Image { .. }
                | Surface::Spinner { .. }
                | Surface::Canvas { .. }
                | Surface::Progress { .. }
                | Surface::Split { .. }
                | Surface::Scrollbar { .. }
                | Surface::FindReplace { .. }
                | Surface::SplitTree { .. } => Vec::new(),
            })
            .collect();
        stops.sort_by(|(_, a), (_, b)| {
            a.y.partial_cmp(&b.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
        });
        stops
    }

    /// [`Self::tab_stops`]'s [`Surface::Widget`] arm: every other
    /// `Surface` variant above contributes at most one stop, hard-coded
    /// per variant, but a [`Surface::Widget`] consults [`DynWidget::focus`]
    /// instead — `FocusSpec::None` contributes none, and both
    /// `FocusSpec::Stop` and `FocusSpec::Children` contribute exactly one
    /// (the whole widget's `rect`, addressed by the widget's own id).
    /// `(WidgetId, Rect)`'s shape has no field for a per-child sub-target,
    /// so `FocusSpec::Children` cannot yet contribute one stop per declared
    /// child without emitting indistinguishable duplicate entries that trap
    /// this flat stepper on the first child forever; collapsing to a single
    /// stop keeps Tab/Shift+Tab able to leave the widget. A real per-child
    /// stop set is [`crate::focus::FocusTree`]'s job instead.
    fn widget_tab_stops(rect: Rect, widget: &dyn DynWidget) -> Vec<(WidgetId, Rect)> {
        let Some(&id) = widget.id() else {
            return Vec::new();
        };
        match widget.focus() {
            crate::interactive::FocusSpec::None => Vec::new(),
            crate::interactive::FocusSpec::Stop | crate::interactive::FocusSpec::Children(_) => {
                vec![(id, rect)]
            }
        }
    }

    /// Compute the hit-test rect/zone for a single surface at index `idx`.
    /// Shared by [`Self::draw`] and [`Self::hit_map`] so the two stay in
    /// lock-step by construction.
    #[allow(deprecated)]
    fn zone_for(idx: usize, surface: &Surface) -> (Rect, FrameZone) {
        match surface {
            Surface::Editor { rect, .. } => (*rect, FrameZone::Editor { idx }),
            Surface::MultiSectionView { rect, .. } => (*rect, FrameZone::MultiSectionView { idx }),
            Surface::Tree { rect, .. } => (*rect, FrameZone::Tree { idx }),
            Surface::List { rect, .. } => (*rect, FrameZone::List { idx }),
            Surface::Split { rect, .. } => (*rect, FrameZone::Split { idx }),
            Surface::Scrollbar { rect, .. } => (*rect, FrameZone::Scrollbar { idx }),
            Surface::FindReplace { rect, .. } => (*rect, FrameZone::FindReplace { idx }),
            Surface::DataTable { rect, .. } => (*rect, FrameZone::DataTable { idx }),
            Surface::Widget { rect, widget } => (
                *rect,
                FrameZone::Widget {
                    idx,
                    id: widget.id().copied(),
                },
            ),
            Surface::Chart { rect, .. } => (*rect, FrameZone::Chart { idx }),
            Surface::Canvas { rect, .. } => (*rect, FrameZone::Canvas { idx }),
            Surface::Board { rect, .. } => (*rect, FrameZone::Board { idx }),
            Surface::DiffView { rect, .. } => (*rect, FrameZone::DiffView { idx }),
            Surface::Image { rect, .. } => (*rect, FrameZone::Image { idx }),
            Surface::MessageList { rect, .. } => (*rect, FrameZone::MessageList { idx }),
            Surface::Minimap { rect, .. } => (*rect, FrameZone::Minimap { idx }),
            Surface::PipelineView { rect, .. } => (*rect, FrameZone::PipelineView { idx }),
            Surface::Progress { rect, .. } => (*rect, FrameZone::Progress { idx }),
            Surface::Spinner { rect, .. } => (*rect, FrameZone::Spinner { idx }),
            Surface::SplitTree { rect, .. } => (*rect, FrameZone::SplitTree { idx }),
        }
    }

    /// Drain this layout's [`Surface::Widget`] entries into a
    /// [`crate::frame_tree::FrameTree`] for
    /// [`crate::runner::AppLogic::frame_tree`], discarding every other
    /// surface — the widget-contract routing engine only ever addresses
    /// the [`crate::interactive::Interactive`] contract, not
    /// `ScreenLayout`'s hand-written primitives.
    ///
    /// Consumes `self`, unlike [`Self::tab_stops`]/[`Self::hit_map`]:
    /// routing an event to a widget means calling
    /// [`crate::primitive::DynWidget::handle`] on the widget *value*
    /// itself (`Surface::Widget`'s `widget: Box<dyn DynWidget>` field),
    /// not just its id and rect, so there is no owned/borrowed split to
    /// preserve — see [`crate::frame_tree`]'s module doc for why a
    /// [`crate::frame_tree::FrameTree`] is built fresh per routed event
    /// rather than cached across a frame the way [`FrameHitMap`] is.
    ///
    /// A widget missing [`crate::primitive::DynWidget::id`] (no id was
    /// ever set on it) is skipped — there is no [`WidgetId`] to route by.
    pub fn into_frame_tree(self) -> crate::frame_tree::FrameTree {
        let mut tree = crate::frame_tree::FrameTree::new();
        for surface in self.surfaces {
            if let Surface::Widget { rect, widget } = surface {
                if let Some(&id) = widget.id() {
                    tree.push(id, rect, widget);
                }
            }
        }
        tree
    }
}

impl Default for ScreenLayout {
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
/// [`Self::FindReplace`] is the fallback for the [`Surface`] variant
/// whose primitive does not (yet) implement
/// `serde::{Serialize, Deserialize}` ([`FindReplacePanel`]) — the node
/// still gets a rect and its own distinct
/// [`FrameZone`] tag via [`OwnedFrame::hit_map`], but carries no content
/// to redraw or inspect. [`Self::Opaque`] is the generic fallback for any
/// future primitive that likewise can't be captured yet.
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum OwnedContent {
    Editor(Editor),
    /// Captured from a [`TabBar`] reached through [`Surface::Widget`] via
    /// [`DynWidget::capture`] — maps to [`FrameZone::Widget`], the same
    /// [`FrameZone`] the live `ScreenLayout::zone_for`'s own
    /// `Surface::Widget` arm reports, so hit-testing a captured frame
    /// agrees with hit-testing the live one. `Surface::Widget` is the
    /// only way a `TabBar` reaches a frame, so this is the only variant
    /// a captured tab bar ever takes.
    TabBar(TabBar),
    StatusBar(StatusBar),
    ActivityBar(ActivityBar),
    CommandLine(CommandLine),
    /// Captured from a [`Terminal`] reached through [`Surface::Widget`] via
    /// [`DynWidget::capture`] — maps to [`FrameZone::Widget`], the only
    /// way a `Terminal` reaches a frame.
    Terminal(Terminal),
    TextDisplay(TextDisplay),
    MultiSectionView(MultiSectionView),
    Tree(TreeView),
    /// Captured from the legacy [`Surface::List`] — maps to
    /// [`FrameZone::List`], distinct from [`Self::ListWidget`] below.
    /// `Surface::List` has no [`DynWidget::capture`] override to reuse
    /// (it is built directly by [`capture_surface`], not through
    /// [`Surface::Widget`]), so it keeps its own variant rather than
    /// colliding with the one [`crate::primitives::list::ListView`]'s
    /// own `Primitive::capture` produces.
    List(ListView),
    /// Captured from a [`ListView`] reached through
    /// [`Surface::Widget`] via [`DynWidget::capture`] — maps to
    /// [`FrameZone::Widget`], the same [`FrameZone`] the live
    /// `ScreenLayout::zone_for`'s own `Surface::Widget` arm reports, so
    /// hit-testing a captured frame agrees with hit-testing the live
    /// one. See [`Self::List`]'s own doc for why this needs a distinct
    /// variant rather than reusing it.
    ListWidget(ListView),
    Form(Form),
    /// Captured from a [`MenuBar`] reached through [`Surface::Widget`] via
    /// [`DynWidget::capture`] — maps to [`FrameZone::Widget`].
    MenuBar(MenuBar),
    Split(Split),
    Scrollbar(Scrollbar),
    Palette(Palette),
    Tooltip(Tooltip),
    /// Captured from a [`ContextMenu`] reached through [`Surface::Widget`]
    /// via [`DynWidget::capture`] — maps to [`FrameZone::Widget`]. Like
    /// every transient overlay, it is not replayed.
    ContextMenu(ContextMenu),
    Dialog(Dialog),
    Float(Float),
    Completions(Completions),
    RichTextPopup(RichTextPopup),
    Toast(ToastOverlay),
    DataTable(DataTable),
    CellGrid(CellGrid),
    Chart(Chart),
    Canvas(Canvas),
    Board(BoardModel),
    /// Captured from a [`CommandCenter`] reached through
    /// [`Surface::Widget`] via [`DynWidget::capture`] — maps to
    /// [`FrameZone::Widget`], the only way a `CommandCenter` reaches a
    /// frame.
    CommandCenter(CommandCenter),
    DiffView(DiffView),
    Image(Image),
    MessageList(MessageList),
    Minimap(Minimap),
    PipelineView(PipelineView),
    Progress(ProgressBar),
    /// Captured from a [`SidebarPanel`] reached through [`Surface::Widget`]
    /// via [`DynWidget::capture`] — maps to [`FrameZone::Widget`].
    SidebarPanel(SidebarPanel),
    Spinner(Spinner),
    SplitTree(SplitTree),
    TextInput(TextInput),
    TextField(TextField),
    /// Captured from a [`Toolbar`] reached through [`Surface::Widget`] via
    /// [`DynWidget::capture`] — maps to [`FrameZone::Widget`].
    Toolbar(Toolbar),
    /// Captured from a [`Switch`] reached through [`Surface::Widget`] via
    /// [`DynWidget::capture`] — maps to [`FrameZone::Widget`], the only
    /// way a standalone `Switch` ever reaches a frame.
    Switch(Switch),
    /// Captured from a [`RadioGroup`] reached through [`Surface::Widget`]
    /// via [`DynWidget::capture`] — maps to [`FrameZone::Widget`], the
    /// only way a standalone `RadioGroup` ever reaches a frame.
    RadioGroup(RadioGroup),
    /// Captured from a [`Button`] reached through [`Surface::Widget`] via
    /// [`DynWidget::capture`] — maps to [`FrameZone::Widget`], the only
    /// way a standalone `Button` ever reaches a frame.
    Button(Button),
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
    /// [`Self::ContextMenu`] — the one overlay variant that still needs
    /// a non-serializable `*Layout` companion to redraw — plus
    /// [`Self::FindReplace`] and [`Self::Opaque`] are no-ops: their
    /// hit-test zone still round-trips, but repainting them needs either
    /// a layout or primitive content this owned snapshot does not keep.
    fn draw(&self, rect: Rect, backend: &mut dyn Backend) {
        match self {
            OwnedContent::Editor(editor) => {
                backend.draw_editor(rect, editor);
            }
            OwnedContent::TabBar(bar) => {
                // `impl Primitive for TabBar` is unconditional, like
                // `CellGrid`'s, so this arm needs no backend-feature
                // guard of the kind `OwnedContent::ListWidget` below
                // still carries.
                backend.draw_primitive(rect, bar);
            }
            OwnedContent::StatusBar(bar) => {
                backend.draw_primitive(rect, bar);
            }
            OwnedContent::ActivityBar(bar) => {
                backend.draw_primitive(rect, bar);
            }
            OwnedContent::CommandLine(cmd) => {
                backend.draw_primitive(rect, cmd);
            }
            OwnedContent::Terminal(term) => {
                backend.draw_primitive(rect, term);
            }
            OwnedContent::TextDisplay(td) => {
                backend.draw_primitive(rect, td);
            }
            OwnedContent::MultiSectionView(view) => backend.draw_multi_section_view(rect, view),
            #[allow(deprecated)]
            OwnedContent::Tree(tree) => backend.draw_tree(rect, tree),
            #[allow(deprecated)]
            OwnedContent::List(list) => backend.draw_list(rect, list),
            OwnedContent::ListWidget(list) => {
                // `ListView: Primitive` is gated the same way
                // `impl Primitive for ListView` itself is (see that
                // impl's own `cfg`) — unlike `CellGrid`, whose
                // unconditional `Primitive` impl needs no such guard.
                // On a build with none of those backend features this
                // content never gets produced in the first place
                // (`ListView::capture`, gated identically, is the only
                // producer), so this is dead code rather than a real
                // no-op fallback; it exists purely so the match stays
                // exhaustive and `OwnedContent` need not cfg-gate the
                // variant itself (every other variant here is
                // unconditional too, including across the plugin/JSON
                // wire boundary this module's doc describes).
                #[cfg(any(
                    feature = "gtk",
                    feature = "tui",
                    feature = "win",
                    all(feature = "macos", target_os = "macos")
                ))]
                {
                    backend.draw_primitive(rect, list);
                }
                #[cfg(not(any(
                    feature = "gtk",
                    feature = "tui",
                    feature = "win",
                    all(feature = "macos", target_os = "macos")
                )))]
                {
                    let _ = (rect, list, backend);
                }
            }
            OwnedContent::Form(form) => {
                backend.draw_primitive(rect, form);
            }
            OwnedContent::MenuBar(bar) => {
                let _ = bar.present(backend, rect);
            }
            OwnedContent::Split(split) => {
                backend.draw_split(rect, split);
            }
            OwnedContent::Scrollbar(sb) => backend.draw_scrollbar(rect, sb),
            OwnedContent::Palette(palette) => {
                backend.draw_primitive(rect, palette);
            }
            OwnedContent::Toast(stack) => {
                backend.draw_primitive(rect, stack);
            }
            #[allow(deprecated)]
            OwnedContent::DataTable(table) => {
                backend.draw_data_table(rect, table, None);
            }
            OwnedContent::CellGrid(grid) => {
                backend.draw_primitive(rect, grid);
            }
            OwnedContent::Chart(chart) => {
                backend.draw_chart(rect, chart, None, None);
            }
            OwnedContent::Canvas(canvas) => {
                backend.draw_canvas(rect, canvas);
            }
            OwnedContent::Board(model) => {
                backend.draw_board(rect, model);
            }
            OwnedContent::CommandCenter(cc) => {
                // `impl Primitive for CommandCenter` is unconditional, like
                // `TabBar`'s above, so this arm needs no backend-feature
                // guard.
                backend.draw_primitive(rect, cc);
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
                // `impl Primitive for SidebarPanel` is unconditional, like
                // `TabBar`'s above.
                backend.draw_primitive(rect, panel);
            }
            OwnedContent::Spinner(spinner) => {
                backend.draw_spinner(rect, spinner);
            }
            OwnedContent::SplitTree(tree) => {
                backend.draw_split_tree(rect, tree);
            }
            OwnedContent::TextInput(ti) => {
                backend.draw_primitive(rect, ti);
            }
            OwnedContent::TextField(tf) => {
                backend.draw_primitive(rect, tf);
            }
            OwnedContent::Toolbar(bar) => {
                backend.draw_primitive(rect, bar);
            }
            OwnedContent::Switch(switch) => {
                // `impl Primitive for Switch` is unconditional, like
                // `TabBar`'s above, so this arm needs no backend-feature
                // guard of the kind `OwnedContent::ListWidget` below
                // still carries.
                backend.draw_primitive(rect, switch);
            }
            OwnedContent::RadioGroup(group) => {
                // `impl Primitive for RadioGroup` is unconditional, like
                // `Switch`'s above, so this arm needs no backend-feature
                // guard of the kind `OwnedContent::ListWidget` below
                // still carries.
                backend.draw_primitive(rect, group);
            }
            OwnedContent::Button(button) => {
                // `impl Primitive for Button` is unconditional, like
                // `Switch`'s above, so this arm needs no backend-feature
                // guard of the kind `OwnedContent::ListWidget` below
                // still carries.
                backend.draw_primitive(rect, button);
            }
            OwnedContent::Tooltip(tooltip) => {
                backend.draw_primitive(rect, tooltip);
            }
            OwnedContent::Dialog(dialog) => {
                backend.draw_primitive(rect, dialog);
            }
            OwnedContent::Float(float) => {
                backend.draw_primitive(rect, float);
            }
            OwnedContent::Completions(completions) => {
                backend.draw_primitive(rect, completions);
            }
            OwnedContent::RichTextPopup(popup) => {
                backend.draw_primitive(rect, popup);
            }
            OwnedContent::ContextMenu(_) | OwnedContent::FindReplace | OwnedContent::Opaque => {}
        }
    }

    /// The [`FrameZone`] this content contributes at frame-local index
    /// `idx`, mirroring [`ScreenLayout::zone_for`]'s tag-per-variant
    /// mapping so a `ScreenLayout` and the [`OwnedFrame`] captured from
    /// it produce identical zones.
    #[allow(deprecated)]
    fn zone(&self, idx: usize) -> FrameZone {
        match self {
            OwnedContent::Editor(_) => FrameZone::Editor { idx },
            OwnedContent::TabBar(bar) => FrameZone::Widget {
                idx,
                id: Some(bar.id),
            },
            OwnedContent::StatusBar(bar) => FrameZone::Widget {
                idx,
                id: Some(bar.id),
            },
            OwnedContent::ActivityBar(bar) => FrameZone::Widget {
                idx,
                id: Some(bar.id),
            },
            OwnedContent::CommandLine(cmd) => FrameZone::Widget {
                idx,
                id: Some(cmd.id),
            },
            OwnedContent::Terminal(term) => FrameZone::Widget {
                idx,
                id: Some(term.id),
            },
            OwnedContent::TextDisplay(td) => FrameZone::Widget {
                idx,
                id: Some(td.id),
            },
            OwnedContent::MultiSectionView(_) => FrameZone::MultiSectionView { idx },
            OwnedContent::Tree(_) => FrameZone::Tree { idx },
            OwnedContent::List(_) => FrameZone::List { idx },
            OwnedContent::ListWidget(list) => FrameZone::Widget {
                idx,
                id: Some(list.id),
            },
            OwnedContent::Form(form) => FrameZone::Widget {
                idx,
                id: Some(form.id),
            },
            OwnedContent::MenuBar(bar) => FrameZone::Widget {
                idx,
                id: Some(bar.id),
            },
            OwnedContent::Split(_) => FrameZone::Split { idx },
            OwnedContent::Scrollbar(_) => FrameZone::Scrollbar { idx },
            OwnedContent::Palette(palette) => FrameZone::Widget {
                idx,
                id: Some(palette.id),
            },
            OwnedContent::Tooltip(tooltip) => FrameZone::Widget {
                idx,
                id: Some(tooltip.id),
            },
            OwnedContent::ContextMenu(menu) => FrameZone::Widget {
                idx,
                id: Some(menu.id),
            },
            OwnedContent::Dialog(dialog) => FrameZone::Widget {
                idx,
                id: Some(dialog.id),
            },
            OwnedContent::Float(float) => FrameZone::Widget {
                idx,
                id: Some(float.id),
            },
            OwnedContent::Completions(completions) => FrameZone::Widget {
                idx,
                id: Some(completions.id),
            },
            OwnedContent::RichTextPopup(popup) => FrameZone::Widget {
                idx,
                id: Some(popup.id),
            },
            OwnedContent::Toast(stack) => FrameZone::Widget {
                idx,
                id: Some(stack.id),
            },
            OwnedContent::DataTable(_) => FrameZone::DataTable { idx },
            OwnedContent::CellGrid(grid) => FrameZone::Widget {
                idx,
                id: Some(grid.id),
            },
            OwnedContent::Chart(_) => FrameZone::Chart { idx },
            OwnedContent::Canvas(_) => FrameZone::Canvas { idx },
            OwnedContent::Board(_) => FrameZone::Board { idx },
            OwnedContent::CommandCenter(cc) => FrameZone::Widget {
                idx,
                id: Some(cc.id),
            },
            OwnedContent::DiffView(_) => FrameZone::DiffView { idx },
            OwnedContent::Image(_) => FrameZone::Image { idx },
            OwnedContent::MessageList(_) => FrameZone::MessageList { idx },
            OwnedContent::Minimap(_) => FrameZone::Minimap { idx },
            OwnedContent::PipelineView(_) => FrameZone::PipelineView { idx },
            OwnedContent::Progress(_) => FrameZone::Progress { idx },
            OwnedContent::SidebarPanel(panel) => FrameZone::Widget {
                idx,
                id: Some(panel.id),
            },
            OwnedContent::Spinner(_) => FrameZone::Spinner { idx },
            OwnedContent::SplitTree(_) => FrameZone::SplitTree { idx },
            OwnedContent::TextInput(ti) => FrameZone::Widget {
                idx,
                id: Some(ti.id),
            },
            OwnedContent::TextField(tf) => FrameZone::Widget {
                idx,
                id: Some(tf.id),
            },
            OwnedContent::Toolbar(bar) => FrameZone::Widget {
                idx,
                id: Some(bar.id),
            },
            OwnedContent::Switch(switch) => FrameZone::Widget {
                idx,
                id: Some(switch.id),
            },
            OwnedContent::RadioGroup(group) => FrameZone::Widget {
                idx,
                id: Some(group.id),
            },
            OwnedContent::Button(button) => FrameZone::Widget {
                idx,
                id: Some(button.id),
            },
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
    /// the overlay variants that carry one).
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

/// A backend's opt-in slot for the frame it most recently painted — the
/// state behind [`Backend::frame_capture`] and [`Backend::last_frame`].
/// See the module doc's "Live-frame capture for accessibility" section.
///
/// A backend owns one of these as a field (`FrameCapture::default()` is
/// disabled and empty) and hands it out from [`Backend::frame_capture`];
/// [`ScreenLayout::draw`] and [`OwnedFrame::draw`] do the recording. The
/// disabled state keeps no frame at all, so capture that is never turned
/// on costs no memory and no clone.
#[derive(Debug, Clone, Default)]
pub struct FrameCapture {
    enabled: bool,
    last: Option<OwnedFrame>,
}

impl FrameCapture {
    /// Turn capture on or off. Turning it off also drops the retained
    /// frame: a consumer that opts out must not keep reading a stale
    /// screen through [`Self::last_frame`].
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.last = None;
        }
    }

    /// Whether [`Self::record`] should be fed. The paint path checks this
    /// *before* building an [`OwnedFrame`], so the disabled case costs a
    /// `bool`, never a clone of every surface drawn.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Retain `frame` as [`Self::last_frame`]'s answer, replacing any
    /// earlier one. A no-op while disabled, so a caller that skipped the
    /// [`Self::is_enabled`] check still cannot smuggle a frame in.
    pub fn record(&mut self, frame: OwnedFrame) {
        if self.enabled {
            self.last = Some(frame);
        }
    }

    /// The most recently recorded frame, or `None` when capture is
    /// disabled or nothing has painted since it was enabled.
    pub fn last_frame(&self) -> Option<&OwnedFrame> {
        self.last.as_ref()
    }
}

/// Turn live-frame capture on or off on `backend`, returning whether it
/// supports capture at all (i.e. whether [`Backend::frame_capture`] gave
/// a slot). The one-line form of
/// `backend.frame_capture().map(|c| c.set_enabled(enabled))`, for a
/// runner that just wants its backend to become agent-visible.
pub fn set_frame_capture(backend: &mut dyn Backend, enabled: bool) -> bool {
    match backend.frame_capture() {
        Some(capture) => {
            capture.set_enabled(enabled);
            true
        }
        None => false,
    }
}

/// Record the frame `build` produces into `backend`'s [`FrameCapture`],
/// but only when the backend has a slot and capture is enabled — `build`
/// is never called otherwise, which is what keeps a disabled capture from
/// cloning anything.
fn record_captured_frame(backend: &mut dyn Backend, build: impl FnOnce() -> OwnedFrame) {
    if let Some(capture) = backend.frame_capture() {
        if capture.is_enabled() {
            capture.record(build());
        }
    }
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

    /// Capture every surface in `layout` into a serializable
    /// [`OwnedFrame`]. Clones each primitive out of `layout`, so the
    /// result is independent of it and can be serialized, persisted, or
    /// sent across a process/wire boundary.
    pub fn from_layout(layout: &ScreenLayout) -> Self {
        let nodes = layout
            .surfaces
            .iter()
            .enumerate()
            .map(|(idx, surface)| {
                let (rect, widget_id, content) = capture_surface(surface);
                // A widget whose content cannot be captured still describes
                // itself to the accessibility tree through its own
                // `A11yPrimitive` impl.
                let a11y = match (surface, &content) {
                    (Surface::Widget { widget, .. }, OwnedContent::Opaque) => {
                        Some(widget.a11y_info())
                    }
                    _ => None,
                };
                OwnedNode {
                    idx,
                    rect,
                    widget_id,
                    a11y,
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

    /// The captured nodes, mutably — the seam a consumer uses to attach
    /// per-surface [`OwnedNode::a11y`] overrides to a frame it has
    /// already captured. The node set itself is fixed by the capture:
    /// nodes can be edited in place, not added or removed, so `idx`
    /// stays in step with the source [`ScreenLayout`] and the
    /// [`FrameZone`]s [`Self::draw`] registers.
    pub fn nodes_mut(&mut self) -> &mut [OwnedNode] {
        &mut self.nodes
    }

    /// Repaint every node through `backend` and return the resulting
    /// [`FrameHitMap`] — the owned-data counterpart of
    /// [`ScreenLayout::draw`]. See [`OwnedContent::draw`] for which
    /// variants are no-ops here (they still register a hit zone).
    ///
    /// Same opt-in capture as [`ScreenLayout::draw`]: a clone of `self`
    /// is recorded in `backend`'s [`FrameCapture`] when that slot exists
    /// and is enabled.
    #[allow(deprecated)]
    pub fn draw(&self, backend: &mut dyn Backend) -> FrameHitMap {
        let mut hit_map = FrameHitMap::new();
        for node in &self.nodes {
            node.content.draw(node.rect, backend);
            hit_map.push(node.rect, node.content.zone(node.idx));
        }
        record_captured_frame(backend, || self.clone());
        hit_map
    }

    /// Derive the [`FrameHitMap`] this frame would produce, without
    /// invoking any `backend.draw_*()` call — the owned-data counterpart
    /// of [`ScreenLayout::hit_map`].
    #[deprecated(
        since = "0.12.0",
        note = "no in-tree replacement reads an `OwnedFrame`'s hit zones; build a `crate::frame_tree::FrameTree` from the source `ScreenLayout` instead"
    )]
    #[allow(deprecated)]
    pub fn hit_map(&self) -> FrameHitMap {
        let mut hit_map = FrameHitMap::new();
        for node in &self.nodes {
            hit_map.push(node.rect, node.content.zone(node.idx));
        }
        hit_map
    }
}

/// Clone one [`Surface`]'s primitive into the `(rect, widget_id,
/// content)` parts an [`OwnedNode`] holds. The widget-id rule mirrors
/// [`ScreenLayout::tab_stops`]'s inclusion list exactly; the rect rule
/// mirrors [`ScreenLayout::zone_for`]'s exactly. [`Surface::FindReplace`]
/// falls back to [`OwnedContent::FindReplace`], because
/// [`FindReplacePanel`] does not implement `serde::{Serialize,
/// Deserialize}` — it still keeps its own distinct [`FrameZone`] tag.
fn capture_surface(surface: &Surface) -> (Rect, Option<WidgetId>, OwnedContent) {
    match surface {
        Surface::Editor { rect, editor } => {
            (*rect, Some(editor.id), OwnedContent::Editor(editor.clone()))
        }
        Surface::MultiSectionView { rect, view } => (
            *rect,
            Some(view.id),
            OwnedContent::MultiSectionView(view.clone()),
        ),
        Surface::Tree { rect, tree } => (*rect, Some(tree.id), OwnedContent::Tree(tree.clone())),
        Surface::List { rect, list } => (*rect, Some(list.id), OwnedContent::List(list.clone())),
        Surface::Split { rect, split } => (*rect, None, OwnedContent::Split(split.clone())),
        Surface::Scrollbar { rect, sb } => (*rect, None, OwnedContent::Scrollbar(sb.clone())),
        Surface::FindReplace { rect, .. } => (*rect, None, OwnedContent::FindReplace),
        Surface::DataTable { rect, table, .. } => (
            *rect,
            Some(table.id),
            OwnedContent::DataTable(table.clone()),
        ),
        Surface::Widget { rect, widget } => (*rect, widget.id().copied(), widget.capture()),
        Surface::Chart { rect, chart, .. } => {
            (*rect, Some(chart.id), OwnedContent::Chart(chart.clone()))
        }
        Surface::Canvas { rect, canvas } => (*rect, None, OwnedContent::Canvas(canvas.clone())),
        Surface::Board { rect, model } => {
            (*rect, Some(model.id), OwnedContent::Board(model.clone()))
        }
        Surface::DiffView { rect, view } => {
            (*rect, Some(view.id), OwnedContent::DiffView(view.clone()))
        }
        Surface::Image { rect, image } => (*rect, None, OwnedContent::Image(image.clone())),
        Surface::MessageList { rect, list } => (
            *rect,
            Some(list.id),
            OwnedContent::MessageList(list.clone()),
        ),
        Surface::Minimap { rect, minimap } => (
            *rect,
            Some(minimap.id),
            OwnedContent::Minimap(minimap.clone()),
        ),
        Surface::PipelineView { rect, view } => (
            *rect,
            Some(view.id),
            OwnedContent::PipelineView(view.clone()),
        ),
        Surface::Progress { rect, bar } => (
            *rect,
            if bar.cancellable { Some(bar.id) } else { None },
            OwnedContent::Progress(bar.clone()),
        ),
        Surface::Spinner { rect, spinner } => (*rect, None, OwnedContent::Spinner(spinner.clone())),
        Surface::SplitTree { rect, tree } => (*rect, None, OwnedContent::SplitTree(tree.clone())),
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
#[allow(deprecated)] // exercises `FrameHitMap`/`FrameZone` directly; see their own `#[deprecated]` notes.
mod tests {
    use super::*;
    use crate::primitives::tree::TreeRow;
    use crate::types::{SelectionMode, StyledText, TreeStyle};

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
            FrameZone::FindReplace { idx: 1 },
        );

        // Point in the overlay → highest-z wins.
        assert_eq!(map.hit_test(25.0, 25.0), FrameZone::FindReplace { idx: 1 });
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
            FrameZone::Scrollbar { idx: 2 },
        );

        assert_eq!(map.hit_test(40.0, 0.5), FrameZone::TabBar { idx: 1 });
        assert_eq!(map.hit_test(40.0, 12.0), FrameZone::Editor { idx: 0 });
        assert_eq!(map.hit_test(40.0, 23.5), FrameZone::Scrollbar { idx: 2 });
    }

    #[test]
    fn screen_layout_default_is_empty() {
        let layout = ScreenLayout::default();
        assert_eq!(layout.surfaces.len(), 0);
    }

    fn sample_tree(id: &str, scroll_offset: usize) -> TreeView {
        TreeView {
            id: WidgetId::new(id),
            rows: vec![TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("src/"),
                badge: None,
                is_expanded: Some(true),
                decoration: Default::default(),
                edit: None,
            }],
            selection_mode: SelectionMode::Single,
            selected_path: Some(vec![0]),
            scroll_offset,
            style: TreeStyle::default(),
            has_focus: false,
        }
    }

    #[test]
    fn tree_frame_carries_handles_with_matching_rects() {
        let mut layout = ScreenLayout::new();
        layout.push(Surface::Tree {
            rect: Rect::new(0.0, 0.0, 40.0, 10.0),
            tree: sample_tree("tree:explorer", 0),
        });

        let frame = layout.tree_frame();

        assert_eq!(frame.len(), 1);
        let (handle, rect) = frame[0];
        assert_eq!(rect, Rect::new(0.0, 0.0, 40.0, 10.0));
        assert_eq!(
            layout.tree_store.get(handle).map(|t| &t.id),
            Some(&WidgetId::new("tree:explorer"))
        );
    }

    #[test]
    fn tree_frame_reuses_the_same_handle_across_frames_without_rebuilding() {
        let mut layout = ScreenLayout::new();
        layout.push(Surface::Tree {
            rect: Rect::new(0.0, 0.0, 40.0, 10.0),
            tree: sample_tree("tree:explorer", 0),
        });

        let first = layout.tree_frame();
        let first_handle = first[0].0;
        assert_eq!(layout.tree_store.len(), 1);

        // Simulate a retained layout's next frame: the same logical tree,
        // unchanged, is still at the same surface index.
        let second = layout.tree_frame();
        assert_eq!(second[0].0, first_handle);
        assert_eq!(layout.tree_store.len(), 1);

        // Replace the surface in place (same `WidgetId`, different scroll
        // state) the way a retained `ScreenLayout` caller would via
        // `surfaces_mut`, and confirm the identity-keyed slot is updated
        // rather than inserted as a new entry.
        layout.surfaces_mut()[0] = Surface::Tree {
            rect: Rect::new(0.0, 0.0, 40.0, 10.0),
            tree: sample_tree("tree:explorer", 5),
        };
        let third = layout.tree_frame();
        assert_eq!(third[0].0, first_handle);
        assert_eq!(layout.tree_store.len(), 1);
        assert_eq!(
            layout.tree_store.get(third[0].0).map(|t| t.scroll_offset),
            Some(5)
        );

        // A genuinely new tree id grows the store by exactly one slot.
        layout.push(Surface::Tree {
            rect: Rect::new(0.0, 10.0, 40.0, 10.0),
            tree: sample_tree("tree:outline", 0),
        });
        let fourth = layout.tree_frame();
        assert_eq!(fourth.len(), 2);
        assert_eq!(layout.tree_store.len(), 2);
    }

    #[test]
    fn tree_frame_evicts_handles_for_ids_no_longer_pushed() {
        let mut layout = ScreenLayout::new();
        layout.push(Surface::Tree {
            rect: Rect::new(0.0, 0.0, 40.0, 10.0),
            tree: sample_tree("tree:explorer", 0),
        });

        let first = layout.tree_frame();
        let stale_handle = first[0].0;
        assert_eq!(layout.tree_store.len(), 1);

        // Swap the surface in place for a *different* tree id — the
        // pattern `surfaces_mut`'s doc calls out (a per-tab explorer, a
        // closed panel reused for a new one). Without eviction this
        // would leave the old id's slot occupied forever.
        layout.surfaces_mut()[0] = Surface::Tree {
            rect: Rect::new(0.0, 0.0, 40.0, 10.0),
            tree: sample_tree("tree:outline", 0),
        };
        let second = layout.tree_frame();

        assert_eq!(layout.tree_store.len(), 1);
        assert!(layout.tree_store.get(stale_handle).is_none());
        assert_eq!(
            layout.tree_store.get(second[0].0).map(|t| &t.id),
            Some(&WidgetId::new("tree:outline"))
        );

        // Dropping the surface entirely (no `Surface::Tree` pushed at
        // all) evicts the last tracked id too, instead of leaving an
        // orphaned slot with nothing in `surfaces` to ever revisit it.
        let mut empty_layout = ScreenLayout::new();
        empty_layout.push(Surface::Tree {
            rect: Rect::new(0.0, 0.0, 40.0, 10.0),
            tree: sample_tree("tree:explorer", 0),
        });
        empty_layout.tree_frame();
        assert_eq!(empty_layout.tree_store.len(), 1);
        empty_layout.surfaces_mut()[0] = Surface::Editor {
            rect: Rect::new(0.0, 0.0, 40.0, 10.0),
            editor: Editor {
                id: "ed".into(),
                rect: Rect::new(0.0, 0.0, 40.0, 10.0),
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
            },
        };
        empty_layout.tree_frame();
        assert_eq!(empty_layout.tree_store.len(), 0);
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
            editor,
        });
        layout.push(Surface::Widget {
            rect: Rect::new(0.0, 0.0, 50.0, 50.0),
            widget: Box::new(palette),
        });

        // No `&mut dyn Backend` in scope anywhere in this test — proves
        // `hit_map()` needs no backend to recover the same zones `draw()`
        // would have registered.
        let hit_map = layout.hit_map();

        assert_eq!(hit_map.len(), 2);
        // Point in the overlay → highest-z (last-pushed) wins.
        assert_eq!(
            hit_map.hit_test(25.0, 25.0),
            FrameZone::Widget {
                idx: 1,
                id: Some("pal".into())
            }
        );
        // Point outside the overlay but inside the editor.
        assert_eq!(hit_map.hit_test(75.0, 75.0), FrameZone::Editor { idx: 0 });
        // Point outside everything.
        assert_eq!(hit_map.hit_test(150.0, 150.0), FrameZone::Empty);
    }

    /// A [`Surface`] owns the primitive it paints, so the primitive's
    /// original binding can go out of scope while the `Surface` lives
    /// on and still describes a complete frame.
    #[test]
    fn surface_owns_its_primitive_and_outlives_the_source() {
        let rect = Rect::new(2.0, 3.0, 20.0, 8.0);

        let surface = {
            let list = MessageList {
                id: "ml".into(),
                rows: Vec::new(),
                scroll_top: 0,
            };
            Surface::MessageList { rect, list }
            // `list`'s binding dies here; the `Surface` keeps the value.
        };

        let mut layout = ScreenLayout::new();
        layout.push(surface);

        let hit_map = layout.hit_map();
        assert_eq!(hit_map.len(), 1);
        assert_eq!(hit_map.zones()[0].0, rect);
        assert_eq!(
            hit_map.hit_test(3.0, 4.0),
            FrameZone::MessageList { idx: 0 }
        );
        assert_eq!(hit_map.hit_test(100.0, 100.0), FrameZone::Empty);
    }

    /// A [`ScreenLayout`] carries no lifetime, so it is a plain return
    /// value: a helper can build a whole frame description and hand it
    /// back, and the caller can both draw it and hit-map it.
    #[test]
    fn screen_layout_returned_from_a_function_draws_and_hit_maps() {
        use crate::testing::RecordingBackend;

        fn build() -> ScreenLayout {
            let list = MessageList {
                id: "ml".into(),
                rows: Vec::new(),
                scroll_top: 0,
            };
            let mut layout = ScreenLayout::new();
            layout.push(Surface::MessageList {
                rect: Rect::new(0.0, 0.0, 40.0, 10.0),
                list,
            });
            layout
        }

        let layout = build();

        let mut backend = RecordingBackend::new();
        let drawn = layout.draw(&mut backend);
        assert!(backend.calls.contains(&"draw_message_list"));

        assert_eq!(drawn, layout.hit_map());
        assert_eq!(drawn.hit_test(1.0, 1.0), FrameZone::MessageList { idx: 0 });
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
            editor,
        });
        layout.push(Surface::Widget {
            rect: Rect::new(0.0, 0.0, 50.0, 5.0),
            widget: Box::new(palette),
        });
        // Structural chrome: must not appear in tab order at all.
        layout.push(Surface::Split {
            rect: Rect::new(0.0, 30.0, 100.0, 1.0),
            split,
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
            col_scroll_offset: 0,
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
            scroll_offset: 0,
            focused_pane: Default::default(),
            has_focus: false,
        };
        let overlay = crate::primitives::drop_zone::DropOverlay {
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
            scroll_top: 0,
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
            scroll_offset: 0,
            scroll_col: 0,
            has_focus: false,
        };
        let toolbar = Toolbar {
            id: "tb".into(),
            buttons: Vec::new(),
            bg: None,
            focused_index: None,
            ..Default::default()
        };

        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let mut layout = ScreenLayout::new();
        layout.push(Surface::Board { rect, model: board });
        layout.push(Surface::Widget {
            rect,
            widget: Box::new(cc),
        });
        layout.push(Surface::DiffView { rect, view: diff });
        layout.push(Surface::Widget {
            rect: overlay.bounds(),
            widget: Box::new(overlay.clone()),
        });
        layout.push(Surface::Image { rect, image });
        layout.push(Surface::MessageList {
            rect,
            list: message_list,
        });
        layout.push(Surface::Minimap { rect, minimap });
        layout.push(Surface::PipelineView {
            rect,
            view: pipeline,
        });
        layout.push(Surface::Progress {
            rect,
            bar: progress,
        });
        layout.push(Surface::Widget {
            rect,
            widget: Box::new(sidebar),
        });
        layout.push(Surface::Spinner { rect, spinner });
        layout.push(Surface::SplitTree {
            rect,
            tree: split_tree,
        });
        layout.push(Surface::Widget {
            rect,
            widget: Box::new(text_input),
        });
        layout.push(Surface::Widget {
            rect,
            widget: Box::new(toolbar),
        });

        let hit_map = layout.hit_map();
        assert_eq!(hit_map.len(), 14);

        let zones: Vec<&FrameZone> = hit_map.zones().iter().map(|(_, z)| z).collect();
        assert_eq!(zones[0], &FrameZone::Board { idx: 0 });
        assert_eq!(
            zones[1],
            &FrameZone::Widget {
                idx: 1,
                id: Some("cc".into())
            }
        );
        assert_eq!(zones[2], &FrameZone::DiffView { idx: 2 });
        assert_eq!(zones[3], &FrameZone::Widget { idx: 3, id: None });
        assert_eq!(zones[4], &FrameZone::Image { idx: 4 });
        assert_eq!(zones[5], &FrameZone::MessageList { idx: 5 });
        assert_eq!(zones[6], &FrameZone::Minimap { idx: 6 });
        assert_eq!(zones[7], &FrameZone::PipelineView { idx: 7 });
        assert_eq!(zones[8], &FrameZone::Progress { idx: 8 });
        assert_eq!(
            zones[9],
            &FrameZone::Widget {
                idx: 9,
                id: Some(WidgetId::new("sp")),
            }
        );
        assert_eq!(zones[10], &FrameZone::Spinner { idx: 10 });
        assert_eq!(zones[11], &FrameZone::SplitTree { idx: 11 });
        assert_eq!(
            zones[12],
            &FrameZone::Widget {
                idx: 12,
                id: Some(WidgetId::new("ti")),
            }
        );
        assert_eq!(
            zones[13],
            &FrameZone::Widget {
                idx: 13,
                id: Some(WidgetId::new("tb")),
            }
        );

        // A drop overlay is pushed at `DropOverlay::bounds()`.
        assert_eq!(hit_map.zones()[3].0, overlay.bounds());

        // Tab order: the non-interactive drop overlay and `SplitTree`
        // (structural chrome, like `Split`) don't participate, and
        // neither do the purely-decorative `Image`/`Spinner` or this
        // non-cancellable `Progress` (no activatable affordance).
        let stops = layout.tab_stops();
        let stop_ids: Vec<&str> = stops.iter().map(|(id, _)| id.as_str()).collect();
        for id in ["board", "cc", "diff", "ml", "mm", "pv", "ti"] {
            assert!(stop_ids.contains(&id), "{id} should be a tab stop");
        }
        // A `SidebarPanel` is pointer-only (`FocusSpec::None`), and a
        // toolbar with no button has nothing to focus.
        for id in ["img", "sn", "pb", "leaf", "sp", "tb"] {
            assert!(!stop_ids.contains(&id), "{id} should not be a tab stop");
        }
        assert_eq!(stop_ids.len(), 7);
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
            bar: progress,
        });
        let stops = layout.tab_stops();
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].0.as_str(), "pb");
    }

    /// A `Float` pushed as `Surface::Widget` paints through
    /// `DrawsPrimitives::draw_dyn`, gets a `FrameZone::Widget` hit region
    /// at its resolved bounds, and stays out of Tab order — the same
    /// transient-overlay treatment `Tooltip`/`ContextMenu`/`Dialog` get
    /// (see this module's `tab_stops` doc).
    #[test]
    fn a_float_widget_surface_paints_and_is_excluded_from_tab_stops() {
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
            editor,
        });
        layout.push(Surface::Widget {
            rect: float_layout.bounds,
            widget: Box::new(float),
        });

        let mut backend = RecordingBackend::new();
        let hit_map = layout.draw(&mut backend);
        assert_eq!(hit_map.zones()[1].0, float_layout.bounds);
        assert_eq!(
            hit_map.zones()[1].1,
            FrameZone::Widget {
                idx: 1,
                id: Some(WidgetId::new("f")),
            }
        );

        let stops = layout.tab_stops();
        assert_eq!(
            stops.len(),
            1,
            "the Float surface must not become a Tab stop — it owns its \
             own focus story via ModalStack::top_focusable"
        );
        assert_eq!(stops[0].0.as_str(), "ed");
    }

    /// A `TextField` pushed as `Surface::Widget` paints through
    /// `DrawsPrimitives::draw_dyn`, produces a `FrameZone::Widget` hit
    /// region at the field's own rect, and is a Tab stop — the same
    /// `Surface`/`FrameZone`/`zone_for`/`tab_stops` wiring every other
    /// focusable widget gets, so a `TextField` dropped into a
    /// `ScreenLayout` (a Finder-style search box above a list, say) is
    /// reachable the same way.
    #[test]
    fn a_text_field_widget_surface_paints_and_is_a_tab_stop() {
        use crate::testing::RecordingBackend;
        use crate::TextField;

        let rect = Rect::new(0.0, 0.0, 10.0, 1.0);
        let field = TextField::new(WidgetId::new("search")).with_value("hi");

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Widget {
            rect,
            widget: Box::new(field),
        });

        let mut backend = RecordingBackend::new();
        let hit_map = layout.draw(&mut backend);
        assert_eq!(hit_map.zones()[0].0, rect);
        assert_eq!(
            hit_map.zones()[0].1,
            FrameZone::Widget {
                idx: 0,
                id: Some(WidgetId::new("search")),
            }
        );

        let stops = layout.tab_stops();
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].0.as_str(), "search");
    }

    /// `CellGrid` has no bespoke `Backend::draw_cell_grid` method — it
    /// paints exclusively through `DrawsPrimitives::draw_dyn` via
    /// `Surface::Widget` (see that `Surface` variant's own doc) — so
    /// unlike every `assert!(backend.calls.contains(...))` check above,
    /// there is no named call to assert on here. What this still proves:
    /// `zone_for` wires a `draw_dyn`-only `Surface` variant exactly like
    /// every bespoke-method one, and `ScreenLayout::draw` never panics
    /// calling it through a backend (`RecordingBackend`) with no
    /// `PaintSurface`/cell-buffer of its own — the exact "resolve
    /// layout, maybe paint" degrade `DrawsPrimitives::draw_dyn`'s own
    /// doc promises.
    ///
    /// `tab_stops()` answers from the widget's own
    /// `DynWidget::focus()`, not from field presence: `CellGrid`
    /// declares `FocusSpec::Stop` while it has cells to put a cursor on,
    /// and `FocusSpec::None` for an empty sheet. Unlike every hard-coded
    /// `Surface` variant above (which contribute a tab stop
    /// unconditionally), the same `Surface::Widget` arm reports one stop
    /// for the populated sheet and none for the empty one, so both
    /// layouts below share one code path and differ only in what the
    /// widget itself declares.
    #[test]
    fn surface_widget_dispatches_through_draw_dyn_and_takes_its_tab_stop_from_focus() {
        use crate::primitives::cell_grid::{Cell, CellGrid};
        use crate::testing::RecordingBackend;

        let rect = Rect::new(0.0, 0.0, 20.0, 5.0);
        let grid = CellGrid {
            id: WidgetId::new("grid:sheet"),
            row_count: 4,
            col_count: 4,
            frozen_rows: 0,
            frozen_cols: 0,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 6.0,
            row_heights: Default::default(),
            col_widths: Default::default(),
            show_row_headers: false,
            show_col_headers: false,
            row_labels: Default::default(),
            col_labels: Default::default(),
            cursor: (0, 0),
            selection_anchor: None,
            cells: vec![vec![Cell::plain("hi")]],
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: true,
            show_scrollbar: false,
            editing: None,
            column_separators: false,
            striped: false,
        };

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Widget {
            rect,
            widget: Box::new(grid.clone()),
        });

        let mut backend = RecordingBackend::new();
        let hit_map = layout.draw(&mut backend);
        assert_eq!(hit_map.zones()[0].0, rect);
        assert_eq!(
            hit_map.zones()[0].1,
            FrameZone::Widget {
                idx: 0,
                id: Some(grid.id)
            }
        );

        assert_eq!(layout.tab_stops(), vec![(grid.id, rect)]);

        let empty_grid = CellGrid {
            row_count: 0,
            col_count: 0,
            cells: Vec::new(),
            ..grid.clone()
        };
        let mut empty_layout = ScreenLayout::new();
        empty_layout.push(Surface::Widget {
            rect,
            widget: Box::new(empty_grid),
        });
        assert_eq!(empty_layout.tab_stops(), Vec::new());
    }

    /// `into_frame_tree` keeps only `Surface::Widget` entries, dropping
    /// every hand-written primitive alongside it — the routing engine it
    /// feeds (`crate::runtime::preprocess_event`) only ever addresses the
    /// `Interactive` contract.
    #[test]
    fn into_frame_tree_keeps_only_widget_surfaces() {
        use crate::primitives::cell_grid::{Cell, CellGrid};
        use crate::types::WidgetId as Id;

        let widget_rect = Rect::new(0.0, 0.0, 20.0, 5.0);
        let grid = CellGrid {
            id: Id::new("grid:sheet"),
            row_count: 1,
            col_count: 1,
            frozen_rows: 0,
            frozen_cols: 0,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 6.0,
            row_heights: Default::default(),
            col_widths: Default::default(),
            show_row_headers: false,
            show_col_headers: false,
            row_labels: Default::default(),
            col_labels: Default::default(),
            cursor: (0, 0),
            selection_anchor: None,
            cells: vec![vec![Cell::plain("hi")]],
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: false,
            show_scrollbar: false,
            editing: None,
            column_separators: false,
            striped: false,
        };

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Progress {
            rect: Rect::new(0.0, 10.0, 20.0, 1.0),
            bar: ProgressBar {
                id: Id::new("status"),
                label: String::new(),
                value: None,
                frame_idx: 0,
                cancellable: false,
                accent: None,
            },
        });
        layout.push(Surface::Widget {
            rect: widget_rect,
            widget: Box::new(grid.clone()),
        });

        let tree = layout.into_frame_tree();
        assert_eq!(tree.nodes().len(), 1);
        let node = tree.find(&grid.id).expect("widget node survives");
        assert_eq!(node.rect, widget_rect);
    }

    /// The TUI side of the same wiring: `ScreenLayout::draw` through a
    /// real `TuiBackend` paints the grid's actual cell text, not just an
    /// empty reserved rect — proving `Surface::Widget` → `draw_dyn` →
    /// `DynWidget::paint` → `Primitive::paint_cells` →
    /// `crate::tui::draw_cell_grid` end to end.
    #[test]
    #[cfg(feature = "tui")]
    fn surface_widget_paints_real_cell_text_through_tui_backend() {
        use crate::primitives::cell_grid::{Cell, CellGrid};
        use crate::runner::{AppLogic, Reaction};
        use crate::tui::testing::TuiDriver;

        let grid = CellGrid {
            id: WidgetId::new("grid:sheet"),
            row_count: 4,
            col_count: 4,
            frozen_rows: 0,
            frozen_cols: 0,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 6.0,
            row_heights: Default::default(),
            col_widths: Default::default(),
            show_row_headers: false,
            show_col_headers: false,
            row_labels: Default::default(),
            col_labels: Default::default(),
            cursor: (0, 0),
            selection_anchor: None,
            cells: vec![vec![Cell::plain("ahoy")]],
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: true,
            show_scrollbar: false,
            editing: None,
            column_separators: false,
            striped: false,
        };

        struct ShowGrid(CellGrid);
        impl AppLogic for ShowGrid {
            type AreaId = ();
            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let mut layout = ScreenLayout::new();
                layout.push(Surface::Widget {
                    rect: Rect::new(0.0, 0.0, 20.0, 5.0),
                    widget: Box::new(self.0.clone()),
                });
                layout.draw(backend);
            }
            fn handle(&mut self, _event: crate::UiEvent, _backend: &mut dyn Backend) -> Reaction {
                Reaction::Continue
            }
        }

        let driver = TuiDriver::new(ShowGrid(grid), 20, 5);
        assert!(
            driver.screen_contains("ahoy"),
            "the grid's own cell text must appear on screen: {}",
            driver.screen()
        );
    }

    /// A tiny second [`Interactive`] widget, distinct from
    /// [`crate::primitives::cell_grid::CellGrid`] in every way that
    /// matters for proving [`Surface::Widget`] is generic rather than
    /// quietly special-cased for one type: its own [`Primitive::Layout`]
    /// shape, its own [`Primitive::Event`], and a real [`FocusSpec::Stop`]
    /// rather than [`crate::non_interactive!`]'s `FocusSpec::None`.
    #[derive(Debug, Clone)]
    struct Badge {
        id: WidgetId,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct BadgeLayout {
        area: Rect,
    }

    impl crate::a11y::A11yPrimitive for Badge {
        fn a11y_info(&self) -> crate::a11y::A11yInfo {
            crate::a11y::A11yInfo::default()
        }
    }

    impl crate::primitive::Primitive for Badge {
        type Layout = BadgeLayout;
        type Event = ();

        fn id(&self) -> Option<&WidgetId> {
            Some(&self.id)
        }

        fn layout(&self, _ctx: &crate::primitive::LayoutCtx, rect: Rect) -> Self::Layout {
            BadgeLayout { area: rect }
        }

        fn paint(
            &self,
            _layout: &Self::Layout,
            _surface: &mut dyn crate::paint_surface::PaintSurface,
            _env: &crate::PaintEnv,
        ) {
        }
    }

    impl crate::interactive::Interactive for Badge {
        type State = ();
        type Action = crate::interactive::Never;

        fn focus(&self) -> crate::interactive::FocusSpec {
            crate::interactive::FocusSpec::Stop
        }

        fn handle(
            &self,
            _state: &mut Self::State,
            _event: &crate::interactive::RoutedEvent,
            _layout: &Self::Layout,
            _cx: &mut crate::interactive::EventCx,
        ) -> crate::Handled<Self::Action> {
            crate::Handled::Ignored
        }
    }

    /// Two different [`Surface::Widget`]s — an empty [`CellGrid`] (whose
    /// [`Interactive::focus`] answers `FocusSpec::None`, there being no
    /// cell to put a cursor on) and the test-local [`Badge`] (a real
    /// [`Interactive`] impl declaring
    /// `FocusSpec::Stop`) — pushed into one [`ScreenLayout`] and drawn
    /// through [`crate::testing::RecordingBackend`]. Proves the single
    /// `Surface::Widget` arm in `draw`/`zone_for` handles both without
    /// caring which concrete widget it erased (each gets its own zone,
    /// with its own id, and — via a direct `draw_dyn` call outside
    /// `ScreenLayout` — its own correctly downcast [`PrimitiveLayout`]),
    /// while `tab_stops` tells them apart correctly by consulting each
    /// one's own `DynWidget::focus()` rather than treating every
    /// `Surface::Widget` alike.
    #[test]
    fn two_different_surface_widgets_get_distinct_zones_tab_stops_and_layouts() {
        use crate::primitives::cell_grid::CellGrid;
        use crate::testing::RecordingBackend;

        let grid_rect = Rect::new(0.0, 0.0, 20.0, 5.0);
        let grid = CellGrid {
            id: WidgetId::new("grid:sheet"),
            row_count: 0,
            col_count: 0,
            frozen_rows: 0,
            frozen_cols: 0,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 6.0,
            row_heights: Default::default(),
            col_widths: Default::default(),
            show_row_headers: false,
            show_col_headers: false,
            row_labels: Default::default(),
            col_labels: Default::default(),
            cursor: (0, 0),
            selection_anchor: None,
            cells: Vec::new(),
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: true,
            show_scrollbar: false,
            editing: None,
            column_separators: false,
            striped: false,
        };
        let badge_rect = Rect::new(0.0, 6.0, 4.0, 1.0);
        let badge = Badge {
            id: WidgetId::new("badge:1"),
        };

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Widget {
            rect: grid_rect,
            widget: Box::new(grid.clone()),
        });
        layout.push(Surface::Widget {
            rect: badge_rect,
            widget: Box::new(badge.clone()),
        });

        let mut backend = RecordingBackend::new();
        let hit_map = layout.draw(&mut backend);

        assert_eq!(hit_map.zones()[0].0, grid_rect);
        assert_eq!(
            hit_map.zones()[0].1,
            FrameZone::Widget {
                idx: 0,
                id: Some(grid.id)
            }
        );
        assert_eq!(hit_map.zones()[1].0, badge_rect);
        assert_eq!(
            hit_map.zones()[1].1,
            FrameZone::Widget {
                idx: 1,
                id: Some(badge.id)
            }
        );

        // Only `badge` is a tab stop: its `Interactive::focus()` answers
        // `FocusSpec::Stop`, while the empty `grid` answers
        // `FocusSpec::None`.
        assert_eq!(layout.tab_stops(), vec![(badge.id, badge_rect)]);

        let grid_layout = backend.draw_dyn(grid_rect, &grid, &());
        let grid_layout = grid_layout
            .downcast_ref::<crate::primitives::cell_grid::CellGridPrimitiveLayout>()
            .expect("CellGrid's own layout downcasts back to CellGridPrimitiveLayout");
        assert_eq!(grid_layout.area, grid_rect);

        let badge_layout = backend.draw_dyn(badge_rect, &badge, &());
        let badge_layout = badge_layout
            .downcast_ref::<BadgeLayout>()
            .expect("Badge's own layout downcasts back to BadgeLayout");
        assert_eq!(badge_layout.area, badge_rect);
    }

    /// A [`ListView`] reached through [`Surface::Widget`] must hit-test
    /// as [`FrameZone::Widget`] both live (`ScreenLayout::zone_for`) and
    /// after an [`OwnedFrame::from_layout`] capture/replay round trip
    /// (`OwnedContent::ListWidget`'s own `zone()`) — the same parity
    /// [`OwnedContent::zone`]'s own doc promises, and distinct from the
    /// legacy [`Surface::List`], which still hit-tests as
    /// [`FrameZone::List`] on both paths.
    #[cfg(feature = "tui")]
    #[test]
    fn list_view_reached_through_surface_widget_round_trips_as_frame_zone_widget() {
        let list_rect = Rect::new(0.0, 0.0, 20.0, 5.0);
        let list = ListView {
            id: WidgetId::new("list:widget"),
            title: None,
            items: Vec::new(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Widget {
            rect: list_rect,
            widget: Box::new(list.clone()),
        });

        let live_hit_map = layout.hit_map();
        assert_eq!(
            live_hit_map.hit_test(1.0, 1.0),
            FrameZone::Widget {
                idx: 0,
                id: Some(list.id)
            }
        );

        let captured = OwnedFrame::from_layout(&layout);
        let json = serde_json::to_string(&captured).expect("OwnedFrame serializes");
        let restored: OwnedFrame = serde_json::from_str(&json).expect("OwnedFrame deserializes");
        let replayed_hit_map = restored.hit_map();
        assert_eq!(
            replayed_hit_map.hit_test(1.0, 1.0),
            FrameZone::Widget {
                idx: 0,
                id: Some(list.id)
            },
            "a captured Surface::Widget(ListView) must hit-test identically to the live layout"
        );
    }

    /// A widget whose [`Interactive::focus`] answers [`FocusSpec::Children`]
    /// with more than one declared child — standing in for the
    /// `DataTable`/`TreeTable` ports [`FocusSpec::Children`] exists for.
    #[derive(Debug, Clone)]
    struct MultiCellWidget {
        id: WidgetId,
    }

    impl crate::a11y::A11yPrimitive for MultiCellWidget {
        fn a11y_info(&self) -> crate::a11y::A11yInfo {
            crate::a11y::A11yInfo::default()
        }
    }

    impl crate::primitive::Primitive for MultiCellWidget {
        type Layout = BadgeLayout;
        type Event = ();

        fn id(&self) -> Option<&WidgetId> {
            Some(&self.id)
        }

        fn layout(&self, _ctx: &crate::primitive::LayoutCtx, rect: Rect) -> Self::Layout {
            BadgeLayout { area: rect }
        }

        fn paint(
            &self,
            _layout: &Self::Layout,
            _surface: &mut dyn crate::paint_surface::PaintSurface,
            _env: &crate::PaintEnv,
        ) {
        }
    }

    impl crate::interactive::Interactive for MultiCellWidget {
        type State = ();
        type Action = crate::interactive::Never;

        fn focus(&self) -> crate::interactive::FocusSpec {
            crate::interactive::FocusSpec::Children(vec![
                crate::interactive::FocusStop {
                    sub: crate::interactive::SubTarget(0),
                },
                crate::interactive::FocusStop {
                    sub: crate::interactive::SubTarget(1),
                },
                crate::interactive::FocusStop {
                    sub: crate::interactive::SubTarget(2),
                },
            ])
        }

        fn handle(
            &self,
            _state: &mut Self::State,
            _event: &crate::interactive::RoutedEvent,
            _layout: &Self::Layout,
            _cx: &mut crate::interactive::EventCx,
        ) -> crate::Handled<Self::Action> {
            crate::Handled::Ignored
        }
    }

    /// [`ScreenLayout::widget_tab_stops`]'s [`FocusSpec::Children`] arm must
    /// contribute exactly one `(WidgetId, Rect)` entry regardless of how
    /// many children the widget declares: `tab_stops()`'s tuple shape has
    /// nowhere to carry a per-child [`crate::interactive::FocusStop::sub`],
    /// so one entry per child would be a run of byte-identical duplicates.
    /// [`crate::focus::FocusTree::focus_next`]'s flat-list stepper resolves
    /// `tab_order.position()` to the *first* matching entry and compares
    /// the *next* slot against the currently focused id — with
    /// duplicates, that next slot is the same id, so `set_focus` sees no
    /// change and Tab/Shift+Tab can never leave the widget. A single stop
    /// keeps the widget reachable and leavable by keyboard until a richer
    /// return shape exists.
    #[test]
    fn widget_tab_stops_collapses_focus_spec_children_to_one_stop() {
        let widget = MultiCellWidget {
            id: WidgetId::new("multicell:1"),
        };
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);

        let stops = ScreenLayout::widget_tab_stops(rect, &widget);
        assert_eq!(
            stops,
            vec![(widget.id, rect)],
            "FocusSpec::Children must collapse to exactly one tab stop, not one per child"
        );

        // Exercised end-to-end through a real `ScreenLayout` too: pushing
        // the widget and reading `tab_stops()` must still show only one
        // stop, and `FocusTree` must therefore be able to step off it.
        let mut layout = ScreenLayout::new();
        layout.push(Surface::Widget {
            rect,
            widget: Box::new(widget.clone()),
        });
        let other_id = WidgetId::new("other:1");
        layout.push(Surface::Widget {
            rect: Rect::new(0.0, 11.0, 10.0, 1.0),
            widget: Box::new(Badge { id: other_id }),
        });

        let all_stops = layout.tab_stops();
        assert_eq!(
            all_stops.iter().filter(|(id, _)| *id == widget.id).count(),
            1,
            "a Children-focus widget must appear exactly once in tab_stops()"
        );

        let mut focus = crate::focus::FocusTree::new();
        focus.sync_tab_order(&all_stops);
        focus.set_focus(Some((widget.id, None)));
        assert!(
            focus.focus_next(),
            "Tab must be able to leave a FocusSpec::Children widget"
        );
        assert_eq!(focus.focused(), Some(&(other_id, None)));
    }

    /// `FrameHitMap` is serializable: round-trips through JSON
    /// (exercising the new `FrameZone` variants too) and reconstructs
    /// via `from_zones`/`zones`.
    #[test]
    fn frame_hit_map_serde_round_trip() {
        let mut map = FrameHitMap::new();
        map.push(Rect::new(0.0, 0.0, 10.0, 10.0), FrameZone::Board { idx: 0 });
        map.push(Rect::new(0.0, 0.0, 5.0, 5.0), FrameZone::Spinner { idx: 1 });

        let json = serde_json::to_string(&map).expect("FrameHitMap serializes");
        let restored: FrameHitMap = serde_json::from_str(&json).expect("FrameHitMap deserializes");
        assert_eq!(restored, map);
        assert_eq!(restored.hit_test(1.0, 1.0), FrameZone::Spinner { idx: 1 });

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
            editor,
        });
        layout.push(Surface::Widget {
            rect: Rect::new(0.0, 0.0, 50.0, 50.0),
            widget: Box::new(palette),
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
        assert_eq!(
            hit_map.hit_test(25.0, 25.0),
            FrameZone::Widget {
                idx: 1,
                id: Some("pal".into())
            }
        );
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

        let mut layout = ScreenLayout::new();
        layout.push(Surface::Editor {
            rect: Rect::new(0.0, 0.0, 20.0, 5.0),
            editor,
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
