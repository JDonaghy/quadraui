//! `FindReplace` overlay primitive — the inline find/replace panel
//! that anchors at the top-right of the active editor group.
//!
//! This primitive owns the data shape, hit-region layout, and click-
//! target enum, plus (as of #809, `NativeSurface` Phase 2b) the shared
//! [`native_surface_paint::paint`] rasteriser every pixel backend
//! (GTK/macOS/Win) calls through `Backend::draw_find_replace`. TUI
//! stays a separate rasteriser, [`crate::tui::draw_find_replace`] —
//! see `native_surface.rs`'s module doc for why TUI never implements
//! `NativeSurface` (a cell grid has no sub-cell `Rect`).
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

// ─────────────────────────────────────────────────────────────────────
// NativeSurface Phase 2b (#809): shared paint implementation
// ─────────────────────────────────────────────────────────────────────
//
// Before this, `gtk::find_replace::draw_find_replace`,
// `macos::find_replace::draw_find_replace` and
// `win::find_replace::draw_find_replace` each independently walked
// `panel.hit_regions` and painted every region with their own cairo /
// CoreGraphics / Direct2D calls (quadraui#785 child #809 — re-measure
// rather than trust the parent audit's 270/220/205 line counts, which
// predate this PR and this repo's own history of two closed-as-wrong
// audit findings, #481/#482).
//
// Behavioural divergence found while unifying — reported per this
// issue's acceptance bar, not resolved silently:
//   - GTK painted a focused input's selection as a 50%-alpha rect
//     drawn *after* the full (unsplit) text run, so the selected
//     characters showed through, tinted.
//   - Windows painted the same highlight as an **opaque** rect, also
//     drawn after the full text run — so on Windows the selected
//     characters were completely hidden underneath it. This reads as
//     a latent rendering bug, not a deliberate style choice.
//   - macOS painted no selection highlight at all — its module doc
//     explicitly called this out as a "Scope omission (follow-up)".
//
// `paint` below resolves this by adopting the same shape
// `primitives::form::paint`'s bracketed-text selection already uses
// (`paint_bracketed_text`, #808): fill the selection rect first, at
// full opacity, then draw the field's text in three runs (prefix /
// selected / suffix) on top, so the selected characters stay legible
// on every backend. `NativeSurface::surface_fill_rect` doesn't
// guarantee alpha blending — GTK's implementation discards the alpha
// channel entirely (`GtkBackend::surface_fill_rect` calls
// `crate::gtk::set_source`, which is `cr.set_source_rgb`) — so an
// opaque fill is the only shape that reads correctly on all three
// pixel backends. It also matches how
// [`crate::tui::draw_find_replace`] already renders this: a solid
// `sel_bg` cell background, never blended.
//
// `#[allow(dead_code)]`: exercised by each backend's own
// `Backend::draw_find_replace` call site once compiled in, and by this
// module's own `RecordingSurface` tests on every leg that enables one
// of the three cfg'd features (gtk, win, or macos-on-macos) — see
// `native_surface.rs`'s module doc for why that's not actually dead
// under `--features win` on a non-Windows host, the same shape this
// module borrows from `primitives::form`'s `native_surface_paint`.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
mod native_surface_paint {
    use super::{FindReplaceClickTarget, FindReplacePanel};
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;

    /// Layout constants shared by every helper below, computed once per
    /// [`paint`] call.
    struct Metrics {
        cw: f32,
        lh: f32,
        content_x: f32,
        content_y: f32,
    }

    /// Convert a **char** offset into `text` (the unit `cursor` /
    /// `sel_anchor` are stored in) to a byte offset safe to slice at.
    /// Out-of-range offsets clamp to `text.len()` — always a valid
    /// slice point — rather than panicking, mirroring the identical
    /// `char_indices().nth(..)` idiom every deleted per-backend copy
    /// used independently.
    fn char_to_byte(text: &str, char_idx: usize) -> usize {
        text.char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(text.len())
    }

    /// A hit-region's bounds in `surface`-absolute coordinates: `col`/
    /// `row` are char-cell offsets from the panel's content corner,
    /// `width` is in char cells. Width is floored to 1 device unit so a
    /// theoretically-zero-width region never produces a degenerate
    /// fill/stroke rect.
    fn region_rect(m: &Metrics, col: u16, row: u16, width: u16) -> Rect {
        Rect::new(
            m.content_x + col as f32 * m.cw,
            m.content_y + row as f32 * m.lh,
            (width as f32 * m.cw).max(1.0),
            m.lh,
        )
    }

    /// Paint a plain text label at a cell position — no background, no
    /// centering. Used for the chevron and the match-count string.
    fn paint_label(
        surface: &mut dyn NativeSurface,
        m: &Metrics,
        col: u16,
        row: u16,
        text: &str,
        fg: Color,
    ) {
        let (tw, _) = surface.surface_measure_text(text);
        let px = m.content_x + col as f32 * m.cw;
        let py = m.content_y + row as f32 * m.lh;
        surface.surface_draw_text_run(Rect::new(px, py, tw, m.lh), text, fg);
    }

    /// Paint a toggle button (`Aa` / `ab` / `.*` / `AB`): filled with
    /// `accent_bg` and inverted text when `active`, outlined with
    /// `separator` and normal text otherwise.
    #[allow(clippy::too_many_arguments)]
    fn paint_toggle(
        surface: &mut dyn NativeSurface,
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
        surface.surface_draw_text_run(Rect::new(tx, rect.y, tw, m.lh), label, fg);
    }

    /// Paint a nav/dismiss/replace glyph (`↑` `↓` `≡` `×` and the
    /// app-supplied replace-one/replace-all glyphs): filled +
    /// inverted when `active`, plain text with no outline otherwise —
    /// every deleted per-backend copy painted no outline here, unlike
    /// [`paint_toggle`]'s inactive state.
    #[allow(clippy::too_many_arguments)]
    fn paint_glyph(
        surface: &mut dyn NativeSurface,
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
        surface.surface_draw_text_run(Rect::new(tx, rect.y, tw, m.lh), label, fg);
    }

    /// Paint a find/replace input field: background + border, the
    /// field's text, and — only when `is_focused` — the selection
    /// highlight and cursor.
    ///
    /// See this module's doc for why the selection highlight is filled
    /// *before* the text (split into prefix/selected/suffix runs)
    /// rather than as a translucent overlay drawn after a single full
    /// run — that's the shape the three deleted per-backend copies
    /// disagreed on.
    #[allow(clippy::too_many_arguments)]
    fn paint_input(
        surface: &mut dyn NativeSurface,
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

        let has_sel = is_focused && sel_anchor.is_some_and(|a| a != cursor) && !text.is_empty();

        if !has_sel {
            let (tw, _) = surface.surface_measure_text(text);
            surface.surface_draw_text_run(
                Rect::new(text_x, rect.y, tw, m.lh),
                text,
                theme.foreground,
            );
        } else {
            let anchor = sel_anchor.expect("has_sel implies sel_anchor is Some");
            let (lo, hi) = (anchor.min(cursor), anchor.max(cursor));
            let lo_b = char_to_byte(text, lo);
            let hi_b = char_to_byte(text, hi);
            let prefix = &text[..lo_b];
            let selected = &text[lo_b..hi_b];
            let suffix = &text[hi_b..];
            let (pw, _) = surface.surface_measure_text(prefix);
            let (sw, _) = surface.surface_measure_text(selected);
            let (suw, _) = surface.surface_measure_text(suffix);

            surface.surface_fill_rect(
                Rect::new(text_x + pw, rect.y, sw.max(1.0), m.lh),
                theme.selection_bg,
            );
            surface.surface_draw_text_run(
                Rect::new(text_x, rect.y, pw, m.lh),
                prefix,
                theme.foreground,
            );
            surface.surface_draw_text_run(
                Rect::new(text_x + pw, rect.y, sw, m.lh),
                selected,
                theme.foreground,
            );
            surface.surface_draw_text_run(
                Rect::new(text_x + pw + sw, rect.y, suw, m.lh),
                suffix,
                theme.foreground,
            );
        }

        if !is_focused {
            return;
        }

        // Cursor: a 2-device-unit-wide bar at the char offset, inset 2
        // top/bottom — same shape all three deleted copies drew.
        let cursor_b = char_to_byte(text, cursor);
        let (cursor_px, _) = surface.surface_measure_text(&text[..cursor_b]);
        surface.surface_fill_rect(
            Rect::new(text_x + cursor_px, rect.y + 2.0, 2.0, (m.lh - 4.0).max(1.0)),
            theme.foreground,
        );
    }

    /// Paint a laid-out [`FindReplacePanel`] using `surface`'s
    /// [`NativeSurface`] verbs.
    ///
    /// Walks `panel.hit_regions` — the same list
    /// [`super::compute_hit_regions`] builds once at panel construction
    /// and every backend's click dispatch hit-tests against — so paint
    /// and click can never disagree about where a target lives.
    pub(crate) fn paint(panel: &FindReplacePanel, surface: &mut dyn NativeSurface, theme: &Theme) {
        use FindReplaceClickTarget as T;

        let cw = surface.surface_char_width().max(1.0);
        let lh = surface.surface_line_height().max(1.0);

        let popup_w = panel.panel_width as f32 * cw;
        let row_count = if panel.show_replace { 2.0 } else { 1.0 };
        let popup_h = (row_count + 2.0) * lh;

        let gb = &panel.group_bounds;
        let popup_x = ((gb.x + gb.width) - popup_w - 10.0).max(gb.x);
        let popup_y = gb.y + 2.0;
        let popup_rect = Rect::new(popup_x, popup_y, popup_w, popup_h);

        surface.surface_fill_rect(popup_rect, theme.surface_bg);
        surface.surface_stroke_rect(popup_rect, theme.separator, 1.0);

        let m = Metrics {
            cw,
            lh,
            content_x: popup_x + cw,
            content_y: popup_y + lh,
        };

        // Positions derived from neighbouring hit regions, not a hit
        // region itself — the match-count string isn't clickable. Same
        // trick every deleted per-backend copy (and TUI) used.
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
                T::ToggleCase => paint_toggle(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    "Aa",
                    panel.case_sensitive,
                ),
                T::ToggleWholeWord => paint_toggle(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    "ab",
                    panel.whole_word,
                ),
                T::ToggleRegex => {
                    paint_toggle(
                        surface,
                        theme,
                        &m,
                        region.col,
                        region.row,
                        region.width,
                        ".*",
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
                T::TogglePreserveCase => paint_toggle(
                    surface,
                    theme,
                    &m,
                    region.col,
                    region.row,
                    region.width,
                    "AB",
                    panel.preserve_case,
                ),
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

        impl NativeSurface for RecordingSurface {
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
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
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

        #[test]
        fn paints_popup_background_and_border() {
            let panel = sample_panel("needle", 6, None);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&panel, &mut surface, &theme);

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

        #[test]
        fn active_toggle_paints_accent_bg() {
            let mut panel = sample_panel("needle", 6, None);
            panel.case_sensitive = true;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&panel, &mut surface, &theme);

            assert!(
                surface.fills.iter().any(|(_, c)| *c == theme.accent_bg),
                "expected an accent_bg fill for the active toggle, fills were {:?}",
                surface.fills,
            );
        }

        /// Regression for the divergence this PR found and resolved
        /// (see the module doc): the selection fill must land *and* the
        /// selected substring must still be drawn as its own text run,
        /// so it stays legible rather than getting hidden the way
        /// Windows's deleted copy hid it.
        #[test]
        fn focused_input_selection_fills_and_still_draws_selected_text() {
            let mut panel = sample_panel("needle", 3, Some(0));
            panel.focus = 0;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&panel, &mut surface, &theme);

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

        /// An unfocused input paints its text but no cursor and no
        /// selection highlight, even if `sel_anchor` is set — matches
        /// every deleted per-backend copy's `if !is_focused { return }`
        /// guard.
        #[test]
        fn unfocused_input_paints_no_selection_or_cursor() {
            let mut panel = sample_panel("needle", 3, Some(0));
            panel.focus = 1; // no replace row exists, so FindInput (focus 0) is unfocused
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&panel, &mut surface, &theme);

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

        /// Regression for issue #503: `cursor`/`sel_anchor` are char
        /// offsets, so a multibyte query with a boundary-adjacent or
        /// out-of-range offset must not panic — twin of the
        /// per-backend `*_with_multibyte_query_does_not_panic` tests
        /// this PR deletes in favour of this one shared assertion.
        #[test]
        fn multibyte_query_with_out_of_range_selection_does_not_panic() {
            let panel = sample_panel("café🎉中文", 3, Some(6));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(&panel, &mut surface, &theme);
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
