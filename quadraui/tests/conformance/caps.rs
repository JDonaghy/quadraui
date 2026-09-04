//! Capability **honesty check** (quadraui#492 acceptance item 2, epic
//! #480).
//!
//! [`quadraui::BackendCaps`] is a backend saying "these optional surfaces
//! are real on me". C0 (`c0.rs`) proves the *required* `draw_*` surface
//! paints something; this module proves the *optional* surface isn't
//! lying, in both directions:
//!
//! - **Declared ⇒ overridden.** A backend that reports
//!   `text_selection: true` must actually override
//!   `register_text_region` *and* `cancel_text_selection_drag`. Otherwise
//!   the trait's no-op default silently eats every registration and the
//!   conformance runner cheerfully *runs* the selection scenarios it
//!   should have skipped.
//! - **Not declared ⇒ untouched default.** The reverse is just as much a
//!   lie, and a more insidious one: a backend that overrides `set_cursor`
//!   but forgets `pointer_cursor: true` makes every scenario gating on it
//!   skip forever, and a skip row looks like a known gap rather than a
//!   stale declaration.
//!
//! This is not a hypothetical. quadraui#492's own review found GTK
//! declaring `text_selection: true` while `cancel_text_selection_drag`
//! was still the trait default — exactly this class of drift, shipped in
//! the same PR that introduced `BackendCaps`, and invisible because this
//! check did not exist yet.
//!
//! ## Why source parsing
//!
//! "Is this trait method overridden, or is it the default?" is not a
//! question a running program can ask: a defaulted method and an
//! overriding one are the same call at the same vtable slot. The only
//! place the answer exists is the source, so that is where this reads it
//! — the same technique `c0::tests::cases_cover_every_draw_method_on_the_trait`
//! already uses to hold `CASES` to the trait.
//!
//! Two things keep that from being a fragile trick:
//!
//! 1. [`declared_in_source`] is cross-checked against the *running*
//!    backend's `Backend::backend_caps()` for every backend this build
//!    compiles in (`conformance::source_parsed_caps_match_the_running_backend`).
//!    If the parser ever goes stale against reformatted source, that test
//!    fails loudly instead of this one quietly passing on an empty parse.
//! 2. [`parse_sanity`] asserts each impl block yields a plausible number
//!    of methods, so a header rename can't silently degrade every backend
//!    to "overrides nothing" — which would read as a clean bill of health.
//!
//! ## Win and macOS
//!
//! Source parsing has one large bonus over a driver-based check: it works
//! on backends that have no conformance driver, and can't even be
//! compiled on this host. `MacBackend` and `WinBackend` are checked here
//! on every run, on every platform — so quadraui#492's "Win/macOS gaps
//! enumerate as red/skip rows, not silence" holds for the capability half
//! even though the C0 paint half still has no Win/macOS column.

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

/// Every `Backend` method that ships a default body, parsed from the
/// trait itself.
///
/// quadraui#492's Problem section counts 13 such methods and calls them
/// the bug: a new backend compiles while silently discarding whatever
/// they carry. `CAP_CONTRACTS` covers the nine that a `BackendCaps`
/// field gates and `c0::CASES` covers the defaulted `draw_*` ones, which
/// leaves a remainder — `set_theme`, `set_nerd_fonts`, `set_editor_font`,
/// `scales_text_rows`, `editor_col_at_x`, `register_zone` — that nothing
/// else in this suite looks at. Those are the issue's *headline* example
/// ("discarding the theme — the Win stub takes this default today") and
/// its `editor_col_at_x` example, so they get a table of their own.
///
/// Derived from source rather than listed, so a new defaulted method
/// enters the report the moment it is added to the trait.
pub fn defaulted_trait_methods() -> Vec<&'static str> {
    let mut lines = BACKEND_TRAIT_SRC
        .lines()
        .skip_while(|l| *l != "pub trait Backend {");
    assert!(
        lines.next().is_some(),
        "src/backend.rs: no line reads exactly `pub trait Backend {{` — the trait header moved, \
         and an empty parse here would report every backend as complete"
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
    // ── TUI: a fixed-cell backend, so the font-shaped methods have no
    // meaning rather than being unfinished.
    (
        "tui",
        "set_editor_font",
        "fixed-cell backend — every glyph already occupies exactly one terminal cell, so there \
         is no font to override (the trait's own doc says so)",
    ),
    (
        "tui",
        "set_ui_font",
        "same fixed-cell reason as `set_editor_font` — chrome glyphs are terminal cells too, so \
         there is no chrome font description to honour (#624)",
    ),
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
    // ── GTK, macOS, Win: `snap_height` is deliberately unimplemented on
    // every pixel backend. The trait's default (identity) *is* the right
    // answer for them — they paint fractional heights exactly, unlike
    // TUI's cell grid — so there is nothing to override (quadraui#632).
    (
        "gtk",
        "snap_height",
        "pixel backend — Cairo paints fractional heights exactly, so the identity default is \
         correct, not unfinished work (quadraui#632)",
    ),
    // ── macOS: real backend, genuinely unfinished. `MacDriver`
    // (quadraui#493) gives it a `ConformanceDriver` and a Tier-1
    // (`backends()`) row, but it's deliberately still absent from
    // `c0_paint_smoke`'s columns — `draw_diff_view`'s known fake (see this
    // module's doc comment) would turn straight into a hard C0 failure,
    // which is its own follow-up, not this list's job.
    (
        "macos",
        "set_editor_font",
        "editor font override not wired to CoreText yet",
    ),
    (
        "macos",
        "set_ui_font",
        "chrome font override not wired to CoreText yet — macOS chrome still paints with the \
         renderer's own font (#624)",
    ),
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
        "snap_height",
        "pixel backend — CoreText paints fractional heights exactly, so the identity default is \
         correct, not unfinished work (quadraui#632)",
    ),
    // ── Win: every method is a `todo!()` stub (#19). Listed one by one
    // anyway — "the whole backend is a stub" is exactly the kind of
    // blanket excuse that outlives the stub. `set_theme`/`set_ui_font`
    // are no longer here (#724): both are now overridden — `set_theme`
    // stores `current_theme` for every `draw_*` rasteriser that used to
    // fall back to `Theme::default()`, and `set_ui_font` builds a chrome
    // `IDWriteTextFormat` alongside the editor one.
    ("win", "set_nerd_fonts", "stub backend — see #19"),
    ("win", "scales_text_rows", "stub backend — see #19"),
    ("win", "editor_col_at_x", "stub backend — see #19"),
    ("win", "register_zone", "stub backend — see #19"),
    (
        "win",
        "tab_bar_layout_with_chrome",
        "stub backend — see #19",
    ),
    (
        "win",
        "snap_height",
        "pixel backend — DirectWrite paints fractional heights exactly, so the identity default \
         is correct, not unfinished work (quadraui#632); unrelated to the #19 stub gaps above",
    ),
    // ── issue #506: `terminal_layout` / `editor_layout` / `diff_view_layout`
    // each ship with a trait default that is a pure function of
    // `Backend::char_width()` / `Backend::line_height()` (plus, for
    // `diff_view_layout`, `DiffView::mode`) — the exact same values every
    // backend's own `draw_terminal` / `draw_editor` / `draw_diff_view`
    // already resolves them to (see `Backend::editor_layout`'s doc for the
    // per-backend trace: `GtkBackend`/`WinBackend`/`MacBackend` all pass
    // `current_char_width` / `current_line_height`, which is exactly what
    // the two accessor methods return; TUI's fixed `(1.0, 1.0)` matches its
    // uniform cell grid). There is no backend-specific measurement these
    // three could add — unlike `board_layout` / `list_layout` (also new in
    // #506), which route through backend-native constants
    // (`BoardMeasure`'s column/card sizing, `ListView`'s scrollbar
    // reservation) and so have no default at all. Declaring all four
    // backends here for all three methods is the honest statement that the
    // default *is* the answer, not a stand-in for one.
    //
    // ── issue #737 revisit: `diff_view_layout`'s exemption above still
    // holds after the row/pane geometry lift. Before #737 the row/pane
    // math (pane widths, divider position, the scroll-clamped visible-line
    // window) existed four times — once inline in this trait's own
    // default body, and once more in each of gtk/macos/tui's own
    // `draw_diff_view` — and this file's job was only ever "does a
    // backend override the *trait method*", which was already true and
    // stayed true. #737 didn't change that: it added
    // `primitives::diff_view::DiffView::layout` as the *fifth* place that
    // math could have lived and made it the *only* place it does — gtk,
    // macos, tui, and win's `draw_diff_view` all call it now instead of
    // re-deriving. `Backend::diff_view_layout`'s default body deliberately
    // keeps its own compact copy rather than delegating to
    // `DiffView::layout`: the two differ in one degenerate corner
    // (`line_height <= 0` with unified rows present) that has never been
    // observed from a real backend's `line_height()`, and collapsing them
    // risked changing behaviour in that corner for a mechanical
    // clarity-only refactor with no accompanying scenario to guard it.
    // Still zero backends override the method, so the honesty check's
    // verdict is unchanged — this note exists so the "revisit" isn't
    // silent.
    // ── issue #506 review fix: `terminal_scrollbar_default_width` backs
    // `terminal_layout`'s scrollbar-gutter reservation (a `Terminal` with
    // `scrollbar: Some(TerminalScrollbar { width: None, .. })` needs a
    // fallback gutter width before it can compute `grid_cols`). GTK,
    // macOS, and Win all default a `None` `TerminalScrollbar::width` to
    // 8px (`sb_width: … .unwrap_or(8.0)` in each backend's own
    // `draw_terminal`) — exactly the trait default — so the three take it
    // as the honest answer, not a stand-in for one. TUI overrides it to
    // `1.0` (one cell), matching `src/tui/terminal.rs`'s
    // `sb_cols: … .unwrap_or(1)`.
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
    // ── issue #776: `scrollbar_reserve` is the width of *toolkit* scrollbar
    // chrome that a backend paints over the content edge, which a caller
    // must subtract before it computes a content viewport width. GTK is
    // the only backend that hosts its content inside something with such
    // chrome (a `ScrolledWindow` and its overlay scrollbar), so GTK is the
    // only backend that overrides it. The other three draw straight into a
    // surface they own end to end — a terminal cell grid, a `CGContext`, an
    // HWND's Direct2D render target — and any scrollbar visible in them is
    // quadraui's own `Scrollbar` primitive, laid out *inside* the rect the
    // caller already passed rather than floated on top of it. Reserving a
    // second gutter for it would double-count. Unlike the `#19` Win stub
    // rows above, these are not unfinished work: `0.0` is the answer, and
    // it does not change when the Win rasterisers land.
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

// ─── The checks ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Every capability has a contract, and every contract is a
    /// capability. A new `BackendCaps` field with no `CAP_CONTRACTS`
    /// entry would be exempt from the honesty check by omission — the
    /// same "silence reads as a pass" failure mode #480 exists to kill.
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

    /// **The honesty check.** For every backend in the tree and every
    /// capability with a checkable contract: declared ⇒ the methods are
    /// overridden, and not declared ⇒ they are untouched defaults.
    ///
    /// Prints a backend × capability table first, so the *shape* of what
    /// each backend claims is visible in the log even on a green run —
    /// quadraui#492's "gaps enumerate, never silence", extended to the
    /// two backends (Win, macOS) that have no C0 column at all.
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

    /// The no-op defaults that neither `CAP_CONTRACTS` nor C0 covers —
    /// `set_theme` and friends — are each either overridden or written
    /// down in [`ACCEPTED_DEFAULTS`] with a reason.
    ///
    /// quadraui#492 review, non-blocking note 2: the issue's headline
    /// example is "a new backend compiles while discarding the theme
    /// (`set_theme` — the Win stub takes this default today)", and
    /// neither C0 (draw methods only) nor `BackendCaps` (the ten named
    /// capabilities) says anything about it. This is where that stops
    /// being invisible. It is deliberately *not* a hard failure — the Win
    /// backend is a declared stub and macOS is unfinished, so a red row
    /// per gap would just be permanently red — but an *undeclared* gap is
    /// a failure, which is the whole difference between a known gap and
    /// silence.
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

    /// The parser must distinguish `cancel_text_selection_drag` from the
    /// inherent helper `cancel_text_selection_drag_impl` that both TUI
    /// and GTK keep beside it. A prefix match would see the helper and
    /// clear a backend that never wired the trait method — the precise
    /// drift quadraui#492's review caught by hand.
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
