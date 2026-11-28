//! The widget contract's deny-list: names that are gone and must stay
//! gone.
//!
//! 1. [`duplicate_symbols_stay_deleted`]: none of [`DELETED_SYMBOLS`]
//!    appears, as a word, in the non-test source of `quadraui/`,
//!    `quadraui-binding-core/`, `quadraui-py/src` or `quadraui-node/src`.
//!    Each one duplicated logic that now lives once on the contract
//!    (`FocusTree`, `Interactive::handle`, `PaintEnv`'s interaction state,
//!    the `CellSurface` verbs).
//! 2. [`backend_trait_has_no_widget_method`]: `trait Backend` declares none
//!    of [`DELETED_BACKEND_METHODS`], no `draw_*` other than
//!    `draw_focus_ring`, and nothing ending in `_layout`. A widget paints
//!    through `DrawsPrimitives::draw_dyn`, never a `Backend` method.
//! 3. [`backend_dirs_hold_no_widget_file`]: no file under
//!    `src/{tui,gtk,macos,win}/` is named after a widget: one of
//!    [`DELETED_BACKEND_FILES`], a `widget_kinds!` kind, or the snake-case
//!    name of a type with an `impl Primitive`. A widget lives in
//!    `src/primitives/`.
//!
//! "Non-test" means a `#[cfg(test)]` (or `#[cfg(all(test, …))]`) item at
//! any depth is skipped, and so is a file an out-of-line `#[cfg(test)] mod`
//! declares. Comments count: a doc that names a deleted symbol as current
//! code is as wrong as a use of it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use syn::spanned::Spanned;
use syn::visit::Visit;

#[path = "common/syn_source.rs"]
mod syn_source;
use syn_source::{collect_rs_files, is_cfg_test, parse_rust_file};

/// Symbols the contract replaced. Every one of them is absent today; a
/// symbol joins this list only once it is gone, so the test never
/// grandfathers a live use.
const DELETED_SYMBOLS: &[&str] = &[
    "FocusGroup",
    "FocusRing",
    "classify_tree_nav_key",
    "tree_row_rename_key_event",
    "DividerDrag",
    "TreeHoverTracker",
    "ListHoverTracker",
    "ratatui_buffer",
];

/// Every `Backend` method the widget contract deleted: a per-widget draw,
/// layout or metric that now belongs to the widget's own `Primitive` impl.
const DELETED_BACKEND_METHODS: &[&str] = &[
    "activity_bar_layout",
    "board_layout",
    "button_layout",
    "canvas_layout",
    "chart_layout",
    "command_center_layout",
    "command_line_layout",
    "context_menu_border_inset",
    "context_menu_viewport",
    "data_table_layout",
    "diff_view_layout",
    "draw_activity_bar",
    "draw_activity_bar_with_style",
    "draw_board",
    "draw_button",
    "draw_button_with_state",
    "draw_canvas",
    "draw_chart",
    "draw_combo_box",
    "draw_command_center",
    "draw_command_line",
    "draw_command_line_selection",
    "draw_completions",
    "draw_context_menu",
    "draw_context_menu_with_glyphs",
    "draw_context_menu_with_state",
    "draw_data_table",
    "draw_data_table_with_glyphs",
    "draw_data_table_with_state",
    "draw_date_picker",
    "draw_dialog",
    "draw_diff_view",
    "draw_drop_overlay",
    "draw_editor",
    "draw_find_replace",
    "draw_float",
    "draw_form",
    "draw_image",
    "draw_list",
    "draw_list_with_state",
    "draw_menu_bar",
    "draw_menu_bar_with_state",
    "draw_message_list",
    "draw_minimap",
    "draw_multi_section_view",
    "draw_number_input",
    "draw_palette",
    "draw_palette_with_state",
    "draw_panel",
    "draw_pipeline_view",
    "draw_progress",
    "draw_radio_group",
    "draw_radio_group_with_state",
    "draw_rich_text_popup",
    "draw_rich_text_popup_with_font_role",
    "draw_scrollbar",
    "draw_settings_chrome",
    "draw_sidebar_panel_interactive",
    "draw_solid_fill",
    "draw_spinner",
    "draw_split",
    "draw_split_tree",
    "draw_status_bar",
    "draw_status_bar_interactive",
    "draw_status_bar_interactive_scaled",
    "draw_switch",
    "draw_switch_with_state",
    "draw_tab_bar",
    "draw_tab_bar_icons",
    "draw_tab_bar_icons_layout",
    "draw_tab_bar_layout",
    "draw_tab_bar_layout_with_state",
    "draw_tab_bar_with_chrome",
    "draw_tab_bar_with_chrome_and_glyphs",
    "draw_tab_bar_with_chrome_layout",
    "draw_tab_bar_with_state",
    "draw_terminal",
    "draw_terminal_divider",
    "draw_text_display",
    "draw_text_input",
    "draw_toast_overlay",
    "draw_toolbar_interactive",
    "draw_toolbar_with_options",
    "draw_tooltip",
    "draw_tooltip_with_chrome",
    "draw_tree",
    "draw_tree_with_state",
    "editor_col_at_x",
    "editor_layout",
    "editor_right_reserve_w",
    "editor_v_scrollbar_width",
    "focus_manager",
    "form_layout",
    "list_char_width",
    "list_hscrollbar",
    "list_layout",
    "list_vscrollbar",
    "menu_bar_layout",
    "minimap_layout",
    "minimap_scale",
    "minimap_sizing",
    "msv_layout",
    "msv_metrics",
    "palette_layout",
    "panel_layout",
    "pipeline_view_layout",
    "progress_layout",
    "radio_group_layout",
    "resolve_tab_bar_layout",
    "resolve_tab_bar_layout_icons",
    "resolve_tab_bar_layout_with_chrome",
    "scrollbar_reserve",
    "set_editor_right_reserve_w",
    "set_editor_v_scrollbar_width",
    "set_minimap_scale",
    "set_minimap_sizing",
    "sidebar_panel_layout",
    "spinner_layout",
    "split_layout",
    "split_tree_layout",
    "status_bar_layout",
    "switch_layout",
    "tab_bar_layout",
    "tab_bar_layout_icons",
    "tab_bar_layout_with_chrome",
    "terminal_layout",
    "terminal_scrollbar_default_width",
    "text_display_layout",
    "text_input_layout",
    "toast_stack_layout",
    "toolbar_layout",
    "tree_layout",
    "tree_paints_own_scrollbar",
    "tree_scroll_row_height",
    "tree_vscrollbar",
];

/// Every per-backend file the widget contract deleted. Each widget's paint
/// moved into `src/primitives/` behind its `Primitive` impl, and the one
/// shared helper (`braille`) moved next to the `CellSurface` seam.
const DELETED_BACKEND_FILES: &[&str] = &[
    "activity_bar",
    "board",
    "braille",
    "button",
    "canvas",
    "chart",
    "combo_box",
    "command_center",
    "command_line",
    "completions",
    "context_menu",
    "data_table",
    "date_picker",
    "dialog",
    "diff_view",
    "drop_overlay",
    "editor",
    "find_replace",
    "float",
    "form",
    "image",
    "list",
    "menu_bar",
    "menu_overlay",
    "message_list",
    "minimap",
    "multi_section_view",
    "number_input",
    "palette",
    "panel",
    "pipeline_view",
    "progress",
    "radio_group",
    "rich_text_popup",
    "scroll_view",
    "scrollbar",
    "sidebar_panel",
    "solid_fill",
    "spinner",
    "split",
    "split_junction",
    "split_tree",
    "status_bar",
    "switch",
    "tab_bar",
    "terminal",
    "text_display",
    "text_input",
    "toast",
    "toolbar",
    "tooltip",
    "tree",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("quadraui sits inside the workspace")
        .to_path_buf()
}

/// Collects the line spans of every test-only item in a file, and the
/// names of the out-of-line test modules it declares.
#[derive(Default)]
struct TestItems {
    lines: BTreeSet<usize>,
    out_of_line_mods: Vec<String>,
}

impl TestItems {
    fn skip(&mut self, attrs: &[syn::Attribute], span: proc_macro2::Span) -> bool {
        if !attrs.iter().any(is_cfg_test) {
            return false;
        }
        self.lines.extend(span.start().line..=span.end().line);
        true
    }
}

impl<'ast> Visit<'ast> for TestItems {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        if self.skip(&m.attrs, m.span()) {
            if m.content.is_none() {
                self.out_of_line_mods.push(m.ident.to_string());
            }
            return;
        }
        syn::visit::visit_item_mod(self, m);
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if !self.skip(&f.attrs, f.span()) {
            syn::visit::visit_item_fn(self, f);
        }
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if !self.skip(&i.attrs, i.span()) {
            syn::visit::visit_item_impl(self, i);
        }
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if !self.skip(&f.attrs, f.span()) {
            syn::visit::visit_impl_item_fn(self, f);
        }
    }

    fn visit_item_struct(&mut self, s: &'ast syn::ItemStruct) {
        self.skip(&s.attrs, s.span());
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        self.skip(&u.attrs, u.span());
    }

    fn visit_item_const(&mut self, c: &'ast syn::ItemConst) {
        self.skip(&c.attrs, c.span());
    }

    fn visit_item_macro(&mut self, m: &'ast syn::ItemMacro) {
        self.skip(&m.attrs, m.span());
    }
}

/// The directory an out-of-line `mod name;` in `file` resolves under.
fn child_module_dir(file: &Path) -> PathBuf {
    let parent = file.parent().expect("a source file has a parent dir");
    match file.file_stem().and_then(|s| s.to_str()) {
        Some("mod" | "lib" | "main") => parent.to_path_buf(),
        Some(stem) => parent.join(stem),
        None => parent.to_path_buf(),
    }
}

/// Whether `word` occurs in `line` with no identifier character on
/// either side.
fn contains_word(line: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    line.match_indices(word).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// The `.rs` files of the non-test source this deny-list covers, each
/// with the lines its test-only items occupy.
fn non_test_sources() -> Vec<(PathBuf, String, BTreeSet<usize>)> {
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in [
        "quadraui/src",
        "quadraui/examples",
        "quadraui/benches",
        "quadraui-binding-core/src",
        "quadraui-py/src",
        "quadraui-node/src",
    ] {
        collect_rs_files(&root.join(dir), &mut files);
    }
    assert!(
        files.len() > 100,
        "expected the workspace's source files; found {}",
        files.len()
    );

    let mut parsed = Vec::new();
    let mut test_files: BTreeSet<PathBuf> = BTreeSet::new();
    for path in files {
        let src =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let file = syn::parse_file(&src)
            .unwrap_or_else(|e| panic!("{}: failed to parse as Rust: {e}", path.display()));
        let mut items = TestItems::default();
        items.visit_file(&file);
        let dir = child_module_dir(&path);
        for name in &items.out_of_line_mods {
            test_files.insert(dir.join(format!("{name}.rs")));
            test_files.insert(dir.join(name).join("mod.rs"));
        }
        parsed.push((path, src, items.lines));
    }
    parsed.retain(|(path, _, _)| !test_files.contains(path));
    parsed
}

#[test]
fn duplicate_symbols_stay_deleted() {
    let root = workspace_root();
    let mut hits = Vec::new();
    for (path, src, test_lines) in non_test_sources() {
        for (index, line) in src.lines().enumerate() {
            if test_lines.contains(&(index + 1)) {
                continue;
            }
            for symbol in DELETED_SYMBOLS {
                if contains_word(line, symbol) {
                    let rel = path.strip_prefix(&root).unwrap_or(&path);
                    hits.push(format!("{}:{}: {symbol}", rel.display(), index + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "a deleted duplicate is back; use the contract's single copy instead \
         (FocusTree for focus, Interactive::handle for keys and hover, the \
         CellSurface verbs for cell paint):\n{}",
        hits.join("\n")
    );
}

/// The method names `trait Backend` declares in `src/backend.rs`.
fn backend_trait_methods() -> BTreeSet<String> {
    let file = parse_rust_file(&workspace_root().join("quadraui/src/backend.rs"));
    let backend = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Trait(t) if t.ident == "Backend" => Some(t),
            _ => None,
        })
        .expect("src/backend.rs declares `trait Backend`");
    backend
        .items
        .iter()
        .filter_map(|item| match item {
            syn::TraitItem::Fn(f) => Some(f.sig.ident.to_string()),
            _ => None,
        })
        .collect()
}

/// Whether a `Backend` method name is shaped like a widget method.
fn is_widget_method_name(name: &str) -> bool {
    (name.starts_with("draw_") && name != "draw_focus_ring") || name.ends_with("_layout")
}

#[test]
fn backend_trait_has_no_widget_method() {
    let methods = backend_trait_methods();
    assert!(
        methods.contains("draw_focus_ring") && methods.len() > 50,
        "expected to read trait Backend's methods; found {methods:?}"
    );
    let back: Vec<&String> = methods
        .iter()
        .filter(|m| DELETED_BACKEND_METHODS.contains(&m.as_str()) || is_widget_method_name(m))
        .collect();
    assert!(
        back.is_empty(),
        "trait Backend holds platform services only; a widget paints through its own \
         `Primitive` impl via `DrawsPrimitives::draw_dyn`. Widget-shaped methods: {back:?}"
    );
}

#[test]
fn the_widget_method_rule_keeps_the_focus_ring() {
    assert!(!is_widget_method_name("draw_focus_ring"));
    assert!(is_widget_method_name("draw_list"));
    assert!(is_widget_method_name("tree_layout"));
    assert!(!is_widget_method_name("measure_text"));
}

/// `ListView` → `list_view`.
fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Collects the self types of `impl Primitive for T` and the kind strings
/// of the `widget_kinds!` table, as file stems.
#[derive(Default)]
struct WidgetNames(BTreeSet<String>);

impl<'ast> Visit<'ast> for WidgetNames {
    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let is_primitive = i
            .trait_
            .as_ref()
            .and_then(|(_, path, _)| path.segments.last())
            .is_some_and(|seg| seg.ident == "Primitive");
        if let (true, syn::Type::Path(p)) = (is_primitive, &*i.self_ty) {
            if let Some(seg) = p.path.segments.last() {
                self.0.insert(snake_case(&seg.ident.to_string()));
            }
        }
        syn::visit::visit_item_impl(self, i);
    }

    fn visit_item_macro(&mut self, m: &'ast syn::ItemMacro) {
        if m.mac.path.is_ident("widget_kinds") {
            for token in m.mac.tokens.clone() {
                if let proc_macro2::TokenTree::Literal(lit) = token {
                    if let Ok(syn::Lit::Str(s)) = syn::parse_str::<syn::Lit>(&lit.to_string()) {
                        self.0.insert(s.value());
                    }
                }
            }
        }
    }
}

#[test]
fn backend_dirs_hold_no_widget_file() {
    let src = workspace_root().join("quadraui/src");
    let mut widget_files = WidgetNames::default();
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    for path in &files {
        widget_files.visit_file(&parse_rust_file(path));
    }
    assert!(
        widget_files.0.contains("list_view") && widget_files.0.contains("cell_grid"),
        "expected the widget names from Primitive impls and widget_kinds!; found {:?}",
        widget_files.0
    );
    widget_files
        .0
        .extend(DELETED_BACKEND_FILES.iter().map(|s| s.to_string()));

    let mut found = Vec::new();
    for dir in ["tui", "gtk", "macos", "win"] {
        let mut backend_files = Vec::new();
        collect_rs_files(&src.join(dir), &mut backend_files);
        for path in backend_files {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if widget_files.0.contains(stem) {
                found.push(format!("src/{dir}/{stem}.rs"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "a widget file is back under a backend directory; a widget lives in \
         src/primitives/ and paints through its Primitive impl: {found:?}"
    );
}

#[test]
fn contains_word_needs_identifier_boundaries() {
    assert!(contains_word("use crate::FocusRing;", "FocusRing"));
    assert!(contains_word("FocusRing", "FocusRing"));
    assert!(!contains_word("draw_focus_ring", "FocusRing"));
    assert!(!contains_word("FocusRingStyle", "FocusRing"));
    assert!(!contains_word("MyFocusRing", "FocusRing"));
}

#[test]
fn test_only_items_are_skipped() {
    let file: syn::File = syn::parse_quote! {
        fn live() {}
        #[cfg(test)]
        mod tests {
            fn uses_a_deleted_symbol() {}
        }
        #[cfg(all(test, feature = "gtk"))]
        mod gtk_tests;
    };
    let mut items = TestItems::default();
    items.visit_file(&file);
    assert_eq!(items.out_of_line_mods, vec!["gtk_tests".to_string()]);
    assert!(!items.lines.is_empty());
}
