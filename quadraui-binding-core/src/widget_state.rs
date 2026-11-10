//! Framework-owned store of per-widget runtime state, keyed by a widget's
//! own stable [`WidgetId`] — the pattern [`crate::text_input::TextEditorCache`]
//! already uses for a standalone `TextInput`'s cursor/selection/undo
//! session, generalised so a future widget kind can reuse it without
//! growing its own `RefCell<HashMap<WidgetId, _>>` and its own pruning
//! logic in [`crate::engine::InteractionEngine`].
//!
//! [`WidgetState`] is type-erased (`Box<dyn Any>` per entry) because this
//! crate's one instance (wired into [`crate::engine::InteractionEngine`])
//! is shared by every widget kind that opts in, and each kind has its own
//! state shape. A caller names the concrete type at each call site via the
//! generic parameter on [`WidgetState::resolve`]/[`WidgetState::get`]/
//! [`WidgetState::set`], the same way `std::any::Any::downcast_ref` always
//! requires the caller to know what it's asking for back.
//!
//! This module defines no widget-specific state itself — only the store
//! and its two policies below. Which widget kinds actually read and write
//! through it is scoped to later work.
//!
//! # Controlled / uncontrolled resolution
//!
//! A widget's descriptor can optionally carry a value for the same piece of
//! state this store also tracks (e.g. a scroll offset, a checked flag).
//! [`WidgetState::resolve`] is the single place that decides which one
//! wins, for every widget kind, so no binding has to reimplement the
//! choice:
//!
//! - `controlled` is `Some(value)` — the descriptor set the field, so the
//!   app's value wins outright. It's also written back into the store (not
//!   just returned), so that if a later frame's descriptor goes back to
//!   `None` for the same id, resolution resumes from this value instead of
//!   whatever older uncontrolled value (if any) predates it.
//! - `controlled` is `None` — the descriptor left the field unset, so the
//!   stored value wins: the entry already in the store for `id`, or
//!   `default()` the first time `id` is seen (or if a previous entry was
//!   stored under a different concrete type — see [`WidgetState::resolve`]'s
//!   own doc for why that can happen and how it's handled).
//!
//! # GC policy
//!
//! State for an id is dropped once that id goes a full frame without being
//! resolved — i.e. isn't part of what got rendered. Tracking this needs no
//! knowledge of *which* ids a frame painted (unlike
//! [`crate::engine::InteractionEngine::set_hits`]'s `TextEditorCache`
//! pruning, which walks the frame's `PaintedHit`s by hand): every
//! [`WidgetState::resolve`] call marks its id "seen this frame", and
//! [`WidgetState::gc`] — called once per frame, after the paint pass that
//! would have called `resolve` for every id still live — removes any entry
//! that wasn't marked, then clears the mark on every entry that survives so
//! the next frame starts the same check over. A widget that stops
//! rendering (removed, navigated away from, …) is therefore collected
//! after exactly one frame of absence, the same lifetime
//! `TextEditorCache`'s own per-frame `retain` already gives a standalone
//! `TextInput`.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

use quadraui::WidgetId;

/// One stored widget-state value plus the GC's own "seen this frame" mark.
struct Entry {
    value: Box<dyn Any>,
    seen_this_frame: bool,
}

/// The store itself — see this module's own doc for its resolution and GC
/// policies. One instance lives in [`crate::engine::InteractionEngine`] and
/// is shared across every widget kind and every frame.
#[derive(Default)]
pub struct WidgetState {
    entries: RefCell<HashMap<WidgetId, Entry>>,
}

impl WidgetState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves `id`'s current value of type `V`, per this module's own
    /// "Controlled / uncontrolled resolution" doc, and marks `id` seen this
    /// frame for [`Self::gc`].
    ///
    /// If an entry already stored for `id` was written under a different
    /// concrete type than `V` (only possible if a caller reuses an id
    /// across two different widget kinds with different state shapes — a
    /// bug in the caller, not something this store can prevent), it's
    /// discarded and reseeded from `default()` rather than panicking: a
    /// stale cross-type entry is still just stale state, not a condition
    /// worth crashing the render loop over.
    pub fn resolve<V>(&self, id: WidgetId, controlled: Option<V>, default: impl FnOnce() -> V) -> V
    where
        V: Clone + 'static,
    {
        let mut entries = self.entries.borrow_mut();
        if let Some(value) = controlled {
            entries.insert(
                id,
                Entry {
                    value: Box::new(value.clone()),
                    seen_this_frame: true,
                },
            );
            return value;
        }
        match entries.get_mut(&id) {
            Some(entry) => {
                entry.seen_this_frame = true;
                match entry.value.downcast_ref::<V>() {
                    Some(value) => value.clone(),
                    None => {
                        let value = default();
                        entry.value = Box::new(value.clone());
                        value
                    }
                }
            }
            None => {
                let value = default();
                entries.insert(
                    id,
                    Entry {
                        value: Box::new(value.clone()),
                        seen_this_frame: true,
                    },
                );
                value
            }
        }
    }

    /// Reads back `id`'s stored value of type `V` without resolving against
    /// a controlled value or a default — `None` when nothing is stored for
    /// `id`, or what's stored was written under a different concrete type.
    /// Does not mark `id` seen; callers that want GC to keep an id alive
    /// should go through [`Self::resolve`] during the same frame.
    pub fn get<V>(&self, id: WidgetId) -> Option<V>
    where
        V: Clone + 'static,
    {
        self.entries
            .borrow()
            .get(&id)
            .and_then(|entry| entry.value.downcast_ref::<V>())
            .cloned()
    }

    /// Overwrites `id`'s stored value, e.g. to persist a mutation an event
    /// handler made outside of [`Self::resolve`]'s own controlled/default
    /// path. Marks `id` seen this frame, same as `resolve`.
    pub fn set<V>(&self, id: WidgetId, value: V)
    where
        V: 'static,
    {
        self.entries.borrow_mut().insert(
            id,
            Entry {
                value: Box::new(value),
                seen_this_frame: true,
            },
        );
    }

    /// Sweeps every entry not marked seen since the last `gc` call, then
    /// clears the mark on every entry that survives. Call once per frame,
    /// after the paint pass that calls [`Self::resolve`]/[`Self::set`] for
    /// every id still live this frame — see this module's own "GC policy"
    /// doc.
    pub fn gc(&self) {
        let mut entries = self.entries.borrow_mut();
        entries.retain(|_, entry| entry.seen_this_frame);
        for entry in entries.values_mut() {
            entry.seen_this_frame = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WidgetState;
    use quadraui::WidgetId;
    use std::cell::Cell;

    #[test]
    fn controlled_value_wins_over_a_stored_uncontrolled_one() {
        let store = WidgetState::new();
        let id = WidgetId::new("widget-state-test:controlled-wins");
        assert_eq!(store.resolve(id, None::<i32>, || 1), 1);
        assert_eq!(store.resolve(id, Some(99), || unreachable!()), 99);
    }

    #[test]
    fn controlled_value_is_written_back_so_a_later_uncontrolled_frame_resumes_from_it() {
        let store = WidgetState::new();
        let id = WidgetId::new("widget-state-test:controlled-writes-back");
        assert_eq!(store.resolve(id, Some(42), || 0), 42);
        // The next frame's descriptor goes uncontrolled (`None`); resolution
        // should resume from the controlled value, not re-run `default`.
        assert_eq!(store.resolve(id, None::<i32>, || unreachable!()), 42);
    }

    #[test]
    fn uncontrolled_value_persists_across_frames_without_recomputing_default() {
        let store = WidgetState::new();
        let id = WidgetId::new("widget-state-test:persists");
        let calls = Cell::new(0);
        let seed = || {
            calls.set(calls.get() + 1);
            7
        };
        assert_eq!(store.resolve(id, None::<i32>, seed), 7);
        // A second frame's resolve for the same id must see the stored
        // value, not call `default` again.
        assert_eq!(store.resolve(id, None::<i32>, || unreachable!()), 7);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn set_overwrites_and_get_reads_back_without_marking_seen() {
        let store = WidgetState::new();
        let id = WidgetId::new("widget-state-test:set-get");
        store.set(id, "hello".to_string());
        assert_eq!(store.get::<String>(id), Some("hello".to_string()));
        assert_eq!(store.get::<i32>(id), None);
    }

    #[test]
    fn gc_keeps_an_id_resolved_this_frame_and_resets_its_mark() {
        let store = WidgetState::new();
        let id = WidgetId::new("widget-state-test:gc-keeps");
        store.resolve(id, None::<i32>, || 5);
        store.gc();
        assert_eq!(store.get::<i32>(id), Some(5));
    }

    #[test]
    fn gc_drops_state_for_an_id_absent_from_the_most_recent_frame() {
        let store = WidgetState::new();
        let id = WidgetId::new("widget-state-test:gc-drops");
        store.resolve(id, None::<i32>, || 5);
        // First gc after the id was resolved: it survives (seen this
        // frame), and its mark resets.
        store.gc();
        assert_eq!(store.get::<i32>(id), Some(5));
        // A frame goes by where `id` is never resolved — e.g. the widget
        // stopped rendering. The next gc should now drop it.
        store.gc();
        assert_eq!(store.get::<i32>(id), None);
    }

    #[test]
    fn resolve_reseeds_from_default_when_the_stored_type_changes() {
        let store = WidgetState::new();
        let id = WidgetId::new("widget-state-test:type-change");
        store.set(id, 123_i32);
        assert_eq!(
            store.resolve(id, None::<String>, || "fresh".to_string()),
            "fresh".to_string()
        );
        assert_eq!(store.get::<String>(id), Some("fresh".to_string()));
        assert_eq!(store.get::<i32>(id), None);
    }
}
