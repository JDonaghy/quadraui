//! GTK implementation of [`quadraui::Backend`].
//!
//! `GtkBackend` is the GTK equivalent of `tui_main::backend::TuiBackend`.
//! It owns the persistent UI state the trait requires (modal stack,
//! drag state, accelerator registry, viewport, platform services) plus
//! a transient frame-scope holding the active `&cairo::Context` and
//! `&pango::Layout` so trait `draw_*` methods can rasterise into the
//! GTK draw callback.
//!
//! ### Frame-scope mechanism (mirror of TUI)
//!
//! GTK's `set_draw_func(da, |da, cr, _w, _h| { … })` callback yields
//! `&cairo::Context` only inside the closure. `enter_frame_scope`
//! stashes type-erased pointers to the cairo context and the
//! per-frame `pango::Layout` (built once at frame start so every
//! `draw_*` reuses the same one), runs the caller's closure, then
//! clears the pointers on exit.
//!
//! ### Event loop adapter (Stage 4)
//!
//! GTK is callback-driven; the trait's `wait_events`/`poll_events`
//! are poll-driven. `events: Rc<RefCell<VecDeque<UiEvent>>>` is the
//! adapter: signal handlers (mouse, key, resize) push translated
//! [`UiEvent`]s onto the queue; `wait_events(timeout)` drains it,
//! using `glib::MainContext::iteration(false)` to give the main
//! loop a chance to fire pending callbacks if the queue is empty.
//!
//! Stage 1 ships the struct shape and stub trait impls; the queue is
//! present but no signal callback is wired up yet (Stage 4).
//!
//! ### Why `Rc<RefCell<...>>` everywhere
//!
//! GTK signal callbacks need shared mutable access to backend state
//! across many widget closures. `Rc<RefCell<>>` is the standard
//! pattern in `gtk4-rs`. The trait method receivers (`&mut self`) work
//! fine: the App component wraps `GtkBackend` in `Rc<RefCell<>>` and
//! borrows mutably for trait calls.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use gtk4::cairo::Context;
use gtk4::pango;

use crate::{
    parse_key_binding, Accelerator, AcceleratorId, AcceleratorScope, ActivityBar, Backend,
    DragState, Form, KeyBinding, ListView, ModalStack, Palette, ParsedBinding, PlatformServices,
    Rect as QRect, StatusBar, TabBar, Terminal as TerminalPrim, TextDisplay, TreeView, UiEvent,
    Viewport,
};

use super::services::GtkPlatformServices;

/// GTK backend implementing [`quadraui::Backend`].
///
/// Field roles:
/// - `viewport` — width × height in DIPs, scale factor (HiDPI). Updated
///   each frame from the active DrawingArea's `width()` / `height()`.
/// - `modal_stack` — pushed by `App` on modal open, popped on close.
///   `quadraui::dispatch::dispatch_mouse_down` consults it.
/// - `drag_state` — at most one in-flight scrollbar drag. Set on
///   click-down on a scrollbar, read each drag-update, cleared on
///   mouse-up.
/// - `accelerators` / `parsed_accelerators` — registered keybindings;
///   `apply_accelerators` rewrites matching `KeyPressed` events to
///   `UiEvent::Accelerator(id, mods)` before they reach the app.
/// - `events` — adapter queue between GTK signal callbacks and the
///   trait's poll-style `wait_events`. Stage 4 wires up the producers.
/// - `current_*_ptr` — frame-scope pointers; non-null only inside
///   [`Self::enter_frame_scope`]. Type-erased through `*mut ()` to
///   avoid threading lifetime parameters onto the struct.
/// - `current_theme` — captured once per frame so `draw_*` calls don't
///   need to re-derive theme colors per primitive.
///
/// The `Rc<RefCell<>>` wrappers on `modal_stack` / `drag_state` /
/// `events` mirror the existing GTK App pattern — signal callbacks
/// clone the `Rc` into their captures and `borrow_mut()` when they
/// fire. The trait method bodies just dereference through.
pub struct GtkBackend {
    viewport: Viewport,
    modal_stack: Rc<std::cell::RefCell<ModalStack>>,
    drag_state: Rc<std::cell::RefCell<DragState>>,
    accelerators: HashMap<AcceleratorId, Accelerator>,
    /// Pre-parsed bindings, kept in lock-step with `accelerators`.
    /// `apply_accelerators` walks this list to rewrite `KeyPressed`
    /// events into `Accelerator` events. First-match-wins, insertion
    /// order. Same shape as `TuiBackend`'s `parsed_accelerators`.
    parsed_accelerators: Vec<(ParsedBinding, AcceleratorId)>,
    /// Adapter queue between GTK callbacks (producers) and
    /// `wait_events` / `poll_events` (consumers). Stage 4 wires the
    /// producers.
    events: Rc<std::cell::RefCell<VecDeque<UiEvent>>>,
    services: GtkPlatformServices,
    /// Type-erased `&cairo::Context` pointer; non-null only inside
    /// [`Self::enter_frame_scope`].
    current_cr_ptr: Cell<*const ()>,
    /// Type-erased `&pango::Layout` pointer; non-null only inside
    /// [`Self::enter_frame_scope`]. Built once per frame from the
    /// cairo context's pangocairo context, reused by every `draw_*`
    /// call so font-metrics setup doesn't repeat per primitive.
    current_layout_ptr: Cell<*const ()>,
    current_theme: crate::Theme,
    /// Per-frame Pango line height in DIPs. Set by the App in its
    /// draw closure (from font metrics) before any trait `draw_*`
    /// invocation. Every primitive that uses text metrics passes
    /// this through.
    current_line_height: f64,
    /// Per-frame Pango approximate-char-width in DIPs. Set by the
    /// App alongside `current_line_height`. Required by primitives
    /// that map cells to pixels (e.g. `draw_terminal`).
    current_char_width: f64,
    /// Whether nerd-font glyphs should be used by primitives that
    /// have icon variants. Apps wire this from their own setting
    /// (vimcode reads `engine.settings.use_nerd_fonts`); kubeui has
    /// its own toggle. Mirrors the `TuiBackend` field of the same
    /// name (#268).
    nerd_fonts_enabled: bool,
    /// Pango font description string for UI chrome (sans-serif text
    /// in title/buttons of `Dialog`, etc). Format is
    /// `"<family> <size>"` per Pango convention. Apps set this from
    /// their settings (vimcode passes `format!("{} {}", ui_font_family,
    /// ui_font_size)`). Falls back to `"Sans 11"` if unset.
    ui_font: String,
}

impl GtkBackend {
    /// Construct a fresh `GtkBackend`. The viewport defaults to
    /// (0, 0, 1.0); the App component overwrites it before the first
    /// frame via [`Backend::begin_frame`]. Call this once at App
    /// initialisation; share the resulting backend via
    /// `Rc<RefCell<GtkBackend>>` to every widget callback that needs
    /// access.
    pub fn new() -> Self {
        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: Rc::new(std::cell::RefCell::new(ModalStack::new())),
            drag_state: Rc::new(std::cell::RefCell::new(DragState::new())),
            accelerators: HashMap::new(),
            parsed_accelerators: Vec::new(),
            events: Rc::new(std::cell::RefCell::new(VecDeque::new())),
            services: GtkPlatformServices::new(),
            current_cr_ptr: Cell::new(std::ptr::null()),
            current_layout_ptr: Cell::new(std::ptr::null()),
            current_theme: crate::Theme::default(),
            current_line_height: 16.0,
            current_char_width: 8.0,
            nerd_fonts_enabled: false,
            ui_font: "Sans 11".to_string(),
        }
    }

    /// Update the cached nerd-font flag. Call from the app's settings
    /// or per-frame sync (vimcode does this in
    /// `App::update::CacheFontMetrics`).
    pub fn set_nerd_fonts(&mut self, enabled: bool) {
        self.nerd_fonts_enabled = enabled;
    }

    /// Update the cached UI font description string (Pango format,
    /// e.g. `"Cantarell 11"`). Call from the app's settings sync.
    pub fn set_ui_font(&mut self, ui_font: impl Into<String>) {
        self.ui_font = ui_font.into();
    }

    /// Shared handle to the modal stack. The App and widget callbacks
    /// clone this to push/pop modals and to feed
    /// `dispatch::dispatch_mouse_down`. The trait's `modal_stack_mut`
    /// borrows through this same handle.
    pub fn modal_stack_handle(&self) -> Rc<std::cell::RefCell<ModalStack>> {
        self.modal_stack.clone()
    }

    /// True if any modal is open (palette, dialog, context menu, …).
    /// Use this to gate hover triggers, focus-stealing animations, and
    /// other behaviours that should pause while a modal is up.
    ///
    /// API surface for issue #248 (Stage 5+ — migrate dialog /
    /// context-menu / completion popup onto `ModalStack`). Today only
    /// the picker pushes onto the stack, so this returns true only
    /// when a picker is open. Each modal migrated by #248 makes
    /// `is_modal_open()` correctly cover that modal type.
    #[allow(dead_code)]
    pub fn is_modal_open(&self) -> bool {
        !self.modal_stack.borrow().is_empty()
    }

    /// Shared handle to the drag state. Mouse-down on a scrollbar
    /// arms it via `borrow_mut().begin(...)`; mouse-drag-update reads
    /// it via `borrow()` to feed `dispatch::dispatch_mouse_drag`;
    /// mouse-up clears it.
    pub fn drag_state_handle(&self) -> Rc<std::cell::RefCell<DragState>> {
        self.drag_state.clone()
    }

    /// Shared handle to the event-queue adapter. Producer-side
    /// signal-callback closures (mouse/key/scroll on the editor
    /// DrawingArea, as of Phase B.5b Stage 1) clone this and push
    /// translated `UiEvent`s into the queue. Drained by
    /// `wait_events` / `poll_events`.
    pub fn events_handle(&self) -> Rc<std::cell::RefCell<VecDeque<UiEvent>>> {
        self.events.clone()
    }

    /// Push a single event onto the queue. Convenience for callbacks
    /// that have a `&GtkBackend` (or `&Rc<RefCell<GtkBackend>>`)
    /// reference and don't want to clone the events handle. Stage 5
    /// uses `events_handle()` directly inside captured closures
    /// because cloning the handle is cheaper than reaching the
    /// backend through `Rc<RefCell<>>`.
    #[allow(dead_code)]
    pub fn push_event(&self, ev: UiEvent) {
        self.events.borrow_mut().push_back(ev);
    }

    /// Update the cached theme. Call once per frame from the App's
    /// draw callback, before any trait `draw_*` invocations.
    #[allow(dead_code)]
    pub fn set_current_theme(&mut self, theme: crate::Theme) {
        self.current_theme = theme;
    }

    /// Update the cached Pango line height (in DIPs). Call once per
    /// frame from the App's draw callback (after measuring font
    /// metrics), before any trait `draw_*` invocations.
    #[allow(dead_code)]
    pub fn set_current_line_height(&mut self, line_height: f64) {
        self.current_line_height = line_height;
    }

    /// Update the cached Pango approximate-char-width (in DIPs).
    /// Call once per frame alongside [`Self::set_current_line_height`].
    /// Required by primitives that map cells to pixels (terminal).
    #[allow(dead_code)]
    pub fn set_current_char_width(&mut self, char_width: f64) {
        self.current_char_width = char_width;
    }

    /// Enter the frame-scope: stash the cairo context + pango layout
    /// pointers, run `f`, then clear them. **Must** be called from
    /// inside a `set_draw_func(...)` closure where `cr` is alive.
    /// The pango layout is freshly created from `cr` via
    /// `pangocairo::create_context` so font-metrics setup is shared
    /// across every `draw_*` in this frame.
    ///
    /// Type-erased through `*const ()` because both `Context` and
    /// `Layout` are reference-counted GObjects whose Rust borrow
    /// lifetimes we don't want to thread onto the struct. Safety
    /// relies on:
    /// 1. The pointers are set immediately before `f` runs and
    ///    cleared after, including on panic.
    /// 2. `f` cannot move the pointers out — only read via the safe
    ///    accessors which return references scoped to the call.
    /// 3. Calls don't nest meaningfully (a nested `enter_frame_scope`
    ///    would alias the same `&Context`, which Rust forbids at
    ///    the caller side anyway).
    #[allow(dead_code)]
    pub fn enter_frame_scope<R>(
        &mut self,
        cr: &Context,
        layout: &pango::Layout,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let cr_ptr = cr as *const Context as *const ();
        let layout_ptr = layout as *const pango::Layout as *const ();
        let prev_cr = self.current_cr_ptr.replace(cr_ptr);
        let prev_layout = self.current_layout_ptr.replace(layout_ptr);
        let result = f(self);
        self.current_cr_ptr.set(prev_cr);
        self.current_layout_ptr.set(prev_layout);
        result
    }

    /// Get the current cairo context + pango layout inside the
    /// frame-scope, or `None` outside. Trait `draw_*` methods call
    /// this and bail (panic in dev) if the scope isn't active.
    fn current_frame_refs(&self) -> Option<(&Context, &pango::Layout)> {
        let cr_ptr = self.current_cr_ptr.get();
        let layout_ptr = self.current_layout_ptr.get();
        if cr_ptr.is_null() || layout_ptr.is_null() {
            return None;
        }
        // SAFETY: `enter_frame_scope` set both pointers from real
        // borrows of `&Context` / `&pango::Layout` and won't return
        // until the scope ends. Outside the scope both pointers are
        // null and we returned above.
        Some(unsafe {
            (
                &*(cr_ptr as *const Context),
                &*(layout_ptr as *const pango::Layout),
            )
        })
    }

    /// Apply registered accelerators to a slice of UiEvents. Mirrors
    /// `TuiBackend::apply_accelerators`. Replaces matching
    /// `UiEvent::KeyPressed` events with `UiEvent::Accelerator(id, mods)`.
    /// Stage 6 wires this into the event-queue drain path.
    #[allow(dead_code)]
    pub fn apply_accelerators(&self, events: &mut [UiEvent]) {
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

    /// Look up a registered Global accelerator for a `(key, modifiers)`
    /// pair. Returns the matching `AcceleratorId` on first hit, or
    /// `None`. Used both by `apply_accelerators` (rewriting queue
    /// events) and by the GTK key callback (synchronous dispatch in
    /// B.5b Stage 2).
    pub fn match_keypress(
        &self,
        key: &crate::Key,
        modifiers: crate::Modifiers,
    ) -> Option<AcceleratorId> {
        let key_name = match key {
            crate::Key::Char(c) => {
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

impl Default for GtkBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a `KeyBinding` (any variant) into a `ParsedBinding`. Mirrors
/// the same helper in `tui_main/backend.rs` — universal arms map to
/// the canonical vim-style strings vimcode already uses elsewhere.
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
/// produces. Same mapping as TuiBackend uses.
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

impl Backend for GtkBackend {
    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    fn end_frame(&mut self) {
        // No-op. GTK's `set_draw_func` closure flushes when it returns;
        // this method exists for parity with backends that need an
        // explicit flush.
    }

    fn poll_events(&mut self) -> Vec<UiEvent> {
        // Drain the queue without blocking. Stage 4 wires up the
        // signal-callback producers; until then this is always empty.
        let mut out: Vec<UiEvent> = self.events.borrow_mut().drain(..).collect();
        self.apply_accelerators(&mut out);
        out
    }

    fn wait_events(&mut self, _timeout: Duration) -> Vec<UiEvent> {
        // Stage 4 will:
        // 1. Drain the queue.
        // 2. If empty, run `glib::MainContext::iteration(false)` to
        //    let pending GTK callbacks fire, then drain again.
        // 3. Repeat with `iteration(true)` (blocking) up to `_timeout`
        //    if still empty.
        //
        // Today the GTK event loop runs natively (Relm4's internals
        // pump GTK signals), so `wait_events` is currently dormant —
        // the App component handles events via Relm4 messages, not
        // through the trait. Stage 4 flips this so the trait owns
        // event flow.
        let mut out: Vec<UiEvent> = self.events.borrow_mut().drain(..).collect();
        self.apply_accelerators(&mut out);
        out
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
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
        // The trait wants `&mut ModalStack`. The backend's modal
        // stack lives behind `Rc<RefCell<>>` because GTK callbacks
        // need shared access. This call leaks a `RefMut<'_>` for
        // the duration of the trait method; the trait method bodies
        // (e.g. modal-aware drawing) read the stack and return —
        // they don't hold the borrow across other calls.
        //
        // SAFETY: `Rc::as_ptr` returns a stable pointer to the
        // `RefCell`'s inner; the `RefCell::borrow_mut` would
        // dynamically check borrow rules, but we know the trait's
        // contract: callers don't reentrantly call into the same
        // backend during a `modal_stack_mut()` borrow. If they did,
        // the panic-on-double-borrow inside `RefCell` would fire.
        //
        // The simpler alternative — making `modal_stack` a plain
        // `ModalStack` field — fails because GTK signal callbacks
        // already need `Rc<RefCell<>>` access; we'd duplicate the
        // state.
        unsafe {
            let cell_ptr = Rc::as_ptr(&self.modal_stack);
            // Leak a `RefMut`'s deref by constructing one and
            // forgetting it. This is wrong for production — Stage 5
            // restructures dispatch so callers go through
            // `modal_stack_handle()` directly and this trait method
            // becomes vestigial. Today it exists to satisfy the
            // trait signature; nothing in the GTK path actually
            // calls it.
            &mut *(*cell_ptr).as_ptr()
        }
    }

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    // ─── Drawing ───────────────────────────────────────────────────────────
    //
    // Stage 1 stubs. Stage 2 fills these in by folding the existing
    // `quadraui_gtk::draw_*` shims into the trait method bodies
    // (mirroring TUI Stage 2). For now they panic with a clear
    // "deferred" message — the GTK draw path doesn't go through the
    // trait yet, so these are unreachable in practice.

    fn draw_tree(&mut self, rect: QRect, tree: &TreeView) {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_tree called outside enter_frame_scope");
        crate::gtk::draw_tree(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            tree,
            &self.current_theme,
            self.current_line_height,
            self.nerd_fonts_enabled,
        );
    }

    fn draw_list(&mut self, rect: QRect, list: &ListView) {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_list called outside enter_frame_scope");
        crate::gtk::draw_list(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            list,
            &self.current_theme,
            self.current_line_height,
            self.nerd_fonts_enabled,
        );
    }

    fn draw_form(&mut self, rect: QRect, form: &Form) {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_form called outside enter_frame_scope");
        crate::gtk::draw_form(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            form,
            &self.current_theme,
            self.current_line_height,
        );
    }

    fn draw_palette(&mut self, rect: QRect, palette: &Palette) {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_palette called outside enter_frame_scope");
        crate::gtk::draw_palette(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            palette,
            &self.current_theme,
            self.current_line_height,
            self.nerd_fonts_enabled,
        );
    }

    // ─── Layout-passthrough primitives ─────────────────────────────────────
    //
    // Phase B.5b Stage 9: trait extended with `&Layout` parameter per
    // `BACKEND_TRAIT_PROPOSAL.md` §6.2. The current GTK rasterisers
    // (`quadraui_gtk::draw_status_bar` etc.) recompute their own
    // layout internally, so the `_layout` parameter is currently
    // ignored — kept for forward compatibility when the GTK
    // rasterisers are updated to consume it. Behaviour is unchanged.

    // Phase B.5b Stage 9: trait extended with `&Layout` parameter
    // per `BACKEND_TRAIT_PROPOSAL.md` §6.2. Three of the five
    // primitives (status_bar, tab_bar, text_display) have
    // quadraui-side rasterisers that already accept a `crate::Theme`,
    // so the trait impls below route through them. The remaining two
    // (activity_bar, terminal) only have the in-tree `quadraui_gtk::*`
    // shims that take the legacy `render::Theme`; until those are
    // lifted into quadraui itself (#223 lift sequence), the trait
    // impls stay as stubs and the GTK call sites continue to use the
    // legacy shims directly.

    fn draw_status_bar(&mut self, rect: QRect, bar: &StatusBar) -> Vec<crate::StatusBarHitRegion> {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_status_bar called outside enter_frame_scope");
        crate::gtk::draw_status_bar(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            self.current_line_height,
            bar,
            &self.current_theme,
        )
    }

    fn draw_tab_bar(
        &mut self,
        rect: QRect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> crate::TabBarHits {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_tab_bar called outside enter_frame_scope");
        // The free fn paints from x=0 to x=width; rect.x ignored.
        crate::gtk::draw_tab_bar(
            cr,
            layout,
            rect.width as f64,
            self.current_line_height,
            rect.y as f64,
            bar,
            &self.current_theme,
            hovered_close_tab,
        )
    }

    fn draw_activity_bar(
        &mut self,
        rect: QRect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<crate::ActivityBarRowHit> {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_activity_bar called outside enter_frame_scope");
        // The rasteriser paints from (0, 0) to (width, height); rect.x/y ignored.
        let _ = rect;
        crate::gtk::draw_activity_bar(
            cr,
            layout,
            rect.width as f64,
            rect.height as f64,
            bar,
            &self.current_theme,
            hovered_idx,
        )
    }

    fn draw_terminal(&mut self, rect: QRect, term: &TerminalPrim) {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_terminal called outside enter_frame_scope");
        crate::gtk::draw_terminal_cells(
            cr,
            layout,
            term,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            self.current_line_height,
            self.current_char_width,
            &self.current_theme,
        );
    }

    fn draw_text_display(&mut self, rect: QRect, td: &TextDisplay) {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_text_display called outside enter_frame_scope");
        crate::gtk::draw_text_display(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            td,
            &self.current_theme,
            self.current_line_height,
        );
    }

    fn draw_tooltip(&mut self, tooltip: &crate::Tooltip, layout_arg: &crate::TooltipLayout) {
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_tooltip called outside enter_frame_scope");
        crate::gtk::draw_tooltip(
            cr,
            pango_layout,
            tooltip,
            layout_arg,
            self.current_line_height,
            self.current_char_width,
            &self.current_theme,
        );
    }

    fn draw_context_menu(
        &mut self,
        menu: &crate::ContextMenu,
        layout_arg: &crate::ContextMenuLayout,
    ) -> Vec<(QRect, crate::WidgetId)> {
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_context_menu called outside enter_frame_scope");
        let hits = crate::gtk::draw_context_menu(
            cr,
            pango_layout,
            menu,
            layout_arg,
            self.current_line_height,
            &self.current_theme,
        );
        // Reshape rasteriser's `(x, y, w, h, id)` tuples into
        // `(Rect, WidgetId)` for the trait return.
        hits.into_iter()
            .map(|(x, y, w, h, id)| (QRect::new(x as f32, y as f32, w as f32, h as f32), id))
            .collect()
    }

    fn draw_dialog(
        &mut self,
        dialog: &crate::Dialog,
        dialog_layout: &crate::DialogLayout,
    ) -> Vec<QRect> {
        let line_height = self.current_line_height;
        let theme = self.current_theme;
        let ui_font_desc = pango::FontDescription::from_string(&self.ui_font);
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_dialog called outside enter_frame_scope");
        let rects = crate::gtk::draw_dialog(
            cr,
            pango_layout,
            &ui_font_desc,
            dialog,
            dialog_layout,
            line_height,
            &theme,
        );
        rects
            .into_iter()
            .map(|(x, y, w, h)| QRect::new(x as f32, y as f32, w as f32, h as f32))
            .collect()
    }
}

// ─── Cross-backend validation tests ──────────────────────────────────────────
//
// Phase B.5 Stage 2: prove the same generic `<B: Backend>` paint
// helper that's already validated on `TuiBackend` (B.4 Stage 3b)
// works against `GtkBackend`. This is a compile-only assertion —
// running the draws would require an active cairo Context, which
// belongs in a real GTK test harness. The compile-only proof is
// enough for the trait constraint check.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WidgetId;

    /// Generic helper — minimal "app render code" that consumes
    /// `Backend` through `<B>`. Same shape as the one in
    /// `tui_main::backend::tests::paint_overlays`.
    fn paint_overlays<B: Backend>(backend: &mut B, palette: &Palette, list: &ListView) {
        backend.draw_palette(QRect::new(10.0, 5.0, 60.0, 14.0), palette);
        backend.draw_list(QRect::new(0.0, 20.0, 80.0, 4.0), list);
    }

    #[test]
    fn paint_overlays_compiles_against_gtk_backend() {
        let _: fn(&mut GtkBackend, &Palette, &ListView) = paint_overlays::<GtkBackend>;
    }

    #[test]
    fn gtk_backend_modal_stack_handle_shares_state() {
        let backend = GtkBackend::new();
        let h1 = backend.modal_stack_handle();
        let h2 = backend.modal_stack_handle();
        // Both handles point at the same `RefCell<ModalStack>`.
        h1.borrow_mut()
            .push(WidgetId::new("test:popup"), QRect::new(0.0, 0.0, 10.0, 5.0));
        assert_eq!(h2.borrow().len(), 1);
    }

    #[test]
    fn gtk_backend_is_modal_open_tracks_stack() {
        let backend = GtkBackend::new();
        assert!(!backend.is_modal_open());
        backend
            .modal_stack_handle()
            .borrow_mut()
            .push(WidgetId::new("test:modal"), QRect::new(0.0, 0.0, 10.0, 5.0));
        assert!(backend.is_modal_open());
        backend
            .modal_stack_handle()
            .borrow_mut()
            .pop(&WidgetId::new("test:modal"));
        assert!(!backend.is_modal_open());
    }

    #[test]
    fn gtk_backend_push_event_round_trip() {
        let backend = GtkBackend::new();
        backend.push_event(crate::UiEvent::WindowFocused(true));
        let q = backend.events_handle();
        assert_eq!(q.borrow().len(), 1);
    }

    #[test]
    fn gtk_backend_register_accelerator_round_trip() {
        let mut backend = GtkBackend::new();
        backend.register_accelerator(&Accelerator {
            id: AcceleratorId::new("test.save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        });
        assert_eq!(backend.accelerators.len(), 1);
        assert_eq!(backend.parsed_accelerators.len(), 1);
        backend.unregister_accelerator(&AcceleratorId::new("test.save"));
        assert!(backend.accelerators.is_empty());
        assert!(backend.parsed_accelerators.is_empty());
    }

    /// Regression test for B5b.2: parse_key_binding correctness for the
    /// two terminal-shortcut strings. If `<C-t>` parses to a binding that
    /// matches Ctrl+Shift+T (or vice versa), the accelerator dispatch
    /// will flip.
    #[test]
    fn parse_binding_terminal_strings_distinct() {
        let p_ct = crate::parse_key_binding("<C-t>").expect("<C-t>");
        assert!(p_ct.modifiers.ctrl);
        assert!(
            !p_ct.modifiers.shift,
            "<C-t> must NOT have shift, got {:?}",
            p_ct
        );
        assert_eq!(p_ct.key, "t");

        let p_cst = crate::parse_key_binding("<C-S-t>").expect("<C-S-t>");
        assert!(p_cst.modifiers.ctrl);
        assert!(
            p_cst.modifiers.shift,
            "<C-S-t> must have shift, got {:?}",
            p_cst
        );
        assert_eq!(p_cst.key, "t");
    }

    /// Regression test for B5b.2: the lookup used by the GTK key handler
    /// must return distinct ids for `<C-t>` vs `<C-S-t>`. Previously the
    /// inputs were swapped at runtime — Ctrl+T fired the maximize action
    /// and Ctrl+Shift+T fired the open action.
    #[test]
    fn gtk_backend_match_keypress_distinguishes_ctrl_vs_ctrl_shift() {
        let mut backend = GtkBackend::new();
        backend.register_accelerator(&Accelerator {
            id: AcceleratorId::new("gtk.panel.open_terminal"),
            binding: KeyBinding::Literal("<C-t>".into()),
            scope: AcceleratorScope::Global,
            label: None,
        });
        backend.register_accelerator(&Accelerator {
            id: AcceleratorId::new("terminal.toggle_maximize"),
            binding: KeyBinding::Literal("<C-S-t>".into()),
            scope: AcceleratorScope::Global,
            label: None,
        });

        let ctrl_only = crate::Modifiers {
            ctrl: true,
            shift: false,
            alt: false,
            cmd: false,
        };
        let ctrl_shift = crate::Modifiers {
            ctrl: true,
            shift: true,
            alt: false,
            cmd: false,
        };

        // Ctrl+T → open_terminal
        let open = backend.match_keypress(&crate::Key::Char('t'), ctrl_only);
        assert_eq!(
            open.as_ref().map(|i| i.as_str()),
            Some("gtk.panel.open_terminal"),
            "Ctrl+T should match open_terminal, got {:?}",
            open
        );

        // Ctrl+Shift+T → toggle_maximize
        let max = backend.match_keypress(&crate::Key::Char('t'), ctrl_shift);
        assert_eq!(
            max.as_ref().map(|i| i.as_str()),
            Some("terminal.toggle_maximize"),
            "Ctrl+Shift+T should match terminal.toggle_maximize, got {:?}",
            max
        );

        // Also try with the GDK-style uppercase 'T' for the shift case —
        // gdk_key_to_quadraui_key returns Key::Char('T') when shift is held.
        let max_upper = backend.match_keypress(&crate::Key::Char('T'), ctrl_shift);
        assert_eq!(
            max_upper.as_ref().map(|i| i.as_str()),
            Some("terminal.toggle_maximize"),
            "Ctrl+Shift+T (with uppercase T) should match terminal.toggle_maximize, got {:?}",
            max_upper
        );
    }
}
