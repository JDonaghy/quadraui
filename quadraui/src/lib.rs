//! # quadraui
//!
//! Cross-platform UI primitives for keyboard-driven desktop and terminal apps.
//!
//! Targets four rendering backends with a single declarative API:
//! - **TUI** (ratatui + crossterm) — full feature parity, works everywhere.
//! - **Linux** (GTK4 + Cairo + Pango) — `gtk4`, full feature parity.
//! - **macOS** (Core Graphics + Core Text) — `objc2`/AppKit. Every
//!   in-window rasteriser shipped; see the crate root `README.md`'s
//!   *Status* section for the two documented divergences from GTK.
//! - **Windows** (Direct2D + DirectWrite) — `windows-rs`. Window creation,
//!   event translation, and platform services are real and exercised by
//!   blocking CI on `windows-latest`; most per-primitive rasterisers are
//!   still `todo!()` stubs, tracked as a non-gating "burn-down" column in
//!   the conformance matrix (see `tests/conformance.rs` and issue #708).
//!
//! ## What's in the box
//!
//! 40 primitives (one module each under `src/primitives/`), each
//! declarative + serde-friendly so apps and Lua plugins can describe UI
//! as data. A representative sample:
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
//! See the crate root `README.md`'s *Primitives* section for the fuller
//! (still not exhaustive) list, or `src/primitives/mod.rs` for the
//! authoritative one.
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
//! // Backend (one of the four in-tree ones — see "Backend implementors"
//! // below) — measure + paint:
//! draw_status_bar(cr, &bar, &theme);
//! ```
//!
//! ## Backend implementors
//!
//! [`Backend`] is `pub` but **sealed** — TUI, GTK, Win-GUI, and macOS are
//! the only implementations that will ever exist, and the trait enforces
//! that rather than just documenting it (quadraui#800). See [`Backend`]'s
//! own rustdoc for the mechanism (a private supertrait, plus a
//! `compile_fail` doctest proving an out-of-crate `impl Backend` doesn't
//! compile) and the reasoning: a required method can be added to
//! `Backend` in an ordinary PR, with no default and no version bump,
//! whenever a new primitive ships — sealing is what makes that safe
//! rather than a silent trap for a downstream implementor. Want to
//! render onto a target none of the four cover? Contribute a fifth
//! in-tree backend — see `BACKEND.md` and `docs/BACKEND.md`.
//!
//! ## Documentation
//!
//! - **`README.md`** (repository root — this crate is a workspace member,
//!   not a standalone `cargo package` with its own README) — quick start,
//!   full primitive list, per-backend status.
//! - **`BACKEND.md`** — contributing a new in-tree render backend: mental
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
//! - **`docs/UI_CRATE_DESIGN.md`** — original design sketch and the §10
//!   plugin invariants every primitive must honour. Predates
//!   implementation (see its own status banner) — treat it as a decision
//!   record, not a live status page.
//! - **`docs/decisions/DECISIONS.md`** — running log of API decisions
//!   (which primitives, why this shape, what was deferred).
//!
//! ## Status
//!
//! `0.0.x` — pre-1.0, not yet published to crates.io, breaking changes
//! allowed. 40 primitives shipped. The TUI and GTK backends have full
//! feature parity and are battle-tested by vimcode (5000+ tests). The
//! macOS backend implements the whole `Backend` trait and is built/tested
//! for real on `macos-latest` CI. The Windows backend's window/event/
//! platform-services infrastructure is real and CI-blocking on
//! `windows-latest`; most per-primitive rasterisers are still unwritten
//! (`todo!()` stubs), tracked as a non-gating conformance-matrix column.
//! See the root `README.md`'s *Status* section for the specifics and
//! issue links.
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
// docs.rs builds every crate with `--cfg docsrs` (nightly rustdoc) so that
// feature-gated items can be annotated with their requirement. `doc(cfg(..))`
// itself is a nightly-only rustdoc attribute, so it's spelled behind
// `cfg_attr(docsrs, ...)` everywhere it's used below — a plain build (this
// repo's own `cargo doc`, on stable, with no `--cfg docsrs`) never sees the
// attribute at all, so this compiles unchanged on stable. See issue #797 and
// `[package.metadata.docs.rs]` in `Cargo.toml`, which is what sets
// `--cfg docsrs` (via `rustdoc-args`) for the published build.
#![cfg_attr(docsrs, feature(doc_cfg))]
// #797: `cargo doc --no-deps -D warnings` is a CI gate (see `ci.yml`'s `doc`
// job). Two rustdoc lints below are pre-existing debt across ~38 files
// (measured 2026-09-06: ~185 warnings, mostly stale intra-doc links to
// renamed/removed methods, plus doc comments on public re-exports that link
// to private helpers by relative path) that predates this gate and is out of
// scope for the release-metadata work that added it (issue #797) — fixing it
// crate-wide is its own follow-up. Allowed here, narrowly, rather than
// disabling `-D warnings` for the whole doc build: every other rustdoc lint
// (`rustdoc::bare_urls`, `rustdoc::invalid_html_tags`, `missing_docs`-style
// checks, etc.) still fails the build. Do not widen this allow list without
// updating this comment.
#![allow(rustdoc::broken_intra_doc_links, rustdoc::private_intra_doc_links)]

pub mod diagnostics;
pub mod diff;
pub mod frame;
pub mod layout;
pub mod prelude;
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
#[cfg_attr(docsrs, doc(cfg(feature = "terminal")))]
pub mod terminal_engine;

// ── Per-backend rasterisers (#223) ──────────────────────────────────────────
// Public `draw_*` rasterisers, gated behind feature flags so apps that only
// consume the data layer don't pull in ratatui / gtk4. Lifted out of vimcode
// (`src/tui_main/quadraui_tui.rs`, `src/gtk/quadraui_gtk.rs`) one primitive
// at a time so external apps stop reimplementing the same draw functions.
#[cfg(feature = "gtk")]
#[cfg_attr(docsrs, doc(cfg(feature = "gtk")))]
pub mod gtk;
#[cfg(all(feature = "macos", target_os = "macos"))]
#[cfg_attr(docsrs, doc(cfg(feature = "macos")))]
pub mod macos;
#[cfg(feature = "tui")]
#[cfg_attr(docsrs, doc(cfg(feature = "tui")))]
pub mod tui;
// Unlike `macos` (target-gated in full — see that arm's comment), `win`
// stays available on every host: `src/win/{backend,run}.rs` internally
// `cfg(target_os = "windows")`-gate each real WinAPI call and fall back
// to their original `todo!()` bodies elsewhere, specifically so this
// module keeps compiling on Linux under plain `--features win` (see
// `ci.yml`'s "Compile check (win feature)" step and `Cargo.toml`'s `win`
// feature comment for why that per-repo, not per-OS, check exists).
#[cfg(feature = "win")]
#[cfg_attr(docsrs, doc(cfg(feature = "win")))]
pub mod win;

pub mod compose;

// ── Phase B.1: Backend trait + UiEvent + Accelerator ────────────────────────
// See quadraui/docs/decisions/BACKEND_TRAIT_PROPOSAL.md for design. These modules add
// the unified cross-backend surface alongside the existing per-backend
// free-function draw pattern; no migration yet (that's Phase B.2).
pub mod accelerator;
pub mod backend;
pub mod event;

// ── NativeSurface (#807, Phase 1 of the NativeSurface milestone) ───────────
// The ~15-verb drawing trait underneath the three pixel backends —
// extracted from helpers each of GtkBackend/MacBackend/WinBackend already
// had privately. `pub(crate)`, not `pub`: purely an internal decomposition
// of `Backend`'s existing (sealed) implementors, so it adds no new public
// API surface. TUI is deliberately excluded (see `native_surface`'s module
// doc); gated the same way `text_selection` below is, on the backends that
// actually implement it, so a `tui`-only build doesn't carry a trait with
// zero implementors under `-D warnings`' dead-code lint.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod native_surface;

// Host-independent paint-geometry helpers (#857) — pure rect/inset
// arithmetic pulled out from behind `src/macos/`'s whole-module
// `target_os = "macos"` gate (mirrors `src/win/msg.rs`'s equivalent split
// for Windows), so `cargo test -p quadraui` exercises it with no
// `--features macos` and no cross-target needed. Deliberately compiled
// unconditionally — see that module's doc for why every item in it also
// carries its own `#[allow(dead_code)]`.
mod paint_geometry;

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
// (quadraui#496), and `win::run` (#707 — originally `EventOutcome` only;
// `win` has no `ReactionSink`/GTK-or-macOS-style window handle to apply an
// outcome to, so `ReactionSink`/`apply_outcome` stay cfg'd to their
// original three). `win` joins this gate (unlike `shell_adapter` above,
// whose consumer set this comment used to claim was identical) so a
// `win`-only build doesn't trip `-D warnings`' dead-code lint on
// `EventOutcome`. `macos` and `win` both adopt `ResizeDebouncer` too as of
// #780 — see that struct's doc for why they need it and GTK doesn't.
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

// Shared text-selection state machine (#741, macOS adopted in #803): the
// region registry plus active-selection tracking every
// `text_selection: true` backend embeds. No toolkit dependency of its
// own, but — unlike `desktop` above — it is gated on the backends that
// actually embed it (`TuiBackend`, `GtkBackend`, `WinBackend`,
// `MacBackend`) rather than compiled unconditionally. Before #803, macOS
// was the one backend that did *not* declare `BackendCaps::text_selection`,
// so a `--features macos` build had no consumer for any of it, and with
// `RUSTFLAGS: -D warnings` the whole module landed as a hard `dead_code`
// error rather than a warning — which is exactly how the `macos (build,
// test)` job failed the first time a PR touched `quadraui/src/macos/**`
// after #741 (that job is `paths`-filtered, so nothing ran it in
// between). If a future backend doesn't adopt this module, mirror that
// same fix rather than compiling it unconditionally.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
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
    Backend, BackendCaps, BackendError, Clipboard, FileDialogOptions, MessageDialogButton,
    MessageDialogChoice, MessageDialogOptions, Notification, PlatformServices, PointerShape,
    ResizeEdge, ServiceResult,
};
pub use event::{
    mouse_down, mouse_moved, mouse_up, scroll, window_resized, BackendNativeEvent, ButtonMask, Key,
    MouseButton, NamedKey, Point, Rect, ScrollDelta, UiEvent, Viewport,
};
pub use frame::{
    check_frame_order, compose_frame, FrameHitMap, FrameOrderViolation, FramePresence, FrameRung,
    FrameZone, ScreenLayout, Surface,
};
// #816: shared layout/hit-test foundation (`Anchor` for overlay
// positioning, `visible_range_walk` replacing the per-primitive
// `Visible*{idx, bounds}` structs). Nothing in `primitives/` consumes
// these yet — each primitive converges onto them in its own PR behind a
// `#[deprecated]` shim per `docs/PRIMITIVE_RULES.md` rule 8.
pub use layout::{visible_range_walk, Anchor, Axis, ResolvedSide, Side, VisibleItem};
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
