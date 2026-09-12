//! Guard test: an un-cfg-gated `todo!()`/`unimplemented!()` inside one of
//! the four real per-platform `Backend` implementations (`TuiBackend`,
//! `GtkBackend`, `MacBackend`, `WinBackend`) is a latent host abort in every
//! consuming app the instant a generic `AppLogic` calls it through
//! `&mut dyn Backend` — see quadraui#923. That already happened once:
//! `MacBackend::draw_minimap`/`minimap_layout`/`draw_image` were bare,
//! un-gated `todo!()`s that took down vimcode's macOS GUI on first paint
//! (vimcode#896, fixed in `dbb3023`).
//!
//! A raw `todo!()` count cannot tell that dangerous case apart from
//! Win-GUI's dozens of *legitimate* stubs, each shaped like:
//!
//! ```ignore
//! #[cfg(target_os = "windows")]
//! if let Some(surface) = &self.surface { /* real Direct2D code */ return; }
//! #[cfg(not(target_os = "windows"))]
//! todo!("Direct2D scrollbar rasteriser (no surface attached yet)")
//! ```
//!
//! On a real Windows target the `todo!()` above is compiled out entirely —
//! these exist purely so `cargo check --features win` can type-check
//! `WinBackend`'s trait completeness on Linux (the "ubuntu win compile
//! gate"). A checker that just counts `todo!()`/`unimplemented!()` would be
//! red on that legitimate scaffolding from day one, get muted, and then
//! miss the next macOS-shaped case exactly as today's absence of any test
//! did — which is the whole finding of #923.
//!
//! ## The wrong predicate this test does NOT use
//!
//! #923 was originally filed on the (wrong) predicate "is there a
//! `#[cfg(not(target_os = …))]` *somewhere nearby*". `#[cfg(...)]` binds to
//! exactly one following statement or item, not to a neighbourhood, so a
//! proximity check is fooled by this extremely common shape:
//!
//! ```ignore
//! #[cfg(target_os = "windows")]
//! if let (Some(surface), Some(dwrite)) = (&self.surface, &self.dwrite) {
//!     /* real Direct2D code */
//!     return;
//! }
//! #[cfg(not(target_os = "windows"))]
//! let _ = (rect, tree);      // <-- the cfg binds to THIS statement only
//! todo!("Direct2D tree rasteriser (no surface attached yet)")   // unconditional!
//! ```
//!
//! Here the `todo!()` carries **no** `cfg` of its own — it compiles (and
//! panics) on every target, Windows included, whenever `self.surface` is
//! `None`. A "cfg somewhere in the last N lines" check would call this
//! safe. It is not.
//!
//! ## The predicate this test does use
//!
//! This test parses each backend's source with [`syn`] — a real Rust
//! grammar, not line-adjacency — and asks, for every `todo!()`/
//! `unimplemented!()` call site: starting from the file's root and walking
//! down through every enclosing `impl`/`fn`/`mod`/`let`/block/expression to
//! this exact call, does **any** `#[cfg(...)]` along that path evaluate to
//! "excluded" when this file is compiled for its own backend's native
//! target? That correctly handles:
//!
//! - a `#[cfg(...)]` directly on the macro call itself (the `begin_frame`/
//!   `end_frame` shape below), and
//! - a `#[cfg(...)]` on an *enclosing* block, `{ let _ = …; todo!(…) }`,
//!   which excludes everything inside it, including a `todo!()` with no
//!   attribute of its own, and
//! - a `#[cfg(...)]` on an enclosing *item* — `win/run.rs`'s
//!   `#[cfg(not(target_os = "windows"))] pub fn run(...) { todo!(...) }` —
//!   several lines above the call, and
//! - `#[cfg(test)] mod tests { … }`, which excludes its contents from every
//!   non-test build regardless of target.
//!
//! ```ignore
//! fn begin_frame(&mut self, viewport: Viewport) {
//!     #[cfg(target_os = "windows")]
//!     if let Some(surface) = &self.surface { /* … */ }
//!     #[cfg(not(target_os = "windows"))]
//!     todo!("ID2D1RenderTarget::BeginDraw()")   // <-- cfg binds directly: SAFE
//! }
//! ```
//!
//! ## Recognised `cfg` spellings (anything else fails loudly)
//!
//! - `cfg(test)` — exempt (never compiled outside a test build).
//! - `cfg(not(test))` — not exempt (this *is* the shipped arm).
//! - `cfg(target_os = "X")` — exempt iff `X` differs from this file's own
//!   native `target_os` (macOS backend ⇒ `"macos"`, Win-GUI ⇒ `"windows"`).
//! - `cfg(not(target_os = "X"))` — exempt iff `X` *matches* this file's
//!   native `target_os`.
//!
//! Refused (`Unclassifiable`, a hard test failure naming the exact
//! attribute rather than a silent pass in either direction — see #923's
//! "make anything unrecognised fail loudly" requirement): `cfg_attr(...)`
//! in any form, `cfg(any(...))`, `cfg(all(...))`, `cfg(not(any(...)))`
//! /`cfg(not(all(...)))`, bare `cfg(unix)`/`cfg(windows)`, a `target_os`
//! comparison inside a backend this test has no native `target_os` for
//! (gtk/tui — see [`BACKENDS`]), or a `cfg(...)` whose inner tokens don't
//! parse as a `syn::Meta` at all.
//!
//! ## Statement-position macro calls
//!
//! `syn` represents a macro invocation used as a mid-block statement
//! differently depending on how it ends: a bare tail expression parses as
//! `Stmt::Expr(Expr::Macro(...), None)` (handled by `visit_expr` above),
//! but a semicolon-terminated or brace-delimited macro *statement* —
//! `todo!("x");` sitting above other code in the same block, e.g.
//! `fn f(&mut self) { todo!("x"); more_code(); }` — parses as the
//! completely separate `Stmt::Macro(StmtMacro)` variant instead. An
//! earlier version of this checker only overrode `visit_expr` and had no
//! `visit_stmt_macro`, so that shape was invisible to it: not `Dangerous`,
//! not `Exempt`, not `Unclassifiable` — simply never visited as a macro
//! call at all. `Walker::visit_stmt_macro` below runs the identical
//! gate-lookup/classification logic for this shape so it can't recur; see
//! `self_test::semicolon_terminated_todo_statement_is_dangerous`.
//!
//! ## Match-arm `#[cfg(...)]` (not applicable today)
//!
//! `syn::Arm` also carries its own `attrs`, so a per-arm `#[cfg(...)]` on a
//! `match` arm is a theoretically distinct gating shape this walker does
//! not special-case (no `visit_arm` override — a `todo!()` inside an arm is
//! only checked against the enclosing fn/block's gate, not the arm's own).
//! No backend uses this pattern today (confirmed by grep across
//! `src/{tui,gtk,macos,win}/`), so it has zero live impact; flagged here so
//! it isn't rediscovered as a silent gap the way statement-position macros
//! were.
//!
//! ## Scope
//!
//! Only `src/tui/`, `src/gtk/`, `src/macos/`, `src/win/` — "each backend's
//! source", the actual `impl Backend for {Tui,Gtk,Mac,Win}Backend` blocks
//! reachable from `quadraui::{tui,gtk,macos,win}::run`. This deliberately
//! excludes `src/testing/mod.rs`'s `RecordingBackend` (a `pub`, always-
//! compiled `Backend` mock for unit tests — its own docs already name the
//! handful of methods it `unimplemented!()`s and tell a caller to use a
//! real backend instead; it never ships as the backend an `AppLogic` is
//! actually run against) and `compose/menu_system.rs`'s private test-only
//! mocks (already behind `#[cfg(test)] mod tests`, which this test's own
//! `cfg(test)` handling would exempt anyway if it were ever asked to look).
//!
//! ## Known debt vs. new regressions
//!
//! Re-measuring with the corrected predicate above finds Win-GUI is not
//! the "83 legitimate stubs" the original #923 body assumed: of the real
//! (non-doc-comment) `todo!()`/`unimplemented!()` call sites in
//! `src/win/{backend,run}.rs`, only 22 are actually excluded from a native
//! Windows build (2 via a direct macro-level `cfg`, 20 via an enclosing
//! `{ }` block's `cfg` — the `minimap_layout`/`list_layout`/… shape) and
//! **59 are unconditional and reachable on Windows itself**, tracked as
//! quadraui#924. Fixing those 59 is explicitly out of scope for #923 ("this
//! issue adds the detector, not the fixes") — [`KNOWN_DANGEROUS_TODOS`] is
//! the frozen baseline of exactly those 59, so this test can both (a) pass
//! today without #924 having to land first, and (b) still fail the instant
//! a *60th* un-gated `todo!()`/`unimplemented!()` shows up anywhere in one
//! of the four backends, whether that's issue #924 regressing further or
//! an unrelated new one. Shrinking [`KNOWN_DANGEROUS_TODOS`] as #924 fixes
//! land is encouraged but not required by this test (a baseline entry with
//! no matching call site left is simply inert).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprArray, ExprAssign, ExprAsync, ExprAwait, ExprBinary, ExprBlock, ExprBreak,
    ExprCall, ExprCast, ExprClosure, ExprConst, ExprContinue, ExprField, ExprForLoop, ExprGroup,
    ExprIf, ExprIndex, ExprInfer, ExprLet, ExprLit, ExprLoop, ExprMacro, ExprMatch, ExprMethodCall,
    ExprParen, ExprPath, ExprRange, ExprReference, ExprRepeat, ExprReturn, ExprStruct, ExprTry,
    ExprTryBlock, ExprTuple, ExprUnary, ExprUnsafe, ExprWhile, ExprYield, ImplItemFn, ItemFn,
    ItemImpl, ItemMod, Local, Macro, Meta, StmtMacro,
};

/// `(source subdirectory, this backend's native `target_os`)`. `None` means
/// the backend has no OS split to reason about (gtk/tui ship cross-platform
/// today) — a `cfg(target_os = "…")`/`cfg(not(target_os = "…"))` found
/// there is refused as [`Gate::Unclassifiable`] rather than guessed at.
const BACKENDS: &[(&str, Option<&str>)] = &[
    ("src/tui", None),
    ("src/gtk", None),
    ("src/macos", Some("macos")),
    ("src/win", Some("windows")),
];

/// Frozen baseline of `(file, enclosing fn, macro argument text)` for every
/// un-gated `todo!()`/`unimplemented!()` this test finds in a real backend
/// today — see the module doc's "Known debt vs. new regressions" section.
/// All 59 are in `src/win/backend.rs`, all tracked as quadraui#924.
const KNOWN_DANGEROUS_TODOS: &[(&str, &str, &str)] = &[
    ("src/win/backend.rs", "fn draw_tree", "\"Direct2D tree rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_list", "\"Direct2D list rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_data_table", "\"Direct2D data table rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn data_table_layout", "\"DirectWrite data table layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_form", "\"Direct2D form rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_palette", "\"Direct2D palette rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_settings_chrome", "\"Direct2D settings chrome rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_status_bar_interactive", "\"Direct2D status bar rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_tab_bar_icons", "\"Direct2D tab bar rasteriser (with per-tab icons) — no surface attached yet\""),
    ("src/win/backend.rs", "fn draw_tab_bar_icons_layout", "\"Direct2D tab bar rasteriser (TabBarLayout, with per-tab icons) — no surface attached yet\""),
    ("src/win/backend.rs", "fn draw_activity_bar", "\"Direct2D activity bar rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn status_bar_layout", "\"DirectWrite status bar layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn tab_bar_layout_icons", "\"DirectWrite tab bar layout (with per-tab icons) — no surface attached yet\""),
    ("src/win/backend.rs", "fn resolve_tab_bar_layout_icons", "\"DirectWrite tab bar layout (TabBarLayout, with per-tab icons) — no surface attached yet\""),
    ("src/win/backend.rs", "fn draw_terminal", "\"Direct2D terminal cell grid rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_terminal_divider", "\"Direct2D terminal split divider rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_text_display", "\"Direct2D text display rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_command_line", "\"Direct2D command line rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_text_input", "\"Direct2D text input rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_tooltip_with_chrome", "\"Direct2D tooltip rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_context_menu", "\"Direct2D context menu rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_dialog", "\"Direct2D dialog rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_multi_section_view", "\"Direct2D MSV rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn form_layout", "\"DirectWrite form layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_editor", "\"Direct2D editor rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_message_list", "\"Direct2D message list rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_rich_text_popup", "\"Direct2D rich text popup rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_find_replace", "\"Direct2D find/replace rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_completions", "\"Direct2D completions rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_scrollbar", "\"Direct2D scrollbar rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_drop_overlay", "\"Direct2D drop overlay rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_menu_bar", "\"Direct2D menu bar rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn menu_bar_layout", "\"DirectWrite menu bar layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_split", "\"Direct2D split rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_split_tree", "\"Direct2D split-tree rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_board", "\"Direct2D board rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_minimap", "\"Direct2D minimap rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_image", "\"Direct2D image rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_panel", "\"Direct2D panel rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn panel_layout", "\"DirectWrite panel layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_toast_stack", "\"Direct2D toast stack rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn toast_stack_layout", "\"DirectWrite toast stack layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_pipeline_view", "\"Direct2D pipeline view rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_progress", "\"Direct2D progress bar rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_spinner", "\"Direct2D spinner rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn spinner_layout", "\"DirectWrite spinner layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_command_center", "\"Direct2D command center rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_toolbar_interactive", "\"Direct2D toolbar rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn toolbar_layout", "\"DirectWrite toolbar layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_sidebar_panel_interactive", "\"Direct2D sidebar-panel rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn sidebar_panel_layout", "\"DirectWrite sidebar-panel layout (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_diff_view", "\"Direct2D DiffView rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn draw_chart", "\"Direct2D chart rasteriser (no surface attached yet)\""),
    ("src/win/backend.rs", "fn surface_fill_rect", "\"Direct2D fill_rect (no surface attached yet)\""),
    ("src/win/backend.rs", "fn surface_stroke_rect", "\"Direct2D stroke_rect (no surface attached yet)\""),
    ("src/win/backend.rs", "fn surface_draw_text_run_styled", "\"DirectWrite draw_text_styled (no surface attached yet)\""),
    ("src/win/backend.rs", "fn surface_draw_line", "\"Direct2D draw_line (no surface attached yet)\""),
    ("src/win/backend.rs", "fn surface_push_clip", "\"Direct2D push_clip (no surface attached yet)\""),
    ("src/win/backend.rs", "fn surface_pop_clip", "\"Direct2D pop_clip (no surface attached yet)\""),
];

/// Every attribute-bearing [`Expr`] variant, mapped to its `attrs` field.
/// Mirrors syn's own (private) `Expr::replace_attrs` match arm list — syn
/// does not expose a public generic accessor for this. `Expr::Verbatim`
/// (and any future variant syn adds) falls through to `&[]`.
fn expr_attrs(e: &Expr) -> &[Attribute] {
    match e {
        Expr::Array(ExprArray { attrs, .. })
        | Expr::Assign(ExprAssign { attrs, .. })
        | Expr::Async(ExprAsync { attrs, .. })
        | Expr::Await(ExprAwait { attrs, .. })
        | Expr::Binary(ExprBinary { attrs, .. })
        | Expr::Block(ExprBlock { attrs, .. })
        | Expr::Break(ExprBreak { attrs, .. })
        | Expr::Call(ExprCall { attrs, .. })
        | Expr::Cast(ExprCast { attrs, .. })
        | Expr::Closure(ExprClosure { attrs, .. })
        | Expr::Const(ExprConst { attrs, .. })
        | Expr::Continue(ExprContinue { attrs, .. })
        | Expr::Field(ExprField { attrs, .. })
        | Expr::ForLoop(ExprForLoop { attrs, .. })
        | Expr::Group(ExprGroup { attrs, .. })
        | Expr::If(ExprIf { attrs, .. })
        | Expr::Index(ExprIndex { attrs, .. })
        | Expr::Infer(ExprInfer { attrs, .. })
        | Expr::Let(ExprLet { attrs, .. })
        | Expr::Lit(ExprLit { attrs, .. })
        | Expr::Loop(ExprLoop { attrs, .. })
        | Expr::Macro(ExprMacro { attrs, .. })
        | Expr::Match(ExprMatch { attrs, .. })
        | Expr::MethodCall(ExprMethodCall { attrs, .. })
        | Expr::Paren(ExprParen { attrs, .. })
        | Expr::Path(ExprPath { attrs, .. })
        | Expr::Range(ExprRange { attrs, .. })
        | Expr::Reference(ExprReference { attrs, .. })
        | Expr::Repeat(ExprRepeat { attrs, .. })
        | Expr::Return(ExprReturn { attrs, .. })
        | Expr::Struct(ExprStruct { attrs, .. })
        | Expr::Try(ExprTry { attrs, .. })
        | Expr::TryBlock(ExprTryBlock { attrs, .. })
        | Expr::Tuple(ExprTuple { attrs, .. })
        | Expr::Unary(ExprUnary { attrs, .. })
        | Expr::Unsafe(ExprUnsafe { attrs, .. })
        | Expr::While(ExprWhile { attrs, .. })
        | Expr::Yield(ExprYield { attrs, .. }) => attrs,
        _ => &[],
    }
}

/// Whether a `todo!()`/`unimplemented!()` call site is reachable when its
/// file is compiled for its own backend's native target, excluded by a
/// recognised `#[cfg(...)]` somewhere between the file root and this call,
/// or gated by something this checker refuses to classify.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Gate {
    /// No exempting `#[cfg(...)]` applies anywhere on the path from the
    /// file root to this node: reachable in a native build.
    Reachable,
    /// Excluded from a native build by a recognised `#[cfg(...)]`.
    Exempt(String),
    /// A `#[cfg(...)]`/`#[cfg_attr(...)]` predicate this checker does not
    /// understand sits on the path from the file root to this node.
    /// Deliberately sticky (see [`combine`]) and deliberately a hard
    /// failure rather than a silent pass in either direction.
    Unclassifiable(String),
}

fn lit_str(expr: &Expr) -> Result<String, String> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Ok(s.value()),
        other => Err(format!(
            "expected a string literal, found `{}`",
            other.to_token_stream()
        )),
    }
}

/// Classify one parsed `cfg(...)` predicate against `native_os` — see the
/// module doc's "Recognised `cfg` spellings" section for the full list.
/// `Ok(Some(reason))` = exempt, `Ok(None)` = recognised but not exempting,
/// `Err(description)` = refused/unclassifiable.
fn classify_cfg(meta: &Meta, native_os: Option<&str>) -> Result<Option<String>, String> {
    match meta {
        Meta::Path(p) if p.is_ident("test") => Ok(Some("cfg(test)".to_string())),
        Meta::NameValue(nv) if nv.path.is_ident("target_os") => {
            let os = lit_str(&nv.value)?;
            match native_os {
                Some(native) if os == native => Ok(None),
                Some(_) => Ok(Some(format!(
                    "cfg(target_os = \"{os}\") (excludes this backend's native target)"
                ))),
                None => Err(format!(
                    "cfg(target_os = \"{os}\") in a backend with no defined native target_os"
                )),
            }
        }
        Meta::List(list) if list.path.is_ident("not") => {
            let inner: Meta = syn::parse2(list.tokens.clone())
                .map_err(|e| format!("cfg(not(...)) with an unparsable inner predicate: {e}"))?;
            match &inner {
                Meta::NameValue(nv) if nv.path.is_ident("target_os") => {
                    let os = lit_str(&nv.value)?;
                    match native_os {
                        Some(native) if os == native => {
                            Ok(Some(format!("cfg(not(target_os = \"{os}\"))")))
                        }
                        Some(_) => Ok(None),
                        None => Err(format!(
                            "cfg(not(target_os = \"{os}\")) in a backend with no defined \
                             native target_os"
                        )),
                    }
                }
                Meta::Path(p) if p.is_ident("test") => Ok(None),
                other => Err(format!(
                    "cfg(not({})) — unrecognised inner predicate",
                    other.to_token_stream()
                )),
            }
        }
        other => Err(format!(
            "cfg({}) — unrecognised predicate",
            other.to_token_stream()
        )),
    }
}

/// Fold every `#[cfg(...)]`/`#[cfg_attr(...)]` on one syntax node (`attrs`)
/// into a new [`Gate`], starting from the inherited `parent` gate.
/// `cfg_attr(...)` is always refused rather than interpreted (its condition
/// controls whether a *different* attribute gets attached, which this
/// checker has no principled way to evaluate without a full `cfg` engine).
fn combine(parent: &Gate, attrs: &[Attribute], native_os: Option<&str>) -> Gate {
    if let Gate::Exempt(_) | Gate::Unclassifiable(_) = parent {
        // Sticky: once the enclosing subtree is known excluded from a
        // native build (or this checker already lost the ability to tell),
        // no nested attribute changes that determination.
        return parent.clone();
    }
    let mut gate = Gate::Reachable;
    for attr in attrs {
        if attr.path().is_ident("cfg") {
            let meta: Meta = match attr.parse_args() {
                Ok(m) => m,
                Err(e) => {
                    gate = Gate::Unclassifiable(format!("unparsable cfg(...) attribute: {e}"));
                    break;
                }
            };
            match classify_cfg(&meta, native_os) {
                Ok(Some(reason)) => gate = Gate::Exempt(reason),
                Ok(None) => {}
                Err(reason) => {
                    gate = Gate::Unclassifiable(reason);
                    break;
                }
            }
        } else if attr.path().is_ident("cfg_attr") {
            gate = Gate::Unclassifiable(format!(
                "cfg_attr(...) attribute: {}",
                attr.to_token_stream()
            ));
            break;
        }
    }
    gate
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Reachable on this backend's own native target: a latent host abort.
    Dangerous,
    /// A `cfg`/`cfg_attr` spelling this checker refuses to evaluate.
    Unclassifiable,
}

#[derive(Debug, Clone)]
struct Finding {
    kind: Kind,
    file: PathBuf,
    line: usize,
    macro_name: String,
    /// The macro's argument tokens, rendered back to source text — used as
    /// the stable half of [`KNOWN_DANGEROUS_TODOS`]'s matching key (a line
    /// number drifts every time someone edits above it; this doesn't).
    message_key: String,
    /// Nearest enclosing `fn` name, for a readable failure message and as
    /// the other half of the baseline matching key.
    context: String,
    detail: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: {}!(...) in {} — {}",
            self.file.display(),
            self.line,
            self.macro_name,
            self.context,
            self.detail
        )
    }
}

struct Walker<'a> {
    file: &'a Path,
    native_os: Option<&'a str>,
    gate_stack: Vec<Gate>,
    fn_ctx_stack: Vec<String>,
    findings: Vec<Finding>,
}

impl<'a> Walker<'a> {
    fn push(&mut self, attrs: &[Attribute]) {
        let parent = self.gate_stack.last().expect("root gate always present");
        let gate = combine(parent, attrs, self.native_os);
        self.gate_stack.push(gate);
    }

    fn pop(&mut self) {
        self.gate_stack.pop();
    }

    fn context(&self) -> String {
        self.fn_ctx_stack
            .last()
            .cloned()
            .unwrap_or_else(|| "<file scope>".to_string())
    }

    /// Shared by every syntax shape a `todo!()`/`unimplemented!()` call can
    /// take (tail-expression via `visit_expr`, or a semicolon-terminated /
    /// brace-delimited statement via `visit_stmt_macro`): if `mac` names one
    /// of the two macros, classify it against the gate on top of
    /// `gate_stack` and record a [`Finding`] unless it's exempt. Must be
    /// called only after the node's own attrs have already been pushed onto
    /// `gate_stack` by the caller.
    fn check_macro_call(&mut self, mac: &Macro) {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if name != "todo" && name != "unimplemented" {
            return;
        }
        let gate = self.gate_stack.last().unwrap().clone();
        let line = mac.span().start().line;
        let message_key = mac.tokens.to_string();
        let context = self.context();
        match gate {
            Gate::Reachable => self.findings.push(Finding {
                kind: Kind::Dangerous,
                file: self.file.to_path_buf(),
                line,
                macro_name: name,
                message_key,
                context,
                detail: "no cfg excludes this call from a native build — reachable".to_string(),
            }),
            Gate::Exempt(_) => {}
            Gate::Unclassifiable(reason) => self.findings.push(Finding {
                kind: Kind::Unclassifiable,
                file: self.file.to_path_buf(),
                line,
                macro_name: name,
                message_key,
                context,
                detail: reason,
            }),
        }
    }
}

impl<'a, 'ast> Visit<'ast> for Walker<'a> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.push(&node.attrs);
        self.fn_ctx_stack.push(format!("fn {}", node.sig.ident));
        visit::visit_item_fn(self, node);
        self.fn_ctx_stack.pop();
        self.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        self.push(&node.attrs);
        self.fn_ctx_stack.push(format!("fn {}", node.sig.ident));
        visit::visit_impl_item_fn(self, node);
        self.fn_ctx_stack.pop();
        self.pop();
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        self.push(&node.attrs);
        visit::visit_item_impl(self, node);
        self.pop();
    }

    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        self.push(&node.attrs);
        visit::visit_item_mod(self, node);
        self.pop();
    }

    fn visit_local(&mut self, node: &'ast Local) {
        self.push(&node.attrs);
        visit::visit_local(self, node);
        self.pop();
    }

    /// Handles a macro invocation used as an ordinary, semicolon-terminated
    /// (or brace-delimited) statement — `syn`'s `Stmt::Macro(StmtMacro)`,
    /// distinct from the tail-expression `Stmt::Expr(Expr::Macro(...), _)`
    /// shape `visit_expr` already covers. See the module doc's
    /// "Statement-position macro calls" section.
    fn visit_stmt_macro(&mut self, node: &'ast StmtMacro) {
        self.push(&node.attrs);
        self.check_macro_call(&node.mac);
        visit::visit_stmt_macro(self, node);
        self.pop();
    }

    fn visit_expr(&mut self, node: &'ast Expr) {
        self.push(expr_attrs(node));

        if let Expr::Macro(ExprMacro { mac, .. }) = node {
            self.check_macro_call(mac);
        }

        visit::visit_expr(self, node);
        self.pop();
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scan_file(path: &Path, native_os: Option<&str>) -> Vec<Finding> {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let file = syn::parse_file(&src)
        .unwrap_or_else(|e| panic!("{}: failed to parse as Rust: {e}", path.display()));
    let mut walker = Walker {
        file: path,
        native_os,
        gate_stack: vec![Gate::Reachable],
        fn_ctx_stack: Vec::new(),
        findings: Vec::new(),
    };
    walker.visit_file(&file);
    walker.findings
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.exists() {
        return;
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Scan every `.rs` file under each entry of [`BACKENDS`] and return
/// `(dangerous, unclassifiable)` findings, plus per-backend dangerous
/// counts for reporting.
fn scan_all_backends() -> (Vec<Finding>, Vec<Finding>, Vec<(&'static str, usize)>) {
    let root = repo_root();
    let mut dangerous = Vec::new();
    let mut unclassifiable = Vec::new();
    let mut per_backend_counts = Vec::new();

    for (dir, native_os) in BACKENDS {
        let mut files = Vec::new();
        collect_rs_files(&root.join(dir), &mut files);
        let mut count = 0;
        for file in &files {
            for finding in scan_file(file, *native_os) {
                match finding.kind {
                    Kind::Dangerous => {
                        count += 1;
                        dangerous.push(finding);
                    }
                    Kind::Unclassifiable => unclassifiable.push(finding),
                }
            }
        }
        per_backend_counts.push((*dir, count));
    }

    (dangerous, unclassifiable, per_backend_counts)
}

#[test]
fn no_unclassifiable_cfg_guards_a_todo_in_a_backend() {
    let (_dangerous, unclassifiable, _counts) = scan_all_backends();
    assert!(
        unclassifiable.is_empty(),
        "found todo!()/unimplemented!() call site(s) gated by a #[cfg(...)]/\
         #[cfg_attr(...)] this checker refuses to evaluate rather than \
         silently guessing which way it resolves:\n{}\n\n\
         Either teach `classify_cfg`/`combine` in \
         tests/backend_todo_gate.rs the new spelling, or rewrite the \
         backend source to use one of the recognised forms documented in \
         this test's module doc.",
        unclassifiable
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn no_new_ungated_todo_beyond_known_win_gui_debt() {
    let (dangerous, _unclassifiable, counts) = scan_all_backends();

    // Always visible in the test binary's stdout (captured on success,
    // shown on failure, or via `--nocapture`) — this is the number
    // docs/TESTING.md's "Backend `todo!()` gate" section commits to.
    for (dir, count) in &counts {
        println!("backend_todo_gate: {dir}: {count} dangerous (un-gated) todo!()/unimplemented!()");
    }

    let known: HashSet<(&str, &str, &str)> = KNOWN_DANGEROUS_TODOS.iter().copied().collect();

    let new_regressions: Vec<&Finding> = dangerous
        .iter()
        .filter(|f| {
            let file = f
                .file
                .strip_prefix(repo_root())
                .unwrap_or(&f.file)
                .to_string_lossy()
                .replace('\\', "/");
            !known.contains(&(file.as_str(), f.context.as_str(), f.message_key.as_str()))
        })
        .collect();

    assert!(
        new_regressions.is_empty(),
        "found {} un-gated todo!()/unimplemented!() call site(s) in a shipped \
         backend NOT in this test's KNOWN_DANGEROUS_TODOS baseline (quadraui#924's \
         59 pre-existing Win-GUI stubs):\n{}\n\n\
         Each of these is reachable on its own backend's native target the \
         instant a generic AppLogic calls the method through &mut dyn Backend \
         — see this file's module doc for the macOS incident (vimcode#896) \
         this guard exists to catch. If this is deliberate legacy debt \
         (another Win-GUI stub landing before its rasteriser), add it to \
         KNOWN_DANGEROUS_TODOS with a comment naming the tracking issue. If \
         it's new, gate it — `#[cfg(not(target_os = \"...\"))]` directly on \
         the macro call, or wrap the whole stub arm in a block carrying that \
         cfg — the way every other stub in this file already does.",
        new_regressions.len(),
        new_regressions
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[cfg(test)]
mod self_test {
    //! Tests for the checker itself, run against inline fixture source
    //! rather than the real backends — quadraui#923 asks for "the minimum
    //! fixture set": one call site the checker must call safe, one it must
    //! call dangerous, plus (since #923's own predicate correction was
    //! *caused* by trusting an under-tested heuristic) coverage of the
    //! enclosing-block and enclosing-item cfg shapes, `cfg(test)`, and the
    //! refused spellings.
    use super::*;

    fn findings_for(src: &str, native_os: Option<&str>) -> Vec<Finding> {
        let file = syn::parse_file(src).expect("fixture parses as Rust");
        let mut walker = Walker {
            file: Path::new("<fixture>"),
            native_os,
            gate_stack: vec![Gate::Reachable],
            fn_ctx_stack: Vec::new(),
            findings: Vec::new(),
        };
        walker.visit_file(&file);
        walker.findings
    }

    fn only_kind(src: &str, native_os: Option<&str>, kind: Kind) -> Vec<Finding> {
        findings_for(src, native_os)
            .into_iter()
            .filter(|f| f.kind == kind)
            .collect()
    }

    #[test]
    fn direct_cfg_on_the_macro_is_safe() {
        // quadraui#923's own minimum fixture: WinBackend::begin_frame's shape.
        let src = r#"
            impl Backend for WinBackend {
                fn begin_frame(&mut self) {
                    #[cfg(target_os = "windows")]
                    if let Some(surface) = &self.surface { return; }
                    #[cfg(not(target_os = "windows"))]
                    todo!("ID2D1RenderTarget::BeginDraw()")
                }
            }
        "#;
        assert!(
            findings_for(src, Some("windows")).is_empty(),
            "a todo!() directly gated by #[cfg(not(target_os = \"windows\"))] \
             must be exempt on a windows-native file"
        );
    }

    #[test]
    fn sibling_statement_cfg_does_not_gate_the_next_statement() {
        // quadraui#923's own minimum fixture: WinBackend::draw_minimap's
        // shape — the exact case the original proximity-based predicate
        // got wrong.
        let src = r#"
            impl Backend for WinBackend {
                fn draw_minimap(&mut self, rect: Rect, minimap: &Minimap) {
                    #[cfg(target_os = "windows")]
                    if let Some(surface) = &self.surface { return; }
                    #[cfg(not(target_os = "windows"))]
                    let _ = (rect, minimap);
                    todo!("Direct2D minimap rasteriser (no surface attached yet)")
                }
            }
        "#;
        let dangerous = only_kind(src, Some("windows"), Kind::Dangerous);
        assert_eq!(
            dangerous.len(),
            1,
            "a todo!() whose ONLY nearby cfg is on the PRECEDING sibling \
             statement (not on itself, not on an enclosing block/item) must \
             be reported dangerous — this is quadraui#923's central case"
        );
    }

    #[test]
    fn cfg_on_enclosing_block_gates_everything_inside_it() {
        // The `minimap_layout`/`list_layout`/… shape: the WHOLE `{ }` arm
        // carries the cfg, so a todo!() with no attrs of its own, nested
        // inside it, is still excluded from a native build.
        let src = r#"
            impl Backend for WinBackend {
                fn minimap_layout(&self, rect: Rect, minimap: &Minimap) -> MinimapLayout {
                    #[cfg(target_os = "windows")]
                    { win_minimap_layout(minimap, rect) }
                    #[cfg(not(target_os = "windows"))]
                    {
                        let _ = (rect, minimap);
                        todo!("Direct2D minimap layout")
                    }
                }
            }
        "#;
        assert!(
            findings_for(src, Some("windows")).is_empty(),
            "a todo!() inside a block whose OWN #[cfg(not(target_os = \
             \"windows\"))] wraps the whole block must be exempt, even \
             though the macro call itself carries no attribute"
        );
    }

    #[test]
    fn cfg_on_enclosing_fn_item_gates_its_whole_body() {
        // win/run.rs's shape: the cfg is several lines above the call, on
        // the free-standing fn item itself.
        let src = r#"
            #[cfg(not(target_os = "windows"))]
            pub fn run<A: AppLogic + 'static>(_app: A) -> std::process::ExitCode {
                todo!(
                    "quadraui::win::run doesn't run on this platform"
                )
            }
        "#;
        assert!(
            findings_for(src, Some("windows")).is_empty(),
            "a todo!() inside a fn item gated by #[cfg(not(target_os = \
             \"windows\"))] must be exempt, even though the cfg is on the \
             fn signature rather than the macro call"
        );
    }

    #[test]
    fn cfg_test_mod_gates_everything_inside_it_regardless_of_target_os() {
        let src = r#"
            #[cfg(test)]
            mod tests {
                fn some_unit_test() {
                    unimplemented!("mock has no real backing state")
                }
            }
        "#;
        assert!(
            findings_for(src, Some("windows")).is_empty(),
            "unimplemented!() inside #[cfg(test)] mod tests must be exempt \
             on every native target — it never ships in a non-test binary"
        );
    }

    #[test]
    fn cfg_any_is_refused_not_silently_passed() {
        let src = r#"
            impl Backend for WinBackend {
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                fn weird(&self) { todo!("ambiguous") }
            }
        "#;
        let findings = findings_for(src, Some("windows"));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, Kind::Unclassifiable);
    }

    #[test]
    fn cfg_attr_is_refused_not_silently_passed() {
        let src = r#"
            impl Backend for WinBackend {
                fn weird(&self) {
                    #[cfg_attr(feature = "x", allow(dead_code))]
                    todo!("gated by an unrelated cfg_attr")
                }
            }
        "#;
        let findings = findings_for(src, Some("windows"));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, Kind::Unclassifiable);
    }

    #[test]
    fn target_os_with_no_native_os_defined_is_refused() {
        // gtk/tui have `native_os: None` in `BACKENDS` — a target_os cfg
        // showing up there has no defined answer, so it must fail loudly
        // rather than guess.
        let src = r#"
            impl Backend for GtkBackend {
                #[cfg(not(target_os = "windows"))]
                fn weird(&self) { todo!("surprising in a cross-platform backend") }
            }
        "#;
        let findings = findings_for(src, None);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, Kind::Unclassifiable);
    }

    #[test]
    fn bare_todo_with_no_cfg_anywhere_is_dangerous() {
        // The vimcode#896 shape: MacBackend::draw_minimap before dbb3023,
        // no cfg at all.
        let src = r#"
            impl Backend for MacBackend {
                fn draw_minimap(&mut self, rect: Rect, minimap: &Minimap) {
                    todo!("not implemented yet")
                }
            }
        "#;
        let dangerous = only_kind(src, Some("macos"), Kind::Dangerous);
        assert_eq!(dangerous.len(), 1);
    }

    #[test]
    fn semicolon_terminated_todo_statement_is_dangerous() {
        // syn parses a semicolon-terminated (or brace-delimited) macro
        // statement as `Stmt::Macro(StmtMacro)`, a completely different
        // variant from the tail-expression `Stmt::Expr(Expr::Macro(...), _)`
        // shape covered by `visit_expr`. An earlier version of this checker
        // had no `visit_stmt_macro` override, so this extremely idiomatic
        // shape — a `todo!()` followed by more statements in the same
        // block, e.g. `fn f(&mut self) { todo!("x"); more_code(); }` — was
        // invisible to it: not Dangerous, not Exempt, not Unclassifiable,
        // simply never visited as a macro call at all. This pins it as
        // Dangerous instead.
        let src = r#"
            impl Backend for WinBackend {
                fn draw_whatever(&mut self, rect: Rect) {
                    todo!("not done yet");
                    let _ = rect;
                }
            }
        "#;
        let dangerous = only_kind(src, Some("windows"), Kind::Dangerous);
        assert_eq!(
            dangerous.len(),
            1,
            "a todo!() written as an ordinary semicolon-terminated \
             statement (not a block's tail expression) must still be \
             detected as dangerous when nothing gates it"
        );
    }

    #[test]
    fn cfg_gated_semicolon_terminated_todo_statement_is_exempt() {
        // The same statement-position shape as above, but with a direct
        // `#[cfg(not(target_os = "windows"))]` on the macro statement
        // itself — must still resolve Exempt, not just Dangerous-by-default
        // now that visit_stmt_macro exists.
        let src = r#"
            impl Backend for WinBackend {
                fn draw_whatever(&mut self, rect: Rect) {
                    #[cfg(target_os = "windows")]
                    if let Some(surface) = &self.surface { return; }
                    #[cfg(not(target_os = "windows"))]
                    todo!("no surface attached yet");
                    let _ = rect;
                }
            }
        "#;
        assert!(
            findings_for(src, Some("windows")).is_empty(),
            "a semicolon-terminated todo!() directly gated by \
             #[cfg(not(target_os = \"windows\"))] must be exempt, the same \
             as the tail-expression shape"
        );
    }

    #[test]
    fn unclassifiable_cfg_on_statement_macro_is_refused() {
        // The Unclassifiable path must also work for the StmtMacro shape,
        // not just the Expr::Macro one.
        let src = r#"
            impl Backend for WinBackend {
                fn weird(&self) {
                    #[cfg(any(target_os = "windows", target_os = "macos"))]
                    todo!("ambiguous");
                }
            }
        "#;
        let findings = findings_for(src, Some("windows"));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, Kind::Unclassifiable);
    }
}
