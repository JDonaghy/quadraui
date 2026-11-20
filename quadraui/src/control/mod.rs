//! Backend-neutral control channel: lets a thread that does not own the
//! event loop drive a running [`crate::AppLogic`] app the same way a
//! user's mouse/keyboard would, without knowing which backend (TUI, GTK,
//! macOS, Windows) is running it.
//!
//! [`channel`] returns a cloneable, `Send` [`ControlHandle`] for the
//! requesting side plus the [`ControlPort`] the owning runner keeps on its
//! own thread. A handle submits a [`ControlRequest`] and blocks (bounded
//! by a timeout) for the matching [`ControlReply`]; [`ControlPort::service`]
//! drains whatever is queued and answers each one against the app's
//! current state — called once per loop iteration by the runner that
//! holds it. This module has no backend of its own, so `service` takes
//! the owning runner's `dispatch` callback and threads every synthesized
//! [`UiEvent`] through it rather than calling [`crate::AppLogic::handle`]
//! directly — on TUI that callback is backend translation
//! (`TuiBackend::translate_injected`) followed by the shared
//! pre-processing pipeline (`crate::runtime::preprocess_event`), the exact
//! two stages real crossterm input goes through. See
//! `tui::run::TuiRunner::attach_control` for the TUI wiring; GTK, macOS
//! and Windows wire their own runners in their own issues.
//!
//! # Levels
//!
//! [`ControlRequest::Snapshot`] and [`ControlRequest::Act`] are "Level 1":
//! UI-level control built entirely on `crate::a11y::tree` and
//! `crate::a11y::route_action`, so every `AppLogic` app gets them for
//! free. [`ControlRequest::Key`] and [`ControlRequest::Type`] synthesize
//! raw input the same way [`crate::tui::testing::TuiDriver`]'s
//! `press`/`type_char` do — both end up going through
//! [`crate::tui::testing::TuiDriver::dispatch`]'s pipeline when the owning
//! runner is `TuiRunner`. [`ControlRequest::Command`] is reserved for
//! "Level 2" app-defined named commands — this module always answers it
//! with an "unknown command" error; a later addition teaches
//! [`ControlPort::service`] to look a name up in an app-supplied registry.
//!
//! # Waking the loop
//!
//! Submitting a request calls the owning backend's [`crate::Backend::waker`]
//! with an empty [`crate::event::UserPayload`], so a runner blocked in
//! `wait_events`/`pump` wakes promptly instead of waiting out its idle-poll
//! ceiling. The payload itself carries nothing meaningful —
//! [`ControlPort::service`] drains its own channel independently of
//! `UiEvent::User` delivery, so any wake (native event, timer, another
//! control request) is enough to make it progress.
//!
//! # Errors, not panics
//!
//! A request against an id `crate::a11y::route_action` doesn't recognise
//! is a logged no-op that still answers [`ControlReply::Ack`] — the same
//! "safe, logged no-op" contract `route_action` itself documents. A
//! request this module cannot service at all (no captured frame yet, an
//! unknown command) answers [`ControlReply::Error`] instead. Nothing here
//! ever panics on untrusted input.

use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::a11y::{route_action, A11yAction, A11yActionValue};
use crate::backend::Backend;
use crate::event::{Key, UiEvent, UserPayload};
use crate::runner::{AppLogic, Reaction};
use crate::types::Modifiers;
use crate::A11yNodeId;

/// How long [`ControlHandle::send`] waits for a reply before giving up,
/// unless overridden via [`ControlHandle::with_timeout`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// A request submitted through a [`ControlHandle`].
///
/// `#[non_exhaustive]` so a future variant doesn't break a consumer's
/// `match` — construct one of these through [`ControlHandle`]'s methods
/// rather than naming the variant directly where possible.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ControlRequest {
    /// The current [`crate::A11yTree`], as its stable JSON form —
    /// see [`ControlHandle::snapshot`].
    Snapshot,
    /// Route an assistive-technology-style action against `node` — see
    /// [`ControlHandle::act`].
    Act {
        /// The target node, from a prior [`Self::Snapshot`]'s JSON `id`
        /// field.
        node: A11yNodeId,
        /// The action to perform.
        action: A11yAction,
        /// The payload a [`A11yAction::SetValue`] action carries.
        value: Option<A11yActionValue>,
    },
    /// Synthesize one key press — see [`ControlHandle::key`].
    Key {
        /// The key.
        key: Key,
        /// Modifier keys held alongside it.
        modifiers: Modifiers,
    },
    /// Synthesize a key press per character, left to right — see
    /// [`ControlHandle::type_text`].
    Type(String),
    /// An app-defined named command — see [`ControlHandle::command`].
    /// Reserved: [`ControlPort::service`] always answers this with
    /// [`ControlReply::Error`] today.
    Command {
        /// The command's name.
        name: String,
        /// Arbitrary JSON arguments, shape defined by the command.
        args: Value,
    },
}

/// A reply to a [`ControlRequest`].
///
/// `#[non_exhaustive]` for the same reason as [`ControlRequest`].
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ControlReply {
    /// A successful reply that carries data, e.g. [`ControlRequest::Snapshot`]'s
    /// JSON tree.
    Value(Value),
    /// A successful reply with nothing to report.
    Ack,
    /// The request could not be serviced — bad input is logged and
    /// answered here, never panicked on.
    Error(String),
}

/// Why a [`ControlHandle`] request did not complete.
#[non_exhaustive]
#[derive(Debug)]
pub enum ControlError {
    /// No reply arrived within the handle's timeout.
    Timeout,
    /// The runner has exited (its [`ControlPort`] was dropped) either
    /// before or while this request was in flight.
    RunnerGone,
    /// The runner answered with [`ControlReply::Error`].
    Failed(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::Timeout => write!(f, "control request timed out"),
            ControlError::RunnerGone => write!(f, "control runner has exited"),
            ControlError::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ControlError {}

/// One request plus the reply channel its answer goes back on.
struct Envelope {
    request: ControlRequest,
    reply_tx: Sender<ControlReply>,
}

/// Cloneable, `Send` handle a thread that does not own the event loop uses
/// to submit a [`ControlRequest`] and block for its [`ControlReply`].
///
/// Cloning is cheap (an `mpsc::Sender` clone plus an `Arc` clone) and every
/// clone shares the same underlying [`ControlPort`] — there is exactly one
/// receiver per [`channel`] call, however many handles are cloned from it.
#[derive(Clone)]
pub struct ControlHandle {
    tx: Sender<Envelope>,
    waker: Arc<dyn Fn(UserPayload) + Send + Sync>,
    timeout: Duration,
}

impl fmt::Debug for ControlHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlHandle").finish_non_exhaustive()
    }
}

impl ControlHandle {
    /// Returns a handle that waits up to `timeout` for a reply instead of
    /// [`DEFAULT_TIMEOUT`]. Does not affect other clones.
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            timeout,
            ..self.clone()
        }
    }

    /// The current [`crate::A11yTree`] — the same stable JSON form
    /// documented on that type — built from whatever the runner most
    /// recently painted. Fails if the backend has no captured frame yet
    /// (live-frame capture, [`crate::frame::set_frame_capture`], must be
    /// enabled first).
    pub fn snapshot(&self) -> Result<Value, ControlError> {
        match self.send(ControlRequest::Snapshot)? {
            ControlReply::Value(v) => Ok(v),
            ControlReply::Error(e) => Err(ControlError::Failed(e)),
            ControlReply::Ack => Err(ControlError::Failed(
                "snapshot: runner answered Ack instead of a tree".into(),
            )),
        }
    }

    /// Route an assistive-technology-style `action` against `node` (a node
    /// id from a prior [`Self::snapshot`]'s JSON `id` field) and feed the
    /// resulting [`UiEvent`]s through the owning runner's dispatch
    /// pipeline (see [`ControlPort::service`]) — the same path real input
    /// takes, ending in [`crate::AppLogic::handle`]. See
    /// `crate::a11y::route_action` for exactly what each action/role
    /// combination produces. A `node`/`action` combination `route_action`
    /// doesn't recognise is a safe, logged no-op: this still answers `Ok`.
    pub fn act(
        &self,
        node: A11yNodeId,
        action: A11yAction,
        value: Option<A11yActionValue>,
    ) -> Result<(), ControlError> {
        self.ack(ControlRequest::Act {
            node,
            action,
            value,
        })
    }

    /// Synthesize one key press as a [`UiEvent::KeyPressed`], fed through
    /// the owning runner's dispatch pipeline exactly like real keyboard
    /// input — on `TuiRunner` that's
    /// [`crate::tui::testing::TuiDriver::dispatch`]'s two stages (backend
    /// translation, then the shared pre-processing pipeline), so Tab
    /// focus cycling, declared accelerators, and Ctrl-C/V/A all fire the
    /// same way they would for a user at the keyboard.
    pub fn key(&self, key: Key, modifiers: Modifiers) -> Result<(), ControlError> {
        self.ack(ControlRequest::Key { key, modifiers })
    }

    /// Synthesize a key press per character of `text`, left to right, no
    /// modifiers — the same thing [`crate::tui::testing::TuiDriver::type_char`]
    /// does per call, batched into one request and dispatched through the
    /// same pipeline [`Self::key`] documents.
    pub fn type_text(&self, text: impl Into<String>) -> Result<(), ControlError> {
        self.ack(ControlRequest::Type(text.into()))
    }

    /// An app-defined named command. Reserved: every runner answers this
    /// with [`ControlError::Failed`] ("unknown command") today — no
    /// command registry exists yet.
    pub fn command(&self, name: impl Into<String>, args: Value) -> Result<Value, ControlError> {
        match self.send(ControlRequest::Command {
            name: name.into(),
            args,
        })? {
            ControlReply::Value(v) => Ok(v),
            ControlReply::Ack => Ok(Value::Null),
            ControlReply::Error(e) => Err(ControlError::Failed(e)),
        }
    }

    /// Submit `request` to a [`ControlReply::Ack`]-shaped request, mapping
    /// a data-carrying reply to success too (a future command registry may
    /// answer `Act`-shaped requests with data; that's still success here).
    fn ack(&self, request: ControlRequest) -> Result<(), ControlError> {
        match self.send(request)? {
            ControlReply::Ack | ControlReply::Value(_) => Ok(()),
            ControlReply::Error(e) => Err(ControlError::Failed(e)),
        }
    }

    /// Submit `request`, wake the runner, and block for up to this
    /// handle's timeout for the reply.
    fn send(&self, request: ControlRequest) -> Result<ControlReply, ControlError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(Envelope { request, reply_tx })
            .map_err(|_| ControlError::RunnerGone)?;
        (self.waker)(UserPayload::new(()));
        reply_rx
            .recv_timeout(self.timeout)
            .map_err(|err| match err {
                RecvTimeoutError::Timeout => ControlError::Timeout,
                RecvTimeoutError::Disconnected => ControlError::RunnerGone,
            })
    }
}

/// Receiver half of a [`channel`] pair, held by the runner on the thread
/// that owns the event loop. Not `Clone` — exactly one runner services a
/// given channel, however many [`ControlHandle`] clones submit to it.
pub struct ControlPort {
    rx: Receiver<Envelope>,
}

/// Build a control channel: a cloneable [`ControlHandle`] for other
/// threads, and the [`ControlPort`] the owning runner services once per
/// loop iteration via [`ControlPort::service`].
///
/// `waker` is normally the backend's own [`crate::Backend::waker`] —
/// called by [`ControlHandle::send`] after every request so a runner
/// blocked waiting for native input wakes up promptly to service it.
pub fn channel(waker: Arc<dyn Fn(UserPayload) + Send + Sync>) -> (ControlHandle, ControlPort) {
    let (tx, rx) = mpsc::channel();
    (
        ControlHandle {
            tx,
            waker,
            timeout: DEFAULT_TIMEOUT,
        },
        ControlPort { rx },
    )
}

impl ControlPort {
    /// Drain every request queued since the last call and answer each one
    /// against `app`/`backend`'s current state, feeding every
    /// [`UiEvent`] a request synthesizes through `dispatch` — the owning
    /// runner's own event pipeline, not a hard-coded
    /// [`crate::AppLogic::handle`] call, so a control-synthesized
    /// [`ControlRequest::Key`]/[`ControlRequest::Type`] behaves exactly
    /// like real input on whichever backend is running: on `TuiRunner`,
    /// `dispatch` is backend translation
    /// ([`crate::tui::testing::TuiDriver::dispatch`]'s first stage)
    /// followed by the shared pre-processing pipeline, so Tab cycling,
    /// accelerators and Ctrl-C/V/A all fire. Merges every [`Reaction`]
    /// `dispatch` returns (via [`Reaction::merge`]) into the one the
    /// caller's own loop iteration should act on. Returns
    /// [`Reaction::Continue`] when nothing was queued.
    ///
    /// The whole batch queued so far is collected before any of it is
    /// processed, so a request submitted *while* this call is draining
    /// (e.g. a `snapshot` a requester sends right after an `act` it just
    /// got acknowledged) lands in the *next* `service` call instead of
    /// this one — after the runner's next repaint, not racing it.
    pub fn service<A, B>(
        &self,
        app: &mut A,
        backend: &mut B,
        mut dispatch: impl FnMut(UiEvent, &mut A, &mut B) -> Reaction,
    ) -> Reaction
    where
        A: AppLogic,
        B: Backend,
    {
        let mut reaction = Reaction::Continue;
        let batch: Vec<Envelope> = self.rx.try_iter().collect();
        for envelope in batch {
            let (reply, r) = service_one(envelope.request, app, backend, &mut dispatch);
            reaction = reaction.merge(r);
            // The requester may have already given up (timed out) — a
            // reply nobody is listening for isn't this loop's problem.
            let _ = envelope.reply_tx.send(reply);
        }
        reaction
    }
}

/// Answer one [`ControlRequest`] against `app`/`backend`'s current state,
/// feeding every synthesized [`UiEvent`] through `dispatch` — see
/// [`ControlPort::service`]'s doc for what that is on each backend.
fn service_one<A, B>(
    request: ControlRequest,
    app: &mut A,
    backend: &mut B,
    dispatch: &mut impl FnMut(UiEvent, &mut A, &mut B) -> Reaction,
) -> (ControlReply, Reaction)
where
    A: AppLogic,
    B: Backend,
{
    match request {
        ControlRequest::Snapshot => match crate::backend_a11y_tree(backend) {
            Some(tree) => match serde_json::to_value(&tree) {
                Ok(v) => (ControlReply::Value(v), Reaction::Continue),
                Err(e) => (
                    ControlReply::Error(format!("snapshot: failed to serialize tree: {e}")),
                    Reaction::Continue,
                ),
            },
            None => (
                ControlReply::Error(
                    "snapshot: no captured frame yet — enable \
                     crate::frame::set_frame_capture first"
                        .into(),
                ),
                Reaction::Continue,
            ),
        },
        ControlRequest::Act {
            node,
            action,
            value,
        } => {
            let Some(tree) = crate::backend_a11y_tree(backend) else {
                return (
                    ControlReply::Error(
                        "act: no captured frame yet — enable \
                         crate::frame::set_frame_capture first"
                            .into(),
                    ),
                    Reaction::Continue,
                );
            };
            let events = route_action(&tree, node, action, value);
            let mut reaction = Reaction::Continue;
            for event in events {
                reaction = reaction.merge(dispatch(event, app, backend));
            }
            (ControlReply::Ack, reaction)
        }
        ControlRequest::Key { key, modifiers } => {
            let reaction = dispatch(
                UiEvent::KeyPressed {
                    key,
                    modifiers,
                    repeat: false,
                },
                app,
                backend,
            );
            (ControlReply::Ack, reaction)
        }
        ControlRequest::Type(text) => {
            let mut reaction = Reaction::Continue;
            for c in text.chars() {
                reaction = reaction.merge(dispatch(
                    UiEvent::KeyPressed {
                        key: Key::Char(c),
                        modifiers: Modifiers::default(),
                        repeat: false,
                    },
                    app,
                    backend,
                ));
            }
            (ControlReply::Ack, reaction)
        }
        ControlRequest::Command { name, .. } => (
            ControlReply::Error(format!("unknown command: {name}")),
            Reaction::Continue,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingBackend;
    use std::thread;

    /// A trivial `AppLogic` that just counts how many events it was
    /// handed — enough to prove events reached [`AppLogic::handle`]
    /// without needing a real primitive or a captured frame.
    #[derive(Default)]
    struct CountingApp {
        events: Vec<UiEvent>,
    }

    impl AppLogic for CountingApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            self.events.push(event);
            Reaction::Redraw
        }
    }

    /// `RecordingBackend` has no pre-processing pipeline of its own (no
    /// `PreprocessBackend` impl — that only exists for the real backends),
    /// so these unit tests dispatch straight to [`AppLogic::handle`], the
    /// same thing a [`ControlPort::service`] caller on a backend with no
    /// pipeline would pass. [`crate::tui::run::TuiRunner`] passes a
    /// pipeline-aware dispatcher instead — see
    /// `tests/control_channel_tui.rs` for that path.
    fn handle_dispatch(
        event: UiEvent,
        app: &mut CountingApp,
        backend: &mut RecordingBackend,
    ) -> Reaction {
        app.handle(event, backend)
    }

    /// A request sent after the runner has dropped its [`ControlPort`]
    /// (simulating the runner having exited) comes back as
    /// [`ControlError::RunnerGone`] immediately, not a hang or a panic.
    #[test]
    fn request_after_runner_exits_is_a_clean_error() {
        let no_op_waker: Arc<dyn Fn(UserPayload) + Send + Sync> = Arc::new(|_payload| {});
        let (handle, port) = channel(no_op_waker);
        drop(port);

        let err = handle.snapshot().expect_err("runner is gone");
        assert!(matches!(err, ControlError::RunnerGone));
    }

    /// [`ControlRequest::Snapshot`] against a backend with no captured
    /// frame (every fresh [`RecordingBackend`], and any real backend
    /// before [`crate::frame::set_frame_capture`] is enabled) answers an
    /// [`ControlError::Failed`], not a panic or a hang.
    #[test]
    fn snapshot_with_no_captured_frame_is_a_clean_error() {
        let mut app = CountingApp::default();
        let mut backend = RecordingBackend::new();
        let (reply, reaction) = service_one(
            ControlRequest::Snapshot,
            &mut app,
            &mut backend,
            &mut handle_dispatch,
        );
        assert!(matches!(reply, ControlReply::Error(_)));
        assert_eq!(reaction, Reaction::Continue);
    }

    /// Acting on a node id with no captured frame to resolve it against is
    /// the same clean error as [`ControlRequest::Snapshot`] above — never
    /// a panic.
    #[test]
    fn act_with_no_captured_frame_is_a_clean_error() {
        let mut app = CountingApp::default();
        let mut backend = RecordingBackend::new();
        let (reply, reaction) = service_one(
            ControlRequest::Act {
                node: A11yNodeId::for_key("anything"),
                action: A11yAction::Click,
                value: None,
            },
            &mut app,
            &mut backend,
            &mut handle_dispatch,
        );
        assert!(matches!(reply, ControlReply::Error(_)));
        assert_eq!(reaction, Reaction::Continue);
    }

    /// [`ControlRequest::Key`] reaches [`AppLogic::handle`] as a
    /// [`UiEvent::KeyPressed`], and [`ControlRequest::Type`] does the same
    /// once per character, in order.
    #[test]
    fn key_and_type_requests_dispatch_key_pressed_events() {
        let mut app = CountingApp::default();
        let mut backend = RecordingBackend::new();

        let (reply, reaction) = service_one(
            ControlRequest::Key {
                key: Key::Char('x'),
                modifiers: Modifiers::default(),
            },
            &mut app,
            &mut backend,
            &mut handle_dispatch,
        );
        assert!(matches!(reply, ControlReply::Ack));
        assert_eq!(reaction, Reaction::Redraw);

        let (reply, _) = service_one(
            ControlRequest::Type("ab".into()),
            &mut app,
            &mut backend,
            &mut handle_dispatch,
        );
        assert!(matches!(reply, ControlReply::Ack));

        let chars: Vec<char> = app
            .events
            .iter()
            .filter_map(|e| match e {
                UiEvent::KeyPressed {
                    key: Key::Char(c), ..
                } => Some(*c),
                _ => None,
            })
            .collect();
        assert_eq!(chars, vec!['x', 'a', 'b']);
    }

    /// `Command` always answers "unknown command" today — no registry
    /// exists yet — and never touches `app`/`backend`.
    #[test]
    fn command_is_reserved_and_answers_unknown() {
        let mut app = CountingApp::default();
        let mut backend = RecordingBackend::new();
        let (reply, reaction) = service_one(
            ControlRequest::Command {
                name: "do_thing".into(),
                args: serde_json::json!({}),
            },
            &mut app,
            &mut backend,
            &mut handle_dispatch,
        );
        match reply {
            ControlReply::Error(msg) => assert!(msg.contains("do_thing")),
            other => panic!("expected an error reply, got {other:?}"),
        }
        assert_eq!(reaction, Reaction::Continue);
        assert!(app.events.is_empty());
    }

    /// End-to-end over the real channel: a background thread submits a
    /// [`ControlRequest::Key`] via [`ControlHandle::key`] and blocks for
    /// the reply while this thread plays the runner's role, calling
    /// [`ControlPort::service`] in a loop the same way
    /// `tui::run::TuiRunner` does once per iteration.
    #[test]
    fn background_thread_round_trips_a_request_over_the_real_channel() {
        let mut app = CountingApp::default();
        let mut backend = RecordingBackend::new();
        let no_op_waker: Arc<dyn Fn(UserPayload) + Send + Sync> = Arc::new(|_payload| {});
        let (handle, port) = channel(no_op_waker);

        let worker = thread::spawn(move || handle.type_text("hi"));

        loop {
            port.service(&mut app, &mut backend, handle_dispatch);
            if worker.is_finished() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        worker
            .join()
            .expect("worker thread panicked")
            .expect("type_text succeeds");

        let chars: Vec<char> = app
            .events
            .iter()
            .filter_map(|e| match e {
                UiEvent::KeyPressed {
                    key: Key::Char(c), ..
                } => Some(*c),
                _ => None,
            })
            .collect();
        assert_eq!(chars, vec!['h', 'i']);
    }

    /// A request against a runner that is still holding its
    /// [`ControlPort`] but has stopped calling [`ControlPort::service`]
    /// (e.g. stuck, or shut down without dropping the port yet) times out
    /// cleanly rather than hanging forever — the other half of
    /// [`request_after_runner_exits_is_a_clean_error`]'s "runner gone"
    /// case above.
    #[test]
    fn request_against_an_unserviced_port_times_out_cleanly() {
        let no_op_waker: Arc<dyn Fn(UserPayload) + Send + Sync> = Arc::new(|_payload| {});
        let (handle, port) = channel(no_op_waker);
        let handle = handle.with_timeout(Duration::from_millis(20));

        let err = handle
            .snapshot()
            .expect_err("nothing ever services the port");
        assert!(matches!(err, ControlError::Timeout));

        // Keep `port` alive for the whole request so the failure really is
        // a timeout, not a dropped-port race with `RunnerGone`.
        drop(port);
    }
}
