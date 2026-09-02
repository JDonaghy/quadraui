//! Shared pixel-layout math for rasterisers (#499).
//!
//! `gtk::tree::gtk_tree_layout` and `macos::tree::mac_tree_layout` were
//! byte-identical apart from the function name and one comment; the
//! same was true of the two backends' `MultiSectionView` body-measure
//! match arms, and they had already **drifted** — GTK measured real
//! `MessageList` section height, macOS returned `0.0`; see
//! [`msv_body_measure`]'s doc for the fix.
//!
//! This module is the shared home for that math: **pure functions,
//! no native drawing handles** (no `cairo::Context`, no
//! `CGContextRef`, no `ID2D1RenderTarget`) — everything here takes
//! `(primitive, rect, line_height, ..)` and returns the primitive's
//! existing `*Layout` / `*Measure` struct. Backends call these and
//! keep only native painting. See
//! `quadraui/docs/PRIMITIVE_RULES.md`'s "Shared pixel-layout math"
//! section for the pattern and when to add to it.
//!
//! Where a rasteriser needs a *real* glyph width (Form's per-item hit
//! regions), it goes through [`TextMeasure`] instead of hardcoding an
//! estimate — each backend supplies a thin adapter over its live
//! font/context (see `macos::form::CtFontMeasure`).

use crate::event::Rect as QRect;
use crate::primitives::form::{FieldKind, FormField, FormFieldMeasure, FormItemMeasure};
use crate::primitives::multi_section_view::{
    LayoutMetrics, MultiSectionView, MultiSectionViewLayout, SectionAux, SectionBody,
    SectionMeasure,
};
use crate::primitives::tree::{TreeRowMeasure, TreeView, TreeViewLayout};
use crate::types::Decoration;
use crate::WidgetId;

/// Backend-supplied text measurement. Implemented per-backend over
/// whatever live font/context object it already carries (a
/// `pango::Layout`, a `CTFont`, DirectWrite metrics, …) — this trait
/// exists so the shared layout math in this module never has to name
/// a native type.
pub trait TextMeasure {
    /// Width, in pixels/DIPs, of `text` rendered in the current UI font.
    fn width_of(&self, text: &str) -> f32;
}

// ── Tree ─────────────────────────────────────────────────────────────

/// Compute the layout any pixel backend (GTK, macOS, and — with no
/// changes — a future Direct2D backend) produces for `tree` in `area`
/// at `line_height`. Header rows use `(line_height * 1.2).round()`
/// pitch, leaves/branches use `(line_height * 1.4).round()` unless
/// [`crate::types::TreeStyle::row_height`] overrides the non-header
/// pitch (#623). Chevron end-x is an *estimate*
/// (`line_height * 0.65` for the glyph) since exact glyph metrics
/// aren't available without laying out each chevron per row — every
/// backend already made this same tradeoff independently.
///
/// Coordinate frame: `visible_rows.bounds` and `hit_regions` are in
/// **tree-local** coords (origin at 0, 0). Callers subtract
/// `area.x`/`area.y` from absolute click coords before calling
/// [`TreeViewLayout::hit_test`].
pub fn tree_layout(tree: &TreeView, area: QRect, line_height: f64) -> TreeViewLayout {
    let header_height = (line_height * 1.2).round();
    let item_height = tree
        .style
        .row_height
        .map(|h| h as f64)
        .unwrap_or(line_height * 1.4)
        .round();
    let indent_px = (line_height * 0.9).round();
    let show_chevrons = tree.style.show_chevrons;
    tree.layout(area.width, area.height, |i| {
        let row = &tree.rows[i];
        let is_header = matches!(row.decoration, Decoration::Header);
        let row_h = if is_header {
            header_height as f32
        } else {
            item_height as f32
        };
        // Approximate chevron end x in tree-local pixels:
        //   2px left margin + indent levels + estimated chevron glyph width + 4px gap.
        let chevron_end_x = if row.is_expanded.is_some() && show_chevrons {
            let est_glyph_w = line_height * 0.65;
            Some((2.0 + row.indent as f64 * indent_px + est_glyph_w + 4.0) as f32)
        } else {
            None
        };
        TreeRowMeasure {
            height: row_h,
            chevron_end_x,
        }
    })
}

// ── MultiSectionView ────────────────────────────────────────────────

/// Compute the [`LayoutMetrics`] any pixel backend derives from a
/// `line_height`. Backends call this AND the primitive's `layout()`
/// with the same metrics so paint and click resolve to the same
/// bounds.
pub fn msv_metrics(line_height: f64, allow_resize: bool) -> LayoutMetrics {
    LayoutMetrics {
        header_size: (line_height * 1.4) as f32,
        divider_size: if allow_resize { 1.0 } else { 0.0 },
        // 8px gives a visible scrollbar against typical dark sidebar
        // backgrounds.
        scrollbar_size: 8.0,
        // Pixel backends paint at sub-pixel precision; no quantization.
        cell_quantum: 0.0,
    }
}

/// Compute the layout for a `MultiSectionView` using [`msv_metrics`].
/// Hosts call this to drive hit-testing without re-computing or
/// re-measuring — paint AND click share this single layout per frame.
pub fn msv_layout(
    view: &MultiSectionView,
    bounds: QRect,
    line_height: f64,
) -> MultiSectionViewLayout {
    let metrics = msv_metrics(line_height, view.allow_resize);
    view.layout(bounds, metrics, |i| {
        msv_body_measure(&view.sections[i].body, &view.sections[i].aux, line_height)
    })
}

/// Measure one section's body content size at `line_height`.
///
/// `SectionBody::MessageList` measures real content height (one
/// header line per message plus its wrapped body lines) rather than
/// returning `0.0`. Before #499 this was GTK-only — macOS's copy of
/// this match returned `0.0` for `MessageList`, a live layout bug
/// (macOS MSV sections containing a `MessageList` body collapsed to
/// zero height / mis-sized dividers). Sharing this function is what
/// fixes it: there is now exactly one measurement, so macOS gets the
/// correct height automatically.
pub fn msv_body_measure(
    body: &SectionBody,
    aux: &Option<SectionAux>,
    line_height: f64,
) -> SectionMeasure {
    let item_h = (line_height * 1.4).round() as f32;
    let aux_size = if aux.is_some() { item_h } else { 0.0 };
    let content_size = match body {
        SectionBody::Tree(t) => {
            let header_h = (line_height * 1.2).round() as f32;
            let mut total = 0.0_f32;
            for row in &t.rows {
                let is_header = matches!(row.decoration, Decoration::Header);
                total += if is_header { header_h } else { item_h };
            }
            total
        }
        SectionBody::List(l) => {
            let title_h = if l.title.is_some() {
                line_height as f32
            } else {
                0.0
            };
            title_h + l.items.len() as f32 * item_h
        }
        SectionBody::Form(f) => f.fields.len() as f32 * item_h,
        SectionBody::Chart(c) => {
            if matches!(c.kind, crate::primitives::chart::ChartKind::Sparkline) {
                line_height as f32
            } else {
                item_h * 8.0
            }
        }
        SectionBody::MessageList(m) => {
            // 1 header row + body lines per message.
            m.rows
                .iter()
                .map(|r| {
                    let lines = r.text.lines().count().max(1) as f32;
                    line_height as f32 + lines * line_height as f32
                })
                .sum()
        }
        SectionBody::Terminal(_) => 0.0,
        SectionBody::Text(lines) => lines.len() as f32 * line_height as f32,
        SectionBody::Empty(_) => item_h * 4.0, // icon + text + hint + action
        SectionBody::Custom(_) => 0.0,
    };
    SectionMeasure {
        content_size,
        aux_size,
    }
}

// ── Form ─────────────────────────────────────────────────────────────

/// Per-field row height at `line_height`: `(line_height * 1.4).round()`.
pub fn form_row_height(line_height: f64) -> f32 {
    (line_height * 1.4).round() as f32
}

/// X offset where row items (`ToggleGroup` / `ButtonRow` /
/// `SegmentedControl` / `Toolbar`) start: 6px row inset + label width
/// + 12px gap (0px gap when the label is empty, for `Toolbar`).
fn items_start_x(label: &str, measure: &dyn TextMeasure) -> f32 {
    let label_w = measure.width_of(label);
    if label_w > 0.0 {
        6.0 + label_w + 12.0
    } else {
        6.0
    }
}

fn label_text(field: &FormField) -> String {
    field.label.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Measure one `Form` field at `row_h`, using `measure` for any
/// per-item glyph widths (`ToggleGroup` / `ButtonRow` /
/// `SegmentedControl` / `Toolbar`).
///
/// `FieldKind::TextArea`'s multi-row height (`row_h * visible_rows`)
/// is intentionally **not** special-cased here — no pixel backend
/// currently paints a real multi-row `TextArea` (see
/// `macos::form`'s module doc, "Scope omissions"), so folding that in
/// here would be a silent behavior change beyond #499's scope, not a
/// dedup. Do that as its own follow-up once a backend actually renders
/// it.
pub fn form_field_measure(
    field: &FormField,
    row_h: f32,
    measure: &dyn TextMeasure,
) -> FormFieldMeasure {
    match &field.kind {
        FieldKind::ToggleGroup { toggles } => {
            let start_x = items_start_x(&label_text(field), measure);
            let items = toggles
                .iter()
                .map(|t| FormItemMeasure {
                    id: t.id.clone(),
                    width: measure.width_of(&t.label),
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, 8.0, items)
        }
        FieldKind::ButtonRow { buttons } => {
            let start_x = items_start_x(&label_text(field), measure);
            let items = buttons
                .iter()
                .map(|b| FormItemMeasure {
                    id: b.id.clone(),
                    width: measure.width_of(&format!("[{}]", b.label)),
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, 8.0, items)
        }
        FieldKind::SegmentedControl { options, .. } => {
            let start_x = items_start_x(&label_text(field), measure);
            let items = options
                .iter()
                .enumerate()
                .map(|(idx, opt)| FormItemMeasure {
                    id: WidgetId::new(format!("{}__seg_{idx}", field.id.as_str())),
                    width: measure.width_of(&format!("[{opt}]")),
                })
                .collect();
            // Segments butt up against each other — no inter-item gap.
            FormFieldMeasure::with_items(row_h, start_x, 0.0, items)
        }
        FieldKind::Toolbar(toolbar) => {
            use crate::primitives::toolbar::ToolbarButton;
            let label = label_text(field);
            let start_x = items_start_x(&label, measure);
            let items = toolbar
                .buttons
                .iter()
                .map(|btn| {
                    let id = match btn {
                        ToolbarButton::Action { id, .. } => id.clone(),
                        _ => field.id.clone(),
                    };
                    let width = match btn {
                        ToolbarButton::Action {
                            label,
                            icon,
                            key_hint,
                            ..
                        } => {
                            let mut text = String::new();
                            if let Some(ic) = icon {
                                text.push_str(ic);
                                text.push(' ');
                            }
                            text.push_str(label);
                            if let Some(hint) = key_hint {
                                text.push_str(" (");
                                text.push_str(hint);
                                text.push(')');
                            }
                            measure.width_of(&text) + 16.0
                        }
                        ToolbarButton::Separator => 12.0,
                        ToolbarButton::Label { text, .. } => measure.width_of(text),
                    };
                    FormItemMeasure { id, width }
                })
                .collect();
            FormFieldMeasure::with_items(row_h, start_x, 0.0, items)
        }
        _ => FormFieldMeasure::new(row_h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::message_list::{MessageList, MessageRow};
    use crate::primitives::multi_section_view::SectionBody;
    use crate::types::Color;

    #[test]
    fn msv_body_measure_message_list_is_not_zero() {
        // Regression for the macOS drift #499 fixes: MessageList bodies
        // must measure real content height, not 0.0.
        let rows = vec![MessageRow::new(
            "line one\nline two",
            Color::rgb(255, 255, 255),
            0.0,
        )];
        let body = SectionBody::MessageList(MessageList {
            id: WidgetId::new("test-msg-list"),
            rows,
            scroll_top: 0,
        });
        let measure = msv_body_measure(&body, &None, 20.0);
        assert!(
            measure.content_size > 0.0,
            "MessageList section must measure non-zero height, got {}",
            measure.content_size
        );
        // 1 header line + 2 body lines, at line_height 20.
        assert_eq!(measure.content_size, 20.0 + 2.0 * 20.0);
    }

    /// Fixed-width stand-in for a real font: `6.0` px per char. Lets
    /// `form_field_measure` (only exercised for real by macOS's
    /// `CtFontMeasure`, which can't run on a non-macOS CI host) get a
    /// platform-independent regression test here instead.
    struct FakeMeasure;
    impl TextMeasure for FakeMeasure {
        fn width_of(&self, text: &str) -> f32 {
            text.chars().count() as f32 * 6.0
        }
    }

    fn field_with(id: &str, label: &str, kind: FieldKind) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: crate::types::StyledText::plain(label),
            kind,
            hint: crate::types::StyledText::plain(""),
            disabled: false,
            validation: None,
        }
    }

    #[test]
    fn form_field_measure_toggle_group_computes_start_x_and_item_widths() {
        use crate::primitives::form::ToggleGroupItem;

        let field = field_with(
            "flags",
            "Flags", // 5 chars * 6.0 = 30.0
            FieldKind::ToggleGroup {
                toggles: vec![
                    ToggleGroupItem {
                        id: WidgetId::new("case"),
                        label: "Aa".into(), // 2 chars
                        value: false,
                    },
                    ToggleGroupItem {
                        id: WidgetId::new("word"),
                        label: "Word".into(), // 4 chars
                        value: true,
                    },
                ],
            },
        );
        let m = form_field_measure(&field, 20.0, &FakeMeasure);
        assert_eq!(m.height, 20.0);
        assert_eq!(m.items_start_x, 6.0 + 30.0 + 12.0);
        assert_eq!(m.item_gap, 8.0);
        assert_eq!(m.item_measures.len(), 2);
        assert_eq!(m.item_measures[0].width, 2.0 * 6.0);
        assert_eq!(m.item_measures[1].width, 4.0 * 6.0);
    }

    #[test]
    fn form_field_measure_segmented_control_has_no_item_gap() {
        let field = field_with(
            "scope",
            "",
            FieldKind::SegmentedControl {
                options: vec!["File".into(), "Project".into()],
                selected_idx: 0,
            },
        );
        let m = form_field_measure(&field, 20.0, &FakeMeasure);
        // Empty label -> 6px inset, no +label_w+12 addition.
        assert_eq!(m.items_start_x, 6.0);
        assert_eq!(m.item_gap, 0.0);
        assert_eq!(m.item_measures[0].id, WidgetId::new("scope__seg_0"));
        assert_eq!(m.item_measures[1].id, WidgetId::new("scope__seg_1"));
    }

    #[test]
    fn form_field_measure_default_kind_has_no_items() {
        let field = field_with("name", "Name", FieldKind::Button);
        let m = form_field_measure(&field, 20.0, &FakeMeasure);
        assert_eq!(m.height, 20.0);
        assert!(m.item_measures.is_empty());
        assert_eq!(m.items_start_x, 0.0);
        assert_eq!(m.item_gap, 0.0);
    }
}
