//! The `Backend` trait — one implementation per platform target.
//!
//! Each backend (TUI, GTK, Win-GUI, and eventually macOS) implements this
//! trait. Apps write render code once, parameterised over `<B: Backend>`,
//! and every supported platform rasterises the same primitive descriptions
//! with platform-native drawing + input.
//!
//! See `quadraui/docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §4 for design rationale.
//!
//! ## `draw_*` here vs. `Surface` in `frame.rs` (issue #456)
//!
//! Every `draw_<name>` method below is the low-level rasteriser entry
//! point for its primitive — always public, never deprecated. A
//! consumer assembling a top-level screen from multiple primitives
//! should reach for [`crate::frame::ScreenLayout`] + [`crate::frame::Surface`]
//! instead of calling `draw_*` methods directly: it routes every
//! backend through the same call site, so two backends of one app
//! cannot silently paint the same primitive two different ways (the
//! drift #456 documents). `ScreenLayout::draw` calls these `draw_*`
//! methods internally — see `frame.rs`'s module doc and
//! `quadraui/docs/decisions/DECISIONS.md` D-006 for the full picture, including
//! the primitives that have no `Surface` variant yet and must still be
//! painted via `draw_*` directly.
//!
//! ## Coordinate frames for `*_layout` methods (issue #505)
//!
//! Every `Backend::<name>_layout` method (and its `draw_<name>` twin,
//! where one returns hit-region data) documents **which** of two frames
//! its `hit_regions` / `bounds` fields are in — there is no third option
//! and no undocumented exception:
//!
//! - **LOCAL** — relative to `rect`'s origin; `(0, 0)` is `rect`'s
//!   top-left corner. Used by primitives a parent composer paints
//!   *inline* and localises clicks for before calling `hit_test`
//!   (`tree_layout`, `form_layout`, `data_table_layout`,
//!   `text_display_layout`, `status_bar_layout`, `activity_bar_layout`,
//!   `list_layout`, `terminal_layout`).
//! - **ABSOLUTE** — shifted by `rect.x` / `rect.y`, i.e. target-surface
//!   coordinates a caller can compare directly against raw click
//!   coordinates with no further adjustment. Used by primitives that are
//!   painted as a freestanding widget at their own screen rect and whose
//!   callers don't otherwise track that rect (`tab_bar_layout`,
//!   `menu_bar_layout`, `split_layout`, `split_tree_layout`,
//!   `panel_layout`, `toast_stack_layout`, `pipeline_view_layout`,
//!   `progress_layout`, `spinner_layout`, `command_center_layout`,
//!   `toolbar_layout`, `sidebar_panel_layout`, `chart_layout`,
//!   `minimap_layout`, `msv_layout`, `text_input_layout`, `board_layout`,
//!   `editor_layout`, `command_line_layout`).
//!
//! A third category returns no coordinates at all — `diff_view_layout`
//! returns row *counts* (`visible_rows` / `total_rows`), not positions —
//! so LOCAL/ABSOLUTE doesn't apply; its doc comment says so explicitly
//! rather than silently picking neither.
//!
//! Both frames are legitimate — the rule this file enforces is that the
//! frame is *stated on the method's doc comment* and *matches what every
//! backend implementation actually returns* (see
//! `quadraui/docs/decisions/DECISIONS.md` D-005 for why the split exists and why
//! it isn't collapsed to one frame; `quadraui/docs/PRIMITIVE_RULES.md`
//! "Coordinate frames for `*_layout` methods" for the authoring rule).
//! `quadraui/docs/LESSONS.md` "Layout helpers must return coords in the
//! same frame across backends" records the bug class this convention
//! guards against: a `*_layout` twin that silently disagrees with its
//! own TUI/GTK siblings about which frame it returns.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crate::dispatch::DragState;
use crate::event::{Point, Rect, UiEvent, UserPayload, Viewport};
use crate::focus::FocusManager;
use crate::interaction::InteractionState;
use crate::modal_stack::ModalStack;
use crate::primitives::activity_bar::{ActivityBarRowHit, ActivityBarStyle};
use crate::primitives::board::{BoardLayout, BoardModel};
use crate::primitives::chart::{Chart, ChartLayout};
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout};
use crate::primitives::command_line::{CommandLine, CommandLineLayout};
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::primitives::data_table::{DataTable, DataTableLayout};
use crate::primitives::dialog::{Dialog, DialogLayout, DialogSeverity};
use crate::primitives::diff_view::{DiffMode, DiffView, DiffViewLayout};
use crate::primitives::drop_zone::DropOverlay;
use crate::primitives::editor::{CursorShape as EditorCursorShape, Editor, EditorLayout};
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::form::FormLayout;
use crate::primitives::image::{Image, ImageSource};
use crate::primitives::list::ListViewLayout;
use crate::primitives::menu_bar::{MenuBar, MenuBarLayout};
use crate::primitives::message_list::MessageList;
use crate::primitives::minimap::{Minimap, MinimapLayout};
use crate::primitives::multi_section_view::{
    MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout,
};
use crate::primitives::panel::{Panel, PanelLayout};
use crate::primitives::pipeline_view::{PipelineView, PipelineViewLayout};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::sidebar_panel::{SidebarPanel, SidebarPanelLayout};
use crate::primitives::spinner::{Spinner, SpinnerLayout};
use crate::primitives::split::{Split, SplitLayout};
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
use crate::primitives::status_bar::StatusBarLayout;
// `TabBarHits` is `#[deprecated]` (issue #823) — this trait still returns it
// from six methods below (the "real, separate follow-up work" its own doc
// names), so the import itself needs the same allow every use site does.
#[allow(deprecated)]
use crate::primitives::tab_bar::{TabBarHits, TabBarLayout, TabChrome, TabIcon};
use crate::primitives::text_display::TextDisplayLayout;
use crate::primitives::text_input::{TextInput, TextInputLayout};
use crate::primitives::toast::{ToastStack, ToastStackLayout};
use crate::primitives::toolbar::{Toolbar, ToolbarLayout};
use crate::primitives::tooltip::{Tooltip, TooltipChrome, TooltipLayout};
use crate::primitives::tree::TreeViewLayout;
use crate::types::{Color, WidgetId};
use crate::{
    Accelerator, AcceleratorId, ActivityBar, Form, ListView, Palette, PaletteLayout, StatusBar,
    TabBar, Terminal, TextDisplay, TreeView,
};
use serde::{Deserialize, Serialize};

/// Which edge or corner of a window a resize gesture originates from.
/// Mirrors `gdk4::SurfaceEdge` 1:1 (see [`Backend::begin_window_resize`]) so
/// the GTK backend's conversion is a plain match with no ambiguity, but the
/// type itself is backend-neutral — TUI and other backends just no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// OS mouse-pointer glyph hint (see [`Backend::set_cursor`]).
///
/// Deliberately named `PointerShape`, not `CursorShape` — that name is
/// already taken by [`crate::primitives::editor::CursorShape`] (the
/// text-editor caret shape, re-exported as `EditorCursorShape`), which is an
/// unrelated concept (text caret vs. OS mouse pointer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerShape {
    /// The platform's normal arrow/default pointer.
    Default,
    /// A directional resize pointer for the given window edge/corner.
    Resize(ResizeEdge),
}

/// Window-state control surface — set/get the title, size, position and
/// tri-state chrome (fullscreen/always-on-top/minimize) of the OS window a
/// backend owns, once a real window exists (issue #950).
///
/// Reached through [`Backend::window`], which returns `Option<&mut dyn
/// WindowControl>` rather than requiring every backend to implement this
/// trait unconditionally — `None` is the honest, *structural* answer for
/// a backend with no OS window at all (TUI has no window even though it
/// implements [`Self::set_title`] — see that method's doc) or for any
/// backend before its window is constructed (the same "no window yet"
/// state [`Backend::begin_window_drag`] and friends already handle by
/// returning `false`). There is deliberately no `BackendCaps` bool for
/// "has a window at all" to keep in sync by hand — `Backend::window`
/// returning `Option` makes the absence a compile-time-checkable match
/// arm instead of a capability flag that could drift from reality.
///
/// Individual methods still return [`ServiceResult<()>`] (D-009 seam-2
/// shape, same as [`PlatformServices`]/[`Clipboard`]'s `_result` twins)
/// rather than a bare `bool`/no-op: `Backend::window` returning `Some`
/// only promises *some* window-control surface is real, not that every
/// method on it is — GTK4/Wayland, for example, has no way to pin a
/// window always-on-top at all (X11-only, via `gdk_x11`, quadraui#950),
/// so [`Self::set_always_on_top`] must report [`BackendError::Unsupported`]
/// honestly there rather than either silently no-op'ing or making the
/// caller distrust the whole surface because one call in it can fail.
///
/// Every method defaults to `Err(BackendError::Unsupported)`, mirroring
/// [`Clipboard::read_primary_selection`]'s "the trait states a capability
/// only some implementors have" shape: a backend that implements
/// `WindowControl` at all (i.e. ever returns `Some` from
/// [`Backend::window`]) overrides only the methods it can genuinely back
/// with a native call, and callers that hit `Unsupported` on the rest
/// treat that exactly like a `None` from `Backend::window` — a real,
/// nameable gap, not a crash.
pub trait WindowControl {
    /// Set the window's title / (GTK) taskbar label / (macOS) titlebar
    /// text / (Win) `WM_SETTEXT` caption.
    ///
    /// **TUI's one genuine `WindowControl` capability**: `TuiBackend`
    /// emits the OSC 0/2 terminal escape sequence (crossterm's
    /// `SetTitle`) to retitle the terminal emulator's tab/window, which
    /// is the terminal-native equivalent of a desktop window's title —
    /// see `TuiBackend`'s `impl WindowControl` for why this is the one
    /// method TUI overrides while every other method here stays
    /// `Unsupported` on it.
    fn set_title(&mut self, _title: &str) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Resize the window's content area to `width` × `height`, in the
    /// backend's native units (DIPs on GTK/Win/macOS).
    fn set_size(&mut self, _width: f32, _height: f32) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Floor the window's resizable range at `width` × `height` — the
    /// user (and [`Self::set_size`]/[`Self::set_bounds`]) can no longer
    /// shrink it smaller.
    fn set_min_size(&mut self, _width: f32, _height: f32) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Cap the window's resizable range at `width` × `height` — the user
    /// (and [`Self::set_size`]/[`Self::set_bounds`]) can no longer grow it
    /// larger.
    fn set_max_size(&mut self, _width: f32, _height: f32) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// The window's current position + size, in the backend's native
    /// units. Position is screen-relative where the platform exposes one
    /// at all — see overriding backends' docs for platforms (GTK4/Wayland)
    /// that structurally cannot report a position and what they return
    /// instead.
    fn bounds(&self) -> ServiceResult<Rect> {
        Err(BackendError::Unsupported)
    }

    /// Move and/or resize the window to `bounds` in one call.
    fn set_bounds(&mut self, _bounds: Rect) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Center the window on its current monitor.
    fn center(&mut self) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Enter (`true`) or exit (`false`) fullscreen. Idempotent — calling
    /// with the state the window is already in is not an error.
    fn set_fullscreen(&mut self, _fullscreen: bool) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Whether the window is currently maximized/zoomed.
    ///
    /// Issue #1022: [`Backend::toggle_window_maximize`] flips the state
    /// but only reports whether the flip succeeded — it cannot tell a
    /// caller which state the window landed in, so a maximize/restore
    /// glyph (vimcode's use case) has nothing to render from. This method
    /// is the missing getter, read-only and side-effect-free, so it can
    /// be polled from a paint path without racing the toggle itself.
    fn is_maximized(&self) -> ServiceResult<bool> {
        Err(BackendError::Unsupported)
    }

    /// Pin (`true`) or unpin (`false`) the window above all others.
    ///
    /// **Not available on GTK4/Wayland** — Wayland's window-stacking
    /// model has no client-requestable always-on-top protocol (X11 only,
    /// via `gdk_x11`, quadraui#950's design note); `GtkBackend` reports
    /// [`BackendError::Unsupported`] there rather than silently doing
    /// nothing, so a caller can tell "not pinned because it's
    /// unsupported here" from "not pinned because nobody asked".
    fn set_always_on_top(&mut self, _on_top: bool) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Show (`true`) or hide (`false`) the window's native chrome
    /// (titlebar + border) without changing its content area.
    ///
    /// Issue #1022: backs vimcode's client-side-decoration path (its
    /// #552) — a host that paints its own titlebar first turns the
    /// native one off with this, rather than living with two titlebars
    /// stacked on top of each other.
    fn set_decorated(&mut self, _decorated: bool) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Minimize (iconify) the window.
    fn minimize(&mut self) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Restore a minimized window to its prior size/position.
    fn restore(&mut self) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Hide the window (removes it from the screen and taskbar/dock
    /// without minimizing) — [`Self::show`] is the inverse.
    fn hide(&mut self) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Show a window previously hidden via [`Self::hide`].
    fn show(&mut self) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Bring the window to the front and give it keyboard focus.
    fn focus(&mut self) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }
}

/// Tray / status-bar icon control surface (issue #953) — set the icon,
/// tooltip, and right-click menu of an OS notification-area/menu-bar
/// icon, once one exists.
///
/// Reached through [`Backend::tray`], which returns `Option<&mut dyn
/// TrayService>` — the same *structural* absence pattern
/// [`Backend::window`] established for [`WindowControl`] (issue #950):
/// `None` on a backend with no tray facility at all (TUI, genuinely —
/// see [`Backend::tray`]'s own doc for why that is not merely "not
/// implemented yet" the way GTK's gap is) or before a lazily-created
/// tray icon exists. Every method still returns [`ServiceResult<()>`]
/// rather than a bare `bool`, mirroring `WindowControl`'s own reasoning:
/// a backend can genuinely implement part of this surface (a status
/// item with an icon but no attached menu) without the whole surface
/// being fake.
///
/// Every method defaults to `Err(BackendError::Unsupported)` — a
/// backend that implements `TrayService` at all overrides only the
/// methods it can genuinely back with a native call.
///
/// ## Icon source
///
/// [`Self::set_icon`] takes an [`ImageSource`] — the same source type
/// [`crate::primitives::image::Image`] already decodes behind
/// `Backend::draw_image` — rather than a tray-specific type, per this
/// issue's own design note (it shares the source type with the
/// clipboard-formats work, quadraui#953).
///
/// ## Menu
///
/// [`Self::set_menu`] takes a [`ContextMenu`] — the same primitive
/// [`Backend::show_context_menu`] already renders natively — rather than
/// a tray-specific menu model, so there is one menu vocabulary and one
/// `WidgetId` activation path (`UiEvent::ContextMenuItemActivated`)
/// shared between a right-click context menu and a tray menu. See each
/// backend's `impl TrayService` for how its native menu-tracking API
/// interacts with the click event `Backend::tray`'s own doc describes
/// (some platforms show the menu automatically on any click once one is
/// attached, superseding the plain click event for that click).
pub trait TrayService {
    /// Set (or replace) the tray icon's image. The first successful call
    /// is what makes the icon appear at all on backends that create the
    /// underlying OS resource lazily (macOS's `NSStatusItem`, Windows'
    /// `Shell_NotifyIconW` slot) — a host that never calls this never
    /// shows anything in the tray, rather than showing an empty/default
    /// icon.
    fn set_icon(&mut self, _icon: ImageSource) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Set the icon's hover tooltip text.
    fn set_tooltip(&mut self, _tooltip: &str) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Attach (or replace) the icon's right-click/click menu. Passing a
    /// menu with no items is not the same as never calling this method —
    /// backends that model "no menu attached" and "menu attached with
    /// zero items" differently should treat an empty
    /// [`ContextMenu::items`] as detaching the menu.
    fn set_menu(&mut self, _menu: &ContextMenu) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }
}

/// Colour fidelity a render target actually supports (quadraui#826).
///
/// Unlike every `bool` field on [`BackendCaps`], this is not a "this
/// backend implements method X" promise — it's a runtime-detected (or
/// test-overridden) property of the *output device* a frame paints to.
/// GTK/Win-GUI/macOS are pixel-native, so they always report
/// [`Self::TrueColor`]. TUI is the one backend where this varies: a
/// real terminal may be 24-bit (`COLORTERM=truecolor`/`24bit`), 256-colour
/// (`TERM` containing `256color` — including inside tmux without
/// passthrough, which is the common case that motivated this), or
/// 16-colour ANSI (everything else — the conservative fallback for an
/// unrecognised `TERM`). See `crate::tui::caps::detect_color_depth` for
/// the detection logic and `crate::tui::color` for the SGR-quantisation
/// this value drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// Palette narrowed to the 16 basic ANSI colours. The safe fallback
    /// for a terminal this backend cannot positively identify as
    /// supporting more. Note this narrows the *palette*, not the
    /// escape-sequence family: crossterm emits these named colours via
    /// the same extended `38;5;n` / `48;5;n` form as [`Self::Indexed256`]
    /// (see `crate::tui::color::rgb_to_ansi16`), never classic `30-37` /
    /// `90-97` codes — so this does not by itself help a terminal that
    /// only understands classic 3/4-bit SGR.
    Ansi16,
    /// 8-bit / 256-colour indexed (SGR `38;5;n` / `48;5;n`).
    Indexed256,
    /// 24-bit truecolor (SGR `38;2;r;g;b` / `48;2;r;g;b`). quadraui's
    /// only behavior before quadraui#826, and still the default for
    /// every non-terminal backend.
    TrueColor,
}

impl Default for ColorDepth {
    /// `TrueColor` — matches quadraui's pre-#826 unconditional behavior
    /// (so a caller that never touches this field sees no change) and is
    /// the honest value for every pixel-native backend (GTK/Win/macOS).
    fn default() -> Self {
        Self::TrueColor
    }
}

/// What a backend actually implements, beyond the required trait surface.
///
/// quadraui#492: several `Backend` methods take a no-op (or `false`)
/// default so a new backend compiles before every optional feature is
/// wired up (see the "Default: no-op" doc comments throughout this
/// trait). That silence is exactly the problem — a backend that never
/// overrides [`Backend::install_menu_bar`] compiles identically to one
/// that has a real native menu, and nothing tells the two apart. Each
/// backend's [`Backend::backend_caps`] is the declared, honest answer:
/// "these are the optional surfaces I actually implement", so the
/// conformance runner can skip a scenario that needs one with a named
/// reason instead of either silently passing or spuriously failing.
///
/// Deliberately a plain bitflag-shaped struct — one `bool` field per
/// capability — rather than pulling in the `bitflags` crate: eleven
/// fields is small enough that a dependency buys nothing but the `|` operator,
/// and every field maps to zero or more `Backend` /
/// [`PlatformServices`] methods (documented per field below) that a
/// `BackendCaps` field of `true` promises are overridden away from their
/// no-op default.
///
/// ## This is the *only* capability vocabulary
///
/// quadraui#492 review: the conformance runner used to carry a second,
/// hand-maintained `&[&str]` per backend (`TUI_CAPS` / `GTK_CAPS` in
/// `tests/conformance.rs`) that a scenario's `requires` list matched
/// against, with nothing tying it to what the backend actually declares.
/// Two vocabularies means silent drift in both directions: a capability
/// here that no scenario could ever name, and a `requires` string no
/// backend could ever declare. So there is now exactly one list — this
/// struct's fields — and `BackendReg::caps` is [`Backend::backend_caps`]
/// itself. That is why the three *input* capabilities below
/// (`mouse`/`scroll`/`drag`) live here alongside the seven optional
/// surfaces quadraui#492 enumerates: they are what the existing Tier-1
/// scenarios gate on, and folding them in is what makes the single
/// vocabulary complete rather than merely smaller.
///
/// ## The honesty check
///
/// A `true` here is a claim about source, not a hope, and
/// `tests/conformance/caps.rs` mechanically checks it for every backend
/// in the tree (including Win/macOS, which have no conformance driver
/// yet): a declared capability whose methods are still the trait's no-op
/// default fails, and so does an *undeclared* capability whose methods
/// are overridden. Each field's doc comment below names the methods that
/// check reads; a capability that cannot be checked that way says so
/// explicitly there and in `CAP_CONTRACTS`.
///
/// Construct with [`BackendCaps::empty`] (or `..BackendCaps::empty()` in
/// struct-update syntax) plus the fields a given backend actually
/// implements — see `TuiBackend::backend_caps` / `GtkBackend::backend_caps`
/// for worked examples. `#[non_exhaustive]`-free on purpose: this is
/// in-tree-only (no external `Backend` implementors, `BACKEND.md`), so a
/// new field is a breaking change to every backend impl by design — the
/// same trade-off the `Backend` trait itself already makes for a new
/// `draw_*` method (`backend.rs` module docs, "Adding a primitive is a
/// breaking change to this trait — intentional").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackendCaps {
    /// This backend delivers pointer press/release events
    /// ([`crate::UiEvent::MouseDown`] / [`crate::UiEvent::MouseUp`]) from
    /// its native event source, so click-driven scenarios mean something
    /// on it.
    ///
    /// Not mechanically checkable: the methods that produce these
    /// ([`Backend::poll_events`] / [`Backend::wait_events`]) are
    /// *required*, so every backend "overrides" them — including the Win
    /// stub, whose bodies are `todo!()`. See `CAP_CONTRACTS`.
    pub mouse: bool,
    /// This backend delivers [`crate::UiEvent::Scroll`] from its native
    /// event source (wheel, trackpad, or terminal scroll reporting).
    /// Same non-checkability as [`Self::mouse`].
    pub scroll: bool,
    /// This backend delivers [`crate::UiEvent::MouseMoved`] while a
    /// button is held, so a press → move → release sequence is a real
    /// drag rather than two unrelated clicks. Same non-checkability as
    /// [`Self::mouse`].
    pub drag: bool,
    /// [`Backend::register_text_region`] / [`Backend::cancel_text_selection_drag`]
    /// are overridden — mouse-drag text selection highlighting is real,
    /// not a silently-dropped registration.
    pub text_selection: bool,
    /// [`Backend::install_menu_bar`] and/or [`Backend::show_context_menu`]
    /// are overridden — this backend can paint a native OS menu.
    pub native_menu: bool,
    /// At least one of [`Backend::begin_window_drag`],
    /// [`Backend::toggle_window_maximize`], [`Backend::begin_window_resize`]
    /// is overridden — client-side-decoration window chrome (drag-to-move,
    /// double-click-to-maximize, edge resize) is wired to a real window.
    pub window_chrome: bool,
    /// [`Backend::set_cursor`] is overridden — hinting the OS pointer
    /// glyph actually changes what the user sees, rather than being a
    /// `false`-returning no-op.
    pub pointer_cursor: bool,
    /// This backend positions a native IME composition window (preedit)
    /// at the caret. `false` on every backend today — quadraui has no
    /// backend-level IME method yet. See `docs/IME_INPUT_PROPOSAL.md`
    /// (issue #502) for the proposed `Backend::set_ime_cursor_area`
    /// method this flag is reserved for, and [`crate::event::UiEvent`]'s
    /// `CharTyped` doc comment for the current (pre-#502) composed-text
    /// contract; composed text arrives pre-resolved from the OS either
    /// way, so this tracks *positioning* the IME candidate window, not
    /// whether typing composed characters works at all.
    pub ime: bool,
    /// [`PlatformServices::show_file_open_dialog`] /
    /// [`PlatformServices::show_file_save_dialog`] show a real native
    /// dialog rather than unconditionally returning `None`. Unlike the
    /// fields above these two `PlatformServices` methods have no no-op
    /// default (the trait requires every backend to implement them), so
    /// this flag distinguishes "returns `None` because the user
    /// cancelled" from "returns `None` because there is no dialog at
    /// all" — the same distinction §5a exists to make visible.
    ///
    /// Not mechanically checkable for the same reason: every backend
    /// implements both methods, so their *presence* proves nothing and
    /// only running a native dialog would tell the two apart. See
    /// `CAP_CONTRACTS`.
    pub file_dialogs: bool,
    /// [`PlatformServices::show_folder_open_dialog`] shows a real native
    /// directory chooser rather than unconditionally returning `None`
    /// (quadraui#935). Split out from [`Self::file_dialogs`] rather than
    /// folded into it: a backend can plausibly have one native facility
    /// without the other, and a host reading this flag needs the honest,
    /// narrower answer — "can I open a *directory* chooser", not "does
    /// this backend have *some* file-picker facility".
    ///
    /// Not mechanically checkable for the same reason as
    /// [`Self::file_dialogs`]: every backend implements the method, so its
    /// *presence* proves nothing. See `CAP_CONTRACTS`.
    pub folder_dialogs: bool,
    /// [`PlatformServices::show_message_dialog`] shows a real native
    /// alert/message dialog rather than unconditionally returning
    /// `None`. Same no-default, same `None`-is-ambiguous shape as
    /// [`Self::file_dialogs`] — this flag is the caller's only way to
    /// tell "the user dismissed it" from "this backend has no native
    /// alert facility" (quadraui#666).
    ///
    /// [`crate::primitives::dialog::native_dialog_options`] is the pure
    /// mapping from a [`Dialog`] descriptor to
    /// [`MessageDialogOptions`], and returns `None` for a dialog
    /// carrying a [`crate::primitives::dialog::DialogTable`] or
    /// [`crate::primitives::dialog::DialogInput`] — no native alert
    /// facility hosts either, so those dialogs stay in-canvas
    /// (`draw_dialog`) even on a backend where this flag is `true`.
    ///
    /// Not mechanically checkable for the same reason as
    /// [`Self::file_dialogs`]: every backend implements the method, so
    /// its *presence* proves nothing. See `CAP_CONTRACTS`.
    pub native_dialogs: bool,
    /// [`PlatformServices::send_notification`] dispatches a real system
    /// notification rather than silently discarding it. Not mechanically
    /// checkable, same as [`Self::file_dialogs`].
    pub notifications: bool,
    /// Both [`Backend::register_font_from_memory`] and
    /// [`Backend::set_nerd_font_fallback`] are overridden — this backend
    /// can register an app-supplied font's raw bytes *and* resolve
    /// Nerd-Font (or other PUA-codepoint) glyphs to a real fallback
    /// family, instead of painting tofu (issue #929).
    ///
    /// Was `Any` rather than `All` until issue #1013: the original
    /// reasoning was that a backend may reasonably wire only the
    /// fallback half (GTK, which already has a system-installed Nerd
    /// Font to point at) or only the registration half. In practice that
    /// let `tests/conformance/caps.rs`'s honesty check pass GTK
    /// vacuously off `set_nerd_font_fallback` alone while
    /// `register_font_from_memory` silently kept the trait's no-op
    /// default — a real bug (a consumer bundling its own icon font got a
    /// silent no-op on GTK) that the `Any` proof was mechanically unable
    /// to catch. Every backend that declares this today
    /// (`MacBackend`/`WinBackend`, and `GtkBackend` since #1013's
    /// `crate::gtk::app_font`) overrides both, so `All` is both the
    /// accurate contract and the one that would have caught #1013 in CI
    /// instead of a consumer.
    pub app_font_registration: bool,
    /// [`Backend::window`] is overridden and returns `Some` at least
    /// sometimes — this backend has a real [`WindowControl`] surface
    /// rather than the trait's always-`None` default (issue #950).
    ///
    /// Doesn't promise every [`WindowControl`] method succeeds — see
    /// that trait's own doc for why individual methods (GTK's
    /// always-on-top, notably) still fail honestly with
    /// [`BackendError::Unsupported`] even on a backend that declares this
    /// `true`.
    pub window_control: bool,
    /// [`Backend::tray`] is overridden and returns `Some` at least
    /// sometimes — this backend has a real [`TrayService`] surface
    /// rather than the trait's always-`None` default (issue #953).
    ///
    /// Doesn't promise every [`TrayService`] method succeeds, same
    /// caveat as [`Self::window_control`]. TUI declares this `false`
    /// permanently — see [`Backend::tray`]'s doc for why that is a
    /// structural fact, not a gap to close.
    pub tray: bool,
    /// [`Backend::set_editor_font`] and [`Backend::set_ui_font`] are both
    /// overridden — this backend resolves CSS/Pango generic family
    /// tokens (`monospace`, `sans-serif`, `system-ui`; see
    /// [`crate::GenericFamily`]) to a real native font instead of
    /// silently discarding whatever `family`/`font_desc` string a caller
    /// handed it (issue #1023). `false` on TUI permanently — a terminal
    /// cell grid has no font concept to resolve a family token into, the
    /// same structural reason both methods take the trait's no-op
    /// default there (see each method's own doc).
    pub generic_font_families: bool,
    /// This render target's actual colour fidelity — see [`ColorDepth`].
    /// Not part of the bool-capability vocabulary below ([`Self::names`] /
    /// [`Self::has`] / [`Self::vocabulary`] / `ALL_NAMES`): those model
    /// "does this backend implement optional surface X", a static
    /// per-backend fact `tests/conformance/caps.rs` can mechanically
    /// check by asking whether a method was overridden. This field is a
    /// runtime-detected (or test-overridden) property of the terminal
    /// the process happens to be running in, so it has no "overridden
    /// method" to check against and is exempt from that machinery —
    /// see `backend_caps_tests::all_names_lists_every_field_exactly_once`'s
    /// exhaustive destructure below for where it is still forced into
    /// view.
    pub color_depth: ColorDepth,
    /// Whether the kitty keyboard protocol (progressive keyboard
    /// enhancement — unambiguous modifier keys, key-release events) is
    /// actually active on this backend right now (quadraui#827).
    ///
    /// Not part of the bool-capability vocabulary below, for the same
    /// reason as [`Self::color_depth`]: this is a runtime-detected (or
    /// test-overridden) property of the *terminal* a TUI session happens
    /// to be running in, not a static "does this backend implement method
    /// X" fact `tests/conformance/caps.rs` can check by asking whether a
    /// method was overridden. `false` on every non-TUI backend (GTK,
    /// Win-GUI, macOS): none of them speak a terminal protocol at all, so
    /// `false` is the honest, structural answer, not a gap. On TUI it is
    /// `true` only when [`crate::tui::run`] actually pushed the
    /// protocol's enhancement flags — see
    /// `crate::tui::caps::probe_kitty_keyboard` for how that is decided,
    /// and `crate::tui::backend::TuiBackend::set_kitty_keyboard` for where
    /// the live answer lands here. Before this flag existed, an app had no
    /// way to tell "the terminal doesn't support this" from "it does, and
    /// the push already happened" — a gesture built assuming the latter
    /// would simply never fire on a terminal where it wasn't, with no
    /// signal anywhere. Check this before relying on a gesture that needs
    /// the protocol (e.g. Ctrl+Enter distinct from Enter) and fall back to
    /// an always-available binding (Alt+Enter) when it's `false` — see
    /// `docs/KITTY_KEYBOARD_PROTOCOL.md`'s degrade table for which real
    /// terminals land on which side.
    pub kitty_keyboard: bool,
}

/// A capability name paired with the accessor that reads it off a
/// `BackendCaps` value — see [`BackendCaps::ALL_NAMES`].
type NamedCap = (&'static str, fn(&BackendCaps) -> bool);

impl BackendCaps {
    /// No optional capability implemented. The honest starting point for
    /// a new backend — every field defaults to `false` the same way the
    /// `Backend` methods they mirror default to a no-op.
    pub const fn empty() -> Self {
        Self {
            mouse: false,
            scroll: false,
            drag: false,
            text_selection: false,
            native_menu: false,
            window_chrome: false,
            pointer_cursor: false,
            ime: false,
            file_dialogs: false,
            folder_dialogs: false,
            native_dialogs: false,
            notifications: false,
            app_font_registration: false,
            window_control: false,
            tray: false,
            generic_font_families: false,
            color_depth: ColorDepth::TrueColor,
            kitty_keyboard: false,
        }
    }

    /// Every capability name this instance declares, in field-declaration
    /// order — the vocabulary a conformance scenario's `requires` list
    /// (quadraui#491) matches capability names against, and what a skip
    /// row names as missing.
    pub fn names(&self) -> Vec<&'static str> {
        Self::ALL_NAMES
            .iter()
            .copied()
            .filter(|(_, get)| get(self))
            .map(|(name, _)| name)
            .collect()
    }

    /// Whether this instance declares `cap` (one of [`Self::vocabulary`]'s
    /// names). Unknown names answer `false` rather than panicking, so a
    /// typo'd `requires` entry reads as "missing", not a crash — the
    /// typo itself is caught separately, and by name, by
    /// `conformance::every_requires_names_a_known_capability`, which
    /// checks each `requires` entry against [`Self::vocabulary`].
    pub fn has(&self, cap: &str) -> bool {
        Self::ALL_NAMES
            .iter()
            .any(|(name, get)| *name == cap && get(self))
    }

    /// Every capability name that exists at all, declared or not, in
    /// field-declaration order.
    ///
    /// This is the closed vocabulary a conformance scenario's `requires`
    /// list may draw from (quadraui#492 review: there used to be a second
    /// hand-maintained list, and the two could drift). [`Self::names`]
    /// is the subset one backend answers `true` for; this is the whole
    /// alphabet, so a `requires` entry outside it can be reported as a
    /// typo rather than silently skipping every backend forever.
    pub fn vocabulary() -> Vec<&'static str> {
        Self::ALL_NAMES.iter().map(|(name, _)| *name).collect()
    }

    /// Every capability name paired with the accessor that reads it off a
    /// `BackendCaps` value — the single source of truth [`Self::names`],
    /// [`Self::has`] and [`Self::vocabulary`] all fold over, so the three
    /// can never disagree about the vocabulary.
    const ALL_NAMES: &'static [NamedCap] = &[
        ("mouse", |c| c.mouse),
        ("scroll", |c| c.scroll),
        ("drag", |c| c.drag),
        ("text_selection", |c| c.text_selection),
        ("native_menu", |c| c.native_menu),
        ("window_chrome", |c| c.window_chrome),
        ("pointer_cursor", |c| c.pointer_cursor),
        ("ime", |c| c.ime),
        ("file_dialogs", |c| c.file_dialogs),
        ("folder_dialogs", |c| c.folder_dialogs),
        ("native_dialogs", |c| c.native_dialogs),
        ("notifications", |c| c.notifications),
        ("app_font_registration", |c| c.app_font_registration),
        ("window_control", |c| c.window_control),
        ("tray", |c| c.tray),
        ("generic_font_families", |c| c.generic_font_families),
    ];
}

// Sealed-trait pattern (Rust API Guidelines C-SEALED): `sealed` is
// `pub(crate)`, so `Sealed` is nameable from anywhere in this crate (every
// in-tree backend implements it below) but not from outside it. That is
// the enforcement mechanism behind the stance this module documents on
// `Backend` itself, `docs/PRIMITIVE_RULES.md` rule 7, and `BACKEND.md`:
// four in-tree backends, no external implementors, ever (quadraui#800).
pub(crate) mod sealed {
    /// Unnameable outside this crate — see [`super::Backend`]'s "Sealed"
    /// docs.
    pub trait Sealed {}
}

/// A backend-reported failure at a frame, event-loop, or
/// [`PlatformServices`]/[`Clipboard`] seam (issue #507, design D-009 in
/// `docs/decisions/DECISIONS.md`; shipped by issue #805).
///
/// Not used by `draw_*` methods or the four CSD `bool` methods
/// (`begin_window_drag`, `toggle_window_maximize`, `begin_window_resize`,
/// `set_cursor`) — D-009's "Why not Result-ify `draw_*` / the CSD bools"
/// section explains why both stay exactly as they are. `Clone`, not
/// `std::error::Error`: `context` is a short, ungrepped human string for
/// logs/error UI, not a machine-matched code — backend authors compose it
/// from whatever the native API gave them (`HRESULT`, `GError`, `errno`)
/// with `format!`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// This backend has no implementation of the surface being asked
    /// for. Prefer a [`BackendCaps`] field where the gap is a whole
    /// method; reach for this only where the gap is data-dependent
    /// within a method `BackendCaps` already declares supported (e.g. a
    /// `MessageDialogOptions` shape this backend's native dialog API
    /// can't represent, even though `native_dialogs` is `true`).
    Unsupported,
    /// A native call failed for a reason this backend cannot recover
    /// from inside the current call. `context` names the failing
    /// native call/API, e.g. `"IDXGISwapChain::Present"`,
    /// `"GtkFileChooserNative"`, `"CreateNotifyIcon"`.
    PlatformFailure {
        /// The failing native call or API, for logs/error UI — not a
        /// machine-matched code.
        context: String,
    },
    /// The render surface/device was lost mid-frame (D3D
    /// `DXGI_ERROR_DEVICE_REMOVED`/`D2DERR_RECREATE_TARGET`, a destroyed
    /// GTK `GdkSurface`, …) and must be recreated before the next
    /// `begin_frame`. Distinct from `PlatformFailure` because callers
    /// handle it differently — recreate-and-retry, not log-and-continue.
    /// `WinBackend::end_frame` is the one in-tree producer today: it
    /// drops the render target and reports this via `last_error()`, and
    /// `WinBackend::ensure_surface` (called from `win::run`'s
    /// `WM_PAINT`/`WM_SIZE` handlers) recreates it before the next frame
    /// paints.
    SurfaceLost,
}

/// The `Result` type every [`BackendError`]-fallible seam returns —
/// `PlatformServices`/`Clipboard` `_result`-suffixed twin methods (D-009
/// seam 2) and any future one like them.
pub type ServiceResult<T> = Result<T, BackendError>;

/// Bundled font metrics for one call to [`Backend::measure`] — the two
/// numbers ([`Backend::char_width`] and [`Backend::line_height`]) every
/// primitive's `*Measure` type is actually built from.
///
/// Before this existed, an app that needed both numbers called
/// `backend.line_height()` and `backend.char_width()` separately and
/// threaded them into its own hand-built `*Measure` literal — every
/// caller re-deriving the same two-field bundle the backend already
/// knows how to hand back in one call (quadraui#817). `Metrics` is that
/// bundle; a primitive's `XMeasure::from_metrics(&Metrics)` constructor
/// (see [`crate::TextInputMeasure::from_metrics`] for the pattern) turns
/// it into that primitive's own measure shape without the app touching
/// `char_width` / `line_height` by name.
///
/// `#[non_exhaustive]`: this is in-tree-only today (no external `Backend`
/// implementors construct it — only [`Backend::measure`]'s default body
/// does), but consumers (`coord-tui`, `vimcode`) *do* read the two public
/// fields, so a later field addition should stay source-compatible with
/// field-access call sites instead of becoming a silent breaking change.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Metrics {
    /// Same value [`Backend::char_width`] returns.
    pub char_width: f32,
    /// Same value [`Backend::line_height`] returns.
    pub line_height: f32,
}

/// One implementation per platform. TUI, GTK, Win-GUI, and (v1.x) macOS.
///
/// # Sealed — no implementations outside this crate
///
/// `Backend` cannot be implemented downstream: it requires the private
/// supertrait [`sealed::Sealed`], which lives in a `pub(crate)` module and
/// so cannot even be *named*, let alone implemented, from another crate.
/// This makes real what `docs/PRIMITIVE_RULES.md` rule 7 already asserted
/// in prose — "adding a required `Backend` method is not a breaking
/// change, because there are no external implementors to break" — instead
/// of leaving it an aspiration a `pub` trait quietly contradicted
/// (quadraui#800). Concretely: a new primitive adds a new required
/// `draw_<name>` method to this trait in the same PR that adds the
/// primitive itself, with no default and no version-bump ceremony, and
/// every in-tree backend (TUI, GTK, Win-GUI, macOS) fills it in as an
/// intentional compile error surfaced by that PR — see `docs/BACKEND.md`
/// and the "Adding a primitive is a breaking change to this trait —
/// intentional" note under *Drawing* below.
///
/// If you want quadraui to render onto a target none of the four in-tree
/// backends cover, the supported path is contributing a fifth in-tree
/// backend (`BACKEND.md` walks through the shape; `docs/BACKEND.md`
/// covers the trait's event-loop/hook conventions) — not an out-of-tree
/// `impl Backend`, which sealing rules out entirely. An out-of-tree impl
/// would have no way to keep up with a trait that gains required methods
/// in ordinary, non-major-bump PRs; sealing turns that into a compile
/// error today instead of a silent trap the first time this crate cuts a
/// release.
///
/// ```compile_fail
/// // Rejected: `Backend`'s private supertrait `sealed::Sealed` can't be
/// // named outside this crate, so no out-of-crate type can satisfy it —
/// // this minimal impl fails to compile for that reason (on top of the
/// // missing method bodies, which a real attempt would also have to
/// // supply and which sealing makes irrelevant to write).
/// struct MyBackend;
/// impl quadraui::Backend for MyBackend {}
/// ```
pub trait Backend: sealed::Sealed {
    // ─── Frame + viewport ──────────────────────────────────────────────
    /// Viewport geometry in native units. TUI: cells; GTK/Win-GUI/macOS:
    /// pixel-ish units with `scale` set to the DPI ratio.
    fn viewport(&self) -> Viewport;

    /// Begin a frame. Backends may set up the render target, clear, etc.
    fn begin_frame(&mut self, viewport: Viewport);

    /// Flush the current frame to screen.
    fn end_frame(&mut self);

    // ─── Theming ───────────────────────────────────────────────────────
    /// Set the active [`crate::Theme`] on the backend.
    ///
    /// Apps that use a single theme call this once from `setup()`; apps
    /// that vary the theme per-pane call it at the start of each pane's
    /// render pass (e.g. to darken the background of a detail pane).
    ///
    /// Default: no-op. Backends that carry a `current_theme` field
    /// (TUI, GTK, macOS) override this to store the value so subsequent
    /// `draw_*` calls consume the updated palette.
    fn set_theme(&mut self, _theme: crate::Theme) {}

    /// Sync the nerd-fonts flag so icon-bearing surfaces (`draw_tree`,
    /// `draw_multi_section_view`, `draw_activity_bar`, etc.) render
    /// `Icon::glyph` when `true` and `Icon::fallback` when `false`.
    ///
    /// **Default value, and why every backend now agrees on it (issue
    /// #683):** every backend that owns this flag (`TuiBackend`,
    /// `GtkBackend`, `MacBackend`) starts it at `false` — fallback, not
    /// glyph. Before #683 the TUI defaulted to `true` and GTK to `false`,
    /// so identical `ShellConfig` produced a different icon variant
    /// depending only on which backend launched the app. `false` is the
    /// safer default of the two failure modes: a wrong `false` shows a
    /// plain-but-correct ASCII/Unicode glyph, while a wrong `true` shows
    /// tofu — particularly for the TUI, where Nerd Font availability is a
    /// property of the user's terminal that the app cannot see or
    /// control. Hosts that know their environment has Nerd Fonts (or that
    /// probe for it) call this explicitly to opt in.
    ///
    /// Call at the start of `render_content()` if the setting can change
    /// at runtime (a settings toggle, a config file reload), or once from
    /// `setup()` only if it is truly static for the process lifetime.
    /// This mirrors `set_theme`'s contract above, and for the same
    /// reason: the backend does not re-derive this flag on its own each
    /// frame the way it re-derives `line_height`/`char_width` from the
    /// editor font — whatever was last set stays set until the host sets
    /// it again.
    ///
    /// **Don't call this only from `setup()` and assume it stays synced.**
    /// vimcode#547 was exactly this bug: nerd-fonts detection used to be
    /// re-applied on a periodic "refresh" message, which stopped firing
    /// after the `ShellApp` cutover — nothing re-called `set_nerd_fonts`
    /// after the first frame, so the flag silently stuck at whatever
    /// `setup()` had seen (often `false`, if the capability probe hadn't
    /// resolved yet at startup), and every icon fell back to ASCII from
    /// then on with no visible error. If the setting can ever change
    /// after `setup()` runs — including "probe finishes after the first
    /// frame" — call this from `render_content()` every frame instead;
    /// it's cheap, and correctness doesn't depend on remembering to
    /// re-fire a refresh path elsewhere.
    ///
    /// Default: no-op. Backends that always use one icon form (e.g. a
    /// headless test backend) can accept this default.
    fn set_nerd_fonts(&mut self, _enabled: bool) {}

    /// Whatever was last handed to [`Self::set_nerd_fonts`].
    ///
    /// Hosts need this to resolve a [`crate::ToolbarIcons`] table at the
    /// point of use — `icons.apply(&bar, backend.nerd_fonts_enabled())`
    /// (issue #913) — without having to mirror the flag in their own
    /// state. `Toolbar` icons are plain strings, so the glyph-or-fallback
    /// choice has to be made before the `Toolbar` reaches the backend;
    /// this is how the caller learns which one to bake in.
    ///
    /// Default: `false`, matching `set_nerd_fonts`'s no-op default — a
    /// backend that ignores the setter reports the ASCII form, which is
    /// what it actually paints.
    fn nerd_fonts_enabled(&self) -> bool {
        false
    }

    /// Override the font used to paint editor content (family name + size
    /// in points).
    ///
    /// Backends that build a shared per-frame text layout (GTK's Pango
    /// layout) resolve `line_height()` / `char_width()` from this same
    /// font on every frame, so painted glyphs and click-to-column math
    /// (e.g. `editor_col_at_x`) always derive from one source of truth —
    /// closing the paint↔click drift that motivated this method (#422).
    /// `family` should name a monospace font: primitives that map columns
    /// to pixels (`draw_editor`'s `scroll_left * char_width`, etc.) assume
    /// uniform glyph width.
    ///
    /// Call once from `setup()` for a static font, or again any time the
    /// app's font preference changes at runtime (e.g. a zoom-in
    /// keybinding) — the change takes effect on the next repaint.
    ///
    /// Default: no-op. Fixed-cell backends (TUI) have no font concept —
    /// every glyph already occupies exactly one terminal cell.
    fn set_editor_font(&mut self, _family: &str, _size_pt: f32) {}

    /// Override the font used to paint **chrome** — status bar, tab bar,
    /// tree, menu bar, dialogs, rich-text popups — as opposed to
    /// [`Self::set_editor_font`], which only affects editor content.
    /// `font_desc` is a Pango-style font description string (e.g.
    /// `"Sans 11"`, `"Cantarell 12"`); unlike the editor font it need not
    /// be monospace, since chrome primitives don't do column math against
    /// it.
    ///
    /// Call once from `setup()` for a static UI font, or again any time
    /// the app's chrome-font preference changes at runtime — the change
    /// takes effect on the next repaint.
    ///
    /// Default: no-op. Fixed-cell backends (TUI) have no font concept —
    /// every glyph already occupies exactly one terminal cell. GTK is
    /// currently the only backend that overrides this (#624).
    fn set_ui_font(&mut self, _font_desc: &str) {}

    /// Register an application-supplied font (raw TTF/OTF bytes) with the
    /// platform font manager for the lifetime of this process — no
    /// filesystem write, no user font directory, no `fc-cache`-style
    /// daemon (issue #929).
    ///
    /// This exists because a backend has no built-in Nerd-Font glyph
    /// coverage to fall back to (unlike GTK, which cascades to
    /// fontconfig's installed `Symbols Nerd Font` automatically — see
    /// `crate::gtk::NERD_FONT_FALLBACK_FAMILY`): an app that wants icon
    /// glyphs to resolve on macOS/Win-GUI has to hand the backend its own
    /// font bytes (e.g. an `include_bytes!`-embedded subset) before
    /// [`Self::set_nerd_font_fallback`] can name a family for [`Self::draw_tree`]/
    /// [`Self::draw_activity_bar`]/etc. to actually resolve glyphs against.
    ///
    /// Returns the family name(s) the font registered under (read back
    /// from the font's own name table, not the caller's guess), so a
    /// caller can pass one straight to [`Self::set_nerd_font_fallback`]
    /// without hardcoding it — or `None` if `bytes` isn't a font this
    /// backend's platform font manager can parse, or registration itself
    /// failed.
    ///
    /// Call once from `setup()`, before [`Self::set_nerd_font_fallback`]
    /// — same "static for the process lifetime" convention as
    /// [`Self::set_editor_font`]/[`Self::set_ui_font`].
    ///
    /// `bytes` itself does **not** need to outlive this call — a caller
    /// may pass a local `Vec` read from disk and drop it the instant this
    /// method returns, same as the `include_bytes!` case. Any backend
    /// whose platform font manager keeps a raw, uncopied pointer into
    /// `bytes` for longer than the call (Win-GUI's DirectWrite
    /// `IDWriteInMemoryFontFileLoader` is documented to do exactly this —
    /// see `win::text::register_font_from_memory`'s doc) is responsible
    /// for making its own owned copy before handing that pointer to the
    /// platform API, and for keeping that copy alive for the process
    /// lifetime itself — mirroring the macOS backend's `Arc<Vec<u8>>`
    /// copy into `CGDataProvider::from_buffer`. The caller's buffer is
    /// never the one the platform ends up holding a live reference to.
    ///
    /// Default: no-op, returns `None`. TUI takes this default for the
    /// same reason [`Self::set_editor_font`] does: a fixed-cell backend
    /// has no font concept at all. GTK used to take this default as well
    /// — reasoning that Fontconfig already resolves a system-installed
    /// Nerd Font via [`Self::set_nerd_fonts`]'s cascade, so there was
    /// nothing to register at the backend level — but that left an app
    /// bundling its *own* icon font (rather than relying on one already
    /// being installed system-wide) with no in-process path on GTK, and
    /// [`BackendCaps::app_font_registration`] declaring `true` regardless
    /// (issue #1013: a lying capability, and the direct reason vimcode
    /// kept a system-wide `~/.local/share/fonts` + `fc-cache` installer
    /// for GTK specifically). GTK now overrides this via
    /// `FcConfigAppFontAddFile` — see `crate::gtk::app_font`'s module
    /// doc.
    fn register_font_from_memory(&mut self, _bytes: &[u8]) -> Option<Vec<String>> {
        None
    }

    /// Set the font family consulted for characters the primary
    /// (editor/UI) font cannot cover — the portable, explicit form of
    /// what GTK already does implicitly via
    /// `crate::gtk::NERD_FONT_FALLBACK_FAMILY` (issue #929).
    ///
    /// `family` is looked up first among any fonts this backend
    /// registered via [`Self::register_font_from_memory`], then (for a
    /// backend that supports it) the platform's own installed fonts —
    /// so this also works for a system-installed Nerd Font with no
    /// `register_font_from_memory` call at all.
    ///
    /// Call once from `setup()` for a static fallback, or again any time
    /// the app's font preference changes at runtime — same convention as
    /// [`Self::set_editor_font`]/[`Self::set_ui_font`], including the
    /// same "a live surface may not rebuild immediately" caveat on
    /// backends that build their text-shaping state once per surface
    /// rather than once per frame.
    ///
    /// Default: no-op. GTK overrides it anyway, even though it already
    /// has a working (if hardcoded) fallback via
    /// `crate::gtk::NERD_FONT_FALLBACK_FAMILY`: this method makes that
    /// family *settable* instead, so an app that treats this as the
    /// portable entry point (rather than reaching for the GTK-specific
    /// constant) gets the same effect on every backend, including GTK.
    /// TUI takes the default for the same fixed-cell reason
    /// [`Self::set_ui_font`] does.
    fn set_nerd_font_fallback(&mut self, _family: &str) {}

    // ─── Text selection ────────────────────────────────────────────────
    /// Register a selectable text region for the current frame.
    ///
    /// Call once per selectable content area during render (in paint
    /// order: back regions first, front regions last). The backend
    /// records the region so that click dispatch via
    /// [`crate::dispatch::dispatch_click`] can begin a
    /// [`crate::DragTarget::TextSelection`] drag when the user clicks
    /// inside the bounds.
    ///
    /// The TUI backend additionally applies per-frame selection
    /// highlights (inverted cells over the selected range) and extracts
    /// text on Ctrl-C. GTK/macOS backends can use it for native
    /// selection support. The registration is cleared at the start of
    /// each frame (`begin_frame`), so apps must call this every frame
    /// for regions that should be selectable.
    ///
    /// Default: no-op. Backends that implement selection highlight
    /// override this to accumulate regions for the current frame.
    fn register_text_region(&mut self, _region: crate::dispatch::TextRegion) {}

    /// Record one hit-testable widget zone painted during the current
    /// frame — the `WidgetId`-keyed counterpart to
    /// [`Self::register_text_region`], and the source
    /// [`crate::testing::FrameInventory::zones`] reads from
    /// (quadraui#490, `docs/SMELL_AUDIT_2026-07.md` §6.2/B3).
    ///
    /// Call once per zone during render (chrome composers like
    /// [`crate::compose::app_shell::AppShell::render`] call this for
    /// each `WidgetId`-bearing region they lay out — activity-bar items,
    /// sidebar panels, the status bar, and so on). The registration is
    /// cleared at the start of each frame by `begin_frame`, mirroring
    /// `register_text_region`'s lifecycle.
    ///
    /// Default: no-op. `TuiBackend`/`GtkBackend` override this to
    /// accumulate the current frame's zones for `ConformanceDriver::inventory`.
    fn register_zone(&mut self, _id: WidgetId, _bounds: Rect) {}

    /// Cancel any in-progress text-selection drag without clearing the
    /// currently displayed selection highlight.
    ///
    /// ## When to call this
    ///
    /// `apply_dispatch` (inside `Backend::wait_events`) speculatively starts
    /// a [`crate::DragTarget::TextSelection`] drag whenever a `MouseDown`
    /// lands on a registered [`crate::dispatch::TextRegion`], *before* the
    /// app's `handle()` is called. Apps that host an embedded terminal with
    /// mouse reporting enabled should call this method after a successful
    /// [`crate::terminal_engine::TerminalSession::forward_mouse`] return
    /// (`true`) so that subsequent `MouseMoved` events do not emit spurious
    /// [`crate::UiEvent::TextSelectionChanged`] events.
    ///
    /// ```ignore
    /// // Inside your AppLogic::handle() / ShellApp::handle():
    /// if in_term_area {
    ///     if sess.forward_mouse(kind, button, col, row, mods) {
    ///         // Cancel the speculative drag the runner started; don't clear
    ///         // any previously finalised selection display.
    ///         backend.cancel_text_selection_drag();
    ///         return Reaction::Redraw;
    ///     }
    /// }
    /// ```
    ///
    /// Default: no-op. The TUI backend cancels the `DragTarget::TextSelection`
    /// drag state without touching the `active_selection` field, preserving
    /// any previously finalised selection highlight on screen.
    fn cancel_text_selection_drag(&mut self) {}

    // ─── Events + keybindings ──────────────────────────────────────────
    /// Drain all queued native events. Returns a fully-translated
    /// `Vec<UiEvent>` ready for app dispatch. Never blocks.
    fn poll_events(&mut self) -> Vec<UiEvent>;

    /// Block for up to `timeout` waiting for at least one event. Returns an
    /// empty `Vec` on timeout. Used by apps that don't want to busy-poll.
    fn wait_events(&mut self, timeout: Duration) -> Vec<UiEvent>;

    /// Return a `Send + Sync` handle a **background thread** can call to
    /// deliver a value to the app and wake this backend's event loop so the
    /// value is observed promptly — issue #831.
    ///
    /// Every other `Backend` method requires `&mut self` or `&self` on the
    /// thread that owns the event loop; this is the one exception,
    /// deliberately shaped for a thread that has neither. Call the
    /// returned closure with a [`UserPayload`] from any thread (including
    /// the owning one) at any time, including before the event loop has
    /// started or after the app has exited (a call after exit is simply
    /// dropped — there is no receiver left to observe it):
    ///
    /// ```ignore
    /// let waker = backend.waker();
    /// std::thread::spawn(move || {
    ///     let result = do_expensive_work();
    ///     waker(UserPayload::new(result));
    /// });
    /// ```
    ///
    /// The backend wakes its event loop and delivers the payload as
    /// [`UiEvent::User`] to [`crate::runner::AppLogic::handle`] — see that
    /// variant's doc for the full contract and why this closes a real gap
    /// rather than a cosmetic one: **before this method existed, every
    /// runner was poll-only** (grep any `run.rs` for `mpsc`/`Waker`/
    /// `channel(` — issue #831 found zero), so an app doing background I/O
    /// had no way to be told a result was ready except by re-checking its
    /// own state from [`crate::runner::AppLogic::tick`] on whatever cadence
    /// the backend happens to poll at. That cadence isn't uniform, and for
    /// two of the four backends it doesn't exist at all absent unrelated
    /// activity:
    /// - TUI polled every 16ms (`tui::run::POLL_TIMEOUT`) regardless —
    ///   `waker` only tightened *when* a background result was folded into
    ///   that already-frequent poll, it didn't newly enable delivery.
    /// - GTK polled every 33ms (`gtk::run::run_with`'s idle timer) — same
    ///   shape as TUI, coarser interval.
    /// - **macOS and Windows called [`crate::runner::AppLogic::tick`] not
    ///   at all** — macOS only drained its event queue from inside a
    ///   paint pass, and Windows' `wndproc` dispatched directly per Win32
    ///   message with no idle timer of its own. Without a live redraw or
    ///   an unrelated native event, a background result on either backend
    ///   would otherwise never reach the app at all. `waker`'s closure is
    ///   what forced the wake on these two: GTK/macOS/Windows
    ///   implementations use their native thread-safe "run this on the UI
    ///   thread" primitive (`glib::MainContext::invoke`,
    ///   `dispatch2::DispatchQueue::main`, `PostMessageW`, respectively)
    ///   precisely because none of the three has anything else that
    ///   reliably runs soon after being poked from off-thread. TUI's
    ///   implementation only needed to feed the payload into the queue its
    ///   existing bounded poll already drains.
    ///
    /// **Since quadraui#832** ([`Self::request_frame_in`]), the fixed-
    /// cadence part of that list is history rather than current
    /// behavior: TUI/GTK's unconditional polls are gone (both now poll a
    /// much coarser fallback ceiling, `crate::runtime::IDLE_POLL_CEILING`,
    /// 250ms — see that constant's doc), and macOS/Windows gained their
    /// first `tick` invocations ever, but at that point *only* when
    /// something (a native event or a `request_frame_in` deadline) asked
    /// for one. **Since quadraui#940**, macOS joined TUI/GTK's coarse
    /// fallback ceiling too (`macos::run`'s repeating `idlePollTick:`
    /// timer) — #832 alone had left macOS silently unable to notice
    /// deferred host work that never called `request_frame_in`/returned
    /// `RedrawAfter`, unlike TUI/GTK's idle-poll safety net. Windows
    /// remains deliberately wake-only, with no fallback ceiling of its
    /// own — see [`crate::runner::AppLogic::tick`]'s per-backend cadence
    /// table for the current state of all four. This method's own
    /// latency contract is unchanged by any of that: a `waker` call
    /// still forces a prompt wake exactly as described above,
    /// independent of whatever cadence (if any) `tick` runs on.
    ///
    /// Implementations must be safe to call from any thread, at any time,
    /// any number of times, including concurrently with each other and
    /// with the owning thread's own use of `self` — that's the entire
    /// point of handing out a `Send + Sync` closure instead of a method on
    /// `&self`/`&mut self`. Backends satisfy this by never touching their
    /// own `!Send` event queue (`Rc<RefCell<VecDeque<UiEvent>>>` on GTK/
    /// macOS/Windows) directly from the returned closure; they stage into
    /// a `Send + Sync` inbox instead ([`crate::runtime::UserEventQueue`])
    /// and drain that inbox back on the owning thread, the same place they
    /// already drain their native event source.
    fn waker(&self) -> Arc<dyn Fn(UserPayload) + Send + Sync>;

    /// Arm a scheduled wake: after `delay`, make sure the event loop wakes
    /// up and [`crate::runner::AppLogic::tick`] (or the app's `handle`
    /// dispatch, for whichever native event the wake rides in on) runs
    /// again — issue #832.
    ///
    /// This is [`Self::waker`]'s sibling for the other half of the
    /// pre-#832 gap that method's doc describes: before #832, TUI and GTK
    /// each polled unconditionally (16ms / 33ms) regardless of whether
    /// anything was scheduled, burning CPU on a fully idle app, while
    /// macOS and Windows didn't call `tick` *at all* absent some
    /// unrelated native event to ride in on. `waker` fixed "a background
    /// thread has a result, deliver it promptly"; this method fixes "the
    /// app itself knows it wants to be woken again in `delay`, without
    /// resorting to a fixed poll cadence to eventually notice" — a
    /// spinner's next frame, a caret blink toggle, a countdown tick.
    ///
    /// Called by the runner from [`crate::runner::Reaction::RedrawAfter`]
    /// (via [`crate::runner::AppLogic::tick`]/`handle`'s return value) —
    /// apps normally reach this indirectly by returning that `Reaction`
    /// rather than calling it directly, but nothing stops a direct call
    /// (e.g. from [`crate::runner::AppLogic::setup`], which has no
    /// `Reaction` return value of its own, to bootstrap a self-rearming
    /// animation chain before the first native event arrives).
    ///
    /// Per the doc on [`crate::runner::Reaction::RedrawAfter`]: the
    /// backend may wake earlier than `delay` for unrelated reasons (a
    /// native event, a `waker` call, another still-pending request) but
    /// never later, and is not required to cancel or coalesce overlapping
    /// requests — calling this every tick while the app has ongoing
    /// time-driven state (the chained-rearm pattern) is the intended
    /// usage, not redundant.
    ///
    /// # Per-backend mechanism
    ///
    /// "A scheduled frame and a thread wake are the same mechanism on
    /// most backends" (the issue's framing): GTK/macOS/Windows implement
    /// this with a real native one-shot timer
    /// (`glib::timeout_add_local_once` / `dispatch_after` / `SetTimer`)
    /// whose fire callback invokes the *same* wake path [`Self::waker`]
    /// posts to from a background thread. TUI is the exception — it has
    /// no native run loop to arm a timer against (its "event loop" is
    /// just [`Self::wait_events`] called in a `loop {}`), so its
    /// implementation records the deadline and folds it into the next
    /// `wait_events` call's timeout instead; see
    /// `crate::runtime::FrameScheduler`.
    ///
    /// This one-shot timer is independent of, and doesn't replace, the
    /// separate always-repeating idle-poll fallback TUI/GTK/macOS also
    /// keep (`crate::runtime::IDLE_POLL_CEILING` — see
    /// [`crate::runner::AppLogic::tick`]'s per-backend cadence table):
    /// this method exists for an app that knows the *exact* interval it
    /// wants to be woken after, the fallback for an app that doesn't ask
    /// at all.
    ///
    /// `WinBackend`'s implementation clamps `delay` to `u32::MAX`
    /// milliseconds (~49.7 days) — `SetTimer`'s elapse parameter is a
    /// 32-bit millisecond count — rather than erroring on a longer
    /// request; no caller asks for anything close to that today.
    fn request_frame_in(&self, delay: Duration);

    /// Ask the backend to paint the *next* frame as if the physical
    /// display were blank, discarding whatever incremental-diff cache it
    /// keeps against the real screen — issue #1037.
    ///
    /// This exists for exactly one situation: the backend's own idea of
    /// "what's on screen" has drifted from what's actually there, through
    /// no fault of the app's rendered state (which is already correct —
    /// a plain [`Reaction::Redraw`][crate::runner::Reaction::Redraw] would
    /// change nothing, because the diff against the *stale* cache still
    /// concludes those cells don't need repainting). On TUI, that drift
    /// is `ratatui::Terminal`'s `Buffer` vs. the real terminal: a PTY
    /// pane writing directly to the shared terminal, a resize escape
    /// sequence the terminal applies before ratatui's own resize
    /// handling catches up, or a dismissed popup can each leave stale
    /// glyphs in cells ratatui's diff believes are already correct and
    /// therefore skips. `TuiBackend::request_full_repaint` itself only
    /// latches a flag (read back via `take_full_repaint_requested`); the
    /// actual `ratatui::Terminal::clear()` call — exactly what
    /// `Terminal::clear()` gives a raw (non-runner) ratatui app — lives in
    /// each render-path caller that consumes that flag before its next
    /// `terminal.draw(...)`: `tui::run::run_inner`'s frame loop for the
    /// live runner, `crate::tui::testing::TuiDriver::render` for the
    /// headless `TestBackend` driver, and `crate::tui::vt_testing::TuiVtDriver::render`
    /// for the headless vt100 driver (so a driver test can assert this
    /// was requested without a real terminal to observe the clear on).
    ///
    /// Default: no-op. GTK's `DrawingArea` repaints in full every frame
    /// via Cairo — there is no incremental diff to desync in the first
    /// place, so `GtkBackend` doesn't override this. A future diff-based
    /// renderer (a terminal-multiplexer-aware Win-GUI console mode, say)
    /// gets the same hook for free by overriding this method instead of
    /// needing a new one.
    ///
    /// Idempotent and cheap to call more than once before the next frame
    /// — a second call while one is already pending changes nothing, the
    /// same "may act as if called once" contract
    /// [`Self::request_frame_in`] documents for overlapping requests.
    fn request_full_repaint(&mut self) {}

    /// Register an accelerator. The backend stores it and emits
    /// [`UiEvent::Accelerator`] when the native key event matches.
    fn register_accelerator(&mut self, acc: &Accelerator);

    /// Remove a previously-registered accelerator.
    fn unregister_accelerator(&mut self, id: &AcceleratorId);

    // ─── Native menu installation ──────────────────────────────────────
    /// Install `bar` as the platform's native menu bar.
    ///
    /// macOS (`MacBackend`) walks `bar.items` → `NSMenu` / `NSMenuItem`
    /// hierarchy and assigns to `NSApp.mainMenu`. A standard app menu
    /// (Hide / Quit etc.) is auto-prepended. Activations arrive on the
    /// event queue as [`UiEvent::MenuActivated`].
    ///
    /// TUI / GTK / Win-GUI: no-op default. Apps that want an in-window
    /// menu keep calling `draw_menu_bar` from their render path; native
    /// installers for Win32 (`SetMenu`) and GTK (`set_menu_bar`) land
    /// in follow-up tickets when consumers need them.
    ///
    /// Apps typically call this once during `AppLogic::setup`. Re-calling
    /// replaces the previously-installed menu wholesale.
    fn install_menu_bar(&mut self, _bar: &crate::primitives::menu_bar::MenuBar) {}

    /// Show `menu` as a native right-click context menu at `anchor`
    /// (view-local coordinates).
    ///
    /// macOS (`MacBackend`) builds an `NSMenu` from `menu.items` and
    /// runs `popUpMenuPositioningItem_atLocation_inView` — AppKit takes
    /// over with a modal event loop until the user picks an item or
    /// dismisses. Activation pushes
    /// [`UiEvent::ContextMenuItemActivated`]; dismissal pushes
    /// [`UiEvent::ContextMenuDismissed`].
    ///
    /// TUI / GTK / Win-GUI: no-op default. Apps that want a painted
    /// right-click menu on those backends continue to manage their
    /// own `ContextMenu` state and call `draw_context_menu` from
    /// their render path. A stash-and-paint default lands in a
    /// follow-up ticket if a consumer asks for it.
    ///
    /// Apps typically invoke this from a `MouseDown { button: Right }`
    /// handler.
    fn show_context_menu(
        &mut self,
        _menu: &crate::primitives::context_menu::ContextMenu,
        _anchor: crate::event::Point,
    ) {
    }

    // ─── Window chrome (CSD) ────────────────────────────────────────────
    /// Begin an OS-native window drag-to-move gesture, using the raw
    /// device/button/timestamp captured from the most recent primary-button
    /// press.
    ///
    /// For apps that draw their own client-side titlebar (`ShellConfig::
    /// with_title_bar` + `window.set_decorated(false)`) into
    /// `AppShellLayout::title_bar_bounds` and want the empty part of that
    /// band to drag the window like a native titlebar. Call from
    /// [`crate::shell::ShellApp::handle`] when a `MouseDown` lands in
    /// [`crate::shell::ShellContext::in_title_bar`] outside any interactive
    /// segment (menu item, min/max/close button).
    ///
    /// `GtkBackend` arms a deferred `gdk4::Toplevel::begin_move` call, using
    /// the press context `gtk::run`'s click controller stashes just before
    /// the press is translated to a portable [`UiEvent`] (see #400) — GDK
    /// requires the *originating* event's device/timestamp, not synthesized
    /// values, or the drag silently no-ops on some compositors. The actual
    /// `begin_move` call only fires once the pointer moves past the drag
    /// threshold (mirroring native `gtk4::WindowHandle`'s `GestureDrag`
    /// `drag-begin` gating); calling this from a `MouseDown` handler does
    /// not itself start an interactive move grab, so a press that turns out
    /// to be the first half of a double-click still reaches the app as
    /// `UiEvent::DoubleClick`.
    ///
    /// Returns `false` when the backend owns no window (TUI, and any
    /// backend before its window is constructed) or no primed press context
    /// is available. Callers should treat `false` as a no-op, not an error.
    /// A `true` return means the drag request was accepted/armed, not that
    /// the window has necessarily started moving yet.
    fn begin_window_drag(&mut self) -> bool {
        false
    }

    /// Toggle the OS window between maximized and restored — the
    /// double-click-to-maximize half of the CSD-titlebar gesture pair
    /// (see [`Self::begin_window_drag`]).
    ///
    /// Call from `ShellApp::handle` on a `DoubleClick` landing in the empty
    /// part of the titlebar band. Returns `false` on backends with no
    /// window (TUI); `true` once the toggle happened.
    fn toggle_window_maximize(&mut self) -> bool {
        false
    }

    /// Begin an OS-native window resize gesture from the given edge, using
    /// the raw device/button/timestamp captured from the most recent
    /// primary-button press (same mechanism as [`Self::begin_window_drag`]).
    ///
    /// For apps that draw their own client-side titlebar and want to offer
    /// edge/corner resize the way native `gtk4::WindowHandle` /
    /// `GDK_SURFACE_EDGE_*` decorations do for free. Apps are expected to
    /// hit-test the pointer against their own window bounds each frame
    /// (see [`crate::shell::ShellContext::window_edge`]) and call this from
    /// a `MouseDown` handler when the press lands within resize-margin
    /// distance of an edge.
    ///
    /// Unlike [`Self::begin_window_drag`], this does not need to defer past
    /// a movement threshold: there is no competing "double-click on an edge
    /// means something else" gesture to protect (double-click-to-maximize
    /// only applies to the empty titlebar band), so `GtkBackend` calls
    /// `gdk4::Toplevel::begin_resize` directly from the stashed press
    /// context.
    ///
    /// Returns `false` when the backend owns no window (TUI, and any
    /// backend before its window is constructed) or no primed press context
    /// is available. Callers should treat `false` as a no-op, not an error.
    fn begin_window_resize(&mut self, _edge: ResizeEdge) -> bool {
        false
    }

    /// Hint the OS mouse-pointer glyph, e.g. to show a resize cursor while
    /// hovering a window edge (see [`Self::begin_window_resize`]).
    ///
    /// Call from `ShellApp::handle` on every `UiEvent::MouseMoved`, passing
    /// [`PointerShape::Resize`] when [`crate::shell::ShellContext::window_edge`]
    /// returns `Some`, else [`PointerShape::Default`] to restore the normal
    /// pointer. Returns `false` on backends with no native pointer concept
    /// (TUI) or no window yet; `true` once the pointer glyph was applied.
    fn set_cursor(&mut self, _shape: PointerShape) -> bool {
        false
    }

    /// Inset at the leading edge of the title-bar band
    /// ([`crate::compose::app_shell::AppShellLayout::title_bar_bounds`])
    /// occupied by backend-drawn window controls, in the same native
    /// units every other `Rect` this trait returns uses. `Rect::default()`
    /// (the default, and the only value on every backend before #947)
    /// means the backend puts nothing of its own into that band, so the
    /// app may paint the whole width of `title_bar_bounds`. That is the
    /// answer on TUI (no window concept at all), and on GTK/Win-GUI,
    /// which don't honour
    /// [`crate::shell::ShellConfig::client_side_titlebar`] yet and so
    /// always keep their window controls in native chrome *outside* the
    /// app's content area (see `ACCEPTED_DEFAULTS` in
    /// `tests/conformance/caps.rs` — when either grows client-side
    /// decorations it has to override this and delete its entry).
    ///
    /// macOS is the one case where a non-empty inset does **not** imply
    /// the app must paint its own controls — quite the opposite: a
    /// non-empty return there means AppKit's native traffic lights
    /// (close/minimize/zoom) are floating over that region and the app
    /// must paint nothing underneath, not that it owns drawing them (see
    /// [`crate::shell::ShellConfig::client_side_titlebar`]'s doc for why
    /// macOS keeps them native rather than app-drawn).
    ///
    /// Defaulted, not required (`PRIMITIVE_RULES.md` rule 7): unlike
    /// [`Self::draw_focus_ring`], a backend that doesn't override this
    /// yet is indistinguishable from one that correctly draws no window
    /// controls, so there is no portability hazard in leaving it
    /// no-op — only a backend that actually reserves native chrome space
    /// (macOS) needs to override it.
    fn titlebar_control_inset(&self) -> Rect {
        Rect::default()
    }

    // ─── Window control (issue #950) ────────────────────────────────────
    /// This backend's window-state control surface, or `None` when this
    /// backend has no OS window to control right now.
    ///
    /// `None` is the *structural* absence this issue's design calls for:
    /// TUI (no OS window concept at all, though see
    /// [`WindowControl::set_title`] for the one thing it genuinely can
    /// do) and any windowed backend before its window is constructed
    /// both answer `None` here, and a caller that only ever sees `Option`
    /// cannot forget to check it the way a silently-false-returning
    /// capability flag can be forgotten. See [`WindowControl`]'s own doc
    /// for why individual methods on the returned trait object still
    /// report [`ServiceResult`] rather than being infallible once
    /// `Some` is reached.
    ///
    /// `&mut self`, not `&self`: every [`WindowControl`] method mutates
    /// OS window state, so the borrow this returns has to allow that —
    /// matching [`Self::modal_stack_handle`]'s reasoning for why that
    /// one is `Rc<RefCell<_>>` instead (window control has no
    /// stash-then-reuse-from-an-unrelated-borrow requirement the way the
    /// modal stack does, so a plain borrow is simpler and sufficient
    /// here).
    ///
    /// Default: `None` — a backend that hasn't wired a `WindowControl`
    /// impl yet is indistinguishable from one that genuinely has no
    /// window, which is the honest answer before this issue's `gtk`/
    /// `macos`/`win`/`tui` implementations land.
    fn window(&mut self) -> Option<&mut dyn WindowControl> {
        None
    }

    // ─── Tray / status-bar icon (issue #953) ────────────────────────────
    /// This backend's tray/status-bar icon control surface, or `None`
    /// when this backend has no way to show one right now.
    ///
    /// `None` is the *structural* absence [`Self::window`] already
    /// established for [`WindowControl`] (issue #950): a backend that
    /// hasn't wired a [`TrayService`] impl yet is indistinguishable from
    /// one with no tray facility at all, which is the honest default
    /// before a given backend's implementation lands.
    ///
    /// **TUI is genuinely, permanently `None` here** — not merely
    /// "not implemented yet" the way [`WindowControl::set_title`] still
    /// found one real capability (`OSC 0/2`) for a backend with no OS
    /// window at all. A terminal has no notification-area/menu-bar
    /// concept whatsoever, so there is no terminal-native analogue this
    /// method could ever back with a real call — unlike `window()`,
    /// which TUI overrides to return `Some` for its one genuine
    /// capability, `tray()` stays at this trait's default on `TuiBackend`
    /// forever. This is exactly why the compose layer's `hide_to_tray`
    /// helper (a host convenience that hides the window and relies on
    /// the tray icon being the only way back) must refuse to run when
    /// `tray()` is `None`: on TUI, hiding "to the tray" would simply
    /// make the app vanish with no way back.
    ///
    /// `&mut self`, not `&self` — same reasoning as [`Self::window`]:
    /// every [`TrayService`] method mutates OS tray state.
    ///
    /// Default: `None`.
    fn tray(&mut self) -> Option<&mut dyn TrayService> {
        None
    }

    // ─── Modal-overlay tracking ────────────────────────────────────────
    /// Shared handle to the backend's modal stack, usable across
    /// unrelated `&mut dyn Backend` (or `&dyn Backend`) borrows. Apps
    /// push when a palette / dialog / context-menu opens and pop when
    /// it closes; quadraui's dispatcher consults the stack so events
    /// inside an open modal can't fall through to widgets behind it.
    ///
    /// This is a shared `Rc<RefCell<ModalStack>>` rather than a plain
    /// `&mut ModalStack`, because the latter ties its lifetime to the
    /// borrow that produced it and so cannot be stashed and used again
    /// later from an unrelated borrow scope — exactly the
    /// stash-then-reuse pattern GTK hosts (and quadraui's own
    /// `gtk::run`) depend on:
    ///
    /// ```ignore
    /// let stack_rc = backend.modal_stack_handle(); // stash, drop the borrow
    /// // ... other code, other borrows of `backend` in between ...
    /// stack_rc.borrow_mut().push(...);             // use it later
    /// ```
    ///
    /// Every in-tree backend implements this by owning its modal stack
    /// behind `Rc<RefCell<ModalStack>>` and cloning the `Rc` here — see
    /// `GtkBackend::modal_stack_handle` for the pattern this trait
    /// method generalises (quadraui#699). No default: a host holding
    /// only `&mut dyn Backend` needs this to work identically on every
    /// backend, including a future macOS/Win-GUI host, so a backend
    /// that forgets to wire it up should fail to compile rather than
    /// silently hand back a handle to a stack nobody else observes.
    ///
    /// Reentrant mutation goes through `RefCell`'s normal runtime
    /// borrow check (a stale `borrow_mut()` still alive when another
    /// call tries to borrow again panics loudly) — quadraui#704 removed
    /// the earlier `modal_stack_mut()` / `drag_and_modal_mut()` bridges,
    /// which synthesized a `&mut` via `unsafe { Rc::as_ptr(..) }` and so
    /// bypassed that check instead of enforcing it.
    ///
    /// See [`ModalStack`] and [`crate::dispatch::dispatch_mouse_down`]
    /// for the routing contract.
    fn modal_stack_handle(&self) -> Rc<RefCell<ModalStack>>;

    /// Shared handle to the drag state, with the same stash-then-reuse
    /// contract as [`Self::modal_stack_handle`]. See that method's docs
    /// for the pattern and rationale (quadraui#699).
    fn drag_state_handle(&self) -> Rc<RefCell<DragState>>;

    // ─── Focus (issue #830) ──────────────────────────────────────────────
    /// Read-only access to this backend's [`FocusManager`] — the single
    /// owner of "what widget currently has keyboard focus". See
    /// [`crate::focus`]'s module doc for the full design and the six
    /// ad-hoc representations it replaces.
    ///
    /// Deliberately `&FocusManager`, not `&mut`: the only code that
    /// mutates it is the shared Tab/Shift+Tab intercept in
    /// [`crate::runtime::preprocess_event`], reached through the
    /// crate-private `PreprocessBackend::focus_manager_mut` — apps read
    /// the current focus here, they don't set it directly. No default:
    /// every backend owns exactly one `FocusManager`, constructed
    /// alongside its other per-frame state.
    fn focus_manager(&self) -> &FocusManager;

    /// Paint the focus-ring convention around `rect` — a themed,
    /// border-only stroke (`Theme::accent_fg`) with no fill, so the
    /// focused widget's own content keeps showing through underneath.
    /// Called by the runner once per frame, immediately after
    /// `AppLogic::render`, whenever [`Self::focus_manager`]`().focused()`
    /// names a widget this frame's tab stops resolve a rect for — apps
    /// never call this directly.
    ///
    /// No default impl — every backend implementer sees this as a
    /// compile error and fills in a real rasteriser
    /// (`docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §4, `PRIMITIVE_RULES.md` rule 7).
    /// Do not add a no-op default here; see
    /// `docs/SMELL_AUDIT_2026-07.md` PORT-01 for why that pattern is a
    /// portability risk, not a precedent to follow.
    fn draw_focus_ring(&mut self, rect: Rect);

    // ─── Error reporting (issue #507, D-009) ────────────────────────────
    /// The most recent [`BackendError`] this backend recorded, if any,
    /// since the last time this method was called.
    ///
    /// One polled method, not a `Result`-returning `begin_frame`/
    /// `end_frame`/`poll_events`/`wait_events` each: those four calls
    /// funnel into the same internal field, because a caller that wants
    /// to notice backend trouble at all polls once per loop iteration
    /// (after `end_frame`, conventionally) and doesn't need to know which
    /// of the four calls produced it — `PlatformFailure`'s `context`
    /// string carries that detail when it matters. See D-009 in
    /// `docs/decisions/DECISIONS.md` for why a `Result`-returning
    /// signature on those four methods was rejected as too costly for
    /// every caller, for a failure class only Win-GUI produces today.
    ///
    /// Default: always `None` — a backend that never sets an internal
    /// error field (TUI, GTK, macOS today) answers this exactly like it
    /// doesn't exist, at zero cost to existing callers. `WinBackend`
    /// overrides this: it sets an internal field from `end_frame` when
    /// `EndDraw` fails (device lost / `D2DERR_RECREATE_TARGET`) and
    /// clears it here — clear-on-read, matching every other "drain and
    /// reset" method already on this trait (`poll_events` itself).
    fn last_error(&mut self) -> Option<BackendError> {
        None
    }

    // ─── Platform services ─────────────────────────────────────────────
    /// Clipboard, file dialogs, notifications, URL opening, platform name.
    fn services(&self) -> &dyn PlatformServices;

    // ─── Capability declaration ─────────────────────────────────────────
    /// This backend's declared [`BackendCaps`] — which optional surfaces
    /// (quadraui#492) it actually implements, versus which ones are still
    /// sitting on the trait's no-op default.
    ///
    /// No default impl: every backend states its own caps explicitly
    /// (`Backend` has no external implementors — `BACKEND.md` — so there
    /// is no "safe" default to fall back to; `BackendCaps::empty()` would
    /// silently under-report a backend that forgot to update this after
    /// overriding a new optional method, exactly the honesty gap this
    /// method exists to close). See `TuiBackend::backend_caps` /
    /// `GtkBackend::backend_caps` for how a real backend derives this
    /// from what it actually overrides.
    fn backend_caps(&self) -> BackendCaps;

    // ─── Measurement ───────────────────────────────────────────────────

    /// Height of one standard text row in the backend's native units.
    /// TUI: `1.0` (one terminal cell). GTK: Pango-resolved line height
    /// in pixels (~14–20 depending on font). Win-GUI (future):
    /// DirectWrite line height in DIPs.
    ///
    /// Apps that need portable rect sizing use this instead of
    /// hardcoded constants. Example: `let status_h = backend.line_height() * 1.5;`
    /// gives 1.5 cells on TUI, ~24px on GTK, proportional DIPs on
    /// Win-GUI — all from the same code path.
    fn line_height(&self) -> f32;

    /// Approximate monospace character width in surface-native units.
    /// TUI returns `1.0` (one cell); GTK returns the Pango
    /// `approximate_char_width` in DIPs.
    ///
    /// Apps use this alongside [`Self::line_height`] for portable
    /// horizontal layout. Example:
    /// `let viewport_cols = ((rect.width - gutter) / backend.char_width()).floor();`
    fn char_width(&self) -> f32;

    /// The advance, in this backend's native units, of the font
    /// [`Self::draw_list`] actually paints `ListView` row text with.
    ///
    /// This is **not** always the same number [`Self::char_width`]
    /// returns. `char_width` reports the *editor* font's advance, but
    /// `draw_list` paints chrome (list rows are UI content, not editor
    /// content — quadraui#416/#624), and on a pixel backend "chrome
    /// font" can be a proportional face (GTK's default `ui_font` is
    /// `"Sans 11"`, macOS's is the CoreText system UI font) with a
    /// different — usually narrower — average glyph advance than the
    /// monospace editor face `char_width` measures. A consumer that
    /// divides a list's pixel width by [`Self::char_width`] to decide
    /// how many characters of row text fit under-fills the row by
    /// whatever ratio separates the two fonts (quadraui#912).
    ///
    /// Call this instead of [`Self::char_width`] when budgeting text for
    /// a `ListView` row. It is still only an *average* advance — a
    /// proportional font has no single width that wraps text exactly
    /// the way a monospace grid does — but it is the right font's
    /// average, which `char_width` is not.
    ///
    /// TUI returns the same `1.0` as `char_width` (one cell either way).
    /// Win-GUI currently returns the same value as `char_width` too:
    /// `draw_list` there still paints with the editor `DWrite` format,
    /// not `chrome_dwrite` (see `WinBackend::chrome_dwrite`'s doc) — the
    /// two will need to diverge together the day that rasteriser moves
    /// to chrome text, so this method is required (no default) on every
    /// backend rather than assumed equal to `char_width`.
    fn list_char_width(&self) -> f32;

    /// [`Self::char_width`] and [`Self::line_height`] bundled into one
    /// [`Metrics`] value, for apps that need both instead of calling each
    /// method separately (quadraui#817).
    ///
    /// Default impl composes the two required methods above, so it
    /// returns each backend's real metrics automatically — no backend
    /// needs to override this to get real numbers instead of defaults.
    /// A primitive's `XMeasure::from_metrics(&backend.measure())` replaces
    /// the old pattern of an app hand-building that `*Measure` literal
    /// from `backend.line_height()` / `backend.char_width()` itself (or,
    /// worse, approximating one of them — see
    /// `examples/common/dialog_table_demo.rs`'s pre-#817 `char_w = lh *
    /// 0.6` guess, replaced with the real [`Self::char_width`] via this
    /// method once it existed).
    fn measure(&self) -> Metrics {
        Metrics {
            char_width: self.char_width(),
            line_height: self.line_height(),
        }
    }

    /// Width this backend reserves for its own native scrollbar overlay
    /// alongside scrollable content — e.g. a GTK `ScrolledWindow`'s
    /// overlay scrollbar, drawn on top of the content edge rather than
    /// laid out beside it. A caller computing a content viewport width
    /// (`rect.width - backend.scrollbar_reserve()`) gets the right
    /// answer without asking which backend it's talking to (issue #776).
    ///
    /// Distinct from [`Self::terminal_scrollbar_default_width`]: that one
    /// is the gutter width of the `Terminal` *primitive*'s own scrollbar,
    /// which paints inside `draw_terminal`'s allotted rect and applies
    /// even to TUI (which draws it as a real column). This one is
    /// surrounding OS/toolkit scrollbar chrome that no primitive
    /// controls — TUI has none of that, so it reserves nothing.
    ///
    /// Default `0.0`, correct for TUI and any backend with no overlay
    /// scrollbar chrome to dodge. GTK overrides this to reserve room for
    /// its `ScrolledWindow` overlay scrollbar.
    fn scrollbar_reserve(&self) -> f32 {
        0.0
    }

    /// Whether this backend can render individual text rows at a larger
    /// font size (per-line scale), so layouts should reserve taller
    /// vertical space for scaled rows.
    ///
    /// Returns `true` for backends that honour per-line font-size scale
    /// (GTK applies a Pango scale attr, so a 2.0× heading row draws at
    /// twice the line height). Returns `false` for fixed-cell backends
    /// like TUI, where a terminal cell can't grow — those render scaled
    /// rows at the normal cell height (bold heading text, no extra
    /// space). Primitives that carry per-line scale (e.g.
    /// [`RichTextPopup::line_scales`][crate::RichTextPopup]) feed this
    /// into their `*Measure` so the shared layout reserves the right
    /// height per backend without the consumer branching on backend
    /// type.
    ///
    /// Default `false` — only backends that actually scale glyphs
    /// override it.
    fn scales_text_rows(&self) -> bool {
        false
    }

    /// Snap a proposed height (in logical units — the same units as
    /// [`Self::line_height`]) to what this backend will actually paint.
    ///
    /// Cell-grid backends (TUI) quantize to whole rows; pixel backends
    /// paint fractional heights exactly and return the input unchanged.
    ///
    /// This is the sanctioned replacement for consumer-side `.round()` on
    /// a `line_height`-derived extent (quadraui#632). Before this method
    /// existed, `coord-tui` reimplemented TUI's cell-rounding rule by hand
    /// in two places — the layout math and the hit-test — that had to be
    /// kept in sync manually, and drifted by one row twice (#464, #995).
    /// Call this instead of modelling the rounding yourself:
    ///
    /// ```ignore
    /// let tab_bar_h = backend.snap_height(backend.line_height() * 1.4);
    /// ```
    ///
    /// Default: returns `h` unchanged (correct for every pixel backend).
    /// Only [`crate::tui::backend::TuiBackend`] overrides it.
    fn snap_height(&self, h: f32) -> f32 {
        h
    }

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
    // (see `docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §4). Backends opt in to the new
    // primitive in the same PR that adds it to the trait.
    /// Paint `tree` into `rect`. Non-header row height defaults to a
    /// backend-native derivation from `line_height` (GTK/macOS:
    /// `line_height * 1.4`; TUI: fixed 1 cell) but a host can pin it via
    /// [`TreeStyle::row_height`](crate::types::TreeStyle::row_height)
    /// instead — e.g. to match a fixed design-system row pitch
    /// independent of editor font size (#623). TUI ignores the override
    /// (a terminal cell can't be subdivided).
    fn draw_tree(&mut self, rect: Rect, tree: &TreeView);
    fn draw_list(&mut self, rect: Rect, list: &ListView);
    /// Draw `table` into `rect` and return its layout for hit-testing.
    /// Coordinate frame: **LOCAL** — `hit_regions` / row bounds are
    /// relative to `rect`'s origin, matching [`Self::data_table_layout`]
    /// (issue #505).
    fn draw_data_table(
        &mut self,
        rect: Rect,
        table: &DataTable,
        hovered_idx: Option<usize>,
    ) -> DataTableLayout;
    /// Compute the data-table layout without painting. Coordinate frame:
    /// **LOCAL** — relative to `rect`'s origin, `(0, 0)` at `rect`'s
    /// top-left; callers subtract `rect.x` / `rect.y` from absolute
    /// click coordinates before calling `hit_test` (issue #505; see the
    /// module doc's *Coordinate frames* section).
    fn data_table_layout(&self, rect: Rect, table: &DataTable) -> DataTableLayout;
    /// Horizontal scrollbar geometry for `list` rendered into `rect`, or
    /// `None` when its content fits. Each backend supplies its native row
    /// height; the resolved track + thumb are the same values the
    /// rasteriser paints, so consumers hit-test the returned thumb to
    /// implement drag without re-deriving geometry. Mirrors
    /// [`Backend::data_table_layout`]; see [`ListView::hscrollbar`].
    fn list_hscrollbar(&self, rect: Rect, list: &ListView) -> Option<Scrollbar>;
    /// Vertical scrollbar geometry for `list` rendered into `rect`, or
    /// `None` when `show_v_scrollbar` is `false` or all items fit. Each
    /// backend supplies its native row height; the resolved track + thumb
    /// are the same values the rasteriser paints, so consumers hit-test
    /// the returned thumb to implement drag without re-deriving geometry.
    /// Mirrors [`Backend::list_hscrollbar`]; see [`ListView::vscrollbar`].
    fn list_vscrollbar(&self, rect: Rect, list: &ListView) -> Option<Scrollbar>;
    /// Compute the list layout without painting — the no-paint twin of
    /// [`Self::draw_list`] (issue #506: `ListView::layout` already existed
    /// but `draw_list` computed it inline, with no way for a host to ask
    /// for the same geometry without repainting). `draw_list` and this
    /// method route through the same backend-internal resolver
    /// (`tui_list_layout` / `gtk_list_layout` / `win_list_layout` /
    /// `mac_list_layout`), so paint and no-paint can't drift apart. Every
    /// pixel backend's resolver shares its h-scrollbar row reservation
    /// via `primitives::layout_metrics::list_layout` (#712) — before
    /// that fix, `mac_list_layout` had no way to compute the reservation
    /// at all and `macos::list::draw_list` recomputed a second, reduced
    /// layout only at paint time, so this method's claim didn't actually
    /// hold on macOS whenever `max_content_width` forced a scrollbar.
    ///
    /// Coordinate frame: **LOCAL** — relative to `rect`'s origin, `(0, 0)`
    /// at `rect`'s top-left; does **not** account for
    /// [`ListView::bordered`]'s 1-cell/1px border inset, matching every
    /// backend's `draw_list` (issue #505).
    fn list_layout(&self, rect: Rect, list: &ListView) -> ListViewLayout;
    fn draw_form(&mut self, rect: Rect, form: &Form);
    fn draw_palette(&mut self, rect: Rect, palette: &Palette);

    /// Compute the palette layout without painting — the no-paint twin
    /// of [`Self::draw_palette`] (issue #818). Added deliberately later
    /// than sibling `<name>_layout` methods: see
    /// `docs/decisions/DECISIONS.md` D-007 "Palette: deferred, not
    /// missed" for why shipping this before fixing a latent 1px item-row
    /// drift in `gtk::draw_palette` (and TUI's `draw_palette` never
    /// calling [`Palette::layout`] at all) would have made that drift
    /// externally visible the moment a host trusted this for
    /// hit-testing. Both are fixed as of this method landing — every
    /// backend's `palette_layout` now shares its row/column arithmetic
    /// with its own `draw_palette` (`tui_palette_layout`,
    /// `gtk_palette_layout`, `mac_palette_layout`, `win_palette_layout`).
    ///
    /// Coordinate frame: **LOCAL** — relative to `rect`'s origin, `(0, 0)`
    /// at `rect`'s top-left, matching [`Palette::layout`]'s native
    /// contract. Callers subtract `rect.x` / `rect.y` from an absolute
    /// click coordinate before calling [`PaletteLayout::hit_test`]
    /// (`PRIMITIVE_RULES.md`'s coordinate-frame convention).
    ///
    /// TUI's implementation never reports [`PaletteLayout::scrollbar`]
    /// (always `None`) — see `tui::palette::tui_palette_layout`'s doc for
    /// why reporting one would invent a second thumb-position formula
    /// disagreeing with what TUI actually paints.
    fn palette_layout(&self, rect: Rect, palette: &Palette) -> PaletteLayout;

    /// Draw settings-panel chrome: a header row and, when `rect.height`
    /// leaves room for it, a search input row beneath it, designed to
    /// sit immediately above a [`Form`] body. See
    /// `tui::draw_settings_chrome` / `gtk::draw_settings_chrome` for the
    /// exact row layout, `" / "`-prefixed prompt construction, and
    /// placeholder logic.
    ///
    /// **Height contract (issue #1041 review):** the header row always
    /// paints at one `line_height`. The search row paints only when
    /// `rect.height` covers a second row (every implementer uses the
    /// same `line_height * 1.5` threshold, matching TUI's `area.height
    /// < 2` cell check) — a caller that reserves a single row (e.g.
    /// [`crate::compose::sidebar_panel_body::SidebarPanelChrome::Header`])
    /// gets a header-only strip, not a second, unrequested row painted
    /// past the rect the caller reserved. Before this contract existed,
    /// GTK/macOS/Windows always painted both rows regardless of
    /// `rect.height`, silently overpainting whatever a "header-only"
    /// caller placed directly beneath the chrome.
    ///
    /// No default impl — every backend implementer sees this as a compile
    /// error and fills in a real rasteriser (`docs/decisions/BACKEND_TRAIT_PROPOSAL.md`
    /// §4, `PRIMITIVE_RULES.md` rule 7). Do not add a no-op default here;
    /// see `docs/SMELL_AUDIT_2026-07.md` PORT-01 for why that pattern is a
    /// portability risk, not a precedent to follow.
    fn draw_settings_chrome(
        &mut self,
        rect: Rect,
        header_text: &str,
        query: &str,
        placeholder: &str,
        active: bool,
    );

    // Layout-passthrough primitives (per docs/decisions/BACKEND_TRAIT_PROPOSAL.md
    // §6.2). Each backend computes the primitive's layout internally
    // using its native measurer (cells for TUI, Pango / DirectWrite /
    // Core Text pixels for the others) — apps don't have access to
    // those handles, so layout precomputation can't live caller-side.
    //
    // Methods that produce hit-region data (clickable segments,
    // close-button rects, link rects) return it directly so callers
    // route clicks against the same data the rasteriser used to paint.
    /// Draw a status bar, reading hover/pressed state from a single
    /// [`InteractionState`] keyed by [`WidgetId`] (issue #819).
    ///
    /// The rasteriser tints the background of the clickable segment
    /// whose `action_id` matches `interaction.hovered()` /
    /// `interaction.pressed()` — the primitive itself carries no mouse
    /// state. Returns hit regions in **bar-local coordinates**
    /// (relative to `rect.x` / `rect.y`) for each segment carrying an
    /// `action_id`. Caller dispatches clicks against the returned list.
    ///
    /// This is the implemented method; the positional
    /// [`Self::draw_status_bar`] is a deprecated shim over it.
    fn draw_status_bar_interactive(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        interaction: &InteractionState,
    ) -> StatusBarLayout;
    /// Draw a status bar with hover/pressed supplied positionally.
    ///
    /// # Deprecated (issue #819)
    ///
    /// Superseded by [`Self::draw_status_bar_interactive`], which reads
    /// the same two values out of one [`InteractionState`] keyed by
    /// [`WidgetId`] instead of two positional `Option<&WidgetId>`
    /// slots. Kept as a *working* forwarding shim — not a stub — per
    /// `CLAUDE.md` rule 3's two-PR deprecate-then-remove protocol:
    /// `vimcode` calls this method directly today (see the PR's
    /// *Downstream impact* section), so removing it outright would
    /// break its build on the next `develop` pull.
    #[deprecated(
        since = "0.0.1",
        note = "use `draw_status_bar_interactive` (hover/pressed come from an `InteractionState` keyed by `WidgetId`) — issue #819"
    )]
    fn draw_status_bar(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> StatusBarLayout {
        let interaction = InteractionState::from_parts(hovered_id.cloned(), pressed_id.cloned());
        self.draw_status_bar_interactive(rect, bar, &interaction)
    }
    /// Draw a tab bar. `hovered_close_tab` carries per-frame hover
    /// state so the rasteriser can paint a hover background behind the
    /// hovered tab's close glyph (the primitive itself carries no
    /// mouse state). Returns [`TabBarHits`] for click dispatch +
    /// scroll-offset reconciliation.
    ///
    /// `TabBarHits` itself is `#[deprecated]` (issue #823); this method's
    /// signature isn't changing in this PR — see that struct's doc for why
    /// the actual six-method/four-backend swap to [`TabBarLayout`] is
    /// separate follow-up work, not something this shim PR does.
    #[allow(deprecated)]
    fn draw_tab_bar(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits;
    /// Draw a tab bar, returning [`TabBarLayout`] instead of the
    /// deprecated [`TabBarHits`] (issue #919).
    ///
    /// #823 deprecated `TabBarHits` but left every accessor returning it
    /// with no replacement a consumer could call instead — #919 is that
    /// replacement, added *alongside* [`Self::draw_tab_bar`] rather than
    /// in place of it, so no existing caller breaks (`CLAUDE.md` rule 8).
    /// Paints exactly what [`Self::draw_tab_bar`] paints (icon-less
    /// sidecar of [`Self::draw_tab_bar_icons_layout`]); the two must never
    /// drift apart, same invariant [`Self::draw_tab_bar`] already holds
    /// against [`Self::draw_tab_bar_icons`].
    ///
    /// Returned in [`TabBarLayout`]'s own **bar-relative** coordinate
    /// space (origin at `rect`'s top-left) — see that struct's doc. This
    /// is deliberately *not* the target-surface-absolute space
    /// [`TabBarHits`] uses; the two return types have different,
    /// independently-documented contracts, and this method follows
    /// `TabBarLayout`'s.
    ///
    /// No default impl — every backend implementer sees this as a
    /// compile error and fills in a real rasteriser, mirroring
    /// [`Self::draw_tab_bar`]'s own no-default rule (rule 7).
    fn draw_tab_bar_layout(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
    ) -> TabBarLayout;
    /// Draw a tab bar with per-tab icon glyphs (#620) — VS Code's
    /// coloured language/file-type badge on each tab.
    ///
    /// `icons` is a **sidecar slice parallel to `bar.tabs`**: entry `i`
    /// decorates tab `i`, `None` (or an index past the slice's end)
    /// means "no icon", and `&[]` is exactly equivalent to
    /// [`Self::draw_tab_bar`]. Resolve entries with
    /// [`crate::tab_icon_at`] / [`crate::tab_icon_cols`] rather than
    /// indexing, so every backend shares one short-slice convention.
    ///
    /// The icons ride *beside* the primitive instead of inside
    /// [`crate::TabItem`] because a new field on that struct is a hard
    /// break for both downstream consumers and the sealed acceptance
    /// slices, which build it with exhaustive literals — whereas a new
    /// `Backend` method with no default breaks nobody
    /// (`PRIMITIVE_RULES.md` rule 8's blast-radius table). See
    /// [`crate::TabIcon`] for the full rationale.
    ///
    /// Implementors: put the real rasteriser **here** and let
    /// [`Self::draw_tab_bar`] forward with `&[]`, so an icon-less bar
    /// and an icon bar can never drift apart. An icon must widen its
    /// tab by exactly what the paint reserves, so close-button and
    /// tab-slot hit geometry stay on the glyphs the user sees; icon-less
    /// tabs must keep byte-identical geometry to `draw_tab_bar`.
    ///
    /// No default impl — every backend implementer sees this as a
    /// compile error and fills in a real rasteriser
    /// (`docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §4, `PRIMITIVE_RULES.md` rule 7).
    ///
    /// `TabBarHits` is `#[deprecated]` (issue #823) — see [`Self::draw_tab_bar`].
    #[allow(deprecated)]
    fn draw_tab_bar_icons(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits;
    /// Draw a tab bar with per-tab icon glyphs, returning [`TabBarLayout`]
    /// instead of the deprecated [`TabBarHits`] (issue #919). The
    /// icon-aware twin of [`Self::draw_tab_bar_layout`], exactly as
    /// [`Self::draw_tab_bar_icons`] is the twin of [`Self::draw_tab_bar`]
    /// — same `icons` sidecar convention, same bar-relative coordinate
    /// space as [`Self::draw_tab_bar_layout`].
    ///
    /// No default impl — same rule-7 reasoning as
    /// [`Self::draw_tab_bar_icons`].
    fn draw_tab_bar_icons_layout(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarLayout;
    /// Draw a tab bar with an explicit [`TabChrome`] request (#631): which
    /// decoration, if any, should enclose the active tab's full content
    /// (label *and* close glyph).
    ///
    /// Added rather than folded into [`Self::draw_tab_bar`]'s signature,
    /// and given a default body, so that #631 breaks no existing `Backend`
    /// implementor and no existing call site — see
    /// `primitives::tab_bar`'s [`TabChrome`] doc, which mirrors #541's
    /// [`crate::TooltipChrome`] shape for exactly this reason.
    ///
    /// The default body **ignores `chrome`** and delegates to
    /// [`Self::draw_tab_bar`], i.e. renders [`crate::TabFrame::None`] —
    /// the correct fallback for a backend with no frame vocabulary of its
    /// own. The TUI and GTK backends override it and honour
    /// [`crate::TabFrame::Brackets`] in full.
    ///
    /// `TabBarHits` is `#[deprecated]` (issue #823) — see [`Self::draw_tab_bar`].
    #[allow(deprecated)]
    fn draw_tab_bar_with_chrome(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
        chrome: &TabChrome,
    ) -> TabBarHits {
        let _ = chrome;
        self.draw_tab_bar(rect, bar, hovered_close_tab)
    }
    /// Draw a tab bar with an explicit [`TabChrome`] request, returning
    /// [`TabBarLayout`] instead of the deprecated [`TabBarHits`] (issue
    /// #919) — the `TabBarLayout`-returning twin of
    /// [`Self::draw_tab_bar_with_chrome`].
    ///
    /// Default body **ignores `chrome`** and delegates to
    /// [`Self::draw_tab_bar_layout`], matching
    /// [`Self::draw_tab_bar_with_chrome`]'s own default exactly.
    fn draw_tab_bar_with_chrome_layout(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        hovered_close_tab: Option<usize>,
        chrome: &TabChrome,
    ) -> TabBarLayout {
        let _ = chrome;
        self.draw_tab_bar_layout(rect, bar, hovered_close_tab)
    }
    /// Draw an activity bar. `hovered_idx` carries per-frame hover
    /// state so the rasteriser can paint a tint on the hovered row.
    /// Returns per-row hit regions for click + tooltip dispatch.
    ///
    /// # Coordinate space — **relative to `rect`** (issue #552)
    ///
    /// Each [`ActivityBarRowHit`]'s `y_start` / `y_end` is measured from
    /// the **top edge of `rect`**: the first row starts at `0.0` no
    /// matter where the bar sits on the target surface. Implementors
    /// must **not** fold `rect.y` into the returned spans, even though
    /// they need the absolute value to paint. Callers add the bar origin
    /// themselves (`hit.y_start + rect.y`).
    ///
    /// Note this is deliberately the *opposite* convention from
    /// [`Self::tab_bar_layout`] / [`Self::draw_tab_bar`], whose
    /// [`TabBarHits`] spans are absolute. The split is historical but now
    /// pinned: the activity bar's space is what GTK, macOS, and the
    /// shared [`activity_bar_hits`] helper already produced, and what
    /// `AppShell` assumes in both its click and hover readers.
    ///
    /// [`Self::activity_bar_layout`] must return the same space. A
    /// drifting backend fails
    /// `tui::activity_bar::tests::hit_regions_are_bar_relative_not_absolute`
    /// / `hit_regions_do_not_move_when_the_bar_does`, plus the
    /// `activity_click_*_parity` / `activity_hover_*_parity` cross-backend
    /// tests in `tests/cross_backend_parity.rs`.
    fn draw_activity_bar(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<ActivityBarRowHit>;

    /// Draw an activity bar with an explicit [`ActivityBarStyle`] request
    /// (#658) — currently just the active item's row-fill colour, VS Code
    /// style (no line, a soft chip on the row itself).
    ///
    /// Added rather than folded into [`Self::draw_activity_bar`]'s
    /// signature, and given a default body, so that #658 breaks no existing
    /// `Backend` implementor and no existing call site — mirrors
    /// [`crate::TooltipChrome`] / [`Self::draw_tooltip_with_chrome`] (#541)
    /// and [`TabChrome`] / [`Self::draw_tab_bar_with_chrome`] (#631), which
    /// solve the identical "additive field would break exhaustive
    /// downstream literals" problem for `Tooltip` and `TabBar`. See
    /// [`ActivityBarStyle`]'s doc for the full reasoning.
    ///
    /// The default body **ignores `style`** and delegates to
    /// [`Self::draw_activity_bar`] — the correct fallback for a backend
    /// with no fill vocabulary of its own. The TUI, GTK, and macOS
    /// backends override it and honour `style.active_bg` in full; Win
    /// takes the default for now (#19 — every `draw_*` method there is a
    /// stub).
    fn draw_activity_bar_with_style(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
        style: &ActivityBarStyle,
    ) -> Vec<ActivityBarRowHit> {
        let _ = style;
        self.draw_activity_bar(rect, bar, hovered_idx)
    }

    /// Compute the status bar layout without painting. Same measurement
    /// logic as `draw_status_bar` — call after `ScreenLayout::draw()` to
    /// recover hit regions for click dispatch.
    ///
    /// Returns `hit_regions` in the same **bar-local** space as
    /// [`Self::draw_status_bar`] (relative to `rect.x` / `rect.y`).
    ///
    /// Audited under issue #552 and **ruled out**: all four paths (TUI /
    /// GTK × draw / layout) return the primitive's own unshifted
    /// `StatusBar::layout` output, so paint and no-paint already agree and
    /// no backend folds the origin in. Unlike the activity bar, nothing
    /// here needed changing — only this note, so the next reader doesn't
    /// have to re-derive it.
    fn status_bar_layout(&self, rect: Rect, bar: &StatusBar) -> StatusBarLayout;

    /// Compute the tab bar layout without painting. Returns the same
    /// `TabBarHits` as `draw_tab_bar` — slot positions, close bounds, and
    /// right-segment bounds are all in **target-surface (absolute)
    /// coordinates**, i.e. shifted by `rect.x` / `rect.y` so callers can
    /// compare them directly against raw click coordinates without any
    /// further adjustment.
    ///
    /// Audited under issue #552 and **fixed**: this was documented
    /// absolute but returned bar-relative x on *both* TUI and GTK, because
    /// only `draw_tab_bar` applied the origin shift. Both impls now route
    /// through [`shift_tab_bar_hits`], the same helper the rasterisers
    /// use. Note the tab bar's absolute convention is the opposite of
    /// [`Self::draw_activity_bar`]'s bar-relative one — deliberate, and
    /// now stated on both.
    ///
    /// # Downstream impact (issue #552)
    ///
    /// This changes the actual values `tab_bar_layout` returns, not just
    /// its doc. `grep -rn "tab_bar_layout" ~/src/claude-coordinator/tui/src
    /// ~/src/vimcode/src`: `coord-tui`'s `tui/src/app/render.rs:229` only
    /// reads `.correct_scroll_offset`, unaffected. `vimcode`'s
    /// `src/gtk/mod.rs` — `tab_hits_to_pixel_hits` (~line 141),
    /// `abs_visible_slots` (~line 216), `abs_close_record` (~line 198),
    /// plus the call sites in `src/gtk/click.rs` and `src/gtk/draw.rs` —
    /// all consume `hits.slot_positions` / `close_bounds` under the
    /// explicit assumption the doc comment at `gtk/mod.rs:135-137` states
    /// ("absolute pixel x, from `Backend::tab_bar_layout`"). Since the
    /// pre-fix implementation didn't actually deliver that, vimcode's
    /// `tab_hits_to_pixel_hits` — which subtracts `bar_left_x` from
    /// already-relative input via its `rel()` closure — was very likely
    /// silently double-subtracting whenever `rect.x != 0` (sidebar
    /// visible, or the 2nd+ split-group tab bar), shifting GTK tab-bar
    /// click/close-button hit-testing left by `rect.x`. This PR is
    /// believed to **fix** that latent bug, not introduce a regression —
    /// but that is this repo's analysis, not a vimcode-side confirmation;
    /// vimcode should verify with its own GTK tab-bar click tests before
    /// relying on the corrected geometry.
    ///
    /// `TabBarHits` is `#[deprecated]` (issue #823) — see [`Self::draw_tab_bar`].
    #[allow(deprecated)]
    fn tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarHits;

    /// Compute the tab bar layout without painting, returning
    /// [`TabBarLayout`] instead of the deprecated [`TabBarHits`] (issue
    /// #919). The `TabBarLayout`-returning twin of [`Self::tab_bar_layout`],
    /// added alongside it rather than in place of it (`CLAUDE.md` rule 8)
    /// — every backend already resolves this geometry internally before
    /// narrowing it down to `TabBarHits`; this exposes it directly.
    ///
    /// Returned in `TabBarLayout`'s own **bar-relative** space — see that
    /// struct's doc — which is the *opposite* convention from
    /// [`Self::tab_bar_layout`]'s documented absolute one. This is not a
    /// coordinate-space bug: the two return types each follow their own
    /// struct's contract, same split `TabBarLayout`'s own doc draws
    /// against `Self::draw_activity_bar` / `Self::activity_bar_layout`.
    /// A caller wanting an absolute point adds the bar's own `rect.x` /
    /// `rect.y` origin, exactly as [`TabBarLayout::tab_center`] /
    /// [`TabBarLayout::tab_close_center`]'s docs already describe for
    /// `TuiDriver` / `GtkDriver`.
    ///
    /// No default impl — same rule-7 reasoning as
    /// [`Self::tab_bar_layout_icons`].
    fn resolve_tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarLayout;

    /// Compute the tab bar layout without painting, for a bar painted
    /// with per-tab icons (#620). The no-paint twin of
    /// [`Self::draw_tab_bar_icons`], exactly as [`Self::tab_bar_layout`]
    /// is the twin of [`Self::draw_tab_bar`] — same absolute-coordinate
    /// contract, same `icons` sidecar convention.
    ///
    /// A caller that paints with icons **must** route its no-paint click
    /// geometry through this method rather than [`Self::tab_bar_layout`]:
    /// the icon reservation widens every decorated tab, so the icon-less
    /// twin would report slot and close-button bounds shifted left of
    /// the painted glyphs. `&[]` makes the two identical.
    ///
    /// No default impl — same rule-7 reasoning as
    /// [`Self::draw_tab_bar_icons`].
    ///
    /// `TabBarHits` is `#[deprecated]` (issue #823) — see [`Self::draw_tab_bar`].
    #[allow(deprecated)]
    fn tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<TabIcon>],
    ) -> TabBarHits;

    /// Compute the tab bar layout without painting, for a bar painted
    /// with per-tab icons, returning [`TabBarLayout`] instead of the
    /// deprecated [`TabBarHits`] (issue #919). The icon-aware twin of
    /// [`Self::resolve_tab_bar_layout`], exactly as
    /// [`Self::tab_bar_layout_icons`] is the twin of
    /// [`Self::tab_bar_layout`] — same `icons` sidecar convention, same
    /// bar-relative coordinate space as [`Self::resolve_tab_bar_layout`].
    ///
    /// No default impl — same rule-7 reasoning as
    /// [`Self::tab_bar_layout_icons`].
    fn resolve_tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<TabIcon>],
    ) -> TabBarLayout;

    /// Compute the tab bar layout without painting, for a bar painted
    /// with [`Self::draw_tab_bar_with_chrome`] (#631). The no-paint twin
    /// of that method, exactly as [`Self::tab_bar_layout`] is the twin of
    /// [`Self::draw_tab_bar`] — same absolute-coordinate contract.
    ///
    /// A caller that paints with chrome **must** route its no-paint click
    /// geometry through this method rather than [`Self::tab_bar_layout`]:
    /// [`crate::TabFrame::Brackets`] widens the active tab and moves its
    /// close-button hit region, so the chrome-less twin would report it
    /// shifted from the painted glyph.
    ///
    /// Default body ignores `chrome` and delegates to
    /// [`Self::tab_bar_layout`], matching [`Self::draw_tab_bar_with_chrome`]'s
    /// default.
    ///
    /// `TabBarHits` is `#[deprecated]` (issue #823) — see [`Self::draw_tab_bar`].
    #[allow(deprecated)]
    fn tab_bar_layout_with_chrome(
        &self,
        rect: Rect,
        bar: &TabBar,
        chrome: &TabChrome,
    ) -> TabBarHits {
        let _ = chrome;
        self.tab_bar_layout(rect, bar)
    }

    /// Compute the tab bar layout without painting, for a bar painted
    /// with chrome, returning [`TabBarLayout`] instead of the deprecated
    /// [`TabBarHits`] (issue #919) — the `TabBarLayout`-returning twin of
    /// [`Self::tab_bar_layout_with_chrome`].
    ///
    /// Default body ignores `chrome` and delegates to
    /// [`Self::resolve_tab_bar_layout`], matching
    /// [`Self::tab_bar_layout_with_chrome`]'s own default exactly.
    fn resolve_tab_bar_layout_with_chrome(
        &self,
        rect: Rect,
        bar: &TabBar,
        chrome: &TabChrome,
    ) -> TabBarLayout {
        let _ = chrome;
        self.resolve_tab_bar_layout(rect, bar)
    }

    /// Compute activity bar row hit regions without painting. Returns
    /// the same **bar-relative** spans as [`Self::draw_activity_bar`] —
    /// `y_start` / `y_end` measured from `rect.y`, first row at `0.0`.
    /// See that method for the full contract (issue #552).
    fn activity_bar_layout(&self, rect: Rect, bar: &ActivityBar) -> Vec<ActivityBarRowHit>;

    /// Draw a terminal cell grid. No hit-region data is returned;
    /// terminal selection is driven by mouse drag against cell
    /// dimensions, which the app already tracks.
    fn draw_terminal(&mut self, rect: Rect, term: &Terminal);
    /// The reserved width of a `Terminal`'s scrollbar gutter when
    /// [`TerminalScrollbar::width`](crate::primitives::terminal::TerminalScrollbar::width)
    /// is `None`, in this backend's own surface-native unit (issue #506
    /// review fix). Every `draw_terminal` implementation falls back to a
    /// hardcoded default when the caller didn't specify a width, and
    /// that default is backend-shaped, not uniform: TUI's cells *are*
    /// the coordinate system, so its gutter is exactly one column
    /// (`src/tui/terminal.rs`'s `sb_cols: … .unwrap_or(1)`); GTK, macOS,
    /// and Win all measure in pixels and use 8px
    /// (`src/gtk/backend.rs`, `src/macos/backend.rs`,
    /// `src/win/backend.rs`, each `sb_width: … .unwrap_or(8.0)`).
    /// [`Self::terminal_layout`]'s default body calls this so its
    /// scrollbar reservation matches whichever default the paint path
    /// actually used, instead of silently assuming the pixel-backend
    /// value for every backend (or ignoring the reservation entirely).
    ///
    /// Default: `8.0`, matching GTK/macOS/Win. TUI overrides this to
    /// `1.0` — see `TuiBackend::terminal_scrollbar_default_width`.
    fn terminal_scrollbar_default_width(&self) -> f32 {
        8.0
    }
    /// Compute the viewport → grid conversion [`Self::draw_terminal`]
    /// implicitly uses (issue #506: `Terminal::layout` already existed as
    /// a pure fn but no `Backend` method exposed it, so hosts had to
    /// re-derive `rect.width / char_width` by hand to hit-test a click
    /// against a cell). Uses this backend's own [`Self::char_width`] /
    /// [`Self::line_height`] as the cell dimensions — TUI's `(1.0, 1.0)`
    /// reproduces its uniform cell grid exactly; pixel backends get the
    /// same font metrics `draw_terminal`'s cell iteration assumes.
    ///
    /// When `term.scrollbar` is `Some`, the viewport width fed to
    /// [`crate::primitives::terminal::Terminal::layout`] is first reduced
    /// by the scrollbar's reserved width — `sb.width` when set, else
    /// [`Self::terminal_scrollbar_default_width`] — exactly as every
    /// backend's real `draw_terminal` reserves that gutter *before*
    /// iterating cells (`cell_area_w = area.width.saturating_sub(sb_cols)`
    /// on TUI; `cell_area_w = (rect.width - sb_width).max(0.0)` on
    /// GTK/macOS/Win). Skipping this step would report `grid_cols` wide
    /// enough to claim the scrollbar gutter itself as a clickable cell —
    /// exactly the "paint and no-paint silently disagree" bug class rule
    /// 5 exists to prevent (issue #506 review fix).
    ///
    /// Coordinate frame: **LOCAL** — relative to `rect`'s origin; see
    /// [`crate::primitives::terminal::TerminalLayout::hit_test`] /
    /// [`crate::primitives::terminal::TerminalLayout::cell_bounds`],
    /// neither of which fold in an origin offset (issue #505).
    ///
    /// Default body: uniform for every backend, since it's a pure
    /// function of the metrics above plus [`Self::terminal_scrollbar_default_width`]
    /// — no backend needs to override this.
    fn terminal_layout(
        &self,
        rect: Rect,
        term: &Terminal,
    ) -> crate::primitives::terminal::TerminalLayout {
        let sb_reserved = match &term.scrollbar {
            Some(sb) => sb
                .width
                .map(|w| w as f32)
                .unwrap_or_else(|| self.terminal_scrollbar_default_width()),
            None => 0.0,
        };
        let cell_area_w = (rect.width - sb_reserved).max(0.0);
        term.layout(
            cell_area_w,
            rect.height,
            self.char_width(),
            self.line_height(),
        )
    }
    /// Draw a vertical divider between two split terminal panes.
    /// `rect.x` is the divider's column, `rect.y` its top row, and
    /// `rect.height` its length; `rect.width` is ignored — the
    /// divider is always a single cell (TUI) or 1px (GTK/macOS) wide.
    /// See `tui::draw_terminal_divider` / `gtk::draw_terminal_divider`
    /// for the exact glyph/fill painted.
    ///
    /// No default impl — every backend implementer sees this as a
    /// compile error and fills in a real rasteriser
    /// (`docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §4, `PRIMITIVE_RULES.md` rule 7).
    /// Do not add a no-op default here; see
    /// `docs/SMELL_AUDIT_2026-07.md` PORT-01 for why that pattern is a
    /// portability risk, not a precedent to follow.
    fn draw_terminal_divider(&mut self, rect: Rect);
    /// Fill `rect` edge-to-edge with a solid `color` — no text, no
    /// segments, no per-row seams. For chrome elements that are a plain
    /// color block (e.g. `AppShell`'s sidebar/editor resize divider),
    /// not a widget.
    ///
    /// Added by issue #996: the divider used to be faked as N stacked
    /// one-row `StatusBar`s (`fg == bg`, blank text) — exact on a cell
    /// grid, where consecutive rows abut by construction, but wrong on a
    /// pixel backend, where `draw_status_bar_interactive` fills only
    /// `current_line_height` regardless of the row rect's own height, so
    /// every row painted short of its own pitch and the gaps between
    /// rows showed as a dashed line. A single fill over the *whole*
    /// rect is exact on every backend and strictly cheaper than N
    /// layouts per frame — use this instead of the `StatusBar` hack for
    /// any future solid-fill chrome.
    ///
    /// No default impl — every backend implementer sees this as a
    /// compile error and fills in a real rasteriser
    /// (`docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §4, `PRIMITIVE_RULES.md` rule 7).
    fn draw_solid_fill(&mut self, rect: Rect, color: Color);
    /// Draw a `TextDisplay` (streaming-text panel — log viewer, output
    /// pane, YAML view, etc). No hit-region data is returned;
    /// `TextDisplay` itself is non-interactive (selection / scroll
    /// happen at the panel chrome level, not at the line/span level).
    fn draw_text_display(&mut self, rect: Rect, td: &TextDisplay);

    /// Draw a [`CommandLine`] bar (editor `:` / `/` / `?` prompt or
    /// message display). Fills `rect` with the command line background,
    /// renders text (left- or right-aligned), and optionally draws an
    /// insert cursor at `cursor_offset`.
    fn draw_command_line(&mut self, rect: Rect, cmd: &CommandLine);

    /// Draw a [`CommandLine`] bar exactly like [`Self::draw_command_line`],
    /// additionally painting a selection highlight behind the text for
    /// `selection` (a `(start, end)` byte-offset pair into `cmd.text`,
    /// either order — same contract as
    /// [`crate::primitives::command_line::CommandLineLayout::selection_bounds`],
    /// which supplies the rect this paints). `None` (or an empty/
    /// zero-width range) paints identically to `draw_command_line`.
    ///
    /// This is a **new method, not a new `CommandLine` field or a changed
    /// `draw_command_line` signature** (issue #1001) — `CommandLine` is an
    /// all-`pub`-field, exhaustively-constructed-by-consumers primitive
    /// (see `crate::primitives::text_input`'s module doc for why that
    /// makes a field addition breaking under `PRIMITIVE_RULES.md` rule 8),
    /// so the selection a host is tracking is threaded through as an
    /// argument instead — the same non-breaking shape `TextInput`/
    /// `TextEditor` (#833) established. `Backend` itself is in-tree-only
    /// (`CLAUDE.md`'s *Downstream consumers* section), so adding a method
    /// here costs no downstream consumer anything.
    fn draw_command_line_selection(
        &mut self,
        rect: Rect,
        cmd: &CommandLine,
        selection: Option<(usize, usize)>,
    );

    /// Compute the click/selection layout `draw_command_line` paints from
    /// (issue #705). Hosts call this to hit-test a click to a **byte
    /// offset** in `cmd.text` (`CommandLineLayout::hit_test`) and to turn a
    /// selection range back into a paintable rect
    /// (`CommandLineLayout::selection_bounds`), without re-deriving glyph
    /// metrics or repainting.
    ///
    /// This is the fix for the gap `CommandLine` shipped with: the TUI
    /// rasteriser could support mouse drag-selection only by reading back
    /// inverted terminal cells after paint, a trick with no GTK/macOS/Win
    /// equivalent — so the command line was mouse-selectable on TUI and
    /// structurally could not be on any pixel backend. Every backend now
    /// exposes the same character-offset mapping, so a host can share one
    /// selection implementation instead of leaving pixel backends behind.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`;
    /// callers compare directly against raw click coordinates, matching
    /// [`Self::text_input_layout`] (issue #505).
    fn command_line_layout(&self, rect: Rect, cmd: &CommandLine) -> CommandLineLayout;

    /// Compute the text-display layout the rasteriser would produce for
    /// `td` in `rect`, using the backend's native metrics. Hosts call
    /// this to drive hit-testing for scrollbar drag interaction without
    /// re-deriving metrics — paint and click consume one layout per
    /// frame, the source-of-truth contract.
    ///
    /// Coordinate frame: **LOCAL** — relative to `rect`'s origin
    /// (issue #505).
    fn text_display_layout(&self, rect: Rect, td: &TextDisplay) -> TextDisplayLayout;

    /// Draw a [`TextInput`] (multi-line text entry) and return the
    /// resolved layout for hit-testing. Backends paint the border,
    /// text lines, cursor, and placeholder (when active).
    ///
    /// Coordinate frame: **ABSOLUTE** — `content_bounds` / hit regions
    /// are shifted by `rect.x` / `rect.y`, matching [`Self::text_input_layout`]
    /// (issue #505).
    fn draw_text_input(&mut self, rect: Rect, ti: &TextInput) -> TextInputLayout;

    /// Compute the layout `draw_text_input` would produce. Used by
    /// hosts to route clicks without re-rendering.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`;
    /// callers compare directly against raw click coordinates
    /// (issue #505).
    fn text_input_layout(&self, rect: Rect, ti: &TextInput) -> TextInputLayout;

    /// Draw a [`Tooltip`] popup at its caller-resolved layout, with the
    /// default chrome ([`crate::TooltipBorder::Full`], no title). The
    /// caller computes anchor + viewport + content measurement and
    /// asks `tooltip.layout(...)` for the bounds. Tooltips are
    /// non-interactive — no hit data returned.
    ///
    /// To ask for a different border, or for a title in the top rule,
    /// call [`Backend::draw_tooltip_with_chrome`].
    fn draw_tooltip(&mut self, tooltip: &Tooltip, layout: &TooltipLayout);

    /// Draw a [`Tooltip`] popup with an explicit [`TooltipChrome`]
    /// request (#541): which border to stroke (`Sides` / `Full` /
    /// `None`) and an optional title to embed in `Full`'s top rule.
    ///
    /// Added rather than folded into [`Backend::draw_tooltip`]'s
    /// signature, and given a default body, so that #541 breaks no
    /// existing `Backend` implementor and no existing call site — see
    /// `primitives::tooltip`'s module doc.
    ///
    /// The default body **ignores `chrome`** and delegates to
    /// `draw_tooltip`, i.e. renders `TooltipChrome::default()`. That is
    /// the correct fallback for a backend that has no chrome vocabulary
    /// of its own, but it does mean a `Sides`/`None`/title request is
    /// silently dropped by any backend that hasn't overridden this.
    /// The TUI, GTK and macOS backends all override it and honour the
    /// request in full.
    fn draw_tooltip_with_chrome(
        &mut self,
        tooltip: &Tooltip,
        layout: &TooltipLayout,
        chrome: &TooltipChrome,
    ) {
        let _ = chrome;
        self.draw_tooltip(tooltip, layout);
    }

    /// Draw a [`ContextMenu`] popup at its caller-resolved layout.
    /// Returns the per-clickable-item hit rectangles + their
    /// `WidgetId`s so the caller's click handler can resolve mouse
    /// events without re-running layout.
    fn draw_context_menu(
        &mut self,
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
    ) -> Vec<(Rect, WidgetId)>;

    /// Draw a [`Dialog`] popup at its caller-resolved layout. Returns
    /// the per-button hit rectangles in the same order as
    /// `dialog.buttons`'s visible entries so the caller's click
    /// handler can resolve a click to a button without re-running
    /// layout. Mirrors [`draw_context_menu`](Self::draw_context_menu).
    fn draw_dialog(&mut self, dialog: &Dialog, layout: &DialogLayout) -> Vec<Rect>;

    /// Draw a [`MultiSectionView`]. The backend computes the layout
    /// internally with native metrics (cells for TUI, pixels +
    /// `line_height` for GTK) and dispatches each section's body to
    /// the appropriate inner-primitive painter (tree, list, etc.).
    /// Hosts that need to hit-test clicks call [`Self::msv_layout`]
    /// for the same layout instance.
    fn draw_multi_section_view(&mut self, rect: Rect, view: &MultiSectionView);

    /// Compute the layout the rasteriser would produce for `view` in
    /// `rect`, using the backend's native metrics. Hosts call this
    /// to drive hit-testing without re-deriving metrics — paint and
    /// click consume one layout per frame, the source-of-truth
    /// contract `MultiSectionView` exists to enforce.
    ///
    /// Coordinate frame: **ABSOLUTE** — `hit_regions` / body bounds are
    /// shifted by `rect.x` / `rect.y`; callers compare them directly
    /// against raw click coordinates (issue #505).
    fn msv_layout(&self, rect: Rect, view: &MultiSectionView) -> MultiSectionViewLayout;

    /// Return the layout metrics this backend uses for MSV layout.
    /// Compose helpers cache these to compute layouts without a Backend
    /// reference at event-handling time.
    fn msv_metrics(&self) -> MsvLayoutMetrics;

    /// Compute the tree layout the rasteriser would produce. Used by
    /// hosts (especially MSV consumers) to resolve body clicks down
    /// to row indices without re-deriving the row pitch (1 cell
    /// uniform on TUI; `1.0×`/`1.4×` line_height by `Decoration` on
    /// GTK).
    ///
    /// Coordinate frame: **LOCAL** — `visible_rows.bounds` / `hit_regions`
    /// are relative to `rect`'s origin (`(0, 0)` at `rect`'s top-left);
    /// callers subtract `rect.x` / `rect.y` from absolute click
    /// coordinates before calling `hit_test` (issue #505; see
    /// `primitives::layout_metrics::tree_layout`'s doc, and
    /// `docs/LESSONS.md`'s `mac_tree_layout` postmortem for why this
    /// frame is load-bearing).
    fn tree_layout(&self, rect: Rect, tree: &TreeView) -> TreeViewLayout;

    /// Compute the form layout the rasteriser would produce for `form`
    /// in `rect`, using the backend's native metrics. Hosts call this
    /// to drive hit-testing — especially for `ToggleGroup` and
    /// `ButtonRow` fields where per-item hit regions depend on
    /// backend-specific text measurement.
    ///
    /// Coordinate frame: **LOCAL** — relative to `rect`'s origin
    /// (issue #505).
    fn form_layout(&self, rect: Rect, form: &Form) -> FormLayout;

    /// Draw an [`Editor`]. Returns paint-side data the host needs
    /// for chrome alignment (cursor pixel position for caret blink
    /// overlays, etc.). Asymmetric across backends: TUI populates
    /// the result; GTK paints its own caret and returns the default.
    fn draw_editor(&mut self, rect: Rect, editor: &Editor) -> EditorPaintResult;

    /// Compute the editor viewport layout (gutter / text / scrollbar
    /// bounds) without painting — the no-paint twin of [`Self::draw_editor`]
    /// (issue #506: `Editor::layout` already existed but no `Backend`
    /// method exposed it). Uses this backend's own [`Self::char_width`] /
    /// [`Self::line_height`] as the cell metrics — the same values
    /// `draw_editor` resolves them to on every backend that implements it
    /// today (`GtkBackend`/`WinBackend`/`MacBackend` all pass
    /// `current_char_width` / `current_line_height`, which is exactly what
    /// [`Self::char_width`] / [`Self::line_height`] return; TUI's fixed
    /// `(1.0, 1.0)` matches its uniform cell grid).
    ///
    /// Coordinate frame: **ABSOLUTE** — `text_bounds` / `gutter_bounds` /
    /// scrollbar bounds are shifted by `rect.x` / `rect.y`, matching
    /// [`Self::editor_col_at_x`]'s "x is an absolute (surface-space)
    /// coordinate" contract (issue #505).
    ///
    /// Default body: uniform for every backend, since it's a pure
    /// function of the two metrics above — no backend needs to override
    /// this.
    ///
    /// **Caller invariant:** `rect` here must be the same rect the
    /// backend actually paints with. This holds by construction on TUI
    /// (`draw_editor` takes the `rect` argument directly), but on GTK
    /// `GtkBackend::draw_editor` ignores its own `rect` parameter
    /// (`let _ = rect;`) and paints at `editor.rect` instead
    /// (`src/gtk/editor.rs`). The two happen to always match in every
    /// call site today, but nothing enforces it — pass a `rect` that
    /// diverges from `editor.rect` and this method's return value quietly
    /// stops matching what GTK painted (issue #506 review follow-up).
    fn editor_layout(&self, rect: Rect, editor: &Editor) -> EditorLayout {
        editor.layout(rect, self.char_width(), self.line_height())
    }

    /// Resolve a click x-coordinate to a text column on one visible row
    /// of an [`Editor`], honouring the same glyph-advance metrics
    /// [`Self::draw_editor`] painted that row with (bold / italic /
    /// `font_scale` spans on GTK's Pango layout; uniform monospace
    /// cells on TUI) — the paint↔click round-trip fix for #420.
    ///
    /// `layout` is the [`EditorLayout`] `editor.layout(...)` produced
    /// for the same viewport/metrics the caller painted with.
    /// `view_row` indexes into `editor.lines` (0 = topmost visible row
    /// — the same row [`crate::primitives::editor::EditorLayout::hit_test`]
    /// resolves from a y-coordinate). `x` is an absolute (surface-space)
    /// coordinate, matching the `x` passed to `hit_test`.
    ///
    /// Returns the resolved character column *within the buffer line*
    /// (already folds in `layout.scroll_left` and the row's
    /// `segment_col_offset` for wrap-continuation rows) — callers
    /// combine it with `hit_test`'s resolved `line` to get a full
    /// buffer position:
    ///
    /// ```ignore
    /// if let EditorHit::BufferPos { line, .. } = layout.hit_test(x, y) {
    ///     let view_row = line - layout.scroll_top;
    ///     let col = backend.editor_col_at_x(&layout, &editor, view_row, x);
    ///     // (line, col) is the resolved buffer position.
    /// }
    /// ```
    ///
    /// Default implementation delegates to
    /// [`EditorLayout::col_at_x`] (uniform monospace division) —
    /// correct for TUI and any backend whose configured font renders
    /// every glyph at the same advance width. `GtkBackend` overrides
    /// this with an exact Pango `xy_to_index` resolution against the
    /// same per-span-attributed layout `draw_editor` painted with, so
    /// a line containing bold / italic / `font_scale` spans (e.g. a
    /// markdown heading) resolves clicks against its *actual* painted
    /// glyph positions instead of a uniform grid.
    fn editor_col_at_x(
        &self,
        layout: &EditorLayout,
        editor: &Editor,
        view_row: usize,
        x: f32,
    ) -> usize {
        layout.col_at_x(editor, view_row, x)
    }

    /// Drive the terminal/OS **hardware** caret to match an
    /// [`Editor`]'s [`EditorCursorShape`] (issue #1015) — block for
    /// Normal/Visual, bar for Insert, underline for a pending
    /// replace-char command. This is a distinct surface from the
    /// caret [`Self::draw_editor`] *paints* into the frame buffer:
    /// GTK/Win/macOS already paint their own caret there and have no
    /// separate hardware cursor to steer, so the default here is a
    /// genuine, permanent no-op for them — not a gap to fill in
    /// later, the same "structurally absent, not merely
    /// unimplemented" reasoning [`Backend::tray`] uses for TUI.
    ///
    /// **TUI is the one backend that overrides this**, emitting
    /// DECSCUSR (`ESC [ n SP q`) via crossterm's `SetCursorStyle` to
    /// steer the terminal emulator's own cursor glyph. Before this
    /// method existed, that write had to happen in consumer code
    /// (vimcode's `shell_app.rs`, straight through crossterm) — a
    /// rule-6 violation this method closes off: only quadraui writes
    /// escape sequences.
    ///
    /// Not [`Self::draw_editor`]'s job to call this itself: painting
    /// happens every frame regardless of whether the shape changed,
    /// while retitling the hardware caret is a comparatively heavy
    /// terminal write hosts should only issue on an actual shape
    /// change. Callers own that debouncing (see
    /// `crate::tui::testing` for why driver tests never exercise the
    /// write path at all — there is no real terminal under
    /// `TestBackend`).
    fn set_caret_shape(&mut self, _shape: EditorCursorShape) {}

    /// Draw a [`MessageList`] (chat-style streaming row history).
    /// The backend pulls panel background from its current theme;
    /// hosts that want a custom panel bg compose the primitive
    /// directly via the backend crate's free function.
    fn draw_message_list(&mut self, rect: Rect, list: &MessageList);

    /// Draw a [`RichTextPopup`] at its caller-resolved layout.
    /// Mirrors [`draw_tooltip`](Self::draw_tooltip): host computes
    /// anchor + viewport + measure and asks `popup.layout(...)` for
    /// the bounds. Link hit regions are tracked on the backend's
    /// internal state; hosts that need them query via the
    /// backend-specific accessor today (link-hit-test trait method
    /// is a follow-up).
    fn draw_rich_text_popup(&mut self, popup: &RichTextPopup, layout: &RichTextPopupLayout);

    /// Draw a [`FindReplacePanel`] (find/replace overlay sitting
    /// above the editor). The backend pulls the editor-relative
    /// origin from `rect.x` (TUI's `editor_left` parameter is
    /// derived from `rect`); hosts that want a non-default offset
    /// compose the panel into a sub-rect.
    fn draw_find_replace(&mut self, rect: Rect, panel: &FindReplacePanel);

    /// Draw a [`Completions`] popup at its caller-resolved layout.
    /// Mirrors [`draw_tooltip`](Self::draw_tooltip): host computes
    /// anchor + viewport + measure and asks `completions.layout(...)`
    /// for the bounds.
    fn draw_completions(&mut self, completions: &Completions, layout: &CompletionsLayout);

    /// Draw a [`Scrollbar`] (standalone primitive, vs the
    /// per-section scrollbars MSV paints internally). The backend
    /// pulls cell/pixel background from its current theme.
    fn draw_scrollbar(&mut self, rect: Rect, scrollbar: &Scrollbar);

    /// Draw a [`DropOverlay`] on top of the current frame during a
    /// tab drag. Renders the highlight rect (tinted background) and/or
    /// insertion bar (thin line at the drop position).
    fn draw_drop_overlay(&mut self, overlay: &DropOverlay);

    /// Draw a [`MenuBar`]. The backend computes the layout internally
    /// with native metrics (cells for TUI, Pango pixels for GTK) and
    /// returns the [`MenuBarLayout`] so hosts can route clicks via
    /// `layout.hit_test(x, y)` without re-deriving metrics. Same
    /// coordinate frame as [`Self::menu_bar_layout`] (ABSOLUTE).
    fn draw_menu_bar(&mut self, rect: Rect, bar: &MenuBar) -> MenuBarLayout;

    /// Compute the menu-bar layout the rasteriser would produce for
    /// `bar` in `rect`, using the backend's native metrics. Hosts
    /// call this in click handlers to resolve hits against the same
    /// layout that was painted — never re-derive with a hand-rolled
    /// measurer.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505).
    fn menu_bar_layout(&self, rect: Rect, bar: &MenuBar) -> MenuBarLayout;

    /// Draw a [`Split`] divider. The backend computes the layout with
    /// its native divider thickness (1 cell for TUI, ~4px for GTK)
    /// and returns the [`SplitLayout`] so hosts can route clicks and
    /// drive drag operations. Pane content is NOT drawn — hosts paint
    /// into `layout.first_bounds` / `layout.second_bounds`. Same
    /// coordinate frame as [`Self::split_layout`] (ABSOLUTE).
    fn draw_split(&mut self, rect: Rect, split: &Split) -> SplitLayout;

    /// Compute the split layout without painting. Hosts call this in
    /// drag handlers to recompute the ratio from cursor position.
    ///
    /// Coordinate frame: **ABSOLUTE** — `first_bounds` / `divider_bounds`
    /// / `second_bounds` are shifted by `rect.x` / `rect.y` (issue #505).
    fn split_layout(&self, rect: Rect, split: &Split) -> SplitLayout;

    /// Draw a [`SplitTree`]'s dividers. The backend computes the
    /// layout with its native divider thickness (1 cell for TUI, ~4px
    /// for GTK) and returns the [`SplitTreeLayout`] so hosts can route
    /// clicks (via [`SplitTreeLayout::hit_test_divider`] /
    /// [`SplitTreeLayout::hit_test_divider_cell`] /
    /// [`SplitTreeLayout::hit_test_leaf`]) and drive drag operations
    /// via [`crate::DragTarget::SplitDivider`]. Leaf content is NOT
    /// drawn — hosts paint into each `layout.leaves[i].1` rect. Same
    /// coordinate frame as [`Self::split_tree_layout`] (ABSOLUTE).
    fn draw_split_tree(&mut self, rect: Rect, tree: &SplitTree) -> SplitTreeLayout;

    /// Compute the split-tree layout without painting. Hosts call this
    /// in drag handlers to recompute a divider's ratio from cursor
    /// position without re-painting.
    ///
    /// Coordinate frame: **ABSOLUTE** — leaf / divider bounds are
    /// shifted by `rect.x` / `rect.y` (issue #505).
    fn split_tree_layout(&self, rect: Rect, tree: &SplitTree) -> SplitTreeLayout;

    /// Draw a [`Panel`] chrome (title bar + action buttons). The
    /// backend computes the layout with its native title-bar height
    /// (1 cell for TUI, line_height for GTK) and returns the
    /// [`PanelLayout`] so hosts can route clicks to actions, title
    /// bar, or content. Content is NOT drawn — hosts paint into
    /// `layout.content_bounds`. Same coordinate frame as
    /// [`Self::panel_layout`] (ABSOLUTE).
    fn draw_panel(&mut self, rect: Rect, panel: &Panel) -> PanelLayout;

    /// Compute the panel layout without painting. Hosts call this in
    /// click handlers to resolve hits without re-deriving metrics.
    ///
    /// Coordinate frame: **ABSOLUTE** — `title_bar_bounds` / action /
    /// `content_bounds` are shifted by `rect.x` / `rect.y` (issue #505).
    fn panel_layout(&self, rect: Rect, panel: &Panel) -> PanelLayout;

    /// Draw a [`ToastStack`] overlay. The backend computes the layout
    /// with its native toast dimensions (cell-width boxes for TUI,
    /// pixel boxes for GTK) and returns the [`ToastStackLayout`] so
    /// hosts can route clicks to dismiss, action, or body. Same
    /// coordinate frame as [`Self::toast_stack_layout`] (ABSOLUTE).
    fn draw_toast_stack(&mut self, rect: Rect, stack: &ToastStack) -> ToastStackLayout;

    /// Compute the toast-stack layout without painting. Hosts call
    /// this in click handlers to resolve hits.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505).
    fn toast_stack_layout(&self, rect: Rect, stack: &ToastStack) -> ToastStackLayout;

    /// Draw a [`PipelineView`] (horizontal multi-stage workflow widget).
    /// The backend paints stage boxes, status icons, labels, optional
    /// action buttons, and arrow connectors. Returns the
    /// [`PipelineViewLayout`] so hosts can route clicks via
    /// `layout.hit_test(x, y)` without re-deriving metrics. Same
    /// coordinate frame as [`Self::pipeline_view_layout`] (ABSOLUTE).
    fn draw_pipeline_view(&mut self, rect: Rect, view: &PipelineView) -> PipelineViewLayout;

    /// Compute pipeline-view layout without painting. Hosts call this in
    /// click handlers to resolve hits against the same layout that was
    /// painted — never re-derive with a hand-rolled measurer.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505).
    fn pipeline_view_layout(&self, rect: Rect, view: &PipelineView) -> PipelineViewLayout;

    /// Draw a [`DiffView`] (two-pane side-by-side or unified diff viewer).
    /// Hunks are app-computed via [`crate::diff::compute_hunks`]; the backend
    /// only rasterises. Returns [`DiffViewLayout`] for scroll clamping.
    fn draw_diff_view(&mut self, rect: Rect, view: &DiffView) -> DiffViewLayout;

    /// Compute the diff-view layout without painting — the no-paint twin
    /// of [`Self::draw_diff_view`] (issue #506). `visible_rows` uses this
    /// backend's [`Self::line_height`] exactly as every backend's
    /// `draw_diff_view` does today: in [`DiffMode::SideBySide`] one
    /// `line_height` band is reserved for the header row when either
    /// label is set (`view.left_label` / `view.right_label`); in
    /// [`DiffMode::Unified`] every row — including each hunk's `@@ … @@`
    /// header — scrolls as content, so no band is reserved.
    /// `total_rows` matches [`DiffViewLayout::total_rows`]'s documented
    /// contract (`view.total_rows()` in side-by-side mode, `+
    /// hunk_count` in unified mode for the synthesized header lines).
    ///
    /// Frame: no coordinates are returned — `visible_rows` /
    /// `total_rows` are counts, not positions — so there is no LOCAL vs
    /// ABSOLUTE distinction to state (issue #505).
    ///
    /// Default body: uniform for every backend, since it's a pure
    /// function of `rect`, `view`, and `Self::line_height` — no backend
    /// needs to override this.
    fn diff_view_layout(&self, rect: Rect, view: &DiffView) -> DiffViewLayout {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return DiffViewLayout {
                visible_rows: 0,
                total_rows: view.total_rows(),
            };
        }
        let lh = self.line_height();
        match view.mode {
            DiffMode::SideBySide => {
                let has_header = view.left_label.is_some() || view.right_label.is_some();
                let header_h = if has_header { lh } else { 0.0 };
                let content_h = (rect.height - header_h).max(0.0);
                let visible_rows = if lh > 0.0 {
                    (content_h / lh).floor() as usize
                } else {
                    0
                };
                DiffViewLayout {
                    visible_rows,
                    total_rows: view.total_rows(),
                }
            }
            DiffMode::Unified => {
                let visible_rows = if lh > 0.0 {
                    (rect.height / lh).floor() as usize
                } else {
                    0
                };
                let total_rows: usize = view.hunks.iter().map(|h| h.rows.len() + 1).sum();
                DiffViewLayout {
                    visible_rows,
                    total_rows,
                }
            }
        }
    }

    /// Draw a [`ProgressBar`]. The backend paints the track, fill,
    /// optional label, and optional cancel affordance. Returns the
    /// [`ProgressBarLayout`] so hosts can route clicks. Same
    /// coordinate frame as [`Self::progress_layout`] (ABSOLUTE).
    fn draw_progress(&mut self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout;

    /// Compute progress-bar layout without painting.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505).
    fn progress_layout(&self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout;

    /// Draw a [`Spinner`] (indeterminate activity indicator). Returns
    /// the [`SpinnerLayout`] for host hit-testing. Same coordinate
    /// frame as [`Self::spinner_layout`] (ABSOLUTE).
    fn draw_spinner(&mut self, rect: Rect, spinner: &Spinner) -> SpinnerLayout;

    /// Compute spinner layout without painting.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505).
    fn spinner_layout(&self, rect: Rect, spinner: &Spinner) -> SpinnerLayout;

    /// Draw a [`CommandCenter`] (nav arrows + search box). Returns the
    /// [`CommandCenterLayout`] so hosts can route clicks. Same
    /// coordinate frame as [`Self::command_center_layout`] (ABSOLUTE).
    fn draw_command_center(&mut self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout;

    /// Compute command-center layout without painting.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505; regression-tested against `LESSONS.md`'s "same
    /// frame across backends" rule by `mac_command_center_layout`'s
    /// non-zero-origin test).
    fn command_center_layout(&self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout;

    /// Draw a [`Toolbar`] (horizontal strip of action buttons above a
    /// content area — distinct from `StatusBar` which is read-only),
    /// reading hover/pressed state from a single [`InteractionState`]
    /// keyed by [`WidgetId`] (issue #819).
    ///
    /// The rasteriser tints the background of the button whose id
    /// matches `interaction.hovered()` / `interaction.pressed()`.
    /// Returns the [`ToolbarLayout`] so hosts can route clicks via
    /// `layout.hit_test(x, y)` without re-deriving metrics. Same
    /// coordinate frame as [`Self::toolbar_layout`] (ABSOLUTE).
    ///
    /// This is the implemented method; the positional
    /// [`Self::draw_toolbar`] is a deprecated shim over it.
    fn draw_toolbar_interactive(
        &mut self,
        rect: Rect,
        bar: &Toolbar,
        interaction: &InteractionState,
    ) -> ToolbarLayout;

    /// Draw a [`Toolbar`] with hover/pressed supplied positionally.
    ///
    /// # Deprecated (issue #819)
    ///
    /// Superseded by [`Self::draw_toolbar_interactive`], which reads
    /// the same two values out of one [`InteractionState`] keyed by
    /// [`WidgetId`] instead of two positional `Option<&WidgetId>`
    /// slots. Kept as a *working* forwarding shim — not a stub — per
    /// `CLAUDE.md` rule 3's two-PR deprecate-then-remove protocol: both
    /// `coord-tui` and `vimcode` call this method directly today (see
    /// the PR's *Downstream impact* section), so removing it outright
    /// would break their builds on the next `develop` pull.
    #[deprecated(
        since = "0.0.1",
        note = "use `draw_toolbar_interactive` (hover/pressed come from an `InteractionState` keyed by `WidgetId`) — issue #819"
    )]
    fn draw_toolbar(
        &mut self,
        rect: Rect,
        bar: &Toolbar,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> ToolbarLayout {
        let interaction = InteractionState::from_parts(hovered_id.cloned(), pressed_id.cloned());
        self.draw_toolbar_interactive(rect, bar, &interaction)
    }

    /// Compute toolbar layout without painting. Hosts call this after
    /// `ScreenLayout::draw()` to recover hit regions for click dispatch.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505).
    fn toolbar_layout(&self, rect: Rect, bar: &Toolbar) -> ToolbarLayout;

    /// Draw a [`SidebarPanel`] — optional header toolbar + content
    /// region. Backends paint the toolbar slot only; the content rect
    /// is returned in `SidebarPanelLayout.content_bounds` for the
    /// host to paint into (tree / list / form / etc). Mirrors the
    /// `Panel` rasteriser contract.
    ///
    /// `interaction`'s hovered / pressed [`WidgetId`]s are forwarded to
    /// the nested toolbar paint for hover / pressed tints (issue #819).
    /// Same coordinate frame as [`Self::sidebar_panel_layout`]
    /// (ABSOLUTE).
    ///
    /// This is the implemented method; the positional
    /// [`Self::draw_sidebar_panel`] is a deprecated shim over it.
    fn draw_sidebar_panel_interactive(
        &mut self,
        rect: Rect,
        panel: &SidebarPanel,
        interaction: &InteractionState,
    ) -> SidebarPanelLayout;

    /// Draw a [`SidebarPanel`] with toolbar hover/pressed supplied
    /// positionally.
    ///
    /// # Deprecated (issue #819)
    ///
    /// Superseded by [`Self::draw_sidebar_panel_interactive`], which
    /// reads the same two values out of one [`InteractionState`] keyed
    /// by [`WidgetId`]. Kept as a *working* forwarding shim — not a
    /// stub — per `CLAUDE.md` rule 3's two-PR deprecate-then-remove
    /// protocol: both `coord-tui` and `vimcode` call this method
    /// directly today (see the PR's *Downstream impact* section).
    #[deprecated(
        since = "0.0.1",
        note = "use `draw_sidebar_panel_interactive` (hover/pressed come from an `InteractionState` keyed by `WidgetId`) — issue #819"
    )]
    fn draw_sidebar_panel(
        &mut self,
        rect: Rect,
        panel: &SidebarPanel,
        hovered_toolbar_id: Option<&WidgetId>,
        pressed_toolbar_id: Option<&WidgetId>,
    ) -> SidebarPanelLayout {
        let interaction =
            InteractionState::from_parts(hovered_toolbar_id.cloned(), pressed_toolbar_id.cloned());
        self.draw_sidebar_panel_interactive(rect, panel, &interaction)
    }

    /// Compute sidebar-panel layout without painting. Hosts call this
    /// in click handlers to resolve hits to the toolbar / content /
    /// outside without re-deriving metrics.
    ///
    /// Coordinate frame: **ABSOLUTE** — `content_bounds` / toolbar
    /// bounds are shifted by `rect.x` / `rect.y` (issue #505).
    fn sidebar_panel_layout(&self, rect: Rect, panel: &SidebarPanel) -> SidebarPanelLayout;

    /// Draw a [`Chart`] (sparkline, line, or bar). `hovered_point`
    /// carries per-frame hover state (series_idx, data_idx) so the
    /// rasteriser can highlight the data point under the cursor.
    /// Returns the [`ChartLayout`] so hosts can route clicks and
    /// resolve nearest-point from mouse position. Same coordinate
    /// frame as [`Self::chart_layout`] (ABSOLUTE).
    fn draw_chart(
        &mut self,
        rect: Rect,
        chart: &Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> ChartLayout;

    /// Compute chart layout without painting.
    ///
    /// Coordinate frame: **ABSOLUTE** — `bounds` / `hit_regions` /
    /// `data_point_positions` are shifted by `rect.x` / `rect.y`
    /// (issue #505).
    fn chart_layout(&self, rect: Rect, chart: &Chart) -> ChartLayout;

    /// Draw a [`BoardModel`] (kanban/pipeline board widget).
    ///
    /// The backend paints columns side by side, each with a header title
    /// and a vertical stack of card boxes. Each card shows the issue
    /// title, an inline badge row, and an optional `hint` callout strip.
    /// The selected card is highlighted.
    ///
    /// Returns a [`BoardLayout`] so hosts can route clicks via
    /// `layout.hit_test(x, y)` and implement selection-follow clamping
    /// (DataTable pattern: host reads `layout.columns[i].visible_cards`
    /// and adjusts `column.scroll_offset` accordingly).
    ///
    /// No default impl — every backend implementer sees this as a compile
    /// error and fills in a real rasteriser (`PRIMITIVE_RULES.md` rule 7).
    /// A no-op default here would let a backend silently paint an empty
    /// board instead of failing to build (quadraui#600, PORT-01).
    fn draw_board(&mut self, rect: Rect, model: &BoardModel) -> BoardLayout;

    /// Compute the board layout without painting — the no-paint twin of
    /// [`Self::draw_board`] (issue #506: [`crate::primitives::board::board_layout`]
    /// already existed as a free fn, and every backend already wrapped it
    /// in its own off-trait helper — `tui_board_layout` / `gtk_board_layout`
    /// / `mac_board_layout` — but nothing put it on the trait, so a host
    /// could not ask for board geometry without a live paint pass). Each
    /// backend routes through the exact same helper `draw_board` calls
    /// internally, using its own [`crate::primitives::board::BoardMeasure`]
    /// (column/card sizing is backend-native, like `TreeStyle::row_height`
    /// — not derivable from [`Self::char_width`] / [`Self::line_height`]
    /// alone), so paint and no-paint can't drift apart.
    ///
    /// Coordinate frame: **ABSOLUTE** — `columns[i].bounds` / card bounds
    /// are shifted by `rect.x` / `rect.y`, matching [`BoardLayout::hit_test`]
    /// (issue #505).
    ///
    /// No default impl, same rule-7 reasoning as [`Self::draw_board`]: a
    /// backend that forgets to override this would otherwise silently
    /// report an empty board's worth of hit regions.
    fn board_layout(&self, rect: Rect, model: &BoardModel) -> BoardLayout;

    /// Draw a [`Minimap`] (code-overview density view). GTK and Win-GUI
    /// both tile rows at a fixed pitch and paint one colour block per
    /// non-blank character column ([`crate::MinimapSizing::FixedPitch`],
    /// #667, #738); TUI packs `U+2800`-block braille dots, also at a fixed
    /// pitch — one cell row per minimap row (`FixedPitch(1.0)`, #992,
    /// which fixed a `Fill`-sizing bug where a short file's stretched
    /// pitch left blank cell rows between painted ones). All three
    /// techniques consume the exact same [`Minimap`] data — the primitive
    /// owns the sampling and colour-aggregation math (`sample_blocks` /
    /// `aggregate_spans`), and, since #738, the legibility/render-mode
    /// threshold and span-lookup helpers too (`crate::primitives::minimap`),
    /// so no backend re-derives any of it (#382, #667, #738).
    ///
    /// Returns [`MinimapPaintResult`] carrying the resolved
    /// [`MinimapLayout`] so hosts can route clicks via
    /// `result.layout.hit_test(x, y)` without re-deriving geometry.
    /// Same coordinate frame as [`Self::minimap_layout`] (ABSOLUTE).
    ///
    /// No default impl — every backend implementer sees this as a
    /// compile error and fills in a real rasteriser (`PRIMITIVE_RULES.md`
    /// rule 7). #382 scoped macOS's Core Graphics/Core Text paint calls
    /// for this out for a time, and #802 covered the resulting gap with
    /// an honest `painted: false` no-op rather than a reachable
    /// `todo!()` — #961 closed it: `MacBackend::draw_minimap` now paints
    /// real pixels through `crate::macos::minimap::draw_minimap` and
    /// reports [`MinimapPaintResult::painted`] `true`, same as
    /// TUI/GTK/Win-GUI.
    fn draw_minimap(&mut self, rect: Rect, minimap: &Minimap) -> MinimapPaintResult;

    /// Compute [`Minimap`] layout without painting — mirrors
    /// [`Backend::chart_layout`] / [`Backend::tree_layout`]. Apps call
    /// this from `AppLogic::handle` (which only has `&mut self`, not the
    /// `MinimapPaintResult` `render` produced) to hit-test a click
    /// against the same geometry the last paint used.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505). Real on every backend, including macOS, which
    /// computes this exact geometry whether or not a paint has run yet
    /// (#802, #961).
    fn minimap_layout(&self, rect: Rect, minimap: &Minimap) -> MinimapLayout;

    /// Paint `image` within `rect`, honoring `image.fit` (see
    /// [`Image::layout`] for the geometry). GTK decodes `image.source`
    /// through `gdk_pixbuf` and paints real pixels; Win is scoped out of
    /// this first pass. macOS's natural decoder (`NSImage`) is also not
    /// wired up yet (#662's first pass scoped GTK only) — rather than a
    /// reachable `todo!()` (#802), `MacBackend::draw_image` reports
    /// [`ImagePaintResult::Unsupported`] and paints nothing, the same
    /// signal TUI's categorical case already used below. This method has
    /// no default (rule 7 below still applies to it), so every backend
    /// implementer still sees a compile error until it picks one of
    /// these two honest outcomes.
    ///
    /// **TUI cannot rasterise an image** — there is no pixel grid to
    /// draw into, and this primitive deliberately does not attempt an
    /// ASCII-art decoder (see `primitives::image` module docs' scope
    /// guard). It paints [`Image::fallback_text`] instead, centered in
    /// `rect`, and reports [`ImagePaintResult::Unsupported`] rather than
    /// a silent no-op — #507's Unsupported-vs-failure question, and this
    /// primitive is a fresh, deliberate instance of it: TUI genuinely
    /// cannot do this, so it says so. A GTK decode failure (bad path,
    /// corrupt bytes) also reports `Unsupported` and paints nothing, and
    /// macOS (no decoder wired up yet, #802) reports `Unsupported`
    /// unconditionally for the same reason — so a host can tell "no
    /// pixels appeared" apart from a successful paint without inspecting
    /// pixels itself.
    ///
    /// No default impl — every backend implementer sees this as a
    /// compile error and fills in a real rasteriser, or an explicit
    /// `todo!()` if the primitive is out of scope for that backend for
    /// now (`PRIMITIVE_RULES.md` rule 7).
    fn draw_image(&mut self, rect: Rect, image: &Image) -> ImagePaintResult;
}

/// Paint-side data returned by [`Backend::draw_minimap`]. See
/// [`EditorPaintResult`] for the sibling pattern this mirrors.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MinimapPaintResult {
    pub layout: MinimapLayout,
    /// Whether this call actually rasterised pixels into the target
    /// rect. `true` on every backend as of #961 (TUI/GTK/Win-GUI, and
    /// now macOS's own Core Graphics/Core Text rasteriser,
    /// `crate::macos::minimap::draw_minimap`) — before that, macOS
    /// reported `false` here (#802: no rasteriser existed yet, #382).
    /// `layout` is always the real geometry
    /// [`Backend::minimap_layout`] would also return (the shared
    /// `Minimap::layout_with_sizing` logic, not a stub), regardless of
    /// `painted`, so a host can always hit-test clicks against it. The
    /// bool-on-a-struct shape (rather than an enum like
    /// [`ImagePaintResult`]) is because this method already returns a
    /// struct carrying a layout; `painted` draws the same
    /// Unsupported-vs-real distinction that enum's `Unsupported` variant
    /// draws for `draw_image` (which still uses that shape — macOS has
    /// no `NSImage` decoder yet, a separate, still-open gap). Defaults
    /// to `false` via `#[derive(Default)]` — the honest "nothing
    /// happened yet" starting point, same reasoning as
    /// [`BackendCaps::empty`].
    pub painted: bool,
}

/// Paint-side result of [`Backend::draw_image`]. Deliberately a plain
/// enum, not a struct carrying a layout like [`MinimapPaintResult`] —
/// [`Image::layout`] is pure geometry with no backend-specific
/// measurement step (unlike `Minimap`'s TUI/GTK sampling density), so
/// callers needing the target rect call that directly instead of
/// threading it through the paint result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePaintResult {
    /// The backend rasterised actual image pixels into the target rect.
    Painted,
    /// The backend could not (or, for TUI/macOS, categorically can not)
    /// rasterise pixels. TUI paints `image.fallback_text` instead; GTK on
    /// a decode failure and macOS unconditionally (#802, no `NSImage`
    /// decoder wired up yet) both paint nothing. See
    /// [`Backend::draw_image`]'s doc comment.
    Unsupported,
}

// ── Shared layout helpers ───────────────────────────────────────────────

/// Shift every x-span in `hits` right by `dx`.
///
/// [`tab_bar_hits_from_layout`] yields **bar-relative** x (the primitive
/// measures from `0.0`), but the [`TabBarHits`] contract is
/// target-surface (absolute) coordinates. Rasterisers and the no-paint
/// `tab_bar_layout` variants both call this with `rect.x` so the two
/// paths return the same space.
///
/// Audited under issue #552: `draw_tab_bar` applied this shift on both
/// TUI and GTK, but `Backend::tab_bar_layout` applied it on neither — so
/// the documented-absolute no-paint path silently returned relative x,
/// off by `rect.x`. That is nonzero for any tab bar right of a sidebar,
/// i.e. the same latent seam as the activity bar's, one primitive over.
///
/// `TabBarHits` is `#[deprecated]` (issue #823) — see its doc for the
/// replacement plan.
#[allow(deprecated)]
pub fn shift_tab_bar_hits(hits: &mut TabBarHits, dx: f64) {
    if dx == 0.0 {
        return;
    }
    for sp in &mut hits.slot_positions {
        // `(0.0, 0.0)` is the sentinel for tabs scrolled out of view —
        // leave it recognisable rather than shifting it to `(dx, dx)`.
        if *sp != (0.0, 0.0) {
            sp.0 += dx;
            sp.1 += dx;
        }
    }
    for cb in hits.close_bounds.iter_mut().flatten() {
        cb.0 += dx;
        cb.1 += dx;
    }
    for rb in &mut hits.right_segment_bounds {
        rb.0 += dx;
        rb.1 += dx;
    }
}

/// Convert a `TabBarLayout` to the legacy `TabBarHits` struct.
///
/// Spans are **bar-relative** on return; callers that owe the
/// [`TabBarHits`] absolute contract must follow up with
/// [`shift_tab_bar_hits`] using `rect.x`.
///
/// # Deprecated (issue #504)
///
/// Renamed to [`tab_bar_hits_from_layout`] — same body, same contract.
/// `TabBarHits`'s `f64` coordinate fields predate the crate's f32-native
/// convention (`Point`/`Rect`), and this converter is the thing that
/// keeps constructing them; every in-repo caller now goes through the
/// new name so this deprecated one has **zero in-repo callers**, per
/// CLAUDE.md's two-PR deprecate-then-remove protocol. Full retirement of
/// `TabBarHits` itself (the `f64` fields, and the six `Backend` trait
/// methods that return it) is a separate, much larger follow-up: `vimcode`
/// holds a real, non-doc-only dependency on `TabBarHits`'s field types
/// (`src/core/engine/mod.rs`, `src/core/engine/terminal_ops.rs`,
/// `src/gtk/mod.rs`), and two of the four backends (`macos::tab_bar`,
/// `win::tab_bar`) construct `TabBarHits` directly without ever computing
/// an intermediate `TabBarLayout`, so a safe migration needs new native
/// per-backend rasterisers, not just a signature change.
#[deprecated(since = "0.0.1", note = "renamed to `tab_bar_hits_from_layout`")]
#[allow(deprecated)] // `TabBarHits` is also `#[deprecated]` (issue #823)
pub fn tab_bar_layout_to_hits(layout: &TabBarLayout, bar: &TabBar) -> TabBarHits {
    tab_bar_hits_from_layout(layout, bar)
}

/// Convert a `TabBarLayout` to the legacy `TabBarHits` struct.
///
/// Spans are **bar-relative** on return; callers that owe the
/// [`TabBarHits`] absolute contract must follow up with
/// [`shift_tab_bar_hits`] using `rect.x`. See
/// [`tab_bar_layout_to_hits`]'s doc for why `TabBarHits` itself — not
/// just this converter's name — is still legacy (issue #504).
///
/// `TabBarHits` is now `#[deprecated]` itself (issue #823); this
/// converter is the one place in-tree still allowed to construct it.
#[allow(deprecated)]
pub fn tab_bar_hits_from_layout(layout: &TabBarLayout, bar: &TabBar) -> TabBarHits {
    let mut slot_positions = vec![(0.0, 0.0); bar.tabs.len()];
    let mut close_bounds = vec![None; bar.tabs.len()];
    let mut right_segment_bounds = Vec::new();

    for vt in &layout.visible_tabs {
        let b = vt.bounds;
        slot_positions[vt.tab_idx] = (b.x as f64, (b.x + b.width) as f64);
        if let Some(cb) = vt.close_bounds {
            close_bounds[vt.tab_idx] = Some((cb.x as f64, (cb.x + cb.width) as f64));
        }
    }
    for vs in &layout.visible_segments {
        let b = vs.bounds;
        right_segment_bounds.push((b.x as f64, (b.x + b.width) as f64));
    }

    TabBarHits {
        slot_positions,
        close_bounds,
        right_segment_bounds,
        available_cols: layout.bar_width as usize,
        correct_scroll_offset: layout.resolved_scroll_offset,
    }
}

/// Compute activity bar hit regions from geometry (no paint).
///
/// Spans are **relative to `rect`** per the [`Backend::draw_activity_bar`]
/// contract: the first top-pinned row starts at `0.0` and only `rect.height`
/// is consulted (to pin the bottom group), never `rect.y`. Issue #552.
pub fn activity_bar_hits(rect: Rect, bar: &ActivityBar, lh: f32) -> Vec<ActivityBarRowHit> {
    let mut hits = Vec::new();
    let mut y = 0.0_f32;
    for item in &bar.top_items {
        hits.push(ActivityBarRowHit {
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
            y_start: y,
            y_end: y + lh,
        });
        y += lh;
    }
    let bottom_start = rect.height - bar.bottom_items.len() as f32 * lh;
    let mut by = bottom_start.max(y);
    for item in &bar.bottom_items {
        hits.push(ActivityBarRowHit {
            id: item.id.clone(),
            tooltip: item.tooltip.clone(),
            y_start: by,
            y_end: by + lh,
        });
        by += lh;
    }
    hits
}

/// Paint-side data returned by [`Backend::draw_editor`]. Carries
/// information the host needs to align external chrome (caret blink
/// overlay, virtual-text positioning) with the editor's painted
/// content. Backends that paint their own caret (GTK) populate the
/// default; backends that delegate caret rendering to the host (TUI
/// terminal cursor) populate the actual cursor cell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditorPaintResult {
    /// Terminal-cell `(x, y)` cursor position, if the host is responsible
    /// for terminal-cursor positioning. `None` when the backend painted
    /// its own caret OR when the cursor is outside the viewport.
    ///
    /// # Deprecated (issue #504)
    ///
    /// This field leaked a TUI-only representation — `ratatui::Frame::
    /// set_cursor_position` wants a `(u16, u16)` cell pair — into a
    /// portable trait return type every other backend had to fake with
    /// `None`. [`Self::cursor_position_native`] replaces it with a
    /// [`Point`] in each backend's own native unit (TUI still rounds to
    /// the nearest cell internally before widening).
    ///
    /// Kept alive, and still populated by the TUI backend, because
    /// `vimcode` (`src/tui_main/render_impl.rs`) reads this field
    /// directly and passes it straight into `Frame::set_cursor_position`,
    /// which only has `impl From<(u16, u16)> for Position` — there is no
    /// `From<Point>` quadraui could add (orphan rule). Per CLAUDE.md's
    /// two-PR deprecate-then-remove protocol, removing this field is a
    /// separate follow-up PR, gated on that vimcode call site migrating
    /// to `cursor_position_native` first.
    #[deprecated(
        since = "0.0.1",
        note = "use `cursor_position_native` (`Point`, native units) instead; kept populated by the TUI backend until vimcode's `Frame::set_cursor_position(result.cursor_position)` call site migrates (issue #504)"
    )]
    pub cursor_position: Option<(u16, u16)>,

    /// Cursor's painted position in backend-native units (issue #504 —
    /// the forward-looking replacement for the deprecated
    /// [`Self::cursor_position`]; every backend but TUI just wants a
    /// [`Point`] in its own native unit), if the host is responsible for
    /// terminal-cursor positioning. `None` when the backend painted its
    /// own caret OR when the cursor is outside the viewport. TUI rounds
    /// to the nearest cell internally before returning, then widens the
    /// cell coordinates back into this field's `f32` unit.
    pub cursor_position_native: Option<Point>,
}

/// Trait for content that can render itself into a rect using any backend.
///
/// Implement this on your tab-content types and place them in
/// [`crate::compose::bottom_panel::BottomPanelTab::content`] to supply
/// the body that renders inside the panel when the tab is active.
///
/// # Rules
///
/// - Implementations **must not** borrow app state — each render is a
///   snapshot draw, same rule as every other `draw_*` primitive.
/// - Implementations **must be `Send + 'static`** so they can be stored
///   inside `ShellAdapter` which the runner moves into a thread.
///
/// # Example
///
/// ```ignore
/// struct LogPanel { lines: Vec<String> }
///
/// impl BackendWidget for LogPanel {
///     fn render(&self, backend: &mut dyn Backend, rect: Rect) {
///         let td = TextDisplay { id: "log".into(), lines: self.lines.iter()
///             .map(|l| TextDisplayLine { spans: vec![StyledSpan::plain(l)], .. })
///             .collect(), .. };
///         backend.draw_text_display(rect, &td);
///     }
/// }
/// ```
pub trait BackendWidget: Send + 'static {
    /// Render this widget's content into `rect` using `backend`.
    ///
    /// Called once per frame when this tab is the active tab in a
    /// [`crate::compose::bottom_panel::BottomPanelConfig`].
    fn render(&self, backend: &mut dyn Backend, rect: Rect);
}

/// Platform services the backend exposes to apps: clipboard, file
/// dialogs, message/alert dialogs, notifications, URL opening, and the
/// OS credential store ([`Self::secret_store`]).
pub trait PlatformServices {
    fn clipboard(&self) -> &dyn Clipboard;

    /// Show a native file-open dialog (blocking). Returns `None` if the
    /// user cancelled. TUI backends have no native dialog to show and
    /// unconditionally return `None` (no stderr hint is written); apps
    /// should provide an in-TUI picker instead.
    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

    /// Show a native file-save dialog.
    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

    /// Show a native directory-select dialog (blocking). Returns `None`
    /// if the user cancelled. quadraui#935: before this existed,
    /// `PlatformServices` could pick a *file* (open or save) but not a
    /// *directory* — a host that wanted "Open Folder" native had no
    /// choice but to give up and build its own in-canvas picker (see
    /// vimcode#815, which did exactly that and left a comment on the
    /// tradeoff pointing at this gap).
    ///
    /// Reuses [`FileDialogOptions`] rather than a narrower type: only
    /// [`FileDialogOptions::title`] and [`FileDialogOptions::initial_dir`]
    /// apply to a directory chooser. [`FileDialogOptions::filters`] and
    /// [`FileDialogOptions::initial_filename`] are meaningless for a
    /// directory (no extension, no "file name" field to seed) and every
    /// implementation of this method ignores them — callers should just
    /// leave them at their `Default` rather than populating them.
    ///
    /// TUI backends have no native dialog to show and unconditionally
    /// return `None` (no stderr hint is written), same as
    /// [`Self::show_file_open_dialog`] — apps should provide an in-canvas
    /// picker instead. Callers distinguish "no native chooser at all"
    /// from "the user cancelled" via [`BackendCaps::folder_dialogs`], the
    /// same way they already do for [`BackendCaps::file_dialogs`].
    fn show_folder_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

    /// Show a native message/alert dialog (blocking). Returns the id of
    /// the button the user chose, or `None` if the dialog was dismissed
    /// without choosing one (Escape, close box) **or** this backend has
    /// no native alert facility at all — mirrors
    /// [`Self::show_file_open_dialog`]'s `None` shape exactly. Callers
    /// separate the two via [`BackendCaps::native_dialogs`], the same
    /// way they already do for [`BackendCaps::file_dialogs`].
    ///
    /// This is a parallel path alongside the in-canvas [`Dialog`]
    /// primitive (`Backend::draw_dialog`), not a replacement for it:
    /// [`crate::primitives::dialog::native_dialog_options`] decides,
    /// per-dialog, whether a given [`Dialog`] can go native at all —
    /// callers should consult that before calling this method, and fall
    /// back to `draw_dialog` when it returns `None`. TUI backends have
    /// no native dialog to show and unconditionally return `None` (no
    /// stderr hint is written); the in-canvas `Dialog` primitive stays
    /// the TUI path.
    fn show_message_dialog(&self, opts: MessageDialogOptions) -> Option<MessageDialogChoice>;

    /// Dispatch a system notification (issue #955 extended the request
    /// shape — see [`Notification`]'s doc for `icon`/`actions`/`silent`/
    /// `tag`).
    ///
    /// A click on the notification (its body, or one of `n`'s declared
    /// actions) should push [`UiEvent::NotificationActivated`] where the
    /// backend has a native channel for it — today only GTK does (see
    /// `gtk::services`'s module doc); macOS's unbundled `osascript`
    /// fallback and Win's `Shell_NotifyIconW` balloon fallback have no
    /// click-through of their own to report through, and honestly do not
    /// emit the event rather than faking it (each backend's
    /// `send_notification` doc explains why, and what a real
    /// implementation there would need).
    fn send_notification(&self, n: Notification);

    /// Open a URL in the platform's default browser.
    fn open_url(&self, url: &str);

    /// Fallible twin of [`Self::open_url`] (issue #949, D-009 seam-2's
    /// `_result`-twin pattern — the same shape
    /// [`Clipboard::write_text_result`] already carries): reports
    /// [`BackendError::Unsupported`] on a backend with no way to open a
    /// URL at all, instead of `open_url`'s silent no-op. `open_url`
    /// itself keeps its infallible signature unchanged — this is rule 2's
    /// "new function alongside the old one" from
    /// `docs/PRIMITIVE_RULES.md`, chosen so both existing consumers (which
    /// only ever call `open_url`) keep compiling untouched.
    ///
    /// Default: calls `open_url` and returns `Ok(())` — a backend whose
    /// `open_url` is a real implementation (GTK via
    /// `gio::AppInfo::launch_default_for_uri`, Win-GUI via
    /// `ShellExecuteW`, macOS via `open`) answers exactly like this method
    /// doesn't exist, at zero cost. `TuiPlatformServices` is the one
    /// override, for two reasons across two issues: quadraui#949 made the
    /// old empty no-op `open_url` body *detectable* (a caller had no way
    /// to tell "the browser opened" from "TUI silently discarded this");
    /// quadraui#969 then made it *functional* — TUI shells out to the
    /// platform's URL opener (`xdg-open`/`open`/`cmd /c start`) with an
    /// OSC 8 hyperlink fallback, and only reports
    /// `Err(BackendError::Unsupported)` when neither reaches anything
    /// (see `tui::services`'s module doc, "URL opening (issue #969)").
    fn open_url_result(&self, url: &str) -> ServiceResult<()> {
        self.open_url(url);
        Ok(())
    }

    /// Reveal `path` in the platform's file manager with it selected —
    /// "Reveal in Finder" / "Show in Explorer" / "Show in Files" (issue
    /// #956, `ELECTRON_PARITY_AUDIT.md` §1.2 G8: Electron's
    /// `shell.showItemInFolder`).
    ///
    /// Default: `Err(BackendError::Unsupported)`, the same "future
    /// backend compiles before it has an opinion" placeholder
    /// [`Self::system_theme`]'s default doc explains. GTK, macOS, and
    /// Win-GUI each override this with a real implementation; TUI keeps
    /// the default — a terminal has no file manager window to reveal
    /// anything in, so `Unsupported` there is the honest final answer,
    /// not a placeholder (see `tui::services`'s module doc).
    fn reveal_in_file_manager(&self, path: &Path) -> ServiceResult<()> {
        let _ = path;
        Err(BackendError::Unsupported)
    }

    /// Open `path` with the platform's default handler for its type —
    /// Electron's `shell.openPath`. Distinct from [`Self::open_url`]:
    /// that method takes a URL string and always goes through the
    /// scheme's registered handler (a browser for `https://`, a mail
    /// client for `mailto:`); this one takes a filesystem path directly.
    ///
    /// Default: `Err(BackendError::Unsupported)`, same placeholder
    /// reasoning as [`Self::reveal_in_file_manager`]. GTK, macOS, and
    /// Win-GUI override with a real implementation. TUI *also* overrides
    /// this one — see `tui::services::TuiPlatformServices::open_path`'s
    /// doc for why a terminal can still shell out to `xdg-open`/`open`
    /// even though it has no browser for [`Self::open_url`] to hand a URL
    /// to.
    fn open_path(&self, path: &Path) -> ServiceResult<()> {
        let _ = path;
        Err(BackendError::Unsupported)
    }

    /// Move `path` to the platform trash/recycle bin — recoverable,
    /// unlike deleting it outright — Electron's `shell.trashItem`.
    ///
    /// Default: `Err(BackendError::Unsupported)`, same placeholder
    /// reasoning as [`Self::reveal_in_file_manager`]. Every backend in
    /// this crate overrides it with the *same* implementation
    /// ([`crate::desktop::move_to_trash`]): the cross-platform `trash`
    /// crate already wraps `NSWorkspace`'s Objective-C
    /// `-trashItemAtURL:resultingItemURL:error:`, `gio::File::trash`'s
    /// freedesktop.org trash-spec equivalent, and
    /// `SHFileOperationW(FOF_ALLOWUNDO)` internally, and needs only a
    /// filesystem — no live desktop/window-server session — on any of
    /// the three. So TUI gets the exact same real implementation as the
    /// other three, not a degraded one (see the module doc's "TUI story"
    /// note this issue itself made about this method).
    fn move_to_trash(&self, path: &Path) -> ServiceResult<()> {
        let _ = path;
        Err(BackendError::Unsupported)
    }

    /// Ring the terminal bell / play the system alert sound — Electron's
    /// `shell.beep`.
    ///
    /// Default: `Err(BackendError::Unsupported)`, same placeholder
    /// reasoning as [`Self::reveal_in_file_manager`]. Every backend
    /// overrides it — TUI's BEL is arguably the most honest of the four:
    /// it's a terminal's *only* notification channel, so this is full
    /// support there, not a degrade.
    fn beep(&self) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Query the OS-level light/dark preference, accent colour, and
    /// high-contrast setting (issue #952) — the `nativeTheme` gap named in
    /// `ELECTRON_PARITY_AUDIT.md` §1.2 G4. Before this method existed,
    /// [`Theme`][crate::theme::Theme] was entirely app-supplied: nothing in
    /// this crate could tell an app "the user just flipped their OS to dark
    /// mode", so an app had to either ignore the OS setting or build its own
    /// per-platform detection outside quadraui.
    ///
    /// Returns [`BackendError::Unsupported`] rather than a guessed value on
    /// a backend/session with no way to answer — unlike
    /// [`Self::show_file_open_dialog`]'s ambiguous `None` (which needed a
    /// dedicated `BackendCaps` flag to disambiguate "cancelled" from "no
    /// native facility"), a `Result` already carries that distinction in
    /// its own type, so this method gets no `BackendCaps` field of its own.
    ///
    /// Default: always `Err(BackendError::Unsupported)` — the honest
    /// starting point (rule 2's "new function alongside the old one" isn't
    /// in play here since no downstream consumer implements
    /// `PlatformServices`, per `CLAUDE.md`'s *Downstream consumers* table,
    /// but a default still means a future fifth backend compiles before it
    /// has an opinion). Every backend in this crate overrides it:
    ///
    /// - **TUI** (`TuiPlatformServices`, `tui::caps`) — honest degrade,
    ///   same detect/probe split as [`crate::backend::BackendCaps::kitty_keyboard`]:
    ///   an environment-only heuristic (`COLORFGBG`) parsed by
    ///   `tui::caps::detect_system_theme_from`. No OS accent colour or
    ///   high-contrast signal exists in a terminal, so those fields are
    ///   always `None`/`false` there. Returns `Unsupported` when
    ///   `COLORFGBG` is unset or unparseable — a terminal that never sets
    ///   it genuinely gives no signal, so guessing would be no more honest
    ///   than refusing.
    /// - **GTK** — `gtk4::Settings::gtk-application-prefer-dark-theme` /
    ///   `gtk-theme-name` (no accent-colour API without libadwaita).
    /// - **macOS** — `NSApp.effectiveAppearance` / `NSColor::controlAccentColor`.
    /// - **Win-GUI** — `UISettings::GetColorValue` (`Background`/`Accent`
    ///   `UIColorType`s).
    ///
    /// No backend pushes [`crate::UiEvent::SystemThemeChanged`] on a live OS
    /// theme change yet (same "declare the gap, don't fake it" posture as
    /// [`crate::UiEvent::WindowStateChanged`], which also has zero producers
    /// today) — this method is a poll-on-demand query, not a subscription.
    fn system_theme(&self) -> ServiceResult<SystemTheme> {
        Err(BackendError::Unsupported)
    }

    /// The OS credential store — Keychain (macOS), Secret Service
    /// (Linux), Windows Credential Manager — for apps that hold an API
    /// token, a password, or an OAuth refresh token and would otherwise
    /// have no choice but to invent their own storage or write it to
    /// plaintext config (issue #958, `ELECTRON_PARITY_AUDIT.md` §1.2 G16,
    /// ranked #9b: "value-to-effort ratio is the best on the list").
    ///
    /// Unlike every other capability in that audit, this one is
    /// **backend-independent**: the returned [`SecretStore`] is backed by
    /// the cross-platform `keyring` crate directly, not by a per-backend
    /// native implementation, so it needs no override in
    /// `tui`/`gtk`/`macos`/`win::services` — the same default body serves
    /// all four. `keyring` itself needs no window, no display server, and
    /// no desktop session (its Linux store talks to the Secret Service
    /// over D-Bus, not through any GUI toolkit), so this is **full
    /// support on TUI**, not a degrade — the reason issue #958 prioritised
    /// it. See [`SecretStore`]'s own doc for the get/set/delete shape.
    ///
    /// Default: a `keyring`-backed [`SecretStore`] when this crate was
    /// built with any of the `tui`/`gtk`/`macos`/`win` features (the same
    /// four that already pull in the cross-platform `trash` crate for
    /// [`Self::move_to_trash`] — see that dependency's Cargo.toml comment
    /// for why the list is exactly those four); a `SecretStore` whose
    /// every method returns `Err(BackendError::Unsupported)` when none of
    /// them are enabled (a bare `cargo check -p quadraui` with no
    /// features, which this crate supports — see `lib.rs`'s unconditional
    /// `pub mod backend;`). No in-tree backend overrides this — there is
    /// nothing backend-specific left to override.
    fn secret_store(&self) -> &dyn SecretStore {
        static STORE: KeyringSecretStore = KeyringSecretStore;
        &STORE
    }

    /// Enumerate every connected display — bounds, usable work area, DPI
    /// scale, and which one is primary (issue #959,
    /// `ELECTRON_PARITY_AUDIT.md` §1.2 G10, ranked #10). Before this
    /// existed, quadraui exposed only [`crate::event::Viewport::scale`]
    /// (the *current window's* scale) and
    /// [`crate::event::UiEvent::DpiChanged`] (a live scale-change
    /// notification) — nothing answered "how many monitors are there,
    /// where do they sit relative to each other, and where can a window
    /// usably be placed on each", needed to restore saved window bounds
    /// after a monitor change, place a new window sensibly, or position a
    /// popup near the cursor across displays. See [`Display`]'s own doc
    /// for the field-by-field contract, including the GTK/Wayland
    /// work-area caveat.
    ///
    /// Default: `Err(BackendError::Unsupported)`, the same placeholder
    /// reasoning [`Self::reveal_in_file_manager`]'s doc explains. Every
    /// backend in this crate overrides it:
    ///
    /// - **macOS** — `NSScreen::screens()`, `frame()`/`visibleFrame()` for
    ///   `bounds`/`work_area`, `backingScaleFactor()` for `scale`.
    ///   `screens()[0]` is always the screen containing the menu bar —
    ///   used as `primary` (distinct from `NSScreen::mainScreen()`, which
    ///   tracks the *key window*'s screen, not the platform's primary
    ///   one).
    /// - **GTK** — `gdk::Display::monitors()` + `Monitor::geometry()` /
    ///   `scale_factor()`. No work-area API exists in GDK4 at all
    ///   (removed from GDK3 entirely, not just absent on Wayland) — GTK's
    ///   `work_area` is always a copy of `bounds` on every desktop. No
    ///   primary-monitor flag either; index `0` (enumeration order) is
    ///   used as a best-effort proxy — see
    ///   [`crate::gtk::services::GtkPlatformServices::displays`]'s doc.
    /// - **Win-GUI** — `EnumDisplayMonitors` + `GetMonitorInfoW` for
    ///   `bounds`/`work_area`/`primary` (`rcMonitor`/`rcWork`/
    ///   `MONITORINFOF_PRIMARY`), `GetDpiForMonitor` for `scale`
    ///   (`dpi / USER_DEFAULT_SCREEN_DPI`).
    /// - **TUI** — a truthful degrade, not `Unsupported`: one [`Display`]
    ///   whose `bounds`/`work_area` are both the terminal's cell grid
    ///   (`0, 0, width, height`, the same cell units
    ///   [`Backend::viewport`] reports), `scale: 1.0`, `primary: true` —
    ///   see [`crate::tui::services`]'s module doc.
    fn displays(&self) -> ServiceResult<Vec<Display>> {
        Err(BackendError::Unsupported)
    }

    /// The mouse cursor's current position in screen coordinates — the
    /// same coordinate system [`Self::displays`]'s `bounds`/`work_area`
    /// use, so a caller can directly test which [`Display`] currently
    /// contains the cursor (e.g. positioning a popup near the cursor
    /// across displays; issue #959).
    ///
    /// Default: `Err(BackendError::Unsupported)`, same placeholder
    /// posture as [`Self::displays`]. **GTK and TUI both keep this
    /// default, for real (not placeholder) reasons documented on each
    /// override:** GDK4 removed the global/root-window pointer-position
    /// query entirely (`Surface::device_position` is *surface-relative*
    /// only, and GDK4 surfaces don't expose their own screen origin
    /// either — a real protocol-level Wayland restriction, not a missing
    /// binding); a terminal has no synchronous "where is the mouse right
    /// now" query at all — only `UiEvent::MouseMoved`, delivered when the
    /// terminal's mouse-tracking mode is on. macOS
    /// (`NSEvent::mouseLocation()`) and Win-GUI (`GetCursorPos`) both
    /// override with a real answer.
    fn cursor_screen_point(&self) -> ServiceResult<Point> {
        Err(BackendError::Unsupported)
    }

    /// Platform identifier — matches the `BackendNative.backend` field.
    /// One of `"tui"`, `"gtk"`, `"win-gui"`, `"macos"`.
    fn platform_name(&self) -> &'static str;
}

/// A single named entry in the OS credential store, keyed by `service` +
/// `account` — [`PlatformServices::secret_store`] (issue #958).
///
/// Deliberately small and string-keyed, mirroring the `keyring` crate's
/// own `Entry::new(service, username)` shape it wraps: an app names its
/// own `service` (e.g. `"my-app"`) and one `account` per credential it
/// wants to keep separate (e.g. a username, or a fixed string like
/// `"api-token"` for an app with only one secret). There is no "list all
/// entries" method — none of macOS Keychain, Windows Credential Manager,
/// or Secret Service expose a *portable* enumeration API without extra
/// per-platform querying `keyring` itself doesn't attempt, so this trait
/// doesn't promise one either.
///
/// `get` returns `Ok(None)` for "no entry has ever been set" — the same
/// "nothing here, not a failure" idiom [`Clipboard::read_text`] already
/// uses — reserving `Err` for a real platform failure (the credential
/// store is locked, unreachable, or returned malformed data). `set` and
/// `delete` have no such "nothing to report" case: a write either
/// succeeds or fails, and deleting an entry that was never set is a real,
/// reportable outcome (`Err`), not silently `Ok(())` — the same "surface
/// the native call's actual outcome rather than papering over it" stance
/// [`PlatformServices::move_to_trash`]'s own doc and test already take
/// for a nonexistent path.
pub trait SecretStore {
    /// Read the secret stored for `service` + `account`. `Ok(None)` when
    /// no entry has ever been set (or it was already deleted); `Err` on a
    /// real platform failure.
    fn get(&self, service: &str, account: &str) -> ServiceResult<Option<String>>;

    /// Write `secret` for `service` + `account`, creating the entry if it
    /// doesn't exist or overwriting it if it does.
    fn set(&self, service: &str, account: &str, secret: &str) -> ServiceResult<()>;

    /// Delete the entry for `service` + `account`. `Err` if no such entry
    /// exists — see this trait's own doc for why that's a real outcome
    /// here, not a no-op success.
    fn delete(&self, service: &str, account: &str) -> ServiceResult<()>;
}

/// `SecretStore` backed by the cross-platform `keyring` crate — the one
/// implementation [`PlatformServices::secret_store`]'s default vends on
/// every backend, TUI included (see that method's doc for why no backend
/// needs its own override).
///
/// A zero-sized marker, not a handle: `keyring::Entry` is cheap to build
/// per call (it does no I/O until `get_password`/`set_password`/
/// `delete_credential` is actually invoked) and this crate has no
/// process-wide keyring state of its own to cache — every call opens a
/// fresh `Entry` for exactly the `service`/`account` pair it was asked
/// for.
#[cfg(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win"))]
struct KeyringSecretStore;

#[cfg(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win"))]
impl SecretStore for KeyringSecretStore {
    fn get(&self, service: &str, account: &str) -> ServiceResult<Option<String>> {
        let entry = keyring::Entry::new(service, account).map_err(keyring_error_to_backend)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_error_to_backend(e)),
        }
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> ServiceResult<()> {
        let entry = keyring::Entry::new(service, account).map_err(keyring_error_to_backend)?;
        entry.set_password(secret).map_err(keyring_error_to_backend)
    }

    fn delete(&self, service: &str, account: &str) -> ServiceResult<()> {
        let entry = keyring::Entry::new(service, account).map_err(keyring_error_to_backend)?;
        entry.delete_credential().map_err(keyring_error_to_backend)
    }
}

/// Map a `keyring` crate error onto this crate's own error vocabulary.
/// `keyring::Error` has no equivalent of [`BackendError::Unsupported`] —
/// every variant is a real failure of *some* underlying store operation
/// (see [`keyring::Error`]'s own doc) — so every arm becomes
/// [`BackendError::PlatformFailure`] with the `Display` text as context;
/// [`keyring::Error::NoEntry`] is handled separately by
/// [`KeyringSecretStore::get`] before it would ever reach here.
#[cfg(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win"))]
fn keyring_error_to_backend(e: keyring::Error) -> BackendError {
    BackendError::PlatformFailure {
        context: format!("keyring: {e}"),
    }
}

/// `SecretStore` fallback for a build with none of the
/// `tui`/`gtk`/`macos`/`win` features enabled — the `keyring` crate isn't
/// even a dependency in that configuration (see its Cargo.toml gate), so
/// there is no store to reach; every method honestly reports
/// [`BackendError::Unsupported`] rather than the crate failing to build
/// at all. See [`PlatformServices::secret_store`]'s doc for why a bare,
/// feature-less `cargo check -p quadraui` must still compile this trait's
/// default method.
#[cfg(not(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win")))]
struct KeyringSecretStore;

#[cfg(not(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win")))]
impl SecretStore for KeyringSecretStore {
    fn get(&self, _service: &str, _account: &str) -> ServiceResult<Option<String>> {
        Err(BackendError::Unsupported)
    }

    fn set(&self, _service: &str, _account: &str, _secret: &str) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    fn delete(&self, _service: &str, _account: &str) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }
}

/// One connected display — bounds, usable work area, DPI scale, and
/// primary-monitor flag ([`PlatformServices::displays`], issue #959).
///
/// `bounds` and `work_area` share the backend's native coordinate system —
/// the same "TUI: cells; GTK/macOS/Win: pixels" convention every other
/// [`Rect`] in this crate already uses. Coordinates are relative to the
/// platform's own virtual-desktop origin, not necessarily `(0, 0)` — a
/// monitor to the left of or above the primary sits at negative `x`/`y`
/// (macOS additionally uses AppKit's bottom-left-origin, y-up convention
/// for both fields — the same one [`PlatformServices::cursor_screen_point`]
/// returns coordinates in on that backend, so the two stay directly
/// comparable there).
///
/// Not `Serialize`/`Deserialize`: unlike [`SystemTheme`], nothing carries
/// a `Display` inside a [`crate::UiEvent`] payload —
/// [`crate::UiEvent::DisplaysChanged`] is a bare change notification a
/// caller reacts to by calling [`PlatformServices::displays`] again, not a
/// snapshot delivery.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    /// The monitor's full bounds, including any OS chrome (taskbar / menu
    /// bar + Dock / panels) that overlaps it.
    pub bounds: Rect,
    /// The usable area inside `bounds` — excludes the taskbar (Windows),
    /// menu bar + Dock (macOS), and panels (GTK/X11 desktop
    /// environments that report one). **Not available on GTK** — GDK4
    /// exposes no work-area API at all (removed from GDK3, not just
    /// absent on Wayland specifically; see
    /// [`crate::gtk::services::GtkPlatformServices::displays`]'s doc), so
    /// the GTK backend always reports a copy of `bounds` here rather than
    /// faking a value on some desktops and not others.
    pub work_area: Rect,
    /// This monitor's DPI/backing scale factor — the same units as
    /// [`crate::event::Viewport::scale`] and
    /// [`crate::event::UiEvent::DpiChanged`]'s payload.
    pub scale: f32,
    /// `true` for the platform's primary/main display. Exactly one
    /// [`Display`] in [`PlatformServices::displays`]'s returned `Vec` has
    /// this set on macOS, Win-GUI, and TUI. **GTK is a best-effort
    /// exception** — GDK4 has no native "primary monitor" concept to
    /// report at all, so the GTK backend marks index `0` (enumeration
    /// order) as `primary` rather than reporting `false` on every entry;
    /// see [`crate::gtk::services::GtkPlatformServices::displays`]'s doc.
    pub primary: bool,
}

/// The OS-level theme preference [`PlatformServices::system_theme`]
/// reports — light/dark, accent colour, and high-contrast (issue #952).
///
/// Deliberately flat and small, mirroring [`Notification`]'s shape: this is
/// a snapshot an app reads once per query (or once per
/// [`crate::UiEvent::SystemThemeChanged`], once some backend emits it), not
/// a live-updating handle. `Serialize`/`Deserialize` (unlike the other
/// `PlatformServices` option/config structs in this module) because it also
/// travels inside [`crate::UiEvent::SystemThemeChanged`], and every
/// `UiEvent` payload satisfies that bound — see `event.rs`'s module doc.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SystemTheme {
    /// `true` when the OS is set to a dark appearance (Windows "Dark" app
    /// mode, macOS Dark Appearance, GTK's `prefer-dark-theme`, or — on TUI —
    /// `COLORFGBG`'s background index reads as dark).
    pub dark: bool,
    /// The OS accent colour, when the platform exposes one and the backend
    /// can read it. `None` on backends/platforms with no accent-colour
    /// concept (TUI always; GTK today, absent libadwaita) rather than a
    /// guessed default — callers should fall back to their own accent
    /// colour, not treat `None` as black/transparent.
    pub accent: Option<Color>,
    /// `true` when the OS high-contrast accessibility setting is active
    /// (Windows High Contrast mode, GTK's `HighContrast*` theme names).
    /// Always `false` on TUI — a terminal has no equivalent OS-level
    /// setting to query.
    pub high_contrast: bool,
}

/// Decoded RGBA8 pixel buffer for a clipboard image round-trip
/// ([`Clipboard::read_image`] / [`Clipboard::write_image`], issue #954).
///
/// Deliberately a bare pixel buffer, not [`ImageSource`] — the type
/// [`TrayService::set_icon`] shares with `Backend::draw_image` per that
/// trait's own doc note. `ImageSource::Bytes`/`Path` describe *where an
/// image comes from* (encoded bytes a backend still has to sniff and
/// decode); a clipboard image is already-decoded pixels the moment it
/// comes off the OS clipboard (`CF_DIB` on Windows, an `NSImage` bitmap
/// on macOS, a decoded PNG atom on Linux) — there is nothing left to
/// decode, so a bare pixel buffer is the honest shape here instead of
/// forcing a round trip through an encoded format.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA8 pixels, row-major, top-to-bottom,
    /// left-to-right. Always `width * height * 4` bytes long.
    pub pixels: Vec<u8>,
}

/// One data format present on the clipboard right now — the vocabulary
/// [`Clipboard::formats`] reports over (issue #954).
///
/// No `Html` variant: [`Clipboard::write_html`] has no `read_html` twin
/// (no in-tree caller needs to read HTML back off the clipboard, only
/// write it), so [`Clipboard::formats`]'s generic default — which only
/// probes formats it has a matching `read_*` method to probe with — has
/// nothing to detect HTML presence with. A backend that can tell HTML is
/// on the clipboard may still surface that by overriding `formats`
/// itself; this enum only names what the default can honestly report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardFormat {
    /// Plain text — what [`Clipboard::read_text`] returns `Some` for.
    Text,
    /// A raster image — what [`Clipboard::read_image`] returns `Ok` for.
    Image,
    /// A non-empty file-path list (a Finder/Explorer copy) — what
    /// [`Clipboard::read_file_list`] returns a non-empty `Ok` for.
    FileList,
}

/// Trait object-safe clipboard access.
pub trait Clipboard {
    /// Read the current clipboard contents as plain text. `None` on
    /// empty / non-text clipboard or platform error.
    fn read_text(&self) -> Option<String>;

    /// Write plain text to the clipboard.
    fn write_text(&self, text: &str);

    /// Fallible twin of [`Self::write_text`] (issue #805, D-009 seam 2's
    /// `_result`-twin pattern): same effect, but surfaces a failure a
    /// caller could not previously observe instead of silently
    /// discarding it. `write_text` itself keeps its infallible signature
    /// unchanged — this is rule 2's "new function alongside the old one"
    /// from `docs/PRIMITIVE_RULES.md`, chosen over a breaking signature
    /// change because every existing call site (both consumers, per
    /// `CLAUDE.md`'s downstream-consumer table) only calls `write_text`
    /// and has no use for a failure reason today.
    ///
    /// Default: calls `write_text` and always returns `Ok(())` — a
    /// backend that never overrides this answers exactly like it doesn't
    /// exist, at zero cost to existing implementors (TUI, GTK, macOS
    /// today swallow the underlying `arboard`/Cocoa error the same way
    /// `write_text` always has). `WinBackend`'s `WinClipboard` overrides
    /// this with the real `OpenClipboard`/`GlobalAlloc`/`SetClipboardData`
    /// failure it can now tell apart from success.
    fn write_text_result(&self, text: &str) -> ServiceResult<()> {
        self.write_text(text);
        Ok(())
    }

    /// Read the X11/Wayland **PRIMARY** selection — the platform
    /// convention behind middle-click paste, distinct from
    /// [`read_text`](Self::read_text)'s CLIPBOARD selection (populated by
    /// an explicit copy). Backed by whatever text was last *selected*
    /// (drag-select, double-click), with no separate copy action needed.
    ///
    /// Defaults to `None` — most platforms (Windows, macOS, TUI-over-any-
    /// terminal) have no PRIMARY-selection concept at all, so this is
    /// only meaningfully overridden by the GTK backend on Linux/BSD
    /// (quadraui#415).
    fn read_primary_selection(&self) -> Option<String> {
        None
    }

    /// Read the current clipboard contents as a decoded RGBA image
    /// (issue #954).
    ///
    /// Default: `Err(BackendError::Unsupported)` — kept **defaulted**,
    /// unlike the rest of [`PlatformServices`]'s required methods, so
    /// every existing `impl Clipboard` (TUI, GTK, macOS, Win) keeps
    /// compiling unchanged; but the default is `Err`, not `Ok(None)` /
    /// `None`, so a backend that never overrides this is honest per call
    /// ("I cannot do this") rather than indistinguishable from "the
    /// clipboard happens to be empty right now" the way an `Option`
    /// return would read. TUI never overrides this — OSC 52 (its only
    /// remote-reachable clipboard channel) has no image form, so TUI's
    /// clipboard is text-only by construction, not by omission.
    fn read_image(&self) -> ServiceResult<RgbaImage> {
        Err(BackendError::Unsupported)
    }

    /// Write an RGBA image to the clipboard (issue #954).
    ///
    /// Default: `Err(BackendError::Unsupported)` — see [`Self::read_image`].
    fn write_image(&self, _image: &RgbaImage) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Write HTML to the clipboard, with a plain-text `alt_text`
    /// fallback for a paste target that only understands text — the same
    /// "rich content + text degrade" shape every native clipboard API
    /// exposes this as (`NSPasteboard` HTML + string types, GTK's
    /// `text/html` + `UTF8_STRING` targets, Win32's `CF_HTML` +
    /// `CF_UNICODETEXT`) (issue #954).
    ///
    /// Default: `Err(BackendError::Unsupported)` — see [`Self::read_image`].
    /// There is deliberately no `read_html` twin; see
    /// [`ClipboardFormat`]'s doc for why.
    fn write_html(&self, _html: &str, _alt_text: &str) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }

    /// Read the list of file paths currently on the clipboard (a
    /// Finder/Explorer copy) (issue #954).
    ///
    /// Default: `Err(BackendError::Unsupported)` — see [`Self::read_image`].
    fn read_file_list(&self) -> ServiceResult<Vec<PathBuf>> {
        Err(BackendError::Unsupported)
    }

    /// Which formats the clipboard currently holds (issue #954).
    ///
    /// Default: probes [`Self::read_text`], [`Self::read_image`], and
    /// [`Self::read_file_list`] **through `self`** — so on a backend that
    /// overrides those, this default already reports real data without
    /// needing its own override — and reports [`ClipboardFormat::Text`] /
    /// [`ClipboardFormat::Image`] / [`ClipboardFormat::FileList`] for
    /// whichever succeeded. On a backend that overrides none of the
    /// three (plain TUI), this correctly degrades to "`Text` if there is
    /// any, otherwise nothing". A backend may still override this
    /// directly to avoid paying for three real clipboard round trips just
    /// to answer "what's there", or to report a format its `read_*`
    /// methods can't (there is none today, but a future one might).
    fn formats(&self) -> Vec<ClipboardFormat> {
        let mut out = Vec::new();
        if self.read_text().is_some() {
            out.push(ClipboardFormat::Text);
        }
        if self.read_image().is_ok() {
            out.push(ClipboardFormat::Image);
        }
        if matches!(self.read_file_list(), Ok(list) if !list.is_empty()) {
            out.push(ClipboardFormat::FileList);
        }
        out
    }

    /// Clear the clipboard of all content, regardless of format
    /// (issue #954).
    ///
    /// Default: `Err(BackendError::Unsupported)` — see [`Self::read_image`].
    fn clear(&self) -> ServiceResult<()> {
        Err(BackendError::Unsupported)
    }
}

/// Options for [`PlatformServices::show_file_open_dialog`],
/// [`PlatformServices::show_file_save_dialog`], and
/// [`PlatformServices::show_folder_open_dialog`].
#[derive(Debug, Clone, Default)]
pub struct FileDialogOptions {
    /// Dialog window title.
    pub title: Option<String>,
    /// Suggested starting directory.
    pub initial_dir: Option<PathBuf>,
    /// Suggested file name (save dialog only — ignored by
    /// [`PlatformServices::show_file_open_dialog`] and
    /// [`PlatformServices::show_folder_open_dialog`]).
    pub initial_filename: Option<String>,
    /// File type filters — `(display_name, &[ext])` pairs. Ignored by
    /// [`PlatformServices::show_folder_open_dialog`]: a directory has no
    /// extension to filter on.
    pub filters: Vec<(String, Vec<String>)>,
}

/// Options for [`PlatformServices::show_message_dialog`]. Produced from
/// a [`Dialog`] descriptor by
/// [`crate::primitives::dialog::native_dialog_options`], or built
/// directly by a caller that just wants a native alert with no
/// in-canvas fallback.
#[derive(Debug, Clone)]
pub struct MessageDialogOptions {
    /// Dialog title / primary message text.
    pub title: String,
    /// Body / secondary detail text.
    pub body: String,
    /// Action buttons. Order is caller-declared intent, not necessarily
    /// paint order — [`crate::gtk::services::GtkPlatformServices`], for
    /// instance, re-orders these per GNOME HIG (cancel leftmost, default
    /// rightmost) before handing them to the native widget.
    pub buttons: Vec<MessageDialogButton>,
    /// Optional severity tint — backends may use this to pick an icon.
    /// `None` = neutral. Mirrors [`DialogSeverity`], the in-canvas
    /// `Dialog`'s equivalent field.
    pub severity: Option<DialogSeverity>,
}

/// One button in a [`MessageDialogOptions`] button row. Mirrors
/// [`crate::primitives::dialog::DialogButton`]'s id/label/default/cancel
/// shape (minus `tint`, which no native alert facility exposes).
#[derive(Debug, Clone)]
pub struct MessageDialogButton {
    pub id: WidgetId,
    pub label: String,
    /// When true, Enter (or the platform's default-action gesture)
    /// activates this button.
    pub is_default: bool,
    /// When true, Escape (or the platform's cancel gesture) activates
    /// this button.
    pub is_cancel: bool,
}

/// The [`WidgetId`] of the [`MessageDialogButton`] the user chose from a
/// [`PlatformServices::show_message_dialog`] call. A plain alias rather
/// than a wrapper type: callers already hold the
/// `Vec<MessageDialogButton>` they passed in and match the result
/// against each button's `id` directly.
pub type MessageDialogChoice = WidgetId;

/// A system notification request (issue #955 extends the original
/// title/body/urgent shape with `icon`/`actions`/`silent`/`tag`).
///
/// `title`, `body`, and `urgent` stay `pub` fields, unchanged since this
/// type's original shape — so code outside this module that already
/// holds a `Notification` (from [`Self::new`]) can still read or
/// directly assign them (`n.urgent = true`), same as before this issue.
/// What that does **not** do is keep an exhaustive `Notification {
/// title, body, urgent }` literal compiling outside `backend`'s own
/// module: Rust's field privacy is module-scoped, not just crate-scoped,
/// so the moment the four new fields below are anything other than
/// `pub`, a literal naming this type from any other module — in-tree or
/// downstream — needs every field filled, and can't fill a private one
/// itself. This crate's own two in-tree call sites
/// (`examples/win_platform_services.rs`, `win::services`'s unit test)
/// hit exactly that and were switched to [`Self::new`] in the same PR.
/// The four new fields are deliberately **not** additional `pub` fields
/// for the same reason `#[non_exhaustive]` was rejected too:
/// [`crate::primitives::toolbar::ToolbarIcons`]'s own doc records why
/// growing an already-`pub`-field struct either breaks every existing
/// exhaustive struct literal outright (`E0063`) or, via
/// `#[non_exhaustive]`, breaks it a different way (`E0639`) — both hard
/// breaks with no deprecation shim, per `CLAUDE.md` rule 8's blast-radius
/// concern. Reached instead through [`Self::new`] plus the chainable
/// `with_*` builder methods below (rule 2's "a builder... instead of a
/// new required constructor argument"), so a future field can be added
/// the same way again without ever repeating this struct's original
/// mistake of an all-`pub`-field literal.
#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Whether the notification is high-priority (e.g. error). Backends
    /// may use this to pick a different icon or sound.
    pub urgent: bool,
    icon: Option<ImageSource>,
    actions: Vec<(WidgetId, String)>,
    silent: bool,
    tag: Option<String>,
}

impl Notification {
    /// A plain notification with no icon, actions, tag, or silent flag —
    /// `urgent: false`. Chain the `with_*` methods below to add any of
    /// those.
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            urgent: false,
            icon: None,
            actions: Vec::new(),
            silent: false,
            tag: None,
        }
    }

    /// Attach an icon. The same [`ImageSource`] shape
    /// [`TrayService::set_icon`] takes (issue #955's design note: it
    /// shares the icon type with the tray work, #953) — raw encoded bytes
    /// or a filesystem path, sniffed/decoded by whichever backend can
    /// honor it. Backends with no icon facility (TUI's toast degrade,
    /// today's macOS `osascript` path) ignore this.
    #[must_use]
    pub fn with_icon(mut self, icon: ImageSource) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Register an action button, `id` first so it reads the same order
    /// as [`UiEvent::NotificationActivated`]'s payload
    /// (`action: Some(id)`) that firing it produces. Call multiple times
    /// for multiple buttons; order is preserved. Backends that can't
    /// natively attach actions to a notification (macOS's unbundled
    /// `osascript` fallback, Win's balloon-tip fallback) drop these
    /// silently — see each backend's `send_notification` doc.
    #[must_use]
    pub fn with_action(mut self, id: WidgetId, label: impl Into<String>) -> Self {
        self.actions.push((id, label.into()));
        self
    }

    /// Suppress the notification sound, on backends that have a sound to
    /// suppress: Win's balloon sets `NIIF_NOSOUND` when this is `true`,
    /// and macOS's `osascript` fallback only adds a `sound name` clause
    /// when this is `false` (see `win::services`/`macos::services`'s
    /// `send_notification` docs). GTK's `gio::Notification` has no sound
    /// control of its own at all — the desktop shell decides — so this
    /// has nothing to bind to there.
    #[must_use]
    pub fn with_silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }

    /// An app-chosen tag, round-tripped unchanged through
    /// [`UiEvent::NotificationActivated::tag`] so an app that fires
    /// several notifications can tell which one a later activation
    /// belongs to without inventing its own id scheme. Also the identity
    /// [`PlatformServices::send_notification`] may use to replace an
    /// already-visible notification with the same tag, on backends that
    /// support that (GTK: `gio::Application::send_notification`'s `id`
    /// parameter does this natively).
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// The icon set via [`Self::with_icon`], if any.
    pub fn icon(&self) -> Option<&ImageSource> {
        self.icon.as_ref()
    }

    /// The action buttons registered via [`Self::with_action`], in
    /// registration order.
    pub fn actions(&self) -> &[(WidgetId, String)] {
        &self.actions
    }

    /// Whether [`Self::with_silent`] was set.
    pub fn is_silent(&self) -> bool {
        self.silent
    }

    /// The tag set via [`Self::with_tag`], if any.
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }
}

#[cfg(test)]
mod notification_tests {
    use super::Notification;
    use crate::primitives::image::ImageSource;
    use crate::types::WidgetId;

    #[test]
    fn new_has_no_icon_actions_tag_and_is_not_silent_or_urgent() {
        let n = Notification::new("t", "b");
        assert_eq!(n.title, "t");
        assert_eq!(n.body, "b");
        assert!(!n.urgent);
        assert!(n.icon().is_none());
        assert!(n.actions().is_empty());
        assert!(!n.is_silent());
        assert!(n.tag().is_none());
    }

    #[test]
    fn with_icon_is_read_back_by_icon() {
        let n = Notification::new("t", "b").with_icon(ImageSource::Bytes(vec![1, 2, 3]));
        assert_eq!(n.icon(), Some(&ImageSource::Bytes(vec![1, 2, 3])));
    }

    #[test]
    fn with_action_appends_in_registration_order() {
        let n = Notification::new("t", "b")
            .with_action(WidgetId::new("first"), "First")
            .with_action(WidgetId::new("second"), "Second");
        assert_eq!(
            n.actions(),
            &[
                (WidgetId::new("first"), "First".to_string()),
                (WidgetId::new("second"), "Second".to_string()),
            ]
        );
    }

    #[test]
    fn with_silent_is_read_back_by_is_silent() {
        assert!(Notification::new("t", "b").with_silent(true).is_silent());
        assert!(!Notification::new("t", "b").with_silent(false).is_silent());
    }

    #[test]
    fn with_tag_is_read_back_by_tag() {
        let n = Notification::new("t", "b").with_tag("build-status");
        assert_eq!(n.tag(), Some("build-status"));
    }
}

#[cfg(test)]
mod backend_caps_tests {
    use super::BackendCaps;

    #[test]
    fn empty_declares_nothing() {
        assert!(BackendCaps::empty().names().is_empty());
        assert!(!BackendCaps::empty().has("text_selection"));
    }

    #[test]
    fn names_round_trips_through_has() {
        let caps = BackendCaps {
            text_selection: true,
            file_dialogs: true,
            ..BackendCaps::empty()
        };
        let names = caps.names();
        assert_eq!(names, vec!["text_selection", "file_dialogs"]);
        for name in &names {
            assert!(caps.has(name), "{name:?} in names() but has() disagrees");
        }
        // Every other capability name is honestly absent.
        for (name, _) in BackendCaps::ALL_NAMES {
            if !names.contains(name) {
                assert!(!caps.has(name), "{name:?} not in names() but has() = true");
            }
        }
    }

    #[test]
    fn has_is_false_not_a_panic_for_an_unknown_name() {
        assert!(!BackendCaps::empty().has("not_a_real_capability"));
    }

    /// A capability name paired with a setter for the field it names —
    /// the write-side mirror of [`super::NamedCap`]'s read-side accessor.
    type NamedSetter = (&'static str, fn(&mut BackendCaps));

    /// Every capability name paired with a setter for the field it is
    /// supposed to read. Deliberately written out rather than derived
    /// from `ALL_NAMES`: this table's whole job is to be an *independent*
    /// statement of the field↔name mapping, so a copy-paste slip in
    /// `ALL_NAMES` (two entries reading the same field — the classic way
    /// a bitflag table goes wrong) has something to disagree with.
    const SETTERS: &[NamedSetter] = &[
        ("mouse", |c| c.mouse = true),
        ("scroll", |c| c.scroll = true),
        ("drag", |c| c.drag = true),
        ("text_selection", |c| c.text_selection = true),
        ("native_menu", |c| c.native_menu = true),
        ("window_chrome", |c| c.window_chrome = true),
        ("pointer_cursor", |c| c.pointer_cursor = true),
        ("ime", |c| c.ime = true),
        ("file_dialogs", |c| c.file_dialogs = true),
        ("folder_dialogs", |c| c.folder_dialogs = true),
        ("native_dialogs", |c| c.native_dialogs = true),
        ("notifications", |c| c.notifications = true),
        ("app_font_registration", |c| c.app_font_registration = true),
        ("window_control", |c| c.window_control = true),
        ("tray", |c| c.tray = true),
        ("generic_font_families", |c| c.generic_font_families = true),
    ];

    #[test]
    fn all_names_lists_every_field_exactly_once() {
        // Exhaustive destructure on purpose: adding a `BackendCaps` field
        // without adding it to `SETTERS` (and therefore to the `want`
        // list below, and therefore to `ALL_NAMES`) is a *compile* error
        // here. A field missing from `ALL_NAMES` would otherwise be
        // invisible to `names()`/`has()`/`vocabulary()` — and so to every
        // scenario `requires` gate and to the C0 honesty check.
        //
        // `color_depth` and `kitty_keyboard` are the deliberate exceptions:
        // neither is a bool *capability* in the "does this backend
        // implement optional surface X" sense (see each field's doc
        // comment on `BackendCaps`) — both are runtime-detected properties
        // of the terminal a TUI session happens to be running in. They are
        // named here — forcing a future field addition to make a conscious
        // choice about which bucket it belongs in — but intentionally left
        // out of `SETTERS`/`want`/`ALL_NAMES`.
        let BackendCaps {
            mouse: _,
            scroll: _,
            drag: _,
            text_selection: _,
            native_menu: _,
            window_chrome: _,
            pointer_cursor: _,
            ime: _,
            file_dialogs: _,
            folder_dialogs: _,
            native_dialogs: _,
            notifications: _,
            app_font_registration: _,
            window_control: _,
            tray: _,
            generic_font_families: _,
            color_depth: _,
            kitty_keyboard: _,
        } = BackendCaps::empty();

        let want: Vec<&str> = SETTERS.iter().map(|(n, _)| *n).collect();
        let got: Vec<&str> = BackendCaps::ALL_NAMES.iter().map(|(n, _)| *n).collect();
        assert_eq!(got, want);
        assert_eq!(BackendCaps::vocabulary(), want);
    }

    #[test]
    fn each_accessor_reads_its_own_field() {
        for (name, set) in SETTERS {
            let mut caps = BackendCaps::empty();
            set(&mut caps);
            assert_eq!(
                caps.names(),
                vec![*name],
                "setting only `{name}` should make exactly `{name}` declared — an `ALL_NAMES` \
                 accessor is reading the wrong field"
            );
            assert!(caps.has(name));
        }
    }

    #[test]
    fn vocabulary_is_the_superset_names_draws_from() {
        let vocab = BackendCaps::vocabulary();
        let mut all = BackendCaps::empty();
        for (_, set) in SETTERS {
            set(&mut all);
        }
        assert_eq!(
            all.names(),
            vocab,
            "with every field true, `names()` must be the whole vocabulary"
        );
    }
}

#[cfg(test)]
mod clipboard_default_tests {
    //! Coverage for [`Clipboard`]'s new-in-#954 defaulted methods
    //! (`read_image`/`write_image`/`write_html`/`read_file_list`/
    //! `formats`/`clear`), backend-agnostic: a bare fake `impl Clipboard`
    //! that overrides nothing but `read_text`/`write_text` stands in for
    //! "a backend that hasn't touched this issue's surface at all" (the
    //! whole point of keeping these methods defaulted rather than
    //! required — every existing implementor keeps compiling). Real
    //! per-backend wiring (arboard-backed image/html/file-list on
    //! macOS/GTK, `CF_HDROP` file-list + `EmptyClipboard` on Win) is
    //! covered in each backend's own `services.rs` tests instead, since
    //! it needs that backend's real native clipboard.
    use super::*;
    use std::cell::RefCell;

    /// Minimal `Clipboard` impl backed by an in-process `RefCell<Option<String>>`
    /// — enough to exercise every trait *default* without touching any OS
    /// clipboard API.
    #[derive(Default)]
    struct FakeTextOnlyClipboard {
        text: RefCell<Option<String>>,
    }

    impl Clipboard for FakeTextOnlyClipboard {
        fn read_text(&self) -> Option<String> {
            self.text.borrow().clone()
        }

        fn write_text(&self, text: &str) {
            *self.text.borrow_mut() = Some(text.to_string());
        }
    }

    #[test]
    fn image_html_file_list_and_clear_default_to_unsupported() {
        let cb = FakeTextOnlyClipboard::default();
        assert_eq!(cb.read_image(), Err(BackendError::Unsupported));
        assert_eq!(
            cb.write_image(&RgbaImage {
                width: 1,
                height: 1,
                pixels: vec![0, 0, 0, 255],
            }),
            Err(BackendError::Unsupported)
        );
        assert_eq!(
            cb.write_html("<b>hi</b>", "hi"),
            Err(BackendError::Unsupported)
        );
        assert_eq!(cb.read_file_list(), Err(BackendError::Unsupported));
        assert_eq!(cb.clear(), Err(BackendError::Unsupported));
    }

    #[test]
    fn formats_is_empty_when_clipboard_is_empty() {
        let cb = FakeTextOnlyClipboard::default();
        assert!(cb.formats().is_empty());
    }

    #[test]
    fn formats_reports_text_once_written_and_nothing_else() {
        let cb = FakeTextOnlyClipboard::default();
        cb.write_text("hello");
        // Image/file-list stay `Unsupported` on this fake, so `formats`
        // must not report them just because text is present.
        assert_eq!(cb.formats(), vec![ClipboardFormat::Text]);
    }

    /// A fake that also backs `read_image`/`read_file_list`, to prove
    /// `formats`'s default dispatches through `self` (i.e. reaches an
    /// override) rather than hardcoding the trait's own default bodies.
    #[derive(Default)]
    struct FakeFullClipboard {
        text: RefCell<Option<String>>,
        image: RefCell<Option<RgbaImage>>,
        file_list: RefCell<Vec<PathBuf>>,
    }

    impl Clipboard for FakeFullClipboard {
        fn read_text(&self) -> Option<String> {
            self.text.borrow().clone()
        }

        fn write_text(&self, text: &str) {
            *self.text.borrow_mut() = Some(text.to_string());
        }

        fn read_image(&self) -> ServiceResult<RgbaImage> {
            self.image.borrow().clone().ok_or(BackendError::Unsupported)
        }

        fn read_file_list(&self) -> ServiceResult<Vec<PathBuf>> {
            Ok(self.file_list.borrow().clone())
        }
    }

    #[test]
    fn formats_dispatches_through_overrides_for_image_and_file_list() {
        let cb = FakeFullClipboard::default();
        assert!(cb.formats().is_empty());

        *cb.image.borrow_mut() = Some(RgbaImage {
            width: 2,
            height: 2,
            pixels: vec![0; 16],
        });
        assert_eq!(cb.formats(), vec![ClipboardFormat::Image]);

        cb.file_list.borrow_mut().push(PathBuf::from("/tmp/a.txt"));
        assert_eq!(
            cb.formats(),
            vec![ClipboardFormat::Image, ClipboardFormat::FileList]
        );

        cb.write_text("hi");
        assert_eq!(
            cb.formats(),
            vec![
                ClipboardFormat::Text,
                ClipboardFormat::Image,
                ClipboardFormat::FileList
            ]
        );
    }

    #[test]
    fn empty_file_list_does_not_count_as_the_file_list_format() {
        let cb = FakeFullClipboard::default();
        // `read_file_list` returns `Ok(vec![])` (empty, but not
        // `Unsupported`) — `formats` must treat that the same as "no
        // file list present", not report `ClipboardFormat::FileList`.
        assert!(cb.formats().is_empty());
    }
}

/// Coverage for [`KeyringSecretStore`] — the real, `keyring`-backed
/// implementation [`PlatformServices::secret_store`]'s default vends
/// (issue #958). Tests a real round trip against this host's actual
/// credential store, same headless-skip posture as `desktop.rs`'s
/// `move_to_trash_tests`: a store that's genuinely unreachable (no D-Bus
/// session, a locked/sandboxed keychain) is an environment limitation,
/// not a bug in this code, so those cases skip with an explanation
/// rather than fail.
///
/// **That posture has to hold for *every* step of a round trip, not just
/// the first one** — the lesson of this module's own CI failure. "No
/// usable credential store" is not one binary condition a test can check
/// up front and then assume away: a Secret Service can be *reachable*
/// (so `set` creates a collection and succeeds) while every subsequent
/// read is gated behind an interactive unlock prompt that a headless
/// runner can never answer (so `get` fails ~25s later). `ci.yml`'s `gtk`
/// job is exactly that host and nothing else in the fleet is: its
/// `apt-get install libgtk-4-dev libpango1.0-dev libcairo2-dev` pulls
/// `gnome-keyring` + a session bus into the runner transitively, which
/// the otherwise-identical `tui` job never gets. So `set`-then-fail is
/// reachable on one Linux CI leg and not the other, and a test that
/// hard-asserts on anything after a successful `set` is asserting on the
/// runner's apt closure. Every store call below therefore treats `Err`
/// as a skip; only *wrong values* from a store that answered are
/// failures.
#[cfg(all(
    test,
    any(feature = "tui", feature = "gtk", feature = "macos", feature = "win")
))]
mod secret_store_tests {
    use super::*;

    /// Every entry this module's tests touch shares this service name and
    /// gets its own `account`, namespaced by process id plus a
    /// test-specific suffix so concurrent test runs (and re-runs against
    /// a real, persistent OS credential store) never collide with each
    /// other or leave state a later run trips over.
    const SERVICE: &str = "quadraui-958-secret-store-test";

    fn unique_account(suffix: &str) -> String {
        format!("account-{}-{suffix}", std::process::id())
    }

    /// `Ok(v)` ⇒ the store answered, keep asserting on `v`. `Err` ⇒ this
    /// host has no usable credential store *for this operation*, so
    /// report why and let the caller bail out — see the module doc for
    /// why that judgement is made per call rather than once up front.
    #[allow(clippy::print_stderr)]
    fn answered<T>(op: &str, result: ServiceResult<T>) -> Option<T> {
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!(
                    "skipping: secret_store {op} failed in this environment ({e:?}) — \
                     no reachable/unlockable keyring (no D-Bus session, or a collection \
                     that only an interactive prompt can unlock)"
                );
                None
            }
        }
    }

    /// Set → get → delete → get against a fresh service/account pair.
    /// Any step that the host's store can't service skips the rest — see
    /// the module doc. A store that *answers* with the wrong secret (or
    /// still has one after `delete`) is always a real failure.
    #[test]
    fn round_trip_set_get_delete() {
        let store = KeyringSecretStore;
        let account = unique_account("round-trip");

        if answered("set()", store.set(SERVICE, &account, "s3cr3t")).is_none() {
            return;
        }

        let Some(read_back) = answered("get()", store.get(SERVICE, &account)) else {
            // The write landed but can't be read back here; don't leave
            // it behind for whatever runs next against this store.
            let _ = store.delete(SERVICE, &account);
            return;
        };
        assert_eq!(
            read_back,
            Some("s3cr3t".to_string()),
            "get() must return exactly what set() just wrote"
        );

        if answered("delete()", store.delete(SERVICE, &account)).is_none() {
            return;
        }

        let Some(after_delete) = answered("get() after delete()", store.get(SERVICE, &account))
        else {
            return;
        };
        assert_eq!(
            after_delete, None,
            "get() after delete() must report no entry, not the deleted secret"
        );
    }

    /// A service/account pair that was never set reads as `Ok(None)` —
    /// this trait's "nothing here, not a failure" idiom (see
    /// [`SecretStore`]'s own doc) — not `Err`. Tolerates `Err` from the
    /// underlying store as an environment-limitation skip, same as
    /// [`round_trip_set_get_delete`], but a `Some` would mean a leaked
    /// entry from a previous run and is always a real failure.
    #[test]
    fn get_on_an_entry_that_was_never_set_reports_no_entry() {
        let store = KeyringSecretStore;
        let account = unique_account("never-set");

        let Some(found) = answered("get()", store.get(SERVICE, &account)) else {
            return;
        };
        assert_eq!(
            found, None,
            "fresh service/account pair should have no entry — leftover from a previous \
             test run?"
        );
    }

    /// Deleting an entry that doesn't exist is a real, reported failure —
    /// not silently `Ok(())` — matching
    /// [`PlatformServices::move_to_trash`]'s identical stance for a
    /// nonexistent path (see [`SecretStore`]'s own doc).
    ///
    /// This one needs no environment skip and deliberately doesn't have
    /// one: `Err` is the assertion, and an unreachable store produces
    /// `Err` too, so the test is meaningful where a store exists and
    /// vacuously true (never wrong) where one doesn't.
    #[test]
    fn delete_on_a_nonexistent_entry_is_a_reported_failure() {
        let store = KeyringSecretStore;
        let account = unique_account("delete-missing");

        assert!(
            store.delete(SERVICE, &account).is_err(),
            "delete() on an entry that was never set must not report success"
        );
    }

    /// [`keyring_error_to_backend`] folds every `keyring::Error` onto
    /// [`BackendError::PlatformFailure`], keeping the crate's own
    /// `Display` text as context — deterministic coverage that holds on
    /// a runner with no credential store at all, where every test above
    /// skips.
    #[test]
    fn keyring_errors_become_platform_failures_carrying_their_display_text() {
        let mapped = keyring_error_to_backend(keyring::Error::NoEntry);
        let BackendError::PlatformFailure { context } = mapped else {
            panic!("every keyring::Error must map to PlatformFailure, got {mapped:?}");
        };
        assert!(
            context.starts_with("keyring: "),
            "context must name the failing subsystem, got {context:?}"
        );
        assert!(
            context.len() > "keyring: ".len(),
            "context must keep the keyring error's own Display text, got {context:?}"
        );
    }
}

/// [`KeyringSecretStore`]'s fallback for a build with none of the
/// `tui`/`gtk`/`macos`/`win` features enabled — see that variant's own
/// doc. Exercised by `cargo test --no-default-features` (not part of
/// this crate's CI quality gate, which always enables at least `tui`,
/// but still a real, buildable configuration this crate supports).
#[cfg(all(
    test,
    not(any(feature = "tui", feature = "gtk", feature = "macos", feature = "win"))
))]
mod secret_store_unsupported_tests {
    use super::*;

    #[test]
    fn every_method_reports_unsupported_without_a_keyring_backend() {
        let store = KeyringSecretStore;
        assert_eq!(store.get("svc", "acct"), Err(BackendError::Unsupported));
        assert_eq!(
            store.set("svc", "acct", "secret"),
            Err(BackendError::Unsupported)
        );
        assert_eq!(store.delete("svc", "acct"), Err(BackendError::Unsupported));
    }
}
