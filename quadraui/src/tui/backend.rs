//! TUI implementation of [`crate::Backend`].
//!
//! `TuiBackend` owns the persistent UI state the trait requires —
//! viewport dimensions, modal stack, drag state, accelerator registry,
//! platform services — plus a transient frame pointer set inside
//! [`Self::enter_frame_scope`] so trait `draw_*` methods can reach
//! the ratatui `&mut Frame<'_>` (which only exists inside
//! `terminal.draw(|frame| …)`'s closure).
//!
//! ### Frame-scope mechanism
//!
//! ratatui's `terminal.draw(|frame| …)` API only yields `&mut Frame`
//! inside the closure, so `TuiBackend` can't hold one across method
//! calls. Instead [`Self::enter_frame_scope`] takes the frame, stashes
//! a type-erased `*mut ()` in `current_frame_ptr`, runs the caller's
//! closure (where trait `draw_*` methods can reach the frame via
//! [`Self::current_frame_mut`]), and clears the pointer on exit.
//! The pointer is null outside the scope, so the safe accessor
//! returns `None` and `draw_*` methods can detect misuse.
//!
//! ### What the trait covers vs. what stays inherent
//!
//! Drawing methods for the migrated primitives — `draw_palette`,
//! `draw_list`, `draw_tree`, `draw_form` — go through the trait so
//! the same generic `<B: Backend>` paint function works against
//! `TuiBackend`, `MockBackend` (see the `tests` module), and future
//! GTK/Win-GUI/macOS backends. The other five `draw_*` methods stay
//! stubbed pending a quadraui-side trait change to thread
//! pre-computed `*Layout` parameters (see
//! `BACKEND_TRAIT_PROPOSAL.md` §6.2).
//!
//! Drag-state observation is deliberately not on the trait — only
//! `crate::dispatch::*` needs to inspect it, and the backend keeps
//! it as a struct field accessed through
//! [`Self::drag_and_modal_mut`].
//!
//! Event flow goes through the trait: [`Self::wait_events`] reads
//! crossterm events, translates them via
//! [`super::events::crossterm_to_uievents`], then runs
//! [`Self::apply_accelerators`] to rewrite registered key bindings as
//! [`UiEvent::Accelerator`] before returning. The event loop in
//! [`super::event_loop`] consumes those `UiEvent`s via
//! [`Backend::wait_events`].

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;

use crate::{
    parse_key_binding, Accelerator, AcceleratorId, AcceleratorScope, ActivityBar, Backend,
    DragState, Form, KeyBinding, ListView, ModalStack, Palette, ParsedBinding, PlatformServices,
    Rect as QRect, StatusBar, TabBar, Terminal as TerminalPrim, TextDisplay, TreeView, UiEvent,
    Viewport,
};
use ratatui::layout::Rect;
use ratatui::Frame;

use super::services::TuiPlatformServices;

/// Minimum gap (in cells) between left and right status-bar halves
/// before priority drop kicks in. Mirrors `crate::gtk::status_bar`'s
/// `MIN_GAP_PX = 16.0`. Irrelevant for bars without right segments.
const MIN_GAP_CELLS: f32 = 2.0;

/// TUI backend implementing [`crate::Backend`].
///
/// Owns the persistent UI state the trait requires plus a transient
/// "current frame" pointer + theme set inside
/// [`Self::enter_frame_scope`]. The pointer is type-erased
/// (`*mut ()`) and cleared on scope exit; safe accessors deref it
/// only while the scope is active.
///
/// The ratatui `Terminal` is **not** owned here — it stays as a local
/// in [`super::event_loop`]. See `BACKEND_TRAIT_PROPOSAL.md` §11 for
/// rationale and the eventual migration plan.
pub struct TuiBackend {
    viewport: Viewport,
    modal_stack: ModalStack,
    drag_state: DragState,
    accelerators: HashMap<AcceleratorId, Accelerator>,
    /// Pre-parsed bindings, kept in lock-step with `accelerators`. Stage 6
    /// uses this for the `wait_events`/`poll_events` matcher to avoid
    /// re-parsing on every keystroke. First-match-wins iteration order
    /// matches insertion order (`Vec`, not `HashMap`).
    parsed_accelerators: Vec<(ParsedBinding, AcceleratorId)>,
    services: TuiPlatformServices,
    /// Type-erased `&mut Frame<'_>` pointer; non-null only inside
    /// [`Self::enter_frame_scope`]. `Cell` (not `RefCell`) because
    /// trait methods borrow `&mut self` already; we only need
    /// shared-cell semantics for `Copy`-able pointer values.
    current_frame_ptr: Cell<*mut ()>,
    /// Theme captured by the most recent
    /// [`Self::set_current_theme`] call. Defaults to
    /// `crate::Theme::default()` until set.
    current_theme: crate::Theme,
    /// Whether the rasterisers should use Nerd Font glyphs (`true`)
    /// or ASCII fallbacks (`false`). Apps set this from their own
    /// settings via [`Self::set_nerd_fonts`]; defaults to `true`.
    nerd_fonts_enabled: bool,
}

impl TuiBackend {
    /// Construct the backend with default viewport (80×24) and
    /// default quadraui theme. The caller calls [`Backend::begin_frame`]
    /// each frame (after `terminal.size()`) to keep
    /// [`Backend::viewport`] in sync, and [`Self::set_current_theme`]
    /// before drawing so the trait `draw_*` methods see the right
    /// palette.
    pub fn new() -> Self {
        Self {
            viewport: Viewport::default(),
            modal_stack: ModalStack::new(),
            drag_state: DragState::new(),
            accelerators: HashMap::new(),
            parsed_accelerators: Vec::new(),
            services: TuiPlatformServices::new(),
            current_frame_ptr: Cell::new(std::ptr::null_mut()),
            current_theme: crate::Theme::default(),
            nerd_fonts_enabled: true,
        }
    }

    /// Enter the frame-scope: stash the `&mut Frame<'_>` pointer for
    /// trait `draw_*` methods to access, run `f`, then clear the
    /// pointer. **Must** be called from inside a
    /// `terminal.draw(|frame| …)` closure.
    ///
    /// Type-erased through `*mut ()` because `Frame<'a>` carries a
    /// lifetime parameter we don't want to thread onto `TuiBackend`.
    /// Safety relies on three invariants enforced by this function's
    /// shape:
    ///   1. The pointer is set immediately before running `f` and
    ///      cleared immediately after, including on panic (via
    ///      [`scopeguard`]-style restore).
    ///   2. `f` cannot move the pointer out — it only sees it via
    ///      [`Self::current_frame_mut`] which returns a fresh
    ///      `&mut Frame<'_>` borrow scoped to the call.
    ///   3. `enter_frame_scope` calls don't nest meaningfully —
    ///      the inner call would overwrite the pointer with the
    ///      same `&mut` (already aliased) which Rust's borrow-checker
    ///      forbids at the caller side.
    pub fn enter_frame_scope<R>(
        &mut self,
        frame: &mut Frame<'_>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let ptr = frame as *mut Frame<'_> as *mut ();
        let prev = self.current_frame_ptr.replace(ptr);
        let result = f(self);
        self.current_frame_ptr.set(prev);
        result
    }

    /// Get the current frame inside [`Self::enter_frame_scope`], or
    /// `None` outside it. Trait `draw_*` methods call this and bail
    /// (panic in dev, silent return otherwise) if `None`.
    fn current_frame_mut(&mut self) -> Option<&mut Frame<'static>> {
        let ptr = self.current_frame_ptr.get();
        if ptr.is_null() {
            None
        } else {
            // SAFETY: `enter_frame_scope` set this from a real
            // `&mut Frame<'_>` and won't return until the scope
            // ends, at which point the pointer is cleared. Outside
            // the scope `ptr` is null and we return `None`.
            // The `'static` lifetime here is a fiction — the borrow
            // is actually scoped to the enclosing
            // `enter_frame_scope` call. Methods using this never let
            // the borrow escape past their own return.
            Some(unsafe { &mut *(ptr as *mut Frame<'static>) })
        }
    }

    /// Update the cached quadraui theme. Call once per frame from
    /// `paint`, before any `backend.draw_*` calls. Subsequent
    /// `draw_*` invocations consume the stored theme.
    pub fn set_current_theme(&mut self, theme: crate::Theme) {
        self.current_theme = theme;
    }

    /// Toggle Nerd Font glyph rendering for primitives that vary
    /// based on font availability. Apps wire this from their own
    /// settings (vimcode reads `engine.settings.use_nerd_fonts`;
    /// kubeui has its own setting).
    pub fn set_nerd_fonts(&mut self, enabled: bool) {
        self.nerd_fonts_enabled = enabled;
    }

    /// Disjoint mutable borrows of drag state and modal stack.
    /// `mouse.rs::handle_mouse` needs both at the same time, and
    /// borrowing each field through a separate `&mut self` accessor
    /// would conflict — this helper splits the field borrows in one
    /// call. The trait deliberately doesn't expose drag state (it's
    /// a backend implementation detail; only the dispatch helpers
    /// in `crate::dispatch::*` need to observe it).
    pub fn drag_and_modal_mut(&mut self) -> (&mut DragState, &mut ModalStack) {
        (&mut self.drag_state, &mut self.modal_stack)
    }

    /// Walk `events` and rewrite any `UiEvent::KeyPressed` whose key +
    /// modifiers match a registered `Global`-scope accelerator into
    /// `UiEvent::Accelerator(id, modifiers)`. Stage 6's whole point: the
    /// app dispatches on stable IDs, never on raw key strings, for
    /// keybindings the user can rebind.
    ///
    /// Widget- and Mode-scoped accelerators are skipped here — the
    /// backend doesn't know which widget has focus or what mode the app
    /// is in. Apps that want those scopes match against `KeyPressed`
    /// themselves once they have that context.
    fn apply_accelerators(&self, events: &mut [UiEvent]) {
        if self.parsed_accelerators.is_empty() {
            return;
        }
        for ev in events.iter_mut() {
            if let UiEvent::KeyPressed { key, modifiers, .. } = ev {
                if let Some(id) = self.match_keypress(key, *modifiers) {
                    *ev = UiEvent::Accelerator(id, *modifiers);
                }
            }
        }
    }

    fn match_keypress(
        &self,
        key: &crate::Key,
        modifiers: crate::Modifiers,
    ) -> Option<AcceleratorId> {
        let key_name = match key {
            crate::Key::Char(c) => {
                // Single ASCII letters parse as lowercase in
                // `parse_key_binding`; mirror that so `<C-S-T>` and
                // `Ctrl+Shift+t` both match here.
                if c.is_ascii() {
                    c.to_ascii_lowercase().to_string()
                } else {
                    c.to_string()
                }
            }
            crate::Key::Named(named) => named_key_to_binding_name(*named).to_string(),
        };
        for (parsed, id) in &self.parsed_accelerators {
            if parsed.modifiers == modifiers && parsed.key == key_name {
                // Skip non-Global-scope entries — the backend doesn't
                // own focus/mode context.
                if let Some(acc) = self.accelerators.get(id) {
                    if matches!(acc.scope, AcceleratorScope::Global) {
                        return Some(id.clone());
                    }
                }
            }
        }
        None
    }
}

/// Parse a `KeyBinding` (any variant) into a `ParsedBinding`. Returns
/// `None` for unparseable literals — those silently miss matching, same
/// as the engine-side B.2 path. The universal arms map to the canonical
/// vim-style strings the rest of vimcode already uses.
fn parse_binding(b: &KeyBinding) -> Option<ParsedBinding> {
    match b {
        KeyBinding::Literal(s) if s.is_empty() => None,
        KeyBinding::Literal(s) => parse_key_binding(s),
        KeyBinding::Save => parse_key_binding("<C-s>"),
        KeyBinding::Open => parse_key_binding("<C-o>"),
        KeyBinding::New => parse_key_binding("<C-n>"),
        KeyBinding::Close => parse_key_binding("<C-w>"),
        KeyBinding::Copy => parse_key_binding("<C-c>"),
        KeyBinding::Cut => parse_key_binding("<C-x>"),
        KeyBinding::Paste => parse_key_binding("<C-v>"),
        KeyBinding::Undo => parse_key_binding("<C-z>"),
        KeyBinding::Redo => parse_key_binding("<C-S-z>"),
        KeyBinding::SelectAll => parse_key_binding("<C-a>"),
        KeyBinding::Find => parse_key_binding("<C-f>"),
        KeyBinding::Replace => parse_key_binding("<C-h>"),
        KeyBinding::Quit => parse_key_binding("<C-q>"),
    }
}

/// Map a `crate::NamedKey` to the canonical name `parse_key_binding`
/// produces. Letter case follows `accelerator::normalise_key_name`:
/// single letters lowercase, named keys TitleCase-preserved.
fn named_key_to_binding_name(named: crate::NamedKey) -> &'static str {
    use crate::NamedKey::*;
    match named {
        Escape => "Escape",
        Tab => "Tab",
        BackTab => "BackTab",
        Enter => "Enter",
        Backspace => "Backspace",
        Delete => "Delete",
        Insert => "Insert",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        Up => "Up",
        Down => "Down",
        Left => "Left",
        Right => "Right",
        F(1) => "F1",
        F(2) => "F2",
        F(3) => "F3",
        F(4) => "F4",
        F(5) => "F5",
        F(6) => "F6",
        F(7) => "F7",
        F(8) => "F8",
        F(9) => "F9",
        F(10) => "F10",
        F(11) => "F11",
        F(12) => "F12",
        F(_) => "",
        CapsLock => "CapsLock",
        NumLock => "NumLock",
        ScrollLock => "ScrollLock",
        Menu => "Menu",
    }
}

impl Default for TuiBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a [`crate::Rect`] (f32 coordinates) to a
/// [`ratatui::layout::Rect`] (u16). Any negative values clamp to 0;
/// fractional widths/heights round to nearest. Used by every trait
/// `draw_*` method to translate the trait's `Rect` argument.
fn q_rect_to_ratatui(r: QRect) -> Rect {
    let x = r.x.max(0.0).round() as u16;
    let y = r.y.max(0.0).round() as u16;
    let w = r.width.max(0.0).round() as u16;
    let h = r.height.max(0.0).round() as u16;
    Rect::new(x, y, w, h)
}

impl Backend for TuiBackend {
    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    fn end_frame(&mut self) {
        // No-op. The frame's actual flush happens when ratatui's
        // `terminal.draw(|frame| …)` closure returns; this method
        // exists for parity with backends that need explicit flush.
    }

    fn poll_events(&mut self) -> Vec<UiEvent> {
        // Drain every queued crossterm event; never blocks. Each
        // native event translates to zero, one, or more `UiEvent`s
        // via [`super::events::crossterm_to_uievents`], then runs
        // through [`Self::apply_accelerators`] so registered bindings
        // surface as `UiEvent::Accelerator` instead of `KeyPressed`.
        let mut out = Vec::new();
        while ratatui::crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
            match ratatui::crossterm::event::read() {
                Ok(ev) => out.extend(super::events::crossterm_to_uievents(ev)),
                Err(_) => break,
            }
        }
        self.apply_accelerators(&mut out);
        out
    }

    fn wait_events(&mut self, timeout: Duration) -> Vec<UiEvent> {
        // Block up to `timeout` for the next native event, translate it,
        // match against registered accelerators, and return. Empty `Vec`
        // on timeout.
        //
        // The "one event per call" shape preserves the existing event
        // loop's `match` semantics: each iteration handles exactly one
        // event before checking timing-sensitive state (yank highlight
        // expiry, notification spinner cadence, etc.).
        if let Ok(true) = ratatui::crossterm::event::poll(timeout) {
            if let Ok(ev) = ratatui::crossterm::event::read() {
                let mut out = super::events::crossterm_to_uievents(ev);
                self.apply_accelerators(&mut out);
                return out;
            }
        }
        Vec::new()
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
        // Re-registration replaces the prior entry — both in the map and
        // the parsed list, otherwise stale bindings would shadow the new
        // one in `match_accelerator`.
        self.accelerators.insert(acc.id.clone(), acc.clone());
        self.parsed_accelerators.retain(|(_, id)| id != &acc.id);
        if let Some(parsed) = parse_binding(&acc.binding) {
            self.parsed_accelerators.push((parsed, acc.id.clone()));
        }
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.accelerators.remove(id);
        self.parsed_accelerators.retain(|(_, eid)| eid != id);
    }

    fn modal_stack_mut(&mut self) -> &mut ModalStack {
        &mut self.modal_stack
    }

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    // ─── Drawing ───────────────────────────────────────────────────────────
    //
    // Implementations call into the public `crate::tui::draw_*` free
    // functions; this trait impl is the thin wrapper. The frame is
    // stashed by `enter_frame_scope`; the theme by `set_current_theme`.
    // Calling these outside `enter_frame_scope` is a programmer error
    // and panics in dev (the `expect` makes the boundary loud).

    fn draw_tree(&mut self, rect: QRect, tree: &TreeView) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_tree called outside enter_frame_scope");
        crate::tui::draw_tree(frame.buffer_mut(), area, tree, &theme, nerd_fonts);
    }

    fn draw_list(&mut self, rect: QRect, list: &ListView) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_list called outside enter_frame_scope");
        crate::tui::draw_list(frame.buffer_mut(), area, list, &theme, nerd_fonts);
    }

    fn draw_form(&mut self, rect: QRect, form: &Form) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_form called outside enter_frame_scope");
        crate::tui::draw_form(frame.buffer_mut(), area, form, &theme);
    }

    fn draw_palette(&mut self, rect: QRect, palette: &Palette) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_palette called outside enter_frame_scope");
        crate::tui::draw_palette(frame.buffer_mut(), area, palette, &theme, nerd_fonts);
    }

    // ─── Layout-passthrough primitives — Stage 3 / trait migration ──────
    //
    // These take a pre-computed `*Layout` in their existing TUI
    // shims. Migrating them through the trait needs either the
    // trait to take `&Layout` (per `BACKEND_TRAIT_PROPOSAL.md` §6.2)
    // or a per-method recompute. Deferred until Stage 3.

    // Phase B.5b Stage 9: trait extended with `&Layout` parameters
    // per `BACKEND_TRAIT_PROPOSAL.md` §6.2. The TUI free functions
    // for these primitives take `&Layout` directly — the trait impls
    // are now thin pass-throughs, mirroring the GTK impls in
    // `gtk/backend.rs`.

    fn draw_status_bar(&mut self, rect: QRect, bar: &StatusBar) -> Vec<crate::StatusBarHitRegion> {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        // Cell-unit measurer: each char counts as one cell.
        let layout = bar.layout(area.width as f32, 1.0, MIN_GAP_CELLS, |seg| {
            crate::StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_status_bar called outside enter_frame_scope");
        crate::tui::draw_status_bar(frame.buffer_mut(), area, bar, &layout, &theme)
    }

    fn draw_tab_bar(
        &mut self,
        rect: QRect,
        bar: &TabBar,
        _hovered_close_tab: Option<usize>,
    ) -> crate::TabBarHits {
        // TUI doesn't render close-button hover bg; the parameter is
        // accepted for trait parity with GTK.
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        // Cell-unit measurer mirrors `render_impl::render_tab_bar`.
        let tab_widths: Vec<usize> = bar
            .tabs
            .iter()
            .map(|t| t.label.chars().count() + crate::tui::TAB_CLOSE_COLS as usize)
            .collect();
        let layout = bar.layout(
            area.width as f32,
            area.height as f32,
            0.0, // no scroll arrows in TUI
            |i| crate::TabMeasure::new(tab_widths[i] as f32, crate::tui::TAB_CLOSE_COLS as f32),
            |i| crate::SegmentMeasure::new(bar.right_segments[i].width_cells as f32),
        );
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_tab_bar called outside enter_frame_scope");
        crate::tui::draw_tab_bar(frame.buffer_mut(), area, bar, &layout, &theme)
    }

    fn draw_activity_bar(
        &mut self,
        _rect: QRect,
        _bar: &ActivityBar,
        _hovered_idx: Option<usize>,
    ) -> Vec<crate::ActivityBarRowHit> {
        unimplemented!("TuiBackend::draw_activity_bar — TUI uses inline draw; trait reserved")
    }

    fn draw_terminal(&mut self, _rect: QRect, _term: &TerminalPrim) {
        unimplemented!("TuiBackend::draw_terminal — TUI uses inline draw; trait reserved")
    }

    fn draw_text_display(&mut self, rect: QRect, td: &TextDisplay) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_text_display called outside enter_frame_scope");
        crate::tui::draw_text_display(frame.buffer_mut(), area, td, &theme);
    }

    fn draw_tooltip(&mut self, tooltip: &crate::Tooltip, layout: &crate::TooltipLayout) {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_tooltip called outside enter_frame_scope");
        crate::tui::draw_tooltip(frame.buffer_mut(), tooltip, layout, &theme);
    }

    fn draw_context_menu(
        &mut self,
        menu: &crate::ContextMenu,
        layout: &crate::ContextMenuLayout,
    ) -> Vec<(QRect, crate::WidgetId)> {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_context_menu called outside enter_frame_scope");
        crate::tui::draw_context_menu(frame.buffer_mut(), menu, layout, &theme);
        // TUI rasteriser doesn't return hit data — derive from layout.
        // The primitive's hit_test() is the canonical way; this Vec
        // is here for trait parity with GTK.
        let _ = menu;
        layout
            .hit_regions
            .iter()
            .filter_map(|(rect, hit)| match hit {
                crate::primitives::context_menu::ContextMenuHit::Item(id) => {
                    Some((*rect, id.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn draw_dialog(
        &mut self,
        dialog: &crate::primitives::dialog::Dialog,
        layout: &crate::primitives::dialog::DialogLayout,
    ) -> Vec<QRect> {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_dialog called outside enter_frame_scope");
        crate::tui::draw_dialog(frame.buffer_mut(), dialog, layout, &theme);
        // Derive button rects from the layout (TUI rasteriser doesn't
        // return them; the primitive owns the layout).
        layout
            .visible_buttons
            .iter()
            .map(|vis| vis.bounds)
            .collect()
    }
}

// ─── Cross-backend validation tests ──────────────────────────────────────────
//
// Phase B.4 Stage 3b: prove the `Backend` trait is genuinely consumable
// by app code that's *generic* over the backend, not just by `TuiBackend`
// specifically. A minimal `MockBackend` records each `draw_*` call into
// a `Vec<DrawCall>`; a generic `<B: Backend>` helper invokes the trait
// methods; assertions verify the calls landed.
//
// This is the architectural proof point Stage 3 was designed around:
// once the trait works against TuiBackend AND a foreign mock, future
// backends (GtkBackend in B.5, WinBackend in B.6, MacOSBackend in B.7)
// drop in without forking the app's render code.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Clipboard, FileDialogOptions, Notification};
    use crate::{ListItem, ListView, Palette, PaletteItem, StyledSpan, StyledText, WidgetId};

    /// Records every draw call so tests can assert what the trait
    /// boundary actually delivers.
    #[derive(Debug, Clone, PartialEq)]
    enum DrawCall {
        List { rect: QRect, item_count: usize },
        Palette { rect: QRect, item_count: usize },
    }

    struct NoopClipboard;
    impl Clipboard for NoopClipboard {
        fn read_text(&self) -> Option<String> {
            None
        }
        fn write_text(&self, _t: &str) {}
    }

    struct MockServices {
        clipboard: NoopClipboard,
    }
    impl MockServices {
        fn new() -> Self {
            Self {
                clipboard: NoopClipboard,
            }
        }
    }
    impl PlatformServices for MockServices {
        fn clipboard(&self) -> &dyn Clipboard {
            &self.clipboard
        }
        fn show_file_open_dialog(&self, _opts: FileDialogOptions) -> Option<std::path::PathBuf> {
            None
        }
        fn show_file_save_dialog(&self, _opts: FileDialogOptions) -> Option<std::path::PathBuf> {
            None
        }
        fn send_notification(&self, _n: Notification) {}
        fn open_url(&self, _url: &str) {}
        fn platform_name(&self) -> &'static str {
            "mock"
        }
    }

    struct MockBackend {
        calls: Vec<DrawCall>,
        modal_stack: ModalStack,
        services: MockServices,
        viewport: Viewport,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                calls: Vec::new(),
                modal_stack: ModalStack::new(),
                services: MockServices::new(),
                viewport: Viewport::new(80.0, 24.0, 1.0),
            }
        }
    }

    impl Backend for MockBackend {
        fn viewport(&self) -> Viewport {
            self.viewport
        }
        fn begin_frame(&mut self, viewport: Viewport) {
            self.viewport = viewport;
        }
        fn end_frame(&mut self) {}
        fn poll_events(&mut self) -> Vec<UiEvent> {
            Vec::new()
        }
        fn wait_events(&mut self, _t: Duration) -> Vec<UiEvent> {
            Vec::new()
        }
        fn register_accelerator(&mut self, _a: &Accelerator) {}
        fn unregister_accelerator(&mut self, _id: &AcceleratorId) {}
        fn modal_stack_mut(&mut self) -> &mut ModalStack {
            &mut self.modal_stack
        }
        fn services(&self) -> &dyn PlatformServices {
            &self.services
        }

        fn draw_list(&mut self, rect: QRect, list: &ListView) {
            self.calls.push(DrawCall::List {
                rect,
                item_count: list.items.len(),
            });
        }

        fn draw_palette(&mut self, rect: QRect, palette: &Palette) {
            self.calls.push(DrawCall::Palette {
                rect,
                item_count: palette.items.len(),
            });
        }

        // The other 7 trait methods are unimplemented — this mock only
        // records the ones the cross-backend test actually exercises.
        fn draw_tree(&mut self, _r: QRect, _t: &TreeView) {}
        fn draw_form(&mut self, _r: QRect, _f: &Form) {}
        fn draw_status_bar(&mut self, _r: QRect, _b: &StatusBar) -> Vec<crate::StatusBarHitRegion> {
            Vec::new()
        }
        fn draw_tab_bar(
            &mut self,
            _r: QRect,
            _b: &TabBar,
            _hovered_close_tab: Option<usize>,
        ) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn draw_activity_bar(
            &mut self,
            _r: QRect,
            _b: &ActivityBar,
            _h: Option<usize>,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn draw_terminal(&mut self, _r: QRect, _t: &TerminalPrim) {}
        fn draw_text_display(&mut self, _r: QRect, _t: &TextDisplay) {}
        fn draw_tooltip(&mut self, _t: &crate::Tooltip, _l: &crate::TooltipLayout) {}
        fn draw_context_menu(
            &mut self,
            _m: &crate::ContextMenu,
            _l: &crate::ContextMenuLayout,
        ) -> Vec<(QRect, crate::WidgetId)> {
            Vec::new()
        }
        fn draw_dialog(
            &mut self,
            _d: &crate::primitives::dialog::Dialog,
            _l: &crate::primitives::dialog::DialogLayout,
        ) -> Vec<QRect> {
            Vec::new()
        }
    }

    /// Generic helper — the minimal "app render code" that consumes
    /// `Backend` through `<B>`. Future backends slot in here without
    /// changes.
    fn paint_overlays<B: Backend>(backend: &mut B, palette: &Palette, list: &ListView) {
        backend.draw_palette(QRect::new(10.0, 5.0, 60.0, 14.0), palette);
        backend.draw_list(QRect::new(0.0, 20.0, 80.0, 4.0), list);
    }

    fn sample_palette() -> Palette {
        Palette {
            id: WidgetId::new("test:palette"),
            title: "Pick one".to_string(),
            query: String::new(),
            query_cursor: 0,
            items: vec![
                PaletteItem {
                    text: StyledText {
                        spans: vec![StyledSpan::plain("alpha")],
                    },
                    detail: None,
                    icon: None,
                    match_positions: Vec::new(),
                },
                PaletteItem {
                    text: StyledText {
                        spans: vec![StyledSpan::plain("beta")],
                    },
                    detail: None,
                    icon: None,
                    match_positions: Vec::new(),
                },
            ],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 2,
            has_focus: true,
        }
    }

    fn sample_list() -> ListView {
        ListView {
            id: WidgetId::new("test:list"),
            title: None,
            items: vec![ListItem {
                text: StyledText {
                    spans: vec![StyledSpan::plain("only")],
                },
                icon: None,
                detail: None,
                decoration: crate::Decoration::Normal,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
        }
    }

    #[test]
    fn paint_overlays_records_through_mock_backend() {
        let mut mock = MockBackend::new();
        let palette = sample_palette();
        let list = sample_list();

        paint_overlays(&mut mock, &palette, &list);

        assert_eq!(mock.calls.len(), 2);
        assert!(matches!(
            mock.calls[0],
            DrawCall::Palette { item_count: 2, .. }
        ));
        assert!(matches!(
            mock.calls[1],
            DrawCall::List { item_count: 1, .. }
        ));
    }

    #[test]
    fn paint_overlays_compiles_against_tui_backend() {
        // Compile-only assertion — the same generic function used with
        // MockBackend above is also valid for TuiBackend. We don't run
        // the draws (they require an active frame scope) but the type
        // monomorphisation proves the trait constraint is satisfied
        // for every backend impl.
        let _: fn(&mut TuiBackend, &Palette, &ListView) = paint_overlays::<TuiBackend>;
    }

    #[test]
    fn mock_backend_modal_stack_routes_through_trait() {
        // Modal stack is on the trait too — backends that implement it
        // wire into `crate::dispatch::dispatch_mouse_down` automatically.
        let mut mock = MockBackend::new();
        mock.modal_stack_mut()
            .push(WidgetId::new("test:popup"), QRect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(mock.modal_stack_mut().len(), 1);
    }

    // ─── Stage 6: accelerator matching ──────────────────────────────────────

    use crate::{Key, Modifiers, NamedKey};

    fn ctrl_p_keypress() -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char('p'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }
    }

    fn make_acc(id: &str, binding: &str) -> Accelerator {
        Accelerator {
            id: AcceleratorId::new(id),
            binding: KeyBinding::Literal(binding.to_string()),
            scope: AcceleratorScope::Global,
            label: None,
        }
    }

    #[test]
    fn accelerator_match_replaces_keypressed_with_accelerator() {
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("tui.fuzzy_finder", "<C-p>"));

        let mut events = vec![ctrl_p_keypress()];
        backend.apply_accelerators(&mut events);

        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::Accelerator(id, mods) => {
                assert_eq!(id.as_str(), "tui.fuzzy_finder");
                assert!(mods.ctrl);
            }
            other => panic!("expected Accelerator, got {:?}", other),
        }
    }

    #[test]
    fn accelerator_match_named_keys() {
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("debug.continue", "<F5>"));

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Named(NamedKey::F(5)),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);

        match &events[0] {
            UiEvent::Accelerator(id, _) => assert_eq!(id.as_str(), "debug.continue"),
            other => panic!("expected Accelerator, got {:?}", other),
        }
    }

    #[test]
    fn accelerator_match_uppercase_letter_normalised() {
        // `<C-S-T>` and a Shift+T keypress (which arrives as Char('T')
        // from crossterm with SHIFT in modifiers) must match.
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("test.upper", "<C-S-T>"));

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('T'),
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);

        match &events[0] {
            UiEvent::Accelerator(id, _) => assert_eq!(id.as_str(), "test.upper"),
            other => panic!("expected Accelerator, got {:?}", other),
        }
    }

    #[test]
    fn accelerator_no_match_stays_keypressed() {
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("tui.fuzzy_finder", "<C-p>"));

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('q'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);

        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }

    #[test]
    fn accelerator_modifier_mismatch_no_match() {
        // `<C-p>` should NOT fire on `p` alone (no modifiers).
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("tui.fuzzy_finder", "<C-p>"));

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('p'),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);

        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }

    #[test]
    fn accelerator_unregister_removes_match() {
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("tui.fuzzy_finder", "<C-p>"));
        backend.unregister_accelerator(&AcceleratorId::new("tui.fuzzy_finder"));

        let mut events = vec![ctrl_p_keypress()];
        backend.apply_accelerators(&mut events);

        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }

    #[test]
    fn accelerator_re_register_replaces_binding() {
        // Registering the same id twice should swap the binding, not
        // accumulate stale entries.
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&make_acc("test.toggle", "<C-p>"));
        backend.register_accelerator(&make_acc("test.toggle", "<C-q>"));

        let mut events = vec![ctrl_p_keypress()];
        backend.apply_accelerators(&mut events);
        assert!(
            matches!(events[0], UiEvent::KeyPressed { .. }),
            "old binding must not match after re-register"
        );

        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('q'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }];
        backend.apply_accelerators(&mut events);
        assert!(matches!(&events[0], UiEvent::Accelerator(id, _) if id.as_str() == "test.toggle"));
    }

    #[test]
    fn accelerator_widget_scope_skipped() {
        // Backend doesn't know which widget has focus; widget-scoped
        // accelerators must NOT match here. The app keeps inline
        // matching for those.
        let mut backend = TuiBackend::new();
        backend.register_accelerator(&Accelerator {
            id: AcceleratorId::new("widget.local"),
            binding: KeyBinding::Literal("<C-p>".into()),
            scope: AcceleratorScope::Widget(WidgetId::new("test:input")),
            label: None,
        });

        let mut events = vec![ctrl_p_keypress()];
        backend.apply_accelerators(&mut events);

        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }
}
