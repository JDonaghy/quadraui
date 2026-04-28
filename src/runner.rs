//! Runner-crate API for `AppLogic`-style apps that delegate event +
//! frame loops to a per-backend runner (`quadraui::tui::run`,
//! `quadraui::gtk::run`, future `quadraui::win::run` /
//! `quadraui::macos::run`).
//!
//! See `docs/BACKEND_SETUP_AUDIT.md` (B.5d / #260) for the design
//! notes that drove the trait shape, and `examples/tui_app.rs` for a
//! minimal end-to-end usage.
//!
//! # Trait shape
//!
//! Apps implement [`AppLogic`] with three methods:
//! - [`AppLogic::setup`] — one-time init: register accelerators,
//!   warm caches, etc.
//! - [`AppLogic::render`] — per-frame paint. The runner enters its
//!   `enter_frame_scope` first; `render` calls `backend.draw_*(...)`
//!   to paint primitives.
//! - [`AppLogic::handle`] — per-event dispatch. Returns a
//!   [`Reaction`] telling the runner what to do next (continue,
//!   redraw, exit).
//!
//! # Why direct `&mut dyn Backend`
//!
//! Apps already need to know the `Backend` trait surface to call
//! `draw_*` methods. Wrapping it in a `RenderCtx` would add a parallel
//! API to maintain without hiding anything meaningful. Direct backend
//! access also gives apps `services()` (clipboard, dialogs) and
//! `modal_stack_mut()` for free in the event handler.
//!
//! # Single-area scope
//!
//! The first runner ships supports apps with a single render target
//! (one ratatui `Frame` for TUI, one `DrawingArea` for GTK).
//! Multi-DrawingArea apps like vimcode need a richer shape — the
//! runner can be extended with multi-target support in a later stage.

use crate::backend::Backend;
use crate::event::UiEvent;

/// Tells the runner what to do after `handle` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    /// Continue the loop. Next event poll. The runner will redraw
    /// only if some other event in the same batch returned
    /// [`Reaction::Redraw`], or if the runner's own internal
    /// invalidation triggers (resize, etc.).
    Continue,
    /// Force a redraw on the next loop iteration. Use after engine
    /// state mutations that need visible feedback.
    Redraw,
    /// Tear down and exit the runner. The runner returns control to
    /// the caller (typically `main`).
    Exit,
}

/// Trait an app implements to plug into [`crate::tui::run`] /
/// [`crate::gtk::run`].
///
/// The runner owns the event loop, frame loop, terminal/widget setup,
/// and tear-down. The app owns its state (`&mut self`), per-frame
/// rendering, and event dispatch.
pub trait AppLogic {
    /// One-time setup hook. Called by the runner after backend
    /// construction but before the first frame. Use this to register
    /// accelerators, warm caches, set up file watchers, etc.
    ///
    /// Default impl is a no-op so apps that don't need setup don't
    /// have to write boilerplate.
    fn setup(&mut self, _backend: &mut dyn Backend) {}

    /// Per-frame paint. Called inside the runner's
    /// `enter_frame_scope`. The app calls `backend.draw_*(...)` for
    /// each primitive it wants drawn. The app is also responsible
    /// for setting `theme` / `line_height` / `char_width` on the
    /// backend if the app's theme system varies (typically once per
    /// frame at the start of `render`).
    fn render(&self, backend: &mut dyn Backend);

    /// Per-event dispatch. The runner calls this for every
    /// [`UiEvent`] returned from `backend.wait_events`. Returns a
    /// [`Reaction`] telling the runner what to do next.
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction;
}
