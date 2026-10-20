//! Spike evidence for #1640: hand-rolled `extern "C"` functions over
//! quadraui's real descriptor-store, callback-vtable, and run/step/wake
//! shapes, with a cbindgen-generated header (`include/quadraui_ffi_cbindgen_spike.h`,
//! written by `build.rs` on every build) as the C ABI surface.
//!
//! This is throwaway investigation for the real `quadraui-ffi` crate (#1123),
//! not shippable binding code. Three capabilities, each backed by the real
//! quadraui type it claims to exercise:
//!
//! 1. **Opaque handle store** — `quadra_store_*` wraps a real
//!    [`quadraui::descriptor_store::DescriptorStore`]. Descriptors cross the
//!    boundary as JSON strings; the FFI-visible handle is a plain `u64`
//!    (not quadraui's own `Handle<T>`, which isn't `repr(C)` and isn't meant
//!    to leak past the crate boundary) mapped internally to a real
//!    `Handle<RowDescriptor>`.
//! 2. **Callback vtable** — `QuadraCallbacks` carries one `extern "C"` function
//!    pointer, `handle`, taking an event JSON string and returning a reaction
//!    JSON string, plus a matching `free_reaction` pointer. The runner (#3
//!    below) parses the reaction JSON into a real
//!    [`quadraui::runner::Reaction`] *without* taking ownership of the
//!    pointer — `free_reaction` is called afterwards so whichever side
//!    allocated the buffer is also the side that frees it. (An earlier
//!    version of this spike had Rust call `CString::from_raw` directly on a
//!    pointer the foreign side had to produce via `malloc`, which only
//!    works when the foreign allocator and Rust's happen to agree; see
//!    `FINDINGS.md` for why that shape is a trap, not a shortcut.)
//! 3. **run/step/wake** — `quadra_runner_step` maps its outcome onto the real
//!    [`quadraui::runner::StepOutcome`]; `quadra_runner_run` loops `step`
//!    until exit. Waking is a *separate* opaque `QuadraWaker` handle
//!    (`quadra_runner_waker` clones it out of a runner), not a raw pointer
//!    to the runner itself — mirroring
//!    [`quadraui::backend::Backend::waker`]'s detachable,
//!    `Arc<dyn Fn + Send + Sync>`-shaped, cross-thread wake contract, and
//!    avoiding materializing a `&QuadraRunner` while `quadra_runner_step`
//!    may hold `&mut QuadraRunner` over the same memory.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use quadraui::descriptor_store::{DescriptorStore, Handle};
use quadraui::runner::{Reaction, StepOutcome};
use serde::{Deserialize, Serialize};

/// The descriptor shape stored per handle. Deliberately simple — a `label`
/// and a `value` — because this spike evaluates ABI *tooling*, not a real
/// widget descriptor's field list.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RowDescriptor {
    label: String,
    value: f64,
}

/// Opaque handle store: real [`DescriptorStore`] underneath, plain `u64`
/// handles at the FFI boundary.
pub struct QuadraStore {
    descriptors: DescriptorStore<RowDescriptor>,
    handles: HashMap<u64, Handle<RowDescriptor>>,
    next_id: u64,
}

impl QuadraStore {
    fn new() -> Self {
        Self {
            descriptors: DescriptorStore::new(),
            handles: HashMap::new(),
            next_id: 1,
        }
    }
}

/// Create a new, empty store. Free with [`quadra_store_free`].
#[no_mangle]
pub extern "C" fn quadra_store_new() -> *mut QuadraStore {
    Box::into_raw(Box::new(QuadraStore::new()))
}

/// Destroy a store created by [`quadra_store_new`]. `store` may be null (no-op).
///
/// # Safety
/// `store` must be a pointer returned by [`quadra_store_new`] that hasn't
/// already been freed, or null.
#[no_mangle]
pub unsafe extern "C" fn quadra_store_free(store: *mut QuadraStore) {
    if store.is_null() {
        return;
    }
    // SAFETY: `store` was produced by `Box::into_raw` in `quadra_store_new`
    // and the caller promises not to use it again after this call.
    unsafe {
        drop(Box::from_raw(store));
    }
}

/// Parse a null-terminated UTF-8 JSON C string into a `RowDescriptor`.
/// Returns `None` on a null pointer, invalid UTF-8, or invalid JSON.
fn parse_descriptor(json: *const c_char) -> Option<RowDescriptor> {
    if json.is_null() {
        return None;
    }
    // SAFETY: caller promises `json` is a valid null-terminated C string
    // for the duration of this call (standard C-ABI string convention).
    let s = unsafe { CStr::from_ptr(json) }.to_str().ok()?;
    serde_json::from_str(s).ok()
}

/// Insert a descriptor (JSON-encoded `{"label": "...", "value": ...}`).
/// Returns a non-zero opaque handle on success, `0` on parse failure or a
/// null `store`/`descriptor_json`.
///
/// # Safety
/// `store` must be a live pointer from [`quadra_store_new`], and
/// `descriptor_json` must be a null-terminated C string (or null).
#[no_mangle]
pub unsafe extern "C" fn quadra_store_insert(
    store: *mut QuadraStore,
    descriptor_json: *const c_char,
) -> u64 {
    let Some(descriptor) = parse_descriptor(descriptor_json) else {
        return 0;
    };
    if store.is_null() {
        return 0;
    }
    // SAFETY: caller promises `store` is a live pointer from `quadra_store_new`.
    let store = unsafe { &mut *store };

    let handle = store.descriptors.insert(descriptor);
    let id = store.next_id;
    store.next_id += 1;
    store.handles.insert(id, handle);
    id
}

/// Overwrite the descriptor at `handle` in place. Returns `true` iff
/// `handle` is live and `descriptor_json` parses.
///
/// # Safety
/// `store` must be a live pointer from [`quadra_store_new`], and
/// `descriptor_json` must be a null-terminated C string (or null).
#[no_mangle]
pub unsafe extern "C" fn quadra_store_update(
    store: *mut QuadraStore,
    handle: u64,
    descriptor_json: *const c_char,
) -> bool {
    let Some(new_value) = parse_descriptor(descriptor_json) else {
        return false;
    };
    if store.is_null() {
        return false;
    }
    // SAFETY: see `quadra_store_insert`.
    let store = unsafe { &mut *store };

    let Some(&real_handle) = store.handles.get(&handle) else {
        return false;
    };
    store
        .descriptors
        .update(real_handle, |slot| *slot = new_value)
}

/// Remove the descriptor at `handle`. Returns `true` iff `handle` was live.
///
/// # Safety
/// `store` must be a live pointer from [`quadra_store_new`], or null.
#[no_mangle]
pub unsafe extern "C" fn quadra_store_remove(store: *mut QuadraStore, handle: u64) -> bool {
    if store.is_null() {
        return false;
    }
    // SAFETY: see `quadra_store_insert`.
    let store = unsafe { &mut *store };

    let Some(real_handle) = store.handles.remove(&handle) else {
        return false;
    };
    store.descriptors.remove(real_handle).is_some()
}

/// Number of live descriptors in `store`. Returns `0` for a null `store`.
///
/// # Safety
/// `store` must be a live pointer from [`quadra_store_new`], or null.
#[no_mangle]
pub unsafe extern "C" fn quadra_store_len(store: *const QuadraStore) -> usize {
    if store.is_null() {
        return 0;
    }
    // SAFETY: see `quadra_store_insert`.
    unsafe { &*store }.descriptors.len()
}

/// One callback in the vtable: foreign code receives an event as JSON,
/// returns a reaction as JSON. The returned pointer is **never freed by
/// Rust** — `quadraui_ffi_cbindgen_spike` only reads it (via `CStr::from_ptr`,
/// borrowing) and then calls `free_reaction` with the same pointer and
/// `user_data`, so the side that allocated the buffer is also the side that
/// frees it, however it chooses to (its own `malloc`/`free`, a language
/// runtime's GC, a reused static buffer, ...). This sidesteps the
/// cross-allocator trap a single `handle`-owns-the-return shape falls into
/// (see `FINDINGS.md`).
#[repr(C)]
pub struct QuadraCallbacks {
    /// Opaque context pointer passed back on every `handle`/`free_reaction`
    /// call — the standard C-ABI pattern for closures without a Rust
    /// closure type.
    pub user_data: *mut c_void,
    pub handle: extern "C" fn(user_data: *mut c_void, event_json: *const c_char) -> *mut c_char,
    /// Called once per `handle` call whose return was non-null, immediately
    /// after Rust is done reading it. Must accept exactly the pointer
    /// `handle` returned.
    pub free_reaction: extern "C" fn(user_data: *mut c_void, reaction_json: *mut c_char),
}

/// Wire-format mirror of [`Reaction`]'s variants the foreign callback can
/// express in JSON. quadraui's own `Reaction` doesn't derive `Deserialize`
/// (it isn't meant to cross a JSON boundary in-tree), so this spike defines
/// the minimal tagged shape and maps it onto the real type below —
/// `quadra_runner_step`'s whole point is proving that mapping is clean.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireReaction {
    Continue,
    Redraw,
    RedrawAfter { after_ms: u64 },
    Exit,
}

impl From<WireReaction> for Reaction {
    fn from(wire: WireReaction) -> Self {
        match wire {
            WireReaction::Continue => Reaction::Continue,
            WireReaction::Redraw => Reaction::Redraw,
            WireReaction::RedrawAfter { after_ms } => {
                Reaction::RedrawAfter(std::time::Duration::from_millis(after_ms))
            }
            WireReaction::Exit => Reaction::Exit,
        }
    }
}

/// `run`/`step`/`wake` control surface. Not a real event loop — it drives
/// the callback with a synthetic event each step — but it exercises the
/// real [`Reaction`] → [`StepOutcome`] contract a real runner enforces.
pub struct QuadraRunner {
    callbacks: QuadraCallbacks,
    woken: Arc<AtomicBool>,
    exited: bool,
    step_count: u64,
}

/// Create a runner around `callbacks`. Free with [`quadra_runner_free`].
#[no_mangle]
pub extern "C" fn quadra_runner_new(callbacks: QuadraCallbacks) -> *mut QuadraRunner {
    Box::into_raw(Box::new(QuadraRunner {
        callbacks,
        woken: Arc::new(AtomicBool::new(false)),
        exited: false,
        step_count: 0,
    }))
}

/// Destroy a runner created by [`quadra_runner_new`]. `runner` may be null.
///
/// # Safety
/// `runner` must be a pointer returned by [`quadra_runner_new`] that hasn't
/// already been freed, or null.
#[no_mangle]
pub unsafe extern "C" fn quadra_runner_free(runner: *mut QuadraRunner) {
    if runner.is_null() {
        return;
    }
    // SAFETY: see `quadra_store_free`.
    unsafe {
        drop(Box::from_raw(runner));
    }
}

/// Mirror of [`StepOutcome`] at the FFI boundary — a plain `u32` tag, since
/// `#[non_exhaustive]` Rust enums aren't `repr(C)`-stable to hand to C.
#[repr(u32)]
pub enum QuadraStepOutcome {
    Continue = 0,
    Exited = 1,
}

fn step_outcome_to_ffi(outcome: StepOutcome) -> QuadraStepOutcome {
    match outcome {
        StepOutcome::Continue => QuadraStepOutcome::Continue,
        StepOutcome::Exited => QuadraStepOutcome::Exited,
        // `StepOutcome` is `#[non_exhaustive]`: a future quadraui minor
        // release may add a variant this spike predates. Treat anything
        // unrecognized as "still running" rather than failing to compile —
        // exactly the forward-compat behavior a frozen C ABI needs.
        _ => QuadraStepOutcome::Continue,
    }
}

/// Non-blocking single pass: synthesizes one event, invokes the callback,
/// maps its JSON reaction onto the real [`Reaction`] type, and returns the
/// matching real [`StepOutcome`] (mirrored as [`QuadraStepOutcome`]).
/// Idempotent after exit, matching `TuiRunner::step`'s contract.
///
/// # Safety
/// `runner` must be a live pointer from [`quadra_runner_new`], or null.
#[no_mangle]
pub unsafe extern "C" fn quadra_runner_step(runner: *mut QuadraRunner) -> QuadraStepOutcome {
    if runner.is_null() {
        return QuadraStepOutcome::Exited;
    }
    // SAFETY: caller promises `runner` is a live pointer from `quadra_runner_new`.
    let runner = unsafe { &mut *runner };

    if runner.exited {
        return QuadraStepOutcome::Exited;
    }

    let woken = runner.woken.swap(false, Ordering::SeqCst);
    let event_json = if woken {
        format!(r#"{{"type":"woke","step":{}}}"#, runner.step_count)
    } else {
        format!(r#"{{"type":"tick","step":{}}}"#, runner.step_count)
    };
    runner.step_count += 1;

    let event_cstring = CString::new(event_json).expect("JSON has no interior NUL");
    let reaction_ptr =
        (runner.callbacks.handle)(runner.callbacks.user_data, event_cstring.as_ptr());

    let reaction = if reaction_ptr.is_null() {
        Reaction::Continue
    } else {
        // SAFETY: `reaction_ptr` is a non-null pointer the callback just
        // returned per the `QuadraCallbacks` contract; it stays valid at
        // least until `free_reaction` is called below, so borrowing it
        // here (not taking ownership) is sound.
        let reaction = unsafe { CStr::from_ptr(reaction_ptr) }
            .to_str()
            .ok()
            .and_then(|s| serde_json::from_str::<WireReaction>(s).ok())
            .map(Reaction::from)
            .unwrap_or(Reaction::Continue);
        (runner.callbacks.free_reaction)(runner.callbacks.user_data, reaction_ptr);
        reaction
    };

    let outcome = match reaction {
        Reaction::Exit => {
            runner.exited = true;
            StepOutcome::Exited
        }
        Reaction::Continue | Reaction::Redraw | Reaction::RedrawAfter(_) => StepOutcome::Continue,
        // `Reaction` is likewise `#[non_exhaustive]`; same forward-compat
        // reasoning as `step_outcome_to_ffi` above.
        _ => StepOutcome::Continue,
    };
    step_outcome_to_ffi(outcome)
}

/// Blocking `run`: calls [`quadra_runner_step`] until it reports
/// [`QuadraStepOutcome::Exited`]. This is the third name the issue asks for
/// alongside `step`/`wake`; it's a thin loop here, not a real event-sourced
/// scheduler, because the interesting FFI question it raises — a blocking
/// `run` whose every iteration calls back into foreign code, so a foreign
/// callback that re-enters `quadra_runner_step`/`quadra_runner_run` on the
/// *same* runner would alias `&mut QuadraRunner` — is a correctness
/// question for the real #1123 implementation, not one this throwaway loop
/// answers. See `FINDINGS.md`.
///
/// # Safety
/// `runner` must be a live pointer from [`quadra_runner_new`], or null (in
/// which case this returns immediately).
#[no_mangle]
pub unsafe extern "C" fn quadra_runner_run(runner: *mut QuadraRunner) {
    loop {
        // SAFETY: forwarded from this function's own contract on each
        // iteration; `runner` is not reassigned.
        match unsafe { quadra_runner_step(runner) } {
            QuadraStepOutcome::Exited => return,
            QuadraStepOutcome::Continue => {}
        }
    }
}

/// A detachable, cross-thread-safe handle that can wake a [`QuadraRunner`]'s
/// next [`quadra_runner_step`]/[`quadra_runner_run`] call. Mirrors
/// [`quadraui::backend::Backend::waker`]'s `Arc<dyn Fn + Send + Sync>`
/// shape: cloning the underlying flag out of the runner (rather than
/// exposing `*const QuadraRunner` to waking code directly) means a
/// concurrent wake never has to materialize a `&QuadraRunner` reference
/// while [`quadra_runner_step`] may hold `&mut QuadraRunner` over the same
/// allocation — the two pointers share no memory.
pub struct QuadraWaker {
    flag: Arc<AtomicBool>,
}

/// Clone a waker out of `runner`. Safe to call from any thread; the
/// returned `QuadraWaker` outlives `runner` and must be freed with
/// [`quadra_waker_free`].
///
/// # Safety
/// `runner` must be a live pointer from [`quadra_runner_new`].
#[no_mangle]
pub unsafe extern "C" fn quadra_runner_waker(runner: *const QuadraRunner) -> *mut QuadraWaker {
    // SAFETY: caller promises `runner` is live; this only reads the `Arc`
    // pointer field to clone it, never materializing `&QuadraRunner` for
    // longer than this statement.
    let flag = unsafe { &*runner }.woken.clone();
    Box::into_raw(Box::new(QuadraWaker { flag }))
}

/// Wake the runner this waker was cloned from, from any thread. Mirrors
/// `Backend::waker`'s contract: the next `step`/`run` call observes the
/// wake unconditionally, regardless of which thread set it. Safe to call
/// after the runner has exited or been freed (the `Arc<AtomicBool>` keeps
/// the flag itself alive independently of the runner).
///
/// # Safety
/// `waker` must be a live pointer from [`quadra_runner_waker`], or null.
#[no_mangle]
pub unsafe extern "C" fn quadra_waker_wake(waker: *const QuadraWaker) {
    if waker.is_null() {
        return;
    }
    // SAFETY: `waker` is a live pointer per the caller's contract; this
    // only touches the atomic flag, never the `QuadraRunner` itself.
    unsafe { &*waker }.flag.store(true, Ordering::SeqCst);
}

/// Destroy a waker created by [`quadra_runner_waker`]. `waker` may be null.
///
/// # Safety
/// `waker` must be a pointer returned by [`quadra_runner_waker`] that
/// hasn't already been freed, or null.
#[no_mangle]
pub unsafe extern "C" fn quadra_waker_free(waker: *mut QuadraWaker) {
    if waker.is_null() {
        return;
    }
    // SAFETY: see `quadra_store_free`.
    unsafe {
        drop(Box::from_raw(waker));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::Mutex;

    #[test]
    fn store_insert_update_remove_round_trips_through_json() {
        // SAFETY: `store` is a live pointer from `quadra_store_new` for the
        // whole block; every C string is a valid, live `CString`.
        unsafe {
            let store = quadra_store_new();
            let json = CString::new(r#"{"label":"row-0","value":1.5}"#).unwrap();
            let handle = quadra_store_insert(store, json.as_ptr());
            assert_ne!(handle, 0);
            assert_eq!(quadra_store_len(store), 1);

            let update = CString::new(r#"{"label":"row-0","value":2.5}"#).unwrap();
            assert!(quadra_store_update(store, handle, update.as_ptr()));

            assert!(quadra_store_remove(store, handle));
            assert_eq!(quadra_store_len(store), 0);
            // Removing twice is a clean `false`, not a crash.
            assert!(!quadra_store_remove(store, handle));

            quadra_store_free(store);
        }
    }

    #[test]
    fn insert_rejects_malformed_json_and_null() {
        // SAFETY: `store` is a live pointer from `quadra_store_new`; a null
        // `descriptor_json` is an explicitly documented valid input.
        unsafe {
            let store = quadra_store_new();
            let bad = CString::new("not json").unwrap();
            assert_eq!(quadra_store_insert(store, bad.as_ptr()), 0);
            assert_eq!(quadra_store_insert(store, std::ptr::null()), 0);
            quadra_store_free(store);
        }
    }

    // All of these test callbacks allocate their reaction string with
    // `CString::into_raw` and free it the same way in `free_reaction` —
    // both sides are this same Rust crate here, so that's a consistent
    // allocator pairing. The point of the `handle`/`free_reaction` split
    // (see `QuadraCallbacks`'s doc comment) is that a *real* foreign
    // caller is free to pick any pairing of its own; `python/client.py`
    // demonstrates a non-Rust-allocator pairing end-to-end.
    extern "C" fn free_via_cstring_from_raw(_user_data: *mut c_void, s: *mut c_char) {
        // SAFETY: every callback below hands back a pointer from
        // `CString::into_raw`, and `quadra_runner_step` calls this exactly
        // once per non-null `handle` return, per the documented contract.
        unsafe {
            drop(CString::from_raw(s));
        }
    }

    extern "C" fn echo_continue(
        _user_data: *mut c_void,
        _event_json: *const c_char,
    ) -> *mut c_char {
        CString::new(r#"{"type":"continue"}"#).unwrap().into_raw()
    }

    extern "C" fn echo_exit(_user_data: *mut c_void, _event_json: *const c_char) -> *mut c_char {
        CString::new(r#"{"type":"exit"}"#).unwrap().into_raw()
    }

    /// Records the `type` field of every event it receives into the
    /// `Mutex<Vec<String>>` pointed to by `user_data`, then always reacts
    /// `continue`. Used to make the wake tests falsifiable: a no-op `wake`
    /// would leave every recorded type as `"tick"`.
    extern "C" fn record_event_type(
        user_data: *mut c_void,
        event_json: *const c_char,
    ) -> *mut c_char {
        // SAFETY: callers below pass a live `*const Mutex<Vec<String>>`
        // (leaked for the test's duration) as `user_data`, and a valid
        // null-terminated `event_json` per the `handle` contract.
        let events = unsafe { &*(user_data as *const Mutex<Vec<String>>) };
        let json = unsafe { CStr::from_ptr(event_json) }.to_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        events
            .lock()
            .unwrap()
            .push(parsed["type"].as_str().unwrap().to_string());
        CString::new(r#"{"type":"continue"}"#).unwrap().into_raw()
    }

    #[test]
    fn runner_step_continues_until_callback_returns_exit() {
        // SAFETY: `runner` is a live pointer from `quadra_runner_new` for the
        // whole block.
        unsafe {
            let runner = quadra_runner_new(QuadraCallbacks {
                user_data: std::ptr::null_mut(),
                handle: echo_continue,
                free_reaction: free_via_cstring_from_raw,
            });

            for _ in 0..3 {
                assert!(matches!(
                    quadra_runner_step(runner),
                    QuadraStepOutcome::Continue
                ));
            }

            quadra_runner_free(runner);
        }
    }

    #[test]
    fn runner_step_exits_and_stays_exited() {
        // SAFETY: `runner` is a live pointer from `quadra_runner_new` for the
        // whole block.
        unsafe {
            let runner = quadra_runner_new(QuadraCallbacks {
                user_data: std::ptr::null_mut(),
                handle: echo_exit,
                free_reaction: free_via_cstring_from_raw,
            });

            assert!(matches!(
                quadra_runner_step(runner),
                QuadraStepOutcome::Exited
            ));
            // Idempotent: calling again after exit is a safe no-op.
            assert!(matches!(
                quadra_runner_step(runner),
                QuadraStepOutcome::Exited
            ));

            quadra_runner_free(runner);
        }
    }

    #[test]
    fn runner_run_loops_until_exit() {
        // SAFETY: `runner` is a live pointer from `quadra_runner_new` for
        // the whole block; `quadra_runner_run` returns once `echo_exit`
        // reports exit, so this doesn't hang.
        unsafe {
            let runner = quadra_runner_new(QuadraCallbacks {
                user_data: std::ptr::null_mut(),
                handle: echo_exit,
                free_reaction: free_via_cstring_from_raw,
            });

            quadra_runner_run(runner);
            // `run` only returns after exit; a further manual `step` stays
            // exited, same as the non-blocking path.
            assert!(matches!(
                quadra_runner_step(runner),
                QuadraStepOutcome::Exited
            ));

            quadra_runner_free(runner);
        }
    }

    #[test]
    fn wake_is_observed_as_the_woke_event_tag_on_the_next_step() {
        // SAFETY: `runner` is a live pointer from `quadra_runner_new` for the
        // whole block; `events` is a live `Mutex<Vec<String>>` for at least
        // as long, leaked deliberately so its address is stable `user_data`.
        unsafe {
            let events: *const Mutex<Vec<String>> =
                Box::into_raw(Box::new(Mutex::new(Vec::new())));
            let runner = quadra_runner_new(QuadraCallbacks {
                user_data: events as *mut c_void,
                handle: record_event_type,
                free_reaction: free_via_cstring_from_raw,
            });

            assert!(matches!(
                quadra_runner_step(runner),
                QuadraStepOutcome::Continue
            ));
            let waker = quadra_runner_waker(runner);
            quadra_waker_wake(waker);
            assert!(matches!(
                quadra_runner_step(runner),
                QuadraStepOutcome::Continue
            ));

            assert_eq!(&*(*events).lock().unwrap(), &["tick", "woke"]);

            quadra_waker_free(waker);
            quadra_runner_free(runner);
            drop(Box::from_raw(events as *mut Mutex<Vec<String>>));
        }
    }

    #[test]
    fn waker_wakes_from_a_different_thread_than_step() {
        // SAFETY: `runner` and `events` are live for the whole block, and
        // `waker` is only ever touched through the atomic it wraps, so
        // sharing it with the spawned thread (via the raw pointer's
        // address, reconstructed as the same `QuadraWaker`) is sound: it
        // never aliases `&mut QuadraRunner`.
        unsafe {
            let events: *const Mutex<Vec<String>> =
                Box::into_raw(Box::new(Mutex::new(Vec::new())));
            let runner = quadra_runner_new(QuadraCallbacks {
                user_data: events as *mut c_void,
                handle: record_event_type,
                free_reaction: free_via_cstring_from_raw,
            });
            let waker = quadra_runner_waker(runner);

            // Prove this is a genuine cross-thread wake, not a same-thread
            // call that happens to work: spawn a thread, let it wake the
            // runner, and join it *before* stepping on the main thread —
            // so the wake provably came from a thread other than the one
            // that observes it.
            let waker_addr = waker as usize;
            // SAFETY (for the whole closure below): `waker` outlives this
            // thread (freed only after `join` below), and only its atomic
            // flag is touched. The closure body is still lexically inside
            // this function's outer `unsafe` block, so no inner `unsafe` is
            // needed here.
            let worker = std::thread::spawn(move || {
                let waker = waker_addr as *const QuadraWaker;
                quadra_waker_wake(waker);
            });
            worker.join().unwrap();

            assert!(matches!(
                quadra_runner_step(runner),
                QuadraStepOutcome::Continue
            ));
            assert_eq!(&*(*events).lock().unwrap(), &["woke"]);

            quadra_waker_free(waker);
            quadra_runner_free(runner);
            drop(Box::from_raw(events as *mut Mutex<Vec<String>>));
        }
    }
}
