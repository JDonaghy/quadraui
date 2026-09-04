//! The `Backend` trait — one implementation per platform target.
//!
//! Each backend (TUI, GTK, Win-GUI, and eventually macOS) implements this
//! trait. Apps write render code once, parameterised over `<B: Backend>`,
//! and every supported platform rasterises the same primitive descriptions
//! with platform-native drawing + input.
//!
//! See `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md` §4 for design rationale.
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
//! `quadraui/docs/DECISIONS.md` D-006 for the full picture, including
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
//! `quadraui/docs/DECISIONS.md` D-005 for why the split exists and why
//! it isn't collapsed to one frame; `quadraui/docs/PRIMITIVE_RULES.md`
//! "Coordinate frames for `*_layout` methods" for the authoring rule).
//! `quadraui/docs/LESSONS.md` "Layout helpers must return coords in the
//! same frame across backends" records the bug class this convention
//! guards against: a `*_layout` twin that silently disagrees with its
//! own TUI/GTK siblings about which frame it returns.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crate::dispatch::DragState;
use crate::event::{Point, Rect, UiEvent, Viewport};
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
use crate::primitives::editor::{Editor, EditorLayout};
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::form::FormLayout;
use crate::primitives::image::Image;
use crate::primitives::list::ListViewLayout;
use crate::primitives::menu_bar::{MenuBar, MenuBarLayout};
use crate::primitives::message_list::MessageList;
use crate::primitives::minimap::{Minimap, MinimapLayout};
use crate::primitives::multi_section_view::{
    LayoutMetrics, MultiSectionView, MultiSectionViewLayout,
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
use crate::primitives::tab_bar::{TabBarHits, TabBarLayout, TabChrome, TabIcon};
use crate::primitives::text_display::TextDisplayLayout;
use crate::primitives::text_input::{TextInput, TextInputLayout};
use crate::primitives::toast::{ToastStack, ToastStackLayout};
use crate::primitives::toolbar::{Toolbar, ToolbarLayout};
use crate::primitives::tooltip::{Tooltip, TooltipChrome, TooltipLayout};
use crate::primitives::tree::TreeViewLayout;
use crate::types::WidgetId;
use crate::{
    Accelerator, AcceleratorId, ActivityBar, Form, ListView, Palette, StatusBar, TabBar, Terminal,
    TextDisplay, TreeView,
};

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
            native_dialogs: false,
            notifications: false,
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
        ("native_dialogs", |c| c.native_dialogs),
        ("notifications", |c| c.notifications),
    ];
}

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
    // (see `BACKEND_TRAIT_PROPOSAL.md` §4). Backends opt in to the new
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

    /// Draw settings-panel chrome: a 2-row strip with a header row and a
    /// search input row, designed to sit immediately above a [`Form`]
    /// body. See `tui::draw_settings_chrome` / `gtk::draw_settings_chrome`
    /// for the exact row layout, `" / "`-prefixed prompt construction, and
    /// placeholder logic.
    ///
    /// No default impl — every backend implementer sees this as a compile
    /// error and fills in a real rasteriser (`BACKEND_TRAIT_PROPOSAL.md`
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

    // Layout-passthrough primitives (per BACKEND_TRAIT_PROPOSAL.md
    // §6.2). Each backend computes the primitive's layout internally
    // using its native measurer (cells for TUI, Pango / DirectWrite /
    // Core Text pixels for the others) — apps don't have access to
    // those handles, so layout precomputation can't live caller-side.
    //
    // Methods that produce hit-region data (clickable segments,
    // close-button rects, link rects) return it directly so callers
    // route clicks against the same data the rasteriser used to paint.
    /// Draw a status bar. `hovered_id` and `pressed_id` carry per-frame
    /// interaction state so the rasteriser can tint the background of the
    /// matching clickable segment (the primitive itself carries no mouse
    /// state — same pattern as `ActivityBar`'s `hovered_idx`). Returns
    /// hit regions in **bar-local coordinates** (relative to `rect.x` /
    /// `rect.y`) for each segment carrying an `action_id`. Caller
    /// dispatches clicks against the returned list.
    fn draw_status_bar(
        &mut self,
        rect: Rect,
        bar: &StatusBar,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> StatusBarLayout;
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
    /// (`BACKEND_TRAIT_PROPOSAL.md` §4, `PRIMITIVE_RULES.md` rule 7).
    fn draw_tab_bar_icons(
        &mut self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<TabIcon>],
        hovered_close_tab: Option<usize>,
    ) -> TabBarHits;
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
    fn tab_bar_layout(&self, rect: Rect, bar: &TabBar) -> TabBarHits;

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
    fn tab_bar_layout_icons(
        &self,
        rect: Rect,
        bar: &TabBar,
        icons: &[Option<TabIcon>],
    ) -> TabBarHits;

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
    fn tab_bar_layout_with_chrome(
        &self,
        rect: Rect,
        bar: &TabBar,
        chrome: &TabChrome,
    ) -> TabBarHits {
        let _ = chrome;
        self.tab_bar_layout(rect, bar)
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
    /// (`BACKEND_TRAIT_PROPOSAL.md` §4, `PRIMITIVE_RULES.md` rule 7).
    /// Do not add a no-op default here; see
    /// `docs/SMELL_AUDIT_2026-07.md` PORT-01 for why that pattern is a
    /// portability risk, not a precedent to follow.
    fn draw_terminal_divider(&mut self, rect: Rect);
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
    fn msv_metrics(&self) -> LayoutMetrics;

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
    /// content area — distinct from `StatusBar` which is read-only).
    /// `hovered_id` / `pressed_id` carry per-frame mouse state so the
    /// rasteriser can tint the matching button's background (same
    /// pattern as `StatusBar`). Returns the [`ToolbarLayout`] so hosts
    /// can route clicks via `layout.hit_test(x, y)` without re-deriving
    /// metrics. Same coordinate frame as [`Self::toolbar_layout`]
    /// (ABSOLUTE).
    fn draw_toolbar(
        &mut self,
        rect: Rect,
        bar: &Toolbar,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> ToolbarLayout;

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
    /// `hovered_toolbar_id` / `pressed_toolbar_id` are forwarded to
    /// the nested toolbar paint for hover / pressed tints. Same
    /// coordinate frame as [`Self::sidebar_panel_layout`] (ABSOLUTE).
    fn draw_sidebar_panel(
        &mut self,
        rect: Rect,
        panel: &SidebarPanel,
        hovered_toolbar_id: Option<&WidgetId>,
        pressed_toolbar_id: Option<&WidgetId>,
    ) -> SidebarPanelLayout;

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
    /// #667, #738); TUI packs `U+2800`-block braille dots at a
    /// stretch-to-fill pitch ([`crate::MinimapSizing::Fill`]). All three
    /// techniques consume the exact same [`Minimap`] data — the primitive
    /// owns the sampling and colour-aggregation math (`sample_lines` /
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
    /// rule 7). Still `todo!()` on macOS — out of scope per #382; #738
    /// lifted the shared decision logic a future macOS rasteriser would
    /// consume, but writing the actual Core Graphics/Core Text paint
    /// calls is not part of that lift.
    fn draw_minimap(&mut self, rect: Rect, minimap: &Minimap) -> MinimapPaintResult;

    /// Compute [`Minimap`] layout without painting — mirrors
    /// [`Backend::chart_layout`] / [`Backend::tree_layout`]. Apps call
    /// this from `AppLogic::handle` (which only has `&mut self`, not the
    /// `MinimapPaintResult` `render` produced) to hit-test a click
    /// against the same geometry the last paint used.
    ///
    /// Coordinate frame: **ABSOLUTE** — shifted by `rect.x` / `rect.y`
    /// (issue #505). Real on GTK and Win-GUI (#738); still `todo!()` on
    /// macOS (out of scope per #382) — do not call on that backend.
    fn minimap_layout(&self, rect: Rect, minimap: &Minimap) -> MinimapLayout;

    /// Paint `image` within `rect`, honoring `image.fit` (see
    /// [`Image::layout`] for the geometry). GTK decodes `image.source`
    /// through `gdk_pixbuf` and paints real pixels; macOS/Win are scoped
    /// out of this first pass the same way `draw_minimap` scopes
    /// macOS/Win out of #382 — a deliberate `todo!()`, not a silent
    /// no-op, because this method has no default (rule 7 below still
    /// applies to it).
    ///
    /// **TUI cannot rasterise an image** — there is no pixel grid to
    /// draw into, and this primitive deliberately does not attempt an
    /// ASCII-art decoder (see `primitives::image` module docs' scope
    /// guard). It paints [`Image::fallback_text`] instead, centered in
    /// `rect`, and reports [`ImagePaintResult::Unsupported`] rather than
    /// a silent no-op — #507's Unsupported-vs-failure question, and this
    /// primitive is a fresh, deliberate instance of it: TUI genuinely
    /// cannot do this, so it says so. A GTK/macOS decode failure (bad
    /// path, corrupt bytes) also reports `Unsupported` and paints
    /// nothing, so a host can tell "no pixels appeared" apart from a
    /// successful paint without inspecting pixels itself.
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
    /// The backend could not (or, for TUI, categorically can not)
    /// rasterise pixels. TUI paints `image.fallback_text` instead;
    /// GTK/macOS on a decode failure paint nothing. See
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
/// dialogs, message/alert dialogs, notifications, URL opening.
pub trait PlatformServices {
    fn clipboard(&self) -> &dyn Clipboard;

    /// Show a native file-open dialog (blocking). Returns `None` if the
    /// user cancelled. TUI backends have no native dialog to show and
    /// unconditionally return `None` (no stderr hint is written); apps
    /// should provide an in-TUI picker instead.
    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

    /// Show a native file-save dialog.
    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

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

/// A system notification request.
#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Whether the notification is high-priority (e.g. error). Backends
    /// may use this to pick a different icon or sound.
    pub urgent: bool,
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
        ("native_dialogs", |c| c.native_dialogs = true),
        ("notifications", |c| c.notifications = true),
    ];

    #[test]
    fn all_names_lists_every_field_exactly_once() {
        // Exhaustive destructure on purpose: adding a `BackendCaps` field
        // without adding it to `SETTERS` (and therefore to the `want`
        // list below, and therefore to `ALL_NAMES`) is a *compile* error
        // here. A field missing from `ALL_NAMES` would otherwise be
        // invisible to `names()`/`has()`/`vocabulary()` — and so to every
        // scenario `requires` gate and to the C0 honesty check.
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
            native_dialogs: _,
            notifications: _,
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
