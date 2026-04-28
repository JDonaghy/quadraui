//! The `Backend` trait — one implementation per platform target.
//!
//! Each backend (TUI, GTK, Win-GUI, and eventually macOS) implements this
//! trait. Apps write render code once, parameterised over `<B: Backend>`,
//! and every supported platform rasterises the same primitive descriptions
//! with platform-native drawing + input.
//!
//! See `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md` §4 for design rationale.

use std::path::PathBuf;
use std::time::Duration;

use crate::event::{Rect, UiEvent, Viewport};
use crate::modal_stack::ModalStack;
use crate::primitives::activity_bar::ActivityBarLayout;
use crate::primitives::status_bar::StatusBarHitRegion;
use crate::primitives::tab_bar::TabBarHits;
use crate::primitives::terminal::TerminalLayout;
use crate::{
    Accelerator, AcceleratorId, ActivityBar, Form, ListView, Palette, StatusBar, TabBar, Terminal,
    TextDisplay, TreeView,
};

/// One implementation per platform. TUI, GTK, Win-GUI, and (v1.x) macOS.
pub trait Backend {
    // ─── Frame + viewport ──────────────────────────────────────────────
    /// Viewport geometry in native units. TUI: cells; GTK/Win-GUI/macOS:
    /// pixel-ish units with `scale` set to the DPI ratio.
    fn viewport(&self) -> Viewport;

    /// Begin a frame. Backends may set up the render target, clear, etc.
    fn begin_frame(&mut self, viewport: Viewport);

    /// Flush the current frame to screen.
    fn end_frame(&mut self);

    // ─── Events + keybindings ──────────────────────────────────────────
    /// Drain all queued native events. Returns a fully-translated
    /// `Vec<UiEvent>` ready for app dispatch. Never blocks.
    fn poll_events(&mut self) -> Vec<UiEvent>;

    /// Block for up to `timeout` waiting for at least one event. Returns an
    /// empty `Vec` on timeout. Used by apps that don't want to busy-poll.
    fn wait_events(&mut self, timeout: Duration) -> Vec<UiEvent>;

    /// Register an accelerator. The backend stores it and emits
    /// [`UiEvent::Accelerator`] when the native key event matches.
    fn register_accelerator(&mut self, acc: &Accelerator);

    /// Remove a previously-registered accelerator.
    fn unregister_accelerator(&mut self, id: &AcceleratorId);

    // ─── Modal-overlay tracking ────────────────────────────────────────
    /// Mutable handle to the backend's modal stack. Apps push when a
    /// palette / dialog / context-menu opens and pop when it closes;
    /// quadraui's dispatcher consults the stack so events inside an
    /// open modal can't fall through to widgets behind it.
    ///
    /// See [`ModalStack`] and [`crate::dispatch::dispatch_mouse_down`]
    /// for the routing contract.
    fn modal_stack_mut(&mut self) -> &mut ModalStack;

    // ─── Platform services ─────────────────────────────────────────────
    /// Clipboard, file dialogs, notifications, URL opening, platform name.
    fn services(&self) -> &dyn PlatformServices;

    // ─── Drawing — one method per primitive ────────────────────────────
    //
    // Implementations are thin wrappers around each backend crate's
    // internal `pub fn draw_*` free functions. Example:
    //
    //   impl Backend for WinBackend {
    //       fn draw_tree(&mut self, rect: Rect, tree: &TreeView) {
    //           quadraui_win::draw_tree(self.ctx(), tree, self.theme(), rect);
    //       }
    //       // ... one per primitive
    //   }
    //
    // Adding a primitive is a breaking change to this trait — intentional
    // (see `BACKEND_TRAIT_PROPOSAL.md` §4). Backends opt in to the new
    // primitive in the same PR that adds it to the trait.
    fn draw_tree(&mut self, rect: Rect, tree: &TreeView);
    fn draw_list(&mut self, rect: Rect, list: &ListView);
    fn draw_form(&mut self, rect: Rect, form: &Form);
    fn draw_palette(&mut self, rect: Rect, palette: &Palette);

    // Layout-passthrough primitives (per BACKEND_TRAIT_PROPOSAL.md
    // §6.2). Each backend computes the primitive's layout internally
    // using its native measurer (cells for TUI, Pango / DirectWrite /
    // Core Text pixels for the others) — apps don't have access to
    // those handles, so layout precomputation can't live caller-side.
    //
    // Methods that produce hit-region data (clickable segments,
    // close-button rects, link rects) return it directly so callers
    // route clicks against the same data the rasteriser used to paint.
    /// Draw a status bar. Returns hit regions in **bar-local
    /// coordinates** (relative to `rect.x` / `rect.y`) for each segment
    /// carrying an `action_id`. Caller dispatches clicks against the
    /// returned list.
    fn draw_status_bar(&mut self, rect: Rect, bar: &StatusBar) -> Vec<StatusBarHitRegion>;
    /// Draw a tab bar. `hovered_close_tab` carries per-frame hover
    /// state so the rasteriser can paint a hover background behind the
    /// hovered tab's close glyph (the primitive itself carries no
    /// mouse state). Returns [`TabBarHits`] for click dispatch +
    /// scroll-offset reconciliation.
    fn draw_tab_bar(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits;
    fn draw_activity_bar(&mut self, rect: Rect, bar: &ActivityBar, layout: &ActivityBarLayout);
    fn draw_terminal(&mut self, rect: Rect, term: &Terminal, layout: &TerminalLayout);
    /// Draw a `TextDisplay` (streaming-text panel — log viewer, output
    /// pane, YAML view, etc). No hit-region data is returned;
    /// `TextDisplay` itself is non-interactive (selection / scroll
    /// happen at the panel chrome level, not at the line/span level).
    fn draw_text_display(&mut self, rect: Rect, td: &TextDisplay);
}

/// Platform services the backend exposes to apps: clipboard, file dialogs,
/// notifications, URL opening.
pub trait PlatformServices {
    fn clipboard(&self) -> &dyn Clipboard;

    /// Show a native file-open dialog (blocking). Returns `None` if the
    /// user cancelled. TUI backends return `None` and write a hint to
    /// stderr; apps should provide an in-TUI picker instead.
    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

    /// Show a native file-save dialog.
    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

    /// Dispatch a system notification.
    fn send_notification(&self, n: Notification);

    /// Open a URL in the platform's default browser.
    fn open_url(&self, url: &str);

    /// Platform identifier — matches the `BackendNative.backend` field.
    /// One of `"tui"`, `"gtk"`, `"win-gui"`, `"macos"`.
    fn platform_name(&self) -> &'static str;
}

/// Trait object-safe clipboard access.
pub trait Clipboard {
    /// Read the current clipboard contents as plain text. `None` on
    /// empty / non-text clipboard or platform error.
    fn read_text(&self) -> Option<String>;

    /// Write plain text to the clipboard.
    fn write_text(&self, text: &str);
}

/// Options for [`PlatformServices::show_file_open_dialog`] and
/// [`PlatformServices::show_file_save_dialog`].
#[derive(Debug, Clone, Default)]
pub struct FileDialogOptions {
    /// Dialog window title.
    pub title: Option<String>,
    /// Suggested starting directory.
    pub initial_dir: Option<PathBuf>,
    /// Suggested file name (save dialog only).
    pub initial_filename: Option<String>,
    /// File type filters — `(display_name, &[ext])` pairs.
    pub filters: Vec<(String, Vec<String>)>,
}

/// A system notification request.
#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Whether the notification is high-priority (e.g. error). Backends
    /// may use this to pick a different icon or sound.
    pub urgent: bool,
}
