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
//!   blocking CI on `windows-latest`, and every per-primitive rasteriser
//!   is shipped; its conformance-matrix column gates like every other
//!   backend's (see `tests/conformance.rs`).
//!
//! ## What's in the box
//!
//! 59 primitive modules (one each under `src/primitives/`) — widgets,
//! each declarative + serde-friendly so apps and Lua plugins can
//! describe UI as data, plus the `layout_metrics` and `row_stack`
//! shared-math helpers the widget modules reuse. A representative
//! sample:
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
//! // below) — measure + paint through the widget contract:
//! backend.draw_dyn(rect, &bar, &());
//! ```
//!
//! ## Backend implementors
//!
//! [`Backend`] is `pub` but **sealed** — TUI, GTK, Win-GUI, and macOS are
//! the only implementations that will ever exist, and the trait enforces
//! that rather than just documenting it. See [`Backend`]'s
//! own rustdoc for the mechanism (a private supertrait, plus a
//! `compile_fail` doctest proving an out-of-crate `impl Backend` doesn't
//! compile) and the reasoning: a required method can be added to
//! `Backend` in an ordinary PR, with no default and no version bump,
//! whenever a new primitive ships — sealing is what makes that safe
//! rather than a silent trap for a downstream implementor. Want to
//! render onto a target none of the four cover? Contribute a fifth
//! in-tree backend — see `BACKEND.md`.
//!
//! Each backend's module (`gtk`, `macos`, `tui`, `win`) exposes a small,
//! documented stable surface — the backend struct itself, `events`,
//! `services`, `shell_runner`, `testing`, and (where it exists) `text` —
//! plus a flat re-export of the handful of free-function rasterisers an
//! app calls directly (`mac_tree_layout`, …). Everything
//! else is a per-primitive painter internal: `#[doc(hidden)]` where it
//! must stay `pub mod` for compatibility (see `src/macos/mod.rs`), or
//! simply private where nothing outside the backend needs the submodule
//! path at all (`src/{gtk,tui,win}/mod.rs`). Either way it is unstable —
//! not covered by semver — so painter internals can be split, merged, or
//! moved between sibling modules without a version bump.
//!
//! ## Documentation
//!
//! Every file named below ships in the published `.crate` — this list
//! deliberately names only files a consumer of the published crate can
//! actually open.
//!
//! - **`README.md`** (repository root) — quick start, full primitive list,
//!   per-backend status.
//! - **`docs/GUIDE.md`** — build a small two-pane app end to end, from
//!   `cargo new` to something you can run and click on.
//! - **`docs/APP_ARCHITECTURE.md`** — where app state lives vs. what
//!   quadraui owns; read before architecting a consumer app on top of it.
//! - **`docs/CONSUMER_PATTERNS.md`** — recipes for common app shapes
//!   (debug sidebars, source-control panels) built from these primitives.
//! - **`BACKEND.md`** — contributing a new in-tree render backend: mental
//!   model, the three contracts (owned data, measurer-parameterised
//!   algorithms, per-primitive contracts), two-pass paint pattern,
//!   click-intercept hierarchy, implementer checklist.
//! - **`examples/tui_demo.rs`** — runnable ratatui example that
//!   exercises the TabBar + StatusBar contracts end-to-end (cell
//!   units). `cargo run --example tui_demo`.
//! - **`examples/gtk_demo.rs`** — same demo rendered with GTK4 +
//!   Cairo + Pango (pixel units, two-pass paint). Requires the
//!   `gtk` feature: `cargo run --example gtk_demo --features gtk`.
//! - **`docs/decisions/DECISIONS.md`** — running log of API decisions
//!   (which primitives, why this shape, what was deferred), including
//!   D-019, the stability policy for what a `0.x` release may change.
//!
//! ## Status
//!
//! `0.11.x` — pre-1.0, published on crates.io. Breaking changes are
//! grouped into scheduled minor releases and recorded in `CHANGELOG.md`;
//! what a `0.x` release may change is D-019 in
//! `quadraui/docs/decisions/DECISIONS.md`. 59 primitive modules
//! shipped. The TUI and GTK backends have full feature parity and are
//! battle-tested by vimcode (5000+ tests). The macOS backend
//! implements the whole `Backend` trait and is built/tested for real
//! on `macos-latest` CI. The Windows backend's window/event/platform-
//! services infrastructure is real and CI-blocking on
//! `windows-latest`, and every per-primitive rasteriser is shipped
//! too, with no `todo!()` left in `src/win/backend.rs` — builds, tests
//! and the cross-backend conformance matrix are all blocking on
//! `windows-latest` CI. See the root `README.md`'s *Status* section
//! for the specifics and issue links.
//!
//! ## Plugin invariants (briefly)
//!
//! Applies to every primitive:
//! 1. [`WidgetId`] is owned (`String`) — not `&'static str`.
//! 2. Events are plain data — no Rust closures.
//! 3. Primitives implement `Serialize + Deserialize` — Lua tables map via JSON.
//! 4. WidgetIds are namespaced (e.g. `"plugin:my-ext:send"`).
//! 5. No global event handlers — every event references a `WidgetId`.
//! 6. Primitives don't borrow app state — owned data or explicit `'a`.
//!
//! Verify all six when adding a new primitive or extending an existing one.

// Library code must never write to stdout/stderr — a host embedding
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
// in `gtk/services.rs`, the stale-GUI-session activation hint both macOS
// backends print via `macos_session_hint::activation_hint`, the `--json`
// Level 2 command helper in `control::cli::run`) carry their own justified
// `#[allow(clippy::print_stdout, clippy::print_stderr)]` — they print to a
// human running a tool directly, not into a host's live UI. `examples/*`
// and any `bin/` are compiled as separate crates and are unaffected by
// this inner attribute; they're expected to print freely.
#![deny(clippy::print_stdout, clippy::print_stderr)]
// `doc(cfg(..))` is a nightly-only rustdoc attribute; docs.rs builds with
// `--cfg docsrs` so feature-gated items can be annotated. A plain build on
// stable never sees this attribute and compiles unchanged.
#![cfg_attr(docsrs, feature(doc_cfg))]
// `cargo doc --no-deps -D warnings` is a CI gate (see `ci.yml`'s `doc`
// job). Two rustdoc lints below are pre-existing debt across ~38 files
// (measured 2026-09-06: ~185 warnings, mostly stale intra-doc links to
// renamed/removed methods, plus doc comments on public re-exports that link
// to private helpers by relative path) that predates this gate and is out of
// scope for the release-metadata work that added it — fixing it
// crate-wide is its own follow-up. Allowed here, narrowly, rather than
// disabling `-D warnings` for the whole doc build: every other rustdoc lint
// (`rustdoc::bare_urls`, `rustdoc::invalid_html_tags`, `missing_docs`-style
// checks, etc.) still fails the build. Do not widen this allow list without
// updating this comment.
#![allow(rustdoc::broken_intra_doc_links, rustdoc::private_intra_doc_links)]

/// Implements `schemars::JsonSchema` for a type whose serde impl is
/// hand-written, by describing it with the schema of the type its wire
/// form actually has.
#[cfg(feature = "schema")]
macro_rules! schema_as {
    ($ty:ty, $name:literal, $wire:ty) => {
        impl schemars::JsonSchema for $ty {
            fn schema_name() -> ::std::borrow::Cow<'static, str> {
                $name.into()
            }

            fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
                <$wire as schemars::JsonSchema>::json_schema(generator)
            }
        }
    };
}
#[cfg(feature = "schema")]
pub(crate) use schema_as;

// Not `pub mod`, and not under `primitives/`: `A11yInfo` is shared
// accessibility groundwork, not a widget primitive — no `Layout`, no
// `hit_test`, no `Backend::draw_*` method — so it doesn't belong in the
// primitive count `readme_truth.rs`'s `lib_doc_states_the_real_primitive_count`
// / `root_readme_states_the_real_primitive_count` derive from
// `primitives/mod.rs`'s `pub mod` list, nor is it exposed as its own
// public module path. Re-exported below as `crate::A11yInfo`, its typed
// role/state types (`A11yRole`, `A11yChecked`, `A11yLive`,
// `A11yValueRange`), and the tree builder (`A11yTreeBuilder` and
// friends).
mod a11y;
// Not under `primitives/`, same reasoning as `font_role`/`generic_font`
// below: a blink state machine is not a widget — no `Layout`, no
// `hit_test`, no `Backend::draw_*` method of its own — so it must not
// inflate the primitive count `readme_truth.rs` derives from
// `primitives/mod.rs`'s `pub mod` list.
pub mod caret_blink;
// Not under `primitives/`, same reasoning as `font_role`/`generic_font`
// above: a handle-addressed value store is not a widget — no `Layout`, no
// `hit_test`, no `Backend::draw_*` method of its own — so it must not
// inflate the primitive count `readme_truth.rs` derives from
// `primitives/mod.rs`'s `pub mod` list.
pub mod descriptor_store;
pub mod diagnostics;
pub mod diff;
// Not under `primitives/`, same reasoning as `font_role`/`generic_font`
// below: a serializable rule AST is not a widget — no `Layout`, no
// `hit_test`, no `Backend::draw_*` method of its own — so it must not
// inflate the primitive count `readme_truth.rs` derives from
// `primitives/mod.rs`'s `pub mod` list. Binding-neutral by design: see the
// module doc for why the `Expr`/`Value` types, not a Rust closure, are the
// thing every Lua/Python/C-ABI binding emits.
pub mod expr;
pub mod focus;
// Not under `primitives/`, for the same reason `a11y` isn't (see the
// comment above `mod a11y`): `font_role` classifies *which font* each
// primitive paints in. It has no `Layout`, no `hit_test`
// and no `Backend::draw_*` method of its own, so it is not a widget
// primitive and must not inflate the primitive count
// `readme_truth.rs`'s `lib_doc_states_the_real_primitive_count` /
// `root_readme_states_the_real_primitive_count` derive from
// `primitives/mod.rs`'s `pub mod` list. It stays a `pub mod` (unlike
// `a11y`) because its module doc *is* the chrome-vs-editor policy every
// pixel backend's `draw_*` methods are written against.
pub mod font_role;
// Not under `primitives/`, same reasoning as `font_role` above:
// `GenericFamily` classifies a font *request*, not a
// widget — no `Layout`, no `hit_test`, no `Backend::draw_*` method of
// its own — so it must not inflate the primitive count `readme_truth.rs`
// derives from `primitives/mod.rs`'s `pub mod` list.
pub mod frame;
pub mod generic_font;
pub mod interaction;
pub mod layout;
// Not under `primitives/`, same reasoning as `font_role`/`generic_font`
// above: app-identity-scoped filesystem locations are not a widget — no
// `Layout`, no `hit_test`, no `Backend::draw_*` method of its own — so this
// must not inflate the primitive count `readme_truth.rs` derives from
// `primitives/mod.rs`'s `pub mod` list.
pub mod paths;
pub mod prelude;
pub mod primitives;
// Pure cursor/selection arithmetic `primitives::cell_grid::CellGrid`'s
// `Interactive` impl needs against the row/column grid — crate-private
// since `CellGrid`'s own public shape doesn't change; its internals call
// through it instead of hand-rolling the same geometry.
pub(crate) mod cell_grid_nav;
pub mod shell;
pub mod style;
pub mod terminal_style;
pub mod testing;
pub mod text_util;
pub mod theme;
// Not under `primitives/`, same reasoning as `font_role`/`generic_font`
// above: a time-based progress helper is not a widget — no `Layout`, no
// `hit_test`, no `Backend::draw_*` method of its own — so it must not
// inflate the primitive count `readme_truth.rs` derives from
// `primitives/mod.rs`'s `pub mod` list.
pub mod transition;
pub mod types;
pub mod undo;
// Not under `primitives/`, same reasoning as `expr` above: declarative
// form-validation rules are not a widget — no `Layout`, no `hit_test`, no
// `Backend::draw_*` method of its own; they compute the `ValidationState`
// that `primitives::form::FormField::validation` already renders — so this
// must not inflate the primitive count `readme_truth.rs` derives from
// `primitives/mod.rs`'s `pub mod` list. Built on `expr::Expr` for
// cross-field rules, so it stays binding-neutral the same way: plain
// serializable data, not a Rust callback.
pub mod validation;
// Not under `primitives/`, same reasoning as `validation` above: declarative
// show/hide and enable/disable rules are not a widget — no `Layout`, no
// `hit_test`, no `Backend::draw_*` method of its own; they decide which
// `primitives::form::FormField`s `compose::FormController` keeps in its
// rendered `Form` and whether each is `disabled` — so this must not inflate
// the primitive count `readme_truth.rs` derives from `primitives/mod.rs`'s
// `pub mod` list. Built on `expr::Expr`, so it stays binding-neutral the
// same way: plain serializable data, not a Rust callback.
pub mod visibility;

// ── Terminal engine (PTY + vt100 + scrollback) ───────────────────────────────
// Gated behind the `terminal` feature so non-terminal consumers don't pull in
// portable-pty / vt100.
#[cfg(feature = "terminal")]
#[cfg_attr(docsrs, doc(cfg(feature = "terminal")))]
pub mod terminal_engine;

// Small LRU cache for decoded image rasterisations, shared by every
// surface's `PaintSurface::surface_draw_image` (and TUI's graphics
// channel). Crate-internal — only backend modules need it. `gtk` needs no
// `target_os` gate (gdk_pixbuf runs anywhere); `macos` and `win` mirror
// their image modules' own `target_os` gates, so a cross-platform
// `cargo check` never compiles the cache without a call site and fails
// `-D warnings` on dead code. `tui` alone paints `Image::fallback_text`
// and needs no cache; `tui-images` keys `tui::image_decode`'s decode
// cache on the same `(ImageSource, target size, scale)` shape, with
// `tui::image_decode::DecodedImage` as the cached value type.
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows"),
    feature = "tui-images"
))]
pub(crate) mod image_cache;

// Shared "macOS never activated this window" stderr diagnostic —
// `macos::run::QuadraAppDelegate`'s `applicationDidFinishLaunching:` and
// `gtk::run`'s post-`present()` macOS check both call
// `macos_session_hint::activation_hint`. Both call sites live inside a
// `cfg(target_os = "macos")` block of their own (`macos::run` is
// target-gated in full; `gtk::run`'s is the inner block alongside
// `activate_macos_process`), so this module is gated the same way —
// `feature = "gtk"` alone (any other target) or `feature = "macos"` alone
// (never true off `target_os = "macos"`, see `mod macos`'s own gate below)
// would leave it compiled with no caller and fail `-D warnings` as dead
// code.
#[cfg(all(any(feature = "gtk", feature = "macos"), target_os = "macos"))]
pub(crate) mod macos_session_hint;

// Bundled codicon icon font: the PUA codepoints + font bytes every GUI
// backend self-registers so built-in chrome glyphs (tree chevrons, tab
// dirty/close, the context-menu submenu arrow, the data-table sort
// arrow) paint as real vector icons instead of plain text glyphs, with
// no app configuration required. `win` needs no `target_os` gate for
// the same reason `win::text`/`win::backend` don't — only the real
// WinAPI/DirectWrite calls built on these constants are
// `cfg(target_os = "windows")`-gated, so this module itself must
// type-check on every host under `--features win`. TUI is excluded: it
// keeps painting the Unicode glyphs it always has, so TUI output stays
// unchanged.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) mod codicon;

// Codicon name → codepoint table backing `Icon::codicon`. Unlike
// `codicon` above (the font bytes + chrome-glyph constants, needed only
// by GUI rasterisers), this is plain data `Icon::codicon` resolves on
// every build — including a TUI-only or zero-backend-feature one — so
// it carries no feature gate. See `codicon_names.rs`'s module doc.
pub(crate) mod codicon_names;

// ── Per-backend rasterisers ─────────────────────────────────────────────────
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

// Drag-out-to-the-OS seam: the `DragOutPayload` type
// `PlatformServices::begin_drag_out` trades in. Compiled unconditionally
// — the contract and its `Unsupported` default need no backend feature;
// see `drag`'s module doc.
pub mod drag;
pub mod event;

// Power monitor + sleep-blocker seam: the `PowerSaveKind`/`BlockerId`/
// `ThermalState`/`PowerEventKind` types `PlatformServices`'
// battery/thermal queries and sleep-blocker methods, and
// `UiEvent::PowerStateChanged`, trade in. Compiled unconditionally — the
// contract and its `Unsupported` default need no backend feature; see
// `power`'s module doc.
pub mod power;

// Launch-at-login seam: `LoginItemOptions`, the type
// `PlatformServices::set_login_item` trades in, reached off
// `PlatformServices::set_login_item`/`PlatformServices::login_item_enabled`
// the same way `power`'s types sit alongside its `PlatformServices`
// methods above. Compiled unconditionally — the contract and its
// `Unsupported` default need no backend feature; see `login_item`'s
// module doc.
pub mod login_item;

// Crash reporter seam: `CrashReporterConfig`/`CrashRecord`, the types
// `install_crash_handler` trades in. A standalone free function, not a
// `PlatformServices` method — a panic hook is identical logic on every
// backend, so there is no backend-varying implementation to override.
// Compiled unconditionally — the panic hook and on-disk record need no
// backend feature; see `crash`'s module doc.
pub mod crash;

// Auto-update seam: `UpdateService` + the feed/info/staged types it
// trades in, reached off `PlatformServices::update_service` the same way
// `PlatformServices::secret_store` exposes `SecretStore`. Compiled
// unconditionally — the contract and its `Unsupported` default need no
// backend feature; see `update`'s module doc.
pub mod update;

// Unified `run`/`run_with` return type — the seam every backend's
// `run.rs`/`shell_runner.rs` converges on, one per-backend issue at a
// time. Compiled unconditionally, like `crash`/`update`/`power`: the
// type itself carries no backend-specific state; see `run_error`'s
// module doc for the full why-not-`ExitCode` rationale.
pub mod run_error;

// ── PaintSurface ────────────────────────────────────────────────────────
// The ~15-verb drawing trait underneath the three pixel backends —
// extracted from helpers each of GtkBackend/MacBackend/WinBackend already
// had privately. `pub`, not `pub(crate)`: this is the public
// backend-paint seam, reached off `Backend::paint_surface` the same way
// `Backend::window`/`Backend::services` expose `WindowControl`/
// `PlatformServices`. TUI is deliberately excluded (see `paint_surface`'s
// module doc) — compiled unconditionally rather than feature-gated:
// a `pub` trait with zero implementors on a `tui`-only build is not
// "dead code" from a library's own crate boundary (an external crate can
// still implement it), so no `-D warnings` dead-code lint applies here the
// way it did back when this was `pub(crate)`.
pub mod paint_surface;

// ── CellSurface ─────────────────────────────────────────────────────────
// `PaintSurface`'s cell-grid counterpart — the drawing seam every cell
// painter writes through. Compiled unconditionally, same rationale as
// `paint_surface` above: every type it trades in is already
// feature-independent.
pub mod cell_surface;

// ── Primitive ──────────────────────────────────────────────────────────────
// The `Primitive` trait — one `layout()` + `paint()` pair a widget outside
// this crate implements to compose with every `PaintSurface` backend through
// `DrawsPrimitives::draw_primitive`, without a bespoke `draw_*`/`*_layout` pair added
// to the sealed `Backend` trait. Compiled unconditionally, same rationale as
// `paint_surface` above: a `pub` trait with zero in-crate implementors on a
// `tui`-only build is not dead code from this crate's own boundary.
pub mod primitive;

// ── Interactive ──────────────────────────────────────────────────────────
// The behaviour half of the widget contract: the `Interactive` trait,
// `RoutedEvent`/`EventHost`/`EventCx`, `FocusSpec`/`FocusStop`, and the
// `non_interactive!` macro for a `Primitive` with no behaviour of its own.
// `Primitive` above is the paint half. Compiled unconditionally, same
// rationale as `primitive` above: a `pub` trait with zero in-crate
// implementors on a `tui`-only build is not dead code from this crate's
// own boundary — the runtime that calls `Interactive::handle` is a later
// addition, not a precondition for the trait itself to exist.
pub mod interactive;

// ── Widget-contract routing engine ───────────────────────────────────────
// `FrameTree` — the current frame's `Interactive` widgets as (id, rect,
// widget) nodes, and `WidgetStateStore`/`WidgetRoutingState` — persistent
// per-widget state plus hover/capture bookkeeping. Together they back the
// hit-test/capture/Enter-Leave/focus/bubbling algorithm in
// `crate::runtime::preprocess_event`. Compiled unconditionally, same
// rationale as `primitive`/`interactive` above.
pub mod frame_tree;
pub mod widget_state;

// Widget-owned popups: `Popup`, what `Interactive::popups` returns; the
// resolved `OpenPopup`; and the per-frame queue the runner paints above
// the frame. Compiled unconditionally, same rationale as `frame_tree`.
pub mod popup;

// `kind` string → descriptor type registry a binding/wire layer decodes
// a JSON-shaped widget payload against, without knowing the concrete
// `Primitive` type that kind names — see that module's own doc.
pub mod widget_kinds;

// ── Behaviour ─────────────────────────────────────────────────────────────
// The single shared implementation of scroll-into-view, list/tree
// navigation and typeahead every widget's `Interactive` impl builds from —
// `behaviour::scroll`, `behaviour::nav`, `behaviour::typeahead`. Compiled
// unconditionally, same rationale as `primitive`/`interactive` above: pure
// arithmetic with no native dependency, so every backend gets it for free.
pub mod behaviour;

// Host-independent paint-geometry helpers — pure rect/inset
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
// See `docs/BACKEND_SETUP_AUDIT.md` for design rationale.
pub mod runner;

// Backend-neutral control channel: a cloneable, `Send` handle a thread
// that does not own the event loop uses to drive a running `AppLogic` app
// (snapshot/act/key/type) without knowing which backend is running it.
// Gated behind the `control` feature so an app that never needs this
// doesn't pay for it — see that module's doc.
#[cfg(feature = "control")]
#[cfg_attr(docsrs, doc(cfg(feature = "control")))]
pub mod control;
// `ShellAdapter` is constructed only by the TUI/GTK/macOS/Win-GUI shell
// runners. Gate the module on the features that actually consume it rather
// than allowing dead code under `-D warnings`.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
pub mod shell_adapter;

// Shared runner plumbing (`EventOutcome`, `ReactionSink` + `apply_outcome`,
// `ResizeDebouncer`) used by the shell runners. Gated to prevent dead-code
// warnings on feature combinations that don't use them.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
mod runtime;

// Shared, backend-neutral desktop/OS-service plumbing: window-drag
// arm/threshold/commit, modal-pump re-entrancy guard, trash, the
// default-handler opener, notifications, clipboard error mapping, headless
// smoke-mode predicates, PointerShape enum-walk scaffold, caught-panic
// reporting. Compiled unconditionally — no toolkit dependency of its own —
// so a brand-new backend gets it for free. See `platform`'s module doc and
// `BACKEND.md` §10.
mod platform;

// Facade re-exporting every `platform` item at its historical
// `crate::desktop::X` path, so every existing call site across
// `gtk`/`macos`/`win`/`tui` keeps compiling unchanged. See `desktop`'s
// module doc.
mod desktop;

// Shared text-selection state machine: the region registry plus active-selection
// tracking every `text_selection: true` backend embeds. Gated on the backends
// (TUI, GTK, Win, macOS) that actually use it.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod text_selection;

// `BackendCore`: the accelerator registry + `text_selection`
// embedding every one of the four backends above needs, consolidated
// into one place. Gated identically to `text_selection` just above —
// same rationale (no consumer, hard `dead_code` error under `-D
// warnings` otherwise).
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod backend_core;

/// Routes an assistive-technology action straight to a live
/// `Surface::Widget`'s `Interactive::handle`, so a widget defined outside
/// this crate answers assistive technology through the same `handle` its
/// mouse and keyboard input reach.
pub use a11y::route_widget_action;
pub use a11y::{
    backend_a11y_tree, frame_to_tree, frame_to_tree_with_modal, markdown_a11y_children, A11yAction,
    A11yActionValue, A11yChecked, A11yChild, A11yInfo, A11yLive, A11yNodeId, A11yPrimitive,
    A11yRole, A11yTree, A11yTreeBuilder, A11yTreeNode, A11yUpdate, A11yValueRange, FocusSource,
};
pub use caret_blink::CaretBlinkState;
pub use diff::compute_hunks;
pub use focus::{FocusAddress, FocusTree};
pub use font_role::{ChromePrimitive, EditorClassPrimitive, FontRole};
pub use generic_font::GenericFamily;
pub use primitives::activity_bar::{
    ActivityBar, ActivityBarEvent, ActivityBarHit, ActivityBarLayout, ActivityBarRowHit,
    ActivityBarStyle, ActivityItem, ActivitySide, VisibleActivityItem,
    DEFAULT_ACTIVITY_ICON_SIZE_PX,
};
pub use primitives::board::{
    measured_board_layout, BadgeStatus, BoardAction, BoardCard, BoardColumn, BoardHit, BoardLayout,
    BoardMeasure, BoardModel, CardBadge, CardId, CardLayout, ColumnLayout, MoveDir,
};
pub use primitives::button::{
    Button, ButtonEvent, ButtonHit, ButtonIconPosition, ButtonLayout, ButtonMetrics, ButtonSpec,
    ButtonVariant,
};
pub use primitives::canvas::{
    Canvas, CanvasCursor, CanvasHit, CanvasLayout, CanvasRegion, CanvasState, DrawOp,
};
pub use primitives::cell_grid::{
    column_letters, Cell, CellAlign, CellGrid, CellGridEvent, CellGridHit, CellGridLayout,
    CellGridPrimitiveLayout, CellGridState, CellStyle, CellsWindow, ResolvedSpan,
};
pub use primitives::chart::{
    Chart, ChartEvent, ChartHit, ChartKind, ChartLayout, ChartMeasure, ChartPrimitiveLayout,
    ChartState, Series,
};
pub use primitives::combo_box::{ComboBox, ComboBoxEvent};
pub use primitives::command_center::{
    CommandCenter, CommandCenterAction, CommandCenterHit, CommandCenterLayout,
    CommandCenterMeasure, CommandCenterTarget,
};
pub use primitives::command_line::{CommandLine, CommandLineLayout, CommandLineMeasure};
pub use primitives::completions::{
    CompletionItem, CompletionItemMeasure, CompletionKind, Completions, CompletionsHit,
    CompletionsLayout, CompletionsPlacement, VisibleCompletion,
};
pub use primitives::context_menu::{
    ContextMenu, ContextMenuGlyphs, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure,
    ContextMenuLayout, ContextMenuPlacement, ResolvedContextMenuPlacement, VisibleContextMenuItem,
};
pub use primitives::data_table::{
    Column, ColumnAlign, ColumnMeasure, ColumnWidth, DataRow, DataTable, DataTableEvent,
    DataTableGlyphs, DataTableHit, DataTableLayout, DataTablePaintState, ResolvedColumn,
    SortDirection,
};
pub use primitives::date_picker::{
    CalendarDate, DateFormat, DateLocale, DatePicker, DatePickerEvent, DatePickerHit,
    DatePickerLayout, DayCell, TimeOfDay, Weekday,
};
pub use primitives::dialog::{
    native_dialog_options, Dialog, DialogButton, DialogContentSlot, DialogHit, DialogInput,
    DialogLayout, DialogMeasure, DialogSeverity, DialogTable, DialogTextInput, VisibleDialogButton,
};
pub use primitives::diff_view::{
    DiffDisplayLine, DiffEditability, DiffHeaderGeometry, DiffHunk, DiffLineContent, DiffMode,
    DiffPane, DiffPaneGeometry, DiffRow, DiffRowKind, DiffView, DiffViewEvent, DiffViewGeometry,
    DiffViewHit, DiffViewLayout,
};
pub use primitives::drop_zone::{
    compute_drop_zone, drop_zone_hit_test, drop_zone_overlay, DropEdge, DropGroupRect, DropOverlay,
    DropOverlayLayout, DropZone, DropZoneHit, DropZoneKind, DROP_OVERLAY_ZONE_ID,
};
pub use primitives::editor::{
    CursorPos as EditorCursorPos, CursorShape as EditorCursorShape, DiagnosticMark,
    DiagnosticSeverity, DiffLine, Editor, EditorCursor, EditorEvent, EditorHit, EditorLayout,
    EditorLine, EditorPaintOptions, EditorSelection, EditorState, EditorStyledSpan, GitLineStatus,
    SelectionCols as EditorSelectionCols, SelectionKind as EditorSelectionKind, SpellMark,
    Style as EditorStyle,
};
pub use primitives::find_replace::{
    compute_hit_regions as compute_find_replace_hit_regions,
    compute_hit_regions_hiding as compute_find_replace_hit_regions_hiding, FindReplaceClickTarget,
    FindReplaceEvent, FindReplaceField, FindReplaceHit, FindReplaceLayout, FindReplaceMetrics,
    FindReplacePanel, FindReplaceToggle, FrHitRegion, FR_PANEL_WIDTH,
};
pub use primitives::float::{Float, FloatHit, FloatLayout, FloatMeasure};
pub use primitives::form::{
    BareFieldLayout, ButtonRowItem, ControlWidth, FieldKind, Form, FormEvent, FormField,
    FormFieldMeasure, FormHit, FormItemMeasure, FormLayout, FormSpacing, LabelAlign, LabelPosition,
    RowAlign, ToggleGroupItem, ValidationState, VisibleFormField,
};
pub use primitives::icon_grid::{
    icon_grid_move_selection, IconGrid, IconGridEvent, IconGridHit, IconGridItem, IconGridLayout,
    IconGridNav, VisibleIconGridTile,
};
// `IconGrid`'s `Primitive::Layout` — defined on every feature set its
// `impl Primitive for IconGrid` itself is (see that impl's own `cfg`):
// the three pixel backends via `native_surface_paint`, plus `tui` via
// `Primitive::paint_cells`.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use primitives::icon_grid::IconGridPrimitiveLayout;
pub use primitives::image::{Image, ImageFit, ImageHit, ImageLayout, ImageSource};
pub use primitives::list::{
    ListItem, ListItemMeasure, ListPaintState, ListView, ListViewEvent, ListViewHit,
    ListViewLayout, ListViewSubPart, VisibleListItem,
};
// `ListView`'s `Primitive::Layout`.
pub use primitives::checkbox::{Checkbox, CheckboxAction};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use primitives::list::ListViewPrimitiveLayout;
pub use primitives::markdown::{Markdown, MarkdownAction, MarkdownLayout, MarkdownLink};
pub use primitives::menu_bar::{
    resolve_bar_height, MenuBar, MenuBarHit, MenuBarItem, MenuBarItemMeasure, MenuBarLayout,
    VisibleMenuBarItem, BAR_HEIGHT,
};
pub use primitives::message_list::{
    MessageList, MessageListEvent, MessageListHit, MessageListLayout, MessageListMeasure,
    MessageRow, VisibleMessageRow,
};
pub use primitives::minimap::{
    aggregate_spans, reserved_width, sample_blocks, sample_window, window_start_line, Minimap,
    MinimapEvent, MinimapGrid, MinimapHit, MinimapLayout, MinimapLine, MinimapScale, MinimapSizing,
    MinimapSpan, MinimapState, VisibleMinimapLine,
};
pub use primitives::multi_section_view::{
    ActionId as MsvActionId, AuxHit, Axis as MsvAxis, DividerBounds, EmptyBody, HeaderAction,
    HeaderHit, InlineInput, MsvLayoutMetrics, MultiSectionView, MultiSectionViewEvent,
    MultiSectionViewHit, MultiSectionViewLayout, ScrollMode, ScrollbarHit, Section, SectionAux,
    SectionBody, SectionHeader, SectionId, SectionLayout, SectionMeasure, SectionSize,
};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use primitives::multi_section_view::{
    MultiSectionViewPrimitiveLayout, MultiSectionViewState, MultiSectionViewSubPart,
    SectionBodyLayout, SectionBodyState, SectionBodyView,
};
pub use primitives::number_input::{
    NumberInput, NumberInputEvent, NumberInputHit, NumberInputLayout,
};
pub use primitives::palette::{
    Palette, PaletteEvent, PaletteHit, PaletteItem, PaletteItemMeasure, PaletteLayout, PaletteMode,
    PalettePreview, PaletteScrollbar, VisiblePaletteItem,
};
pub use primitives::password_input::{PasswordInput, PasswordInputAction};
pub use primitives::pipeline_view::{
    PipelineEvent, PipelineHit, PipelineStage, PipelineView, PipelineViewLayout,
    PipelineViewMeasure, PipelineViewPrimitiveLayout, StageBounds, StageStatus,
};
pub use primitives::progress::{
    ProgressBar, ProgressBarHit, ProgressBarLayout, ProgressBarMeasure,
};
pub use primitives::radio_group::{
    RadioGroup, RadioGroupEvent, RadioGroupHit, RadioGroupLayout, RadioOption, RadioOptionMeasure,
};
pub use primitives::rich_text_popup::{
    PopupPlacement, PopupScrollbar, RichTextLink, RichTextPopup, RichTextPopupHit,
    RichTextPopupLayout, RichTextPopupMeasure, TextSelection, VisibleRichTextLine,
};
pub use primitives::scroll_view::{ScrollView, ScrollViewEvent, ScrollViewLayout, ScrollViewState};
pub use primitives::scrollbar::{
    clamp_scroll_offset_f32, fit_thumb, scroll_by_clamped_f32, scroll_rows, wheel_delta,
    wheel_row_delta, PressZone, ScrollAxis, Scrollbar, ScrollbarEvent, ScrollbarLayout,
    ScrollbarState, SCROLLBAR_WHEEL_VIEWPORTS_PER_NOTCH,
};
pub use primitives::select::{Select, SelectAction};
pub use primitives::sidebar_panel::{
    SidebarPanel, SidebarPanelAction, SidebarPanelHit, SidebarPanelLayout, SidebarPanelState,
};
pub use primitives::slider::{Slider, SliderAction, SliderState};
pub use primitives::spinner::{Spinner, SpinnerHit, SpinnerLayout, SpinnerMeasure};
pub use primitives::split::{
    Split, SplitDirection, SplitDividerDrag, SplitHit, SplitLayout, SplitMeasure, SplitState,
    SplitTreeState,
};
pub use primitives::split_tree::{
    SplitTree, SplitTreeDivider, SplitTreeLayout, SplitTreeMeasure,
    MAX_RATIO as SPLIT_TREE_MAX_RATIO, MIN_RATIO as SPLIT_TREE_MIN_RATIO,
};
pub use primitives::status_bar::{
    StatusBar, StatusBarEvent, StatusBarHit, StatusBarHitRegion, StatusBarLayout, StatusBarSegment,
    StatusSegmentMeasure, StatusSegmentSide, VisibleStatusSegment,
};
pub use primitives::switch::{Switch, SwitchEvent, SwitchHit, SwitchLayout};
pub use primitives::text_area::{TextArea, TextAreaAction};
// `TabBarHits` is deprecated (D-6 pre-refactor hit struct; `TabBarLayout` with
// `Rect` is the replacement). Allow it here since `pub use` of a deprecated
// item is itself a use site. Removal is blocked on the six-method/four-backend
// signature swap to `TabBarLayout` — see `TabBarHits`'s own doc.
#[allow(deprecated)]
pub use primitives::tab_bar::{
    tab_icon_at, tab_icon_cols, SegmentMeasure, TabBar, TabBarEvent, TabBarGlyphs, TabBarHit,
    TabBarHits, TabBarLayout, TabBarSegment, TabChrome, TabFrame, TabIcon, TabItem, TabMeasure,
    TabSeparator, VisibleSegment, VisibleTab,
};
// `TabBar`'s `Primitive::Layout` — unconditional, like `impl Primitive
// for TabBar` itself: the three pixel backends paint it via
// `native_surface_paint`, `tui` via
// `Primitive::paint_cells`/`cell_paint::paint`, and a build with no
// rasteriser at all still resolves the pixel-measured layout so a
// backend-generic `AppLogic` type-checks.
pub use primitives::tab_bar::TabBarPrimitiveLayout;
pub use primitives::terminal::{
    Terminal, TerminalCell, TerminalCellSize, TerminalCursorShape, TerminalDivider,
    TerminalDividerLayout, TerminalEvent, TerminalHit, TerminalLayout, TerminalScrollbar,
    TerminalSplitHit, TerminalSplitLayout, TerminalState,
};
pub use primitives::text_display::{
    TextDisplay, TextDisplayEvent, TextDisplayHit, TextDisplayLayout, TextDisplayLine,
    TextDisplayLineMeasure, VisibleTextDisplayLine,
};
pub use primitives::text_field::{TextField, TextFieldHit, TextFieldLayout, TextFieldMeasure};
pub use primitives::text_input::{
    EditOp, TextEditor, TextInput, TextInputHit, TextInputLayout, TextInputMeasure,
    TextInputMetrics, VisibleTextInputLine,
};
pub use primitives::toast::{
    Toast, ToastButton, ToastCorner, ToastFocus, ToastFocusTarget, ToastHit, ToastMeasure,
    ToastOverlay, ToastSeverity, ToastStackLayout, VisibleToast,
};
pub use primitives::toolbar::{
    GroupChildBounds, Toolbar, ToolbarButton, ToolbarEvent, ToolbarHit, ToolbarIcons,
    ToolbarItemKind, ToolbarItemMeasure, ToolbarItemRole, ToolbarItemWidth, ToolbarLayout,
    ToolbarLayoutOptions, ToolbarOrientation, ToolbarOverflow, ToolbarOverflowPriorities,
    ToolbarPaintOptions, ToolbarRailStyle, ToolbarVAlign, VisibleToolbarItem,
    TOOLBAR_OVERFLOW_ITEM_IDX,
};
pub use primitives::tooltip::{
    ResolvedPlacement, Tooltip, TooltipBorder, TooltipChrome, TooltipHit, TooltipLayout,
    TooltipMeasure, TooltipPlacement,
};
pub use primitives::tree::{
    selected_paths_for_indices, TreeEvent, TreePaintState, TreeRow, TreeRowEditState,
    TreeRowMeasure, TreeView, TreeViewHit, TreeViewLayout, VisibleTreeRow,
};
pub use primitives::tree_table::{
    TreeTable, TreeTableEvent, TreeTableHit, TreeTableLayout, TreeTableModelEvent, TreeTableNode,
};
pub use style::{CellOverflow, Style};
pub use text_util::{
    fuzzy_score, next_char_boundary, prev_char_boundary, safe_prefix, safe_slice,
    snap_to_char_boundary, word_wrap, wrap_spans, WrapPolicy,
};
pub use theme::Theme;
pub use transition::{lerp, lerp_rect, Transition};
pub use types::{
    apply_selection_gesture, Badge, Color, Decoration, Icon, Modifiers, SelectionCore,
    SelectionGesture, SelectionMode, StyledSpan, StyledText, TreePath, TreeStyle, WidgetId,
};
pub use undo::UndoStack;

// Phase B.1 re-exports.
pub use accelerator::{
    parse_key_binding, render_accelerator, render_binding, Accelerator, AcceleratorId,
    AcceleratorScope, GlobalShortcutId, GlobalShortcutRegistry, KeyBinding, ParsedBinding,
    Platform,
};
pub use backend::{
    Backend, BackendCaps, BackendError, Clipboard, ClipboardFormat, ColorDepth, Display,
    DrawsPrimitives, FileDialogOptions, InvalidationLog, MenuStyle, MessageDialogButton,
    MessageDialogChoice, MessageDialogOptions, Metrics, Notification, PlatformFontDefaults,
    PlatformServices, PointerShape, ResizeEdge, ResolvedMenuStyle, RgbaImage, Screenshot,
    ScreenshotRun, SecretStore, ServiceResult, SystemTheme, WindowControl,
};
// `PlatformServices::update_service`'s vended trait and types — same
// service-seam shape as `Clipboard`/`SecretStore` above; see `update`'s
// module doc.
pub use event::{
    mouse_down, mouse_moved, mouse_up, scroll, window_resized, BackendNativeEvent, ButtonMask,
    CanvasEvent, Key, MouseButton, NamedKey, Point, Rect, ScrollDelta, UiEvent, UserPayload,
    Viewport,
};
pub use update::{StagedUpdate, UpdateFeed, UpdateInfo, UpdateInfoBuilder, UpdateService};
// The unified `run`/`run_with` return type every backend converges on —
// see `run_error`'s module doc.
pub use run_error::RunError;
// `PlatformServices::on_battery`/`thermal_state`/`begin_power_save_block`/
// `end_power_save_block`'s types and `UiEvent::PowerStateChanged`'s
// payload — see `power`'s module doc.
pub use power::{BlockerId, PowerEventKind, PowerSaveKind, ThermalState};
// `PlatformServices::begin_drag_out`'s argument type — see `drag`'s
// module doc.
pub use drag::DragOutPayload;
// `PlatformServices::set_login_item`'s argument type — see `login_item`'s
// module doc.
pub use login_item::LoginItemOptions;
// `PlatformServices::app_path`'s argument type, same precedent as
// `DragOutPayload` above. The app-identifier getter/setters stay reachable
// at `quadraui::paths::*` only — call sites there, not here.
pub use paths::AppPathKind;
// The public paint-verb seam — see `paint_surface`'s module doc.
// `FrameHitMap`/`FrameZone` are deprecated (use `crate::frame_tree::FrameTree`
// instead — see their own `#[deprecated]` notes in `frame.rs`). Allow it here
// since `pub use` of a deprecated item is itself a use site.
#[allow(deprecated)]
pub use frame::{
    check_frame_order, compose_frame, set_frame_capture, FrameCapture, FrameHitMap,
    FrameOrderViolation, FramePresence, FrameRung, FrameZone, OwnedContent, OwnedFrame, OwnedNode,
    ScreenLayout, Surface,
};
pub use paint_surface::{AlphaSheet, PaintSurface, PathVerb, TextRunStyle};
// `PaintSurface`'s cell-grid counterpart — see `cell_surface`'s module doc.
// `CellStyle` is aliased the same way `primitives::editor`'s own
// `CursorShape`/`CursorPos` are above (`EditorCursorShape`,
// `EditorCursorPos`): `primitives::cell_grid` already has an unrelated,
// unaliased `CellStyle` (spreadsheet-cell formatting) at this crate root.
pub use cell_surface::{
    BorderSet, CellModifier, CellStyle as CellSurfaceStyle, CellSurface, CursorShape,
};
// The `Primitive` trait — see that module's doc.
pub use primitive::{LayoutCtx, PaintEnv, Placed, Primitive};
// The behaviour half of the widget contract — see `interactive`'s module
// doc. `non_interactive!` itself is exported at the crate root by
// `#[macro_export]`, not re-exported here.
pub use interactive::{
    EventCx, EventHost, FocusIndicator, FocusSpec, FocusStop, Interactive, KeyEvent, Never,
    PointerKind, RoutedEvent, WidgetAction,
};
// The widget-contract routing engine's two public types — see
// `frame_tree`/`widget_state`'s own module docs.
pub use frame_tree::{FrameHit, FrameTree, FrameTreeNode};
pub use widget_state::{
    lend_widget_state, with_widget_states, WidgetRoutingState, WidgetStateStore,
};
// Widget-owned popups — see `popup`'s own module doc.
pub use popup::{
    paint_open_popups, DismissReason, OpenPopup, Popup, PopupDismiss, PopupKey, PopupSide,
};
// `decode_widget`/`decode_widget_from`/`KindError` — see `widget_kinds`'s
// own module doc. `widget_kinds!` itself is exported at the crate root by
// `#[macro_export]`, not re-exported here, the same convention as
// `non_interactive!` above.
pub use widget_kinds::{
    decode_widget, decode_widget_from, register_widget_kind, register_widget_kind_with_schema,
    runtime_widget_kind_schema, unregister_widget_kind, visit_widget_kinds, widget_kind_names,
    DecodeFn, KindError, KindSchema, KindVisitor, SchemaFn,
};
// `InteractionState` — a single hover/pressed store keyed by `WidgetId`,
// meant to replace the bespoke per-primitive trackers and positional arguments
// (`hovered_id`, `pressed_id`, `hovered_toolbar_id`) the old `Backend` methods
// used to take. New `*_interactive` variants take `&InteractionState`; old
// names survive only as `#[deprecated]` shims per rule 3 in `CLAUDE.md` (both
// are `pub` surface with real downstream call sites). See `interaction.rs`'s
// module doc for the full before/after table and the four index-keyed methods
// deliberately left unmigrated. `examples/common/toolbar_app.rs` demonstrates
// the pattern as an app-owned drop-in replacement.
// Shared layout/hit-test foundation (`Anchor` for overlay positioning,
// `visible_range_walk` replacing per-primitive `Visible*{idx, bounds}` structs).
pub use interaction::InteractionState;
// Shared layout/hit-test foundation (`Anchor` for overlay
// positioning, `visible_range_walk` replacing the per-primitive
// `Visible*{idx, bounds}` structs). Nothing in `primitives/` consumes
// these yet — each primitive converges onto them in its own PR behind a
// `#[deprecated]` shim per `docs/PRIMITIVE_RULES.md` rule 8.
pub use layout::{visible_range_walk, Anchor, Axis, NaturalSize, ResolvedSide, Side, VisibleItem};
// The stack layout solver's public vocabulary.
pub use layout::stack::{Align, EdgeInsets, Justify, Spacing};
pub use shell::{ShellApp, ShellConfig, ShellContext};

// Phase B.4 re-exports.
pub use compose::markdown::{
    render_markdown_to_styled, render_markdown_to_styled_wrapped, CodeBlockRange, RenderedMarkdown,
};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use compose::SidebarLayout;
pub use compose::SidebarState;
pub use compose::{drive_ui_event, Driven};
pub use compose::{
    filter_help_actions, help_actions_to_palette_items, notify_or_toast, BackendWidget, BottomBand,
    BottomPanel, BottomPanelConfig, BottomPanelEvent, BottomPanelItem, BottomPanelPrimitiveLayout,
    BottomPanelState, BottomPanelTab, Chat, ChatEvent, ChatLayout, ChatRole, ChatState, ChatTurn,
    ChatZones, ContextMenuController, ContextMenuOutcome, DualModePaletteAction, Edge,
    FilePickerMode, FormController, FormControllerAction, FormDialogAction, GroupLayout, Handled,
    HelpAction, HelpNote, HelpOverlayAction, HelpRegistry, MenuDef, NavigationMode,
    PanelDefinition, PathPickerAction, SectionKind, ShellDrag, ShellEvent, ShellLayout,
    ShellPosition, ShellRegion, ShellView, ShellViewLayout, ShellViewState, Sidebar, SidebarAction,
    SidebarBody, SidebarPanelBody, SidebarPanelBodyLayout, SidebarPanelChrome, SidebarSection,
    SidebarSectionDef, StatusBarInteraction, TabGroup, TabGroupEvent, TabGroupPane,
    TabGroupPaneLayout, TabGroupPrimitiveLayout, TabGroupState, TabGroupSubPart, TabGroupTab,
    ToastStackAction, ToastStackController, ToolbarHoverTracker, ViewHelp, Workspace, WorkspaceDoc,
    WorkspaceEvent, WorkspacePrimitiveLayout, WorkspaceState, PALETTE_CHROME_ROWS,
};
pub use compose::{
    AppMenu, AppMenuEvent, AppMenuState, AppMenuView, DualModePalette, DualModePaletteState,
    FilePicker, FolderPicker, FormDialog, FormDialogState, HelpOverlay, HelpOverlayState,
    MessageDialog, MessageDialogState, PathPicker, PathPickerEditState, PathPickerState,
};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use compose::{
    TreeTableController, TreeTableControllerEvent, TreeTableModel, TreeTableModelState,
    TreeTableNodeData,
};
// `NodeId` is `compose::tree_table_controller::NodeId` — a plain `String`
// alias, not re-exported at the crate root to avoid shadowing any other
// crate's own `NodeId`; reach it via `quadraui::compose::NodeId`.
// `StatusBarAction`, `ToastStackEvent` and `FormControllerEvent` are
// `#[deprecated]` (use `StatusBarInteraction::handle_event`'s
// `Handled<WidgetId>`, `ToastStackController::handle_event`'s
// `Handled<ToastStackAction>` and
// `FormController::handle_event`/`handle_cached_event`'s
// `Handled<FormControllerAction>` instead). Allow them here since `pub use`
// of a deprecated item is itself a use site.
#[allow(deprecated)]
pub use compose::{FormControllerEvent, StatusBarAction, ToastStackEvent};
pub use dispatch::{
    dispatch_click, dispatch_mouse_down, dispatch_mouse_drag, dispatch_mouse_up, dispatch_scroll,
    text_selection_line_range, DragState, DragTarget, ScrollSurface, SurfaceScrollbar, TextRegion,
};
pub use modal_stack::{ModalEntry, ModalStack};
pub use runner::{AppLogic, Reaction, StepOutcome};

/// Crate version, sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    /// First `version = "…"` value inside the manifest's `[package]` table.
    ///
    /// Deliberately stops at the next table header, so a `[dependencies.*]`
    /// entry's own `version` key can never be mistaken for the package's.
    /// `rust-version` is not a false positive either — it does not *start*
    /// with `version`.
    fn package_version(manifest: &str) -> Option<&str> {
        manifest
            .lines()
            .map(str::trim)
            .skip_while(|line| *line != "[package]")
            .skip(1)
            .take_while(|line| !line.starts_with('['))
            .find_map(|line| {
                let value = line
                    .strip_prefix("version")?
                    .trim_start()
                    .strip_prefix('=')?;
                value.trim().strip_prefix('"')?.split('"').next()
            })
    }

    /// [`VERSION`] must be exactly the `[package] version` in `Cargo.toml`.
    ///
    /// The expectation is *derived from the manifest text*, never a
    /// hardcoded literal: a literal makes every release bump two edits
    /// instead of one, and forgetting the second is precisely how the
    /// `v0.1.0` bump turned this test red. `include_str!`
    /// resolves relative to this source file and cargo always ships a
    /// normalised `Cargo.toml` at the package root, so this works inside
    /// the published `.crate` as well as in-tree — on every platform, since
    /// rustc accepts `/` in `include_str!` paths everywhere.
    #[test]
    fn version_matches_cargo_toml() {
        let manifest = include_str!("../Cargo.toml");
        let declared = package_version(manifest).expect(
            "quadraui/Cargo.toml should have a `version = \"...\"` key in its \
             `[package]` table",
        );
        assert_eq!(
            VERSION, declared,
            "CARGO_PKG_VERSION ({VERSION}) disagrees with quadraui/Cargo.toml's \
             [package] version ({declared}) — either the build is reading a stale \
             manifest, or this test's parser no longer matches the manifest layout."
        );
    }

    #[test]
    fn package_version_parser_ignores_other_tables() {
        let manifest = "\
[package]\n\
name = \"quadraui\"\n\
version = \"1.2.3\"\n\
rust-version = \"1.97.1\"\n\
\n\
[dependencies.serde]\n\
version = \"9.9.9\"\n";
        assert_eq!(package_version(manifest), Some("1.2.3"));
    }

    #[test]
    fn package_version_parser_reports_a_missing_key() {
        assert_eq!(package_version("[package]\nname = \"quadraui\"\n"), None);
        assert_eq!(package_version("[dependencies]\nversion = \"1.0\"\n"), None);
    }

    /// The manifest's declared `rust-version`, in the exact
    /// `rust-version = "X.Y"` / `rust-version = "X.Y.Z"` shape
    /// `.github/workflows/ci.yml`'s `msrv` job parses with a `sed`
    /// one-liner — that job's own source of truth for the consumer MSRV
    /// floor it builds against. Returns `None` for anything that shape
    /// doesn't cover (missing key, non-numeric component, a fourth
    /// component, trailing text after the closing quote), mirroring what
    /// that job's parser treats as a parse failure.
    fn declared_rust_version(manifest: &str) -> Option<&str> {
        manifest.lines().map(str::trim).find_map(|line| {
            let value = line
                .strip_prefix("rust-version")?
                .trim_start()
                .strip_prefix('=')?
                .trim();
            let inner = value.strip_prefix('"')?.strip_suffix('"')?;
            let mut parts = inner.split('.');
            let major = parts.next()?;
            let minor = parts.next()?;
            let patch = parts.next();
            if parts.next().is_some() {
                return None;
            }
            let is_digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
            if !is_digits(major) || !is_digits(minor) {
                return None;
            }
            if let Some(p) = patch {
                if !is_digits(p) {
                    return None;
                }
            }
            Some(inner)
        })
    }

    /// `rust-version` must parse in the exact shape
    /// `.github/workflows/ci.yml`'s `msrv` job expects, or that job's own
    /// `::error::` guard fires on every PR instead of testing the declared
    /// floor. Catches a shape drift here, in-tree, instead of in a red CI
    /// run.
    #[test]
    fn rust_version_parses_in_the_shape_the_msrv_job_expects() {
        let manifest = include_str!("../Cargo.toml");
        assert!(
            declared_rust_version(manifest).is_some(),
            "quadraui/Cargo.toml's rust-version line is missing, or not in \
             the exact `rust-version = \"X.Y\"` / `rust-version = \"X.Y.Z\"` \
             shape `.github/workflows/ci.yml`'s `msrv` job parses with `sed`. \
             Fix the field's shape, and update that job's parser to match if \
             the shape itself is deliberately changing.",
        );
    }

    #[test]
    fn declared_rust_version_rejects_malformed_shapes() {
        assert_eq!(declared_rust_version("rust-version = \"1\"\n"), None);
        assert_eq!(declared_rust_version("rust-version = \"1.92.0.1\"\n"), None);
        assert_eq!(declared_rust_version("rust-version = \"x.y\"\n"), None);
        assert_eq!(
            declared_rust_version("rust-version = \"1.92.0\"  # trailing\n"),
            None
        );
        assert_eq!(declared_rust_version("name = \"quadraui\"\n"), None);
        assert_eq!(
            declared_rust_version("rust-version = \"1.92\"\n"),
            Some("1.92")
        );
        assert_eq!(
            declared_rust_version("rust-version = \"1.92.0\"\n"),
            Some("1.92.0")
        );
    }
}
