//! Spike evidence for #1640 — the Diplomat-generated half of the C-ABI
//! production method comparison. See `spikes/ffi-cbindgen/src/lib.rs` for
//! the hand-rolled `extern "C"` + cbindgen half; both skeletons implement
//! the same three surfaces the issue asks a candidate ABI method to prove
//! out, over the same two real quadraui types
//! (`quadraui::descriptor_store::{DescriptorStore, Handle}` and
//! `quadraui::runner::{Reaction, StepOutcome}`), so the two can be judged
//! side by side rather than against different problems:
//!
//! 1. an opaque `u64`-handle store (`QuadraStore`) wrapping `DescriptorStore`,
//! 2. a callback crossing the FFI boundary (here: an ordinary Rust `impl Fn`
//!    bound, not a hand-written `#[repr(C)]` vtable — see the doc comment on
//!    `QuadraRunner::step` for what that buys and costs),
//! 3. `run`/`step`/`wake` mapped onto the real `Reaction`/`StepOutcome`
//!    types, including a detachable cross-thread `QuadraWaker`.
//!
//! Everything inside `mod ffi` is processed by Diplomat's `#[diplomat::bridge]`
//! macro. `diplomat-tool` (a separate CLI, not a `build.rs` step — unlike
//! cbindgen, Diplomat's codegen is normally invoked out-of-band, see
//! `NOTE.md`) would read this module and generate per-language bindings (C,
//! Python/nanobind, JS, ...) from it; this spike does not invoke that tool
//! (see `NOTE.md` for why), so there is no generated header or Python client
//! alongside this file the way there is for `ffi-cbindgen`. What's here is
//! evidence of what the *Rust source* a Diplomat-based `quadraui-ffi` would
//! contain, and what writing it is like, not a full generated-artifact
//! parity check.
//!
//! ## Deliberate simplification vs. the cbindgen spike
//!
//! The cbindgen spike's callback carries a full JSON event/reaction
//! round-trip (`{"type": "woke", "step": N}` in, `{"type": "exit"}` out) to
//! exercise string marshaling and the cross-allocator ownership trap
//! together. Diplomat's `DiplomatStr`/`DiplomatWrite` machinery (see
//! `QuadraStore::label_at` below) already covers the string-marshaling
//! question on its own, so `QuadraRunner::step`'s callback is deliberately
//! simplified to `impl Fn(u64, bool) -> bool` (step count, woke-flag →
//! should-exit) rather than re-deriving the same JSON contract through a
//! `DiplomatStr`/`DiplomatWrite` pair. This keeps the comparison's callback
//! surface legible within this iteration's time budget while still proving
//! out the actually-interesting Diplomat question: does a closure-shaped
//! Rust signature remove the hand-written vtable boilerplate. It does. What
//! it does *not* remove — the `&mut`/`&` aliasing-safety design burden
//! around `run`'s re-entrancy and `wake`'s cross-thread access — is called
//! out explicitly below, because that finding is the whole reason this
//! skeleton also implements `QuadraWaker` rather than stopping at `step`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use quadraui::descriptor_store::{DescriptorStore, Handle};
use quadraui::runner::{Reaction, StepOutcome};

/// Plain Rust value type, same shape as `ffi-cbindgen`'s `RowDescriptor`.
/// Diplomat opaque types typically wrap an ordinary Rust struct like this
/// one rather than exposing its fields across the FFI boundary directly —
/// `QuadraStore` below is the opaque wrapper; `RowDescriptor` itself never
/// appears in `mod ffi`.
#[derive(Debug, Clone)]
struct RowDescriptor {
    label: String,
    value: f64,
}

#[diplomat::bridge]
pub mod ffi {
    use diplomat_runtime::{DiplomatStr, DiplomatWrite};
    use quadraui::runner::StepOutcome;
    use std::fmt::Write as _;

    use super::{QuadraRunnerState, QuadraStoreState, QuadraWakerState, RowDescriptor};

    /// Opaque handle store: real `DescriptorStore` underneath, plain `u64`
    /// handles at the FFI boundary — identical shape to `ffi-cbindgen`'s
    /// `QuadraStore`, so the only difference under evaluation is how the
    /// wrapper functions around it get written (by hand + cbindgen vs. by
    /// this macro).
    #[diplomat::opaque]
    pub struct QuadraStore(QuadraStoreState);

    impl QuadraStore {
        /// Create a new, empty store.
        pub fn create() -> Box<QuadraStore> {
            Box::new(QuadraStore(QuadraStoreState::new()))
        }

        /// Insert a descriptor. Returns a non-zero opaque handle on success,
        /// `0` if `label` isn't valid UTF-8 (Diplomat decodes `&DiplomatStr`
        /// for us up to that point — there is no JSON parse step here at
        /// all, unlike `quadra_store_insert`'s `parse_descriptor`).
        pub fn insert(&mut self, label: &DiplomatStr, value: f64) -> u64 {
            let Ok(label) = String::from_utf8(label.to_owned()) else {
                return 0;
            };
            self.0.insert(RowDescriptor { label, value })
        }

        /// Overwrite the descriptor at `id` in place. Returns `true` iff
        /// `id` is live and `label` is valid UTF-8.
        pub fn update(&mut self, id: u64, label: &DiplomatStr, value: f64) -> bool {
            let Ok(label) = String::from_utf8(label.to_owned()) else {
                return false;
            };
            self.0.update(id, RowDescriptor { label, value })
        }

        /// Remove the descriptor at `id`. Returns `true` iff `id` was live.
        pub fn remove(&mut self, id: u64) -> bool {
            self.0.remove(id)
        }

        /// Number of live descriptors.
        pub fn len(&self) -> usize {
            self.0.len()
        }

        /// Whether the store currently holds no descriptors.
        pub fn is_empty(&self) -> bool {
            self.0.len() == 0
        }

        /// Write the label at `id` into `write`, or leave `write` untouched
        /// if `id` isn't live. This is the Diplomat-idiomatic string-return
        /// shape: no caller-owned buffer, no free callback, no JSON — the
        /// generated binding allocates the target-language string (a
        /// Python `str`, a JS `string`, ...) directly from what `write!`
        /// writes here. Contrast with `ffi-cbindgen`'s `free_reaction`
        /// split, which exists *only* because that spike's callback shape
        /// hands back an owned pointer across the allocator boundary in
        /// the first place; `DiplomatWrite` sidesteps the trap by never
        /// handing back an owned pointer to begin with.
        pub fn label_at(&self, id: u64, write: &mut DiplomatWrite) {
            if let Some(label) = self.0.label(id) {
                let _infallible = write!(write, "{label}");
            }
        }

        /// The numeric half of the descriptor at `id`, or `0.0` if `id`
        /// isn't live. A plain `f64` return needs none of Diplomat's string
        /// machinery — included so both `RowDescriptor` fields are actually
        /// reachable across the FFI boundary, matching `ffi-cbindgen`'s
        /// full-JSON `insert`/`update` round-trip exposing both.
        pub fn value_at(&self, id: u64) -> f64 {
            self.0.value(id).unwrap_or(0.0)
        }
    }

    /// `run`/`step`/`wake` control surface — same real `Reaction`/
    /// `StepOutcome` contract `ffi-cbindgen`'s `QuadraRunner` maps onto, with
    /// a deliberately simplified callback shape (see the module doc comment
    /// for why: `impl Fn(u64, bool) -> bool` in place of the cbindgen
    /// spike's full JSON event/reaction round-trip).
    #[diplomat::opaque]
    pub struct QuadraRunner(QuadraRunnerState);

    /// Mirror of `StepOutcome` at the FFI boundary — a plain C-like enum,
    /// same reasoning as `ffi-cbindgen`'s `QuadraStepOutcome`:
    /// `#[non_exhaustive]` Rust enums aren't handed across the ABI directly.
    pub enum QuadraStepOutcome {
        Continue,
        Exited,
    }

    impl QuadraRunner {
        /// Create a runner. Free with Diplomat's generated destructor (an
        /// opaque type's `Box` is dropped by the generated binding when the
        /// foreign-language wrapper object is collected/disposed — there is
        /// no hand-written `quadra_runner_free` to declare here).
        pub fn create() -> Box<QuadraRunner> {
            Box::new(QuadraRunner(QuadraRunnerState::new()))
        }

        /// Non-blocking single pass: synthesizes one step (a step count and
        /// a woken flag, in place of `ffi-cbindgen`'s JSON event), invokes
        /// `on_event`, and maps its `bool` ("should exit") onto the real
        /// `Reaction`/`StepOutcome` types. Idempotent after exit, matching
        /// `TuiRunner::step`'s contract.
        ///
        /// `on_event: impl Fn(u64, bool) -> bool` is the entire callback
        /// surface — no `#[repr(C)]` vtable struct, no `user_data` pointer,
        /// no separate free function. Diplomat's codegen is what turns this
        /// ordinary Rust closure bound into a per-language callback
        /// (a function-pointer + context pair in C, a native closure
        /// wrapper in Python/JS) — that generation is exactly the
        /// boilerplate `ffi-cbindgen`'s `QuadraCallbacks` struct and its
        /// `handle`/`free_reaction` pair had to be hand-written to cover.
        pub fn step(&mut self, on_event: impl Fn(u64, bool) -> bool) -> QuadraStepOutcome {
            step_outcome_to_ffi(self.0.step(on_event))
        }

        /// Blocking `run`: calls `step` until it reports `Exited`. Same
        /// re-entrancy caveat as `ffi-cbindgen`'s `quadra_runner_run`
        /// applies here unchanged — see the module doc comment's "what
        /// Diplomat does not remove" note and `NOTE.md`.
        pub fn run(&mut self, on_event: impl Fn(u64, bool) -> bool) {
            loop {
                if let StepOutcome::Exited = self.0.step(&on_event) {
                    return;
                }
            }
        }

        /// Clone a waker out of this runner. Safe to call from any thread;
        /// the returned `QuadraWaker` outlives the runner.
        pub fn waker(&self) -> Box<QuadraWaker> {
            Box::new(QuadraWaker(QuadraWakerState(self.0.woken.clone())))
        }
    }

    fn step_outcome_to_ffi(outcome: StepOutcome) -> QuadraStepOutcome {
        match outcome {
            StepOutcome::Continue => QuadraStepOutcome::Continue,
            StepOutcome::Exited => QuadraStepOutcome::Exited,
            // `StepOutcome` is `#[non_exhaustive]`; fail safe toward "keep
            // calling step" for any future variant, same fallback
            // `ffi-cbindgen`'s own `step_outcome_to_ffi` uses.
            _ => QuadraStepOutcome::Continue,
        }
    }

    /// A detachable, cross-thread-safe handle that can wake a
    /// `QuadraRunner`'s next `step`/`run` call. Exactly the same shape as
    /// `ffi-cbindgen`'s `QuadraWaker` — Diplomat's `#[diplomat::opaque]`
    /// macro generates the wrapper functions, but the *design* (clone an
    /// `Arc<AtomicBool>` out of the runner into a wholly separate
    /// allocation, rather than exposing a raw pointer to the runner itself
    /// to waking code) is identical, and had to be arrived at the same way:
    /// Diplomat's codegen only mirrors whatever `&self`/`&mut self` method
    /// shapes this Rust source already has into the generated bindings. It
    /// does not analyze or warn about aliasing across `step`'s `&mut self`
    /// and a naively-shaped `wake(&self)` on the *same* opaque type — that
    /// safety reasoning is still 100% on the author of this file, same as
    /// `ffi-cbindgen`'s module doc comment finding 2 describes. This is
    /// this spike's single most important finding for the Diplomat side:
    /// **the bridge macro generates glue, not a safety analysis.**
    #[diplomat::opaque]
    pub struct QuadraWaker(QuadraWakerState);

    impl QuadraWaker {
        /// Wake the runner this waker was cloned from, from any thread.
        pub fn wake(&self) {
            self.0 .0.store(true, super::Ordering::SeqCst);
        }
    }
}

/// Non-FFI state backing `ffi::QuadraStore`. Kept outside `mod ffi` because
/// Diplomat's bridge macro only processes the opaque wrapper's own method
/// bodies for signature extraction; splitting the real fields and logic out
/// here keeps that boundary obvious and matches how `ffi-cbindgen` also
/// keeps `QuadraStore`'s fields private to its own `impl` block.
struct QuadraStoreState {
    descriptors: DescriptorStore<RowDescriptor>,
    handles: HashMap<u64, Handle<RowDescriptor>>,
    next_id: u64,
}

impl QuadraStoreState {
    fn new() -> Self {
        Self {
            descriptors: DescriptorStore::new(),
            handles: HashMap::new(),
            next_id: 1,
        }
    }

    fn insert(&mut self, descriptor: RowDescriptor) -> u64 {
        let handle = self.descriptors.insert(descriptor);
        let id = self.next_id;
        self.next_id += 1;
        self.handles.insert(id, handle);
        id
    }

    fn update(&mut self, id: u64, new_value: RowDescriptor) -> bool {
        let Some(&handle) = self.handles.get(&id) else {
            return false;
        };
        self.descriptors.update(handle, |slot| *slot = new_value)
    }

    fn remove(&mut self, id: u64) -> bool {
        let Some(handle) = self.handles.remove(&id) else {
            return false;
        };
        self.descriptors.remove(handle).is_some()
    }

    fn len(&self) -> usize {
        self.descriptors.len()
    }

    fn label(&self, id: u64) -> Option<&str> {
        let handle = *self.handles.get(&id)?;
        self.descriptors.get(handle).map(|d| d.label.as_str())
    }

    fn value(&self, id: u64) -> Option<f64> {
        let handle = *self.handles.get(&id)?;
        self.descriptors.get(handle).map(|d| d.value)
    }
}

/// Non-FFI state backing `ffi::QuadraRunner` — same `woken`/`exited`/
/// `step_count` shape as `ffi-cbindgen`'s `QuadraRunner`, so the two
/// runners' actual control-flow logic is identical; only the callback
/// invocation differs (`impl Fn(u64, bool) -> bool` here vs. the vtable
/// there).
struct QuadraRunnerState {
    woken: Arc<AtomicBool>,
    exited: bool,
    step_count: u64,
}

impl QuadraRunnerState {
    fn new() -> Self {
        Self {
            woken: Arc::new(AtomicBool::new(false)),
            exited: false,
            step_count: 0,
        }
    }

    fn step(&mut self, on_event: impl Fn(u64, bool) -> bool) -> StepOutcome {
        if self.exited {
            return StepOutcome::Exited;
        }
        let woke = self.woken.swap(false, Ordering::SeqCst);
        let step = self.step_count;
        self.step_count += 1;
        let should_exit = on_event(step, woke);
        let reaction = if should_exit {
            Reaction::Exit
        } else {
            Reaction::Continue
        };
        match reaction {
            Reaction::Exit => {
                self.exited = true;
                StepOutcome::Exited
            }
            // `Reaction` is `#[non_exhaustive]`; every other known variant
            // (and any future one) just means "keep going" for this
            // skeleton's purposes — same fallback `ffi-cbindgen`'s own
            // `WireReaction`-to-`Reaction` mapping uses.
            Reaction::Continue | Reaction::Redraw | Reaction::RedrawAfter(_) | _ => {
                StepOutcome::Continue
            }
        }
    }
}

/// Non-FFI state backing `ffi::QuadraWaker` — a bare newtype around the
/// `Arc<AtomicBool>` cloned out of a `QuadraRunnerState`. `pub(crate)` field
/// visibility so `ffi::QuadraWaker::wake` (in `mod ffi`, a child module) can
/// reach it while nothing outside this crate can.
struct QuadraWakerState(Arc<AtomicBool>);

#[cfg(test)]
mod tests {
    use super::ffi::{QuadraRunner, QuadraStepOutcome, QuadraStore};
    use std::thread;

    #[test]
    fn store_insert_update_remove_round_trips() {
        let mut store = QuadraStore::create();
        let id = store.insert("first row".as_bytes(), 1.0);
        assert_ne!(id, 0);
        assert!(store.update(id, "first row (edited)".as_bytes(), 2.0));
        assert_eq!(store.len(), 1);

        // `DiplomatWrite` has no `::new()` — Diplomat's generated glue is the
        // only normal caller of it, and it only ever gets one through
        // `diplomat_buffer_write_create`/`diplomat_simple_write` (or the
        // per-backend equivalent). A hand-written Rust unit test has to go
        // through the same free functions real foreign callers' generated
        // bindings would use, which means `unsafe` even in a pure-Rust test —
        // a real friction point `ffi-cbindgen`'s plain `*const c_char` +
        // `CStr::from_ptr` output never has.
        let write_ptr = diplomat_runtime::diplomat_buffer_write_create(64);
        let out = unsafe {
            // Safety: `write_ptr` was just constructed above by
            // `diplomat_buffer_write_create` and is destroyed exactly once,
            // right after this block reads it out.
            let write = &mut *write_ptr;
            store.label_at(id, write);
            write.flush();
            let bytes = write.as_bytes().to_vec();
            diplomat_runtime::diplomat_buffer_write_destroy(write_ptr);
            String::from_utf8(bytes).unwrap()
        };
        assert_eq!(out, "first row (edited)");
        assert_eq!(store.value_at(id), 2.0);

        assert!(store.remove(id));
        assert_eq!(store.len(), 0);
        assert!(!store.remove(id));
    }

    #[test]
    fn insert_rejects_invalid_utf8() {
        let mut store = QuadraStore::create();
        let invalid = [0xFFu8, 0xFE];
        assert_eq!(store.insert(&invalid, 1.0), 0);
    }

    #[test]
    fn runner_step_continues_until_callback_returns_exit() {
        let mut runner = QuadraRunner::create();
        let calls = std::cell::RefCell::new(Vec::new());
        let on_event = |step: u64, woke: bool| {
            calls.borrow_mut().push((step, woke));
            step >= 2
        };
        assert!(matches!(runner.step(on_event), QuadraStepOutcome::Continue));
        assert!(matches!(runner.step(on_event), QuadraStepOutcome::Continue));
        assert!(matches!(runner.step(on_event), QuadraStepOutcome::Exited));
        // Idempotent after exit.
        assert!(matches!(runner.step(on_event), QuadraStepOutcome::Exited));
        assert_eq!(*calls.borrow(), vec![(0, false), (1, false), (2, false)]);
    }

    #[test]
    fn runner_run_loops_until_exit() {
        let mut runner = QuadraRunner::create();
        let seen = std::cell::RefCell::new(0u64);
        runner.run(|step, _woke| {
            *seen.borrow_mut() = step;
            step >= 3
        });
        assert_eq!(*seen.borrow(), 3);
    }

    #[test]
    fn waker_wakes_from_a_different_thread_than_step() {
        let mut runner = QuadraRunner::create();
        let waker = runner.waker();

        assert!(matches!(
            runner.step(|_step, woke| {
                assert!(!woke);
                false
            }),
            QuadraStepOutcome::Continue
        ));

        let handle = thread::spawn(move || {
            waker.wake();
        });
        handle.join().unwrap();

        let observed_woke = std::cell::Cell::new(false);
        runner.step(|_step, woke| {
            observed_woke.set(woke);
            true
        });
        assert!(observed_woke.get());
    }
}
