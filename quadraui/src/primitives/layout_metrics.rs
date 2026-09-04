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
use crate::primitives::list::{ListItemMeasure, ListView, ListViewLayout};
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

// ── List ─────────────────────────────────────────────────────────────

/// Compute the pixel-unit layout for a [`ListView`], including the
/// horizontal-scrollbar row reservation (#712). Every pixel backend
/// (`gtk_list_layout`, `mac_list_layout`, and any future Direct2D
/// equivalent) calls this one function so [`crate::Backend::list_layout`]
/// and the layout a backend actually paints can never disagree about
/// whether a row is reserved for the h-scrollbar.
///
/// Before #712, `gtk_list_layout` had this reservation inline and
/// `mac_list_layout` had none at all — `macos::list::draw_list`
/// recomputed a *second*, reduced-height layout only when painting, so
/// `MacBackend::list_layout` was one row taller than what macOS
/// actually painted whenever `max_content_width` forced a scrollbar.
/// This function is that reservation logic, generalised over
/// `border_inset` so both backends share one implementation instead of
/// two copies that can drift.
///
/// `char_width` is used only for the h-scrollbar-overflow threshold
/// check (`ListView::max_content_width` is in character columns); pass
/// [`crate::Backend::char_width`]'s cached value when no live text
/// measurer is available — the same approximation
/// `GtkBackend::list_hscrollbar` / `MacBackend::list_hscrollbar` already
/// use for this exact check.
///
/// `border_inset` is the pixel border a backend reserves around a
/// `bordered` list: `1.0` for GTK's `bordered` lists, `0.0` for
/// backends — like macOS today — that don't yet paint a list border
/// (see `macos::list`'s module doc "Scope omissions").
///
/// Coordinate frame: **LOCAL** — relative to `(0, 0)`, matching every
/// other layout_metrics fn. Windows does not yet reserve an
/// h-scrollbar row at all (`win_list_layout` has no `char_width`
/// parameter and no overflow check) — a real gap, not drift, tracked
/// separately from this fix; see `win::list`'s module doc.
pub fn list_layout(
    list: &ListView,
    w: f64,
    h: f64,
    line_height: f64,
    char_width: f64,
    border_inset: f64,
) -> ListViewLayout {
    let visible_px = (w - border_inset * 2.0).max(0.0);
    let needs_hscrollbar = list
        .max_content_width
        .is_some_and(|n| n as f64 * char_width > visible_px);
    let hscrollbar_h = if needs_hscrollbar { line_height } else { 0.0 };
    let title_h = if list.title.is_some() {
        line_height as f32
    } else {
        0.0
    };
    let layout_w = (w - border_inset * 2.0) as f32;
    let layout_h = (h - border_inset * 2.0 - hscrollbar_h).max(0.0) as f32;
    list.layout(layout_w, layout_h, title_h, |_| {
        ListItemMeasure::new(line_height as f32)
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
/// + 12px gap.
///
/// `for_toolbar` selects `Toolbar`'s pre-#499 behaviour: when its
/// label is empty, skip the `+ label_w + 12` entirely and start at
/// the 6px inset. `ToggleGroup` / `ButtonRow` / `SegmentedControl`
/// never had that special case — pre-#499 macOS computed the
/// unconditional `6.0 + label_w + 12.0` for all three even with an
/// empty label (GTK's still-unmigrated inline copy in
/// `gtk/backend.rs` does the same) — so `for_toolbar` must be `false`
/// for those three to keep this shared fn from silently changing
/// their output. See `quadraui/docs/PRIMITIVE_RULES.md`'s "Shared
/// pixel-layout math" section.
fn items_start_x(label: &str, measure: &dyn TextMeasure, for_toolbar: bool) -> f32 {
    if for_toolbar && label.is_empty() {
        return 6.0;
    }
    let label_w = measure.width_of(label);
    6.0 + label_w + 12.0
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
            let start_x = items_start_x(&label_text(field), measure, false);
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
            let start_x = items_start_x(&label_text(field), measure, false);
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
            let start_x = items_start_x(&label_text(field), measure, false);
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
            use crate::primitives::toolbar::{measure_button, ToolbarButton};
            let label = label_text(field);
            let start_x = items_start_x(&label, measure, true);
            let items = toolbar
                .buttons
                .iter()
                .map(|btn| {
                    let id = match btn {
                        ToolbarButton::Action { id, .. } => id.clone(),
                        _ => field.id.clone(),
                    };
                    // Single button-measure formula shared with every
                    // pixel rasteriser (#730) — see `measure_button`'s
                    // doc.
                    let width = measure_button(measure, btn);
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
        // `SegmentedControl` (unlike `Toolbar`) has no empty-label
        // special case: pre-#499 macOS and GTK's still-unmigrated
        // inline copy both compute the unconditional
        // `6.0 + label_w + 12.0` even when the label is empty.
        // Regression guard for the #499 review finding: this used to
        // silently drop to `6.0`.
        assert_eq!(m.items_start_x, 6.0 + 0.0 + 12.0);
        assert_eq!(m.item_gap, 0.0);
        assert_eq!(m.item_measures[0].id, WidgetId::new("scope__seg_0"));
        assert_eq!(m.item_measures[1].id, WidgetId::new("scope__seg_1"));
    }

    #[test]
    fn form_field_measure_toolbar_start_x_skips_gap_only_when_label_empty() {
        use crate::primitives::toolbar::{Toolbar, ToolbarButton};

        let toolbar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Separator],
            bg: None,
            focused_index: None,
        };

        // Empty label -> Toolbar's pre-#499 special case: 6px inset,
        // no +label_w+12 addition.
        let empty_label = field_with("tb", "", FieldKind::Toolbar(toolbar.clone()));
        let m = form_field_measure(&empty_label, 20.0, &FakeMeasure);
        assert_eq!(m.items_start_x, 6.0);

        // Non-empty label -> unconditional 6 + label_w + 12, same as
        // the other row-item kinds.
        let with_label = field_with("tb", "Go", FieldKind::Toolbar(toolbar)); // 2 chars * 6.0 = 12.0
        let m = form_field_measure(&with_label, 20.0, &FakeMeasure);
        assert_eq!(m.items_start_x, 6.0 + 12.0 + 12.0);
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

    /// Parity guard for issue #710: `GtkBackend::form_layout`,
    /// `macos::form::mac_form_layout`, and `win::form::win_form_layout`
    /// all measure every `FieldKind` by calling this one function —
    /// so this test is the single place that proves the invariant
    /// their painters rely on: `FormFieldMeasure::height` is always
    /// exactly one `row_h`, for *every* `FieldKind`, regardless of any
    /// field-specific content (multiple toggles, a long `TextArea`
    /// value, a large `visible_rows`).
    ///
    /// #710 was exactly a violation of this on GTK: before that fix,
    /// `GtkBackend::form_layout` had its own inline measurer (not this
    /// fn) that sized `FieldKind::TextArea` as `row_h * visible_rows`,
    /// while `gtk::form::draw_form` only ever painted one row per
    /// field — so the `visible_rows - 1` rows the layout believed
    /// belonged to the `TextArea` were actually painted with the
    /// *following* fields, and still hit-tested to the `TextArea`.
    /// Now that GTK measures via this shared fn too (matching macOS
    /// and Windows), a future regression that special-cases `TextArea`
    /// (or any other kind) back to a multi-row height here would be
    /// caught by every backend at once, instead of silently
    /// reappearing on just one of them.
    #[test]
    fn form_field_measure_height_is_one_row_for_every_field_kind() {
        use crate::primitives::form::{ButtonRowItem, ToggleGroupItem};
        use crate::primitives::toolbar::Toolbar;
        use crate::types::{Color, StyledText};

        const ROW_H: f32 = 20.0;

        let kinds = vec![
            ("label", FieldKind::Label),
            ("toggle", FieldKind::Toggle { value: true }),
            (
                "text-input",
                FieldKind::TextInput {
                    value: "hello".into(),
                    placeholder: String::new(),
                    cursor: Some(2),
                    selection_anchor: None,
                },
            ),
            ("button", FieldKind::Button),
            (
                "read-only",
                FieldKind::ReadOnly {
                    value: StyledText::plain("v1.0"),
                },
            ),
            (
                "slider",
                FieldKind::Slider {
                    value: 5.0,
                    min: 0.0,
                    max: 10.0,
                    step: 1.0,
                },
            ),
            (
                "color-picker",
                FieldKind::ColorPicker {
                    value: Color::rgb(255, 0, 0),
                },
            ),
            (
                "dropdown",
                FieldKind::Dropdown {
                    options: vec![StyledText::plain("a"), StyledText::plain("b")],
                    selected_idx: 0,
                },
            ),
            (
                // The regression case: a large `visible_rows` must NOT
                // scale `height` — see this test's own doc comment.
                "text-area",
                FieldKind::TextArea {
                    value: "line one\nline two\nline three".into(),
                    placeholder: String::new(),
                    cursor: Some(3),
                    visible_rows: 6,
                },
            ),
            (
                "password",
                FieldKind::PasswordInput {
                    value: "hunter2".into(),
                    placeholder: String::new(),
                    cursor: Some(3),
                    mask_char: '•',
                },
            ),
            (
                "segmented",
                FieldKind::SegmentedControl {
                    options: vec!["File".into(), "Folder".into()],
                    selected_idx: 0,
                },
            ),
            (
                "toggle-group",
                FieldKind::ToggleGroup {
                    toggles: vec![
                        ToggleGroupItem {
                            id: WidgetId::new("a"),
                            label: "Aa".into(),
                            value: false,
                        },
                        ToggleGroupItem {
                            id: WidgetId::new("b"),
                            label: "Bb".into(),
                            value: true,
                        },
                    ],
                },
            ),
            (
                "button-row",
                FieldKind::ButtonRow {
                    buttons: vec![ButtonRowItem {
                        id: WidgetId::new("find"),
                        label: "Find".into(),
                        disabled: false,
                        icon: None,
                    }],
                },
            ),
            (
                "toolbar",
                FieldKind::Toolbar(Toolbar {
                    id: WidgetId::new("tb"),
                    buttons: vec![],
                    bg: None,
                    focused_index: None,
                }),
            ),
        ];

        for (name, kind) in kinds {
            let field = field_with(name, "Label", kind);
            let m = form_field_measure(&field, ROW_H, &FakeMeasure);
            assert_eq!(
                m.height, ROW_H,
                "FieldKind::{name} must measure exactly one row_h ({ROW_H}), got {}",
                m.height,
            );
        }
    }

    // ── List (#712) ──────────────────────────────────────────────────

    fn list_with_max_content_width(n_items: usize, max_content_width: Option<usize>) -> ListView {
        use crate::primitives::list::ListItem;
        use crate::types::StyledText;

        ListView {
            id: WidgetId::new("l"),
            title: None,
            items: (0..n_items)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("row {i}")),
                    icon: None,
                    detail: None,
                    decoration: Decoration::Normal,
                })
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width,
            show_v_scrollbar: false,
        }
    }

    /// The single reservation rule #712 exists to unify: when
    /// `max_content_width` (in chars) exceeds the visible width (chars ×
    /// `char_width`, in pixels), the bottom `line_height` row is reserved
    /// for the h-scrollbar and is not available to items — one fewer row
    /// fits than when no overflow is signalled. Every pixel backend
    /// shares this exact function, so proving it here proves it for all
    /// of them at once.
    #[test]
    fn list_layout_reserves_hscrollbar_row_when_content_overflows() {
        const LINE_HEIGHT: f64 = 10.0;
        const CHAR_WIDTH: f64 = 8.0;
        const W: f64 = 200.0; // 25 chars visible at CHAR_WIDTH.
        const H: f64 = 100.0; // 10 rows at LINE_HEIGHT with no reservation.

        let overflowing = list_with_max_content_width(12, Some(1000));
        let fitting = list_with_max_content_width(12, None);

        let with_reservation = list_layout(&overflowing, W, H, LINE_HEIGHT, CHAR_WIDTH, 0.0);
        let without_reservation = list_layout(&fitting, W, H, LINE_HEIGHT, CHAR_WIDTH, 0.0);

        assert_eq!(
            without_reservation.visible_items.len(),
            10,
            "sanity: 100px / 10px rows == 10 full rows with no reservation"
        );
        assert_eq!(
            with_reservation.visible_items.len(),
            9,
            "overflowing max_content_width must reserve exactly one \
             line_height row for the h-scrollbar, leaving 9 rows"
        );

        let last = with_reservation.visible_items.last().unwrap();
        assert!(
            (last.bounds.y + last.bounds.height) as f64 <= H - LINE_HEIGHT,
            "reserved layout's content must stop at or before the \
             scrollbar row's top edge"
        );
    }

    #[test]
    fn list_layout_border_inset_reduces_visible_width_for_overflow_check() {
        // A `max_content_width` that only overflows once the border
        // inset narrows the visible width must still trigger the
        // reservation — mirrors GTK's `bordered` lists.
        const LINE_HEIGHT: f64 = 10.0;
        const CHAR_WIDTH: f64 = 8.0;
        const W: f64 = 200.0;
        const H: f64 = 100.0;
        const BORDER_INSET: f64 = 1.0;

        // 24 chars * 8px = 192px, which fits in the full 200px width but
        // not in the 198px width left after a 1px border inset on each
        // side... so pick a value that straddles exactly that gap.
        let list = list_with_max_content_width(12, Some(25)); // 25*8=200 > 198

        let with_border = list_layout(&list, W, H, LINE_HEIGHT, CHAR_WIDTH, BORDER_INSET);
        let without_border = list_layout(&list, W, H, LINE_HEIGHT, CHAR_WIDTH, 0.0);

        assert_eq!(
            without_border.visible_items.len(),
            10,
            "200px content width == 200px visible width: no overflow, no reservation"
        );
        assert_eq!(
            with_border.visible_items.len(),
            9,
            "200px content width > 198px visible width once border-inset \
             narrows it: overflow, reservation kicks in"
        );
    }
}
