//! `FindReplace` overlay primitive — the inline find/replace panel
//! that anchors at the top-right of the active editor group.
//!
//! The panel is a contract widget: [`crate::primitive::Primitive`] lays it
//! out filling the rect it is painted in (the popup box, border included)
//! and paints it through the shared [`native_surface_paint::paint`] on a
//! pixel surface or the `paint_cells` painter on a cell grid;
//! [`crate::interactive::Interactive`] turns clicks and keys into
//! [`FindReplaceEvent`]s in controlled mode — the host owns every field
//! and writes each event back with [`FindReplacePanel::apply_event`].
//! [`FindReplacePanel::anchored_rect`] places the box at the top-right of
//! [`FindReplacePanel::group_bounds`] for the host to paint it in.
//!
//! # Why a primitive (and not just a `StatusBar` variant)
//!
//! The find/replace overlay has multi-row content (find row + optional
//! replace row), input-field cursor + selection, toggle-button states
//! (match-case / whole-word / regex / preserve-case / in-selection —
//! the cell painter paints these as plain-text glyph pairs `Aa` / `ab` /
//! `.*` / `AB`, pixel surfaces paint codicon glyphs, see
//! [`native_surface_paint`]), and clickable nav + action buttons all
//! packed inside a 50-cell-wide bordered box.
//! That doesn't fit any of the simpler primitives — so it gets its
//! own `FindReplacePanel` shape.
//!
//! # Hit-region cell-coordinate contract
//!
//! Hit regions are in **character-cell units** relative to the panel
//! content corner (inside the borders). [`FindReplaceLayout`] projects
//! them to the surface's native units with its own char width and row
//! pitch, so paint and hit-test read the same geometry on a cell grid and
//! a pixel surface alike.

use crate::event::Rect;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

#[cfg(feature = "tui")]
mod paint_cells;

/// Click target within the find/replace overlay. [`FindReplacePanel::hit_test`]
/// resolves a native point to one of these, and the panel's own
/// [`crate::interactive::Interactive::handle`] turns it into a
/// [`FindReplaceEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum FindReplaceClickTarget {
    /// Toggle replace row visibility (the ▶/▼ chevron).
    Chevron,
    /// Click in the find input field at the given char offset.
    FindInput(usize),
    /// Click in the replace input field at the given char offset.
    ReplaceInput(usize),
    ToggleCase,
    ToggleWholeWord,
    ToggleRegex,
    PrevMatch,
    NextMatch,
    ToggleInSelection,
    Close,
    TogglePreserveCase,
    ReplaceCurrent,
    ReplaceAll,
}

/// A hit region within the find/replace panel, expressed in
/// character-cell units relative to the panel's top-left **content**
/// corner (inside borders).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FrHitRegion {
    /// Column offset from panel content-left edge.
    pub col: u16,
    /// Row: 0 = find row, 1 = replace row.
    pub row: u16,
    /// Width of this region in char cells.
    pub width: u16,
}

/// Default panel width for the find/replace overlay (in char cells,
/// including borders).
pub const FR_PANEL_WIDTH: u16 = 50;

/// Compute hit regions for the find/replace overlay.
///
/// Layout (cell widths — glyph names are shorthand for the toggle
/// slot, not a claim about what's painted there; the cell painter paints
/// the named text pair, pixel surfaces paint a same-width codicon glyph):
/// `[chevron(2)] [input(variable)] [Aa(2+1)][ab(2+1)][.*(2+1)] [count(max(len,5)+1)] [↑(2)][↓(2)][≡(2)][×(2)]`
///
/// Returns `(regions, input_width)` — the input field's width (in
/// cells) is computed last and returned alongside so the caller can
/// store it on the panel for cursor placement.
///
/// `replace_one_glyph_width` and `replace_all_glyph_width` are the
/// char-cell widths of the replace-button glyphs. Apps with Nerd Font
/// glyphs typically pass `1` (single-cell glyph); ASCII-fallback apps
/// pass the string's `chars().count()` (e.g. 2 for `"R1"` / `"R*"`).
/// The widths are app-supplied (rather than read from a global icon
/// registry) so this primitive doesn't depend on any host app's icon
/// system.
pub fn compute_hit_regions(
    panel_w: u16,
    show_replace: bool,
    match_info: &str,
    replace_one_glyph_width: u16,
    replace_all_glyph_width: u16,
) -> (Vec<(FrHitRegion, FindReplaceClickTarget)>, u16) {
    use FindReplaceClickTarget::*;

    let mut regions = Vec::with_capacity(16);
    let content_w = panel_w.saturating_sub(2);

    // Chevron: cols 0..2
    regions.push((
        FrHitRegion {
            col: 0,
            row: 0,
            width: 2,
        },
        Chevron,
    ));

    // Find input: starts at col 2, right side uses remaining space for buttons.
    // Right side: toggles(3×3=9) + gap(1) + match_info(max(len,5)) + gap(1) + nav(4×2=8) = dynamic
    let info_len = (match_info.len() as u16).max(5);
    let right_side_w: u16 = 9 + info_len + 1 + 8; // toggles + count + gap + nav
    let input_start: u16 = 2;
    let input_w = content_w.saturating_sub(2 + right_side_w);
    regions.push((
        FrHitRegion {
            col: input_start,
            row: 0,
            width: input_w,
        },
        FindInput(0),
    ));

    // Toggle buttons: [Aa(2)gap(1)] [ab(2)gap(1)] [.*(2)gap(1)] — cell
    // widths only; glyph names are shorthand (the cell painter paints
    // these text pairs literally, pixel surfaces paint same-width
    // codicon glyphs).
    let mut tx = input_start + input_w + 1;
    for target in [ToggleCase, ToggleWholeWord, ToggleRegex] {
        regions.push((
            FrHitRegion {
                col: tx,
                row: 0,
                width: 2,
            },
            target,
        ));
        tx += 3;
    }

    // Match count (not clickable)
    tx += info_len + 1;

    // Nav buttons: [↑(2)][↓(2)][≡(2)][×(2)]
    for target in [PrevMatch, NextMatch, ToggleInSelection, Close] {
        regions.push((
            FrHitRegion {
                col: tx,
                row: 0,
                width: 2,
            },
            target,
        ));
        tx += 2;
    }

    // Replace row (row 1)
    if show_replace {
        regions.push((
            FrHitRegion {
                col: input_start,
                row: 1,
                width: input_w,
            },
            ReplaceInput(0),
        ));

        let mut bx = input_start + input_w + 1;
        regions.push((
            FrHitRegion {
                col: bx,
                row: 1,
                width: 2,
            },
            TogglePreserveCase,
        ));
        bx += 3;

        regions.push((
            FrHitRegion {
                col: bx,
                row: 1,
                width: replace_one_glyph_width,
            },
            ReplaceCurrent,
        ));
        bx += replace_one_glyph_width + 1;

        regions.push((
            FrHitRegion {
                col: bx,
                row: 1,
                width: replace_all_glyph_width,
            },
            ReplaceAll,
        ));
    }

    (regions, input_w)
}

/// The inline find/replace overlay displayed at the top-right of the
/// active editor group.
///
/// Paint and hit-test both walk `hit_regions`, so a painted button and
/// its clickable band can never disagree.
///
/// Glyph fields (`replace_one_glyph` / `replace_all_glyph`) are
/// app-supplied strings — apps with Nerd Font glyphs pass single-char
/// strings, ASCII apps pass multi-char fallbacks like `"R1"` / `"R*"`.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     compute_find_replace_hit_regions, FindReplaceClickTarget, FindReplaceHit, FindReplacePanel,
///     Rect, WidgetId,
/// };
///
/// let panel_width = 50;
/// let (hit_regions, _input_width) =
///     compute_find_replace_hit_regions(panel_width, false, "1 of 3", 1, 1);
///
/// let panel = FindReplacePanel {
///     id: WidgetId::new("find_replace"),
///     query: "foo".to_string(),
///     replacement: String::new(),
///     show_replace: false,
///     focus: 0,
///     cursor: 3,
///     sel_anchor: None,
///     match_info: "1 of 3".to_string(),
///     case_sensitive: false,
///     whole_word: false,
///     use_regex: false,
///     preserve_case: false,
///     in_selection: false,
///     group_bounds: Rect::new(0.0, 0.0, 80.0, 24.0),
///     panel_width,
///     replace_one_glyph: "R1".to_string(),
///     replace_all_glyph: "R*".to_string(),
///     hit_regions,
/// };
///
/// // Column 0, row 0 is the chevron that toggles the replace row.
/// let hit = panel.hit_test(0.0, 0.0, (0.0, 0.0), 1.0, 1.0);
/// assert_eq!(hit, FindReplaceHit::Target(FindReplaceClickTarget::Chevron));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FindReplacePanel {
    /// This overlay's id — the one its [`FindReplaceEvent`]s are reported
    /// under. Defaults to `"find_replace"`: one overlay is live per editor
    /// window.
    #[serde(default = "default_find_replace_id")]
    pub id: WidgetId,
    /// Current query text in the find field.
    pub query: String,
    /// Current replacement text (only shown when `show_replace` is true).
    #[serde(default)]
    pub replacement: String,
    /// Whether the replace row is visible.
    #[serde(default)]
    pub show_replace: bool,
    /// Which field has focus: 0 = find, 1 = replace.
    #[serde(default)]
    pub focus: u8,
    /// Cursor position within the focused field (char offset).
    #[serde(default)]
    pub cursor: usize,
    /// Selection anchor in the focused field. When Some, text between anchor and cursor is selected.
    #[serde(default)]
    pub sel_anchor: Option<usize>,
    /// "N of M" match count display, or "No results" / empty.
    #[serde(default)]
    pub match_info: String,
    /// Toggle button states (find row).
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub use_regex: bool,
    /// Toggle button states (replace row).
    #[serde(default)]
    pub preserve_case: bool,
    /// Find in selection mode.
    #[serde(default)]
    pub in_selection: bool,
    /// Bounding rect of the active editor group, in the surface's own
    /// units. [`Self::anchored_rect`] places the overlay at its top-right.
    /// Apps with f64 native rect types convert via `as f32` at
    /// construction.
    pub group_bounds: Rect,
    /// Panel width in char cells, borders included.
    #[serde(default = "default_panel_width")]
    pub panel_width: u16,
    /// App-supplied glyph string for the "replace current" button.
    /// Single char (Nerd Font) or multi-char (ASCII fallback).
    #[serde(default)]
    pub replace_one_glyph: String,
    /// App-supplied glyph string for the "replace all" button.
    #[serde(default)]
    pub replace_all_glyph: String,
    /// Hit regions for click handling, in char-cell units relative to
    /// the panel content corner (inside borders). Computed via
    /// [`compute_hit_regions`] (or [`Self::rebuild_hit_regions`]) whenever
    /// `show_replace`, `match_info` or a glyph changes.
    #[serde(default)]
    pub hit_regions: Vec<(FrHitRegion, FindReplaceClickTarget)>,
}

/// `#[serde(default)]` helper for [`FindReplacePanel::panel_width`] — a
/// decoded panel with no explicit width gets this crate's own default
/// overlay width ([`FR_PANEL_WIDTH`]) rather than `0`, which a rasteriser
/// would paint as a zero-width box.
fn default_panel_width() -> u16 {
    FR_PANEL_WIDTH
}

/// `#[serde(default)]` helper for [`FindReplacePanel::id`].
fn default_find_replace_id() -> WidgetId {
    WidgetId::new("find_replace")
}

/// Hit-test classification for a click against a [`FindReplacePanel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindReplaceHit {
    /// Click landed on a clickable target.
    Target(FindReplaceClickTarget),
    /// Click missed every hit region (e.g. the borders, the non-clickable
    /// match-count text, or outside the panel entirely).
    Empty,
}

/// One of the panel's two text fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum FindReplaceField {
    /// The find (query) field — [`FindReplacePanel::query`], `focus == 0`.
    Find,
    /// The replace field — [`FindReplacePanel::replacement`], `focus == 1`.
    Replace,
}

impl FindReplaceField {
    /// The [`FindReplacePanel::focus`] value naming this field.
    pub fn focus_index(self) -> u8 {
        match self {
            FindReplaceField::Find => 0,
            FindReplaceField::Replace => 1,
        }
    }
}

/// One of the panel's on/off switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum FindReplaceToggle {
    /// The chevron: [`FindReplacePanel::show_replace`].
    ShowReplace,
    /// [`FindReplacePanel::case_sensitive`].
    CaseSensitive,
    /// [`FindReplacePanel::whole_word`].
    WholeWord,
    /// [`FindReplacePanel::use_regex`].
    Regex,
    /// [`FindReplacePanel::preserve_case`].
    PreserveCase,
    /// [`FindReplacePanel::in_selection`].
    InSelection,
}

/// What a [`FindReplacePanel`] reports. The panel is controlled: every
/// field is the host's, and [`FindReplacePanel::apply_event`] writes one
/// of these back onto it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum FindReplaceEvent {
    /// A field's text, caret or selection changed, or focus moved to it
    /// (a click in it, Tab). `cursor` and `sel_anchor` are char offsets
    /// into `text`; the host sets `focus` to `field`.
    FieldEdited {
        field: FindReplaceField,
        text: String,
        cursor: usize,
        sel_anchor: Option<usize>,
    },
    /// A switch was flipped; `on` is its new value.
    Toggled { toggle: FindReplaceToggle, on: bool },
    /// Go to the previous match (↑, Shift+Enter in the find field).
    PrevMatch,
    /// Go to the next match (↓, Enter in the find field).
    NextMatch,
    /// Replace the current match (its button, Enter in the replace field).
    ReplaceCurrent,
    /// Replace every match (its button, Ctrl/Cmd+Enter in the replace
    /// field).
    ReplaceAll,
    /// Dismiss the overlay (×, Escape).
    Close,
}

impl crate::interactive::WidgetAction for FindReplaceEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::FindReplace(id, self)
    }
}

impl FindReplacePanel {
    /// Hit-test a click at surface-native `(x, y)` against
    /// [`Self::hit_regions`].
    ///
    /// `content_origin` is the absolute position of the panel's content
    /// corner — inside the popup's own border and padding. `char_width`
    /// is the surface's per-cell width. `line_height` is this call's row
    /// *pitch* — the vertical distance from one row's top edge to the
    /// next — **not** necessarily the backend's raw text line height.
    /// [`FindReplaceLayout::hit_test`] passes its own resolved origin and
    /// pitch, so a caller holding a layout never picks these by hand.
    ///
    /// Coordinate frame: **ABSOLUTE** — `(x, y)` is compared directly
    /// against `content_origin`, no further adjustment by the caller.
    ///
    /// Each [`FrHitRegion`] is stored in character-cell units, but the
    /// comparison below is done in native units: every region's cell rect
    /// is projected — `rx0 = ox + col * char_width`, `rx1 = rx0 + width *
    /// char_width`, `ry0 = oy + row * line_height`, `ry1 = ry0 +
    /// line_height` — and `(x, y)` is tested against that rect directly.
    /// On a cell grid `char_width = line_height = 1.0`, where native and
    /// cell space are numerically identical.
    pub fn hit_test(
        &self,
        x: f32,
        y: f32,
        content_origin: (f32, f32),
        char_width: f32,
        line_height: f32,
    ) -> FindReplaceHit {
        if char_width <= 0.0 || line_height <= 0.0 {
            return FindReplaceHit::Empty;
        }
        let (ox, oy) = content_origin;
        if x < ox || y < oy {
            return FindReplaceHit::Empty;
        }
        for (region, target) in &self.hit_regions {
            let rx0 = ox + region.col as f32 * char_width;
            let rx1 = rx0 + region.width as f32 * char_width;
            let ry0 = oy + region.row as f32 * line_height;
            let ry1 = ry0 + line_height;
            if x >= rx0 && x < rx1 && y >= ry0 && y < ry1 {
                return FindReplaceHit::Target(*target);
            }
        }
        FindReplaceHit::Empty
    }

    /// Recompute [`Self::hit_regions`] from the panel's own width,
    /// `show_replace`, `match_info` and replace-glyph widths (each glyph's
    /// char count).
    pub fn rebuild_hit_regions(&mut self) {
        let (regions, _input_w) = compute_hit_regions(
            self.panel_width,
            self.show_replace,
            &self.match_info,
            self.replace_one_glyph.chars().count() as u16,
            self.replace_all_glyph.chars().count() as u16,
        );
        self.hit_regions = regions;
    }

    /// The box this overlay occupies at the top-right of
    /// [`Self::group_bounds`] — the rect a host paints and routes it in.
    ///
    /// `bounds` is the area the overlay may use: on a cell grid the box
    /// is at most `bounds.width - 2` cells wide and never starts left of
    /// `bounds.x`. `style`, `char_width` and `line_height` are the
    /// painting backend's own ([`crate::Backend::style`],
    /// [`crate::Backend::char_width`], [`crate::Backend::chrome_line_height`]);
    /// a `line_height` of `1.0` or less is a cell grid.
    ///
    /// - Cell grid: `panel_width` cells wide, one row per visible field
    ///   plus the two border rows, its right border one cell inside the
    ///   group's right edge, and never on row 0.
    /// - Pixel surface: `panel_width` char advances wide, one
    ///   [`crate::Style::control_height`] row per visible field plus
    ///   [`crate::Style::padding`] above and below, 10 units inside the
    ///   group's right edge and 2 below its top.
    pub fn anchored_rect_in(
        &self,
        bounds: Rect,
        style: &crate::style::Style,
        char_width: f32,
        line_height: f32,
    ) -> Rect {
        if line_height > 1.0 {
            return FindReplaceMetrics::resolve(self, style, char_width, line_height).popup_rect;
        }
        let panel_w = (self.panel_width as f32).min((bounds.width - 2.0).max(0.0));
        let rows = if self.show_replace { 2.0 } else { 1.0 };
        let gb = &self.group_bounds;
        let right = gb.x.floor() + gb.width.floor();
        let x = (right - panel_w - 1.0).max(bounds.x).max(0.0);
        let y = gb.y.floor().max(1.0);
        Rect::new(x, y, panel_w, rows + 2.0)
    }

    /// [`Self::anchored_rect_in`] with `backend`'s own style and metrics.
    pub fn anchored_rect(&self, backend: &dyn crate::Backend, bounds: Rect) -> Rect {
        self.anchored_rect_in(
            bounds,
            &backend.style(),
            backend.char_width(),
            backend.chrome_line_height(),
        )
    }

    /// The text of `field`.
    pub fn field_text(&self, field: FindReplaceField) -> &str {
        match field {
            FindReplaceField::Find => &self.query,
            FindReplaceField::Replace => &self.replacement,
        }
    }

    /// The field [`Self::focus`] names. The replace field counts only
    /// while its row is visible.
    pub fn focused_field(&self) -> FindReplaceField {
        if self.focus == 1 && self.show_replace {
            FindReplaceField::Replace
        } else {
            FindReplaceField::Find
        }
    }

    /// Write `event` back onto the panel — the controlled-mode apply step
    /// a host runs for every [`FindReplaceEvent`] it accepts. Toggling the
    /// replace row rebuilds [`Self::hit_regions`], and hiding it moves
    /// focus back to the find field. The match actions
    /// ([`FindReplaceEvent::NextMatch`] and the rest) change nothing here:
    /// they are the host's to perform.
    pub fn apply_event(&mut self, event: &FindReplaceEvent) {
        match event {
            FindReplaceEvent::FieldEdited {
                field,
                text,
                cursor,
                sel_anchor,
            } => {
                match field {
                    FindReplaceField::Find => self.query = text.clone(),
                    FindReplaceField::Replace => self.replacement = text.clone(),
                }
                self.focus = field.focus_index();
                self.cursor = *cursor;
                self.sel_anchor = *sel_anchor;
            }
            FindReplaceEvent::Toggled { toggle, on } => match toggle {
                FindReplaceToggle::ShowReplace => {
                    self.show_replace = *on;
                    if !*on && self.focus == 1 {
                        self.focus = 0;
                        self.cursor = self.query.chars().count();
                        self.sel_anchor = None;
                    }
                    self.rebuild_hit_regions();
                }
                FindReplaceToggle::CaseSensitive => self.case_sensitive = *on,
                FindReplaceToggle::WholeWord => self.whole_word = *on,
                FindReplaceToggle::Regex => self.use_regex = *on,
                FindReplaceToggle::PreserveCase => self.preserve_case = *on,
                FindReplaceToggle::InSelection => self.in_selection = *on,
            },
            FindReplaceEvent::PrevMatch
            | FindReplaceEvent::NextMatch
            | FindReplaceEvent::ReplaceCurrent
            | FindReplaceEvent::ReplaceAll
            | FindReplaceEvent::Close => {}
        }
    }

    /// The current value of `toggle`.
    fn toggle_value(&self, toggle: FindReplaceToggle) -> bool {
        match toggle {
            FindReplaceToggle::ShowReplace => self.show_replace,
            FindReplaceToggle::CaseSensitive => self.case_sensitive,
            FindReplaceToggle::WholeWord => self.whole_word,
            FindReplaceToggle::Regex => self.use_regex,
            FindReplaceToggle::PreserveCase => self.preserve_case,
            FindReplaceToggle::InSelection => self.in_selection,
        }
    }

    /// The event flipping `toggle` reports.
    fn flip(&self, toggle: FindReplaceToggle) -> FindReplaceEvent {
        FindReplaceEvent::Toggled {
            toggle,
            on: !self.toggle_value(toggle),
        }
    }

    /// The event a click on `target` reports, with `offset` the char
    /// offset under the pointer for a click in a field.
    fn click_event(&self, target: FindReplaceClickTarget, offset: usize) -> FindReplaceEvent {
        use FindReplaceClickTarget as T;
        let focus_field = |field: FindReplaceField| {
            let text = self.field_text(field).to_string();
            let cursor = offset.min(text.chars().count());
            FindReplaceEvent::FieldEdited {
                field,
                text,
                cursor,
                sel_anchor: None,
            }
        };
        match target {
            T::Chevron => self.flip(FindReplaceToggle::ShowReplace),
            T::FindInput(_) => focus_field(FindReplaceField::Find),
            T::ReplaceInput(_) => focus_field(FindReplaceField::Replace),
            T::ToggleCase => self.flip(FindReplaceToggle::CaseSensitive),
            T::ToggleWholeWord => self.flip(FindReplaceToggle::WholeWord),
            T::ToggleRegex => self.flip(FindReplaceToggle::Regex),
            T::TogglePreserveCase => self.flip(FindReplaceToggle::PreserveCase),
            T::ToggleInSelection => self.flip(FindReplaceToggle::InSelection),
            T::PrevMatch => FindReplaceEvent::PrevMatch,
            T::NextMatch => FindReplaceEvent::NextMatch,
            T::Close => FindReplaceEvent::Close,
            T::ReplaceCurrent => FindReplaceEvent::ReplaceCurrent,
            T::ReplaceAll => FindReplaceEvent::ReplaceAll,
        }
    }

    /// Apply one text-edit `op` to the focused field, reporting the
    /// result when the text, caret or selection moved.
    fn edit_focused(
        &self,
        op: crate::behaviour::text_edit::EditOp,
    ) -> crate::Handled<FindReplaceEvent> {
        use crate::behaviour::text_edit::{apply_flat_edit_op, col_to_byte};
        let field = self.focused_field();
        let text = self.field_text(field);
        let len = text.chars().count();
        let cursor = self.cursor.min(len);
        let anchor = self.sel_anchor.map(|a| a.min(len));
        let (new_text, new_cursor_b, new_anchor_b) = apply_flat_edit_op(
            text,
            col_to_byte(text, cursor),
            anchor.map(|a| col_to_byte(text, a)),
            op,
        );
        let to_chars = |b: usize| new_text[..b].chars().count();
        let new_cursor = to_chars(new_cursor_b);
        let new_anchor = new_anchor_b.map(to_chars).filter(|a| *a != new_cursor);
        if new_text == text && new_cursor == cursor && new_anchor == anchor {
            return crate::Handled::Consumed;
        }
        crate::Handled::Event(FindReplaceEvent::FieldEdited {
            field,
            text: new_text,
            cursor: new_cursor,
            sel_anchor: new_anchor,
        })
    }

    /// The selected text of the focused field, if any.
    fn focused_selection(&self) -> Option<String> {
        use crate::behaviour::text_edit::{col_to_byte, flat_selected_text};
        let text = self.field_text(self.focused_field());
        let anchor = self.sel_anchor.map(|a| col_to_byte(text, a));
        flat_selected_text(text, col_to_byte(text, self.cursor), anchor)
    }

    /// Resolve one key against the panel: Escape closes, Enter steps or
    /// replaces, Tab moves between the two fields while the replace row
    /// shows, Alt+C/W/R/P flip match case, whole word, regex and preserve
    /// case, and every other key edits the focused field through the
    /// shared flat text-edit core.
    fn handle_key(
        &self,
        key: &crate::interactive::KeyEvent,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<FindReplaceEvent> {
        use crate::behaviour::text_edit::{EditIntent, EditOp};
        use crate::event::{Key, NamedKey};
        use crate::Handled;

        let mods = key.modifiers;
        let field = self.focused_field();
        match &key.key {
            Key::Named(NamedKey::Escape) => return Handled::Event(FindReplaceEvent::Close),
            Key::Named(NamedKey::Enter) => {
                return Handled::Event(match field {
                    FindReplaceField::Find if mods.shift => FindReplaceEvent::PrevMatch,
                    FindReplaceField::Find => FindReplaceEvent::NextMatch,
                    FindReplaceField::Replace if mods.ctrl || mods.cmd => {
                        FindReplaceEvent::ReplaceAll
                    }
                    FindReplaceField::Replace => FindReplaceEvent::ReplaceCurrent,
                });
            }
            Key::Named(NamedKey::Tab) => {
                if !self.show_replace {
                    return Handled::Ignored;
                }
                let other = match field {
                    FindReplaceField::Find => FindReplaceField::Replace,
                    FindReplaceField::Replace => FindReplaceField::Find,
                };
                let text = self.field_text(other).to_string();
                let cursor = text.chars().count();
                return Handled::Event(FindReplaceEvent::FieldEdited {
                    field: other,
                    text,
                    cursor,
                    sel_anchor: None,
                });
            }
            Key::Named(NamedKey::Up | NamedKey::Down) => return Handled::Ignored,
            Key::Char(c) if mods.alt && !mods.ctrl && !mods.cmd => {
                let toggle = match c.to_ascii_lowercase() {
                    'c' => Some(FindReplaceToggle::CaseSensitive),
                    'w' => Some(FindReplaceToggle::WholeWord),
                    'r' => Some(FindReplaceToggle::Regex),
                    'p' => Some(FindReplaceToggle::PreserveCase),
                    _ => None,
                };
                if let Some(toggle) = toggle {
                    return Handled::Event(self.flip(toggle));
                }
            }
            _ => {}
        }
        match EditIntent::from_key(&key.key, mods) {
            Some(EditIntent::SelectAll) => self.edit_focused(EditOp::SelectAll),
            Some(EditIntent::Op(op)) => self.edit_focused(op),
            Some(EditIntent::Copy) => {
                if let Some(text) = self.focused_selection() {
                    cx.emit(crate::UiEvent::TextCopied(text));
                }
                Handled::Consumed
            }
            Some(EditIntent::Cut) => match self.focused_selection() {
                Some(text) => {
                    cx.emit(crate::UiEvent::TextCopied(text));
                    self.edit_focused(EditOp::DeleteSelection)
                }
                None => Handled::Consumed,
            },
            Some(EditIntent::Paste) | None => Handled::Ignored,
        }
    }
}

/// Resolved pixel geometry for a [`FindReplacePanel`] on a pixel surface
/// — the popup box, its content corner and its row pitch.
///
/// [`Self::resolve`] places the popup from the panel's
/// [`FindReplacePanel::group_bounds`] the way
/// [`FindReplacePanel::anchored_rect_in`] does; [`Self::for_popup`] reads
/// the same geometry off an already-placed box, which is what
/// [`FindReplaceLayout`] does for the rect a host paints the panel in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FindReplaceMetrics {
    /// Absolute position of the panel's content corner — inside the
    /// popup's own border and [`Style::padding`] inset. Pass straight
    /// into [`FindReplacePanel::hit_test`]'s `content_origin` parameter.
    ///
    /// [`Style::padding`]: crate::Style::padding
    pub content_origin: (f32, f32),
    /// Row pitch/box height every input and toggle/nav/action box is
    /// painted at — [`Style::control_height`], floored at the backend's
    /// `line_height` so an unusually tall chrome font is never clipped.
    /// Pass into [`FindReplacePanel::hit_test`]'s `line_height` parameter.
    ///
    /// [`Style::control_height`]: crate::Style::control_height
    pub row_height: f32,
    /// Absolute bounds of the popup itself, border included.
    pub popup_rect: Rect,
}

impl FindReplaceMetrics {
    /// Resolve `panel`'s pixel geometry anchored at its own
    /// `group_bounds`, given the host's `Style` and its own per-cell
    /// metrics (`cw` = char width, `lh` = line height, both in
    /// surface-native pixels).
    pub fn resolve(
        panel: &FindReplacePanel,
        style: &crate::style::Style,
        cw: f32,
        lh: f32,
    ) -> Self {
        let cw = cw.max(1.0);
        let lh = lh.max(1.0);
        let row_height = style.control_height.max(lh);
        let pad = style.padding;

        let popup_w = panel.panel_width as f32 * cw;
        let row_count = if panel.show_replace { 2.0 } else { 1.0 };
        let popup_h = row_count * row_height + 2.0 * pad;

        let gb = &panel.group_bounds;
        let popup_x = ((gb.x + gb.width) - popup_w - 10.0).max(gb.x);
        let popup_y = gb.y + 2.0;

        Self::for_popup(Rect::new(popup_x, popup_y, popup_w, popup_h), style, lh)
    }

    /// The geometry of a panel painted in `popup`, with `lh` the
    /// surface's line height.
    pub fn for_popup(popup: Rect, style: &crate::style::Style, lh: f32) -> Self {
        let pad = style.padding;
        FindReplaceMetrics {
            content_origin: (popup.x + pad, popup.y + pad),
            row_height: style.control_height.max(lh.max(1.0)),
            popup_rect: popup,
        }
    }
}

/// A [`FindReplacePanel`]'s resolved layout: the box it fills and the
/// projection of its cell-unit [`FrHitRegion`]s into the surface's native
/// units. Paint and hit-test both read this one value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FindReplaceLayout {
    /// The popup box, border included — the rect the panel was laid out
    /// in.
    pub area: Rect,
    /// Absolute position of the content corner, inside the border (one
    /// cell on a cell grid, [`crate::Style::padding`] on a pixel surface).
    pub content_origin: (f32, f32),
    /// Width of one region cell in native units.
    pub char_width: f32,
    /// Row pitch in native units.
    pub row_height: f32,
}

impl FindReplaceLayout {
    /// Lay a panel out filling `area`: `units` picks the cell-grid or
    /// pixel geometry, `char_width`/`line_height` are the surface's own.
    fn resolve(
        area: Rect,
        units: crate::primitive::Units,
        style: &crate::style::Style,
        char_width: f32,
        line_height: f32,
    ) -> Self {
        match units {
            crate::primitive::Units::Cells => FindReplaceLayout {
                area,
                content_origin: (area.x + 1.0, area.y + 1.0),
                char_width: 1.0,
                row_height: 1.0,
            },
            _ => {
                let m = FindReplaceMetrics::for_popup(area, style, line_height);
                FindReplaceLayout {
                    area,
                    content_origin: m.content_origin,
                    char_width: char_width.max(1.0),
                    row_height: m.row_height,
                }
            }
        }
    }

    /// The target under absolute point `(x, y)`.
    pub fn hit_test(&self, panel: &FindReplacePanel, x: f32, y: f32) -> FindReplaceHit {
        panel.hit_test(x, y, self.content_origin, self.char_width, self.row_height)
    }

    /// `region`'s absolute rect.
    pub fn region_rect(&self, region: &FrHitRegion) -> Rect {
        Rect::new(
            self.content_origin.0 + region.col as f32 * self.char_width,
            self.content_origin.1 + region.row as f32 * self.row_height,
            region.width as f32 * self.char_width,
            self.row_height,
        )
    }

    /// The char offset into a field whose region is `region` under
    /// absolute `x`. A pixel field's text starts 4 units in from its box.
    fn offset_at(&self, region: &FrHitRegion, x: f32) -> usize {
        let rect = self.region_rect(region);
        let inset = if self.char_width > 1.0 { 4.0 } else { 0.0 };
        let rel = ((x - rect.x - inset) / self.char_width).max(0.0);
        if self.char_width > 1.0 {
            rel.round() as usize
        } else {
            rel.floor() as usize
        }
    }
}

impl crate::a11y::A11yPrimitive for FindReplacePanel {
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        crate::a11y::A11yInfo::new()
            .with_role(crate::a11y::A11yRole::Form)
            .with_label("Find and replace")
    }

    /// The find field, and the replace field while its row shows.
    fn a11y_children(&self) -> Vec<crate::a11y::A11yChild> {
        use crate::a11y::{A11yChild, A11yInfo, A11yRole};
        let field = |key: &str, label: &str, value: &str| {
            let mut info = A11yInfo::new()
                .with_role(A11yRole::TextInput)
                .with_label(label);
            if !value.is_empty() {
                info = info.with_value(value);
            }
            A11yChild::new(key, info)
        };
        let mut out = vec![field("find", "Find", &self.query)];
        if self.show_replace {
            out.push(field("replace", "Replace", &self.replacement));
        }
        out
    }
}

impl crate::primitive::Primitive for FindReplacePanel {
    type Layout = FindReplaceLayout;
    type Event = FindReplaceEvent;

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::FindReplace
    }

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Fills `rect`, the box [`FindReplacePanel::anchored_rect`] placed.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> FindReplaceLayout {
        FindReplaceLayout::resolve(
            rect,
            ctx.units(),
            ctx.style(),
            ctx.char_width,
            ctx.chrome_line_height(),
        )
    }

    fn paint(
        &self,
        layout: &FindReplaceLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        native_surface_paint::paint(self, surface, env.theme(), layout);
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &FindReplaceLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        paint_cells::paint(cells, self, layout, env.theme());
    }
}

impl crate::interactive::Interactive for FindReplacePanel {
    type State = ();
    type Action = FindReplaceEvent;

    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    /// Every hit region, named by its index in [`FindReplacePanel::hit_regions`].
    fn sub_regions(
        &self,
        layout: &FindReplaceLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let (ox, oy) = (layout.area.x, layout.area.y);
        self.hit_regions
            .iter()
            .enumerate()
            .map(|(i, (region, _))| {
                let r = layout.region_rect(region);
                (
                    Rect::new(r.x - ox, r.y - oy, r.width, r.height),
                    crate::interactive::SubTarget(i as u64),
                )
            })
            .collect()
    }

    /// - A left press takes focus; on a toggle, nav or action button it
    ///   reports that button's event, and in a field it moves focus and
    ///   the caret there ([`FindReplaceEvent::FieldEdited`]).
    /// - Keys: see [`FindReplacePanel::handle_key`]. Committed text (a
    ///   paste, an IME commit) is inserted into the focused field.
    fn handle(
        &self,
        _state: &mut (),
        event: &crate::interactive::RoutedEvent,
        layout: &FindReplaceLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<FindReplaceEvent> {
        use crate::interactive::{PointerKind, RoutedEvent};
        use crate::Handled;

        match event {
            RoutedEvent::Key(key) => self.handle_key(key, cx),
            RoutedEvent::Text(text) => self.edit_focused(
                crate::behaviour::text_edit::EditOp::InsertText(text.clone()),
            ),
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => {
                cx.request_focus(None);
                let (x, y) = (local.x + layout.area.x, local.y + layout.area.y);
                let FindReplaceHit::Target(target) = layout.hit_test(self, x, y) else {
                    return Handled::Consumed;
                };
                let offset = self
                    .hit_regions
                    .iter()
                    .find(|(_, t)| *t == target)
                    .map_or(0, |(region, _)| layout.offset_at(region, x));
                Handled::Event(self.click_event(target, offset))
            }
            _ => Handled::Ignored,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Shared pixel paint
// ─────────────────────────────────────────────────────────────────────
//
// Every pixel surface (GTK/macOS/Win) paints the panel through `paint`
// below, walking `panel.hit_regions` against the panel's
// `FindReplaceLayout`.
//
// A focused input's selection is filled first, at full opacity, and the
// field's text is then drawn in three runs (prefix / selected / suffix) on
// top — the same shape `primitives::form::paint`'s bracketed-text
// selection uses — so the selected characters stay legible on every
// backend. `PaintSurface::surface_fill_rect` doesn't guarantee alpha
// blending (GTK's implementation discards the alpha channel), so an
// opaque fill under the text is the only shape that reads correctly on
// all three pixel backends. It also matches the cell painter, which
// renders a selection as a solid `selection_bg` cell background.
//
// `#[allow(dead_code)]`: exercised through `Primitive::paint` once a pixel
// backend is compiled in, and by this module's own `RecordingSurface`
// tests on every leg that enables one of the three cfg'd features (gtk,
// win, or macos-on-macos) — see `paint_surface.rs`'s module doc for why
// that's not actually dead under `--features win` on a non-Windows host.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
mod native_surface_paint {
    use super::{FindReplaceClickTarget, FindReplaceLayout, FindReplacePanel};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;

    /// Layout constants shared by every helper below, computed once per
    /// [`paint`] call.
    struct Metrics {
        cw: f32,
        /// Backend text line height, in surface-native pixels — used only
        /// for sizing/centering a text run, never for a row's own pitch
        /// (see [`Self::row_h`]).
        lh: f32,
        /// Row pitch/height every input and toggle/nav/action box is
        /// painted at, in surface-native pixels — [`crate::Style::control_height`]
        /// (26px by default), the same "standard single-line control"
        /// metric [`crate::primitives::button::ButtonMetrics`] and
        /// [`crate::primitives::text_input::TextInputMetrics`] resolve a
        /// `Button`/`TextInput`'s own box height from, floored at `lh` so a
        /// backend reporting an unusually tall font never clips its own
        /// text.
        row_h: f32,
        content_x: f32,
        content_y: f32,
    }

    /// A hit-region's bounds in `surface`-absolute coordinates: `col`/
    /// `row` are char-cell offsets from the panel's content corner,
    /// `width` is in char cells. Width is floored to 1 device unit so a
    /// theoretically-zero-width region never produces a degenerate
    /// fill/stroke rect. Row pitch and box height both use [`Metrics::row_h`]
    /// (not [`Metrics::lh`]) so every input/toggle/nav/action box reaches
    /// the full [`crate::Style::control_height`] instead of just the raw chrome
    /// text line height.
    fn region_rect(m: &Metrics, col: u16, row: u16, width: u16) -> Rect {
        Rect::new(
            m.content_x + col as f32 * m.cw,
            m.content_y + row as f32 * m.row_h,
            (width as f32 * m.cw).max(1.0),
            m.row_h,
        )
    }

    /// Vertical offset, in surface-native pixels, that centers an `lh`-tall
    /// text run inside an `m.row_h`-tall box — shared by every text-run
    /// draw call below so a box taller than its own text still centers
    /// its label instead of pinning it to the box's top edge.
    fn center_text_y(m: &Metrics, box_y: f32) -> f32 {
        box_y + ((m.row_h - m.lh) / 2.0).max(0.0)
    }

    /// Paint a plain text label at a cell position — no background, no
    /// centering. Used for the chevron and the match-count string.
    fn paint_label(
        surface: &mut dyn PaintSurface,
        m: &Metrics,
        col: u16,
        row: u16,
        text: &str,
        fg: Color,
    ) {
        let (tw, _) = surface.surface_measure_text(text);
        let px = m.content_x + col as f32 * m.cw;
        let box_y = m.content_y + row as f32 * m.row_h;
        let py = center_text_y(m, box_y);
        surface.surface_draw_text_run(Rect::new(px, py, tw, m.lh), text, fg);
    }

    /// Paint a toggle button (codicon glyph for match-case / whole-word /
    /// regex / preserve-case — TUI paints the plain-text equivalents `Aa`
    /// / `ab` / `.*` / `AB` instead, see `paint_cells`): filled with
    /// `accent_bg` and inverted text when `active`, outlined with
    /// `separator` and normal text otherwise.
    #[allow(clippy::too_many_arguments)]
    fn paint_toggle(
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        m: &Metrics,
        col: u16,
        row: u16,
        width: u16,
        label: &str,
        active: bool,
    ) {
        let rect = region_rect(m, col, row, width);
        let fg = if active {
            surface.surface_fill_rect(rect, theme.accent_bg);
            theme.background
        } else {
            surface.surface_stroke_rect(rect, theme.separator, 0.5);
            theme.foreground
        };
        let (tw, _) = surface.surface_measure_text(label);
        let tx = rect.x + ((rect.width - tw) / 2.0).max(0.0);
        let ty = center_text_y(m, rect.y);
        surface.surface_draw_text_run(Rect::new(tx, ty, tw, m.lh), label, fg);
    }

    /// Paint a nav/dismiss/replace glyph (`↑` `↓` `≡` `×` and the
    /// app-supplied replace-one/replace-all glyphs): filled +
    /// inverted when `active`, plain text with no outline otherwise,
    /// unlike [`paint_toggle`]'s inactive state.
    #[allow(clippy::too_many_arguments)]
    fn paint_glyph(
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        m: &Metrics,
        col: u16,
        row: u16,
        width: u16,
        label: &str,
        active: bool,
    ) {
        let rect = region_rect(m, col, row, width);
        let fg = if active {
            surface.surface_fill_rect(rect, theme.accent_bg);
            theme.background
        } else {
            theme.foreground
        };
        let (tw, _) = surface.surface_measure_text(label);
        let tx = rect.x + ((rect.width - tw) / 2.0).max(0.0);
        let ty = center_text_y(m, rect.y);
        surface.surface_draw_text_run(Rect::new(tx, ty, tw, m.lh), label, fg);
    }

    /// Paint a find/replace input field: background + border, the
    /// field's text, and — only when `is_focused` — the selection
    /// highlight and cursor.
    ///
    /// See this module's doc for why the selection highlight is filled
    /// *before* the text (split into prefix/selected/suffix runs)
    /// rather than as a translucent overlay drawn after a single full
    /// run.
    #[allow(clippy::too_many_arguments)]
    fn paint_input(
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        m: &Metrics,
        col: u16,
        row: u16,
        width: u16,
        text: &str,
        is_focused: bool,
        cursor: usize,
        sel_anchor: Option<usize>,
    ) {
        let rect = region_rect(m, col, row, width);
        surface.surface_fill_rect(rect, theme.background);
        surface.surface_stroke_rect(rect, theme.separator, 0.5);

        let text_x = rect.x + 4.0;
        let text_y = center_text_y(m, rect.y);

        let has_sel = is_focused && sel_anchor.is_some_and(|a| a != cursor) && !text.is_empty();

        if !has_sel {
            let (tw, _) = surface.surface_measure_text(text);
            surface.surface_draw_text_run(
                Rect::new(text_x, text_y, tw, m.lh),
                text,
                theme.foreground,
            );
        } else {
            let anchor = sel_anchor.expect("has_sel implies sel_anchor is Some");
            let (lo, hi) = (anchor.min(cursor), anchor.max(cursor));
            let lo_b = crate::text_util::char_to_byte_idx(text, lo);
            let hi_b = crate::text_util::char_to_byte_idx(text, hi);
            let prefix = &text[..lo_b];
            let selected = &text[lo_b..hi_b];
            let suffix = &text[hi_b..];
            let (pw, _) = surface.surface_measure_text(prefix);
            let (sw, _) = surface.surface_measure_text(selected);
            let (suw, _) = surface.surface_measure_text(suffix);

            // The selection fill spans the full input box height
            // (`rect.height`, `Metrics::row_h`), not just the text line
            // height — the same "highlight the whole control row" shape a
            // native text editor's selection uses.
            surface.surface_fill_rect(
                Rect::new(text_x + pw, rect.y, sw.max(1.0), rect.height),
                theme.selection_bg,
            );
            surface.surface_draw_text_run(
                Rect::new(text_x, text_y, pw, m.lh),
                prefix,
                theme.foreground,
            );
            surface.surface_draw_text_run(
                Rect::new(text_x + pw, text_y, sw, m.lh),
                selected,
                theme.foreground,
            );
            surface.surface_draw_text_run(
                Rect::new(text_x + pw + sw, text_y, suw, m.lh),
                suffix,
                theme.foreground,
            );
        }

        if !is_focused {
            return;
        }

        // Cursor: a 2-device-unit-wide bar at the char offset, inset 2
        // top/bottom within the full input box height.
        let cursor_b = crate::text_util::char_to_byte_idx(text, cursor);
        let (cursor_px, _) = surface.surface_measure_text(&text[..cursor_b]);
        surface.surface_fill_rect(
            Rect::new(
                text_x + cursor_px,
                rect.y + 2.0,
                2.0,
                (rect.height - 4.0).max(1.0),
            ),
            theme.foreground,
        );
    }

    /// Paint `panel` over `layout` using `surface`'s [`PaintSurface`]
    /// verbs.
    ///
    /// Walks `panel.hit_regions` through `layout`'s own projection — the
    /// one [`FindReplaceLayout::hit_test`] reads — so paint and click can
    /// never disagree about where a target lives.
    pub(crate) fn paint(
        panel: &FindReplacePanel,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        layout: &FindReplaceLayout,
    ) {
        use FindReplaceClickTarget as T;

        let lh = surface.surface_line_height().max(1.0);

        surface.surface_fill_rect(layout.area, theme.surface_bg);
        surface.surface_stroke_rect(layout.area, theme.separator, 1.0);

        let m = Metrics {
            cw: layout.char_width,
            lh,
            row_h: layout.row_height,
            content_x: layout.content_origin.0,
            content_y: layout.content_origin.1,
        };

        // Positions derived from neighbouring hit regions, not a hit
        // region itself — the match-count string isn't clickable. The cell
        // painter derives it the same way.
        let mut regex_end_col: Option<u16> = None;
        let mut prev_match_col: Option<u16> = None;

        for (region, target) in &panel.hit_regions {
            match target {
                T::Chevron => {
                    let chevron = if panel.show_replace {
                        "\u{25bc}"
                    } else {
                        "\u{25b6}"
                    };
                    paint_label(
                        surface,
                        &m,
                        region.col,
                        region.row,
                        chevron,
                        theme.foreground,
                    );
                }
                T::FindInput(_) => paint_input(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    &panel.query,
                    panel.focus == 0,
                    panel.cursor,
                    panel.sel_anchor,
                ),
                T::ReplaceInput(_) => paint_input(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    &panel.replacement,
                    panel.focus == 1,
                    panel.cursor,
                    panel.sel_anchor,
                ),
                T::ToggleCase => {
                    let label = crate::codicon::CASE_SENSITIVE.to_string();
                    paint_toggle(
                        surface,
                        theme,
                        &m,
                        region.col,
                        region.row,
                        region.width,
                        &label,
                        panel.case_sensitive,
                    );
                }
                T::ToggleWholeWord => {
                    let label = crate::codicon::WHOLE_WORD.to_string();
                    paint_toggle(
                        surface,
                        theme,
                        &m,
                        region.col,
                        region.row,
                        region.width,
                        &label,
                        panel.whole_word,
                    );
                }
                T::ToggleRegex => {
                    let label = crate::codicon::REGEX.to_string();
                    paint_toggle(
                        surface,
                        theme,
                        &m,
                        region.col,
                        region.row,
                        region.width,
                        &label,
                        panel.use_regex,
                    );
                    regex_end_col = Some(region.col + region.width);
                }
                T::PrevMatch => {
                    paint_glyph(
                        surface,
                        theme,
                        &m,
                        region.col,
                        region.row,
                        region.width,
                        "\u{2191}",
                        false,
                    );
                    prev_match_col.get_or_insert(region.col);
                }
                T::NextMatch => paint_glyph(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    "\u{2193}",
                    false,
                ),
                T::ToggleInSelection => paint_glyph(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    "\u{2261}",
                    panel.in_selection,
                ),
                T::Close => paint_glyph(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    "\u{00d7}",
                    false,
                ),
                T::TogglePreserveCase => {
                    let label = crate::codicon::PRESERVE_CASE.to_string();
                    paint_toggle(
                        surface,
                        theme,
                        &m,
                        region.col,
                        region.row,
                        region.width,
                        &label,
                        panel.preserve_case,
                    );
                }
                T::ReplaceCurrent => paint_glyph(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    &panel.replace_one_glyph,
                    false,
                ),
                T::ReplaceAll => paint_glyph(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    &panel.replace_all_glyph,
                    false,
                ),
            }
        }

        if let (Some(start_col), Some(end_col)) = (regex_end_col, prev_match_col) {
            let info_col = start_col + 1;
            if end_col > info_col + 1 {
                paint_label(
                    surface,
                    &m,
                    info_col,
                    0,
                    &panel.match_info,
                    theme.foreground,
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::find_replace::compute_hit_regions;
        use crate::style::Style;
        use crate::Image;

        /// Records every drawing verb `paint` issues, so a paint
        /// assertion can run on any host — no cairo, Core Graphics or
        /// Direct2D needed. The pixel backends' own probes (real
        /// `ImageSurface`/`BitmapSurface`/`HeadlessSurface` renders)
        /// still cover "the verb reached real pixels"; this covers "the
        /// shared painter emits the right verb at all", on every leg of
        /// the quality gate that enables gtk, win, or macos — not just
        /// whichever one happens to run on this host.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(600.0, 200.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            /// Test-only recorder — `paint` never calls this verb
            /// (see this module's own doc for why no primitive here has been
            /// migrated onto it yet); this exists only so `RecordingSurface`
            /// satisfies the trait. Records into the same `fills` list as
            /// `surface_fill_rect` (radius dropped) — no test asserts on it
            /// today.
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
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn sample_panel(query: &str, cursor: usize, sel_anchor: Option<usize>) -> FindReplacePanel {
            let (hit_regions, _input_width) = compute_hit_regions(50, false, "1 of 3", 2, 2);
            FindReplacePanel {
                id: crate::WidgetId::new("find_replace"),
                query: query.into(),
                replacement: String::new(),
                show_replace: false,
                focus: 0,
                cursor,
                sel_anchor,
                match_info: "1 of 3".into(),
                case_sensitive: false,
                whole_word: false,
                use_regex: false,
                preserve_case: false,
                in_selection: false,
                group_bounds: Rect::new(0.0, 0.0, 600.0, 200.0),
                panel_width: 50,
                replace_one_glyph: "R1".into(),
                replace_all_glyph: "R*".into(),
                hit_regions,
            }
        }

        /// Paint `panel` anchored at its own `group_bounds`, laid out with
        /// `surface`'s metrics and `style` — the geometry a pixel backend
        /// resolves for `panel.anchored_rect`.
        fn paint_resolved(
            panel: &FindReplacePanel,
            surface: &mut RecordingSurface,
            theme: &Theme,
            style: &Style,
        ) {
            let cw = surface.surface_char_width();
            let lh = surface.surface_line_height();
            let m =
                crate::primitives::find_replace::FindReplaceMetrics::resolve(panel, style, cw, lh);
            let layout = FindReplaceLayout {
                area: m.popup_rect,
                content_origin: m.content_origin,
                char_width: cw,
                row_height: m.row_height,
            };
            paint(panel, surface, theme, &layout);
        }

        #[test]
        fn paints_popup_background_and_border() {
            let panel = sample_panel("needle", 6, None);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint_resolved(&panel, &mut surface, &theme, &Style::default());

            assert!(
                surface.fills.iter().any(|(_, c)| *c == theme.surface_bg),
                "expected a surface_bg fill for the popup background, fills were {:?}",
                surface.fills,
            );
            assert!(
                surface
                    .strokes
                    .iter()
                    .any(|(_, c, w)| *c == theme.separator && *w == 1.0),
                "expected a 1.0-wide separator stroke for the popup border, strokes were {:?}",
                surface.strokes,
            );
        }

        /// The find input's own box (the `theme.background` fill
        /// `paint_input` draws before its border stroke) is exactly
        /// [`crate::Style::control_height`] (26px) tall, not the backend's raw
        /// `surface_line_height()` (16.0 on this `RecordingSurface`) —
        /// the same "standard single-line control" height `Button` and
        /// `TextInput` size their own boxes from.
        #[test]
        fn find_input_box_is_control_height_tall() {
            let panel = sample_panel("needle", 6, None);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint_resolved(&panel, &mut surface, &theme, &Style::default());

            let expected_h = Style::default().control_height;
            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| *c == theme.background && (r.height - expected_h).abs() < 0.01),
                "expected the find input's background fill to be {expected_h}px tall \
                 (Style::control_height), fills were {:?}",
                surface.fills,
            );
        }

        /// Toggle buttons are laid out one `Style::control_height`-tall
        /// row apart from the find input (row 0) to the replace row
        /// (row 1), not one raw `surface_line_height()` apart — so the
        /// replace row's own boxes line up with the taller find-row
        /// boxes instead of overlapping them.
        #[test]
        fn replace_row_is_offset_by_control_height_not_line_height() {
            let mut panel = sample_panel("needle", 6, None);
            panel.show_replace = true;
            let (hit_regions, _input_width) = compute_hit_regions(50, true, "1 of 3", 2, 2);
            panel.hit_regions = hit_regions;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint_resolved(&panel, &mut surface, &theme, &Style::default());

            let control_height = Style::default().control_height;
            let find_row_y = surface
                .fills
                .iter()
                .find(|(_, c)| *c == theme.background)
                .map(|(r, _)| r.y)
                .expect("find input background fill recorded");
            let replace_row_y = surface
                .fills
                .iter()
                .filter(|(_, c)| *c == theme.background)
                .map(|(r, _)| r.y)
                .find(|y| (*y - find_row_y).abs() > 0.01)
                .expect("replace input background fill recorded at a distinct row");

            assert!(
                ((replace_row_y - find_row_y) - control_height).abs() < 0.01,
                "expected the replace row to sit {control_height}px below the find row, \
                 found find_row_y={find_row_y}, replace_row_y={replace_row_y}",
            );
        }

        #[test]
        fn active_toggle_paints_accent_bg() {
            let mut panel = sample_panel("needle", 6, None);
            panel.case_sensitive = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint_resolved(&panel, &mut surface, &theme, &Style::default());

            assert!(
                surface.fills.iter().any(|(_, c)| *c == theme.accent_bg),
                "expected an accent_bg fill for the active toggle, fills were {:?}",
                surface.fills,
            );
        }

        /// Verify that the selection fill renders and selected text remains
        /// visible, so it is not obscured by the fill rect.
        #[test]
        fn focused_input_selection_fills_and_still_draws_selected_text() {
            let mut panel = sample_panel("needle", 3, Some(0));
            panel.focus = 0;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint_resolved(&panel, &mut surface, &theme, &Style::default());

            assert!(
                surface.fills.iter().any(|(_, c)| *c == theme.selection_bg),
                "expected a selection_bg fill for the focused input's selection, fills were {:?}",
                surface.fills,
            );
            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == "nee"),
                "selected substring \"nee\" must still be drawn as its own text run \
                 (the fill happens before text, so it can't hide it), text runs were {:?}",
                surface.text_runs,
            );
        }

        /// GUI backends (GTK/macOS/Win — anything routing through this
        /// shared `paint`) render the four toggle buttons as codicon
        /// glyphs, not the TUI's literal text pairs. TUI's own
        /// painter (`paint_cells`) has no `PaintSurface` impl
        /// and keeps painting `"Aa"`/`"ab"`/`".*"`/`"AB"`; this test
        /// covers only the `PaintSurface` path this module owns.
        ///
        /// Beyond "each glyph is painted somewhere", this binds each
        /// glyph to *its own* hit region's projected x — the same
        /// `region_rect`/centering math `paint_toggle` uses — so a slot
        /// mix-up (e.g. the case-sensitive glyph landing in the regex
        /// toggle's rect) would fail here even though every glyph would
        /// still technically appear in `text_runs`.
        #[test]
        fn toggle_buttons_paint_codicon_glyphs_not_literal_text() {
            let mut panel = sample_panel("needle", 6, None);
            panel.show_replace = true;
            let (hit_regions, _input_width) = compute_hit_regions(50, true, "1 of 3", 2, 2);
            panel.hit_regions = hit_regions.clone();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint_resolved(&panel, &mut surface, &theme, &Style::default());

            // Derive content_x the same way `paint` does — from the
            // popup background fill it always records first — rather
            // than re-deriving the popup-placement arithmetic by hand.
            let cw = surface.surface_char_width();
            let popup_rect = surface
                .fills
                .first()
                .map(|(r, _)| *r)
                .expect("popup background fill recorded first");
            let content_x = popup_rect.x + cw;

            let toggles = [
                (
                    FindReplaceClickTarget::ToggleCase,
                    crate::codicon::CASE_SENSITIVE,
                ),
                (
                    FindReplaceClickTarget::ToggleWholeWord,
                    crate::codicon::WHOLE_WORD,
                ),
                (FindReplaceClickTarget::ToggleRegex, crate::codicon::REGEX),
                (
                    FindReplaceClickTarget::TogglePreserveCase,
                    crate::codicon::PRESERVE_CASE,
                ),
            ];

            for (target, glyph) in toggles {
                let (region, _) = hit_regions
                    .iter()
                    .find(|(_, t)| *t == target)
                    .unwrap_or_else(|| panic!("{target:?} region must exist"));
                let label = glyph.to_string();
                let (tw, _) = surface.surface_measure_text(&label);
                let rect_width = region.width as f32 * cw;
                let expected_x =
                    content_x + region.col as f32 * cw + ((rect_width - tw) / 2.0).max(0.0);

                let run = surface
                    .text_runs
                    .iter()
                    .find(|(_, t, _)| t == &label)
                    .unwrap_or_else(|| {
                        panic!(
                            "expected codicon glyph {glyph:?} for {target:?} to be painted, \
                             text runs were {:?}",
                            surface.text_runs
                        )
                    });
                assert!(
                    (run.0.x - expected_x).abs() < 0.01,
                    "{target:?}'s glyph {glyph:?} painted at x={}, expected x={} \
                     (its own region's projected rx0, not a neighbouring toggle's) — \
                     text runs were {:?}",
                    run.0.x,
                    expected_x,
                    surface.text_runs,
                );
            }

            for stale in ["Aa", "ab", ".*", "AB"] {
                assert!(
                    !surface.text_runs.iter().any(|(_, t, _)| t == stale),
                    "GUI toggle buttons must not paint the TUI-only literal {stale:?} \
                     text pair, text runs were {:?}",
                    surface.text_runs,
                );
            }
        }

        /// An unfocused input paints its text but no cursor and no
        /// selection highlight, even if `sel_anchor` is set.
        #[test]
        fn unfocused_input_paints_no_selection_or_cursor() {
            let mut panel = sample_panel("needle", 3, Some(0));
            panel.focus = 1; // no replace row exists, so FindInput (focus 0) is unfocused
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint_resolved(&panel, &mut surface, &theme, &Style::default());

            assert!(
                !surface.fills.iter().any(|(_, c)| *c == theme.selection_bg),
                "unfocused input must not paint a selection highlight, fills were {:?}",
                surface.fills,
            );
            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == "needle"),
                "unfocused input must still paint its full text as one run, text runs were {:?}",
                surface.text_runs,
            );
        }

        /// Verify that multibyte queries with boundary-adjacent or out-of-range
        /// cursor/selection offsets do not panic, since offsets are character-based.
        #[test]
        fn multibyte_query_with_out_of_range_selection_does_not_panic() {
            let panel = sample_panel("café🎉中文", 3, Some(6));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint_resolved(&panel, &mut surface, &theme, &Style::default());
        }
    }
}

// ── #818: hit_test (ungated — no PaintSurface feature needed) ─────────────

#[cfg(test)]
mod hit_test_tests {
    use super::*;
    use crate::style::Style;

    fn panel(show_replace: bool) -> FindReplacePanel {
        let (hit_regions, _input_width) = compute_hit_regions(50, show_replace, "1 of 3", 2, 2);
        FindReplacePanel {
            id: crate::WidgetId::new("find_replace"),
            query: "needle".into(),
            replacement: String::new(),
            show_replace,
            focus: 0,
            cursor: 0,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: Rect::new(0.0, 0.0, 600.0, 200.0),
            panel_width: 50,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions,
        }
    }

    #[test]
    fn hit_test_chevron_at_content_origin() {
        let p = panel(false);
        // Chevron region: col 0..2, row 0 — content_origin (10,10), 8x16 cells.
        let hit = p.hit_test(11.0, 12.0, (10.0, 10.0), 8.0, 16.0);
        assert_eq!(hit, FindReplaceHit::Target(FindReplaceClickTarget::Chevron));
    }

    #[test]
    fn hit_test_toggle_case_on_find_row() {
        let p = panel(false);
        // ToggleCase sits right after the find input; find its column
        // from the computed regions rather than hardcoding it.
        let (region, _) = p
            .hit_regions
            .iter()
            .find(|(_, t)| matches!(t, FindReplaceClickTarget::ToggleCase))
            .expect("ToggleCase region present");
        let x = 10.0 + (region.col as f32 + 0.5) * 8.0;
        let y = 10.0 + (region.row as f32 + 0.5) * 16.0;
        let hit = p.hit_test(x, y, (10.0, 10.0), 8.0, 16.0);
        assert_eq!(
            hit,
            FindReplaceHit::Target(FindReplaceClickTarget::ToggleCase)
        );
    }

    #[test]
    fn hit_test_replace_row_only_present_when_show_replace() {
        let p = panel(true);
        let (region, _) = p
            .hit_regions
            .iter()
            .find(|(_, t)| matches!(t, FindReplaceClickTarget::ReplaceCurrent))
            .expect("ReplaceCurrent region present when show_replace");
        let x = 10.0 + (region.col as f32 + 0.5) * 8.0;
        let y = 10.0 + (region.row as f32 + 0.5) * 16.0;
        assert_eq!(
            p.hit_test(x, y, (10.0, 10.0), 8.0, 16.0),
            FindReplaceHit::Target(FindReplaceClickTarget::ReplaceCurrent)
        );
    }

    #[test]
    fn hit_test_miss_returns_empty() {
        let p = panel(false);
        assert_eq!(
            p.hit_test(0.0, 0.0, (10.0, 10.0), 8.0, 16.0),
            FindReplaceHit::Empty
        );
    }

    #[test]
    fn hit_test_zero_metrics_returns_empty() {
        let p = panel(false);
        assert_eq!(
            p.hit_test(11.0, 12.0, (10.0, 10.0), 0.0, 16.0),
            FindReplaceHit::Empty
        );
    }

    #[test]
    fn hit_test_right_edge_is_exclusive() {
        let p = panel(false);
        let (region, target) = p
            .hit_regions
            .iter()
            .find(|(_, t)| matches!(t, FindReplaceClickTarget::ToggleCase))
            .expect("ToggleCase region present");
        let target = *target;
        let rx1 = 10.0 + (region.col as f32 + region.width as f32) * 8.0;
        let y = 10.0 + (region.row as f32 + 0.5) * 16.0;

        // Exactly on the right edge: must miss this region (half-open
        // interval — `x < rx1`, not `x <= rx1`).
        let hit_at_edge = p.hit_test(rx1, y, (10.0, 10.0), 8.0, 16.0);
        assert_ne!(hit_at_edge, FindReplaceHit::Target(target));

        // A hair inside the edge: must still hit.
        let eps = 0.01;
        let hit_just_inside = p.hit_test(rx1 - eps, y, (10.0, 10.0), 8.0, 16.0);
        assert_eq!(hit_just_inside, FindReplaceHit::Target(target));
    }

    #[test]
    fn hit_test_fractional_char_width_diverges_from_cell_space() {
        let p = panel(false);
        let (region, target) = p
            .hit_regions
            .iter()
            .find(|(_, t)| matches!(t, FindReplaceClickTarget::ToggleCase))
            .expect("ToggleCase region present");
        let target = *target;
        // char_width = 7.5: px and cell-index space diverge (unlike the
        // integer-cw cases above, `col` here is not an integer multiple
        // of 1px). Pick a point inside the region's px rect and confirm
        // it still resolves — exercising the px comparison directly
        // rather than any cell-space shortcut.
        let cw = 7.5_f32;
        let lh = 16.0_f32;
        let rx0 = 10.0 + region.col as f32 * cw;
        let x = rx0 + 0.5;
        let y = 10.0 + (region.row as f32 + 0.5) * lh;
        let hit = p.hit_test(x, y, (10.0, 10.0), cw, lh);
        assert_eq!(hit, FindReplaceHit::Target(target));
    }

    /// [`FindReplaceMetrics::resolve`]'s `content_origin`/`row_height`,
    /// fed straight into [`FindReplacePanel::hit_test`], must resolve a
    /// click on the replace row to [`FindReplaceClickTarget::ReplaceInput`]
    /// — not to the find row above it — when `Style::control_height` is
    /// far from the backend's raw chrome `line_height` (18px vs. 40px
    /// here). A caller that instead passed the backend's own
    /// `line_height` as the row pitch (the bug this seam exists to
    /// prevent) would under-shoot the replace row and mis-hit the find
    /// row's own `ToggleCase` region at this same click point.
    #[test]
    fn metrics_resolve_row_height_feeds_hit_test_to_the_replace_row() {
        let p = panel(true);
        let style = Style::default().with_control_height(40.0);
        let cw = 8.0_f32;
        let lh = 18.0_f32; // backend's raw chrome line height, deliberately far from 40.0
        let metrics = FindReplaceMetrics::resolve(&p, &style, cw, lh);
        assert_eq!(metrics.row_height, 40.0);

        let (region, target) = p
            .hit_regions
            .iter()
            .find(|(_, t)| matches!(t, FindReplaceClickTarget::ReplaceInput(_)))
            .expect("ReplaceInput region present on the replace row");
        let target = *target;
        let (ox, oy) = metrics.content_origin;
        let x = ox + (region.col as f32 + 0.5) * cw;
        let y = oy + (region.row as f32 + 0.5) * metrics.row_height;

        let hit = p.hit_test(x, y, metrics.content_origin, cw, metrics.row_height);
        assert_eq!(hit, FindReplaceHit::Target(target));
    }
}

/// The panel's `Interactive` resolved directly, the leg a binding host
/// takes: geometry of the cell and pixel layouts, the controlled-mode
/// apply step, and the edits a driver test cannot reach cheaply.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::event::{Key, MouseButton, NamedKey, Point};
    use crate::interactive::{
        EventCx, EventHost, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::style::Style;
    use crate::types::Modifiers;
    use crate::Handled;

    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: usize,
        emitted: Vec<crate::UiEvent>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {
            self.focus_requests += 1;
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: crate::PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn panel(show_replace: bool) -> FindReplacePanel {
        let mut p = FindReplacePanel {
            id: WidgetId::new("fr"),
            query: "needle".into(),
            replacement: "pin".into(),
            show_replace,
            focus: 0,
            cursor: 6,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: Rect::new(0.0, 0.0, 80.0, 20.0),
            panel_width: FR_PANEL_WIDTH,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions: Vec::new(),
        };
        p.rebuild_hit_regions();
        p
    }

    fn cell_layout(p: &FindReplacePanel) -> FindReplaceLayout {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let rect = p.anchored_rect_in(Rect::new(0.0, 0.0, 80.0, 20.0), &Style::default(), 1.0, 1.0);
        Primitive::layout(p, &ctx, rect)
    }

    fn run(p: &FindReplacePanel, event: RoutedEvent) -> (Handled<FindReplaceEvent>, MockHost) {
        let layout = cell_layout(p);
        let mut host = MockHost::default();
        let handled = {
            let mut cx = EventCx::new(p.id, &mut host);
            p.handle(&mut (), &event, &layout, &mut cx)
        };
        (handled, host)
    }

    fn key(key: Key, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key,
            modifiers,
            repeat: false,
        })
    }

    fn press_at(p: &FindReplacePanel, x: f32, y: f32) -> RoutedEvent {
        let layout = cell_layout(p);
        RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(x - layout.area.x, y - layout.area.y),
            mods: Modifiers::default(),
        }
    }

    /// The cell box sits where the cell painter has always put it: right
    /// border one cell inside the group's right edge, top border on row 1.
    #[test]
    fn cell_anchor_is_the_top_right_of_the_group() {
        let p = panel(false);
        let r = p.anchored_rect_in(Rect::new(0.0, 0.0, 80.0, 20.0), &Style::default(), 1.0, 1.0);
        assert_eq!(r, Rect::new(29.0, 1.0, 50.0, 3.0));
        let narrow =
            p.anchored_rect_in(Rect::new(0.0, 0.0, 30.0, 20.0), &Style::default(), 1.0, 1.0);
        assert_eq!(narrow.width, 28.0, "clamped to the bounds less two cells");
        assert_eq!(
            panel(true)
                .anchored_rect_in(Rect::new(0.0, 0.0, 80.0, 20.0), &Style::default(), 1.0, 1.0)
                .height,
            4.0
        );
    }

    /// On a pixel surface the anchored box and the layout's content
    /// corner and pitch are the ones `FindReplaceMetrics::resolve` gives.
    #[test]
    fn pixel_layout_matches_the_resolved_metrics() {
        let p = panel(true);
        let style = Style::default();
        let m = FindReplaceMetrics::resolve(&p, &style, 8.0, 16.0);
        let rect = p.anchored_rect_in(Rect::new(0.0, 0.0, 800.0, 400.0), &style, 8.0, 16.0);
        assert_eq!(rect, m.popup_rect);
        let ctx = LayoutCtx::new(16.0, 8.0, false);
        let layout = Primitive::layout(&p, &ctx, rect);
        assert_eq!(layout.content_origin, m.content_origin);
        assert_eq!(layout.row_height, m.row_height);
        assert_eq!(layout.char_width, 8.0);
    }

    #[test]
    fn clicking_each_switch_reports_it_flipped() {
        let p = panel(true);
        let layout = cell_layout(&p);
        for (target, toggle) in [
            (
                FindReplaceClickTarget::Chevron,
                FindReplaceToggle::ShowReplace,
            ),
            (
                FindReplaceClickTarget::ToggleCase,
                FindReplaceToggle::CaseSensitive,
            ),
            (
                FindReplaceClickTarget::ToggleWholeWord,
                FindReplaceToggle::WholeWord,
            ),
            (
                FindReplaceClickTarget::ToggleRegex,
                FindReplaceToggle::Regex,
            ),
            (
                FindReplaceClickTarget::TogglePreserveCase,
                FindReplaceToggle::PreserveCase,
            ),
            (
                FindReplaceClickTarget::ToggleInSelection,
                FindReplaceToggle::InSelection,
            ),
        ] {
            let (region, _) = p.hit_regions.iter().find(|(_, t)| *t == target).unwrap();
            let r = layout.region_rect(region);
            let (handled, host) = run(&p, press_at(&p, r.x, r.y));
            let on = toggle != FindReplaceToggle::ShowReplace;
            assert_eq!(
                handled,
                Handled::Event(FindReplaceEvent::Toggled { toggle, on }),
                "{target:?}"
            );
            assert_eq!(host.focus_requests, 1, "a press takes focus");
        }
    }

    #[test]
    fn clicking_in_the_find_field_moves_the_caret_there() {
        let p = panel(false);
        let layout = cell_layout(&p);
        let (region, _) = &p.hit_regions[1];
        let r = layout.region_rect(region);
        let (handled, _) = run(&p, press_at(&p, r.x + 2.0, r.y));
        assert_eq!(
            handled,
            Handled::Event(FindReplaceEvent::FieldEdited {
                field: FindReplaceField::Find,
                text: "needle".into(),
                cursor: 2,
                sel_anchor: None,
            })
        );
    }

    /// Typing goes through the flat text-edit core in char offsets, so a
    /// multibyte query edits by whole characters.
    #[test]
    fn typing_and_backspace_edit_the_focused_field_by_chars() {
        let mut p = panel(false);
        p.query = "café".into();
        p.cursor = 4;
        let (handled, _) = run(
            &p,
            key(Key::Named(NamedKey::Backspace), Modifiers::default()),
        );
        assert_eq!(
            handled,
            Handled::Event(FindReplaceEvent::FieldEdited {
                field: FindReplaceField::Find,
                text: "caf".into(),
                cursor: 3,
                sel_anchor: None,
            })
        );
        p.cursor = 1;
        let (handled, _) = run(&p, key(Key::Char('h'), Modifiers::default()));
        assert_eq!(
            handled,
            Handled::Event(FindReplaceEvent::FieldEdited {
                field: FindReplaceField::Find,
                text: "chafé".into(),
                cursor: 2,
                sel_anchor: None,
            })
        );
    }

    #[test]
    fn shift_left_selects_and_copy_emits_the_selection() {
        let p = panel(false);
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let (handled, _) = run(&p, key(Key::Named(NamedKey::Left), shift));
        let Handled::Event(event) = handled else {
            panic!("expected an edit, got {handled:?}");
        };
        assert_eq!(
            event,
            FindReplaceEvent::FieldEdited {
                field: FindReplaceField::Find,
                text: "needle".into(),
                cursor: 5,
                sel_anchor: Some(6),
            }
        );
        let mut selected = p.clone();
        selected.apply_event(&event);
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let (handled, host) = run(&selected, key(Key::Char('c'), ctrl));
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(host.emitted, vec![crate::UiEvent::TextCopied("e".into())]);
    }

    #[test]
    fn enter_escape_and_tab_map_to_their_actions() {
        let p = panel(true);
        let none = Modifiers::default();
        let shift = Modifiers {
            shift: true,
            ..none
        };
        let ctrl = Modifiers { ctrl: true, ..none };
        let enter = || Key::Named(NamedKey::Enter);
        assert_eq!(
            run(&p, key(enter(), none)).0,
            Handled::Event(FindReplaceEvent::NextMatch)
        );
        assert_eq!(
            run(&p, key(enter(), shift)).0,
            Handled::Event(FindReplaceEvent::PrevMatch)
        );
        assert_eq!(
            run(&p, key(Key::Named(NamedKey::Escape), none)).0,
            Handled::Event(FindReplaceEvent::Close)
        );
        let (tab, _) = run(&p, key(Key::Named(NamedKey::Tab), none));
        let Handled::Event(tab) = tab else {
            panic!("Tab moves to the replace field");
        };
        let mut on_replace = p.clone();
        on_replace.apply_event(&tab);
        assert_eq!(on_replace.focused_field(), FindReplaceField::Replace);
        assert_eq!(on_replace.cursor, 3);
        assert_eq!(
            run(&on_replace, key(enter(), none)).0,
            Handled::Event(FindReplaceEvent::ReplaceCurrent)
        );
        assert_eq!(
            run(&on_replace, key(enter(), ctrl)).0,
            Handled::Event(FindReplaceEvent::ReplaceAll)
        );
        assert_eq!(
            run(&panel(false), key(Key::Named(NamedKey::Tab), none)).0,
            Handled::Ignored,
            "with no replace row Tab leaves the panel"
        );
    }

    #[test]
    fn alt_letters_flip_the_matching_switches() {
        let p = panel(true);
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        for (c, toggle) in [
            ('c', FindReplaceToggle::CaseSensitive),
            ('w', FindReplaceToggle::WholeWord),
            ('r', FindReplaceToggle::Regex),
            ('p', FindReplaceToggle::PreserveCase),
        ] {
            assert_eq!(
                run(&p, key(Key::Char(c), alt)).0,
                Handled::Event(FindReplaceEvent::Toggled { toggle, on: true })
            );
        }
    }

    /// Hiding the replace row rebuilds the hit regions without it and
    /// pulls focus back to the find field.
    #[test]
    fn hiding_the_replace_row_refocuses_find_and_drops_its_regions() {
        let mut p = panel(true);
        p.focus = 1;
        p.apply_event(&FindReplaceEvent::Toggled {
            toggle: FindReplaceToggle::ShowReplace,
            on: false,
        });
        assert_eq!(p.focus, 0);
        assert_eq!(p.cursor, 6);
        assert!(p.hit_regions.iter().all(|(r, _)| r.row == 0));
    }
}
