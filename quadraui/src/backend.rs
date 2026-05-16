//! The `Backend` trait — one implementation per platform target.
//!
//! Each backend (TUI, GTK, Win-GUI, and eventually macOS) implements this
//! trait. Apps write render code once, parameterised over `<B: Backend>`,
//! and every supported platform rasterises the same primitive descriptions
//! with platform-native drawing + input.
//!
//! See `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md` §4 for design rationale.

use std::path::PathBuf;
use std::time::Duration;

use crate::event::{Rect, UiEvent, Viewport};
use crate::modal_stack::ModalStack;
use crate::primitives::activity_bar::ActivityBarRowHit;
use crate::primitives::chart::{Chart, ChartLayout};
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout};
use crate::primitives::command_line::CommandLine;
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
    Accelerator, AcceleratorId, ActivityBar, Form, ListView, Palette, StatusBar, TabBar, Terminal,
    TextDisplay, TreeView,
};

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

    // ─── Modal-overlay tracking ────────────────────────────────────────
    /// Mutable handle to the backend's modal stack. Apps push when a
    /// palette / dialog / context-menu opens and pop when it closes;
    /// quadraui's dispatcher consults the stack so events inside an
    /// open modal can't fall through to widgets behind it.
    ///
    /// See [`ModalStack`] and [`crate::dispatch::dispatch_mouse_down`]
    /// for the routing contract.
    fn modal_stack_mut(&mut self) -> &mut ModalStack;

    // ─── Platform services ─────────────────────────────────────────────
    /// Clipboard, file dialogs, notifications, URL opening, platform name.
    fn services(&self) -> &dyn PlatformServices;

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
    fn draw_tree(&mut self, rect: Rect, tree: &TreeView);
    fn draw_list(&mut self, rect: Rect, list: &ListView);
    fn draw_data_table(
        &mut self,
        rect: Rect,
        table: &DataTable,
        hovered_idx: Option<usize>,
    ) -> DataTableLayout;
    fn data_table_layout(&self, rect: Rect, table: &DataTable) -> DataTableLayout;
    fn draw_form(&mut self, rect: Rect, form: &Form);
    fn draw_palette(&mut self, rect: Rect, palette: &Palette);

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
    /// Draw an activity bar. `hovered_idx` carries per-frame hover
    /// state so the rasteriser can paint a tint on the hovered row.
    /// Returns per-row hit regions for click + tooltip dispatch.
    fn draw_activity_bar(
        &mut self,
        rect: Rect,
        bar: &ActivityBar,
        hovered_idx: Option<usize>,
    ) -> Vec<ActivityBarRowHit>;
    /// Draw a terminal cell grid. No hit-region data is returned;
    /// terminal selection is driven by mouse drag against cell
    /// dimensions, which the app already tracks.
    fn draw_terminal(&mut self, rect: Rect, term: &Terminal);
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

    /// Compute the text-display layout the rasteriser would produce for
    /// `td` in `rect`, using the backend's native metrics. Hosts call
    /// this to drive hit-testing for scrollbar drag interaction without
    /// re-deriving metrics — paint and click consume one layout per
    /// frame, the source-of-truth contract.
    fn text_display_layout(&self, rect: Rect, td: &TextDisplay) -> TextDisplayLayout;

    /// Draw a [`Tooltip`] popup at its caller-resolved layout. The
    /// caller computes anchor + viewport + content measurement and
    /// asks `tooltip.layout(...)` for the bounds. Tooltips are
    /// non-interactive — no hit data returned.
    fn draw_tooltip(&mut self, tooltip: &Tooltip, layout: &TooltipLayout);

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
    fn tree_layout(&self, rect: Rect, tree: &TreeView) -> TreeViewLayout;

    /// Compute the form layout the rasteriser would produce for `form`
    /// in `rect`, using the backend's native metrics. Hosts call this
    /// to drive hit-testing — especially for `ToggleGroup` and
    /// `ButtonRow` fields where per-item hit regions depend on
    /// backend-specific text measurement.
    fn form_layout(&self, rect: Rect, form: &Form) -> FormLayout;

    /// Draw an [`Editor`]. Returns paint-side data the host needs
    /// for chrome alignment (cursor pixel position for caret blink
    /// overlays, etc.). Asymmetric across backends: TUI populates
    /// the result; GTK paints its own caret and returns the default.
    fn draw_editor(&mut self, rect: Rect, editor: &Editor) -> EditorPaintResult;

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

    /// Draw a [`MenuBar`]. The backend computes the layout internally
    /// with native metrics (cells for TUI, Pango pixels for GTK) and
    /// returns the [`MenuBarLayout`] so hosts can route clicks via
    /// `layout.hit_test(x, y)` without re-deriving metrics.
    fn draw_menu_bar(&mut self, rect: Rect, bar: &MenuBar) -> MenuBarLayout;

    /// Compute the menu-bar layout the rasteriser would produce for
    /// `bar` in `rect`, using the backend's native metrics. Hosts
    /// call this in click handlers to resolve hits against the same
    /// layout that was painted — never re-derive with a hand-rolled
    /// measurer.
    fn menu_bar_layout(&self, rect: Rect, bar: &MenuBar) -> MenuBarLayout;

    /// Draw a [`Split`] divider. The backend computes the layout with
    /// its native divider thickness (1 cell for TUI, ~4px for GTK)
    /// and returns the [`SplitLayout`] so hosts can route clicks and
    /// drive drag operations. Pane content is NOT drawn — hosts paint
    /// into `layout.first_bounds` / `layout.second_bounds`.
    fn draw_split(&mut self, rect: Rect, split: &Split) -> SplitLayout;

    /// Compute the split layout without painting. Hosts call this in
    /// drag handlers to recompute the ratio from cursor position.
    fn split_layout(&self, rect: Rect, split: &Split) -> SplitLayout;

    /// Draw a [`Panel`] chrome (title bar + action buttons). The
    /// backend computes the layout with its native title-bar height
    /// (1 cell for TUI, line_height for GTK) and returns the
    /// [`PanelLayout`] so hosts can route clicks to actions, title
    /// bar, or content. Content is NOT drawn — hosts paint into
    /// `layout.content_bounds`.
    fn draw_panel(&mut self, rect: Rect, panel: &Panel) -> PanelLayout;

    /// Compute the panel layout without painting. Hosts call this in
    /// click handlers to resolve hits without re-deriving metrics.
    fn panel_layout(&self, rect: Rect, panel: &Panel) -> PanelLayout;

    /// Draw a [`ToastStack`] overlay. The backend computes the layout
    /// with its native toast dimensions (cell-width boxes for TUI,
    /// pixel boxes for GTK) and returns the [`ToastStackLayout`] so
    /// hosts can route clicks to dismiss, action, or body.
    fn draw_toast_stack(&mut self, rect: Rect, stack: &ToastStack) -> ToastStackLayout;

    /// Compute the toast-stack layout without painting. Hosts call
    /// this in click handlers to resolve hits.
    fn toast_stack_layout(&self, rect: Rect, stack: &ToastStack) -> ToastStackLayout;

    /// Draw a [`ProgressBar`]. The backend paints the track, fill,
    /// optional label, and optional cancel affordance. Returns the
    /// [`ProgressBarLayout`] so hosts can route clicks.
    fn draw_progress(&mut self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout;

    /// Compute progress-bar layout without painting.
    fn progress_layout(&self, rect: Rect, bar: &ProgressBar) -> ProgressBarLayout;

    /// Draw a [`Spinner`] (indeterminate activity indicator). Returns
    /// the [`SpinnerLayout`] for host hit-testing.
    fn draw_spinner(&mut self, rect: Rect, spinner: &Spinner) -> SpinnerLayout;

    /// Compute spinner layout without painting.
    fn spinner_layout(&self, rect: Rect, spinner: &Spinner) -> SpinnerLayout;

    /// Draw a [`CommandCenter`] (nav arrows + search box). Returns the
    /// [`CommandCenterLayout`] so hosts can route clicks.
    fn draw_command_center(&mut self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout;

    /// Compute command-center layout without painting.
    fn command_center_layout(&self, rect: Rect, cc: &CommandCenter) -> CommandCenterLayout;

    /// Draw a [`Chart`] (sparkline, line, or bar). `hovered_point`
    /// carries per-frame hover state (series_idx, data_idx) so the
    /// rasteriser can highlight the data point under the cursor.
    /// Returns the [`ChartLayout`] so hosts can route clicks and
    /// resolve nearest-point from mouse position.
    fn draw_chart(
        &mut self,
        rect: Rect,
        chart: &Chart,
        hovered_point: Option<(usize, usize)>,
        crosshair_x: Option<f64>,
    ) -> ChartLayout;

    /// Compute chart layout without painting.
    fn chart_layout(&self, rect: Rect, chart: &Chart) -> ChartLayout;
}

/// Paint-side data returned by [`Backend::draw_editor`]. Carries
/// information the host needs to align external chrome (caret blink
/// overlay, virtual-text positioning) with the editor's painted
/// content. Backends that paint their own caret (GTK) populate the
/// default; backends that delegate caret rendering to the host (TUI
/// terminal cursor) populate the actual cursor cell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditorPaintResult {
    /// Cursor's painted position in backend-native units, if the
    /// host is responsible for terminal-cursor positioning. `None`
    /// when the backend painted its own caret OR when the cursor is
    /// outside the viewport.
    pub cursor_position: Option<(u16, u16)>,
}

/// Platform services the backend exposes to apps: clipboard, file dialogs,
/// notifications, URL opening.
pub trait PlatformServices {
    fn clipboard(&self) -> &dyn Clipboard;

    /// Show a native file-open dialog (blocking). Returns `None` if the
    /// user cancelled. TUI backends return `None` and write a hint to
    /// stderr; apps should provide an in-TUI picker instead.
    fn show_file_open_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

    /// Show a native file-save dialog.
    fn show_file_save_dialog(&self, opts: FileDialogOptions) -> Option<PathBuf>;

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

/// A system notification request.
#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Whether the notification is high-priority (e.g. error). Backends
    /// may use this to pick a different icon or sound.
    pub urgent: bool,
}
