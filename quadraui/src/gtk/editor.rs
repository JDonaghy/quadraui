//! GTK rasteriser for the [`Editor`] primitive (#276 Stage 1D).
//!
//! Verbatim port of vimcode's `src/gtk/draw::draw_window` body.
//! Handles every editor paint category: backgrounds (active /
//! cursorline / DAP-stopped / diff), pre-text selection overlays
//! (drawn before the text so text reads on top in Cairo's painter
//! order), gutter (BP / git / line numbers right-aligned / diagnostic
//! dot / lightbulb), syntax-highlighted text via Pango attributes,
//! ghost continuation, inline annotations, indent guides, color
//! columns, bracket-match (alpha rect), wavy diagnostic underlines,
//! dotted spell underlines, cursor (Block alpha rect, Bar 2px,
//! Underline 12% line height), AI ghost text after the cursor, and
//! secondary cursors.
//!
//! ## Selection paint ordering vs TUI
//!
//! GTK paints selections **before** text (this rasteriser does this
//! at line 1140-style early). TUI paints selections **after** text
//! (modifies bg on cells whose chars are already in the buffer). The
//! divergence is intrinsic to the surfaces — Cairo paints in order,
//! while ratatui cells coalesce fg+bg+char. The data primitive is
//! shared; the paint approach is not. Don't try to consolidate the
//! orders across rasterisers.
//!
//! ## Status line
//!
//! The per-window status line (lifted Session 241) is **not** painted
//! by this rasteriser. The host shrinks `editor.rect` for the status
//! row before calling `draw_editor` and paints the status line
//! separately afterwards.
//!
//! ## Scrollbars
//!
//! GTK paints both scrollbars **outside** this rasteriser today
//! (vimcode's `draw_window` calls scrollbar paint elsewhere — see
//! `draw_window_scrollbars`). The host preserves that arrangement;
//! `draw_editor` does not paint scrollbars on GTK.

use crate::primitives::editor::{
    CursorShape, DiagnosticSeverity, DiffLine, Editor, EditorLine, EditorSelection, GitLineStatus,
    SelectionKind, StyledSpan,
};
use crate::theme::Theme;
use crate::types::Color;
use gtk4::cairo::Context;
use gtk4::pango::{self, AttrColor, AttrList};
use std::collections::HashMap;

/// Paint an [`Editor`] primitive into the supplied Cairo context.
///
/// `char_width` and `line_height` are surface-native pixel
/// measurements obtained from the host's font metrics — passed in so
/// the host's font/scale settings stay authoritative.
#[allow(clippy::too_many_arguments)]
pub fn draw_editor(
    cr: &Context,
    layout: &pango::Layout,
    font_metrics: &pango::FontMetrics,
    editor: &Editor,
    theme: &Theme,
    char_width: f64,
    line_height: f64,
) {
    let rect = &editor.rect;
    let gutter_width = editor.gutter_char_width as f64 * char_width;
    let h_scroll_offset = editor.scroll_left as f64 * char_width;
    let text_x_offset = rect.x as f64 + gutter_width - h_scroll_offset;

    // ── Window background ──────────────────────────────────────────────
    let bg = if editor.show_active_bg {
        theme.editor_active_background
    } else {
        theme.background
    };
    let (br, bg_g, bb) = cairo_rgb(bg);
    cr.set_source_rgb(br, bg_g, bb);
    cr.rectangle(
        rect.x as f64,
        rect.y as f64,
        rect.width as f64,
        rect.height as f64,
    );
    cr.fill().ok();

    // ── Cursorline / Diff / DAP stopped-line backgrounds ───────────────
    for (view_idx, rl) in editor.lines.iter().enumerate() {
        let y = rect.y as f64 + view_idx as f64 * line_height;
        let bg_color = if rl.is_dap_current {
            Some(theme.dap_stopped_bg)
        } else if let Some(diff_status) = rl.diff_status {
            match diff_status {
                DiffLine::Added => Some(theme.diff_added_bg),
                DiffLine::Removed => Some(theme.diff_removed_bg),
                DiffLine::Padding => Some(theme.diff_padding_bg),
                DiffLine::Same => None,
            }
        } else if rl.is_current_line && editor.is_active && editor.cursorline {
            Some(theme.cursorline_bg)
        } else {
            None
        };
        if let Some(color) = bg_color {
            let (dr, dg, db) = cairo_rgb(color);
            cr.set_source_rgb(dr, dg, db);
            cr.rectangle(rect.x as f64, y, rect.width as f64, line_height);
            cr.fill().ok();
        }
    }

    // ── Selection overlays (drawn before text so text is on top) ───────
    if let Some(sel) = &editor.selection {
        draw_visual_selection(
            cr,
            layout,
            sel,
            &editor.lines,
            rect,
            line_height,
            text_x_offset,
            theme.selection,
            theme.selection_alpha as f64,
        );
    }
    for esel in &editor.extra_selections {
        draw_visual_selection(
            cr,
            layout,
            esel,
            &editor.lines,
            rect,
            line_height,
            text_x_offset,
            theme.selection,
            theme.selection_alpha as f64,
        );
    }
    if let Some(yh) = &editor.yank_highlight {
        draw_visual_selection(
            cr,
            layout,
            yh,
            &editor.lines,
            rect,
            line_height,
            text_x_offset,
            theme.yank_highlight_bg,
            theme.yank_highlight_alpha as f64,
        );
    }

    // ── Gutter (bp + git + line numbers right-aligned + diag/lightbulb) ─
    if editor.gutter_char_width > 0 {
        for (_view_idx, rl) in editor.lines.iter().enumerate() {
            let view_idx = _view_idx;
            let y = rect.y as f64 + view_idx as f64 * line_height;
            let mut char_offset = 0usize;

            // Breakpoint column (leftmost when has_breakpoints).
            if editor.has_breakpoints {
                let bp_ch: String = rl.gutter_text.chars().take(1).collect();
                let bp_color = if rl.is_dap_current || rl.is_breakpoint {
                    theme.diagnostic_error
                } else {
                    theme.line_number_fg
                };
                layout.set_text(&bp_ch);
                layout.set_attributes(None);
                let (br, bg_c, bb) = cairo_rgb(bp_color);
                cr.set_source_rgb(br, bg_c, bb);
                cr.move_to(rect.x as f64 + 3.0, y);
                pangocairo::show_layout(cr, layout);
                char_offset += 1;
            }

            // Git marker column.
            if editor.has_git_diff {
                let git_ch: String = rl.gutter_text.chars().skip(char_offset).take(1).collect();
                let git_color = match rl.git_diff {
                    Some(GitLineStatus::Added) => theme.git_added,
                    Some(GitLineStatus::Modified) => theme.git_modified,
                    Some(GitLineStatus::Deleted) => theme.git_deleted,
                    None => theme.line_number_fg,
                };
                layout.set_text(&git_ch);
                layout.set_attributes(None);
                let (gr, gg, gb) = cairo_rgb(git_color);
                cr.set_source_rgb(gr, gg, gb);
                cr.move_to(rect.x as f64 + char_offset as f64 * char_width + 3.0, y);
                pangocairo::show_layout(cr, layout);
                char_offset += 1;

                // Fold + line numbers portion right-aligned.
                let rest: String = rl.gutter_text.chars().skip(char_offset).collect();
                layout.set_text(&rest);
                layout.set_attributes(None);
            } else if char_offset > 0 {
                let rest: String = rl.gutter_text.chars().skip(char_offset).collect();
                layout.set_text(&rest);
                layout.set_attributes(None);
            } else {
                layout.set_text(&rl.gutter_text);
                layout.set_attributes(None);
            }

            let (num_width, _) = layout.pixel_size();
            let num_x = rect.x as f64 + gutter_width - num_width as f64 - char_width + 3.0;

            let num_color = if editor.is_active && rl.is_current_line {
                theme.line_number_active_fg
            } else {
                theme.line_number_fg
            };
            let (nr, ng, nb) = cairo_rgb(num_color);
            cr.set_source_rgb(nr, ng, nb);
            cr.move_to(num_x, y);
            pangocairo::show_layout(cr, layout);

            // Diagnostic gutter dot (overrides lightbulb when both apply).
            if let Some(severity) = editor.diagnostic_gutter.get(&rl.line_idx) {
                let diag_color = match severity {
                    DiagnosticSeverity::Error => theme.diagnostic_error,
                    DiagnosticSeverity::Warning => theme.diagnostic_warning,
                    DiagnosticSeverity::Information => theme.diagnostic_info,
                    DiagnosticSeverity::Hint => theme.diagnostic_hint,
                };
                let (dr, dg, db) = cairo_rgb(diag_color);
                cr.set_source_rgb(dr, dg, db);
                let dot_r = line_height * 0.2;
                let dot_cx = rect.x as f64 + 3.0 + dot_r;
                let dot_cy = y + line_height * 0.5;
                cr.arc(dot_cx, dot_cy, dot_r, 0.0, 2.0 * std::f64::consts::PI);
                cr.fill().ok();
            } else if !rl.is_wrap_continuation
                && editor.code_action_lines.contains(&rl.line_idx)
                && editor.lightbulb_glyph != '\0'
            {
                let (lr, lg, lb) = cairo_rgb(theme.lightbulb);
                cr.set_source_rgb(lr, lg, lb);
                let bulb_layout = layout.clone();
                bulb_layout.set_text(&editor.lightbulb_glyph.to_string());
                cr.move_to(rect.x as f64 + 1.0, y);
                pangocairo::show_layout(cr, &bulb_layout);
            }
        }
    }

    // ── Clip to text area (excludes gutter) ────────────────────────────
    cr.save().ok();
    cr.rectangle(
        rect.x as f64 + gutter_width,
        rect.y as f64,
        rect.width as f64 - gutter_width,
        rect.height as f64,
    );
    cr.clip();

    // ── Render each visible line ───────────────────────────────────────
    for (view_idx, rl) in editor.lines.iter().enumerate() {
        let y = rect.y as f64 + view_idx as f64 * line_height;

        layout.set_text(&rl.raw_text);
        let attrs = build_pango_attrs(&rl.spans);
        layout.set_attributes(Some(&attrs));

        let (fr, fg_g, fb) = cairo_rgb(theme.foreground);
        cr.set_source_rgb(fr, fg_g, fb);
        cr.move_to(text_x_offset, y);
        pangocairo::show_layout(cr, layout);

        // Ghost continuation lines — full line in ghost colour.
        if rl.is_ghost_continuation {
            if let Some(ghost) = &rl.ghost_suffix {
                let (gr, gg, gb) = cairo_rgb(theme.ghost_text_fg);
                cr.set_source_rgb(gr, gg, gb);
                cr.move_to(text_x_offset, y);
                layout.set_text(ghost);
                layout.set_attributes(None);
                pangocairo::show_layout(cr, layout);
            }
        }

        // Inline annotation / virtual text (e.g. git blame).
        if let Some(ann) = &rl.annotation {
            let text_pixel_width = layout.pixel_size().0 as f64;
            let ann_x = text_x_offset + text_pixel_width + char_width * 2.0;
            let (ar, ag, ab) = cairo_rgb(theme.annotation_fg);
            cr.set_source_rgb(ar, ag, ab);
            cr.move_to(ann_x, y);
            layout.set_text(ann);
            layout.set_attributes(None);
            pangocairo::show_layout(cr, layout);
        }

        // Indent guides: thin vertical lines at each guide column.
        if !rl.indent_guides.is_empty() {
            cr.set_line_width(1.0);
            for &guide_col in &rl.indent_guides {
                let is_active = editor.active_indent_col == Some(guide_col);
                let (gr, gg, gb) = if is_active {
                    cairo_rgb(theme.indent_guide_active_fg)
                } else {
                    cairo_rgb(theme.indent_guide_fg)
                };
                cr.set_source_rgb(gr, gg, gb);
                let gx = text_x_offset + guide_col as f64 * char_width;
                cr.move_to(gx, y);
                cr.line_to(gx, y + line_height);
                cr.stroke().ok();
            }
        }

        // Color columns: tinted background rectangle at each column.
        if !rl.colorcolumns.is_empty() {
            let (cr2, cg, cb) = cairo_rgb(theme.colorcolumn_bg);
            cr.set_source_rgb(cr2, cg, cb);
            for &cc_col in &rl.colorcolumns {
                let cx = text_x_offset + cc_col as f64 * char_width;
                cr.rectangle(cx, y, char_width, line_height);
                cr.fill().ok();
            }
        }

        // Bracket match highlighting (semi-transparent rect).
        for &(bm_view_line, bm_col) in &editor.bracket_match_positions {
            if bm_view_line == view_idx {
                let (br, bg_c, bb) = cairo_rgb(theme.bracket_match_bg);
                cr.set_source_rgba(br, bg_c, bb, 0.6);
                let bx = text_x_offset + bm_col as f64 * char_width;
                cr.rectangle(bx, y, char_width, line_height);
                cr.fill().ok();
            }
        }

        // Restore layout to match rendered text (needed for correct
        // index_to_pos when font_scale != 1.0, e.g. markdown headings).
        layout.set_text(&rl.raw_text);
        let line_attrs = build_pango_attrs(&rl.spans);
        layout.set_attributes(Some(&line_attrs));

        // Diagnostic underlines (wavy squiggle).
        for dm in &rl.diagnostics {
            let diag_color = match dm.severity {
                DiagnosticSeverity::Error => theme.diagnostic_error,
                DiagnosticSeverity::Warning => theme.diagnostic_warning,
                DiagnosticSeverity::Information => theme.diagnostic_info,
                DiagnosticSeverity::Hint => theme.diagnostic_hint,
            };
            let (dr, dg, db) = cairo_rgb(diag_color);
            cr.set_source_rgb(dr, dg, db);
            cr.set_line_width(1.0);

            let start_byte = rl
                .raw_text
                .char_indices()
                .nth(dm.start_col)
                .map(|(i, _)| i)
                .unwrap_or(rl.raw_text.len());
            let end_byte = rl
                .raw_text
                .char_indices()
                .nth(dm.end_col)
                .map(|(i, _)| i)
                .unwrap_or(rl.raw_text.len());

            let start_pos = layout.index_to_pos(start_byte as i32);
            let end_pos = layout.index_to_pos(end_byte as i32);
            let x0 = text_x_offset + start_pos.x() as f64 / pango::SCALE as f64;
            let x1 = text_x_offset + end_pos.x() as f64 / pango::SCALE as f64;
            let underline_y = y + line_height - 2.0;

            let wave_h = 1.5;
            let wave_len = 4.0;
            cr.move_to(x0, underline_y);
            let mut wx = x0;
            let mut up = true;
            while wx < x1 {
                let next_x = (wx + wave_len).min(x1);
                let cy = if up {
                    underline_y - wave_h
                } else {
                    underline_y + wave_h
                };
                cr.curve_to(
                    wx + (next_x - wx) * 0.5,
                    cy,
                    wx + (next_x - wx) * 0.5,
                    cy,
                    next_x,
                    underline_y,
                );
                wx = next_x;
                up = !up;
            }
            cr.stroke().ok();
        }

        // Spell error underlines (dotted).
        for sm in &rl.spell_errors {
            let (sr, sg, sb) = cairo_rgb(theme.spell_error);
            cr.set_source_rgb(sr, sg, sb);
            cr.set_line_width(1.0);

            let start_byte = rl
                .raw_text
                .char_indices()
                .nth(sm.start_col)
                .map(|(i, _)| i)
                .unwrap_or(rl.raw_text.len());
            let end_byte = rl
                .raw_text
                .char_indices()
                .nth(sm.end_col)
                .map(|(i, _)| i)
                .unwrap_or(rl.raw_text.len());

            let start_pos = layout.index_to_pos(start_byte as i32);
            let end_pos = layout.index_to_pos(end_byte as i32);
            let x0 = text_x_offset + start_pos.x() as f64 / pango::SCALE as f64;
            let x1 = text_x_offset + end_pos.x() as f64 / pango::SCALE as f64;
            let underline_y = y + line_height - 2.0;

            let dot_spacing = 3.0;
            let mut dx = x0;
            while dx < x1 {
                cr.rectangle(dx, underline_y, 1.0, 1.0);
                dx += dot_spacing;
            }
            cr.fill().ok();
        }
    }

    cr.restore().ok();

    // ── Cursor (Block alpha rect / Bar 2px / Underline 12% line height) ─
    if let Some(cursor) = &editor.cursor {
        if let Some(rl) = editor.lines.get(cursor.pos.view_line) {
            layout.set_text(&rl.raw_text);
            let cursor_attrs = build_pango_attrs(&rl.spans);
            layout.set_attributes(Some(&cursor_attrs));

            let render_col =
                if !editor.extra_selections.is_empty() && cursor.shape == CursorShape::Bar {
                    cursor.pos.col + 1
                } else {
                    cursor.pos.col
                };
            let byte_offset: usize = rl
                .raw_text
                .char_indices()
                .nth(render_col)
                .map(|(i, _)| i)
                .unwrap_or(rl.raw_text.len());

            let pos = layout.index_to_pos(byte_offset as i32);
            let cursor_x = text_x_offset + pos.x() as f64 / pango::SCALE as f64;
            let raw_w = pos.width() as f64 / pango::SCALE as f64;
            let cursor_y = rect.y as f64 + cursor.pos.view_line as f64 * line_height;

            let (cr_r, cr_g, cr_b) = cairo_rgb(theme.cursor);
            let char_w = if raw_w > 0.0 {
                raw_w
            } else {
                font_metrics.approximate_char_width() as f64 / pango::SCALE as f64
            };
            match cursor.shape {
                CursorShape::Block => {
                    cr.set_source_rgba(cr_r, cr_g, cr_b, theme.cursor_normal_alpha as f64);
                    cr.rectangle(cursor_x, cursor_y, char_w, line_height);
                    cr.fill().ok();
                }
                CursorShape::Bar => {
                    cr.set_source_rgb(cr_r, cr_g, cr_b);
                    cr.rectangle(cursor_x, cursor_y, 2.0, line_height);
                    cr.fill().ok();
                }
                CursorShape::Underline => {
                    cr.set_source_rgb(cr_r, cr_g, cr_b);
                    let bar_h = (line_height * 0.12).max(2.0);
                    cr.rectangle(cursor_x, cursor_y + line_height - bar_h, char_w, bar_h);
                    cr.fill().ok();
                }
            }
        }
    }

    // ── AI ghost text (after cursor on cursor line) ────────────────────
    if let Some(cursor) = &editor.cursor {
        if let Some(rl) = editor.lines.get(cursor.pos.view_line) {
            if let Some(ghost) = &rl.ghost_suffix {
                layout.set_text(&rl.raw_text);
                let ghost_line_attrs = build_pango_attrs(&rl.spans);
                layout.set_attributes(Some(&ghost_line_attrs));
                let byte_offset: usize = rl
                    .raw_text
                    .char_indices()
                    .nth(cursor.pos.col)
                    .map(|(i, _)| i)
                    .unwrap_or(rl.raw_text.len());
                let pos = layout.index_to_pos(byte_offset as i32);
                let ghost_x = text_x_offset + pos.x() as f64 / pango::SCALE as f64;
                let ghost_y = rect.y as f64 + cursor.pos.view_line as f64 * line_height;
                let (gr, gg, gb) = cairo_rgb(theme.ghost_text_fg);
                cr.set_source_rgb(gr, gg, gb);
                cr.move_to(ghost_x, ghost_y);
                layout.set_text(ghost);
                layout.set_attributes(None);
                pangocairo::show_layout(cr, layout);
            }
        }
    }

    // ── Secondary cursors (multi-cursor) ───────────────────────────────
    let extra_cursor_shape = editor
        .cursor
        .as_ref()
        .map(|c| c.shape)
        .unwrap_or(CursorShape::Bar);
    let has_extra_sels = !editor.extra_selections.is_empty();
    let fallback_char_w = font_metrics.approximate_char_width() as f64 / pango::SCALE as f64;
    let (cr_r, cr_g, cr_b) = cairo_rgb(theme.cursor);
    for extra_pos in &editor.extra_cursors {
        if let Some(rl) = editor.lines.get(extra_pos.view_line) {
            layout.set_text(&rl.raw_text);
            let extra_attrs = build_pango_attrs(&rl.spans);
            layout.set_attributes(Some(&extra_attrs));
            let render_col = if has_extra_sels && extra_cursor_shape == CursorShape::Bar {
                extra_pos.col + 1
            } else {
                extra_pos.col
            };
            let byte_offset: usize = rl
                .raw_text
                .char_indices()
                .nth(render_col)
                .map(|(i, _)| i)
                .unwrap_or(rl.raw_text.len());
            let pos = layout.index_to_pos(byte_offset as i32);
            let ex = text_x_offset + pos.x() as f64 / pango::SCALE as f64;
            let ew = {
                let w = pos.width() as f64 / pango::SCALE as f64;
                if w > 0.0 {
                    w
                } else {
                    fallback_char_w
                }
            };
            let ey = rect.y as f64 + extra_pos.view_line as f64 * line_height;
            match extra_cursor_shape {
                CursorShape::Bar => {
                    cr.set_source_rgb(cr_r, cr_g, cr_b);
                    cr.rectangle(ex, ey, 2.0, line_height);
                    cr.fill().ok();
                }
                CursorShape::Block => {
                    cr.set_source_rgba(cr_r, cr_g, cr_b, theme.cursor_normal_alpha as f64);
                    cr.rectangle(ex, ey, ew, line_height);
                    cr.fill().ok();
                }
                CursorShape::Underline => {
                    cr.set_source_rgb(cr_r, cr_g, cr_b);
                    let bar_h = (line_height * 0.12).max(2.0);
                    cr.rectangle(ex, ey + line_height - bar_h, ew, bar_h);
                    cr.fill().ok();
                }
            }
        }
    }
}

// ─── Private helpers ────────────────────────────────────────────────────

/// Same shape as `quadraui::gtk::cairo_rgb`, kept as a local helper so
/// this module's call sites stay terse.
fn cairo_rgb(c: Color) -> (f64, f64, f64) {
    (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0)
}

/// Convert a `quadraui::Color` into the 16-bit (0..65535) values
/// expected by Pango's `AttrColor` constructors.
fn pango_u16(c: Color) -> (u16, u16, u16) {
    (c.r as u16 * 257, c.g as u16 * 257, c.b as u16 * 257)
}

/// Build a Pango `AttrList` from the editor primitive's byte-range
/// spans. Mirrors `vimcode::gtk::draw::build_pango_attrs`.
fn build_pango_attrs(spans: &[StyledSpan]) -> AttrList {
    let attrs = AttrList::new();
    for span in spans {
        let (fr, fg_g, fb) = pango_u16(span.style.fg);
        let mut fg_attr = AttrColor::new_foreground(fr, fg_g, fb);
        fg_attr.set_start_index(span.start_byte as u32);
        fg_attr.set_end_index(span.end_byte as u32);
        attrs.insert(fg_attr);

        if let Some(bg) = span.style.bg {
            let (br, bg_g, bb) = pango_u16(bg);
            let mut bg_attr = AttrColor::new_background(br, bg_g, bb);
            bg_attr.set_start_index(span.start_byte as u32);
            bg_attr.set_end_index(span.end_byte as u32);
            attrs.insert(bg_attr);
        }
        if span.style.bold {
            let mut w = pango::AttrInt::new_weight(pango::Weight::Bold);
            w.set_start_index(span.start_byte as u32);
            w.set_end_index(span.end_byte as u32);
            attrs.insert(w);
        }
        if span.style.italic {
            let mut s = pango::AttrInt::new_style(pango::Style::Italic);
            s.set_start_index(span.start_byte as u32);
            s.set_end_index(span.end_byte as u32);
            attrs.insert(s);
        }
        if (span.style.font_scale - 1.0).abs() > f32::EPSILON {
            let mut sc = pango::AttrFloat::new_scale(span.style.font_scale as f64);
            sc.set_start_index(span.start_byte as u32);
            sc.set_end_index(span.end_byte as u32);
            attrs.insert(sc);
        }
    }
    attrs
}

/// Paint a visual selection range (Char / Line / Block) onto `cr`.
/// Handles wrap-continuation skipping (chooses the LAST non-skippable
/// view row per buffer line). Mirrors
/// `vimcode::gtk::draw::draw_visual_selection`.
#[allow(clippy::too_many_arguments)]
fn draw_visual_selection(
    cr: &Context,
    layout: &pango::Layout,
    sel: &EditorSelection,
    lines: &[EditorLine],
    rect: &crate::event::Rect,
    line_height: f64,
    text_x_offset: f64,
    color: Color,
    alpha: f64,
) {
    let (sr, sg, sb) = cairo_rgb(color);
    cr.set_source_rgba(sr, sg, sb, alpha);

    // Build buffer line → view row index mapping. For each buffer line
    // pick the LAST non-skippable rendered row (skip wrap continuations,
    // diff padding, and ghost continuations sharing the same line_idx).
    let mut line_to_view: HashMap<usize, usize> = HashMap::new();
    for (view_idx, rl) in lines.iter().enumerate() {
        if rl.is_wrap_continuation || rl.is_ghost_continuation {
            continue;
        }
        if rl.diff_status == Some(DiffLine::Padding) {
            continue;
        }
        line_to_view.insert(rl.line_idx, view_idx);
    }
    match sel.kind {
        SelectionKind::Line => {
            // Highlight ALL visual rows (including wrap continuations) for
            // each selected buffer line so wrapped lines are fully covered.
            for (view_idx, rl) in lines.iter().enumerate() {
                if rl.line_idx >= sel.start_line
                    && rl.line_idx <= sel.end_line
                    && rl.diff_status != Some(DiffLine::Padding)
                    && !rl.is_ghost_continuation
                {
                    let y = rect.y as f64 + view_idx as f64 * line_height;
                    let highlight_width = rect.width as f64 - (text_x_offset - rect.x as f64);
                    cr.rectangle(text_x_offset, y, highlight_width, line_height);
                }
            }
            cr.fill().ok();
        }
        SelectionKind::Char => {
            for line_idx in sel.start_line..=sel.end_line {
                let Some(&view_idx) = line_to_view.get(&line_idx) else {
                    continue;
                };
                let rl = &lines[view_idx];
                let y = rect.y as f64 + view_idx as f64 * line_height;
                let line_text = &rl.raw_text;

                layout.set_text(line_text);
                layout.set_attributes(None);

                if sel.start_line == sel.end_line {
                    let start_byte = line_text
                        .char_indices()
                        .nth(sel.start_col)
                        .map(|(i, _)| i)
                        .unwrap_or(line_text.len());
                    let start_pos = layout.index_to_pos(start_byte as i32);
                    let start_x = text_x_offset + start_pos.x() as f64 / pango::SCALE as f64;

                    let end_col = (sel.end_col + 1).min(line_text.chars().count());
                    let end_byte = line_text
                        .char_indices()
                        .nth(end_col)
                        .map(|(i, _)| i)
                        .unwrap_or(line_text.len());
                    let end_pos = layout.index_to_pos(end_byte as i32);
                    let end_x = text_x_offset + end_pos.x() as f64 / pango::SCALE as f64;

                    cr.rectangle(start_x, y, end_x - start_x, line_height);
                    cr.fill().ok();
                } else if line_idx == sel.start_line {
                    let start_byte = line_text
                        .char_indices()
                        .nth(sel.start_col)
                        .map(|(i, _)| i)
                        .unwrap_or(line_text.len());
                    let start_pos = layout.index_to_pos(start_byte as i32);
                    let start_x = text_x_offset + start_pos.x() as f64 / pango::SCALE as f64;
                    let (line_width, _) = layout.pixel_size();
                    cr.rectangle(
                        start_x,
                        y,
                        text_x_offset + line_width as f64 - start_x,
                        line_height,
                    );
                    cr.fill().ok();
                } else if line_idx == sel.end_line {
                    let end_col = (sel.end_col + 1).min(line_text.chars().count());
                    let end_byte = line_text
                        .char_indices()
                        .nth(end_col)
                        .map(|(i, _)| i)
                        .unwrap_or(line_text.len());
                    let end_pos = layout.index_to_pos(end_byte as i32);
                    let end_x = text_x_offset + end_pos.x() as f64 / pango::SCALE as f64;
                    cr.rectangle(text_x_offset, y, end_x - text_x_offset, line_height);
                    cr.fill().ok();
                } else {
                    let (line_width, _) = layout.pixel_size();
                    cr.rectangle(text_x_offset, y, line_width as f64, line_height);
                    cr.fill().ok();
                }
            }
        }
        SelectionKind::Block => {
            for line_idx in sel.start_line..=sel.end_line {
                let Some(&view_idx) = line_to_view.get(&line_idx) else {
                    continue;
                };
                let rl = &lines[view_idx];
                let y = rect.y as f64 + view_idx as f64 * line_height;
                let line_text = &rl.raw_text;
                let line_len = line_text.chars().count();

                layout.set_text(line_text);
                layout.set_attributes(None);

                if sel.start_col < line_len {
                    let start_byte = line_text
                        .char_indices()
                        .nth(sel.start_col)
                        .map(|(i, _)| i)
                        .unwrap_or(line_text.len());
                    let start_pos = layout.index_to_pos(start_byte as i32);
                    let start_x = text_x_offset + start_pos.x() as f64 / pango::SCALE as f64;

                    let block_end_col = (sel.end_col + 1).min(line_len);
                    let end_byte = line_text
                        .char_indices()
                        .nth(block_end_col)
                        .map(|(i, _)| i)
                        .unwrap_or(line_text.len());
                    let end_pos = layout.index_to_pos(end_byte as i32);
                    let end_x = text_x_offset + end_pos.x() as f64 / pango::SCALE as f64;

                    cr.rectangle(start_x, y, end_x - start_x, line_height);
                }
            }
            cr.fill().ok();
        }
    }
}
