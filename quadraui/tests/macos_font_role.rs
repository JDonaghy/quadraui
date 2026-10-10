//! macOS font-role conformance scenario (`crate::font_role`'s module doc,
//! `quadraui/src/font_role.rs`, has the full policy account).
//!
//! On macOS, `MacBackend`'s own `PaintSurface` implementation is the
//! default surface every non-editor-class primitive paints through —
//! DataTable, Form, Toast, Panel, Tooltip, Palette, Progress, and Spinner
//! among them. This file paints each of those eight primitives twice,
//! with the chrome/editor font sizes swapped between runs, and asserts
//! the painted text run tracks `chrome_font`'s size every time, proving
//! the primitive paints in [`quadraui::FontRole::Chrome`] regardless of
//! what the editor font is set to.
//!
//! It also covers the opposite direction for every
//! `EditorClassPrimitive` with a macOS call site: `TextDisplay`,
//! `Terminal` and `DiffView` each paint through `EditorSurface`, the
//! dedicated adapter that opts them back into `current_font` — so
//! `draw_text_display_paints_in_editor_font`,
//! `draw_terminal_paints_in_editor_font` and
//! `draw_diff_view_paints_in_editor_font` run the same paired-run probe
//! with the assertion flipped, proving each call site tracks the
//! *editor* font's size regardless of what chrome is set to. Each is
//! wired independently (three separate `EditorSurface` constructions in
//! `macos::backend`), so each gets its own scenario rather than relying
//! on one to stand in for the adapter as a whole.
//!
//! `Terminal`'s probe looks for a one-character needle, not [`LABEL`]:
//! `primitives::terminal::paint` emits one text run per grid cell, so
//! the widest run it can produce is a single glyph.
//!
//! ## Why painted-run *width*, and not an ink-pixel count
//!
//! A pixel probe ("count the non-black pixels") is the right oracle for
//! a bare `surface_draw_text_run` call — `macos::backend`'s own in-crate
//! `mac_backend_paint_surface_draw_text_run_with_role_uses_the_requested_fonts_size`
//! test uses exactly that for the explicit-role entry point, because the
//! only thing it paints *is* a glyph. It is the wrong oracle for a whole
//! primitive: a `Panel`, `ProgressBar` or `DataTable` fills a background,
//! strokes a border and paints a filled track, all of which are ink and
//! none of which depend on the font. Those font-independent pixels swamp
//! the few hundred belonging to the label, so the ratio between two font
//! sizes collapses towards 1.0 and says nothing about which font shaped
//! the glyphs.
//!
//! So this file reads the text run each primitive actually painted
//! instead, via [`quadraui::macos::testing::MacDriver::find_bounds`]:
//! those bounds are recorded at the `macos::text::draw_text` choke point
//! from the painting font's own metrics, so the run's width *is* a direct
//! readout of the font that shaped it — exact, independent of
//! backgrounds, and unaffected by clipping.
//!
//! Scoped to macOS only, matching `src/macos/backend.rs`'s own scope. A
//! primitive is intentionally absent from this file if it is
//! editor-class (paints in `current_font` on purpose —
//! `crate::font_role::EditorClassPrimitive::ALL`) or if it is already
//! covered by `ChromePrimitive`'s direct-call-site convention, proven by
//! `font_role`'s own `every_chrome_primitive_is_chrome` unit test rather
//! than a painted scenario.
//!
//! A standalone integration test (not wired into `tests/conformance.rs`'s
//! shared harness) so it carries its own `cfg` gate rather than a second
//! `mod` declaration elsewhere: `target_os = "macos"` as well as the
//! feature, since `quadraui::macos` itself only compiles under both.

#![cfg(all(feature = "macos", target_os = "macos"))]

use quadraui::macos::testing::MacDriver;
use quadraui::{
    AppLogic, Backend, Color, Column, ColumnAlign, ColumnWidth, DataRow, DataTable, Decoration,
    DiffEditability, DiffHunk, DiffMode, DiffPane, DiffRow, DiffRowKind, DiffView, FieldKind, Form,
    FormField, Palette, PaletteItem, PaletteMode, Panel, ProgressBar, Reaction, Rect, Spinner,
    StyledSpan, StyledText, Terminal, TerminalCell, TerminalCursorShape, TextDisplay,
    TextDisplayLine, Toast, ToastCorner, ToastOverlay, ToastSeverity, Tooltip, TooltipMeasure,
    TooltipPlacement, UiEvent, WidgetId,
};

const W: u32 = 640;
const H: u32 = 400;

/// The label text every primitive under test paints. Deliberately a
/// string no primitive's own chrome (title, placeholder, spinner frame,
/// `×` affordance) can contain, so `find_bounds` can only match the run
/// this file asked for.
const LABEL: &str = "ZZZ";

/// The two font sizes swapped between the paired runs. Far enough apart
/// that no rounding, hinting or fallback difference could account for
/// the gap.
const BIG_PT: f32 = 32.0;
const SMALL_PT: f32 = 5.0;

/// `font_desc` strings for [`Backend::set_ui_font`] matching the two
/// sizes above.
const BIG_UI_FONT: &str = "Menlo 32";
const SMALL_UI_FONT: &str = "Menlo 5";

/// Row pitch and column pitch pinned on the backend before every paint,
/// so the two runs of a pair lay out identically and the *only* thing
/// differing between them is which font shapes the glyphs. Without this
/// the editor font's own metrics would drive the layout (that is what
/// `MacBackend::set_current_font` derives them from), making the
/// big-chrome run and the big-editor run paint different geometry as
/// well as different glyphs. Sized generously enough that a 32pt run
/// still fits the row it is painted into.
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
        // Editor font first: installing it is what derives the backend's
        // row pitch, which `render` then overrides (see
        // `PINNED_LINE_HEIGHT`).
        backend.set_editor_font("Menlo", self.editor_pt);
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

/// Width (points) of the `needle`-bearing run `paint` painted, with
/// `ui_font_desc` as the chrome font and `editor_pt`-sized Menlo as the
/// editor font.
fn painted_run_width<F: Fn(&mut dyn Backend, Rect)>(
    name: &str,
    needle: &str,
    ui_font_desc: &'static str,
    editor_pt: f32,
    paint: F,
) -> f32 {
    let driver = MacDriver::new(
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
             {ui_font_desc:?} / editor font Menlo {editor_pt}pt, so this scenario \
             cannot say anything about the font role. Painted runs: {:?}",
            driver.painted_texts()
        ),
    }
}

/// Asserts `paint` paints in [`quadraui::FontRole::Chrome`]: the painted
/// run's width tracks `chrome_font`'s size, not `current_font`'s (see
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
         font should paint a far wider run than a {SMALL_PT}pt one if `chrome_font` really \
         drives this primitive's glyphs: big_chrome={big_chrome} ({BIG_PT}pt chrome / \
         {SMALL_PT}pt editor), big_editor={big_editor} ({SMALL_PT}pt chrome / {BIG_PT}pt \
         editor)"
    );
}

/// Asserts `paint` paints in [`quadraui::FontRole::Editor`]: the painted
/// run's width tracks `current_font`'s size, not `chrome_font`'s — the
/// mirror image of [`assert_paints_in_chrome_font`] above.
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
         font should paint a far wider run than a {SMALL_PT}pt one if `current_font` really \
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

#[test]
fn draw_data_table_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_data_table", |b, area| {
        let table = DataTable {
            id: id("data-table"),
            columns: vec![Column {
                title: "col".to_string(),
                width: ColumnWidth::Flex(1.0),
                align: ColumnAlign::Left,
            }],
            rows: vec![DataRow {
                cells: vec![StyledText::plain(LABEL)],
                decoration: Decoration::Normal,
            }],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
        };
        let _ = b.draw_data_table(area, &table, None);
    });
}

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
fn draw_tooltip_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_tooltip", |b, area| {
        let cw = b.char_width();
        let lh = b.line_height();
        let anchor = Rect::new(area.x, area.y, area.width, lh);
        let mut tooltip = Tooltip::new(id("tooltip"), LABEL);
        tooltip.placement = TooltipPlacement::Bottom;
        let measure = TooltipMeasure::new(cw * 9.0, lh * 3.0);
        let layout = tooltip.layout(anchor, area, measure, lh);
        b.draw_tooltip(&tooltip, &layout);
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

#[test]
fn draw_progress_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_progress", |b, area| {
        let bar = ProgressBar {
            id: id("progress"),
            label: LABEL.to_string(),
            value: Some(0.5),
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let _ = b.draw_progress(area, &bar);
    });
}

#[test]
fn draw_spinner_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_spinner", |b, area| {
        let spinner = Spinner {
            id: id("spinner"),
            label: LABEL.to_string(),
            frame_idx: 0,
            accent: None,
        };
        let _ = b.draw_spinner(area, &spinner);
    });
}
