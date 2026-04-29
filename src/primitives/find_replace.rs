//! `FindReplace` overlay primitive — the inline find/replace panel
//! that anchors at the top-right of the active editor group.
//!
//! This primitive owns the data shape, hit-region layout, and click-
//! target enum. It is the smallest set of types that lets a backend's
//! rasteriser paint the overlay and route clicks back through a
//! shared dispatch path. The rasteriser itself lives in
//! [`crate::tui::draw_find_replace`] and [`crate::gtk::draw_find_replace`].
//!
//! # Why a primitive (and not just a `StatusBar` variant)
//!
//! The find/replace overlay has multi-row content (find row + optional
//! replace row), input-field cursor + selection, toggle-button states
//! (Aa / ab / .* / preserve-case / in-selection), and clickable nav +
//! action buttons all packed inside a 50-cell-wide bordered box.
//! That doesn't fit any of the simpler primitives — so it gets its
//! own `FindReplacePanel` shape.
//!
//! # Hit-region cell-coordinate contract
//!
//! Hit regions are in **character-cell units** relative to the panel
//! content corner (inside the 1-cell borders). Backends translate to
//! native units when dispatching clicks. This keeps the layout
//! algorithm cell-based and identical across TUI and GTK — backends
//! supply their own pixel-per-cell measurer (`char_width`).

use crate::event::Rect;
use serde::{Deserialize, Serialize};

/// Click target within the find/replace overlay. Backends resolve
/// native coordinates → `FindReplaceClickTarget`, then call the
/// engine's shared `handle_find_replace_click` dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
/// Layout: `[chevron(2)] [input(variable)] [Aa(2+1)][ab(2+1)][.*(2+1)] [count(max(len,5)+1)] [↑(2)][↓(2)][≡(2)][×(2)]`
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

    // Toggle buttons: [Aa(2)gap(1)] [ab(2)gap(1)] [.*(2)gap(1)]
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
/// Backends consume this by walking `hit_regions` (paint per region
/// + click hit-test against the same list).
///
/// Glyph fields (`replace_one_glyph` / `replace_all_glyph`) are
/// app-supplied strings — apps with Nerd Font glyphs pass single-char
/// strings, ASCII apps pass multi-char fallbacks like `"R1"` / `"R*"`.
#[derive(Debug, Clone)]
pub struct FindReplacePanel {
    /// Current query text in the find field.
    pub query: String,
    /// Current replacement text (only shown when `show_replace` is true).
    pub replacement: String,
    /// Whether the replace row is visible.
    pub show_replace: bool,
    /// Which field has focus: 0 = find, 1 = replace.
    pub focus: u8,
    /// Cursor position within the focused field (char offset).
    pub cursor: usize,
    /// Selection anchor in the focused field. When Some, text between anchor and cursor is selected.
    pub sel_anchor: Option<usize>,
    /// "N of M" match count display, or "No results" / empty.
    pub match_info: String,
    /// Toggle button states (find row).
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub use_regex: bool,
    /// Toggle button states (replace row).
    pub preserve_case: bool,
    /// Find in selection mode.
    pub in_selection: bool,
    /// Bounding rect of the active editor group. The overlay positions
    /// itself at the top-right of this rect. Apps with f64 native
    /// rect types convert via `as f32` at construction.
    pub group_bounds: Rect,
    /// Panel width in char cells (used by backends for positioning).
    pub panel_width: u16,
    /// App-supplied glyph string for the "replace current" button.
    /// Single char (Nerd Font) or multi-char (ASCII fallback).
    pub replace_one_glyph: String,
    /// App-supplied glyph string for the "replace all" button.
    pub replace_all_glyph: String,
    /// Hit regions for click handling, in char-cell units relative to
    /// the panel content corner (inside borders). Computed once via
    /// [`compute_hit_regions`] at panel construction.
    pub hit_regions: Vec<(FrHitRegion, FindReplaceClickTarget)>,
}
