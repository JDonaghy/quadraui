//! `Workspace` — an **open-N-view-one** set of opaque document ids with
//! exactly one active, painted and routed through a [`TabBar`] child on
//! the widget contract.
//!
//! # What it is
//!
//! An ordered `Vec` of *opaque* document ids plus one active id. The
//! [`Workspace`] descriptor is the model: [`Workspace::open`] /
//! [`Workspace::close`] / [`Workspace::activate`] / [`Workspace::reorder`]
//! change it and report what changed as [`WorkspaceEvent`]s, so the host
//! app can react (load a file, drop a buffer, persist the session)
//! without diffing it. Painted as a `Surface::Widget` and registered in
//! the app's `FrameTree`, its strip routes clicks and keys through
//! [`Interactive::handle`], which reports intents; the app writes each
//! back with [`Workspace::apply`].
//!
//! ```ignore
//! use quadraui::compose::workspace::{Workspace, WorkspaceDoc, WorkspaceEvent};
//!
//! let mut ws = Workspace::new(WidgetId::new("panel:docs"));
//! ws.open(WorkspaceDoc::new("doc:a", "alpha.rs"));
//! ws.open(WorkspaceDoc::new("doc:b", "beta.rs"));
//!
//! // In AppLogic::render — paint the strip into whatever rect the host
//! // gives it (a panel's rect, a shell's content area, a split pane):
//! let painted = backend.draw_dyn(panel_rect, &ws, &());
//! let layout = painted.downcast_ref::<WorkspacePrimitiveLayout>();
//! // …host paints the active document's body into `layout.body_bounds`.
//!
//! // In AppLogic::frame_tree:
//! tree.push(ws.id, panel_rect, Box::new(ws.clone()));
//!
//! // In AppLogic::handle:
//! if let UiEvent::Workspace(_, intent) = event {
//!     for ev in ws.apply(intent) {
//!         if let WorkspaceEvent::Closed { id, .. } = ev { /* drop the buffer */ }
//!     }
//! }
//! ```
//!
//! # How it differs from [`TabGroup`]
//!
//! [`TabGroup`](crate::compose::tab_group::TabGroup) is N panes in a split
//! tree with tab drag-and-drop; `Workspace` is one strip. A document is
//! just an id plus a label, and the host paints the body itself into
//! [`WorkspacePrimitiveLayout::body_bounds`], so the body can borrow the
//! host's app state:
//!
//! | | `TabGroup` | `Workspace` |
//! |---|---|---|
//! | Panes | N, in a split tree | 1 |
//! | Split / drag-and-drop | yes | no |
//! | Preview tab | no | yes |
//!
//! # Non-goals
//!
//! - **No persistence.** The host app owns session save/restore; the
//!   workspace is pure in-memory state.
//! - **No content ownership.** See above.
//! - **No shell slot.** The strip mounts into *any* rect, which is what
//!   lets a consumer put one inside a single panel.
//!
//! # Close-neighbour rule
//!
//! Closing the **active** document activates the document that now
//! occupies the closed one's index — i.e. its **right-hand neighbour**.
//! When the closed document was the last in the strip there is no
//! right-hand neighbour, so the new last document (its left-hand
//! neighbour) becomes active. Closing the only document leaves the
//! workspace empty and [`Workspace::active_id`] returns `None`.
//! Closing a **non-active** document never changes which document is
//! active (the active index shifts to track it).
//!
//! This is VS Code's rule, and it is pinned by
//! `close_active_activates_right_neighbour` /
//! `close_last_active_activates_left_neighbour` below.
//!
//! # Keyboard
//!
//! The strip's [`Interactive::handle`] answers, all wrapping around the
//! ends of the strip:
//!
//! | Key | Effect |
//! |---|---|
//! | `Ctrl+Tab` / `Ctrl+PageDown` | activate the next document |
//! | `Ctrl+Shift+Tab` / `Ctrl+PageUp` | activate the previous document |
//!
//! # Overflow
//!
//! Overflow scrolling is the [`TabBar`] primitive's job
//! ([`TabBar::fit_active_scroll_offset`]). [`Primitive::layout`] resolves
//! the offset every frame from the active document, so the active tab is
//! visible on the *same* frame it becomes active — not one frame later.
//!
//! # Preview tabs
//!
//! Ported from vimcode's working implementation (`preview_buffer_id`,
//! `promote_preview`, the find-existing-else-replace `open_file_preview`)
//! rather than designed fresh, because claude-coordinator needs the
//! identical policy for its per-panel document tabs and the two apps are
//! meant to stay in lock-step.
//!
//! At most **one** document per workspace is the preview — the
//! replaceable slot VS Code shows italic and reuses for the next
//! single-click navigation instead of accumulating a tab per click.
//! [`Workspace::open_preview`] is the entry point:
//!
//! 1. `id` already open (permanent, or the current preview) → activate it.
//!    No new tab, no replace, no promotion.
//! 2. Otherwise, a preview tab already exists → replace its contents **in
//!    that tab's slot** (same index), discarding the old preview.
//! 3. Otherwise → append a new tab flagged preview.
//!
//! [`Workspace::promote`] drops the preview flag, turning it into an
//! ordinary permanent tab, and is the one primitive every promotion
//! trigger below reduces to.
//!
//! ## The promotion triggers
//!
//! Ported verbatim — this is the behaviour to preserve, not to improve.
//! Note the second row **deliberately diverges from VS Code** (which keeps
//! a tab in preview when clicked in the tab bar): vimcode promotes on
//! select, and that is the contract both consumers are written against.
//!
//! | Trigger | How | vimcode reference |
//! |---|---|---|
//! | Re-opening it as permanent (incl. a double click on the source row, which just calls the permanent open instead of the preview one) | [`Workspace::open`] promotes when `id` is already the preview | `engine/windows.rs:2297-2299`, `engine/tests.rs::test_double_click_promotes_preview` |
//! | Selecting the preview tab | [`Workspace::activate`] / [`Workspace::activate_index`] promote when the newly-selected tab is the preview; a left click on its tab reports [`WorkspaceEvent::Promoted`] after the activation | `engine/windows.rs:1757-1761` (`goto_tab`) |
//! | Editing the document | [`Workspace::set_dirty`]`(id, true)` promotes when `id` is the preview | `engine/keys.rs:427-430` |
//! | Saving | the host calls [`Workspace::promote`] from its save handler | `engine/tests.rs::test_save_promotes_preview` |
//! | Explicit pin | the host calls [`Workspace::promote`] from a pin action | — |
//!
//! Sequential keyboard cycling ([`Workspace::cycle`] / `Ctrl+Tab`)
//! deliberately does **not** promote — only an explicit jump to a
//! specific tab (`activate`/`activate_index`/a click) does, matching
//! `goto_tab` rather than vimcode's separate tab-cycling path. That is why
//! [`Workspace::apply`] activates an [`WorkspaceEvent::Activated`] intent
//! without promoting: a click on the preview tab reports the promotion as
//! its own [`WorkspaceEvent::Promoted`] intent.
//!
//! [`WorkspaceEvent::Promoted`] is emitted exactly once per promotion,
//! whichever trigger caused it. [`Workspace::is_preview`] feeds
//! [`TabItem::is_preview`], which every backend renders italic
//! (`quadraui/src/primitives/tab_bar.rs`).

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::compose::Handled;
use crate::event::{Key, MouseButton, NamedKey, Point, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::paint_surface::PaintSurface;
use crate::primitive::{LayoutCtx, PaintEnv, Primitive};
use crate::primitives::tab_bar::{
    tab_bar_sub_regions, TabBar, TabBarEvent, TabBarPrimitiveLayout, TabBarState, TabItem,
};
use crate::text_util::display_width;
use crate::types::{Modifiers, WidgetId};
#[cfg(feature = "tui")]
use crate::CellSurface;

/// Columns the workspace's portable pre-paint estimate reserves for a
/// closable tab's close affordance: the `×` / `●` glyph plus the trailing
/// separator cell.
///
/// This mirrors `quadraui::tui::TAB_CLOSE_COLS`, which cannot be named
/// here — `compose` compiles with no backend feature enabled, and that
/// constant lives behind `#[cfg(feature = "tui")]`. The two are pinned
/// together by `close_cols_estimate_matches_tui_rasteriser` below, which
/// runs whenever the `tui` feature is on.
const CLOSE_COLS_ESTIMATE: usize = 2;

/// One document in a [`Workspace`].
///
/// `id` is **opaque** to quadraui: a path, a URI, a database key,
/// whatever the host wants. The workspace only ever compares ids for
/// equality and hands them back in [`WorkspaceEvent`]s. `label` is what
/// the tab paints.
///
/// `#[non_exhaustive]`: brand new, so marking it costs no consumer
/// anything today and a later field (an icon, a preview flag) stays
/// additive rather than breaking every literal — see `CLAUDE.md`'s
/// *Downstream consumers* rule 2. Build with [`WorkspaceDoc::new`] plus
/// the `with_*` setters.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDoc {
    /// Opaque, host-defined identity. Unique within one workspace.
    pub id: String,
    /// Text painted on the tab.
    pub label: String,
    /// When `false` the tab paints no close button and neither a click
    /// on it nor a middle click closes it from the strip. Defaults to
    /// `true`.
    pub closable: bool,
    /// Unsaved-changes marker — the tab paints `●` instead of `×`.
    pub dirty: bool,
}

impl WorkspaceDoc {
    /// A closable, clean document.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            closable: true,
            dirty: false,
        }
    }

    /// Set whether this document can be closed from the tab strip.
    pub fn with_closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }

    /// Set the unsaved-changes marker.
    pub fn with_dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }
}

/// What the workspace just did. Returned from every mutating entry point
/// so the host can react without diffing the workspace.
///
/// `#[non_exhaustive]`: see [`WorkspaceDoc`]. Match with a trailing
/// `_ => {}` arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkspaceEvent {
    /// A document was added to the strip and made active.
    Opened {
        /// The document's opaque id.
        id: String,
        /// Where it landed in the strip.
        index: usize,
    },
    /// A different document became the active one.
    Activated {
        /// The now-active document's opaque id.
        id: String,
        /// Its index in the strip.
        index: usize,
    },
    /// A document was removed from the strip. Emitted **before** any
    /// [`Self::Activated`] the close triggers, so a host that drops
    /// buffers on `Closed` never sees an `Activated` for a document it is
    /// about to discard.
    Closed {
        /// The removed document's opaque id.
        id: String,
        /// The index it occupied *before* removal.
        index: usize,
    },
    /// A document moved within the strip.
    Reordered {
        /// The moved document's opaque id.
        id: String,
        /// Its index before the move.
        from: usize,
        /// Its index after the move.
        to: usize,
    },
    /// The preview document became permanent — see the module doc's
    /// *Preview tabs* section for the six triggers that cause this.
    /// Emitted exactly once per promotion, never for a document that
    /// wasn't the preview.
    Promoted {
        /// The promoted document's opaque id.
        id: String,
    },
}

// ── Workspace: the widget-contract descriptor ─────────────────────────

/// An open-N-view-one document strip as a widget-contract descriptor: the
/// documents, which one is active and which one is the preview. The app
/// owns all of it (controlled mode) and applies the [`WorkspaceEvent`]s
/// [`Interactive::handle`] reports with [`Self::apply`], or changes it
/// directly through the model methods ([`Self::open`], [`Self::close`],
/// …), each of which reports what it changed.
///
/// The strip paints and routes through a [`TabBar`] child that shares this
/// workspace's [`Self::id`], so hover, the painted-widget record and the
/// tab-bar drivers (`tab_center`, `tab_close_center`) address the strip by
/// the same id whether it was painted as a `Workspace` or a bare `TabBar`.
///
/// `#[non_exhaustive]`: build with [`Self::new`] plus the `with_*` setters.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The strip's id — also its [`TabBar`] child's id.
    pub id: WidgetId,
    /// Every open document, in strip order.
    pub docs: Vec<WorkspaceDoc>,
    /// Index into [`Self::docs`] of the active document.
    pub active: Option<usize>,
    /// Opaque id of the single preview document, if any — see the module
    /// doc's *Preview tabs* section.
    pub preview: Option<String>,
}

impl Workspace {
    /// An empty workspace whose strip paints under `id`.
    pub fn new(id: impl Into<WidgetId>) -> Self {
        Self {
            id: id.into(),
            docs: Vec::new(),
            active: None,
            preview: None,
        }
    }

    /// Set the open documents.
    #[must_use]
    pub fn with_docs(mut self, docs: Vec<WorkspaceDoc>) -> Self {
        self.docs = docs;
        self
    }

    /// Set the active document's index.
    #[must_use]
    pub fn with_active(mut self, active: Option<usize>) -> Self {
        self.active = active;
        self
    }

    /// Set the preview document's id.
    #[must_use]
    pub fn with_preview(mut self, preview: Option<String>) -> Self {
        self.preview = preview;
        self
    }

    /// Whether `id` is the preview document.
    fn is_preview_str(&self, id: &str) -> bool {
        self.preview.as_deref() == Some(id)
    }

    /// The [`TabBar`] child this workspace paints, scrolled to
    /// `scroll_offset`.
    pub fn tab_bar(&self, scroll_offset: usize) -> TabBar {
        TabBar {
            id: self.id,
            tabs: self
                .docs
                .iter()
                .enumerate()
                .map(|(i, d)| TabItem {
                    label: d.label.clone(),
                    is_active: self.active == Some(i),
                    is_dirty: d.dirty,
                    is_preview: self.is_preview_str(&d.id),
                    is_closable: d.closable,
                })
                .collect(),
            scroll_offset,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: true,
            compact: false,
            ..Default::default()
        }
    }

    /// Largest offset that still shows at least one tab.
    fn max_scroll_offset(&self) -> usize {
        self.docs.len().saturating_sub(1)
    }

    /// Portable pre-paint scroll offset: measure each tab in char cells
    /// (label display width plus the close affordance) and ask the
    /// primitive where the active tab fits. Exact for a cell grid, an
    /// estimate for pixel backends, which [`Primitive::layout`] then
    /// corrects from the tab bar's own measurements.
    fn estimate_scroll_offset(&self, strip_width: f32, char_width: f32) -> usize {
        let Some(active) = self.active else {
            return 0;
        };
        let cell = if char_width > 0.0 { char_width } else { 1.0 };
        let available = (strip_width / cell).max(0.0) as usize;
        TabBar::fit_active_scroll_offset(active, self.docs.len(), available, |i| {
            let doc = &self.docs[i];
            display_width(&doc.label) + if doc.closable { CLOSE_COLS_ESTIMATE } else { 0 }
        })
    }

    /// The step a key press asks the strip to cycle by: `+1` for
    /// `Ctrl+Tab` / `Ctrl+PageDown`, `-1` for `Ctrl+Shift+Tab` /
    /// `Ctrl+PageUp` (crossterm spells the former `BackTab`, GTK a shifted
    /// `Tab`). `None` for every other key.
    pub fn cycle_delta(key: &Key, modifiers: Modifiers) -> Option<isize> {
        if !modifiers.ctrl {
            return None;
        }
        match key {
            Key::Named(NamedKey::BackTab) => Some(-1),
            Key::Named(NamedKey::Tab) if modifiers.shift => Some(-1),
            Key::Named(NamedKey::Tab) => Some(1),
            Key::Named(NamedKey::PageUp) => Some(-1),
            Key::Named(NamedKey::PageDown) => Some(1),
            _ => None,
        }
    }

    /// The index `delta` positions from the active document, wrapping at
    /// both ends; `None` for an empty workspace.
    pub fn cycle_target(&self, delta: isize) -> Option<usize> {
        let n = self.docs.len();
        if n == 0 {
            return None;
        }
        let active = self.active.unwrap_or(0);
        // `FlatNav`'s `PageUp`/`PageDown` treat a zero magnitude as one
        // step, so a zero delta is answered here.
        if delta == 0 {
            return Some(active);
        }
        let key = if delta > 0 {
            NavKey::PageDown(delta as usize)
        } else {
            NavKey::PageUp((-delta) as usize)
        };
        Some(FlatNav::mv(n, Some(active), key, true, |_| true).unwrap_or(active))
    }

    /// The tab strip's height for `ctx`: [`crate::Style::tab_height`] (or
    /// the chrome line height, when taller) on a pixel backend, one row on
    /// a cell grid.
    pub(crate) fn strip_height(ctx: &LayoutCtx) -> f32 {
        let lh = ctx.chrome_line_height();
        if lh > 1.0 {
            lh.max(ctx.style().tab_height)
        } else {
            lh
        }
    }

    /// The intent a tab-bar event on this strip stands for. Activating
    /// the preview tab promotes it: the activation is emitted ahead of
    /// the returned [`WorkspaceEvent::Promoted`].
    fn map_tab_event(&self, event: TabBarEvent, cx: &mut EventCx) -> Handled<WorkspaceEvent> {
        match event {
            TabBarEvent::TabActivated { index } => {
                let Some(doc) = self.docs.get(index) else {
                    return Handled::Consumed;
                };
                let promoted = WorkspaceEvent::Promoted { id: doc.id.clone() };
                let preview = self.is_preview_str(&doc.id);
                if self.active == Some(index) {
                    return if preview {
                        Handled::Event(promoted)
                    } else {
                        Handled::Consumed
                    };
                }
                let activated = WorkspaceEvent::Activated {
                    id: doc.id.clone(),
                    index,
                };
                if preview {
                    cx.emit(activated.into_ui_event(cx.id));
                    Handled::Event(promoted)
                } else {
                    Handled::Event(activated)
                }
            }
            TabBarEvent::TabClosed { index } => self.close_intent(index),
            _ => Handled::Consumed,
        }
    }

    /// [`WorkspaceEvent::Closed`] for the tab at `index`, or
    /// [`Handled::Consumed`] when that document is not closable from the
    /// strip.
    fn close_intent(&self, index: usize) -> Handled<WorkspaceEvent> {
        match self.docs.get(index) {
            Some(doc) if doc.closable => Handled::Event(WorkspaceEvent::Closed {
                id: doc.id.clone(),
                index,
            }),
            _ => Handled::Consumed,
        }
    }

    // ── Model ───────────────────────────────────────────────────────

    /// How many documents are open.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether no documents are open.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Opaque id of the active document, or `None` when the workspace is
    /// empty.
    pub fn active_id(&self) -> Option<&str> {
        self.active_doc().map(|d| d.id.as_str())
    }

    /// The active document, or `None` when the workspace is empty.
    pub fn active_doc(&self) -> Option<&WorkspaceDoc> {
        self.active.and_then(|i| self.docs.get(i))
    }

    /// Strip index of `id`, or `None` when it isn't open.
    pub fn index_of(&self, id: impl Into<WidgetId>) -> Option<usize> {
        let id = id.into();
        self.docs.iter().position(|d| d.id == id.as_str())
    }

    /// Whether `id` is open.
    pub fn contains(&self, id: impl Into<WidgetId>) -> bool {
        self.index_of(id).is_some()
    }

    /// Whether `id` is the workspace's single preview document. See the
    /// module doc's *Preview tabs* section. `false` for an unknown id and
    /// for a permanent (promoted or never-preview) document.
    pub fn is_preview(&self, id: impl Into<WidgetId>) -> bool {
        self.is_preview_str(id.into().as_str())
    }

    /// Append `doc` to the strip as a **permanent** tab and make it
    /// active.
    ///
    /// Re-opening an already-open id does **not** duplicate the tab: the
    /// existing document is activated instead, and the returned events
    /// contain [`WorkspaceEvent::Activated`] rather than
    /// [`WorkspaceEvent::Opened`]. (Its label is left alone — the host
    /// owns labels; use [`Self::set_label`] to change one.)
    ///
    /// When `doc.id` is currently the **preview** document, this also
    /// promotes it — "re-opening it as permanent" is one of the module
    /// doc's promotion triggers. The returned `Vec` then carries both the
    /// activation (if the document wasn't already active) and
    /// [`WorkspaceEvent::Promoted`], in that order. Use
    /// [`Self::open_preview`] instead when the caller wants the
    /// replaceable preview slot, not a permanent tab.
    ///
    /// Returns an empty `Vec` when nothing changed at all — the document
    /// was already open, already active, and already permanent.
    pub fn open(&mut self, doc: WorkspaceDoc) -> Vec<WorkspaceEvent> {
        if let Some(existing) = self.index_of(&doc.id) {
            let mut events: Vec<WorkspaceEvent> =
                self.set_active_index(existing).into_iter().collect();
            let id = self.docs[existing].id.clone();
            events.extend(self.promote(&id));
            return events;
        }
        let index = self.docs.len();
        let id = doc.id.clone();
        self.docs.push(doc);
        self.active = Some(index);
        vec![WorkspaceEvent::Opened { id, index }]
    }

    /// Open `doc` as the workspace's single **preview** document — see the
    /// module doc's *Preview tabs* section for the full open-semantics
    /// table. Never promotes: re-opening the current preview through this
    /// entry point (branch 1) leaves it exactly as previewed, which is
    /// what lets a host click through a list of documents without pinning
    /// each one. Use [`Self::open`] for a permanent tab, and
    /// [`Self::promote`] to pin the current preview explicitly.
    pub fn open_preview(&mut self, doc: WorkspaceDoc) -> Vec<WorkspaceEvent> {
        // 1. Already open — permanent, or the current preview — just
        //    activate it. No new tab, no replace, no promotion.
        if let Some(existing) = self.index_of(&doc.id) {
            return self.set_active_index(existing).into_iter().collect();
        }
        // 2. A preview tab exists for a different document → replace its
        //    contents in that tab's slot.
        if let Some(preview_id) = self.preview.clone() {
            if let Some(index) = self.index_of(&preview_id) {
                let new_id = doc.id.clone();
                self.docs[index] = doc;
                self.preview = Some(new_id.clone());
                self.active = Some(index);
                return vec![
                    WorkspaceEvent::Closed {
                        id: preview_id,
                        index,
                    },
                    WorkspaceEvent::Opened { id: new_id, index },
                ];
            }
        }
        // 3. Otherwise append a new tab flagged preview.
        let index = self.docs.len();
        let id = doc.id.clone();
        self.docs.push(doc);
        self.active = Some(index);
        self.preview = Some(id.clone());
        vec![WorkspaceEvent::Opened { id, index }]
    }

    /// Drop `id`'s preview flag, turning it into an ordinary permanent
    /// tab. Returns [`WorkspaceEvent::Promoted`] when `id` was in fact the
    /// preview; `None` (a no-op) for an unknown id, or a document that was
    /// already permanent.
    ///
    /// This is the primitive every one of the module doc's promotion
    /// triggers reduces to — most of them (saving, an explicit pin) have
    /// no other hook and call this directly from the host's own event
    /// handling.
    pub fn promote(&mut self, id: impl Into<WidgetId>) -> Option<WorkspaceEvent> {
        let id = id.into();
        if !self.is_preview(id) {
            return None;
        }
        self.preview = None;
        Some(WorkspaceEvent::Promoted {
            id: id.as_str().to_string(),
        })
    }

    /// Activate the document with `id`. Returns an empty `Vec` when `id`
    /// isn't open or is already active. Promotes when `id` is the
    /// preview — "selecting the preview tab" (see the module doc).
    pub fn activate(&mut self, id: impl Into<WidgetId>) -> Vec<WorkspaceEvent> {
        match self.index_of(id) {
            Some(index) => self.activate_index(index),
            None => Vec::new(),
        }
    }

    /// Activate the document at `index`. Returns an empty `Vec` when
    /// `index` is out of range. Promotes when the target document is the
    /// preview — "selecting the preview tab" (see the module doc); this
    /// runs even when the tab was already active, since selecting an
    /// already-active preview tab still promotes it. [`Self::cycle`]
    /// deliberately does **not** go through this — sequential keyboard
    /// cycling doesn't promote, only an explicit jump to a tab does.
    pub fn activate_index(&mut self, index: usize) -> Vec<WorkspaceEvent> {
        let mut events: Vec<WorkspaceEvent> = self.set_active_index(index).into_iter().collect();
        if let Some(doc) = self.docs.get(index) {
            let id = doc.id.clone();
            events.extend(self.promote(&id));
        }
        events
    }

    /// Activation with **no** promotion side effect. Shared by
    /// [`Self::open`]/[`Self::open_preview`] (which apply their own,
    /// narrower promotion rules), [`Self::cycle`] and [`Self::apply`].
    /// Returns `None` when `index` is out of range or already active.
    fn set_active_index(&mut self, index: usize) -> Option<WorkspaceEvent> {
        if index >= self.docs.len() || self.active == Some(index) {
            return None;
        }
        self.active = Some(index);
        Some(WorkspaceEvent::Activated {
            id: self.docs[index].id.clone(),
            index,
        })
    }

    /// Close the document with `id`.
    ///
    /// Returns the emitted events in order: always a
    /// [`WorkspaceEvent::Closed`], followed by a
    /// [`WorkspaceEvent::Activated`] when the close moved the active
    /// document (see the module doc's close-neighbour rule). Returns an
    /// empty `Vec` when `id` isn't open. Closing the last remaining
    /// document leaves the workspace empty — it does not panic, and it
    /// still emits `Closed`.
    ///
    /// This ignores [`WorkspaceDoc::closable`]: that flag governs the
    /// *tab strip's* close affordance, not the host's ability to close a
    /// document programmatically.
    pub fn close(&mut self, id: impl Into<WidgetId>) -> Vec<WorkspaceEvent> {
        let Some(index) = self.index_of(id) else {
            return Vec::new();
        };
        let removed = self.docs.remove(index);
        if self.preview.as_deref() == Some(removed.id.as_str()) {
            self.preview = None;
        }
        let mut events = vec![WorkspaceEvent::Closed {
            id: removed.id,
            index,
        }];

        let previous_active = self.active;
        self.active = match previous_active {
            _ if self.docs.is_empty() => None,
            // The closed document *was* active: the tab that slid into
            // its index (its right-hand neighbour) takes over; when it
            // was last, clamp back onto the new last tab.
            Some(active) if active == index => Some(index.min(self.docs.len() - 1)),
            // A tab left of the active one closed: the active document
            // is unchanged, its index shifted left by one.
            Some(active) if active > index => Some(active - 1),
            other => other,
        };

        // Only report an activation when the *document* changed, not
        // when the active document's index merely shifted left.
        if previous_active == Some(index) {
            if let Some(new_active) = self.active {
                events.push(WorkspaceEvent::Activated {
                    id: self.docs[new_active].id.clone(),
                    index: new_active,
                });
            }
        }
        events
    }

    /// Move the document at `from` to index `to`, shifting the documents
    /// in between. The **active document is preserved by identity**, not
    /// by index. Returns `None` when either index is out of range or the
    /// move is a no-op.
    pub fn reorder(&mut self, from: usize, to: usize) -> Option<WorkspaceEvent> {
        if from >= self.docs.len() || to >= self.docs.len() || from == to {
            return None;
        }
        let active_id = self.active_id().map(str::to_string);
        let doc = self.docs.remove(from);
        let id = doc.id.clone();
        self.docs.insert(to, doc);
        self.active = active_id.and_then(|id| self.index_of(&id));
        Some(WorkspaceEvent::Reordered { id, from, to })
    }

    /// Activate the document `delta` positions from the active one,
    /// wrapping at both ends. `cycle(1)` is Ctrl+Tab, `cycle(-1)` is
    /// Ctrl+Shift+Tab. Returns `None` for an empty workspace or when the
    /// step lands back on the already-active document.
    ///
    /// Deliberately does not promote even when it lands on the preview
    /// tab — see [`Self::activate_index`]'s doc.
    pub fn cycle(&mut self, delta: isize) -> Option<WorkspaceEvent> {
        let next = self.cycle_target(delta)?;
        self.set_active_index(next)
    }

    /// Replace `id`'s tab label. Returns `false` when `id` isn't open.
    pub fn set_label(&mut self, id: impl Into<WidgetId>, label: impl Into<String>) -> bool {
        match self.index_of(id) {
            Some(i) => {
                self.docs[i].label = label.into();
                true
            }
            None => false,
        }
    }

    /// Set `id`'s unsaved-changes marker.
    ///
    /// Setting `dirty: true` also promotes `id` when it is the preview
    /// document — "editing the document" is one of the module doc's
    /// promotion triggers, since a host only marks a document dirty in
    /// response to the user changing it. `dirty: false` (e.g. after a
    /// save that doesn't also pin) never promotes on its own; a save that
    /// should promote calls [`Self::promote`] separately.
    ///
    /// Returns the events produced — `[Promoted]`, when this promoted;
    /// otherwise empty, whether or not `id` was found. Use
    /// [`Self::contains`] first to distinguish "unknown id" from "known
    /// id, nothing to report".
    pub fn set_dirty(&mut self, id: impl Into<WidgetId>, dirty: bool) -> Vec<WorkspaceEvent> {
        let id = id.into();
        let Some(i) = self.index_of(id) else {
            return Vec::new();
        };
        self.docs[i].dirty = dirty;
        if dirty {
            self.promote(id).into_iter().collect()
        } else {
            Vec::new()
        }
    }

    /// Write one of the strip's intents back onto the descriptor and
    /// return the changes it made, in temporal order:
    ///
    /// - [`WorkspaceEvent::Activated`] activates the document **without**
    ///   promoting it — a click on the preview tab reports its promotion
    ///   as a separate [`WorkspaceEvent::Promoted`] intent, and a
    ///   `Ctrl+Tab` cycle never promotes.
    /// - [`WorkspaceEvent::Closed`] closes the document — see
    ///   [`Self::close`].
    /// - [`WorkspaceEvent::Promoted`] promotes it — see [`Self::promote`].
    /// - [`WorkspaceEvent::Reordered`] moves it — see [`Self::reorder`].
    /// - [`WorkspaceEvent::Opened`] changes nothing.
    pub fn apply(&mut self, intent: WorkspaceEvent) -> Vec<WorkspaceEvent> {
        match intent {
            WorkspaceEvent::Activated { index, .. } => {
                self.set_active_index(index).into_iter().collect()
            }
            WorkspaceEvent::Closed { id, .. } => self.close(id.as_str()),
            WorkspaceEvent::Promoted { id } => self.promote(id.as_str()).into_iter().collect(),
            WorkspaceEvent::Reordered { from, to, .. } => {
                self.reorder(from, to).into_iter().collect()
            }
            WorkspaceEvent::Opened { .. } => Vec::new(),
        }
    }
}

/// [`Primitive::Layout`] for [`Workspace`]: the strip and body split, the
/// scroll offset this frame resolved and the [`TabBar`] child laid out at
/// that offset.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspacePrimitiveLayout {
    /// The rect the workspace was laid out into.
    pub area: Rect,
    /// The tab strip, flush with the top of [`Self::area`].
    pub strip_bounds: Rect,
    /// Everything below the strip — the host paints the active document
    /// here.
    pub body_bounds: Rect,
    /// The offset that keeps the active tab visible under this frame's
    /// measurements.
    pub scroll_offset: usize,
    /// The child strip, scrolled to [`Self::scroll_offset`].
    pub bar: TabBar,
    /// The child strip's own layout.
    pub tabs: TabBarPrimitiveLayout,
}

impl A11yPrimitive for Workspace {
    fn a11y_info(&self) -> A11yInfo {
        self.tab_bar(0).a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.tab_bar(0).a11y_children()
    }
}

impl Primitive for Workspace {
    type Layout = WorkspacePrimitiveLayout;
    type Event = WorkspaceEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Splits the strip off the top of `rect` and lays the [`TabBar`]
    /// child out at the scroll offset that keeps the active tab visible.
    /// The offset starts from a char-cell estimate; when the child's own
    /// measurements resolve a different one, the child is laid out again
    /// at that offset, so the active tab is on screen the frame it
    /// becomes active.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let strip_height = Self::strip_height(ctx).min(rect.height.max(0.0));
        let strip_bounds = Rect::new(rect.x, rect.y, rect.width, strip_height);
        let body_bounds = Rect::new(
            rect.x,
            rect.y + strip_height,
            rect.width,
            (rect.height - strip_height).max(0.0),
        );
        let mut scroll_offset = self.estimate_scroll_offset(rect.width, ctx.char_width);
        let mut bar = self.tab_bar(scroll_offset);
        let mut tabs = Primitive::layout(&bar, ctx, strip_bounds);
        if tabs.corrected_scroll_offset != scroll_offset {
            scroll_offset = tabs.corrected_scroll_offset.min(self.max_scroll_offset());
            bar = self.tab_bar(scroll_offset);
            tabs = Primitive::layout(&bar, ctx, strip_bounds);
        }
        WorkspacePrimitiveLayout {
            area: rect,
            strip_bounds,
            body_bounds,
            scroll_offset,
            bar,
            tabs,
        }
    }

    /// The strip, through the [`TabBar`] child's own paint; the body is
    /// the host's.
    fn paint(&self, layout: &Self::Layout, surface: &mut dyn PaintSurface, env: &PaintEnv) {
        Primitive::paint(&layout.bar, &layout.tabs, surface, env);
    }

    /// The strip, through the [`TabBar`] child's own cell paint.
    #[cfg(feature = "tui")]
    fn paint_cells(&self, layout: &Self::Layout, cells: &mut dyn CellSurface, env: &PaintEnv) {
        Primitive::paint_cells(&layout.bar, &layout.tabs, cells, env);
    }
}

/// [`Interactive::State`] for [`Workspace`]: the [`TabBar`] child's own
/// state (its keyboard-focused tab). The strip's scroll offset is not
/// stored: [`Primitive::layout`] derives it from the active document every
/// frame.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceState {
    /// The tab strip's interaction state.
    pub tab_bar: TabBarState,
}

impl WidgetAction for WorkspaceEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Workspace(id, self)
    }
}

/// Whether workspace-local `local` lies on the strip.
fn in_strip(layout: &WorkspacePrimitiveLayout, local: Point) -> bool {
    local.x >= 0.0
        && local.y >= 0.0
        && local.x < layout.strip_bounds.width
        && local.y < layout.strip_bounds.height
}

impl Interactive for Workspace {
    type State = WorkspaceState;
    type Action = WorkspaceEvent;

    /// The [`TabBar`] child's own focus: one stop, or none for an empty
    /// strip.
    fn focus(&self) -> FocusSpec {
        Interactive::focus(&self.tab_bar(0))
    }

    /// The strip's tabs, close buttons and chevrons. The strip shares the
    /// workspace's origin, so the child's strip-local rects are already
    /// workspace-local.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, SubTarget)> {
        tab_bar_sub_regions(&layout.tabs.layout)
    }

    /// Routes strip gestures and tab keys through the [`TabBar`] child and
    /// turns what it reports into [`WorkspaceEvent`]s: activating a tab
    /// (promoting it when it is the preview), closing a closable one.
    /// A middle click closes the tab under it anywhere on its span, and
    /// `Ctrl+Tab`-style keys cycle the active document without promoting
    /// it. A press on the body is the host's.
    fn handle(
        &self,
        state: &mut WorkspaceState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<WorkspaceEvent> {
        let strip_press = match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(button) | PointerKind::DoubleClick(button),
                local,
                ..
            } => {
                if !in_strip(layout, *local) {
                    return Handled::Ignored;
                }
                if *button == MouseButton::Middle {
                    let hit = layout.tabs.layout.visible_tabs.iter().find(|vt| {
                        local.x >= vt.bounds.x
                            && local.x < vt.bounds.x + vt.bounds.width
                            && local.y >= vt.bounds.y
                            && local.y < vt.bounds.y + vt.bounds.height
                    });
                    return match hit {
                        Some(vt) => self.close_intent(vt.tab_idx),
                        None => Handled::Consumed,
                    };
                }
                true
            }
            RoutedEvent::Key(KeyEvent { key, modifiers, .. }) => {
                if let Some(delta) = Self::cycle_delta(key, *modifiers) {
                    return match self.cycle_target(delta) {
                        Some(index) if self.active != Some(index) => {
                            Handled::Event(WorkspaceEvent::Activated {
                                id: self.docs[index].id.clone(),
                                index,
                            })
                        }
                        _ => Handled::Consumed,
                    };
                }
                false
            }
            _ => false,
        };
        // The child shares this workspace's id, so it runs against the
        // same `cx`.
        match Interactive::handle(&layout.bar, &mut state.tab_bar, event, &layout.tabs, cx) {
            Handled::Event(tab_event) => self.map_tab_event(tab_event, cx),
            Handled::Consumed => Handled::Consumed,
            Handled::Ignored if strip_press => Handled::Consumed,
            Handled::Ignored => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "tui")]
    use crate::compose::widget_host::drive;
    #[cfg(feature = "tui")]
    use crate::primitive::Units;

    fn ws(ids: &[&str]) -> Workspace {
        let mut ws = Workspace::new(WidgetId::new("test:workspace"));
        for id in ids {
            ws.open(WorkspaceDoc::new(*id, *id));
        }
        ws
    }

    fn named(id: &str) -> Workspace {
        Workspace::new(WidgetId::new(id))
    }

    fn ids(ws: &Workspace) -> Vec<&str> {
        ws.docs.iter().map(|d| d.id.as_str()).collect()
    }

    /// A host for one strip: routes input through the workspace's own
    /// [`Interactive::handle`] against a real cell layout and writes every
    /// intent back with [`Workspace::apply`], as an app does with the
    /// `UiEvent::Workspace` events the runtime delivers.
    #[cfg(feature = "tui")]
    struct Strip {
        ws: Workspace,
        state: WorkspaceState,
        rect: Rect,
        layout: WorkspacePrimitiveLayout,
    }

    #[cfg(feature = "tui")]
    impl Strip {
        fn new(ws: Workspace, rect: Rect) -> Self {
            let layout = Primitive::layout(&ws, &LayoutCtx::new(1.0, 1.0, false), rect);
            Self {
                ws,
                state: WorkspaceState::default(),
                rect,
                layout,
            }
        }

        /// Route `event`: [`Handled::Event`] with the changes applied,
        /// [`Handled::Consumed`] when the strip claimed it but nothing
        /// changed, [`Handled::Ignored`] when it was not the strip's.
        fn route(&mut self, event: RoutedEvent) -> Handled<Vec<WorkspaceEvent>> {
            let driven = drive(
                &self.ws,
                self.ws.id,
                &mut self.state,
                &event,
                &self.layout,
                Units::Cells,
            );
            let claimed = !driven.handled.is_ignored();
            let mut intents: Vec<WorkspaceEvent> = driven
                .emitted
                .into_iter()
                .filter_map(|event| match event {
                    crate::UiEvent::Workspace(_, intent) => Some(intent),
                    _ => None,
                })
                .collect();
            intents.extend(driven.handled.into_event());
            let mut events = Vec::new();
            for intent in intents {
                events.extend(self.ws.apply(intent));
            }
            self.layout = Primitive::layout(&self.ws, &LayoutCtx::new(1.0, 1.0, false), self.rect);
            match (claimed, events.is_empty()) {
                (_, false) => Handled::Event(events),
                (true, true) => Handled::Consumed,
                (false, true) => Handled::Ignored,
            }
        }

        /// A press at absolute `(x, y)`.
        fn click(&mut self, x: f32, y: f32, button: MouseButton) -> Handled<Vec<WorkspaceEvent>> {
            self.route(RoutedEvent::Pointer {
                kind: PointerKind::Down(button),
                sub: None,
                local: Point::new(x - self.rect.x, y - self.rect.y),
                mods: Modifiers::default(),
            })
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) -> Handled<Vec<WorkspaceEvent>> {
            self.route(RoutedEvent::Key(KeyEvent {
                key,
                modifiers,
                repeat: false,
            }))
        }

        /// Absolute centre of tab `index`'s body, clear of its close glyph.
        fn tab_body(&self, index: usize) -> (f32, f32) {
            let vt = self
                .layout
                .tabs
                .layout
                .visible_tabs
                .iter()
                .find(|vt| vt.tab_idx == index)
                .expect("tab is visible");
            let strip = self.layout.strip_bounds;
            (strip.x + vt.bounds.x + 0.5, strip.y + vt.bounds.y)
        }
    }

    #[test]
    fn open_appends_and_activates() {
        let mut ws = named("w");
        assert!(ws.is_empty());
        assert_eq!(ws.active_id(), None);

        let ev = ws.open(WorkspaceDoc::new("a", "alpha"));
        assert_eq!(
            ev,
            vec![WorkspaceEvent::Opened {
                id: "a".into(),
                index: 0
            }]
        );
        let ev = ws.open(WorkspaceDoc::new("b", "beta"));
        assert_eq!(
            ev,
            vec![WorkspaceEvent::Opened {
                id: "b".into(),
                index: 1
            }]
        );
        assert_eq!(ids(&ws), ["a", "b"]);
        assert_eq!(ws.active_id(), Some("b"));
        assert_eq!(ws.len(), 2);
    }

    #[test]
    fn reopening_an_open_id_activates_instead_of_duplicating() {
        let mut ws = ws(&["a", "b", "c"]);
        let ev = ws.open(WorkspaceDoc::new("a", "alpha (again)"));
        assert_eq!(
            ev,
            vec![WorkspaceEvent::Activated {
                id: "a".into(),
                index: 0
            }]
        );
        assert_eq!(ids(&ws), ["a", "b", "c"], "no duplicate tab");
        assert_eq!(
            ws.docs[0].label, "a",
            "re-open must not silently relabel an existing document"
        );
        // Re-opening the already-active one is a no-op — it's permanent,
        // not the preview, so there's nothing to promote either.
        assert_eq!(ws.open(WorkspaceDoc::new("a", "alpha")), vec![]);
    }

    #[test]
    fn activate_reports_only_real_changes() {
        let mut ws = ws(&["a", "b"]);
        assert_eq!(ws.active_id(), Some("b"));
        assert_eq!(ws.activate("b"), vec![], "already active");
        assert_eq!(ws.activate("nope"), vec![], "unknown id");
        assert_eq!(
            ws.activate("a"),
            vec![WorkspaceEvent::Activated {
                id: "a".into(),
                index: 0
            }]
        );
    }

    // ── Close-neighbour rule (module doc) ───────────────────────────

    #[test]
    fn close_active_activates_right_neighbour() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("b");
        let events = ws.close("b");
        assert_eq!(
            events,
            vec![
                WorkspaceEvent::Closed {
                    id: "b".into(),
                    index: 1
                },
                WorkspaceEvent::Activated {
                    id: "c".into(),
                    index: 1
                },
            ],
            "Closed must precede the Activated it triggers"
        );
        assert_eq!(ids(&ws), ["a", "c"]);
        assert_eq!(ws.active_id(), Some("c"));
    }

    #[test]
    fn close_last_active_activates_left_neighbour() {
        let mut ws = ws(&["a", "b", "c"]);
        assert_eq!(ws.active_id(), Some("c"));
        let events = ws.close("c");
        assert_eq!(
            events,
            vec![
                WorkspaceEvent::Closed {
                    id: "c".into(),
                    index: 2
                },
                WorkspaceEvent::Activated {
                    id: "b".into(),
                    index: 1
                },
            ]
        );
        assert_eq!(ws.active_id(), Some("b"));
    }

    #[test]
    fn closing_a_non_active_document_keeps_the_active_one() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("c");
        // Close a tab to the *left* of the active one: index shifts, the
        // active document does not change, and no Activated is emitted.
        let events = ws.close("a");
        assert_eq!(
            events,
            vec![WorkspaceEvent::Closed {
                id: "a".into(),
                index: 0
            }]
        );
        assert_eq!(ws.active_id(), Some("c"));
        assert_eq!(ws.active, Some(1), "index tracked the shift");

        // …and one to the right.
        let mut ws = ws2();
        ws.activate("a");
        let events = ws.close("c");
        assert_eq!(
            events,
            vec![WorkspaceEvent::Closed {
                id: "c".into(),
                index: 2
            }]
        );
        assert_eq!(ws.active_id(), Some("a"));
        assert_eq!(ws.active, Some(0));
    }

    fn ws2() -> Workspace {
        ws(&["a", "b", "c"])
    }

    #[test]
    fn closing_the_last_document_empties_the_workspace_without_panicking() {
        let mut ws = ws(&["only"]);
        let events = ws.close("only");
        assert_eq!(
            events,
            vec![WorkspaceEvent::Closed {
                id: "only".into(),
                index: 0
            }],
            "no Activated — there is nothing left to activate"
        );
        assert!(ws.is_empty());
        assert_eq!(ws.active_id(), None);
        assert_eq!(ws.active, None);
        // Every query and cycle stays safe on an empty workspace.
        assert_eq!(ws.cycle(1), None);
        assert_eq!(ws.close("only"), vec![], "closing twice is a no-op");
        assert!(ws.tab_bar(0).tabs.is_empty());
    }

    #[test]
    fn close_unknown_id_is_a_noop() {
        let mut ws = ws(&["a"]);
        assert_eq!(ws.close("ghost"), vec![]);
        assert_eq!(ws.len(), 1);
    }

    #[test]
    fn id_taking_methods_also_accept_a_widget_id_directly() {
        // Every id-taking method is `impl Into<WidgetId>` — a raw `&str`
        // (every other test in this module) and an actual `WidgetId` both
        // resolve to the same document.
        let mut ws = ws(&["a"]);
        let a = WidgetId::new("a");
        assert!(ws.contains(a));
        assert_eq!(ws.index_of(a), Some(0));
        assert!(!ws.is_preview(a));
        assert_eq!(ws.activate(a), vec![], "already active, so a no-op");
        assert!(ws.set_label(a, "Alpha"));
        assert_eq!(
            ws.close(a),
            vec![WorkspaceEvent::Closed {
                id: "a".into(),
                index: 0
            }]
        );
        assert!(!ws.contains(a));
    }

    // ── Cycling ─────────────────────────────────────────────────────

    #[test]
    fn cycle_wraps_in_both_directions() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("a");
        assert_eq!(ws.cycle(1).map(event_id), Some("b".to_string()));
        assert_eq!(ws.cycle(1).map(event_id), Some("c".to_string()));
        assert_eq!(ws.cycle(1).map(event_id), Some("a".to_string()), "wraps");
        assert_eq!(
            ws.cycle(-1).map(event_id),
            Some("c".to_string()),
            "wraps backwards"
        );
    }

    #[test]
    fn cycle_on_a_single_document_is_a_noop() {
        let mut ws = ws(&["a"]);
        assert_eq!(ws.cycle(1), None);
        assert_eq!(ws.active_id(), Some("a"));
    }

    fn event_id(ev: WorkspaceEvent) -> String {
        match ev {
            WorkspaceEvent::Opened { id, .. }
            | WorkspaceEvent::Activated { id, .. }
            | WorkspaceEvent::Closed { id, .. }
            | WorkspaceEvent::Reordered { id, .. }
            | WorkspaceEvent::Promoted { id } => id,
        }
    }

    // ── Keyboard ────────────────────────────────────────────────────

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        }
    }

    /// Unwrap a strip's answer to the id of the single event it produced.
    #[cfg(feature = "tui")]
    fn only_id(handled: Handled<Vec<WorkspaceEvent>>) -> Option<String> {
        match handled {
            Handled::Event(mut events) if events.len() == 1 => events.pop().map(event_id),
            other => panic!("expected one event, got {other:?}"),
        }
    }

    #[cfg(feature = "tui")]
    #[test]
    fn ctrl_tab_and_page_keys_cycle() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("a");
        let mut strip = Strip::new(ws, Rect::new(0.0, 0.0, 80.0, 4.0));

        let mut step = |key: NamedKey, mods: Modifiers| only_id(strip.key(Key::Named(key), mods));
        assert_eq!(step(NamedKey::Tab, ctrl()), Some("b".to_string()));
        assert_eq!(step(NamedKey::Tab, ctrl_shift()), Some("a".to_string()));
        assert_eq!(
            step(NamedKey::BackTab, ctrl()),
            Some("c".to_string()),
            "crossterm's Ctrl+Shift+Tab spelling"
        );
        assert_eq!(step(NamedKey::PageDown, ctrl()), Some("a".to_string()));
        assert_eq!(step(NamedKey::PageUp, ctrl()), Some("c".to_string()));
    }

    #[cfg(feature = "tui")]
    #[test]
    fn a_cycle_key_on_a_single_document_is_claimed_but_changes_nothing() {
        let mut strip = Strip::new(ws(&["a"]), Rect::new(0.0, 0.0, 80.0, 4.0));
        assert_eq!(
            strip.key(Key::Named(NamedKey::Tab), ctrl()),
            Handled::Consumed
        );
        assert_eq!(strip.ws.active_id(), Some("a"));
    }

    #[cfg(feature = "tui")]
    #[test]
    fn unbound_keys_are_left_for_the_caller() {
        let mut strip = Strip::new(ws(&["a", "b"]), Rect::new(0.0, 0.0, 80.0, 4.0));
        assert_eq!(
            strip.key(Key::Named(NamedKey::Tab), Modifiers::default()),
            Handled::Ignored,
            "plain Tab is not the workspace's key"
        );
        assert_eq!(strip.key(Key::Char('q'), ctrl()), Handled::Ignored);
        assert_eq!(strip.ws.active_id(), Some("b"));
    }

    #[test]
    fn reorder_moves_a_tab_and_preserves_the_active_document() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("a");
        let ev = ws.reorder(0, 2);
        assert_eq!(
            ev,
            Some(WorkspaceEvent::Reordered {
                id: "a".into(),
                from: 0,
                to: 2
            })
        );
        assert_eq!(ids(&ws), ["b", "c", "a"]);
        assert_eq!(
            ws.active_id(),
            Some("a"),
            "active is preserved by identity, not index"
        );
        assert_eq!(ws.active, Some(2));
    }

    #[test]
    fn reorder_rejects_out_of_range_and_noop_moves() {
        let mut ws = ws(&["a", "b"]);
        assert_eq!(ws.reorder(0, 0), None);
        assert_eq!(ws.reorder(5, 0), None);
        assert_eq!(ws.reorder(0, 5), None);
        assert_eq!(ids(&ws), ["a", "b"]);
    }

    // ── Tab bar projection ──────────────────────────────────────────

    #[test]
    fn tab_bar_marks_exactly_one_active_tab_and_carries_doc_flags() {
        let mut ws = named("w");
        ws.open(WorkspaceDoc::new("a", "alpha"));
        ws.open(WorkspaceDoc::new("b", "beta").with_closable(false));
        ws.set_dirty("a", true);
        ws.activate("a");

        let bar = ws.tab_bar(0);
        assert_eq!(bar.id.as_str(), "w");
        assert_eq!(bar.tabs.len(), 2);
        assert_eq!(
            bar.tabs.iter().filter(|t| t.is_active).count(),
            1,
            "open-N-view-one: exactly one active tab"
        );
        assert!(bar.tabs[0].is_active);
        assert!(bar.tabs[0].is_dirty);
        assert!(bar.tabs[0].is_closable);
        assert!(!bar.tabs[1].is_closable);
        assert_eq!(bar.tabs[1].label, "beta");
    }

    #[test]
    fn set_label_reports_unknown_ids() {
        let mut ws = ws(&["a"]);
        assert!(ws.set_label("a", "renamed"));
        assert!(!ws.set_label("ghost", "x"));
        assert_eq!(ws.docs[0].label, "renamed");
    }

    #[test]
    fn set_dirty_sets_the_flag_regardless_of_preview_state() {
        let mut ws = ws(&["a"]);
        // "a" is an ordinary permanent tab — not the preview — so marking
        // it dirty has no events to report. `set_dirty` doesn't return
        // whether the id was known (see its doc); use `contains` for
        // that.
        assert!(ws.contains("a"));
        assert_eq!(ws.set_dirty("a", true), vec![]);
        assert!(!ws.contains("ghost"));
        assert_eq!(ws.set_dirty("ghost", true), vec![], "unknown id is a no-op");
        assert!(ws.docs[0].dirty);
    }

    // ── Overflow ────────────────────────────────────────────────────

    #[test]
    fn scroll_offset_estimate_keeps_the_active_tab_visible() {
        // 8 tabs × 10 columns each ("doc-0" is 5 wide + 2 close cols = 7;
        // use longer labels so the arithmetic is unambiguous) in a
        // 30-column strip: only three fit at a time.
        let mut ws = named("w");
        for i in 0..8 {
            ws.open(WorkspaceDoc::new(format!("d{i}"), format!("docum{i}")));
        }
        // "docum0" = 6 cols + 2 close = 8 per tab; 30 / 8 = 3 tabs fit.
        ws.activate("d0");
        assert_eq!(ws.estimate_scroll_offset(30.0, 1.0), 0);

        ws.activate("d7");
        let offset = ws.estimate_scroll_offset(30.0, 1.0);
        assert!(
            offset > 0 && offset <= 7,
            "activating the last tab must scroll it into view, got {offset}"
        );
        assert!(
            (7 - offset) * 8 <= 30,
            "tabs from the resolved offset through the active one must fit \
             in 30 columns, offset was {offset}"
        );

        ws.activate("d0");
        assert_eq!(
            ws.estimate_scroll_offset(30.0, 1.0),
            0,
            "activating the first tab scrolls back to the start"
        );
    }

    #[test]
    fn scroll_offset_estimate_is_zero_when_everything_fits() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("c");
        assert_eq!(ws.estimate_scroll_offset(200.0, 1.0), 0);
    }

    #[test]
    fn scroll_offset_estimate_survives_degenerate_geometry() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("c");
        // A zero/negative char width must not divide by zero or panic,
        // and must clamp to the same estimate a `char_width` of exactly
        // `1.0` (the fallback `estimate_scroll_offset` substitutes for
        // any non-positive value) would have produced — not some other
        // garbage value that happens not to crash.
        let fallback = ws.estimate_scroll_offset(0.0, 1.0);
        assert_eq!(ws.estimate_scroll_offset(0.0, 0.0), fallback);
        assert_eq!(ws.estimate_scroll_offset(-5.0, 1.0), fallback);
    }

    /// The workspace's portable pre-paint measurement (see
    /// [`CLOSE_COLS_ESTIMATE`]) must agree with what the TUI rasteriser
    /// actually reserves, or the estimate silently drifts and the active
    /// tab lands off-screen at the exact boundary widths.
    #[cfg(feature = "tui")]
    #[test]
    fn close_cols_estimate_matches_tui_rasteriser() {
        assert_eq!(CLOSE_COLS_ESTIMATE, crate::tui::TAB_CLOSE_COLS as usize);
    }

    // ── Preview tabs ────────────────────────────────────────────────

    #[test]
    fn is_preview_is_false_for_unknown_and_permanent_docs() {
        let ws = ws(&["a"]);
        assert!(!ws.is_preview("a"), "a plain open() never sets preview");
        assert!(!ws.is_preview("ghost"));
    }

    #[test]
    fn open_preview_appends_a_flagged_tab_and_activates_it() {
        let mut ws = named("w");
        let events = ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        assert_eq!(
            events,
            vec![WorkspaceEvent::Opened {
                id: "a".into(),
                index: 0
            }]
        );
        assert!(ws.is_preview("a"));
        assert_eq!(ws.active_id(), Some("a"));
        assert!(
            ws.tab_bar(0).tabs[0].is_preview,
            "must feed TabItem::is_preview"
        );
    }

    #[test]
    fn open_preview_of_a_second_doc_replaces_the_first_in_place() {
        // Acceptance: opening doc A as preview, then doc B as preview,
        // leaves one tab, showing B.
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        let events = ws.open_preview(WorkspaceDoc::new("b", "beta"));
        assert_eq!(
            events,
            vec![
                WorkspaceEvent::Closed {
                    id: "a".into(),
                    index: 0
                },
                WorkspaceEvent::Opened {
                    id: "b".into(),
                    index: 0
                },
            ]
        );
        assert_eq!(ids(&ws), ["b"], "one tab, not two");
        assert_eq!(ws.active_id(), Some("b"));
        assert!(ws.is_preview("b"));
        assert!(
            !ws.is_preview("a"),
            "a's slot was taken over, not preserved"
        );
    }

    #[test]
    fn promoting_then_opening_another_preview_yields_two_tabs() {
        // Acceptance: after promote(A), opening B as preview yields two
        // tabs — A permanent, B preview.
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        assert_eq!(
            ws.promote("a"),
            Some(WorkspaceEvent::Promoted { id: "a".into() })
        );
        let events = ws.open_preview(WorkspaceDoc::new("b", "beta"));
        assert_eq!(
            events,
            vec![WorkspaceEvent::Opened {
                id: "b".into(),
                index: 1
            }]
        );
        assert_eq!(ids(&ws), ["a", "b"]);
        assert!(!ws.is_preview("a"), "a was promoted, so it keeps its slot");
        assert!(ws.is_preview("b"));
    }

    #[test]
    fn open_preview_of_an_already_permanent_doc_activates_without_touching_the_preview_slot() {
        // Acceptance: opening a document already present as a permanent
        // tab activates it and does not disturb the preview slot.
        let mut ws = named("w");
        ws.open(WorkspaceDoc::new("a", "alpha")); // permanent
        ws.open_preview(WorkspaceDoc::new("b", "beta")); // preview, active
        assert_eq!(ws.active_id(), Some("b"));

        let events = ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        assert_eq!(
            events,
            vec![WorkspaceEvent::Activated {
                id: "a".into(),
                index: 0
            }]
        );
        assert_eq!(ids(&ws), ["a", "b"], "no new tab, no replace");
        assert_eq!(ws.active_id(), Some("a"));
        assert!(ws.is_preview("b"), "b's preview slot is untouched");
    }

    #[test]
    fn open_preview_of_the_current_preview_reactivates_without_promoting() {
        // Re-navigating to the document that's already the preview must
        // not promote it — otherwise clicking through a list of files
        // would pin the first one on the second glance.
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        ws.activate_index(1_000_000); // no-op, keeps "a" active for clarity
        let events = ws.open_preview(WorkspaceDoc::new("a", "alpha again"));
        assert_eq!(events, vec![], "already open and already active");
        assert!(ws.is_preview("a"), "must stay preview");
        assert_eq!(
            ws.docs[0].label, "alpha",
            "re-open via open_preview doesn't relabel either"
        );
    }

    #[test]
    fn promote_is_a_noop_for_unknown_or_non_preview_ids() {
        let mut ws = ws(&["a"]);
        assert_eq!(ws.promote("a"), None, "a is permanent, not preview");
        assert_eq!(ws.promote("ghost"), None);
    }

    #[test]
    fn promote_is_idempotent_emitting_the_event_exactly_once() {
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        assert_eq!(
            ws.promote("a"),
            Some(WorkspaceEvent::Promoted { id: "a".into() })
        );
        assert!(!ws.is_preview("a"));
        assert_eq!(ws.promote("a"), None, "already permanent — no second event");
    }

    #[test]
    fn closing_the_preview_tab_clears_the_preview_slot() {
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        ws.close("a");
        // A later doc reusing a similar id must not inherit a dangling
        // preview flag from the closed tab.
        ws.open(WorkspaceDoc::new("b", "beta"));
        assert!(!ws.is_preview("b"));
    }

    // ── The six promotion triggers (module doc) ─────────────────────

    #[test]
    fn trigger_reopening_the_preview_as_permanent_promotes_it() {
        // Covers both "re-opening it as permanent" and, since vimcode's
        // double-click-on-the-source-row handler is just this same call,
        // "double click on the source row": both reduce to `open()` on an
        // id that's currently the preview.
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        assert!(ws.is_preview("a"));

        let events = ws.open(WorkspaceDoc::new("a", "alpha"));
        assert_eq!(events, vec![WorkspaceEvent::Promoted { id: "a".into() }]);
        assert!(!ws.is_preview("a"));
    }

    #[test]
    fn trigger_selecting_the_preview_tab_via_activate_promotes_it() {
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        ws.open(WorkspaceDoc::new("b", "beta")); // active moves off "a"
        assert!(ws.is_preview("a"));

        let events = ws.activate("a");
        assert_eq!(
            events,
            vec![
                WorkspaceEvent::Activated {
                    id: "a".into(),
                    index: 0
                },
                WorkspaceEvent::Promoted { id: "a".into() },
            ],
            "selecting the preview tab promotes it — the deliberate VS Code divergence"
        );
        assert!(!ws.is_preview("a"));
    }

    #[cfg(feature = "tui")]
    #[test]
    fn trigger_clicking_the_preview_tab_promotes_it() {
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        ws.open(WorkspaceDoc::new("b", "beta"));
        assert!(ws.is_preview("a"));

        let mut strip = Strip::new(ws, Rect::new(0.0, 0.0, 80.0, 4.0));
        let (x, y) = strip.tab_body(0);
        let events = strip
            .click(x, y, MouseButton::Left)
            .into_event()
            .unwrap_or_default();
        assert_eq!(
            events,
            vec![
                WorkspaceEvent::Activated {
                    id: "a".into(),
                    index: 0
                },
                WorkspaceEvent::Promoted { id: "a".into() },
            ],
            "left click on the preview tab activates, then promotes it"
        );
        assert!(!strip.ws.is_preview("a"));
    }

    #[cfg(feature = "tui")]
    #[test]
    fn clicking_the_already_active_preview_tab_promotes_it() {
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        let mut strip = Strip::new(ws, Rect::new(0.0, 0.0, 80.0, 4.0));
        let (x, y) = strip.tab_body(0);
        assert_eq!(
            strip.click(x, y, MouseButton::Left),
            Handled::Event(vec![WorkspaceEvent::Promoted { id: "a".into() }])
        );
    }

    #[test]
    fn apply_activates_without_promoting_the_preview() {
        let mut ws = named("w");
        ws.open(WorkspaceDoc::new("a", "alpha"));
        ws.open_preview(WorkspaceDoc::new("b", "beta"));
        ws.activate("a");
        let events = ws.apply(WorkspaceEvent::Activated {
            id: "b".into(),
            index: 1,
        });
        assert_eq!(
            events,
            vec![WorkspaceEvent::Activated {
                id: "b".into(),
                index: 1
            }]
        );
        assert!(ws.is_preview("b"), "only a Promoted intent promotes");
        assert_eq!(
            ws.apply(WorkspaceEvent::Promoted { id: "b".into() }),
            vec![WorkspaceEvent::Promoted { id: "b".into() }]
        );
        assert!(!ws.is_preview("b"));
    }

    #[test]
    fn apply_carries_out_close_and_reorder_intents() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("a");
        assert_eq!(
            ws.apply(WorkspaceEvent::Reordered {
                id: "a".into(),
                from: 0,
                to: 2
            }),
            vec![WorkspaceEvent::Reordered {
                id: "a".into(),
                from: 0,
                to: 2
            }]
        );
        assert_eq!(ids(&ws), ["b", "c", "a"]);
        assert_eq!(
            ws.apply(WorkspaceEvent::Closed {
                id: "a".into(),
                index: 2
            }),
            vec![
                WorkspaceEvent::Closed {
                    id: "a".into(),
                    index: 2
                },
                WorkspaceEvent::Activated {
                    id: "c".into(),
                    index: 1
                },
            ]
        );
        assert_eq!(
            ws.apply(WorkspaceEvent::Opened {
                id: "x".into(),
                index: 0
            }),
            vec![],
            "an Opened intent changes nothing"
        );
    }

    #[test]
    fn trigger_editing_the_preview_document_promotes_it() {
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        let events = ws.set_dirty("a", true);
        assert_eq!(events, vec![WorkspaceEvent::Promoted { id: "a".into() }]);
        assert!(!ws.is_preview("a"));
        assert!(ws.docs[0].dirty, "the dirty flag itself is still set");
    }

    #[test]
    fn trigger_saving_promotes_the_preview_document() {
        // Workspace owns no persistence (module doc's
        // non-goals), so "on save" is the host calling `promote`
        // directly from its save handler.
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        ws.set_dirty("a", true); // already promotes via the edit trigger…
        assert!(!ws.is_preview("a"));
        // …so re-preview it to exercise "saving" as its own trigger.
        ws.open_preview(WorkspaceDoc::new("c", "gamma"));
        ws.promote("c").expect("save handler promotes the preview");
        assert!(!ws.is_preview("c"));
    }

    #[test]
    fn trigger_explicit_pin_promotes_the_preview_document() {
        let mut ws = named("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        assert!(ws.is_preview("a"));
        // A pin button in the host's chrome calls the same primitive.
        let event = ws.promote("a");
        assert_eq!(event, Some(WorkspaceEvent::Promoted { id: "a".into() }));
        assert!(!ws.is_preview("a"));
    }

    #[test]
    fn cycle_does_not_promote_even_when_it_lands_on_the_preview_tab() {
        let mut ws = named("w");
        ws.open(WorkspaceDoc::new("a", "alpha"));
        ws.open_preview(WorkspaceDoc::new("b", "beta"));
        ws.activate("a");
        assert!(ws.is_preview("b"));

        let ev = ws.cycle(1); // a -> b
        assert_eq!(ev.map(event_id), Some("b".to_string()));
        assert!(
            ws.is_preview("b"),
            "sequential cycling must not promote — only an explicit jump does"
        );
    }
    #[cfg(feature = "tui")]
    #[test]
    fn middle_click_closes_a_tab_left_click_activates() {
        let mut ws = named("w");
        ws.open(WorkspaceDoc::new("a", "alpha"));
        ws.open(WorkspaceDoc::new("b", "beta"));
        ws.activate("a");
        let mut strip = Strip::new(ws, Rect::new(0.0, 0.0, 80.0, 4.0));

        let (x, y) = strip.tab_body(1);
        let events = strip
            .click(x, y, MouseButton::Middle)
            .into_event()
            .unwrap_or_default();
        assert_eq!(
            events,
            vec![WorkspaceEvent::Closed {
                id: "b".into(),
                index: 1
            }],
            "middle click closes, it does not activate first"
        );
        assert_eq!(ids(&strip.ws), ["a"]);
        assert_eq!(
            strip.ws.active_id(),
            Some("a"),
            "middle-closing a non-active tab must not disturb the active one"
        );
    }

    #[cfg(feature = "tui")]
    #[test]
    fn middle_click_respects_the_closable_flag() {
        let mut ws = named("w");
        ws.open(WorkspaceDoc::new("a", "alpha").with_closable(false));
        let mut strip = Strip::new(ws, Rect::new(0.0, 0.0, 80.0, 4.0));
        let (x, y) = strip.tab_body(0);
        assert_eq!(strip.click(x, y, MouseButton::Middle), Handled::Consumed);
        assert_eq!(
            strip.ws.len(),
            1,
            "non-closable tab survives a middle click"
        );
    }

    #[cfg(feature = "tui")]
    #[test]
    fn a_press_below_the_strip_is_left_for_the_host() {
        let mut strip = Strip::new(ws(&["a", "b"]), Rect::new(0.0, 0.0, 80.0, 4.0));
        assert_eq!(strip.click(3.0, 2.0, MouseButton::Left), Handled::Ignored);
        assert_eq!(strip.ws.active_id(), Some("b"));
    }

    // ── Tab strip height ──────────────────────────────────────────────

    fn gui_backend() -> crate::testing::RecordingBackend {
        crate::testing::RecordingBackend {
            line_height: 20.0,
            chrome_line_height: Some(18.0),
            viewport: crate::Viewport {
                width: 800.0,
                height: 600.0,
                scale: 1.0,
            },
            char_width: 8.0,
            ..Default::default()
        }
    }

    /// Paint `ws` into `rect` through the widget contract, as a host's
    /// `Surface::Widget` does, and read back the layout it painted.
    fn paint(
        ws: &Workspace,
        backend: &mut crate::testing::RecordingBackend,
        rect: Rect,
    ) -> WorkspacePrimitiveLayout {
        use crate::backend::DrawsPrimitives;
        backend
            .draw_dyn(rect, ws, &())
            .downcast_ref::<WorkspacePrimitiveLayout>()
            .cloned()
            .expect("a workspace paints its own layout")
    }

    /// On a pixel backend (GUI), the tab strip is sized off
    /// `Style::tab_height` (35, the default) rather than the raw chrome
    /// line height.
    #[test]
    fn painted_strip_height_uses_style_tab_height_on_gui() {
        let mut backend = gui_backend();
        let layout = paint(
            &ws(&["a", "b"]),
            &mut backend,
            Rect::new(0.0, 0.0, 800.0, 600.0),
        );
        assert_eq!(layout.strip_bounds.height, 35.0);
    }

    /// `Backend::set_style` overriding `tab_height` changes the painted
    /// strip height.
    #[test]
    fn painted_strip_height_follows_a_set_style_override() {
        let mut backend = gui_backend();
        crate::Backend::set_style(&mut backend, crate::Style::default().with_tab_height(50.0));
        let layout = paint(
            &ws(&["a", "b"]),
            &mut backend,
            Rect::new(0.0, 0.0, 800.0, 600.0),
        );
        assert_eq!(layout.strip_bounds.height, 50.0);
    }

    /// On a cell grid (TUI), the strip stays exactly one row tall.
    #[test]
    fn painted_strip_height_ignores_style_tab_height_on_a_cell_grid() {
        let mut backend = crate::testing::RecordingBackend::new();
        crate::Backend::set_style(&mut backend, crate::Style::default().with_tab_height(50.0));
        let layout = paint(
            &ws(&["a", "b"]),
            &mut backend,
            Rect::new(0.0, 0.0, 80.0, 24.0),
        );
        assert_eq!(layout.strip_bounds.height, 1.0);
    }

    // ── Strip geometry from a real layout ─────────────────────────────

    /// A click on a tab's own close glyph closes *that* tab — the
    /// close-before-body classification order `hit_test` owns.
    #[cfg(feature = "tui")]
    #[test]
    fn a_close_glyph_click_closes_that_tab() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("a");
        let mut strip = Strip::new(ws, Rect::new(3.0, 2.0, 60.0, 10.0));
        let origin = strip.layout.strip_bounds;
        let (cx, cy) = strip
            .layout
            .tabs
            .layout
            .tab_close_center(1)
            .expect("tab 1 painted a close glyph");

        let handled = strip.click(origin.x + cx, origin.y + cy, MouseButton::Left);
        assert!(matches!(handled, Handled::Event(_)), "close emitted events");
        assert_eq!(ids(&strip.ws), vec!["a", "c"]);
    }

    /// On a tab body the click activates that tab instead of closing it.
    #[cfg(feature = "tui")]
    #[test]
    fn a_tab_body_click_activates_that_tab() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("a");
        let mut strip = Strip::new(ws, Rect::new(3.0, 2.0, 60.0, 10.0));
        let (x, y) = strip.tab_body(2);
        assert!(matches!(
            strip.click(x, y, MouseButton::Left),
            Handled::Event(_)
        ));
        assert_eq!(strip.ws.active_id(), Some("c"));
        assert_eq!(ids(&strip.ws), vec!["a", "b", "c"]);
    }

    /// A strip too narrow for every tab still keeps the active tab
    /// clickable: the layout adopts the corrected offset, so the active
    /// tab is in `visible_tabs`.
    #[cfg(feature = "tui")]
    #[test]
    fn an_overflowing_strip_keeps_the_active_tab_hittable() {
        let labels: Vec<String> = (0..30).map(|i| format!("doc-{i:02}")).collect();
        let mut ws = named("test:overflow");
        for label in &labels {
            ws.open(WorkspaceDoc::new(label.as_str(), label.as_str()));
        }
        ws.activate(labels[25].as_str());

        let mut strip = Strip::new(ws, Rect::new(0.0, 0.0, 40.0, 10.0));
        assert!(
            strip
                .layout
                .tabs
                .layout
                .visible_tabs
                .iter()
                .any(|vt| vt.tab_idx == 25),
            "the active tab scrolled into view: {:?}",
            strip
                .layout
                .tabs
                .layout
                .visible_tabs
                .iter()
                .map(|v| v.tab_idx)
                .collect::<Vec<_>>()
        );
        assert!(strip.layout.scroll_offset > 0);
        let (x, y) = strip.tab_body(25);
        assert!(matches!(
            strip.click(x, y, MouseButton::Left),
            Handled::Consumed | Handled::Event(_)
        ));
        assert_eq!(strip.ws.active_id(), Some("doc-25"));
    }
}
