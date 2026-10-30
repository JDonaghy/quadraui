//! Accessibility mappings for content primitives.
//!
//! One [`A11yPrimitive`] impl per content primitive: editor, text input,
//! text display, terminal, diff view, chart, image, minimap and canvas.
//! These impls expose basic role, label and value/bounds semantics; a full
//! editable-text model (cursor/selection ranges exposed to AT, inline
//! completions, etc.) is a separate, later issue.
//!
//! Three primitives have no textual equivalent for their content and so
//! carry no synthesized label:
//!
//! * [`Chart`] maps to [`A11yRole::Chart`]. A chart's visual series data has
//!   no textual equivalent, so the accessible name is whatever the app
//!   supplies via its own label override; an unlabelled chart is announced
//!   as unlabelled.
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
//! [`Terminal`] exposes its role and the currently visible grid as a single
//! joined value; scrollback history is out of scope here.
//!
//! [`TextDisplay`] exposes its title (when set) as the accessible label and
//! each line as a plain [`A11yRole::Label`] child. Markdown structure —
//! headings as headings, links as links — is a known limitation: headings
//! and links are not part of `TextDisplay`'s own data model (lines carry
//! styled spans, not semantic markup), so surfacing them would require
//! plumbing changes in `types.rs`/`compose/markdown.rs` that are out of
//! scope for basic content semantics.

use super::tree::{non_empty, A11yChild, A11yPrimitive};
use crate::a11y::{A11yInfo, A11yRole};
use crate::primitives::canvas::Canvas;
use crate::primitives::chart::Chart;
use crate::primitives::diff_view::{DiffEditability, DiffView};
use crate::primitives::editor::Editor;
use crate::primitives::image::Image;
use crate::primitives::minimap::Minimap;
use crate::primitives::terminal::Terminal;
use crate::primitives::text_display::TextDisplay;
use crate::primitives::text_input::TextInput;

impl A11yPrimitive for Editor {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new().with_role(A11yRole::Editor);
        let value: String = self
            .lines
            .iter()
            .map(|line| line.raw_text.as_str())
            .collect();
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
        match self
            .title
            .as_ref()
            .map(super::tree::plain_text)
            .and_then(non_empty)
        {
            Some(label) => info.with_label(label),
            None => info,
        }
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| {
                let text: String = line.spans.iter().map(|span| span.text.as_str()).collect();
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
        let value = self
            .cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
            })
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

impl A11yPrimitive for Chart {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new().with_role(A11yRole::Chart)
    }
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
