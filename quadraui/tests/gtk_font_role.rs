//! GTK font-role conformance scenario (`crate::font_role`'s module doc,
//! `quadraui/src/font_role.rs`, has the full policy account) — the GTK
//! twin of `tests/macos_font_role.rs`.
//!
//! On GTK, `GtkBackend`'s own `PaintSurface` implementation is the
//! default surface every non-editor-class primitive paints through —
//! Form, Palette, TextInput, Completions, Panel and Toast among them.
//! This file paints each of those six primitives twice, with the
//! chrome/editor font sizes swapped between runs, and asserts the
//! painted text run tracks the chrome font's size every time, proving
//! the primitive paints in [`quadraui::FontRole::Chrome`] regardless of
//! what the editor font is set to. `DataTable` is chrome too but is
//! deliberately absent from this file — see the comment next to where
//! its test would have gone, just above `draw_form_paints_in_chrome_font`
//! below.
//!
//! It also covers the opposite direction for every
//! `EditorClassPrimitive` with a GTK call site that paints through
//! `GtkBackend`'s own `PaintSurface` impl: `TextDisplay`, `Terminal` and
//! `DiffView` each paint through `EditorSurface` instead (the dedicated
//! adapter that opts them back into the editor font), so
//! `draw_text_display_paints_in_editor_font`,
//! `draw_terminal_paints_in_editor_font` and
//! `draw_diff_view_paints_in_editor_font` run the same paired-run probe
//! with the assertion flipped, proving each call site tracks the
//! *editor* font's size regardless of what chrome is set to.
//!
//! `Terminal`'s probe looks for a one-character needle, not [`LABEL`]:
//! `primitives::terminal::paint` emits one text run per grid cell, so
//! the widest run it can produce is a single glyph.
//!
//! Reads painted runs through
//! [`quadraui::gtk::testing::GtkDriver::find_bounds`], backed by
//! `GtkBackend::painted_text` — recorded at the
//! `gtk::painted_text::show_layout` choke point every GTK rasteriser
//! paints through, so a run's width is a direct readout of the font
//! that shaped it, immune to the background fill / border stroke /
//! filled track every one of these primitives also paints (see
//! `tests/macos_font_role.rs`'s module doc, "Why painted-run width, and
//! not an ink-pixel count", for the fuller account of why a pixel probe
//! is the wrong oracle for a whole primitive).
//!
//! Scoped to the `gtk` feature only, matching `src/gtk/backend.rs`'s own
//! scope. A primitive is intentionally absent from this file if it is
//! editor-class (paints in the editor font on purpose —
//! `crate::font_role::EditorClassPrimitive::ALL`) or if it paints no
//! text of its own (`draw_split`, `draw_split_tree`, `draw_scrollbar`,
//! `draw_drop_overlay`).

#![cfg(feature = "gtk")]

use quadraui::gtk::testing::GtkDriver;
use quadraui::{
    AppLogic, Backend, Color, CompletionItem, CompletionItemMeasure, CompletionKind, Completions,
    Decoration, DiffEditability, DiffHunk, DiffMode, DiffPane, DiffRow, DiffRowKind, DiffView,
    FieldKind, Form, FormField, Palette, PaletteItem, PaletteMode, Panel, Reaction, Rect,
    StyledSpan, StyledText, Terminal, TerminalCell, TerminalCursorShape, TextDisplay,
    TextDisplayLine, TextInput, Toast, ToastCorner, ToastOverlay, ToastSeverity, UiEvent, WidgetId,
};

const W: i32 = 640;
const H: i32 = 400;

/// The label text every primitive under test paints. Deliberately a
/// string no primitive's own chrome (title, placeholder, `×` affordance)
/// can contain, so `find_bounds` can only match the run this file asked
/// for.
const LABEL: &str = "ZZZ";

/// The two font sizes swapped between the paired runs. Far enough apart
/// that no rounding, hinting or fallback difference could account for
/// the gap.
const BIG_PT: f32 = 32.0;
const SMALL_PT: f32 = 5.0;

/// `font_desc` strings for [`Backend::set_ui_font`] matching the two
/// sizes above.
const BIG_UI_FONT: &str = "Sans 32";
const SMALL_UI_FONT: &str = "Sans 5";

/// Row pitch and column pitch pinned on the backend before every paint,
/// so the two runs of a pair lay out identically and the *only* thing
/// differing between them is which font shapes the glyphs. Without this
/// the editor font's own metrics would drive the layout (`gtk/run.rs`'s
/// `render_frame` derives `current_line_height`/`current_char_width`
/// from the editor font every frame), making the big-chrome run and the
/// big-editor run paint different geometry as well as different glyphs.
/// Sized generously enough that a 32pt run still fits the row it is
/// painted into.
const PINNED_LINE_HEIGHT: f32 = 40.0;
const PINNED_CHAR_WIDTH: f32 = 20.0;

fn id(suffix: &str) -> WidgetId {
    WidgetId::new(format!("font-role-{suffix}"))
}

/// Minimal [`AppLogic`] that installs a chrome/editor font pair and
/// paints one primitive through `paint`.
struct PaintApp<F> {
    ui_font_desc: &'static str,
    editor_pt: f32,
    paint: F,
}

impl<F: Fn(&mut dyn Backend, Rect)> AppLogic for PaintApp<F> {
    type AreaId = ();

    fn setup(&mut self, backend: &mut dyn Backend) {
        backend.set_editor_font("Monospace", self.editor_pt);
        backend.set_ui_font(self.ui_font_desc);
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.set_current_line_height(PINNED_LINE_HEIGHT);
        backend.set_current_char_width(PINNED_CHAR_WIDTH);
        (self.paint)(backend, Rect::new(4.0, 4.0, (W - 8) as f32, (H - 8) as f32));
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// Width (pixels) of the `needle`-bearing run `paint` painted, with
/// `ui_font_desc` as the chrome font and `editor_pt`-sized Monospace as
/// the editor font.
fn painted_run_width<F: Fn(&mut dyn Backend, Rect)>(
    name: &str,
    needle: &str,
    ui_font_desc: &'static str,
    editor_pt: f32,
    paint: F,
) -> f32 {
    let driver = GtkDriver::new(
        PaintApp {
            ui_font_desc,
            editor_pt,
            paint,
        },
        W,
        H,
    );
    match driver.find_bounds(needle) {
        Some(bounds) => bounds.width,
        None => panic!(
            "{name}: nothing containing {needle:?} was painted with chrome font \
             {ui_font_desc:?} / editor font Monospace {editor_pt}pt, so this scenario \
             cannot say anything about the font role. Painted runs: {:?}",
            driver.painted_texts()
        ),
    }
}

/// Asserts `paint` paints in [`quadraui::FontRole::Chrome`]: the painted
/// run's width tracks the chrome font's size, not the editor font's (see
/// this file's module doc).
fn assert_paints_in_chrome_font<F>(name: &str, paint: F)
where
    F: Fn(&mut dyn Backend, Rect) + Copy,
{
    let big_chrome = painted_run_width(name, LABEL, BIG_UI_FONT, SMALL_PT, paint);
    let big_editor = painted_run_width(name, LABEL, SMALL_UI_FONT, BIG_PT, paint);
    assert!(
        big_chrome > 0.0 && big_editor > 0.0,
        "{name}: both font configurations must paint a measurable run: \
         big_chrome={big_chrome}, big_editor={big_editor}"
    );
    assert!(
        big_chrome > big_editor * 3.0,
        "{name} must paint in FontRole::Chrome, not FontRole::Editor — a {BIG_PT}pt chrome \
         font should paint a far wider run than a {SMALL_PT}pt one if the chrome font really \
         drives this primitive's glyphs: big_chrome={big_chrome} ({BIG_PT}pt chrome / \
         {SMALL_PT}pt editor), big_editor={big_editor} ({SMALL_PT}pt chrome / {BIG_PT}pt \
         editor)"
    );
}

/// Asserts `paint` paints in [`quadraui::FontRole::Editor`]: the painted
/// run's width tracks the editor font's size, not the chrome font's —
/// the mirror image of [`assert_paints_in_chrome_font`] above.
fn assert_paints_in_editor_font<F>(name: &str, paint: F)
where
    F: Fn(&mut dyn Backend, Rect) + Copy,
{
    assert_run_paints_in_editor_font(name, LABEL, paint)
}

/// [`assert_paints_in_editor_font`] for a primitive whose widest painted
/// run can't carry the whole of [`LABEL`] — `Terminal`, which paints one
/// run per grid cell.
fn assert_run_paints_in_editor_font<F>(name: &str, needle: &str, paint: F)
where
    F: Fn(&mut dyn Backend, Rect) + Copy,
{
    let big_chrome = painted_run_width(name, needle, BIG_UI_FONT, SMALL_PT, paint);
    let big_editor = painted_run_width(name, needle, SMALL_UI_FONT, BIG_PT, paint);
    assert!(
        big_chrome > 0.0 && big_editor > 0.0,
        "{name}: both font configurations must paint a measurable run: \
         big_chrome={big_chrome}, big_editor={big_editor}"
    );
    assert!(
        big_editor > big_chrome * 3.0,
        "{name} must paint in FontRole::Editor, not FontRole::Chrome — a {BIG_PT}pt editor \
         font should paint a far wider run than a {SMALL_PT}pt one if the editor font really \
         drives this primitive's glyphs: big_editor={big_editor} ({BIG_PT}pt editor / \
         {SMALL_PT}pt chrome), big_chrome={big_chrome} ({SMALL_PT}pt editor / {BIG_PT}pt \
         chrome)"
    );
}

#[test]
fn draw_text_display_paints_in_editor_font() {
    assert_paints_in_editor_font("draw_text_display", |b, area| {
        let td = TextDisplay {
            id: id("text-display"),
            lines: vec![TextDisplayLine {
                spans: vec![StyledSpan::plain(LABEL)],
                decoration: Decoration::Normal,
                timestamp: None,
            }],
            scroll_offset: 0,
            auto_scroll: true,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        b.draw_text_display(area, &td);
    });
}

/// `Terminal` is editor-class: its cell glyphs must stay monospace at
/// the editor font's size even though the surface they paint through
/// defaults to chrome. One run per cell, so the needle is one glyph —
/// see this file's module doc.
#[test]
fn draw_terminal_paints_in_editor_font() {
    const CELL: &str = "Z";
    assert_run_paints_in_editor_font("draw_terminal", CELL, |b, area| {
        let term = Terminal {
            id: id("terminal"),
            cells: vec![vec![TerminalCell {
                text: CELL.to_string(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                selected: false,
                is_cursor: false,
                cursor_shape: TerminalCursorShape::Block,
                cursor_blinking: false,
                is_find_match: false,
                is_find_active: false,
            }]],
            scrollbar: None,
        };
        b.draw_terminal(area, &term);
    });
}

/// `DiffView` is editor-class for the same reason `Terminal` is: both
/// panes show file content on a column grid, so a chrome-font row would
/// stop lining up with the editor the diff was opened from.
#[test]
fn draw_diff_view_paints_in_editor_font() {
    assert_paints_in_editor_font("draw_diff_view", |b, area| {
        let view = DiffView {
            id: id("diff-view"),
            left: LABEL.to_string(),
            right: "other".to_string(),
            left_label: None,
            right_label: None,
            hunks: vec![DiffHunk {
                left_start: 1,
                right_start: 1,
                rows: vec![DiffRow {
                    left: Some(LABEL.to_string()),
                    right: Some("other".to_string()),
                    kind: DiffRowKind::Changed,
                }],
            }],
            mode: DiffMode::SideBySide,
            editability: DiffEditability::ReadOnly,
            scroll_offset: 0,
            focused_pane: DiffPane::Left,
            has_focus: false,
        };
        let _ = b.draw_diff_view(area, &view);
    });
}

// `draw_data_table` is deliberately absent from this file: the bounds
// `draw_data_table` records into `painted_text` (what `find_bounds`
// reads) are derived from `DataTableLayout`'s declarative column
// geometry, not re-measured Pango glyph extent (see
// `GtkBackend::draw_data_table`'s own doc comment on that recording),
// so a `find_bounds`-width probe can't tell which font shaped a cell's
// glyphs — both runs report the same column width regardless of font
// size. `gtk_backend_draw_data_table_uses_ui_font_not_editor_font` in
// `src/gtk/backend.rs`'s own test module covers this primitive instead,
// with a pixel-ink scan as the oracle — the same shape
// `gtk_backend_draw_tree_uses_ui_font_not_editor_font` already uses.

#[test]
fn draw_form_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_form", |b, area| {
        let form = Form {
            id: id("form"),
            fields: vec![FormField {
                id: id("form-field"),
                label: StyledText::plain(LABEL),
                kind: FieldKind::ReadOnly {
                    value: StyledText::plain("value"),
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        b.draw_form(area, &form);
    });
}

#[test]
fn draw_toast_overlay_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_toast_overlay", |b, area| {
        let stack = ToastOverlay {
            id: id("toast-stack"),
            corner: ToastCorner::BottomRight,
            toasts: vec![Toast {
                id: id("toast"),
                title: LABEL.to_string(),
                body: String::new(),
                severity: ToastSeverity::Info,
                actions: vec![],
                accent: None,
            }],
            focus: None,
        };
        let _ = b.draw_toast_overlay(area, &stack);
    });
}

#[test]
fn draw_panel_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_panel", |b, area| {
        let panel = Panel {
            id: id("panel"),
            title: Some(StyledText::plain(LABEL)),
            actions: vec![],
            accent: None,
            collapsed: false,
        };
        let _ = b.draw_panel(area, &panel);
    });
}

#[test]
fn draw_palette_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_palette", |b, area| {
        let palette = Palette {
            id: id("palette"),
            title: "Palette".to_string(),
            query: String::new(),
            query_cursor: 0,
            items: vec![PaletteItem {
                text: StyledText::plain(LABEL),
                detail: None,
                icon: None,
                match_positions: vec![],
                depth: 0,
                expandable: false,
                expanded: false,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            total_count: 0,
            has_focus: false,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        };
        b.draw_palette(area, &palette);
    });
}

/// `TextInput` is chrome (not in
/// `crate::font_role::EditorClassPrimitive::ALL`) — a search box or
/// find/replace field, not the code editor itself.
#[test]
fn draw_text_input_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_text_input", |b, area| {
        let ti = TextInput::new(id("text-input")).with_lines(vec![LABEL.to_string()]);
        let _ = b.draw_text_input(area, &ti);
    });
}

/// The completions popup is chrome, like every other floating chrome
/// overlay — autocomplete candidates are chrome UI, not the editor's own
/// content.
#[test]
fn draw_completions_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_completions", |b, area| {
        let completions = Completions {
            id: id("completions"),
            items: vec![CompletionItem {
                label: StyledText::plain(LABEL),
                detail: None,
                documentation: None,
                kind: CompletionKind::Method,
                icon: None,
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
        };
        let layout = completions.layout(
            area.x,
            area.y,
            PINNED_LINE_HEIGHT,
            area,
            area.width.min(200.0),
            area.height.min(200.0),
            |_| CompletionItemMeasure::new(PINNED_LINE_HEIGHT),
        );
        b.draw_completions(&completions, &layout);
    });
}
