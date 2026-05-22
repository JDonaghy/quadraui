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

use crate::backend::{activity_bar_hits, tab_bar_layout_to_hits};
use crate::{
    parse_key_binding, Accelerator, AcceleratorId, AcceleratorScope, ActivityBar, Backend,
    CommandLine, DragState, Form, KeyBinding, ListView, MenuBar, ModalStack, Palette,
    ParsedBinding, PlatformServices, Rect as QRect, Split, StatusBar, TabBar,
    Terminal as TerminalPrim, TextDisplay, TreeView, UiEvent, Viewport,
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
    /// Pango context for text measurement outside the draw callback.
    /// Set once via [`Self::set_pango_context`] during init; used by
    /// `form_layout()` and other `_layout()` methods that need exact
    /// Pango measurement rather than the `current_char_width` approximation.
    pango_ctx: Option<pango::Context>,
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
            pango_ctx: None,
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

    /// Read-only accessor for the cached theme. Used by the runner
    /// to paint a full-DA background before each frame.
    pub fn current_theme(&self) -> &crate::Theme {
        &self.current_theme
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

    /// Store the widget's Pango context for text measurement outside
    /// the draw callback. Call once during init (e.g. from the runner
    /// after `DrawingArea::realize`). The context carries the font
    /// configuration and is valid for the widget's lifetime.
    pub fn set_pango_context(&mut self, ctx: pango::Context) {
        self.pango_ctx = Some(ctx);
    }

    /// Create a `pango::Layout` from the stored Pango context (set via
    /// [`Self::set_pango_context`]). The layout inherits the stable font
    /// options from init, avoiding per-frame font-hinting variance that
    /// occurs when creating a context from a transient Cairo surface.
    /// Returns `None` if no Pango context has been stored.
    pub fn create_stable_pango_layout(&self) -> Option<pango::Layout> {
        self.pango_ctx.as_ref().map(pango::Layout::new)
    }

    /// Measure a `StyledText` label width in pixels using Pango if
    /// available, falling back to `visible_width * char_w`.
    fn pango_text_width(
        &self,
        pango_layout: &Option<pango::Layout>,
        text: &crate::types::StyledText,
        char_w: f32,
    ) -> f32 {
        let plain: String = text.spans.iter().map(|s| s.text.as_str()).collect();
        self.pango_str_width(pango_layout, &plain, char_w)
    }

    /// Measure a plain string width in pixels using Pango if
    /// available, falling back to `chars().count() * char_w`.
    fn pango_str_width(
        &self,
        pango_layout: &Option<pango::Layout>,
        text: &str,
        char_w: f32,
    ) -> f32 {
        if let Some(pl) = pango_layout {
            pl.set_text(text);
            let (w, _) = pl.pixel_size();
            w as f32
        } else {
            (text.chars().count() as f32 * char_w).ceil() + 2.0
        }
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

    fn line_height(&self) -> f32 {
        self.current_line_height as f32
    }

    fn char_width(&self) -> f32 {
        self.current_char_width as f32
    }

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

    fn draw_data_table(
        &mut self,
        rect: QRect,
        table: &crate::DataTable,
        hovered_idx: Option<usize>,
    ) -> crate::DataTableLayout {
        let lh = self.current_line_height;
        let theme = self.current_theme;
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_data_table called outside enter_frame_scope");
        crate::gtk::draw_data_table(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            table,
            &theme,
            lh,
            hovered_idx,
        )
    }

    fn data_table_layout(&self, rect: QRect, table: &crate::DataTable) -> crate::DataTableLayout {
        let lh = self.current_line_height;
        let header_height = (lh * 1.2).round();
        table.layout(
            rect.width,
            rect.height,
            lh as f32,
            header_height as f32,
            8.0,
            |col| {
                crate::ColumnMeasure::new(col.title.len() as f32 * self.current_char_width as f32)
            },
        )
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

    fn draw_status_bar(
        &mut self,
        rect: QRect,
        bar: &StatusBar,
        hovered_id: Option<&crate::types::WidgetId>,
        pressed_id: Option<&crate::types::WidgetId>,
    ) -> crate::StatusBarLayout {
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
            hovered_id,
            pressed_id,
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
            rect.height as f64,
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
        cr.save().ok();
        cr.translate(rect.x as f64, rect.y as f64);
        let hits = crate::gtk::draw_activity_bar(
            cr,
            layout,
            rect.width as f64,
            rect.height as f64,
            bar,
            &self.current_theme,
            hovered_idx,
        );
        cr.restore().ok();
        hits
    }

    fn status_bar_layout(&self, rect: QRect, bar: &StatusBar) -> crate::StatusBarLayout {
        let char_w = self.current_char_width as f32;
        let lh = self.current_line_height as f32;
        let frame_layout = self.current_frame_refs().map(|(_, l)| l.clone());
        let pango_layout = frame_layout.or_else(|| self.pango_ctx.as_ref().map(pango::Layout::new));
        bar.layout(rect.width, lh, crate::gtk::MIN_GAP_PX, |seg| {
            let text_w = self.pango_str_width(&pango_layout, &seg.text, char_w);
            crate::StatusSegmentMeasure::new(text_w)
        })
    }

    fn tab_bar_layout(&self, rect: QRect, bar: &TabBar) -> crate::TabBarHits {
        let char_w = self.current_char_width as f32;
        let frame_layout = self.current_frame_refs().map(|(_, l)| l.clone());
        let pango_layout = frame_layout.or_else(|| self.pango_ctx.as_ref().map(pango::Layout::new));

        let tab_pad: f32 = if bar.compact { 2.0 } else { 14.0 };
        let tab_inner_gap: f32 = if bar.compact { 4.0 } else { 10.0 };
        let tab_outer_gap: f32 = if bar.compact { 0.0 } else { 1.0 };

        let close_glyph_w = if bar.show_tab_close {
            self.pango_str_width(&pango_layout, "×", char_w)
        } else {
            0.0
        };
        let close_extra = if bar.show_tab_close {
            tab_inner_gap + close_glyph_w
        } else {
            0.0
        };

        let tab_name_widths: Vec<f32> = bar
            .tabs
            .iter()
            .map(|t| self.pango_str_width(&pango_layout, &t.label, char_w))
            .collect();

        let layout = bar.layout(
            rect.width,
            rect.height,
            0.0, // no scroll arrows — matches the draw path
            |i| {
                let total = tab_pad + tab_name_widths[i] + close_extra + tab_pad + tab_outer_gap;
                let close_w = if bar.show_tab_close {
                    tab_inner_gap + close_glyph_w + tab_pad + tab_outer_gap
                } else {
                    0.0
                };
                crate::TabMeasure::new(total, close_w)
            },
            |i| {
                let text_w =
                    self.pango_str_width(&pango_layout, &bar.right_segments[i].text, char_w);
                crate::SegmentMeasure::new(text_w)
            },
        );

        let mut hits = tab_bar_layout_to_hits(&layout, bar);

        let active_idx = bar.tabs.iter().position(|t| t.is_active);
        let reserved_px: f32 = bar
            .right_segments
            .iter()
            .map(|seg| self.pango_str_width(&pango_layout, &seg.text, char_w))
            .sum();
        let effective_tab_area = (rect.width - reserved_px).max(0.0);

        hits.correct_scroll_offset = if let Some(active) = active_idx {
            TabBar::fit_active_scroll_offset(
                active,
                bar.tabs.len(),
                effective_tab_area as usize,
                |i| {
                    (tab_pad + tab_name_widths[i] + close_extra + tab_pad + tab_outer_gap).ceil()
                        as usize
                },
            )
        } else {
            bar.scroll_offset
        };

        hits
    }

    fn activity_bar_layout(&self, rect: QRect, bar: &ActivityBar) -> Vec<crate::ActivityBarRowHit> {
        activity_bar_hits(rect, bar, crate::gtk::ACTIVITY_ROW_PX as f32)
    }

    fn draw_terminal(&mut self, rect: QRect, term: &TerminalPrim) {
        let lh = self.current_line_height;
        let cw = self.current_char_width;
        let theme = self.current_theme;
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_terminal called outside enter_frame_scope");

        let sb_width = match &term.scrollbar {
            Some(sb) => sb.width.map(|w| w as f64).unwrap_or(8.0),
            None => 0.0,
        };
        let cell_area_w = (rect.width as f64 - sb_width).max(0.0);

        crate::gtk::draw_terminal_cells(
            cr,
            layout,
            term,
            rect.x as f64,
            rect.y as f64,
            cell_area_w,
            lh,
            cw,
            &theme,
        );

        if let Some(ref sb_state) = term.scrollbar {
            let sb = crate::primitives::scrollbar::Scrollbar::vertical(
                term.id.clone(),
                crate::event::Rect::new(
                    rect.x + cell_area_w as f32,
                    rect.y,
                    sb_width as f32,
                    rect.height,
                ),
                sb_state.effective_scroll_offset() as f32,
                sb_state.total_lines as f32,
                sb_state.visible_lines as f32,
                lh as f32,
            );
            crate::gtk::draw_scrollbar(cr, &sb, &theme);
        }
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

    fn draw_command_line(&mut self, rect: QRect, cmd: &CommandLine) {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_command_line called outside enter_frame_scope");
        crate::gtk::command_line::draw_command_line(
            cr,
            layout,
            cmd,
            &self.current_theme,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            self.current_line_height,
        );
    }

    fn text_display_layout(
        &self,
        rect: QRect,
        td: &TextDisplay,
    ) -> crate::primitives::text_display::TextDisplayLayout {
        crate::gtk::gtk_text_display_layout(td, rect, self.current_line_height)
    }

    fn draw_text_input(
        &mut self,
        rect: QRect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        let theme = self.current_theme;
        let lh = self.current_line_height;
        let cw = self.current_char_width;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_text_input called outside enter_frame_scope");
        crate::gtk::draw_text_input(cr, pango_layout, rect, ti, &theme, lh, cw)
    }

    fn text_input_layout(
        &self,
        rect: QRect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        crate::gtk::gtk_text_input_layout(
            ti,
            rect,
            self.current_line_height as f32,
            self.current_char_width as f32,
        )
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

    // ─── #13: trait coverage for the rest of the rasterised primitives ──

    fn draw_multi_section_view(
        &mut self,
        rect: QRect,
        view: &crate::primitives::multi_section_view::MultiSectionView,
    ) {
        let line_height = self.current_line_height;
        let theme = self.current_theme;
        let nerd_fonts = self.nerd_fonts_enabled;
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_multi_section_view called outside enter_frame_scope");
        crate::gtk::draw_multi_section_view(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            view,
            &theme,
            line_height,
            nerd_fonts,
        );
    }

    fn msv_layout(
        &self,
        rect: QRect,
        view: &crate::primitives::multi_section_view::MultiSectionView,
    ) -> crate::primitives::multi_section_view::MultiSectionViewLayout {
        crate::gtk::gtk_msv_layout(view, rect, self.current_line_height)
    }

    fn msv_metrics(&self) -> crate::primitives::multi_section_view::LayoutMetrics {
        crate::gtk::multi_section_view::metrics_for(self.current_line_height, false)
    }

    fn tree_layout(&self, rect: QRect, tree: &TreeView) -> crate::primitives::tree::TreeViewLayout {
        crate::gtk::gtk_tree_layout(tree, rect, self.current_line_height)
    }

    fn form_layout(&self, rect: QRect, form: &Form) -> crate::primitives::form::FormLayout {
        let row_h = (self.current_line_height * 1.4).round() as f32;
        let char_w = self.current_char_width as f32;
        let gap = 8.0_f32;

        let pango_layout = self.pango_ctx.as_ref().map(pango::Layout::new);

        form.layout(rect.width, rect.height, |i| {
            let field = &form.fields[i];
            match &field.kind {
                crate::primitives::form::FieldKind::ToggleGroup { toggles } => {
                    let label_w = self.pango_text_width(&pango_layout, &field.label, char_w);
                    let start_x = 6.0 + label_w + 12.0;
                    let items = toggles
                        .iter()
                        .map(|t| crate::primitives::form::FormItemMeasure {
                            id: t.id.clone(),
                            width: self.pango_str_width(&pango_layout, &t.label, char_w),
                        })
                        .collect();
                    crate::primitives::form::FormFieldMeasure::with_items(
                        row_h, start_x, gap, items,
                    )
                }
                crate::primitives::form::FieldKind::ButtonRow { buttons } => {
                    let label_w = self.pango_text_width(&pango_layout, &field.label, char_w);
                    let start_x = 6.0 + label_w + 12.0;
                    let items = buttons
                        .iter()
                        .map(|b| {
                            let bracketed = format!("[{}]", b.label);
                            crate::primitives::form::FormItemMeasure {
                                id: b.id.clone(),
                                width: self.pango_str_width(&pango_layout, &bracketed, char_w),
                            }
                        })
                        .collect();
                    crate::primitives::form::FormFieldMeasure::with_items(
                        row_h, start_x, gap, items,
                    )
                }
                crate::primitives::form::FieldKind::SegmentedControl { options, .. } => {
                    let label_w = self.pango_text_width(&pango_layout, &field.label, char_w);
                    let start_x = 6.0 + label_w + 12.0;
                    let items = options
                        .iter()
                        .enumerate()
                        .map(|(idx, opt)| {
                            let bracketed = format!("[{opt}]");
                            crate::primitives::form::FormItemMeasure {
                                id: crate::WidgetId::new(format!(
                                    "{}__seg_{idx}",
                                    field.id.as_str()
                                )),
                                width: self.pango_str_width(&pango_layout, &bracketed, char_w),
                            }
                        })
                        .collect();
                    crate::primitives::form::FormFieldMeasure::with_items(
                        row_h, start_x, 0.0, items,
                    )
                }
                crate::primitives::form::FieldKind::TextArea { visible_rows, .. } => {
                    crate::primitives::form::FormFieldMeasure::new(row_h * *visible_rows as f32)
                }
                _ => crate::primitives::form::FormFieldMeasure::new(row_h),
            }
        })
    }

    fn draw_editor(
        &mut self,
        rect: QRect,
        editor: &crate::primitives::editor::Editor,
    ) -> crate::backend::EditorPaintResult {
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_editor called outside enter_frame_scope");
        // GTK draw_editor needs FontMetrics; resolve from the layout's
        // font description. The TUI return type carries a TUI-specific
        // cursor cell — GTK paints its own caret, so we return the
        // default. Future GTK refinement can populate fields if hosts
        // need GTK-side cursor pixel coordinates.
        let pango_ctx = pango_layout.context();
        let font_desc = pango_layout
            .font_description()
            .or_else(|| pango_ctx.font_description());
        let metrics = pango_ctx.metrics(font_desc.as_ref(), None);
        crate::gtk::draw_editor(
            cr,
            pango_layout,
            &metrics,
            editor,
            &theme,
            char_width,
            line_height,
        );
        let _ = rect;
        crate::backend::EditorPaintResult::default()
    }

    fn draw_message_list(
        &mut self,
        rect: QRect,
        list: &crate::primitives::message_list::MessageList,
    ) {
        let line_height = self.current_line_height;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_message_list called outside enter_frame_scope");
        crate::gtk::draw_message_list(
            cr,
            pango_layout,
            list,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            (rect.y + rect.height) as f64,
            line_height,
        );
    }

    fn draw_rich_text_popup(
        &mut self,
        popup: &crate::primitives::rich_text_popup::RichTextPopup,
        layout_arg: &crate::primitives::rich_text_popup::RichTextPopupLayout,
    ) {
        let theme = self.current_theme;
        let ui_font_desc = pango::FontDescription::from_string(&self.ui_font);
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_rich_text_popup called outside enter_frame_scope");
        // GTK rasteriser returns link bounds; for trait parity we
        // discard them. Hosts that need link hit-testing query the
        // primitive's own `popup.layout(...).hit_test(...)` API.
        let _ = crate::gtk::draw_rich_text_popup(
            cr,
            pango_layout,
            &ui_font_desc,
            popup,
            layout_arg,
            &theme,
        );
    }

    fn draw_find_replace(
        &mut self,
        rect: QRect,
        panel: &crate::primitives::find_replace::FindReplacePanel,
    ) {
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_find_replace called outside enter_frame_scope");
        // GTK rasteriser positions the panel via its own anchor logic;
        // `rect` parameter is currently unused (forward-compat for a
        // host-resolved layout per BACKEND_TRAIT_PROPOSAL §6.2).
        let _ = rect;
        crate::gtk::draw_find_replace(cr, pango_layout, panel, &theme, line_height, char_width);
    }

    fn draw_completions(
        &mut self,
        completions: &crate::primitives::completions::Completions,
        layout_arg: &crate::primitives::completions::CompletionsLayout,
    ) {
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_completions called outside enter_frame_scope");
        crate::gtk::draw_completions(cr, pango_layout, completions, layout_arg, &theme);
    }

    fn draw_scrollbar(
        &mut self,
        _rect: QRect,
        scrollbar: &crate::primitives::scrollbar::Scrollbar,
    ) {
        let theme = self.current_theme;
        let (cr, _layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_scrollbar called outside enter_frame_scope");
        crate::gtk::draw_scrollbar(cr, scrollbar, &theme);
    }

    fn draw_drop_overlay(&mut self, overlay: &crate::primitives::drop_zone::DropOverlay) {
        let theme = self.current_theme;
        let (cr, _layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_drop_overlay called outside enter_frame_scope");
        crate::gtk::draw_drop_overlay(cr, overlay, &theme);
    }

    fn draw_menu_bar(
        &mut self,
        rect: QRect,
        bar: &MenuBar,
    ) -> crate::primitives::menu_bar::MenuBarLayout {
        let (cr, layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_menu_bar called outside enter_frame_scope");
        crate::gtk::draw_menu_bar(
            cr,
            layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            bar,
            &self.current_theme,
        )
    }

    fn menu_bar_layout(
        &self,
        rect: QRect,
        bar: &MenuBar,
    ) -> crate::primitives::menu_bar::MenuBarLayout {
        let bounds = crate::event::Rect::new(rect.x, rect.y, rect.width, rect.height);
        let char_w = self.current_char_width as f32;
        let pango_layout = self.pango_ctx.as_ref().map(pango::Layout::new);
        bar.layout(bounds, |i| {
            let text: String = bar.items[i].label.chars().filter(|&c| c != '&').collect();
            let text_w = self.pango_str_width(&pango_layout, &text, char_w);
            crate::primitives::menu_bar::MenuBarItemMeasure::new(text_w + 16.0)
        })
    }

    fn draw_split(&mut self, rect: QRect, split: &Split) -> crate::primitives::split::SplitLayout {
        let (cr, _layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_split called outside enter_frame_scope");
        crate::gtk::draw_split(
            cr,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            split,
            &self.current_theme,
        )
    }

    fn split_layout(&self, rect: QRect, split: &Split) -> crate::primitives::split::SplitLayout {
        crate::gtk::gtk_split_layout(
            split,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    fn draw_panel(
        &mut self,
        rect: QRect,
        panel: &crate::primitives::panel::Panel,
    ) -> crate::primitives::panel::PanelLayout {
        let line_height = self.current_line_height;
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_panel called outside enter_frame_scope");
        crate::gtk::draw_panel(
            cr,
            pango_layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            panel,
            &theme,
            line_height,
        )
    }

    fn panel_layout(
        &self,
        rect: QRect,
        panel: &crate::primitives::panel::Panel,
    ) -> crate::primitives::panel::PanelLayout {
        crate::gtk::gtk_panel_layout(
            panel,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
        )
    }

    fn draw_toast_stack(
        &mut self,
        rect: QRect,
        stack: &crate::primitives::toast::ToastStack,
    ) -> crate::primitives::toast::ToastStackLayout {
        let line_height = self.current_line_height;
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_toast_stack called outside enter_frame_scope");
        crate::gtk::draw_toast_stack(
            cr,
            pango_layout,
            rect.width as f64,
            rect.height as f64,
            stack,
            &theme,
            line_height,
        )
    }

    fn draw_pipeline_view(
        &mut self,
        rect: QRect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_pipeline_view called outside enter_frame_scope");
        crate::gtk::draw_pipeline_view(
            cr,
            pango_layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            view,
            &theme,
        )
    }

    fn pipeline_view_layout(
        &self,
        rect: QRect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        crate::gtk::gtk_pipeline_view_layout(
            view,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    fn draw_progress(
        &mut self,
        rect: QRect,
        bar: &crate::primitives::progress::ProgressBar,
    ) -> crate::primitives::progress::ProgressBarLayout {
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_progress called outside enter_frame_scope");
        crate::gtk::draw_progress(
            cr,
            pango_layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            bar,
            &theme,
        )
    }

    fn progress_layout(
        &self,
        rect: QRect,
        bar: &crate::primitives::progress::ProgressBar,
    ) -> crate::primitives::progress::ProgressBarLayout {
        crate::gtk::gtk_progress_layout(
            bar,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    fn draw_spinner(
        &mut self,
        rect: QRect,
        spinner: &crate::primitives::spinner::Spinner,
    ) -> crate::primitives::spinner::SpinnerLayout {
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_spinner called outside enter_frame_scope");
        crate::gtk::draw_spinner(
            cr,
            pango_layout,
            rect.x as f64,
            rect.y as f64,
            spinner,
            &theme,
        )
    }

    fn spinner_layout(
        &self,
        rect: QRect,
        spinner: &crate::primitives::spinner::Spinner,
    ) -> crate::primitives::spinner::SpinnerLayout {
        spinner.layout(
            rect.x,
            rect.y,
            crate::primitives::spinner::SpinnerMeasure::new(
                rect.width,
                self.current_line_height as f32,
            ),
        )
    }

    fn toast_stack_layout(
        &self,
        rect: QRect,
        stack: &crate::primitives::toast::ToastStack,
    ) -> crate::primitives::toast::ToastStackLayout {
        // Layout-only path needs Pango for text measurement, but this
        // runs outside the frame scope (from click handlers). Use a
        // fixed-size approximation — same pattern as menu_bar_layout
        // which uses current_char_width.
        stack.layout(rect.width, rect.height, 12.0, 8.0, |i| {
            let toast = &stack.toasts[i];
            let h = if toast.body.is_empty() {
                self.current_line_height as f32 + 16.0
            } else {
                self.current_line_height as f32 * 2.0 + 16.0
            };
            let action_w = toast
                .action
                .as_ref()
                .map(|a| a.label.len() as f32 * self.current_char_width as f32 + 16.0)
                .unwrap_or(0.0);
            crate::primitives::toast::ToastMeasure {
                width: 320.0_f32.min(rect.width - 24.0),
                height: h,
                dismiss_width: 28.0,
                action_width: action_w,
            }
        })
    }

    fn draw_command_center(
        &mut self,
        rect: QRect,
        cc: &crate::primitives::command_center::CommandCenter,
    ) -> crate::primitives::command_center::CommandCenterLayout {
        let line_height = self.current_line_height;
        let theme = self.current_theme;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_command_center called outside enter_frame_scope");
        crate::gtk::draw_command_center(
            cr,
            pango_layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            cc,
            &theme,
            line_height,
        )
    }

    fn command_center_layout(
        &self,
        rect: QRect,
        cc: &crate::primitives::command_center::CommandCenter,
    ) -> crate::primitives::command_center::CommandCenterLayout {
        let char_w = self.current_char_width as f32;
        let bounds = crate::event::Rect::new(rect.x, rect.y, rect.width, rect.height);
        let search_w = if cc.search_label.is_empty() {
            0.0
        } else {
            (cc.search_label.len() as f32 * char_w + 16.0).max(280.0)
        };
        cc.layout(
            bounds,
            crate::primitives::command_center::CommandCenterMeasure {
                arrow_width: 24.0,
                gap: 8.0,
                search_box_width: search_w,
                height: rect.height,
            },
        )
    }

    fn draw_chart(
        &mut self,
        rect: QRect,
        chart: &crate::primitives::chart::Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> crate::primitives::chart::ChartLayout {
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        let (cr, pango_layout) = self
            .current_frame_refs()
            .expect("GtkBackend::draw_chart called outside enter_frame_scope");
        crate::gtk::draw_chart(
            cr,
            pango_layout,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            chart,
            &theme,
            line_height,
            char_width,
            hovered_point,
            crosshair_x,
        )
    }

    fn chart_layout(
        &self,
        rect: QRect,
        chart: &crate::primitives::chart::Chart,
    ) -> crate::primitives::chart::ChartLayout {
        crate::gtk::gtk_chart_layout(
            chart,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
            self.current_char_width,
        )
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
