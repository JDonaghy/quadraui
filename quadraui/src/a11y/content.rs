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
//! value is the joined line text regardless.
//!
//! [`Terminal`] exposes its role and the currently visible grid as a single
//! joined value, trimmed of each row's trailing blank padding (vt100 cells
//! default to a single space when never written); scrollback history is out
//! of scope here.
//!
//! [`TextDisplay`] exposes its title (when set) as the accessible label and
//! each line as a plain [`A11yRole::Label`] child, prefixed with its
//! timestamp when set. Children walk the full line buffer rather than just
//! the scrolled-into-view rows: the trait has no viewport to bound against,
//! and a screen-reader user driving a log by its accessible tree benefits
//! more from being able to read the whole history than from it tracking
//! exactly what is on screen (`max_lines`, when non-zero, already caps the
//! buffer itself). Markdown structure — headings as headings, links as
//! links — is a known limitation: headings and links are not part of
//! `TextDisplay`'s own data model (lines carry styled spans, not semantic
//! markup), so surfacing them would require plumbing changes in
//! `types.rs`/`compose/markdown.rs` that are out of scope for basic content
//! semantics.
//!
//! [`DiffView`] exposes a single read-only-aware [`A11yRole::Document`] pane
//! in [`crate::primitives::diff_view::DiffMode::Unified`] (there is only one
//! column to read, prefixed `@@ … @@` hunk headers and `+`/`-`/` ` row
//! markers, matching what every backend paints), and the existing
//! left/right pane pair in
//! [`crate::primitives::diff_view::DiffMode::SideBySide`].
//!
//! [`TextInput`] uses its placeholder as the accessible label when the app
//! has not supplied its own — this is a visual-caveat fallback, not a true
//! label: a placeholder disappears once the field has a value, so a screen
//! reader user who tabs into a filled-in field never hears it.

use super::tree::{non_empty, plain_text, A11yChild, A11yPrimitive};
use crate::a11y::{A11yInfo, A11yRole};
use crate::primitives::canvas::Canvas;
use crate::primitives::chart::Chart;
use crate::primitives::diff_view::{
    unified_hunk_header, unified_row_text, DiffEditability, DiffMode, DiffRowKind, DiffView,
};
use crate::primitives::editor::Editor;
use crate::primitives::image::Image;
use crate::primitives::minimap::Minimap;
use crate::primitives::terminal::Terminal;
use crate::primitives::text_display::TextDisplay;
use crate::primitives::text_input::TextInput;

impl A11yPrimitive for Editor {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::Editor);
        // `EditorLine::raw_text` is inconsistent about a trailing `\n`, so
        // strip one if present before rejoining — otherwise a buffer whose
        // lines already carry it reads with blank lines between every row,
        // and one whose lines don't carry it reads as a single run-together
        // string.
        let value = self
            .lines
            .iter()
            .map(|line| line.raw_text.trim_end_matches('\n'))
            .collect::<Vec<_>>()
            .join("\n");
        match non_empty(value) {
            Some(value) => info.with_value(value),
            None => info,
        }
    }
}

impl A11yPrimitive for TextInput {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::TextInput);
        let info = match self.placeholder.as_ref().and_then(|p| non_empty(p.clone())) {
            Some(placeholder) => info.with_label(placeholder),
            None => info,
        };
        match non_empty(self.lines.join("\n")) {
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
                        A11yInfo::new().with_role(A11yRole::Label).with_label(text),
                    )
                })
            })
            .collect()
    }
}

impl A11yPrimitive for Terminal {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::Terminal);
        // Never-written cells default to a single space (vt100 convention),
        // so an un-trimmed row reads as its full grid width of padding —
        // trim each row's trailing blank run before joining, the same
        // presentation-only-character stripping convention `TextDisplay`'s
        // and `TextInput`'s values already follow.
        let value = self
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
            })
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        match non_empty(value) {
            Some(value) => info.with_value(value),
            None => info,
        }
    }
}

impl A11yPrimitive for DiffView {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new().with_role(A11yRole::Document)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        match self.mode {
            DiffMode::Unified => {
                let right_read_only = !matches!(self.editability, DiffEditability::RightEditable);
                let value = unified_diff_text(self);
                let info = A11yInfo::new()
                    .with_role(A11yRole::Document)
                    .with_read_only(right_read_only);
                let info = match non_empty(value) {
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
                vec![
                    A11yChild::new(
                        "left",
                        A11yInfo::new()
                            .with_role(A11yRole::Document)
                            .with_label(left_label)
                            .with_read_only(true)
                            .with_value(self.left.clone()),
                    ),
                    A11yChild::new(
                        "right",
                        A11yInfo::new()
                            .with_role(A11yRole::Document)
                            .with_label(right_label)
                            .with_read_only(right_read_only)
                            .with_value(self.right.clone()),
                    ),
                ]
            }
        }
    }
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
