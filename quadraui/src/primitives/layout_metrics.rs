//! Shared pixel-layout math for rasterisers.
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
//! estimate — each backend supplies this however fits its own font
//! API: GTK wraps a scratch `pango::Layout` (`gtk::backend::PangoTextMeasure`),
//! while macOS's `CTFont` and Windows' `DWrite` handle implement
//! [`TextMeasure`] directly, with no wrapper struct needed.

use crate::event::Rect as QRect;
use crate::primitives::board::{BoardLayout, BoardMeasure, BoardModel};
use crate::primitives::chart::{Chart, ChartLayout, ChartMeasure};
use crate::primitives::data_table::{ColumnMeasure, DataTable, DataTableLayout};
use crate::primitives::form::{
    FieldKind, FormField, FormFieldMeasure, FormItemMeasure, ValidationState,
};
use crate::primitives::list::{ListItemMeasure, ListView, ListViewLayout};
use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapScale, MinimapSizing};
use crate::primitives::multi_section_view::{
    MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout, SectionAux, SectionBody,
    SectionMeasure,
};
use crate::primitives::panel::{Panel, PanelLayout, PanelMeasure};
use crate::primitives::pipeline_view::{PipelineView, PipelineViewLayout, PipelineViewMeasure};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout, ProgressBarMeasure};
use crate::primitives::split::{Split, SplitLayout, SplitMeasure};
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout, SplitTreeMeasure};
use crate::primitives::tab_bar::{
    SegmentMeasure, TabBar, TabBarLayout, TabChrome, TabFrame, TabMeasure,
};
use crate::primitives::toast::{ToastMeasure, ToastOverlay, ToastStackLayout};
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

/// Pixel/DIP-unit layout constants: logical pixels for `gtk`/`macos`,
/// DIPs for `win` — the same numeric unit convention every backend shares.
/// One definition here means one place to change a value backends must
/// agree on.
pub mod pixel {
    /// [`crate::Split`] / [`crate::primitives::split_tree::SplitTree`]
    /// divider thickness.
    pub const DIVIDER: f32 = 4.0;

    /// [`crate::ToastOverlay`] max toast width.
    pub const TOAST_WIDTH: f32 = 320.0;
    /// [`crate::ToastOverlay`] margin from the viewport edge.
    pub const TOAST_MARGIN: f32 = 12.0;
    /// [`crate::ToastOverlay`] gap between stacked toasts.
    pub const TOAST_GAP: f32 = 8.0;
    /// [`crate::ToastOverlay`] vertical padding inside a toast box.
    pub const TOAST_PADDING: f32 = 8.0;
    /// [`crate::ToastOverlay`] width (and height — the dismiss affordance
    /// is a square) of the dismiss (`×`) affordance.
    pub const TOAST_DISMISS_WIDTH: f32 = 28.0;
    /// [`crate::ToastOverlay`] extra width reserved around an action label.
    pub const TOAST_ACTION_PADDING: f32 = 16.0;
    /// [`crate::ToastOverlay`] horizontal gap between two adjacent action
    /// buttons on the button row.
    pub const TOAST_ACTION_GAP: f32 = 8.0;

    /// [`crate::ProgressBar`] width of the cancel (`×`) affordance.
    pub const PROGRESS_CANCEL_WIDTH: f32 = 28.0;
    /// [`crate::ProgressBar`] width of the sliding indeterminate pulse.
    pub const PROGRESS_PULSE_WIDTH: f32 = 40.0;

    /// [`crate::Panel`] width reserved per title-bar action button.
    pub const PANEL_ACTION_BUTTON: f32 = 24.0;

    /// [`crate::primitives::pipeline_view::PipelineView`] arrow connector
    /// width between stage boxes.
    pub const PIPELINE_ARROW_WIDTH: f32 = 32.0;
    /// [`crate::primitives::pipeline_view::PipelineView`] height reserved
    /// for a stage's action button.
    pub const PIPELINE_ACTION_HEIGHT: f32 = 22.0;
    /// [`crate::primitives::pipeline_view::PipelineView`] height reserved
    /// above stage boxes for the keyboard-focus caret strip.
    pub const PIPELINE_FOCUS_INDICATOR_H: f32 = 8.0;
    /// [`crate::primitives::pipeline_view::PipelineView`] stage-box corner
    /// radius (paint-only — [`super::PipelineViewLayout`] carries only
    /// rectangles).
    pub const CORNER_RADIUS: f64 = 4.0;
    /// [`crate::primitives::pipeline_view::PipelineView`] horizontal
    /// padding inside a stage box (paint-only).
    pub const PIPELINE_H_PAD: f64 = 8.0;
    /// [`crate::primitives::pipeline_view::PipelineView`] stage-box
    /// border stroke width (paint-only).
    pub const PIPELINE_BORDER_WIDTH: f64 = 1.0;

    /// [`crate::DataTable`] width reserved for the vertical scrollbar.
    pub const DATA_TABLE_SCROLLBAR_WIDTH: f32 = 8.0;

    /// [`crate::TreeView`] vertical scrollbar column width on pixel
    /// backends, matching VS Code's list/tree scrollbar (10px) — see
    /// [`super::tree_row_pitch`]'s doc for why the column width must not
    /// be derived from row pitch. TUI passes `1.0` (one cell) instead.
    pub const TREE_VSCROLLBAR_WIDTH: f32 = 10.0;

    /// [`crate::Form`] horizontal padding (both sides combined) reserved
    /// around a `ButtonRow` / `SegmentedControl` item's label. Replaces
    /// measuring the label wrapped in `"[label]"` bracket glyphs — that
    /// over-counted width by the bracket chars' own measured width
    /// while the GUI painter never actually drew them, and read as a
    /// terminal affordance on a pixel canvas.
    pub const FORM_BUTTON_ITEM_PADDING: f32 = 16.0;

    /// [`crate::primitives::minimap::Minimap`] buffer lines painted per
    /// minimap row. Every pixel backend shows one buffer line per row (no
    /// cross-line colour reduction — see [`crate::MinimapGrid`]'s doc for
    /// why TUI's braille packing differs).
    pub const MINIMAP_LINES_PER_ROW: usize = 1;

    /// [`crate::TabBar`] non-compact per-tab horizontal padding (left +
    /// right) inside the tab background fill. `gtk::tab_bar::TAB_PAD`,
    /// `macos::tab_bar::TAB_PAD`, and `win::tab_bar::TAB_PAD_DIP` were all
    /// `14.0` independently.
    pub const TAB_PAD: f32 = 14.0;
    /// [`crate::TabBar`] gap between a tab's label and its close glyph.
    pub const TAB_INNER_GAP: f32 = 10.0;
    /// [`crate::TabBar`] gap between adjacent tabs.
    pub const TAB_OUTER_GAP: f32 = 1.0;
    /// [`crate::TabBar`] gap between a tab's icon glyph
    /// ([`crate::TabIcon`]) and its label.
    pub const TAB_ICON_GAP: f32 = 6.0;
    /// 15-glyph sample string every pixel backend measures once per frame
    /// to estimate an average glyph advance for
    /// [`super::pixel_tab_bar_layout`]'s `available_cols` — proportional
    /// fonts have no single "char width", so this is the same estimate
    /// convention `gtk`/`macos` already used independently.
    pub const TAB_CELL_WIDTH_SAMPLE: &str = "ABCDabcd0123.:_";
}

// ── Tree ─────────────────────────────────────────────────────────────

/// Compute the layout any pixel backend (GTK, macOS, and — with no
/// changes — a future Direct2D backend) produces for `tree` in `area`
/// at `line_height`. Header rows use `(line_height * 1.2).round()`
/// pitch, leaves/branches use `(line_height * 1.4).round()` unless
/// [`crate::types::TreeStyle::row_height`] overrides the non-header
/// pitch. Chevron end-x is an *estimate*
/// (`line_height * 0.65` for the glyph) since no [`TextMeasure`] is
/// available here — every backend already made this same tradeoff
/// independently. Callers with a real measurer should prefer
/// [`tree_layout_measured`], which measures
/// [`crate::types::TreeStyle::chevron_expanded`] /
/// [`crate::types::TreeStyle::chevron_collapsed`] exactly instead of
/// estimating.
///
/// Coordinate frame: `visible_rows.bounds` and `hit_regions` are in
/// **tree-local** coords (origin at 0, 0). Callers subtract
/// `area.x`/`area.y` from absolute click coords before calling
/// [`TreeViewLayout::hit_test`].
pub fn tree_layout(tree: &TreeView, area: QRect, line_height: f64) -> TreeViewLayout {
    tree_layout_inner(tree, area, line_height, None)
}

/// Like [`tree_layout`], but measures the chevron glyph actually
/// configured on `tree.style` (`chevron_expanded` / `chevron_collapsed`)
/// through `measure` instead of estimating its pixel width as
/// `line_height * 0.65`. This is the pixel-backend equivalent of
/// `tui::tree::tui_tree_layout`'s exact character-count measurement —
/// TUI cells are fixed-width so an exact count is free there; pixel
/// backends need a real [`TextMeasure`] to get the same precision.
pub fn tree_layout_measured(
    tree: &TreeView,
    area: QRect,
    line_height: f64,
    measure: &dyn TextMeasure,
) -> TreeViewLayout {
    tree_layout_inner(tree, area, line_height, Some(measure))
}

fn tree_layout_inner(
    tree: &TreeView,
    area: QRect,
    line_height: f64,
    measure: Option<&dyn TextMeasure>,
) -> TreeViewLayout {
    let header_height = (line_height * 1.2).round();
    let item_height = tree_row_pitch(tree, line_height);
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
        // Chevron end x in tree-local pixels:
        //   2px left margin + indent levels + glyph width + 4px gap.
        let chevron_end_x = if row.is_expanded.is_some() && show_chevrons {
            let glyph_w = match (measure, row.is_expanded) {
                (Some(m), Some(true)) => m.width_of(&tree.style.chevron_expanded) as f64,
                (Some(m), Some(false)) => m.width_of(&tree.style.chevron_collapsed) as f64,
                _ => line_height * 0.65,
            };
            Some((2.0 + row.indent as f64 * indent_px + glyph_w + 4.0) as f32)
        } else {
            None
        };
        TreeRowMeasure {
            height: row_h,
            chevron_end_x,
        }
    })
}

/// The non-header row pitch [`tree_layout`] uses for the bulk of a
/// tree's rows: [`crate::types::TreeStyle::row_height`] when the host
/// set one, else `(line_height * 1.4).round()`.
///
/// [`TreeView::vscrollbar`] takes a single, uniform `row_height` — it
/// has no way to reproduce `tree_layout`'s header-vs-leaf/branch split
/// (headers pitch at `line_height * 1.2`) — so this is the pitch every
/// pixel backend's `tree_vscrollbar` should pass it. Passing the leaf/branch
/// pitch instead of raw `line_height` matches what `tree_layout` paints for
/// non-header rows (the overwhelming majority of rows in any real tree),
/// and respects `TreeStyle::row_height` when set.
pub fn tree_row_pitch(tree: &TreeView, line_height: f64) -> f64 {
    tree.style
        .row_height
        .map(|h| h as f64)
        .unwrap_or(line_height * 1.4)
        .round()
}

// ── List ─────────────────────────────────────────────────────────────

/// Compute the pixel-unit layout for a [`ListView`], including the
/// horizontal-scrollbar row reservation. Every pixel backend
/// (`gtk_list_layout`, `mac_list_layout`, and any future Direct2D
/// equivalent) calls this one function so [`crate::Backend::list_layout`]
/// and the layout a backend actually paints can never disagree about
/// whether a row is reserved for the h-scrollbar.
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
/// other layout_metrics fn.
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

/// Compute the [`MsvLayoutMetrics`] any pixel backend derives from a
/// `line_height`. Backends call this AND the primitive's `layout()`
/// with the same metrics so paint and click resolve to the same
/// bounds.
pub fn msv_metrics(line_height: f64, allow_resize: bool) -> MsvLayoutMetrics {
    MsvLayoutMetrics {
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
/// returning `0.0`.
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
/// `for_toolbar` selects `Toolbar`'s behaviour: when its label is empty,
/// skip the `+ label_w + 12` entirely and start at the 6px inset.
/// `ToggleGroup` / `ButtonRow` / `SegmentedControl` always compute the
/// unconditional `6.0 + label_w + 12.0` regardless of label content,
/// so `for_toolbar` must be `false` for those three to keep this shared fn
/// from changing their output. See `quadraui/docs/PRIMITIVE_RULES.md`'s
/// "Shared pixel-layout math" section.
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

/// `field.validation`'s message, when present and non-empty — the text a
/// GUI painter reserves a second line beneath the field for, rather
/// than painting it on top of the label at the same vertical centre
/// (see `native_surface_paint::paint`'s doc in `form.rs`).
fn validation_message(field: &FormField) -> Option<&str> {
    match &field.validation {
        Some(ValidationState::Error(m)) | Some(ValidationState::Warning(m)) if !m.is_empty() => {
            Some(m.as_str())
        }
        _ => None,
    }
}

/// Measure one `Form` field at `row_h`, using `measure` for any
/// per-item glyph widths (`ToggleGroup` / `ButtonRow` /
/// `SegmentedControl` / `Toolbar`).
///
/// `FieldKind::TextArea`'s multi-row height (`row_h * visible_rows`)
/// is intentionally **not** special-cased here — no pixel backend
/// currently paints a real multi-row `TextArea` (see
/// `macos::form`'s module doc, "Scope omissions").
///
/// When the field carries a non-empty [`ValidationState`] message, the
/// returned height is `row_h` **plus** one more approximate text line
/// (`row_h / 1.4`, inverting [`form_row_height`]) reserved for that
/// message — so the field no longer squeezes the message into the same
/// single row the label occupies. Every other field still measures
/// exactly one `row_h`, as
/// `form_field_measure_height_is_one_row_for_every_field_kind` asserts.
pub fn form_field_measure(
    field: &FormField,
    row_h: f32,
    measure: &dyn TextMeasure,
) -> FormFieldMeasure {
    let mut m = form_field_measure_inner(field, row_h, measure);
    if validation_message(field).is_some() {
        m.height += row_h / 1.4;
    }
    m
}

fn form_field_measure_inner(
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
                    id: t.id,
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
                    id: b.id,
                    width: measure.width_of(&b.label) + pixel::FORM_BUTTON_ITEM_PADDING,
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
                    width: measure.width_of(opt) + pixel::FORM_BUTTON_ITEM_PADDING,
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
                        ToolbarButton::Action { id, .. } => *id,
                        _ => field.id,
                    };
                    // Single button-measure formula shared with every
                    // pixel rasteriser — see `measure_button`'s
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

// ── Toast ────────────────────────────────────────────────────────────

/// Compute the pixel-unit layout for a [`ToastOverlay`] any pixel backend
/// produces — `gtk_toast_stack_layout`, `mac_toast_stack_layout`,
/// `win_toast_stack_layout` are copies of this exact formula.
///
/// `(origin_x, origin_y)` is baked into the returned bounds (absolute
/// frame) — hosts call `layout.hit_test(x, y)` with raw click
/// coordinates, no localisation needed, matching every other
/// `*_toast_stack_layout`.
///
/// Height grows with a wrapped body (up to
/// [`crate::primitives::toast::MAX_BODY_LINES`] lines via
/// [`crate::primitives::toast::wrap_text_lines`]), matching what
/// `native_surface_paint::paint`'s own copy of this formula actually
/// draws, so a no-paint layout call here always agrees with the last
/// paint.
#[allow(clippy::too_many_arguments)]
pub fn pixel_toast_stack_layout(
    stack: &ToastOverlay,
    measure: &dyn TextMeasure,
    origin_x: f32,
    origin_y: f32,
    viewport_width: f32,
    viewport_height: f32,
    line_height: f32,
) -> ToastStackLayout {
    stack.layout(
        origin_x,
        origin_y,
        viewport_width,
        viewport_height,
        pixel::TOAST_MARGIN,
        pixel::TOAST_GAP,
        |i| {
            let toast = &stack.toasts[i];
            let width =
                pixel::TOAST_WIDTH.min((viewport_width - pixel::TOAST_MARGIN * 2.0).max(0.0));
            let body_avail = (width - pixel::TOAST_PADDING * 2.0).max(0.0);
            let body_lines = if toast.body.is_empty() {
                0
            } else {
                crate::primitives::toast::wrap_text_lines(
                    &toast.body,
                    body_avail,
                    crate::primitives::toast::MAX_BODY_LINES,
                    &|s| measure.width_of(s),
                )
                .len()
                .max(1)
            };
            // Button row: a separate row below the title/body,
            // added only when the toast has actions.
            let has_actions = !toast.actions.is_empty();
            let button_row_h = if has_actions {
                pixel::TOAST_PADDING + line_height
            } else {
                0.0
            };
            let h = line_height
                + pixel::TOAST_PADDING * 2.0
                + body_lines as f32 * line_height
                + button_row_h;
            let action_widths: Vec<f32> = toast
                .actions
                .iter()
                .map(|a| measure.width_of(&a.label) + pixel::TOAST_ACTION_PADDING)
                .collect();
            let (dismiss_rect, action_rects) = crate::primitives::toast::toast_button_rects(
                width,
                h,
                pixel::TOAST_PADDING,
                pixel::TOAST_DISMISS_WIDTH,
                // Height is a single text row; see `native_surface_paint::paint`'s
                line_height,
                &action_widths,
                line_height,
                pixel::TOAST_ACTION_GAP,
            );
            ToastMeasure {
                width,
                height: h,
                dismiss_rect,
                action_rects,
            }
        },
    )
}

// ── Split / SplitTree ───────────────────────────────────────────────

/// Compute a [`Split`]'s layout at the shared [`pixel::DIVIDER`]
/// thickness.
pub fn pixel_split_layout(split: &Split, bounds: QRect) -> SplitLayout {
    split.layout(bounds, SplitMeasure::new(pixel::DIVIDER))
}

/// Compute a [`SplitTree`]'s layout at the shared [`pixel::DIVIDER`]
/// thickness.
pub fn pixel_split_tree_layout(tree: &SplitTree, bounds: QRect) -> SplitTreeLayout {
    tree.layout(bounds, SplitTreeMeasure::new(pixel::DIVIDER))
}

// ── Progress ─────────────────────────────────────────────────────────

/// Compute a [`ProgressBar`]'s layout — the twin of every backend's
/// `*_progress_layout`. `x`/`y`/`w`/`h` are the bar's own
/// bounds; the cancel affordance (when `bar.cancellable`) reserves
/// [`pixel::PROGRESS_CANCEL_WIDTH`].
pub fn pixel_progress_layout(
    bar: &ProgressBar,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> ProgressBarLayout {
    let cancel_width = if bar.cancellable {
        pixel::PROGRESS_CANCEL_WIDTH
    } else {
        0.0
    };
    bar.layout(
        x,
        y,
        ProgressBarMeasure {
            width: w,
            height: h,
            cancel_width,
        },
    )
}

// ── PipelineView ─────────────────────────────────────────────────────

/// Compute a [`PipelineView`]'s layout — the twin of every backend's
/// `*_pipeline_view_layout`.
///
/// Note: the returned layout (incl. `bounds`) is offset down by
/// [`pixel::PIPELINE_FOCUS_INDICATOR_H`], so `bounds.y` starts below the
/// reserved caret strip. The focus caret is painted in the gap between
/// the passed-in `y` and `bounds.y`; a host that clips drawing to
/// `layout.bounds` would clip the caret — clip to the original `(y, h)`
/// instead.
pub fn pixel_pipeline_view_layout(
    view: &PipelineView,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> PipelineViewLayout {
    let action_h = if view.stages.iter().any(|s| s.action.is_some()) {
        pixel::PIPELINE_ACTION_HEIGHT
    } else {
        0.0
    };
    view.layout(
        x,
        y + pixel::PIPELINE_FOCUS_INDICATOR_H,
        PipelineViewMeasure::new(
            w,
            (h - pixel::PIPELINE_FOCUS_INDICATOR_H).max(0.0),
            pixel::PIPELINE_ARROW_WIDTH,
            action_h,
        ),
    )
}

// ── Board ────────────────────────────────────────────────────────────

/// Compute a [`BoardModel`]'s layout — the twin of every backend's
/// `*_board_layout`. The per-column/card geometry
/// constants (`BOARD_COL_MIN_PX` etc.) were already shared via
/// [`crate::primitives::board`] before this issue; this fn just removes
/// the last bit of drift, the three near-identical wrapper calls.
pub fn pixel_board_layout(model: &BoardModel, x: f32, y: f32, w: f32, h: f32) -> BoardLayout {
    use crate::primitives::board::{
        board_layout, BOARD_CARD_GAP_PX, BOARD_CARD_H_PX, BOARD_COL_GAP_PX, BOARD_COL_MIN_PX,
        BOARD_HEADER_H_PX,
    };
    board_layout(
        model,
        x,
        y,
        w,
        h,
        BoardMeasure::new(
            BOARD_COL_MIN_PX,
            BOARD_COL_GAP_PX,
            BOARD_HEADER_H_PX,
            BOARD_CARD_H_PX,
            BOARD_CARD_GAP_PX,
        ),
    )
}

// ── Panel ────────────────────────────────────────────────────────────

/// Compute a [`Panel`]'s layout. `content_padding` is always `0.0`
/// today — no backend passes a non-zero value.
pub fn pixel_panel_layout(
    panel: &Panel,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    line_height: f32,
) -> PanelLayout {
    let bounds = QRect::new(x, y, w, h);
    let measure = PanelMeasure {
        title_bar_height: if panel.title.is_some() {
            line_height
        } else {
            0.0
        },
        action_button_width: pixel::PANEL_ACTION_BUTTON,
        content_padding: 0.0,
    };
    panel.layout(bounds, measure)
}

// ── Chart ────────────────────────────────────────────────────────────

/// Compute a [`Chart`]'s layout — the twin of every backend's
/// `*_chart_layout`. Pure passthrough to
/// [`Chart::layout`]; kept here (rather than inlined at each call site)
/// so a future backend gets it for free, matching every other fn in
/// this module.
pub fn pixel_chart_layout(
    chart: &Chart,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    line_height: f32,
    char_width: f32,
) -> ChartLayout {
    chart.layout(
        x,
        y,
        ChartMeasure {
            width: w,
            height: h,
            char_width,
            line_height,
        },
    )
}

// ── DataTable ────────────────────────────────────────────────────────

/// Compute a [`DataTable`]'s layout via `measure`.
///
/// Column headers are measured via `measure: &dyn TextMeasure` for
/// real glyph metrics when a live font/context is available, or a
/// character-count estimate otherwise, ensuring consistent column
/// width computation across all backends.
pub fn pixel_data_table_layout(
    table: &DataTable,
    w: f32,
    h: f32,
    line_height: f32,
    measure: &dyn TextMeasure,
) -> DataTableLayout {
    let header_height = (line_height * 1.2).round();
    table.layout(
        w,
        h,
        line_height,
        header_height,
        pixel::DATA_TABLE_SCROLLBAR_WIDTH,
        |col| ColumnMeasure::new(measure.width_of(&col.title)),
    )
}

// ── Minimap ──────────────────────────────────────────────────────────

/// Compute a [`Minimap`]'s layout at an explicit [`MinimapScale`].
/// Every pixel backend shows one buffer line per painted row
/// ([`pixel::MINIMAP_LINES_PER_ROW`]) — no cross-line colour reduction
/// (see [`crate::MinimapGrid`]'s doc for why TUI's braille packing
/// differs).
pub fn pixel_minimap_layout_scaled(
    minimap: &Minimap,
    bounds: QRect,
    scale: MinimapScale,
) -> MinimapLayout {
    minimap.layout_with_sizing(
        bounds,
        pixel::MINIMAP_LINES_PER_ROW,
        MinimapSizing::FixedPitch(scale.row_pitch_px() as f32),
    )
}

// ── TabBar ───────────────────────────────────────────────────────────

/// Compute a [`TabBar`]'s pixel-unit layout. Backends still do their own
/// **text measurement** — Pango / Core Text / DirectWrite each need to
/// switch fonts mid-tab (italic for a preview label, a Nerd-Font family
/// swap for an icon glyph), which a single `&dyn TextMeasure` call can't
/// express — so `tab_name_widths` and `tab_icon_extras` are that
/// backend-specific pass, precomputed by the caller and handed in as
/// parallel arrays indexed like `bar.tabs` (`0.0` for a tab with no icon).
/// Everything else is single-font and lives here: close-button geometry,
/// bracket framing, right-segment widths, the "corrected scroll offset"
/// engine-feedback signal, and `available_cols`.
///
/// `tab_pad` / `tab_inner_gap` / `tab_outer_gap` are the caller-resolved
/// values for this bar — pass the compact variant when `bar.compact` is
/// set, matching every backend's existing convention.
///
/// # The close-button fix
///
/// The close-button hit region is sized as `tab_inner_gap + close_glyph_w`
/// and trailing chrome (tab's own right padding and the gap to the next tab)
/// is pushed into [`TabMeasure::trailing_width`] — the close box now
/// covers just the glyph plus its leading gap, on every backend, bracket
/// framing or not.
///
/// # Returns
///
/// `(layout, corrected_scroll_offset, available_cols)`.
///
/// - `layout` — the resolved [`TabBarLayout`]; iterate `visible_tabs` /
///   `visible_segments` to paint, call `hit_test` for clicks. Computed
///   with `scroll_arrow_width: 0.0` (no backend using this fn paints
///   scroll arrows), so `layout.resolved_scroll_offset` is `bar
///   .scroll_offset` clamped to a valid index, not corrected — see
///   `corrected_scroll_offset` below for that.
/// - `corrected_scroll_offset` — the scroll offset that would keep the
///   active tab visible under *this frame's real measurements*. Callers
///   write this back to their stored scroll state for the next frame
///   (the "two-pass paint" pattern [`TabBar::layout`]'s doc describes).
/// - `available_cols` — the tab area's width in character-columns,
///   estimated from a [`pixel::TAB_CELL_WIDTH_SAMPLE`] sample measured
///   through `measure`. Hosts that budget tab visibility in cell units
///   even on a proportional-font backend (e.g. vimcode's
///   `Engine::set_tab_visible_count`) use this instead of a raw pixel
///   width.
#[allow(clippy::too_many_arguments)]
pub fn pixel_tab_bar_layout(
    bar: &TabBar,
    width: f32,
    height: f32,
    tab_pad: f32,
    tab_inner_gap: f32,
    tab_outer_gap: f32,
    tab_name_widths: &[f32],
    tab_icon_extras: &[f32],
    chrome: &TabChrome,
    measure: &dyn TextMeasure,
) -> (TabBarLayout, usize, usize) {
    let close_glyph_w = if bar.show_tab_close {
        measure.width_of("×")
    } else {
        0.0
    };
    let brackets = matches!(chrome.active_frame, TabFrame::Brackets);
    let (bracket_open_w, bracket_close_w) = if brackets {
        (measure.width_of("["), measure.width_of("]"))
    } else {
        (0.0, 0.0)
    };

    let measure_tab = |i: usize| -> TabMeasure {
        let name_w = tab_name_widths[i];
        let icon_extra = tab_icon_extras[i];
        let has_close = bar.show_tab_close && bar.tabs[i].is_closable;
        let is_bracket = brackets && bar.tabs[i].is_active;
        let close_extra = if has_close {
            tab_inner_gap + close_glyph_w
        } else {
            0.0
        };
        let bracket_extra = if is_bracket {
            bracket_open_w + bracket_close_w
        } else {
            0.0
        };
        let total =
            tab_pad + bracket_extra + icon_extra + name_w + close_extra + tab_pad + tab_outer_gap;
        if has_close {
            // Tight close region — just the glyph and its leading
            // gap. Everything painted after it (the tab's own trailing
            // pad + outer gap, plus a closing bracket glyph when this is
            // the bracket-framed active tab) is `trailing_width`, not
            // part of the clickable close box.
            let close_w = tab_inner_gap + close_glyph_w;
            let trailing = tab_pad + tab_outer_gap + if is_bracket { bracket_close_w } else { 0.0 };
            TabMeasure::new(total, close_w).with_trailing(trailing)
        } else {
            TabMeasure::new(total, 0.0)
        }
    };
    let tab_measures: Vec<TabMeasure> = (0..bar.tabs.len()).map(&measure_tab).collect();

    let seg_widths: Vec<f32> = (0..bar.right_segments.len())
        .map(|i| measure.width_of(&bar.right_segments[i].text))
        .collect();

    let layout = bar.layout(
        width,
        height,
        0.0, // no scroll arrows — every pixel backend defers scroll to this fn
        |i| tab_measures[i],
        |i| SegmentMeasure::new(seg_widths[i]),
    );

    let reserved_px: f32 = seg_widths.iter().sum();
    let effective_tab_area = (width - reserved_px).max(0.0);

    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let corrected_scroll_offset = match active_idx {
        Some(active) => TabBar::fit_active_scroll_offset(
            active,
            bar.tabs.len(),
            effective_tab_area as usize,
            |i| tab_measures[i].total_width.ceil() as usize,
        ),
        None => bar.scroll_offset,
    };

    let sample_w = measure.width_of(pixel::TAB_CELL_WIDTH_SAMPLE);
    let char_w = (sample_w / pixel::TAB_CELL_WIDTH_SAMPLE.chars().count() as f32).max(1.0);
    let available_cols = (effective_tab_area / char_w).floor().max(0.0) as usize;

    (layout, corrected_scroll_offset, available_cols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::message_list::{MessageList, MessageRow};
    use crate::primitives::multi_section_view::SectionBody;
    use crate::types::Color;

    #[test]
    fn msv_body_measure_message_list_is_not_zero() {
        // MessageList bodies must measure real content height, not 0.0.
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

    /// A `ButtonRow` item's width is the label's own measured width
    /// plus [`pixel::FORM_BUTTON_ITEM_PADDING`] — not the label wrapped
    /// in `"[label]"` bracket glyphs (which the GUI painter never
    /// actually draws, so that formula over-measured).
    #[test]
    fn form_field_measure_button_row_item_width_has_no_bracket_glyphs() {
        use crate::primitives::form::ButtonRowItem;

        let field = field_with(
            "actions",
            "",
            FieldKind::ButtonRow {
                buttons: vec![ButtonRowItem {
                    id: WidgetId::new("find"),
                    label: "Find".into(), // 4 chars * 6.0 = 24.0
                    disabled: false,
                    icon: None,
                }],
            },
        );
        let m = form_field_measure(&field, 20.0, &FakeMeasure);
        assert_eq!(
            m.item_measures[0].width,
            4.0 * 6.0 + pixel::FORM_BUTTON_ITEM_PADDING,
            "width must be the plain label width plus the padding \
             constant, not a `[Find]` bracket-wrapped measurement"
        );
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
        // `SegmentedControl` (unlike `Toolbar`) has no empty-label special case:
        // it always computes the unconditional `6.0 + label_w + 12.0`.
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

        // Empty label → Toolbar's special case: 6px inset, no +label_w+12.
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

    /// A field with a non-empty validation message measures one extra
    /// approximate text line beyond `row_h`, so the GUI painter has
    /// real room to put the message below the label instead of
    /// overlapping it. A field with no validation (or an
    /// empty message) is unaffected.
    #[test]
    fn form_field_measure_reserves_extra_height_for_validation_message() {
        use crate::primitives::form::ValidationState;

        const ROW_H: f32 = 20.0;
        let mut field = field_with("name", "Name", FieldKind::Button);
        let base = form_field_measure(&field, ROW_H, &FakeMeasure);
        assert_eq!(base.height, ROW_H);

        field.validation = Some(ValidationState::Error("Required".to_string()));
        let with_msg = form_field_measure(&field, ROW_H, &FakeMeasure);
        assert!(
            with_msg.height > ROW_H,
            "a non-empty validation message must reserve extra height \
             beyond row_h ({ROW_H}), got {}",
            with_msg.height
        );

        field.validation = Some(ValidationState::Error(String::new()));
        let empty_msg = form_field_measure(&field, ROW_H, &FakeMeasure);
        assert_eq!(
            empty_msg.height, ROW_H,
            "an empty validation message must not reserve extra height"
        );
    }

    /// Every backend's form layout measures every `FieldKind` by calling
    /// this one function, so this test proves the invariant their painters
    /// rely on: `FormFieldMeasure::height` is always exactly one `row_h`,
    /// for *every* `FieldKind`, regardless of any field-specific content
    /// (multiple toggles, a long `TextArea` value, a large `visible_rows`).
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

    // ── List ─────────────────────────────────────────────────────────

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

    /// When `max_content_width` (in chars) exceeds the visible width
    /// (chars × `char_width`, in pixels), the bottom `line_height` row is
    /// reserved for the h-scrollbar and is not available to items — one
    /// fewer row fits than when no overflow is signalled. Every pixel
    /// backend shares this exact function, so proving it here proves it for
    /// all of them at once.
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

    fn bare_tree() -> TreeView {
        TreeView {
            id: WidgetId::new("t"),
            rows: vec![],
            selection_mode: crate::types::SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: crate::types::TreeStyle::default(),
            has_focus: false,
        }
    }

    #[test]
    fn tree_row_pitch_matches_tree_layout_non_header_pitch_without_override() {
        // Must stay identical to the `item_height` `tree_layout` computes
        // for non-header rows — that's the whole point of sharing this fn.
        let mut tree = bare_tree();
        tree.style.row_height = None;
        assert_eq!(tree_row_pitch(&tree, 16.0), (16.0_f64 * 1.4).round());
    }

    fn tree_with_expanded_row() -> TreeView {
        let mut tree = bare_tree();
        tree.rows = vec![crate::primitives::tree::TreeRow {
            path: vec![0],
            indent: 1,
            icon: None,
            text: crate::types::StyledText::plain("src/"),
            badge: None,
            is_expanded: Some(true),
            decoration: Decoration::Normal,
            edit: None,
        }];
        tree
    }

    #[test]
    fn tree_layout_measured_uses_real_chevron_glyph_width_not_the_estimate() {
        // `tree.style.chevron_expanded` defaults to "▾" (one char); FakeMeasure
        // reports 6.0px/char, so the measured chevron end-x must differ from
        // `tree_layout`'s `line_height * 0.65` estimate.
        let tree = tree_with_expanded_row();
        let area = QRect::new(0.0, 0.0, 100.0, 80.0);
        let estimated = tree_layout(&tree, area, 16.0);
        let measured = tree_layout_measured(&tree, area, 16.0, &FakeMeasure);

        // The Chevron hit-region's width *is* the row's chevron_end_x
        // (see `TreeView::layout`'s hit-region split).
        let est_x = estimated.hit_regions[0].0.width;
        let measured_x = measured.hit_regions[0].0.width;
        assert_ne!(
            est_x, measured_x,
            "measured chevron end-x must use the real glyph width, not the estimate"
        );
        // indent_px = (16.0 * 0.9).round() = 14.0; glyph_w = 6.0 ("▾" is one char).
        let indent_px = (16.0_f64 * 0.9).round();
        let expected = (2.0 + 1.0 * indent_px + 6.0 + 4.0) as f32;
        assert_eq!(measured_x, expected);
    }

    #[test]
    fn tree_row_pitch_honors_tree_style_row_height_override() {
        // A host-set `TreeStyle::row_height` must win over the
        // `line_height`-derived default, exactly like `tree_layout` does.
        let mut tree = bare_tree();
        tree.style.row_height = Some(42);
        assert_eq!(tree_row_pitch(&tree, 16.0), 42.0);
        // And it must NOT vary with `line_height` once set.
        assert_eq!(tree_row_pitch(&tree, 100.0), 42.0);
    }

    /// Fixed-width stand-in for a proportional font: every glyph measures
    /// `6.0` px, matching `FakeMeasure` above but named locally so this
    /// section reads standalone.
    struct FakeTabMeasure;
    impl TextMeasure for FakeTabMeasure {
        fn width_of(&self, text: &str) -> f32 {
            text.chars().count() as f32 * 6.0
        }
    }

    fn tab_bar_with(tabs: Vec<crate::primitives::tab_bar::TabItem>) -> TabBar {
        TabBar {
            id: WidgetId::new("tabs"),
            tabs,
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    fn closable_tab(label: &str, is_active: bool) -> crate::primitives::tab_bar::TabItem {
        crate::primitives::tab_bar::TabItem {
            label: label.to_string(),
            is_active,
            is_dirty: false,
            is_preview: false,
            is_closable: true,
        }
    }

    #[test]
    fn pixel_tab_bar_layout_populates_available_cols() {
        let bar = tab_bar_with(vec![closable_tab("main.rs", true)]);
        let name_widths = [bar.tabs[0].label.chars().count() as f32 * 6.0];
        let icon_extras = [0.0_f32];
        let (_, _, available_cols) = pixel_tab_bar_layout(
            &bar,
            400.0,
            22.0,
            pixel::TAB_PAD,
            pixel::TAB_INNER_GAP,
            pixel::TAB_OUTER_GAP,
            &name_widths,
            &icon_extras,
            &TabChrome::default(),
            &FakeTabMeasure,
        );
        assert!(
            available_cols > 0,
            "a 400px bar with a single short tab should report a non-zero \
             available_cols, got {available_cols}"
        );
    }

    /// A click in the dead space between the × glyph and the tab's right edge
    /// must resolve to `Tab`, not `TabClose`. This test verifies that the
    /// close region excludes trailing padding and gap.
    #[test]
    fn pixel_tab_bar_layout_close_region_excludes_trailing_padding() {
        use crate::primitives::tab_bar::TabBarHit;

        let bar = tab_bar_with(vec![closable_tab("main.rs", true)]);
        let name_widths = [bar.tabs[0].label.chars().count() as f32 * 6.0];
        let icon_extras = [0.0_f32];
        let (layout, _, _) = pixel_tab_bar_layout(
            &bar,
            400.0,
            22.0,
            pixel::TAB_PAD,
            pixel::TAB_INNER_GAP,
            pixel::TAB_OUTER_GAP,
            &name_widths,
            &icon_extras,
            &TabChrome::default(),
            &FakeTabMeasure,
        );

        let vt = &layout.visible_tabs[0];
        let close = vt
            .close_bounds
            .expect("closable tab must report close bounds");
        let tab_right_edge = vt.bounds.x + vt.bounds.width;

        // The old (buggy) close region ran all the way to the tab's right
        // edge. The fixed region must stop short of it, leaving the
        // trailing `tab_pad + tab_outer_gap` as dead space that belongs to
        // the tab body, not the close button.
        assert!(
            close.x + close.width < tab_right_edge,
            "close region [{}, {}) must stop before the tab's right edge {}",
            close.x,
            close.x + close.width,
            tab_right_edge,
        );

        // A click just inside that trailing dead space — 1px short of the
        // tab's right edge — must resolve to `Tab`, not `TabClose`.
        let probe_x = tab_right_edge - 1.0;
        assert!(
            probe_x >= close.x + close.width,
            "test fixture's probe point must actually fall in the trailing \
             gap, past the close region's own right edge"
        );
        match layout.hit_test(probe_x, vt.bounds.y + vt.bounds.height / 2.0) {
            TabBarHit::Tab(0) => {}
            other => panic!(
                "click at x={probe_x} (old padding, now outside the close glyph) \
                 expected Tab(0), got {other:?}"
            ),
        }

        // And a click at the close region's own centre must still resolve
        // to `TabClose` — the fix must not shrink the box past the glyph
        // itself.
        let (cx, cy) = (
            close.x + close.width / 2.0,
            vt.bounds.y + vt.bounds.height / 2.0,
        );
        assert_eq!(layout.hit_test(cx, cy), TabBarHit::TabClose(0));
    }

    #[test]
    fn pixel_tab_bar_layout_corrected_scroll_offset_keeps_active_tab_visible() {
        let tabs: Vec<_> = (0..10)
            .map(|i| closable_tab(&format!("t{i}"), i == 9))
            .collect();
        let mut bar = tab_bar_with(tabs);
        bar.scroll_offset = 0; // stale — active tab (9) is not visible at offset 0
        let name_widths = vec![6.0_f32; bar.tabs.len()]; // "t0".."t9" — 2 chars * 6px
        let icon_extras = vec![0.0_f32; bar.tabs.len()];
        let (_, corrected_scroll_offset, _) = pixel_tab_bar_layout(
            &bar,
            120.0,
            22.0,
            pixel::TAB_PAD,
            pixel::TAB_INNER_GAP,
            pixel::TAB_OUTER_GAP,
            &name_widths,
            &icon_extras,
            &TabChrome::default(),
            &FakeTabMeasure,
        );
        assert!(
            corrected_scroll_offset > 0,
            "active tab 9 isn't visible at the stale scroll_offset 0; the \
             corrected offset must be non-zero"
        );
    }

    #[test]
    fn pixel_tab_bar_layout_bracket_chrome_still_excludes_trailing_padding() {
        // Bracket framing must not regress the close-button fix: the close
        use crate::primitives::tab_bar::{TabBarHit, TabFrame};

        let bar = tab_bar_with(vec![closable_tab("main.rs", true)]);
        let name_widths = [bar.tabs[0].label.chars().count() as f32 * 6.0];
        let icon_extras = [0.0_f32];
        let chrome = TabChrome::new(TabFrame::Brackets);
        let (layout, _, _) = pixel_tab_bar_layout(
            &bar,
            400.0,
            22.0,
            pixel::TAB_PAD,
            pixel::TAB_INNER_GAP,
            pixel::TAB_OUTER_GAP,
            &name_widths,
            &icon_extras,
            &chrome,
            &FakeTabMeasure,
        );

        let vt = &layout.visible_tabs[0];
        let close = vt
            .close_bounds
            .expect("closable tab must report close bounds");
        let tab_right_edge = vt.bounds.x + vt.bounds.width;
        assert!(
            close.x + close.width < tab_right_edge,
            "bracket-framed close region must still stop before the tab's \
             right edge (now with room for the closing bracket too)"
        );
        match layout.hit_test(tab_right_edge - 1.0, vt.bounds.y + vt.bounds.height / 2.0) {
            TabBarHit::Tab(0) => {}
            other => {
                panic!("click in the trailing bracket+padding gap expected Tab(0), got {other:?}")
            }
        }
    }
}
