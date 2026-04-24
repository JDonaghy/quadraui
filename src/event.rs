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
//!   `BACKEND_TRAIT_PROPOSAL.md` §2 for rationale.
//! - Owned data only — no closures, no non-`'static` references. A `UiEvent`
//!   can be logged, replayed, serialised for a plugin boundary, or sent
//!   across threads with no ceremony.
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
//!
//! The consequence apps rely on: **scroll wheel events dispatch to the
//! widget under the cursor, regardless of which widget has keyboard focus.**
//! Native convention on Win32, Cocoa, and GTK.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::types::{Modifiers, WidgetId};
use crate::{
    ActivityBarEvent, FormEvent, ListViewEvent, PaletteEvent, StatusBarEvent, TabBarEvent,
    TerminalEvent, TextDisplayEvent, TreeEvent,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
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

    /// A character was typed (IME-composed, ready for insertion). Routes
    /// to the focused text-input widget.
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
    MouseEntered {
        widget: WidgetId,
    },
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
    WindowClose,
    WindowFocused(bool),
    DpiChanged(f32),

    // ── Drops + paste ──────────────────────────────────────────────────
    FilesDropped {
        paths: Vec<PathBuf>,
        position: Point,
    },
    ClipboardPaste(String),

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

    // ── Escape hatch ───────────────────────────────────────────────────
    /// Backend-specific event the crate couldn't normalise. Apps ignore
    /// unless they want to special-case a platform.
    BackendNative(BackendNativeEvent),
}
