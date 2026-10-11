//! Capability honesty check: [`quadraui::BackendCaps`] is a backend saying "these optional surfaces
//! are real on me". C0 (`c0.rs`) proves the *required* `draw_*` surface paints something; this
//! module proves the *optional* surface isn't lying, in both directions:
//!
//! - **Declared ⇒ overridden.** A backend that reports `text_selection: true` must actually
//!   override `register_text_region` *and* `cancel_text_selection_drag`. Otherwise the trait's
//!   no-op default silently eats every registration.
//! - **Not declared ⇒ untouched default.** A backend that overrides `set_cursor` but forgets
//!   `pointer_cursor: true` makes every scenario gating on it skip forever, and a skip row
//!   looks like a known gap rather than a stale declaration.
//!
//! ## Why source parsing
//!
//! "Is this trait method overridden, or is it the default?" is not a question a running
//! program can ask: a defaulted method and an overriding one are the same call at the same
//! vtable slot. The only place the answer exists is the source, so that is where this reads it
//! — the same technique [`c0::tests::cases_cover_every_draw_method_on_the_trait`] already uses
//! to hold `CASES` to the trait.
//!
//! Two things keep that from being a fragile trick:
//!
//! 1. [`declared_in_source`] is cross-checked against the *running* backend's
//!    `Backend::backend_caps()` for every backend this build compiles in
//!    (`conformance::source_parsed_caps_match_the_running_backend`). If the parser ever goes
//!    stale against reformatted source, that test fails loudly instead of this one quietly
//!    passing on an empty parse.
//! 2. [`parse_sanity`] asserts each impl block yields a plausible number of methods, so a
//!    header rename can't silently degrade every backend to "overrides nothing" — which
//!    would read as a clean bill of health.
//!
//! ## Win and macOS
//!
//! Source parsing works on backends that have no conformance driver, and can't even be
//! compiled on this host. `MacBackend` and `WinBackend` are checked here on every run, on
//! every platform — so capability gaps enumerate as red/skip rows, not silence.

use std::collections::BTreeSet;

use quadraui::BackendCaps;

// ─── What each capability promises ──────────────────────────────────────

/// How a capability's claim can be checked against source.
pub enum Proof {
    /// Declaring it promises **every** listed `Backend` method is
    /// overridden away from its no-op default.
    All(&'static [&'static str]),
    /// Declaring it promises **at least one** of the listed methods is
    /// overridden (the capability is a family, and a backend may
    /// reasonably implement part of it).
    Any(&'static [&'static str]),
    /// No `Backend` method's presence can prove or disprove this one.
    /// Carries the reason, which is the whole value of the variant: an
    /// unprovable capability is *stated* to be unprovable rather than
    /// quietly skipped.
    Unprovable(&'static str),
}

impl Proof {
    /// The methods this proof reads, or `&[]` when unprovable.
    fn methods(&self) -> &'static [&'static str] {
        match self {
            Proof::All(m) | Proof::Any(m) => m,
            Proof::Unprovable(_) => &[],
        }
    }
}

/// One capability and the source-level claim it makes.
pub struct CapContract {
    pub cap: &'static str,
    pub proof: Proof,
}

/// Every [`BackendCaps`] field, paired with what declaring it promises.
///
/// `cap_contracts_cover_every_capability` asserts this names exactly
/// `BackendCaps::vocabulary()`, so a new capability field cannot be added
/// without deciding — here, in writing — how it is to be checked.
pub const CAP_CONTRACTS: &[CapContract] = &[
    CapContract {
        cap: "mouse",
        proof: Proof::Unprovable(
            "produced by `poll_events`/`wait_events`, which are *required* trait methods — \
             every backend overrides them, including the Win stub whose bodies are `todo!()`, \
             so presence proves nothing. Behaviour is proven instead by the Tier-1 scenarios \
             that click.",
        ),
    },
    CapContract {
        cap: "scroll",
        proof: Proof::Unprovable("same required-method source as `mouse`"),
    },
    CapContract {
        cap: "drag",
        proof: Proof::Unprovable("same required-method source as `mouse`"),
    },
    CapContract {
        cap: "text_selection",
        proof: Proof::All(&["register_text_region", "cancel_text_selection_drag"]),
    },
    CapContract {
        cap: "native_menu",
        proof: Proof::Any(&["install_menu_bar", "show_context_menu"]),
    },
    CapContract {
        cap: "window_chrome",
        proof: Proof::Any(&[
            "begin_window_drag",
            "toggle_window_maximize",
            "begin_window_resize",
        ]),
    },
    CapContract {
        cap: "pointer_cursor",
        proof: Proof::All(&["set_cursor"]),
    },
    CapContract {
        cap: "ime",
        proof: Proof::Unprovable(
            "there is no backend-level IME method to override yet (see `BackendCaps::ime`); \
             every backend declares `false`, so there is nothing to check until one lands",
        ),
    },
    CapContract {
        cap: "file_dialogs",
        proof: Proof::Unprovable(
            "`PlatformServices::show_file_open_dialog`/`show_file_save_dialog` have no no-op \
             default — every backend implements both — so only *running* a native picker \
             distinguishes a real dialog from a `None`-returning stub",
        ),
    },
    CapContract {
        cap: "folder_dialogs",
        proof: Proof::Unprovable(
            "`PlatformServices::show_folder_open_dialog` likewise has no no-op default — every \
             backend implements it — so only *running* a native directory chooser distinguishes \
             a real dialog from a `None`-returning stub, same as `file_dialogs` (quadraui#935)",
        ),
    },
    CapContract {
        cap: "native_dialogs",
        proof: Proof::Unprovable(
            "`PlatformServices::show_message_dialog` likewise has no no-op default — every \
             backend implements it — so only *running* a native alert distinguishes a real \
             dialog from a `None`-returning stub, same as `file_dialogs` (quadraui#666). \
             `GtkDriver` paints Cairo offscreen and never opens the native window an \
             `AlertDialog` lives in, so its visibility has no automated coverage here — see \
             the manual smoke item in `docs/TESTING.md`'s \"What unit tests don't cover\" \
             section (`gtk_message_dialog` example) for the procedure that gap calls for",
        ),
    },
    CapContract {
        cap: "notifications",
        proof: Proof::Unprovable(
            "`PlatformServices::send_notification` likewise has no default to diverge from",
        ),
    },
    // `generic_font_families` entry: every backend that declares this capability
    // overrides both `register_font_from_memory` and `set_nerd_font_fallback`
    CapContract {
        cap: "app_font_registration",
        proof: Proof::All(&["register_font_from_memory", "set_nerd_font_fallback"]),
    },
    CapContract {
        cap: "window_control",
        proof: Proof::All(&["window"]),
    },
    CapContract {
        cap: "tray",
        proof: Proof::All(&["tray"]),
    },
    CapContract {
        cap: "generic_font_families",
        proof: Proof::All(&["set_editor_font", "set_ui_font"]),
    },
];

// ─── Every backend in the tree, compiled here or not ────────────────────

/// One backend's `impl Backend for …` block, as source.
pub struct BackendSource {
    /// Matches the `BackendReg::name` a compiled-in backend registers
    /// under, so `source_parsed_caps_match_the_running_backend` can pair
    /// the two.
    pub name: &'static str,
    /// Path, for failure text only.
    pub path: &'static str,
    /// The whole file.
    src: &'static str,
    /// The `impl` line, verbatim and at column 0.
    header: &'static str,
}

/// Every backend the crate ships — *not* only the ones this build
/// compiles. Win and macOS are checked on Linux CI exactly as GTK is.
pub const BACKENDS: &[BackendSource] = &[
    BackendSource {
        name: "tui",
        path: "src/tui/backend.rs",
        src: include_str!("../../src/tui/backend.rs"),
        header: "impl Backend for TuiBackend {",
    },
    BackendSource {
        name: "gtk",
        path: "src/gtk/backend.rs",
        src: include_str!("../../src/gtk/backend.rs"),
        header: "impl Backend for GtkBackend {",
    },
    BackendSource {
        name: "macos",
        path: "src/macos/backend.rs",
        src: include_str!("../../src/macos/backend.rs"),
        header: "impl Backend for MacBackend {",
    },
    BackendSource {
        name: "win",
        path: "src/win/backend.rs",
        src: include_str!("../../src/win/backend.rs"),
        header: "impl Backend for WinBackend {",
    },
];

impl BackendSource {
    /// The lines of this backend's `impl Backend for …` block, excluding
    /// the header and the closing brace.
    ///
    /// Relies on rustfmt's guarantee that a top-level `impl` closes with
    /// a `}` at column 0 — which also neatly excludes the *inherent*
    /// `impl TuiBackend` / `impl GtkBackend` blocks and the nested test
    /// backends under `mod tests` (indented, so never at column 0).
    fn impl_lines(&self) -> Vec<&'static str> {
        let mut lines = self.src.lines().skip_while(|l| *l != self.header);
        assert!(
            lines.next().is_some(),
            "{}: no line reads exactly {:?} — the impl header was renamed or reformatted, and \
             a silently-empty parse here would clear every backend of every capability claim",
            self.path,
            self.header
        );
        lines.take_while(|l| *l != "}").collect()
    }

    /// Every `Backend` method this backend overrides.
    ///
    /// One indent level inside the impl block, so `    fn name(` — which
    /// excludes both nested closures and the `…_impl` inherent helpers
    /// that live in the *other* impl block anyway.
    fn overrides(&self) -> BTreeSet<&'static str> {
        self.impl_lines()
            .into_iter()
            .filter(|l| l.starts_with("    fn ") && !l.starts_with("     "))
            .filter_map(|l| fn_name(l.trim_start()))
            .collect()
    }

    /// The capabilities this backend's `fn backend_caps` body sets to
    /// `true`, read from source.
    ///
    /// Cross-checked against the *running* backend wherever this build
    /// has one — see this module's docs.
    fn declared(&self) -> BTreeSet<&'static str> {
        let lines = self.impl_lines();
        let body = lines
            .iter()
            .skip_while(|l| !l.trim_start().starts_with("fn backend_caps"))
            .take_while(|l| **l != "    }");
        let vocabulary = BackendCaps::vocabulary();
        body.filter_map(|l| l.trim().strip_suffix(": true,"))
            .map(|cap| {
                *vocabulary
                    .iter()
                    .find(|known| **known == cap)
                    .unwrap_or_else(|| {
                        panic!(
                            "{}: `backend_caps` sets {cap:?}, which is not a `BackendCaps` \
                             field — capabilities are {vocabulary:?}",
                            self.path
                        )
                    })
            })
            .collect()
    }
}

/// The name in `fn <name>(…)` / `fn <name><…>(…)`, given a trimmed line.
fn fn_name(trimmed: &str) -> Option<&str> {
    trimmed
        .strip_prefix("fn ")?
        .split(['(', '<', ' '])
        .next()
        .filter(|n| !n.is_empty())
}

// ─── The rest of the no-op defaults ─────────────────────────────────────

/// `Backend`'s own source, for enumerating which trait methods carry a
/// default body.
const BACKEND_TRAIT_SRC: &str = include_str!("../../src/backend.rs");

/// Every `Backend` method that ships a default body, parsed from the trait itself.
///
/// `CAP_CONTRACTS` covers the ones a `BackendCaps` field gates, and C0 covers the
/// defaulted `draw_*` methods. This returns the remainder — `set_theme`, `set_nerd_fonts`,
/// `scales_text_rows`, `editor_col_at_x`, `register_zone` — that nothing else in this
/// suite looks at.
///
/// Derived from source rather than listed, so a new defaulted method enters the report
/// the moment it is added to the trait.
pub fn defaulted_trait_methods() -> Vec<&'static str> {
    let mut lines = BACKEND_TRAIT_SRC
        .lines()
        .skip_while(|l| *l != "pub trait Backend: sealed::Sealed {");
    assert!(
        lines.next().is_some(),
        "src/backend.rs: no line reads exactly `pub trait Backend: sealed::Sealed {{` — the \
         trait header moved, and an empty parse here would report every backend as complete"
    );
    let body: Vec<&str> = lines.take_while(|l| *l != "}").collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        // Exactly one indent level, so nested items and the prose `fn`
        // inside the trait's own doc-comment example are both excluded.
        if !(body[i].starts_with("    fn ") && !body[i].starts_with("     ")) {
            i += 1;
            continue;
        }
        let name = fn_name(body[i].trim_start());
        // Walk the signature to its end: `;` = required, `{` = defaulted.
        let mut j = i;
        while j < body.len() && !body[j].contains('{') && !body[j].trim_end().ends_with(';') {
            j += 1;
        }
        if j < body.len() && body[j].contains('{') {
            if let Some(name) = name {
                out.push(name);
            }
        }
        i = j + 1;
    }
    out
}

/// One backend legitimately (or knowingly) taking a defaulted method.
///
/// Every entry is a checklist line, not an excuse — delete one and watch
/// `silent_defaults_are_declared_gaps_not_silence` go red to confirm the
/// override landed. The `draw_*` defaults are absent on purpose: C0 owns
/// those, and it is a hard failure there rather than an entry here.
pub const ACCEPTED_DEFAULTS: &[(&str, &str, &str)] = &[
    // `last_error`: `WinBackend` is the only backend with a concrete error producer
    // (`end_frame` recording `BackendError::SurfaceLost` on a failed `EndDraw`), so it's
    // the only one that overrides this. TUI/GTK/macOS never set an internal error field.
    (
        "tui",
        "last_error",
        "crossterm has no concrete BackendError producer yet — the default answers exactly like \
         the method doesn't exist, per Backend::last_error's own doc",
    ),
    (
        "gtk",
        "last_error",
        "no concrete BackendError producer yet — D-009's follow-up 4 (wiring the ~25 swallowed \
         `cr.fill().ok()`/`cr.stroke().ok()` call sites through last_error) is unstarted",
    ),
    (
        "macos",
        "last_error",
        "no concrete BackendError producer yet — Cocoa/CoreGraphics failures aren't wired to \
         this channel",
    ),
    // ── GTK, macOS, Win: `request_full_repaint` exists to
    // discard an incremental-diff cache that can desync from the physical
    // display — TUI's problem, because `ratatui::Terminal` tracks a
    // `Buffer` it diffs against the real terminal's actual contents,
    // which a PTY write / resize race / popup dismissal can silently
    // invalidate. None of these three backends has an equivalent cache:
    // each repaints its entire surface from scratch on every single
    // frame (GTK's `DrawingArea` via Cairo, macOS's `drawRect:` via Core
    // Graphics, Win's `BeginDraw`/`EndDraw` via Direct2D) — there is
    // nothing for a stale-diff bug to hide in, so the no-op default
    // answers exactly like the method doesn't exist, per
    // `Backend::request_full_repaint`'s own doc.
    (
        "gtk",
        "request_full_repaint",
        "Cairo repaints the DrawingArea in full every frame — no incremental diff cache to \
         desync, so the no-op default is correct, not unfinished work (quadraui#1037)",
    ),
    (
        "macos",
        "request_full_repaint",
        "Core Graphics repaints the whole view from `drawRect:` every frame — no incremental \
         diff cache to desync, so the no-op default is correct (quadraui#1037)",
    ),
    (
        "win",
        "request_full_repaint",
        "Direct2D repaints the whole render target between `BeginDraw`/`EndDraw` every frame — \
         no incremental diff cache to desync, so the no-op default is correct (quadraui#1037)",
    ),
    // TUI: a fixed-cell backend, so the font-shaped methods have no meaning.
    (
        "tui",
        "scales_text_rows",
        "a terminal cell cannot grow; the `false` default is the correct answer, not a missing \
         one — scaled rows render bold at normal cell height",
    ),
    (
        "tui",
        "editor_col_at_x",
        "the default *is* uniform-monospace division (`EditorLayout::col_at_x`), which is exact \
         for a cell grid — GTK overrides it only because Pango advance widths vary",
    ),
    // TUI: `tree_paints_own_scrollbar` — scrolling a tree in a terminal has
    // no spare pixels for an overlay scrollbar, so `draw_tree` never paints
    // one and the default `false` is the honest answer: the host
    // (`TreeController`) reserves a column and paints the scrollbar itself.
    // GTK, macOS and Win all override this to `true` because each paints the
    // overlay inside `draw_tree`.
    (
        "tui",
        "tree_paints_own_scrollbar",
        "a terminal has no spare pixels for an overlay scrollbar, so `draw_tree` never paints \
         one — the host reserves a column and paints it instead (quadraui#1476)",
    ),
    // TUI: `has_font_family` answers "does the platform have
    // this font installed", which a terminal emulator's hosting process
    // cannot answer — the terminal's own font choice is invisible to
    // whatever it's running. `None` (the trait default) *is* the honest
    // "don't know" answer here, not a missing implementation; GTK/macOS/Win
    // all override it because each can genuinely enumerate installed
    // families (Fontconfig / Core Text / DirectWrite).
    (
        "tui",
        "has_font_family",
        "a terminal emulator's own font is invisible to the process it hosts — `None` (\"don't \
         know\") is the honest answer, not a missing implementation (quadraui#1024)",
    ),
    // ── GTK, macOS, Win: `snap_height` is deliberately unimplemented on
    // every pixel backend. The trait's default (identity) *is* the right
    // answer for them — they paint fractional heights exactly, unlike
    // TUI's cell grid — so there is nothing to override.
    (
        "gtk",
        "snap_height",
        "pixel backend — Cairo paints fractional heights exactly, so the identity default is \
         correct, not unfinished work (quadraui#632)",
    ),
    // macOS: real backend with unfinished work. `MacDriver` gives it a `ConformanceDriver`
    // and a Tier-1 row, but it's deliberately absent from `c0_paint_smoke`'s columns.
    (
        "macos",
        "scales_text_rows",
        "per-row glyph scaling not implemented, so `false` is honest today",
    ),
    (
        "macos",
        "editor_col_at_x",
        "no CoreText hit-test resolution yet; the uniform-division default is used",
    ),
    (
        "macos",
        "tab_bar_layout_with_chrome",
        "#631's `TabFrame::Brackets` isn't wired to the CoreText tab-bar rasteriser yet — the \
         issue's acceptance bar is TUI + GTK; macOS falls back to the plain `tab_bar_layout` \
         geometry (no bracket reservation) until that lands",
    ),
    (
        "macos",
        "resolve_tab_bar_layout_with_chrome",
        "issue #919's `TabBarLayout`-returning counterpart to `tab_bar_layout_with_chrome` — same \
         gap, same reason: #631's `TabFrame::Brackets` isn't wired to the CoreText tab-bar \
         rasteriser, so this falls back to the plain `resolve_tab_bar_layout` geometry too",
    ),
    (
        "macos",
        "snap_height",
        "pixel backend — CoreText paints fractional heights exactly, so the identity default is \
         correct, not unfinished work (quadraui#632)",
    ),
    // Win: every method is a `todo!()` stub. Listed one by one anyway — "the whole
    // backend is a stub" is exactly the kind of blanket excuse that outlives the stub.
    ("win", "scales_text_rows", "stub backend — see #19"),
    ("win", "editor_col_at_x", "stub backend — see #19"),
    (
        "win",
        "tab_bar_layout_with_chrome",
        "stub backend — see #19",
    ),
    (
        "win",
        "resolve_tab_bar_layout_with_chrome",
        "issue #919's `TabBarLayout`-returning counterpart to `tab_bar_layout_with_chrome` — same \
         stub-backend gap, see #19",
    ),
    (
        "win",
        "snap_height",
        "pixel backend — DirectWrite paints fractional heights exactly, so the identity default \
         is correct, not unfinished work (quadraui#632); unrelated to the #19 stub gaps above",
    ),
    // `titlebar_control_inset` reports how much of the leading edge of the app-drawn
    // title band the *backend's own* chrome already occupies. macOS overrides it (AppKit's
    // traffic lights float over the content area); the other three take the empty default
    // for different reasons:
    //
    //   * TUI has no window concept at all — no native chrome could overlap the band.
    //   * GTK and Win-GUI don't read `ShellConfig::client_side_titlebar` yet, so their
    //     windows keep full native decorations and their window controls stay outside the
    //     app's content area. These are live TODOs: whichever backend grows client-side
    //     decorations must override this method and delete its line here.
    (
        "tui",
        "titlebar_control_inset",
        "no window concept — a terminal has no native window controls that could overlap the \
         title band, so the empty default is the correct answer, not a missing one (#947)",
    ),
    (
        "gtk",
        "titlebar_control_inset",
        "doesn't honour `ShellConfig::client_side_titlebar` yet (#947 wired macOS only) — GTK \
         windows keep native decorations, so no GTK-drawn control overlaps the app's title band; \
         override this and delete this line when CSD lands",
    ),
    (
        "win",
        "titlebar_control_inset",
        "doesn't honour `ShellConfig::client_side_titlebar` yet (#947 wired macOS only) — the \
         Win-GUI window keeps its native non-client titlebar, so no Win-drawn control overlaps \
         the app's title band; override this and delete this line when CSD lands",
    ),
    // `terminal_layout` / `editor_layout` / `diff_view_layout` each ship with a trait
    // default that is a pure function of `Backend::char_width()` / `Backend::line_height()`
    // (plus, for `diff_view_layout`, `DiffView::mode`) — the exact same values every
    // backend's own `draw_terminal` / `draw_editor` / `draw_diff_view` already resolves
    // them to. There is no backend-specific measurement these three could add, unlike
    // `board_layout` / `list_layout`, which route through backend-native constants and
    // have no default at all. All four backends take the default as the honest answer.
    //
    // `diff_view_layout`'s default still holds after the row/pane geometry lift: its
    // default body keeps its own copy of the pane math rather than delegating to
    // `DiffView::layout`, differing only in one degenerate corner.
    //
    // `terminal_scrollbar_default_width` backs `terminal_layout`'s scrollbar-gutter
    // reservation. GTK, macOS, and Win all default `TerminalScrollbar::width` to 8px —
    // exactly the trait default — so the three take it as the honest answer. TUI
    // overrides it to `1.0` (one cell).
    (
        "gtk",
        "terminal_scrollbar_default_width",
        "pixel backend default (8px) matches draw_terminal's own `unwrap_or(8.0)` fallback \
         (issue #506 review fix)",
    ),
    (
        "macos",
        "terminal_scrollbar_default_width",
        "pixel backend default (8px) matches draw_terminal's own `unwrap_or(8.0)` fallback \
         (issue #506 review fix)",
    ),
    (
        "win",
        "terminal_scrollbar_default_width",
        "pixel backend default (8px) matches draw_terminal's own `unwrap_or(8.0)` fallback \
         (issue #506 review fix)",
    ),
    (
        "tui",
        "terminal_layout",
        "pure fn of char_width()/line_height() — see the block comment above (#506)",
    ),
    (
        "tui",
        "editor_layout",
        "pure fn of char_width()/line_height() — see the block comment above (#506)",
    ),
    (
        "tui",
        "diff_view_layout",
        "pure fn of line_height() + DiffView::mode — see the block comment above (#506)",
    ),
    (
        "gtk",
        "terminal_layout",
        "pure fn of char_width()/line_height() — see the block comment above (#506)",
    ),
    (
        "gtk",
        "editor_layout",
        "pure fn of char_width()/line_height() — see the block comment above (#506)",
    ),
    (
        "gtk",
        "diff_view_layout",
        "pure fn of line_height() + DiffView::mode — see the block comment above (#506)",
    ),
    (
        "macos",
        "terminal_layout",
        "pure fn of char_width()/line_height() — see the block comment above (#506)",
    ),
    (
        "macos",
        "editor_layout",
        "pure fn of char_width()/line_height() — see the block comment above (#506)",
    ),
    (
        "macos",
        "diff_view_layout",
        "pure fn of line_height() + DiffView::mode — see the block comment above (#506)",
    ),
    (
        "win",
        "terminal_layout",
        "pure fn of char_width()/line_height() — see the block comment above (#506)",
    ),
    (
        "win",
        "editor_layout",
        "pure fn of char_width()/line_height() — see the block comment above (#506)",
    ),
    (
        "win",
        "diff_view_layout",
        "pure fn of line_height() + DiffView::mode — see the block comment above (#506)",
    ),
    // `canvas_layout` is `CanvasLayout { bounds: rect }` — a pure function of `rect`
    // alone, with no backend-specific metric. All four backends take the default.
    (
        "tui",
        "canvas_layout",
        "pure identity fn of rect — Canvas::layout has no backend-specific metric (#1102)",
    ),
    (
        "gtk",
        "canvas_layout",
        "pure identity fn of rect — Canvas::layout has no backend-specific metric (#1102)",
    ),
    (
        "macos",
        "canvas_layout",
        "pure identity fn of rect — Canvas::layout has no backend-specific metric (#1102)",
    ),
    (
        "win",
        "canvas_layout",
        "pure identity fn of rect — Canvas::layout has no backend-specific metric (#1102)",
    ),
    // `scrollbar_reserve` is the width of *toolkit* scrollbar chrome that a backend paints
    // over the content edge. GTK is the only backend that hosts its content inside
    // something with such chrome (a `ScrolledWindow`). The other three draw into a surface
    // they own end to end, and any scrollbar visible in them is quadraui's own `Scrollbar`
    // primitive, laid out *inside* the rect, not floated on top. `0.0` is the answer.
    (
        "tui",
        "scrollbar_reserve",
        "no toolkit overlay chrome — a terminal has no scrollbar of its own, and the `Scrollbar` \
         primitive TUI paints occupies a real cell column inside the caller's rect (#776)",
    ),
    (
        "macos",
        "scrollbar_reserve",
        "no toolkit overlay chrome — `MacBackend` draws into a `CGContext` it owns, not an \
         `NSScrollView`, and paints the `Scrollbar` primitive inside the caller's rect (#776)",
    ),
    (
        "win",
        "scrollbar_reserve",
        "no toolkit overlay chrome — Direct2D paints into the HWND's render target directly, with \
         no native scrollbar floated over the content edge; unrelated to the #19 stub gaps above \
         (#776)",
    ),
    // `measure` bundles `char_width()`/`line_height()` into one `Metrics` value. Its trait
    // default is a pure function of those two *required* methods, so every backend's default
    // answer is that backend's own real metrics. There is nothing backend-specific left to add.
    (
        "tui",
        "measure",
        "pure fn of char_width()/line_height() — see the block comment above (#506); nothing \
         backend-specific to add (#817)",
    ),
    (
        "gtk",
        "measure",
        "pure fn of char_width()/line_height() — see the block comment above (#506); nothing \
         backend-specific to add (#817)",
    ),
    (
        "macos",
        "measure",
        "pure fn of char_width()/line_height() — see the block comment above (#506); nothing \
         backend-specific to add (#817)",
    ),
    (
        "win",
        "measure",
        "pure fn of char_width()/line_height() — see the block comment above (#506); nothing \
         backend-specific to add (#817)",
    ),
    // `set_caret_shape` drives the terminal/OS *hardware* caret to match an Editor's shape.
    // TUI is the only backend with a hardware caret distinct from the one it paints (DECSCUSR).
    // GTK/macOS/Win-GUI all paint their own caret glyph as part of `draw_editor` and have no
    // separate hardware cursor to steer, so the trait's no-op default is the honest answer.
    (
        "gtk",
        "set_caret_shape",
        "GtkBackend paints its own caret in draw_editor; there is no separate hardware caret to \
         steer (#1015)",
    ),
    (
        "macos",
        "set_caret_shape",
        "MacBackend paints its own caret in draw_editor; there is no separate hardware caret to \
         steer (#1015)",
    ),
    (
        "win",
        "set_caret_shape",
        "WinBackend paints its own caret in draw_editor; there is no separate hardware caret to \
         steer (#1015)",
    ),
    // `minimap_scale`/`set_minimap_scale` pick the on-screen size of one character cell in
    // `MinimapRenderMode::Characters`. TUI's braille rasteriser has no font to scale at all
    // (it packs 4 buffer lines x 2 columns into one cell-native `U+2800`-block dot cell),
    // so the trait's no-op default is the correct answer. GTK/macOS/Win-GUI override both.
    (
        "tui",
        "minimap_scale",
        "braille has no font to scale — the cell-native dot packing described in \
         primitives::minimap's module doc has no `MinimapScale` concept to hold (#1143)",
    ),
    (
        "tui",
        "set_minimap_scale",
        "braille has no font to scale — the cell-native dot packing described in \
         primitives::minimap's module doc has no `MinimapScale` concept to hold (#1143)",
    ),
    // `set_current_line_height`/`set_current_char_width` let a host re-apply a pixel
    // backend's font-derived metrics. TUI is a fixed-cell backend — its line height and
    // character width are `1.0` by construction — so there is no font-derived value for a
    // host to ever re-apply, and the no-op default is the honest answer. GTK/macOS/Win-GUI
    // override both.
    (
        "tui",
        "set_current_line_height",
        "fixed-cell backend — line_height() is 1.0 by construction, so there is no font-derived \
         value to re-apply (#1086)",
    ),
    (
        "tui",
        "set_current_char_width",
        "fixed-cell backend — char_width() is 1.0 by construction, so there is no font-derived \
         value to re-apply (#1086)",
    ),
    // `effective_menu_style` is `menu_style().resolve(&backend_caps())` — built purely
    // from two methods every backend already supplies. There is nothing backend-specific
    // left to override.
    (
        "tui",
        "effective_menu_style",
        "menu_style().resolve(&backend_caps()) — derived purely from two already-overridden/\
         required methods, nothing backend-specific to add (#1187)",
    ),
    (
        "gtk",
        "effective_menu_style",
        "menu_style().resolve(&backend_caps()) — derived purely from two already-overridden/\
         required methods, nothing backend-specific to add (#1187)",
    ),
    (
        "macos",
        "effective_menu_style",
        "menu_style().resolve(&backend_caps()) — derived purely from two already-overridden/\
         required methods, nothing backend-specific to add (#1187)",
    ),
    (
        "win",
        "effective_menu_style",
        "menu_style().resolve(&backend_caps()) — derived purely from two already-overridden/\
         required methods, nothing backend-specific to add (#1187)",
    ),
    // `Style` carries non-colour geometry tokens. TUI takes the trait's no-op default
    // on purpose, not as unfinished work: every `Style` token so far has a TUI story of
    // "no-op" because the TUI rasterisers never read sub-cell geometry at all. There is
    // nothing for a TUI `current_style` field to do yet; the day a cell-quantised token
    // lands, TUI gains a real override.
    (
        "tui",
        "set_style",
        "every Style token so far maps to a TUI no-op (Style's own module doc) — nothing for a \
         TUI-side current_style field to store yet (#1133)",
    ),
    (
        "tui",
        "style",
        "mirrors set_style above — default Style::default() is correct since TUI never stores \
         an override (#1133)",
    ),
    // ── TUI: `paint_surface` exposes the public
    // `PaintSurface` pixel-paint-verb seam — `Rect`/`Point`-taking fills,
    // strokes, rounded rects, text runs. TUI paints a cell grid, not a
    // pixel canvas (no sub-cell `Rect`, no fractional stroke width, no
    // clip rect narrower than a whole cell — see `PaintSurface`'s own
    // module doc, "Why TUI stays out"), so `None` here is the honest
    // structural answer, not a missing override — `TuiBackend` could
    // never implement this trait meaningfully. GTK/macOS/Win all override
    // it because each has a real pixel canvas to hand back.
    (
        "tui",
        "paint_surface",
        "a terminal cell grid has no sub-cell Rect to paint into — PaintSurface's own module \
         doc, \"Why TUI stays out\", explains why TuiBackend never implements this trait (#1101)",
    ),
    // ── TUI: `set_editor_v_scrollbar_width` / `editor_v_scrollbar_width`.
    // The editor's vertical scrollbar on a terminal is one cell wide by
    // construction — a pixel width has no cell grid to land on — so TUI
    // ignores the setter and reports `None`, per the trait doc. GUI
    // backends override both.
    (
        "tui",
        "set_editor_v_scrollbar_width",
        "a terminal scrollbar column is one cell wide; a pixel width has nothing to map to, \
         per Backend::set_editor_v_scrollbar_width's own doc",
    ),
    (
        "tui",
        "editor_v_scrollbar_width",
        "TUI ignores set_editor_v_scrollbar_width, so the always-None default is the truthful \
         answer",
    ),
];

/// The capabilities `name`'s `backend_caps` declares, parsed from source.
///
/// Public so `tests/conformance.rs` can hold the parser to the *running*
/// backend for every backend this build compiles in — the guard that
/// keeps everything else in this module from being a plausible-looking
/// no-op. Panics if `name` is not a backend in the tree.
pub fn declared_in_source(name: &str) -> BTreeSet<&'static str> {
    BACKENDS
        .iter()
        .find(|b| b.name == name)
        .unwrap_or_else(|| panic!("no backend source registered under {name:?}"))
        .declared()
}

// ─── PlatformServices honesty ───────────────────────────────────────────
//
// `BackendCaps`/`CAP_CONTRACTS` above polices `Backend`'s defaulted
// methods: declare a capability and the matching override must exist, in
// both directions. `PlatformServices` had no equivalent — every one of
// its methods was on the honour system, and `TuiPlatformServices::open_url`
// shipped as `fn open_url(&self, _url: &str) {}` with no `BackendCaps`
// entry at all: a `()`-returning no-op with nothing anywhere for an app
// to check, the textbook case this module exists to rule out for
// `Backend` itself.
//
// The fix mirrors `CAP_CONTRACTS`'s shape rather than reusing it
// directly: `PlatformServices` methods aren't gated by a `BackendCaps`
// bool the way `Backend`'s optional surfaces are — some *are* (`notifications`,
// `file_dialogs`, …), but others report absence through a typed return
// instead (`ServiceResult`'s `_result`-twin pattern, D-009 seam 2). Both
// are legitimate honesty stories; a bare `()` return with *neither* is
// the one thing this file exists to catch.

/// Every `PlatformServices` method, in trait-declaration order:
/// `(name, has_default_body, returns_service_result, is_bare_unit)`.
///
/// Parsed from `src/backend.rs` with the same technique
/// [`defaulted_trait_methods`] uses for `Backend` — source is the only
/// place "does this method return a typed signal or a bare `()`" can be
/// read at all, since both compile to the same trait-object call.
pub fn platform_services_methods() -> Vec<(&'static str, bool, bool, bool)> {
    let mut lines = BACKEND_TRAIT_SRC
        .lines()
        .skip_while(|l| *l != "pub trait PlatformServices {");
    assert!(
        lines.next().is_some(),
        "src/backend.rs: no line reads exactly `pub trait PlatformServices {{` — the trait \
         header moved or was reformatted, and an empty parse here would report every method as \
         honestly covered"
    );
    let body: Vec<&str> = lines.take_while(|l| *l != "}").collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        // Exactly one indent level, so nested items are excluded.
        if !(body[i].starts_with("    fn ") && !body[i].starts_with("     ")) {
            i += 1;
            continue;
        }
        let name = fn_name(body[i].trim_start());
        // Walk the signature to its end: `;` = required, `{` = defaulted.
        let mut j = i;
        let mut sig = String::new();
        while j < body.len() && !body[j].contains('{') && !body[j].trim_end().ends_with(';') {
            sig.push_str(body[j].trim());
            sig.push(' ');
            j += 1;
        }
        if j < body.len() {
            sig.push_str(body[j].trim());
        }
        if let Some(name) = name {
            let has_default = j < body.len() && body[j].contains('{');
            let returns_service_result = sig.contains("-> ServiceResult");
            let is_bare_unit = !sig.contains("->");
            out.push((name, has_default, returns_service_result, is_bare_unit));
        }
        i = j + 1;
    }
    out
}

/// How a `PlatformServices` method's absence on a given backend can be
/// discovered by an app — the `PlatformServices` counterpart to [`Proof`]
/// above.
pub enum ServiceHonesty {
    /// A [`BackendCaps`] field a caller reads before trusting this
    /// method's answer — the same "`None`-is-ambiguous-without-it" shape
    /// `file_dialogs` / `folder_dialogs` / `native_dialogs` /
    /// `notifications` already use.
    CapBacked(&'static str),
    /// This method returns a bare `()`, so nothing on the method itself
    /// can report absence — but a `<name>_result` twin exists, returning
    /// `ServiceResult<()>` (D-009 seam 2, the pattern
    /// `Clipboard::write_text`/`write_text_result` already established),
    /// and that is the honest entry point a caller who cares should use.
    ResultTwin(&'static str),
    /// Vends another trait object (`Clipboard`) whose own methods carry
    /// their own honesty story — not this trait's job to police further.
    Delegates(&'static str),
    /// Always meaningful on every backend — there is no "unsupported" a
    /// caller could ever need to detect (a plain identifier, or a method
    /// that already returns a fully typed, unambiguous signal on its
    /// own).
    AlwaysReal,
}

/// Every `PlatformServices` method, paired with its honesty story.
///
/// `platform_service_contracts_cover_every_method` asserts this names
/// exactly [`platform_services_methods`], in trait-declaration order — a
/// new method with no entry here is silently exempt from the whole
/// check, the same "silence reads as a pass" failure `CAP_CONTRACTS`
/// already guards against on the `Backend` side.
pub const PLATFORM_SERVICE_CONTRACTS: &[(&str, ServiceHonesty)] = &[
    ("clipboard", ServiceHonesty::Delegates("Clipboard")),
    (
        "show_file_open_dialog",
        ServiceHonesty::CapBacked("file_dialogs"),
    ),
    (
        "show_file_save_dialog",
        ServiceHonesty::CapBacked("file_dialogs"),
    ),
    (
        "show_folder_open_dialog",
        ServiceHonesty::CapBacked("folder_dialogs"),
    ),
    (
        "show_message_dialog",
        ServiceHonesty::CapBacked("native_dialogs"),
    ),
    (
        "send_notification",
        ServiceHonesty::CapBacked("notifications"),
    ),
    ("open_url", ServiceHonesty::ResultTwin("open_url_result")),
    // The twin itself already returns a typed `ServiceResult` — there is
    // nothing further to report absence of.
    ("open_url_result", ServiceHonesty::AlwaysReal),
    // The four `shell.*`-style methods all return `ServiceResult<()>` directly — the
    // `Result` type itself already carries the "unsupported" signal, so there is no
    // separate `BackendCaps` flag or `_result` twin to check.
    ("reveal_in_file_manager", ServiceHonesty::AlwaysReal),
    ("open_path", ServiceHonesty::AlwaysReal),
    ("move_to_trash", ServiceHonesty::AlwaysReal),
    ("beep", ServiceHonesty::AlwaysReal),
    // `system_theme` returns `ServiceResult<SystemTheme>`
    // directly — same reasoning as `open_url_result` just above: the
    // `Result` type itself already carries the "unsupported" signal, so
    // there is no separate `BackendCaps` flag or `_result` twin to check.
    ("system_theme", ServiceHonesty::AlwaysReal),
    // `secret_store` vends a `&dyn SecretStore` rather than
    // answering anything itself — same shape as `clipboard` above, so the
    // honesty story lives on the vended trait's own methods, each of which
    // returns `ServiceResult<...>` (including the no-feature build's
    // always-`Unsupported` store). Nothing for this method to report.
    ("secret_store", ServiceHonesty::Delegates("SecretStore")),
    // `displays`/`cursor_screen_point` both return
    // `ServiceResult<...>` directly — same reasoning as `system_theme`
    // above: the `Result` type itself already carries the "unsupported"
    // signal (GTK/TUI's honest `cursor_screen_point` gap included), so
    // there is no separate `BackendCaps` flag or `_result` twin to check.
    ("displays", ServiceHonesty::AlwaysReal),
    ("cursor_screen_point", ServiceHonesty::AlwaysReal),
    ("platform_name", ServiceHonesty::AlwaysReal),
];

// ─── The checks ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Every capability has a contract, and every contract is a capability. A new
    /// `BackendCaps` field with no `CAP_CONTRACTS` entry would be exempt from the honesty
    /// check by omission.
    #[test]
    fn cap_contracts_cover_every_capability() {
        let vocabulary = BackendCaps::vocabulary();
        let contracted: Vec<&str> = CAP_CONTRACTS.iter().map(|c| c.cap).collect();
        assert_eq!(
            contracted, vocabulary,
            "`CAP_CONTRACTS` must name every `BackendCaps` capability exactly once, in \
             vocabulary order — a capability with no contract is silently unchecked, and a \
             contract for a capability that no longer exists is dead weight (quadraui#492)"
        );
    }

    /// Every `PlatformServices` method has a honesty story, and every
    /// story names a real method. A new method added with no entry in
    /// `PLATFORM_SERVICE_CONTRACTS` would be exempt from
    /// [`every_bare_unit_service_method_has_a_typed_honesty_story`] by
    /// omission — the same "silence reads as a pass" failure mode
    /// `cap_contracts_cover_every_capability` guards against for
    /// `BackendCaps`.
    #[test]
    fn platform_service_contracts_cover_every_method() {
        let parsed: Vec<&str> = platform_services_methods()
            .iter()
            .map(|(name, ..)| *name)
            .collect();
        let contracted: Vec<&str> = PLATFORM_SERVICE_CONTRACTS.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            contracted, parsed,
            "`PLATFORM_SERVICE_CONTRACTS` must name every `PlatformServices` method exactly \
             once, in trait-declaration order — a method with no contract is silently \
             unchecked, and a contract for a method that no longer exists is dead weight \
             (quadraui#949)"
        );
    }

    /// Guard against the parser degrading to "finds nothing", which would
    /// make the two tests above and below pass vacuously for exactly the
    /// reason they exist to catch.
    #[test]
    fn platform_services_parse_sanity() {
        let methods = platform_services_methods();
        assert!(
            methods.len() >= 6,
            "parsed only {} PlatformServices method(s) from src/backend.rs — the trait header \
             moved or was reformatted, and a near-empty parse would make every other check here \
             pass vacuously: {methods:?}",
            methods.len()
        );
        assert!(
            methods.iter().any(|(name, ..)| *name == "open_url_result"),
            "`open_url_result` must be among the parsed methods — it is quadraui#949's own fix; \
             parsed: {methods:?}"
        );
    }

    /// **The honesty check.** A `PlatformServices` method that returns a bare `()`
    /// has no way to report absence on itself. Every such method must be backed by a
    /// [`BackendCaps`] field or have a `<name>_result` twin returning `ServiceResult<...>`.
    /// `AlwaysReal`/`Delegates` are not valid stories for a bare-`()` method.
    #[test]
    fn every_bare_unit_service_method_has_a_typed_honesty_story() {
        let methods = platform_services_methods();
        for (name, _has_default, _returns_service_result, is_bare_unit) in &methods {
            let honesty = PLATFORM_SERVICE_CONTRACTS
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, h)| h)
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: no `PLATFORM_SERVICE_CONTRACTS` entry — should have been \
                         caught by `platform_service_contracts_cover_every_method`"
                    )
                });
            if !is_bare_unit {
                continue;
            }
            match honesty {
                ServiceHonesty::ResultTwin(twin) => {
                    let twin_is_typed = methods.iter().any(|(n, _, ret, _)| n == twin && *ret);
                    assert!(
                        twin_is_typed,
                        "{name}: declared `ResultTwin({twin:?})`, but no `PlatformServices` \
                         method named {twin:?} returns `ServiceResult<...>` — the bare `()` \
                         return has no honesty story after all"
                    );
                }
                ServiceHonesty::CapBacked(cap) => {
                    assert!(
                        BackendCaps::vocabulary().contains(cap),
                        "{name}: declared `CapBacked({cap:?})`, which is not a `BackendCaps` \
                         field — capabilities are {:?}",
                        BackendCaps::vocabulary()
                    );
                }
                ServiceHonesty::Delegates(_) | ServiceHonesty::AlwaysReal => panic!(
                    "{name}: returns a bare `()` with a {:?} honesty story — that is exactly \
                     the undetectable silent no-op quadraui#949 exists to catch (see \
                     `TuiPlatformServices::open_url` before its fix). Add a `<name>_result` \
                     twin returning `ServiceResult<()>`, or gate it behind a `BackendCaps` \
                     field.",
                    match honesty {
                        ServiceHonesty::Delegates(d) => format!("Delegates({d:?})"),
                        ServiceHonesty::AlwaysReal => "AlwaysReal".to_string(),
                        _ => unreachable!(),
                    }
                ),
            }
        }
    }

    /// Guard against the parser degrading to "finds nothing", which would
    /// make [`backends_declare_only_what_they_override`] pass vacuously
    /// for exactly the reason it exists to catch.
    #[test]
    fn parse_sanity() {
        for b in BACKENDS {
            let overrides = b.overrides();
            assert!(
                overrides.len() >= 30,
                "{}: parsed only {} overridden method(s) from {:?} — every backend implements \
                 dozens of required `Backend` methods, so this is a broken parse, not a small \
                 backend: {overrides:?}",
                b.path,
                overrides.len(),
                b.header
            );
            assert!(
                overrides.contains("backend_caps"),
                "{}: `backend_caps` itself is not among the parsed overrides — the parse is \
                 wrong, since `Backend::backend_caps` has no default and every backend must \
                 implement it",
                b.path
            );
        }
    }

    /// **The honesty check.** For every backend in the tree and every capability with
    /// a checkable contract: declared ⇒ the methods are overridden, and not declared ⇒
    /// they are untouched defaults.
    ///
    /// Prints a backend × capability table first, so the *shape* of what each backend
    /// claims is visible in the log even on a green run.
    #[test]
    fn backends_declare_only_what_they_override() {
        let mut table = String::from("\nCapability declarations (backend × capability)\n");
        let name_w = BACKENDS
            .iter()
            .map(|b| b.name.len())
            .chain(std::iter::once("backend".len()))
            .max()
            .unwrap_or(7);
        // Wide enough for the longest marker ("decl*"), or the column
        // header if that is longer — otherwise a marker overflows its
        // cell and shunts every column to its right out of alignment.
        let cap_w = |c: &str| c.len().max("decl*".len());

        table.push_str(&format!("{:<name_w$}", "backend"));
        for c in CAP_CONTRACTS {
            table.push_str(&format!("  {:<w$}", c.cap, w = cap_w(c.cap)));
        }
        table.push('\n');

        let mut lies: Vec<String> = Vec::new();
        for b in BACKENDS {
            let declared = b.declared();
            let overrides = b.overrides();
            table.push_str(&format!("{:<name_w$}", b.name));

            for contract in CAP_CONTRACTS {
                let cap = contract.cap;
                let is_declared = declared.contains(cap);
                let present: Vec<&str> = contract
                    .proof
                    .methods()
                    .iter()
                    .copied()
                    .filter(|m| overrides.contains(m))
                    .collect();
                let absent: Vec<&str> = contract
                    .proof
                    .methods()
                    .iter()
                    .copied()
                    .filter(|m| !overrides.contains(m))
                    .collect();

                let honest = match (&contract.proof, is_declared) {
                    // Unprovable either way: nothing to contradict.
                    (Proof::Unprovable(_), _) => true,
                    (Proof::All(_), true) => absent.is_empty(),
                    (Proof::Any(_), true) => !present.is_empty(),
                    // Undeclared: *nothing* may be overridden, under
                    // either quantifier. A half-built capability is a
                    // capability to finish or a method to delete, not a
                    // reason to leave the declaration false.
                    (_, false) => present.is_empty(),
                };

                let mark = match (&contract.proof, is_declared, honest) {
                    (_, _, false) => "LIE",
                    (Proof::Unprovable(_), true, _) => "decl*",
                    (Proof::Unprovable(_), false, _) => "-*",
                    (_, true, _) => "decl",
                    (_, false, _) => "-",
                };
                table.push_str(&format!("  {mark:<w$}", w = cap_w(cap)));

                if !honest {
                    lies.push(match (&contract.proof, is_declared) {
                        (Proof::All(all), true) => format!(
                            "{}/{cap}: declared, but {absent:?} {} still the trait's no-op \
                             default (all of {all:?} must be overridden) — {}",
                            b.name,
                            if absent.len() == 1 { "is" } else { "are" },
                            b.path
                        ),
                        (Proof::Any(any), true) => format!(
                            "{}/{cap}: declared, but none of {any:?} is overridden — {}",
                            b.name, b.path
                        ),
                        (_, false) => format!(
                            "{}/{cap}: NOT declared, yet {present:?} {} overridden — either \
                             declare the capability or drop the override; as it stands every \
                             scenario requiring {cap:?} skips on this backend even though it \
                             works — {}",
                            b.name,
                            if present.len() == 1 { "is" } else { "are" },
                            b.path
                        ),
                        (Proof::Unprovable(_), true) => unreachable!("unprovable is never a lie"),
                    });
                }
            }
            table.push('\n');
        }
        table.push_str("\n  decl = declared   - = not declared   * = unprovable from source\n");
        for c in CAP_CONTRACTS {
            if let Proof::Unprovable(why) = &c.proof {
                table.push_str(&format!("  * {}: {why}\n", c.cap));
            }
        }
        println!("{table}");

        assert!(
            lies.is_empty(),
            "{} dishonest capability declaration(s) — a `BackendCaps` field must match what \
             the backend's source actually overrides, in both directions (quadraui#492 \
             acceptance item 2):\n{}\n{table}",
            lies.len(),
            lies.join("\n")
        );
    }

    /// The no-op defaults that neither `CAP_CONTRACTS` nor C0 covers — `set_theme`
    /// and friends — are each either overridden or written down in [`ACCEPTED_DEFAULTS`]
    /// with a reason. An undeclared gap is a failure, which distinguishes a known gap
    /// from silence.
    #[test]
    fn silent_defaults_are_declared_gaps_not_silence() {
        let capability_methods: BTreeSet<&str> = CAP_CONTRACTS
            .iter()
            .flat_map(|c| c.proof.methods().iter().copied())
            .collect();
        let watched: Vec<&str> = defaulted_trait_methods()
            .into_iter()
            // `draw_*` defaults are C0's job, and a hard failure there.
            .filter(|m| !m.starts_with("draw_"))
            .filter(|m| !capability_methods.contains(m))
            .collect();
        assert!(
            !watched.is_empty(),
            "parsed no defaulted non-capability `Backend` methods at all — the trait parser is \
             broken, and an empty watch list would make this test pass vacuously"
        );
        assert!(
            watched.contains(&"set_theme"),
            "`set_theme` must be watched here — it is quadraui#492's headline example. Parsed: \
             {watched:?}"
        );

        let mut table = String::from("\nNo-op defaults outside `BackendCaps` (backend × method)\n");
        let name_w = BACKENDS
            .iter()
            .map(|b| b.name.len())
            .chain(std::iter::once("backend".len()))
            .max()
            .unwrap_or(7);
        let col_w = |m: &str| m.len().max("default".len());
        table.push_str(&format!("{:<name_w$}", "backend"));
        for m in &watched {
            table.push_str(&format!("  {:<w$}", m, w = col_w(m)));
        }
        table.push('\n');

        let mut undeclared: Vec<String> = Vec::new();
        for b in BACKENDS {
            let overrides = b.overrides();
            table.push_str(&format!("{:<name_w$}", b.name));
            for m in &watched {
                let declared_gap = ACCEPTED_DEFAULTS
                    .iter()
                    .any(|(backend, method, _)| *backend == b.name && method == m);
                let mark = if overrides.contains(m) {
                    "ok"
                } else if declared_gap {
                    "GAP"
                } else {
                    "SILENT"
                };
                table.push_str(&format!("  {mark:<w$}", w = col_w(m)));
                if mark == "SILENT" {
                    undeclared.push(format!(
                        "{}/{m}: takes the trait's no-op default, and `ACCEPTED_DEFAULTS` does \
                         not say why — {}",
                        b.name, b.path
                    ));
                }
            }
            table.push('\n');
        }
        table.push_str("\n  ok = overridden   GAP = declared gap (see ACCEPTED_DEFAULTS)\n");
        println!("{table}");

        assert!(
            undeclared.is_empty(),
            "{} silently-defaulted `Backend` method(s) — each discards whatever the caller \
             handed it, and nothing in a green build says so (quadraui#492). Override it, or \
             add it to `ACCEPTED_DEFAULTS` with the reason:\n{}\n{table}",
            undeclared.len(),
            undeclared.join("\n")
        );

        // The reverse direction, so the checklist can't rot: an entry
        // whose override has since landed (or whose method is gone) has
        // to be deleted, which is how the list shrinks as gaps close.
        let stale: Vec<String> = ACCEPTED_DEFAULTS
            .iter()
            .filter(
                |(backend, method, _)| match BACKENDS.iter().find(|b| b.name == *backend) {
                    None => true,
                    Some(b) => !watched.contains(method) || b.overrides().contains(method),
                },
            )
            .map(|(backend, method, _)| format!("{backend}/{method}"))
            .collect();
        assert!(
            stale.is_empty(),
            "`ACCEPTED_DEFAULTS` still excuses {stale:?}, but each is now overridden (or is no \
             longer a defaulted trait method / known backend) — delete the stale entr(ies) so \
             the list stays a live checklist"
        );
    }

    /// The parser must distinguish `cancel_text_selection_drag` from the inherent
    /// helper `cancel_text_selection_drag_impl` that both TUI and GTK keep beside it.
    /// A prefix match would see the helper and clear a backend that never wired the trait method.
    #[test]
    fn fn_name_matches_whole_names_not_prefixes() {
        assert_eq!(
            fn_name("fn cancel_text_selection_drag(&mut self) {"),
            Some("cancel_text_selection_drag")
        );
        assert_eq!(
            fn_name("fn cancel_text_selection_drag_impl(&mut self) {"),
            Some("cancel_text_selection_drag_impl")
        );
        assert_eq!(fn_name("fn draw_chart<T>(&mut self) {"), Some("draw_chart"));
        assert_eq!(fn_name("let x = 1;"), None);
    }
}
