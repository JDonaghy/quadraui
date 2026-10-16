//! Rust-owned retained descriptor store: handles + in-place mutation.
//!
//! A host that rebuilds a fresh descriptor `Vec` every frame pays an
//! allocation-and-populate cost proportional to the *whole* collection, even
//! though a backend only ever paints the visible window (see
//! `docs/ROWS_PROVIDER_PROPOSAL.md` for why pushing a windowed provider
//! *inside* a descriptor was tried and rejected — it breaks `PartialEq`/
//! `Serialize` on every descriptor that adopts it). `DescriptorStore<T>` is
//! the alternative that sits *outside* the descriptor instead: the host
//! inserts each descriptor once and gets back a cheap-to-copy [`Handle<T>`],
//! mutates it in place with [`DescriptorStore::update`] as its own state
//! changes, and removes it with [`DescriptorStore::remove`] when it goes
//! away. A frame then becomes `Vec<(Handle<T>, Rect)>` — geometry only — with
//! no descriptor rebuild on the steady-state path at all.
//!
//! ```
//! use quadraui::descriptor_store::{DescriptorStore, Handle};
//!
//! let mut store = DescriptorStore::new();
//! let handle = store.insert(String::from("first"));
//!
//! // Steady state: mutate the descriptor in place, no `Vec` rebuild.
//! store.update(handle, |s| s.push_str(" row"));
//! assert_eq!(store.get(handle).map(String::as_str), Some("first row"));
//!
//! // A frame is geometry keyed by handle, not descriptors by value.
//! let frame: Vec<(Handle<String>, (u32, u32, u32, u32))> = vec![(handle, (0, 0, 100, 20))];
//! assert_eq!(frame.len(), 1);
//!
//! store.remove(handle);
//! assert!(store.get(handle).is_none());
//! ```
//!
//! This module is backend-neutral: it has no `Backend` dependency, no
//! feature gate, and no primitive-specific code. It is the seam every
//! handle-based binding is built on top of; wiring a specific primitive
//! (`TreeView`, `DataTable`, ...) through a store is follow-on, per-primitive
//! work and not this module's concern.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-unique id minted once per [`DescriptorStore`], so a [`Handle`] it
/// hands out can never be mistaken for a same-numbered slot in a different
/// store.
static NEXT_STORE_ID: AtomicU64 = AtomicU64::new(0);

/// Opaque handle to a value inside a [`DescriptorStore<T>`].
///
/// Returned by [`DescriptorStore::insert`] and passed back in to
/// [`DescriptorStore::get`]/[`DescriptorStore::get_mut`]/
/// [`DescriptorStore::update`]/[`DescriptorStore::remove`]. Cheap to copy and
/// carries no reference into the store, so it stays valid to hold across
/// `&mut DescriptorStore` calls.
///
/// Carries its owning store's id alongside a generation-checked slot index:
/// removing a value bumps that slot's generation, so a handle from a
/// different store, or a stale handle into a slot that has since been
/// removed (and possibly reused by a later [`insert`](DescriptorStore::insert)),
/// is rejected by every lookup rather than silently resolving to the wrong
/// value.
pub struct Handle<T> {
    store_id: u64,
    index: u32,
    generation: u32,
    // `fn() -> T` rather than `T` so `Handle<T>` never requires `T: Clone`/
    // `Copy`/etc. just to derive its own marker traits — a handle carries no
    // `T`, it only names a slot that holds one.
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.store_id == other.store_id
            && self.index == other.index
            && self.generation == other.generation
    }
}

impl<T> Eq for Handle<T> {}

impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.store_id.hash(state);
        self.index.hash(state);
        self.generation.hash(state);
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("store_id", &self.store_id)
            .field("index", &self.index)
            .field("generation", &self.generation)
            .finish()
    }
}

/// A slot in a [`DescriptorStore`]: either occupied, or free and carrying the
/// generation the next occupant will be stamped with.
enum Slot<T> {
    Occupied { value: T, generation: u32 },
    Free { generation: u32 },
}

/// Rust-owned store of `T` values, addressed by [`Handle<T>`] instead of by
/// position, so an entry can be mutated or removed in place without
/// rebuilding the collection around it.
///
/// See the module doc for the per-frame-rebuild problem this solves.
pub struct DescriptorStore<T> {
    id: u64,
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> DescriptorStore<T> {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            id: NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed),
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    /// Insert `value` and return a [`Handle`] that resolves to it until it is
    /// [`remove`](Self::remove)d.
    pub fn insert(&mut self, value: T) -> Handle<T> {
        let index = match self.free.pop() {
            Some(index) => {
                let generation = match &self.slots[index as usize] {
                    Slot::Free { generation } => *generation,
                    Slot::Occupied { .. } => {
                        unreachable!("a free-list index must point at a Slot::Free slot")
                    }
                };
                self.slots[index as usize] = Slot::Occupied { value, generation };
                index
            }
            None => {
                let index = self.slots.len() as u32;
                self.slots.push(Slot::Occupied {
                    value,
                    generation: 0,
                });
                index
            }
        };
        self.len += 1;
        let generation = match &self.slots[index as usize] {
            Slot::Occupied { generation, .. } => *generation,
            Slot::Free { .. } => unreachable!("just inserted into this slot"),
        };
        Handle {
            store_id: self.id,
            index,
            generation,
            _marker: PhantomData,
        }
    }

    /// Borrow the value `handle` resolves to, or `None` if it belongs to a
    /// different store or has been removed.
    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        if handle.store_id != self.id {
            return None;
        }
        match self.slots.get(handle.index as usize)? {
            Slot::Occupied { value, generation } if *generation == handle.generation => Some(value),
            _ => None,
        }
    }

    /// Mutably borrow the value `handle` resolves to, or `None` if it belongs
    /// to a different store or has been removed.
    pub fn get_mut(&mut self, handle: Handle<T>) -> Option<&mut T> {
        if handle.store_id != self.id {
            return None;
        }
        match self.slots.get_mut(handle.index as usize)? {
            Slot::Occupied { value, generation } if *generation == handle.generation => Some(value),
            _ => None,
        }
    }

    /// Mutate the value `handle` resolves to in place, in lieu of rebuilding
    /// it from scratch. Returns `true` if `handle` resolved to a live value
    /// (and `f` ran), `false` otherwise.
    pub fn update<F: FnOnce(&mut T)>(&mut self, handle: Handle<T>, f: F) -> bool {
        match self.get_mut(handle) {
            Some(value) => {
                f(value);
                true
            }
            None => false,
        }
    }

    /// Remove and return the value `handle` resolves to, invalidating
    /// `handle` (and every other copy of it) for future lookups. Returns
    /// `None` if `handle` belongs to a different store or was already
    /// removed.
    pub fn remove(&mut self, handle: Handle<T>) -> Option<T> {
        if handle.store_id != self.id {
            return None;
        }
        let slot = self.slots.get_mut(handle.index as usize)?;
        match slot {
            Slot::Occupied { generation, .. } if *generation == handle.generation => {
                let next_generation = generation.wrapping_add(1);
                let Slot::Occupied { value, .. } = std::mem::replace(
                    slot,
                    Slot::Free {
                        generation: next_generation,
                    },
                ) else {
                    unreachable!("matched Slot::Occupied above");
                };
                self.free.push(handle.index);
                self.len -= 1;
                Some(value)
            }
            _ => None,
        }
    }

    /// Number of live values currently in the store.
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` if the store holds no live values.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterate every live `(Handle<T>, &T)` pair, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        let store_id = self.id;
        self.slots
            .iter()
            .enumerate()
            .filter_map(move |(index, slot)| match slot {
                Slot::Occupied { value, generation } => Some((
                    Handle {
                        store_id,
                        index: index as u32,
                        generation: *generation,
                        _marker: PhantomData,
                    },
                    value,
                )),
                Slot::Free { .. } => None,
            })
    }
}

impl<T> Default for DescriptorStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_then_get_roundtrips() {
        let mut store = DescriptorStore::new();
        let handle = store.insert(42);
        assert_eq!(store.get(handle), Some(&42));
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
    }

    #[test]
    fn update_mutates_in_place() {
        let mut store = DescriptorStore::new();
        let handle = store.insert(String::from("a"));
        let ran = store.update(handle, |s| s.push('b'));
        assert!(ran);
        assert_eq!(store.get(handle).map(String::as_str), Some("ab"));
    }

    #[test]
    fn get_mut_allows_direct_mutation() {
        let mut store = DescriptorStore::new();
        let handle = store.insert(1);
        *store.get_mut(handle).unwrap() += 1;
        assert_eq!(store.get(handle), Some(&2));
    }

    #[test]
    fn remove_invalidates_the_handle() {
        let mut store = DescriptorStore::new();
        let handle = store.insert("x");
        assert_eq!(store.remove(handle), Some("x"));
        assert_eq!(store.get(handle), None);
        assert_eq!(store.get_mut(handle), None);
        assert!(!store.update(handle, |_| {}));
        assert_eq!(store.remove(handle), None);
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn reused_slot_gets_a_fresh_generation() {
        let mut store = DescriptorStore::new();
        let first = store.insert(1);
        store.remove(first);
        let second = store.insert(2);

        // The free slot is reused (no unbounded growth from churn)...
        assert_eq!(store.len(), 1);
        // ...but the stale handle into it does not resolve to the new value.
        assert_eq!(store.get(first), None);
        assert_eq!(store.get(second), Some(&2));
        assert_ne!(first, second);
    }

    #[test]
    fn handles_from_different_stores_do_not_collide() {
        let mut a = DescriptorStore::new();
        let mut b = DescriptorStore::new();
        let handle_a = a.insert("from a");
        let handle_b = b.insert("from b");

        // Same slot index in both stores, but each store rejects the other's
        // handle.
        assert_eq!(a.get(handle_b), None);
        assert_eq!(b.get(handle_a), None);
        assert_eq!(a.get(handle_a), Some(&"from a"));
        assert_eq!(b.get(handle_b), Some(&"from b"));
    }

    #[test]
    fn iter_yields_only_live_handles_in_slot_order() {
        let mut store = DescriptorStore::new();
        let first = store.insert(1);
        let second = store.insert(2);
        store.remove(first);
        let third = store.insert(3);

        let collected: Vec<(Handle<i32>, i32)> = store.iter().map(|(h, v)| (h, *v)).collect();
        assert_eq!(collected, vec![(third, 3), (second, 2)]);
    }

    #[test]
    fn handle_is_copy_and_hashable() {
        use std::collections::HashSet;

        let mut store = DescriptorStore::new();
        let handle = store.insert(());
        let copied = handle;
        // `handle` is still usable after the copy — it really is `Copy`.
        assert_eq!(store.get(handle), Some(&()));
        assert_eq!(handle, copied);

        let mut set = HashSet::new();
        set.insert(handle);
        assert!(set.contains(&copied));
    }

    #[test]
    fn frame_as_handle_rect_pairs() {
        // The pattern the module doc describes: a frame is geometry keyed by
        // handle, with the descriptor store mutated in place rather than the
        // frame rebuilding descriptors by value.
        let mut store = DescriptorStore::new();
        let a = store.insert(String::from("row a"));
        let b = store.insert(String::from("row b"));

        let frame: Vec<(Handle<String>, (u32, u32, u32, u32))> =
            vec![(a, (0, 0, 100, 20)), (b, (0, 20, 100, 20))];

        store.update(a, |s| s.push_str(" (edited)"));

        assert_eq!(frame.len(), 2);
        assert_eq!(
            store.get(frame[0].0).map(String::as_str),
            Some("row a (edited)")
        );
        assert_eq!(store.get(frame[1].0).map(String::as_str), Some("row b"));
    }
}
