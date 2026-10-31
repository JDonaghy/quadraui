//! Accessibility mappings for content primitives.
//!
//! One [`A11yPrimitive`] impl per content primitive: editor, text input,
//! text display, terminal, diff view, chart, image, minimap and canvas.
//! These impls expose basic role, label and value/bounds semantics; a full
//! editable-text model (cursor/selection ranges exposed to AT, inline
//! completions, etc.) is a separate, later issue.
//!
//! None of these impls fall back to a kind-name label the way an unmapped
//! primitive does (see `tree.rs`'s module doc) — a [`Chart`], [`Canvas`],
//! [`Editor`] or [`Minimap`] with no app-supplied label is announced with
//! just its role, not a synthesized name. Only [`Minimap`] departs from that
//! (it carries a fixed `"Minimap"` label, documented below) because it is
//! always presentation-only; the others stay silent on purpose, so an app
//! that never supplies a label finds out from a blank announcement rather
//! than a misleadingly specific one.
//!
//! Three primitives have no textual equivalent for their content and so
//! carry no synthesized label by default:
//!
//! * [`Chart`] maps to [`A11yRole::Chart`]. A chart's visual series data has
//!   no textual equivalent, so the accessible name prefers, in order: the
//!   app's own label override, [`Chart::y_label`], [`Chart::x_label`], then
//!   — only when [`Chart::show_legend`] is set, since that is the only
//!   visual cue that series names are meant to be read — the series labels
//!   joined with `", "`. A chart with none of those is announced as
//!   unlabelled.
//! * [`Canvas`] maps to [`A11yRole::Canvas`] for the same reason — it has no
//!   label field of its own, only a list of draw operations.
//! * [`Image`] maps to [`A11yRole::Image`] and falls back to
//!   [`Image::fallback_text`] (the text actually rendered on TUI, and shown
//!   on GTK/macOS when decoding fails) when the app has not supplied its
//!   own label; an image with an empty fallback is announced as unlabelled.
//!
//! [`Minimap`] is [`A11yRole::Presentation`] and marked hidden: it is a
//! zoomed-out, read-only duplicate of the editor's own content, so exposing
//! it to assistive technology would double-announce the buffer.
//!
//! [`Editor`] has no label field of its own either (same shape as `Chart`/
//! `Canvas`) — only an app-supplied label override would give it a name; the
//! value is the joined text of [`Editor::lines`], which is the currently
//! visible viewport rather than the whole buffer ([`Editor::scroll_top`] and
//! [`Editor::total_lines`] say where in the buffer it sits). Two
//! [`crate::primitives::editor::EditorLine`] kinds are *visual* rows rather
//! than logical lines and so do not get a line break of their own: a
//! `is_wrap_continuation` row is appended to the row it wraps from, and a
//! `is_ghost_continuation` row is skipped entirely — its `raw_text` is empty
//! and its text lives in `ghost_suffix`, i.e. it is an unaccepted AI
//! suggestion rather than buffer content, and inline completions are part of
//! the later full-text model.
//!
//! [`Terminal`] exposes its role and the currently visible grid as a single
//! joined value, trimmed of each row's trailing blank padding (vt100 cells
//! default to a single space when never written) and with vt100's wide-glyph
//! spacer columns dropped (the cell after a fullwidth glyph exists only to
//! occupy the column it bleeds into, and normalises to a single space —
//! speaking it would put a space after every CJK/emoji glyph); scrollback
//! history is out of scope here.
//!
//! [`TextDisplay`] exposes its title (when set) as the accessible label and
//! each line as a plain [`A11yRole::Label`] child carrying its `one of N`
//! position, prefixed with its timestamp when set. Children walk the full
//! line buffer rather than just the scrolled-into-view rows: the trait has
//! no viewport to bound against, and a screen-reader user driving a log by
//! its accessible tree benefits more from being able to read the whole
//! history than from it tracking exactly what is on screen. That walk is
//! unbounded when
//! [`TextDisplay::max_lines`](crate::primitives::text_display::TextDisplay::max_lines)
//! is `0` (its "keep everything" value), so an app that streams an unbounded
//! log into one and rebuilds the tree per frame pays one child per buffer
//! line per frame; cap `max_lines` to bound it. A `TextDisplay` rendering
//! [`crate::compose::markdown::RenderedMarkdown`] has no heading/link
//! structure of its own, since lines carry styled spans, not semantic
//! markup — [`markdown_a11y_children`] builds that structure separately,
//! straight from the `RenderedMarkdown` an app already has on hand from
//! [`crate::compose::markdown::render_markdown_to_styled`], exposing each
//! heading line as [`A11yRole::Heading`] (with its level from
//! `line_scales`) and each link span as [`A11yRole::Link`] (with its target
//! in [`A11yInfo::value`]).
//!
//! [`DiffView`] exposes a single always-read-only [`A11yRole::Document`] pane
//! in [`crate::primitives::diff_view::DiffMode::Unified`] (there is only one
//! column to read, prefixed `@@ … @@` hunk headers and `+`/`-`/` ` row
//! markers, matching what every backend paints — and that interleaved column
//! is not the right pane
//! [`DiffEditability::RightEditable`] makes editable, so unified mode never
//! advertises itself as writable), and the read-only-aware left/right pane
//! pair in [`crate::primitives::diff_view::DiffMode::SideBySide`].
//!
//! [`TextInput`] follows the same placeholder convention as the form-field
//! text entries in `chrome.rs`: the placeholder is a *value* fallback, used
//! only while the field is empty, never a label. A placeholder disappears
//! once the field has a value, so announcing it as the field's name would
//! name the field after something no longer on screen — and the same string
//! would then land in a different field depending on whether the text box
//! arrived as a standalone [`crate::Surface::TextInput`] or as a
//! `FieldKind::TextInput` inside a form.

use super::tree::{in_set, non_empty, plain_text, A11yChild, A11yPrimitive};
use crate::a11y::{A11yInfo, A11yRole};
use crate::compose::markdown::RenderedMarkdown;
use crate::primitives::canvas::Canvas;
use crate::primitives::chart::Chart;
use crate::primitives::diff_view::{
    unified_hunk_header, unified_row_text, DiffEditability, DiffMode, DiffRowKind, DiffView,
};
use crate::primitives::editor::Editor;
use crate::primitives::image::Image;
use crate::primitives::minimap::Minimap;
use crate::primitives::terminal::{Terminal, TerminalCell};
use crate::primitives::text_display::TextDisplay;
use crate::primitives::text_input::TextInput;

impl A11yPrimitive for Editor {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::Editor);
        match non_empty(editor_text(self)) {
            Some(value) => info.with_value(value),
            None => info,
        }
    }
}

/// The visible buffer text of an [`Editor`], one logical line per output
/// line.
///
/// `EditorLine::raw_text` is inconsistent about a trailing `\n`, so one is
/// stripped if present before rejoining — otherwise a buffer whose lines
/// already carry it reads with blank lines between every row, and one whose
/// lines don't carry it reads as a single run-together string.
///
/// Only rows that are logical lines start a new output line. A
/// `is_wrap_continuation` row is the 2nd+ *visual* row of one long buffer
/// line, so it is appended to the row it wraps from; a
/// `is_ghost_continuation` row is a virtual AI-completion row whose
/// `raw_text` is empty, so it is dropped rather than contributing a blank
/// line (its `ghost_suffix` text is unaccepted suggestion, not buffer
/// content).
fn editor_text(editor: &Editor) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in &editor.lines {
        if line.is_ghost_continuation {
            continue;
        }
        let text = line.raw_text.trim_end_matches('\n');
        match lines.last_mut() {
            Some(last) if line.is_wrap_continuation => last.push_str(text),
            _ => lines.push(text.to_string()),
        }
    }
    lines.join("\n")
}

impl A11yPrimitive for TextInput {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::TextInput);
        // Placeholder as a value fallback, never a label — the same
        // convention `chrome.rs`'s `text_entry` applies to the form-field
        // text entries, so one string never announces in two different
        // fields depending on where the text box was declared.
        let value = non_empty(self.lines.join("\n")).or_else(|| {
            self.placeholder
                .as_ref()
                .and_then(|placeholder| non_empty(placeholder.clone()))
        });
        match value {
            Some(value) => info.with_value(value),
            None => info,
        }
    }
}

impl A11yPrimitive for TextDisplay {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::Document);
        match self.title.as_ref().map(plain_text).and_then(non_empty) {
            Some(label) => info.with_label(label),
            None => info,
        }
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        // `one of N` counts buffer lines, so a child's position matches the
        // line number a sighted user would count to, even when a blank line
        // in between contributes no child of its own.
        let total = self.lines.len();
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| {
                let spans: String = line.spans.iter().map(|span| span.text.as_str()).collect();
                let text = match &line.timestamp {
                    Some(timestamp) if !timestamp.is_empty() => format!("{timestamp} {spans}"),
                    _ => spans,
                };
                non_empty(text).map(|text| {
                    A11yChild::new(
                        format!("line.{idx}"),
                        in_set(
                            A11yInfo::new().with_role(A11yRole::Label).with_label(text),
                            idx,
                            total,
                        ),
                    )
                })
            })
            .collect()
    }
}

/// Builds the accessibility children for a rendered-markdown surface:
/// one [`A11yRole::Heading`] (or plain line, if it is body text) per line,
/// each carrying its own [`A11yRole::Link`] children for the link spans
/// that land on that line.
///
/// `TextDisplay` itself has no slot for this — it renders plain styled
/// lines, not [`RenderedMarkdown`]'s side-channels — so an app that renders
/// markdown through [`crate::compose::markdown::render_markdown_to_styled`]
/// and wants its heading/link structure exposed to assistive tech calls
/// this directly on the `RenderedMarkdown` it already has, rather than
/// going through `TextDisplay::a11y_children`.
pub fn markdown_a11y_children(rendered: &RenderedMarkdown) -> Vec<A11yChild> {
    let total = rendered.line_text.len();
    (0..total)
        .filter_map(|idx| {
            let text = rendered.line_text.get(idx)?;
            let scale = rendered.line_scales.get(idx).copied().unwrap_or(1.0);
            let links = markdown_link_children(rendered, idx);
            let has_text = non_empty(text.clone()).is_some();
            if !has_text && links.is_empty() {
                return None;
            }
            let info = match heading_level(scale) {
                Some(level) => A11yInfo::new()
                    .with_role(A11yRole::Heading)
                    .with_level(level)
                    .with_label(text.clone()),
                None => A11yInfo::new()
                    .with_role(A11yRole::Label)
                    .with_label(text.clone()),
            };
            Some(
                A11yChild::new(format!("line.{idx}"), in_set(info, idx, total))
                    .with_children(links),
            )
        })
        .collect()
}

/// The heading level [`RenderedMarkdown::line_scales`] encodes for one
/// line, or `None` for body text.
///
/// `line_scales` stores a font-scale multiplier, not a level number
/// (`2.0`/`1.5`/`1.2` for H1/H2/H3, `1.0` for body text) — compared with
/// `f32::EPSILON` tolerance rather than `==` since it is a float.
fn heading_level(scale: f32) -> Option<u32> {
    const LEVELS: [(f32, u32); 3] = [(2.0, 1), (1.5, 2), (1.2, 3)];
    LEVELS
        .iter()
        .find(|(level_scale, _)| (scale - level_scale).abs() < f32::EPSILON)
        .map(|(_, level)| *level)
}

/// The [`A11yRole::Link`] children for one line of [`RenderedMarkdown`],
/// built from its `links` side-channel.
///
/// Each entry is `(line_idx, byte_range, url)`, where `byte_range` indexes
/// into `line_text[line_idx]` — out-of-range or non-UTF-8-boundary ranges
/// (which should not occur for a `RenderedMarkdown` built by
/// [`crate::compose::markdown::render_markdown_to_styled`]) are skipped
/// rather than panicking.
fn markdown_link_children(rendered: &RenderedMarkdown, line_idx: usize) -> Vec<A11yChild> {
    let Some(line_text) = rendered.line_text.get(line_idx) else {
        return Vec::new();
    };
    rendered
        .links
        .iter()
        .filter(|(idx, _, _)| *idx == line_idx)
        .enumerate()
        .filter_map(|(child_idx, (_, range, url))| {
            let link_text = line_text.get(range.clone())?;
            let label = non_empty(link_text.to_string()).unwrap_or_default();
            let info = A11yInfo::new()
                .with_role(A11yRole::Link)
                .with_label(label)
                .with_value(url.clone());
            Some(A11yChild::new(format!("link.{child_idx}"), info))
        })
        .collect()
}

impl A11yPrimitive for Terminal {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::Terminal);
        let value = self
            .cells
            .iter()
            .map(|row| terminal_row_text(row))
            .collect::<Vec<_>>()
            .join("\n");
        match non_empty(value) {
            Some(value) => info.with_value(value),
            None => info,
        }
    }
}

/// The spoken text of one [`Terminal`] grid row, with every
/// presentation-only character dropped.
///
/// Two of vt100's column-accounting conventions put characters in the grid
/// that exist only to occupy a column, and neither belongs in an accessible
/// value:
///
/// * A blank / never-written cell stores a single space
///   ([`TerminalCell::text`]), so an untrimmed
///   row reads as its full grid width of padding — hence the trailing trim.
/// * The cell *following* a fullwidth glyph is that glyph's spacer column,
///   normalised to a single space by [`crate::terminal_engine`]. Speaking it
///   would put a space after every CJK/emoji glyph, so it is skipped: a cell
///   whose [`TerminalCell::cell_width`] is `2` already covers the column its
///   successor holds, by the same `unicode-width` table vt100 consults.
fn terminal_row_text(row: &[TerminalCell]) -> String {
    let mut text = String::new();
    let mut in_spacer = false;
    for cell in row {
        if in_spacer {
            in_spacer = false;
            continue;
        }
        in_spacer = cell.cell_width() == 2;
        text.push_str(&cell.text);
    }
    text.trim_end().to_string()
}

impl A11yPrimitive for DiffView {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new().with_role(A11yRole::Document)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        match self.mode {
            DiffMode::Unified => {
                // Always read-only: the single unified column interleaves
                // removed and added rows behind `@@`/`-`/`+`/` ` markers, so
                // it is not the right pane `RightEditable` makes editable,
                // and telling AT it can be typed into would be misleading.
                let info = A11yInfo::new()
                    .with_role(A11yRole::Document)
                    .with_read_only(true);
                let info = match pane_value(&unified_diff_text(self)) {
                    Some(value) => info.with_value(value),
                    None => info,
                };
                vec![A11yChild::new("unified", info)]
            }
            DiffMode::SideBySide => {
                let left_label = self
                    .left_label
                    .clone()
                    .unwrap_or_else(|| "Left".to_string());
                let right_label = self
                    .right_label
                    .clone()
                    .unwrap_or_else(|| "Right".to_string());
                let right_read_only = !matches!(self.editability, DiffEditability::RightEditable);
                let pane = |label: String, read_only: bool, text: &str| {
                    let info = A11yInfo::new()
                        .with_role(A11yRole::Document)
                        .with_label(label)
                        .with_read_only(read_only);
                    match pane_value(text) {
                        Some(value) => info.with_value(value),
                        None => info,
                    }
                };
                vec![
                    A11yChild::new("left", pane(left_label, true, &self.left)),
                    A11yChild::new("right", pane(right_label, right_read_only, &self.right)),
                ]
            }
        }
    }
}

/// One [`DiffView`] pane's value: its text with the trailing newline a
/// whole-file pane usually carries stripped, and `None` for an empty pane,
/// so a pane emits no `value` token at all rather than an empty one — the
/// same shape [`Editor`], [`TextInput`] and [`Terminal`] values take.
fn pane_value(text: &str) -> Option<String> {
    non_empty(text.trim_end_matches('\n').to_string())
}

/// The unified-mode text for a [`DiffView`]: each hunk's `@@ … @@` header
/// followed by its rows, `+`/`-`/` ` prefixed exactly as every backend's
/// `draw_unified` paints them (see `unified_row_style` for the prefix
/// table), one row per line.
fn unified_diff_text(view: &DiffView) -> String {
    let mut lines = Vec::new();
    for hunk in &view.hunks {
        lines.push(unified_hunk_header(hunk));
        for row in &hunk.rows {
            let prefix = match row.kind {
                DiffRowKind::Same => ' ',
                DiffRowKind::Removed | DiffRowKind::Changed => '-',
                DiffRowKind::Added => '+',
            };
            lines.push(format!("{prefix}{}", unified_row_text(row)));
        }
    }
    lines.join("\n")
}

impl A11yPrimitive for Chart {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::Chart);
        match chart_label(self) {
            Some(label) => info.with_label(label),
            None => info,
        }
    }
}

/// [`Chart`]'s accessible-name fallback chain: `y_label`, then `x_label`,
/// then — only when [`Chart::show_legend`] is set, since that's the only
/// visual signal that series names are meant to be read — every
/// [`crate::primitives::chart::Series::label`] joined with `", "`.
fn chart_label(chart: &Chart) -> Option<String> {
    if let Some(label) = chart.y_label.clone().and_then(non_empty) {
        return Some(label);
    }
    if let Some(label) = chart.x_label.clone().and_then(non_empty) {
        return Some(label);
    }
    if chart.show_legend {
        let labels: Vec<&str> = chart
            .series
            .iter()
            .map(|series| series.label.as_str())
            .filter(|label| !label.is_empty())
            .collect();
        if !labels.is_empty() {
            return Some(labels.join(", "));
        }
    }
    None
}

impl A11yPrimitive for Image {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::Image);
        match non_empty(self.fallback_text.clone()) {
            Some(label) => info.with_label(label),
            None => info,
        }
    }
}

impl A11yPrimitive for Minimap {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Presentation)
            .with_label("Minimap")
            .with_hidden(true)
    }
}

impl A11yPrimitive for Canvas {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new().with_role(A11yRole::Canvas)
    }
}
