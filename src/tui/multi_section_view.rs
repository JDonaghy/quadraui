//! TUI rasteriser for [`crate::MultiSectionView`].
//!
//! Per D6: this function asks the primitive for a
//! [`crate::MultiSectionViewLayout`] using TUI-native metrics (1 cell
//! per header, 1 cell per scrollbar, 1 cell per divider) and paints
//! the resolved positions verbatim. Body content is dispatched to the
//! existing per-primitive rasterisers (`draw_tree`, `draw_list`, etc.)
//! using the body bounds returned by the layout.
//!
//! Vertical-only in v1 (per #294 / D-003 in `quadraui/docs/DECISIONS.md`);
//! horizontal sections fall through to a no-op until the horizontal
//! rasteriser ships.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as TuiRect;

use super::{draw_form, draw_list, draw_message_list, draw_tree, qc, ratatui_color, set_cell};
use crate::event::Rect as QRect;
use crate::primitives::multi_section_view::{
    Axis, EmptyBody, LayoutMetrics, MultiSectionView, SectionAux, SectionBody, SectionHeader,
    SectionMeasure,
};
use crate::theme::Theme;
use crate::types::StyledText;

/// Draw a [`MultiSectionView`] into `area` on `buf`. Body content is
/// dispatched to the appropriate body-primitive rasteriser using the
/// layout's body bounds.
///
/// `nerd_fonts_enabled` is forwarded to body painters that consume it
/// (currently `draw_tree` and `draw_list`).
pub fn draw_multi_section_view(
    buf: &mut Buffer,
    area: TuiRect,
    view: &MultiSectionView,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    if area.width == 0 || area.height == 0 || view.axis == Axis::Horizontal {
        // Horizontal axis is not yet rasterised (#294). Paint nothing
        // rather than draw incorrect chrome — the host gets a visibly
        // empty region and surfaces the gap in their tests.
        return;
    }

    // TUI metrics: 1 cell per header row, 1 cell per scrollbar gutter,
    // 1 cell per divider (only when allow_resize is true; otherwise we
    // omit the strip entirely). `cell_quantum: 1.0` snaps section sizes
    // to whole cells inside `MultiSectionView::layout` so paint
    // (rounded to integer rows) and hit_test (raw fractional bounds)
    // agree by construction.
    let metrics = LayoutMetrics {
        header_size: 1.0,
        divider_size: if view.allow_resize { 1.0 } else { 0.0 },
        scrollbar_size: 1.0,
        cell_quantum: 1.0,
    };

    let bounds = QRect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    );

    // Per-section measure: aux is always 1 cell tall in TUI; content
    // size is the inner body's natural height in cells.
    let measure = |i: usize| -> SectionMeasure {
        let s = &view.sections[i];
        let aux_size = if s.aux.is_some() { 1.0 } else { 0.0 };
        let content_size = body_content_rows(&s.body) as f32;
        SectionMeasure {
            content_size,
            aux_size,
        }
    };

    let layout = view.layout(bounds, metrics, measure);

    let panel_bg = ratatui_color(theme.background);

    let viewport_top = bounds.y;
    let viewport_bottom = bounds.y + bounds.height;

    for s_layout in &layout.sections {
        let section = &view.sections[s_layout.section_idx];

        // Header — clipped against viewport.
        if let Some(clipped) =
            clip_to_viewport(s_layout.header_bounds, viewport_top, viewport_bottom)
        {
            paint_header(buf, clipped, &section.header, section.collapsed, theme);
        }

        if !s_layout.collapsed {
            if let Some(aux_b) = s_layout.aux_bounds {
                if let Some(clipped) = clip_to_viewport(aux_b, viewport_top, viewport_bottom) {
                    if let Some(aux) = &section.aux {
                        paint_aux(buf, clipped, aux, theme);
                    }
                }
            }

            // Body fill — only clear the visible portion.
            if let Some(clipped_body) =
                clip_to_viewport(s_layout.body_bounds, viewport_top, viewport_bottom)
            {
                fill_rect(buf, clipped_body, ' ', panel_bg, panel_bg);
                paint_body(
                    buf,
                    s_layout.body_bounds,
                    clipped_body,
                    &section.body,
                    theme,
                    nerd_fonts_enabled,
                );
            }

            if let Some(sb_b) = s_layout.scrollbar_bounds {
                if let Some(clipped) = clip_to_viewport(sb_b, viewport_top, viewport_bottom) {
                    paint_scrollbar(buf, clipped, theme);
                }
            }
        }
    }

    // Dividers (horizontal stripes between sections when allow_resize).
    if view.allow_resize {
        for d in &layout.dividers {
            paint_divider(buf, d.bounds, theme);
        }
    }

    // Panel-level scrollbar (WholePanel mode when content overflows).
    if let Some(panel_sb) = layout.panel_scrollbar {
        let total_content: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
        paint_panel_scrollbar(buf, panel_sb, view.panel_scroll, total_content, theme);
    }
}

// ── Section paint helpers ──────────────────────────────────────────────────

fn paint_header(
    buf: &mut Buffer,
    bounds: QRect,
    header: &SectionHeader,
    collapsed: bool,
    theme: &Theme,
) {
    let bg = ratatui_color(theme.header_bg);
    let fg = ratatui_color(theme.header_fg);
    let dim = ratatui_color(theme.muted_fg);

    fill_rect(buf, bounds, ' ', fg, bg);

    let row_y = bounds.y.round() as u16;
    if row_y >= buf.area.y + buf.area.height {
        return;
    }

    let left = bounds.x.round() as i32;
    let right = (bounds.x + bounds.width).round() as i32;
    let mut col = left;

    if header.show_chevron {
        // ▾ when expanded, ▸ when collapsed. Match the GTK convention
        // and VSCode's chevron direction.
        let glyph = if collapsed { '▸' } else { '▾' };
        if col < right {
            set_cell(buf, col as u16, row_y, glyph, fg, bg);
            col += 1;
        }
        if col < right {
            set_cell(buf, col as u16, row_y, ' ', fg, bg);
            col += 1;
        }
    }

    // Reserve the trailing slot for action buttons (right-to-left).
    let mut right_cursor = right;
    for action in header.actions.iter().rev() {
        let glyph_chars = action.icon.fallback.chars().count() as i32;
        let action_w = glyph_chars.max(1) + 1; // glyph + 1 space pad
        if right_cursor - action_w < col {
            break;
        }
        let icon_x = right_cursor - action_w + 1; // skip the trailing space
        let action_fg = if action.enabled { fg } else { dim };
        let mut x = icon_x;
        for ch in action.icon.fallback.chars() {
            if x >= right_cursor {
                break;
            }
            set_cell(buf, x as u16, row_y, ch, action_fg, bg);
            x += 1;
        }
        right_cursor -= action_w;
    }

    // Title in the middle. Truncate if it doesn't fit.
    let title_w = (right_cursor - col).max(0);
    if title_w > 0 {
        let mut x = col;
        for span in &header.title.spans {
            let span_fg = span.fg.map(qc).unwrap_or(fg);
            for ch in span.text.chars() {
                if x >= col + title_w {
                    break;
                }
                set_cell(buf, x as u16, row_y, ch, span_fg, bg);
                x += 1;
            }
            if x >= col + title_w {
                break;
            }
        }
        // Badge after title (if room).
        if let Some(badge) = &header.badge {
            // Pad single space, then badge.
            if x + 1 < col + title_w {
                set_cell(buf, x as u16, row_y, ' ', fg, bg);
                x += 1;
                for span in &badge.spans {
                    let span_fg = span.fg.map(qc).unwrap_or(dim);
                    for ch in span.text.chars() {
                        if x >= col + title_w {
                            break;
                        }
                        set_cell(buf, x as u16, row_y, ch, span_fg, bg);
                        x += 1;
                    }
                    if x >= col + title_w {
                        break;
                    }
                }
            }
        }
    }
}

fn paint_aux(buf: &mut Buffer, bounds: QRect, aux: &SectionAux, theme: &Theme) {
    let bg = ratatui_color(theme.input_bg);
    let fg = ratatui_color(theme.foreground);
    let dim = ratatui_color(theme.muted_fg);

    fill_rect(buf, bounds, ' ', fg, bg);
    let row_y = bounds.y.round() as u16;
    let left = bounds.x.round() as i32;
    let right = (bounds.x + bounds.width).round() as i32;
    if row_y >= buf.area.y + buf.area.height || right <= left {
        return;
    }

    match aux {
        SectionAux::Input(input) | SectionAux::Search(input) => {
            let mut col = left;
            // Show the actual text or the placeholder if empty + unfocused.
            if input.text.is_empty() && !input.has_focus {
                if let Some(ph) = &input.placeholder {
                    for ch in ph.chars() {
                        if col >= right {
                            break;
                        }
                        set_cell(buf, col as u16, row_y, ch, dim, bg);
                        col += 1;
                    }
                }
            } else {
                for ch in input.text.chars() {
                    if col >= right {
                        break;
                    }
                    set_cell(buf, col as u16, row_y, ch, fg, bg);
                    col += 1;
                }
                // Caret block: invert the cell at caret column when focused.
                if input.has_focus {
                    let caret_col = left + input.caret as i32;
                    if caret_col >= left && caret_col < right {
                        let cell = &mut buf[(caret_col as u16, row_y)];
                        cell.set_bg(fg).set_fg(bg);
                    }
                }
            }
        }
        SectionAux::Toolbar(actions) => {
            let mut col = left;
            for a in actions {
                let action_fg = if a.enabled { fg } else { dim };
                for ch in a.icon.fallback.chars() {
                    if col >= right {
                        break;
                    }
                    set_cell(buf, col as u16, row_y, ch, action_fg, bg);
                    col += 1;
                }
                if col < right {
                    set_cell(buf, col as u16, row_y, ' ', fg, bg);
                    col += 1;
                }
            }
        }
        SectionAux::Custom(_) => {
            // Host paints in the bounds we already cleared.
        }
    }
}

fn paint_body(
    buf: &mut Buffer,
    full_bounds: QRect,
    visible_bounds: QRect,
    body: &SectionBody,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    let area = q_to_tui_rect(visible_bounds);
    if area.width == 0 || area.height == 0 {
        return;
    }

    // How many TUI rows of the body extend above the viewport — these
    // need to be skipped via the inner primitive's scroll_offset so the
    // visible area shows the right rows. TUI = 1 cell per row, so the
    // skip count equals the clipped-above height.
    let clip_above = (full_bounds.y - visible_bounds.y).abs().round() as usize;

    match body {
        SectionBody::Tree(t) => {
            if clip_above == 0 {
                draw_tree(buf, area, t, theme, nerd_fonts_enabled);
            } else {
                let mut t_clone = t.clone();
                t_clone.scroll_offset = t.scroll_offset.saturating_add(clip_above);
                draw_tree(buf, area, &t_clone, theme, nerd_fonts_enabled);
            }
        }
        SectionBody::List(l) => {
            if clip_above == 0 {
                draw_list(buf, area, l, theme, nerd_fonts_enabled);
            } else {
                let mut l_clone = l.clone();
                l_clone.scroll_offset = l.scroll_offset.saturating_add(clip_above);
                draw_list(buf, area, &l_clone, theme, nerd_fonts_enabled);
            }
        }
        SectionBody::Form(f) => draw_form(buf, area, f, theme),
        SectionBody::MessageList(m) => draw_message_list(buf, area, m, theme.background),
        SectionBody::Terminal(_) => {
            // No standalone Terminal rasteriser today — host paints.
        }
        SectionBody::Text(lines) => paint_text_lines(buf, area, lines, theme),
        SectionBody::Empty(empty) => paint_empty_body(buf, area, empty, theme),
        SectionBody::Custom(_) => {
            // Host paints in the bounds.
        }
    }
}

fn paint_text_lines(buf: &mut Buffer, area: TuiRect, lines: &[StyledText], theme: &Theme) {
    let bg = ratatui_color(theme.background);
    let fg = ratatui_color(theme.foreground);
    for (i, line) in lines.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let y = area.y + i as u16;
        let mut x = area.x;
        for span in &line.spans {
            let span_fg = span.fg.map(qc).unwrap_or(fg);
            let span_bg = span.bg.map(qc).unwrap_or(bg);
            for ch in span.text.chars() {
                if x >= area.x + area.width {
                    break;
                }
                set_cell(buf, x, y, ch, span_fg, span_bg);
                x += 1;
            }
        }
    }
}

fn paint_empty_body(buf: &mut Buffer, area: TuiRect, empty: &EmptyBody, theme: &Theme) {
    let bg = ratatui_color(theme.background);
    let fg = ratatui_color(theme.muted_fg);
    let primary_fg = ratatui_color(theme.foreground);
    let action_fg = ratatui_color(theme.accent_fg);

    if area.height == 0 || area.width == 0 {
        return;
    }

    // Compute total content height: icon (1) + text (1) + hint (1 if any) + action (1 if any).
    let mut lines: Vec<(StyledText, ratatui::style::Color)> = Vec::new();
    if let Some(icon) = &empty.icon {
        lines.push((StyledText::plain(icon.fallback.clone()), primary_fg));
    }
    lines.push((empty.text.clone(), primary_fg));
    if let Some(hint) = &empty.hint {
        lines.push((hint.clone(), fg));
    }
    if let Some(action) = &empty.action {
        // Render as `[ tooltip-or-icon ]`.
        let label = action
            .tooltip
            .clone()
            .unwrap_or_else(|| action.icon.fallback.clone());
        lines.push((StyledText::plain(format!("[ {label} ]")), action_fg));
    }

    let total = lines.len() as u16;
    if total == 0 {
        return;
    }
    let start_y = area.y + area.height.saturating_sub(total) / 2;

    for (i, (line, default_fg)) in lines.iter().enumerate() {
        let y = start_y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        // Center horizontally.
        let line_w = line.visible_width() as u16;
        let x_start = area.x + area.width.saturating_sub(line_w) / 2;
        let mut x = x_start;
        for span in &line.spans {
            let span_fg = span.fg.map(qc).unwrap_or(*default_fg);
            for ch in span.text.chars() {
                if x >= area.x + area.width {
                    break;
                }
                set_cell(buf, x, y, ch, span_fg, bg);
                x += 1;
            }
        }
    }
}

fn paint_scrollbar(buf: &mut Buffer, bounds: QRect, theme: &Theme) {
    let bg = ratatui_color(theme.background);
    let track = ratatui_color(theme.scrollbar_track);
    let thumb = ratatui_color(theme.scrollbar_thumb);

    let x = bounds.x.round() as u16;
    let y_start = bounds.y.round() as u16;
    let height = bounds.height.round() as u16;
    if height == 0 {
        return;
    }

    // Per-section scrollbar (used by `PerSection` mode when an inner
    // body overflows). Default-rendered as a full track with a 1-cell
    // thumb at the top — hosts overlay precise geometry on top via the
    // standalone `Scrollbar` primitive when they have real scroll state.
    for dy in 0..height {
        let cell_y = y_start + dy;
        if cell_y >= buf.area.y + buf.area.height {
            break;
        }
        set_cell(buf, x, cell_y, '░', track, bg);
    }
    if height >= 1 {
        set_cell(buf, x, y_start, '█', thumb, bg);
    }
}

/// Panel-level scrollbar. Computes thumb size + position from the
/// panel-wide `scroll` and `total_content` heights.
fn paint_panel_scrollbar(buf: &mut Buffer, bounds: QRect, scroll: f32, total: f32, theme: &Theme) {
    let bg = ratatui_color(theme.background);
    let track = ratatui_color(theme.scrollbar_track);
    let thumb = ratatui_color(theme.scrollbar_thumb);

    let x = bounds.x.round() as u16;
    let y_start = bounds.y.round() as u16;
    let height = bounds.height.round() as u16;
    if height == 0 || total <= 0.0 {
        return;
    }

    // Track.
    for dy in 0..height {
        let cell_y = y_start + dy;
        if cell_y >= buf.area.y + buf.area.height {
            break;
        }
        set_cell(buf, x, cell_y, '░', track, bg);
    }

    // Thumb position + size.
    let visible_frac = (height as f32 / total).min(1.0);
    let scroll_frac = if total > height as f32 {
        scroll / (total - height as f32)
    } else {
        0.0
    };
    let thumb_h = ((height as f32 * visible_frac).ceil() as u16).max(1);
    let thumb_track = height.saturating_sub(thumb_h);
    let thumb_offset = (thumb_track as f32 * scroll_frac).round() as u16;
    for dy in 0..thumb_h {
        let cell_y = y_start + thumb_offset + dy;
        if cell_y >= y_start + height {
            break;
        }
        if cell_y >= buf.area.y + buf.area.height {
            break;
        }
        set_cell(buf, x, cell_y, '█', thumb, bg);
    }
}

fn paint_divider(buf: &mut Buffer, bounds: QRect, theme: &Theme) {
    let bg = ratatui_color(theme.background);
    let fg = ratatui_color(theme.separator);

    let y = bounds.y.round() as u16;
    let x_start = bounds.x.round() as u16;
    let width = bounds.width.round() as u16;
    for dx in 0..width {
        set_cell(buf, x_start + dx, y, '─', fg, bg);
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Intersect `r` with the y-range `[viewport_top, viewport_bottom)`.
/// Returns `None` when the rect lies entirely outside.
fn clip_to_viewport(r: QRect, viewport_top: f32, viewport_bottom: f32) -> Option<QRect> {
    let r_bottom = r.y + r.height;
    if r.height <= 0.0 || r_bottom <= viewport_top || r.y >= viewport_bottom {
        return None;
    }
    let new_y = r.y.max(viewport_top);
    let new_bottom = r_bottom.min(viewport_bottom);
    let new_h = (new_bottom - new_y).max(0.0);
    if new_h <= 0.0 {
        return None;
    }
    Some(QRect::new(r.x, new_y, r.width, new_h))
}

fn fill_rect(
    buf: &mut Buffer,
    bounds: QRect,
    ch: char,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    let x_start = bounds.x.round() as i32;
    let y_start = bounds.y.round() as i32;
    let x_end = (bounds.x + bounds.width).round() as i32;
    let y_end = (bounds.y + bounds.height).round() as i32;
    let buf_x_end = (buf.area.x + buf.area.width) as i32;
    let buf_y_end = (buf.area.y + buf.area.height) as i32;
    for y in y_start..y_end.min(buf_y_end) {
        for x in x_start..x_end.min(buf_x_end) {
            if x < 0 || y < 0 {
                continue;
            }
            set_cell(buf, x as u16, y as u16, ch, fg, bg);
        }
    }
}

fn q_to_tui_rect(r: QRect) -> TuiRect {
    TuiRect {
        x: r.x.round().max(0.0) as u16,
        y: r.y.round().max(0.0) as u16,
        width: r.width.round().max(0.0) as u16,
        height: r.height.round().max(0.0) as u16,
    }
}

fn body_content_rows(body: &SectionBody) -> usize {
    match body {
        SectionBody::Tree(t) => t.rows.len(),
        SectionBody::List(l) => l.items.len() + if l.title.is_some() { 1 } else { 0 },
        SectionBody::Form(f) => f.fields.len(),
        SectionBody::MessageList(m) => m.rows.iter().map(|r| 1 + r.text.lines().count()).sum(),
        SectionBody::Terminal(_) => 0,
        SectionBody::Text(lines) => lines.len(),
        SectionBody::Empty(_) => 4, // icon + text + hint + action, capped
        SectionBody::Custom(_) => 0,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::multi_section_view::*;
    use crate::types::{Icon, StyledText, WidgetId};

    fn empty_section(id: &str, size: SectionSize) -> Section {
        Section {
            id: id.into(),
            header: SectionHeader {
                title: StyledText::plain(id.to_uppercase()),
                show_chevron: true,
                ..Default::default()
            },
            body: SectionBody::Empty(EmptyBody {
                text: StyledText::plain("No data"),
                ..Default::default()
            }),
            aux: None,
            size,
            collapsed: false,
            min_size: None,
            max_size: None,
        }
    }

    fn view_with(sections: Vec<Section>) -> MultiSectionView {
        MultiSectionView {
            id: WidgetId::new("v"),
            sections,
            active_section: None,
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: false,
            panel_scroll: 0.0,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_chevron_and_uppercase_title_in_header() {
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 30, 6));
        let v = view_with(vec![empty_section("a", SectionSize::EqualShare)]);
        draw_multi_section_view(
            &mut buf,
            TuiRect::new(0, 0, 30, 6),
            &v,
            &Theme::default(),
            false,
        );
        assert_eq!(cell_char(&buf, 0, 0), '▾');
        // Title starts at col 2 (chevron + space).
        let title: String = (2..3).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(title, "A");
    }

    #[test]
    fn horizontal_axis_is_no_op() {
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 30, 6));
        let mut v = view_with(vec![empty_section("a", SectionSize::EqualShare)]);
        v.axis = Axis::Horizontal;
        draw_multi_section_view(
            &mut buf,
            TuiRect::new(0, 0, 30, 6),
            &v,
            &Theme::default(),
            false,
        );
        // Nothing was painted — chevron isn't there.
        assert_ne!(cell_char(&buf, 0, 0), '▾');
    }

    #[test]
    fn action_button_paints_in_rightmost_slot() {
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 30, 6));
        let mut s = empty_section("a", SectionSize::EqualShare);
        s.header.actions = vec![HeaderAction {
            id: "r".into(),
            icon: Icon::new("", "R"),
            tooltip: None,
            enabled: true,
        }];
        let v = view_with(vec![s]);
        draw_multi_section_view(
            &mut buf,
            TuiRect::new(0, 0, 30, 6),
            &v,
            &Theme::default(),
            false,
        );
        // 'R' is painted at column 28 (icon at right - 2 + 1 = right - 1, but our calc
        // uses action_w = glyph_chars + 1 space pad = 2; right_cursor=30; icon_x=29).
        // Let's just scan the last 3 cells of row 0 for an 'R'.
        let tail: String = (27..30).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(tail.contains('R'), "expected 'R' in tail, got {tail:?}");
    }

    #[test]
    fn input_aux_renders_placeholder_when_empty_and_unfocused() {
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 30, 6));
        let mut s = empty_section("sc", SectionSize::EqualShare);
        s.aux = Some(SectionAux::Input(InlineInput {
            id: WidgetId::new("commit"),
            text: String::new(),
            caret: 0,
            placeholder: Some("Commit".into()),
            has_focus: false,
        }));
        let v = view_with(vec![s]);
        draw_multi_section_view(
            &mut buf,
            TuiRect::new(0, 0, 30, 6),
            &v,
            &Theme::default(),
            false,
        );
        // Aux row at y=1.
        let placeholder: String = (0..6).map(|x| cell_char(&buf, x, 1)).collect();
        assert_eq!(placeholder, "Commit");
    }

    #[test]
    fn divider_painted_only_when_resize_allowed() {
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 10, 8));
        let mut v = view_with(vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
        ]);
        // Without allow_resize: no divider strip, sections take 4 rows each.
        draw_multi_section_view(
            &mut buf,
            TuiRect::new(0, 0, 10, 8),
            &v,
            &Theme::default(),
            false,
        );
        // No `─` glyph anywhere.
        for y in 0..8 {
            for x in 0..10 {
                assert_ne!(cell_char(&buf, x, y), '─');
            }
        }

        v.allow_resize = true;
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 10, 9));
        draw_multi_section_view(
            &mut buf,
            TuiRect::new(0, 0, 10, 9),
            &v,
            &Theme::default(),
            false,
        );
        // Should now have a `─` row somewhere.
        let mut found = false;
        for y in 0..9 {
            for x in 0..10 {
                if cell_char(&buf, x, y) == '─' {
                    found = true;
                }
            }
        }
        assert!(found, "expected `─` divider strip");
    }
}
