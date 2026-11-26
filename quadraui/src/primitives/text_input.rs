//! [`TextInput`] — standalone multi-line text input primitive.
//!
//! Used for free-form text entry (commit messages, multi-line search,
//! note-taking). Stores text as a `Vec<String>` of lines, with cursor
//! position tracked as `(line, col)`. The primitive handles vertical
//! scroll auto-clamp (cursor stays in viewport) and emits hit regions
//! per visible line so consumers route clicks back to text positions.
//!
//! V1 is line-based (consumer pre-splits long input into lines). Word
//! wrap inside the primitive is a future extension; today consumers
//! split on `\n` or pre-wrap at their preferred width.
//!
//! ## Editing — [`EditOp`] / [`TextEditor`]
//!
//! [`EditOp`] is a closed set of edit intents (insert, delete, cursor
//! movement, selection, undo/redo); [`TextEditor::apply`] is the single
//! mutator that turns one into the next buffer/cursor/selection state,
//! so every consumer gets the same (correct, multibyte-safe) editing
//! behaviour. [`EditOp::from_key`] maps a plain keypress to the `EditOp`
//! it means; [`EditOp::from_key_binding`] maps the universal
//! [`crate::accelerator::KeyBinding::Undo`] /
//! [`crate::accelerator::KeyBinding::Redo`] /
//! [`crate::accelerator::KeyBinding::SelectAll`] accelerators the same
//! way.
//!
//! ## Why the editing state is a wrapper, not new `TextInput` fields
//!
//! [`TextEditor`] is a *wrapper* around `TextInput`, not a set of extra
//! fields on it, and that split is load-bearing rather than stylistic.
//!
//! `TextInput` is this crate's public paint-time snapshot type: all eight
//! of its fields are `pub`, and external consumers construct it with an
//! **exhaustive struct literal**. Rust gives no way to grow such a
//! struct without breaking those call sites:
//!
//! * a **private** field makes every external `TextInput { .. }` literal
//!   fail with `E0451`, and `..Default::default()` does *not* rescue it;
//! * a **public** field makes the same literal fail with `E0063`
//!   (`missing field`), unless every call site already used `..base`.
//!
//! Either way it is a breaking change under this repo's *Downstream
//! consumers* policy (`CLAUDE.md`) and `docs/PRIMITIVE_RULES.md` rule 8,
//! whose first preference is "a shape that isn't breaking at all". So
//! **both** pieces of live editing state — the selection anchor and the
//! undo history — live on `TextEditor`, and `TextInput`'s field list is
//! unchanged.
//!
//! `TextEditor` `Deref`s to the wrapped `TextInput`, so `.lines`,
//! `.cursor_line`, `.layout(..)` and `backend.draw_text_input(rect, &ed)`
//! all work directly on it. Every content-mutating op records the
//! pre-mutation snapshot into a bounded [`crate::undo::UndoStack`]; pure
//! cursor movement does not, so moving the cursor between two edits
//! doesn't split them into separate undo steps.
//!
//! Selection is therefore readable via [`TextEditor::selection_range`] /
//! [`TextEditor::selected_text`] rather than off a bare `&TextInput`. No
//! rasteriser paints a selection highlight yet; when one does, it takes
//! the range as an argument (a new function alongside the old one — the
//! non-breaking shape rule 8 asks for) rather than reading a new
//! `TextInput` field.

use serde::{Deserialize, Serialize};

use crate::backend::PointerShape;
use crate::behaviour::scroll::ensure_visible;
use crate::behaviour::text_edit;
use crate::event::{Point, Rect};
use crate::style::Style;
use crate::types::WidgetId;
use crate::undo::UndoStack;
use crate::Handled;

/// Multi-line text input. Text is stored as one entry per line — empty
/// lines are empty strings. Cursor is `(line, col)` in *char columns*
/// (not bytes); the rasterisers convert as needed.
///
/// `PartialEq`/`Eq` are derived: every field here is content/cursor/
/// viewport state with no runtime-only editing state attached — see this
/// module's *Why the editing state is a wrapper* section for why the
/// selection anchor and undo history live on [`TextEditor`] instead of
/// being fields here (short version: adding **any** field, `pub` or
/// private, breaks external exhaustive `TextInput { .. }` literals).
///
/// Every field below has a matching `with_*` builder on [`TextInput::new`],
/// and the type implements `Default`, so a consumer constructs with
/// `TextInput::new(id)` / `TextInput::default()` and chains `with_*`
/// instead of an exhaustive struct literal.
///
/// # Examples
///
/// ```
/// use quadraui::{Rect, TextInput, TextInputMeasure, WidgetId};
///
/// let input = TextInput::new(WidgetId::new("input:commit_message"))
///     .with_lines(vec!["fix: typo".to_string()])
///     .with_cursor_col(9)
///     .with_has_focus(true);
///
/// let rect = Rect::new(0.0, 0.0, 40.0, 3.0);
/// let layout = input.layout(rect, TextInputMeasure::new(1.0, 1.0));
///
/// assert_eq!(layout.visible_lines.len(), 1);
/// assert!(layout.cursor_bounds.is_some());
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextInput {
    pub id: WidgetId,
    /// One entry per logical line. Newlines are implicit between
    /// consecutive entries. Empty input is `vec![String::new()]`.
    pub lines: Vec<String>,
    /// Cursor line index. Clamped to `lines.len().saturating_sub(1)`
    /// by [`Self::layout`].
    #[serde(default)]
    pub cursor_line: usize,
    /// Cursor column within the cursor line, in *char columns*.
    /// Clamped to the line's char count by [`Self::layout`].
    #[serde(default)]
    pub cursor_col: usize,
    /// Optional placeholder shown when `lines` is empty or contains a
    /// single empty string. Rendered in muted color.
    #[serde(default)]
    pub placeholder: Option<String>,
    /// First visible line index. The primitive clamps this so the
    /// cursor stays inside the viewport.
    #[serde(default)]
    pub scroll_offset: usize,
    /// Horizontal scroll, in char columns. Applied to *every* line.
    /// The primitive clamps this so the cursor stays in view.
    #[serde(default)]
    pub scroll_col: usize,
    /// Whether the input has keyboard focus. Controls cursor visibility,
    /// border color, and — on a backend whose rasteriser paints one —
    /// the focus-visible ring; each backend decides its own treatment.
    ///
    /// Carries no "how did focus arrive" information, same convention
    /// as [`crate::Button::focused`]: a host that wants VS Code's
    /// `:focus-visible` suppression after a mouse click composes this
    /// as `focus.is_focused(id, None) && focus.is_focus_visible()` via
    /// [`crate::FocusTree`] rather than setting it unconditionally on
    /// focus.
    #[serde(default)]
    pub has_focus: bool,
}

impl TextInput {
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            placeholder: None,
            scroll_offset: 0,
            scroll_col: 0,
            has_focus: false,
        }
    }

    /// Replace [`Self::lines`] — one entry per logical line.
    #[must_use]
    pub fn with_lines(mut self, lines: Vec<String>) -> Self {
        self.lines = lines;
        self
    }

    /// Set [`Self::cursor_line`].
    #[must_use]
    pub fn with_cursor_line(mut self, cursor_line: usize) -> Self {
        self.cursor_line = cursor_line;
        self
    }

    /// Set [`Self::cursor_col`].
    #[must_use]
    pub fn with_cursor_col(mut self, cursor_col: usize) -> Self {
        self.cursor_col = cursor_col;
        self
    }

    /// Set [`Self::placeholder`].
    #[must_use]
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Set [`Self::scroll_offset`].
    #[must_use]
    pub fn with_scroll_offset(mut self, scroll_offset: usize) -> Self {
        self.scroll_offset = scroll_offset;
        self
    }

    /// Set [`Self::scroll_col`].
    #[must_use]
    pub fn with_scroll_col(mut self, scroll_col: usize) -> Self {
        self.scroll_col = scroll_col;
        self
    }

    /// Set [`Self::has_focus`].
    #[must_use]
    pub fn with_has_focus(mut self, has_focus: bool) -> Self {
        self.has_focus = has_focus;
        self
    }
}

/// Same shape as [`TextInput::new`] with an empty `WidgetId` — the
/// `new(required…)`/`with_*`/`Default` trio that lets a
/// consumer build a `TextInput` without an exhaustive struct literal.
impl Default for TextInput {
    fn default() -> Self {
        Self::new(WidgetId::new(String::new()))
    }
}

/// Re-exported from [`crate::behaviour::text_edit`], where the editing
/// core every editable text widget shares lives — see that module's doc.
pub use crate::behaviour::text_edit::EditOp;

/// Snapshot of the editable state — everything [`EditOp::Undo`]/
/// [`EditOp::Redo`] restore. Deliberately excludes `id`/`placeholder`/
/// `scroll_offset`/`scroll_col`/`has_focus`: those aren't buffer content,
/// so undoing an edit shouldn't also snap the viewport back.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TextInputSnapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    selection_anchor: Option<(usize, usize)>,
}

/// Bounded so a long-lived editor (e.g. a persistent chat input, or a
/// large commit-message buffer) doesn't accumulate unbounded history —
/// each undo step clones the full `lines: Vec<String>`. 200 steps is
/// generous for interactive editing while keeping worst-case memory
/// proportional to a small constant times the buffer size, not to how
/// long the widget has been alive.
const UNDO_HISTORY_LIMIT: usize = 200;

/// An editing session over a [`TextInput`] — handles insert / delete /
/// cursor-movement / selection / undo behaviour.
///
/// `TextEditor` owns the two pieces of live editing state that
/// deliberately are **not** `TextInput` fields: the selection anchor, and
/// a bounded [`crate::undo::UndoStack`] of buffer snapshots. See this
/// module's *Why the editing state is a wrapper* section for why — the
/// short version is that adding **any** field to `TextInput`, `pub` or
/// private, breaks external exhaustive `TextInput { .. }` struct literals
/// (`E0063` / `E0451`).
///
/// Access the wrapped `TextInput` through the public [`Self::input`]
/// field or through `Deref`/`DerefMut`, so `.lines`, `.cursor_line`,
/// `.layout(..)` and `backend.draw_text_input(rect, &editor)` all work
/// directly on a `TextEditor`.
/// ```
/// use quadraui::{EditOp, TextEditor, TextInput, WidgetId};
///
/// let mut ed = TextEditor::new(TextInput::new(WidgetId::new("demo")));
/// ed.apply(EditOp::InsertText("hi".into()));
/// ed.apply(EditOp::MoveLeft { extend: true });
/// assert_eq!(ed.selected_text().as_deref(), Some("i"));
/// ed.apply(EditOp::Undo);
/// assert_eq!(ed.lines, vec![String::new()]);
/// ```
#[derive(Debug, Clone)]
pub struct TextEditor {
    /// The wrapped paint-time state — hand this to a rasteriser.
    pub input: TextInput,
    selection_anchor: Option<(usize, usize)>,
    undo_stack: UndoStack<TextInputSnapshot>,
}

impl TextEditor {
    /// Start an editing session over `input`, with an empty undo history
    /// and no active selection.
    pub fn new(input: TextInput) -> Self {
        Self {
            input,
            selection_anchor: None,
            undo_stack: UndoStack::with_limit(UNDO_HISTORY_LIMIT),
        }
    }

    /// The selection anchor `(line, col)`, in the same char-column space
    /// as [`TextInput::cursor_line`]/[`TextInput::cursor_col`]. The
    /// cursor is the *moving* end of a selection; this is the *fixed*
    /// end. `None` means no active selection. A selection exists only
    /// when this is `Some` **and** differs from the cursor position —
    /// see [`Self::selection_range`].
    pub fn selection_anchor(&self) -> Option<(usize, usize)> {
        self.selection_anchor
    }

    /// Set (or clear) the selection anchor directly. Most callers should
    /// go through [`EditOp`]'s `extend` flag instead; this exists for
    /// backends driving a selection from a native gesture that doesn't
    /// decompose into `EditOp`s.
    pub fn set_selection_anchor(&mut self, anchor: Option<(usize, usize)>) {
        self.selection_anchor = anchor;
    }

    /// The selection as an ordered `(start, end)` pair of `(line, col)`
    /// positions, or `None` if no selection is active (the anchor is
    /// `None`, or equals the current cursor position).
    pub fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        text_edit::selection_range(
            (self.input.cursor_line, self.input.cursor_col),
            self.selection_anchor,
        )
    }

    /// The text currently selected, or `None` if no selection is active.
    /// Multi-line selections join lines with `'\n'`, mirroring how
    /// [`EditOp::InsertText`] would re-insert the same text verbatim.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        Some(text_edit::text_in_range(&self.input.lines, start, end))
    }

    fn snapshot(&self) -> TextInputSnapshot {
        TextInputSnapshot {
            lines: self.input.lines.clone(),
            cursor_line: self.input.cursor_line,
            cursor_col: self.input.cursor_col,
            selection_anchor: self.selection_anchor,
        }
    }

    fn restore(&mut self, snap: TextInputSnapshot) {
        self.input.lines = snap.lines;
        self.input.cursor_line = snap.cursor_line;
        self.input.cursor_col = snap.cursor_col;
        self.selection_anchor = snap.selection_anchor;
    }

    /// A borrowed [`text_edit::EditView`] over this editor's buffer,
    /// caret and selection anchor — the single mutator every [`EditOp`]
    /// runs through, shared with [`crate::TextField`] and
    /// [`crate::CommandLine`] (those two via
    /// [`text_edit::apply_flat_edit_op`]).
    fn view(&mut self) -> text_edit::EditView<'_> {
        text_edit::EditView {
            lines: &mut self.input.lines,
            cursor_line: &mut self.input.cursor_line,
            cursor_col: &mut self.input.cursor_col,
            anchor: &mut self.selection_anchor,
        }
    }

    /// Apply one [`EditOp`], mutating the wrapped [`TextInput`] in place.
    /// Returns whether anything actually changed (useful for deciding
    /// whether a redraw is needed).
    ///
    /// `Undo`/`Redo` are served from this editor's history. Every other
    /// op goes to [`text_edit::EditView::apply`], with a pre-mutation
    /// snapshot recorded first for the content-mutating ones
    /// ([`text_edit::mutates_content`]) so undo can restore them. Pure
    /// cursor/selection movement is not recorded, so moving the cursor
    /// between two edits doesn't split them into separate undo steps.
    pub fn apply(&mut self, op: EditOp) -> bool {
        match op {
            EditOp::Undo => {
                let current = self.snapshot();
                match self.undo_stack.undo(current) {
                    Some(prev) => {
                        self.restore(prev);
                        true
                    }
                    None => false,
                }
            }
            EditOp::Redo => {
                let current = self.snapshot();
                match self.undo_stack.redo(current) {
                    Some(next) => {
                        self.restore(next);
                        true
                    }
                    None => false,
                }
            }
            other => {
                if text_edit::mutates_content(&other) {
                    let snap = self.snapshot();
                    let changed = self.view().apply(&other);
                    if changed {
                        self.undo_stack.record(snap);
                    }
                    changed
                } else {
                    self.view().apply(&other)
                }
            }
        }
    }
}

impl std::ops::Deref for TextEditor {
    type Target = TextInput;
    fn deref(&self) -> &TextInput {
        &self.input
    }
}

impl std::ops::DerefMut for TextEditor {
    fn deref_mut(&mut self) -> &mut TextInput {
        &mut self.input
    }
}

/// Hit-test classification for clicks inside a `TextInput`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputHit {
    /// Click landed on a text line — the consumer maps `(line, col)`
    /// to a new cursor position.
    Line { line_idx: usize },
    /// Click landed in the content area but past the last line.
    EmptyArea,
}

/// One visible line resolved by layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTextInputLine {
    /// Index into [`TextInput::lines`].
    pub line_idx: usize,
    pub bounds: Rect,
}

/// Fully-resolved layout.
#[derive(Debug, Clone, PartialEq)]
pub struct TextInputLayout {
    /// Outer bounds (matches the `rect` argument to layout).
    pub bounds: Rect,
    /// Content bounds (inside any border/padding the rasteriser draws).
    pub content_bounds: Rect,
    pub visible_lines: Vec<VisibleTextInputLine>,
    /// Cursor bounds in viewport pixels/cells when the cursor is inside
    /// the visible window, otherwise `None`. Rasterisers paint a cursor
    /// glyph (TUI) or vertical bar (GTK) at this rect.
    pub cursor_bounds: Option<Rect>,
    /// Vertical scroll offset after auto-clamp. The primitive
    /// guarantees the cursor is visible by adjusting this.
    pub resolved_scroll_offset: usize,
    /// Horizontal scroll offset (char columns) after auto-clamp.
    /// Rasterisers slice each line `[resolved_scroll_col..]` before
    /// painting.
    pub resolved_scroll_col: usize,
    /// Per-region hit map for click routing.
    pub hit_regions: Vec<(Rect, TextInputHit)>,
    /// True when the placeholder was rendered (lines empty / single
    /// empty line). Rasterisers consult this to draw the placeholder
    /// in a muted color.
    pub placeholder_active: bool,
}

impl TextInputLayout {
    /// Hit-test a click at surface-native coordinates `(x, y)` against
    /// this layout's `hit_regions`.
    ///
    /// Coordinate frame: **ABSOLUTE** — matches [`TextInputLayout`]'s own
    /// convention (`bounds`/`content_bounds`/`hit_regions` all carry
    /// `rect.x`/`rect.y`, per `PRIMITIVE_RULES.md`'s coordinate-frame
    /// table). Returns [`TextInputHit::EmptyArea`] when `(x, y)` falls
    /// outside every region.
    pub fn hit_test(&self, x: f32, y: f32) -> TextInputHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        TextInputHit::EmptyArea
    }

    /// Pointer glyph a backend should request while the mouse sits at
    /// surface-native coordinates `(x, y)` — an I-beam over editable text
    /// and over the blank band inside [`Self::content_bounds`] below the
    /// last visible line (still part of the editable box, same as every
    /// native multi-line text editor), the default arrow outside
    /// `content_bounds` entirely.
    ///
    /// Coordinate frame: **ABSOLUTE**, same as [`Self::hit_test`].
    pub fn pointer_shape(&self, x: f32, y: f32) -> PointerShape {
        match self.hit_test(x, y) {
            TextInputHit::Line { .. } => PointerShape::Text,
            TextInputHit::EmptyArea if self.content_bounds.contains(Point::new(x, y)) => {
                PointerShape::Text
            }
            TextInputHit::EmptyArea => PointerShape::Default,
        }
    }
}

/// Per-line measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextInputMeasure {
    /// Height of one row in surface-native units (TUI: 1.0 cells;
    /// GTK: line_height pixels).
    pub row_height: f32,
    /// Width of one character column in surface-native units. TUI: 1.0
    /// (cells). GTK: the monospace `char_width` from the backend — used to
    /// position the cursor and route clicks back to columns.
    pub char_width: f32,
}

impl TextInputMeasure {
    pub fn new(row_height: f32, char_width: f32) -> Self {
        Self {
            row_height,
            char_width,
        }
    }

    /// Build from the backend's own [`crate::backend::Metrics`]
    /// (`backend.measure()`) instead of hand-threading
    /// `backend.line_height()` / `backend.char_width()` through
    /// [`Self::new`] — the two are identical, this just names the
    /// intent.
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.line_height, m.char_width)
    }
}

/// Fully-resolved, backend-neutral box height for a **single-line**
/// `TextInput` (one line, no wrap — the shape every standalone text
/// field and every `Form` `TextInput`/`PasswordInput` row uses): the
/// same "standard single-line control" metric
/// [`crate::primitives::button::ButtonMetrics`] resolves a `Button`'s
/// own height from, read straight from [`Style::control_height`]
/// instead of derived from [`TextInputMeasure::row_height`] (a backend's
/// line-height measurement, which under-sizes a single-line box relative
/// to every other control sized against `control_height`). A consumer
/// sizing a standalone single-line input's rect reads [`Self::resolve`]
/// instead of `backend.line_height()`.
///
/// **TUI story: no-op**, same as `ButtonMetrics` — a terminal row is
/// always exactly one cell tall and has no sub-cell height to refine, so
/// `crate::tui::text_input` has no use for this struct.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TextInputMetrics {
    /// Resolved single-line input box height, in surface-native pixels
    /// — read straight from [`Style::control_height`].
    pub height: f32,
}

impl TextInputMetrics {
    /// Resolve a single-line `TextInput`'s box height from `style`.
    pub fn resolve(style: &Style) -> Self {
        Self {
            height: style.control_height,
        }
    }
}

/// Border (1 cell/px) + horizontal padding (1 char column) inset shared by
/// [`TextInput::layout`] (which uses it to compute `content_bounds`) and
/// [`TextInput::content_cols`] (which uses it to compute a pre-wrap column
/// budget) — kept as one function so the two computations can't drift
/// apart.
fn content_x_inset(char_w: f32) -> f32 {
    let border = 1.0;
    let pad_x = char_w;
    border + pad_x
}

impl TextInput {
    /// Compute layout for `rect`. `measure` supplies row height + char
    /// width in the backend's native units.
    pub fn layout(&self, rect: Rect, measure: TextInputMeasure) -> TextInputLayout {
        let row_h = measure.row_height.max(1.0);
        let char_w = measure.char_width.max(1.0);

        // Border (1 cell / 1 px) + horizontal padding (1 char column).
        // Vertical padding is zero so each row neatly fills `row_h`.
        let border = 1.0;
        let inset = content_x_inset(char_w);
        let content_x = rect.x + inset;
        let content_y = rect.y + border;
        let content_w = (rect.width - inset * 2.0).max(0.0);
        let content_h = (rect.height - border * 2.0).max(0.0);
        let content_bounds = Rect::new(content_x, content_y, content_w, content_h);

        let total_lines = self.lines.len().max(1);
        let max_rows = ((content_h / row_h).floor() as usize).max(1);
        let visible_cols = ((content_w / char_w).floor() as usize).max(1);

        // Clamp cursor line/col to text bounds.
        let cursor_line = self.cursor_line.min(total_lines.saturating_sub(1));
        let cursor_col = {
            let line = self.lines.get(cursor_line).map_or("", String::as_str);
            self.cursor_col.min(line.chars().count())
        };

        // Vertical scroll auto-clamp.
        let max_scroll = total_lines.saturating_sub(max_rows);
        let scroll = ensure_visible(self.scroll_offset.min(max_scroll), max_rows, cursor_line)
            .min(max_scroll);

        // Horizontal scroll auto-clamp — keep cursor inside [scroll_col,
        // scroll_col + visible_cols). Reserve one column of slack so the
        // cursor itself stays visible (not flush against the right edge).
        let scroll_col = ensure_visible(self.scroll_col, visible_cols, cursor_col);

        let visible_count = total_lines.saturating_sub(scroll).min(max_rows);
        let mut visible_lines: Vec<VisibleTextInputLine> = Vec::with_capacity(visible_count);
        let mut hit_regions: Vec<(Rect, TextInputHit)> = Vec::with_capacity(visible_count + 1);

        let placeholder_active = self.placeholder.is_some()
            && (self.lines.is_empty() || (self.lines.len() == 1 && self.lines[0].is_empty()));

        for i in 0..visible_count {
            let line_idx = scroll + i;
            let row_y = content_y + i as f32 * row_h;
            let row_bounds = Rect::new(content_x, row_y, content_w, row_h);
            visible_lines.push(VisibleTextInputLine {
                line_idx,
                bounds: row_bounds,
            });
            hit_regions.push((row_bounds, TextInputHit::Line { line_idx }));
        }

        // Empty-area hit zone: any vertical space below the last
        // visible line.
        let used_h = visible_count as f32 * row_h;
        if used_h < content_h {
            hit_regions.push((
                Rect::new(content_x, content_y + used_h, content_w, content_h - used_h),
                TextInputHit::EmptyArea,
            ));
        }

        // Cursor bounds — only when in the visible window.
        let cursor_bounds = if cursor_line >= scroll && cursor_line < scroll + visible_count {
            let row_off = cursor_line - scroll;
            let col_off = cursor_col.saturating_sub(scroll_col);
            let cursor_x = content_x + col_off as f32 * char_w;
            let cursor_y = content_y + row_off as f32 * row_h;
            Some(Rect::new(cursor_x, cursor_y, char_w, row_h))
        } else {
            None
        };

        TextInputLayout {
            bounds: rect,
            content_bounds,
            visible_lines,
            cursor_bounds,
            resolved_scroll_offset: scroll,
            resolved_scroll_col: scroll_col,
            hit_regions,
            placeholder_active,
        }
    }

    /// Content width in *char columns* for an outer `TextInput` width of
    /// `width` at `char_width` — the same border+padding inset
    /// [`Self::layout`] uses internally to compute `content_bounds`, minus
    /// the row-height/height terms `layout` also needs (a column budget
    /// doesn't depend on `rect.height`).
    ///
    /// Exposed so callers that need to pre-wrap text to a fixed width
    /// *before* calling [`Self::layout`] — e.g.
    /// [`crate::compose::ChatController`]'s soft-wrapping, auto-growing
    /// chat input — compute the same budget `layout` will
    /// end up rendering into, without duplicating the border/padding
    /// constants (and risking the two drifting apart).
    pub fn content_cols(width: f32, char_width: f32) -> usize {
        let char_w = char_width.max(1.0);
        let inset = content_x_inset(char_w);
        let content_w = (width - inset * 2.0).max(0.0);
        ((content_w / char_w).floor() as usize).max(1)
    }
}

// ── Flat-value `EditOp` bridge ──────────────────────────────────────────

/// Re-exported from [`crate::behaviour::text_edit`]: the one bridge
/// every flat `value: String` + byte-offset cursor/anchor field
/// ([`crate::primitives::form::FieldKind::TextInput`],
/// [`crate::TextField`], [`crate::CommandLine`]) applies an [`EditOp`]
/// through, so none of them keeps a second editor.
pub(crate) use crate::behaviour::text_edit::apply_flat_edit_op;

// ── PaintSurface paint ─────────────────────────────────────────────────
//
// This module provides the portable paint implementation `MacBackend`,
// `GtkBackend` and `WinBackend` all route `draw_text_input` through — GTK
// and Windows via a thin per-backend wrapper, macOS directly. This module
// avoids a bespoke per-backend copy by building once against the portable
// `PaintSurface` verbs.
//
// The shape is the simplest a `TextInput` rasteriser can take: no
// selection-range painting, since `TextInput` itself carries none; see
// this module's own doc, "Selection is therefore readable via
// `TextEditor::selection_range`..."): fill the
// background, stroke a focus-aware border, paint the placeholder or each
// visible (post-h-scroll) line, then — only when focused — a 2-unit-wide
// cursor bar at `TextInputLayout::cursor_bounds`, the exact rect
// `text_input_layout` already hands a host for click routing and IME
// placement. No geometry is re-derived here — the whole point of taking
// an already-resolved `&TextInputLayout` rather than laying out `ti`
// itself.
//
// When `ti.has_focus`, this painter also strokes a focus-visible ring:
// the outset rect/width `crate::primitives::button::native_surface_paint::
// focus_ring_geometry` resolves from `style.focus_ring_width` — the same
// helper `Button`'s shared painter calls — so the ring matches `Button`'s
// outset gap and stroke width. `TextInput`'s own border always strokes
// square (`surface_stroke_rect`, no radius; see this module's own border
// stroke below), so the ring strokes square too via `surface_stroke_rect`
// rather than `surface_stroke_rounded_rect`, ignoring the geometry's
// `corner_radius` — matching a rounded ring to a square border would
// trace a mismatched outline.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
mod native_surface_paint {
    use super::{TextInput, TextInputLayout};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::primitives::button::native_surface_paint::focus_ring_geometry;
    use crate::style::Style;
    use crate::theme::Theme;

    /// Insert-cursor width in surface-native units. Matches
    /// `gtk::text_input`'s cursor bar.
    const CURSOR_W: f32 = 2.0;

    /// Slice `line` at char offset `off`, returning the tail (the part
    /// visible given horizontal scroll) — same idiom every existing
    /// `draw_text_input` twin uses independently.
    fn slice_from(line: &str, off: usize) -> String {
        if off == 0 {
            line.to_string()
        } else {
            line.chars().skip(off).collect()
        }
    }

    /// Paint `ti` using `layout` (must be the same layout the caller
    /// hit-tests against — typically `ti.layout(rect, measure)`'s own
    /// return value) via `surface`'s [`PaintSurface`] verbs. `style`
    /// supplies the focus-visible ring's width (`Style::focus_ring_width`),
    /// read fresh each call rather than hardcoded, same as `Button`'s
    /// shared painter.
    pub(crate) fn paint(
        ti: &TextInput,
        layout: &TextInputLayout,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    ) {
        let rect = layout.bounds;
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        surface.surface_fill_rect(rect, theme.background);
        let border_color = if ti.has_focus {
            theme.accent_fg
        } else {
            theme.border_fg
        };
        surface.surface_stroke_rect(rect, border_color, 1.0);

        // Focus-visible ring: an outset stroke in the theme's ring
        // colour, independent of the border tint above — see this
        // module's doc for why it strokes square rather than at
        // `ring.corner_radius`. `ti.has_focus` already carries VS
        // Code's focus-visible suppression (composed by the caller via
        // `crate::FocusTree::is_focus_visible` before it's set on
        // `TextInput`, same convention as `Button::focused`), so no
        // separate suppression check is needed here.
        if ti.has_focus {
            let ring = focus_ring_geometry(rect, style);
            surface.surface_stroke_rect(ring.rect, theme.focus_ring_color(), ring.width);
        }

        let h_scroll = layout.resolved_scroll_col;

        // `vis.bounds`/placeholder bounds are not truncated to
        // `visible_cols` (a horizontally-scrolled line's painted tail
        // can run arbitrarily long, and a proportional surface font can
        // make even an in-budget line wider than its monospace
        // estimate), so clip to `content_bounds` before drawing text —
        // same pattern as `rich_text_popup::native_surface_paint::paint`
        // and `palette::native_surface_paint`'s row text.
        surface.surface_push_clip(layout.content_bounds);

        if layout.placeholder_active {
            if let (Some(text), Some(first)) =
                (ti.placeholder.as_ref(), layout.visible_lines.first())
            {
                surface.surface_draw_text_run(first.bounds, text, theme.muted_fg);
            }
        } else {
            for vis in &layout.visible_lines {
                let full = ti.lines.get(vis.line_idx).map(String::as_str).unwrap_or("");
                let visible = slice_from(full, h_scroll);
                if visible.is_empty() {
                    continue;
                }
                surface.surface_draw_text_run(vis.bounds, &visible, theme.foreground);
            }
        }

        surface.surface_pop_clip();

        if ti.has_focus {
            if let Some(cb) = layout.cursor_bounds {
                let cursor_rect = Rect::new(cb.x, cb.y, CURSOR_W, cb.height);
                surface.surface_fill_rect(cursor_rect, theme.cursor);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::primitives::text_input::TextInputMeasure;
        use crate::types::{Color, WidgetId};
        use crate::{Image, Point, Viewport};

        /// Records every `PaintSurface` verb `paint` calls, without a
        /// real pixel backend — the same fake shape
        /// `find_replace::native_surface_paint::tests::RecordingSurface`
        /// and `form::native_surface_paint::tests::RecordingSurface`
        /// already use.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            text_runs: Vec<(Rect, String, Color)>,
            clips: Vec<Rect>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(0.0, 0.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                _radius: f32,
                color: Color,
                stroke_width: f32,
            ) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: Point,
                _to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clips.push(rect);
            }
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn sample(lines: Vec<&str>, cursor_line: usize, cursor_col: usize) -> TextInput {
            let mut ti = TextInput::new(WidgetId::new("ti"));
            ti.lines = lines.into_iter().map(String::from).collect();
            ti.cursor_line = cursor_line;
            ti.cursor_col = cursor_col;
            ti.has_focus = true;
            ti
        }

        /// A text input's value and cursor must both appear in the recorded
        /// paint.
        #[test]
        fn value_and_cursor_both_paint() {
            let ti = sample(vec!["hello"], 0, 3);
            let rect = Rect::new(0.0, 0.0, 200.0, 20.0);
            let layout = ti.layout(rect, TextInputMeasure::new(16.0, 8.0));
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();

            paint(&ti, &layout, &mut surface, &theme, &Style::default());

            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == "hello"),
                "expected the input's value to be drawn as a text run, text runs were {:?}",
                surface.text_runs,
            );
            let cb = layout.cursor_bounds.expect("cursor visible");
            let cursor_fill = surface
                .fills
                .iter()
                .find(|(r, c)| *c == theme.cursor && (r.x - cb.x).abs() < 0.01);
            assert!(
                cursor_fill.is_some(),
                "expected a cursor-colored fill at cursor_bounds.x, fills were {:?}",
                surface.fills,
            );
            // Pinned to `CURSOR_W` (2.0) — every backend that routes
            // through this shared painter (macOS, GTK) paints the same
            // caret width; a backend-local override drifting back to a
            // narrower bar would fail here.
            assert_eq!(
                cursor_fill.unwrap().0.width,
                CURSOR_W,
                "caret width must match the shared CURSOR_W constant",
            );
        }

        #[test]
        fn unfocused_input_paints_no_cursor() {
            let mut ti = sample(vec!["hello"], 0, 3);
            ti.has_focus = false;
            let rect = Rect::new(0.0, 0.0, 200.0, 20.0);
            let layout = ti.layout(rect, TextInputMeasure::new(16.0, 8.0));
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();

            paint(&ti, &layout, &mut surface, &theme, &Style::default());

            assert!(
                !surface.fills.iter().any(|(_, c)| *c == theme.cursor),
                "unfocused input must not paint a cursor, fills were {:?}",
                surface.fills,
            );
            assert!(surface.text_runs.iter().any(|(_, t, _)| t == "hello"));
        }

        #[test]
        fn placeholder_paints_in_muted_fg_when_empty() {
            let mut ti = TextInput::new(WidgetId::new("ti"));
            ti.placeholder = Some("type here".into());
            ti.has_focus = false;
            let rect = Rect::new(0.0, 0.0, 200.0, 20.0);
            let layout = ti.layout(rect, TextInputMeasure::new(16.0, 8.0));
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();

            paint(&ti, &layout, &mut surface, &theme, &Style::default());

            assert!(surface
                .text_runs
                .iter()
                .any(|(_, t, c)| t == "type here" && *c == theme.muted_fg));
        }

        /// `paint` clips text drawing to `layout.content_bounds` — so a
        /// horizontally-unscrolled long line (or a wide glyph under a
        /// proportional surface font) can't bleed past the widget's
        /// border on any backend that routes through this shared
        /// painter.
        #[test]
        fn text_drawing_is_clipped_to_content_bounds() {
            let ti = sample(vec!["hello"], 0, 3);
            let rect = Rect::new(0.0, 0.0, 200.0, 20.0);
            let layout = ti.layout(rect, TextInputMeasure::new(16.0, 8.0));
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();

            paint(&ti, &layout, &mut surface, &theme, &Style::default());

            assert!(
                surface.clips.contains(&layout.content_bounds),
                "expected a clip pushed at content_bounds, clips were {:?}",
                surface.clips,
            );
        }

        #[test]
        fn zero_size_rect_is_a_no_op() {
            let ti = sample(vec!["hello"], 0, 0);
            let rect = Rect::new(0.0, 0.0, 0.0, 20.0);
            let layout = ti.layout(rect, TextInputMeasure::new(16.0, 8.0));
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();

            paint(&ti, &layout, &mut surface, &theme, &Style::default());

            assert!(surface.fills.is_empty());
            assert!(surface.strokes.is_empty());
            assert!(surface.text_runs.is_empty());
        }

        /// Focused paints an outset stroke at exactly
        /// `focus_ring_geometry(rect, &style).rect`, with
        /// `style.focus_ring_width` — geometry the border stroke (always
        /// `rect` itself, at width `1.0`) can't satisfy, so this can't
        /// pass from the border stroke alone; unfocused paints no stroke
        /// at that outset rect at all.
        #[test]
        fn focus_ring_strokes_only_when_focused() {
            let theme = Theme::default();
            let style = Style::default();
            let rect = Rect::new(0.0, 0.0, 200.0, 20.0);
            let ring = focus_ring_geometry(rect, &style);
            assert_ne!(
                ring.rect, rect,
                "ring rect must be outset from the border rect for this test to discriminate them"
            );

            let focused = sample(vec!["hello"], 0, 3);
            let layout = focused.layout(rect, TextInputMeasure::new(16.0, 8.0));
            let mut surface = RecordingSurface::default();
            paint(&focused, &layout, &mut surface, &theme, &style);
            assert!(
                surface.strokes.iter().any(|(r, c, w)| {
                    *r == ring.rect
                        && *c == theme.focus_ring_color()
                        && *w == style.focus_ring_width
                }),
                "expected a ring stroke at the outset geometry when focused, strokes were {:?}",
                surface.strokes,
            );

            let mut unfocused = sample(vec!["hello"], 0, 3);
            unfocused.has_focus = false;
            let layout = unfocused.layout(rect, TextInputMeasure::new(16.0, 8.0));
            let mut surface = RecordingSurface::default();
            paint(&unfocused, &layout, &mut surface, &theme, &style);
            assert!(
                !surface.strokes.iter().any(|(r, _, _)| *r == ring.rect),
                "expected no stroke at the outset geometry when unfocused, strokes were {:?}",
                surface.strokes,
            );
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(unused_imports)]
pub(crate) use native_surface_paint::paint;

// ── Cell-grid layout + paint ───────────────────────────────────────────

/// Resolve this input's layout in **whole cell** units — the one
/// cell-grid entry point [`crate::primitive::Primitive::layout`] and
/// [`crate::tui::tui_text_input_layout`] share, so the widget contract
/// and the legacy `Backend::draw_text_input` path can never disagree
/// about where a row or the caret sits.
pub fn cell_layout(ti: &TextInput, rect: Rect) -> TextInputLayout {
    ti.layout(rect, TextInputMeasure::new(1.0, 1.0))
}

/// `TextInput`'s cell-grid rasteriser, written against
/// [`crate::CellSurface`]'s verbs — the cell-grid twin of
/// [`native_surface_paint::paint`].
///
/// Paints a 1-cell square border (tinted [`crate::Theme::accent_fg`]
/// while focused, [`crate::Theme::border_fg`] otherwise), the visible
/// lines sliced at [`TextInputLayout::resolved_scroll_col`], or the
/// placeholder in [`crate::Theme::muted_fg`] when
/// [`TextInputLayout::placeholder_active`], and — only while focused — a
/// block caret made by swapping the caret cell's own foreground and
/// background.
///
/// [`crate::CellSurface::draw_border`] is deliberately not used: it
/// paints nothing under 2×2 cells, while a one-row or one-column input
/// still paints a degenerate frame here, so the glyphs go down with
/// [`crate::CellSurface::put_char`] one at a time.
/// `#[cfg(feature = "tui")]`: `crate::tui`'s own delegate is this
/// painter's only caller, so a pixel-only or featureless build would
/// compile it unused — and CI's `-D warnings` legs turn that into a hard
/// error.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{TextInput, TextInputLayout};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::event::Rect;
    use crate::theme::Theme;

    /// Paint `ti` at the already-resolved `layout` onto `surface`.
    pub(crate) fn paint(
        ti: &TextInput,
        layout: &TextInputLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let rect = layout.bounds;
        if rect.width < 1.0 || rect.height < 1.0 {
            return;
        }
        let plain = CellStyle {
            fg: Some(theme.foreground),
            bg: Some(theme.background),
            ..CellStyle::default()
        };
        let muted = CellStyle {
            fg: Some(theme.muted_fg),
            ..plain
        };
        let border = CellStyle {
            fg: Some(if ti.has_focus {
                theme.accent_fg
            } else {
                theme.border_fg
            }),
            ..plain
        };

        surface.fill(rect, plain);

        let x0 = rect.x as u16;
        let y0 = rect.y as u16;
        let x1 = x0 + (rect.width as u16).saturating_sub(1);
        let y1 = y0 + (rect.height as u16).saturating_sub(1);
        for x in x0..=x1 {
            surface.put_char(x, y0, '─', border);
            if y1 != y0 {
                surface.put_char(x, y1, '─', border);
            }
        }
        for y in y0..=y1 {
            surface.put_char(x0, y, '│', border);
            if x1 != x0 {
                surface.put_char(x1, y, '│', border);
            }
        }
        surface.put_char(x0, y0, '┌', border);
        surface.put_char(x1, y0, '┐', border);
        surface.put_char(x0, y1, '└', border);
        surface.put_char(x1, y1, '┘', border);

        if layout.placeholder_active {
            if let (Some(text), Some(first)) =
                (ti.placeholder.as_ref(), layout.visible_lines.first())
            {
                put_row(surface, first.bounds, text, 0, muted);
            }
        } else {
            let h_scroll = layout.resolved_scroll_col;
            for vis in &layout.visible_lines {
                let line = ti.lines.get(vis.line_idx).map(String::as_str).unwrap_or("");
                put_row(surface, vis.bounds, line, h_scroll, plain);
            }
        }

        // Block caret: swap the caret cell's own colours, so the glyph
        // already painted there stays legible in reverse video.
        if ti.has_focus {
            if let Some(cb) = layout.cursor_bounds {
                let (cx, cy) = (cb.x as u16, cb.y as u16);
                let (ch, under) = surface.get(cx, cy).unwrap_or((' ', plain));
                surface.put_char(
                    cx,
                    cy,
                    ch,
                    CellStyle {
                        fg: under.bg.or(Some(theme.background)),
                        bg: under.fg.or(Some(theme.foreground)),
                        ..under
                    },
                );
            }
        }
    }

    /// Paint `text`, with its first `skip` characters scrolled out of
    /// view, into one row's `bounds`.
    fn put_row(
        surface: &mut dyn CellSurface,
        bounds: Rect,
        text: &str,
        skip: usize,
        style: CellStyle,
    ) {
        let visible: String = text.chars().skip(skip).collect();
        surface.put_str_clipped(
            bounds.x as u16,
            bounds.y as u16,
            &visible,
            bounds.width as u16,
            style,
        );
    }
}

// ── Widget contract ────────────────────────────────────────────────────

/// [`crate::primitive::Primitive::Layout`] for [`TextInput`]: the
/// absolute area the input was laid out into, the resolved
/// [`TextInputLayout`], and the char-column width the laying-out backend
/// measured with.
///
/// `char_width` is carried because [`crate::interactive::Interactive`]
/// gets no [`crate::primitive::LayoutCtx`] of its own, and resolving a
/// click's *column* inside a row needs the same advance the layout
/// positioned glyphs with.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct TextInputPrimitiveLayout {
    /// The absolute rect the input was laid out into.
    pub area: Rect,
    /// The resolved input layout, in absolute coordinates.
    pub layout: TextInputLayout,
    /// One character's advance, in the laying-out backend's units.
    pub char_width: f32,
}

impl TextInputPrimitiveLayout {
    /// Classify an absolute point against this input — see
    /// [`TextInputLayout::hit_test`].
    pub fn hit_test(&self, x: f32, y: f32) -> TextInputHit {
        self.layout.hit_test(x, y)
    }

    /// The `(line, char_column)` an absolute point resolves to, or `None`
    /// when it lands outside every text row.
    ///
    /// The column is rounded to the *nearest* boundary rather than
    /// floored, so a click on a glyph's right half puts the caret after
    /// it — what every native text editor does.
    pub fn caret_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let TextInputHit::Line { line_idx } = self.layout.hit_test(x, y) else {
            return None;
        };
        let char_w = self.char_width.max(1.0);
        let offset = ((x - self.layout.content_bounds.x) / char_w)
            .round()
            .max(0.0) as usize;
        Some((line_idx, self.layout.resolved_scroll_col + offset))
    }
}

impl crate::primitive::Primitive for TextInput {
    type Layout = TextInputPrimitiveLayout;
    type Event = TextInputHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::TextInput(self.clone())
    }

    /// The focus-visible ring strokes outside the input's own box, at the
    /// outset gap `focus_ring_geometry` uses, so the paint clip has to
    /// reach the far edge of that stroke.
    fn paint_bleed(&self, style: &crate::Style) -> f32 {
        crate::primitives::button::FOCUS_RING_GAP + style.focus_ring_width / 2.0
    }

    /// One row per buffer line plus the two border rows, and — as a
    /// width — the longest line plus the border/padding inset
    /// [`TextInput::layout`] reserves on both sides.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let char_w = ctx.char_width.max(1.0);
        let row_h = ctx.line_height.max(1.0);
        // One border cell/pixel on each edge, the same inset
        // `TextInput::layout` reserves — a terminal row and a pixel
        // surface both spend exactly one unit on it.
        let border = 1.0;
        let longest = self
            .lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0) as f32;
        let width = (longest * char_w + content_x_inset(char_w) * 2.0).min(avail.width);
        let height = self.lines.len().max(1) as f32 * row_h + border * 2.0;
        crate::NaturalSize::new(Some(width), Some(height))
    }

    /// `rect` is the input's own box. On a cell grid the layout comes
    /// from [`cell_layout`]; on a pixel surface from
    /// [`TextInput::layout`] with the backend's own line height and the
    /// advance of the font this input actually paints its glyphs in.
    ///
    /// That advance is measured through `ctx`'s own role-aware measurer
    /// rather than read off [`crate::primitive::LayoutCtx::char_width`]:
    /// `char_width` is the ambient *editor* advance, a different pitch
    /// from the chrome one whenever the two fonts differ, and the caret
    /// column has to sit on the same grid as the glyphs beside it.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> TextInputPrimitiveLayout {
        let rect = match ctx.units() {
            crate::primitive::Units::Cells => crate::primitive::cell_grid_rect(rect),
            _ => rect,
        };
        let (layout, char_width) = match ctx.units() {
            crate::primitive::Units::Cells => (cell_layout(self, rect), 1.0),
            _ => {
                let advance = ctx.text_width("0", self.font_role()).max(1.0);
                (
                    self.layout(rect, TextInputMeasure::new(ctx.line_height, advance)),
                    advance,
                )
            }
        };
        TextInputPrimitiveLayout {
            area: rect,
            layout,
            char_width,
        }
    }

    /// The shared pixel painter — the same one every pixel backend's
    /// `draw_text_input` routes through.
    fn paint(
        &self,
        layout: &TextInputPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            native_surface_paint::paint(self, &layout.layout, surface, theme, style);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_text_input`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`. That
    /// entry point re-resolves the layout from the same area through the
    /// same [`cell_layout`] this `Primitive` laid out with, so the glyphs
    /// land exactly where `layout` says they do.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &TextInputPrimitiveLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        // The painter never moves the cursor, so the sink is never read.
        let mut cursor_sink = None;
        let mut surface = crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            &mut cursor_sink,
        );
        cell_paint::paint(self, &layout.layout, &mut surface, theme);
    }
}

/// [`crate::interactive::Interactive::State`] for [`TextInput`]: the
/// caret and selection anchor this widget's own key handling moves.
///
/// # Why the caret lives here
///
/// A widget descriptor is rebuilt from the app's own fields every frame
/// (see [`crate::widget_state`]'s module doc), so `TextInput`'s
/// `cursor_line`/`cursor_col` fields are whatever the *app* last set.
/// The caret a run of keystrokes walks is interaction state, which is
/// exactly what this type is for: [`Self::caret`] is seeded from the
/// descriptor on [`crate::interactive::RoutedEvent::FocusIn`] and from
/// the hit-tested position on a click, then owned by this widget until
/// focus leaves. Content changes are reported to the app as
/// [`TextInputAction::Changed`]; the app owns the buffer.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextInputState {
    /// The caret this widget's own editing has moved to, or `None` while
    /// it still follows the descriptor's own
    /// `cursor_line`/`cursor_col`.
    pub caret: Option<(usize, usize)>,
    /// The selection's fixed end, if a selection is active —
    /// `TextInput` itself carries no field for this, by design (see that
    /// type's doc).
    pub anchor: Option<(usize, usize)>,
}

/// What a [`TextInput`]'s own [`crate::interactive::Interactive::handle`]
/// resolves one gesture to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputAction {
    /// The buffer changed. Carries the whole new text, lines joined with
    /// `'\n'` — the shape
    /// [`crate::primitives::form::FormEvent::TextInputChanged`] already
    /// names, so an app routes a standalone input through the handler it
    /// already has for a `Form` row.
    Changed(String),
    /// Ctrl-C (or the copy half of Ctrl-X) put the selection on the
    /// clipboard.
    Copied(String),
}

impl crate::interactive::WidgetAction for TextInputAction {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            TextInputAction::Changed(value) => crate::UiEvent::Form(
                id,
                crate::primitives::form::FormEvent::TextInputChanged { id, value },
            ),
            TextInputAction::Copied(text) => crate::UiEvent::TextCopied(text),
        }
    }
}

impl TextInput {
    /// The caret one `handle` call starts from: whatever this widget's
    /// own state has walked to, else the descriptor's own cursor —
    /// clamped into the buffer either way.
    fn effective_caret(&self, state: &TextInputState) -> (usize, usize) {
        let (line, col) = state.caret.unwrap_or((self.cursor_line, self.cursor_col));
        let line = line.min(self.lines.len().saturating_sub(1));
        let col = col.min(self.lines.get(line).map_or(0, |l| l.chars().count()));
        (line, col)
    }

    /// Apply one [`EditOp`] against the descriptor's buffer and the
    /// caret/anchor in `state`, writing the resulting caret back into
    /// `state`.
    ///
    /// Returns the new buffer text when content changed, `None` when the
    /// op only moved the caret or the selection.
    fn apply_op(&self, state: &mut TextInputState, op: &EditOp) -> Option<String> {
        let mut lines = if self.lines.is_empty() {
            vec![String::new()]
        } else {
            self.lines.clone()
        };
        let (mut line, mut col) = self.effective_caret(state);
        let mut anchor = state.anchor;
        let changed = text_edit::EditView {
            lines: &mut lines,
            cursor_line: &mut line,
            cursor_col: &mut col,
            anchor: &mut anchor,
        }
        .apply(op);
        state.caret = Some((line, col));
        state.anchor = anchor;
        if changed && lines != self.lines {
            Some(lines.join("\n"))
        } else {
            None
        }
    }

    /// The selected text, as [`text_edit::EditView::selected_text`]
    /// resolves it against this descriptor's buffer and `state`'s
    /// caret/anchor.
    fn selection_text(&self, state: &TextInputState) -> Option<String> {
        let (start, end) = text_edit::selection_range(self.effective_caret(state), state.anchor)?;
        Some(text_edit::text_in_range(&self.lines, start, end))
    }
}

impl crate::interactive::Interactive for TextInput {
    type State = TextInputState;
    type Action = TextInputAction;

    /// One focus stop as a whole: a multi-line input routes every
    /// keystroke itself rather than declaring a stop per line.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    /// One region per visible text row, keyed by its buffer line index,
    /// translated into the widget-local space this method answers in.
    fn sub_regions(
        &self,
        layout: &TextInputPrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let (ox, oy) = (layout.area.x, layout.area.y);
        layout
            .layout
            .visible_lines
            .iter()
            .map(|vis| {
                (
                    Rect::new(
                        vis.bounds.x - ox,
                        vis.bounds.y - oy,
                        vis.bounds.width,
                        vis.bounds.height,
                    ),
                    crate::interactive::SubTarget(vis.line_idx as u64),
                )
            })
            .collect()
    }

    /// A screen reader can focus the input and replace its whole value
    /// ([`crate::a11y::A11yAction::SetValue`] — see
    /// [`crate::a11y::A11yActionValue::Text`]).
    fn a11y_actions(&self, _state: &TextInputState) -> Vec<crate::a11y::A11yAction> {
        vec![
            crate::a11y::A11yAction::Focus,
            crate::a11y::A11yAction::SetValue,
        ]
    }

    /// Keystrokes go through [`EditOp::from_key`] and the shared
    /// [`text_edit::EditView`] mutator, so this widget and
    /// [`TextEditor`] can never disagree about what a key means.
    /// Ctrl-A selects all, Ctrl-C copies the selection, Ctrl-X cuts it
    /// (emitting [`crate::UiEvent::TextCopied`] alongside the change),
    /// and committed IME text inserts verbatim. A left click moves the
    /// caret to the clicked column and takes focus; a shift-click
    /// extends the selection to it.
    fn handle(
        &self,
        state: &mut TextInputState,
        event: &crate::interactive::RoutedEvent,
        layout: &TextInputPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TextInputAction> {
        use crate::interactive::{PointerKind, RoutedEvent};

        match event {
            RoutedEvent::Key(key) => self.handle_key(state, &key.key, key.modifiers, cx),
            RoutedEvent::Text(text) => self.commit(state, &EditOp::InsertText(text.clone()), cx),
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                mods,
                ..
            } => {
                let (x, y) = (layout.area.x + local.x, layout.area.y + local.y);
                match layout.caret_at(x, y) {
                    Some((line, col)) => {
                        cx.request_focus(None);
                        self.commit(
                            state,
                            &EditOp::SetCursor {
                                line,
                                col,
                                extend: mods.shift,
                            },
                            cx,
                        )
                    }
                    None => {
                        cx.request_focus(None);
                        Handled::Consumed
                    }
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Enter,
                local,
                ..
            } => {
                // Hinted on entry, not on every move: a plain move only
                // reaches a widget holding an active pointer capture, and
                // this one doesn't take one, so entry is the event this
                // widget actually gets.
                let (x, y) = (layout.area.x + local.x, layout.area.y + local.y);
                cx.set_cursor(layout.layout.pointer_shape(x, y));
                Handled::Ignored
            }
            RoutedEvent::Action(crate::a11y::A11yAction::Focus) => {
                cx.request_focus(None);
                Handled::Consumed
            }
            RoutedEvent::FocusIn(_) => {
                // Re-seed from the descriptor: the app may have moved the
                // caret while this input was unfocused.
                state.caret = Some((self.cursor_line, self.cursor_col));
                state.anchor = None;
                Handled::Consumed
            }
            RoutedEvent::FocusOut => {
                state.caret = None;
                state.anchor = None;
                Handled::Consumed
            }
            _ => Handled::Ignored,
        }
    }
}

impl TextInput {
    /// One keystroke, with the Ctrl-chorded clipboard/select-all keys
    /// resolved by
    /// [`EditIntent::from_key`](text_edit::EditIntent::from_key) ahead of
    /// [`EditOp::from_key`] (which would otherwise read `Ctrl+A` as
    /// typing an `a`).
    fn handle_key(
        &self,
        state: &mut TextInputState,
        key: &crate::event::Key,
        modifiers: crate::types::Modifiers,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TextInputAction> {
        use text_edit::EditIntent;

        match EditIntent::from_key(key, modifiers) {
            Some(EditIntent::SelectAll) => self.commit(state, &EditOp::SelectAll, cx),
            Some(EditIntent::Copy) => match self.selection_text(state) {
                Some(text) => Handled::Event(TextInputAction::Copied(text)),
                None => Handled::Consumed,
            },
            Some(EditIntent::Cut) => match self.selection_text(state) {
                Some(text) => {
                    cx.emit(crate::UiEvent::TextCopied(text));
                    self.commit(state, &EditOp::DeleteSelection, cx)
                }
                None => Handled::Consumed,
            },
            // Paste is the host's: the clipboard text arrives as
            // `UiEvent::ClipboardPaste`, which a host feeds back in as
            // `RoutedEvent::Text`.
            Some(EditIntent::Paste) => Handled::Ignored,
            Some(EditIntent::Op(op)) => self.commit(state, &op, cx),
            None => Handled::Ignored,
        }
    }

    /// Apply `op` and report it: a content change as
    /// [`TextInputAction::Changed`], a caret-only move as
    /// [`Handled::Consumed`] plus a redraw request.
    fn commit(
        &self,
        state: &mut TextInputState,
        op: &EditOp,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TextInputAction> {
        match self.apply_op(state, op) {
            Some(value) => Handled::Event(TextInputAction::Changed(value)),
            None => {
                cx.request_redraw();
                Handled::Consumed
            }
        }
    }
}

/// Routes a headless `TuiDriver` test can't reach: the
/// assistive-technology action path, a host-driven focus transition, the
/// widget-local `sub_regions` translation, and word motion.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::event::{Key, MouseButton, NamedKey, Point};
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::types::Modifiers;
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock — the same shape every other widget
    /// module's own `Interactive` tests use.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
        emitted: Vec<crate::UiEvent>,
        cursors: Vec<PointerShape>,
        redraws: usize,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, cursor: PointerShape) {
            self.cursors.push(cursor);
        }
        fn request_redraw(&mut self) {
            self.redraws += 1;
        }
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn sample(lines: &[&str]) -> TextInput {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = lines.iter().map(|l| l.to_string()).collect();
        ti.has_focus = true;
        ti
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 20.0, 5.0)
    }

    fn layout_of(ti: &TextInput) -> TextInputPrimitiveLayout {
        Primitive::layout(ti, &LayoutCtx::new(1.0, 1.0, false), area())
    }

    fn key(key: Key, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key,
            modifiers,
            repeat: false,
        })
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_text_input_is_one_focus_stop() {
        assert_eq!(Interactive::focus(&sample(&["a"])), FocusSpec::Stop);
    }

    #[test]
    fn sub_regions_name_each_visible_row_in_widget_local_space() {
        let ti = sample(&["alpha", "beta"]);
        let layout = layout_of(&ti);
        let regions = Interactive::sub_regions(&ti, &layout);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].1, SubTarget(0));
        assert_eq!(regions[1].1, SubTarget(1));
        // Local space: the first row sits one border row down and one
        // border + one pad column in, never at the absolute origin.
        assert_eq!((regions[0].0.x, regions[0].0.y), (2.0, 1.0));
    }

    #[test]
    fn an_a11y_focus_action_requests_focus_on_the_widget_itself() {
        let ti = sample(&["a"]);
        let layout = layout_of(&ti);
        let mut host = MockHost::default();
        let mut state = TextInputState::default();
        let mut cx = EventCx::new(ti.id, &mut host);
        let handled = Interactive::handle(
            &ti,
            &mut state,
            &RoutedEvent::Action(A11yAction::Focus),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn focus_in_seeds_the_caret_from_the_descriptor_and_focus_out_releases_it() {
        let mut ti = sample(&["alpha"]);
        ti.cursor_col = 3;
        let layout = layout_of(&ti);
        let mut host = MockHost::default();
        let mut state = TextInputState {
            caret: Some((0, 0)),
            anchor: Some((0, 1)),
        };
        let mut cx = EventCx::new(ti.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &ti,
                &mut state,
                &RoutedEvent::FocusIn(None),
                &layout,
                &mut cx
            ),
            Handled::Consumed
        );
        assert_eq!(state.caret, Some((0, 3)));
        assert_eq!(state.anchor, None);

        assert_eq!(
            Interactive::handle(&ti, &mut state, &RoutedEvent::FocusOut, &layout, &mut cx),
            Handled::Consumed
        );
        assert_eq!(state.caret, None);
    }

    #[test]
    fn ctrl_right_moves_the_caret_a_whole_word() {
        let ti = sample(&["foo_bar baz"]);
        let layout = layout_of(&ti);
        let mut host = MockHost::default();
        let mut state = TextInputState::default();
        let mut cx = EventCx::new(ti.id, &mut host);
        let handled = Interactive::handle(
            &ti,
            &mut state,
            &key(Key::Named(NamedKey::Right), ctrl()),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(state.caret, Some((0, 7)));
        assert_eq!(host.redraws, 1);
    }

    #[test]
    fn ctrl_backspace_deletes_the_word_before_the_caret() {
        let mut ti = sample(&["let foo_bar"]);
        ti.cursor_col = 11;
        let layout = layout_of(&ti);
        let mut host = MockHost::default();
        let mut state = TextInputState::default();
        let mut cx = EventCx::new(ti.id, &mut host);
        let handled = Interactive::handle(
            &ti,
            &mut state,
            &key(Key::Named(NamedKey::Backspace), ctrl()),
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(TextInputAction::Changed("let ".into()))
        );
    }

    #[test]
    fn committed_ime_text_inserts_verbatim() {
        let ti = sample(&[""]);
        let layout = layout_of(&ti);
        let mut host = MockHost::default();
        let mut state = TextInputState::default();
        let mut cx = EventCx::new(ti.id, &mut host);
        let handled = Interactive::handle(
            &ti,
            &mut state,
            &RoutedEvent::Text("日本".into()),
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(TextInputAction::Changed("日本".into()))
        );
    }

    #[test]
    fn a_shift_click_extends_the_selection_to_the_clicked_column() {
        let ti = sample(&["alpha"]);
        let layout = layout_of(&ti);
        let mut host = MockHost::default();
        let mut state = TextInputState::default();
        let mut cx = EventCx::new(ti.id, &mut host);
        // Park the caret at column 1 with a plain click first.
        assert_eq!(
            Interactive::handle(
                &ti,
                &mut state,
                &RoutedEvent::Pointer {
                    kind: PointerKind::Down(MouseButton::Left),
                    sub: None,
                    local: Point::new(3.0, 1.0),
                    mods: Modifiers::default(),
                },
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        assert_eq!(state.caret, Some((0, 1)));
        assert_eq!(state.anchor, None);

        assert_eq!(
            Interactive::handle(
                &ti,
                &mut state,
                &RoutedEvent::Pointer {
                    kind: PointerKind::Down(MouseButton::Left),
                    sub: None,
                    local: Point::new(6.0, 1.0),
                    mods: Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                },
                &layout,
                &mut cx,
            ),
            Handled::Consumed
        );
        assert_eq!(state.caret, Some((0, 4)));
        assert_eq!(state.anchor, Some((0, 1)));
    }

    #[test]
    fn a_pointer_entering_over_the_text_asks_for_an_i_beam() {
        let ti = sample(&["alpha"]);
        let layout = layout_of(&ti);
        let mut host = MockHost::default();
        let mut state = TextInputState::default();
        let mut cx = EventCx::new(ti.id, &mut host);
        // A hover only hints the shape: entry stays available to whatever
        // else the host routes it to.
        assert_eq!(
            Interactive::handle(
                &ti,
                &mut state,
                &RoutedEvent::Pointer {
                    kind: PointerKind::Enter,
                    sub: None,
                    local: Point::new(3.0, 1.0),
                    mods: Modifiers::default(),
                },
                &layout,
                &mut cx,
            ),
            Handled::Ignored
        );
        assert_eq!(host.cursors, vec![PointerShape::Text]);
    }

    #[test]
    fn a_pixel_layout_measures_its_caret_column_with_the_backends_char_width() {
        let ti = sample(&["alpha"]);
        let layout = Primitive::layout(
            &ti,
            &LayoutCtx::new(16.0, 8.0, false),
            Rect::new(0.0, 0.0, 200.0, 48.0),
        );
        assert_eq!(layout.char_width, 8.0);
        // Three char advances into the content column resolves to column 3.
        let x = layout.layout.content_bounds.x + 8.0 * 3.0;
        assert_eq!(
            layout.caret_at(x, layout.layout.content_bounds.y + 1.0),
            Some((0, 3))
        );
    }

    #[test]
    fn measure_reports_a_row_per_line_plus_the_two_border_rows() {
        let ti = sample(&["alpha", "beta", "gamma"]);
        let size = Primitive::measure(
            &ti,
            &LayoutCtx::new(1.0, 1.0, false),
            crate::primitive::Size::new(40.0, 40.0),
        );
        assert_eq!(size.height, Some(5.0));
        assert_eq!(size.width, Some(9.0));
    }
}

/// Cell-grid paint tests for [`cell_paint`], driven through
/// [`crate::tui::draw_text_input`] — the delegate that owns the
/// `Buffer` → [`crate::CellSurface`] adapter the painter needs.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::cell_test_paint::draw_text_input;
    use crate::event::Point;
    use crate::primitives::text_input::{TextInput, TextInputHit};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};

    /// The ratatui colour `crate::tui`'s own cell writers land for a
    /// quadraui [`Color`] — spelled here because that crate-private
    /// conversion isn't reachable from a widget module.
    fn qc(c: Color) -> ratatui::style::Color {
        ratatui::style::Color::Rgb(c.r, c.g, c.b)
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_input(lines: Vec<&str>) -> TextInput {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = lines.into_iter().map(String::from).collect();
        ti.has_focus = true;
        ti
    }

    // Parametrized over the area's origin: layout helpers must return
    // coords in the same frame across backends. `tui_text_input_layout`
    // bakes `rect.x`/`rect.y` straight into `ti.layout`'s returned bounds
    // (absolute frame: `TextInput::layout` computes
    // `content_x = rect.x + border + pad_x`), so this test verifies a full
    // paint+hit_test round trip at a non-zero origin.
    fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let rect = Rect::new(origin_x, origin_y, 10, 4);
        let mut buf = Buffer::empty(rect);
        let ti = mk_input(vec!["hello"]);
        let layout = draw_text_input(&mut buf, rect, &ti, &Theme::default());

        // Border top-left corner paints at the absolute origin.
        assert_eq!(cell_char(&buf, origin_x, origin_y), '┌');

        // Content starts after the 1-cell border + 1-char horizontal
        // padding, at the absolute screen position.
        let content_x = origin_x + 2;
        let content_y = origin_y + 1;
        assert_eq!(cell_char(&buf, content_x, content_y), 'h');

        // hit_test: a click at the first content cell (absolute coords)
        // should resolve to the (only) visible line.
        let p = Point::new(content_x as f32 + 0.5, content_y as f32 + 0.5);
        let hit = layout
            .hit_regions
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| h.clone());
        assert_eq!(hit, Some(TextInputHit::Line { line_idx: 0 }));
    }

    #[test]
    fn paint_and_click_round_trip() {
        paint_and_click_round_trip_at(0, 0);
    }

    /// Paint and hit-test must work correctly when the input is not at the
    /// screen origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7, 13);
    }

    /// TUI's focus treatment for a `TextInput`: the border tints to
    /// `theme.accent_fg` while `has_focus` is `true`, and back to
    /// `theme.border_fg` while it's `false`. This is the "existing
    /// treatment" the primitive's focus-ring seam defers to rather than
    /// adding a TUI-side ring stroke.
    #[test]
    fn border_tints_to_accent_when_focused_and_back_when_not() {
        let theme = Theme::default();
        let rect = Rect::new(0, 0, 10, 4);

        let mut focused = mk_input(vec!["hi"]);
        focused.has_focus = true;
        let mut buf_focused = Buffer::empty(rect);
        draw_text_input(&mut buf_focused, rect, &focused, &theme);

        let mut unfocused = mk_input(vec!["hi"]);
        unfocused.has_focus = false;
        let mut buf_unfocused = Buffer::empty(rect);
        draw_text_input(&mut buf_unfocused, rect, &unfocused, &theme);

        // Sample the top border edge (not a corner, not content) in both
        // buffers.
        let focused_border_fg = buf_focused[(1, 0)].fg;
        let unfocused_border_fg = buf_unfocused[(1, 0)].fg;

        assert_eq!(focused_border_fg, qc(theme.accent_fg));
        assert_eq!(unfocused_border_fg, qc(theme.border_fg));
        assert_ne!(
            focused_border_fg, unfocused_border_fg,
            "focus must change the painted border colour"
        );
    }

    /// A headless, end-to-end proof that [`draw_text_input`]'s border-tint
    /// treatment (above) reflects [`crate::focus::FocusTree`]'s real
    /// Tab-vs-click `:focus-visible` convention — composed as
    /// `tree.is_focused(id, None) && tree.is_focus_visible()`, the bool a
    /// host assigns to [`TextInput::has_focus`] — not just a literal
    /// `true`/`false` this test assigns itself.
    #[test]
    fn focused_text_input_paints_a_different_border_after_tab_than_after_click() {
        use crate::focus::FocusTree;

        let id = WidgetId::new("search:input");
        let mut tree = FocusTree::new();
        tree.sync_tab_order(&[(id, crate::event::Rect::new(0.0, 0.0, 0.0, 0.0))]);
        let mut input = mk_input(vec!["hi"]);
        let theme = Theme::default();
        let rect = Rect::new(0, 0, 10, 3);
        // Sample the top border edge, not a corner (paints the same
        // colour) and not the content row.
        let border_fg = |buf: &Buffer| buf[(1, 0)].fg;

        // Tab: `focus_next()` is the real keyboard-move path, not just
        // construction's initial state.
        tree.focus_next();
        input.has_focus = tree.is_focused(&id, None) && tree.is_focus_visible();
        assert!(
            input.has_focus,
            "a Tab-reached text input must report focus-visible"
        );
        let mut buf_tab = Buffer::empty(rect);
        draw_text_input(&mut buf_tab, rect, &input, &theme);

        // Click: `set_focus` re-focuses the same widget via the pointer path.
        tree.set_focus(Some((id, None)));
        input.has_focus = tree.is_focused(&id, None) && tree.is_focus_visible();
        assert!(
            tree.is_focused(&id, None),
            "the click did move focus to this input"
        );
        assert!(
            !input.has_focus,
            "a mouse click must suppress the text input's focus-visible ring"
        );
        let mut buf_click = Buffer::empty(rect);
        draw_text_input(&mut buf_click, rect, &input, &theme);

        assert_ne!(
            border_fg(&buf_tab),
            border_fg(&buf_click),
            "the painted border colour must differ between the Tab-reached \
             and clicked states"
        );

        // Shift+Tab afterwards restores both the composed bool and the
        // painted border.
        tree.focus_prev();
        input.has_focus = tree.is_focused(&id, None) && tree.is_focus_visible();
        assert!(
            input.has_focus,
            "a subsequent keyboard move restores focus-visible"
        );
        let mut buf_retreat = Buffer::empty(rect);
        draw_text_input(&mut buf_retreat, rect, &input, &theme);
        assert_eq!(
            border_fg(&buf_tab),
            border_fg(&buf_retreat),
            "Shift+Tab repaints the same focus-visible border Tab did"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accelerator::KeyBinding;
    use crate::event::{Key, NamedKey};
    use crate::types::Modifiers;

    fn input(lines: Vec<&str>) -> TextInput {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = lines.into_iter().map(String::from).collect();
        ti.has_focus = true;
        ti
    }

    /// An editing session over [`input`]'s buffer — the entry point for
    /// every `apply(EditOp)` test below. `TextEditor` `Deref`s to the
    /// wrapped `TextInput`, so `.lines` / `.cursor_col` reads and writes
    /// in these tests still go straight through to the primitive.
    fn editor(lines: Vec<&str>) -> TextEditor {
        TextEditor::new(input(lines))
    }

    fn measure() -> TextInputMeasure {
        TextInputMeasure::new(1.0, 1.0)
    }

    fn rect(w: f32, h: f32) -> Rect {
        Rect::new(0.0, 0.0, w, h)
    }

    #[test]
    fn empty_input_renders_one_row() {
        let ti = TextInput::new(WidgetId::new("ti"));
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.visible_lines.len(), 1);
        assert_eq!(l.visible_lines[0].line_idx, 0);
    }

    #[test]
    fn placeholder_active_when_empty() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.placeholder = Some("type here".into());
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(l.placeholder_active);
    }

    #[test]
    fn placeholder_inactive_with_content() {
        let mut ti = input(vec!["hello"]);
        ti.placeholder = Some("type here".into());
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(!l.placeholder_active);
    }

    #[test]
    fn cursor_bounds_at_origin_when_empty() {
        let ti = TextInput::new(WidgetId::new("ti"));
        let l = ti.layout(rect(20.0, 10.0), measure());
        let cb = l.cursor_bounds.unwrap();
        // Border (1) + horizontal padding (1 char_width) -> content starts at x=2.
        // Vertical padding is zero so content_y = border = 1.
        assert_eq!(cb.x, 2.0);
        assert_eq!(cb.y, 1.0);
        assert_eq!(cb.width, 1.0);
        assert_eq!(cb.height, 1.0);
    }

    #[test]
    fn cursor_bounds_track_col() {
        let mut ti = input(vec!["hello"]);
        ti.cursor_col = 3;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.cursor_bounds.unwrap().x, 2.0 + 3.0); // content_x + col
    }

    #[test]
    fn cursor_col_clamped_to_line_length() {
        let mut ti = input(vec!["hi"]);
        ti.cursor_col = 99;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.cursor_bounds.unwrap().x, 2.0 + 2.0); // clamped to 2
    }

    #[test]
    fn cursor_line_clamped_to_total() {
        let mut ti = input(vec!["a", "b", "c"]);
        ti.cursor_line = 99;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(l.cursor_bounds.is_some());
        // Cursor lands on line index 2 (last available).
        let cb = l.cursor_bounds.unwrap();
        assert_eq!(cb.y, 1.0 + 2.0); // border + 2 rows
    }

    // ── Horizontal scroll ────────────────────────────────────────────

    #[test]
    fn h_scroll_auto_pulls_cursor_into_view_rightward() {
        // Wide line, narrow viewport.
        let mut ti = input(vec!["abcdefghijklmnopqrstuvwxyz"]);
        // content_w = 10 - 2(border) - 2(pad) = 6 -> visible_cols = 6.
        ti.cursor_col = 20;
        let l = ti.layout(rect(10.0, 5.0), measure());
        // Cursor should be visible: scroll_col = 20 + 1 - 6 = 15.
        assert_eq!(l.resolved_scroll_col, 15);
        // Cursor x = content_x + (cursor_col - scroll_col) * char_w = 2 + 5 = 7.
        assert_eq!(l.cursor_bounds.unwrap().x, 7.0);
    }

    #[test]
    fn h_scroll_auto_pulls_cursor_into_view_leftward() {
        let mut ti = input(vec!["abcdefghijklmnopqrstuvwxyz"]);
        ti.cursor_col = 2;
        ti.scroll_col = 20; // stale — cursor is left of view
        let l = ti.layout(rect(10.0, 5.0), measure());
        assert_eq!(l.resolved_scroll_col, 2);
    }

    #[test]
    fn h_scroll_zero_when_line_fits() {
        let mut ti = input(vec!["short"]);
        ti.cursor_col = 5;
        let l = ti.layout(rect(20.0, 5.0), measure());
        assert_eq!(l.resolved_scroll_col, 0);
    }

    #[test]
    fn scroll_auto_pulls_cursor_into_view_downward() {
        // 10 lines, viewport fits 3, cursor on line 7.
        let mut ti = input(vec!["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        ti.cursor_line = 7;
        // content height = 5 - 2 (border) = 3, max_rows = 3.
        let l = ti.layout(rect(20.0, 5.0), measure());
        // Cursor should be visible: scroll = 7 + 1 - 3 = 5
        assert_eq!(l.resolved_scroll_offset, 5);
        assert!(l.cursor_bounds.is_some());
    }

    #[test]
    fn scroll_auto_pulls_cursor_into_view_upward() {
        let mut ti = input(vec!["0", "1", "2", "3", "4", "5"]);
        ti.cursor_line = 1;
        ti.scroll_offset = 4; // stale — cursor is above viewport
        let l = ti.layout(rect(20.0, 5.0), measure());
        assert_eq!(l.resolved_scroll_offset, 1);
    }

    #[test]
    fn scroll_clamped_when_cursor_in_view() {
        let mut ti = input(vec!["0", "1", "2", "3"]);
        ti.cursor_line = 0;
        ti.scroll_offset = 99; // wildly stale
        let l = ti.layout(rect(20.0, 10.0), measure());
        // max_scroll = 4 - (10-2)/1 = 4 - 8 = 0, so clamp to 0
        assert_eq!(l.resolved_scroll_offset, 0);
    }

    #[test]
    fn hit_regions_one_per_visible_line() {
        let ti = input(vec!["a", "b", "c"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        let line_hits: Vec<_> = l
            .hit_regions
            .iter()
            .filter(|(_, h)| matches!(h, TextInputHit::Line { .. }))
            .collect();
        assert_eq!(line_hits.len(), 3);
    }

    #[test]
    fn empty_area_hit_region_below_last_line() {
        let ti = input(vec!["only"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        let empty = l
            .hit_regions
            .iter()
            .find(|(_, h)| matches!(h, TextInputHit::EmptyArea));
        assert!(empty.is_some());
    }

    #[test]
    fn hit_test_line_returns_line_idx() {
        let ti = input(vec!["a", "b", "c"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        // Row 1 (0-indexed) is line_idx 1, at content_y=1.0 + 1*row_h=1.0.
        assert_eq!(l.hit_test(5.0, 2.0), TextInputHit::Line { line_idx: 1 });
    }

    #[test]
    fn hit_test_below_last_line_returns_empty_area() {
        let ti = input(vec!["only"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.hit_test(5.0, 8.0), TextInputHit::EmptyArea);
    }

    #[test]
    fn hit_test_outside_bounds_returns_empty_area() {
        let ti = input(vec!["only"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.hit_test(-5.0, -5.0), TextInputHit::EmptyArea);
    }

    #[test]
    fn pointer_shape_is_text_over_a_line_and_default_outside_content_bounds() {
        let ti = input(vec!["a", "b", "c"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.pointer_shape(5.0, 2.0), PointerShape::Text);
        assert_eq!(l.pointer_shape(-5.0, -5.0), PointerShape::Default);
    }

    #[test]
    fn pointer_shape_is_text_over_the_in_content_empty_band_below_last_line() {
        // 3 lines in a tall box: the `EmptyArea` hit region below the last
        // visible line (y in [4, 9) here) still falls inside
        // `content_bounds` (y in [1, 9)) — a native multi-line editor
        // shows the I-beam there too, not an arrow, even though
        // `hit_test` still reports `EmptyArea` for click routing.
        let ti = input(vec!["a", "b", "c"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.hit_test(5.0, 6.0), TextInputHit::EmptyArea);
        assert!(l.content_bounds.contains(Point::new(5.0, 6.0)));
        assert_eq!(l.pointer_shape(5.0, 6.0), PointerShape::Text);

        // Below `content_bounds` entirely (y = 9 is outside [1, 9)): back
        // to the default arrow.
        assert!(!l.content_bounds.contains(Point::new(5.0, 9.0)));
        assert_eq!(l.pointer_shape(5.0, 9.0), PointerShape::Default);
    }

    #[test]
    fn cursor_bounds_none_when_off_screen() {
        // Force scroll_offset stale by manipulating cursor_line.
        // Build 10 lines, cursor on 0, but max_rows shows 3 — should
        // auto-scroll to keep cursor in view (so this is harder to test).
        // Test the literal off-screen case: cursor_line clamped to 2,
        // scroll forced to 0, max_rows = 1.
        let mut ti = input(vec!["a", "b", "c"]);
        ti.cursor_line = 2;
        ti.scroll_offset = 0;
        let l = ti.layout(rect(20.0, 3.0), measure()); // content_h = 1, max_rows = 1
                                                       // Auto-scroll keeps cursor in view, so it's still visible:
        assert!(l.cursor_bounds.is_some());
        assert_eq!(l.resolved_scroll_offset, 2);
    }

    // ── content_cols (column budget for pre-wrap) ──────────────────────────

    #[test]
    fn content_cols_matches_layouts_content_width() {
        // Same rect/measure `layout()` would use — the column budget must
        // agree with what `content_bounds.width` actually renders, or a
        // caller pre-wrapping text to `content_cols` would produce rows
        // that don't fit (or leave slack) once painted.
        let ti = TextInput::new(WidgetId::new("ti"));
        let l = ti.layout(rect(20.0, 10.0), measure());
        let cols = TextInput::content_cols(20.0, 1.0);
        assert_eq!(cols, l.content_bounds.width as usize);
    }

    #[test]
    fn content_cols_never_below_one() {
        // A pathologically narrow rect must still yield a usable (>=1)
        // wrap budget, not zero (which would make callers loop forever
        // trying to fit even a single character per row).
        assert_eq!(TextInput::content_cols(0.0, 1.0), 1);
        assert_eq!(TextInput::content_cols(1.0, 8.0), 1);
    }

    // ── apply(EditOp) — insert / delete / cursor / selection / undo ─────

    #[test]
    fn insert_char_advances_cursor() {
        let mut ti = editor(vec![""]);
        assert!(ti.apply(EditOp::InsertChar('h')));
        assert!(ti.apply(EditOp::InsertChar('i')));
        assert_eq!(ti.lines, vec!["hi".to_string()]);
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 2));
    }

    #[test]
    fn insert_char_newline_splits_line() {
        let mut ti = editor(vec!["helloworld"]);
        ti.cursor_col = 5;
        assert!(ti.apply(EditOp::InsertChar('\n')));
        assert_eq!(ti.lines, vec!["hello".to_string(), "world".to_string()]);
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 0));
    }

    #[test]
    fn insert_char_is_multibyte_safe() {
        // Cursor sits after 'h' + é (a 2-byte char) — inserting must not
        // panic slicing mid-char, and must land after the é (char column
        // 2), not the byte offset.
        let mut ti = editor(vec!["héllo"]);
        ti.cursor_col = 2; // after 'h', 'é'
        assert!(ti.apply(EditOp::InsertChar('X')));
        assert_eq!(ti.lines, vec!["héXllo".to_string()]);
    }

    #[test]
    fn insert_text_with_embedded_newline_creates_lines() {
        let mut ti = editor(vec![""]);
        assert!(ti.apply(EditOp::InsertText("ab\ncd".to_string())));
        assert_eq!(ti.lines, vec!["ab".to_string(), "cd".to_string()]);
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 2));
    }

    #[test]
    fn delete_backward_removes_prior_char() {
        let mut ti = editor(vec!["hi"]);
        ti.cursor_col = 2;
        assert!(ti.apply(EditOp::DeleteBackward));
        assert_eq!(ti.lines, vec!["h".to_string()]);
        assert_eq!(ti.cursor_col, 1);
    }

    #[test]
    fn delete_backward_at_line_start_merges_with_previous_line() {
        let mut ti = editor(vec!["foo", "bar"]);
        ti.cursor_line = 1;
        ti.cursor_col = 0;
        assert!(ti.apply(EditOp::DeleteBackward));
        assert_eq!(ti.lines, vec!["foobar".to_string()]);
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 3));
    }

    #[test]
    fn delete_backward_at_doc_start_is_a_no_op() {
        let mut ti = editor(vec!["hi"]);
        assert!(!ti.apply(EditOp::DeleteBackward));
        assert_eq!(ti.lines, vec!["hi".to_string()]);
    }

    #[test]
    fn delete_forward_removes_next_char() {
        let mut ti = editor(vec!["hi"]);
        assert!(ti.apply(EditOp::DeleteForward));
        assert_eq!(ti.lines, vec!["i".to_string()]);
        assert_eq!(ti.cursor_col, 0);
    }

    #[test]
    fn delete_forward_at_line_end_merges_next_line() {
        let mut ti = editor(vec!["foo", "bar"]);
        ti.cursor_col = 3; // end of "foo"
        assert!(ti.apply(EditOp::DeleteForward));
        assert_eq!(ti.lines, vec!["foobar".to_string()]);
    }

    #[test]
    fn delete_forward_at_doc_end_is_a_no_op() {
        let mut ti = editor(vec!["hi"]);
        ti.cursor_col = 2;
        assert!(!ti.apply(EditOp::DeleteForward));
    }

    #[test]
    fn move_right_then_left_round_trips() {
        let mut ti = editor(vec!["hi"]);
        assert!(ti.apply(EditOp::MoveRight { extend: false }));
        assert_eq!(ti.cursor_col, 1);
        assert!(ti.apply(EditOp::MoveLeft { extend: false }));
        assert_eq!(ti.cursor_col, 0);
    }

    #[test]
    fn move_right_at_line_end_wraps_to_next_line() {
        let mut ti = editor(vec!["ab", "cd"]);
        ti.cursor_col = 2;
        assert!(ti.apply(EditOp::MoveRight { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 0));
    }

    #[test]
    fn move_left_at_line_start_wraps_to_previous_line_end() {
        let mut ti = editor(vec!["ab", "cd"]);
        ti.cursor_line = 1;
        assert!(ti.apply(EditOp::MoveLeft { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 2));
    }

    #[test]
    fn move_up_down_clamp_column_to_shorter_line() {
        let mut ti = editor(vec!["abcdef", "xy"]);
        ti.cursor_col = 5;
        assert!(ti.apply(EditOp::MoveDown { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 2)); // clamped to "xy"'s length
        assert!(ti.apply(EditOp::MoveUp { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 2)); // clamped col preserved
    }

    #[test]
    fn move_line_start_end_and_doc_start_end() {
        let mut ti = editor(vec!["abc", "de"]);
        ti.cursor_line = 1;
        ti.cursor_col = 1;
        assert!(ti.apply(EditOp::MoveLineEnd { extend: false }));
        assert_eq!(ti.cursor_col, 2);
        assert!(ti.apply(EditOp::MoveLineStart { extend: false }));
        assert_eq!(ti.cursor_col, 0);
        assert!(ti.apply(EditOp::MoveDocEnd { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 2));
        assert!(ti.apply(EditOp::MoveDocStart { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 0));
    }

    #[test]
    fn set_cursor_clamps_out_of_range_position() {
        let mut ti = editor(vec!["abc"]);
        assert!(ti.apply(EditOp::SetCursor {
            line: 99,
            col: 99,
            extend: false
        }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 3));
    }

    // ── selection ─────────────────────────────────────────────────────

    #[test]
    fn extend_move_creates_selection_and_selected_text() {
        let mut ti = editor(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true });
        assert_eq!(ti.selection_range(), Some(((0, 0), (0, 2))));
        assert_eq!(ti.selected_text().as_deref(), Some("he"));
    }

    #[test]
    fn plain_move_after_extend_collapses_selection() {
        let mut ti = editor(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: false });
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn unmodified_left_after_selection_lands_on_near_edge() {
        // Selecting rightward then pressing plain Left should land the
        // cursor exactly on the selection's start, not one further step
        // left of the (moving-end) cursor position.
        let mut ti = editor(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true }); // selects "he", cursor at col 2
        assert!(ti.apply(EditOp::MoveLeft { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 0));
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn unmodified_right_after_selection_lands_on_far_edge() {
        // Same as above, mirrored: plain Right lands on the selection's
        // end rather than stepping one further right of the cursor.
        let mut ti = editor(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true }); // selects "he", cursor at col 2
        assert!(ti.apply(EditOp::MoveRight { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 2));
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn unmodified_left_after_leftward_selection_lands_on_near_edge() {
        // Selecting *leftward* (cursor is the near edge already) then
        // pressing plain Left should still land on the selection start —
        // exercising the anchor > cursor ordering in `selection_range`.
        let mut ti = editor(vec!["hello"]);
        ti.cursor_col = 3;
        ti.apply(EditOp::MoveLeft { extend: true });
        ti.apply(EditOp::MoveLeft { extend: true }); // selects "el" (cols 1..3), cursor at col 1
        assert!(ti.apply(EditOp::MoveLeft { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 1));
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn select_all_selects_entire_buffer() {
        let mut ti = editor(vec!["ab", "cde"]);
        assert!(ti.apply(EditOp::SelectAll));
        assert_eq!(ti.selected_text().as_deref(), Some("ab\ncde"));
    }

    #[test]
    fn insert_over_selection_replaces_it() {
        let mut ti = editor(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true }); // selects "he"
        assert!(ti.apply(EditOp::InsertChar('X')));
        assert_eq!(ti.lines, vec!["Xllo".to_string()]);
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn delete_backward_over_selection_deletes_selection_not_one_char() {
        let mut ti = editor(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true }); // selects "he"
        assert!(ti.apply(EditOp::DeleteBackward));
        assert_eq!(ti.lines, vec!["llo".to_string()]);
    }

    #[test]
    fn delete_selection_op_clears_selection_with_no_replacement() {
        let mut ti = editor(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true });
        assert!(ti.apply(EditOp::DeleteSelection));
        assert_eq!(ti.lines, vec!["llo".to_string()]);
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn delete_selection_with_no_selection_is_a_no_op() {
        let mut ti = editor(vec!["hello"]);
        assert!(!ti.apply(EditOp::DeleteSelection));
    }

    #[test]
    fn multiline_selection_spans_lines() {
        let mut ti = editor(vec!["abc", "def"]);
        ti.apply(EditOp::MoveDown { extend: true });
        ti.apply(EditOp::MoveLineEnd { extend: true });
        assert_eq!(ti.selected_text().as_deref(), Some("abc\ndef"));
    }

    // ── undo / redo (via `TextEditor` — see its module doc for
    //    why history is not a `TextInput` field) ─────────────────────

    #[test]
    fn redo_with_no_history_is_a_no_op() {
        let mut ti = editor(vec![""]);
        ti.apply(EditOp::InsertChar('h'));
        assert!(!ti.apply(EditOp::Redo), "nothing has been undone yet");
        assert_eq!(ti.lines, vec!["h".to_string()]);
    }

    #[test]
    fn undo_reverts_last_insert() {
        let mut ti = editor(vec![""]);
        ti.apply(EditOp::InsertChar('h'));
        ti.apply(EditOp::InsertChar('i'));
        assert_eq!(ti.lines, vec!["hi".to_string()]);
        assert!(ti.apply(EditOp::Undo));
        assert_eq!(ti.lines, vec!["h".to_string()]);
        assert!(ti.apply(EditOp::Undo));
        assert_eq!(ti.lines, vec!["".to_string()]);
    }

    #[test]
    fn undo_with_no_history_is_a_no_op() {
        let mut ti = editor(vec![""]);
        assert!(!ti.apply(EditOp::Undo));
    }

    #[test]
    fn redo_restores_undone_edit() {
        let mut ti = editor(vec![""]);
        ti.apply(EditOp::InsertChar('h'));
        ti.apply(EditOp::Undo);
        assert_eq!(ti.lines, vec!["".to_string()]);
        assert!(ti.apply(EditOp::Redo));
        assert_eq!(ti.lines, vec!["h".to_string()]);
    }

    #[test]
    fn new_edit_after_undo_clears_redo_history() {
        let mut ti = editor(vec![""]);
        ti.apply(EditOp::InsertChar('a'));
        ti.apply(EditOp::Undo);
        ti.apply(EditOp::InsertChar('b'));
        assert!(
            !ti.apply(EditOp::Redo),
            "redo history should have been invalidated by the new edit"
        );
        assert_eq!(ti.lines, vec!["b".to_string()]);
    }

    #[test]
    fn cursor_movement_does_not_create_an_undo_step() {
        // Typing, then moving, then undoing should undo the typing — not
        // a no-op "undo the move" step.
        let mut ti = editor(vec![""]);
        ti.apply(EditOp::InsertChar('a'));
        ti.apply(EditOp::MoveLeft { extend: false });
        assert!(ti.apply(EditOp::Undo));
        assert_eq!(ti.lines, vec!["".to_string()]);
    }

    #[test]
    fn undo_restores_selection_state() {
        let mut ti = editor(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true }); // selects "he"
        ti.apply(EditOp::DeleteSelection);
        assert_eq!(ti.lines, vec!["llo".to_string()]);
        assert!(ti.apply(EditOp::Undo));
        assert_eq!(ti.lines, vec!["hello".to_string()]);
        assert_eq!(ti.selection_range(), Some(((0, 0), (0, 2))));
    }

    #[test]
    fn undo_history_is_bounded() {
        // Regression test for the unbounded-memory concern: recording
        // more than `UNDO_HISTORY_LIMIT` steps evicts the oldest ones
        // instead of growing forever.
        let mut ti = editor(vec![""]);
        for _ in 0..UNDO_HISTORY_LIMIT + 50 {
            ti.apply(EditOp::InsertChar('a'));
        }
        let mut undone = 0;
        while ti.apply(EditOp::Undo) {
            undone += 1;
        }
        assert_eq!(undone, UNDO_HISTORY_LIMIT);
    }

    // ── EditOp::from_key / from_key_binding ──────────────────────────

    #[test]
    fn from_key_maps_plain_char_to_insert() {
        assert_eq!(
            EditOp::from_key(&Key::Char('x'), Modifiers::default()),
            Some(EditOp::InsertChar('x'))
        );
    }

    #[test]
    fn from_key_maps_arrows_with_shift_to_extend() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Right), shift),
            Some(EditOp::MoveRight { extend: true })
        );
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Right), Modifiers::default()),
            Some(EditOp::MoveRight { extend: false })
        );
    }

    #[test]
    fn from_key_ctrl_home_end_map_to_doc_start_end() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Home), ctrl),
            Some(EditOp::MoveDocStart { extend: false })
        );
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::End), ctrl),
            Some(EditOp::MoveDocEnd { extend: false })
        );
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Home), Modifiers::default()),
            Some(EditOp::MoveLineStart { extend: false })
        );
    }

    #[test]
    fn from_key_unmapped_key_returns_none() {
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Escape), Modifiers::default()),
            None
        );
    }

    #[test]
    fn from_key_binding_wires_undo_redo_select_all() {
        assert_eq!(
            EditOp::from_key_binding(&KeyBinding::Undo),
            Some(EditOp::Undo)
        );
        assert_eq!(
            EditOp::from_key_binding(&KeyBinding::Redo),
            Some(EditOp::Redo)
        );
        assert_eq!(
            EditOp::from_key_binding(&KeyBinding::SelectAll),
            Some(EditOp::SelectAll)
        );
        assert_eq!(EditOp::from_key_binding(&KeyBinding::Copy), None);
    }

    // ── TextInput value equality ignores undo history ────────────────

    #[test]
    fn equality_ignores_undo_history() {
        // `TextInput` carries no history at all (see `TextEditor`'s
        // module doc for why), so its `PartialEq` never has history to
        // ignore in the first place. Demonstrate that at the wrapper
        // level: `a` reaches "x" directly, `b` reaches "x" via an extra
        // insert-then-undo round trip that leaves `b`'s redo stack
        // non-empty (unlike `a`'s) — the wrapped `TextInput`s are still
        // equal.
        let mut a = editor(vec![""]);
        let mut b = editor(vec![""]);
        a.apply(EditOp::InsertChar('x'));
        b.apply(EditOp::InsertChar('x'));
        b.apply(EditOp::InsertChar('y'));
        b.apply(EditOp::Undo);
        assert_eq!(a.lines, b.lines);
        assert_eq!(
            a.input, b.input,
            "equality must compare content, not undo history"
        );
    }

    // ── `new`/`with_*`/`Default` builders ──────────────────────────────

    /// `Default::default()` matches `TextInput::new` with an empty id —
    /// the `new(required…)`/`with_*`/`Default` trio only differs in what
    /// `id` it carries.
    #[test]
    fn default_matches_new_with_empty_id() {
        assert_eq!(TextInput::default(), TextInput::new(WidgetId::new("")));
    }

    /// Chaining every `with_*` builder reaches exactly the values the
    /// exhaustive literal in `tests/downstream_struct_literals.rs` sets
    /// field-for-field, without writing a struct literal at all.
    #[test]
    fn with_builders_reach_every_field_a_struct_literal_can_set() {
        let ti = TextInput::new(WidgetId::new("sc:commit_input"))
            .with_lines(vec![
                "subject".to_string(),
                String::new(),
                "body".to_string(),
            ])
            .with_cursor_line(2)
            .with_cursor_col(4)
            .with_placeholder("Message (press c)")
            .with_scroll_offset(0)
            .with_scroll_col(0)
            .with_has_focus(true);

        assert_eq!(
            ti,
            TextInput {
                id: WidgetId::new("sc:commit_input"),
                lines: vec!["subject".to_string(), String::new(), "body".to_string()],
                cursor_line: 2,
                cursor_col: 4,
                placeholder: Some("Message (press c)".to_string()),
                scroll_offset: 0,
                scroll_col: 0,
                has_focus: true,
            }
        );
    }

    // ── `TextInputMetrics` ──────────────────────────────────────────────

    #[test]
    fn text_input_metrics_resolves_height_straight_from_style() {
        let style = Style::default().with_control_height(32.0);
        let metrics = TextInputMetrics::resolve(&style);
        assert_eq!(metrics.height, 32.0);
    }

    /// Default `Style::control_height` is `26.0` — the acceptance number
    /// a single-line `TextInput`/`PasswordInput` box must reach, in
    /// contrast to a backend's own (smaller) line-height measurement.
    #[test]
    fn text_input_metrics_matches_style_default() {
        let style = Style::default();
        let metrics = TextInputMetrics::resolve(&style);
        assert_eq!(metrics.height, 26.0);
        assert_eq!(metrics.height, style.control_height);
    }

    // ── `apply_flat_edit_op` — the flat-value `EditOp` bridge ───────────

    #[test]
    fn apply_flat_edit_op_inserts_at_the_cursor() {
        let (value, cursor, anchor) = apply_flat_edit_op("hi", 1, None, EditOp::InsertChar('!'));
        assert_eq!(value, "h!i");
        assert_eq!(cursor, 2);
        assert_eq!(anchor, None);
    }

    #[test]
    fn apply_flat_edit_op_delete_backward_removes_the_prior_char() {
        let (value, cursor, _) = apply_flat_edit_op("hi", 2, None, EditOp::DeleteBackward);
        assert_eq!(value, "h");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn apply_flat_edit_op_replaces_an_active_selection() {
        let (value, cursor, anchor) =
            apply_flat_edit_op("hello", 5, Some(1), EditOp::InsertChar('!'));
        assert_eq!(value, "h!");
        assert_eq!(cursor, 2);
        assert_eq!(anchor, None);
    }

    #[test]
    fn apply_flat_edit_op_round_trips_a_newline_through_line_col_conversion() {
        // Exercises `byte_offset_to_line_col`/`line_col_to_byte_offset`
        // against a multi-line flat value (what `FieldKind::TextArea`
        // hands this bridge), not just the single-line path.
        let (value, cursor, _) = apply_flat_edit_op("ab\ncd", 3, None, EditOp::InsertChar('X'));
        assert_eq!(value, "ab\nXcd");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn apply_flat_edit_op_snaps_a_mid_char_selection_anchor_instead_of_panicking() {
        // `selection_anchor` is app-owned public data and isn't
        // guaranteed to land on a char boundary — byte 2 is mid-`é` in
        // "h\u{e9}llo".
        let value = "h\u{e9}llo";
        let (new_value, _, _) =
            apply_flat_edit_op(value, value.len(), Some(2), EditOp::DeleteSelection);
        assert_eq!(new_value, "h");
    }
}

/// Cell-painting entry points used by this module's own `#[cfg(test)]`
/// rasteriser tests: they build the `Buffer` -> [`crate::CellSurface`]
/// adapter (private to `crate::tui`) the same way
/// [`crate::primitive::Primitive::paint_cells`] does, so a test can
/// paint into a bare ratatui buffer without a backend.
#[cfg(all(test, feature = "tui"))]
mod cell_test_paint {
    use super::*;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;

    fn surface<'a>(
        buf: &'a mut Buffer,
        sink: &'a mut Option<(u16, u16, crate::cell_surface::CursorShape)>,
    ) -> impl crate::CellSurface + 'a {
        crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            sink,
        )
    }

    pub fn draw_text_input(
        buf: &mut Buffer,
        rect: ratatui::layout::Rect,
        ti: &TextInput,
        theme: &Theme,
    ) -> TextInputLayout {
        let area = Rect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        );
        let layout = cell_layout(ti, area);
        let mut sink = None;
        cell_paint::paint(ti, &layout, &mut surface(buf, &mut sink), theme);
        layout
    }
}
