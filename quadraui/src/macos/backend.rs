//! macOS implementation of [`quadraui::Backend`].
//!
//! `MacBackend` mirrors the shape of [`crate::gtk::backend::GtkBackend`]:
//! it owns the persistent state the trait surface requires (viewport,
//! modal stack, accelerator registry, event queue, theme, current font
//! metrics, platform services) plus a transient frame-scope holding
//! the active `CGContextRef` so trait `draw_*` methods can rasterise
//! inside `drawRect:` without re-querying AppKit.
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
//! through the app's [`crate::runner::AppLogic`] synchronously —
//! including the accelerator-match / double-click-fold / paste-
//! interception pre-processing `run`'s `handle` closure applies before
//! `AppLogic::handle` sees the event. The queue here exists for
//! parity with [`Backend`] callers that prefer the poll API and for
//! backend-side producers (native menu activations, context-menu
//! results). `WindowResized` (from the `NSViewFrameDidChangeNotification`
//! observer, #486) dispatches synchronously like mouse/keyboard events
//! rather than going through the queue.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use core_graphics::base::CGFloat;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::rc::Retained;
use objc2_app_kit::{
    NSCursor, NSEvent, NSFloatingWindowLevel, NSNormalWindowLevel, NSWindow, NSWindowButton,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use crate::backend::{
    Backend, BackendError, EditorPaintResult, PointerShape, ResizeEdge, ServiceResult,
    WindowControl,
};
use crate::desktop::WindowDragArm;
use crate::dispatch::{DoubleClickDetector, DragState, TextRegion};
use crate::event::{Point, Rect, UiEvent, UserPayload, Viewport};
use crate::generic_font::GenericFamily;
use crate::modal_stack::ModalStack;
use crate::paint_surface::PaintSurface;
use crate::primitives::activity_bar::ActivityBarRowHit;
use crate::primitives::board::{BoardLayout, BoardModel};
use crate::primitives::button::{Button, ButtonLayout};
use crate::primitives::canvas::{Canvas, CanvasLayout};
use crate::primitives::chart::{Chart, ChartLayout};
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout};
use crate::primitives::command_line::CommandLine;
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::primitives::data_table::{DataTable, DataTableLayout};
use crate::primitives::dialog::{Dialog, DialogInput, DialogLayout};
use crate::primitives::editor::Editor;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::form::FormLayout;
use crate::primitives::menu_bar::{MenuBar, MenuBarLayout};
use crate::primitives::message_list::MessageList;
use crate::primitives::multi_section_view::{
    MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout,
};
use crate::primitives::panel::{Panel, PanelLayout};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
use crate::primitives::radio_group::{
    RadioGroup, RadioGroupLayout, RadioOption, RadioOptionMeasure,
};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::spinner::{Spinner, SpinnerLayout};
use crate::primitives::split::{Split, SplitLayout};
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
use crate::primitives::status_bar::StatusBarLayout;
use crate::primitives::switch::{Switch, SwitchLayout};
// `TabBarHits` is deprecated — this backend constructs it directly
// (per the audit: `macos::tab_bar` has no intermediate `TabBarLayout`),
// so the import needs the same allow every use site below does.
#[allow(deprecated)]
use crate::primitives::tab_bar::TabBarHits;
use crate::primitives::text_display::TextDisplayLayout;
use crate::primitives::toast::{ToastCorner, ToastOverlay, ToastStackLayout};
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::primitives::tree::TreeViewLayout;
use crate::testing::{TextRun, ZoneRec};
use crate::transition::{lerp, Transition};
use crate::types::{Color, WidgetId};
use crate::KeyBinding;
use crate::{
    Accelerator, AcceleratorId, ActivityBar, FieldKind, Form, Key, ListView, Modifiers, Palette,
    ParsedBinding, PlatformServices, StatusBar, TabBar, TabBarLayout, Terminal, TextDisplay, Theme,
    TreeView,
};

use super::services::MacPlatformServices;

/// macOS backend implementing [`Backend`].
///
/// Field roles (mirroring [`crate::gtk::backend::GtkBackend`]):
/// - `viewport` — width × height in points, scale = `backingScaleFactor`.
///   Updated each frame from the active `QuadraView`'s bounds.
/// - `modal_stack` — pushed by hosts on modal open, popped on close.
/// - `core` — registered keybindings + text-selection bookkeeping,
///   consolidated into [`crate::backend_core::BackendCore`].
///   [`Self::match_keypress`] resolves a native keypress against the
///   registered bindings. [`run`][super::run]'s `handle` closure
///   rewrites a matching `KeyPressed` into `Accelerator` before
///   `AppLogic::handle` sees it.
/// - `double_click` — folds a `MouseDown` into `DoubleClick`; see
///   [`Self::fold_double_click`].
/// - `events` — adapter queue. [`run`][super::run]'s responder methods
///   dispatch synchronously today; the queue is used for backend-side
///   producers (native menu activations, context-menu results).
/// - `current_cg_ptr` — frame-scope pointer; non-null only inside
///   [`Self::enter_frame_scope`].
/// - `current_font` / `current_line_height` / `current_char_width` —
///   per-app *editor* font state. Apps set these once in `setup()` via
///   [`Self::set_current_font`].
/// - `chrome_font` / `chrome_line_height` / `chrome_char_width` — the
///   independent *chrome* (UI) font state, defaulting to the
///   CoreText system UI font. Set via [`Self::set_chrome_font`]
///   (`Backend::set_ui_font`'s inherent twin).
pub struct MacBackend {
    viewport: Viewport,
    /// `Rc<RefCell<>>` (not a plain field) so [`Backend::modal_stack_handle`]
    /// can hand back a handle that outlives any single `&mut self`
    /// borrow — the shape `GtkBackend` and `TuiBackend` already use for
    /// this.
    modal_stack: Rc<RefCell<ModalStack>>,
    /// See `modal_stack`'s doc comment — same rationale.
    drag_state: Rc<RefCell<DragState>>,
    /// Accelerator registry + text-selection bookkeeping — see
    /// [`crate::backend_core::BackendCore`]'s module doc. Replaces the
    /// formerly macOS-local `accelerators`/`parsed_accelerators` fields
    /// verbatim, plus (below) the formerly separate `text_selection`
    /// field.
    core: crate::backend_core::BackendCore,
    /// Folds a `MouseDown` `NSEvent` into `DoubleClick` when it lands
    /// within the time/position window of the previous click.
    /// `macos::run` dispatches synchronously per `NSEvent`, so this
    /// runs on one event at a time via `Self::fold_double_click`
    /// rather than `TuiBackend`'s per-poll batch.
    double_click: DoubleClickDetector,
    events: Rc<std::cell::RefCell<VecDeque<UiEvent>>>,
    services: MacPlatformServices,
    /// Type-erased `CGContextRef`; non-null only inside
    /// [`Self::enter_frame_scope`]. Stored as `*const ()` so the
    /// struct doesn't need a lifetime parameter.
    current_cg_ptr: Cell<*const ()>,
    current_theme: Theme,
    /// Captured once per frame, mirroring `current_theme` — see
    /// [`crate::Style`]'s module doc for why this is a separate field
    /// rather than folded into `Theme`.
    current_style: crate::Style,
    /// Decoded-`CGImage` cache for [`Backend::draw_image`]
    /// — see [`crate::image_cache`]'s module doc and
    /// [`super::image::draw_image`]'s "Decode cache" section for why
    /// this backend keys on source only (a `CGImage` decode is reused at
    /// every paint size, unlike GTK's already-scaled `Pixbuf`). Survives
    /// across frames, mirroring `GtkBackend::image_cache`.
    image_cache: crate::image_cache::ImageCache<CGImage>,
    /// Downsampled Core Text glyph atlas for [`Backend::draw_minimap`]'s
    /// [`MinimapRenderMode::Characters`] branch — see
    /// [`super::minimap`]'s "Glyph atlas" module doc section and
    /// [`crate::primitives::minimap::MinimapAtlasCache`]'s doc. Survives
    /// across frames, mirroring `GtkBackend::minimap_atlas_cache`.
    ///
    /// [`MinimapRenderMode::Characters`]: crate::primitives::minimap::MinimapRenderMode::Characters
    minimap_atlas_cache: crate::primitives::minimap::MinimapAtlasCache,
    /// Set once via [`Self::set_current_font`] during app setup.
    /// `draw_*` methods recover this for text rendering +
    /// measurement. Wrapped in `Option` so apps that don't paint
    /// text can skip the setup call.
    ///
    /// Every `draw_*`/`surface_*` call site that reads this
    /// falls back to `chrome_font` — never `None` — when it's unset,
    /// instead of panicking. A host that forgets
    /// `set_current_font` gets editor text rendered in the chrome font
    /// rather than an abort; `set_current_font` still overrides it the
    /// moment it's called, same as before.
    current_font: Option<CTFont>,
    current_line_height: f64,
    current_char_width: f64,
    /// Editor vertical scrollbar column width set through
    /// [`Backend::set_editor_v_scrollbar_width`]; `None` = one
    /// `current_char_width` wide.
    editor_v_scrollbar_w: Option<f32>,
    /// Chrome (UI) font — macOS is the only pixel backend where
    /// `set_ui_font` was a silent no-op and every piece of chrome
    /// painted in `current_font`, the *editor* font, instead. Unlike
    /// `current_font`, this is never `None`: [`Self::new`] seeds it with
    /// the CoreText system UI font so chrome already looks right before
    /// any [`Backend::set_ui_font`] call, mirroring GTK's `"Sans 11"` /
    /// Win-GUI's `Segoe UI` un-set defaults rather than macOS's own
    /// editor font falling back to "no font at all". Set via
    /// [`Self::set_chrome_font`]; never touched by
    /// [`Self::set_current_font`]/[`Backend::set_editor_font`] — see
    /// the metric-mismatch hazard that separation exists to avoid.
    chrome_font: CTFont,
    /// Cached [`super::text::font_metrics`] line height for
    /// `chrome_font`, in points — the chrome twin of
    /// `current_line_height`. Recomputed by [`Self::set_chrome_font`]
    /// whenever `chrome_font` changes.
    chrome_line_height: f64,
    /// Cached `char_width` for `chrome_font` — the chrome twin of
    /// `current_char_width`.
    chrome_char_width: f64,
    /// Family last set via [`Backend::set_nerd_font_fallback`], if any.
    /// Applied to `current_font` immediately if one is already set
    /// (see [`Self::set_nerd_font_fallback`]), and consulted by
    /// [`Self::set_current_font`] so a later `set_current_font` call
    /// (e.g. a runtime font-preference change) doesn't silently drop a
    /// fallback that was configured first — the two setters can land in
    /// either order.
    nerd_font_fallback_family: Option<String>,
    /// Retained installer target from the last [`Backend::install_menu_bar`]
    /// call. Holds it alive so action selectors on installed `NSMenuItem`s
    /// don't dangle. Replaced wholesale on each re-install.
    menu_target: Option<objc2::rc::Retained<super::menu_bar_install::QuadraMenuTarget>>,
    /// Last [`MenuBar`] installed via the native-default
    /// [`Backend::draw_menu_bar`] path. Compared against
    /// the incoming `bar` on every call so a generic caller (`frame.rs`,
    /// `MenuSystem::render`) can call `draw_menu_bar` unconditionally
    /// every frame without reinstalling an unchanged `NSMenu` tree each
    /// tick — see [`Self::install_menu_bar_if_changed`]. `None` until
    /// the native path installs for the first time; also `None` on a
    /// backend that never takes the native path (e.g. `Custom` style),
    /// which never reads this field.
    last_native_menu_bar: Option<MenuBar>,
    /// Tray/status-bar icon state — see [`super::tray`]'s
    /// module doc and `impl TrayService for MacBackend` below.
    tray: super::tray::MacTrayState,
    /// Whether `InlineInput` carets should currently paint their stroke
    /// (the "on" half of the blink cycle). Shared `Rc<Cell>` so the
    /// macOS run-loop blink timer can toggle it without holding a
    /// `MacBackend` reference. Defaults to `true` so headless tests
    /// (and the first frame after startup) paint a visible caret
    /// without any timer running.
    caret_visible: std::rc::Rc<std::cell::Cell<bool>>,
    /// Until this instant, the blink timer's tick callback skips
    /// toggling — used to keep the caret solid while the user types.
    /// Reset on every `KeyPressed` event in `macos::run`.
    caret_blink_pause_until: std::rc::Rc<std::cell::Cell<std::time::Instant>>,
    /// Widget zones registered during the current frame via
    /// [`Backend::register_zone`]. Cleared at the start of each frame by
    /// [`Backend::begin_frame`]. Mirrors `GtkBackend::zones` /
    /// `TuiBackend::zones`. Read by
    /// [`crate::testing::FrameInventory::zones`] via
    /// [`super::testing::MacDriver::inventory`].
    zones: Vec<ZoneRec>,
    /// Whether [`Self::enter_frame_scope`] should wrap its closure in
    /// [`super::text::start_recording_text`] / `stop_recording_text` and
    /// stash the result into `text_runs`. Off by default — the live
    /// runner never reads it — [`super::testing::MacDriver::new`] turns
    /// it on. Mirrors `GtkBackend::painted_text_recording`.
    painted_text_recording: bool,
    /// When `true`, entrance-slide and hover-fade animations complete
    /// instantly (zero-duration transitions). Off by default (the live
    /// runner shows the full animation);
    /// [`super::testing::MacDriver::new`] turns it on so conformance
    /// scenarios that `click_text` a toast's dismiss affordance hit the
    /// widget at its resting position rather than its mid-slide painted
    /// position.
    skip_animations: bool,
    /// Every [`super::text::draw_text`] call recorded during the last
    /// [`Self::enter_frame_scope`] with `painted_text_recording` on —
    /// see [`Self::set_painted_text_recording`].
    text_runs: Vec<TextRun>,
    /// `WidgetId` of the [`ActivityBar`] that painted itself with
    /// `is_keyboard_focused = true` this frame, if any. Set by
    /// [`Backend::draw_activity_bar`], cleared by [`Backend::begin_frame`]
    /// (same per-frame lifecycle as `zones`), read by
    /// [`super::run::dispatch_event`] to redirect the next `KeyPressed`
    /// into the bar. Mirrors `GtkBackend::focused_activity_bar`.
    focused_activity_bar: Option<WidgetId>,
    /// [`crate::backend::Backend::minimap_scale`]'s backing field — read by
    /// [`Backend::draw_minimap`]/[`Backend::minimap_layout`] to resolve
    /// the row pitch (and per-column width) a paint or layout call uses.
    /// Defaults to [`crate::primitives::minimap::MinimapScale::One`],
    /// the fixed pitch, so a host that never calls
    /// [`Backend::set_minimap_scale`] sees no behaviour change.
    minimap_scale: crate::primitives::minimap::MinimapScale,
    /// Top-level window handle, set once by `macos::run::run` via
    /// [`Self::set_window`]. `None` until the runner finishes
    /// constructing the window (and in unit tests, which never call
    /// it). Backs [`Backend::begin_window_drag`] /
    /// [`Backend::toggle_window_maximize`] / [`Backend::set_cursor`]
    /// — mirrors `GtkBackend::window`.
    window: Option<Retained<NSWindow>>,
    /// Raw press `NSEvent`, stashed by `macos::run`'s `mouseDown:`
    /// responder before the press is translated to a portable
    /// `UiEvent`. [`Backend::begin_window_drag`] consumes this (via
    /// `.take()`) — `performWindowDragWithEvent:` requires the
    /// *originating* mouse-down event, not a synthesized one.
    ///
    /// Stored in the shared [`crate::desktop::WindowDragArm`]
    /// purely as an arm/take stash: unlike `GtkBackend`'s
    /// `armed_window_drag`, there's no threshold-gated commit step
    /// here, because AppKit's `performWindowDragWithEvent:` already
    /// disambiguates a drag from a double-click internally (see that
    /// type's doc comment) — so the `origin_x`/`origin_y` `arm()`
    /// arguments are always `0.0` and go unread.
    pending_window_press: WindowDragArm<Retained<NSEvent>>,
    /// Mirrors `TuiBackend::nerd_fonts_enabled` / `GtkBackend::nerd_fonts_enabled`
    // /. Picks `Icon::glyph` vs `Icon::fallback` in
    /// `draw_activity_bar` and (since #804) `draw_tree`. Set via
    /// [`Backend::set_nerd_fonts`]; defaults to `true` on this backend —
    /// see `GtkBackend`'s matching field doc for why a
    /// GUI backend with the bundled codicon font always available
    /// carries none of the "terminal with no Nerd Font installed" risk
    /// [`Backend::set_nerd_fonts`]'s own doc gives for keeping TUI's
    /// default at `false`.
    nerd_fonts_enabled: bool,
    /// Single owner of keyboard focus — see
    /// [`crate::focus`]'s module doc. Mutated only by the shared
    /// Tab/Shift+Tab intercept in [`crate::runtime::preprocess_event`]
    /// via [`crate::runtime::PreprocessBackend::focus_manager_mut`];
    /// read elsewhere via [`Backend::focus_manager`].
    focus: crate::focus::FocusManager,
    /// Mirrors `TuiBackend::nerd_fonts_enabled` / `GtkBackend::nerd_fonts_enabled`.
    /// Picks `Icon::glyph` vs `Icon::fallback` in `draw_activity_bar` and
    /// `draw_tree`. Set via [`Backend::set_nerd_fonts`]; defaults to `true`
    /// on this backend — see `GtkBackend`'s matching field doc for why a
    /// GUI backend with the bundled codicon font always available carries
    /// none of the "terminal with no Nerd Font installed" risk
    /// [`Backend::set_nerd_fonts`]'s own doc gives for keeping TUI's
    /// default at `false`.
    user_events: Arc<crate::runtime::UserEventQueue>,
    /// Set once by `macos::run::run` via [`Self::set_wake_callback`],
    /// right after the `QuadraView` exists — see that method's doc for
    /// what it stores and why `waker()` needs a handle to it rather than
    /// touching AppKit directly. `None` until then (e.g. a `MacBackend`
    /// constructed directly by a test, never handed to `macos::run`);
    /// `waker()`'s returned closure is a documented no-op in that case.
    wake_callback: WakeCallback,
    /// Thread-safe inbox for [`crate::UiEvent::User`] payloads — see
    /// [`crate::runtime::UserEventQueue`]'s doc. [`Self::waker`] clones
    /// this `Arc` into the closure it hands out; [`Self::poll_events`]
    /// drains it into `UiEvent::User` alongside `events`, on the AppKit
    /// main thread, same as any other queued event.
    tick_callback: WakeCallback,
    /// Per-toast entrance-slide animation, keyed by [`Toast::id`]. Inserted
    /// in [`Self::draw_toast_overlay`] the first time a toast id is seen and
    /// retained only while that id is still present in the
    /// [`ToastOverlay`] passed in; drives the x-offset
    /// `crate::primitives::toast::native_surface_paint::paint_with_x_offset`
    /// slides a toast in from its stack's off-screen edge. Toasts are
    /// removed from the overlay (not animated out) on dismissal, so this
    /// only ever covers the entrance half of the motion: there is no
    /// lingering `Toast` to sample geometry from once dismissal removes it,
    /// so an exit slide has nothing to paint.
    toast_enter: HashMap<WidgetId, Transition>,
    /// Per-toolbar-item hover-fade-in animation, keyed by
    /// [`crate::primitives::toolbar::VisibleToolbarItem::action_id`].
    /// Inserted in [`Self::draw_toolbar_with_options`] the first frame an
    /// item is hovered and dropped the frame it stops being hovered — same
    /// entrance-only scoping as `toast_enter` above, since there is no
    /// hover-exit state to keep animating once the pointer has moved on.
    hover_fade: HashMap<WidgetId, Transition>,
}

/// The wake-target [`MacBackend::waker`] invokes — see
/// [`MacBackend::set_wake_callback`]'s doc for what installs it and why
/// it's shaped this way. Named to keep `MacBackend`'s field declaration
/// (and `clippy::type_complexity`) readable — mirrors
/// `gtk::backend`'s wake plumbing in intent only, not in shape: this one
/// wraps the `!Send` `Rc` in `dispatch2::MainThreadBound` (upstream's
/// audited wrapper — it carries a `MainThreadMarker` proof and
/// re-dispatches its own `Drop` back to the main thread), whereas GTK
/// never lets the `Rc` cross a thread boundary at all, parking it in a
/// thread-local keyed by an integer id.
type WakeCallback = Arc<std::sync::OnceLock<MainThreadBound<Rc<dyn Fn()>>>>;

/// Set once by `macos::run::run` via [`Self::set_tick_callback`],
/// right after the `QuadraView` exists — same shape as
/// [`Self::wake_callback`], but wired to `view.dispatch_tick()`
/// (fires `AppLogic::tick`) instead of a bare repaint request.
/// [`Self::request_frame_in`]'s `DispatchQueue::main().after` closure
/// looks this up when the scheduled delay elapses. `None` until then,
/// same "no-op, not a panic" posture as `wake_callback`.
const MAC_DOUBLE_CLICK_RADIUS: f32 = 4.0;

/// Default chrome (UI) font size in points, paired with the CoreText
/// system UI font [`MacBackend::new`] installs as `chrome_font` before
/// any [`Backend::set_ui_font`] call — matches GTK's `"Sans 11"` /
/// Win-GUI's `DEFAULT_UI_FONT_SIZE_PT` default.
const DEFAULT_UI_FONT_SIZE_PT: f64 = 11.0;

/// What [`parse_ui_font_desc`] resolved a font description's family
/// portion to.
enum ParsedFamily {
    /// A CSS/Pango generic token — always resolvable, since
    /// it names a native system role rather than an installed family; see
    /// [`GenericFamily`].
    Generic(GenericFamily),
    /// Position tolerance, in points, for [`MacBackend::fold_double_click`]'s
    /// [`DoubleClickDetector`].
    ///
    /// `ns_mouse_down` (`macos/events.rs`) passes raw `NSEvent` `x`/`y`
    /// straight through — unlike TUI's whole character cells, these are
    /// point-precision, so the detector's default radius is far tighter
    /// than two real mouse/trackpad clicks can reliably land within.
    /// `4.0` points is a rough approximation of AppKit's own double-click
    /// hit region; it's a heuristic, not a measured constant.
    Named(String),
}

/// Parse a Pango-style font description (`Backend::set_ui_font`'s
/// documented shape, e.g. `"Sans 13"`) into a Core-Text-ready
/// `(family, size_pt)` pair — the same convention
/// `win::backend::parse_ui_font_desc` implements for the identical trait
/// contract. A trailing whitespace-separated numeric token is the point
/// size, defaulting to [`DEFAULT_UI_FONT_SIZE_PT`] when `desc` carries
/// none — a bare comma-separated fallback list (`"SF Pro Text, Helvetica
/// Neue, Sans"`, no size at all) is a real shape `Backend::set_ui_font`
/// callers use, not a parse failure.
///
/// `desc`'s family portion is a Pango-style comma-separated fallback
/// list: everything before the size is split on `,`, each candidate
/// trimmed, and tried in order — a CSS/Pango generic token
/// ([`GenericFamily::parse`]) always resolves; a concrete name resolves
/// if [`super::text::make_font_exact`] confirms Core Text has it
/// installed. The first candidate that resolves either way wins, the
/// same semantics Pango/fontconfig give a comma family list.
///
/// Because trying each candidate is now this function's job, it calls
/// [`super::text::make_font_exact`] itself rather than leaving every
/// installed-family check to the caller. The caller ([`MacBackend::set_ui_font`])
/// still owns the *final* degrade decision — what to do when not one
/// candidate in the list resolved — via [`ParsedFamily::Named`]'s
/// unchecked-fallback case.
///
/// Returns `None` only when `desc` is empty/whitespace-only, or its
/// family portion is empty (e.g. `desc` is just a bare size).
fn parse_ui_font_desc(desc: &str) -> Option<(ParsedFamily, f64)> {
    let desc = desc.trim();
    if desc.is_empty() {
        return None;
    }
    let (family_part, size_pt) = match desc.rfind(' ') {
        Some(idx) if desc[idx + 1..].trim().parse::<f64>().is_ok() => {
            let (family, size_str) = desc.split_at(idx);
            (
                family.trim(),
                size_str
                    .trim()
                    .parse::<f64>()
                    .expect("just matched by the guard above"),
            )
        }
        _ => (desc, DEFAULT_UI_FONT_SIZE_PT),
    };
    if family_part.is_empty() {
        return None;
    }

    for candidate in family_part.split(',') {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        if let Some(generic) = GenericFamily::parse(candidate) {
            return Some((ParsedFamily::Generic(generic), size_pt));
        }
        if super::text::make_font_exact(candidate, size_pt).is_some() {
            return Some((ParsedFamily::Named(candidate.to_string()), size_pt));
        }
    }

    // Nothing in the list resolved — hand the caller the first candidate
    // unchecked, same as this function always did for a single
    // unresolvable family, so `set_ui_font` still makes the final
    // "degrade to the system font" call itself.
    let first = family_part
        .split(',')
        .map(str::trim)
        .find(|c| !c.is_empty())?;
    Some((ParsedFamily::Named(first.to_string()), size_pt))
}

/// Translate a parsed universal [`KeyBinding`] to macOS's native Cmd
/// idiom.
///
/// [`crate::accelerator::parse_binding`] is shared with `TuiBackend`, so
/// its universal arms (`KeyBinding::Save`, `Copy`, `Paste`, …) resolve to
/// a **Ctrl**-modifier `ParsedBinding` regardless of platform — correct
/// for TUI, wrong for macOS. `menu_bar_install::accelerator_to_ns` (the
/// native menu path) and `crate::accelerator::render_binding` (the
/// display/tooltip path) both already render these as **Cmd** on macOS,
/// so a `MacBackend::match_keypress` that compared the raw Ctrl
/// `ParsedBinding` against a real Cmd keypress would never fire.
/// Swap Ctrl for Cmd here so registration, native-menu resolution,
/// and rendering all agree.
///
/// `KeyBinding::Literal` bindings are left untouched — the app author
/// already chose the exact modifier they want (e.g. `<C-s>` for a
/// deliberate Ctrl+S that coexists with the native Cmd+S), and
/// `accelerator_to_ns`/`render_binding` don't rewrite literals either.
fn macos_universal_binding_modifiers(
    binding: &KeyBinding,
    mut parsed: ParsedBinding,
) -> ParsedBinding {
    if !matches!(binding, KeyBinding::Literal(_)) && parsed.modifiers.ctrl {
        parsed.modifiers.ctrl = false;
        parsed.modifiers.cmd = true;
    }
    parsed
}

/// Which of AppKit's three usable public cursor singletons a
/// [`PointerShape`] resolves to.
///
/// This exists so the *mapping decision* — the part with an opinion in
/// it, and the part a regression can silently change — is a plain Rust
/// value that can be asserted anywhere, on any thread, on any OS.
/// Turning a variant into the actual `Retained<NSCursor>` is a separate,
/// trivial step ([`mac_cursor_for_shape`]) that has to call AppKit and
/// therefore can only be exercised on a real macOS main thread.
///
/// Splitting the two is not cosmetic: `cargo test`'s libtest harness
/// runs each `#[test]` on a spawned worker thread, and AppKit object
/// vending off the main thread (in a process that never created an
/// `NSApplication`) is exactly the class of call `macos::menu_bar_install`'s
/// tests already gate behind `MainThreadMarker::new()`. A mapping test
/// written directly against `NSCursor` singletons inherits that hazard
/// while proving nothing AppKit-specific — the singleton identity is
/// Apple's invariant, not ours. Ours is *which* singleton each shape
/// picks, and that is what [`mac_cursor_kind`] makes testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacCursorKind {
    /// `NSCursor::arrowCursor` — the plain pointer.
    Arrow,
    /// `NSCursor::resizeUpDownCursor` — vertical resize glyph.
    ResizeUpDown,
    /// `NSCursor::resizeLeftRightCursor` — horizontal resize glyph.
    ResizeLeftRight,
}

/// The smallest `NSRect` enclosing both `a` and `b` — the same shape as
/// AppKit's own `NSUnionRect` C function, which `objc2-foundation`
/// doesn't bind, reimplemented here in pure Rust so
/// [`MacBackend::titlebar_control_inset`] can union the
/// Close/Miniaturize/Zoom button frames without an `unsafe` FFI call.
/// Pure and AppKit-free on purpose — trivially unit-testable without
/// a live `NSWindow`.
fn union_ns_rect(a: NSRect, b: NSRect) -> NSRect {
    let min_x = a.origin.x.min(b.origin.x);
    let min_y = a.origin.y.min(b.origin.y);
    let max_x = (a.origin.x + a.size.width).max(b.origin.x + b.size.width);
    let max_y = (a.origin.y + a.size.height).max(b.origin.y + b.size.height);
    NSRect {
        origin: NSPoint { x: min_x, y: min_y },
        size: NSSize {
            width: max_x - min_x,
            height: max_y - min_y,
        },
    }
}

/// Map a [`PointerShape`] to the [`MacCursorKind`] it should show.
/// Mirrors `gtk::backend::pointer_shape_cursor_name`, but `NSCursor`'s
/// public API (`objc2-app-kit`'s binding of it — see
/// `MacBackend::set_cursor`) only exposes horizontal
/// (`resizeLeftRightCursor`) and vertical (`resizeUpDownCursor`) resize
/// glyphs, unlike GTK's CSS cursor-name space which has all 8 directional
/// keywords. There is no public diagonal-resize `NSCursor` to fall back to
/// (`_windowResize*Cursor` selectors are private API), so the 4 corner
/// edges fall back to the plain arrow — honest about the gap (see
/// `crate::desktop`'s pointer-shape-scaffold doc) rather than pointing a
/// horizontal or vertical glyph in a direction the user isn't actually
/// dragging.
///
/// Pure and AppKit-free on purpose — see [`MacCursorKind`].
fn mac_cursor_kind(shape: PointerShape) -> MacCursorKind {
    match shape {
        PointerShape::Default => MacCursorKind::Arrow,
        PointerShape::Resize(ResizeEdge::North | ResizeEdge::South) => MacCursorKind::ResizeUpDown,
        PointerShape::Resize(ResizeEdge::East | ResizeEdge::West) => MacCursorKind::ResizeLeftRight,
        PointerShape::Resize(
            ResizeEdge::NorthEast
            | ResizeEdge::NorthWest
            | ResizeEdge::SouthEast
            | ResizeEdge::SouthWest,
        ) => MacCursorKind::Arrow,
    }
}

/// Vend the shared `NSCursor` singleton for a [`PointerShape`], via
/// [`mac_cursor_kind`]. The only AppKit-touching half of the
/// mapping; see [`MacCursorKind`] for why the two halves are separate.
fn mac_cursor_for_shape(shape: PointerShape) -> Retained<NSCursor> {
    // `resizeUpDownCursor`/`resizeLeftRightCursor` are deprecated in AppKit
    // in favour of `rowResizeCursorInDirections:`/`columnResizeCursorInDirections:`
    // (divider-drag) or `frameResizeCursorFromPosition:inDirections:` (rectangular
    // resize) — neither is a drop-in replacement (both take a directions
    // argument this call site has no opinion on), so this keeps the simple
    // singleton for now rather than guessing at the right direction set.
    // The legacy cursors still work on every macOS version quadraui supports.
    #[allow(deprecated)]
    match mac_cursor_kind(shape) {
        MacCursorKind::Arrow => NSCursor::arrowCursor(),
        MacCursorKind::ResizeUpDown => NSCursor::resizeUpDownCursor(),
        MacCursorKind::ResizeLeftRight => NSCursor::resizeLeftRightCursor(),
    }
}

impl MacBackend {
    /// Construct a fresh `MacBackend` with a default viewport, empty
    /// event queue, default theme, and no font. The runner overwrites
    /// the viewport each frame via [`Backend::begin_frame`]; apps
    /// install a font via [`Self::set_current_font`] in `setup()`.
    pub fn new() -> Self {
        // Self-register the bundled codicon font unconditionally
        // before anything paints — see
        // `crate::gtk::app_font::ensure_codicon_registered`'s matching
        // call for the full "degrade, don't fail" rationale, which
        // applies identically here.
        super::text::ensure_codicon_registered();

        // Seed chrome with the CoreText system UI font so
        // chrome paints correctly before any `set_ui_font` call — see
        // `chrome_font`'s field doc. Wrapped with the bundled codicon
        // cascade so built-in chrome glyphs (tab dirty/close) resolve
        // from the very first frame, with no app call required — see
        // `font_with_builtin_fallback`'s own doc.
        let chrome_font = super::text::font_with_builtin_fallback(
            &super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT),
            None,
        );
        let chrome_metrics = super::text::font_metrics(&chrome_font);
        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: Rc::new(RefCell::new(ModalStack::new())),
            drag_state: Rc::new(RefCell::new(DragState::new())),
            core: crate::backend_core::BackendCore::default(),
            double_click: DoubleClickDetector::with_radius(MAC_DOUBLE_CLICK_RADIUS),
            events: Rc::new(std::cell::RefCell::new(VecDeque::new())),
            services: MacPlatformServices::new(),
            current_cg_ptr: Cell::new(std::ptr::null()),
            current_theme: Theme::default(),
            current_style: crate::Style::default(),
            image_cache: crate::image_cache::ImageCache::default(),
            minimap_atlas_cache: crate::primitives::minimap::MinimapAtlasCache::default(),
            current_font: None,
            current_line_height: 16.0,
            current_char_width: 8.0,
            editor_v_scrollbar_w: None,
            chrome_font,
            chrome_line_height: chrome_metrics.line_height,
            chrome_char_width: chrome_metrics.char_width,
            nerd_font_fallback_family: None,
            menu_target: None,
            last_native_menu_bar: None,
            tray: super::tray::MacTrayState::default(),
            caret_visible: std::rc::Rc::new(std::cell::Cell::new(true)),
            caret_blink_pause_until: std::rc::Rc::new(std::cell::Cell::new(
                std::time::Instant::now(),
            )),
            zones: Vec::new(),
            painted_text_recording: false,
            skip_animations: false,
            text_runs: Vec::new(),
            focused_activity_bar: None,
            minimap_scale: crate::primitives::minimap::MinimapScale::default(),
            window: None,
            pending_window_press: WindowDragArm::new(),
            nerd_fonts_enabled: true,
            focus: crate::focus::FocusManager::new(),
            user_events: crate::runtime::UserEventQueue::new(),
            wake_callback: Arc::new(std::sync::OnceLock::new()),
            tick_callback: Arc::new(std::sync::OnceLock::new()),
            toast_enter: HashMap::new(),
            hover_fade: HashMap::new(),
        }
    }

    /// Store the top-level window handle. Called once by
    /// `macos::run::run` right after the window is constructed. Backs
    /// [`Backend::begin_window_drag`] / [`Backend::toggle_window_maximize`]
    /// / [`Backend::set_cursor`] — mirrors `GtkBackend::set_window`.
    pub(crate) fn set_window(&mut self, window: Retained<NSWindow>) {
        self.window = Some(window);
    }

    /// Install the closure [`Backend::waker`]'s returned handle invokes on
    /// the AppKit main thread. `macos::run::run` calls this
    /// once, right after constructing the `QuadraView`, with a closure
    /// that calls `view.setNeedsDisplay(true)` — the same
    /// `ReactionSink::request_redraw` path `EventOutcome::Redraw` already
    /// uses for every other event, so a background-thread wake schedules a
    /// repaint through one call site rather than a second bespoke one.
    /// `mtm` proves the call happens on the main thread, matching
    /// [`dispatch2::MainThreadBound::new`]'s contract.
    ///
    /// A second call is a no-op ([`std::sync::OnceLock::set`] silently
    /// ignores it) — `run` only ever calls this once per backend instance.
    pub(crate) fn set_wake_callback(&self, callback: Rc<dyn Fn()>, mtm: MainThreadMarker) {
        let _ = self.wake_callback.set(MainThreadBound::new(callback, mtm));
    }

    /// Install the closure [`Self::request_frame_in`]'s scheduled-wake
    /// timer invokes once its delay elapses.
    /// `macos::run::run` calls this once, right after constructing the
    /// `QuadraView`, with a closure that calls `view.dispatch_tick()` —
    /// firing `AppLogic::tick` and applying its `Reaction`, same as
    /// [`Self::set_wake_callback`]'s closure does for `request_redraw`.
    ///
    /// A second call is a no-op ([`std::sync::OnceLock::set`] silently
    /// ignores it) — `run` only ever calls this once per backend instance.
    pub(crate) fn set_tick_callback(&self, callback: Rc<dyn Fn()>, mtm: MainThreadMarker) {
        let _ = self.tick_callback.set(MainThreadBound::new(callback, mtm));
    }

    /// Stash the raw press `NSEvent`. Called by `macos::run`'s
    /// `mouseDown:` responder before the press is translated to a
    /// portable `UiEvent`, so [`Backend::begin_window_drag`] has the
    /// originating event `performWindowDragWithEvent:` requires.
    /// Overwritten by the next press; harmless if never consumed
    /// (mirrors `GtkBackend::stash_window_press`).
    pub(crate) fn stash_window_press(&mut self, event: Retained<NSEvent>) {
        self.pending_window_press.arm(event, 0.0, 0.0);
    }

    /// Shared `Rc<Cell<bool>>` controlling whether `InlineInput` carets
    /// paint their stroke each frame. The run-loop blink timer clones
    /// this and toggles the cell to drive the blink animation; tests
    /// can pin a deterministic phase via [`Self::set_caret_visible`].
    pub fn caret_visible_handle(&self) -> std::rc::Rc<std::cell::Cell<bool>> {
        self.caret_visible.clone()
    }

    /// Shared `Rc<Cell<Instant>>` the blink timer reads to decide
    /// whether to skip its toggle this tick. `macos::run` resets it to
    /// `now + 500ms` on every `KeyPressed` so the caret stays solid
    /// while the user types.
    pub fn caret_blink_pause_handle(&self) -> std::rc::Rc<std::cell::Cell<std::time::Instant>> {
        self.caret_blink_pause_until.clone()
    }

    /// Override the caret-blink phase. Tests pin this to get
    /// reproducible paint snapshots; live apps let the blink timer
    /// drive it instead.
    pub fn set_caret_visible(&mut self, visible: bool) {
        self.caret_visible.set(visible);
    }

    /// Current blink phase. Read once per paint; the
    /// `multi_section_view` rasteriser skips the caret `fill_rect`
    /// when this is `false`.
    pub fn caret_visible(&self) -> bool {
        self.caret_visible.get()
    }

    /// Install the font that subsequent `draw_*` calls use for text.
    /// Updates `current_line_height` + `current_char_width` from the
    /// font's typographic metrics.
    ///
    /// If [`Backend::set_nerd_font_fallback`] was already called, `font`
    /// gets that fallback family applied (via
    /// [`super::text::font_with_builtin_fallback`]) before it's stored.
    /// Whichever of `set_nerd_font_fallback`/`set_current_font` runs
    /// second re-applies the other's effect instead of silently dropping it.
    pub fn set_current_font(&mut self, font: CTFont) {
        // `font_with_builtin_fallback` always appends the bundled
        // codicon family, plus `nerd_font_fallback_family` ahead of it
        // when the app has set one, so built-in chrome glyphs resolve
        // whether or not the app ever configured a Nerd-Font fallback.
        let font = super::text::font_with_builtin_fallback(
            &font,
            self.nerd_font_fallback_family.as_deref(),
        );
        let metrics = super::text::font_metrics(&font);
        self.current_line_height = metrics.line_height;
        self.current_char_width = metrics.char_width;
        self.current_font = Some(font);
    }

    /// Install the chrome (UI) font and refresh `chrome_line_height` /
    /// `chrome_char_width` from its metrics — the chrome twin of
    /// [`Self::set_current_font`].
    ///
    /// Unlike previous versions, `chrome_font` now *does* consult
    /// [`Self::nerd_font_fallback_family`] — the same `font_with_fallback`
    /// cascade [`Self::set_current_font`] applies. Icon glyphs in
    /// `draw_tree`/`draw_tab_bar_icons`/`draw_activity_bar_with_style`/
    /// `draw_toolbar_interactive`/`draw_sidebar_panel_interactive` now
    /// paint through `chrome_font`, not `current_font` — without this,
    /// a host that calls `set_nerd_font_fallback` would see icon glyphs
    /// render correctly in the editor but as tofu in every chrome
    /// primitive, since `chrome_font` would never have received the
    /// fallback cascade. Mirrors `GtkBackend::chrome_font_description`,
    /// which has applied `with_nerd_font_fallback` at every call site.
    pub fn set_chrome_font(&mut self, font: CTFont) {
        // See `set_current_font`'s matching comment —
        // `font_with_builtin_fallback` always carries the bundled
        // codicon cascade entry.
        let font = super::text::font_with_builtin_fallback(
            &font,
            self.nerd_font_fallback_family.as_deref(),
        );
        let metrics = super::text::font_metrics(&font);
        self.chrome_line_height = metrics.line_height;
        self.chrome_char_width = metrics.char_width;
        self.chrome_font = font;
    }

    /// Override the cached line height (in points) that every `draw_*`
    /// method reads via `self.current_line_height` — mirrors
    /// `GtkBackend::set_current_line_height` / `WinBackend::set_current_line_height`.
    ///
    /// Normally this backend derives `current_line_height` itself, from
    /// the real `CTFont` metrics, inside [`Self::set_current_font`] —
    /// unlike `GtkBackend`, which is *told* its metrics because Pango
    /// layout only happens host-side. A host that computes its own chrome
    /// layout generically across backends — sizing a header row, a toolbar
    /// strip, or the first painted line's offset from a *portable*
    /// line-height value — has no way to align that value with what
    /// `MacBackend` actually paints with, because `GtkBackend`/`WinBackend`
    /// accept the override and `MacBackend` silently ignored it. This
    /// setter closes that gap: a host that measures its own notion of line
    /// height (or wants pixel-for-pixel parity with a GTK/Win sibling build)
    /// can push it here, same call shape as the other two backends, instead
    /// of being stuck with whatever `set_current_font` derived.
    ///
    /// Does not touch `current_font` itself — glyph baseline positioning
    /// inside [`super::text::draw_text`] still uses `font.ascent()`
    /// directly, independent of this cached row-spacing value, exactly as
    /// it did before this method existed.
    pub fn set_current_line_height(&mut self, line_height: f64) {
        self.current_line_height = line_height;
    }

    /// Override the cached character width (in points) that every
    /// `draw_*` method reads via `self.current_char_width` — mirrors
    /// [`Self::set_current_line_height`]'s rationale and
    /// `GtkBackend::set_current_char_width` / `WinBackend::set_current_char_width`.
    pub fn set_current_char_width(&mut self, char_width: f64) {
        self.current_char_width = char_width;
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

    /// Override the current style. Mirrors [`Self::set_current_theme`]
    /// — see [`crate::Style`]'s module doc.
    pub fn set_current_style(&mut self, style: crate::Style) {
        self.current_style = style;
    }

    /// The current style. Mirrors [`Self::current_theme`].
    pub fn current_style(&self) -> &crate::Style {
        &self.current_style
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
        if self.painted_text_recording {
            super::text::start_recording_text();
        }
        let result = f(self);
        if self.painted_text_recording {
            self.text_runs = super::text::stop_recording_text();
        }
        self.current_cg_ptr.set(prev);
        result
    }

    /// Toggle whether [`Self::enter_frame_scope`] records every
    /// [`super::text::draw_text`] call into `text_runs`. Off by default;
    /// [`super::testing::MacDriver::new`] turns it on so
    /// [`crate::testing::FrameInventory::text_runs`] has something to
    /// report — mirrors `GtkBackend::set_painted_text_recording`.
    pub(crate) fn set_painted_text_recording(&mut self, enabled: bool) {
        self.painted_text_recording = enabled;
    }

    /// When `true`, entrance-slide and hover-fade transitions use
    /// `Duration::ZERO`, completing instantly so conformance-test
    /// click targets are at their resting position on the first frame.
    pub(crate) fn set_skip_animations(&mut self, skip: bool) {
        self.skip_animations = skip;
    }

    /// Text runs recorded during the last [`Self::enter_frame_scope`]
    /// call, when [`Self::set_painted_text_recording`] is on.
    pub(crate) fn text_runs(&self) -> &[TextRun] {
        &self.text_runs
    }

    /// `WidgetId` of the [`ActivityBar`] that declared
    /// `is_keyboard_focused = true` during the most recent render pass,
    /// or `None` if no bar is focused.
    ///
    /// Read by [`super::run::dispatch_event`] to decide whether a
    /// `KeyPressed` should be redirected into
    /// `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })`
    /// instead of reaching `AppLogic::handle` as a raw key. The macOS twin
    /// of `GtkBackend::focused_activity_bar_id` / `TuiBackend`'s
    /// `apply_dispatch` translation — without it, `ShellAdapter`'s
    /// built-in activity-bar keyboard navigation is unreachable on
    /// this backend.
    pub(crate) fn focused_activity_bar_id(&self) -> Option<&WidgetId> {
        self.focused_activity_bar.as_ref()
    }

    /// Zones registered during the last frame via
    /// [`Backend::register_zone`].
    pub(crate) fn zones(&self) -> &[ZoneRec] {
        &self.zones
    }

    /// The currently-stashed `CGContextRef`, or null outside a frame
    /// scope. `draw_*` methods panic if this returns null — same
    /// shape as `GtkBackend::current_cr`.
    pub(crate) fn current_cg(&self) -> CGContextRef {
        self.current_cg_ptr.get() as CGContextRef
    }

    // ── Accelerator matching ─────────────────────────────────────────

    /// Look up a registered `Global`-scope accelerator for a
    /// `(key, modifiers)` pair. Mirrors `TuiBackend::match_keypress` /
    /// `GtkBackend::match_keypress` — non-Global entries are skipped
    /// because this backend doesn't own focus/mode context the way a
    /// scoped `KeyMap` resolver does.
    ///
    /// Native Cmd keypresses (`ns_modifier_flags_to_quadraui` maps a real
    /// Cmd into `Modifiers { cmd: true, .. }`) compare directly against
    /// `BackendCore`'s parsed-binding list, which already stores
    /// universal bindings with Cmd instead of Ctrl — see
    /// [`macos_universal_binding_modifiers`], applied once at
    /// `register_accelerator` time (via
    /// [`crate::backend_core::BackendCore::register_accelerator_with`])
    /// so this lookup stays a plain equality check.
    pub(crate) fn match_keypress(&self, key: &Key, modifiers: Modifiers) -> Option<AcceleratorId> {
        self.core.match_keypress(key, modifiers)
    }

    // ── Double-click folding ─────────────────────────────────────────

    /// Fold a `MouseDown` into `DoubleClick` if it lands within the
    /// detector's time/position window of the previous click. Every
    /// other variant passes through unchanged. `macos::run` calls this
    /// on each translated `NSEvent` before handing it to `AppLogic`.
    pub(crate) fn fold_double_click(&mut self, ev: UiEvent) -> UiEvent {
        let mut events = [ev];
        self.double_click.process(&mut events);
        let [ev] = events;
        ev
    }

    // ── Text selection ───────────────────────────────────────────────
    //
    // The region registry + active-selection state machine lives in
    // [`crate::text_selection::TextSelectionState`] — the same shared
    // implementation `GtkBackend`/`TuiBackend`/`WinBackend` embed. Every
    // method below except [`Self::apply_selection_highlight`]/
    // [`Self::extract_selection_text`] (CoreGraphics painting via
    // [`super::text_selection::draw_selection_highlight`] /
    // `TextRegion::lines` extraction via the shared pixel-based
    // `crate::text_selection::extract_lines_pixel` — same helper GTK/Win
    // use, since macOS is pixel-based too) is a thin delegation. Mirrors
    // `WinBackend`'s identically-named, identically-shaped block in
    // `win/backend.rs`.

    /// Every `TextRegion` registered so far this frame. Test-only since
    /// text selection queries now go through [`Self::text_selection_state_mut`]
    /// instead.
    #[cfg(test)]
    pub(crate) fn text_regions(&self) -> &[TextRegion] {
        &self.core.text_selection.text_regions
    }

    /// Mutable access to the whole [`crate::text_selection::TextSelectionState`]
    /// — the shape [`crate::dispatch::route_pointer`] needs
    /// to both read `text_regions` and call `track_focused_text_region`
    /// without two separate accessor calls. Mirrors
    /// `GtkBackend::text_selection_state_mut`.
    pub(crate) fn text_selection_state_mut(
        &mut self,
    ) -> &mut crate::text_selection::TextSelectionState {
        &mut self.core.text_selection
    }

    /// Return the current active text selection, if any.
    pub(crate) fn active_text_selection(
        &self,
    ) -> Option<&crate::text_selection::ActiveTextSelection> {
        self.core.active_text_selection()
    }

    /// Update (or start) the active text selection. Called by
    /// `macos::run::dispatch_event` when a [`UiEvent::TextSelectionChanged`]
    /// event arrives, and by [`Self::select_all_text_region`].
    pub(crate) fn set_active_text_selection(
        &mut self,
        region: WidgetId,
        anchor: crate::event::Point,
        focus: crate::event::Point,
    ) {
        self.core.set_active_text_selection(region, anchor, focus);
    }

    /// Clear the active text selection highlight only (does NOT end an
    /// in-progress `TextSelection` drag). Called before dispatching a new
    /// mouse-down so the old highlight disappears without interrupting the
    /// drag that is about to start. Mirrors `GtkBackend::clear_selection_display`.
    pub(crate) fn clear_selection_display(&mut self) {
        self.core.clear_selection_display();
    }

    /// Clear the active text selection and end any in-progress
    /// `TextSelection` drag. Called after Ctrl-C copies the selection or
    /// on a plain click outside any text region.
    pub(crate) fn clear_text_selection(&mut self) {
        let mut drag = self.drag_state.borrow_mut();
        self.core.clear_text_selection(&mut drag);
    }

    /// End any in-progress `TextSelection` drag without clearing the
    /// displayed selection. Backs the `Backend` trait's
    /// `cancel_text_selection_drag` override — apps hosting an embedded
    /// terminal call it to abort a speculative drag before forwarding a
    /// click to a PTY. Mirrors `GtkBackend::cancel_text_selection_drag_impl`/
    /// `WinBackend`'s identically-named method.
    fn cancel_text_selection_drag_impl(&mut self) {
        let mut drag = self.drag_state.borrow_mut();
        self.core.cancel_text_selection_drag(&mut drag);
    }

    /// Set the active selection to cover the entire visible content of the
    /// most-recently focused `TextRegion` (the Ctrl-A target). See
    /// [`crate::text_selection::TextSelectionState::select_all_text_region`]
    /// for the resolution order and the viewport-only limitation.
    pub(crate) fn select_all_text_region(&mut self) -> bool {
        self.core.select_all_text_region()
    }

    /// Extract the selected text from the active selection's `TextRegion`
    /// using its stored `lines` (macOS is pixel-based, like GTK/Win-GUI —
    /// see `TextRegion::lines`'s doc). Empty when there is no active
    /// selection, the region isn't registered this frame, or it has no
    /// `lines` content.
    pub(crate) fn extract_selection_text(&self) -> String {
        self.core.extract_selection_text_pixel(
            self.current_line_height as f32,
            self.current_char_width as f32,
        )
    }

    /// Paint the active text-selection highlight on top of the frame's
    /// already-painted content. Must be called from inside
    /// [`Self::enter_frame_scope`], after `app.render` has run (so the
    /// highlight sits on top of the rendered content) — see
    /// `macos::run::render_frame`. Mirrors `GtkBackend::apply_selection_highlight`'s
    /// Cairo twin and `WinBackend::apply_selection_highlight`'s Direct2D
    /// twin. No-op when there is no active selection, the region isn't
    /// registered this frame, or metrics aren't known yet.
    ///
    /// The rect resolution is
    /// [`crate::backend_core::BackendCore::selection_highlight_rects`] —
    /// shared with `GtkBackend`/`WinBackend`'s twins; only the
    /// real `CGContextFillRect` calls in
    /// [`super::text_selection::draw_selection_highlight`] are macOS-
    /// specific real toolkit code.
    pub(crate) fn apply_selection_highlight(&self) {
        let char_w = self.current_char_width as f32;
        let line_h = self.current_line_height as f32;
        let Some(rects) = self.core.selection_highlight_rects(line_h, char_w) else {
            return;
        };
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::apply_selection_highlight called outside enter_frame_scope",
        );
        let color = self.current_theme.text_selection_highlight();
        // SAFETY: ctx is non-null inside the frame scope (see the
        // debug_assert above — a null ctx here means a caller ran this
        // outside `enter_frame_scope`, same contract every other draw_*
        // method on this backend relies on).
        unsafe {
            super::text_selection::draw_selection_highlight(ctx, &rects, color);
        }
    }

    /// Shared body of [`Backend::draw_status_bar_interactive`] /
    /// [`Backend::draw_status_bar_interactive_scaled`]. `font_scale`
    /// multiplies `chrome_font`'s point size for this paint only — `1.0`
    /// is the unscaled case.
    ///
    /// Temporarily swaps `chrome_font`/`chrome_line_height`/
    /// `chrome_char_width` (restored in every case, including the
    /// `font_scale == 1.0` no-op path, which skips the swap entirely
    /// rather than swap-then-immediately-restore the same values) rather
    /// than calling [`Self::set_chrome_font`] — that setter also
    /// re-applies [`Self::nerd_font_fallback_family`]'s cascade, which
    /// `chrome_font` already carries; re-running it here would append the
    /// fallback family to the family list a second time.
    fn status_bar_paint_scaled(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        interaction: &crate::interaction::InteractionState,
        font_scale: f32,
    ) -> StatusBarLayout {
        let (hovered_id, pressed_id) = (interaction.hovered(), interaction.pressed());
        let saved = if font_scale != 1.0 {
            // `.max(0.1)` guards against a degenerate `font_scale` (e.g.
            // `0.0` or negative) handing `clone_with_font_size` a
            // zero/negative point size — `CTFont` has no documented
            // fallback for that, so without a floor the behaviour would be
            // backend-inconsistent with GTK's own `.max(1)` guard on its
            // (integer, `PANGO_SCALE`-ths-of-a-point) size for the same
            // input. `0.1pt` mirrors GTK's intent of "paint (nearly)
            // nothing, honestly" rather than silently falling back to some
            // ambient size.
            let scaled_size = (self.chrome_font.pt_size() * font_scale as f64).max(0.1);
            let scaled_font = self.chrome_font.clone_with_font_size(scaled_size);
            let metrics = super::text::font_metrics(&scaled_font);
            let saved_font = std::mem::replace(&mut self.chrome_font, scaled_font);
            let saved_lh = self.chrome_line_height;
            let saved_cw = self.chrome_char_width;
            self.chrome_line_height = metrics.line_height;
            self.chrome_char_width = metrics.char_width;
            Some((saved_font, saved_lh, saved_cw))
        } else {
            None
        };
        // The status bar is chrome, not editor content, so it paints
        // through `ChromeSurface` (routes text through
        // `chrome_font`/`chrome_line_height`/`chrome_char_width`) —
        // self-documenting at this call site that the *scaled* chrome
        // font swapped in above, not whatever `self`'s own `PaintSurface`
        // impl happens to default to, is what this paint must use. See
        // `ChromeSurface`'s doc.
        let theme = self.current_theme;
        // Pass the rect's own height, not just the chrome line height,
        // so a caller that hands this a `rect` taller than the chrome
        // font's own line height (e.g. a fixed-height breadcrumb row)
        // gets a background fill that covers `rect.height` entirely,
        // matching `WinBackend`'s already-correct call.
        let line_height = rect.height;
        let mut surface = ChromeSurface { backend: self };
        let layout = crate::primitives::status_bar::native_surface_paint::paint(
            bar,
            &mut surface,
            &theme,
            rect.x,
            rect.y,
            rect.width,
            line_height,
            hovered_id,
            pressed_id,
        );
        if let Some((saved_font, saved_lh, saved_cw)) = saved {
            self.chrome_font = saved_font;
            self.chrome_line_height = saved_lh;
            self.chrome_char_width = saved_cw;
        }
        layout
    }

    /// Install `bar` as the real system `NSMenu` via
    /// [`Self::install_menu_bar`] unless it's unchanged from the last
    /// native-path install — see the `Backend::draw_menu_bar` impl's
    /// "Per-frame idempotence" doc section below for why a per-frame
    /// caller needs this to be idempotent rather than rebuilding the
    /// whole `NSMenu` tree on every call.
    ///
    /// Returns `true` when the `NSMenu` tree was (re)built, `false` when
    /// the call was a no-op because `bar` matched the last install.
    fn install_menu_bar_if_changed(&mut self, bar: &MenuBar) -> bool {
        if self.last_native_menu_bar.as_ref() == Some(bar) {
            return false;
        }
        self.install_menu_bar(bar);
        self.last_native_menu_bar = Some(bar.clone());
        true
    }

    /// The in-window painted menu-bar strip. Reachable as the
    /// `Custom`-style fallback from the `Backend::draw_menu_bar` trait
    /// method below, and directly by tests that want the painted pixels
    /// regardless of `menu_style`.
    fn draw_menu_bar_painted(&mut self, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_menu_bar called outside enter_frame_scope",
        );
        // The menu bar is chrome (`ChromePrimitive::MenuBar`)
        // — see `draw_tree`'s comment for why this reads `chrome_font`
        // instead of `current_font`.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx non-null inside frame scope.
        unsafe {
            super::menu_bar::draw_menu_bar(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                bar,
                &theme,
            )
        }
    }
}

/// The in-window layout of a menu bar AppKit draws natively: zero
/// height and no items, since the real menu bar lives outside the window.
fn native_menu_bar_layout(rect: Rect) -> MenuBarLayout {
    MenuBarLayout {
        bounds: Rect::new(rect.x, rect.y, rect.width, 0.0),
        visible_items: Vec::new(),
        hit_regions: Vec::new(),
    }
}

/// Defensive clamp for [`MacBackend::draw_editor`]: both `editor.rect`
/// and `viewport` are already in points (see [`Viewport`]'s doc), so no
/// scale conversion is needed — this only ever shrinks `editor.rect`'s
/// right/bottom edge back inside `viewport`, never moves its origin or
/// grows it.
///
/// Returns `None` when `editor.rect` already fits (the overwhelmingly
/// common case — no clone), and `None` rather than clamping at all when
/// `viewport` is the all-zero default (`MacBackend::new` before its first
/// `begin_frame`), so a backend used outside the normal
/// `begin_frame` → `enter_frame_scope` → `draw_editor` sequence keeps
/// unclamped behaviour instead of silently painting nothing.
fn clamp_editor_rect_to_viewport(editor: &Editor, viewport: Viewport) -> Option<Editor> {
    if viewport.width <= 0.0 || viewport.height <= 0.0 {
        return None;
    }
    let rect = editor.rect;
    let max_width = (viewport.width - rect.x).max(0.0);
    let max_height = (viewport.height - rect.y).max(0.0);
    if rect.width <= max_width && rect.height <= max_height {
        return None;
    }
    let mut clamped = editor.clone();
    clamped.rect.width = rect.width.min(max_width);
    clamped.rect.height = rect.height.min(max_height);
    Some(clamped)
}

impl Default for MacBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::runtime::PreprocessBackend for MacBackend {
    fn active_text_selection(&self) -> Option<&crate::text_selection::ActiveTextSelection> {
        self.active_text_selection()
    }

    fn set_active_text_selection(&mut self, region: WidgetId, anchor: Point, focus: Point) {
        self.set_active_text_selection(region, anchor, focus)
    }

    fn clear_text_selection(&mut self) {
        self.clear_text_selection()
    }

    fn clear_selection_display(&mut self) {
        self.clear_selection_display()
    }

    fn select_all_text_region(&mut self) -> bool {
        self.select_all_text_region()
    }

    fn selection_text_for_copy(&self) -> String {
        self.extract_selection_text()
    }

    fn focused_activity_bar_id(&self) -> Option<&WidgetId> {
        self.focused_activity_bar_id()
    }

    fn focus_manager_mut(&mut self) -> &mut crate::focus::FocusManager {
        &mut self.focus
    }

    fn match_keypress(&self, key: &Key, modifiers: Modifiers) -> Option<AcceleratorId> {
        self.match_keypress(key, modifiers)
    }

    fn fold_double_click(&mut self, ev: UiEvent) -> UiEvent {
        self.fold_double_click(ev)
    }

    /// macOS pastes on Cmd-V/Cmd-Shift-V — the platform convention. The
    /// copy shortcut stays literal Ctrl-C everywhere (this trait's
    /// `is_copy_keypress` default, which `MacBackend` does not override)
    /// — a deliberate cross-platform choice, not an oversight; see
    /// `macos::run::dispatch_event`'s doc.
    fn paste_modifier(&self) -> crate::desktop::PasteModifier {
        crate::desktop::PasteModifier::Cmd
    }
}

impl crate::backend::sealed::Sealed for MacBackend {}

impl Backend for MacBackend {
    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        // Cleared here (frame start) rather than at `end_frame`, matching
        // `GtkBackend`/`TuiBackend`: zones must survive from the moment
        // `register_zone` is called (during `app.render`, inside
        // `enter_frame_scope`) until whatever reads them after the frame
        // (e.g. `MacDriver::inventory`).
        self.zones.clear();
        // Clear the focused activity bar — re-set by `draw_activity_bar`
        // during this render pass if a bar is still keyboard-focused. Same
        // per-frame lifecycle as `zones`, matching `GtkBackend::begin_frame`.
        self.focused_activity_bar = None;
        // Clear last frame's modal paint marks so this frame has
        // to earn them again (via draw_dialog/draw_palette/draw_context_menu).
        self.modal_stack.borrow_mut().reset_frame_paint();
        // Clear per-frame text regions so stale registrations from the
        // previous frame don't linger. Mirrors `GtkBackend`/`TuiBackend`/
        // `WinBackend`'s identical `begin_frame` clear.
        self.core.begin_frame();
    }

    fn end_frame(&mut self) {
        // No-op flush-wise. AppKit's `drawRect:` flushes when it returns;
        // this method exists for parity with backends that need an
        // explicit flush.
        //
        // In debug builds, warn about any modal that's registered
        // in the ModalStack (and therefore hit-testable) but that this
        // frame's `AppLogic::render` never painted — the "registered but
        // invisible" defect class made detectable instead
        // of silently shipping.
        #[cfg(debug_assertions)]
        for id in self.modal_stack.borrow().unpainted_ids() {
            crate::diagnostics::emit(crate::modal_stack::ModalStack::unpainted_modal_message(&id));
        }
    }

    fn set_theme(&mut self, theme: Theme) {
        self.set_current_theme(theme);
    }

    fn theme(&self) -> Theme {
        self.current_theme
    }

    fn set_style(&mut self, style: crate::Style) {
        self.set_current_style(style);
    }

    fn style(&self) -> crate::Style {
        self.current_style
    }

    fn set_nerd_fonts(&mut self, enabled: bool) {
        self.nerd_fonts_enabled = enabled;
    }

    fn nerd_fonts_enabled(&self) -> bool {
        self.nerd_fonts_enabled
    }

    /// Register `bytes` with Core Text via
    /// [`super::text::register_font_from_memory`] — no
    /// filesystem write, process-lifetime only.
    fn register_font_from_memory(&mut self, bytes: &[u8]) -> Option<Vec<String>> {
        super::text::register_font_from_memory(bytes).map(|name| vec![name])
    }

    /// Answers via [`super::text::make_font_exact`]: Core
    /// Text substitutes Helvetica for an unknown family rather than
    /// failing, so plain `CTFontCreateWithName` success can't mean
    /// "installed" — `make_font_exact` already carries the "resolved
    /// font's own name must equal what was asked" check this needs, the
    /// same one [`Self::set_editor_font`] relies on. The probe size
    /// (12pt) is arbitrary: family existence doesn't depend on it.
    fn has_font_family(&self, family: &str) -> Option<bool> {
        Some(super::text::make_font_exact(family, 12.0).is_some())
    }

    /// Store `family` as the Nerd-Font (or other PUA-codepoint) fallback
    /// and, if [`Self::set_current_font`] already installed a font,
    /// re-apply it immediately via [`super::text::font_with_builtin_fallback`]
    /// — the two setters can land in either order without either effect
    /// being lost.
    ///
    /// Also re-applies to `chrome_font` so chrome icon glyphs have the
    /// same cascade as editor ones.
    fn set_nerd_font_fallback(&mut self, family: &str) {
        self.nerd_font_fallback_family = Some(family.to_string());
        // A cascade list is a full replacement, not an append, so it is
        // rebuilt through `font_with_builtin_fallback` — a list naming
        // `family` alone would drop the bundled codicon entry
        // `set_current_font`/`set_chrome_font` already put on these two
        // fonts.
        if let Some(font) = self.current_font.take() {
            self.current_font = Some(super::text::font_with_builtin_fallback(&font, Some(family)));
        }
        self.chrome_font = super::text::font_with_builtin_fallback(&self.chrome_font, Some(family));
    }

    /// Maps onto the existing [`Self::set_current_font`] machinery —
    /// `current_font` already *is* "the editor font" on this backend;
    /// there was previously just no `Backend`-trait-level way to reach it
    /// by (family, size) the way GTK/Win-GUI expose. If `family` doesn't
    /// resolve to an installed Core Text family, this silently keeps
    /// whatever font was already installed rather than panicking or
    /// clearing it — same "degrade, don't fail" posture
    /// [`super::text::font_with_fallback`]'s doc already documents.
    /// That "doesn't resolve" check goes through
    /// [`super::text::make_font_exact`], not plain `make_font`: Core Text
    /// substitutes Helvetica for an unknown family instead of failing,
    /// so the plain constructor would install Helvetica as the *editor*
    /// font here rather than keeping the caller's previous monospace face.
    fn set_editor_font(&mut self, family: &str, size_pt: f32) {
        // A CSS/Pango generic token (`"monospace"`, …) always resolves
        // to a native system font rather than going through the
        // installed-family check below — there is no "family" to look up,
        // only a system role to build directly.
        let font = match GenericFamily::parse(family) {
            Some(GenericFamily::Monospace) => {
                Some(super::text::system_monospace_font(size_pt as f64))
            }
            // `sans-serif`/`system-ui` aren't monospace — `set_editor_font`'s
            // doc asks for a monospace family — but there is no error
            // channel to reject the mismatch through, so resolve them
            // honestly (the system UI font) rather than silently treating
            // them as an unknown concrete name.
            Some(GenericFamily::SansSerif | GenericFamily::SystemUi) => {
                Some(super::text::system_ui_font(size_pt as f64))
            }
            None => super::text::make_font_exact(family, size_pt as f64),
        };
        if let Some(font) = font {
            self.set_current_font(font);
        }
    }

    /// Chrome twin of [`Self::set_editor_font`] — installs
    /// `font_desc` (Pango-style, e.g. `"Sans 13"`) as `chrome_font` via
    /// [`Self::set_chrome_font`], parsed by [`parse_ui_font_desc`]. Never
    /// touches `current_font`/`current_line_height`/`current_char_width`
    /// — keeping editor and chrome metrics from the same font avoids
    /// the metric-mismatch hazard this separation exists to protect against.
    ///
    /// Degrades to the CoreText system UI font (at the requested size, or
    /// [`DEFAULT_UI_FONT_SIZE_PT`] if `font_desc` doesn't parse at all)
    /// when the named family isn't installed, rather than panicking or
    /// silently keeping a stale chrome font a caller explicitly asked to
    /// change.
    ///
    /// "Isn't installed" is decided by [`super::text::make_font_exact`]
    /// rather than plain `make_font` — `CTFontCreateWithName` answers an
    /// unknown family with Helvetica instead of an error, so the plain
    /// constructor would quietly paint chrome in Helvetica here and never
    /// reach the system-UI-font fallback this doc promises.
    fn set_ui_font(&mut self, font_desc: &str) {
        let font = match parse_ui_font_desc(font_desc) {
            Some((ParsedFamily::Generic(GenericFamily::Monospace), size_pt)) => {
                super::text::system_monospace_font(size_pt)
            }
            Some((
                ParsedFamily::Generic(GenericFamily::SansSerif | GenericFamily::SystemUi),
                size_pt,
            )) => super::text::system_ui_font(size_pt),
            Some((ParsedFamily::Named(family), size_pt)) => {
                super::text::make_font_exact(&family, size_pt)
                    .unwrap_or_else(|| super::text::system_ui_font(size_pt))
            }
            None => super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT),
        };
        self.set_chrome_font(font);
    }

    /// macOS's platform-native convention: `Menlo 12`
    /// for editor content — VS Code's own macOS default, and a
    /// concrete literal rather than a live CoreText query, since
    /// `system_monospace_font`'s `kCTFontUserFixedPitchFontType`
    /// resolves to whatever fixed-pitch face the user's Font Book
    /// preference names, not necessarily the historic coding-font
    /// convention this default represents — and `13pt` UI chrome,
    /// resolved through the same [`super::text::system_ui_font`] query
    /// [`Self::new`] seeds `chrome_font` from, so `ui_family` always
    /// names whatever San Francisco variant Core Text actually
    /// resolves on the running OS version rather than a version-pinned
    /// guess.
    fn default_fonts(&self) -> crate::backend::PlatformFontDefaults {
        crate::backend::PlatformFontDefaults {
            editor_family: "Menlo".to_string(),
            editor_size_pt: 12.0,
            ui_family: super::text::system_ui_font(13.0).family_name(),
            ui_size_pt: 13.0,
        }
    }

    fn poll_events(&mut self) -> Vec<UiEvent> {
        let mut out: Vec<UiEvent> = self.events.borrow_mut().drain(..).collect();
        // Fold in any `UiEvent::User` payloads a background thread queued
        // via `waker()` since the last drain. Unconditional, matching
        // `TuiBackend`/`GtkBackend::poll_events` — `macos::run`'s `paint`
        // closure calls this on every repaint, and `waker()`'s
        // `DispatchQueue::main().exec_async` nudge schedules exactly such
        // a repaint out-of-band the moment a background thread wakes.
        self.user_events.drain_into(&mut out);
        out
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

    /// See [`Backend::waker`]'s doc for the full cross-backend contract.
    /// macOS's implementation, like GTK's, can't rely on a bounded poll
    /// alone: `macos::run`'s `paint` closure only drains
    /// [`Self::poll_events`] when AppKit actually calls `drawRect:`, and
    /// nothing periodic forces that — unlike GTK's 33ms idle timer or
    /// TUI's bounded `wait_events(timeout)`, a `MacBackend` app with no
    /// unrelated user input or timer would never repaint at all, so a
    /// background result would sit in the queue forever with nothing to
    /// surface it. `dispatch2::DispatchQueue::main().exec_async` is GCD's
    /// own thread-safe "run this on the main thread" primitive.
    ///
    /// The dispatched closure can't touch `Rc<RefCell<MacBackend>>`/
    /// `Rc<RefCell<App>>`/the `QuadraView` directly (none are `Send`, and
    /// AppKit's own `NSObject` types are main-thread-only by design in
    /// `objc2` — `waker()` only has `&self` in any case, no handle to the
    /// app or view at all). It instead calls back into
    /// [`Self::wake_callback`], the `Rc<dyn Fn()>` `macos::run::run`
    /// installs via [`Self::set_wake_callback`] that calls
    /// `view.setNeedsDisplay(true)` — the same repaint request
    /// [`crate::runtime::ReactionSink::request_redraw`] issues for every
    /// other event. [`dispatch2::MainThreadBound`] is what makes carrying
    /// that `!Send` closure across the `Send + Sync` boundary `waker()`'s
    /// return type demands sound — see its doc for the argument. macOS
    /// uses `dispatch2`'s own audited wrapper (already a dependency here
    /// for `exec_async`) rather than anything homegrown, precisely because
    /// the hard part is the terminal drop: `MainThreadBound` stores its
    /// value in a `ManuallyDrop` and re-dispatches `T`'s destructor back
    /// to the main thread via `run_on_main`. The other two backends dodge
    /// the question instead of answering it — GTK keeps its `Rc` in a
    /// main-thread thread-local and sends only an integer id, and Windows
    /// needs no wrapper at all because `wndproc` already holds the state
    /// its `PostMessageW` wake dispatches through.
    fn waker(&self) -> Arc<dyn Fn(UserPayload) + Send + Sync> {
        let queue = Arc::clone(&self.user_events);
        let wake_callback = Arc::clone(&self.wake_callback);
        Arc::new(move |payload: UserPayload| {
            queue.push(payload.into_arc());
            let wake_callback = Arc::clone(&wake_callback);
            DispatchQueue::main().exec_async(move || {
                if let Some(bound) = wake_callback.get() {
                    // SAFETY: `exec_async`'s closure is guaranteed by GCD's
                    // own contract to run on the main queue's thread — the
                    // real, OS-level main thread for a process that has
                    // called `NSApplicationMain`/run an `NSApplication`,
                    // which every `macos::run::run` caller has by the time
                    // `set_wake_callback` could have installed anything
                    // here to call.
                    let mtm = unsafe { MainThreadMarker::new_unchecked() };
                    (bound.get(mtm))();
                }
            });
        })
    }

    /// See [`Backend::request_frame_in`]'s doc for the cross-backend
    /// contract. Runs synchronously on the main thread
    /// (called directly from event/tick handling, never from a
    /// background thread), but still needs [`Self::tick_callback`]'s
    /// `MainThreadBound` indirection rather than capturing `view`/`app`
    /// state directly: `DispatchQueue::after`'s closure type is bounded
    /// `Send` regardless of when it actually runs (GCD's Rust binding
    /// can't statically prove "this queue happens to be the main one, so
    /// a `!Send` capture is fine"), the same restriction [`Self::waker`]
    /// works around — this just reaches the *tick* target instead of the
    /// *redraw* one.
    ///
    /// A no-op if [`Self::tick_callback`] hasn't been installed yet (a
    /// `MacBackend` never handed to `macos::run::run`, or a call during
    /// the narrow window before `run` installs it) — the scheduled
    /// closure below simply finds nothing to call, same posture as
    /// `waker`'s own "no callback yet" case.
    fn request_frame_in(&self, delay: Duration) {
        let tick_callback = Arc::clone(&self.tick_callback);
        let Ok(when) = DispatchTime::try_from(delay) else {
            return;
        };
        let _ = DispatchQueue::main().after(when, move || {
            if let Some(bound) = tick_callback.get() {
                // SAFETY: see `waker`'s identical comment above — `after`
                // guarantees this closure runs on the main queue's thread.
                let mtm = unsafe { MainThreadMarker::new_unchecked() };
                (bound.get(mtm))();
            }
        });
    }

    fn invalidate_rect(&mut self, rect: crate::event::Rect) {
        self.core.record_invalidation(rect);
        // `QuadraView` sets `isFlipped` to `true` (see `run.rs`), so its
        // coordinate space is already top-left-origin with y growing
        // downward — the same convention `crate::event::Rect` uses, so no
        // y-flip is needed when building the native rect.
        if let Some(view) = self.window.as_ref().and_then(|w| w.contentView()) {
            let ns_rect = NSRect::new(
                NSPoint::new(rect.x as f64, rect.y as f64),
                NSSize::new(rect.width as f64, rect.height as f64),
            );
            view.setNeedsDisplayInRect(ns_rect);
        }
    }

    fn invalidation_log(&self) -> &crate::backend::InvalidationLog {
        self.core.invalidation_log()
    }

    /// Unlike every other backend, macOS's universal bindings (`Save`,
    /// `Copy`, …) map onto Cmd instead of Ctrl — this override applies
    /// that translation once here, via
    /// [`crate::backend_core::BackendCore::register_accelerator_with`]'s
    /// transform hook, so [`Self::match_keypress`] stays a plain equality
    /// check against a native Cmd keypress.
    fn register_accelerator(&mut self, acc: &Accelerator) {
        let binding = acc.binding.clone();
        self.core.register_accelerator_with(acc, |parsed| {
            macos_universal_binding_modifiers(&binding, parsed)
        });
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.core.unregister_accelerator(id);
    }

    /// Persists via `self.core`, same one-liner delegation as the
    /// accelerator methods above. `Auto` (the default) resolves to
    /// `Native` here since [`Self::backend_caps`] declares
    /// `native_menu: true` — see [`crate::backend::MenuStyle::resolve`].
    fn menu_style(&self) -> crate::backend::MenuStyle {
        self.core.menu_style()
    }

    fn set_menu_style(&mut self, style: crate::backend::MenuStyle) {
        self.core.set_menu_style(style);
    }

    /// This used to `.expect()` the `MainThreadMarker`, which panics
    /// whenever called off the main thread. Every *real* call site
    /// (`macos::shell_runner`) is already on the main thread — but Rust's
    /// test runner hands every `#[test]` fn its own spawned thread, so any
    /// harness driving `ShellApp::setup` through a call to this method
    /// panicked, taking down a `setup()` that has nothing else to do with
    /// menus. The sibling helpers in [`super::menu_bar_install`] already
    /// degrade gracefully instead of panicking; this matches that shape —
    /// a silent no-op off the main thread, same posture as
    /// `request_frame_in`'s "no callback yet" no-op.
    fn install_menu_bar(&mut self, bar: &crate::primitives::menu_bar::MenuBar) {
        let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
            return;
        };
        // Replacing wholesale — the previous target drops when this
        // assignment runs, after the new menu is installed.
        let target = super::menu_bar_install::install_menu_bar(mtm, bar, self.events.clone());
        self.menu_target = Some(target);
    }

    /// See [`Self::install_menu_bar`]'s doc — same off-main-thread
    /// degradation, same rationale.
    fn show_context_menu(
        &mut self,
        menu: &crate::primitives::context_menu::ContextMenu,
        anchor: crate::event::Point,
    ) {
        let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
            return;
        };
        // Blocks on AppKit's modal pop-up loop until the user picks
        // an item or dismisses; pushes `ContextMenuItemActivated` /
        // `ContextMenuDismissed` onto the events queue.
        super::menu_bar_install::show_context_menu(
            mtm,
            menu,
            anchor.x as f64,
            anchor.y as f64,
            self.events.clone(),
        );
    }

    fn modal_stack_handle(&self) -> Rc<RefCell<ModalStack>> {
        self.modal_stack.clone()
    }

    fn drag_state_handle(&self) -> Rc<RefCell<DragState>> {
        self.drag_state.clone()
    }

    fn focus_manager(&self) -> &crate::focus::FocusManager {
        &self.focus
    }

    fn draw_focus_ring(&mut self, rect: Rect) {
        let theme = self.current_theme;
        let stroke_width = self.current_style.focus_ring_width;
        self.surface_stroke_rect(rect, theme.accent_fg, stroke_width);
        self.register_zone(WidgetId::new("chrome:focus-ring"), rect);
    }

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    /// `MacBackend` implements [`PaintSurface`] directly (see this
    /// module's own `impl PaintSurface for MacBackend` block below), so
    /// this always answers `Some(self)` — never the `None` default
    /// `Backend::paint_surface` falls back to.
    fn paint_surface(&mut self) -> Option<&mut dyn PaintSurface> {
        Some(self)
    }

    fn register_zone(&mut self, id: WidgetId, bounds: Rect) {
        self.zones.push(ZoneRec { id, bounds });
    }

    // ─── Text selection ─────────────────────────────────────────────────

    /// Overrides the trait's no-op default — see [`Self::text_regions`]
    /// and `crate::text_selection::TextSelectionState`. Mirrors
    /// `GtkBackend`/`WinBackend`'s identical override.
    fn register_text_region(&mut self, region: TextRegion) {
        self.core.register_text_region(region);
    }

    /// Overrides the trait's no-op default — see
    /// [`Self::cancel_text_selection_drag_impl`].
    fn cancel_text_selection_drag(&mut self) {
        self.cancel_text_selection_drag_impl();
    }

    /// Honest per-method, not aspirational.
    ///
    /// - `mouse` / `scroll` / `drag`: `macos::run`'s view subclass forwards
    ///   `mouseDown:`/`mouseUp:`/`scrollWheel:`/`mouseDragged:` through
    ///   `macos::events`, so all three input kinds reach `poll_events`.
    /// - `native_menu`: `install_menu_bar` / `show_context_menu` are both
    ///   overridden below (`NSMenu`).
    /// - `file_dialogs` / `folder_dialogs` / `notifications`:
    ///   `MacPlatformServices` uses real `NSOpenPanel`/`NSSavePanel`
    ///   and `osascript` notifications (`src/macos/services.rs`), not stubs.
    /// - `native_dialogs`: `MacPlatformServices::show_message_dialog`
    ///   shows a real `NSAlert` and returns the chosen button's id —
    ///   see `src/macos/services.rs`. Like Win-GUI's equivalent
    ///   declaration, `CAP_CONTRACTS`'s `native_dialogs` entry is
    ///   `Unprovable` (every backend implements it), so this declaration
    ///   isn't source-checked by the honesty test; it's true because the
    ///   alert is real.
    /// - `window_chrome`: `begin_window_drag` / `toggle_window_maximize`
    ///   are overridden below, via the shared `crate::desktop::WindowDragArm`
    ///   — `CAP_CONTRACTS`'s `window_chrome` cap only requires *any* of
    ///   the three CSD methods, and these two satisfy it.
    ///   `begin_window_resize` stays the trait's no-op default: the
    ///   vendored `objc2-app-kit` binding has no public "begin native
    ///   edge-resize" primitive (unlike `performWindowDragWithEvent:` /
    ///   `zoom:`), and a manual `setFrame_display`-driven implementation
    ///   would need live mouse-motion wiring through `QuadraView` that
    ///   doesn't exist yet — a real gap, tracked as follow-up rather than
    ///   guessed at blind.
    /// - `pointer_cursor`: `set_cursor` is overridden below, via
    ///   `crate::desktop`'s `PointerShape`/`ResizeEdge` scaffold.
    /// - `text_selection`: `register_text_region`/
    ///   `cancel_text_selection_drag` are both overridden above, backed by
    ///   the shared `crate::text_selection::TextSelectionState` that
    ///   `GtkBackend`/`TuiBackend`/`WinBackend` also embed —
    ///   `macos::run::dispatch_event` wires drag-select and Ctrl-C through
    ///   the same pipeline.
    /// - `app_font_registration`: `register_font_from_memory` /
    ///   `set_nerd_font_fallback` are both overridden above, backed by
    ///   real `CTFontManagerRegisterGraphicsFont` /
    ///   `CTFontCreateCopyWithAttributes` calls in `super::text` rather
    ///   than the trait's no-op default.
    /// - `generic_font_families`: `set_editor_font`/`set_ui_font`
    ///   are both overridden above and resolve CSS/Pango generic family
    ///   tokens to a real CoreText font (`super::text::system_monospace_font`/
    ///   `system_ui_font`) instead of discarding an unresolvable literal
    ///   family name.
    /// - Everything else — `ime` — is **not** declared: no macOS IME
    ///   integration exists yet.
    fn backend_caps(&self) -> crate::backend::BackendCaps {
        crate::backend::BackendCaps {
            mouse: true,
            scroll: true,
            drag: true,
            native_menu: true,
            file_dialogs: true,
            folder_dialogs: true,
            notifications: true,
            window_chrome: true,
            pointer_cursor: true,
            text_selection: true,
            app_font_registration: true,
            native_dialogs: true,
            // `generic_font_families`: `set_editor_font`/
            // `set_ui_font` are both overridden above and now resolve
            // CSS/Pango generic tokens to a real CoreText font
            // (`super::text::system_monospace_font`/`system_ui_font`)
            // instead of an unresolvable literal family name.
            generic_font_families: true,
            // `window` is overridden below and returns
            // `Some` once `set_window` has stashed a real `NSWindow` —
            // see `impl WindowControl for MacBackend`'s doc for the
            // AppKit calls backing each method.
            window_control: true,
            // `tray` is overridden below and always returns
            // `Some` — see `impl TrayService for MacBackend`'s doc for
            // the `NSStatusBar`/`NSStatusItem` calls backing each method.
            tray: true,
            ..crate::backend::BackendCaps::empty()
        }
    }

    // ─── Window chrome (CSD) ──────────────────────────────────────────
    //
    // See `backend_caps`'s doc above for exactly which of the three
    // window-chrome methods are overridden and why.

    fn begin_window_drag(&mut self) -> bool {
        // Unlike `GtkBackend::begin_window_drag`, this does not need to
        // defer past a movement threshold: `NSWindow::performWindowDragWithEvent:`
        // is designed to be called synchronously from `mouseDown:` and
        // disambiguates a drag from a double-click internally (that's
        // the whole point of the API — see Apple's docs), so there is no
        // "swallows the second press" hazard `WindowDragArm::commit_if_past_threshold`
        // exists to avoid on GTK/GDK. Takes the stashed press
        // unconditionally instead.
        let Some(window) = self.window.as_ref() else {
            return false;
        };
        let Some(event) = self.pending_window_press.take() else {
            return false;
        };
        window.performWindowDragWithEvent(&event);
        true
    }

    fn toggle_window_maximize(&mut self) -> bool {
        let Some(window) = self.window.as_ref() else {
            return false;
        };
        // `zoom:` (not `NSWindow::toggleFullScreen:`) is the
        // double-click-to-maximize equivalent: it toggles between the
        // window's current frame and its "zoomed" (ideal) frame, same
        // as clicking the native green titlebar button.
        window.zoom(None);
        true
    }

    /// Sets the cursor via `NSCursor::set()`, not the `push`/`pop`
    /// pairing AppKit's own `cursorUpdate:`/tracking-area handlers
    /// conventionally use. That pairing exists to *restore* whatever
    /// cursor was showing before a transient region was entered; this
    /// backend has no such transient-region concept — `Backend::set_cursor`
    /// is called once per frame with the pointer shape the app wants
    /// showing *now* (mirroring GTK's equally stateless `set_cursor`
    /// override), and there is no "previous" cursor context to pop back
    /// to when that ends. A `push`/`pop` implementation here would leak
    /// stack depth with no matching `pop` call to balance it. If a
    /// future caller needs scoped push/pop semantics (e.g. a transient
    /// hover cursor that must restore the ambient one), that's a new,
    /// deliberately-scoped API on top of this one — not a reason to
    /// replace this plain `.set()`.
    fn set_cursor(&mut self, shape: PointerShape) -> bool {
        if self.window.is_none() {
            return false;
        }
        // `NSCursor::set` is a safe method — called from `AppLogic::handle`
        // (via `Backend::set_cursor`), which only ever runs on the main
        // thread inside the live AppKit run loop.
        mac_cursor_for_shape(shape).set();
        true
    }

    // ─── Window control ─────────────────────────────────────────────────
    fn window(&mut self) -> Option<&mut dyn crate::backend::WindowControl> {
        self.window.as_ref()?;
        Some(self)
    }

    // ─── Tray / status-bar icon ─────────────────────────────────────────
    /// Unlike [`Self::window`], not gated on any prior state — an
    /// `NSStatusItem` is created lazily, on first [`TrayService::set_icon`]
    /// call (see `impl TrayService for MacBackend`'s doc), so there is no
    /// "not constructed yet" case to report `None` for the way an
    /// `NSWindow` genuinely can be absent before `macos::run` builds one.
    fn tray(&mut self) -> Option<&mut dyn crate::backend::TrayService> {
        Some(self)
    }

    /// Reports the region AppKit's native traffic-light controls
    /// (close/minimize/zoom) occupy, in `QuadraView`'s own top-left-origin,
    /// y-down coordinate space — the same space every other `Rect` this
    /// trait hands back (and receives, e.g. `draw_focus_ring`) already
    /// uses, since `QuadraView` sets `isFlipped = YES` (see
    /// `macos::headless`'s module doc for the same convention on the
    /// headless bitmap path).
    ///
    /// Derives the inset from the **union of the Close/Miniaturize/Zoom
    /// button frames**, each converted individually into window
    /// coordinates, rather than from
    /// `standardWindowButton(NSWindowButton::CloseButton)`'s immediate
    /// superview.
    ///
    /// That superview is AppKit's own private container view for the
    /// three-button cluster — but on a *real* (non-headless) window it
    /// is `NSTitlebarView` itself, which spans the window's **entire
    /// width**, not just the button cluster. Reading its `bounds()`
    /// therefore reports a false full-width inset, leaving consumers
    /// (e.g. vimcode's command-center band) with zero width to paint
    /// into. Unioning the three buttons' own frames instead of trusting
    /// their shared ancestor's bounds sidesteps that private-view
    /// assumption entirely — no hardcoded ~78pt HIG spacing, still
    /// correct if accessibility settings (`Increase contrast`, larger
    /// click targets) resize the cluster, since it's still reading
    /// AppKit's own per-button layout.
    ///
    /// Returns `Rect::default()` — same as the trait default — when no
    /// window is set yet (`self.window` stays `None` until
    /// `macos::run::run_with` calls [`Self::set_window`]) or AppKit
    /// hands back none of the three buttons, e.g. a borderless style
    /// mask with no `Titled` bit. Never called before
    /// `client_side_titlebar` was set at window-creation time in
    /// practice, but safe to call any time regardless.
    fn titlebar_control_inset(&self) -> Rect {
        let Some(window) = self.window.as_ref() else {
            return Rect::default();
        };
        // Miniaturize/Zoom can be individually absent (e.g. a style mask
        // that omits `Miniaturizable`/`Resizable`), so each is optional;
        // only a total absence of all three (no `Titled` style mask at
        // all) falls through to `Rect::default()` below.
        let button_kinds = [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ];
        let mut union_rect: Option<NSRect> = None;
        for kind in button_kinds {
            let Some(button) = window.standardWindowButton(kind) else {
                continue;
            };
            // In native full screen (or mid-zoom-transition),
            // AppKit hides the traffic-light cluster (or its private
            // titlebar ancestor) rather than removing it — the button
            // objects (and their stale `frame()`s) still exist. Treat a
            // hidden button as absent, same as a missing one, so the
            // union — and therefore the reported inset — collapses to
            // `Rect::default()` while the buttons aren't actually on
            // screen, instead of reporting a phantom cluster-sized rect
            // a consumer then leaves unpainted (the "grey rectangle").
            if button.isHiddenOrHasHiddenAncestor() {
                continue;
            }
            // SAFETY: `superview` is safe to call on any live `NSView` on
            // the main thread — a currently-installed standard window
            // button always has one.
            let Some(superview) = (unsafe { button.superview() }) else {
                continue;
            };
            // `frame()` is already in `superview`'s coordinate space;
            // `convertRect_toView(_, None)` reaches the window's base
            // coordinate system (bottom-left origin) from there,
            // regardless of how deep this button sits in AppKit's
            // private titlebar view hierarchy.
            let in_window = superview.convertRect_toView(button.frame(), None);
            union_rect = Some(match union_rect {
                Some(acc) => union_ns_rect(acc, in_window),
                None => in_window,
            });
        }
        let Some(in_window) = union_rect else {
            return Rect::default();
        };
        let content_height = window
            .contentView()
            .map(|view| view.frame().size.height)
            .unwrap_or_else(|| window.frame().size.height);
        // Flip AppKit's bottom-left-origin button frame into
        // `QuadraView`'s top-left-origin, y-down space.
        let bottom_from_top = (content_height - in_window.origin.y).max(0.0);
        Rect::new(
            0.0,
            0.0,
            (in_window.origin.x + in_window.size.width) as f32,
            bottom_from_top as f32,
        )
    }

    fn line_height(&self) -> f32 {
        self.current_line_height as f32
    }

    fn chrome_line_height(&self) -> f32 {
        self.chrome_line_height as f32
    }

    fn char_width(&self) -> f32 {
        self.current_char_width as f32
    }

    /// Forwards to [`Self::set_current_line_height`] — the
    /// `Backend`-trait-object-reachable twin of that inherent method, the
    /// macOS counterpart of `GtkBackend`'s / `WinBackend`'s overrides of
    /// the same trait method.
    fn set_current_line_height(&mut self, line_height: f32) {
        MacBackend::set_current_line_height(self, line_height as f64);
    }

    /// Forwards to [`Self::set_current_char_width`] — see
    /// [`Backend::set_current_line_height`]'s doc.
    fn set_current_char_width(&mut self, char_width: f32) {
        MacBackend::set_current_char_width(self, char_width as f64);
    }

    /// `chrome_char_width`, not `current_char_width` — [`Self::draw_list`]
    /// paints row text with `chrome_font`, and
    /// `chrome_char_width` is that font's real advance, kept in sync by
    /// [`Self::set_chrome_font`] whenever [`Backend::set_ui_font`]
    /// changes it. This is exactly the metric-mismatch hazard `chrome_font`
    /// / `current_font`'s separate cached widths exist to avoid —
    /// see `chrome_font`'s field doc.
    fn list_char_width(&self) -> f32 {
        self.chrome_char_width as f32
    }

    /// Real Core Text measurement via [`super::text::measure_text`]
    /// against `chrome_font`/`current_font` — the same per-role `CTFont`
    /// resolution [`Self::surface_draw_text_run_with_role`] uses,
    /// including its "falls back to `chrome_font`, never panics" behaviour
    /// for [`crate::FontRole::Editor`] before [`Self::set_current_font`]
    /// has been called.
    fn measure_text(&self, text: &str, role: crate::FontRole) -> (f32, f32) {
        let font = match role {
            crate::FontRole::Chrome => &self.chrome_font,
            crate::FontRole::Editor => self.current_font.as_ref().unwrap_or(&self.chrome_font),
        };
        let (w, h) = super::text::measure_text(font, text);
        (w as f32, h as f32)
    }

    // ── Drawing ────────────────────────────────────────────────────

    fn draw_tree(&mut self, rect: Rect, tree: &TreeView) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tree called outside enter_frame_scope",
        );
        // Row labels/badges/chevrons are chrome (`ChromePrimitive::Tree`),
        // so row spacing uses the chrome metric — same reason as the MSV
        // fix in 7dfbad5e.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.chrome_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::tree::draw_tree(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                tree,
                &theme,
                line_height,
                self.nerd_fonts_enabled,
            );
        }
    }
    fn draw_list(&mut self, rect: Rect, list: &ListView) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_list called outside enter_frame_scope",
        );
        // The list is chrome (`ChromePrimitive::List`) — paint through
        // `chrome_font` at `chrome_line_height`, matching the compose
        // layer's layout and `list_layout` hit regions.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.chrome_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::list::draw_list(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                list,
                &theme,
                line_height,
                self.nerd_fonts_enabled,
            );
        }
    }
    fn draw_data_table(
        &mut self,
        rect: Rect,
        table: &DataTable,
        hovered_idx: Option<usize>,
    ) -> DataTableLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_data_table called outside enter_frame_scope",
        );
        // `DataTable` is chrome, not editor content — paint through
        // `chrome_font`, matching `draw_tree`'s comment above.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::data_table::draw_data_table(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                table,
                &theme,
                line_height,
                hovered_idx,
            )
        }
    }
    fn data_table_layout(&self, rect: Rect, table: &DataTable) -> DataTableLayout {
        // No-paint twin of `draw_data_table` — must agree with what
        // that method painted.
        let font = &self.chrome_font;
        super::data_table::mac_data_table_layout(
            table,
            font,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
        )
    }
    fn list_hscrollbar(&self, rect: Rect, list: &ListView) -> Option<crate::Scrollbar> {
        // `ListView::h_scroll` and `max_content_width` are in character columns,
        // but macOS works in pixels.  Convert with `current_char_width` so the
        // returned `Scrollbar` track/thumb are in pixel units — matching what
        // `macos::draw_list` paints and what mouse-event coords use.
        let char_w = self.current_char_width as f32;
        let max_w_chars = list.max_content_width? as f32;
        let content_px = max_w_chars * char_w;
        let border_inset = if list.bordered { char_w } else { 0.0 };
        let visible_px = (rect.width - 2.0 * border_inset).max(0.0);
        if content_px <= visible_px {
            return None;
        }
        let row_h = self.line_height();
        let track = crate::primitives::list::hscrollbar_track(rect, list.bordered, char_w, row_h);
        Some(crate::Scrollbar::horizontal(
            list.id,
            track,
            list.h_scroll as f32 * char_w,
            content_px,
            visible_px,
            row_h,
        ))
    }
    fn list_vscrollbar(&self, rect: Rect, list: &ListView) -> Option<crate::Scrollbar> {
        // macOS ListView vertical-scrollbar rasteriser not yet implemented.
        // Delegate to the primitive's geometry method using pixel units:
        // each "row" is one line_height tall (the primitive column-width
        // parameter is reused as row_height here, same as for TUI).
        let row_h = self.chrome_line_height();
        list.vscrollbar(
            crate::event::Rect::new(rect.x, rect.y, rect.width, rect.height),
            row_h,
        )
    }
    fn list_layout(&self, rect: Rect, list: &ListView) -> crate::ListViewLayout {
        // Lists are sidebar chrome — use the chrome metric so
        // hit regions match `draw_list`'s painted row positions.
        super::list::mac_list_layout(
            list,
            rect.width as f64,
            rect.height as f64,
            self.chrome_line_height,
            self.chrome_char_width,
        )
    }
    /// Shared field-kind painting lives in
    /// [`crate::primitives::form::paint`] now — see that fn's doc for why
    /// `FieldKind::Toolbar` is the one variant painted here instead,
    /// after `paint` releases its exclusive borrow of `self`.
    fn draw_form(&mut self, rect: Rect, form: &Form) {
        debug_assert!(
            !self.current_cg().is_null(),
            "MacBackend::draw_form called outside enter_frame_scope",
        );
        // `Form` is chrome, not editor content — see `draw_data_table`'s
        // comment. `crate::primitives::form::paint` below paints through
        // `self` as `&mut dyn PaintSurface`, which defaults to
        // `chrome_font` too (see `MacBackend`'s own `PaintSurface` impl),
        // so this `font` variable — used only for layout/measurement
        // here — has to agree with it.
        let font = self.chrome_font.clone();
        let theme = self.current_theme;
        let flayout = super::form::mac_form_layout(form, rect, self.current_line_height, &font);
        let origin = Point::new(rect.x, rect.y);
        crate::primitives::form::paint(form, &flayout, self, &theme, origin);

        for vf in &flayout.visible_fields {
            let Some(field) = form.fields.get(vf.field_idx) else {
                continue;
            };
            let FieldKind::Toolbar(toolbar) = &field.kind else {
                continue;
            };
            let label_text: String = field.label.spans.iter().map(|s| s.text.as_str()).collect();
            let no_label = label_text.is_empty();
            let (label_w, _) = super::text::measure_text(&font, &label_text);
            let row_x = (origin.x + vf.bounds.x) as f64;
            let row_y = (origin.y + vf.bounds.y) as f64;
            let row_w = vf.bounds.width as f64;
            let row_h = vf.bounds.height as f64;
            let toolbar_x = if no_label {
                row_x + 6.0
            } else {
                row_x + 6.0 + label_w + 12.0
            };
            let toolbar_w = row_x + row_w - toolbar_x;
            if toolbar_w > 0.0 {
                let ctx = self.current_cg();
                debug_assert!(
                    !ctx.is_null(),
                    "MacBackend::draw_form called outside enter_frame_scope",
                );
                // SAFETY: ctx is non-null inside the frame scope.
                unsafe {
                    super::toolbar::draw_toolbar(
                        ctx, &font, toolbar_x, row_y, toolbar_w, row_h, toolbar, &theme, None, None,
                    );
                }
            }
        }
    }
    fn draw_palette(&mut self, rect: Rect, palette: &Palette) {
        // See `modal_stack.rs`'s "Paint-consistency detection" docs —
        // records that this palette's surface was actually painted this frame.
        self.modal_stack.borrow_mut().mark_painted(&palette.id);
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_palette called outside enter_frame_scope",
        );
        // `Palette` is chrome, not editor content — see
        // `draw_data_table`'s comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let style = self.style();
        let line_height = self.current_line_height;
        if rect.width < 20.0 || rect.height < line_height as f32 * 4.0 {
            return;
        }
        let (palette_layout, rows_h) = crate::primitives::palette::native_surface_paint::layout(
            rect.width,
            rect.height,
            palette,
            line_height as f32,
        );
        // Calls the shared paint directly (rather than through
        // `super::palette::draw_palette`, whose public signature stays
        // additive-only and so cannot take a live `&Style`) so this live
        // backend path honours `Self::set_style` overrides.
        let mut surface = super::surface::CgSurface {
            ctx,
            font: Some(font),
        };
        crate::primitives::palette::native_surface_paint::paint(
            palette,
            rect,
            &palette_layout,
            rows_h,
            line_height as f32,
            self.nerd_fonts_enabled,
            &mut surface,
            &theme,
            &style,
        );
    }

    fn palette_layout(&self, rect: Rect, palette: &Palette) -> crate::PaletteLayout {
        super::palette::mac_palette_layout(
            palette,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
        )
    }

    fn draw_settings_chrome(
        &mut self,
        rect: Rect,
        header_text: &str,
        query: &str,
        placeholder: &str,
        active: bool,
    ) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_settings_chrome called outside enter_frame_scope",
        );
        // The settings header/search bar is chrome (part of `Form`),
        // not editor content — see `draw_data_table`'s comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::form::draw_settings_chrome(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                line_height,
                header_text,
                query,
                placeholder,
                active,
                &theme,
            );
        }
    }

    fn draw_status_bar_interactive(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        interaction: &crate::interaction::InteractionState,
    ) -> StatusBarLayout {
        self.status_bar_paint_scaled(rect, bar, interaction, 1.0)
    }

    /// Same paint as [`Self::draw_status_bar_interactive`], scaling only
    /// this call's chrome font size — see
    /// [`crate::Backend::draw_status_bar_interactive_scaled`]'s doc for
    /// the full rationale. Both this and the unscaled method above funnel
    /// through [`Self::status_bar_paint_scaled`] so a fix to one applies
    /// to both; `1.0` there is exactly the unscaled call with no scaling.
    fn draw_status_bar_interactive_scaled(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        interaction: &crate::interaction::InteractionState,
        font_scale: f32,
    ) -> StatusBarLayout {
        self.status_bar_paint_scaled(rect, bar, interaction, font_scale)
    }
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        // Icon-less spelling of `draw_tab_bar_icons` — an empty sidecar
        // reproduces this bar pixel for pixel, so the two share
        // one implementation rather than two paint paths that can drift.
        self.draw_tab_bar_icons(rect, bar, &[], hovered_close_tab)
    }
    /// Per-tab icon glyphs, implemented on macOS via [`super::tab_bar::mac_tab_icon_extras`]
    /// which measures each glyph with CoreText and every geometry consumer
    /// reserves that width, so a decorated tab's label and close-button hit
    /// box shift together and a click on the painted × still closes the tab.
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn draw_tab_bar_icons(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tab_bar called outside enter_frame_scope",
        );
        // Chrome primitives are drawn through `chrome_font` — see
        // `draw_tree`'s comment. This method covers both `draw_tab_bar`
        // and `draw_tab_bar_with_chrome`: both delegate here with
        // an empty icon sidecar. The centring math below must use
        // `chrome_line_height`, not `current_line_height` (the editor
        // metric) — same hazard `Self::draw_text_input`'s doc documents —
        // or the label centres against a line height that doesn't
        // describe the font actually painting it, leaving the row taller
        // than the label needs and the label off-centre within it.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.chrome_line_height;
        // SAFETY: `ctx` is non-null inside the frame scope.
        // `super::tab_bar::draw_tab_bar_icons` honours `rect.y`
        // via its own `y_offset` parameter, but is never handed `rect.x`
        // and always paints/measures from `x = 0`. The
        // `CGContextTranslateCTM` X-only shift places the ink at
        // `rect.x` (mirroring `Self::draw_activity_bar`'s CTM treatment);
        // `y_offset` stays `rect.y` as before. `shift_tab_bar_hits`
        // afterward makes the returned `TabBarHits` honour
        // `Backend::tab_bar_layout`'s documented absolute-x contract.
        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, rect.x as f64, 0.0);
            let mut hits = super::tab_bar::draw_tab_bar_icons(
                ctx,
                font,
                rect.width as f64,
                line_height,
                rect.y as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_close_tab,
                icons,
            );
            CGContextRestoreGState(ctx);
            crate::backend::shift_tab_bar_hits(&mut hits, rect.x as f64);
            hits
        }
    }
    /// `TabBarLayout`-returning counterpart to
    /// [`Self::draw_tab_bar`]. Paints exactly as that method does
    /// (discarding the deprecated `TabBarHits` it returns), then
    /// separately resolves the real `TabBarLayout` via
    /// [`super::tab_bar::mac_tab_bar_native_layout_icons`] — see that
    /// function's doc for why macOS can't share one measurement path
    /// between its `TabBarHits` and `TabBarLayout` accessors the way
    /// TUI/GTK/Win do.
    fn draw_tab_bar_layout(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> TabBarLayout {
        // Icon-less spelling of `draw_tab_bar_icons_layout` — see
        // `draw_tab_bar` above for why the empty sidecar shares one path.
        self.draw_tab_bar_icons_layout(rect, bar, &[], hovered_close_tab)
    }
    /// `TabBarLayout`-returning counterpart to
    /// [`Self::draw_tab_bar_icons`] — same CoreText icon-width pass
    /// applied to both the paint and the separately-computed
    /// [`super::tab_bar::mac_tab_bar_native_layout_icons`] so the
    /// returned layout describes the pixels this call just painted.
    fn draw_tab_bar_icons_layout(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tab_bar_layout called outside enter_frame_scope",
        );
        // Chrome primitives are drawn through `chrome_font` — see
        // `draw_tab_bar_icons`' comment, including why centring uses
        // `chrome_line_height` rather than the editor's.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.chrome_line_height;
        // SAFETY: `ctx` is non-null inside the frame scope. Same X-only
        // CTM translate as `Self::draw_tab_bar_icons` above — this call
        // discards the painted `TabBarHits` anyway, so there is no return
        // value left to shift; only the ink needs to land at `rect.x`.
        #[allow(deprecated)] // discarded `TabBarHits` — issue #823
        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, rect.x as f64, 0.0);
            let _ = super::tab_bar::draw_tab_bar_icons(
                ctx,
                font,
                rect.width as f64,
                line_height,
                rect.y as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_close_tab,
                icons,
            );
            CGContextRestoreGState(ctx);
        }
        super::tab_bar::mac_tab_bar_native_layout_icons(
            font,
            rect.width as f64,
            rect.height as f64,
            bar,
            icons,
        )
    }
    fn draw_activity_bar(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<ActivityBarRowHit> {
        // Track keyboard focus so `run::dispatch_event` can redirect the
        // next `KeyPressed` into this bar — same contract
        // `GtkBackend::draw_activity_bar` implements.
        if bar.is_keyboard_focused {
            self.focused_activity_bar = Some(bar.id);
        }
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_activity_bar called outside enter_frame_scope",
        );
        // Chrome primitives are drawn through `chrome_font` —
        // `draw_tree`'s comment explains the swap. This is the mandatory,
        // non-default method real hosts reach. It painted its icon glyph
        // through `current_font` (the editor font) until fixed to use
        // `chrome_font`, matching every other chrome primitive's swap.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx non-null inside frame scope. `super::activity_bar`'s
        // rasteriser is bar-relative by contract — it always
        // paints into `(0, 0, width, height)` and returns bar-relative hit
        // spans — so the CTM translate below is what actually places the
        // ink at `rect`'s absolute origin, matching
        // `GtkBackend::draw_activity_bar`'s `cr.translate(rect.x, rect.y)`.
        // The returned `ActivityBarRowHit`s are untouched by the
        // translate — they stay bar-relative, per contract.
        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, rect.x as f64, rect.y as f64);
            let hits = super::activity_bar::draw_activity_bar(
                ctx,
                font,
                rect.width as f64,
                rect.height as f64,
                bar,
                &theme,
                hovered_idx,
                self.nerd_fonts_enabled,
            );
            CGContextRestoreGState(ctx);
            hits
        }
    }

    fn draw_activity_bar_with_style(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
        style: &crate::ActivityBarStyle,
    ) -> Vec<ActivityBarRowHit> {
        // Track keyboard focus — same contract as `draw_activity_bar` above.
        if bar.is_keyboard_focused {
            self.focused_activity_bar = Some(bar.id);
        }
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_activity_bar_with_style called outside enter_frame_scope",
        );
        // Chrome primitives are drawn through `chrome_font` —
        // `draw_tree`'s comment explains the swap. This method also paints
        // its icon glyph through `chrome_font`, matching `draw_activity_bar`
        // above and every other chrome primitive.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx non-null inside frame scope. See `draw_activity_bar`
        // above for why the CTM translate is what makes this bar-relative
        // rasteriser paint at `rect`'s absolute origin.
        unsafe {
            CGContextSaveGState(ctx);
            CGContextTranslateCTM(ctx, rect.x as f64, rect.y as f64);
            let hits = super::activity_bar::draw_activity_bar_with_style(
                ctx,
                font,
                rect.width as f64,
                rect.height as f64,
                bar,
                style,
                &theme,
                hovered_idx,
                self.nerd_fonts_enabled,
            );
            CGContextRestoreGState(ctx);
            hits
        }
    }

    fn status_bar_layout(&self, rect: Rect, bar: &StatusBar) -> StatusBarLayout {
        // No-paint twin of `draw_status_bar`: same `mac_status_bar_layout`
        // call, so hit regions match the painted frame exactly. Hit
        // regions are bar-local, so `rect.x` / `rect.y` are deliberately
        // not folded in. Measures against `chrome_font`/`chrome_line_height`,
        // matching `draw_status_bar_interactive`'s resolution.
        super::status_bar::mac_status_bar_layout(
            &self.chrome_font,
            rect.width as f64,
            self.chrome_line_height,
            bar,
        )
    }

    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarHits {
        // No-paint twin of `draw_tab_bar`, routed through the same
        // `mac_tab_bar_layout_icons` + `shift_tab_bar_hits` pair
        // `tab_bar_layout_icons` below uses, so both agree on the
        // documented absolute contract.
        self.tab_bar_layout_icons(rect, bar, &[])
    }

    /// No-paint twin of [`Self::draw_tab_bar_icons`], routed through the
    /// same [`super::tab_bar::mac_tab_bar_layout_icons`] with the same
    /// sidecar — so the load-bearing macOS invariant (`tab_bar_layout*`
    /// returns exactly what `draw_tab_bar*` painted) holds for decorated
    /// tabs too, not just icon-less ones.
    #[allow(deprecated)] // returns the deprecated `TabBarHits` — issue #823
    fn tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
    ) -> TabBarHits {
        // No-paint twin of `draw_tab_bar_icons` — must agree with what
        // that method painted, so it measures against `chrome_font` too.
        let mut hits = super::tab_bar::mac_tab_bar_layout_icons(
            &self.chrome_font,
            rect.width as f64,
            bar,
            icons,
        );
        // Bar-relative → target-surface-absolute, the same
        // shift `Self::draw_tab_bar_icons` applies to its own returned
        // hits after painting, so a caller that measures here and paints
        // there sees one agreed geometry.
        crate::backend::shift_tab_bar_hits(&mut hits, rect.x as f64);
        hits
    }

    /// `TabBarLayout`-returning counterpart to
    /// [`Self::tab_bar_layout`], routed through
    /// [`super::tab_bar::mac_tab_bar_native_layout_icons`] — see that
    /// function's doc for why macOS can't share one measurement path
    /// between its `TabBarHits` and `TabBarLayout` accessors.
    fn resolve_tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarLayout {
        self.resolve_tab_bar_layout_icons(rect, bar, &[])
    }

    /// No-paint twin of [`Self::draw_tab_bar_icons_layout`], routed
    /// through [`super::tab_bar::mac_tab_bar_native_layout_icons`] with
    /// the same sidecar — so a consumer that measures with this
    /// and paints with `draw_tab_bar_icons` gets one agreed geometry.
    fn resolve_tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<crate::TabIcon>],
    ) -> TabBarLayout {
        // No-paint twin of `draw_tab_bar_icons_layout` — must agree with
        // what that method painted, so it measures against `chrome_font` too.
        super::tab_bar::mac_tab_bar_native_layout_icons(
            &self.chrome_font,
            rect.width as f64,
            rect.height as f64,
            bar,
            icons,
        )
    }

    fn activity_bar_layout(&self, rect: Rect, bar: &ActivityBar) -> Vec<ActivityBarRowHit> {
        // No-paint twin of `draw_activity_bar`; both walk the same
        // activity-bar layout, so the returned bar-relative spans
        // are exactly the rows that were painted.
        super::activity_bar::mac_activity_bar_layout(rect.width as f64, rect.height as f64, bar)
    }

    /// Shared cell-grid painting lives in
    /// [`crate::primitives::terminal::paint`] now — see that fn's doc
    /// for the divergences resolved while unifying
    /// `gtk::terminal::draw_terminal_cells`,
    /// `macos::terminal::draw_terminal_cells` and
    /// `win::terminal::draw_terminal_cells` into one implementation.
    fn draw_terminal(&mut self, rect: Rect, term: &Terminal) {
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;

        let sb_width = match &term.scrollbar {
            Some(sb) => sb.width.map(|w| w as f64).unwrap_or(8.0),
            None => 0.0,
        };
        let cell_area_w = (rect.width as f64 - sb_width).max(0.0);

        // The terminal is editor-class — paint through `EditorSurface`
        // so its cell glyphs paint in `current_font` even though the
        // plain `self` surface defaults to `chrome_font` (see
        // `EditorSurface`'s doc). Scoped to a block so `self` is free
        // again for the scrollbar paint below, which has no text of its
        // own and is fine through the plain (chrome) surface.
        {
            let mut surface = EditorSurface { backend: self };
            crate::primitives::terminal::paint(
                term,
                &mut surface,
                &theme,
                rect.x,
                rect.y,
                cell_area_w as f32,
                rect.height,
                line_height as f32,
                char_width as f32,
                None,
            );
        }

        if let Some(ref sb_state) = term.scrollbar {
            let sb = crate::primitives::scrollbar::Scrollbar::vertical(
                term.id,
                Rect::new(
                    rect.x + cell_area_w as f32,
                    rect.y,
                    sb_width as f32,
                    rect.height,
                ),
                sb_state.effective_scroll_offset() as f32,
                sb_state.total_lines as f32,
                sb_state.visible_lines as f32,
                line_height as f32,
            );
            crate::primitives::scrollbar::native_surface_paint::paint(&sb, self, &theme);
        }
    }
    /// Shared divider painting lives in
    /// [`crate::primitives::terminal::paint_divider`] now.
    fn draw_terminal_divider(&mut self, rect: Rect) {
        let theme = self.current_theme;
        crate::primitives::terminal::paint_divider(self, rect.x, rect.y, rect.height, &theme);
    }
    /// See `Backend::draw_solid_fill`'s doc — a pixel backend can
    /// honor `rect` exactly via `PaintSurface::surface_fill_rect`.
    fn draw_solid_fill(&mut self, rect: Rect, color: Color) {
        self.surface_fill_rect(rect, color);
        // Chrome-only paint (no text of its own) — see
        // `TuiBackend::draw_solid_fill`'s identical registration for why.
        self.register_zone(WidgetId::new("chrome:solid-fill"), rect);
    }
    /// Shared text-display painting lives in
    /// [`crate::primitives::text_display::paint`] now — see that fn's
    /// doc for the divergence resolved while unifying
    /// `gtk::text_display::draw_text_display`,
    /// `macos::text_display::draw_text_display` and
    /// `win::text_display::draw_text_display` into one implementation.
    fn draw_text_display(&mut self, rect: Rect, td: &TextDisplay) {
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        let char_width = self.current_char_width as f32;
        // `TextDisplay` is editor-class — see `draw_terminal`'s comment
        // / `EditorSurface`'s doc.
        let mut surface = EditorSurface { backend: self };
        crate::primitives::text_display::paint(
            td,
            rect,
            &mut surface,
            &theme,
            line_height,
            char_width,
        );
    }
    fn draw_command_line(&mut self, rect: Rect, cmd: &CommandLine) {
        self.draw_command_line_selection(rect, cmd, None);
    }
    /// Real selection-highlight paint via the shared
    /// `primitives::command_line::native_surface_paint::paint`.
    fn draw_command_line_selection(
        &mut self,
        rect: Rect,
        cmd: &CommandLine,
        selection: Option<(usize, usize)>,
    ) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_command_line_selection called outside enter_frame_scope",
        );
        let font = self.current_font.as_ref().unwrap_or(&self.chrome_font);
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width as f32;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::command_line::draw_command_line_selection(
                ctx,
                font,
                cmd,
                &theme,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                line_height,
                char_width,
                selection,
            );
        }
    }
    fn command_line_layout(
        &self,
        rect: Rect,
        cmd: &CommandLine,
    ) -> crate::primitives::command_line::CommandLineLayout {
        cmd.layout(
            rect,
            crate::primitives::command_line::CommandLineMeasure::from_metrics(&self.measure()),
        )
    }
    fn text_display_layout(&self, rect: Rect, td: &TextDisplay) -> TextDisplayLayout {
        super::text_display::mac_text_display_layout(
            td,
            rect,
            self.current_line_height,
            self.current_char_width,
        )
    }
    /// Was a layout-only stub — no background, text, or cursor
    /// ever painted. Now builds the layout via
    /// [`super::text_input::mac_text_input_layout`] and hands it to the
    /// shared [`crate::primitives::text_input::paint`] through this
    /// backend's own [`PaintSurface`] impl below, mirroring
    /// `draw_text_display`'s / `draw_form`'s shape.
    fn draw_text_input(
        &mut self,
        rect: Rect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        debug_assert!(
            !self.current_cg().is_null(),
            "MacBackend::draw_text_input called outside enter_frame_scope",
        );
        // `TextInput` is chrome: it paints through plain `self`, which
        // resolves `chrome_font` for every glyph (see
        // `Self::surface_measure_text`'s doc). The grid this layout
        // builds has to use that same font's advance — `chrome_char_width`
        // / `chrome_line_height`, not `current_char_width` /
        // `current_line_height` — or the caret and click-to-column grid
        // disagree with the glyphs they sit between. Same hazard
        // `Self::list_char_width`'s doc documents for `draw_list`.
        let theme = self.current_theme;
        let line_height = self.chrome_line_height as f32;
        let char_width = self.chrome_char_width as f32;
        let layout = super::text_input::mac_text_input_layout(ti, rect, line_height, char_width);
        crate::primitives::text_input::paint(ti, &layout, self, &theme);
        layout
    }
    fn text_input_layout(
        &self,
        rect: Rect,
        ti: &crate::primitives::text_input::TextInput,
    ) -> crate::primitives::text_input::TextInputLayout {
        // No-paint twin of `draw_text_input` — must agree with what that
        // method painted, so it resolves the same chrome metrics.
        super::text_input::mac_text_input_layout(
            ti,
            rect,
            self.chrome_line_height as f32,
            self.chrome_char_width as f32,
        )
    }
    fn draw_tooltip(&mut self, tooltip: &Tooltip, layout: &TooltipLayout) {
        self.draw_tooltip_with_chrome(tooltip, layout, &crate::TooltipChrome::default());
    }

    fn draw_tooltip_with_chrome(
        &mut self,
        tooltip: &Tooltip,
        layout: &TooltipLayout,
        chrome: &crate::TooltipChrome,
    ) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_tooltip called outside enter_frame_scope",
        );
        // `Tooltip` is chrome, not editor content — see
        // `draw_data_table`'s comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let style = self.style();
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        // Calls the shared paint directly (rather than through
        // `super::tooltip::draw_tooltip_with_chrome`, whose public
        // signature stays additive-only and so cannot take a live
        // `&Style`) so this live backend path honours `Self::set_style`
        // overrides — see that free function's own doc.
        let mut surface = super::surface::CgSurface {
            ctx,
            font: Some(font),
        };
        crate::primitives::tooltip::native_surface_paint::paint(
            tooltip,
            layout,
            chrome,
            line_height as f32,
            char_width as f32,
            &mut surface,
            &theme,
            &style,
        );
    }
    fn draw_context_menu(
        &mut self,
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
    ) -> Vec<(Rect, WidgetId)> {
        // See draw_palette for why this happens before the CG borrow.
        self.modal_stack.borrow_mut().mark_painted(&menu.id);
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_context_menu called outside enter_frame_scope",
        );
        // Chrome primitives are drawn through `chrome_font` —
        // `draw_tree`'s comment explains the swap.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let style = self.style();
        // Calls the shared paint directly (rather than through
        // `super::context_menu::draw_context_menu`, whose public
        // signature stays additive-only and so cannot take a live
        // `&Style`) so this live backend path honours `Self::set_style`
        // overrides.
        let mut surface = super::surface::CgSurface {
            ctx,
            font: Some(font),
        };
        crate::primitives::context_menu::native_surface_paint::paint(
            menu,
            layout,
            crate::accelerator::Platform::Macos,
            &mut surface,
            &theme,
            &style,
        )
    }
    fn draw_dialog(&mut self, dialog: &Dialog, layout: &DialogLayout) -> Vec<Rect> {
        // See draw_palette for why this happens before the CG borrow.
        self.modal_stack.borrow_mut().mark_painted(&dialog.id);
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_dialog called outside enter_frame_scope",
        );
        // Chrome primitives are drawn through `chrome_font` —
        // `draw_tree`'s comment explains the swap and why `line_height`
        // stays the editor pitch.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let style = self.style();
        let line_height = self.current_line_height;
        // Calls the shared paint directly (rather than through
        // `super::dialog::draw_dialog`, whose public signature stays
        // additive-only and so cannot take a live `&Style`) so this live
        // backend path honours `Self::set_style` overrides — the
        // toolbar-sub-paint step below mirrors that free function's own
        // body exactly.
        let mut surface = super::surface::CgSurface {
            ctx,
            font: Some(font),
        };
        let button_rects = crate::primitives::dialog::native_surface_paint::paint(
            dialog,
            layout,
            line_height as f32,
            &mut surface,
            &theme,
            &style,
        );
        if let (Some(input_b), Some(DialogInput::Toolbar(toolbar))) =
            (layout.input_bounds, dialog.input.as_ref())
        {
            // SAFETY: ctx is non-null inside the frame scope.
            unsafe {
                super::toolbar::draw_toolbar(
                    ctx,
                    font,
                    input_b.x as f64,
                    input_b.y as f64,
                    input_b.width as f64,
                    input_b.height as f64,
                    toolbar,
                    &theme,
                    None,
                    None,
                );
            }
        }
        button_rects
    }
    fn draw_float(
        &mut self,
        float: &crate::primitives::float::Float,
        layout: &crate::primitives::float::FloatLayout,
    ) {
        // See draw_palette for why this happens before the CG borrow.
        self.modal_stack.borrow_mut().mark_painted(&float.id);
        let theme = self.current_theme;
        crate::primitives::float::native_surface_paint::paint(float, layout, self, &theme);
        // Same convention `draw_minimap`/`draw_image` use for a
        // chrome-only primitive: register the float's own bounds under
        // its id so it's inventory-observable.
        self.register_zone(float.id, layout.bounds);
    }
    fn draw_number_input(
        &mut self,
        number_input: &crate::primitives::number_input::NumberInput,
        layout: &crate::primitives::number_input::NumberInputLayout,
    ) {
        let theme = self.current_theme;
        crate::primitives::number_input::native_surface_paint::paint(
            number_input,
            layout,
            self,
            &theme,
        );
        self.register_zone(number_input.id, layout.bounds);
    }
    /// Same shape as [`Self::draw_number_input`] above.
    fn draw_combo_box(
        &mut self,
        combo_box: &crate::primitives::combo_box::ComboBox,
        layout: &crate::primitives::combo_box::ComboBoxLayout,
    ) {
        let theme = self.current_theme;
        crate::primitives::combo_box::native_surface_paint::paint(combo_box, layout, self, &theme);
        self.register_zone(combo_box.id, layout.field_bounds);
    }
    fn draw_date_picker(
        &mut self,
        date_picker: &crate::primitives::date_picker::DatePicker,
        layout: &crate::primitives::date_picker::DatePickerLayout,
    ) {
        let theme = self.current_theme;
        crate::primitives::date_picker::native_surface_paint::paint(
            date_picker,
            layout,
            self,
            &theme,
        );
        self.register_zone(date_picker.id, layout.bounds());
    }
    fn draw_multi_section_view(&mut self, rect: Rect, view: &MultiSectionView) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_multi_section_view called outside enter_frame_scope",
        );
        // Section headers and Tree/List body rows are sidebar chrome
        // (`ChromePrimitive::MultiSectionView`). The font swaps to chrome,
        // and `line_height` uses the chrome metric so row spacing matches
        // the compose layer's `chrome_line_height()`-based hit regions.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.chrome_line_height;
        let char_width = self.chrome_char_width;
        let caret_visible = self.caret_visible.get();
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::multi_section_view::draw_multi_section_view(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                view,
                &theme,
                line_height,
                char_width,
                caret_visible,
            )
        }
    }
    fn msv_layout(&self, rect: Rect, view: &MultiSectionView) -> MultiSectionViewLayout {
        super::multi_section_view::mac_msv_layout(view, rect, self.chrome_line_height)
    }
    fn msv_metrics(&self) -> MsvLayoutMetrics {
        super::multi_section_view::mac_msv_metrics(self.chrome_line_height, false)
    }
    fn tree_layout(&self, rect: Rect, tree: &TreeView) -> TreeViewLayout {
        super::tree::mac_tree_layout(tree, rect, self.chrome_line_height, &self.chrome_font)
    }
    fn tree_vscrollbar(&self, rect: Rect, tree: &TreeView) -> Option<crate::Scrollbar> {
        let row_h =
            crate::primitives::layout_metrics::tree_row_pitch(tree, self.chrome_line_height);
        tree.vscrollbar_with_track_width(
            rect,
            row_h as f32,
            crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH,
        )
    }
    fn tree_paints_own_scrollbar(&self) -> bool {
        true
    }
    fn form_layout(&self, rect: Rect, form: &Form) -> FormLayout {
        // No-paint twin of `draw_form` — must agree with what that
        // method painted.
        let font = &self.chrome_font;
        super::form::mac_form_layout(form, rect, self.current_line_height, font)
    }
    fn draw_editor(&mut self, _rect: Rect, editor: &Editor) -> EditorPaintResult {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_editor called outside enter_frame_scope",
        );
        let font = self.current_font.as_ref().unwrap_or(&self.chrome_font);
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let char_width = self.current_char_width;
        let v_scrollbar_w = self.editor_v_scrollbar_w;
        // `editor.rect` is the sole geometry
        // `draw_editor_with_options_and_v_scrollbar_w` reads (see that
        // function's module doc) — this trait method's own `_rect`
        // argument is unused by design, same as `GtkBackend::draw_editor`
        // (see `Backend::draw_editor`'s doc for the caller invariant that
        // the two are expected to agree). A host's `editor.rect` can
        // disagree with this backend's real window bounds (e.g. a
        // rounding/DPI mismatch making `editor.rect` wider than the
        // window); painting straight into it lays the reserved scrollbar
        // column mostly or wholly past the right edge, leaving only a
        // sliver visible. Clamp a defensive copy so nothing this
        // rasteriser paints — least of all the scrollbar track — can
        // extend past the real viewport.
        let clamped = clamp_editor_rect_to_viewport(editor, self.viewport);
        let editor = clamped.as_ref().unwrap_or(editor);
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::editor::draw_editor_with_options_and_v_scrollbar_w(
                ctx,
                font,
                editor,
                &theme,
                char_width,
                line_height,
                crate::primitives::editor::EditorPaintOptions::default(),
                v_scrollbar_w,
            )
        }
    }
    fn set_editor_v_scrollbar_width(&mut self, px: Option<f32>) {
        self.editor_v_scrollbar_w = px;
    }
    fn editor_v_scrollbar_width(&self) -> Option<f32> {
        self.editor_v_scrollbar_w
    }
    fn draw_message_list(&mut self, rect: Rect, list: &MessageList) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_message_list called outside enter_frame_scope",
        );
        // `MessageList` is chrome, not editor content — see
        // `draw_data_table`'s comment.
        let font = &self.chrome_font;
        let line_height = self.current_line_height;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::message_list::draw_message_list(
                ctx,
                font,
                list,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                (rect.y + rect.height) as f64,
                line_height,
            );
        }
    }
    fn draw_rich_text_popup(&mut self, popup: &RichTextPopup, layout: &RichTextPopupLayout) {
        // Chrome primitives are drawn through `chrome_font` —
        // `draw_tree`'s comment explains the swap.
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_rich_text_popup called outside enter_frame_scope",
        );
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::rich_text_popup::draw_rich_text_popup(ctx, font, popup, layout, &theme) }
    }

    fn draw_rich_text_popup_with_font_role(
        &mut self,
        popup: &RichTextPopup,
        layout: &RichTextPopupLayout,
        role: crate::FontRole,
    ) {
        // `role` picks which live `CTFont` this call
        // paints the popup in — `FontRole::Chrome` keeps `chrome_font`
        // (same as plain `draw_rich_text_popup`); `FontRole::Editor`
        // swaps in `current_font`, the same resolution
        // `Self::measure_text`/`Self::surface_draw_text_run_with_role`
        // already use per role.
        if role == crate::FontRole::Chrome {
            self.draw_rich_text_popup(popup, layout);
            return;
        }
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_rich_text_popup_with_font_role called outside enter_frame_scope",
        );
        let font = self.current_font.as_ref().unwrap_or(&self.chrome_font);
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::rich_text_popup::draw_rich_text_popup(ctx, font, popup, layout, &theme) }
    }
    fn draw_find_replace(&mut self, _rect: Rect, panel: &FindReplacePanel) {
        debug_assert!(
            !self.current_cg().is_null(),
            "MacBackend::draw_find_replace called outside enter_frame_scope",
        );
        // No `set_current_font` precondition here —
        // `crate::primitives::find_replace::paint` reaches text
        // rendering through `PaintSurface`, whose
        // `surface_measure_text`/`surface_draw_text_run*` read
        // `chrome_font` unconditionally (see those methods' docs), so
        // there is nothing for this call to precondition on.
        let theme = self.current_theme;
        crate::primitives::find_replace::paint(panel, self, &theme);
    }
    fn draw_completions(&mut self, completions: &Completions, layout: &CompletionsLayout) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_completions called outside enter_frame_scope",
        );
        // The completions popup chrome (labels, icons) is chrome, not
        // editor content — see `draw_data_table`'s comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { super::completions::draw_completions(ctx, font, completions, layout, &theme) }
    }
    fn draw_scrollbar(&mut self, _rect: Rect, scrollbar: &Scrollbar) {
        let theme = self.current_theme;
        crate::primitives::scrollbar::native_surface_paint::paint(scrollbar, self, &theme);
    }

    fn draw_drop_overlay(&mut self, overlay: &crate::primitives::drop_zone::DropOverlay) {
        let theme = self.current_theme;
        crate::primitives::drop_zone::native_surface_paint::paint(overlay, self, &theme);
    }
    /// macOS's own idiomatic default for a top-level menu bar:
    /// presents `bar` as the real system `NSMenu` whenever
    /// [`Backend::effective_menu_style`] resolves to
    /// [`crate::backend::ResolvedMenuStyle::Native`] — the default on
    /// macOS, since [`Backend::backend_caps`] declares `native_menu:
    /// true` here — falling back to [`Self::draw_menu_bar_painted`]'s
    /// in-window strip once the app opts out via
    /// [`Backend::set_menu_style`]`(`[`crate::backend::MenuStyle::Custom`]`)`.
    ///
    /// Every generic caller of this trait method — [`crate::frame`]'s
    /// `Surface::MenuBar` dispatch, [`crate::compose::MenuSystem::render`]
    /// — gets this for free through `&mut dyn Backend`, with no
    /// macOS-specific call site required: macOS has no Alt-mnemonic
    /// convention and VS Code itself defers to the system menu bar
    /// there, so a painted `File Edit View` strip is the one that needs
    /// opting into, not the other way round.
    ///
    /// # Layout contract
    ///
    /// The native path paints nothing and returns a layout whose
    /// `bounds.height` is `0.0` — a caller computing "content starts
    /// below the menu bar" from that height reserves no blank strip,
    /// since AppKit's own menu bar lives outside the window entirely.
    /// The painted (`Custom`) path returns exactly what
    /// [`Self::draw_menu_bar_painted`] always has.
    ///
    /// # Per-frame idempotence
    ///
    /// Called every frame by the generic callers above, but
    /// [`Self::install_menu_bar_if_changed`] only actually reinstalls
    /// the `NSMenu` tree when `bar` differs from the last install — so
    /// this does not churn `NSApp.setMainMenu:` on every tick, only on
    /// an actual menu-state change (a new accelerator, a toggled
    /// checkmark, …).
    fn draw_menu_bar(&mut self, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
        match self.effective_menu_style() {
            crate::backend::ResolvedMenuStyle::Native => {
                self.install_menu_bar_if_changed(bar);
                native_menu_bar_layout(rect)
            }
            crate::backend::ResolvedMenuStyle::Custom => self.draw_menu_bar_painted(rect, bar),
        }
    }
    /// No-paint twin of [`Self::draw_menu_bar`] — must agree with what
    /// that method returns, including its `MenuStyle` branch. Under the
    /// native style the in-window strip has no items, so a hit-test
    /// against this layout (as [`crate::compose::MenuSystem`]'s mouse
    /// handling does) finds nothing to open instead of a painted
    /// dropdown over content the strip never covered.
    fn menu_bar_layout(&self, rect: Rect, bar: &MenuBar) -> MenuBarLayout {
        if self.effective_menu_style() == crate::backend::ResolvedMenuStyle::Native {
            return native_menu_bar_layout(rect);
        }
        let font = &self.chrome_font;
        super::menu_bar::mac_menu_bar_layout(
            font,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            bar,
        )
    }
    fn draw_split(&mut self, rect: Rect, split: &Split) -> SplitLayout {
        let layout = super::split::mac_split_layout(
            split,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        let theme = self.current_theme;
        crate::primitives::split::native_surface_paint::paint(&layout, self, &theme);
        layout
    }
    fn split_layout(&self, rect: Rect, split: &Split) -> SplitLayout {
        super::split::mac_split_layout(
            split,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    fn draw_split_tree(&mut self, rect: Rect, tree: &SplitTree) -> SplitTreeLayout {
        let theme = self.current_theme;
        let layout = super::split_tree::mac_split_tree_layout(
            tree,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        crate::primitives::split_tree::native_surface_paint::paint(&layout, self, &theme);
        layout
    }
    fn split_tree_layout(&self, rect: Rect, tree: &SplitTree) -> SplitTreeLayout {
        super::split_tree::mac_split_tree_layout(
            tree,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    fn draw_panel(&mut self, rect: Rect, panel: &Panel) -> PanelLayout {
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        let layout = super::panel::mac_panel_layout(
            panel,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            line_height,
        );
        crate::primitives::panel::native_surface_paint::paint(panel, &layout, self, &theme);
        layout
    }
    fn panel_layout(&self, rect: Rect, panel: &Panel) -> PanelLayout {
        super::panel::mac_panel_layout(
            panel,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.current_line_height,
        )
    }
    fn draw_canvas(&mut self, rect: Rect, canvas: &Canvas) -> CanvasLayout {
        let layout = canvas.layout(rect);
        crate::primitives::canvas::native_surface_paint::paint(canvas, &layout, self);
        layout
    }
    /// Paint via the shared [`crate::primitives::button::native_surface_paint::paint`].
    fn draw_button(&mut self, rect: Rect, button: &Button) -> ButtonLayout {
        let theme = self.current_theme;
        let style = self.style();
        let layout = crate::primitives::button::native_surface_paint::paint(
            button, self, &theme, rect, &style,
        );
        self.register_zone(button.id, rect);
        layout
    }
    fn button_layout(&self, rect: Rect, button: &Button) -> ButtonLayout {
        button.layout(rect)
    }
    /// Paint via the shared [`crate::primitives::radio_group::native_surface_paint::paint`].
    /// The per-option measure is nominal (`rect.width` × the chrome
    /// line height) since there is no native text-metrics surface to
    /// measure against yet.
    fn draw_radio_group(&mut self, rect: Rect, group: &RadioGroup) -> RadioGroupLayout {
        let theme = self.current_theme;
        let row_height = self.current_line_height as f32;
        let layout = crate::primitives::radio_group::native_surface_paint::paint(
            group, self, &theme, rect, row_height,
        );
        for (option, item) in group.options.iter().zip(layout.items.iter()) {
            self.register_zone(option.id, *item);
        }
        layout
    }
    fn radio_group_layout(&self, rect: Rect, group: &RadioGroup) -> RadioGroupLayout {
        let row_height = self.current_line_height as f32;
        group.layout(rect, |_option: &RadioOption| RadioOptionMeasure {
            width: rect.width,
            height: row_height,
        })
    }
    /// Paint via the shared [`crate::primitives::switch::native_surface_paint::paint`].
    /// Chrome-only (no label of its own), so this also registers a
    /// click zone — same "paint + register" pairing as
    /// [`Self::draw_focus_ring`].
    fn draw_switch(&mut self, rect: Rect, switch: &Switch) -> SwitchLayout {
        let theme = self.current_theme;
        let layout =
            crate::primitives::switch::native_surface_paint::paint(switch, self, &theme, rect);
        self.register_zone(switch.id, rect);
        layout
    }
    fn switch_layout(&self, rect: Rect, switch: &Switch) -> SwitchLayout {
        switch.layout(rect)
    }
    fn draw_toast_overlay(&mut self, rect: Rect, stack: &ToastOverlay) -> ToastStackLayout {
        // `PaintSurface::surface_fill_rect`/`surface_draw_text_run` (etc)
        // each debug_assert their own frame + font internally, so
        // this method needs no separate ctx/font fetch of its own.
        let theme = self.current_theme;
        let style = self.style();
        let line_height = self.current_line_height as f32;

        let toast_slide_duration = if self.skip_animations {
            Duration::ZERO
        } else {
            Duration::from_millis(220)
        };
        let now = std::time::Instant::now();

        // Track one entrance transition per live toast id: drop any whose
        // toast has disappeared (dismissed toasts are removed from `stack`
        // immediately, so there is nothing left to animate out), and start
        // a fresh one for any id seen for the first time this frame.
        let live_ids: HashSet<WidgetId> = stack.toasts.iter().map(|t| t.id).collect();
        self.toast_enter.retain(|id, _| live_ids.contains(id));
        for id in &live_ids {
            self.toast_enter
                .entry(*id)
                .or_insert_with(|| Transition::start(toast_slide_duration));
        }

        // Toasts slide in from their stack's off-screen edge: right corners
        // from beyond the viewport's right edge, left corners from beyond
        // its left edge.
        let start_dx = match stack.corner {
            ToastCorner::BottomRight | ToastCorner::TopRight => rect.width,
            ToastCorner::BottomLeft | ToastCorner::TopLeft => -rect.width,
        };

        let mut still_animating = false;
        let offsets: HashMap<WidgetId, f32> = self
            .toast_enter
            .iter()
            .map(|(id, transition)| {
                let progress = transition.progress_at(now);
                if progress < 1.0 {
                    still_animating = true;
                }
                (*id, lerp(start_dx, 0.0, progress))
            })
            .collect();

        let layout = crate::primitives::toast::native_surface_paint::paint_with_x_offset(
            stack,
            self,
            &theme,
            &style,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            line_height,
            |id| offsets.get(id).copied().unwrap_or(0.0),
        );

        if still_animating {
            self.request_frame_in(Duration::from_millis(16));
        }

        layout
    }
    fn toast_stack_layout(&self, rect: Rect, stack: &ToastOverlay) -> ToastStackLayout {
        // No-paint twin of `draw_toast_overlay` — that method paints
        // through `self` as `&mut dyn PaintSurface`, which defaults to
        // `chrome_font` (`Toast` is chrome, not editor content) — see
        // `draw_data_table`'s comment and `MacBackend`'s own
        // `PaintSurface` impl.
        let font = &self.chrome_font;
        super::toast::mac_toast_stack_layout(
            stack,
            font,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            self.current_line_height,
        )
    }
    /// Paint via the shared
    /// [`crate::primitives::pipeline_view::native_surface_paint::paint`]
    /// — see that fn's doc for the seven named divergences found while
    /// unifying `gtk::pipeline_view::draw_pipeline_view`,
    /// `macos::pipeline_view::draw_pipeline_view` and
    /// `win::pipeline_view::draw_pipeline_view` into one implementation.
    fn draw_pipeline_view(
        &mut self,
        rect: Rect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        let theme = self.current_theme;
        crate::primitives::pipeline_view::native_surface_paint::paint(view, self, &theme, rect)
    }
    fn pipeline_view_layout(
        &self,
        rect: Rect,
        view: &crate::primitives::pipeline_view::PipelineView,
    ) -> crate::primitives::pipeline_view::PipelineViewLayout {
        super::pipeline_view::mac_pipeline_view_layout(
            view,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    /// Paint via the shared
    /// [`crate::primitives::progress::native_surface_paint::paint`].
    fn draw_progress(&mut self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
        let theme = self.current_theme;
        let style = self.style();
        crate::primitives::progress::native_surface_paint::paint(bar, self, &theme, rect, &style)
    }
    fn progress_layout(&self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout {
        super::progress::mac_progress_layout(
            bar,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    fn draw_spinner(&mut self, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_spinner called outside enter_frame_scope",
        );
        // `Spinner` is chrome, not editor content — see
        // `draw_data_table`'s comment.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::spinner::draw_spinner(ctx, font, rect.x as f64, rect.y as f64, spinner, &theme)
        }
    }
    fn spinner_layout(&self, rect: Rect, spinner: &Spinner) -> SpinnerLayout {
        // No-paint twin of `draw_spinner` — must agree with what that
        // method painted.
        let font = &self.chrome_font;
        super::spinner::mac_spinner_layout(spinner, font, rect.x as f64, rect.y as f64)
    }
    fn draw_command_center(&mut self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_command_center called outside enter_frame_scope",
        );
        // Chrome primitives are drawn through `chrome_font` —
        // `draw_tree`'s comment explains the swap. `line_height` stays
        // the editor pitch (used only for vertical centring).
        let font = &self.chrome_font;
        let theme = self.current_theme;
        let line_height = self.current_line_height;
        // SAFETY: ctx non-null inside frame scope.
        unsafe {
            super::command_center::draw_command_center(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                cc,
                &theme,
                line_height,
            )
        }
    }
    fn command_center_layout(&self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout {
        // No-paint twin of `draw_command_center` — must agree with what
        // that method painted.
        let font = &self.chrome_font;
        super::command_center::mac_command_center_layout(
            cc,
            font,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }
    /// Shared chart painting lives in
    /// [`crate::primitives::chart::paint`] now — see that fn's doc for
    /// the divergences resolved while unifying `gtk::chart::draw_chart`,
    /// `macos::chart::draw_chart` and `win::chart::draw_chart` into one
    /// implementation.
    fn draw_chart(
        &mut self,
        rect: Rect,
        chart: &Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> ChartLayout {
        let theme = self.current_theme;
        let layout = self.chart_layout(rect, chart);
        crate::primitives::chart::paint(chart, &layout, self, &theme, hovered_point, crosshair_x);
        layout
    }
    /// Measured in `chrome_line_height`/`chrome_char_width`, not the
    /// editor pitch: the geometry this returns reserves room for text —
    /// the y-axis tick gutter is `char_width * (longest_label + 1)`, and
    /// the x-tick, x-label and legend rows are each one `line_height`
    /// tall — and every one of those labels is painted by
    /// [`crate::primitives::chart::paint`] through plain `self`, whose
    /// [`PaintSurface`] impl resolves `chrome_font` (see
    /// [`Self::surface_measure_text`]). Sizing the gutter and rows from
    /// the editor font while the glyphs that land in them come from the
    /// chrome font is the metric mismatch the two cached widths exist to
    /// avoid: axis and legend labels would clip against the plot area or
    /// spill outside the chart's own bounds whenever the two fonts
    /// differ in size. [`Self::surface_line_height`] reports the same
    /// chrome pitch `paint` re-reads for the x-label offset, so layout
    /// and paint agree on every row.
    fn chart_layout(&self, rect: Rect, chart: &Chart) -> ChartLayout {
        super::chart::mac_chart_layout(
            chart,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
            self.chrome_line_height,
            self.chrome_char_width,
        )
    }

    fn draw_toolbar_with_options(
        &mut self,
        rect: Rect,
        bar: &crate::primitives::toolbar::Toolbar,
        interaction: &crate::interaction::InteractionState,
        options: crate::primitives::toolbar::ToolbarPaintOptions,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        let (hovered_id, pressed_id) = (interaction.hovered(), interaction.pressed());

        let hover_fade_duration = if self.skip_animations {
            Duration::ZERO
        } else {
            Duration::from_millis(120)
        };
        let now = std::time::Instant::now();

        // Track a fade-in transition for whichever item is currently
        // hovered; at most one id is hovered at a time, and there is no
        // hover-exit state to keep animating once the pointer moves on —
        // same entrance-only scoping as `toast_enter`.
        if let Some(id) = hovered_id {
            self.hover_fade.retain(|k, _| k == id);
            self.hover_fade
                .entry(*id)
                .or_insert_with(|| Transition::start(hover_fade_duration));
        } else {
            self.hover_fade.clear();
        }
        let fade_progress =
            hovered_id.and_then(|id| self.hover_fade.get(id).map(|t| t.progress_at(now)));
        let still_fading = matches!(fade_progress, Some(p) if p < 1.0);

        // While the fade-in is still animating, suppress the shared paint
        // code's binary hover highlight (it would otherwise snap straight
        // to full opacity) in favor of the translucent overlay painted
        // below at `fade_progress` alpha.
        let painted_hovered_id = if still_fading { None } else { hovered_id };

        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_toolbar called outside enter_frame_scope",
        );
        // Chrome primitives are drawn through `chrome_font` —
        // `draw_tree`'s comment explains the swap.
        let font = &self.chrome_font;
        let theme = self.current_theme;
        // SAFETY: ctx is non-null inside the frame scope.
        let layout = unsafe {
            super::toolbar::draw_toolbar_with_options(
                ctx,
                font,
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                bar,
                &theme,
                painted_hovered_id,
                pressed_id,
                options,
            )
        };

        if still_fading {
            if let (Some(id), Some(progress)) = (hovered_id, fade_progress) {
                if let Some(vis) = layout
                    .visible_items
                    .iter()
                    .find(|v| v.action_id.as_ref() == Some(id))
                {
                    let item = vis.bounds;
                    if item.width > 0.0 && item.height > 0.0 {
                        // Mirrors `primitives::toolbar::native_surface_paint`'s
                        // own hover-highlight geometry exactly, so the faded
                        // overlay lines up with the opaque highlight it eases
                        // into.
                        const CORNER_RADIUS: f32 = 4.0;
                        let inset = Rect::new(
                            item.x + 2.0,
                            item.y + 2.0,
                            (item.width - 4.0).max(0.0),
                            (item.height - 4.0).max(0.0),
                        );
                        let color = theme.hover_bg.with_alpha(progress as f64);
                        // SAFETY: ctx is the same non-null frame-scope
                        // pointer used by the paint call above.
                        let mut surface = super::surface::CgSurface {
                            ctx,
                            font: Some(font),
                        };
                        surface.surface_fill_rounded_rect(inset, CORNER_RADIUS, color);
                    }
                }
            }
            self.request_frame_in(Duration::from_millis(16));
        }

        layout
    }

    fn toolbar_layout(
        &self,
        rect: Rect,
        bar: &crate::primitives::toolbar::Toolbar,
    ) -> crate::primitives::toolbar::ToolbarLayout {
        // No-paint twin of `draw_toolbar_interactive` — must agree with
        // what that method painted, so it measures against `chrome_font` too.
        super::toolbar::mac_toolbar_layout(
            bar,
            &self.chrome_font,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    fn draw_sidebar_panel_interactive(
        &mut self,
        rect: Rect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
        interaction: &crate::interaction::InteractionState,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        let (hovered_toolbar_id, pressed_toolbar_id) =
            (interaction.hovered(), interaction.pressed());
        debug_assert!(
            !self.current_cg().is_null(),
            "MacBackend::draw_sidebar_panel called outside enter_frame_scope",
        );
        // `SidebarPanel` composes a `Toolbar` header — its icon glyphs
        // are chrome (`ChromePrimitive::SidebarPanel`), not editor
        // content — so this paints through `ChromeSurface` (routes text
        // through `chrome_font`), the same switch
        // `draw_status_bar_interactive` makes. The default header slot
        // height must be sized off that same `chrome_line_height`
        // metric: the row paints with `chrome_font`, so sizing it off
        // `current_line_height` (the editor's, independently-sized,
        // font pitch) under-reserves the slot whenever the chrome font's
        // line height exceeds the editor's, clipping the tops of the
        // toolbar's icon glyphs against the header's own clip rect.
        let theme = self.current_theme;
        let line_height = self.chrome_line_height as f32;
        let mut surface = ChromeSurface { backend: self };
        crate::primitives::sidebar_panel::native_surface_paint::paint(
            panel,
            &mut surface,
            &theme,
            rect,
            line_height,
            hovered_toolbar_id,
            pressed_toolbar_id,
        )
    }

    /// Paint via the shared
    /// [`crate::primitives::diff_view::native_surface_paint::paint`] —
    /// see that fn's doc for the two named divergences (row/header text
    /// vertical alignment; header-label ellipsize vs. hard-clip) found
    /// while unifying `gtk::diff_view::draw_diff_view`,
    /// `macos::diff_view::draw_diff_view` and
    /// `win::diff_view::draw_diff_view` into one implementation.
    fn draw_diff_view(
        &mut self,
        rect: Rect,
        view: &crate::primitives::diff_view::DiffView,
    ) -> crate::primitives::diff_view::DiffViewLayout {
        // `PaintSurface::surface_fill_rect`/`surface_draw_text_run` (etc)
        // each debug_assert their own `!ctx.is_null()` internally, so
        // this method needs no separate ctx/font fetch of its own.
        //
        // `DiffView` is editor-class — see `draw_terminal`'s comment /
        // `EditorSurface`'s doc.
        let theme = self.current_theme;
        let line_height = self.current_line_height as f32;
        let mut surface = EditorSurface { backend: self };
        crate::primitives::diff_view::native_surface_paint::paint(
            view,
            &mut surface,
            &theme,
            rect,
            line_height,
        )
    }

    fn sidebar_panel_layout(
        &self,
        rect: Rect,
        panel: &crate::primitives::sidebar_panel::SidebarPanel,
    ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
        // No-paint twin of `draw_sidebar_panel_interactive` — must agree
        // with what that method painted, so its header toolbar both
        // measures against `chrome_font` and sizes its default slot
        // height off `chrome_line_height`, matching
        // `draw_sidebar_panel_interactive` above. `chrome_font` is never
        // `None` (seeded at construction — see its field doc), so there
        // is no "no font installed yet" fallback to guard against here.
        super::sidebar_panel::mac_sidebar_panel_layout(
            panel,
            &self.chrome_font,
            self.chrome_line_height,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    /// Override of the trait's no-op default, which returns an empty
    /// `BoardLayout` and paints nothing. Paint via the shared
    /// [`crate::primitives::board::native_surface_paint::paint`].
    fn draw_board(&mut self, rect: Rect, model: &BoardModel) -> BoardLayout {
        let theme = self.current_theme;
        crate::primitives::board::native_surface_paint::paint(model, self, &theme, rect)
    }

    fn board_layout(&self, rect: Rect, model: &BoardModel) -> BoardLayout {
        super::board::mac_board_layout(
            model,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        )
    }

    /// Paints through [`super::minimap::draw_minimap_cached`] (Core
    /// Graphics fills and `CTLine` glyph runs), using the shared
    /// `crate::primitives::minimap` legibility and span helpers GTK and
    /// Win-GUI also use. Only the cached, atlas-backed path shapes glyphs
    /// small enough to fit their row band once
    /// [`crate::primitives::minimap::MinimapScale::Two`] clears the
    /// legibility floor; the plain `draw_minimap_scaled` would use the
    /// full editor-size font.
    fn draw_minimap(
        &mut self,
        rect: Rect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::backend::MinimapPaintResult {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::draw_minimap called outside enter_frame_scope",
        );
        self.register_zone(minimap.id, rect);
        let font = self.current_font.as_ref().unwrap_or(&self.chrome_font);
        let theme = self.current_theme;
        let dpi_scale = self.viewport.scale as f64;
        // SAFETY: ctx is non-null inside the frame scope (checked above).
        let layout = unsafe {
            super::minimap::draw_minimap_cached(
                ctx,
                font,
                rect,
                minimap,
                &theme,
                self.minimap_scale,
                &mut self.minimap_atlas_cache,
                dpi_scale,
            )
        };
        crate::backend::MinimapPaintResult {
            layout,
            painted: true,
        }
    }

    /// The no-paint layout query — same geometry `draw_minimap` computes
    /// internally, exposed standalone for click-routing callers that don't
    /// need a repaint. See [`Self::draw_minimap`]'s doc comment.
    fn minimap_layout(
        &self,
        rect: Rect,
        minimap: &crate::primitives::minimap::Minimap,
    ) -> crate::primitives::minimap::MinimapLayout {
        super::minimap::mac_minimap_layout_scaled(minimap, rect, self.minimap_scale)
    }

    fn minimap_scale(&self) -> crate::primitives::minimap::MinimapScale {
        self.minimap_scale
    }

    fn set_minimap_scale(&mut self, scale: crate::primitives::minimap::MinimapScale) {
        self.minimap_scale = scale;
    }

    /// The macOS `Image` rasteriser uses Core Graphics/ImageIO to
    /// decode and paint images, the same shape of backend-specific work
    /// that Win-GUI does. This backend also maintains a decode cache.
    fn draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        self.register_zone(image.id, rect);
        let ctx = self.current_cg();
        let scale = self.viewport.scale;
        // Unlike `draw_minimap`, `draw_image` can legitimately return
        // `Unsupported` (a decode failure, or a zero-size `rect`) without
        // ever touching `ctx` — so the null-context guard lives inside
        // `super::image::draw_image_cached` itself, right before the
        // CoreGraphics calls that actually need it, rather than an
        // unconditional upfront `debug_assert!` here.
        //
        // SAFETY: `super::image::draw_image_cached` only dereferences
        // `ctx` after confirming it's non-null.
        unsafe { super::image::draw_image_cached(ctx, rect, image, &mut self.image_cache, scale) }
    }

}

/// macOS's `WindowControl` surface, backed by exactly the
/// `NSWindow` methods the issue's own design note names:
/// `setTitle`/`setContentSize`/`setContentMinSize`/`setContentMaxSize`/
/// `setLevel(NSFloatingWindowLevel)`/`toggleFullScreen:`/`miniaturize:`/
/// `orderOut:`/`makeKeyAndOrderFront:`. Every method is real here — macOS
/// has no `GtkBackend`-style structural gap the way Wayland's missing
/// window-position/always-on-top protocols force on GTK.
///
/// **Coordinate note:** [`Self::bounds`]/[`Self::set_bounds`] hand back
/// and accept `NSWindow::frame()`'s raw AppKit screen coordinates —
/// **origin bottom-left, Y increasing upward** — unlike every other
/// in-tree backend's top-left/Y-down convention (GTK's `bounds` — itself
/// position-less on Wayland — and Win's `GetWindowRect`/`SetWindowPos`
/// both use top-left/Y-down screen space). This is a genuine AppKit
/// platform difference, not an oversight: flipping it would need the
/// *screen's* height (`NSScreen::frame()`, tracked separately per
/// display and changeable at runtime as displays are added/removed/
/// resized), and getting that flip wrong silently would be worse than
/// documenting the native convention plainly. Contrast
/// [`Self::titlebar_control_inset`], which *does* flip — but that flips
/// relative to the window's own content-view height, a value already at
/// hand for an unrelated reason, not the screen's.
impl WindowControl for MacBackend {
    fn set_title(&mut self, title: &str) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setTitle(&NSString::from_str(title));
        Ok(())
    }

    fn set_size(&mut self, width: f32, height: f32) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setContentSize(NSSize {
            width: width as f64,
            height: height as f64,
        });
        Ok(())
    }

    fn set_min_size(&mut self, width: f32, height: f32) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setContentMinSize(NSSize {
            width: width as f64,
            height: height as f64,
        });
        Ok(())
    }

    fn set_max_size(&mut self, width: f32, height: f32) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setContentMaxSize(NSSize {
            width: width as f64,
            height: height as f64,
        });
        Ok(())
    }

    /// `NSWindow::frame()` — see this `impl` block's own doc for the
    /// bottom-left-origin coordinate note.
    fn bounds(&self) -> ServiceResult<Rect> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        let frame = window.frame();
        Ok(Rect::new(
            frame.origin.x as f32,
            frame.origin.y as f32,
            frame.size.width as f32,
            frame.size.height as f32,
        ))
    }

    fn set_bounds(&mut self, bounds: Rect) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setFrame_display(
            NSRect {
                origin: NSPoint {
                    x: bounds.x as f64,
                    y: bounds.y as f64,
                },
                size: NSSize {
                    width: bounds.width as f64,
                    height: bounds.height as f64,
                },
            },
            true,
        );
        Ok(())
    }

    /// `NSWindow::center()` — unlike [`crate::win::backend::WinBackend`]'s
    /// `center`/[`crate::gtk::backend::GtkBackend`]'s `Unsupported`, this
    /// needs no manual monitor-geometry math: AppKit centers on whichever
    /// screen "most closely intersects [the window's] current position"
    /// (Apple's own doc for the call) in one round trip.
    fn center(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.center();
        Ok(())
    }

    /// `NSWindow::toggleFullScreen:` — unlike
    /// [`Backend::toggle_window_maximize`]'s `zoom:` (the
    /// double-click-to-maximize equivalent, see that method's doc for why
    /// it is deliberately *not* this call), this is the real AppKit
    /// fullscreen transition. Idempotent: reports the current state via
    /// `styleMask().contains(NSWindowStyleMask::FullScreen)` and only
    /// calls `toggleFullScreen:` when the requested state actually
    /// differs, so entering while already fullscreen (or exiting while
    /// not) is a no-op rather than toggling to the wrong state.
    fn set_fullscreen(&mut self, fullscreen: bool) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        let currently_fullscreen = window.styleMask().contains(NSWindowStyleMask::FullScreen);
        if currently_fullscreen != fullscreen {
            window.toggleFullScreen(None);
        }
        Ok(())
    }

    /// `NSWindow::isZoomed` — AppKit's own maximize-state
    /// query, the same notion [`Backend::toggle_window_maximize`]'s
    /// `zoom:` call flips.
    fn is_maximized(&self) -> ServiceResult<bool> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        Ok(window.isZoomed())
    }

    /// `NSWindow::setLevel(NSFloatingWindowLevel)` — the one always-on-top
    /// mechanism among the four in-tree backends with no platform-level
    /// gap (contrast [`crate::gtk::backend::GtkBackend`]'s `Unsupported`
    /// on GTK4/Wayland).
    fn set_always_on_top(&mut self, on_top: bool) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.setLevel(if on_top {
            NSFloatingWindowLevel
        } else {
            NSNormalWindowLevel
        });
        Ok(())
    }

    /// Toggles `NSWindowStyleMask::Titled` — backs client-side-decoration
    /// paths. AppKit ties the titlebar to this style bit: removing it also
    /// removes the traffic-light buttons and title text.
    fn set_decorated(&mut self, decorated: bool) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        let mut style = window.styleMask();
        if decorated {
            style.insert(NSWindowStyleMask::Titled);
        } else {
            style.remove(NSWindowStyleMask::Titled);
        }
        window.setStyleMask(style);
        Ok(())
    }

    fn minimize(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.miniaturize(None);
        Ok(())
    }

    /// Undoes both a minimize (`deminiaturize:`) and a maximize (`zoom:`,
    /// if currently zoomed) — same "restore from either state" posture as
    /// [`crate::gtk::backend::GtkBackend::restore`] (see that method's
    /// doc for the Electron-parity rationale).
    fn restore(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        if window.isMiniaturized() {
            window.deminiaturize(None);
        }
        if window.isZoomed() {
            window.zoom(None);
        }
        Ok(())
    }

    fn hide(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.orderOut(None);
        Ok(())
    }

    fn show(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.makeKeyAndOrderFront(None);
        Ok(())
    }

    fn focus(&mut self) -> ServiceResult<()> {
        let window = self.window.as_ref().ok_or(BackendError::Unsupported)?;
        window.makeKeyAndOrderFront(None);
        Ok(())
    }
}

/// macOS's `TrayService` surface, backed by
/// `NSStatusBar`/`NSStatusItem` — see [`super::tray`]'s module doc for
/// the full design (lazy item creation, the menu-vs-plain-click
/// trade-off, why `ContextMenuDismissed` doesn't fire for a tray menu).
/// Every method here just forwards to a free function in that module,
/// the same "thin impl on the struct, real logic in a sibling module"
/// shape `Backend::install_menu_bar`/`Backend::show_context_menu` already
/// use for `super::menu_bar_install`.
impl crate::backend::TrayService for MacBackend {
    fn set_icon(&mut self, icon: crate::primitives::image::ImageSource) -> ServiceResult<()> {
        super::tray::set_icon(&mut self.tray, self.events.clone(), icon)
    }

    fn set_tooltip(&mut self, tooltip: &str) -> ServiceResult<()> {
        super::tray::set_tooltip(&mut self.tray, self.events.clone(), tooltip)
    }

    fn set_menu(
        &mut self,
        menu: &crate::primitives::context_menu::ContextMenu,
    ) -> ServiceResult<()> {
        super::tray::set_menu(&mut self.tray, self.events.clone(), menu)
    }
}

// ─── PaintSurface (#807, Phase 1) ───────────────────────────────────────────
//
// The ~15-verb drawing surface underneath `Backend::draw_*`, extracted from
// helpers this backend already had privately — every macOS rasteriser module
// (`activity_bar.rs`, `tooltip.rs`, `board.rs`, ...) declares its own private
// copy of `color_to_cg`/`fill_rect`/`stroke_rect`/the `CGContext*` extern
// block; this impl is one canonical copy, scoped to this file, that Phase 2
// can point those ~30 call sites at instead of their own duplicate. Phase 1
// itself changes no behaviour: nothing calls through `PaintSurface` yet, so
// every existing rasteriser keeps using its own private copy untouched.
// See `paint_surface`'s module doc for the full scope note and why these
// methods are `surface_`-prefixed instead of colliding with `Backend`'s.
/// Adapts `&mut MacBackend` to [`PaintSurface`], routing text
/// measurement and painting through `chrome_font`/`chrome_line_height`/
/// `chrome_char_width` explicitly — the same resolution
/// `MacBackend`'s own `PaintSurface` impl (below) now defaults to for
/// every non-editor-class primitive. A call site reaches for this
/// adapter instead of plain `self` when it needs a *scoped* chrome
/// font — [`Self::status_bar_paint_scaled`] temporarily swaps
/// `chrome_font`/`chrome_line_height`/`chrome_char_width` to a scaled
/// variant and restores them afterward, and wrapping `self` in this
/// type for the paint keeps that swap's intent self-documenting at the
/// call site regardless of what `MacBackend`'s own default happens to
/// resolve to. See `chrome_font`'s field doc for why `chrome_font` and
/// `current_font` must stay separate, and [`EditorSurface`] for the
/// mirror-image adapter that opts an editor-class primitive back into
/// `current_font`.
///
/// Every font-agnostic method (fills, strokes, clip, lines, images, frame
/// lifecycle) forwards straight through to `MacBackend`'s own impl, which
/// doesn't touch the font either way — only the three text-shaped methods
/// below actually differ.
struct ChromeSurface<'a> {
    backend: &'a mut MacBackend,
}

impl PaintSurface for ChromeSurface<'_> {
    fn surface_begin_frame(&mut self, viewport: Viewport) {
        self.backend.surface_begin_frame(viewport)
    }

    fn surface_end_frame(&mut self) {
        self.backend.surface_end_frame()
    }

    fn surface_viewport(&self) -> Viewport {
        self.backend.surface_viewport()
    }

    fn surface_line_height(&self) -> f32 {
        self.backend.chrome_line_height as f32
    }

    fn surface_char_width(&self) -> f32 {
        self.backend.chrome_char_width as f32
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let (w, h) = super::text::measure_text(&self.backend.chrome_font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        self.backend.surface_fill_rect(rect, color)
    }

    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        self.backend.surface_fill_rounded_rect(rect, radius, color)
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        self.backend.surface_stroke_rect(rect, color, stroke_width)
    }

    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        self.backend
            .surface_stroke_rounded_rect(rect, radius, color, stroke_width)
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let ctx = self.backend.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "ChromeSurface::surface_draw_text_run called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text(
                ctx,
                &self.backend.chrome_font,
                text,
                rect.x as f64,
                rect.y as f64,
                ns_color_to_cg(color),
            );
        }
    }

    /// The chrome twin of `MacBackend`'s own override — same
    /// `bold`/`italic`/`underline`-unsupported posture, same `scale_x`
    /// support via [`super::text::draw_text_scaled_x`], just against
    /// `chrome_font`.
    ///
    /// Not optional: this is the method the shared status-bar paint
    /// actually calls. Without it the adapter would inherit the trait
    /// default (forward to [`Self::surface_draw_text_run`]) — right font,
    /// but `scale_x` silently dropped.
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (bold, italic, underline);
        let ctx = self.backend.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "ChromeSurface::surface_draw_text_run_styled called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text_scaled_x(
                ctx,
                &self.backend.chrome_font,
                text,
                rect.x as f64,
                rect.y as f64,
                scale_x as f64,
                ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        self.backend
            .surface_draw_line(from, to, color, stroke_width)
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        self.backend.surface_push_clip(rect)
    }

    fn surface_pop_clip(&mut self) {
        self.backend.surface_pop_clip()
    }

    fn surface_draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        self.backend.surface_draw_image(rect, image)
    }
}

/// The editor-class twin of [`ChromeSurface`] just above. That adapter
/// exists to reach `chrome_font` from a primitive-generic call site.
/// This one exists to reach `current_font` from one, since
/// [`MacBackend`]'s own [`PaintSurface`] impl (below) defaults to
/// `chrome_font` and a handful of primitives still need the editor font.
/// The terminal and the plain text display paint through this trait and
/// need `EditorSurface`; the diff view also paints through this trait
/// and needs it too. The full-screen editor paints through its own
/// dedicated Core Text calls instead of this trait at all, so it needs
/// neither adapter.
///
/// Every font-agnostic method (fills, strokes, clip, lines, images, frame
/// lifecycle) forwards straight through to [`MacBackend`]'s own impl,
/// which doesn't read the font either way. The line-height/char-width
/// pitch methods read `current_line_height`/`current_char_width`
/// directly rather than forwarding, since `MacBackend`'s own impl of
/// those reports the *chrome* metric (see
/// [`MacBackend::surface_line_height`]'s doc) — forwarding would hand an
/// editor-class caller the wrong row pitch. The three text-shaped
/// methods resolve `current_font` with the same "falls back to
/// `chrome_font`, never panics" behaviour as every other `current_font`
/// read site in this file.
struct EditorSurface<'a> {
    backend: &'a mut MacBackend,
}

impl PaintSurface for EditorSurface<'_> {
    fn surface_begin_frame(&mut self, viewport: Viewport) {
        self.backend.surface_begin_frame(viewport)
    }

    fn surface_end_frame(&mut self) {
        self.backend.surface_end_frame()
    }

    fn surface_viewport(&self) -> Viewport {
        self.backend.surface_viewport()
    }

    /// Reads `current_line_height` directly rather than forwarding to
    /// `self.backend.surface_line_height()` — that method reports
    /// the chrome metric (see its doc), which would hand an
    /// editor-class caller the wrong row pitch the moment chrome and
    /// editor fonts diverge.
    fn surface_line_height(&self) -> f32 {
        self.backend.current_line_height as f32
    }

    /// See [`Self::surface_line_height`]'s doc.
    fn surface_char_width(&self) -> f32 {
        self.backend.current_char_width as f32
    }

    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let font = self
            .backend
            .current_font
            .as_ref()
            .unwrap_or(&self.backend.chrome_font);
        let (w, h) = super::text::measure_text(font, text);
        (w as f32, h as f32)
    }

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        self.backend.surface_fill_rect(rect, color)
    }

    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        self.backend.surface_fill_rounded_rect(rect, radius, color)
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        self.backend.surface_stroke_rect(rect, color, stroke_width)
    }

    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        self.backend
            .surface_stroke_rounded_rect(rect, radius, color, stroke_width)
    }

    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let ctx = self.backend.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "EditorSurface::surface_draw_text_run called outside enter_frame_scope",
        );
        let font = self
            .backend
            .current_font
            .as_ref()
            .unwrap_or(&self.backend.chrome_font);
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text(
                ctx,
                font,
                text,
                rect.x as f64,
                rect.y as f64,
                ns_color_to_cg(color),
            );
        }
    }

    /// The `current_font` twin of [`MacBackend`]'s own
    /// `surface_draw_text_run_styled` — same `bold`/`italic`/`underline`-
    /// unsupported posture, same `scale_x` support via
    /// [`super::text::draw_text_scaled_x`] (which fits the terminal's
    /// wide glyphs to their cell advance), just against `current_font`.
    ///
    /// No `surface_draw_text_run_with_role` override here (nor on
    /// [`ChromeSurface`]): both adapters are already constructed against
    /// one specific, caller-chosen font, so the trait default's
    /// "answered before this verb is reached" reasoning
    /// ([`crate::PaintSurface::surface_draw_text_run_with_role`]'s doc)
    /// applies to both. An explicit `FontRole::Chrome` request made
    /// through a live `EditorSurface` would still paint in
    /// `current_font` — there is no call site that does this today.
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (bold, italic, underline);
        let ctx = self.backend.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "EditorSurface::surface_draw_text_run_styled called outside enter_frame_scope",
        );
        let font = self
            .backend
            .current_font
            .as_ref()
            .unwrap_or(&self.backend.chrome_font);
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text_scaled_x(
                ctx,
                font,
                text,
                rect.x as f64,
                rect.y as f64,
                scale_x as f64,
                ns_color_to_cg(color),
            );
        }
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        self.backend
            .surface_draw_line(from, to, color, stroke_width)
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        self.backend.surface_push_clip(rect)
    }

    fn surface_pop_clip(&mut self) {
        self.backend.surface_pop_clip()
    }

    fn surface_draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        self.backend.surface_draw_image(rect, image)
    }
}

impl PaintSurface for MacBackend {
    fn surface_begin_frame(&mut self, viewport: Viewport) {
        Backend::begin_frame(self, viewport);
    }

    fn surface_end_frame(&mut self) {
        Backend::end_frame(self);
    }

    fn surface_viewport(&self) -> Viewport {
        Backend::viewport(self)
    }

    /// `self` is the chrome surface (see [`Self::surface_measure_text`]'s
    /// doc), so this reports `chrome_line_height`, not
    /// `Backend::line_height`'s `current_line_height` — a grid-positioned
    /// chrome primitive (`TextInput`, `FindReplacePanel`, `Chart`, …)
    /// that asks `self` for its row pitch gets the same font's metric
    /// its glyphs paint in, not the editor's.
    fn surface_line_height(&self) -> f32 {
        self.chrome_line_height as f32
    }

    /// See [`Self::surface_line_height`]'s doc.
    fn surface_char_width(&self) -> f32 {
        self.chrome_char_width as f32
    }

    /// `self` is the surface every non-editor-class primitive paints
    /// through unwrapped (`draw_panel`, `draw_toast_overlay`,
    /// `draw_progress`, …), so this reads `chrome_font` unconditionally
    /// rather than `current_font` — see [`EditorSurface`]'s doc for the
    /// three editor-class primitives that opt back into `current_font`
    /// via a dedicated adapter instead, and `crate::font_role`'s module
    /// doc for why a non-editor-class primitive must never resolve
    /// `current_font`.
    fn surface_measure_text(&self, text: &str) -> (f32, f32) {
        let (w, h) = super::text::measure_text(&self.chrome_font, text);
        (w as f32, h as f32)
    }

    // The trait default (ignore `bold`, forward here) already reproduces
    // this backend's pre-#860 `status_bar` rasteriser exactly: it measured
    // and rendered every segment at the same weight regardless of `bold`.

    fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_fill_rect called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { ns_fill_rect(ctx, rect, color) };
    }

    fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_fill_rounded_rect called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { ns_fill_rounded_rect(ctx, rect, radius, color) };
    }

    fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_stroke_rect called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { ns_stroke_rect(ctx, rect, color, stroke_width as f64) };
    }

    /// `ns_stroke_rounded_rect`'s twin of
    /// [`Self::surface_stroke_rect`] above.
    fn surface_stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
        stroke_width: f32,
    ) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_stroke_rounded_rect called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { ns_stroke_rounded_rect(ctx, rect, radius, color, stroke_width as f64) };
    }

    /// See [`Self::surface_measure_text`]'s doc for why this reads
    /// `chrome_font` unconditionally rather than `current_font`.
    fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_draw_text_run called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text(
                ctx,
                &self.chrome_font,
                text,
                rect.x as f64,
                rect.y as f64,
                ns_color_to_cg(color),
            );
        }
    }

    /// Overrides the default (which drops both styling and scale) for
    /// `scale_x` only — `bold`/`italic`/`underline` stay unsupported
    /// (this backend's documented "not rendered yet" posture), but
    /// `draw_text_scaled_x` already existed here for the wide-glyph
    /// advance fix. Reads `chrome_font` unconditionally — see
    /// [`Self::surface_measure_text`]'s doc. [`EditorSurface`] carries the
    /// `current_font` twin for editor-class callers that still need it.
    #[allow(clippy::too_many_arguments)]
    fn surface_draw_text_run_styled(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        bold: bool,
        italic: bool,
        underline: bool,
        scale_x: f32,
    ) {
        let _ = (bold, italic, underline);
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_draw_text_run_styled called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text_scaled_x(
                ctx,
                &self.chrome_font,
                text,
                rect.x as f64,
                rect.y as f64,
                scale_x as f64,
                ns_color_to_cg(color),
            );
        }
    }

    /// Overrides the default (which ignores `role`) — `chrome_font`
    /// and `current_font` are already genuinely different `CTFont`s on
    /// this backend (see `chrome_font`'s field doc), so a `dyn
    /// PaintSurface` caller holding a live `MacBackend` can now paint
    /// either without a separate [`ChromeSurface`] wrapper. `italic` is
    /// dropped, matching [`Self::surface_draw_text_run_styled`]'s own
    /// documented "not rendered yet" posture — adding real italic
    /// support is out of this issue's scope.
    fn surface_draw_text_run_with_role(
        &mut self,
        rect: Rect,
        text: &str,
        color: Color,
        role: crate::FontRole,
        italic: bool,
    ) {
        let _ = italic;
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_draw_text_run_with_role called outside enter_frame_scope",
        );
        let font = match role {
            crate::FontRole::Chrome => &self.chrome_font,
            // Falls back to `chrome_font` — never `None` — instead of
            // panicking when a host draws `FontRole::Editor` text before
            // calling `set_current_font`.
            crate::FontRole::Editor => self.current_font.as_ref().unwrap_or(&self.chrome_font),
        };
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            super::text::draw_text(
                ctx,
                font,
                text,
                rect.x as f64,
                rect.y as f64,
                ns_color_to_cg(color),
            );
        }
    }

    /// Overrides the default only to document why it needn't
    /// change anything — [`Self::set_current_font`]/[`Self::set_chrome_font`]
    /// already re-apply [`Self::set_nerd_font_fallback`]'s cascade to
    /// both `current_font` and `chrome_font` (see those methods' docs),
    /// so [`Self::surface_draw_text_run`] already resolves an icon
    /// glyph the same way this verb would.
    fn surface_draw_icon_glyph(&mut self, rect: Rect, text: &str, color: Color) {
        self.surface_draw_text_run(rect, text, color)
    }

    fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_draw_line called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe {
            ns_draw_line(
                ctx,
                from.x as f64,
                from.y as f64,
                to.x as f64,
                to.y as f64,
                color,
                stroke_width as f64,
            );
        }
    }

    fn surface_push_clip(&mut self, rect: Rect) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_push_clip called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope. Every push here
        // must be balanced by a `surface_pop_clip` call — see that method.
        unsafe { ns_push_clip(ctx, rect) };
    }

    fn surface_pop_clip(&mut self) {
        let ctx = self.current_cg();
        debug_assert!(
            !ctx.is_null(),
            "MacBackend::surface_pop_clip called outside enter_frame_scope",
        );
        // SAFETY: ctx is non-null inside the frame scope.
        unsafe { CGContextRestoreGState(ctx) };
    }

    fn surface_draw_image(
        &mut self,
        rect: Rect,
        image: &crate::primitives::image::Image,
    ) -> crate::backend::ImagePaintResult {
        Backend::draw_image(self, rect, image)
    }
}

/// `Color` (0-255 per channel) to CoreGraphics' 0.0-1.0 RGBA tuple —
/// identical math to every per-rasteriser private `color_to_cg` this
/// canonicalises (see this section's module comment).
pub(crate) fn ns_color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

fn ns_cg_rect(rect: Rect) -> CGRect {
    CGRect::new(
        &CGPoint::new(rect.x as f64, rect.y as f64),
        &CGSize::new(rect.width as f64, rect.height as f64),
    )
}

/// # Safety
/// `ctx` must be a valid, non-null `CGContextRef` borrowed for the
/// duration of this call.
pub(crate) unsafe fn ns_fill_rect(ctx: CGContextRef, rect: Rect, c: Color) {
    let (r, g, b, a) = ns_color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, ns_cg_rect(rect));
}

/// [`ns_fill_rect`]'s rounded-corner twin. Builds the
/// rounded-rect path with the classic `CGContextAddArcToPoint`
/// tangent-arc technique (four straight edges, each corner rounded by
/// one arc of `radius`) rather than `CGPathCreateWithRoundedRect` —
/// that constructor returns an owned `CGPathRef` the caller must
/// `CFRelease`, which this file's raw-FFI `extern "C"` block (see its
/// own doc) has no safe place to do automatically; `AddArcToPoint`
/// builds directly on `ctx`'s existing path with no extra object to
/// manage, mirroring `crate::gtk::rounded_rect_path`'s `cr.arc` recipe
/// one level lower (arc-to-point instead of four full arcs) since
/// CoreGraphics has no `cr.arc`-style "arc with explicit start/end
/// angle" primitive as convenient to chain as Cairo's.
///
/// `radius` is clamped to half of `rect`'s shorter side — see
/// [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]'s
/// doc for why every implementation of that verb does this.
///
/// A degenerate (zero or negative width/height) `rect` is a no-op,
/// same guard as [`super::cg::fill_rect_alpha`] — unlike a plain
/// `CGContextFillRect`, which CoreGraphics itself no-ops harmlessly on
/// such a rect, the `AddArcToPoint` path below would otherwise build a
/// malformed path from a negative `r`.
///
/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_fill_rounded_rect(ctx: CGContextRef, rect: Rect, radius: f32, c: Color) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let r = (radius as f64)
        .min(rect.width as f64 / 2.0)
        .min(rect.height as f64 / 2.0)
        .max(0.0);
    let (x, y, w, h) = (
        rect.x as f64,
        rect.y as f64,
        rect.width as f64,
        rect.height as f64,
    );
    let (r0, g0, b0, a0) = ns_color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r0, g0, b0, a0);
    CGContextBeginPath(ctx);
    CGContextMoveToPoint(ctx, x + r, y);
    CGContextAddLineToPoint(ctx, x + w - r, y);
    CGContextAddArcToPoint(ctx, x + w, y, x + w, y + r, r);
    CGContextAddLineToPoint(ctx, x + w, y + h - r);
    CGContextAddArcToPoint(ctx, x + w, y + h, x + w - r, y + h, r);
    CGContextAddLineToPoint(ctx, x + r, y + h);
    CGContextAddArcToPoint(ctx, x, y + h, x, y + h - r, r);
    CGContextAddLineToPoint(ctx, x, y + r);
    CGContextAddArcToPoint(ctx, x, y, x + r, y, r);
    CGContextClosePath(ctx);
    CGContextFillPath(ctx);
}

/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_stroke_rect(ctx: CGContextRef, rect: Rect, c: Color, line_width: f64) {
    let (r, g, b, a) = ns_color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    CGContextStrokeRect(ctx, ns_cg_rect(rect));
}

/// [`ns_stroke_rect`]'s rounded-corner twin — builds the
/// same `CGContextAddArcToPoint` tangent-arc path as
/// [`ns_fill_rounded_rect`], then `CGContextStrokePath` instead of
/// `CGContextFillPath`. See that function's doc for why this path
/// technique over `CGPathCreateWithRoundedRect`.
///
/// `radius` is clamped the same way [`ns_fill_rounded_rect`]'s is.
///
/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_stroke_rounded_rect(
    ctx: CGContextRef,
    rect: Rect,
    radius: f32,
    c: Color,
    line_width: f64,
) {
    let r = (radius as f64)
        .min(rect.width as f64 / 2.0)
        .min(rect.height as f64 / 2.0)
        .max(0.0);
    let (x, y, w, h) = (
        rect.x as f64,
        rect.y as f64,
        rect.width as f64,
        rect.height as f64,
    );
    let (r0, g0, b0, a0) = ns_color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r0, g0, b0, a0);
    CGContextSetLineWidth(ctx, line_width);
    CGContextBeginPath(ctx);
    CGContextMoveToPoint(ctx, x + r, y);
    CGContextAddLineToPoint(ctx, x + w - r, y);
    CGContextAddArcToPoint(ctx, x + w, y, x + w, y + r, r);
    CGContextAddLineToPoint(ctx, x + w, y + h - r);
    CGContextAddArcToPoint(ctx, x + w, y + h, x + w - r, y + h, r);
    CGContextAddLineToPoint(ctx, x + r, y + h);
    CGContextAddArcToPoint(ctx, x, y + h, x, y + h - r, r);
    CGContextAddLineToPoint(ctx, x, y + r);
    CGContextAddArcToPoint(ctx, x, y, x + r, y, r);
    CGContextClosePath(ctx);
    CGContextStrokePath(ctx);
}

/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_draw_line(
    ctx: CGContextRef,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    c: Color,
    line_width: f64,
) {
    let (r, g, b, a) = ns_color_to_cg(c);
    CGContextSetRGBStrokeColor(ctx, r, g, b, a);
    CGContextSetLineWidth(ctx, line_width);
    CGContextMoveToPoint(ctx, x0, y0);
    CGContextAddLineToPoint(ctx, x1, y1);
    CGContextStrokePath(ctx);
}

/// Push an axis-aligned clip rect by saving graphics state then clipping —
/// CoreGraphics has no clip-only push/pop pair, so (as every macOS
/// rasteriser that clips already does — e.g. `macos::board`'s column/card
/// clipping) this piggybacks on the save/restore GState stack:
/// `MacBackend`'s `PaintSurface::surface_pop_clip` impl calls
/// `CGContextRestoreGState` directly to match.
///
/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_push_clip(ctx: CGContextRef, rect: Rect) {
    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, ns_cg_rect(rect));
}

/// Pop a clip pushed by [`ns_push_clip`]. `pub(crate)` alongside it so a
/// caller with only a raw `CGContextRef` (no live `MacBackend`) — e.g.
/// [`crate::primitives::multi_section_view`]'s embedded-`Form` section
/// body — can balance its own `ns_push_clip` call the same way
/// `MacBackend`'s `PaintSurface::surface_pop_clip` impl does.
///
/// # Safety
/// Same contract as [`ns_fill_rect`].
pub(crate) unsafe fn ns_pop_clip(ctx: CGContextRef) {
    CGContextRestoreGState(ctx);
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextTranslateCTM(c: CGContextRef, tx: CGFloat, ty: CGFloat);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: CGFloat,
        green: CGFloat,
        blue: CGFloat,
        alpha: CGFloat,
    );
    fn CGContextSetRGBStrokeColor(
        c: CGContextRef,
        red: CGFloat,
        green: CGFloat,
        blue: CGFloat,
        alpha: CGFloat,
    );
    fn CGContextSetLineWidth(c: CGContextRef, width: CGFloat);
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
    fn CGContextStrokeRect(c: CGContextRef, rect: CGRect);
    fn CGContextMoveToPoint(c: CGContextRef, x: CGFloat, y: CGFloat);
    fn CGContextAddLineToPoint(c: CGContextRef, x: CGFloat, y: CGFloat);
    fn CGContextStrokePath(c: CGContextRef);
    // Used by `ns_fill_rounded_rect`'s tangent-arc rounded-rect path.
    fn CGContextBeginPath(c: CGContextRef);
    fn CGContextAddArcToPoint(
        c: CGContextRef,
        x1: CGFloat,
        y1: CGFloat,
        x2: CGFloat,
        y2: CGFloat,
        radius: CGFloat,
    );
    fn CGContextClosePath(c: CGContextRef);
    fn CGContextFillPath(c: CGContextRef);
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

    // ── bundled codicon font self-registration ───────────────────────────

    /// `MacBackend::new` self-registers the bundled codicon font before
    /// any app code runs, so the family is resolvable for painting with
    /// no app configuration — the macOS half of the contract
    /// `codicon::FONT_FAMILY` documents, and the counterpart of
    /// `gtk::backend::tests::gtk_backend_new_self_registers_the_bundled_codicon_font`.
    ///
    /// Deliberately probes with [`crate::macos::text::make_font_exact`]
    /// rather than `ensure_codicon_registered()`: that function
    /// *performs* the registration on first call, so asserting its own
    /// return value would pass even if `MacBackend::new()` never called
    /// it. `make_font_exact` is a pure Core Text name lookup with no
    /// registration side effect, so it can only see the family if
    /// `new()` actually registered it.
    #[test]
    fn mac_backend_new_self_registers_the_bundled_codicon_font() {
        let _ = MacBackend::new();
        assert!(
            crate::macos::text::make_font_exact(crate::codicon::FONT_FAMILY, 12.0).is_some(),
            "MacBackend::new() must leave the bundled codicon font registered"
        );
    }

    /// `nerd_fonts_enabled` defaults to `true` on this backend — see the
    /// field's own doc for why that default differs from TUI's. Mirrors
    /// `WinBackend`'s `nerd_fonts_enabled_defaults_to_true` and
    /// `GtkBackend`'s `gtk_backend_nerd_fonts_enabled_defaults_to_true`.
    #[test]
    fn mac_backend_nerd_fonts_enabled_defaults_to_true() {
        let b = MacBackend::new();
        assert!(
            b.nerd_fonts_enabled(),
            "icons are on by default on this GUI backend — the bundled \
             codicon font is always available here"
        );
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

    /// `MacBackend::modal_stack_handle` must hand back a
    /// handle that shares state with the backend's own modal stack —
    /// this is the whole point of the issue: a host holding only
    /// `&mut dyn Backend` needs this to work identically on macOS to
    /// how it already works on `GtkBackend`
    /// (`gtk::backend::tests::gtk_backend_modal_stack_handle_shares_state`)
    /// and `TuiBackend`
    /// (`tui::backend::tests::tui_backend_modal_stack_handle_shares_state`).
    #[test]
    fn mac_backend_modal_stack_handle_shares_state() {
        let backend = MacBackend::new();
        let h1 = backend.modal_stack_handle();
        let h2 = backend.modal_stack_handle();
        h1.borrow_mut().push(
            crate::types::WidgetId::new("test:popup"),
            crate::event::Rect::new(0.0, 0.0, 10.0, 5.0),
        );
        assert_eq!(h2.borrow().len(), 1);
    }

    /// On macOS headless, `Auto` resolves to `Native` (this backend
    /// declares `caps.native_menu`) and `Custom` resolves to painted.
    #[test]
    fn mac_backend_auto_resolves_native_and_custom_resolves_painted() {
        let mut backend = MacBackend::new();

        // Default is `Auto`, and `Auto` resolves `Native` here because
        // `MacBackend::backend_caps().native_menu` is `true`.
        assert_eq!(backend.menu_style(), crate::backend::MenuStyle::Auto);
        assert_eq!(
            backend.effective_menu_style(),
            crate::backend::ResolvedMenuStyle::Native
        );

        backend.set_menu_style(crate::backend::MenuStyle::Custom);
        assert_eq!(backend.menu_style(), crate::backend::MenuStyle::Custom);
        assert_eq!(
            backend.effective_menu_style(),
            crate::backend::ResolvedMenuStyle::Custom
        );
    }

    /// `surface_measure_text` reads `chrome_font` unconditionally — not
    /// just as a fallback for an unset `current_font`, but regardless of
    /// whether `current_font` is set at all, and regardless of what it's
    /// set to (see that method's own doc). A fresh, never-configured
    /// `MacBackend` needs no live `enter_frame_scope`/`CGContextRef` to
    /// exercise this, so it's the cheapest coverage for the always-chrome
    /// behaviour.
    #[test]
    fn surface_measure_text_always_reads_chrome_font() {
        let mut backend = MacBackend::new();
        let (w, h) = backend.surface_measure_text("A");
        assert!(
            w > 0.0 && h > 0.0,
            "must measure through the always-`Some` chrome_font, not panic or report zero: \
             got ({w}, {h})"
        );

        // Matches the real font's own answer, not some hardcoded
        // stand-in.
        let (expect_w, expect_h) = crate::macos::text::measure_text(&backend.chrome_font, "A");
        assert_eq!((w, h), (expect_w as f32, expect_h as f32));

        // Installing a `current_font` with very different metrics must
        // not move the measurement — this reads `chrome_font` only,
        // never `current_font`, unlike `EditorSurface::surface_measure_text`.
        backend.set_current_font(
            crate::macos::text::make_font("Menlo", 200.0).expect("Menlo installed"),
        );
        let (w2, h2) = backend.surface_measure_text("A");
        assert_eq!(
            (w2, h2),
            (w, h),
            "surface_measure_text must stay pinned to chrome_font even once current_font is set"
        );
    }

    /// Regression test: `install_menu_bar` must not panic when called
    /// off the main thread. `MacBackend` is `!Send`, so this test builds
    /// the backend inside the spawned thread. After the fix, the call
    /// degrades to a silent no-op: no `menu_target` retained.
    #[test]
    fn install_menu_bar_off_main_thread_does_not_panic() {
        let bar = crate::primitives::menu_bar::MenuBar {
            id: WidgetId::new("menubar"),
            items: vec![],
            open_item: None,
            focused_item: None,
        };
        let handle = std::thread::spawn(move || {
            let mut backend = MacBackend::new();
            backend.install_menu_bar(&bar);
            backend.menu_target.is_none()
        });
        let no_target_installed = handle
            .join()
            .expect("install_menu_bar must return, not panic, off the main thread (#930)");
        assert!(
            no_target_installed,
            "off the main thread there is no MainThreadMarker, so install_menu_bar must be a \
             no-op that leaves menu_target unset"
        );
    }

    /// Regression test for `show_context_menu`: like `install_menu_bar`,
    /// it must not panic when called off the main thread, degrading to a
    /// silent no-op with no menu retained.
    #[test]
    fn show_context_menu_off_main_thread_does_not_panic() {
        let menu = crate::primitives::context_menu::ContextMenu {
            id: WidgetId::new("ctx"),
            items: vec![],
            selected_idx: 0,
            bg: None,
            placement: Default::default(),
        };
        let handle = std::thread::spawn(move || {
            let mut backend = MacBackend::new();
            backend.show_context_menu(&menu, Point::new(0.0, 0.0));
        });
        handle
            .join()
            .expect("show_context_menu must return, not panic, off the main thread (#930)");
    }

    /// The stash-then-reuse pattern this issue exists to
    /// unblock — obtain the handle through `&mut dyn Backend`, drop
    /// that borrow, and use the handle afterwards from an unrelated
    /// borrow scope. Mirrors
    /// `tui::backend::tests::modal_stack_handle_outlives_the_backend_borrow_through_the_trait`.
    #[test]
    fn modal_stack_handle_outlives_the_backend_borrow_through_the_trait() {
        let mut backend = MacBackend::new();
        let stack_rc = {
            let dyn_backend: &mut dyn Backend = &mut backend;
            dyn_backend.modal_stack_handle()
        };
        backend.begin_frame(Viewport::new(80.0, 24.0, 1.0));
        stack_rc.borrow_mut().push(
            crate::types::WidgetId::new("test:popup"),
            crate::event::Rect::new(0.0, 0.0, 10.0, 5.0),
        );
        assert_eq!(backend.modal_stack_handle().borrow().len(), 1);
    }

    /// Same shared-state guarantee as
    /// `mac_backend_modal_stack_handle_shares_state`, for
    /// `drag_state_handle`.
    #[test]
    fn mac_backend_drag_state_handle_shares_state() {
        let backend = MacBackend::new();
        let h1 = backend.drag_state_handle();
        let h2 = backend.drag_state_handle();
        h1.borrow_mut()
            .begin(crate::dispatch::DragTarget::TextSelection {
                region: crate::types::WidgetId::new("r"),
                anchor: Point::new(0.0, 0.0),
            });
        assert!(h2.borrow().is_active());
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

    /// The registry itself now lives in `BackendCore` — its
    /// `contains_key` isn't reachable from here anymore, so this observes
    /// the same round trip behaviourally via `match_keypress`.
    #[test]
    fn register_and_unregister_accelerator_round_trip() {
        let mut b = MacBackend::new();
        let a = acc("save", "<C-s>");
        b.register_accelerator(&a);
        let ctrl_s = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            b.match_keypress(&Key::Char('s'), ctrl_s),
            Some(AcceleratorId::new("save"))
        );
        b.unregister_accelerator(&AcceleratorId::new("save"));
        assert_eq!(b.match_keypress(&Key::Char('s'), ctrl_s), None);
    }

    // ── Accelerator matching ─────────────────────────────────────────

    #[test]
    fn match_keypress_finds_registered_global_binding() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("save", "<D-s>"));
        let id = b.match_keypress(
            &crate::Key::Char('s'),
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        );
        assert_eq!(id, Some(AcceleratorId::new("save")));
    }

    #[test]
    fn match_keypress_modifier_mismatch_no_match() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("save", "<D-s>"));
        // Same key, wrong modifiers (Ctrl instead of Cmd).
        let id = b.match_keypress(
            &crate::Key::Char('s'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert_eq!(id, None);
    }

    #[test]
    fn match_keypress_skips_non_global_scope() {
        let mut b = MacBackend::new();
        b.register_accelerator(&Accelerator {
            id: AcceleratorId::new("find-in-tree"),
            binding: KeyBinding::Literal("<D-f>".to_string()),
            scope: AcceleratorScope::Mode("tree".into()),
            label: None,
        });
        let id = b.match_keypress(
            &crate::Key::Char('f'),
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        );
        assert_eq!(id, None, "non-Global scope must not match here");
    }

    #[test]
    fn match_keypress_unregister_removes_match() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("save", "<D-s>"));
        b.unregister_accelerator(&AcceleratorId::new("save"));
        let id = b.match_keypress(
            &crate::Key::Char('s'),
            Modifiers {
                cmd: true,
                ..Default::default()
            },
        );
        assert_eq!(id, None);
    }

    #[test]
    fn match_keypress_re_register_replaces_binding() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("save", "<D-s>"));
        b.register_accelerator(&acc("save", "<D-S-s>")); // Cmd+Shift+S
        assert!(b
            .match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    cmd: true,
                    ..Default::default()
                }
            )
            .is_none());
        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    cmd: true,
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(AcceleratorId::new("save"))
        );
    }

    /// `parse_binding` resolves every universal `KeyBinding` variant to a
    /// **Ctrl** `ParsedBinding` regardless of platform. `accelerator_to_ns`
    /// (native menu path) and `render_binding` (display path) both render
    /// these as **Cmd** on macOS, so a real Cmd+S keypress must match a
    /// `KeyBinding::Save` registration.
    #[test]
    fn match_keypress_universal_binding_matches_native_cmd_not_ctrl() {
        let mut b = MacBackend::new();
        b.register_accelerator(&Accelerator {
            id: AcceleratorId::new("save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        });

        // The advertised shortcut (⌘S, matching `accelerator_to_ns` /
        // `render_binding`) must fire.
        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    cmd: true,
                    ..Default::default()
                },
            ),
            Some(AcceleratorId::new("save")),
        );

        // The raw literal Ctrl+S that `parse_binding` alone would
        // produce must NOT fire — that's not what's on screen and not
        // the native macOS idiom.
        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            None,
        );
    }

    /// `KeyBinding::Redo` parses to `<C-S-z>` (Ctrl+Shift+Z) — verifies
    /// the Ctrl→Cmd translation preserves the co-occurring Shift
    /// modifier instead of clobbering it, matching
    /// `accelerator_to_ns`'s `cmd() | shift()` for the same variant.
    #[test]
    fn match_keypress_universal_binding_preserves_shift_modifier() {
        let mut b = MacBackend::new();
        b.register_accelerator(&Accelerator {
            id: AcceleratorId::new("redo"),
            binding: KeyBinding::Redo,
            scope: AcceleratorScope::Global,
            label: None,
        });

        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('z'),
                Modifiers {
                    cmd: true,
                    shift: true,
                    ..Default::default()
                },
            ),
            Some(AcceleratorId::new("redo")),
        );
    }

    /// `KeyBinding::Literal` bindings are an app author's deliberate,
    /// exact choice (e.g. a literal Ctrl+S that coexists with the
    /// native Cmd+S) — `accelerator_to_ns`/`render_binding` don't
    /// rewrite literals either, so `match_keypress` must not.
    #[test]
    fn match_keypress_literal_binding_ctrl_is_not_translated_to_cmd() {
        let mut b = MacBackend::new();
        b.register_accelerator(&acc("literal-ctrl-s", "<C-s>"));

        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            Some(AcceleratorId::new("literal-ctrl-s")),
        );
        assert_eq!(
            b.match_keypress(
                &crate::Key::Char('s'),
                Modifiers {
                    cmd: true,
                    ..Default::default()
                },
            ),
            None,
        );
    }

    // ── Double-click folding ─────────────────────────────────────────

    fn mouse_down(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn fold_double_click_second_click_same_position_becomes_double_click() {
        let mut b = MacBackend::new();
        let first = b.fold_double_click(mouse_down(5.0, 3.0));
        assert!(matches!(first, UiEvent::MouseDown { .. }));

        let second = b.fold_double_click(mouse_down(5.0, 3.0));
        assert!(
            matches!(second, UiEvent::DoubleClick { .. }),
            "second click at the same position should fold to DoubleClick"
        );
    }

    #[test]
    fn fold_double_click_different_position_stays_mouse_down() {
        let mut b = MacBackend::new();
        let _ = b.fold_double_click(mouse_down(5.0, 3.0));
        let second = b.fold_double_click(mouse_down(50.0, 30.0));
        assert!(matches!(second, UiEvent::MouseDown { .. }));
    }

    #[test]
    fn fold_double_click_passes_non_mouse_down_events_through() {
        let mut b = MacBackend::new();
        let ev = b.fold_double_click(UiEvent::WindowFocused(true));
        assert_eq!(ev, UiEvent::WindowFocused(true));
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

    // ── Background-thread wake ──────────────────────────────────────────
    //
    // `MacBackend::waker`'s `DispatchQueue::main().exec_async` half needs a
    // real, running `NSApplication` main-queue pump to observe — a plain
    // `cargo test` process never drains the main dispatch queue, so
    // asserting on `wake_callback` actually firing here would be
    // meaningless (unlike GTK's `MainContext::invoke`, GCD gives no
    // "no owner, run synchronously" fallback). That half is exercised by
    // `macos.yml`'s real `macos-latest` runner instead. What's
    // host-independent and testable here is the `UserEventQueue` plumbing
    // every platform's `waker()` shares.

    /// A payload pushed from a background thread through the closure
    /// `Backend::waker()` hands out lands in `poll_events()`'s output as a
    /// `UiEvent::User` — the same `Send + Sync` staging queue every
    /// backend's `waker()` shares (`crate::runtime::UserEventQueue`).
    #[test]
    fn waker_payload_reaches_poll_events_as_user_event() {
        let backend = MacBackend::new();
        let waker = Backend::waker(&backend);

        let handle = std::thread::spawn(move || {
            waker(crate::UserPayload::new("from background".to_string()));
        });
        handle.join().expect("background thread must not panic");

        let mut backend = backend;
        let events = backend.poll_events();
        assert_eq!(events.len(), 1);
        match &events[0] {
            UiEvent::User(payload) => {
                assert_eq!(
                    payload.downcast_ref::<String>().map(String::as_str),
                    Some("from background")
                );
            }
            other => panic!("expected UiEvent::User, got {other:?}"),
        }
    }

    /// Before `set_wake_callback` is ever called (a `MacBackend`
    /// constructed directly, never handed to `macos::run`), `waker()`
    /// must not panic — it silently has nothing to wake.
    #[test]
    fn waker_does_not_panic_before_wake_callback_is_installed() {
        let backend = MacBackend::new();
        let waker = Backend::waker(&backend);
        waker(crate::UserPayload::new(1_i32));
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

    /// `MacBackend` now has public `set_current_line_height` and
    /// `set_current_char_width` setters, matching `GtkBackend` and
    /// `WinBackend`. These must override cached values from `set_current_font`,
    /// and must not require a font to be installed. This test would not
    /// compile before this fix.
    #[test]
    fn set_current_line_height_and_char_width_override_font_derived_defaults() {
        let mut b = MacBackend::new();
        let font = super::super::text::make_font("Menlo", 14.0).expect("Menlo installed");
        b.set_current_font(font);
        let font_derived_line_height = b.line_height();
        let font_derived_char_width = b.char_width();

        // Override with values a host's own (portable) measurement
        // produced — deliberately not equal to whatever Menlo-14pt
        // resolved to, so the assertions below can't pass by coincidence.
        let overridden_line_height = font_derived_line_height + 3.0;
        let overridden_char_width = font_derived_char_width + 1.5;
        b.set_current_line_height(overridden_line_height as f64);
        b.set_current_char_width(overridden_char_width as f64);

        assert_eq!(b.line_height(), overridden_line_height);
        assert_eq!(b.char_width(), overridden_char_width);

        // The override doesn't clear `current_font` — draw_* methods that
        // require a font (see e.g. `draw_tree`'s `.expect(...)`) must
        // keep working after a metrics override.
        assert!(b.current_font.is_some());
    }

    // ── Window chrome (CSD, #498) ────────────────────────────────────
    //
    // Every unit test runs with no `NSWindow` constructed (`self.window`
    // stays `None` — `set_window` is only ever called by `macos::run::run`)
    // and no `NSApplication` main-loop running, so these only cover the
    // no-window guard clauses: each method must short-circuit and return
    // `false` *before* touching any `objc2-app-kit` API, not attempt a
    // real AppKit call (which would need a live window server connection
    // this CI/test environment doesn't have) and hang or panic. Mirrors
    // `gtk::backend`'s equivalent `*_false_without_window` tests.

    #[test]
    fn begin_window_drag_false_without_window() {
        let mut b = MacBackend::new();
        assert!(!Backend::begin_window_drag(&mut b));
    }

    /// Also no-ops when a window is present but no press was ever
    /// stashed — there's no real `NSEvent` to hand
    /// `performWindowDragWithEvent:`. Can't construct a real `NSWindow`
    /// headlessly, so this exercises the guard against the private
    /// `pending_window_press` field directly, same rationale as GTK's
    /// analogous `armed_window_drag`-field test.
    #[test]
    fn begin_window_drag_false_without_pending_press() {
        let mut b = MacBackend::new();
        assert!(b.pending_window_press.take().is_none());
    }

    #[test]
    fn toggle_window_maximize_false_without_window() {
        let mut b = MacBackend::new();
        assert!(!Backend::toggle_window_maximize(&mut b));
    }

    /// `is_maximized`/`set_decorated` guard on `self.window` the
    /// same way every other `WindowControl` method here does — no real
    /// `NSWindow` exists in a headless unit test, so both must report
    /// `Unsupported` rather than panic.
    #[test]
    fn is_maximized_err_without_window() {
        let b = MacBackend::new();
        assert!(matches!(
            crate::backend::WindowControl::is_maximized(&b),
            Err(BackendError::Unsupported)
        ));
    }

    #[test]
    fn set_decorated_err_without_window() {
        let mut b = MacBackend::new();
        assert!(matches!(
            crate::backend::WindowControl::set_decorated(&mut b, false),
            Err(BackendError::Unsupported)
        ));
    }

    #[test]
    fn set_cursor_false_without_window() {
        let mut b = MacBackend::new();
        assert!(!Backend::set_cursor(&mut b, PointerShape::Default));
        assert!(!Backend::set_cursor(
            &mut b,
            PointerShape::Resize(ResizeEdge::North)
        ));
    }

    /// Same no-window guard as the three tests above —
    /// `titlebar_control_inset` must short-circuit to the trait's own
    /// `Rect::default()` before touching `standardWindowButton` at all.
    /// The live-window case (non-empty inset matching AppKit's actual
    /// traffic-light layout) can't be constructed headlessly in this test
    /// binary either, same rationale as `begin_window_drag`'s sibling
    /// comment above — covered by the operator-run smoke tier instead.
    #[test]
    fn titlebar_control_inset_default_without_window() {
        let b = MacBackend::new();
        assert_eq!(Backend::titlebar_control_inset(&b), Rect::default());
    }

    /// With a real `NSWindow`, `titlebar_control_inset` must report
    /// the actual traffic-light cluster width, not the full window width
    /// the pre-fix `close_button.superview()` read (that private
    /// container is `NSTitlebarView`, which spans the window edge to
    /// edge). Main-thread-gated — a no-op under a default `cargo test`
    /// run, same rationale as
    /// `mac_cursor_for_shape_vends_the_kind_it_maps_to` above; runs for
    /// real under `cargo test -- --test-threads=1` on a macOS host.
    ///
    /// Pins two invariants straight from the bug report: well under the
    /// ~78pt HIG cluster width (150pt of slack for accessibility-driven
    /// resizing), and strictly less than the window's own width — the
    /// exact symptom (`inset.width == window width`) this issue was filed
    /// against.
    #[test]
    fn titlebar_control_inset_is_narrow_on_a_live_window() {
        use objc2::msg_send;
        use objc2_app_kit::{NSBackingStoreType, NSWindowStyleMask};

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let window_width = 800.0_f64;
        let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(window_width, 600.0));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::Miniaturizable;
        // SAFETY: `initWithContentRect:styleMask:backing:defer:` is
        // `NSWindow`'s designated initializer, called here on the main
        // thread (guarded by the `MainThreadMarker` above) with a valid,
        // freshly-`alloc`'d receiver — mirrors `macos::run::run_with`'s
        // own construction of the real app window.
        let window: Retained<NSWindow> = unsafe {
            msg_send![
                mtm.alloc::<NSWindow>(),
                initWithContentRect: content_rect,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };

        let mut b = MacBackend::new();
        b.set_window(window);
        let inset = Backend::titlebar_control_inset(&b);

        assert!(
            inset.width < 150.0,
            "traffic-light cluster width should be well under 150pt (HIG spacing is ~78pt) — \
             got {inset:?}; a width anywhere near the window width means this is reading the \
             full-width NSTitlebarView again, not the button cluster (#1154)",
        );
        assert!(
            (inset.width as f64) < window_width,
            "inset width {} must be strictly less than the window width {window_width} — equal \
             (or greater) is the exact #1154 symptom",
            inset.width,
        );
    }

    /// Once the traffic-light buttons are hidden — the state
    /// AppKit puts them in while the window is in native full screen —
    /// `titlebar_control_inset` must collapse to `Rect::default()`
    /// instead of continuing to report the last-known cluster rect. A
    /// stale non-empty inset here is exactly the "grey rectangle" bug
    /// report: a consumer trusts this rect to know how much of the top
    /// band it owns, leaves that many points unpainted assuming the
    /// native buttons will show through, and nothing does because
    /// they're hidden.
    ///
    /// Then flips `setHidden(false)` back and asserts the inset returns
    /// to the same narrow, non-empty rect
    /// `titlebar_control_inset_is_narrow_on_a_live_window` pins above —
    /// covering the acceptance criterion's second half ("the correct
    /// cluster rect again once they reappear"), not just the
    /// hidden-collapses-to-default half.
    #[test]
    fn titlebar_control_inset_collapses_to_default_when_buttons_hidden() {
        use objc2::msg_send;
        use objc2_app_kit::{NSBackingStoreType, NSWindowButton, NSWindowStyleMask};

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::Miniaturizable;
        // SAFETY: same designated-initializer call as
        // `titlebar_control_inset_is_narrow_on_a_live_window` above.
        let window: Retained<NSWindow> = unsafe {
            msg_send![
                mtm.alloc::<NSWindow>(),
                initWithContentRect: content_rect,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };

        let button_kinds = [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ];
        let buttons: Vec<_> = button_kinds
            .iter()
            .filter_map(|kind| window.standardWindowButton(*kind))
            .collect();
        assert!(
            !buttons.is_empty(),
            "a Titled/Closable/Resizable/Miniaturizable window must vend at least one standard \
             window button to hide for this test to mean anything",
        );

        let mut b = MacBackend::new();
        b.set_window(window);

        // Sanity: with the buttons visible (the default just after
        // window creation), the inset is the same narrow non-empty rect
        // the sibling test pins.
        let visible_inset = Backend::titlebar_control_inset(&b);
        assert!(
            visible_inset.width > 0.0,
            "expected a non-empty inset before hiding any buttons, got {visible_inset:?}",
        );

        // Simulate the full-screen transition hiding the cluster.
        for button in &buttons {
            button.setHidden(true);
        }
        assert_eq!(
            Backend::titlebar_control_inset(&b),
            Rect::default(),
            "hidden traffic lights must report Rect::default(), not a stale cluster rect — a \
             non-default result here is the #1177 'grey rectangle' bug",
        );

        // Simulate exiting full screen: the buttons reappear.
        for button in &buttons {
            button.setHidden(false);
        }
        assert_eq!(
            Backend::titlebar_control_inset(&b),
            visible_inset,
            "the inset must report the same cluster rect again once the buttons un-hide",
        );
    }

    /// `PointerShape` → cursor mapping must cover every variant.
    /// Asserts against [`MacCursorKind`], not against `NSCursor` singletons:
    /// `MacBackend` runs on spawned worker threads where AppKit calls are
    /// unsafe, and the invariant to test is which cursor each shape picks,
    /// not that Apple's factory methods return shared instances.
    #[test]
    fn mac_cursor_kind_maps_every_variant() {
        // One expected kind per `desktop::all_pointer_shapes()` entry, in
        // the same order (`Default`, then one `Resize(edge)` per
        // `desktop::ALL_RESIZE_EDGES`: N, S, E, W, NE, NW, SE, SW). The
        // four corner edges fall back to the plain arrow — see
        // `mac_cursor_kind`'s doc for why there's no public
        // diagonal-resize `NSCursor` to use instead.
        let expected = [
            MacCursorKind::Arrow,
            MacCursorKind::ResizeUpDown,
            MacCursorKind::ResizeUpDown,
            MacCursorKind::ResizeLeftRight,
            MacCursorKind::ResizeLeftRight,
            MacCursorKind::Arrow,
            MacCursorKind::Arrow,
            MacCursorKind::Arrow,
            MacCursorKind::Arrow,
        ];
        let shapes = crate::desktop::all_pointer_shapes();
        assert_eq!(
            shapes.len(),
            expected.len(),
            "every PointerShape must have an expected cursor kind — a new variant means a new \
             row here, not a silently-truncated zip"
        );

        for (shape, expected) in shapes.iter().zip(expected.iter()) {
            assert_eq!(
                mac_cursor_kind(*shape),
                *expected,
                "PointerShape {shape:?} did not map to the expected cursor kind",
            );
        }
    }

    /// Pure, AppKit-free coverage of [`union_ns_rect`] — the
    /// building block `titlebar_control_inset` unions the three button
    /// frames with, runnable on every platform and every thread (no
    /// `MainThreadMarker` gate needed, unlike
    /// `titlebar_control_inset_is_narrow_on_a_live_window` above).
    #[test]
    fn union_ns_rect_covers_both_inputs() {
        // Disjoint rects, `b` to the right of and taller than `a` — the
        // union must reach from `a`'s left/bottom edge to `b`'s
        // right/top edge, not just pick one input or average them.
        let a = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 10.0));
        let b = NSRect::new(NSPoint::new(20.0, 5.0), NSSize::new(10.0, 20.0));

        let u = union_ns_rect(a, b);
        assert_eq!(u.origin.x, 0.0);
        assert_eq!(u.origin.y, 0.0);
        assert_eq!(u.size.width, 30.0); // 20.0 + 10.0 - 0.0
        assert_eq!(u.size.height, 25.0); // 5.0 + 20.0 - 0.0

        // Order-independence: `union_ns_rect(a, b) == union_ns_rect(b, a)`,
        // matching the loop in `titlebar_control_inset` folding buttons in
        // whatever order AppKit hands them back.
        let u_swapped = union_ns_rect(b, a);
        assert_eq!(u.origin.x, u_swapped.origin.x);
        assert_eq!(u.origin.y, u_swapped.origin.y);
        assert_eq!(u.size.width, u_swapped.size.width);
        assert_eq!(u.size.height, u_swapped.size.height);
    }

    /// A rect wholly inside another must not shrink the union — this is
    /// what would happen from an accidental intersection instead of a
    /// union.
    #[test]
    fn union_ns_rect_with_nested_rect_keeps_the_larger_one() {
        let outer = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(100.0, 50.0));
        let inner = NSRect::new(NSPoint::new(10.0, 10.0), NSSize::new(5.0, 5.0));

        let u = union_ns_rect(outer, inner);
        assert_eq!(u.origin.x, 0.0);
        assert_eq!(u.origin.y, 0.0);
        assert_eq!(u.size.width, 100.0);
        assert_eq!(u.size.height, 50.0);
    }

    /// The AppKit half of the mapping: each [`MacCursorKind`] vends the
    /// `NSCursor` singleton it names, and distinct kinds are distinct
    /// cursors (so a copy-paste slip in `mac_cursor_for_shape`'s dispatch
    /// — every arm returning `arrowCursor()` — is caught).
    ///
    /// Main-thread-gated, and therefore a no-op under a default
    /// `cargo test` run (libtest hands each test to a worker thread):
    /// vending AppKit objects off the main thread in a process with no
    /// `NSApplication` is the hazard this whole split exists to keep out
    /// of CI. Same guard, same reason, as
    /// `macos::menu_bar_install`'s `install_then_simulate_activation_pushes_event`.
    /// Runs for real under `cargo test -- --test-threads=1` on a macOS
    /// host; the mapping itself is covered unconditionally by
    /// `mac_cursor_kind_maps_every_variant` above.
    #[test]
    // `NSCursor::resizeUpDownCursor`/`resizeLeftRightCursor` are deprecated
    // (see `mac_cursor_for_shape`'s comment) — this test asserts against
    // the exact singletons that function still (deliberately) vends.
    #[allow(deprecated)]
    fn mac_cursor_for_shape_vends_the_kind_it_maps_to() {
        if objc2_foundation::MainThreadMarker::new().is_none() {
            return;
        }
        let arrow = mac_cursor_for_shape(PointerShape::Default);
        let up_down = mac_cursor_for_shape(PointerShape::Resize(ResizeEdge::North));
        let left_right = mac_cursor_for_shape(PointerShape::Resize(ResizeEdge::East));

        assert_eq!(
            Retained::as_ptr(&arrow),
            Retained::as_ptr(&NSCursor::arrowCursor())
        );
        assert_eq!(
            Retained::as_ptr(&up_down),
            Retained::as_ptr(&NSCursor::resizeUpDownCursor())
        );
        assert_eq!(
            Retained::as_ptr(&left_right),
            Retained::as_ptr(&NSCursor::resizeLeftRightCursor())
        );
        assert_ne!(Retained::as_ptr(&arrow), Retained::as_ptr(&up_down));
        assert_ne!(Retained::as_ptr(&arrow), Retained::as_ptr(&left_right));
        assert_ne!(Retained::as_ptr(&up_down), Retained::as_ptr(&left_right));
    }

    /// `backend_caps` must stay honest — declaring `window_chrome`
    /// / `pointer_cursor` requires the corresponding methods actually be
    /// overridden (see `tests/conformance/caps.rs`'s
    /// `backends_declare_only_what_they_override`, which checks this
    /// mechanically from source; this just pins the two flags directly).
    #[test]
    fn backend_caps_declares_window_chrome_and_pointer_cursor() {
        let b = MacBackend::new();
        let caps = b.backend_caps();
        assert!(caps.window_chrome);
        assert!(caps.pointer_cursor);
    }

    /// Before this issue's fix,
    /// `backend_caps().native_dialogs` was `false` (the stub's own doc
    /// comment said so). `native_dialogs` is `Unprovable` in
    /// `CAP_CONTRACTS` (see `backend_caps`'s doc comment above), so
    /// nothing else pins this mechanically — this direct assertion is
    /// the whole gate.
    #[test]
    fn backend_caps_declares_native_dialogs() {
        let b = MacBackend::new();
        assert!(b.backend_caps().native_dialogs);
    }

    /// Build a flat `ListView` whose widest row is `content_width` chars
    /// wide, scrolled to `h_scroll`, with `max_content_width` populated so
    /// it always overflows a narrow viewport. Mirrors
    /// `primitives::list::hscrollbar_tests::list`.
    fn bordered_overflow_list(content_width: usize, h_scroll: usize, bordered: bool) -> ListView {
        ListView {
            id: crate::types::WidgetId::new("l"),
            title: None,
            items: vec![crate::ListItem {
                text: crate::types::StyledText::plain("x".repeat(content_width)),
                detail: None,
                icon: None,
                decoration: crate::types::Decoration::default(),
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered,
            h_scroll,
            max_content_width: Some(content_width),
            show_v_scrollbar: false,
        }
    }

    /// `MacBackend::list_hscrollbar` must inset the track for bordered
    /// lists, matching the shared `primitives::list::hscrollbar_track`
    /// helper and the GTK/Windows implementations. The vertical sibling
    /// bug was fixed in #712, but the horizontal geometry was missed.
    #[test]
    fn list_hscrollbar_insets_track_for_bordered_list() {
        let mut b = MacBackend::new();
        b.current_char_width = 8.0;
        b.current_line_height = 16.0;
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);

        let flat = bordered_overflow_list(40, 0, false);
        let flat_sb = b
            .list_hscrollbar(rect, &flat)
            .expect("overflowing content should yield a scrollbar");
        assert_eq!(flat_sb.track.x, 0.0);
        assert_eq!(flat_sb.track.width, 200.0);

        let bordered = bordered_overflow_list(40, 0, true);
        let bordered_sb = b
            .list_hscrollbar(rect, &bordered)
            .expect("overflowing content should yield a scrollbar");
        // Inset by one char width (8px) on each side, and the thumb
        // (contained within the track) must therefore sit strictly inside
        // the border rather than overpainting it.
        assert_eq!(bordered_sb.track.x, 8.0);
        assert_eq!(bordered_sb.track.width, 184.0);
        assert_eq!(bordered_sb.track.y, 100.0 - 2.0 * 16.0);
        assert!(bordered_sb.track.x >= rect.x + 8.0);
        assert!(bordered_sb.track.x + bordered_sb.track.width <= rect.x + rect.width - 8.0);
    }

    // ── #802/#961/#962: Minimap/Image must not panic on macOS, and both
    // must actually paint ────────────────────────────────────────────────
    //
    // Before #802, `MacBackend::draw_minimap`/`draw_image` were `todo!()`
    // — any `AppLogic` calling either generically through `&mut dyn
    // Backend` (exactly what `examples/common/minimap_app.rs` /
    // `image_app.rs` do, the same fixtures `tests/macos_example_driver.rs`
    // now drives) panicked and took the whole host down on macOS while
    // working fine on TUI/GTK/Win-GUI. #802 replaced that with an honest
    // `painted: false` no-op / `Unsupported` result. #961 replaced the
    // minimap no-op with a real Core Graphics/Core Text paint
    // (`super::minimap::draw_minimap`); #962 does the same for `draw_image`
    // via `super::image::draw_image` (Core Graphics + ImageIO). macOS is
    // no longer the only backend painting nothing for either primitive, so
    // `draw_minimap_paints_and_reports_painted` and
    // `draw_image_paints_and_reports_painted` below assert pixels actually
    // landed, not just "didn't panic" — the corrupt/missing-source cases
    // still assert the clean `Unsupported` degrade (`draw_image_from_*`
    // below).

    fn sample_minimap() -> crate::primitives::minimap::Minimap {
        crate::primitives::minimap::Minimap {
            id: WidgetId::new("minimap"),
            lines: (0..40)
                .map(|i| crate::primitives::minimap::MinimapLine {
                    text: format!("line {i}"),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 10,
            total_buffer_lines: 40,
        }
    }

    /// `draw_minimap` must paint pixels and report `painted: true`.
    /// This replaced an earlier `draw_minimap_does_not_panic_and_reports_unpainted`
    /// test after macOS gained a real Core Graphics/Core Text minimap rasteriser.
    #[test]
    fn draw_minimap_paints_and_reports_painted() {
        use super::super::headless::BitmapSurface;
        use super::super::text::make_font;

        const W: u32 = 100;
        const H: u32 = 200;

        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut b = MacBackend::new();
        b.set_current_font(make_font("Menlo", 12.0).expect("Menlo installed"));
        let minimap = sample_minimap();
        let rect = Rect::new(0.0, 0.0, 20.0, H as f32);

        let result = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            *result.borrow_mut() = Some(backend.draw_minimap(rect, &minimap));
        });
        let result = result
            .into_inner()
            .expect("draw_minimap ran inside the frame scope");

        assert!(
            result.painted,
            "macOS now has a Core Graphics/Core Text minimap rasteriser (#961) -- \
             `painted` must report `true`"
        );
        assert!(
            !result.layout.visible_lines.is_empty(),
            "the layout must be real geometry"
        );
        assert!(
            b.zones().iter().any(|z| z.id == minimap.id),
            "a click zone must be registered so a host can route clicks"
        );

        let painted_any = (0..W).any(|x| {
            (0..H).any(|y| {
                let (r, g, bl, _) = surface.pixel(x, y);
                (r, g, bl) != (255, 255, 255)
            })
        });
        assert!(painted_any, "draw_minimap must paint non-background pixels");
    }

    /// `minimap_layout` (the no-paint query `AppLogic::handle` calls for
    /// click routing) must agree with the layout `draw_minimap` just
    /// returned — same contract every other backend upholds.
    #[test]
    fn minimap_layout_agrees_with_draw_minimap() {
        use super::super::headless::BitmapSurface;
        use super::super::text::make_font;

        let surface = BitmapSurface::new(100, 200);
        let mut b = MacBackend::new();
        b.set_current_font(make_font("Menlo", 12.0).expect("Menlo installed"));
        let minimap = sample_minimap();
        let rect = Rect::new(0.0, 0.0, 20.0, 100.0);

        let painted = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            *painted.borrow_mut() = Some(backend.draw_minimap(rect, &minimap));
        });
        let painted = painted
            .into_inner()
            .expect("draw_minimap ran inside the frame scope");
        let layout_only = b.minimap_layout(rect, &minimap);

        assert_eq!(painted.layout, layout_only);
    }

    /// Empty byte source is a clean `Unsupported` result: the decoder was
    /// tried and had nothing to decode. Deliberately called outside
    /// `enter_frame_scope` (no `ctx`) to prove decoding is attempted before
    /// `draw_image` ever needs a live CoreGraphics context.
    #[test]
    fn draw_image_from_empty_bytes_reports_unsupported() {
        let mut b = MacBackend::new();
        let image = crate::primitives::image::Image {
            id: WidgetId::new("logo"),
            source: crate::primitives::image::ImageSource::Bytes(Vec::new()),
            intrinsic_size: Some((24, 24)),
            fit: crate::primitives::image::ImageFit::Contain,
            fallback_text: "[Q]".into(),
        };
        let rect = Rect::new(0.0, 0.0, 24.0, 24.0);

        let result = b.draw_image(rect, &image);

        assert_eq!(
            result,
            crate::backend::ImagePaintResult::Unsupported,
            "an empty byte source has nothing to decode -- this must be a clean \
             Unsupported result, not a panic"
        );
        assert!(
            b.zones().iter().any(|z| z.id == image.id),
            "a click/hover zone must still be registered even though nothing painted"
        );
    }

    /// A tiny solid-colour PNG, generated at test time — same fixture
    /// shape `super::super::image::tests::tiny_png_bytes` /
    /// `win::image::tests::tiny_png_bytes` build, duplicated here (rather
    /// than shared) because it's a test-only fixture with no runtime
    /// purpose, matching how each backend's own `image.rs` test module
    /// already keeps its own copy.
    fn tiny_png_bytes() -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");

        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = Vec::with_capacity(4 + data.len());
            body.extend_from_slice(kind);
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&4u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);

        // `clippy::same_item_push` misreads the filter-byte push as a
        // "replace this loop with vec![0; N]" candidate — the loop body
        // also appends the row's actual pixel bytes right after it, so
        // that rewrite doesn't apply (see `macos::image::tests::tiny_png_bytes`,
        // whose scanline loop has the same shape).
        #[allow(clippy::same_item_push)]
        let raw = {
            let mut raw = Vec::new();
            for _ in 0..4 {
                raw.push(0u8);
                for _ in 0..4 {
                    raw.extend_from_slice(&[0x00, 0xff, 0x00]);
                }
            }
            raw
        };
        let idat = zlib_store(&raw);
        chunk(&mut png, b"IDAT", &idat);

        chunk(&mut png, b"IEND", &[]);
        png
    }

    fn zlib_store(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let len = data.len() as u16;
        out.push(1);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(&adler32(data).to_be_bytes());
        out
    }

    fn adler32(data: &[u8]) -> u32 {
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// `draw_image` must paint pixels and report `Painted`. This replaced
    /// an earlier degrade-to-`Unsupported` test after macOS gained a real
    /// Core Graphics/ImageIO image rasteriser.
    #[test]
    fn draw_image_paints_and_reports_painted() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 64;
        const H: u32 = 64;

        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut b = MacBackend::new();
        let image = crate::primitives::image::Image {
            id: WidgetId::new("logo"),
            source: crate::primitives::image::ImageSource::Bytes(tiny_png_bytes()),
            intrinsic_size: Some((4, 4)),
            fit: crate::primitives::image::ImageFit::Fill,
            fallback_text: "[Q]".into(),
        };
        let rect = Rect::new(10.0, 10.0, 20.0, 20.0);

        let result = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            *result.borrow_mut() = Some(backend.draw_image(rect, &image));
        });
        let result = result
            .into_inner()
            .expect("draw_image ran inside the frame scope");

        assert_eq!(
            result,
            crate::backend::ImagePaintResult::Painted,
            "macOS now has a Core Graphics/ImageIO image rasteriser (#962) -- a \
             decodable source must report Painted"
        );
        assert!(
            b.zones().iter().any(|z| z.id == image.id),
            "a click/hover zone must be registered"
        );

        let painted_any = (0..W).any(|x| {
            (0..H).any(|y| {
                let (r, g, bl, _) = surface.pixel(x, y);
                (r, g, bl) != (255, 255, 255)
            })
        });
        assert!(painted_any, "draw_image must paint non-background pixels");
    }

    // ── Text selection ───────────────────────────────────────────────
    //
    // State-machine delegation coverage, mirroring `gtk::backend::tests`'
    // (now-lifted, see `crate::text_selection`'s own test module for the
    // shared behaviour) `gtk_*_text_selection*` suite. `apply_selection_highlight`'s
    // actual CoreGraphics paint is covered separately below, against a real
    // `BitmapSurface`.

    fn text_region(id: &str, x: f32, y: f32, w: f32, h: f32, lines: Vec<&str>) -> TextRegion {
        TextRegion {
            id: WidgetId::new(id),
            bounds: crate::event::Rect::new(x, y, w, h),
            lines: lines.into_iter().map(String::from).collect(),
        }
    }

    /// Mirrors `line_height_picks_up_set_current_line_height_via_font_install`'s
    /// inline `make_font` call above, lifted into a helper since the
    /// text-selection extraction tests below need it more than once.
    fn font() -> CTFont {
        super::super::text::make_font("Menlo", 14.0).expect("Menlo installed")
    }

    #[test]
    fn mac_backend_declares_text_selection_capability() {
        let b = MacBackend::new();
        assert!(
            b.backend_caps().text_selection,
            "#803: MacBackend must declare text_selection now that register_text_region/\
             cancel_text_selection_drag are both overridden"
        );
    }

    // ── #929: register_font_from_memory / set_nerd_font_fallback ────────

    #[test]
    fn mac_backend_declares_app_font_registration_capability() {
        let b = MacBackend::new();
        assert!(
            b.backend_caps().app_font_registration,
            "#929: MacBackend must declare app_font_registration now that \
             register_font_from_memory/set_nerd_font_fallback are both overridden"
        );
    }

    /// `set_nerd_font_fallback` called *before* `set_current_font` (the
    /// documented "call once from `setup()`" order) must still land on
    /// the font `set_current_font` installs.
    #[test]
    fn set_nerd_font_fallback_then_set_current_font_carries_the_fallback() {
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;
        use core_text::font_descriptor::kCTFontCascadeListAttribute;

        let mut b = MacBackend::new();
        b.set_nerd_font_fallback("Helvetica");
        b.set_current_font(font());
        let attrs = b
            .current_font
            .as_ref()
            .expect("set_current_font must install a font")
            .copy_descriptor()
            .attributes()
            .to_untyped();
        let cascade_key = unsafe { CFString::wrap_under_get_rule(kCTFontCascadeListAttribute) };
        assert!(
            attrs.contains_key(&cascade_key.as_CFTypeRef()),
            "a fallback set before set_current_font must still be applied to the installed font"
        );
    }

    /// The reverse order — `set_current_font` first, `set_nerd_font_fallback`
    /// second — must retroactively apply the fallback to the
    /// already-installed font rather than requiring `set_current_font`
    /// to be called again.
    #[test]
    fn set_current_font_then_set_nerd_font_fallback_retroactively_applies() {
        use core_foundation::base::TCFType;
        use core_foundation::string::CFString;
        use core_text::font_descriptor::kCTFontCascadeListAttribute;

        let mut b = MacBackend::new();
        b.set_current_font(font());
        b.set_nerd_font_fallback("Helvetica");
        let attrs = b
            .current_font
            .as_ref()
            .expect("current_font must still be installed")
            .copy_descriptor()
            .attributes()
            .to_untyped();
        let cascade_key = unsafe { CFString::wrap_under_get_rule(kCTFontCascadeListAttribute) };
        assert!(
            attrs.contains_key(&cascade_key.as_CFTypeRef()),
            "set_nerd_font_fallback must retroactively apply to an already-installed \
             current_font"
        );
    }

    /// Garbage bytes aren't a font Core Graphics can parse —
    /// `register_font_from_memory` must report that as `None`.
    #[test]
    fn mac_backend_register_font_from_memory_rejects_bytes_that_are_not_a_font() {
        let mut b = MacBackend::new();
        let garbage = [0u8; 64];
        assert!(
            b.register_font_from_memory(&garbage).is_none(),
            "64 zero bytes are not a parseable font — must report None, not a fabricated family"
        );
    }

    #[test]
    fn mac_register_text_region_and_begin_frame_clears_it() {
        let mut b = MacBackend::new();
        b.register_text_region(text_region("r", 0.0, 0.0, 100.0, 50.0, vec![]));
        assert_eq!(b.text_regions().len(), 1);
        b.begin_frame(Viewport::new(200.0, 100.0, 1.0));
        assert!(
            b.text_regions().is_empty(),
            "begin_frame must clear the previous frame's registered regions"
        );
    }

    #[test]
    fn mac_clear_selection_display_clears_active_selection_only() {
        let mut b = MacBackend::new();
        assert!(b.active_text_selection().is_none());
        b.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(50.0, 20.0),
        );
        assert!(b.active_text_selection().is_some());
        b.clear_selection_display();
        assert!(b.active_text_selection().is_none());
    }

    #[test]
    fn mac_clear_text_selection_also_ends_a_text_selection_drag() {
        let mut b = MacBackend::new();
        b.drag_state
            .borrow_mut()
            .begin(crate::dispatch::DragTarget::TextSelection {
                region: WidgetId::new("r"),
                anchor: Point::new(0.0, 0.0),
            });
        b.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
        );
        b.clear_text_selection();
        assert!(b.active_text_selection().is_none());
        assert!(
            !b.drag_state.borrow().is_active(),
            "clear_text_selection must also end an in-progress TextSelection drag"
        );
    }

    #[test]
    fn mac_cancel_text_selection_drag_preserves_the_displayed_selection() {
        let mut b = MacBackend::new();
        b.drag_state
            .borrow_mut()
            .begin(crate::dispatch::DragTarget::TextSelection {
                region: WidgetId::new("r"),
                anchor: Point::new(0.0, 0.0),
            });
        b.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(10.0, 10.0),
        );
        b.cancel_text_selection_drag_impl();
        assert!(!b.drag_state.borrow().is_active());
        assert!(
            b.active_text_selection().is_some(),
            "cancel_text_selection_drag must not clear the displayed selection"
        );
    }

    #[test]
    fn mac_select_all_text_region_targets_the_sole_region() {
        let mut b = MacBackend::new();
        b.register_text_region(text_region("body", 0.0, 0.0, 10.0, 5.0, vec![]));
        assert!(b.select_all_text_region());
        let sel = b
            .active_text_selection()
            .expect("selection should be active after select_all_text_region");
        assert_eq!(sel.anchor, Point::new(0.0, 0.0));
        assert_eq!(sel.focus, Point::new(10.0, 5.0));
    }

    #[test]
    fn mac_select_all_text_region_returns_false_with_no_regions() {
        let mut b = MacBackend::new();
        assert!(!b.select_all_text_region());
        assert!(b.active_text_selection().is_none());
    }

    #[test]
    fn mac_extract_selection_text_single_row() {
        let mut b = MacBackend::new();
        b.set_current_font(font());
        let line = "The quick brown fox jumps over the lazy dog.";
        // Size the region from the backend's *real* CoreText char_width
        // (Menlo 14pt) rather than a hardcoded pixel width — GTK's/Win's
        // equivalent tests get to assume a synthetic fixed char_width;
        // macOS's is whatever CoreText actually measures. A couple of
        // cells of slack past the line's length keeps `end_col`'s clamp
        // (see `text_selection_line_range`'s doc) from truncating it.
        let width = (line.chars().count() as f32 + 2.0) * b.char_width();
        b.register_text_region(text_region("body", 0.0, 0.0, width, 100.0, vec![line]));
        b.set_active_text_selection(
            WidgetId::new("body"),
            Point::new(0.0, 0.0),
            Point::new(width, 1.0),
        );
        let text = b.extract_selection_text();
        assert_eq!(text, line);
    }

    #[test]
    fn mac_extract_selection_text_empty_when_no_selection() {
        let b = MacBackend::new();
        assert_eq!(b.extract_selection_text(), "");
    }

    #[test]
    fn mac_extract_selection_text_empty_when_no_lines() {
        let mut b = MacBackend::new();
        b.set_current_font(font());
        b.register_text_region(text_region("body", 0.0, 0.0, 200.0, 32.0, vec![]));
        b.set_active_text_selection(
            WidgetId::new("body"),
            Point::new(0.0, 0.0),
            Point::new(200.0, 32.0),
        );
        assert_eq!(
            b.extract_selection_text(),
            "",
            "a region with no `lines` content has nothing to extract"
        );
    }

    /// `apply_selection_highlight` paints a translucent blue overlay via
    /// the blend `bg + 0.30 * (highlight - bg)`. The probe column is derived
    /// from the painted `StatusBarLayout` (just past the text segment's right
    /// edge, where no glyph reaches) rather than hardcoded, because the blue
    /// rise otherwise may disappear into antialiasing noise when the text stem
    /// underneath is already bright.
    #[test]
    fn mac_apply_selection_highlight_paints_over_the_background() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::types::Color;

        const W: u32 = 200;
        const H: u32 = 32;
        const BG: Color = Color::rgb(10, 10, 10);
        const TEXT: &str = "hello world";

        let surface = BitmapSurface::new(W, H);
        let mut b = MacBackend::new();
        b.set_current_font(font());
        b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |bk| {
            let l = bk.draw_status_bar_interactive(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &StatusBar {
                    id: WidgetId::new("bg"),
                    left_segments: vec![StatusBarSegment {
                        text: TEXT.into(),
                        fg: Color::rgb(255, 255, 255),
                        bg: BG,
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                },
                &crate::InteractionState::new(),
            );
            *layout.borrow_mut() = Some(l);
        });
        let layout = layout.into_inner().expect("draw_status_bar ran");
        let seg = layout
            .visible_segments
            .first()
            .expect("the single left segment is visible in a 200pt-wide bar");

        // Glyph-free background column: two points past the text
        // segment's right edge, still inside the bar's full-width fill.
        let probe_x = (seg.bounds.x + seg.bounds.width) as u32 + 2;
        assert!(
            probe_x < W,
            "probe column {probe_x} must stay inside the {W}pt surface — the \
             '{TEXT}' segment measured wider than expected ({seg:?})"
        );
        let probe_y = 2;
        // A row below the single selected row: still inside the surface
        // but outside the highlight, so it proves the fill is clipped to
        // the selection instead of flooding the region.
        let line_h = b.line_height();
        let unselected_y = line_h as u32 + 2;
        assert!(
            unselected_y < H,
            "the unselected probe row {unselected_y} must stay inside the \
             {H}pt surface (line_height = {line_h})"
        );

        b.register_text_region(text_region("bg", 0.0, 0.0, W as f32, H as f32, vec![TEXT]));
        b.set_active_text_selection(
            WidgetId::new("bg"),
            Point::new(0.0, 0.0),
            Point::new(W as f32, 1.0),
        );

        // Sample before painting the highlight, then paint it and sample
        // again — same probe pixels, so the diff isolates exactly what
        // `apply_selection_highlight` changed.
        let before = surface.pixel(probe_x, probe_y);
        let before_unselected = surface.pixel(probe_x, unselected_y);
        assert_eq!(
            before,
            (BG.r, BG.g, BG.b, 255),
            "the probe pixel must sit on the bar's flat background fill, not on \
             a glyph — otherwise the blend below is measured against the wrong base"
        );
        b.enter_frame_scope(surface.context_ptr(), |bk| {
            bk.apply_selection_highlight();
        });
        let after = surface.pixel(probe_x, probe_y);
        let after_unselected = surface.pixel(probe_x, unselected_y);

        assert_ne!(
            before, after,
            "apply_selection_highlight must paint over the background at a selected pixel"
        );
        // i32 arithmetic so an unexpected sample can never overflow a u8
        // and panic *before* the assertion message gets to print it.
        let (ar, ag, ab) = (after.0 as i32, after.1 as i32, after.2 as i32);
        assert!(
            ab > before.2 as i32 + 20,
            "the highlight is translucent blue, so the blue channel must rise \
             noticeably at a selected pixel: before={before:?} after={after:?}"
        );
        assert!(
            ab > ar + 20 && ab > ag + 20,
            "blue must end up the dominant channel after a translucent-blue \
             highlight: after={after:?}"
        );
        assert_eq!(
            before_unselected, after_unselected,
            "the highlight must be clipped to the selected row — row at y={unselected_y} \
             is outside the one-row selection and must be untouched"
        );
    }

    // ── PaintSurface (#807, Phase 1) ────────────────────────────────
    //
    // Unlike the GTK/Windows twins of these two tests, these only run on
    // a real Mac — this whole module (`mod macos` in `lib.rs`) is gated
    // `#[cfg(all(feature = "macos", target_os = "macos"))]`, so there is
    // no "type-checks on Linux, runs for real on CI" split to call out
    // here the way `win_backend_paint_surface_verbs_do_not_panic` does:
    // if this file compiles at all, it's on `macos.yml`'s `macos-latest`
    // runner, and these tests run there for real, pixels and all.

    #[test]
    fn mac_backend_paint_surface_fill_rect_paints_solid_color() {
        use super::super::headless::BitmapSurface;
        use crate::types::Color;

        const W: u32 = 32;
        const H: u32 = 32;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());

        let red = Color::rgb(200, 20, 20);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), red);
        });
        backend.end_frame();

        let (r, g, b, _a) = surface.pixel(5, 5);
        assert_eq!(
            (r, g, b),
            (red.r, red.g, red.b),
            "surface_fill_rect must paint the solid color it was given"
        );
    }

    /// Exercise every `PaintSurface` method at least once end-to-end
    /// (frame lifecycle, measurement, stroke, line, clip push/pop, text run,
    /// image) so implementation bugs fail a test instead of shipping silently.
    /// Before this test existed, no code called `MacBackend::surface_*` methods,
    /// so the hand-written CoreGraphics FFI had no pixel-level coverage.
    #[test]
    fn mac_backend_paint_surface_verbs_do_not_panic() {
        use super::super::headless::BitmapSurface;
        use crate::types::Color;

        const W: u32 = 64;
        const H: u32 = 64;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());

        let viewport = Viewport::new(W as f32, H as f32, 1.0);
        backend.surface_begin_frame(viewport);
        assert_eq!(
            backend.surface_viewport(),
            viewport,
            "surface_viewport must forward to Backend::begin_frame's stored value"
        );
        // `self` as a plain `PaintSurface` is the chrome surface (see
        // `Self::surface_measure_text`'s doc), so its grid metrics are
        // `chrome_line_height`/`chrome_char_width`, not the editor
        // `current_line_height`/`current_char_width` `font()` installed
        // above — the two deliberately differ here so this assertion
        // can't pass by coincidence.
        assert_eq!(
            backend.surface_line_height(),
            backend.chrome_line_height as f32
        );
        assert_eq!(
            backend.surface_char_width(),
            backend.chrome_char_width as f32
        );

        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let blue = Color::rgb(20, 20, 200);
            let white = Color::rgb(255, 255, 255);
            b.surface_stroke_rect(Rect::new(0.0, 0.0, 40.0, 40.0), blue, 2.0);
            b.surface_draw_line(Point::new(0.0, 0.0), Point::new(40.0, 40.0), blue, 1.0);
            b.surface_push_clip(Rect::new(0.0, 0.0, 40.0, 40.0));
            b.surface_draw_text_run(Rect::new(2.0, 2.0, 30.0, 10.0), "hi", white);
            b.surface_pop_clip();

            let (w, h) = b.surface_measure_text("hi");
            assert!(
                w > 0.0 && h > 0.0,
                "surface_measure_text must report a nonzero footprint for \
                 non-empty text: got ({w}, {h})"
            );

            let image = crate::primitives::image::Image {
                id: WidgetId::new("test:native-surface-image"),
                source: crate::primitives::image::ImageSource::Bytes(Vec::new()),
                intrinsic_size: Some((8, 8)),
                fit: crate::primitives::image::ImageFit::Contain,
                fallback_text: "[i]".to_string(),
            };
            // Empty bytes have nothing to decode, so this stays `Unsupported`
            // even with a real decoder. This call proves it reaches the same code
            // `Backend::draw_image` does (inside a live frame scope).
            let _ = b.surface_draw_image(Rect::new(0.0, 40.0, 8.0, 8.0), &image);
        });

        backend.surface_end_frame();
    }

    // ── #1073: surface_fill_rounded_rect / surface_fill_rect_alpha /
    //    surface_draw_text_run_with_role / surface_draw_icon_glyph ──────

    /// A corner pixel well inside the fillet radius must stay untouched
    /// while the box's centre paints solid — the `ns_fill_rounded_rect`
    /// twin of `mac_backend_paint_surface_fill_rect_paints_solid_color`
    /// above.
    #[test]
    fn mac_backend_paint_surface_fill_rounded_rect_clips_the_corners() {
        use super::super::headless::BitmapSurface;
        use crate::types::Color;

        const W: u32 = 40;
        const H: u32 = 40;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());

        let red = Color::rgb(200, 20, 20);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_fill_rounded_rect(Rect::new(0.0, 0.0, W as f32, H as f32), 15.0, red);
        });
        backend.end_frame();

        let (r, g, b, _a) = surface.pixel(20, 20);
        assert_eq!(
            (r, g, b),
            (red.r, red.g, red.b),
            "surface_fill_rounded_rect must paint the solid color at the box's centre"
        );
        let (r, g, b, _a) = surface.pixel(1, 1);
        assert_eq!(
            (r, g, b),
            (0, 0, 0),
            "a corner pixel well inside a radius-15 fillet on a 40x40 box must stay \
             untouched — otherwise this is just `surface_fill_rect` under a new name"
        );
    }

    /// A translucent `surface_fill_rect_alpha` fill over an opaque
    /// background must land a real alpha composite — neither the fill
    /// colour verbatim nor the background untouched.
    #[test]
    fn mac_backend_paint_surface_fill_rect_alpha_blends_with_the_background() {
        use super::super::headless::BitmapSurface;
        use crate::types::Color;

        const W: u32 = 40;
        const H: u32 = 40;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());

        let white = Color::rgb(255, 255, 255);
        let red = Color::rgb(200, 20, 20);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), white);
            b.surface_fill_rect_alpha(Rect::new(0.0, 0.0, W as f32, H as f32), red, 0.5);
        });
        backend.end_frame();

        let (r, g, b, _a) = surface.pixel(20, 20);
        assert!(
            (r, g, b) != (white.r, white.g, white.b) && (r, g, b) != (red.r, red.g, red.b),
            "a 50%-alpha fill over an opaque background must land a real blend, \
             not the background or the fill colour verbatim: got ({r}, {g}, {b})"
        );
        assert!(
            (200..245).contains(&r),
            "red channel should sit between the fill's 200 and white's 255: got {r}"
        );
        assert!(
            (100..160).contains(&g) && (100..160).contains(&b),
            "green/blue channels should sit roughly halfway between the fill's 20 \
             and white's 255: got ({g}, {b})"
        );
    }

    /// `role` must select a genuinely different live font — proven by a
    /// huge chrome-font size versus a tiny editor-font size painting
    /// visibly different amounts of ink for the same glyph.
    #[test]
    fn mac_backend_paint_surface_draw_text_run_with_role_uses_the_requested_fonts_size() {
        use super::super::headless::BitmapSurface;
        use crate::types::Color;

        fn ink_pixel_count(surface: &BitmapSurface) -> u32 {
            let mut count = 0;
            for y in 0..80 {
                for x in 0..80 {
                    let (r, g, b, _a) = surface.pixel(x, y);
                    if r as u32 + g as u32 + b as u32 > 0 {
                        count += 1;
                    }
                }
            }
            count
        }

        const W: u32 = 80;
        const H: u32 = 80;
        let white = Color::rgb(255, 255, 255);

        let chrome_surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(
            super::super::text::make_font("Menlo", 6.0).expect("Menlo installed"),
        );
        backend.set_chrome_font(
            super::super::text::make_font("Menlo", 48.0).expect("Menlo installed"),
        );
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(chrome_surface.context_ptr(), |b| {
            b.surface_draw_text_run_with_role(
                Rect::new(2.0, 2.0, 76.0, 76.0),
                "A",
                white,
                crate::FontRole::Chrome,
                false,
            );
        });
        backend.end_frame();
        let chrome_ink = ink_pixel_count(&chrome_surface);

        let editor_surface = BitmapSurface::new(W, H);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(editor_surface.context_ptr(), |b| {
            b.surface_draw_text_run_with_role(
                Rect::new(2.0, 2.0, 76.0, 76.0),
                "A",
                white,
                crate::FontRole::Editor,
                false,
            );
        });
        backend.end_frame();
        let editor_ink = ink_pixel_count(&editor_surface);

        assert!(
            chrome_ink > 0 && editor_ink > 0,
            "both roles must paint real ink: chrome={chrome_ink}, editor={editor_ink}"
        );
        assert!(
            chrome_ink > editor_ink * 3,
            "a 48pt chrome font must paint far more ink than a 6pt editor font for the \
             same glyph if `role` really selects a different live font: \
             chrome={chrome_ink}, editor={editor_ink}"
        );

        // Italic must not panic.
        let italic_surface = BitmapSurface::new(W, H);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(italic_surface.context_ptr(), |b| {
            b.surface_draw_text_run_with_role(
                Rect::new(2.0, 2.0, 76.0, 76.0),
                "A",
                white,
                crate::FontRole::Chrome,
                true,
            );
        });
        backend.end_frame();
    }

    /// `surface_draw_icon_glyph` must reach real CoreGraphics painting,
    /// not silently drop the call.
    #[test]
    fn mac_backend_paint_surface_draw_icon_glyph_paints_real_ink() {
        use super::super::headless::BitmapSurface;
        use crate::types::Color;

        const W: u32 = 40;
        const H: u32 = 40;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());

        let white = Color::rgb(255, 255, 255);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_draw_icon_glyph(Rect::new(2.0, 2.0, 30.0, 30.0), "i", white);
        });
        backend.end_frame();

        let mut ink = 0u32;
        for y in 0..H {
            for x in 0..W {
                let (r, g, b, _a) = surface.pixel(x, y);
                ink += r as u32 + g as u32 + b as u32;
            }
        }
        assert!(
            ink > 0,
            "surface_draw_icon_glyph must paint real ink, not silently no-op"
        );
    }

    // ── Form (#808, PaintSurface Phase 2a) ──────────────────────────

    /// `macos::form::draw_form` previously matched only 10 of 14
    /// `FieldKind` variants and silently fell through on `Slider` /
    /// `ColorPicker` / `Dropdown` / `TextArea`, painting nothing beyond
    /// label and row background. It now routes through the shared
    /// `primitives::form::paint`. This scans each field's row past where
    /// its label ends for a pixel that isn't the row background.
    #[test]
    fn mac_backend_draw_form_paints_the_four_field_kinds_macos_used_to_drop() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::form::{FieldKind, Form, FormField};
        use crate::types::StyledText;

        const W: u32 = 320;
        const H: u32 = 160;

        fn field(id: &str, label: &str, kind: FieldKind) -> FormField {
            FormField {
                id: WidgetId::new(id),
                label: StyledText::plain(label),
                kind,
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }
        }

        let form = Form {
            id: WidgetId::new("regression-808"),
            fields: vec![
                field(
                    "font-size",
                    "Font size",
                    FieldKind::Slider {
                        value: 14.0,
                        min: 8.0,
                        max: 32.0,
                        step: 1.0,
                    },
                ),
                field(
                    "accent",
                    "Accent",
                    FieldKind::ColorPicker {
                        value: Color::rgb(0x7a, 0xb4, 0xff),
                    },
                ),
                field(
                    "theme",
                    "Theme",
                    FieldKind::Dropdown {
                        options: vec![
                            StyledText::plain("One Dark"),
                            StyledText::plain("Solarized"),
                        ],
                        selected_idx: 1,
                    },
                ),
                field(
                    "notes",
                    "Notes",
                    FieldKind::TextArea {
                        value: "release notes".into(),
                        placeholder: String::new(),
                        cursor: None,
                        visible_rows: 3,
                    },
                ),
            ],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        // From `Backend::form_layout`, the no-paint twin `draw_form` is
        // contractually required to agree with, rather than a local
        // `mac_form_layout` call with a font of this test's own
        // choosing: a `Form` is chrome, so the paint measures its
        // per-field rows in `chrome_font`, and re-deriving them here
        // against the editor face installed above would drift every
        // row-relative probe below by the two advances' difference.
        // Same reasoning as `macos::form::tests::paint_via_backend`.
        let flayout = backend.form_layout(rect, &form);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_form(rect, &form);
        });
        backend.end_frame();

        let theme = Theme::default();
        let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);

        for vf in &flayout.visible_fields {
            let field = &form.fields[vf.field_idx];
            let x0 = (vf.bounds.x + vf.bounds.width * 0.6) as u32;
            let x1 = ((vf.bounds.x + vf.bounds.width) as u32).min(W);
            let y0 = vf.bounds.y as u32;
            let y1 = ((vf.bounds.y + vf.bounds.height) as u32).min(H);

            let mut painted_beyond_label = false;
            'scan: for y in y0..y1 {
                for x in x0..x1 {
                    let (r, g, b, _a) = surface.pixel(x, y);
                    if (r, g, b) != bg {
                        painted_beyond_label = true;
                        break 'scan;
                    }
                }
            }
            assert!(
                painted_beyond_label,
                "field {:?} ({:?}) must paint something in the right ~40% of its \
                 row (past the label) — this is exactly the #808 silent-drop bug: \
                 pre-fix, macos::form::draw_form's `_ => {{}}` matched Slider/\
                 ColorPicker/Dropdown/TextArea and painted only the row background \
                 there",
                field.id, field.kind,
            );
        }
    }

    // ── find_replace (#809, `PaintSurface` Phase 2b) ──────────────────
    //
    // `macos::find_replace` used to carry its own `#[cfg(test)]` module
    // with `panel_paints_surface_bg`, `panel_with_multibyte_query_does_not_panic`
    // and `hit_regions_present_for_basic_panel`, exercising
    // `macos::find_replace::draw_find_replace` against a real
    // `BitmapSurface`/Core Text stack. That module (and the whole file)
    // is deleted — the paint logic it tested now lives in
    // `crate::primitives::find_replace::paint`, already covered on every
    // host by that module's own `RecordingSurface` tests. These two
    // tests are the twin that stays macOS-only for a reason: they prove
    // the *plumbing* — `MacBackend::draw_find_replace` really does reach
    // `paint` and `paint` really does land real Core Text pixels through
    // `MacBackend`'s `PaintSurface` impl — not the paint logic itself.

    fn find_replace_sample_panel(w: f32, h: f32) -> FindReplacePanel {
        let (hit_regions, _input_width) =
            crate::primitives::find_replace::compute_hit_regions(50, false, "1 of 3", 2, 2);
        FindReplacePanel {
            query: "needle".into(),
            replacement: String::new(),
            show_replace: false,
            focus: 0,
            cursor: 6,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: crate::event::Rect::new(0.0, 0.0, w, h),
            panel_width: 50,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions,
        }
    }

    /// End-to-end pixel probe: `Backend::draw_find_replace` on a real
    /// `MacBackend`, over a headless `BitmapSurface`, must actually paint
    /// the popup background — proving `MacBackend`'s `PaintSurface`
    /// impl really reaches Core Graphics, not just that the shared
    /// painter emits the right verb (the primitive-level
    /// `RecordingSurface` test already covers that half, portably).
    ///
    /// Both fonts are pinned to the same face and size, so the popup
    /// rect this test re-derives is a fixed, host-independent
    /// geometry. `primitives::find_replace::paint` sizes the panel on
    /// the `PaintSurface` grid, which this backend reports in the
    /// chrome font (see [`MacBackend::surface_line_height`]'s doc);
    /// leaving chrome on the CoreText system-UI default would make the
    /// probe coordinates below depend on whatever that face happens to
    /// measure on the host. Which font the panel paints in is
    /// `tests/macos_font_role.rs`'s subject, not this test's — here the
    /// only claim is that the fill lands real Core Graphics pixels.
    #[test]
    fn mac_backend_draw_find_replace_paints_popup_background() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 600;
        const H: u32 = 200;

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_chrome_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let panel = find_replace_sample_panel(W as f32, H as f32);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_find_replace(
                crate::event::Rect::new(0.0, 0.0, W as f32, H as f32),
                &panel,
            );
        });
        backend.end_frame();

        let theme = crate::theme::Theme::default();
        // The popup's geometry comes from the metrics the paint itself
        // reads — `MacBackend`'s `PaintSurface` grid, which is the chrome
        // advance and line height — not `Backend::char_width`/
        // `line_height`, which report the editor font's metrics.
        let cw = PaintSurface::surface_char_width(&backend).max(1.0);
        let lh = PaintSurface::surface_line_height(&backend).max(1.0);
        let popup_w = panel.panel_width as f32 * cw;
        let popup_h = 3.0 * lh; // show_replace == false: 1 content row + 2 border rows
        let popup_x = (panel.group_bounds.x + panel.group_bounds.width - popup_w - 10.0)
            .max(panel.group_bounds.x);
        let popup_y = panel.group_bounds.y + 2.0;

        // A few px inside the popup's bottom-right corner: past the
        // border stroke and below the single (`row == 0`) content row,
        // so nothing but the background fill can have painted here.
        let probe_x = (popup_x + popup_w - 4.0) as u32;
        let probe_y = (popup_y + popup_h - 4.0) as u32;
        assert!(
            probe_x < W
                && probe_y < H
                && (probe_x as f32) >= popup_x
                && (probe_y as f32) >= popup_y,
            "test setup: the probe ({probe_x}, {probe_y}) must sit inside both the \
             {W}×{H} surface and the popup ({popup_w}×{popup_h} at ({popup_x}, \
             {popup_y}), chrome advance {cw}, line height {lh})",
        );
        let (r, g, b, _a) = surface.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
            "find/replace popup background must be painted through MacBackend's \
             real PaintSurface impl — probed ({probe_x}, {probe_y}) inside the \
             popup ({popup_w}×{popup_h} at ({popup_x}, {popup_y}), chrome advance \
             {cw}, line height {lh})",
        );
    }

    /// Regression twin of `primitives::find_replace`'s own
    /// `multibyte_query_with_out_of_range_selection_does_not_panic`:
    /// that test proves the shared slicing logic is safe in the
    /// abstract; this proves real Core Text measurement/drawing of the
    /// same multibyte text through `MacBackend` doesn't panic either.
    #[test]
    fn mac_backend_draw_find_replace_with_multibyte_query_does_not_panic() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 600;
        const H: u32 = 200;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let mut panel = find_replace_sample_panel(W as f32, H as f32);
        panel.query = "café🎉中文".into();
        panel.cursor = 3;
        panel.sel_anchor = Some(6);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_find_replace(
                crate::event::Rect::new(0.0, 0.0, W as f32, H as f32),
                &panel,
            );
        });
        backend.end_frame();
    }

    // ── #1093: draw_text_input real-pixel driver test ────────────────────
    //
    // Regression for #1093: pre-fix, `MacBackend::draw_text_input` was a
    // bare layout-only stub — `ti.layout(..)` and nothing else — so a
    // text input was completely invisible on macOS: no background, no
    // value, no cursor, and no `BackendCaps` flag said so. Confirmed to
    // reproduce (both assertions below fail) against the unfixed stub.
    // `draw_text_input` now routes through the shared
    // `primitives::text_input::paint` via this backend's own
    // `PaintSurface` impl — this drives that real Core Text/CoreGraphics
    // path end to end, the same "acceptance bar" shape
    // `mac_backend_draw_find_replace_paints_popup_background` and
    // `mac_backend_draw_form_paints_the_four_field_kinds_macos_used_to_drop`
    // use above.
    #[test]
    fn mac_backend_draw_text_input_paints_value_and_cursor() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::text_input::TextInput;

        const W: u32 = 200;
        const H: u32 = 40;

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let mut ti = TextInput::new(WidgetId::new("regression-1093"));
        ti.lines = vec!["hello".to_string()];
        ti.cursor_line = 0;
        ti.cursor_col = 3;
        ti.has_focus = true;

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout_cell = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_text_input(rect, &ti);
            *layout_cell.borrow_mut() = Some(l);
        });
        backend.end_frame();
        let layout = layout_cell.into_inner().expect("draw_text_input ran");

        let theme = Theme::default();
        let bg = (theme.background.r, theme.background.g, theme.background.b);

        // "text_ok": some non-background pixel painted inside the
        // content area — proves Core Text actually ran, not just the
        // background/border.
        let cb = layout.content_bounds;
        let mut painted_any = false;
        'scan: for y in (cb.y as u32)..(((cb.y + cb.height) as u32).min(H)) {
            for x in (cb.x as u32)..(((cb.x + 30.0) as u32).min(W)) {
                let (r, g, b, _a) = surface.pixel(x, y);
                if (r, g, b) != bg {
                    painted_any = true;
                    break 'scan;
                }
            }
        }
        assert!(
            painted_any,
            "expected draw_text_input to paint the value's glyphs, not just \
             background/border — this is exactly #1093's silent-stub bug",
        );

        // The insert cursor must paint at exactly `cursor_bounds` — the
        // same rect a host reads back for IME/caret placement.
        let cb2 = layout.cursor_bounds.expect("cursor visible when has_focus");
        let px = (cb2.x + 1.0) as u32;
        let py = (cb2.y + cb2.height / 2.0) as u32;
        let (r, g, b, _a) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "expected the insert cursor to paint at cursor_bounds in theme.cursor",
        );
    }

    /// `TextInput` paints through plain `self`, which resolves
    /// `chrome_font` for its glyphs — so its caret grid has to use
    /// `chrome_char_width`, not `current_char_width`, or the caret lands
    /// somewhere other than where the glyphs it sits between actually
    /// are. Installs a Menlo editor font and a deliberately much
    /// narrower Menlo chrome font so the two advances can't agree by
    /// coincidence, then checks `cursor_bounds.x` against each.
    #[test]
    fn text_input_layout_cursor_grid_uses_chrome_char_width_not_editor() {
        let mut backend = MacBackend::new();
        backend.set_current_font(font()); // Menlo 14pt — a wide editor advance.
        backend.set_ui_font("Menlo 6"); // Menlo 6pt — a narrow chrome advance.
        assert!(
            (backend.chrome_char_width - backend.current_char_width).abs() > 1.0,
            "chrome and editor char widths must differ enough that a grid built from \
             the wrong one can't pass by coincidence: chrome={}, editor={}",
            backend.chrome_char_width,
            backend.current_char_width,
        );

        let mut ti =
            crate::primitives::text_input::TextInput::new(WidgetId::new("chrome-grid-regression"));
        ti.lines = vec!["hello".to_string()];
        ti.cursor_line = 0;
        ti.cursor_col = 3;
        ti.has_focus = true;

        let rect = Rect::new(0.0, 0.0, 200.0, 40.0);
        let layout = backend.text_input_layout(rect, &ti);
        let cursor = layout.cursor_bounds.expect("cursor visible when has_focus");

        let expected_x = layout.content_bounds.x + 3.0 * backend.chrome_char_width as f32;
        let wrong_x = layout.content_bounds.x + 3.0 * backend.current_char_width as f32;
        assert!(
            (cursor.x - expected_x).abs() < 0.01,
            "cursor_bounds.x must sit on the chrome_char_width grid \
             `draw_text_input` actually paints glyphs on: got {}, expected {expected_x} \
             (the current_char_width grid would have put it at {wrong_x})",
            cursor.x,
        );
    }

    // ── #810: draw_chart real-pixel driver tests ────────────────────────
    //
    // Ported from the deleted `macos::chart::tests` (that module already
    // drove the real `MacBackend::draw_chart` via its own local
    // `paint_via_backend` helper — the shape every other backend's #810
    // migration now also uses) to live alongside every other
    // `draw_*`-family driver test in this module.

    use super::super::headless::BitmapSurface;
    use crate::primitives::chart::{
        Chart as ChartModel, ChartHit, ChartKind, Series, SERIES_COLORS,
    };

    const CHART_W: u32 = 200;
    const CHART_H: u32 = 120;

    fn chart_sample_line() -> ChartModel {
        ChartModel {
            id: WidgetId::new("ch"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "x".into(),
                data: vec![0.0, 1.0, 0.5, 2.0, 1.0],
                color: Some(Color::rgb(80, 160, 255)),
                fill: false,
            }],
            x_label: None,
            y_label: None,
            y_range: Some((0.0, 2.0)),
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
        }
    }

    fn paint_chart_via_backend(
        chart: &ChartModel,
        hovered: Option<(usize, usize)>,
    ) -> (BitmapSurface, ChartLayout) {
        let surface = BitmapSurface::new(CHART_W, CHART_H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(CHART_W as f32, CHART_H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_chart(
                Rect::new(0.0, 0.0, CHART_W as f32, CHART_H as f32),
                chart,
                hovered,
                None,
            );
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn mac_backend_draw_chart_plot_area_paints_background() {
        let chart = chart_sample_line();
        let (surface, layout) = paint_chart_via_backend(&chart, None);
        let theme = Theme::default();
        let px = (layout.plot_area.x + layout.plot_area.width - 2.0) as u32;
        let py = (layout.plot_area.y + 2.0) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
        );
    }

    #[test]
    fn mac_backend_draw_chart_line_stroke_paints_series_color() {
        let chart = chart_sample_line();
        let (surface, layout) = paint_chart_via_backend(&chart, None);
        // Sample at data point index 1 (value 1.0 in [0..2] range = mid
        // plot, comfortably inside bounds — first and last points sit
        // on the plot edges).
        let (_, _, sx, sy) = layout.data_point_positions[1];
        let (r, g, b, _) = surface.pixel(sx as u32, sy as u32);
        assert!(
            b > r,
            "expected blue dominant at data point 1 ({}, {}), got ({}, {}, {})",
            sx as u32,
            sy as u32,
            r,
            g,
            b,
        );
    }

    #[test]
    fn mac_backend_draw_chart_hit_test_inside_plot_area_returns_body() {
        let chart = chart_sample_line();
        let (_surface, layout) = paint_chart_via_backend(&chart, None);
        let cx = layout.plot_area.x + layout.plot_area.width * 0.5;
        let cy = layout.plot_area.y + layout.plot_area.height * 0.5;
        assert_eq!(layout.hit_test(cx, cy), ChartHit::Body(WidgetId::new("ch")),);
    }

    #[test]
    fn mac_backend_draw_chart_hover_marker_painted_when_set() {
        let chart = chart_sample_line();
        let (surface, layout) = paint_chart_via_backend(&chart, Some((0, 2)));
        let (_, _, sx, sy) = layout.data_point_positions[2];
        let (r, _, b, _) = surface.pixel(sx as u32, sy as u32);
        assert!(b > r);
    }

    // ── Multi-series bars (#584 review — macOS had no bar coverage) ──────

    fn chart_bar_series(label: &str, data: Vec<f64>) -> Series {
        Series {
            label: label.into(),
            data,
            color: None,
            fill: false,
        }
    }

    fn chart_sample_bar(kind: ChartKind, series: Vec<Series>, y_range: (f64, f64)) -> ChartModel {
        ChartModel {
            id: WidgetId::new("ch"),
            kind,
            series,
            x_label: None,
            y_label: None,
            y_range: Some(y_range),
            x_range: None,
            show_legend: false,
            y_ticks: Some(0),
            x_ticks: Some(0),
            show_grid: false,
        }
    }

    #[test]
    fn mac_backend_draw_chart_stacked_bar_paints_each_series_in_its_own_color() {
        let chart = chart_sample_bar(
            ChartKind::Bar,
            vec![
                chart_bar_series("a", vec![1.0]),
                chart_bar_series("b", vec![1.0]),
                chart_bar_series("c", vec![1.0]),
            ],
            (0.0, 3.0),
        );
        let (surface, _layout) = paint_chart_via_backend(&chart, None);

        // slot_w = pw = 200, gap = 30, bar_w = 170, slot_x = 15 → mid-x = 100.
        let mid_x = 100;
        let (r0, g0, b0, _) = surface.pixel(mid_x, 100);
        assert_eq!(
            (r0, g0, b0),
            (SERIES_COLORS[0].r, SERIES_COLORS[0].g, SERIES_COLORS[0].b)
        );
        let (r1, g1, b1, _) = surface.pixel(mid_x, 60);
        assert_eq!(
            (r1, g1, b1),
            (SERIES_COLORS[1].r, SERIES_COLORS[1].g, SERIES_COLORS[1].b)
        );
        let (r2, g2, b2, _) = surface.pixel(mid_x, 20);
        assert_eq!(
            (r2, g2, b2),
            (SERIES_COLORS[2].r, SERIES_COLORS[2].g, SERIES_COLORS[2].b)
        );
    }

    #[test]
    fn mac_backend_draw_chart_grouped_bar_paints_series_side_by_side() {
        let chart = chart_sample_bar(
            ChartKind::BarGrouped,
            vec![
                chart_bar_series("a", vec![1.0]),
                chart_bar_series("b", vec![2.0]),
                chart_bar_series("c", vec![3.0]),
            ],
            (0.0, 3.0),
        );
        let (surface, _layout) = paint_chart_via_backend(&chart, None);

        let probe_y = 100;
        let (r0, g0, b0, _) = surface.pixel(43, probe_y);
        assert_eq!(
            (r0, g0, b0),
            (SERIES_COLORS[0].r, SERIES_COLORS[0].g, SERIES_COLORS[0].b),
            "leftmost sub-bar should be series 0"
        );
        let (r1, g1, b1, _) = surface.pixel(100, probe_y);
        assert_eq!(
            (r1, g1, b1),
            (SERIES_COLORS[1].r, SERIES_COLORS[1].g, SERIES_COLORS[1].b),
            "middle sub-bar should be series 1"
        );
        let (r2, g2, b2, _) = surface.pixel(157, probe_y);
        assert_eq!(
            (r2, g2, b2),
            (SERIES_COLORS[2].r, SERIES_COLORS[2].g, SERIES_COLORS[2].b),
            "rightmost sub-bar should be series 2"
        );
    }

    /// `macos::chart` already clipped via `CGContextSaveGState`; the shared
    /// `primitives::chart::paint` now owns this clip for every pixel backend.
    /// This is the driver-tier proof for macOS that the hover marker stays
    /// within the chart rect.
    #[test]
    fn mac_backend_draw_chart_hover_marker_does_not_escape_chart_rect() {
        let chart = chart_sample_bar(
            ChartKind::Sparkline,
            vec![chart_bar_series("a", vec![10.0, 0.0])],
            (0.0, 10.0),
        );

        let canvas_w = 40;
        let canvas_h = 40;
        let sentinel = Color::rgb(0, 0, 0);
        let (chart_x, chart_y, chart_w, chart_h) = (10.0_f32, 10.0_f32, 20.0_f32, 20.0_f32);

        let surface = BitmapSurface::new(canvas_w, canvas_h);
        surface.fill(0.0, 0.0, 0.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(canvas_w as f32, canvas_h as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_fill_rect(
                Rect::new(0.0, 0.0, canvas_w as f32, canvas_h as f32),
                sentinel,
            );
            b.draw_chart(
                Rect::new(chart_x, chart_y, chart_w, chart_h),
                &chart,
                Some((0, 0)),
                None,
            );
        });
        backend.end_frame();

        // Inside the marker's 8-unit-radius ring, centred at the
        // chart's own top-left corner (10, 10), but outside the
        // chart's rect.
        let (r, g, b, _) = surface.pixel(5, 5);
        assert_eq!(
            (r, g, b),
            (sentinel.r, sentinel.g, sentinel.b),
            "hover marker must not paint outside the chart's own rect",
        );
    }

    /// `chart_layout` reserves the y-axis tick gutter and the
    /// x-tick/legend rows from cached font metrics, but every label that
    /// lands in those regions is painted by `primitives::chart::paint`
    /// through plain `self`, whose `PaintSurface` impl resolves
    /// `chrome_font`. So the reservation has to be measured in the chrome
    /// font too, or the labels overflow the space set aside for them.
    ///
    /// Pins a 24pt chrome font against a 5pt editor font — far enough
    /// apart that an editor-sized gutter cannot hold a chrome-painted
    /// label — paints the real chart, and checks the runs
    /// `MacBackend::text_runs` recorded against the regions
    /// `ChartLayout` reserved. Recorded bounds are the paint position
    /// plus the painting font's own measured footprint (see
    /// `macos::text::draw_text`'s recording hook), so each run's rect is
    /// exactly the ink the gutter and rows have to contain.
    #[test]
    fn mac_backend_chart_axis_label_space_is_reserved_in_chrome_metrics() {
        const CW: u32 = 320;
        const CH: u32 = 200;

        let chart = ChartModel {
            id: WidgetId::new("ch"),
            kind: ChartKind::Line,
            series: vec![Series {
                label: "SER".into(),
                data: vec![0.0, 20.0, 40.0, 60.0, 80.0],
                color: None,
                fill: false,
            }],
            x_label: None,
            y_label: None,
            // Tick labels no two of which collide across the two axes
            // ("0"/"40"/"80" against "1000"/"2000"/"3000"), so a run
            // lookup by text can only match the axis it belongs to.
            y_range: Some((0.0, 80.0)),
            x_range: Some((1000.0, 3000.0)),
            show_legend: true,
            y_ticks: Some(2),
            x_ticks: Some(2),
            show_grid: false,
        };

        let surface = BitmapSurface::new(CW, CH);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(super::super::text::make_font("Menlo", 5.0).expect("Menlo"));
        backend.set_chrome_font(super::super::text::make_font("Menlo", 24.0).expect("Menlo"));
        assert!(
            backend.chrome_char_width - backend.current_char_width > 1.0
                && backend.chrome_line_height - backend.current_line_height > 1.0,
            "chrome and editor metrics must differ enough that a layout built from the \
             editor's can't pass by coincidence: chrome=(cw {}, lh {}), editor=(cw {}, lh {})",
            backend.chrome_char_width,
            backend.chrome_line_height,
            backend.current_char_width,
            backend.current_line_height,
        );

        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(CW as f32, CH as f32, 1.0));
        let layout_cell = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_chart(
                Rect::new(0.0, 0.0, CW as f32, CH as f32),
                &chart,
                None,
                None,
            );
            *layout_cell.borrow_mut() = Some(l);
        });
        backend.end_frame();
        let layout = layout_cell.into_inner().expect("draw_chart ran");
        let runs = backend.text_runs();
        let run_for = |label: &str| {
            runs.iter()
                .find(|r| r.text == label)
                .unwrap_or_else(|| {
                    panic!(
                        "expected a painted run for the axis label {label:?}; painted: {:?}",
                        runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
                    )
                })
                .bounds
        };

        // The gutter is `char_width * (longest_label + 1)` — three
        // chrome advances here, since the widest y-tick label ("40",
        // "80") is two characters.
        let gutter = layout.plot_area.x - layout.bounds.x;
        let chrome_gutter = backend.chrome_char_width as f32 * 3.0;
        let editor_gutter = backend.current_char_width as f32 * 3.0;
        assert!(
            (gutter - chrome_gutter).abs() < 0.01,
            "the y-axis gutter must be sized in chrome_char_width, the advance the \
             tick labels actually paint in: got {gutter}, expected {chrome_gutter} \
             (the editor advance would have reserved {editor_gutter})",
        );

        // Every y-tick label is right-aligned to `plot_area.x - 4`, so
        // too narrow a gutter pushes its left edge outside the chart.
        assert!(
            !layout.y_tick_positions.is_empty(),
            "y_ticks: Some(2) must produce tick positions to label",
        );
        for &(_, val) in &layout.y_tick_positions {
            let label = crate::primitives::chart::format_tick_value(val);
            let rb = run_for(&label);
            assert!(
                rb.x >= layout.bounds.x - 0.5,
                "y-tick label {label:?} must fit inside the reserved gutter, not spill \
                 past the chart's left edge: run x={}, chart x={}",
                rb.x,
                layout.bounds.x,
            );
            assert!(
                rb.x + rb.width <= layout.plot_area.x + 0.5,
                "y-tick label {label:?} must stay left of the plot area it labels: run \
                 ends at {}, plot area starts at {}",
                rb.x + rb.width,
                layout.plot_area.x,
            );
        }

        // The x-tick row is one `line_height` tall and sits below the
        // plot area, so sizing it from the editor's pitch pushes a
        // chrome-painted tick label past the chart's bottom edge.
        assert!(
            !layout.x_tick_positions.is_empty(),
            "x_ticks: Some(2) must produce tick positions to label",
        );
        let chart_bottom = layout.bounds.y + layout.bounds.height;
        for &(_, val) in &layout.x_tick_positions {
            let label = crate::primitives::chart::format_tick_value(val);
            let rb = run_for(&label);
            assert!(
                rb.y + rb.height <= chart_bottom + 1.0,
                "x-tick label {label:?} must fit the reserved tick row inside the chart's \
                 own bounds: run bottom={}, chart bottom={chart_bottom}",
                rb.y + rb.height,
            );
        }

        // Same story one row up for the legend.
        let lb = layout
            .legend_bounds
            .expect("show_legend: true must reserve a legend row");
        assert!(
            (lb.height - backend.chrome_line_height as f32).abs() < 0.01,
            "the legend row must be one chrome line height tall: got {}, chrome={}, \
             editor={}",
            lb.height,
            backend.chrome_line_height,
            backend.current_line_height,
        );
        let legend_run = run_for("SER");
        assert!(
            legend_run.y + legend_run.height <= lb.y + lb.height + 1.0,
            "the legend label must fit the reserved legend row: run bottom={}, row \
             bottom={}",
            legend_run.y + legend_run.height,
            lb.y + lb.height,
        );
    }

    /// The shared `primitives::status_bar::native_surface_paint::paint`
    /// guards against zero/negative sizes; this proves the guard short-circuits
    /// through the real `MacBackend`. macOS already carried this pre-guard,
    /// so this is completeness coverage. CoreGraphics doesn't mirror Cairo's
    /// negative-size behaviour, so a literal zero width is tested.
    #[test]
    fn mac_backend_draw_status_bar_zero_width_does_not_paint() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::types::Color;

        const W: u32 = 40;
        const H: u32 = 40;
        let sentinel = Color::rgb(7, 8, 9);

        let surface = BitmapSurface::new(W, H);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.surface_fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), sentinel);
            b.draw_status_bar_interactive(
                Rect::new(10.0, 10.0, 0.0, 15.0),
                &StatusBar {
                    id: WidgetId::new("test:status-bar"),
                    left_segments: vec![StatusBarSegment {
                        text: "READY".into(),
                        fg: Color::rgb(255, 255, 255),
                        bg: Color::rgb(200, 0, 0),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                },
                &crate::InteractionState::new(),
            );
        });
        backend.end_frame();

        for y in 0..H {
            for x in 0..W {
                let (r, g, b, _) = surface.pixel(x, y);
                assert_eq!(
                    (r, g, b),
                    (sentinel.r, sentinel.g, sentinel.b),
                    "pixel ({x}, {y}) must stay untouched: a zero-width rect must \
                     short-circuit to the no-paint layout",
                );
            }
        }
    }

    // ── #810: draw_text_display real-pixel driver tests ─────────────────
    //
    // Ported from the deleted `macos::text_display::tests` (that module
    // already drove the real `MacBackend::draw_text_display` via its own
    // local `paint`/`paint_at` helpers) to live alongside every other
    // `draw_*`-family driver test in this module.

    use crate::primitives::text_display::{
        TextDisplay as TextDisplayModel, TextDisplayHit, TextDisplayLine as TextDisplayLineModel,
    };
    use crate::types::Decoration;

    const TD_W: u32 = 240;
    const TD_H: u32 = 160;

    fn td_line(text: &str) -> TextDisplayLineModel {
        TextDisplayLineModel {
            spans: vec![crate::types::StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn make_td(lines: usize, show_scrollbar: bool) -> TextDisplayModel {
        TextDisplayModel {
            id: WidgetId::new("td"),
            lines: (0..lines).map(|i| td_line(&format!("ln{i}"))).collect(),
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar,
        }
    }

    fn paint_td_via_backend(
        td: &TextDisplayModel,
        rect: Rect,
    ) -> (
        BitmapSurface,
        crate::primitives::text_display::TextDisplayLayout,
    ) {
        let surface = BitmapSurface::new(TD_W, TD_H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(TD_W as f32, TD_H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_text_display(rect, td);
            let l = b.text_display_layout(rect, td);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn mac_backend_draw_text_display_background_fills_theme_background() {
        let td = make_td(0, false);
        let (s, _) = paint_td_via_backend(&td, Rect::new(0.0, 0.0, TD_W as f32, TD_H as f32));
        let theme = Theme::default();
        let (r, g, b, _) = s.pixel(TD_W / 2, TD_H / 2);
        assert_eq!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
        );
    }

    #[test]
    fn mac_backend_draw_text_display_scrollbar_gutter_paints_track_colour() {
        let td = make_td(100, true);
        let (s, layout) = paint_td_via_backend(&td, Rect::new(0.0, 0.0, TD_W as f32, TD_H as f32));
        let gutter = layout.scrollbar_bounds.expect("gutter present");
        let probe_x = (gutter.x + gutter.width / 2.0) as u32;
        let probe_y = (gutter.y + gutter.height - 2.0) as u32;
        let (r, g, b, _) = s.pixel(probe_x, probe_y);
        let theme = Theme::default();
        assert_eq!(
            (r, g, b),
            (
                theme.scrollbar_track.r,
                theme.scrollbar_track.g,
                theme.scrollbar_track.b,
            ),
        );
    }

    #[test]
    fn mac_backend_draw_text_display_scrollbar_thumb_paints_thumb_colour() {
        let td = make_td(100, true);
        let (s, layout) = paint_td_via_backend(&td, Rect::new(0.0, 0.0, TD_W as f32, TD_H as f32));
        let thumb = layout.thumb_bounds.expect("thumb present");
        let probe_x = (thumb.x + thumb.width / 2.0) as u32;
        let probe_y = (thumb.y + thumb.height / 2.0) as u32;
        let (r, g, b, _) = s.pixel(probe_x, probe_y);
        let theme = Theme::default();
        assert_eq!(
            (r, g, b),
            (
                theme.scrollbar_thumb.r,
                theme.scrollbar_thumb.g,
                theme.scrollbar_thumb.b,
            ),
        );
    }

    #[test]
    fn mac_backend_draw_text_display_layout_hit_test_resolves_lines() {
        let td = make_td(20, false);
        let (_, layout) = paint_td_via_backend(&td, Rect::new(0.0, 0.0, TD_W as f32, TD_H as f32));
        let vis = &layout.visible_lines[0];
        let cx = vis.bounds.x + vis.bounds.width / 2.0;
        let cy = vis.bounds.y + vis.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, vis.line_idx),
            other => panic!("expected Line, got {:?}", other),
        }
    }

    /// Layout hit-test must resolve lines correctly at a non-zero rect
    /// origin with a title row present, round-tripping an absolute click
    /// the way a real host does.
    #[test]
    fn mac_backend_draw_text_display_layout_hit_test_resolves_lines_at_nonzero_origin() {
        let mut td = make_td(20, false);
        td.title = Some(crate::types::StyledText::plain("Logs"));

        let rect_x = 9.0_f32;
        let rect_y = 17.0_f32;
        let rect = Rect::new(rect_x, rect_y, TD_W as f32 - rect_x, TD_H as f32 - rect_y);
        let (_surface, layout) = paint_td_via_backend(&td, rect);

        // `MacBackend::new()`'s default `current_line_height`, matching
        // `paint_td_via_backend`'s backend construction (no explicit
        // line-height override in this test module).
        let line_height = 16.0_f32;
        let body_y = rect_y + line_height;

        let vis = &layout.visible_lines[0];
        assert_eq!(
            vis.bounds.y, 0.0,
            "visible_lines bounds.y must be body-local, got {}",
            vis.bounds.y,
        );

        let abs_x = rect_x + vis.bounds.x + vis.bounds.width * 0.5;
        let abs_y = body_y + vis.bounds.y + vis.bounds.height * 0.5;
        let local_x = abs_x - rect_x;
        let local_y = abs_y - body_y;
        match layout.hit_test(local_x, local_y) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, vis.line_idx),
            other => panic!("expected Line, got {:?}", other),
        }
    }

    // ── Chrome font ──────────────────────────────────────────────────
    //
    // Acceptance coverage: macOS now carries a `chrome_font` distinct
    // from `current_font` (the editor font), defaulting to the CoreText
    // system UI font, and `Backend::set_ui_font`/`set_editor_font` are no
    // longer no-ops. See `ChromeSurface`'s doc for the paint-time wiring
    // these tests exercise through `draw_status_bar_interactive`.

    /// A `MacBackend` that never received a `set_ui_font` call must still
    /// default its chrome to the CoreText system UI font, not a monospace
    /// editor face — matching GTK's `"Sans 11"` / Win-GUI's `"Segoe UI"`
    /// un-set defaults instead of macOS's old "no chrome font at all"
    /// posture.
    #[test]
    fn chrome_font_defaults_to_system_ui_font_not_monospace() {
        let b = MacBackend::new();
        let expected = super::super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT);
        assert_eq!(
            b.chrome_font.family_name(),
            expected.family_name(),
            "an untouched MacBackend's chrome_font must be the CoreText system UI font"
        );
        assert_ne!(
            b.chrome_font.family_name(),
            "Menlo",
            "the default chrome font must not be a monospace editor face"
        );
    }

    /// `set_editor_font` and `set_ui_font` must land on independent state.
    /// Growing one must never move the other's cached line height, and each
    /// setter must install exactly the font it was asked for.
    #[test]
    fn set_editor_font_and_set_ui_font_are_independent() {
        use crate::Backend;

        let mut b = MacBackend::new();
        b.set_editor_font("Menlo", 14.0);
        let editor_line_height_before = b.current_line_height;
        let chrome_line_height_before = b.chrome_line_height;

        // A much larger chrome font must move `chrome_line_height` but
        // leave the editor's untouched.
        Backend::set_ui_font(&mut b, "Helvetica 40");
        assert_eq!(
            b.current_line_height, editor_line_height_before,
            "set_ui_font must not touch the editor font's cached line height"
        );
        assert!(
            b.chrome_line_height > chrome_line_height_before,
            "set_ui_font(\"Helvetica 40\") must grow chrome_line_height from the \
             {DEFAULT_UI_FONT_SIZE_PT}pt default: before={chrome_line_height_before}, \
             after={}",
            b.chrome_line_height,
        );
        assert_eq!(b.chrome_font.family_name(), "Helvetica");
        assert_eq!(b.chrome_font.pt_size(), 40.0);

        // And the reverse: growing the editor font afterwards must not
        // move `chrome_line_height` back.
        let chrome_line_height_after_ui = b.chrome_line_height;
        Backend::set_editor_font(&mut b, "Menlo", 40.0);
        assert_eq!(
            b.chrome_line_height, chrome_line_height_after_ui,
            "set_editor_font must not touch chrome_line_height"
        );
        assert_eq!(
            b.current_font
                .as_ref()
                .expect("set_current_font ran")
                .family_name(),
            "Menlo",
        );
    }

    /// `measure_text` must resolve a genuinely different `CTFont` per
    /// [`crate::FontRole`] — `chrome_font` for [`crate::FontRole::Chrome`],
    /// `current_font` for [`crate::FontRole::Editor`] — not the same cached
    /// advance twice. A much larger chrome font must measure the same string
    /// wider than a small editor font does.
    #[test]
    fn measure_text_resolves_a_different_font_per_role() {
        use crate::{Backend, FontRole};

        let mut b = MacBackend::new();
        b.set_editor_font("Menlo", 10.0);
        Backend::set_ui_font(&mut b, "Helvetica 40");

        let (chrome_w, _) = Backend::measure_text(&b, "quadraui", FontRole::Chrome);
        let (editor_w, _) = Backend::measure_text(&b, "quadraui", FontRole::Editor);

        assert!(
            chrome_w > editor_w * 2.0,
            "a 40pt chrome font should measure \"quadraui\" much wider than a \
             10pt editor font: chrome_w={chrome_w}, editor_w={editor_w}"
        );
    }

    /// `measure_text` with [`crate::FontRole::Editor`] must fall back to
    /// `chrome_font` before `set_current_font` is called, never panicking.
    /// A fresh `MacBackend` must still return a real (non-zero) measurement.
    #[test]
    fn measure_text_editor_role_falls_back_to_chrome_font_before_set_current_font() {
        use crate::{Backend, FontRole};

        let b = MacBackend::new();
        let (w, h) = Backend::measure_text(&b, "quadraui", FontRole::Editor);
        assert!(w > 0.0, "expected a real measurement, got width {w}");
        assert!(h > 0.0, "expected a real measurement, got height {h}");
    }

    /// `default_fonts()` must report macOS's VS-Code-alignment convention:
    /// `Menlo 12` for editor content, and the real CoreText-resolved system
    /// UI font name at 13pt for chrome.
    #[test]
    fn mac_backend_default_fonts_reports_platform_convention() {
        use crate::Backend;

        let b = MacBackend::new();
        let defaults = Backend::default_fonts(&b);
        assert_eq!(defaults.editor_family, "Menlo");
        assert_eq!(defaults.editor_size_pt, 12.0);
        assert_eq!(
            defaults.ui_family,
            super::super::text::system_ui_font(13.0).family_name()
        );
        assert_eq!(defaults.ui_size_pt, 13.0);
    }

    /// A prior `set_editor_font`/`set_ui_font` call must not perturb
    /// `default_fonts()` — it reports a static platform fact, not this
    /// instance's current, possibly-overridden state (see
    /// `PlatformFontDefaults`'s doc).
    #[test]
    fn mac_backend_default_fonts_is_unaffected_by_set_editor_font_and_set_ui_font() {
        use crate::Backend;

        let mut b = MacBackend::new();
        b.set_editor_font("Helvetica", 40.0);
        Backend::set_ui_font(&mut b, "Helvetica 40");
        let defaults = Backend::default_fonts(&b);
        assert_eq!(defaults.editor_family, "Menlo");
        assert_eq!(defaults.editor_size_pt, 12.0);
        assert_eq!(
            defaults.ui_family,
            super::super::text::system_ui_font(13.0).family_name(),
            "ui_family is computed via a live system_ui_font() call, not a \
             literal — it's the field most worth re-checking after a prior \
             set_editor_font/set_ui_font override"
        );
        assert_eq!(defaults.ui_size_pt, 13.0);
    }

    /// `set_ui_font` degrades to the CoreText system UI font (rather
    /// than panicking or silently keeping the old chrome font) when the
    /// description doesn't parse or names a family Core Text doesn't
    /// have installed.
    #[test]
    fn set_ui_font_degrades_to_system_font_on_unparseable_or_unknown_family() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_ui_font(&mut b, "not-a-valid-description");
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT).family_name(),
            "an unparseable font_desc must degrade to the system UI font"
        );

        Backend::set_ui_font(&mut b, "Definitely Not A Real Font Family 12");
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_ui_font(12.0).family_name(),
            "an unknown family must degrade to the system UI font at the requested size"
        );
        assert_eq!(b.chrome_font.pt_size(), 12.0);
    }

    /// `set_editor_font` must keep the previously installed face when the
    /// requested family isn't installed — the "degrade, don't fail" posture.
    /// Without proper family resolution, an unknown name silently installed
    /// Helvetica (Core Text's substitute) as the editor font.
    #[test]
    fn set_editor_font_keeps_the_installed_font_for_an_unknown_family() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_editor_font(&mut b, "Menlo", 14.0);
        let line_height_before = b.current_line_height;

        Backend::set_editor_font(&mut b, "Definitely Not A Real Font Family", 30.0);
        assert_eq!(
            b.current_font
                .as_ref()
                .expect("editor font still installed")
                .family_name(),
            "Menlo",
            "an unknown family must not replace the installed editor font",
        );
        assert_eq!(
            b.current_line_height, line_height_before,
            "a rejected set_editor_font must not move the cached line height",
        );
    }

    /// `set_editor_font("monospace", …)` must resolve to the CoreText
    /// fixed-pitch system font, not a literal (and unresolvable) family
    /// named `"monospace"`.
    #[test]
    fn set_editor_font_resolves_the_monospace_generic_token() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_editor_font(&mut b, "monospace", 14.0);
        let expected = super::super::text::system_monospace_font(14.0);
        assert_eq!(
            b.current_font
                .as_ref()
                .expect("set_current_font ran")
                .family_name(),
            expected.family_name(),
            "\"monospace\" must resolve to the CoreText fixed-pitch system font"
        );
    }

    /// `set_ui_font` must resolve all three CSS generic tokens (plus Pango's
    /// `Sans`/`Monospace` aliases) to their native CoreText counterparts.
    #[test]
    fn set_ui_font_resolves_generic_tokens() {
        use crate::Backend;

        let mut b = MacBackend::new();

        Backend::set_ui_font(&mut b, "monospace 13");
        let expected_mono = super::super::text::system_monospace_font(13.0);
        assert_eq!(b.chrome_font.family_name(), expected_mono.family_name());
        assert_eq!(b.chrome_font.pt_size(), 13.0);

        Backend::set_ui_font(&mut b, "sans-serif 15");
        let expected_ui = super::super::text::system_ui_font(15.0);
        assert_eq!(b.chrome_font.family_name(), expected_ui.family_name());
        assert_eq!(b.chrome_font.pt_size(), 15.0);

        Backend::set_ui_font(&mut b, "system-ui 17");
        assert_eq!(b.chrome_font.family_name(), expected_ui.family_name());
        assert_eq!(b.chrome_font.pt_size(), 17.0);

        // Pango's own alias spellings for the first two resolve the same
        // way — GTK already treats them as synonyms via fontconfig, and
        // this backend now agrees.
        Backend::set_ui_font(&mut b, "Monospace 19");
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_monospace_font(19.0).family_name()
        );
        Backend::set_ui_font(&mut b, "Sans 21");
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_ui_font(21.0).family_name()
        );
    }

    /// `parse_ui_font_desc` must split a Pango-style comma-separated
    /// fallback list and resolve to the first family that's actually installed.
    /// `"Definitely Not A Real Font Family One, Definitely Not A Real Font
    /// Family Two, Menlo"` must land on `Menlo`, not fail the whole description.
    #[test]
    fn set_ui_font_resolves_the_first_installed_family_in_a_comma_list() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_ui_font(
            &mut b,
            "Definitely Not A Real Font Family One, Definitely Not A Real Font Family Two, \
             Menlo 16",
        );
        assert_eq!(b.chrome_font.family_name(), "Menlo");
        assert_eq!(b.chrome_font.pt_size(), 16.0);
    }

    /// Same as the sized case above, but with no trailing size at all —
    /// the real-world shape this issue was filed over (vimcode's
    /// `UI_FONT_FAMILY` constants carry no size, only a comma-separated
    /// family list). Must still resolve to the first installed family,
    /// at the chrome default size.
    #[test]
    fn set_ui_font_resolves_a_comma_list_with_no_trailing_size() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_ui_font(
            &mut b,
            "Definitely Not A Real Font Family One, Definitely Not A Real Font Family Two, Menlo",
        );
        assert_eq!(b.chrome_font.family_name(), "Menlo");
        assert_eq!(b.chrome_font.pt_size(), DEFAULT_UI_FONT_SIZE_PT);
    }

    /// A generic token appearing later in a comma list (Pango's own
    /// "…, Sans" convention for "fall back to the platform default") is
    /// still an always-resolvable stop, exactly like a concrete
    /// installed family — the fallback chain from this issue's own
    /// motivating example (`"SF Pro Text, Helvetica Neue, Lucida Grande,
    /// Sans"`).
    #[test]
    fn set_ui_font_resolves_a_generic_token_later_in_a_comma_list() {
        use crate::Backend;

        let mut b = MacBackend::new();
        Backend::set_ui_font(
            &mut b,
            "Definitely Not A Real Font Family One, Definitely Not A Real Font Family Two, \
             sans-serif",
        );
        assert_eq!(
            b.chrome_font.family_name(),
            super::super::text::system_ui_font(DEFAULT_UI_FONT_SIZE_PT).family_name()
        );
    }

    /// Black-box acceptance test: paint a `StatusBar`
    /// through the real `Backend::draw_status_bar_interactive` path and
    /// prove its painted geometry tracks `set_ui_font`, not
    /// `set_editor_font` — the two-font separation actually reaching a
    /// rasteriser, not just backend-internal state.
    #[test]
    fn draw_status_bar_interactive_paints_with_chrome_font_not_editor_font() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::Backend;

        const W: u32 = 600;
        const H: u32 = 40;

        fn painted_segment_width(chrome_desc: &str, editor: (&str, f32)) -> f32 {
            let surface = BitmapSurface::new(W, H);
            let mut b = MacBackend::new();
            Backend::set_editor_font(&mut b, editor.0, editor.1);
            Backend::set_ui_font(&mut b, chrome_desc);
            b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

            let bar = StatusBar {
                id: WidgetId::new("status"),
                left_segments: vec![StatusBarSegment {
                    text: "Save".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(10, 10, 10),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let layout = std::cell::RefCell::new(None);
            b.enter_frame_scope(surface.context_ptr(), |backend| {
                let l = backend.draw_status_bar_interactive(
                    Rect::new(0.0, 0.0, W as f32, H as f32),
                    &bar,
                    &crate::InteractionState::new(),
                );
                *layout.borrow_mut() = Some(l);
            });
            b.end_frame();
            layout.into_inner().unwrap().visible_segments[0]
                .bounds
                .width
        }

        // Same chrome font, wildly different editor font sizes: painted
        // width must not move — proves the status bar reads chrome_font,
        // not current_font.
        let w_small_editor = painted_segment_width("Menlo 11", ("Menlo", 10.0));
        let w_huge_editor = painted_segment_width("Menlo 11", ("Menlo", 80.0));
        assert!(
            (w_small_editor - w_huge_editor).abs() < 0.01,
            "status bar width must be unaffected by set_editor_font: {w_small_editor} vs \
             {w_huge_editor}"
        );

        // Same editor font, wildly different chrome font sizes: painted
        // width MUST move — proves it does read chrome_font.
        let w_small_chrome = painted_segment_width("Menlo 8", ("Menlo", 14.0));
        let w_huge_chrome = painted_segment_width("Menlo 60", ("Menlo", 14.0));
        assert!(
            w_huge_chrome > w_small_chrome * 2.0,
            "status bar width must grow with set_ui_font: {w_small_chrome} vs {w_huge_chrome}"
        );
    }

    /// `draw_status_bar_interactive_scaled` must paint narrower text at a
    /// smaller `font_scale`, and must not leave the ambient chrome font
    /// perturbed for the next call. An unscaled paint immediately after
    /// must come back to exactly its own pre-scaled width.
    #[test]
    fn draw_status_bar_interactive_scaled_shrinks_without_perturbing_later_unscaled_paints() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::Backend;

        const W: u32 = 600;
        const H: u32 = 40;

        fn sample_bar() -> StatusBar {
            StatusBar {
                id: WidgetId::new("status"),
                left_segments: vec![StatusBarSegment {
                    text: "Save".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(10, 10, 10),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            }
        }

        let surface = BitmapSurface::new(W, H);
        let mut b = MacBackend::new();
        Backend::set_ui_font(&mut b, "Menlo 40");
        b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let bar = sample_bar();
        let unscaled_w = std::cell::RefCell::new(0.0f32);
        let scaled_w = std::cell::RefCell::new(0.0f32);
        let unscaled_w_again = std::cell::RefCell::new(0.0f32);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            let l1 = backend.draw_status_bar_interactive(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &bar,
                &crate::InteractionState::new(),
            );
            *unscaled_w.borrow_mut() = l1.visible_segments[0].bounds.width;

            let l2 = backend.draw_status_bar_interactive_scaled(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &bar,
                &crate::InteractionState::new(),
                0.5,
            );
            *scaled_w.borrow_mut() = l2.visible_segments[0].bounds.width;

            let l3 = backend.draw_status_bar_interactive(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &bar,
                &crate::InteractionState::new(),
            );
            *unscaled_w_again.borrow_mut() = l3.visible_segments[0].bounds.width;
        });
        b.end_frame();

        let unscaled_w = unscaled_w.into_inner();
        let scaled_w = scaled_w.into_inner();
        let unscaled_w_again = unscaled_w_again.into_inner();

        assert!(
            scaled_w < unscaled_w * 0.75,
            "font_scale=0.5 should paint noticeably narrower: {scaled_w} vs {unscaled_w}",
        );
        assert!(
            (unscaled_w_again - unscaled_w).abs() < 0.01,
            "an unscaled paint after a scaled one must match the original unscaled width \
             exactly — chrome font state must be restored, not left scaled: \
             {unscaled_w_again} vs {unscaled_w}",
        );
    }

    /// `status_bar_layout` (the no-paint twin) must agree with what
    /// `draw_status_bar_interactive` actually painted once both read
    /// `chrome_font`, keeping the two call sites in sync.
    #[test]
    fn status_bar_layout_matches_painted_layout_after_set_ui_font() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
        use crate::Backend;

        const W: u32 = 300;
        const H: u32 = 30;

        let mut b = MacBackend::new();
        Backend::set_editor_font(&mut b, "Menlo", 14.0);
        Backend::set_ui_font(&mut b, "Helvetica 22");
        b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let bar = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: "Save".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(10, 10, 10),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };

        let surface = BitmapSurface::new(W, H);
        let painted = std::cell::RefCell::new(None);
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            let l = backend.draw_status_bar_interactive(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &bar,
                &crate::InteractionState::new(),
            );
            *painted.borrow_mut() = Some(l);
        });
        b.end_frame();
        let painted = painted.into_inner().unwrap();

        let computed = b.status_bar_layout(Rect::new(0.0, 0.0, W as f32, H as f32), &bar);
        assert_eq!(
            painted.visible_segments.len(),
            computed.visible_segments.len(),
        );
        for (p, c) in painted
            .visible_segments
            .iter()
            .zip(computed.visible_segments.iter())
        {
            assert!((p.bounds.x - c.bounds.x).abs() < 0.001);
            assert!((p.bounds.width - c.bounds.width).abs() < 0.001);
        }
    }

    /// A `StatusBar` drawn into a rect taller than `chrome_line_height`
    /// must fill the whole rect, not just its own line height. The leftover
    /// strip must match `theme.background`, not show the frame's clear colour.
    #[test]
    fn draw_status_bar_interactive_fills_the_whole_rect_height_not_just_line_height() {
        use super::super::headless::BitmapSurface;
        use crate::primitives::status_bar::StatusBar;
        use crate::theme::Theme;
        use crate::types::Color;

        const W: u32 = 200;
        const H: u32 = 40;

        // A light background, proving the fill extends beyond chrome_line_height.
        let theme = Theme {
            background: Color::rgb(236, 236, 236),
            ..Theme::default()
        };

        let surface = BitmapSurface::new(W, H);
        let mut b = MacBackend::new();
        b.set_current_theme(theme);
        // Deliberately small chrome font so `chrome_line_height` sits
        // well under `H` — the gap this test is pinning.
        Backend::set_ui_font(&mut b, "Menlo 8");
        assert!(
            (b.chrome_line_height as u32) < H,
            "test setup: chrome_line_height ({}) must be smaller than the rect height \
             ({H}) for this to exercise the bug",
            b.chrome_line_height,
        );

        b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let bar = StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![],
            right_segments: vec![],
        };
        b.enter_frame_scope(surface.context_ptr(), |backend| {
            let _ = backend.draw_status_bar_interactive(
                Rect::new(0.0, 0.0, W as f32, H as f32),
                &bar,
                &crate::InteractionState::new(),
            );
        });
        b.end_frame();

        // The bottom row — below where chrome_line_height alone would stop filling.
        let (r, g, bl, _a) = surface.pixel(W / 2, H - 2);
        assert_eq!(
            (r, g, bl),
            (theme.background.r, theme.background.g, theme.background.b),
            "the bottom of a status-bar rect taller than chrome_line_height must be \
             filled with the theme background, not left unpainted: got ({r}, {g}, {bl})"
        );
    }

    /// Acceptance test: paint a `TreeView` through the real
    /// `Backend::draw_tree` path and verify its painted row-label extent
    /// tracks `set_ui_font`, not `set_editor_font`. Same structure as
    /// `GtkBackend`'s `gtk_backend_draw_tree_uses_ui_font_not_editor_font`
    /// (`gtk/backend.rs`): paint the same single-row tree under two
    /// wildly different *editor* font sizes with `ui_font` left at its
    /// default — the painted extent must be identical — then change
    /// `ui_font` alone as a positive control and see the extent move.
    ///
    /// The one place this differs from its GTK twin is row *geometry*.
    /// `GtkBackend` is **told** its metrics (`current_line_height`
    /// / `current_char_width` are pushed in by the host), so swapping the
    /// Pango layout's font description there changes only the glyphs.
    /// `MacBackend` **derives** them, inside `set_editor_font` — and row
    /// pitch (`line_height * 1.4`) plus the leaf row's own horizontal
    /// chevron reservation (`macos::tree::draw_tree`'s
    /// `cursor_x += line_height * 0.8`) both read that derived value, by
    /// design: row pitch tracking the editor line height is the
    /// deliberately-deferred half of #624/#1003 (see
    /// `MacBackend::draw_tree`'s comment), not part of the font swap under
    /// test. So the two cached metrics are pinned to fixed values after
    /// each `set_editor_font`, leaving the *glyph font* as the only thing
    /// that varies between calls. Without that pin, a `Menlo 60` editor
    /// font alone pushes row 0's height past the surface (the rasteriser
    /// then skips the clipped row entirely) and shifts the label's start
    /// x by ~40px — the test would swing on row layout, never reaching
    /// the question it was written to ask.
    #[test]
    fn draw_tree_uses_ui_font_not_editor_font() {
        use super::super::headless::BitmapSurface;
        use crate::types::{Decoration, SelectionMode, StyledText, TreeStyle};

        const W: u32 = 800;
        /// Tall enough for a whole `PINNED_LINE_HEIGHT * 1.4` row plus the
        /// 60pt positive-control glyph, so nothing under test is clipped.
        const H: u32 = 120;
        /// Pinned row metrics — see this test's doc comment.
        const PINNED_LINE_HEIGHT: f64 = 60.0;
        const PINNED_CHAR_WIDTH: f64 = 8.0;

        fn row_text_extent(editor: (&str, f32), ui_font: Option<&str>) -> u32 {
            let surface = BitmapSurface::new(W, H);
            surface.fill(1.0, 1.0, 1.0, 1.0);

            let mut b = MacBackend::new();
            Backend::set_editor_font(&mut b, editor.0, editor.1);
            // Pin row geometry *after* the editor font installed its own
            // derived metrics — see this test's doc comment for why.
            b.set_current_line_height(PINNED_LINE_HEIGHT);
            b.set_current_char_width(PINNED_CHAR_WIDTH);
            if let Some(f) = ui_font {
                Backend::set_ui_font(&mut b, f);
            }
            // White `tab_bar_bg`/`background` so `draw_tree`'s own
            // row-background fill doesn't itself read as "non-white" and
            // saturate the rightmost-painted-pixel scan below — mirrors
            // `GtkBackend`'s `gtk_backend_draw_tree_uses_ui_font_not_editor_font`.
            b.set_current_theme(crate::Theme {
                tab_bar_bg: Color::rgb(255, 255, 255),
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                ..crate::Theme::default()
            });
            b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

            let tree = TreeView {
                id: WidgetId::new("test:tree"),
                rows: vec![crate::primitives::tree::TreeRow {
                    path: vec![0],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain("m".to_string()),
                    badge: None,
                    is_expanded: None,
                    decoration: Decoration::Normal,
                    edit: None,
                }],
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: TreeStyle::default(),
                has_focus: false,
            };
            b.enter_frame_scope(surface.context_ptr(), |backend| {
                backend.draw_tree(Rect::new(0.0, 0.0, W as f32, H as f32), &tree);
            });
            b.end_frame();

            // Rightmost non-white pixel anywhere on the surface — a
            // proxy for the painted label's glyph extent. The whole
            // surface is scanned rather than one chosen scanline
            // because the label is vertically centred in its row using
            // the *chrome* font's measured height, so an 11pt default
            // and a 60pt `ui_font` put ink on completely different rows;
            // a fixed probe line would measure "is there a glyph at
            // y=N", not "how wide is the glyph". Row background and
            // area fill are both white (see the theme above), so only
            // glyph ink can trip this.
            (0..W)
                .rev()
                .find(|&x| (0..H).any(|y| surface.pixel(x, y) != (255, 255, 255, 255)))
                .unwrap_or(0)
        }

        let small_editor_extent = row_text_extent(("Menlo", 8.0), None);
        let large_editor_extent = row_text_extent(("Menlo", 60.0), None);
        assert!(
            small_editor_extent.abs_diff(large_editor_extent) <= 1,
            "tree row glyph extent must be editor-font-size independent: \
             small_editor={small_editor_extent}, large_editor={large_editor_extent}"
        );

        let ui_font_extent = row_text_extent(("Menlo", 8.0), Some("Helvetica 60"));
        assert!(
            ui_font_extent > small_editor_extent + 20,
            "changing ui_font alone must visibly widen the painted row label: \
             default_ui_font={small_editor_extent}, ui_font_Helvetica_60={ui_font_extent}"
        );
    }

    /// Build a flat tree of `n_rows` leaf rows for `tree_vscrollbar`
    /// integration tests.
    fn flat_tree(n_rows: usize) -> TreeView {
        TreeView {
            id: WidgetId::new("test:tree:vscrollbar"),
            rows: (0..n_rows)
                .map(|i| crate::primitives::tree::TreeRow {
                    path: vec![i as u16],
                    indent: 0,
                    icon: None,
                    text: crate::types::StyledText::plain(format!("row{i}")),
                    badge: None,
                    is_expanded: None,
                    decoration: crate::types::Decoration::Normal,
                    edit: None,
                })
                .collect(),
            selection_mode: crate::types::SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: crate::types::TreeStyle::default(),
            has_focus: false,
        }
    }

    /// `tree_vscrollbar` must pass `line_height * 1.4` pitch (not raw
    /// `line_height`) as `TreeView::vscrollbar`'s `row_height`, since that
    /// is what `mac_tree_layout` actually paints for non-header rows.
    /// With `current_line_height` at 16px default, 15 rows painted at the
    /// real 22.4px→22px pitch only fit 13 into a 300px-tall viewport
    /// (300/22 = 13), so an overflow scrollbar must appear. The row pitch
    /// governs the minimum thumb length, not the track's (fixed 10px)
    /// width.
    #[test]
    fn mac_backend_tree_vscrollbar_uses_tree_layout_row_pitch_not_raw_line_height() {
        let backend = MacBackend::new();
        let row_pitch = (backend.chrome_line_height * 1.4).round();
        let n_rows = ((250.0 / row_pitch).ceil() as usize) + 2;
        let tree = flat_tree(n_rows);
        let rect = Rect::new(0.0, 0.0, 20.0, 250.0);

        let sb = Backend::tree_vscrollbar(&backend, rect, &tree)
            .expect("rows at chrome_line_height row pitch must overflow a 250px viewport");

        let expected_row_h = row_pitch as f32;
        assert_eq!(
            sb.track.width,
            crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH,
            "track width must stay the fixed VS Code list/tree scrollbar width"
        );
        assert!(
            sb.thumb_len >= expected_row_h,
            "minimum thumb length (and row pitch) must match tree_layout's \
             non-header item_height, not the raw line_height"
        );
    }

    /// A host-set `TreeStyle::row_height` must be honored
    /// by `tree_vscrollbar`, exactly as `mac_tree_layout`/`draw_tree`
    /// honor it — not silently ignored in favor of `line_height`. It
    /// governs the minimum thumb length, not the track's (fixed 10px)
    /// width.
    #[test]
    fn mac_backend_tree_vscrollbar_honors_tree_style_row_height_override() {
        let backend = MacBackend::new();
        let mut tree = flat_tree(15);
        tree.style.row_height = Some(50);
        let rect = Rect::new(0.0, 0.0, 20.0, 300.0);

        let sb = Backend::tree_vscrollbar(&backend, rect, &tree)
            .expect("15 rows at a 50px override overflow a 300px viewport (6 visible)");
        assert_eq!(
            sb.track.width,
            crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH
        );
        assert!(sb.thumb_len >= 50.0);
    }

    /// `menu_bar_layout`/`draw_menu_bar` twin of
    /// `draw_tree_uses_ui_font_not_editor_font`: verify the returned
    /// `MenuBarLayout`'s measured item width is correct. `draw_menu_bar`
    /// hands back real geometry, so there is no need to rasterise and
    /// re-measure.
    #[test]
    fn draw_menu_bar_uses_ui_font_not_editor_font() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 800;
        const H: u32 = 30;

        fn item_width(editor: (&str, f32), ui_font: &str) -> f32 {
            let surface = BitmapSurface::new(W, H);
            let mut b = MacBackend::new();
            Backend::set_editor_font(&mut b, editor.0, editor.1);
            Backend::set_ui_font(&mut b, ui_font);
            // The painted strip is the `Custom` opt-out on macOS; `Auto`
            // installs the native menu bar and lays out no in-window items.
            Backend::set_menu_style(&mut b, crate::backend::MenuStyle::Custom);
            b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

            let bar = MenuBar {
                id: WidgetId::new("test:menu-bar"),
                items: vec![crate::MenuBarItem {
                    id: WidgetId::new("test:menu-bar:file"),
                    label: "&File".to_string(),
                    disabled: false,
                    submenu: None,
                }],
                open_item: None,
                focused_item: None,
            };
            let width = std::cell::RefCell::new(0.0f32);
            b.enter_frame_scope(surface.context_ptr(), |backend| {
                let layout = backend.draw_menu_bar(Rect::new(0.0, 0.0, W as f32, H as f32), &bar);
                *width.borrow_mut() = layout.visible_items[0].bounds.width;
            });
            b.end_frame();
            width.into_inner()
        }

        let w_small_editor = item_width(("Menlo", 10.0), "Helvetica 11");
        let w_huge_editor = item_width(("Menlo", 80.0), "Helvetica 11");
        assert!(
            (w_small_editor - w_huge_editor).abs() < 0.5,
            "menu bar item width must be unaffected by set_editor_font: \
             {w_small_editor} vs {w_huge_editor}"
        );

        let w_small_chrome = item_width(("Menlo", 14.0), "Helvetica 8");
        let w_huge_chrome = item_width(("Menlo", 14.0), "Helvetica 60");
        assert!(
            w_huge_chrome > w_small_chrome * 2.0,
            "menu bar item width must grow with set_ui_font: {w_small_chrome} vs {w_huge_chrome}"
        );
    }

    fn sample_menu_bar_for_native_default_tests() -> MenuBar {
        MenuBar {
            id: WidgetId::new("test:menu-bar"),
            items: vec![crate::MenuBarItem {
                id: WidgetId::new("test:menu-bar:file"),
                label: "&File".to_string(),
                disabled: false,
                submenu: None,
            }],
            open_item: None,
            focused_item: None,
        }
    }

    /// `Backend::draw_menu_bar`'s default (`Auto`, unresolved by any
    /// `set_menu_style` call) must take the native path — paint nothing
    /// in-window and report a zero-height layout, so a caller sizing
    /// content from `layout.bounds.height` reserves no blank strip
    /// (AppKit's own menu bar lives outside the window entirely).
    /// `install_menu_bar` itself silently no-ops off the main thread —
    /// every `#[test]` fn runs on one; see
    /// `install_menu_bar_off_main_thread_does_not_panic` above — so this
    /// test only proves the layout/no-paint half of the contract. Going
    /// through the trait method (`b.draw_menu_bar`, not a sidecar) is
    /// itself part of what this pins: every generic caller gets the
    /// native default for free.
    #[test]
    fn draw_menu_bar_defaults_to_native_and_zeroes_height_on_auto() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 300;
        const H: u32 = 24;
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);

        let bar = sample_menu_bar_for_native_default_tests();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let mut backend = MacBackend::new();
        assert_eq!(backend.menu_style(), crate::backend::MenuStyle::Auto);

        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *layout.borrow_mut() = Some(b.draw_menu_bar(rect, &bar));
        });
        let layout = layout
            .into_inner()
            .expect("draw_menu_bar ran inside the frame scope");

        assert_eq!(
            layout.bounds.height, 0.0,
            "native path must reserve no vertical space for an in-window strip"
        );
        assert_eq!(layout.bounds.width, rect.width);
        assert!(layout.visible_items.is_empty());
        assert!(layout.hit_regions.is_empty());

        let painted_any = (0..W).any(|x| {
            (0..H).any(|y| {
                let (r, g, b, _) = surface.pixel(x, y);
                (r, g, b) != (255, 255, 255)
            })
        });
        assert!(
            !painted_any,
            "native path must not paint the in-window strip at all"
        );
    }

    /// `menu_bar_layout` is `draw_menu_bar`'s no-paint twin, so it must
    /// follow the same `MenuStyle` branch: empty under the native
    /// default (a `MenuSystem` click on the unpainted strip must not
    /// hit-test into a painted dropdown), the painted geometry under the
    /// `Custom` opt-out.
    #[test]
    fn menu_bar_layout_agrees_with_draw_menu_bar_under_each_menu_style() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 300;
        const H: u32 = 24;
        let bar = sample_menu_bar_for_native_default_tests();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        for style in [
            crate::backend::MenuStyle::Auto,
            crate::backend::MenuStyle::Custom,
        ] {
            let surface = BitmapSurface::new(W, H);
            let mut backend = MacBackend::new();
            backend.set_menu_style(style);
            let no_paint = backend.menu_bar_layout(rect, &bar);
            let painted =
                backend.enter_frame_scope(surface.context_ptr(), |b| b.draw_menu_bar(rect, &bar));

            assert_eq!(no_paint, painted, "{style:?}");
            if style == crate::backend::MenuStyle::Custom {
                assert!(
                    !no_paint.visible_items.is_empty(),
                    "the painted opt-out lays out its items"
                );
            }
        }
    }

    /// Companion to the test above: once an app opts out via
    /// `set_menu_style(MenuStyle::Custom)`, `Backend::draw_menu_bar`
    /// must fall back to exactly what the painted strip
    /// (`draw_menu_bar_painted`) always has — same non-zero-height
    /// layout, same pixels — so the opt-out really is a
    /// no-behaviour-change escape hatch, not a degraded one.
    #[test]
    fn draw_menu_bar_falls_back_to_painted_strip_on_custom() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 300;
        const H: u32 = 24;
        let bar = sample_menu_bar_for_native_default_tests();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        // Reference: the painted strip directly, bypassing style
        // resolution — what the `Custom` path must match exactly.
        let direct_surface = BitmapSurface::new(W, H);
        direct_surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut direct_backend = MacBackend::new();
        let direct_layout = std::cell::RefCell::new(None);
        direct_backend.enter_frame_scope(direct_surface.context_ptr(), |b| {
            *direct_layout.borrow_mut() = Some(b.draw_menu_bar_painted(rect, &bar));
        });
        let direct_layout = direct_layout.into_inner().unwrap();

        // Under test: the same bar, via the trait method with the
        // `Custom` opt-out set.
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_menu_style(crate::backend::MenuStyle::Custom);
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *layout.borrow_mut() = Some(b.draw_menu_bar(rect, &bar));
        });
        let layout = layout.into_inner().unwrap();

        assert_eq!(layout, direct_layout);
        assert!(
            layout.bounds.height > 0.0,
            "the painted strip must reserve real vertical space"
        );

        for y in 0..H {
            for x in 0..W {
                assert_eq!(
                    surface.pixel(x, y),
                    direct_surface.pixel(x, y),
                    "pixel ({x}, {y}) must match a direct draw_menu_bar_painted call once opted out",
                );
            }
        }
    }

    /// The "call once per frame" contract for the native
    /// path: calling `Backend::draw_menu_bar` twice in a row with an
    /// unchanged `bar` must not reinstall the `NSMenu` tree the second
    /// time — `install_menu_bar_if_changed` must see `last_native_menu_bar`
    /// already matching and skip straight past `install_menu_bar`.
    /// `install_menu_bar`'s own AppKit half no-ops off the main thread
    /// (every `#[test]` fn runs off it), so `menu_target` can't be used
    /// as the signal here; the helper's return value reports whether the
    /// `NSMenu` rebuild was entered at all, which is exactly the
    /// per-frame property under test.
    #[test]
    fn install_menu_bar_if_changed_skips_reinstall_for_an_unchanged_bar() {
        let bar = sample_menu_bar_for_native_default_tests();
        let mut backend = MacBackend::new();

        assert!(backend.last_native_menu_bar.is_none());
        assert!(
            backend.install_menu_bar_if_changed(&bar),
            "the first install of a bar must build the NSMenu tree",
        );
        assert_eq!(backend.last_native_menu_bar.as_ref(), Some(&bar));

        assert!(
            !backend.install_menu_bar_if_changed(&bar),
            "an unchanged bar must not rebuild the NSMenu tree",
        );
        assert_eq!(backend.last_native_menu_bar.as_ref(), Some(&bar));

        // A genuinely different bar must install again, and update the record.
        let mut other = bar.clone();
        other.items[0].label = "&Edit".to_string();
        assert!(
            backend.install_menu_bar_if_changed(&other),
            "a changed bar must rebuild the NSMenu tree",
        );
        assert_eq!(backend.last_native_menu_bar.as_ref(), Some(&other));
    }

    /// Regression test for `draw_activity_bar` — the mandatory,
    /// non-default `Backend` method (see its own comment for why
    /// `draw_activity_bar_with_style`, which already had equivalent review
    /// attention, doesn't cover this path: `AppShell::render` and
    /// `ScreenLayout::draw`'s `Surface::ActivityBar` arm both call this
    /// method directly, never the styled one).
    ///
    /// Verifies that `draw_activity_bar` paints through `chrome_font`,
    /// never `current_font`/the editor font. Icon extent is pinned to
    /// `ActivityBarStyle::resolved_icon_size_px()` regardless of font point
    /// size (see `macos::activity_bar` module doc). This test uses three
    /// probes:
    /// - icon extent is independent of `editor_font`'s size;
    /// - icon extent is independent of `editor_font`'s family — a
    ///   regression would swap the editor family and move the rendered
    ///   glyph;
    /// - icon extent is independent of `ui_font`'s size.
    #[test]
    fn draw_activity_bar_uses_ui_font_not_editor_font() {
        use super::super::headless::BitmapSurface;

        const W: u32 = 200;
        const H: u32 = 60;

        fn icon_extent(editor: (&str, f32), ui_font: Option<&str>) -> u32 {
            let surface = BitmapSurface::new(W, H);
            surface.fill(1.0, 1.0, 1.0, 1.0);

            let mut b = MacBackend::new();
            Backend::set_editor_font(&mut b, editor.0, editor.1);
            if let Some(f) = ui_font {
                Backend::set_ui_font(&mut b, f);
            }
            // White `tab_bar_bg`/`background`, black `inactive_fg`, so
            // only glyph ink trips the pixel scan below — mirrors
            // `draw_tree_uses_ui_font_not_editor_font`'s theme override.
            b.set_current_theme(crate::Theme {
                tab_bar_bg: Color::rgb(255, 255, 255),
                background: Color::rgb(255, 255, 255),
                foreground: Color::rgb(0, 0, 0),
                inactive_fg: Color::rgb(0, 0, 0),
                ..crate::Theme::default()
            });
            b.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

            let bar = crate::ActivityBar {
                id: WidgetId::new("test:activity-bar"),
                top_items: vec![crate::ActivityItem {
                    id: WidgetId::new("test:activity:item"),
                    // `nerd_fonts_enabled` defaults to `false` (see
                    // `MacBackend::new`), so the rasteriser paints
                    // `fallback`, not `glyph` — a multi-character
                    // fallback makes the width swing more visible than
                    // a single glyph would.
                    icon: crate::types::Icon::new("x", "WWWW"),
                    tooltip: String::new(),
                    is_active: false,
                    is_keyboard_selected: false,
                }],
                bottom_items: vec![],
                active_accent: None,
                selection_bg: None,
                is_keyboard_focused: false,
            };
            b.enter_frame_scope(surface.context_ptr(), |backend| {
                backend.draw_activity_bar(Rect::new(0.0, 0.0, W as f32, H as f32), &bar, None);
            });
            b.end_frame();

            // Horizontal span between the leftmost and rightmost
            // non-white pixel anywhere on the surface — a proxy for the
            // painted icon glyph's width.
            let mut min_x: Option<u32> = None;
            let mut max_x: Option<u32> = None;
            for x in 0..W {
                for y in 0..H {
                    if surface.pixel(x, y) != (255, 255, 255, 255) {
                        min_x = Some(min_x.map_or(x, |m| m.min(x)));
                        max_x = Some(max_x.map_or(x, |m| m.max(x)));
                    }
                }
            }
            match (min_x, max_x) {
                (Some(lo), Some(hi)) => hi - lo,
                _ => 0,
            }
        }

        let small_editor_extent = icon_extent(("Menlo", 8.0), None);
        let large_editor_extent = icon_extent(("Menlo", 60.0), None);
        assert!(
            small_editor_extent.abs_diff(large_editor_extent) <= 1,
            "activity bar icon glyph extent must be editor-font-size independent: \
             small_editor={small_editor_extent}, large_editor={large_editor_extent}"
        );

        // Editor *family* must be just as inert as editor *size* — if
        // `draw_activity_bar` ever regressed to reading `current_font`
        // instead of `chrome_font`, a family swap this different
        // (monospace code font vs a decorative display face) would move
        // the rendered glyph.
        let other_editor_family_extent = icon_extent(("Papyrus", 8.0), None);
        assert!(
            small_editor_extent.abs_diff(other_editor_family_extent) <= 1,
            "activity bar icon glyph extent must be editor-font-family independent: \
             menlo_editor={small_editor_extent}, papyrus_editor={other_editor_family_extent}"
        );

        // Icon extent must also be ui_font-*size* independent —
        // the fix this issue shipped, exercised through the mandatory
        // `draw_activity_bar` entry point rather than only the styled
        // one.
        let huge_ui_font_extent = icon_extent(("Menlo", 8.0), Some("Helvetica 60"));
        assert!(
            small_editor_extent.abs_diff(huge_ui_font_extent) <= 1,
            "activity bar icon glyph extent must be ui-font-size independent (issue #1157): \
             default_ui_font={small_editor_extent}, ui_font_Helvetica_60={huge_ui_font_extent}"
        );
    }

    /// `draw_tab_bar_icons` paints labels through `chrome_font`, so the
    /// vertical-centring math it feeds `tab_bar::draw_tab_bar_icons` must
    /// use `chrome_line_height`, not the editor's `current_line_height` —
    /// otherwise a tab bar painted with a chrome font that differs from
    /// the editor font centres the label against a line height that
    /// doesn't describe the font actually painting it.
    ///
    /// Pins a tiny editor font against a much larger chrome font (mirrors
    /// `mac_backend_chart_axis_label_space_is_reserved_in_chrome_metrics`'s
    /// divergence check) so the two centring formulas land far enough
    /// apart that the chrome-correct one can't pass by coincidence, then
    /// asserts the painted label's recorded top edge matches centring on
    /// `chrome_line_height`.
    #[test]
    #[allow(deprecated)] // `draw_tab_bar` returns the deprecated `TabBarHits` — issue #823
    fn mac_backend_tab_bar_label_centres_on_chrome_line_height() {
        const CW: u32 = 320;
        const CH: u32 = 80;
        const ROW_Y: f32 = 10.0;
        const ROW_H: f32 = 60.0;

        let surface = BitmapSurface::new(CW, CH);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(super::super::text::make_font("Menlo", 5.0).expect("Menlo"));
        backend.set_chrome_font(super::super::text::make_font("Menlo", 24.0).expect("Menlo"));
        assert!(
            backend.chrome_line_height - backend.current_line_height > 1.0,
            "chrome and editor line heights must differ enough that the two centring \
             formulas land at different y positions: chrome={}, editor={}",
            backend.chrome_line_height,
            backend.current_line_height,
        );

        let bar = TabBar {
            id: WidgetId::new("tabbar:test"),
            tabs: vec![crate::TabItem {
                label: "a".to_string(),
                is_active: true,
                ..Default::default()
            }],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: false,
            compact: false,
        };

        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(CW as f32, CH as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_tab_bar(Rect::new(0.0, ROW_Y, CW as f32, ROW_H), &bar, None);
        });
        backend.end_frame();

        let runs = backend.text_runs();
        let run = runs.iter().find(|r| r.text == "a").unwrap_or_else(|| {
            panic!(
                "expected a painted run for the tab label \"a\"; painted: {:?}",
                runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
            )
        });

        let expected_y = ROW_Y + (ROW_H - backend.chrome_line_height as f32) / 2.0;
        let buggy_y = ROW_Y + (ROW_H - backend.current_line_height as f32) / 2.0;
        assert!(
            (buggy_y - expected_y).abs() > 1.0,
            "the two candidate centrings must differ enough to tell them apart: \
             chrome-centred={expected_y}, editor-centred={buggy_y}",
        );
        assert!(
            (run.bounds.y - expected_y).abs() < 0.01,
            "tab label must be centred using chrome_line_height, not current_line_height: \
             got y={}, expected={expected_y} (editor-metric centring would give {buggy_y})",
            run.bounds.y,
        );
    }

    // ── toast overlay entrance slide ─────────────────────────────────────

    const TOAST_W: u32 = 400;
    const TOAST_H: u32 = 200;

    fn sample_toast_overlay() -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("toasts:bottom_right"),
            corner: ToastCorner::BottomRight,
            toasts: vec![crate::primitives::toast::Toast {
                id: WidgetId::new("toast:1"),
                title: "Saved".to_string(),
                body: String::new(),
                severity: crate::primitives::toast::ToastSeverity::Success,
                actions: vec![],
                accent: None,
            }],
            focus: None,
        }
    }

    /// Paints `overlay` through `MacBackend::draw_toast_overlay` on a fresh
    /// frame and returns both the surface and the resolved layout, which
    /// stays pinned to each toast's resting bounds regardless of any
    /// in-flight slide offset (see `paint_with_x_offset`'s own doc).
    fn paint_toast_overlay(
        backend: &mut MacBackend,
        overlay: &ToastOverlay,
    ) -> (super::super::headless::BitmapSurface, ToastStackLayout) {
        use super::super::headless::BitmapSurface;

        let surface = BitmapSurface::new(TOAST_W, TOAST_H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        backend.begin_frame(Viewport::new(TOAST_W as f32, TOAST_H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l =
                b.draw_toast_overlay(Rect::new(0.0, 0.0, TOAST_W as f32, TOAST_H as f32), overlay);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// `MacBackend::draw_toast_overlay` slides a newly-shown toast in from
    /// its stack's off-screen edge via `Transition`, instead of painting it
    /// at full, resting position on the very first frame: sampling the
    /// toast's resting-position centre on frame one must not show its
    /// (deterministic, severity-driven) fill colour, since the box is still
    /// painted off-canvas at that point.
    #[test]
    fn toast_overlay_slides_in_from_off_screen_on_first_frame() {
        let overlay = sample_toast_overlay();
        let mut backend = MacBackend::new();
        backend.set_current_font(super::super::text::make_font("Menlo", 14.0).expect("Menlo"));

        let (surface, layout) = paint_toast_overlay(&mut backend, &overlay);
        let vt = &layout.visible_toasts[0];
        let cx = (vt.bounds.x + vt.bounds.width / 2.0) as u32;
        let cy = (vt.bounds.y + vt.bounds.height / 2.0) as u32;
        let (r, g, b, _a) = surface.pixel(cx, cy);
        let success_bg = Color::rgb(30, 80, 30);
        assert_ne!(
            (r, g, b),
            (success_bg.r, success_bg.g, success_bg.b),
            "first frame should paint the toast off-screen, slid in from the stack's edge, \
             not already at its resting position",
        );
    }

    /// Once a toast has been visible for longer than its slide's duration,
    /// it should be painted at its resting position — the animation hands
    /// off to a static layout once it finishes, same contract as the
    /// toolbar hover fade
    /// (`toolbar::tests::hover_highlight_reaches_full_opacity_after_fade_duration`).
    #[test]
    fn toast_overlay_reaches_resting_position_after_slide_duration() {
        let overlay = sample_toast_overlay();
        let mut backend = MacBackend::new();
        backend.set_current_font(super::super::text::make_font("Menlo", 14.0).expect("Menlo"));

        let _first = paint_toast_overlay(&mut backend, &overlay);

        // The slide's duration (220ms) plus generous margin for scheduler
        // jitter — mirrors the sleep-based timing tests elsewhere in this
        // crate (e.g. `toolbar::tests`, `tui::backend`, `win::run`).
        std::thread::sleep(Duration::from_millis(320));

        let (surface, layout) = paint_toast_overlay(&mut backend, &overlay);
        let vt = &layout.visible_toasts[0];
        let cx = (vt.bounds.x + vt.bounds.width / 2.0) as u32;
        let cy = (vt.bounds.y + vt.bounds.height / 2.0) as u32;
        let (r, g, b, _a) = surface.pixel(cx, cy);
        let success_bg = Color::rgb(30, 80, 30);
        assert_eq!(
            (r, g, b),
            (success_bg.r, success_bg.g, success_bg.b),
            "toast should be painted at its resting position once the slide finishes",
        );
    }
}
