//! # quadraui
//!
//! Cross-platform UI primitives for keyboard-driven desktop and terminal apps.
//!
//! Targets four rendering backends with a single declarative API:
//! - **Windows** (Direct2D + DirectWrite) — `windows-rs`
//! - **Linux** (GTK4 + Cairo + Pango) — `gtk4`
//! - **macOS** (Core Graphics + Core Text) — *planned, v1.x*
//! - **TUI** (ratatui + crossterm) — works everywhere as a fallback
//!
//! ## What's in the box
//!
//! Nine primitives, each declarative + serde-friendly so apps and Lua
//! plugins can describe UI as data:
//!
//! | Primitive | Use for |
//! |-----------|---------|
//! | [`TreeView`] | File explorers, source-control panels, hierarchical lists |
//! | [`ListView`] | Quickfix, search results, flat selectable lists |
//! | [`Form`] | Settings panels, config editors |
//! | [`Palette`] | Command palettes, fuzzy pickers |
//! | [`StatusBar`] | Mode/file/cursor strips, footer bars |
//! | [`TabBar`] | Editor tabs, document tabs |
//! | [`ActivityBar`] | Vertical icon strips (VSCode-style) |
//! | [`Terminal`] | Cell grids for terminal emulators |
//! | [`TextDisplay`] | Streaming logs, AI chat output |
//!
//! ## How it works
//!
//! Apps build primitive descriptions from their own state. Backends consume
//! the descriptions and rasterise them. Events flow back as `*Event` enums
//! that reference primitives by [`WidgetId`] (owned strings — plugin-safe,
//! no `&'static str`).
//!
//! ```ignore
//! // App (your code) — state-driven, declarative:
//! let bar = quadraui::StatusBar {
//!     id: WidgetId::new("status:editor"),
//!     left_segments: vec![mode_segment(), filename_segment()],
//!     right_segments: vec![lsp_segment(), cursor_segment()],
//! };
//!
//! // Backend (yours or one of the existing ones) — measure + paint:
//! draw_status_bar(cr, &bar, &theme);
//! ```
//!
//! ## Documentation
//!
//! - **`README.md`** (in this crate) — quick start, primitive guide.
//! - **`BACKEND.md`** — implementing a new render backend: mental
//!   model, the three contracts (owned data, measurer-parameterised
//!   algorithms, per-primitive contracts), two-pass paint pattern,
//!   click-intercept hierarchy, implementer checklist.
//! - **`examples/tui_demo.rs`** — runnable ratatui example that
//!   exercises the TabBar + StatusBar contracts end-to-end (cell
//!   units). `cargo run --example tui_demo`.
//! - **`examples/gtk_demo.rs`** — same demo rendered with GTK4 +
//!   Cairo + Pango (pixel units, two-pass paint). Requires the
//!   `gtk-example` feature: `cargo run --example gtk_demo
//!   --features gtk-example`.
//! - **`docs/UI_CRATE_DESIGN.md`** — full design rationale and the §10
//!   plugin invariants every primitive must honour.
//! - **`docs/DECISIONS.md`** — running log of API decisions
//!   (which primitives, why this shape, what was deferred).
//!
//! ## Status
//!
//! Pre-1.0 (`v0.1.x`). API will stabilise before publishing to crates.io.
//! All nine primitives shipped; the TUI and GTK backends are battle-tested
//! by vimcode (5000+ tests), the Win-GUI backend ships SC + explorer panel
//! migrations and is queued for tab/status/activity bar parity. macOS is
//! v1.x.
//!
//! ## Plugin invariants (briefly)
//!
//! From `docs/UI_CRATE_DESIGN.md` §10 — applies to every primitive:
//! 1. [`WidgetId`] is owned (`String`) — not `&'static str`.
//! 2. Events are plain data — no Rust closures.
//! 3. Primitives implement `Serialize + Deserialize` — Lua tables map via JSON.
//! 4. WidgetIds are namespaced (e.g. `"plugin:my-ext:send"`).
//! 5. No global event handlers — every event references a `WidgetId`.
//! 6. Primitives don't borrow app state — owned data or explicit `'a`.
//!
//! Verify all six when adding a new primitive or extending an existing one.

// #619: library code must never write to stdout/stderr — a host embedding
// a quadraui backend (vimcode's TUI, in raw mode on the alternate screen)
// owns the terminal, and a stray `eprintln!`/`println!` lands as raw bytes
// in its live cell grid, bypassing ratatui entirely. Denied here, not just
// asked for at review, so the next print macro added anywhere under `src/`
// fails the build instead of reaching a user's screen. Diagnostics route
// through `diagnostics::emit` instead (see that module).
//
// The handful of genuinely CLI-shaped call sites this deny would otherwise
// break (the GTK headless-smoke harness in `gtk/run.rs`, the macOS
// visual-confirmation dev tool in `macos/headless.rs`, a test-skip notice
// in `gtk/services.rs`) carry their own justified
// `#[allow(clippy::print_stderr)]` — they print to a human running a tool
// directly, not into a host's live UI. `examples/*` and any `bin/` are
// compiled as separate crates and are unaffected by this inner attribute;
// they're expected to print freely.
#![deny(clippy::print_stdout, clippy::print_stderr)]

pub mod diagnostics;
pub mod diff;
pub mod frame;
pub mod primitives;
pub mod shell;
pub mod terminal_style;
pub mod testing;
pub mod text_util;
pub mod theme;
pub mod types;

// ── Terminal engine (PTY + vt100 + scrollback) ───────────────────────────────
// Gated behind the `terminal` feature so non-terminal consumers don't pull in
// portable-pty / vt100.
#[cfg(feature = "terminal")]
pub mod terminal_engine;

// ── Per-backend rasterisers (#223) ──────────────────────────────────────────
// Public `draw_*` rasterisers, gated behind feature flags so apps that only
// consume the data layer don't pull in ratatui / gtk4. Lifted out of vimcode
// (`src/tui_main/quadraui_tui.rs`, `src/gtk/quadraui_gtk.rs`) one primitive
// at a time so external apps stop reimplementing the same draw functions.
#[cfg(feature = "gtk")]
pub mod gtk;
#[cfg(all(feature = "macos", target_os = "macos"))]
pub mod macos;
#[cfg(feature = "tui")]
pub mod tui;
// Unlike `macos` (target-gated in full — see that arm's comment), `win`
// stays available on every host: `src/win/{backend,run}.rs` internally
// `cfg(target_os = "windows")`-gate each real WinAPI call and fall back
// to their original `todo!()` bodies elsewhere, specifically so this
// module keeps compiling on Linux under plain `--features win` (see
// `ci.yml`'s "Compile check (win feature)" step and `Cargo.toml`'s `win`
// feature comment for why that per-repo, not per-OS, check exists).
#[cfg(feature = "win")]
pub mod win;

pub mod compose;

// ── Phase B.1: Backend trait + UiEvent + Accelerator ────────────────────────
// See quadraui/docs/BACKEND_TRAIT_PROPOSAL.md for design. These modules add
// the unified cross-backend surface alongside the existing per-backend
// free-function draw pattern; no migration yet (that's Phase B.2).
pub mod accelerator;
pub mod backend;
pub mod event;

// ── Phase B.4: cross-backend event routing ──────────────────────────────────
// ModalStack + dispatch free functions. Backends hold one ModalStack and
// call into dispatch to translate raw mouse events into Vec<UiEvent>
// without each backend reimplementing modal-precedence / backdrop-dismiss.
pub mod dispatch;
pub mod modal_stack;

// ── Phase B.5e: runner-crate API ────────────────────────────────────────────
// AppLogic trait + Reaction enum that per-backend `run<A: AppLogic>(app)`
// runners (in `quadraui::tui::run`, `quadraui::gtk::run`) drive against.
// See `docs/BACKEND_SETUP_AUDIT.md` (#260) for design rationale.
pub mod runner;
// `ShellAdapter` is constructed only by the TUI/GTK/macOS/Win-GUI shell
// runners (`crate::tui::shell_runner`, `crate::gtk::shell_runner`,
// `crate::macos::shell_runner`, `crate::win::shell_runner`, #465 + #707);
// under a feature set with none of those runners nothing builds one, so
// the whole module — struct, impls, and its private helpers — goes
// dead-code under `-D warnings` (#540). Gate the module on the features
// that actually consume it rather than `#[allow(dead_code)]`-ing the
// individual items.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
pub mod shell_adapter;

// Shared runner plumbing (`EventOutcome`, `ReactionSink` + `apply_outcome`,
// `ResizeDebouncer`) used by `tui::run`, `gtk::run`, `macos::run`
// (quadraui#496), and `win::run` (#707 — `EventOutcome` only; `win` has no
// `ReactionSink`/GTK-or-macOS-style window handle to apply an outcome to,
// so `ReactionSink`/`apply_outcome` stay cfg'd to their original three).
// `win` joins this gate (unlike `shell_adapter` above, whose consumer set
// this comment used to claim was identical) so a `win`-only build doesn't
// trip `-D warnings`' dead-code lint on `EventOutcome`.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
mod runtime;

// Shared, backend-neutral desktop-interaction plumbing (#498): window-drag
// arm/threshold/commit, modal-pump re-entrancy guard, headless smoke-mode
// predicates, PointerShape enum-walk scaffold. Compiled unconditionally —
// no toolkit dependency of its own — so a brand-new backend gets it for
// free. See `desktop`'s module doc and `BACKEND.md` §10.
mod desktop;

// Shared text-selection state machine (#741): the region registry plus
// active-selection tracking every `text_selection: true` backend embeds.
// Compiled unconditionally — no toolkit dependency — same posture as
// `desktop` above. See `text_selection`'s module doc.
mod text_selection;

pub use diff::compute_hunks;
pub use primitives::activity_bar::{
    ActivityBar, ActivityBarEvent, ActivityBarHit, ActivityBarLayout, ActivityBarRowHit,
    ActivityBarStyle, ActivityItem, ActivitySide, VisibleActivityItem,
};
pub use primitives::board::{
    board_layout, BadgeStatus, BoardAction, BoardCard, BoardColumn, BoardHit, BoardLayout,
    BoardMeasure, BoardModel, CardBadge, CardId, CardLayout, ColumnLayout, MoveDir,
};
pub use primitives::chart::{
    Chart, ChartEvent, ChartHit, ChartKind, ChartLayout, ChartMeasure, Series,
};
pub use primitives::command_center::{
    CommandCenter, CommandCenterHit, CommandCenterLayout, CommandCenterMeasure,
};
pub use primitives::command_line::{CommandLine, CommandLineLayout, CommandLineMeasure};
pub use primitives::completions::{
    CompletionItem, CompletionItemMeasure, CompletionKind, Completions, CompletionsHit,
    CompletionsLayout, CompletionsPlacement, VisibleCompletion,
};
pub use primitives::context_menu::{
    ContextMenu, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure, ContextMenuLayout,
    ContextMenuPlacement, ResolvedContextMenuPlacement, VisibleContextMenuItem,
};
pub use primitives::data_table::{
    Column, ColumnAlign, ColumnMeasure, ColumnWidth, DataRow, DataTable, DataTableEvent,
    DataTableHit, DataTableLayout, ResolvedColumn, SortDirection,
};
pub use primitives::dialog::{
    native_dialog_options, Dialog, DialogButton, DialogHit, DialogInput, DialogLayout,
    DialogMeasure, DialogSeverity, DialogTable, DialogTextInput, VisibleDialogButton,
};
pub use primitives::diff_view::{
    DiffDisplayLine, DiffEditability, DiffHeaderGeometry, DiffHunk, DiffLineContent, DiffMode,
    DiffPane, DiffPaneGeometry, DiffRow, DiffRowKind, DiffView, DiffViewGeometry, DiffViewLayout,
};
pub use primitives::drop_zone::{
    compute_drop_zone, drop_zone_overlay, DropEdge, DropGroupRect, DropOverlay, DropZone,
    DropZoneKind,
};
pub use primitives::editor::{
    CursorPos as EditorCursorPos, CursorShape as EditorCursorShape, DiagnosticMark,
    DiagnosticSeverity, DiffLine, Editor, EditorCursor, EditorHit, EditorLayout, EditorLine,
    EditorSelection, GitLineStatus, SelectionKind as EditorSelectionKind, SpellMark,
    Style as EditorStyle, StyledSpan as EditorStyledSpan,
};
pub use primitives::find_replace::{
    compute_hit_regions as compute_find_replace_hit_regions, FindReplaceClickTarget,
    FindReplacePanel, FrHitRegion, FR_PANEL_WIDTH,
};
pub use primitives::form::{
    ButtonRowItem, FieldKind, Form, FormEvent, FormField, FormFieldMeasure, FormHit,
    FormItemMeasure, FormLayout, ToggleGroupItem, ValidationState, VisibleFormField,
};
pub use primitives::image::{Image, ImageFit, ImageLayout, ImageSource};
pub use primitives::list::{
    ListItem, ListItemMeasure, ListView, ListViewEvent, ListViewHit, ListViewLayout,
    VisibleListItem,
};
pub use primitives::menu_bar::{
    MenuBar, MenuBarHit, MenuBarItem, MenuBarItemMeasure, MenuBarLayout, VisibleMenuBarItem,
};
pub use primitives::message_list::{MessageList, MessageRow};
pub use primitives::minimap::{
    aggregate_spans, reserved_width, sample_lines, Minimap, MinimapGrid, MinimapHit, MinimapLayout,
    MinimapLine, MinimapSizing, MinimapSpan, SyntaxSpan, VisibleMinimapLine,
};
pub use primitives::multi_section_view::{
    ActionId as MsvActionId, AuxHit, Axis as MsvAxis, DividerBounds, EmptyBody, HeaderAction,
    HeaderHit, InlineInput, LayoutMetrics as MsvLayoutMetrics, MultiSectionView,
    MultiSectionViewHit, MultiSectionViewLayout, ScrollMode, ScrollbarHit, Section, SectionAux,
    SectionBody, SectionHeader, SectionId, SectionLayout, SectionMeasure, SectionSize,
};
pub use primitives::palette::{
    Palette, PaletteEvent, PaletteHit, PaletteItem, PaletteItemMeasure, PaletteLayout, PaletteMode,
    PalettePreview, PaletteScrollbar, VisiblePaletteItem,
};
pub use primitives::panel::{
    Panel, PanelAction, PanelHit, PanelLayout, PanelMeasure, VisiblePanelAction,
};
pub use primitives::pipeline_view::{
    PipelineEvent, PipelineHit, PipelineStage, PipelineView, PipelineViewLayout,
    PipelineViewMeasure, StageBounds, StageStatus,
};
pub use primitives::progress::{
    ProgressBar, ProgressBarHit, ProgressBarLayout, ProgressBarMeasure,
};
pub use primitives::rich_text_popup::{
    PopupPlacement, PopupScrollbar, RichTextLink, RichTextPopup, RichTextPopupHit,
    RichTextPopupLayout, RichTextPopupMeasure, TextSelection, VisibleRichTextLine,
};
pub use primitives::scrollbar::{fit_thumb, ScrollAxis, Scrollbar};
pub use primitives::sidebar_panel::{
    SidebarPanel, SidebarPanelHit, SidebarPanelLayout, SidebarPanelMeasure,
};
pub use primitives::spinner::{Spinner, SpinnerHit, SpinnerLayout, SpinnerMeasure};
pub use primitives::split::{Split, SplitDirection, SplitHit, SplitLayout, SplitMeasure};
pub use primitives::split_tree::{
    SplitTree, SplitTreeDivider, SplitTreeLayout, SplitTreeMeasure,
    MAX_RATIO as SPLIT_TREE_MAX_RATIO, MIN_RATIO as SPLIT_TREE_MIN_RATIO,
};
pub use primitives::status_bar::{
    StatusBar, StatusBarEvent, StatusBarHit, StatusBarHitRegion, StatusBarLayout, StatusBarSegment,
    StatusSegmentMeasure, StatusSegmentSide, VisibleStatusSegment,
};
pub use primitives::tab_bar::{
    tab_icon_at, tab_icon_cols, SegmentMeasure, TabBar, TabBarEvent, TabBarHit, TabBarHits,
    TabBarLayout, TabBarSegment, TabChrome, TabFrame, TabIcon, TabItem, TabMeasure, VisibleSegment,
    VisibleTab,
};
pub use primitives::terminal::{
    Terminal, TerminalCell, TerminalCellSize, TerminalEvent, TerminalHit, TerminalLayout,
    TerminalScrollbar, TerminalSplitHit, TerminalSplitLayout,
};
pub use primitives::text_display::{
    TextDisplay, TextDisplayEvent, TextDisplayHit, TextDisplayLayout, TextDisplayLine,
    TextDisplayLineMeasure, VisibleTextDisplayLine,
};
pub use primitives::text_input::{
    TextInput, TextInputHit, TextInputLayout, TextInputMeasure, VisibleTextInputLine,
};
pub use primitives::toast::{
    ToastAction, ToastCorner, ToastHit, ToastItem, ToastMeasure, ToastSeverity, ToastStack,
    ToastStackLayout, VisibleToast,
};
pub use primitives::toolbar::{
    Toolbar, ToolbarButton, ToolbarHit, ToolbarItemKind, ToolbarItemMeasure, ToolbarLayout,
    VisibleToolbarItem,
};
pub use primitives::tooltip::{
    ResolvedPlacement, Tooltip, TooltipBorder, TooltipChrome, TooltipHit, TooltipLayout,
    TooltipMeasure, TooltipPlacement,
};
pub use primitives::tree::{
    TreeEvent, TreeRow, TreeRowEditState, TreeRowMeasure, TreeView, TreeViewHit, TreeViewLayout,
    VisibleTreeRow,
};
pub use text_util::{
    fuzzy_score, next_char_boundary, prev_char_boundary, safe_prefix, safe_slice,
    snap_to_char_boundary, word_wrap,
};
pub use theme::Theme;
pub use types::{
    Badge, Color, Decoration, Icon, Modifiers, SelectionMode, StyledSpan, StyledText, TreePath,
    TreeStyle, WidgetId,
};

// Phase B.1 re-exports.
pub use accelerator::{
    parse_key_binding, render_accelerator, render_binding, Accelerator, AcceleratorId,
    AcceleratorScope, KeyBinding, ParsedBinding, Platform,
};
pub use backend::{
    Backend, BackendCaps, Clipboard, FileDialogOptions, MessageDialogButton, MessageDialogChoice,
    MessageDialogOptions, Notification, PlatformServices, PointerShape, ResizeEdge,
};
pub use event::{
    mouse_down, mouse_moved, mouse_up, scroll, window_resized, BackendNativeEvent, ButtonMask, Key,
    MouseButton, NamedKey, Point, Rect, ScrollDelta, UiEvent, Viewport,
};
pub use frame::{
    check_frame_order, compose_frame, FrameHitMap, FrameOrderViolation, FramePresence, FrameRung,
    FrameZone, ScreenLayout, Surface,
};
pub use shell::{ShellApp, ShellConfig, ShellContext};

// Phase B.4 re-exports.
pub use compose::markdown::{
    render_markdown_to_styled, render_markdown_to_styled_wrapped, CodeBlockRange, RenderedMarkdown,
};
pub use compose::{
    filter_help_actions, help_actions_to_palette_items, AppShell, AppShellEvent, AppShellLayout,
    BackendWidget, BottomPanelConfig, BottomPanelController, BottomPanelEvent, BottomPanelLayout,
    BottomPanelTab, ChatController, ChatControllerEvent, ChatRole, ChatTurn,
    DualModePaletteController, DualModePaletteEvent, FocusGroup, FocusRing, FolderPickerController,
    FolderPickerEvent, FormController, FormControllerEvent, GroupLayout, HelpAction, HelpNote,
    HelpOverlayController, HelpOverlayEvent, HelpRegistry, KeyContext, KeyMap, MenuDef, MenuEvent,
    MenuSystem, NavigationMode, Pane, PaneDragRect, PaneTab, PanelDefinition, SectionKind,
    ShellPosition, SidebarEvent, SidebarSectionDef, SidebarSystem, StatusBarAction,
    StatusBarInteraction, TabGroupController, TabGroupEvent, TabGroupLayout, ToolbarHoverTracker,
    TreeController, TreeControllerEvent, ViewHelp, WorkspaceController, WorkspaceDoc,
    WorkspaceEvent, WorkspaceLayout, PALETTE_CHROME_ROWS,
};
pub use dispatch::{
    dispatch_click, dispatch_mouse_down, dispatch_mouse_drag, dispatch_mouse_up, dispatch_scroll,
    text_selection_line_range, DragState, DragTarget, ScrollSurface, SurfaceScrollbar, TextRegion,
};
pub use modal_stack::{ModalEntry, ModalStack};
pub use runner::{AppLogic, Reaction};

/// Crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_cargo_toml() {
        assert_eq!(VERSION, "0.0.1");
    }
}
