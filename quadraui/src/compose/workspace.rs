//! `WorkspaceController` — an **open-N-view-one** set of opaque document
//! ids with exactly one active, rendered through the [`TabBar`] primitive
//! (quadraui#596, the controller half of #469).
//!
//! # What it is
//!
//! An ordered `Vec` of *opaque* document ids plus one active id, with
//! `open` / `close` / `activate` / `reorder`, a tab strip painted through
//! [`Backend::draw_tab_bar`], and click + keyboard routing. Every state
//! change is reported as a [`WorkspaceEvent`] so the host app can react
//! (load a file, drop a buffer, persist the session) without inspecting
//! controller internals.
//!
//! ```ignore
//! use quadraui::compose::workspace::{WorkspaceController, WorkspaceDoc, WorkspaceEvent};
//!
//! let mut ws = WorkspaceController::new("panel:docs");
//! ws.open(WorkspaceDoc::new("doc:a", "alpha.rs"));
//! ws.open(WorkspaceDoc::new("doc:b", "beta.rs"));
//!
//! // In AppLogic::render — the controller paints its own strip into
//! // whatever rect the host gives it (a panel's rect, a shell's content
//! // area, a split pane — the controller does not care):
//! let layout = ws.render(backend, panel_rect);
//! // …host paints the active document's body into `layout.body_bounds`.
//!
//! // In AppLogic::handle:
//! # let button = quadraui::MouseButton::Left;
//! for ev in ws.handle_click(pos.x, pos.y, button) {
//!     if let WorkspaceEvent::Closed { id, .. } = ev { /* drop the buffer */ }
//! }
//! ```
//!
//! # How it differs from [`TabGroupController`]
//!
//! [`TabGroupController`](crate::compose::tab_group::TabGroupController)
//! owns its content: `PaneTab::content` is a
//! `Box<dyn BackendWidget>` (`Send + 'static`), which *cannot* hold a view
//! that borrows the host's app state. `WorkspaceController` owns **no
//! content at all** — a document is just an id plus a label, and the host
//! paints the body itself into [`WorkspaceLayout::body_bounds`]. That is
//! the deliberate difference, and it is why the two live side by side
//! rather than one replacing the other:
//!
//! | | `TabGroupController` | `WorkspaceController` |
//! |---|---|---|
//! | Panes | N, in a split tree | 1 |
//! | Content | owned `Box<dyn BackendWidget>` | host-painted, id only |
//! | Body borrows app state | impossible (`'static`) | natural |
//! | Split / drag-and-drop | yes | no |
//!
//! # Non-goals
//!
//! - **No persistence.** The host app owns session save/restore; the
//!   controller is pure in-memory state.
//! - **No content ownership.** See above.
//! - **No shell slot.** Mounting a document tab strip into `AppShell`'s
//!   chrome is #469's remaining half; this controller is deliberately
//!   mountable into *any* rect, which is what lets a consumer put one
//!   inside a single panel.
//!
//! # Close-neighbour rule
//!
//! Closing the **active** document activates the document that now
//! occupies the closed one's index — i.e. its **right-hand neighbour**.
//! When the closed document was the last in the strip there is no
//! right-hand neighbour, so the new last document (its left-hand
//! neighbour) becomes active. Closing the only document leaves the
//! workspace empty and [`WorkspaceController::active_id`] returns `None`.
//! Closing a **non-active** document never changes which document is
//! active (the active index shifts to track it).
//!
//! This is VS Code's rule, and it is pinned by
//! `close_active_activates_right_neighbour` /
//! `close_last_active_activates_left_neighbour` below.
//!
//! # Keyboard
//!
//! [`WorkspaceController::handle_key`] consumes, all wrapping around the
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
//! ([`TabBar::fit_active_scroll_offset`]); the controller owns the
//! resulting `scroll_offset` and recomputes it every frame so the active
//! document's tab is visible on the *same* frame it becomes active — not
//! one frame later. See [`WorkspaceController::render`] for the two-pass
//! detail.
//!
//! # Preview tabs (quadraui#597)
//!
//! Ported from vimcode's working implementation (`preview_buffer_id`,
//! `promote_preview`, the find-existing-else-replace `open_file_preview`)
//! rather than designed fresh, because claude-coordinator needs the
//! identical policy for its per-panel document tabs and the two apps are
//! meant to stay in lock-step — see the issue for the "one implementation"
//! rationale.
//!
//! At most **one** document per workspace is the preview — the
//! replaceable slot VS Code shows italic and reuses for the next
//! single-click navigation instead of accumulating a tab per click.
//! [`Self::open_preview`] is the entry point:
//!
//! 1. `id` already open (permanent, or the current preview) → activate it.
//!    No new tab, no replace, no promotion.
//! 2. Otherwise, a preview tab already exists → replace its contents **in
//!    that tab's slot** (same index), discarding the old preview.
//! 3. Otherwise → append a new tab flagged preview.
//!
//! [`Self::promote`] drops the preview flag, turning it into an ordinary
//! permanent tab, and is the one primitive every promotion trigger below
//! reduces to.
//!
//! ## The six promotion triggers
//!
//! Ported verbatim — this is the behaviour to preserve, not to improve.
//! Note the second row **deliberately diverges from VS Code** (which keeps
//! a tab in preview when clicked in the tab bar): vimcode promotes on
//! select, and that is the contract both consumers are written against.
//!
//! | Trigger | How | vimcode reference |
//! |---|---|---|
//! | Re-opening it as permanent (incl. a double click on the source row, which just calls the permanent open instead of the preview one) | [`Self::open`] promotes when `id` is already the preview | `engine/windows.rs:2297-2299`, `engine/tests.rs::test_double_click_promotes_preview` |
//! | Selecting the preview tab | [`Self::activate`] / [`Self::activate_index`] / a left click on the tab body (via [`Self::handle_click`]) promote when the newly-selected tab is the preview | `engine/windows.rs:1757-1761` (`goto_tab`) |
//! | Editing the document | [`Self::set_dirty`]`(id, true)` promotes when `id` is the preview | `engine/keys.rs:427-430` |
//! | Saving | the host calls [`Self::promote`] from its save handler | `engine/tests.rs::test_save_promotes_preview` |
//! | Explicit pin | the host calls [`Self::promote`] from a pin action | — |
//!
//! Sequential keyboard cycling ([`Self::cycle`] / `Ctrl+Tab`) deliberately
//! does **not** promote — only an explicit jump to a specific tab
//! (`activate`/`activate_index`/a click) does, matching `goto_tab` rather
//! than vimcode's separate tab-cycling path.
//!
//! [`WorkspaceEvent::Promoted`] is emitted exactly once per promotion,
//! whichever trigger caused it. [`Self::is_preview`] feeds
//! [`TabItem::is_preview`], which every backend already renders italic
//! (`quadraui/src/tui/tab_bar.rs`).

use crate::event::{Key, MouseButton, NamedKey, Rect};
use crate::primitives::tab_bar::{TabBar, TabBarHits, TabItem};
use crate::text_util::display_width;
use crate::types::{Modifiers, WidgetId};
use crate::Backend;

/// Columns the controller's portable pre-paint estimate reserves for a
/// closable tab's close affordance: the `×` / `●` glyph plus the trailing
/// separator cell.
///
/// This mirrors `quadraui::tui::TAB_CLOSE_COLS`, which cannot be named
/// here — `compose` compiles with no backend feature enabled, and that
/// constant lives behind `#[cfg(feature = "tui")]`. The two are pinned
/// together by `close_cols_estimate_matches_tui_rasteriser` below, which
/// runs whenever the `tui` feature is on.
const CLOSE_COLS_ESTIMATE: usize = 2;

/// One document in a [`WorkspaceController`].
///
/// `id` is **opaque** to quadraui: a path, a URI, a database key,
/// whatever the host wants. The controller only ever compares ids for
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
    /// Opaque, host-defined identity. Unique within one controller.
    pub id: String,
    /// Text painted on the tab.
    pub label: String,
    /// When `false` the tab paints no close button and
    /// [`WorkspaceController::handle_click`] cannot close it. Defaults to
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
/// so the host can react without diffing controller state.
///
/// `#[non_exhaustive]`: see [`WorkspaceDoc`]. Match with a trailing
/// `_ => {}` arm.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Resolved rects from one [`WorkspaceController::render`] call.
///
/// `#[non_exhaustive]`: see [`WorkspaceDoc`]. Only ever returned by
/// quadraui, never constructed by consumers, so this forbids exhaustive
/// destructuring but not field access.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceLayout {
    /// The tab strip the controller painted — one `line_height` tall,
    /// flush with the top of the rect it was given.
    pub strip_bounds: Rect,
    /// Everything below the strip. The **host** paints the active
    /// document's content here; the controller draws nothing in it.
    pub body_bounds: Rect,
}

/// An ordered set of opaque document ids with one active, rendered as a
/// [`TabBar`]. See the module documentation for the close-neighbour rule,
/// the keyboard table, and how this differs from
/// [`TabGroupController`](crate::compose::tab_group::TabGroupController).
#[derive(Debug, Clone)]
pub struct WorkspaceController {
    bar_id: WidgetId,
    docs: Vec<WorkspaceDoc>,
    active: Option<usize>,
    /// Opaque id of the current preview document, if any. See the module
    /// doc's *Preview tabs* section. Tracked by id rather than index so
    /// `close`/`reorder` don't have to keep it in sync by hand.
    preview: Option<String>,
    scroll_offset: usize,
    last_strip: Option<Rect>,
    last_hits: Option<TabBarHits>,
}

impl WorkspaceController {
    /// An empty workspace whose tab strip paints under `bar_id`.
    ///
    /// The id is what a driver test passes to
    /// `TuiDriver::tab_center` / `GtkDriver::tab_center`, so give it
    /// something stable and unique per strip.
    pub fn new(bar_id: impl Into<String>) -> Self {
        Self {
            bar_id: WidgetId::new(bar_id),
            docs: Vec::new(),
            active: None,
            preview: None,
            scroll_offset: 0,
            last_strip: None,
            last_hits: None,
        }
    }

    // ── Queries ─────────────────────────────────────────────────────

    /// The [`WidgetId`] the tab strip paints under.
    pub fn bar_id(&self) -> &WidgetId {
        &self.bar_id
    }

    /// Every open document, in strip order.
    pub fn docs(&self) -> &[WorkspaceDoc] {
        &self.docs
    }

    /// How many documents are open.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether no documents are open.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Index of the active document, or `None` when the workspace is
    /// empty.
    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    /// Opaque id of the active document, or `None` when the workspace is
    /// empty.
    pub fn active_id(&self) -> Option<&str> {
        self.active.map(|i| self.docs[i].id.as_str())
    }

    /// The active document, or `None` when the workspace is empty.
    pub fn active_doc(&self) -> Option<&WorkspaceDoc> {
        self.active.map(|i| &self.docs[i])
    }

    /// Strip index of `id`, or `None` when it isn't open.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.docs.iter().position(|d| d.id == id)
    }

    /// Whether `id` is open.
    pub fn contains(&self, id: &str) -> bool {
        self.index_of(id).is_some()
    }

    /// Whether `id` is the workspace's single preview document. See the
    /// module doc's *Preview tabs* section. `false` for an unknown id and
    /// for a permanent (promoted or never-preview) document.
    pub fn is_preview(&self, id: &str) -> bool {
        self.preview.as_deref() == Some(id)
    }

    /// Scroll offset the last [`Self::render`] resolved — the index of
    /// the leftmost visible tab. Exposed for tests and for hosts that
    /// mirror the strip elsewhere.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    // ── Mutation ────────────────────────────────────────────────────

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
    /// doc's six promotion triggers, and this is the one primitive it
    /// reduces to (a double click on a source row that already opened its
    /// preview is just this same call). The returned `Vec` then carries
    /// both the activation (if the document wasn't already active) and
    /// [`WorkspaceEvent::Promoted`], in that order. Use [`Self::open_preview`]
    /// instead when the caller wants the replaceable preview slot, not a
    /// permanent tab.
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
                self.clamp_scroll_offset();
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
    /// This is the primitive every one of the module doc's six promotion
    /// triggers reduces to — most of them (saving, an explicit pin) have
    /// no other `WorkspaceController` hook and call this directly from
    /// the host's own event handling.
    pub fn promote(&mut self, id: &str) -> Option<WorkspaceEvent> {
        if !self.is_preview(id) {
            return None;
        }
        self.preview = None;
        Some(WorkspaceEvent::Promoted { id: id.to_string() })
    }

    /// Activate the document with `id`. Returns an empty `Vec` when `id`
    /// isn't open or is already active. Promotes when `id` is the
    /// preview — "selecting the preview tab" (see the module doc).
    pub fn activate(&mut self, id: &str) -> Vec<WorkspaceEvent> {
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

    /// Low-level activation with **no** promotion side effect. Shared by
    /// [`Self::open`]/[`Self::open_preview`] (which apply their own,
    /// narrower promotion rules) and [`Self::cycle`] (sequential cycling
    /// never promotes). Returns `None` when `index` is out of range or
    /// already active.
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
    /// *tab strip's* close affordance (see [`Self::handle_click`]), not
    /// the host's ability to close a document programmatically.
    pub fn close(&mut self, id: &str) -> Vec<WorkspaceEvent> {
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
        let closed_the_active_doc = previous_active == Some(index);
        if closed_the_active_doc {
            if let Some(new_active) = self.active {
                events.push(WorkspaceEvent::Activated {
                    id: self.docs[new_active].id.clone(),
                    index: new_active,
                });
            }
        }
        self.clamp_scroll_offset();
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
        self.clamp_scroll_offset();
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
        let n = self.docs.len();
        if n == 0 {
            return None;
        }
        let active = self.active.unwrap_or(0) as isize;
        let n_i = n as isize;
        let next = (active + delta).rem_euclid(n_i) as usize;
        self.set_active_index(next)
    }

    /// Replace `id`'s tab label. Returns `false` when `id` isn't open.
    pub fn set_label(&mut self, id: &str, label: impl Into<String>) -> bool {
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
    /// document — "editing the document" is one of the module doc's six
    /// promotion triggers, since a host only marks a document dirty in
    /// response to the user changing it. `dirty: false` (e.g. after a
    /// save that doesn't also pin) never promotes on its own; a save that
    /// should promote calls [`Self::promote`] separately.
    ///
    /// Returns the events produced — `[Promoted]`, when this promoted;
    /// otherwise empty, whether or not `id` was found. Use
    /// [`Self::contains`] first if the caller needs to distinguish
    /// "unknown id" from "known id, nothing to report".
    pub fn set_dirty(&mut self, id: &str, dirty: bool) -> Vec<WorkspaceEvent> {
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

    // ── Rendering ───────────────────────────────────────────────────

    /// The [`TabBar`] this controller would paint right now. Exposed so a
    /// host can measure or mirror the strip without painting it.
    pub fn tab_bar(&self) -> TabBar {
        TabBar {
            id: self.bar_id.clone(),
            tabs: self
                .docs
                .iter()
                .enumerate()
                .map(|(i, d)| TabItem {
                    label: d.label.clone(),
                    is_active: self.active == Some(i),
                    is_dirty: d.dirty,
                    is_preview: self.is_preview(&d.id),
                    is_closable: d.closable,
                })
                .collect(),
            scroll_offset: self.scroll_offset,
            right_segments: Vec::new(),
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    /// Paint the tab strip along the top of `bounds` and return the
    /// resolved strip / body rects. The host paints the active document's
    /// content into [`WorkspaceLayout::body_bounds`] itself.
    ///
    /// # Keeping the active tab visible
    ///
    /// Two things happen here, and both are needed:
    ///
    /// 1. **Before painting**, the offset is recomputed from a portable
    ///    char-cell estimate via [`TabBar::fit_active_scroll_offset`].
    ///    Backends whose rasteriser reports `correct_scroll_offset`
    ///    verbatim — the TUI one does, because its char-based fit is
    ///    exact and it therefore trusts whatever the caller stored —
    ///    would otherwise paint the *previous* frame's offset, leaving a
    ///    freshly-activated tab off-screen for a frame.
    /// 2. **After painting**, a backend that measures in its own unit
    ///    (GTK's Pango widths) may disagree with that estimate and hand
    ///    back a corrected `correct_scroll_offset`. When it does, the
    ///    strip is repainted inline with the corrected value — the
    ///    two-pass-paint pattern `TabBar`'s module doc prescribes for
    ///    event-driven backends, where a queued redraw is unreliable.
    pub fn render(&mut self, backend: &mut dyn Backend, bounds: Rect) -> WorkspaceLayout {
        let strip_height = backend.line_height().min(bounds.height.max(0.0));
        let strip = Rect::new(bounds.x, bounds.y, bounds.width, strip_height);
        let body = Rect::new(
            bounds.x,
            bounds.y + strip_height,
            bounds.width,
            (bounds.height - strip_height).max(0.0),
        );

        self.scroll_offset = self.estimate_scroll_offset(strip.width, backend.char_width());
        let mut hits = backend.draw_tab_bar(strip, &self.tab_bar(), None);
        if hits.correct_scroll_offset != self.scroll_offset {
            self.scroll_offset = hits.correct_scroll_offset.min(self.max_scroll_offset());
            hits = backend.draw_tab_bar(strip, &self.tab_bar(), None);
        }

        self.last_strip = Some(strip);
        self.last_hits = Some(hits);
        WorkspaceLayout {
            strip_bounds: strip,
            body_bounds: body,
        }
    }

    /// The strip rect the last [`Self::render`] painted, if any.
    pub fn strip_bounds(&self) -> Option<Rect> {
        self.last_strip
    }

    // ── Input ───────────────────────────────────────────────────────

    /// Route a mouse click at `(x, y)` in surface coordinates.
    ///
    /// - [`MouseButton::Middle`] anywhere on a tab (body *or* close
    ///   button) closes it — VS Code's middle-click-to-close, and the
    ///   module doc's "middle-click closes a tab" contract. Every other
    ///   button falls through to the existing left-click behaviour: a
    ///   click on a tab's close button closes it (close buttons win over
    ///   tab bodies, matching
    ///   [`TabGroupController::handle_click`](crate::compose::tab_group::TabGroupController::handle_click));
    ///   a click on a tab body activates it, promoting it when it's the
    ///   preview (see [`Self::activate_index`]).
    ///
    /// Both close paths ignore [`WorkspaceDoc::closable`]: a `false` tab
    /// closes for neither the close button nor a middle click. Anything
    /// else — the body area, dead space in the strip, a click before the
    /// first [`Self::render`] — returns an empty `Vec`.
    pub fn handle_click(&mut self, x: f32, y: f32, button: MouseButton) -> Vec<WorkspaceEvent> {
        let (Some(strip), Some(hits)) = (self.last_strip, self.last_hits.as_ref()) else {
            return Vec::new();
        };
        if y < strip.y || y >= strip.y + strip.height || x < strip.x || x >= strip.x + strip.width {
            return Vec::new();
        }
        let click_x = x as f64;
        let in_range = |(start, end): (f64, f64)| end > start && click_x >= start && click_x < end;

        let body_hit = hits
            .slot_positions
            .iter()
            .enumerate()
            .find(|(_, range)| in_range(**range))
            .map(|(i, _)| i);

        if button == MouseButton::Middle {
            // Middle-click closes whatever tab is under the pointer,
            // anywhere on its span — unlike the close button, it doesn't
            // require landing on the × glyph specifically.
            let Some(i) = body_hit else {
                return Vec::new();
            };
            let Some(doc) = self.docs.get(i) else {
                return Vec::new();
            };
            if !doc.closable {
                return Vec::new();
            }
            let id = doc.id.clone();
            return self.close(&id);
        }

        // Close buttons take precedence over the tab bodies that contain
        // them.
        let close_hit = hits
            .close_bounds
            .iter()
            .enumerate()
            .find(|(_, cb)| cb.is_some_and(in_range))
            .map(|(i, _)| i);
        if let Some(i) = close_hit {
            let Some(doc) = self.docs.get(i) else {
                return Vec::new();
            };
            if !doc.closable {
                return Vec::new();
            }
            let id = doc.id.clone();
            return self.close(&id);
        }

        match body_hit {
            Some(i) => self.activate_index(i),
            None => Vec::new(),
        }
    }

    /// Route a key press. Returns the emitted event, or `None` when the
    /// key isn't one this controller binds (the caller should keep
    /// dispatching it).
    ///
    /// See the module doc for the full binding table: `Ctrl+Tab` /
    /// `Ctrl+PageDown` step forward, `Ctrl+Shift+Tab` / `Ctrl+PageUp`
    /// step back, both wrapping.
    pub fn handle_key(&mut self, key: &Key, modifiers: Modifiers) -> Option<WorkspaceEvent> {
        if !modifiers.ctrl {
            return None;
        }
        let delta = match key {
            // Crossterm reports Ctrl+Shift+Tab as `BackTab`; GTK reports a
            // shifted `Tab`. Accept both spellings so the binding works on
            // every backend without the consumer normalising first.
            Key::Named(NamedKey::BackTab) => -1,
            Key::Named(NamedKey::Tab) if modifiers.shift => -1,
            Key::Named(NamedKey::Tab) => 1,
            Key::Named(NamedKey::PageUp) => -1,
            Key::Named(NamedKey::PageDown) => 1,
            _ => return None,
        };
        self.cycle(delta)
    }

    // ── Internals ───────────────────────────────────────────────────

    /// Largest offset that still shows at least one tab.
    fn max_scroll_offset(&self) -> usize {
        self.docs.len().saturating_sub(1)
    }

    fn clamp_scroll_offset(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
    }

    /// Portable pre-paint scroll offset: measure each tab in char cells
    /// (label display width plus the close affordance) and ask the
    /// primitive where the active tab fits. Exact for the TUI rasteriser,
    /// an estimate for pixel backends — which correct it themselves via
    /// `TabBarHits::correct_scroll_offset`, see [`Self::render`].
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(ids: &[&str]) -> WorkspaceController {
        let mut ws = WorkspaceController::new("test:workspace");
        for id in ids {
            ws.open(WorkspaceDoc::new(*id, *id));
        }
        ws
    }

    fn ids(ws: &WorkspaceController) -> Vec<&str> {
        ws.docs().iter().map(|d| d.id.as_str()).collect()
    }

    #[test]
    fn open_appends_and_activates() {
        let mut ws = WorkspaceController::new("w");
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
            ws.docs()[0].label,
            "a",
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
        assert_eq!(ws.active_index(), Some(1), "index tracked the shift");

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
        assert_eq!(ws.active_index(), Some(0));
    }

    fn ws2() -> WorkspaceController {
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
        assert_eq!(ws.active_index(), None);
        // Every query and cycle stays safe on an empty workspace.
        assert_eq!(ws.cycle(1), None);
        assert_eq!(ws.close("only"), vec![], "closing twice is a no-op");
        assert!(ws.tab_bar().tabs.is_empty());
    }

    #[test]
    fn close_unknown_id_is_a_noop() {
        let mut ws = ws(&["a"]);
        assert_eq!(ws.close("ghost"), vec![]);
        assert_eq!(ws.len(), 1);
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

    #[test]
    fn ctrl_tab_and_page_keys_cycle() {
        let mut ws = ws(&["a", "b", "c"]);
        ws.activate("a");

        assert_eq!(
            ws.handle_key(&Key::Named(NamedKey::Tab), ctrl())
                .map(event_id),
            Some("b".to_string())
        );
        assert_eq!(
            ws.handle_key(&Key::Named(NamedKey::Tab), ctrl_shift())
                .map(event_id),
            Some("a".to_string())
        );
        assert_eq!(
            ws.handle_key(&Key::Named(NamedKey::BackTab), ctrl())
                .map(event_id),
            Some("c".to_string()),
            "crossterm's Ctrl+Shift+Tab spelling"
        );
        assert_eq!(
            ws.handle_key(&Key::Named(NamedKey::PageDown), ctrl())
                .map(event_id),
            Some("a".to_string())
        );
        assert_eq!(
            ws.handle_key(&Key::Named(NamedKey::PageUp), ctrl())
                .map(event_id),
            Some("c".to_string())
        );
    }

    #[test]
    fn unbound_keys_are_left_for_the_caller() {
        let mut ws = ws(&["a", "b"]);
        assert_eq!(
            ws.handle_key(&Key::Named(NamedKey::Tab), Modifiers::default()),
            None,
            "plain Tab is not the workspace's key"
        );
        assert_eq!(ws.handle_key(&Key::Char('q'), ctrl()), None);
        assert_eq!(
            ws.handle_key(&Key::Named(NamedKey::Right), ctrl()),
            None,
            "arrows belong to whatever owns the body"
        );
    }

    // ── Reorder ─────────────────────────────────────────────────────

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
        assert_eq!(ws.active_index(), Some(2));
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
        let mut ws = WorkspaceController::new("w");
        ws.open(WorkspaceDoc::new("a", "alpha"));
        ws.open(WorkspaceDoc::new("b", "beta").with_closable(false));
        ws.set_dirty("a", true);
        ws.activate("a");

        let bar = ws.tab_bar();
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
        assert_eq!(ws.docs()[0].label, "renamed");
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
        assert!(ws.docs()[0].dirty);
    }

    // ── Overflow ────────────────────────────────────────────────────

    #[test]
    fn scroll_offset_estimate_keeps_the_active_tab_visible() {
        // 8 tabs × 10 columns each ("doc-0" is 5 wide + 2 close cols = 7;
        // use longer labels so the arithmetic is unambiguous) in a
        // 30-column strip: only three fit at a time.
        let mut ws = WorkspaceController::new("w");
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
        // A zero/negative char width must not divide by zero or panic.
        let _ = ws.estimate_scroll_offset(0.0, 0.0);
        let _ = ws.estimate_scroll_offset(-5.0, 1.0);
    }

    #[test]
    fn click_before_the_first_render_is_ignored() {
        let mut ws = ws(&["a", "b"]);
        assert_eq!(
            ws.handle_click(3.0, 0.0, MouseButton::Left),
            vec![],
            "no cached strip geometry yet"
        );
    }

    /// The controller's portable pre-paint measurement (see
    /// [`CLOSE_COLS_ESTIMATE`]) must agree with what the TUI rasteriser
    /// actually reserves, or the estimate silently drifts and the active
    /// tab lands off-screen at the exact boundary widths.
    #[cfg(feature = "tui")]
    #[test]
    fn close_cols_estimate_matches_tui_rasteriser() {
        assert_eq!(CLOSE_COLS_ESTIMATE, crate::tui::TAB_CLOSE_COLS as usize);
    }

    // ── Preview tabs (quadraui#597) ─────────────────────────────────

    #[test]
    fn is_preview_is_false_for_unknown_and_permanent_docs() {
        let ws = ws(&["a"]);
        assert!(!ws.is_preview("a"), "a plain open() never sets preview");
        assert!(!ws.is_preview("ghost"));
    }

    #[test]
    fn open_preview_appends_a_flagged_tab_and_activates_it() {
        let mut ws = WorkspaceController::new("w");
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
            ws.tab_bar().tabs[0].is_preview,
            "must feed TabItem::is_preview"
        );
    }

    #[test]
    fn open_preview_of_a_second_doc_replaces_the_first_in_place() {
        // Acceptance: opening doc A as preview, then doc B as preview,
        // leaves one tab, showing B.
        let mut ws = WorkspaceController::new("w");
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
        let mut ws = WorkspaceController::new("w");
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
        let mut ws = WorkspaceController::new("w");
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
        let mut ws = WorkspaceController::new("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        ws.activate_index(1_000_000); // no-op, keeps "a" active for clarity
        let events = ws.open_preview(WorkspaceDoc::new("a", "alpha again"));
        assert_eq!(events, vec![], "already open and already active");
        assert!(ws.is_preview("a"), "must stay preview");
        assert_eq!(
            ws.docs()[0].label,
            "alpha",
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
        let mut ws = WorkspaceController::new("w");
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
        let mut ws = WorkspaceController::new("w");
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
        let mut ws = WorkspaceController::new("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        assert!(ws.is_preview("a"));

        let events = ws.open(WorkspaceDoc::new("a", "alpha"));
        assert_eq!(events, vec![WorkspaceEvent::Promoted { id: "a".into() }]);
        assert!(!ws.is_preview("a"));
    }

    #[test]
    fn trigger_selecting_the_preview_tab_via_activate_promotes_it() {
        let mut ws = WorkspaceController::new("w");
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

    #[test]
    fn trigger_clicking_the_preview_tab_promotes_it() {
        let mut ws = WorkspaceController::new("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        ws.open(WorkspaceDoc::new("b", "beta"));
        assert!(ws.is_preview("a"));

        seed_click_geometry(
            &mut ws,
            Rect::new(0.0, 0.0, 80.0, 1.0),
            &[(0.0, 10.0), (10.0, 20.0)],
        );
        let events = ws.handle_click(5.0, 0.5, MouseButton::Left);
        assert!(
            events.contains(&WorkspaceEvent::Promoted { id: "a".into() }),
            "left click on the preview tab must promote it: {events:?}"
        );
        assert!(!ws.is_preview("a"));
    }

    #[test]
    fn trigger_editing_the_preview_document_promotes_it() {
        let mut ws = WorkspaceController::new("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        let events = ws.set_dirty("a", true);
        assert_eq!(events, vec![WorkspaceEvent::Promoted { id: "a".into() }]);
        assert!(!ws.is_preview("a"));
        assert!(ws.docs()[0].dirty, "the dirty flag itself is still set");
    }

    #[test]
    fn trigger_saving_promotes_the_preview_document() {
        // WorkspaceController owns no persistence (module doc's
        // non-goals), so "on save" is the host calling `promote`
        // directly from its save handler.
        let mut ws = WorkspaceController::new("w");
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
        let mut ws = WorkspaceController::new("w");
        ws.open_preview(WorkspaceDoc::new("a", "alpha"));
        assert!(ws.is_preview("a"));
        // A pin button in the host's chrome calls the same primitive.
        let event = ws.promote("a");
        assert_eq!(event, Some(WorkspaceEvent::Promoted { id: "a".into() }));
        assert!(!ws.is_preview("a"));
    }

    #[test]
    fn cycle_does_not_promote_even_when_it_lands_on_the_preview_tab() {
        let mut ws = WorkspaceController::new("w");
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

    #[test]
    fn middle_click_closes_a_tab_left_click_activates() {
        let mut ws = WorkspaceController::new("w");
        ws.open(WorkspaceDoc::new("a", "alpha"));
        ws.open(WorkspaceDoc::new("b", "beta"));
        ws.activate("a");

        seed_click_geometry(
            &mut ws,
            Rect::new(0.0, 0.0, 80.0, 1.0),
            &[(0.0, 10.0), (10.0, 20.0)],
        );

        let events = ws.handle_click(15.0, 0.5, MouseButton::Middle);
        assert_eq!(
            events,
            vec![WorkspaceEvent::Closed {
                id: "b".into(),
                index: 1
            }],
            "middle click closes, it does not activate first"
        );
        assert_eq!(ids(&ws), ["a"]);
        assert_eq!(
            ws.active_id(),
            Some("a"),
            "middle-closing a non-active tab must not disturb the active one"
        );
    }

    #[test]
    fn middle_click_respects_the_closable_flag() {
        let mut ws = WorkspaceController::new("w");
        ws.open(WorkspaceDoc::new("a", "alpha").with_closable(false));

        seed_click_geometry(&mut ws, Rect::new(0.0, 0.0, 80.0, 1.0), &[(0.0, 10.0)]);

        assert_eq!(ws.handle_click(5.0, 0.5, MouseButton::Middle), vec![]);
        assert_eq!(ws.len(), 1, "non-closable tab survives a middle click");
    }

    /// Test-only: seed the controller's cached click geometry as if
    /// [`WorkspaceController::render`] had just painted `bounds`, tab `i`
    /// spanning `tabs[i]`. Exercises `handle_click`'s real hit-testing and
    /// event logic without a `Backend`; the rasteriser step itself
    /// (actually painting a `TabBar` into those bounds) is covered
    /// end-to-end by the driver tests in `tests/tui_example_driver.rs`.
    fn seed_click_geometry(ws: &mut WorkspaceController, bounds: Rect, tabs: &[(f64, f64)]) {
        ws.last_strip = Some(bounds);
        ws.last_hits = Some(TabBarHits {
            slot_positions: tabs.to_vec(),
            close_bounds: vec![None; tabs.len()],
            right_segment_bounds: Vec::new(),
            available_cols: bounds.width as usize,
            correct_scroll_offset: 0,
        });
    }
}
