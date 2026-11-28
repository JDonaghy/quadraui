//! Pins of `Backend`'s trait body: the trait holds platform services only,
//! and every widget reaches every backend through
//! [`quadraui::Primitive`] and
//! [`quadraui::backend::DrawsPrimitives::draw_dyn`] rather than a
//! `draw_*`/`*_layout` pair on the sealed `Backend` trait.
//!
//! Every test parses real source with [`syn`] rather than grepping line
//! counts (mirrors `tests/backend_todo_gate.rs`'s approach):
//!
//! 1. [`backend_trait_holds_exactly_the_platform_methods`] asserts the
//!    trait-body method set equals [`BACKEND_PLATFORM_METHODS`], so the
//!    count is pinned at exactly [`BACKEND_METHOD_COUNT_BASELINE`].
//! 2. [`backend_trait_has_no_widget_methods`] fails on any trait method
//!    named `draw_*` (other than the runner's frame-level
//!    `draw_focus_ring`) or ending in `_layout`.
//! 3. [`primitives_dir_types_skip_bespoke_backend_methods`] finds every
//!    `impl Primitive for X` under `src/primitives/` and asserts `Backend`
//!    carries no bespoke `draw_<stem>`/`<stem>_layout` pair for any
//!    leading-word stem of `X`'s name (`ListView` -> `list_view` or
//!    `list`).
//! 4. [`the_backend_trait_declares_no_tab_bar_method`] pins `TabBar`'s
//!    completed port: the trait declares no `*tab_bar*` method at all.
//! 5. [`no_backend_directory_carries_a_tab_bar_file`] pins the file-level
//!    half of the same claim.

use std::collections::HashSet;
use std::path::PathBuf;

use syn::{Item, ItemImpl, ItemTrait, TraitItem, Type};

#[path = "common/syn_source.rs"]
mod syn_source;
use syn_source::{collect_rs_files, parse_rust_file};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Find a top-level `trait <name> { ... }` item in a parsed file.
fn find_trait<'a>(file: &'a syn::File, name: &str) -> &'a ItemTrait {
    file.items
        .iter()
        .find_map(|item| match item {
            Item::Trait(t) if t.ident == name => Some(t),
            _ => None,
        })
        .unwrap_or_else(|| panic!("trait `{name}` not found"))
}

/// Every method name declared directly in a trait's body (default or not).
fn trait_method_names(t: &ItemTrait) -> Vec<String> {
    t.items
        .iter()
        .filter_map(|item| match item {
            TraitItem::Fn(f) => Some(f.sig.ident.to_string()),
            _ => None,
        })
        .collect()
}

/// The platform services `Backend` holds: windowing, input, fonts and
/// metrics, focus, clipping, capture and the paint seams. No widget paints
/// through a method of its own.
const BACKEND_PLATFORM_METHODS: &[&str] = &[
    "viewport",
    "cell_grid",
    "begin_frame",
    "end_frame",
    "set_theme",
    "theme",
    "set_style",
    "style",
    "set_nerd_fonts",
    "nerd_fonts_enabled",
    "set_editor_font",
    "set_ui_font",
    "default_fonts",
    "register_font_from_memory",
    "set_nerd_font_fallback",
    "has_font_family",
    "register_text_region",
    "register_zone",
    "cancel_text_selection_drag",
    "poll_events",
    "wait_events",
    "waker",
    "request_frame_in",
    "request_full_repaint",
    "invalidate_rect",
    "invalidation_log",
    "register_accelerator",
    "unregister_accelerator",
    "install_menu_bar",
    "show_context_menu",
    "menu_style",
    "set_menu_style",
    "effective_menu_style",
    "begin_window_drag",
    "toggle_window_maximize",
    "begin_window_resize",
    "set_cursor",
    "titlebar_control_inset",
    "window",
    "tray",
    "modal_stack_handle",
    "drag_state_handle",
    "focus_tree",
    "set_focus",
    "draw_focus_ring",
    "frame_capture",
    "last_frame",
    "screenshot",
    "last_error",
    "services",
    "paint_surface",
    "cell_surface",
    "backend_caps",
    "line_height",
    "char_width",
    "wheel_delta_rows",
    "chrome_line_height",
    "set_current_line_height",
    "set_current_char_width",
    "measure",
    "measure_text",
    "scales_text_rows",
    "snap_height",
    "push_clip",
    "pop_clip",
    "set_caret_shape",
];

/// `Backend`'s exact trait-body method count. A widget that needs a paint
/// path implements `quadraui::Primitive` (`src/primitive.rs`) and reaches
/// every backend through `DrawsPrimitives::draw_dyn` — see
/// `tests/primitive_public_api.rs` for a worked third-party example — so
/// this number does not move for a widget.
const BACKEND_METHOD_COUNT_BASELINE: usize = 66;

#[test]
fn backend_trait_holds_exactly_the_platform_methods() {
    let file = parse_rust_file(&repo_root().join("src/backend.rs"));
    let methods = trait_method_names(find_trait(&file, "Backend"));

    assert_eq!(
        BACKEND_PLATFORM_METHODS.len(),
        BACKEND_METHOD_COUNT_BASELINE,
        "BACKEND_PLATFORM_METHODS and BACKEND_METHOD_COUNT_BASELINE disagree"
    );
    assert_eq!(
        methods.len(),
        BACKEND_METHOD_COUNT_BASELINE,
        "Backend's trait body has {} methods, not {BACKEND_METHOD_COUNT_BASELINE}. A widget \
         implements `quadraui::Primitive` and reaches every backend through \
         `DrawsPrimitives::draw_dyn`; it never adds a `Backend` method. See \
         `docs/PRIMITIVE_RULES.md`.\n\nMethods found: {:#?}",
        methods.len(),
        methods,
    );

    let found: HashSet<&str> = methods.iter().map(String::as_str).collect();
    let expected: HashSet<&str> = BACKEND_PLATFORM_METHODS.iter().copied().collect();
    let mut extra: Vec<&str> = found.difference(&expected).copied().collect();
    let mut missing: Vec<&str> = expected.difference(&found).copied().collect();
    extra.sort_unstable();
    missing.sort_unstable();
    assert!(
        extra.is_empty() && missing.is_empty(),
        "Backend's trait body differs from BACKEND_PLATFORM_METHODS: not in the list \
         {extra:?}, absent from the trait {missing:?}"
    );
}

/// Every trait method whose name is shaped like a widget paint or layout
/// entry point: `draw_*` (other than `draw_focus_ring`, the runner's
/// frame-level focus-visible stroke) or `*_layout`.
fn widget_shaped_method_names(methods: &[String]) -> Vec<String> {
    methods
        .iter()
        .filter(|m| {
            (m.starts_with("draw_") && m.as_str() != "draw_focus_ring") || m.ends_with("_layout")
        })
        .cloned()
        .collect()
}

#[test]
fn backend_trait_has_no_widget_methods() {
    let file = parse_rust_file(&repo_root().join("src/backend.rs"));
    let methods = trait_method_names(find_trait(&file, "Backend"));
    let widget_methods = widget_shaped_method_names(&methods);

    assert!(
        widget_methods.is_empty(),
        "`Backend` declares widget-shaped method(s) {widget_methods:#?} — a widget paints \
         through its `Primitive` impl and `DrawsPrimitives::draw_dyn`, and its geometry comes \
         from `Primitive::layout` / `DrawsPrimitives::layout_primitive`."
    );
}

/// Render a `syn::Type` naming a bare local type (e.g. `ListView`) back to
/// its identifier, or `None` for anything more exotic (a reference, a
/// generic, a path with multiple segments, …) — every `impl Primitive for
/// X` in this crate today names a bare local struct, so this deliberately
/// does not try to handle more.
fn bare_type_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) if p.qself.is_none() => {
            p.path.segments.last().map(|seg| seg.ident.to_string())
        }
        _ => None,
    }
}

/// Whether an `impl` block's trait path names `Primitive` — matches both
/// `Primitive` (direct `use`) and a qualified `crate::primitive::Primitive`
/// spelling, the same way `list.rs` writes it.
fn impl_trait_is_primitive(item: &ItemImpl) -> bool {
    item.trait_
        .as_ref()
        .and_then(|(_, path, _)| path.segments.last())
        .map(|seg| seg.ident == "Primitive")
        .unwrap_or(false)
}

/// Every type name with an `impl Primitive for <Type>` in one parsed file.
fn primitive_impl_type_names(file: &syn::File) -> Vec<String> {
    file.items
        .iter()
        .filter_map(|item| match item {
            Item::Impl(imp) if impl_trait_is_primitive(imp) => bare_type_ident(&imp.self_ty),
            _ => None,
        })
        .collect()
}

/// Lowercase ASCII `snake_case` of a `PascalCase`/`camelCase` identifier —
/// `ListView` -> `list_view`. `Backend`'s bespoke methods are not always
/// named after this *full* form (`draw_foo`/`foo_layout` for a `FooView`);
/// [`candidate_stems`] below
/// derives the shorter leading-word forms those methods actually use.
fn to_snake_case(ident: &str) -> String {
    let mut out = String::new();
    for (i, ch) in ident.chars().enumerate() {
        if ch.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Leading-word prefixes of a `snake_case` identifier, longest first —
/// `list_view` -> `["list_view", "list"]`, `data_table` -> `["data_table",
/// "data"]`, `swatch` -> `["swatch"]`. A bespoke `Backend` method is named
/// after one of these stems, not necessarily the type's full snake_case
/// name, so every candidate below is checked against each stem.
fn candidate_stems(snake: &str) -> Vec<String> {
    let words: Vec<&str> = snake.split('_').collect();
    (1..=words.len())
        .rev()
        .map(|n| words[..n].join("_"))
        .collect()
}

/// Whether `method` is a `Backend` method shaped like a bespoke
/// `draw_<stem>`, `<stem>_layout`, or `draw_<stem>_with_*` state variant
/// (e.g. `draw_foo_with_state`) for any leading-word stem of `type_name`.
fn bespoke_method_for_type(method: &str, type_name: &str) -> bool {
    let snake = to_snake_case(type_name);
    candidate_stems(&snake).iter().any(|stem| {
        *method == format!("draw_{stem}")
            || *method == format!("{stem}_layout")
            || method.starts_with(&format!("draw_{stem}_with"))
    })
}

/// For every `type_name` in `primitive_type_names`, find `Backend` methods
/// in `backend_methods` that match a bespoke `draw_*`/`*_layout` shape for
/// one of its stems. Returns
/// `(type_name, method)` for each violation — pulled out of the test body
/// so it can be driven with synthetic inputs in the `tests` module below.
fn find_bespoke_violations<'a>(
    primitive_type_names: &'a [String],
    backend_methods: &'a [String],
) -> Vec<(&'a str, &'a str)> {
    let mut violations = Vec::new();
    for type_name in primitive_type_names {
        for method in backend_methods {
            if !bespoke_method_for_type(method, type_name) {
                continue;
            }
            violations.push((type_name.as_str(), method.as_str()));
        }
    }
    violations
}

#[test]
fn primitives_dir_types_skip_bespoke_backend_methods() {
    let root = repo_root();

    let mut primitive_files = Vec::new();
    collect_rs_files(&root.join("src/primitives"), &mut primitive_files);
    assert!(
        !primitive_files.is_empty(),
        "expected at least one .rs file under src/primitives/"
    );

    let mut primitive_types: Vec<(String, PathBuf)> = Vec::new();
    for path in &primitive_files {
        let file = parse_rust_file(path);
        for name in primitive_impl_type_names(&file) {
            primitive_types.push((name, path.clone()));
        }
    }
    assert!(
        !primitive_types.is_empty(),
        "expected at least one `impl Primitive for ...` under src/primitives/ (ListView's \
         pilot impl, quadraui#2647) — none found; did it move or get renamed?"
    );

    let backend_file = parse_rust_file(&root.join("src/backend.rs"));
    let backend_trait = find_trait(&backend_file, "Backend");
    let backend_methods = trait_method_names(backend_trait);

    let type_names: Vec<String> = primitive_types
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    let violating = find_bespoke_violations(&type_names, &backend_methods);

    let violations: Vec<String> = violating
        .iter()
        .map(|(type_name, method)| {
            let path = primitive_types
                .iter()
                .find(|(name, _)| name == type_name)
                .map(|(_, p)| p.display().to_string())
                .unwrap_or_default();
            format!("`{type_name}` (impl Primitive in {path}) has a bespoke `Backend::{method}`")
        })
        .collect();

    assert!(
        violations.is_empty(),
        "found Primitive implementor(s) with a stray per-backend draw path alongside their \
         Primitive impl (quadraui#2608): a widget that implements `quadraui::Primitive` must \
         reach every backend through `DrawsPrimitives::draw_primitive` alone, not a fresh \
         `draw_*`/`*_layout` pair on the sealed `Backend` trait. Delete the bespoke methods \
         and rely on `draw_dyn` instead:\n{}",
        violations.join("\n"),
    );
}

/// Every `Backend` trait method whose name names a tab bar. Read off
/// the trait body rather than hardcoded, so a method re-added under any
/// `*tab_bar*` spelling is caught the day it lands.
fn tab_bar_method_names(backend_trait: &ItemTrait) -> HashSet<String> {
    trait_method_names(backend_trait)
        .into_iter()
        .filter(|name| name.contains("tab_bar"))
        .collect()
}

/// `Backend` must declare **no** tab-bar method at all: a tab strip is
/// painted by pushing the `TabBar` descriptor through
/// `DrawsPrimitives::draw_dyn` (or `Surface::Widget`), its geometry comes
/// back as the `TabBarPrimitiveLayout` that paint resolved
/// (`DrawsPrimitives::layout_primitive` when no paint happened), and
/// `TabBar::chrome` / `icons` / `glyphs` carry what the old sidecar
/// entry points took as explicit arguments.
#[test]
fn the_backend_trait_declares_no_tab_bar_method() {
    let root = repo_root();
    let backend_file = parse_rust_file(&root.join("src/backend.rs"));
    let names = tab_bar_method_names(find_trait(&backend_file, "Backend"));

    assert!(
        names.is_empty(),
        "`Backend` declares {} tab-bar method(s) again: {names:#?} — a tab strip reaches \
         every backend through `TabBar`'s `Primitive`/`Interactive` impls, not a \
         per-backend `draw_tab_bar*`/`tab_bar_layout*` pair on the sealed trait. Put the \
         new behaviour on the descriptor (`TabBar::chrome`/`icons`/`glyphs`) or in \
         `primitives::tab_bar`, and keep this ratchet at zero.",
        names.len(),
    );
}

/// No backend directory may carry a `tab_bar.rs` of its own either —
/// the file-level half of the same claim. `TabBar`'s cell and pixel
/// painters both live in `src/primitives/tab_bar.rs`, reached through
/// `Primitive::paint`/`paint_cells`.
#[test]
fn no_backend_directory_carries_a_tab_bar_file() {
    let root = repo_root();
    let strays: Vec<String> = ["tui", "gtk", "macos", "win"]
        .into_iter()
        .map(|dir| format!("src/{dir}/tab_bar.rs"))
        .filter(|rel| root.join(rel).exists())
        .collect();

    assert!(
        strays.is_empty(),
        "found per-backend tab-bar rasteriser file(s): {strays:?} — `TabBar` paints through \
         `src/primitives/tab_bar.rs`'s shared `cell_paint`/`native_surface_paint` on every \
         backend, so a backend directory needs no tab-bar file of its own."
    );
}

mod tests {
    use super::*;

    #[test]
    fn tab_bar_method_names_picks_the_tab_bar_methods_out_of_a_trait() {
        let backend_trait: ItemTrait = syn::parse_str(
            "trait Backend { fn draw_tab_bar(&mut self); fn resolve_tab_bar_layout(&self); \
             fn char_width(&self) -> f32; }",
        )
        .unwrap();

        let names = tab_bar_method_names(&backend_trait);

        assert_eq!(
            names,
            ["draw_tab_bar", "resolve_tab_bar_layout"]
                .into_iter()
                .map(str::to_string)
                .collect::<HashSet<String>>()
        );
    }

    #[test]
    fn widget_shaped_method_names_keeps_draw_focus_ring() {
        let methods: Vec<String> = [
            "draw_focus_ring",
            "draw_list",
            "progress_layout",
            "char_width",
            "begin_frame",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();

        assert_eq!(
            widget_shaped_method_names(&methods),
            vec!["draw_list".to_string(), "progress_layout".to_string()]
        );
    }

    #[test]
    fn to_snake_case_matches_existing_backend_method_convention() {
        assert_eq!(to_snake_case("ListView"), "list_view");
        assert_eq!(to_snake_case("TreeView"), "tree_view");
        assert_eq!(to_snake_case("DataTable"), "data_table");
        assert_eq!(to_snake_case("Swatch"), "swatch");
    }

    #[test]
    fn candidate_stems_orders_longest_first() {
        assert_eq!(candidate_stems("list_view"), vec!["list_view", "list"]);
        assert_eq!(candidate_stems("data_table"), vec!["data_table", "data"]);
        assert_eq!(candidate_stems("swatch"), vec!["swatch"]);
    }

    #[test]
    fn bespoke_method_for_type_matches_a_leading_word_stem() {
        // `FooView`'s bespoke `draw_foo`/`foo_layout` should be caught even
        // though the type's full snake_case form is `foo_view`: the
        // method is named after the `foo` stem, not the full name.
        assert!(bespoke_method_for_type("draw_foo", "FooView"));
        assert!(bespoke_method_for_type("foo_layout", "FooView"));
        assert!(bespoke_method_for_type("draw_foo_with_state", "FooView"));
        assert!(!bespoke_method_for_type("draw_bar", "FooView"));
        assert!(!bespoke_method_for_type("foo_view_char_width", "FooView"));
    }

    #[test]
    fn find_bespoke_violations_catches_a_bespoke_pair() {
        let types = vec!["FooView".to_string()];
        let methods = vec![
            "draw_foo".to_string(),
            "foo_layout".to_string(),
            "unrelated_method".to_string(),
        ];
        let violations = find_bespoke_violations(&types, &methods);

        assert_eq!(
            violations,
            vec![("FooView", "draw_foo"), ("FooView", "foo_layout")]
        );
    }

    #[test]
    fn find_bespoke_violations_catches_a_draw_layout_trio() {
        // The shape the ratchet must flag for any `Primitive` implementor
        // that also keeps a bespoke paint/layout trio on `Backend`.
        let types = vec!["FooView".to_string()];
        let methods = vec![
            "draw_foo".to_string(),
            "foo_layout".to_string(),
            "draw_foo_with_state".to_string(),
        ];
        let violations = find_bespoke_violations(&types, &methods);

        assert_eq!(violations.len(), 3);
    }

    #[test]
    fn bare_type_ident_reads_a_simple_struct_name() {
        let ty: Type = syn::parse_str("ListView").unwrap();
        assert_eq!(bare_type_ident(&ty), Some("ListView".to_string()));
    }

    #[test]
    fn impl_trait_is_primitive_matches_qualified_path() {
        let item: ItemImpl = syn::parse_str(
            "impl crate::primitive::Primitive for ListView { type Layout = (); type Event = (); \
             fn layout(&self, _c: &LayoutCtx, _r: Rect) {} fn paint(&self, _l: &(), _s: &mut dyn \
             PaintSurface, _env: &quadraui::PaintEnv) {} }",
        )
        .unwrap();
        assert!(impl_trait_is_primitive(&item));
    }

    #[test]
    fn impl_trait_is_primitive_rejects_a11y_primitive() {
        let item: ItemImpl = syn::parse_str(
            "impl A11yPrimitive for ListView { fn a11y_info(&self) -> A11yInfo { \
             A11yInfo::default() } }",
        )
        .unwrap();
        assert!(!impl_trait_is_primitive(&item));
    }
}
