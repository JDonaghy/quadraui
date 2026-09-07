//! Backend-neutral event type.
//!
//! `UiEvent` is what flows up from the active [`Backend`][crate::Backend] to
//! the app each frame. Backends translate their native input events (crossterm
//! key events, GTK signals, Win32 messages, Cocoa responder methods) into
//! `UiEvent` variants; apps dispatch on the variant without caring which
//! backend produced it.
//!
//! ## Invariants
//!
//! Every `UiEvent` satisfies:
//! - `Debug + Clone + PartialEq + Serialize + Deserialize` — see
//!   `docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §2 for rationale. One
//!   variant, [`Self::User`], satisfies the `Serialize`/`Deserialize`/
//!   `PartialEq` half only nominally — see [`UserPayload`]'s doc for why
//!   an opaque `Any` payload can't do better and what its impls actually
//!   do instead.
//! - Owned data only — no closures, no non-`'static` references. A `UiEvent`
//!   can be logged, replayed, serialised for a plugin boundary, or sent
//!   across threads with no ceremony. [`Self::User`] is the one exception
//!   to "serialised for a plugin boundary" — it's an in-process-only
//!   handoff by design (issue #831).
//! - Mouse events carry `Option<WidgetId>` — the backend does hit-testing
//!   **before** emitting so apps dispatch on widget identity.
//!
//! ## Event routing — hit-test vs focus
//!
//! | Class | Routed by |
//! |---|---|
//! | Mouse (`MouseDown`, `MouseUp`, `MouseMoved`, `MouseEntered`, `MouseLeft`, `DoubleClick`, `Scroll`) | Hit-test at cursor position |
//! | Keyboard (`KeyPressed`, `CharTyped`) | Focus |
//! | Accelerator | [`AcceleratorScope`][crate::AcceleratorScope] |
//! | Window (`WindowResized`, `WindowClose`, `WindowFocused`, `DpiChanged`) | Application-global |
//! | `FilesDropped` | Hit-test at drop position |
//! | `ClipboardPaste` | Focus |
//! | `TextCopied` | Broadcast (no target) |
//! | `FocusChanged` | Broadcast — names the new focus, if any |
//!
//! The consequence apps rely on: **scroll wheel events dispatch to the
//! widget under the cursor, regardless of which widget has keyboard focus.**
//! Native convention on Win32, Cocoa, and GTK.
//!
//! ## Emission conformance — required vs. optional (issue #501)
//!
//! Not every backend emits every variant, and not every variant needs
//! to be emitted by every backend to be conformant. `docs/BACKEND.md`'s
//! "UiEvent emission matrix" is the published required/optional table
//! per backend, kept in sync with `docs/TESTING.md`'s C2 conformance
//! tier; `docs/decisions/DECISIONS.md`'s D-010 records the per-variant disposition
//! (wire / optional-capability / keep-undocumented-no-longer) this doc
//! comment reflects. Two variants get their own doc-comment note below
//! because the matrix alone doesn't explain *why*: [`Self::CharTyped`]
//! (the IME-vs-raw-keystroke duality) and [`Self::WindowClose`] (wired
//! for GTK/Win, tracked as a gap elsewhere for macOS/TUI).

use serde::{Deserialize, Serialize};
use std::any::Any;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::types::{Modifiers, WidgetId};
use crate::{
    ActivityBarEvent, ChartEvent, DataTableEvent, FormEvent, ListViewEvent, PaletteEvent,
    StatusBarEvent, TabBarEvent, TerminalEvent, TextDisplayEvent, TreeEvent,
};

// ─── Supporting types ───────────────────────────────────────────────────────

/// Keyboard key identity — a printable character or a named non-printable.
///
/// Apps that want every keystroke (text inputs, terminal passthrough) match
/// on `Key`; apps that only want keybindings prefer [`UiEvent::Accelerator`]
/// which already resolves the key + modifiers to a declared
/// [`AcceleratorId`][crate::AcceleratorId].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Key {
    /// Printable character (after keyboard layout resolution).
    Char(char),
    /// Named non-printable key.
    Named(NamedKey),
}

/// Non-printable keyboard keys that have a stable cross-platform name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NamedKey {
    Escape,
    Tab,
    BackTab,
    Enter,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    /// Function key 1-24. Values outside that range are backend-specific
    /// (emitted via [`UiEvent::BackendNative`] instead).
    F(u8),
    /// Caps lock, num lock, scroll lock — typically consumed by the OS but
    /// emitted for completeness.
    CapsLock,
    NumLock,
    ScrollLock,
    /// Menu / application key (right-click keyboard equivalent).
    Menu,
}

/// Mouse button identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
    /// Back / forward navigation buttons on 5-button mice.
    X1,
    X2,
    /// Backend-specific button index.
    Other(u8),
}

/// Bitmask of mouse buttons currently held down during a `MouseMoved` event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ButtonMask {
    #[serde(default)]
    pub left: bool,
    #[serde(default)]
    pub right: bool,
    #[serde(default)]
    pub middle: bool,
}

/// Cursor position in the backend's native units.
///
/// - **TUI**: whole cells (typically integral values stored as `f32`).
/// - **GTK**: device-independent pixels (Cairo / Pango coordinates).
/// - **Win-GUI**: Direct2D DIPs.
/// - **macOS** (planned): Core Graphics points.
///
/// Apps that need to convert should use [`Viewport::scale`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Scroll-wheel delta. Positive `y` = scroll up (toward the top of content).
/// Backends that report scroll in lines/cells/pixels normalise to their
/// native unit before emitting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ScrollDelta {
    pub x: f32,
    pub y: f32,
}

impl ScrollDelta {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Rectangular region in the backend's native units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.x + self.width && p.y >= self.y && p.y < self.y + self.height
    }
}

/// Backend viewport dimensions in native units.
///
/// TUI: `width` and `height` are cell counts; `scale = 1.0`.
/// GTK / Win-GUI / macOS: pixel-ish units with `scale` = DPI ratio.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl Viewport {
    pub const fn new(width: f32, height: f32, scale: f32) -> Self {
        Self {
            width,
            height,
            scale,
        }
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new(80.0, 24.0, 1.0)
    }
}

/// Backend-specific event the crate couldn't normalise.
///
/// The `payload` is an opaque backend-defined string (typically JSON). Apps
/// ignore this variant by default; only special-case it when a specific
/// platform feature is required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendNativeEvent {
    /// Backend identifier — matches [`Backend::platform_name`][crate::PlatformServices::platform_name].
    pub backend: String,
    /// Short name for the native event, e.g. `"win32.wm_sizing"`.
    pub kind: String,
    /// Opaque payload. Apps choosing to handle this variant parse it
    /// per-backend.
    pub payload: String,
}

// ─── Background-thread wake payload (issue #831) ───────────────────────────

/// Opaque payload carried by [`UiEvent::User`].
///
/// Every other `UiEvent` variant satisfies this module's documented
/// invariant — `Debug + Clone + PartialEq + Serialize + Deserialize` — for
/// free, via the enum-level `#[derive(..)]`. An arbitrary app value can't:
/// a boxed trait object has no generic `Clone` (the vtable doesn't carry
/// one), no generic equality, and nothing to serialise into. `UserPayload`
/// is a hand-rolled wrapper that satisfies the same four traits by
/// necessity rather than by content:
///
/// - **`Clone`** — wraps `Arc<dyn Any + Send + Sync>` rather than
///   `Box<dyn Any + Send>`, so cloning bumps a refcount instead of
///   requiring the erased payload to know how to duplicate itself. This is
///   also what makes a `UiEvent::User` cross-thread-safe *after* delivery
///   (`Send + Sync`), on top of `Backend::waker`'s closure needing `Send`
///   to be called from a background thread in the first place.
/// - **`PartialEq`** — pointer identity ([`Arc::ptr_eq`]), the only
///   equality relation that makes sense for an erased type with no `Eq`
///   bound of its own.
/// - **`Debug`** — a fixed placeholder; the payload's real shape is opaque
///   by design.
/// - **`Serialize`/`Deserialize`** — both fail at the call site with a
///   descriptive error instead of failing to compile. `UiEvent::User` is
///   an in-process-only handoff (a background thread's result, delivered
///   through [`crate::Backend::waker`]) — it never legitimately crosses a
///   JSON boundary (a conformance scenario replay, a plugin wire format),
///   so there is no "right" wire representation to invent. A caller that
///   accidentally serialises a `Vec<UiEvent>` containing one gets a clear
///   error naming the cause instead of a silently-dropped field or a type
///   that doesn't compile for every other, genuinely serialisable variant.
pub struct UserPayload(Arc<dyn Any + Send + Sync>);

impl UserPayload {
    /// Wrap `value` for delivery as `UiEvent::User` — see
    /// [`crate::Backend::waker`] for the intended background-thread usage.
    pub fn new<T: Any + Send + Sync + 'static>(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// Attempt to recover the original value. Returns `None` if `T` doesn't
    /// match the concrete type [`Self::new`] was called with.
    pub fn downcast_ref<T: Any + Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }
}

// The raw-`Arc` seam between `UserPayload` and `crate::runtime::UserEventQueue`.
// Gated on exactly the same predicate as `mod runtime` in `lib.rs`, because
// those are its only callers: `from_arc` from `UserEventQueue::drain_into`,
// `into_arc` from each backend's `Backend::waker` closure (tui, gtk, macos,
// win). A default-features build (no backend feature at all) compiles
// `UserPayload` — `UiEvent::User` is unconditional — but not `runtime`, so
// without this gate both methods are dead code and `-D warnings` fails the
// `cargo package` verification build. Keep this cfg in lockstep with
// `lib.rs`'s `mod runtime` gate; a backend added to one must be added to the
// other.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
impl UserPayload {
    /// Crate-internal constructor for backends draining their own
    /// already-erased staging queue (`crate::runtime::UserEventQueue`)
    /// straight into a `UiEvent::User`.
    pub(crate) fn from_arc(payload: Arc<dyn Any + Send + Sync>) -> Self {
        Self(payload)
    }

    /// Crate-internal inverse of [`Self::from_arc`] — recovers the raw
    /// `Arc` so a `Backend::waker` closure can push it straight into
    /// [`crate::runtime::UserEventQueue`] without re-wrapping.
    pub(crate) fn into_arc(self) -> Arc<dyn Any + Send + Sync> {
        self.0
    }
}

impl fmt::Debug for UserPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("UserPayload").field(&"<opaque>").finish()
    }
}

impl Clone for UserPayload {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl PartialEq for UserPayload {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Serialize for UserPayload {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom(
            "UiEvent::User carries an opaque Arc<dyn Any> payload and cannot be serialised",
        ))
    }
}

impl<'de> Deserialize<'de> for UserPayload {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "UiEvent::User cannot be deserialised — it is an in-process-only \
             background-thread wake payload, never a wire value",
        ))
    }
}

// ─── The main event enum ────────────────────────────────────────────────────

/// Everything a user (or platform) can do that an app might care about.
///
/// Produced by [`Backend::poll_events`][crate::Backend::poll_events] every
/// frame; consumed by app dispatch code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UiEvent {
    // ── Input ───────────────────────────────────────────────────────────
    /// A declared accelerator fired. The backend matched a
    /// [`KeyBinding`][crate::KeyBinding] to one of the app's registered
    /// accelerators and is reporting the ID.
    Accelerator(crate::AcceleratorId, Modifiers),

    /// A raw key was pressed. Routes to the focused widget. Apps that only
    /// want keybindings should prefer `Accelerator` — the backend handles
    /// match-and-dispatch for them.
    KeyPressed {
        key: Key,
        modifiers: Modifiers,
        repeat: bool,
    },

    /// An IME **committed** the composed text for one character. Routes
    /// to the focused text-input widget, same as [`Self::KeyPressed`].
    ///
    /// **This is not "a character was typed" in general — that's
    /// `KeyPressed { key: Key::Char(c), .. }`, which every backend
    /// already emits and which every text-input consumer in this crate
    /// (`compose::folder_picker`, and the plain-typing path inside
    /// `compose::{tree_controller,sidebar_system,chat_controller}`)
    /// treats as the canonical, always-present text-input event.**
    /// `CharTyped` is reserved exclusively for the *output* of an IME
    /// composition sequence (dead-key accents, Japanese/Chinese/Korean
    /// input, emoji picker commit) — text a user produced through
    /// multiple keystrokes/UI interactions that resolve to one commit,
    /// which has no sensible `KeyPressed` translation of its own.
    ///
    /// No backend implements IME today (tracked under epic #481, IME
    /// design story #502) and consequently **no backend emits this
    /// variant** — every `KeyPressed{Key::Char}` a user types already
    /// reaches apps through the always-on path above. A future IME
    /// backend must not emit both for the same keystroke: real IME
    /// composition consumes the raw keydown while a composition is in
    /// progress (nothing reaches `KeyPressed` for it), and only the
    /// final commit produces `CharTyped`. Emitting both would double-
    /// insert on any consumer that (correctly, per its own doc) listens
    /// to both — `sidebar_system::SidebarSystem::handle_inner` and
    /// `tree_controller::TreeController::handle` both call
    /// `edit_insert_char` from *either* event today, on the assumption
    /// they're mutually exclusive per keystroke.
    CharTyped(char),

    // ── Mouse ──────────────────────────────────────────────────────────
    MouseDown {
        widget: Option<WidgetId>,
        button: MouseButton,
        position: Point,
        modifiers: Modifiers,
    },
    MouseUp {
        widget: Option<WidgetId>,
        button: MouseButton,
        position: Point,
    },
    MouseMoved {
        position: Point,
        buttons: ButtonMask,
    },
    /// **Optional capability** (D-010, issue #501) — no backend emits
    /// this today, and no in-tree or downstream consumer matches on it.
    /// Kept (not removed) because hover state is a real, likely-future
    /// desktop need (tooltip auto-show, hover highlighting) that has no
    /// substitute mechanism the way the D-008 dead `*Event` enums did
    /// (those had a working `*Hit`/`*Layout` replacement already
    /// shipping; this doesn't). A backend that never emits it is fully
    /// conformant — declare the gap, don't fake it.
    MouseEntered {
        widget: WidgetId,
    },
    /// See [`Self::MouseEntered`] — same optional-capability status,
    /// same rationale, always emitted/not-emitted as a pair.
    MouseLeft {
        widget: WidgetId,
    },
    DoubleClick {
        widget: Option<WidgetId>,
        position: Point,
    },
    /// Scroll-wheel event. **Routes to the widget under the cursor, not
    /// to the focused widget.** This is the native convention on every
    /// major desktop platform.
    Scroll {
        widget: Option<WidgetId>,
        delta: ScrollDelta,
        position: Point,
    },

    // ── Window ─────────────────────────────────────────────────────────
    WindowResized {
        viewport: Viewport,
    },
    /// **Required capability on every windowed (non-TUI) backend**
    /// (D-010, issue #501) — the app's only hook to observe or veto an
    /// OS-level window close (the "×" button, Alt-F4, window-manager
    /// close). Not applicable to TUI: a terminal has no OS window to
    /// close independently of the process exiting, so TUI legitimately
    /// never emits this. GTK's `close-request` signal (`gtk::run`) and
    /// Win's `WM_CLOSE` (`win::run`) both dispatch this event and only
    /// let the close proceed when the app's `Reaction` is `Exit` —
    /// anything else vetoes it. macOS wiring is tracked separately
    /// (issue #486's window-lifecycle scope), not by this issue.
    WindowClose,
    WindowFocused(bool),
    /// **Optional capability** (D-010, issue #501; wired for GTK/macOS by
    /// issue #834) — Win emits this from `WM_DPICHANGED` (`win::run`);
    /// GTK observes the `DrawingArea`'s `notify::scale-factor` signal
    /// (`gtk::run`) and macOS observes
    /// `NSWindowDidChangeBackingPropertiesNotification` (`macos::run`),
    /// both firing on a live runtime DPI change (monitor move, external
    /// monitor plug/unplug) as well as at startup; TUI is always
    /// `scale == 1.0`, so it never applies there. `crate::runtime::preprocess_event`
    /// forces `EventOutcome::Redraw` for this variant regardless of what
    /// `AppLogic::handle` itself returns, so a repaint (and therefore a
    /// re-measure of any DPI-dependent layout) always follows receipt —
    /// see that function's doc for why an app-optional redraw isn't
    /// enough here.
    DpiChanged(f32),

    // ── Drops + paste ──────────────────────────────────────────────────
    /// **Optional capability** (D-010, issue #501; wired by issue #834) —
    /// emitted by GTK (`gtk::DropTarget` on the `DrawingArea`), macOS
    /// (`NSDraggingDestination` on `QuadraView`), and Windows
    /// (`WM_DROPFILES`) when the user drops OS files onto the window.
    /// Not applicable to TUI: a terminal has no native drag-and-drop
    /// surface. `position` is in the same native units as every other
    /// pointer event (DIPs for GTK/macOS/Win).
    FilesDropped {
        paths: Vec<PathBuf>,
        position: Point,
    },
    ClipboardPaste(String),

    // ── Clipboard copy notification ────────────────────────────────
    /// Text was copied to the clipboard by the TUI runner's built-in
    /// text-selection mechanism (click-drag → Ctrl-C). The payload is
    /// the text that was copied.
    ///
    /// This event is **distinct from [`Self::ClipboardPaste`]**. Paste
    /// means "insert text into the focused input"; `TextCopied` is a
    /// one-way notification that lets apps display a confirmation badge
    /// ("Copied!") without any risk of accidentally inserting text into
    /// an input widget that happens to handle `ClipboardPaste`.
    ///
    /// Routing: broadcast (no widget target — it is not input-focus
    /// routed). Apps that don't need copy confirmation can ignore it.
    TextCopied(String),

    // ── Focus ────────────────────────────────────────────────────────
    /// Keyboard focus moved (or was cleared) — issue #830.
    ///
    /// Emitted by the shared runner pipeline
    /// ([`crate::runtime::preprocess_event`]) whenever
    /// [`crate::focus::FocusManager::focus_next`]/`focus_prev` (driven
    /// by Tab/Shift+Tab) or [`crate::focus::FocusManager::set_focus`]
    /// actually changes [`crate::Backend::focus_manager`]'s
    /// [`crate::focus::FocusManager::focused`] value. `None` means focus
    /// was cleared — no widget focused. A no-op cycle (e.g. Tab in a
    /// single-widget tab order) does not re-fire this event.
    ///
    /// Routing: broadcast, like [`Self::TextCopied`] — apps read the new
    /// value directly off the payload (or off
    /// [`crate::Backend::focus_manager`] at any later point) rather than
    /// this being routed to a specific widget's handler. Only fires for
    /// apps that opt in by returning a non-empty
    /// [`crate::runner::AppLogic::tab_stops`] — see that method's doc
    /// for the full opt-in contract, including why Tab/Shift+Tab stop
    /// reaching [`Self::KeyPressed`] once an app opts in.
    FocusChanged(Option<WidgetId>),

    // ── Cross-primitive scroll event ──────────────────────────────────
    /// A scrollbar drag or click resolved to a new offset. Generic
    /// over widget type — the `widget` field carries whatever
    /// `WidgetId` the app used when calling
    /// [`crate::DragState::begin`] with
    /// [`crate::DragTarget::ScrollbarY`]. Apps dispatch on `widget`
    /// and apply `new_offset` to the corresponding scroll-state
    /// field (palette `scroll_offset`, tree `scroll_top`, etc.).
    /// Replaces the old per-primitive `ScrollOffsetChanged` variants.
    ScrollOffsetChanged {
        widget: WidgetId,
        new_offset: usize,
    },

    // ── Text selection ────────────────────────────────────────────────
    /// A mouse text-selection drag extended or finalised its range.
    /// Emitted by [`crate::dispatch::dispatch_mouse_drag`] while a
    /// [`crate::DragTarget::TextSelection`] is active.
    ///
    /// `anchor` is the screen position where the drag started (set
    /// once at click-down; does not change during the drag). `focus`
    /// is the current cursor position. Both are in the backend's
    /// native units (TUI: cells; GTK/macOS: pixels).
    ///
    /// The range `anchor → focus` defines what is currently selected —
    /// call [`crate::dispatch::text_selection_line_range`] to convert
    /// the pair to a list of `(row, col_start, col_end)` spans.
    /// The TUI backend applies selection highlights automatically
    /// inside the draw loop; other backends may consume this event to
    /// drive native selection highlight.
    ///
    /// If the user releases without moving (plain click), this event
    /// is never emitted; the app receives a plain `MouseDown + MouseUp`
    /// pair instead.
    TextSelectionChanged {
        region: WidgetId,
        anchor: Point,
        focus: Point,
    },

    // ── Split-tree divider drag ────────────────────────────────────────
    /// A [`crate::SplitTree`] divider drag moved. Emitted by
    /// [`crate::dispatch::dispatch_mouse_drag`] while a
    /// [`crate::DragTarget::SplitDivider`] is active.
    ///
    /// `tree` identifies which tree (apps may host more than one — e.g.
    /// vimcode's editor-group tree and per-group window tree); apps
    /// dispatch on it and apply the update to the corresponding
    /// `SplitTree` state. `split_index` is the pre-order index of the
    /// `SplitTree::Split` node being resized (see
    /// [`crate::SplitTreeDivider::split_index`]). `new_ratio` is the
    /// resolved, already-clamped `0.0..=1.0` ratio ready to feed
    /// directly to [`crate::SplitTree::set_ratio_at_index`].
    SplitDividerDragged {
        tree: WidgetId,
        split_index: usize,
        new_ratio: f32,
    },

    // ── Native menu activation ────────────────────────────────────────
    /// A menu item was activated via a native menu installer
    /// ([`Backend::install_menu_bar`][crate::Backend::install_menu_bar]).
    /// Carries the activated item's `WidgetId` regardless of nesting
    /// depth — submenu structure is transparent to the app.
    ///
    /// The in-window `MenuBar` primitive resolves clicks via
    /// [`crate::MenuBarLayout::hit_test`] / [`crate::MenuBarHit::Item`]
    /// instead; this variant is specifically for system-installed menus
    /// (macOS NSMenu; future Win32 `SetMenu`).
    MenuActivated(WidgetId),

    /// A native right-click context menu (shown via
    /// [`Backend::show_context_menu`][crate::Backend::show_context_menu])
    /// had one of its items activated. Carries the activated item's
    /// `WidgetId`. Distinct from [`Self::MenuActivated`] so apps can
    /// route menu-bar and context-menu activations through different
    /// handlers when needed.
    ContextMenuItemActivated(WidgetId),

    /// A native right-click context menu was dismissed — either after
    /// activation (immediately following the matching
    /// [`Self::ContextMenuItemActivated`]) or by cancel (Escape /
    /// click outside / second right-click). Apps that don't track
    /// open-menu state can ignore this variant.
    ContextMenuDismissed,

    // ── Primitive-specific events bubble up by WidgetId ───────────────
    Tree(WidgetId, TreeEvent),
    List(WidgetId, ListViewEvent),
    Form(WidgetId, FormEvent),
    Palette(WidgetId, PaletteEvent),
    TabBar(WidgetId, TabBarEvent),
    StatusBar(WidgetId, StatusBarEvent),
    ActivityBar(WidgetId, ActivityBarEvent),
    Terminal(WidgetId, TerminalEvent),
    TextDisplay(WidgetId, TextDisplayEvent),
    Chart(WidgetId, ChartEvent),
    DataTable(WidgetId, DataTableEvent),

    // ── Escape hatch ───────────────────────────────────────────────────
    /// Backend-specific event the crate couldn't normalise. Apps ignore
    /// unless they want to special-case a platform.
    BackendNative(BackendNativeEvent),

    // ── Background-thread wake (issue #831) ─────────────────────────────
    /// An app-defined value delivered from a background thread through
    /// [`crate::Backend::waker`].
    ///
    /// Before #831, every runner was poll-only: an app doing I/O had no
    /// way to be told a background task finished short of checking its own
    /// channel/flag from [`crate::runner::AppLogic::tick`] on whatever
    /// cadence the backend happens to poll at (TUI: every 16ms; GTK: every
    /// 33ms; macOS/Windows: never, absent a live redraw or native event —
    /// see `Backend::waker`'s doc for why that made this the more
    /// consequential gap on those two). `waker()` returns a `Send + Sync`
    /// closure any thread can call with a [`UserPayload`]; the backend
    /// wakes its event loop and delivers the payload as `UiEvent::User` to
    /// [`crate::runner::AppLogic::handle`], the same way any other event
    /// reaches the app — no polling required on the app's part.
    ///
    /// Routing: broadcast, like [`Self::TextCopied`]/[`Self::FocusChanged`]
    /// — there is no widget to hit-test or focus to route through, since
    /// the value didn't originate from input. Apps downcast the payload
    /// via [`UserPayload::downcast_ref`] against whatever concrete type
    /// they handed to [`UserPayload::new`] on the producing thread.
    User(UserPayload),
}

// ─── Shared constructors (issue #495) ───────────────────────────────────────
//
// Every backend's `events.rs` (`gtk::events`, `macos::events`, `tui::events`)
// translates native input into `UiEvent`. Only three things are genuinely
// backend-native: the keysym/keycode → `Key` table, the button-number →
// `MouseButton` map, and the modifier-mask → `Modifiers` map. The event
// *shape* — which fields `MouseDown` carries, that `widget` starts `None`
// pending hit-test, the scroll sign-flip convention — was previously
// re-typed identically in each translator. These free functions are the
// single place that shape lives; backends call them instead of writing the
// `UiEvent::MouseDown { .. }` struct literal themselves.
//
// A new backend's event translation (see `BACKEND.md`'s worked estimate)
// only needs to supply the three native-mapping tables and call these —
// roughly 40 lines, not the 150+ each of `gtk::events` / `macos::events`
// carried before this module existed.

/// Build a [`UiEvent::MouseDown`]. `widget` always starts `None` — hit-test
/// resolution happens downstream in [`crate::dispatch`], not in the
/// translator.
pub fn mouse_down(button: MouseButton, x: f32, y: f32, modifiers: Modifiers) -> UiEvent {
    UiEvent::MouseDown {
        widget: None,
        button,
        position: Point::new(x, y),
        modifiers,
    }
}

/// Build a [`UiEvent::MouseUp`].
pub fn mouse_up(button: MouseButton, x: f32, y: f32) -> UiEvent {
    UiEvent::MouseUp {
        widget: None,
        button,
        position: Point::new(x, y),
    }
}

/// Build a [`UiEvent::MouseMoved`].
pub fn mouse_moved(x: f32, y: f32, buttons: ButtonMask) -> UiEvent {
    UiEvent::MouseMoved {
        position: Point::new(x, y),
        buttons,
    }
}

/// Build a [`UiEvent::Scroll`], owning the native-to-quadraui sign flip
/// once instead of every backend re-deriving (and re-commenting) it.
///
/// `dy_native_down_positive` is the backend's raw vertical delta using the
/// convention where **positive means scroll-down / content-forward** — GTK
/// `EventControllerScroll`'s `dy`, Cocoa `NSEvent.scrollingDeltaY`, and
/// crossterm's `ScrollUp`/`ScrollDown` (translated to the equivalent -1.0 /
/// +1.0 notch) all use this convention natively. `UiEvent::Scroll::delta`'s
/// convention is the opposite — **positive `y` = up, toward the top of
/// content** — so this negates once, here, rather than in each caller.
///
/// `dx` is passed straight through unnegated: every native backend
/// quadraui supports already agrees with quadraui's positive-x-is-right
/// convention, so there is no flip to own.
///
/// Win-GUI is the one native backend that does *not* follow the
/// positive-down convention on its raw delta (`win::events::win_wheel_to_uievent`'s
/// doc explains why) — its translator does not call this constructor.
pub fn scroll(dx: f32, dy_native_down_positive: f32, x: f32, y: f32) -> UiEvent {
    UiEvent::Scroll {
        widget: None,
        delta: ScrollDelta::new(dx, -dy_native_down_positive),
        position: Point::new(x, y),
    }
}

/// Build a [`UiEvent::WindowResized`].
pub fn window_resized(width: f32, height: f32, scale: f32) -> UiEvent {
    UiEvent::WindowResized {
        viewport: Viewport::new(width, height, scale),
    }
}

/// Build a [`UiEvent::FilesDropped`] (issue #834). `x`/`y` are the drop
/// position in the backend's native units — already-scaled DIPs for
/// GTK/macOS/Win, matching every other pointer event's convention.
pub fn files_dropped(paths: Vec<PathBuf>, x: f32, y: f32) -> UiEvent {
    UiEvent::FilesDropped {
        paths,
        position: Point::new(x, y),
    }
}

#[cfg(test)]
mod user_payload_tests {
    use super::*;

    #[test]
    fn downcast_ref_recovers_the_original_type() {
        let payload = UserPayload::new(42_i32);
        assert_eq!(payload.downcast_ref::<i32>(), Some(&42));
        assert_eq!(payload.downcast_ref::<String>(), None);
    }

    #[test]
    fn clone_is_a_shallow_arc_clone_not_a_deep_copy() {
        let payload = UserPayload::new(String::from("hello"));
        let cloned = payload.clone();
        // Both point at the exact same allocation — cloning a `UserPayload`
        // must not require the erased inner type to implement `Clone`.
        assert_eq!(
            cloned.downcast_ref::<String>(),
            payload.downcast_ref::<String>()
        );
        assert_eq!(payload, cloned, "a clone must be `==` to its source");
    }

    #[test]
    fn partial_eq_is_pointer_identity_not_structural() {
        let a = UserPayload::new(1_i32);
        let b = UserPayload::new(1_i32);
        assert_ne!(
            a, b,
            "two payloads with equal contents but distinct allocations must not be equal — \
             there is no generic structural equality for an erased Any"
        );
        let a_clone = a.clone();
        assert_eq!(a, a_clone, "a clone of the same allocation must be equal");
    }

    #[test]
    fn debug_never_panics_and_never_exposes_raw_content() {
        let payload = UserPayload::new("do not leak me".to_string());
        let rendered = format!("{payload:?}");
        assert!(
            !rendered.contains("do not leak me"),
            "Debug must print a placeholder, not the erased payload's contents"
        );
    }

    #[test]
    fn serialize_fails_with_a_descriptive_error_instead_of_panicking() {
        let payload = UserPayload::new(1_i32);
        let err = serde_json::to_string(&payload)
            .expect_err("UiEvent::User payloads must not be serialisable");
        assert!(err.to_string().contains("cannot be serialised"));
    }

    #[test]
    fn deserialize_fails_with_a_descriptive_error_instead_of_panicking() {
        let err = serde_json::from_str::<UserPayload>("null")
            .expect_err("UiEvent::User must never be constructible from wire data");
        assert!(err.to_string().contains("cannot be deserialised"));
    }

    /// The whole point of #831: a `UserPayload` must round-trip through a
    /// `UiEvent::User` and back out through the crate's own `Clone`/
    /// `PartialEq`/`Debug` derive on the enum — i.e. adding this variant
    /// didn't require weakening any of `UiEvent`'s other documented
    /// invariants for every *other* variant.
    #[test]
    fn user_event_clones_and_compares_via_the_wrapped_payload() {
        let ev = UiEvent::User(UserPayload::new(7_i32));
        let cloned = ev.clone();
        assert_eq!(ev, cloned);
        match &cloned {
            UiEvent::User(payload) => assert_eq!(payload.downcast_ref::<i32>(), Some(&7)),
            other => panic!("expected UiEvent::User, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod shared_constructor_tests {
    use super::*;

    #[test]
    fn mouse_down_builds_expected_shape() {
        let mods = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let ev = mouse_down(MouseButton::Left, 1.0, 2.0, mods);
        match ev {
            UiEvent::MouseDown {
                widget,
                button,
                position,
                modifiers,
            } => {
                assert!(widget.is_none());
                assert_eq!(button, MouseButton::Left);
                assert_eq!(position, Point::new(1.0, 2.0));
                assert!(modifiers.ctrl);
            }
            other => panic!("expected MouseDown, got {other:?}"),
        }
    }

    #[test]
    fn mouse_up_builds_expected_shape() {
        let ev = mouse_up(MouseButton::Right, 3.0, 4.0);
        assert_eq!(
            ev,
            UiEvent::MouseUp {
                widget: None,
                button: MouseButton::Right,
                position: Point::new(3.0, 4.0),
            }
        );
    }

    #[test]
    fn mouse_moved_builds_expected_shape() {
        let buttons = ButtonMask {
            left: true,
            ..Default::default()
        };
        let ev = mouse_moved(5.0, 6.0, buttons);
        assert_eq!(
            ev,
            UiEvent::MouseMoved {
                position: Point::new(5.0, 6.0),
                buttons,
            }
        );
    }

    #[test]
    fn scroll_negates_native_down_positive_dy() {
        // Native wheel-down (dy = +1) becomes quadraui's "not up" (-1).
        let ev = scroll(0.0, 1.0, 10.0, 20.0);
        assert_eq!(
            ev,
            UiEvent::Scroll {
                widget: None,
                delta: ScrollDelta::new(0.0, -1.0),
                position: Point::new(10.0, 20.0),
            }
        );
    }

    #[test]
    fn scroll_leaves_dx_unflipped() {
        let ev = scroll(2.5, 0.0, 0.0, 0.0);
        match ev {
            UiEvent::Scroll { delta, .. } => {
                assert_eq!(delta.x, 2.5);
                assert_eq!(delta.y, 0.0);
            }
            other => panic!("expected Scroll, got {other:?}"),
        }
    }

    #[test]
    fn window_resized_builds_expected_shape() {
        let ev = window_resized(1920.0, 1080.0, 2.0);
        assert_eq!(
            ev,
            UiEvent::WindowResized {
                viewport: Viewport::new(1920.0, 1080.0, 2.0),
            }
        );
    }
}
