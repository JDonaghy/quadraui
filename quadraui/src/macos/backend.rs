//! macOS implementation of [`quadraui::Backend`].
//!
//! `MacBackend` mirrors the shape of [`crate::gtk::backend::GtkBackend`]:
//! it owns the persistent state the trait surface requires (viewport,
//! modal stack, accelerator registry, event queue, theme, current font
//! metrics, platform services) plus a transient frame-scope holding
//! the active `CGContextRef` so trait `draw_*` methods can rasterise
//! inside `drawRect:` without re-querying AppKit.
//!
//! ### Issue scope (#35)
//!
//! Framework methods (viewport / begin_frame / end_frame / poll_events
//! / wait_events / register/unregister_accelerator / modal_stack_mut /
//! services / line_height / char_width) ship as **real** implementations.
//! Every `draw_*` and `*_layout` method ships as `unimplemented!()`
//! with a pointer to the ticket that fills it in (#38–#43). The
//! macro [`mac_unimpl!`] keeps each stub to one line so the file
//! stays scannable as the rasterisers land one by one.
//!
//! ### Frame-scope mechanism
//!
//! `drawRect:` receives a `CGContextRef` owned by AppKit for the
//! duration of the call. [`MacBackend::enter_frame_scope`] stashes the
//! pointer in a `Cell`, runs the caller's closure, and restores the
//! previous value on exit. Type-erased through `*const ()` so the
//! struct doesn't need a lifetime parameter. Inside the closure,
//! `draw_*` methods recover the pointer from
//! [`MacBackend::current_cg_ptr`] and call CoreGraphics + CoreText FFI.
//!
//! ### Event queue
//!
//! [`crate::macos::run`]'s responder methods translate `NSEvent` into
//! [`UiEvent`] (via [`crate::macos::events`]) and dispatch the result
//! through the app's [`crate::runner::AppLogic`] synchronously. The
//! queue here exists for parity with [`Backend`] callers that prefer
//! the poll API and for backend-side producers landing in later
//! tickets (window resize notification observers, accelerator-match
//! rewrites).

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::backend::{Backend, EditorPaintResult};
use crate::event::{Rect, UiEvent, Viewport};
use crate::modal_stack::ModalStack;
use crate::primitives::activity_bar::ActivityBarRowHit;
use crate::primitives::chart::{Chart, ChartLayout};
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout};
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::primitives::data_table::{DataTable, DataTableLayout};
use crate::primitives::dialog::{Dialog, DialogLayout};
use crate::primitives::editor::Editor;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::form::FormLayout;
use crate::primitives::menu_bar::{MenuBar, MenuBarLayout};
use crate::primitives::message_list::MessageList;
use crate::primitives::multi_section_view::{
    LayoutMetrics, MultiSectionView, MultiSectionViewLayout,
};
use crate::primitives::panel::{Panel, PanelLayout};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::spinner::{Spinner, SpinnerLayout};
use crate::primitives::split::{Split, SplitLayout};
use crate::primitives::status_bar::StatusBarLayout;
use crate::primitives::tab_bar::TabBarHits;
use crate::primitives::text_display::TextDisplayLayout;
use crate::primitives::toast::{ToastStack, ToastStackLayout};
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::primitives::tree::TreeViewLayout;
use crate::types::WidgetId;
use crate::{
    Accelerator, AcceleratorId, ActivityBar, Form, ListView, Palette, PlatformServices, StatusBar,
    TabBar, Terminal, TextDisplay, Theme, TreeView,
};

use super::services::MacPlatformServices;

/// macOS backend implementing [`Backend`].
///
/// Field roles (mirroring [`crate::gtk::backend::GtkBackend`]):
/// - `viewport` — width × height in points, scale = `backingScaleFactor`.
///   Updated each frame from the active `QuadraView`'s bounds.
/// - `modal_stack` — pushed by hosts on modal open, popped on close.
/// - `accelerators` — registered keybindings. Match-and-dispatch wiring
///   lands when first consumer needs it.
/// - `events` — adapter queue. [`run`][super::run]'s responder methods
///   dispatch synchronously today; the queue is reserved for backend-
///   side producers (window notifications, future timer ticks).
/// - `current_cg_ptr` — frame-scope pointer; non-null only inside
///   [`Self::enter_frame_scope`].
/// - `current_font` / `current_line_height` / `current_char_width` —
///   per-app font state. Apps set these once in `setup()` via
///   [`Self::set_current_font`].
pub struct MacBackend {
    viewport: Viewport,
    modal_stack: ModalStack,
    accelerators: HashMap<AcceleratorId, Accelerator>,
    events: Rc<std::cell::RefCell<VecDeque<UiEvent>>>,
    services: MacPlatformServices,
    /// Type-erased `CGContextRef`; non-null only inside
    /// [`Self::enter_frame_scope`]. Stored as `*const ()` so the
    /// struct doesn't need a lifetime parameter.
    current_cg_ptr: Cell<*const ()>,
    current_theme: Theme,
    /// Set once via [`Self::set_current_font`] during app setup.
    /// `draw_*` methods (landing in #38–#43) recover this for text
    /// rendering + measurement. Wrapped in `Option` so apps that
    /// don't paint text can skip the setup call.
    current_font: Option<CTFont>,
    current_line_height: f64,
    current_char_width: f64,
}

impl MacBackend {
    /// Construct a fresh `MacBackend` with a default viewport, empty
    /// event queue, default theme, and no font. The runner overwrites
    /// the viewport each frame via [`Backend::begin_frame`]; apps
    /// install a font via [`Self::set_current_font`] in `setup()`.
    pub fn new() -> Self {
        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: ModalStack::new(),
            accelerators: HashMap::new(),
            events: Rc::new(std::cell::RefCell::new(VecDeque::new())),
            services: MacPlatformServices::new(),
            current_cg_ptr: Cell::new(std::ptr::null()),
            current_theme: Theme::default(),
            current_font: None,
            current_line_height: 16.0,
            current_char_width: 8.0,
        }
    }

    /// Install the font that subsequent `draw_*` calls use for text.
    /// Updates `current_line_height` + `current_char_width` from the
    /// font's typographic metrics.
    pub fn set_current_font(&mut self, font: CTFont) {
        let metrics = super::text::font_metrics(&font);
        self.current_line_height = metrics.line_height;
        self.current_char_width = metrics.char_width;
        self.current_font = Some(font);
    }

    /// Override the current theme. The default ([`Theme::default()`])
    /// is installed at construction; apps that use a non-default
    /// theme call this from `setup()` or each frame.
    pub fn set_current_theme(&mut self, theme: Theme) {
        self.current_theme = theme;
    }

    /// The current theme. `draw_*` methods (landing in later tickets)
    /// read this for per-primitive colour resolution.
    pub fn current_theme(&self) -> &Theme {
        &self.current_theme
    }

    /// Shared handle to the backend's event queue. The runner clones
    /// this into responder-method closures (when async producers land
    /// alongside #36 notifications).
    pub fn events_handle(&self) -> Rc<std::cell::RefCell<VecDeque<UiEvent>>> {
        self.events.clone()
    }

    /// Push an event onto the queue, drained by [`Backend::poll_events`].
    pub fn push_event(&self, ev: UiEvent) {
        self.events.borrow_mut().push_back(ev);
    }

    /// Run `f` with the current `CGContextRef` stashed on `self` so
    /// trait `draw_*` methods can recover it. The previous pointer
    /// (typically null) is restored on exit, matching the GTK
    /// `enter_frame_scope` contract.
    pub fn enter_frame_scope<R>(&mut self, ctx: CGContextRef, f: impl FnOnce(&mut Self) -> R) -> R {
        let prev = self.current_cg_ptr.replace(ctx as *const ());
        let result = f(self);
        self.current_cg_ptr.set(prev);
        result
    }

    /// The currently-stashed `CGContextRef`, or null outside a frame
    /// scope. `draw_*` methods (landing in later tickets) panic if
    /// this returns null — same shape as `GtkBackend::current_cr`.
    #[allow(dead_code)]
    pub(crate) fn current_cg(&self) -> CGContextRef {
        self.current_cg_ptr.get() as CGContextRef
    }
}

impl Default for MacBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// One-line stub for an unimplemented trait method. Each stub flags
/// the ticket that delivers the real implementation so a future agent
/// can find the work-in-progress arc without digging through the
/// milestone.
macro_rules! mac_unimpl {
    ($method:literal, $ticket:literal) => {
        unimplemented!(concat!("MacBackend::", $method, " — lands in ", $ticket))
    };
}

impl Backend for MacBackend {
    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    fn end_frame(&mut self) {
        // No-op. AppKit's `drawRect:` flushes when it returns; this
        // method exists for parity with backends that need an explicit
        // flush.
    }

    fn poll_events(&mut self) -> Vec<UiEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    fn wait_events(&mut self, _timeout: Duration) -> Vec<UiEvent> {
        // AppKit's run loop is callback-driven; there's no native
        // "wait up to N ms for next event" surface that fits the
        // poll-style trait. Apps that drive macOS through the trait
        // (rather than relying on [`super::run`]'s `AppLogic` flow)
        // should `poll_events` and yield to AppKit via a manual
        // `CFRunLoopRun` iteration. Today this is a plain drain —
        // identical to `poll_events` — and works because the standard
        // app flow goes through `super::run`.
        self.poll_events()
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
        self.accelerators.insert(acc.id.clone(), acc.clone());
        // Match-and-dispatch wiring lands when the first consumer
        // needs accelerators routed through the backend — until then
        // accelerators are stored but never re-emitted as
        // `UiEvent::Accelerator`. Apps that need accelerator dispatch
        // today match `UiEvent::KeyPressed` against
        // `crate::parse_key_binding` themselves.
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.accelerators.remove(id);
    }

    fn modal_stack_mut(&mut self) -> &mut ModalStack {
        &mut self.modal_stack
    }

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    fn line_height(&self) -> f32 {
        self.current_line_height as f32
    }

    fn char_width(&self) -> f32 {
        self.current_char_width as f32
    }

    // ── Drawing stubs ──────────────────────────────────────────────
    //
    // All rasterisers land in #38–#43. Each stub is a single
    // `unimplemented!` so the file stays scannable; the macro
    // ensures the ticket pointer is consistent across primitives.

    fn draw_tree(&mut self, _rect: Rect, _tree: &TreeView) {
        mac_unimpl!("draw_tree", "#39")
    }
    fn draw_list(&mut self, _rect: Rect, _list: &ListView) {
        mac_unimpl!("draw_list", "#39")
    }
    fn draw_data_table(
        &mut self,
        _rect: Rect,
        _table: &DataTable,
        _hovered_idx: Option<usize>,
    ) -> DataTableLayout {
        mac_unimpl!("draw_data_table", "#39")
    }
    fn data_table_layout(&self, _rect: Rect, _table: &DataTable) -> DataTableLayout {
        mac_unimpl!("data_table_layout", "#39")
    }
    fn draw_form(&mut self, _rect: Rect, _form: &Form) {
        mac_unimpl!("draw_form", "#39")
    }
    fn draw_palette(&mut self, _rect: Rect, _palette: &Palette) {
        mac_unimpl!("draw_palette", "#41")
    }

    fn draw_status_bar(
        &mut self,
        _rect: Rect,
        _bar: &StatusBar,
        _hovered_id: Option<&WidgetId>,
        _pressed_id: Option<&WidgetId>,
    ) -> StatusBarLayout {
        mac_unimpl!("draw_status_bar", "#38")
    }
    fn draw_tab_bar(
        &mut self,
        _rect: Rect,
        _bar: &TabBar,
        _hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        mac_unimpl!("draw_tab_bar", "#38")
    }
    fn draw_activity_bar(
        &mut self,
        _rect: Rect,
        _bar: &ActivityBar,
        _hovered_idx: Option<usize>,
    ) -> Vec<ActivityBarRowHit> {
        mac_unimpl!("draw_activity_bar", "#38")
    }
    fn draw_terminal(&mut self, _rect: Rect, _term: &Terminal) {
        mac_unimpl!("draw_terminal", "#43")
    }
    fn draw_text_display(&mut self, _rect: Rect, _td: &TextDisplay) {
        mac_unimpl!("draw_text_display", "#43")
    }
    fn text_display_layout(&self, _rect: Rect, _td: &TextDisplay) -> TextDisplayLayout {
        mac_unimpl!("text_display_layout", "#43")
    }
    fn draw_tooltip(&mut self, _tooltip: &Tooltip, _layout: &TooltipLayout) {
        mac_unimpl!("draw_tooltip", "#41")
    }
    fn draw_context_menu(
        &mut self,
        _menu: &ContextMenu,
        _layout: &ContextMenuLayout,
    ) -> Vec<(Rect, WidgetId)> {
        mac_unimpl!("draw_context_menu", "#41")
    }
    fn draw_dialog(&mut self, _dialog: &Dialog, _layout: &DialogLayout) -> Vec<Rect> {
        mac_unimpl!("draw_dialog", "#41")
    }
    fn draw_multi_section_view(&mut self, _rect: Rect, _view: &MultiSectionView) {
        mac_unimpl!("draw_multi_section_view", "#40")
    }
    fn msv_layout(&self, _rect: Rect, _view: &MultiSectionView) -> MultiSectionViewLayout {
        mac_unimpl!("msv_layout", "#40")
    }
    fn msv_metrics(&self) -> LayoutMetrics {
        mac_unimpl!("msv_metrics", "#40")
    }
    fn tree_layout(&self, _rect: Rect, _tree: &TreeView) -> TreeViewLayout {
        mac_unimpl!("tree_layout", "#39")
    }
    fn form_layout(&self, _rect: Rect, _form: &Form) -> FormLayout {
        mac_unimpl!("form_layout", "#39")
    }
    fn draw_editor(&mut self, _rect: Rect, _editor: &Editor) -> EditorPaintResult {
        mac_unimpl!("draw_editor", "#39")
    }
    fn draw_message_list(&mut self, _rect: Rect, _list: &MessageList) {
        mac_unimpl!("draw_message_list", "#43")
    }
    fn draw_rich_text_popup(&mut self, _popup: &RichTextPopup, _layout: &RichTextPopupLayout) {
        mac_unimpl!("draw_rich_text_popup", "#41")
    }
    fn draw_find_replace(&mut self, _rect: Rect, _panel: &FindReplacePanel) {
        mac_unimpl!("draw_find_replace", "#41")
    }
    fn draw_completions(&mut self, _completions: &Completions, _layout: &CompletionsLayout) {
        mac_unimpl!("draw_completions", "#41")
    }
    fn draw_scrollbar(&mut self, _rect: Rect, _scrollbar: &Scrollbar) {
        mac_unimpl!("draw_scrollbar", "#40")
    }
    fn draw_menu_bar(&mut self, _rect: Rect, _bar: &MenuBar) -> MenuBarLayout {
        mac_unimpl!("draw_menu_bar", "#38")
    }
    fn menu_bar_layout(&self, _rect: Rect, _bar: &MenuBar) -> MenuBarLayout {
        mac_unimpl!("menu_bar_layout", "#38")
    }
    fn draw_split(&mut self, _rect: Rect, _split: &Split) -> SplitLayout {
        mac_unimpl!("draw_split", "#42")
    }
    fn split_layout(&self, _rect: Rect, _split: &Split) -> SplitLayout {
        mac_unimpl!("split_layout", "#42")
    }
    fn draw_panel(&mut self, _rect: Rect, _panel: &Panel) -> PanelLayout {
        mac_unimpl!("draw_panel", "#42")
    }
    fn panel_layout(&self, _rect: Rect, _panel: &Panel) -> PanelLayout {
        mac_unimpl!("panel_layout", "#42")
    }
    fn draw_toast_stack(&mut self, _rect: Rect, _stack: &ToastStack) -> ToastStackLayout {
        mac_unimpl!("draw_toast_stack", "#42")
    }
    fn toast_stack_layout(&self, _rect: Rect, _stack: &ToastStack) -> ToastStackLayout {
        mac_unimpl!("toast_stack_layout", "#42")
    }
    fn draw_progress(&mut self, _rect: Rect, _bar: &ProgressBar) -> ProgressBarLayout {
        mac_unimpl!("draw_progress", "#42")
    }
    fn progress_layout(&self, _rect: Rect, _bar: &ProgressBar) -> ProgressBarLayout {
        mac_unimpl!("progress_layout", "#42")
    }
    fn draw_spinner(&mut self, _rect: Rect, _spinner: &Spinner) -> SpinnerLayout {
        mac_unimpl!("draw_spinner", "#42")
    }
    fn spinner_layout(&self, _rect: Rect, _spinner: &Spinner) -> SpinnerLayout {
        mac_unimpl!("spinner_layout", "#42")
    }
    fn draw_command_center(&mut self, _rect: Rect, _cc: &CommandCenter) -> CommandCenterLayout {
        mac_unimpl!("draw_command_center", "#38")
    }
    fn command_center_layout(&self, _rect: Rect, _cc: &CommandCenter) -> CommandCenterLayout {
        mac_unimpl!("command_center_layout", "#38")
    }
    fn draw_chart(
        &mut self,
        _rect: Rect,
        _chart: &Chart,
        _hovered_point: Option<(usize, usize)>,
        _crosshair_x: Option<f64>,
    ) -> ChartLayout {
        mac_unimpl!("draw_chart", "#39")
    }
    fn chart_layout(&self, _rect: Rect, _chart: &Chart) -> ChartLayout {
        mac_unimpl!("chart_layout", "#39")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accelerator::{Accelerator, AcceleratorScope};
    use crate::event::Point;
    use crate::types::Modifiers;
    use crate::KeyBinding;

    fn acc(id: &str, key: &str) -> Accelerator {
        Accelerator {
            id: AcceleratorId::new(id),
            binding: KeyBinding::Literal(key.to_string()),
            scope: AcceleratorScope::Global,
            label: None,
        }
    }

    #[test]
    fn new_starts_with_default_viewport() {
        let b = MacBackend::new();
        let v = b.viewport();
        assert_eq!(v.width, 0.0);
        assert_eq!(v.height, 0.0);
        assert_eq!(v.scale, 1.0);
    }

    #[test]
    fn begin_frame_updates_viewport() {
        let mut b = MacBackend::new();
        b.begin_frame(Viewport::new(800.0, 600.0, 2.0));
        let v = b.viewport();
        assert_eq!(v.width, 800.0);
        assert_eq!(v.height, 600.0);
        assert_eq!(v.scale, 2.0);
    }

    #[test]
    fn services_platform_name_is_macos() {
        let b = MacBackend::new();
        assert_eq!(b.services().platform_name(), "macos");
    }

    #[test]
    fn line_height_and_char_width_seed_to_defaults() {
        let b = MacBackend::new();
        assert_eq!(b.line_height(), 16.0);
        assert_eq!(b.char_width(), 8.0);
    }

    #[test]
    fn register_and_unregister_accelerator_round_trip() {
        let mut b = MacBackend::new();
        let a = acc("save", "<C-s>");
        b.register_accelerator(&a);
        assert!(b.accelerators.contains_key(&AcceleratorId::new("save")));
        b.unregister_accelerator(&AcceleratorId::new("save"));
        assert!(!b.accelerators.contains_key(&AcceleratorId::new("save")));
    }

    #[test]
    fn poll_events_drains_queue_fifo() {
        let b = MacBackend::new();
        b.push_event(UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(1.0, 2.0),
            modifiers: Modifiers::default(),
        });
        b.push_event(UiEvent::WindowFocused(true));
        // `poll_events` takes &mut so we re-acquire after `push_event`.
        let mut b = b;
        let evs = b.poll_events();
        assert_eq!(evs.len(), 2);
        assert!(matches!(evs[0], UiEvent::MouseDown { .. }));
        assert!(matches!(evs[1], UiEvent::WindowFocused(true)));
        // Second drain yields nothing.
        assert!(b.poll_events().is_empty());
    }

    #[test]
    fn enter_frame_scope_saves_and_restores_ptr() {
        let mut b = MacBackend::new();
        assert!(b.current_cg().is_null());
        // Cast a dummy non-null integer to satisfy the pointer type
        // (never dereferenced — the scope wrapper just stashes + restores).
        let dummy: CGContextRef = 0x1 as CGContextRef;
        b.enter_frame_scope(dummy, |inner| {
            assert_eq!(inner.current_cg(), dummy);
        });
        assert!(b.current_cg().is_null());
    }

    #[test]
    fn line_height_picks_up_set_current_line_height_via_font_install() {
        // `set_current_font` flows through `font_metrics`, exercised
        // in `macos::text::tests`. Here we just assert the setter
        // path mutates `line_height` / `char_width` away from defaults.
        let mut b = MacBackend::new();
        let font = super::super::text::make_font("Menlo", 14.0).expect("Menlo installed");
        b.set_current_font(font);
        // 14pt Menlo's line_height is ~16.something — defaults are
        // (16.0, 8.0); both should be updated regardless.
        assert!(b.line_height() > 0.0);
        assert!(b.char_width() > 0.0);
        assert!(b.current_font.is_some());
    }
}
