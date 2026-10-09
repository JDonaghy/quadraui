//! macOS font-role conformance scenario (`crate::font_role`'s module doc,
//! `quadraui/src/font_role.rs`, has the full policy account).
//!
//! On macOS, `MacBackend`'s own `PaintSurface` implementation is the
//! default surface every non-editor-class primitive paints through —
//! DataTable, Form, Toast, Panel, Tooltip, Palette, Progress, and Spinner
//! among them. This file paints each of those eight primitives twice,
//! with the chrome/editor font sizes swapped between runs, and asserts
//! the painted ink tracks `chrome_font`'s size every time, proving the
//! primitive paints in [`quadraui::FontRole::Chrome`] regardless of what
//! the editor font is set to. Same "two visibly different font sizes"
//! technique `macos::backend`'s own in-crate
//! `mac_backend_paint_surface_draw_text_run_with_role_uses_the_requested_fonts_size`
//! test uses for the explicit-role entry point, applied here to the
//! *default* (no-role-argument) entry points instead.
//!
//! Scoped to macOS only, matching `src/macos/backend.rs`'s own scope. A
//! primitive is intentionally absent from this file if it is
//! editor-class (paints in `current_font` on purpose —
//! `crate::font_role::EditorClassPrimitive::ALL`) or if it is already
//! covered by `ChromePrimitive`'s direct-call-site convention, proven by
//! `font_role`'s own `every_chrome_primitive_is_chrome` unit test rather
//! than a pixel scenario.
//!
//! A standalone integration test (not wired into `tests/conformance.rs`'s
//! shared harness) so it carries its own `cfg` gate rather than a second
//! `mod` declaration elsewhere: `target_os = "macos"` as well as the
//! feature, since `quadraui::macos` itself only compiles under both.

#![cfg(all(feature = "macos", target_os = "macos"))]

use quadraui::macos::backend::MacBackend;
use quadraui::macos::headless::BitmapSurface;
use quadraui::macos::text::make_font;
use quadraui::{
    Backend, Column, ColumnAlign, ColumnWidth, DataRow, DataTable, Decoration, FieldKind, Form,
    FormField, Palette, PaletteItem, PaletteMode, Panel, ProgressBar, Rect, Spinner, StyledText,
    Toast, ToastCorner, ToastOverlay, ToastSeverity, Tooltip, TooltipMeasure, TooltipPlacement,
    Viewport, WidgetId,
};

const W: u32 = 160;
const H: u32 = 80;

fn id(suffix: &str) -> WidgetId {
    WidgetId::new(format!("font-role-{suffix}"))
}

/// Count of non-black pixels in `surface` — the same ink-pixel probe
/// `macos::backend`'s own role test uses.
fn ink_pixel_count(surface: &BitmapSurface) -> u32 {
    let mut count = 0;
    for y in 0..H {
        for x in 0..W {
            let (r, g, b, _a) = surface.pixel(x, y);
            if r as u32 + g as u32 + b as u32 > 0 {
                count += 1;
            }
        }
    }
    count
}

/// Paints `paint` twice — once with a huge `chrome_font` / tiny
/// `current_font`, once with the sizes swapped — and returns the
/// resulting `(ink_with_big_chrome, ink_with_big_editor)`.
fn ink_under_font_sizes(paint: impl Fn(&mut MacBackend, Rect)) -> (u32, u32) {
    let area = Rect::new(2.0, 2.0, (W - 4) as f32, (H - 4) as f32);

    let mut backend = MacBackend::new();
    backend.set_chrome_font(make_font("Menlo", 32.0).expect("Menlo installed"));
    backend.set_current_font(make_font("Menlo", 4.0).expect("Menlo installed"));
    let big_chrome_surface = BitmapSurface::new(W, H);
    backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
    backend.enter_frame_scope(big_chrome_surface.context_ptr(), |b| paint(b, area));
    backend.end_frame();
    let big_chrome_ink = ink_pixel_count(&big_chrome_surface);

    backend.set_chrome_font(make_font("Menlo", 4.0).expect("Menlo installed"));
    backend.set_current_font(make_font("Menlo", 32.0).expect("Menlo installed"));
    let big_editor_surface = BitmapSurface::new(W, H);
    backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
    backend.enter_frame_scope(big_editor_surface.context_ptr(), |b| paint(b, area));
    backend.end_frame();
    let big_editor_ink = ink_pixel_count(&big_editor_surface);

    (big_chrome_ink, big_editor_ink)
}

/// Asserts `paint` paints in [`quadraui::FontRole::Chrome`]: ink tracks
/// `chrome_font`'s size, not `current_font`'s (see this file's module
/// doc).
fn assert_paints_in_chrome_font(name: &str, paint: impl Fn(&mut MacBackend, Rect)) {
    let (big_chrome_ink, big_editor_ink) = ink_under_font_sizes(paint);
    assert!(
        big_chrome_ink > 0 && big_editor_ink > 0,
        "{name}: both font configurations must paint real ink: \
         big_chrome={big_chrome_ink}, big_editor={big_editor_ink}"
    );
    assert!(
        big_chrome_ink > big_editor_ink * 2,
        "{name} must paint in FontRole::Chrome, not FontRole::Editor — a 32pt chrome \
         font should paint far more ink than a 4pt one if `chrome_font` really drives \
         this primitive's glyphs: big_chrome={big_chrome_ink} (32pt chrome / 4pt editor), \
         big_editor={big_editor_ink} (4pt chrome / 32pt editor)"
    );
}

#[test]
fn draw_data_table_paints_in_chrome_font() {
    assert_paints_in_chrome_font("draw_data_table", |b, area| {
        let table = DataTable {
            id: id("data-table"),
            columns: vec![Column {
                title: "fr".to_string(),
                width: ColumnWidth::Flex(1.0),
                align: ColumnAlign::Left,
            }],
            rows: vec![DataRow {
                cells: vec![StyledText::plain("row")],
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
                label: StyledText::plain("fr"),
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
                title: "fr".to_string(),
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
            title: Some(StyledText::plain("fr")),
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
        let mut tooltip = Tooltip::new(id("tooltip"), "fr");
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
                text: StyledText::plain("fr"),
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
            label: "fr".to_string(),
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
            label: "fr".to_string(),
            frame_idx: 0,
            accent: None,
        };
        let _ = b.draw_spinner(area, &spinner);
    });
}
