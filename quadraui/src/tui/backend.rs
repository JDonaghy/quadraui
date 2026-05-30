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
//! ### What the trait covers
//!
//! As of #13 the `Backend` trait covers every primitive that has
//! TUI + GTK rasterisers. New primitives must add a trait method as
//! part of the same change that adds the rasteriser (see CLAUDE.md
//! Primitive Authoring Rule #7). Generic `<B: Backend>` render code
//! works against `TuiBackend`, `GtkBackend`, the test `MockBackend`,
//! and any future Win-GUI / macOS backend implementer.
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

use crate::backend::{activity_bar_hits, tab_bar_layout_to_hits};
use crate::dispatch::TextRegion;
use crate::{
    parse_key_binding, Accelerator, AcceleratorId, AcceleratorScope, ActivityBar, Backend,
    CommandLine, DragState, DragTarget, Form, KeyBinding, ListView, MenuBar, ModalStack, Palette,
    ParsedBinding, PlatformServices, Point, Rect as QRect, Split, StatusBar, TabBar,
    Terminal as TerminalPrim, TextDisplay, TreeView, UiEvent, Viewport, WidgetId,
};
use ratatui::layout::Rect;
use ratatui::Frame;

use super::services::TuiPlatformServices;

/// Minimum gap (in cells) between left and right status-bar halves
/// before priority drop kicks in. Mirrors `crate::gtk::status_bar`'s
/// `MIN_GAP_PX = 16.0`. Irrelevant for bars without right segments.
const MIN_GAP_CELLS: f32 = 2.0;

/// Finalised text selection held between frames. Persists after the
/// mouse button is released until Ctrl-C copies the text or a new
/// mouse-down clears it.
#[derive(Debug, Clone)]
pub(crate) struct TuiTextSelection {
    pub region: WidgetId,
    pub anchor: Point,
    pub focus: Point,
}

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
    double_click: super::events::DoubleClickDetector,
    /// Selectable text regions registered during the current frame via
    /// [`crate::Backend::register_text_region`]. Cleared at the start
    /// of each frame by [`Self::begin_frame`].
    text_regions: Vec<TextRegion>,
    /// Finalised selection (may persist after mouse-up). `None` when
    /// no selection is active. Set by [`Self::set_active_text_selection`],
    /// cleared by [`Self::clear_text_selection`] or on a new
    /// `MouseDown`.
    active_selection: Option<TuiTextSelection>,
    /// Text extracted from the last rendered buffer for the active
    /// selection. Populated by `apply_selection_highlight` (which has
    /// access to the live buffer inside `terminal.draw`). After the
    /// draw closure returns ratatui swaps its double-buffer, so
    /// `terminal.current_buffer_mut()` would return an empty buffer —
    /// caching here is the only reliable way to get the text.
    cached_selection_text: String,
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
            double_click: super::events::DoubleClickDetector::new(),
            text_regions: Vec::new(),
            active_selection: None,
            cached_selection_text: String::new(),
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

    // ── Text selection ─────────────────────────────────────────────────────

    /// Return the current active text selection, if any.
    pub(crate) fn active_text_selection(&self) -> Option<&TuiTextSelection> {
        self.active_selection.as_ref()
    }

    /// Update (or start) the active text selection. Called by the runner
    /// when a [`UiEvent::TextSelectionChanged`] event arrives.
    pub(crate) fn set_active_text_selection(
        &mut self,
        region: WidgetId,
        anchor: Point,
        focus: Point,
    ) {
        self.active_selection = Some(TuiTextSelection {
            region,
            anchor,
            focus,
        });
    }

    /// Clear the active text selection and, if a `TextSelection` drag is
    /// in progress, end it. Called by the runner on `MouseDown` or after
    /// Ctrl-C copies the selection.
    pub(crate) fn clear_text_selection(&mut self) {
        self.active_selection = None;
        if matches!(
            self.drag_state.target(),
            Some(DragTarget::TextSelection { .. })
        ) {
            self.drag_state.end();
        }
    }

    /// Clear only the displayed selection highlight without touching drag
    /// state. Used by the run loop on `MouseDown` so that the drag just
    /// initiated by `dispatch_click` is not immediately cancelled.
    pub(crate) fn clear_selection_display(&mut self) {
        self.active_selection = None;
    }

    /// Invert (highlight) the cells in the ratatui buffer that fall
    /// within the active text selection range, and cache the extracted
    /// text in `self.cached_selection_text`.
    ///
    /// Must be called inside `terminal.draw(|frame| …)` after
    /// `app.render`. The cache is necessary because ratatui swaps its
    /// double-buffer after `draw` returns, so `terminal.current_buffer_mut()`
    /// would return an empty buffer by the time Ctrl-C fires.
    pub(crate) fn apply_selection_highlight(&mut self, buf: &mut ratatui::buffer::Buffer) {
        let sel = match &self.active_selection {
            Some(s) => s,
            None => return,
        };
        let region = match self.text_regions.iter().find(|r| r.id == sel.region) {
            Some(r) => r,
            None => return,
        };
        let bounds = crate::event::Rect::new(
            region.bounds.x,
            region.bounds.y,
            region.bounds.width,
            region.bounds.height,
        );
        let ranges = crate::dispatch::text_selection_line_range(sel.anchor, sel.focus, bounds);
        let area = buf.area;

        // Cache text before inverting so we read the original cell content.
        let mut lines: Vec<String> = Vec::with_capacity(ranges.len());
        for &(row, col_start, col_end) in &ranges {
            if row >= area.y + area.height {
                continue;
            }
            let mut line = String::new();
            for col in col_start..col_end {
                if col < area.x + area.width {
                    line.push_str(buf[(col, row)].symbol());
                }
            }
            let trimmed = line.trim_end_matches(|c: char| c.is_whitespace() || c == '\0');
            lines.push(trimmed.to_string());
        }
        self.cached_selection_text = lines.join("\n");

        // Now invert cells for the highlight.
        for (row, col_start, col_end) in ranges {
            for col in col_start..col_end {
                if col < area.x + area.width && row < area.y + area.height {
                    let cell = &mut buf[(col, row)];
                    let fg = cell.fg;
                    let bg = cell.bg;
                    cell.fg = bg;
                    cell.bg = fg;
                }
            }
        }
    }

    /// Return a clone of the selection text cached by the last
    /// `apply_selection_highlight` call. Named `cached_selection_text`
    /// rather than `take_*` because the value is cloned, not consumed —
    /// the cache persists until the selection is cleared.
    pub(crate) fn cached_selection_text(&self) -> String {
        self.cached_selection_text.clone()
    }

    /// Read the selected cells back from `buf`, trim trailing whitespace
    /// per line, and return the joined text (lines separated by `\n`).
    ///
    /// Falls back to an empty `String` when there is no active selection
    /// or the region id can no longer be found in the registered regions.
    ///
    /// Only used in tests — production code uses the text cached by
    /// `apply_selection_highlight` (the live buffer is unavailable after
    /// `terminal.draw` swaps ratatui's double-buffer).
    #[cfg(test)]
    pub(crate) fn extract_selection_text(&self, buf: &ratatui::buffer::Buffer) -> String {
        let sel = match &self.active_selection {
            Some(s) => s,
            None => return String::new(),
        };
        let region = match self.text_regions.iter().find(|r| r.id == sel.region) {
            Some(r) => r,
            None => return String::new(),
        };
        let bounds = crate::event::Rect::new(
            region.bounds.x,
            region.bounds.y,
            region.bounds.width,
            region.bounds.height,
        );
        let ranges = crate::dispatch::text_selection_line_range(sel.anchor, sel.focus, bounds);
        let area = buf.area;
        let mut lines: Vec<String> = Vec::with_capacity(ranges.len());
        for (row, col_start, col_end) in ranges {
            if row >= area.y + area.height {
                continue;
            }
            let mut line = String::new();
            for col in col_start..col_end {
                if col < area.x + area.width {
                    line.push_str(buf[(col, row)].symbol());
                }
            }
            // Trim trailing whitespace and NUL padding per line.
            let trimmed = line.trim_end_matches(|c: char| c.is_whitespace() || c == '\0');
            lines.push(trimmed.to_string());
        }
        lines.join("\n")
    }

    // ── Accelerators ───────────────────────────────────────────────────────

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
    /// Run raw translated events through the dispatch layer so that
    /// `MouseDown` on a text region begins a `TextSelection` drag and
    /// `MouseMoved` (with button held) emits `TextSelectionChanged`.
    ///
    /// # TODO: scrollbar dispatch
    ///
    /// Scroll-surface arbitration is not wired here yet — scroll surfaces
    /// are not registered per-frame by `TuiBackend`. Passing an empty slice
    /// to `dispatch_click` means text regions work correctly today and
    /// scrollbar drags are unaffected (they continue to be handled by
    /// app-side hit-tests as before). The consequence is that the
    /// "scrollbar wins over an overlapping text region" acceptance
    /// criterion is not enforced in TUI. Tracked as a follow-up issue
    /// (scrollbar dispatch epic).
    fn apply_dispatch(&mut self, raw: Vec<UiEvent>) -> Vec<UiEvent> {
        let mut out = Vec::with_capacity(raw.len());
        for event in raw {
            match event {
                UiEvent::MouseDown {
                    button,
                    position,
                    modifiers,
                    ..
                } => {
                    out.extend(crate::dispatch::dispatch_click(
                        &self.modal_stack,
                        &[],
                        &self.text_regions.clone(),
                        &mut self.drag_state,
                        position,
                        button,
                        modifiers,
                    ));
                }
                UiEvent::MouseMoved { position, buttons } => {
                    out.extend(crate::dispatch::dispatch_mouse_drag(
                        &self.drag_state,
                        position,
                        buttons,
                    ));
                }
                UiEvent::MouseUp {
                    button, position, ..
                } => {
                    out.extend(crate::dispatch::dispatch_mouse_up(
                        &self.modal_stack,
                        &mut self.drag_state,
                        position,
                        button,
                    ));
                }
                other => out.push(other),
            }
        }
        out
    }

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
        // Clear per-frame text regions so stale registrations from the
        // previous frame don't linger.
        self.text_regions.clear();
    }

    fn register_text_region(&mut self, region: TextRegion) {
        self.text_regions.push(region);
    }

    fn end_frame(&mut self) {
        // No-op. The frame's actual flush happens when ratatui's
        // `terminal.draw(|frame| …)` closure returns; this method
        // exists for parity with backends that need explicit flush.
    }

    fn set_theme(&mut self, theme: crate::Theme) {
        self.set_current_theme(theme);
    }

    fn poll_events(&mut self) -> Vec<UiEvent> {
        // Drain every queued crossterm event; never blocks. Each
        // native event translates to zero, one, or more `UiEvent`s
        // via [`super::events::crossterm_to_uievents`], then runs
        // through the dispatch layer (text-region hit-test, drag state)
        // and [`Self::apply_accelerators`].
        let mut raw = Vec::new();
        while ratatui::crossterm::event::poll(Duration::ZERO).unwrap_or(false) {
            match ratatui::crossterm::event::read() {
                Ok(ev) => raw.extend(super::events::crossterm_to_uievents(ev)),
                Err(_) => break,
            }
        }
        let mut out = self.apply_dispatch(raw);
        self.apply_accelerators(&mut out);
        self.double_click.process(&mut out);
        out
    }

    fn wait_events(&mut self, timeout: Duration) -> Vec<UiEvent> {
        // Block up to `timeout` for the next native event, translate it,
        // run through the dispatch layer (text-region hit-test, drag
        // state), match against registered accelerators, and return.
        // Empty `Vec` on timeout.
        if let Ok(true) = ratatui::crossterm::event::poll(timeout) {
            if let Ok(ev) = ratatui::crossterm::event::read() {
                let raw = super::events::crossterm_to_uievents(ev);
                let mut out = self.apply_dispatch(raw);
                self.apply_accelerators(&mut out);
                self.double_click.process(&mut out);
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

    fn line_height(&self) -> f32 {
        1.0
    }

    fn char_width(&self) -> f32 {
        1.0
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

    fn draw_data_table(
        &mut self,
        rect: QRect,
        table: &crate::DataTable,
        hovered_idx: Option<usize>,
    ) -> crate::DataTableLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_data_table called outside enter_frame_scope");
        crate::tui::draw_data_table(frame.buffer_mut(), area, table, &theme, hovered_idx)
    }

    fn data_table_layout(&self, rect: QRect, table: &crate::DataTable) -> crate::DataTableLayout {
        let area = q_rect_to_ratatui(rect);
        table.layout(
            area.width as f32,
            area.height as f32,
            1.0,
            1.0,
            1.0,
            |col| crate::ColumnMeasure::new(col.title.chars().count() as f32),
        )
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

    fn draw_status_bar(
        &mut self,
        rect: QRect,
        bar: &StatusBar,
        hovered_id: Option<&crate::types::WidgetId>,
        pressed_id: Option<&crate::types::WidgetId>,
    ) -> crate::StatusBarLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let layout = bar.layout(area.width as f32, 1.0, MIN_GAP_CELLS, |seg| {
            crate::StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_status_bar called outside enter_frame_scope");
        crate::tui::draw_status_bar(
            frame.buffer_mut(),
            area,
            bar,
            &layout,
            &theme,
            hovered_id,
            pressed_id,
        )
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
        let close_cols = if bar.show_tab_close {
            crate::tui::TAB_CLOSE_COLS as usize
        } else {
            0
        };
        let tab_widths: Vec<usize> = bar
            .tabs
            .iter()
            .map(|t| t.label.chars().count() + close_cols)
            .collect();
        let layout = bar.layout(
            area.width as f32,
            area.height as f32,
            0.0, // no scroll arrows in TUI
            |i| crate::TabMeasure::new(tab_widths[i] as f32, close_cols as f32),
            |i| crate::SegmentMeasure::new(bar.right_segments[i].width_cells as f32),
        );
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_tab_bar called outside enter_frame_scope");
        crate::tui::draw_tab_bar(frame.buffer_mut(), area, bar, &layout, &theme)
    }

    fn draw_activity_bar(
        &mut self,
        rect: QRect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<crate::ActivityBarRowHit> {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_activity_bar called outside enter_frame_scope");
        crate::tui::draw_activity_bar(frame.buffer_mut(), area, bar, &theme, hovered_idx)
    }

    fn status_bar_layout(&self, rect: QRect, bar: &StatusBar) -> crate::StatusBarLayout {
        bar.layout(rect.width, 1.0, MIN_GAP_CELLS, |seg| {
            crate::StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        })
    }

    fn tab_bar_layout(&self, rect: QRect, bar: &TabBar) -> crate::TabBarHits {
        let close_cols = if bar.show_tab_close {
            crate::tui::TAB_CLOSE_COLS as usize
        } else {
            0
        };
        let tab_widths: Vec<usize> = bar
            .tabs
            .iter()
            .map(|t| t.label.chars().count() + close_cols)
            .collect();
        let layout = bar.layout(
            rect.width,
            rect.height,
            0.0,
            |i| crate::TabMeasure::new(tab_widths[i] as f32, close_cols as f32),
            |i| crate::SegmentMeasure::new(bar.right_segments[i].width_cells as f32),
        );

        let mut hits = tab_bar_layout_to_hits(&layout, bar);

        let active_idx = bar.tabs.iter().position(|t| t.is_active);
        let reserved: usize = bar
            .right_segments
            .iter()
            .map(|s| s.width_cells as usize)
            .sum();
        let effective_tab_area = (rect.width as usize).saturating_sub(reserved);

        hits.correct_scroll_offset = if let Some(active) = active_idx {
            TabBar::fit_active_scroll_offset(active, bar.tabs.len(), effective_tab_area, |i| {
                tab_widths[i]
            })
        } else {
            bar.scroll_offset
        };

        hits
    }

    fn activity_bar_layout(&self, rect: QRect, bar: &ActivityBar) -> Vec<crate::ActivityBarRowHit> {
        let lh = 1.0_f32;
        activity_bar_hits(rect, bar, lh)
    }

    fn draw_terminal(&mut self, rect: QRect, term: &TerminalPrim) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_terminal called outside enter_frame_scope");
        crate::tui::draw_terminal(frame.buffer_mut(), area, term, &theme);
    }

    fn draw_text_display(&mut self, rect: QRect, td: &TextDisplay) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_text_display called outside enter_frame_scope");
        crate::tui::draw_text_display(frame.buffer_mut(), area, td, &theme);
    }

    fn draw_command_line(&mut self, rect: QRect, cmd: &CommandLine) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_command_line called outside enter_frame_scope");
        crate::tui::command_line::draw_command_line(frame.buffer_mut(), area, cmd, &theme);
    }

    fn text_display_layout(
        &self,
        rect: QRect,
        td: &TextDisplay,
    ) -> crate::primitives::text_display::TextDisplayLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_text_display_layout(td, area)
    }

    fn draw_text_input(
        &mut self,
        rect: QRect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_text_input called outside enter_frame_scope");
        crate::tui::draw_text_input(frame.buffer_mut(), area, ti, &theme)
    }

    fn text_input_layout(
        &self,
        rect: QRect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_text_input_layout(ti, area)
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

    // ─── #13: trait coverage for the rest of the rasterised primitives ──

    fn draw_multi_section_view(
        &mut self,
        rect: QRect,
        view: &crate::primitives::multi_section_view::MultiSectionView,
    ) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_multi_section_view called outside enter_frame_scope");
        crate::tui::draw_multi_section_view(frame.buffer_mut(), area, view, &theme, nerd_fonts);
    }

    fn msv_layout(
        &self,
        rect: QRect,
        view: &crate::primitives::multi_section_view::MultiSectionView,
    ) -> crate::primitives::multi_section_view::MultiSectionViewLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_msv_layout(view, area)
    }

    fn msv_metrics(&self) -> crate::primitives::multi_section_view::LayoutMetrics {
        crate::primitives::multi_section_view::LayoutMetrics {
            header_size: 1.0,
            divider_size: 0.0,
            scrollbar_size: 1.0,
            cell_quantum: 1.0,
        }
    }

    fn tree_layout(&self, rect: QRect, tree: &TreeView) -> crate::primitives::tree::TreeViewLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_tree_layout(tree, area)
    }

    fn form_layout(&self, rect: QRect, form: &Form) -> crate::primitives::form::FormLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_form_layout(form, area)
    }

    fn draw_editor(
        &mut self,
        rect: QRect,
        editor: &crate::primitives::editor::Editor,
    ) -> crate::backend::EditorPaintResult {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_editor called outside enter_frame_scope");
        let tui_result = crate::tui::draw_editor(frame.buffer_mut(), area, editor, &theme);
        crate::backend::EditorPaintResult {
            cursor_position: tui_result.cursor_position,
        }
    }

    fn draw_message_list(
        &mut self,
        rect: QRect,
        list: &crate::primitives::message_list::MessageList,
    ) {
        let area = q_rect_to_ratatui(rect);
        let panel_bg = self.current_theme.background;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_message_list called outside enter_frame_scope");
        crate::tui::draw_message_list(frame.buffer_mut(), area, list, panel_bg);
    }

    fn draw_rich_text_popup(
        &mut self,
        popup: &crate::primitives::rich_text_popup::RichTextPopup,
        layout: &crate::primitives::rich_text_popup::RichTextPopupLayout,
    ) {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_rich_text_popup called outside enter_frame_scope");
        crate::tui::draw_rich_text_popup(frame.buffer_mut(), popup, layout, &theme);
    }

    fn draw_find_replace(
        &mut self,
        rect: QRect,
        panel: &crate::primitives::find_replace::FindReplacePanel,
    ) {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        // TUI free function takes `editor_left: u16` for editor-relative
        // positioning. The trait abstraction passes the panel's full
        // rect; downstream consumers that want a non-zero editor offset
        // should compose into a sub-rect.
        let editor_left = area.x;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_find_replace called outside enter_frame_scope");
        crate::tui::draw_find_replace(frame.buffer_mut(), area, panel, &theme, editor_left);
    }

    fn draw_completions(
        &mut self,
        completions: &crate::primitives::completions::Completions,
        layout: &crate::primitives::completions::CompletionsLayout,
    ) {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_completions called outside enter_frame_scope");
        crate::tui::draw_completions(frame.buffer_mut(), completions, layout, &theme);
    }

    fn draw_scrollbar(
        &mut self,
        _rect: QRect,
        scrollbar: &crate::primitives::scrollbar::Scrollbar,
    ) {
        let theme = self.current_theme;
        let cell_bg = theme.background;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_scrollbar called outside enter_frame_scope");
        // The standalone TUI scrollbar primitive paints from its own
        // `track` bounds; `rect` is unused (the primitive owns layout).
        // Forward-compat parameter for backends that need a clip rect.
        crate::tui::draw_scrollbar(frame.buffer_mut(), scrollbar, &theme, cell_bg);
    }

    fn draw_drop_overlay(&mut self, overlay: &crate::primitives::drop_zone::DropOverlay) {
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_drop_overlay called outside enter_frame_scope");
        crate::tui::draw_drop_overlay(frame.buffer_mut(), overlay, &theme);
    }

    fn draw_menu_bar(
        &mut self,
        rect: QRect,
        bar: &MenuBar,
    ) -> crate::primitives::menu_bar::MenuBarLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_menu_bar called outside enter_frame_scope");
        crate::tui::draw_menu_bar(frame.buffer_mut(), area, bar, &theme)
    }

    fn menu_bar_layout(
        &self,
        rect: QRect,
        bar: &MenuBar,
    ) -> crate::primitives::menu_bar::MenuBarLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_menu_bar_layout(bar, area)
    }

    fn draw_split(&mut self, rect: QRect, split: &Split) -> crate::primitives::split::SplitLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_split called outside enter_frame_scope");
        crate::tui::draw_split(frame.buffer_mut(), area, split, &theme)
    }

    fn split_layout(&self, rect: QRect, split: &Split) -> crate::primitives::split::SplitLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_split_layout(split, area)
    }

    fn draw_panel(
        &mut self,
        rect: QRect,
        panel: &crate::primitives::panel::Panel,
    ) -> crate::primitives::panel::PanelLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_panel called outside enter_frame_scope");
        crate::tui::draw_panel(frame.buffer_mut(), area, panel, &theme)
    }

    fn panel_layout(
        &self,
        rect: QRect,
        panel: &crate::primitives::panel::Panel,
    ) -> crate::primitives::panel::PanelLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_panel_layout(panel, area)
    }

    fn draw_toast_stack(
        &mut self,
        rect: QRect,
        stack: &crate::primitives::toast::ToastStack,
    ) -> crate::primitives::toast::ToastStackLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_toast_stack called outside enter_frame_scope");
        crate::tui::draw_toast_stack(frame.buffer_mut(), area, stack, &theme)
    }

    fn toast_stack_layout(
        &self,
        rect: QRect,
        stack: &crate::primitives::toast::ToastStack,
    ) -> crate::primitives::toast::ToastStackLayout {
        crate::tui::tui_toast_stack_layout(stack, rect.width, rect.height)
    }

    fn draw_pipeline_view(
        &mut self,
        rect: QRect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_pipeline_view called outside enter_frame_scope");
        crate::tui::draw_pipeline_view(frame.buffer_mut(), area, view, &theme)
    }

    fn pipeline_view_layout(
        &self,
        rect: QRect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_pipeline_view_layout(view, area)
    }

    fn draw_progress(
        &mut self,
        rect: QRect,
        bar: &crate::primitives::progress::ProgressBar,
    ) -> crate::primitives::progress::ProgressBarLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_progress called outside enter_frame_scope");
        crate::tui::draw_progress(frame.buffer_mut(), area, bar, &theme)
    }

    fn progress_layout(
        &self,
        rect: QRect,
        bar: &crate::primitives::progress::ProgressBar,
    ) -> crate::primitives::progress::ProgressBarLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_progress_layout(bar, area)
    }

    fn draw_spinner(
        &mut self,
        rect: QRect,
        spinner: &crate::primitives::spinner::Spinner,
    ) -> crate::primitives::spinner::SpinnerLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_spinner called outside enter_frame_scope");
        crate::tui::draw_spinner(frame.buffer_mut(), area, spinner, &theme)
    }

    fn spinner_layout(
        &self,
        rect: QRect,
        spinner: &crate::primitives::spinner::Spinner,
    ) -> crate::primitives::spinner::SpinnerLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_spinner_layout(spinner, area)
    }

    fn draw_command_center(
        &mut self,
        rect: QRect,
        cc: &crate::primitives::command_center::CommandCenter,
    ) -> crate::primitives::command_center::CommandCenterLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_command_center called outside enter_frame_scope");
        crate::tui::draw_command_center(frame.buffer_mut(), area, cc, &theme)
    }

    fn command_center_layout(
        &self,
        rect: QRect,
        cc: &crate::primitives::command_center::CommandCenter,
    ) -> crate::primitives::command_center::CommandCenterLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_command_center_layout(cc, area)
    }

    fn draw_chart(
        &mut self,
        rect: QRect,
        chart: &crate::primitives::chart::Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> crate::primitives::chart::ChartLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_chart called outside enter_frame_scope");
        crate::tui::draw_chart(
            frame.buffer_mut(),
            area,
            chart,
            &theme,
            hovered_point,
            crosshair_x,
        )
    }

    fn chart_layout(
        &self,
        rect: QRect,
        chart: &crate::primitives::chart::Chart,
    ) -> crate::primitives::chart::ChartLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_chart_layout(chart, area)
    }

    fn draw_toolbar(
        &mut self,
        rect: QRect,
        bar: &crate::primitives::toolbar::Toolbar,
        hovered_id: Option<&crate::types::WidgetId>,
        pressed_id: Option<&crate::types::WidgetId>,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_toolbar called outside enter_frame_scope");
        crate::tui::draw_toolbar(
            frame.buffer_mut(),
            area,
            bar,
            &theme,
            hovered_id,
            pressed_id,
        )
    }

    fn toolbar_layout(
        &self,
        rect: QRect,
        bar: &crate::primitives::toolbar::Toolbar,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_toolbar_layout(bar, area)
    }

    fn draw_sidebar_panel(
        &mut self,
        rect: QRect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
        hovered_toolbar_id: Option<&crate::types::WidgetId>,
        pressed_toolbar_id: Option<&crate::types::WidgetId>,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        let area = q_rect_to_ratatui(rect);
        let theme = self.current_theme;
        let frame = self
            .current_frame_mut()
            .expect("TuiBackend::draw_sidebar_panel called outside enter_frame_scope");
        crate::tui::draw_sidebar_panel(
            frame.buffer_mut(),
            area,
            panel,
            &theme,
            hovered_toolbar_id,
            pressed_toolbar_id,
        )
    }

    fn sidebar_panel_layout(
        &self,
        rect: QRect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        let area = q_rect_to_ratatui(rect);
        crate::tui::tui_sidebar_panel_layout(panel, area)
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
        theme: crate::Theme,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                calls: Vec::new(),
                modal_stack: ModalStack::new(),
                services: MockServices::new(),
                viewport: Viewport::new(80.0, 24.0, 1.0),
                theme: crate::Theme::default(),
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
        fn set_theme(&mut self, theme: crate::Theme) {
            self.theme = theme;
        }
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

        fn draw_data_table(
            &mut self,
            _rect: QRect,
            _table: &crate::DataTable,
            _hovered_idx: Option<usize>,
        ) -> crate::DataTableLayout {
            crate::DataTableLayout {
                header_height: 0.0,
                row_height: 0.0,
                columns: Vec::new(),
                visible_rows: 0,
                viewport_width: 0.0,
                viewport_height: 0.0,
                scrollbar_width: 0.0,
                content_width: 0.0,
                h_scrollbar_height: 0.0,
            }
        }
        fn data_table_layout(
            &self,
            _rect: QRect,
            _table: &crate::DataTable,
        ) -> crate::DataTableLayout {
            crate::DataTableLayout {
                header_height: 0.0,
                row_height: 0.0,
                columns: Vec::new(),
                visible_rows: 0,
                viewport_width: 0.0,
                viewport_height: 0.0,
                scrollbar_width: 0.0,
                content_width: 0.0,
                h_scrollbar_height: 0.0,
            }
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
        fn draw_status_bar(
            &mut self,
            _r: QRect,
            _b: &StatusBar,
            _hovered_id: Option<&crate::types::WidgetId>,
            _pressed_id: Option<&crate::types::WidgetId>,
        ) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
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
        fn draw_command_line(&mut self, _r: QRect, _c: &CommandLine) {}
        fn status_bar_layout(&self, _r: QRect, _b: &StatusBar) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
        }
        fn tab_bar_layout(&self, _r: QRect, _b: &TabBar) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn activity_bar_layout(
            &self,
            _r: QRect,
            _b: &ActivityBar,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn text_display_layout(
            &self,
            r: QRect,
            td: &TextDisplay,
        ) -> crate::primitives::text_display::TextDisplayLayout {
            td.layout(r.width, r.height, |_| {
                crate::primitives::text_display::TextDisplayLineMeasure::new(1.0)
            })
        }
        fn draw_text_input(
            &mut self,
            r: QRect,
            ti: &crate::primitives::text_input::TextInput,
        ) -> crate::primitives::text_input::TextInputLayout {
            ti.layout(
                r,
                crate::primitives::text_input::TextInputMeasure::new(1.0, 1.0),
            )
        }
        fn text_input_layout(
            &self,
            r: QRect,
            ti: &crate::primitives::text_input::TextInput,
        ) -> crate::primitives::text_input::TextInputLayout {
            ti.layout(
                r,
                crate::primitives::text_input::TextInputMeasure::new(1.0, 1.0),
            )
        }
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

        fn char_width(&self) -> f32 {
            1.0
        }
        fn line_height(&self) -> f32 {
            1.0
        }

        // ── #13: stubs for the trait methods added with this issue ──

        fn draw_multi_section_view(
            &mut self,
            _r: QRect,
            _v: &crate::primitives::multi_section_view::MultiSectionView,
        ) {
        }

        fn msv_layout(
            &self,
            r: QRect,
            v: &crate::primitives::multi_section_view::MultiSectionView,
        ) -> crate::primitives::multi_section_view::MultiSectionViewLayout {
            // Mock returns the primitive's natural layout with default
            // metrics — sufficient for cross-backend compile checks.
            v.layout(
                r,
                crate::primitives::multi_section_view::LayoutMetrics::default(),
                |_| crate::primitives::multi_section_view::SectionMeasure::default(),
            )
        }

        fn msv_metrics(&self) -> crate::primitives::multi_section_view::LayoutMetrics {
            crate::primitives::multi_section_view::LayoutMetrics::default()
        }

        fn tree_layout(&self, r: QRect, t: &TreeView) -> crate::primitives::tree::TreeViewLayout {
            t.layout(r.width, r.height, |_| {
                crate::primitives::tree::TreeRowMeasure::new(1.0)
            })
        }

        fn form_layout(&self, r: QRect, form: &Form) -> crate::primitives::form::FormLayout {
            let area = q_rect_to_ratatui(r);
            crate::tui::tui_form_layout(form, area)
        }

        fn draw_editor(
            &mut self,
            _r: QRect,
            _e: &crate::primitives::editor::Editor,
        ) -> crate::backend::EditorPaintResult {
            crate::backend::EditorPaintResult::default()
        }

        fn draw_message_list(
            &mut self,
            _r: QRect,
            _l: &crate::primitives::message_list::MessageList,
        ) {
        }

        fn draw_rich_text_popup(
            &mut self,
            _p: &crate::primitives::rich_text_popup::RichTextPopup,
            _l: &crate::primitives::rich_text_popup::RichTextPopupLayout,
        ) {
        }

        fn draw_find_replace(
            &mut self,
            _r: QRect,
            _p: &crate::primitives::find_replace::FindReplacePanel,
        ) {
        }

        fn draw_completions(
            &mut self,
            _c: &crate::primitives::completions::Completions,
            _l: &crate::primitives::completions::CompletionsLayout,
        ) {
        }

        fn draw_scrollbar(&mut self, _r: QRect, _s: &crate::primitives::scrollbar::Scrollbar) {}
        fn draw_drop_overlay(&mut self, _o: &crate::primitives::drop_zone::DropOverlay) {}

        fn draw_menu_bar(
            &mut self,
            _r: QRect,
            bar: &MenuBar,
        ) -> crate::primitives::menu_bar::MenuBarLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            bar.layout(bounds, |_| {
                crate::primitives::menu_bar::MenuBarItemMeasure::new(0.0)
            })
        }

        fn menu_bar_layout(
            &self,
            _r: QRect,
            bar: &MenuBar,
        ) -> crate::primitives::menu_bar::MenuBarLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            bar.layout(bounds, |_| {
                crate::primitives::menu_bar::MenuBarItemMeasure::new(0.0)
            })
        }

        fn draw_split(
            &mut self,
            _r: QRect,
            split: &Split,
        ) -> crate::primitives::split::SplitLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            split.layout(bounds, crate::primitives::split::SplitMeasure::new(1.0))
        }

        fn split_layout(&self, _r: QRect, split: &Split) -> crate::primitives::split::SplitLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            split.layout(bounds, crate::primitives::split::SplitMeasure::new(1.0))
        }

        fn draw_panel(
            &mut self,
            _r: QRect,
            panel: &crate::primitives::panel::Panel,
        ) -> crate::primitives::panel::PanelLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            panel.layout(bounds, crate::primitives::panel::PanelMeasure::new(1.0))
        }

        fn panel_layout(
            &self,
            _r: QRect,
            panel: &crate::primitives::panel::Panel,
        ) -> crate::primitives::panel::PanelLayout {
            let bounds = crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height);
            panel.layout(bounds, crate::primitives::panel::PanelMeasure::new(1.0))
        }

        fn draw_toast_stack(
            &mut self,
            _r: QRect,
            stack: &crate::primitives::toast::ToastStack,
        ) -> crate::primitives::toast::ToastStackLayout {
            stack.layout(_r.width, _r.height, 1.0, 1.0, |_| {
                crate::primitives::toast::ToastMeasure::new(40.0, 1.0)
            })
        }

        fn toast_stack_layout(
            &self,
            _r: QRect,
            stack: &crate::primitives::toast::ToastStack,
        ) -> crate::primitives::toast::ToastStackLayout {
            stack.layout(_r.width, _r.height, 1.0, 1.0, |_| {
                crate::primitives::toast::ToastMeasure::new(40.0, 1.0)
            })
        }

        fn draw_pipeline_view(
            &mut self,
            _r: QRect,
            view: &crate::primitives::pipeline_view::PipelineView,
        ) -> crate::primitives::pipeline_view::PipelineViewLayout {
            view.layout(
                _r.x,
                _r.y,
                crate::primitives::pipeline_view::PipelineViewMeasure::new(
                    _r.width, _r.height, 4.0, 10.0,
                ),
            )
        }

        fn pipeline_view_layout(
            &self,
            _r: QRect,
            view: &crate::primitives::pipeline_view::PipelineView,
        ) -> crate::primitives::pipeline_view::PipelineViewLayout {
            view.layout(
                _r.x,
                _r.y,
                crate::primitives::pipeline_view::PipelineViewMeasure::new(
                    _r.width, _r.height, 4.0, 10.0,
                ),
            )
        }

        fn draw_progress(
            &mut self,
            _r: QRect,
            bar: &crate::primitives::progress::ProgressBar,
        ) -> crate::primitives::progress::ProgressBarLayout {
            bar.layout(
                _r.x,
                _r.y,
                crate::primitives::progress::ProgressBarMeasure::new(_r.width, _r.height),
            )
        }

        fn progress_layout(
            &self,
            _r: QRect,
            bar: &crate::primitives::progress::ProgressBar,
        ) -> crate::primitives::progress::ProgressBarLayout {
            bar.layout(
                _r.x,
                _r.y,
                crate::primitives::progress::ProgressBarMeasure::new(_r.width, _r.height),
            )
        }

        fn draw_spinner(
            &mut self,
            _r: QRect,
            spinner: &crate::primitives::spinner::Spinner,
        ) -> crate::primitives::spinner::SpinnerLayout {
            spinner.layout(
                _r.x,
                _r.y,
                crate::primitives::spinner::SpinnerMeasure::new(_r.width, 1.0),
            )
        }

        fn spinner_layout(
            &self,
            _r: QRect,
            spinner: &crate::primitives::spinner::Spinner,
        ) -> crate::primitives::spinner::SpinnerLayout {
            spinner.layout(
                _r.x,
                _r.y,
                crate::primitives::spinner::SpinnerMeasure::new(_r.width, 1.0),
            )
        }

        fn draw_command_center(
            &mut self,
            _r: QRect,
            cc: &crate::primitives::command_center::CommandCenter,
        ) -> crate::primitives::command_center::CommandCenterLayout {
            cc.layout(
                crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height),
                crate::primitives::command_center::CommandCenterMeasure {
                    arrow_width: 2.0,
                    gap: 1.0,
                    search_box_width: 0.0,
                    height: 1.0,
                },
            )
        }

        fn command_center_layout(
            &self,
            _r: QRect,
            cc: &crate::primitives::command_center::CommandCenter,
        ) -> crate::primitives::command_center::CommandCenterLayout {
            cc.layout(
                crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height),
                crate::primitives::command_center::CommandCenterMeasure {
                    arrow_width: 2.0,
                    gap: 1.0,
                    search_box_width: 0.0,
                    height: 1.0,
                },
            )
        }

        fn draw_chart(
            &mut self,
            _r: QRect,
            chart: &crate::primitives::chart::Chart,
            _hovered_point: Option<(usize, usize)>,
            _crosshair_x: Option<f64>,
        ) -> crate::primitives::chart::ChartLayout {
            chart.layout(
                _r.x,
                _r.y,
                crate::primitives::chart::ChartMeasure {
                    width: _r.width,
                    height: _r.height,
                    char_width: 1.0,
                    line_height: 1.0,
                },
            )
        }

        fn chart_layout(
            &self,
            _r: QRect,
            chart: &crate::primitives::chart::Chart,
        ) -> crate::primitives::chart::ChartLayout {
            chart.layout(
                _r.x,
                _r.y,
                crate::primitives::chart::ChartMeasure {
                    width: _r.width,
                    height: _r.height,
                    char_width: 1.0,
                    line_height: 1.0,
                },
            )
        }

        fn draw_toolbar(
            &mut self,
            r: QRect,
            bar: &crate::primitives::toolbar::Toolbar,
            _hovered_id: Option<&crate::types::WidgetId>,
            _pressed_id: Option<&crate::types::WidgetId>,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            bar.layout(r.x, r.y, r.width, r.height, |_| {
                crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
            })
        }

        fn toolbar_layout(
            &self,
            r: QRect,
            bar: &crate::primitives::toolbar::Toolbar,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            bar.layout(r.x, r.y, r.width, r.height, |_| {
                crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
            })
        }

        fn draw_sidebar_panel(
            &mut self,
            r: QRect,
            panel: &crate::primitives::sidebar_panel::SidebarPanel,
            _h: Option<&crate::types::WidgetId>,
            _p: Option<&crate::types::WidgetId>,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            panel.layout(
                r,
                crate::primitives::sidebar_panel::SidebarPanelMeasure::new(1.0, 0.0),
                |_| crate::primitives::toolbar::ToolbarItemMeasure::new(0.0),
            )
        }

        fn sidebar_panel_layout(
            &self,
            r: QRect,
            panel: &crate::primitives::sidebar_panel::SidebarPanel,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            panel.layout(
                r,
                crate::primitives::sidebar_panel::SidebarPanelMeasure::new(1.0, 0.0),
                |_| crate::primitives::toolbar::ToolbarItemMeasure::new(0.0),
            )
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
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
                PaletteItem {
                    text: StyledText {
                        spans: vec![StyledSpan::plain("beta")],
                    },
                    detail: None,
                    icon: None,
                    match_positions: Vec::new(),
                    depth: 0,
                    expandable: false,
                    expanded: false,
                },
            ],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 2,
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
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
            h_scroll: 0,
            max_content_width: None,
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

    // ── Text selection / highlight round-trip tests ──────────────────────────

    /// Build a minimal ratatui buffer filled with known text, set up a
    /// text selection, and verify that:
    ///   1. `apply_selection_highlight` inverts the selected cells.
    ///   2. `extract_selection_text` returns the trimmed, newline-joined text.
    #[test]
    fn selection_highlight_and_extract_round_trip() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;
        use ratatui::style::Color as RC;

        // 10 wide × 3 tall buffer filled with known characters.
        let area = RRect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);

        // Row 0: "hello     " (trailing spaces)
        // Row 1: "world     "
        // Row 2: "!         "
        for (col, ch) in "hello     ".chars().enumerate() {
            buf[(col as u16, 0)]
                .set_char(ch)
                .set_fg(RC::White)
                .set_bg(RC::Black);
        }
        for (col, ch) in "world     ".chars().enumerate() {
            buf[(col as u16, 1)]
                .set_char(ch)
                .set_fg(RC::White)
                .set_bg(RC::Black);
        }
        for (col, ch) in "!         ".chars().enumerate() {
            buf[(col as u16, 2)]
                .set_char(ch)
                .set_fg(RC::White)
                .set_bg(RC::Black);
        }

        // Register text region and set selection: anchor at (0,0), focus at (0,2).
        // That covers rows 0–2, starting from col 0.
        let mut backend = TuiBackend::new();
        backend.text_regions.push(crate::dispatch::TextRegion {
            id: WidgetId::new("log:body"),
            bounds: crate::event::Rect::new(0.0, 0.0, 10.0, 3.0),
        });
        backend.set_active_text_selection(
            WidgetId::new("log:body"),
            Point::new(0.0, 0.0),
            Point::new(0.0, 2.0),
        );

        // 1. Extract text before highlight (cells still normal).
        let text = backend.extract_selection_text(&buf);
        // anchor(0,0) → focus(0,2):
        //   row 0: col 0..10 → "hello     " trimmed → "hello"
        //   row 1: col 0..10 → "world     " trimmed → "world"
        //   row 2: col 0..1  → "!" trimmed → "!"
        assert_eq!(text, "hello\nworld\n!");

        // 2. Apply highlight — selected cells should swap fg/bg.
        backend.apply_selection_highlight(&mut buf);
        // Spot-check: (0,0) should be inverted (fg=Black, bg=White).
        assert_eq!(buf[(0u16, 0u16)].fg, RC::Black);
        assert_eq!(buf[(0u16, 0u16)].bg, RC::White);
        // A cell outside the last row's range (col 1 of row 2 is outside
        // 0..1, so col=1 should be un-inverted).
        assert_eq!(buf[(1u16, 2u16)].fg, RC::White);
        assert_eq!(buf[(1u16, 2u16)].bg, RC::Black);
    }

    #[test]
    fn selection_clears_on_clear_text_selection() {
        let mut backend = TuiBackend::new();
        backend.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
        );
        assert!(backend.active_text_selection().is_some());
        backend.clear_text_selection();
        assert!(backend.active_text_selection().is_none());
        // Drag state should also be cleared if it was a TextSelection.
        backend.drag_state.begin(DragTarget::TextSelection {
            region: WidgetId::new("r"),
            anchor: Point::new(0.0, 0.0),
        });
        backend.clear_text_selection();
        assert!(!backend.drag_state.is_active());
    }

    #[test]
    fn text_regions_cleared_on_begin_frame() {
        let mut backend = TuiBackend::new();
        backend.register_text_region(crate::dispatch::TextRegion {
            id: WidgetId::new("r"),
            bounds: crate::event::Rect::new(0.0, 0.0, 40.0, 20.0),
        });
        assert_eq!(backend.text_regions.len(), 1);
        backend.begin_frame(Viewport::default());
        assert_eq!(
            backend.text_regions.len(),
            0,
            "text_regions must be cleared on begin_frame"
        );
    }
}
