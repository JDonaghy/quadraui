//! Cell-grid painter for [`MultiSectionView`]: headers, aux rows,
//! scrollbars and dividers in whole cells, `Text` and `Empty` bodies with
//! this module's own painters, and every other body through its own
//! widget's [`Primitive::paint_cells`].

use super::contract::with_body_paint;
use super::{
    Axis, EmptyBody, MultiSectionView, MultiSectionViewPrimitiveLayout, SectionAux, SectionBody,
    SectionBodyLayout, SectionHeader,
};
use crate::cell_surface::{solid_style, CellStyle, CellSurface};
use crate::event::Rect;
use crate::primitive::Primitive;
use crate::primitives::toolbar::action_text;
use crate::text_util::char_cell_width;
use crate::theme::Theme;
use crate::types::{Color, StyledText};
use crate::PaintEnv;

/// Paint `view` at `layout` onto `cells`. Every row is clipped to the
/// view's bounds, so sections scrolled past either edge paint only their
/// visible rows.
pub(crate) fn paint(
    view: &MultiSectionView,
    layout: &MultiSectionViewPrimitiveLayout,
    cells: &mut dyn CellSurface,
    env: &PaintEnv,
) {
    let theme = env.theme();
    let bounds = layout.chrome.bounds;
    if bounds.width < 1.0 || bounds.height < 1.0 || view.axis == Axis::Horizontal {
        return;
    }
    let top = bounds.y;
    let bottom = bounds.y + bounds.height;
    let clip = |r: Rect| super::layout::clip_rows(r, top, bottom);

    for s in &layout.chrome.sections {
        let idx = s.section_idx;
        let section = &view.sections[idx];
        if let Some(header) = clip(s.header_bounds) {
            paint_header(cells, header, &section.header, section.collapsed, theme);
        }
        if s.collapsed {
            continue;
        }
        if let (Some(aux), Some(aux_b)) = (&section.aux, s.aux_bounds.and_then(clip)) {
            paint_aux(cells, aux_b, aux, theme);
        }
        if let Some(body) = layout.body(idx) {
            fill(
                cells,
                body.clip,
                solid_style(theme.background, theme.background),
            );
            let descriptor = body.body.as_ref().unwrap_or(&section.body);
            match (descriptor, &body.layout) {
                (SectionBody::Text(lines), SectionBodyLayout::Text(rect)) => {
                    paint_text_lines(cells, *rect, lines, theme);
                }
                (SectionBody::Empty(empty), SectionBodyLayout::Empty(rect)) => {
                    paint_empty_body(cells, *rect, empty, theme);
                }
                _ => {
                    let interaction = view.body_interaction(idx, env);
                    let child_env = view.body_env(idx, env, &interaction);
                    with_body_paint!(descriptor, &body.layout, |w, l| Primitive::paint_cells(
                        w, l, cells, &child_env
                    ));
                }
            }
        }
        if let Some(gutter) = s.scrollbar_bounds.and_then(clip) {
            let thumb = s.thumb_bounds.and_then(clip);
            paint_scrollbar(cells, gutter, thumb, theme);
        }
    }

    if view.allow_resize {
        for d in &layout.chrome.dividers {
            paint_divider(cells, d.bounds, theme);
        }
    }

    if let Some(panel) = layout.chrome.panel_scrollbar {
        paint_scrollbar(cells, panel, layout.chrome.panel_scrollbar_thumb, theme);
    }
}

/// `x` rounded to the cell it starts in, or `None` off the left/top edge.
fn cell(v: f32) -> Option<u16> {
    let v = v.round();
    (v >= 0.0 && v <= u16::MAX as f32).then_some(v as u16)
}

/// Fill the whole cells `rect` rounds to.
fn fill(cells: &mut dyn CellSurface, rect: Rect, style: CellStyle) {
    let (x0, y0) = (rect.x.round() as i32, rect.y.round() as i32);
    let x1 = (rect.x + rect.width).round() as i32;
    let y1 = (rect.y + rect.height).round() as i32;
    for y in y0.max(0)..y1 {
        for x in x0.max(0)..x1 {
            cells.put_char(x as u16, y as u16, ' ', style);
        }
    }
}

fn paint_header(
    cells: &mut dyn CellSurface,
    bounds: Rect,
    header: &SectionHeader,
    collapsed: bool,
    theme: &Theme,
) {
    let (fg, bg, dim) = (theme.header_fg, theme.header_bg, theme.muted_fg);
    fill(cells, bounds, solid_style(fg, bg));
    let Some(row) = cell(bounds.y) else {
        return;
    };
    let put = |cells: &mut dyn CellSurface, x: i32, ch: char, fg: Color| {
        if x >= 0 {
            cells.put_char(x as u16, row, ch, solid_style(fg, bg));
        }
    };

    let left = bounds.x.round() as i32;
    let right = (bounds.x + bounds.width).round() as i32;
    let mut col = left;

    if header.show_chevron {
        let glyph = if collapsed { '▸' } else { '▾' };
        for ch in [glyph, ' '] {
            if col < right {
                put(cells, col, ch, fg);
                col += 1;
            }
        }
    }

    // Action glyphs take the trailing slots, right to left, each with one
    // cell of padding before it.
    let mut right_cursor = right;
    for action in header.actions.iter().rev() {
        let glyph = action_text(&action.icon.fallback, None, None);
        let action_w = glyph.chars().count().max(1) as i32 + 1;
        if right_cursor - action_w < col {
            break;
        }
        let action_fg = if action.enabled { fg } else { dim };
        for (x, ch) in (right_cursor - action_w + 1..).zip(glyph.chars()) {
            if x >= right_cursor {
                break;
            }
            put(cells, x, ch, action_fg);
        }
        right_cursor -= action_w;
    }

    let end = right_cursor;
    let mut x = col;
    let spans = |cells: &mut dyn CellSurface, x: &mut i32, text: &StyledText, default: Color| {
        for span in &text.spans {
            let span_fg = span.fg.unwrap_or(default);
            for ch in span.text.chars() {
                if *x >= end {
                    return;
                }
                put(cells, *x, ch, span_fg);
                *x += 1;
            }
        }
    };
    spans(cells, &mut x, &header.title, fg);
    if let Some(badge) = &header.badge {
        if x + 1 < end {
            put(cells, x, ' ', fg);
            x += 1;
            spans(cells, &mut x, badge, dim);
        }
    }
}

fn paint_aux(cells: &mut dyn CellSurface, bounds: Rect, aux: &SectionAux, theme: &Theme) {
    let (fg, bg, dim) = (theme.foreground, theme.input_bg, theme.muted_fg);
    fill(cells, bounds, solid_style(fg, bg));
    let Some(row) = cell(bounds.y) else {
        return;
    };
    let left = bounds.x.round() as i32;
    let right = (bounds.x + bounds.width).round() as i32;
    if right <= left || left < 0 {
        return;
    }
    let mut col = left;
    let put = |cells: &mut dyn CellSurface, col: &mut i32, ch: char, fg: Color| {
        if *col < right {
            cells.put_char(*col as u16, row, ch, solid_style(fg, bg));
            *col += 1;
        }
    };

    match aux {
        SectionAux::Input(input) | SectionAux::Search(input) => {
            if input.text.is_empty() && !input.has_focus {
                for ch in input.placeholder.as_deref().unwrap_or("").chars() {
                    put(cells, &mut col, ch, dim);
                }
            } else {
                for ch in input.text.chars() {
                    put(cells, &mut col, ch, fg);
                }
                // The caret is the cell under it in inverted colours.
                let caret = left + input.caret as i32;
                if input.has_focus && caret < right {
                    let ch = cells.get(caret as u16, row).map_or(' ', |(ch, _)| ch);
                    cells.put_char(caret as u16, row, ch, solid_style(bg, fg));
                }
            }
        }
        SectionAux::Toolbar(actions) => {
            for a in actions {
                let action_fg = if a.enabled { fg } else { dim };
                for ch in action_text(&a.icon.fallback, None, None).chars() {
                    put(cells, &mut col, ch, action_fg);
                }
                put(cells, &mut col, ' ', fg);
            }
        }
        SectionAux::Custom(_) => {}
    }
}

/// One styled line per row from the top of `rect`, wide glyphs taking two
/// cells.
fn paint_text_lines(cells: &mut dyn CellSurface, rect: Rect, lines: &[StyledText], theme: &Theme) {
    let (Some(x0), Some(y0)) = (cell(rect.x), cell(rect.y)) else {
        return;
    };
    let width = rect.width.floor().max(0.0) as u16;
    let height = rect.height.floor().max(0.0) as u16;
    for (i, line) in lines.iter().take(height as usize).enumerate() {
        put_line(
            cells,
            x0,
            x0 + width,
            y0 + i as u16,
            line,
            theme.foreground,
            theme.background,
        );
    }
}

/// The empty state centred in `rect`: icon, message, hint and a
/// `[ label ]` action, each line centred by display width.
fn paint_empty_body(cells: &mut dyn CellSurface, rect: Rect, empty: &EmptyBody, theme: &Theme) {
    let (Some(x0), Some(y0)) = (cell(rect.x), cell(rect.y)) else {
        return;
    };
    let width = rect.width.floor().max(0.0) as u16;
    let height = rect.height.floor().max(0.0) as u16;
    if width == 0 || height == 0 {
        return;
    }
    let mut lines: Vec<(StyledText, Color)> = Vec::new();
    if let Some(icon) = &empty.icon {
        lines.push((StyledText::plain(icon.fallback.clone()), theme.foreground));
    }
    lines.push((empty.text.clone(), theme.foreground));
    if let Some(hint) = &empty.hint {
        lines.push((hint.clone(), theme.muted_fg));
    }
    if let Some(action) = &empty.action {
        let label = action
            .tooltip
            .clone()
            .unwrap_or_else(|| action.icon.fallback.clone());
        lines.push((StyledText::plain(format!("[ {label} ]")), theme.accent_fg));
    }
    let start = y0 + height.saturating_sub(lines.len() as u16) / 2;
    for (i, (line, fg)) in lines.iter().enumerate() {
        let y = start + i as u16;
        if y >= y0 + height {
            break;
        }
        let x = x0 + width.saturating_sub(line.visible_width() as u16) / 2;
        put_line(cells, x, x0 + width, y, line, *fg, theme.background);
    }
}

fn put_line(
    cells: &mut dyn CellSurface,
    x0: u16,
    end: u16,
    y: u16,
    line: &StyledText,
    fg: Color,
    bg: Color,
) {
    let mut x = x0;
    for span in &line.spans {
        let style = solid_style(span.fg.unwrap_or(fg), span.bg.unwrap_or(bg));
        for ch in span.text.chars() {
            if x >= end {
                return;
            }
            let w = char_cell_width(ch);
            if w == 2 && x + 1 < end {
                cells.put_wide(x, y, ch, style);
            } else {
                cells.put_char(x, y, ch, style);
            }
            x += w;
        }
    }
}

/// A one-column scrollbar: a `░` track with a `█` thumb at the bounds the
/// layout resolved, or a one-cell thumb at the top when it resolved none.
fn paint_scrollbar(cells: &mut dyn CellSurface, track: Rect, thumb: Option<Rect>, theme: &Theme) {
    let (Some(x), Some(y0)) = (cell(track.x), cell(track.y)) else {
        return;
    };
    let height = track.height.round() as u16;
    let track_style = solid_style(theme.scrollbar_track, theme.background);
    for y in y0..y0 + height {
        cells.put_char(x, y, '░', track_style);
    }
    let (thumb_y, thumb_h) = match thumb {
        Some(t) => (t.y.round() as u16, t.height.round().max(1.0) as u16),
        None => (y0, 1),
    };
    let thumb_style = solid_style(theme.scrollbar_thumb, theme.background);
    for y in thumb_y..(thumb_y + thumb_h).min(y0 + height) {
        cells.put_char(x, y, '█', thumb_style);
    }
}

fn paint_divider(cells: &mut dyn CellSurface, bounds: Rect, theme: &Theme) {
    let (Some(x0), Some(y)) = (cell(bounds.x), cell(bounds.y)) else {
        return;
    };
    let style = solid_style(theme.separator, theme.background);
    for x in x0..x0 + bounds.width.round() as u16 {
        cells.put_char(x, y, '─', style);
    }
}

#[cfg(test)]
mod tests {
    use super::paint_empty_body;
    use crate::primitives::multi_section_view::*;
    use crate::theme::Theme;
    use crate::tui::BufferCellSurface;
    use crate::types::{Icon, StyledText, WidgetId};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as TuiRect;

    fn q_rect(area: TuiRect) -> crate::event::Rect {
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        )
    }

    /// Paint `view` into `area` on `buf` through its own cell painter;
    /// `nerd_fonts_enabled` reaches the tree and list bodies.
    fn paint_view(
        buf: &mut Buffer,
        area: TuiRect,
        view: &MultiSectionView,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) {
        use crate::primitive::{LayoutCtx, Primitive};
        let ctx = LayoutCtx::new(1.0, 1.0, nerd_fonts_enabled);
        let layout = Primitive::layout(view, &ctx, q_rect(area));
        let style = crate::Style::default();
        view.paint_cells(
            &layout,
            &mut BufferCellSurface::new(buf),
            &crate::PaintEnv::new(theme, &style),
        );
    }

    /// The chrome layout [`paint_view`] paints `view` with in `area`.
    fn chrome_layout(view: &MultiSectionView, area: TuiRect) -> MultiSectionViewLayout {
        cell_msv_layout(view, q_rect(area))
    }

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
        paint_view(
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
        paint_view(
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
        paint_view(
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
        paint_view(
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

    // ── Paint↔click round-trip harness ─────────────────────────────────────
    //
    // Primitives that promise paint/click consistency need empirical
    // verification, not just structural design. These tests paint a
    // `MultiSectionView` into a ratatui `Buffer`, find the cells the
    // rasteriser actually wrote glyphs into, then hit_test those exact
    // coordinates against the same `MultiSectionViewLayout` and assert the
    // hit identifies the painted-into section. If paint and click drift in
    // the TUI rasteriser, one of these fails.

    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::types::SelectionMode;

    /// Paint into `buf` and return the layout the rasteriser used. Hit
    /// tests query the SAME layout instance — that's the source-of-truth
    /// contract the harness verifies. (`paint_view`
    /// internally calls `chrome_layout`; calling it again here returns
    /// an equivalent layout because `chrome_layout` is pure: same
    /// `view` + `area` → identical bounds.)
    fn paint_then_layout(
        buf: &mut Buffer,
        area: TuiRect,
        view: &MultiSectionView,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) -> MultiSectionViewLayout {
        paint_view(buf, area, view, theme, nerd_fonts_enabled);
        chrome_layout(view, area)
    }

    fn tree_section(id: &str, items: &[&str], size: SectionSize) -> Section {
        let rows: Vec<TreeRow> = items
            .iter()
            .enumerate()
            .map(|(i, t)| TreeRow {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain((*t).to_string()),
                badge: None,
                is_expanded: None,
                decoration: Default::default(),
                edit: None,
            })
            .collect();
        Section {
            id: id.into(),
            header: SectionHeader {
                title: StyledText::plain(id.to_uppercase()),
                show_chevron: false,
                ..Default::default()
            },
            body: SectionBody::Tree(TreeView {
                id: WidgetId::new(format!("{}-tree", id)),
                rows,
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                items_window: None,
                scroll_px: None,
                style: Default::default(),
                has_focus: true,
            }),
            aux: None,
            size,
            collapsed: false,
            min_size: None,
            max_size: None,
        }
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        (area.x..area.x + area.width)
            .map(|x| cell_char(buf, x, y))
            .collect()
    }

    fn find_row_with(buf: &Buffer, needle: &str) -> Option<u16> {
        let area = buf.area;
        (area.y..area.y + area.height).find(|&y| row_text(buf, y).contains(needle))
    }

    /// Round-trip: paint, find each section's title row in the buffer,
    /// hit_test that coordinate, assert the hit identifies the same
    /// section. Uses a fractional `EqualShare` distribution (4 sections
    /// in 21 cells = 5.25 each) — the worst case for paint/click drift.
    ///
    /// Run at both the origin and a non-zero `area` origin. `chrome_layout`
    /// bakes `area.x`/`area.y` straight into the returned bounds (absolute
    /// frame), and `paint_then_layout` already takes `area` as a parameter,
    /// so this re-runs the identical logic at a shifted origin. `find_row_with`/
    /// `row_text` derive their scan range from `buf.area` (not a hardcoded
    /// rect), so they track the origin too.
    fn header_clicks_land_in_painted_section_under_fractional_distribution_at(
        origin_x: u16,
        origin_y: u16,
    ) {
        let area = TuiRect::new(origin_x, origin_y, 30, 21);
        let mut buf = Buffer::empty(area);
        let v = view_with(vec![
            tree_section("alpha", &["a1", "a2", "a3"], SectionSize::EqualShare),
            tree_section("beta", &["b1", "b2", "b3"], SectionSize::EqualShare),
            tree_section("gamma", &["g1", "g2", "g3"], SectionSize::EqualShare),
            tree_section("delta", &["d1", "d2", "d3"], SectionSize::EqualShare),
        ]);
        let layout = paint_then_layout(&mut buf, area, &v, &Theme::default(), false);

        for s in &layout.sections {
            let needle = v.sections[s.section_idx]
                .header
                .title
                .spans
                .first()
                .map(|sp| sp.text.clone())
                .unwrap_or_default();
            let painted_y = find_row_with(&buf, &needle).unwrap_or_else(|| {
                panic!(
                    "section {} title {:?} was not painted into the buffer",
                    s.section_idx, needle
                )
            });
            let hit = layout.hit_test(area.x as f32 + 5.0, painted_y as f32);
            match hit {
                MultiSectionViewHit::Header { section, .. } => assert_eq!(
                    section, s.section_idx,
                    "row {} paints section {} title {:?} but hit_test returns section {}",
                    painted_y, s.section_idx, needle, section
                ),
                other => panic!(
                    "row {} paints section {} title {:?} but hit_test returns {:?}",
                    painted_y, s.section_idx, needle, other
                ),
            }
        }
    }

    #[test]
    fn header_clicks_land_in_painted_section_under_fractional_distribution() {
        header_clicks_land_in_painted_section_under_fractional_distribution_at(0, 0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn header_clicks_land_in_painted_section_under_fractional_distribution_at_nonzero_origin() {
        header_clicks_land_in_painted_section_under_fractional_distribution_at(7, 13);
    }

    /// Round-trip: paint, find each section's body item rows (rows
    /// containing item glyphs but NOT the title), hit_test those
    /// coordinates, assert each lands in the SAME section's `Body`.
    ///
    /// Run at both the origin and a non-zero `area` origin. See
    /// `header_clicks_land_in_painted_section_under_fractional_distribution_at`
    /// for why coordinate translation via a shifted `area` is necessary.
    fn body_clicks_land_in_painted_section_under_fractional_distribution_at(
        origin_x: u16,
        origin_y: u16,
    ) {
        let area = TuiRect::new(origin_x, origin_y, 30, 21);
        let mut buf = Buffer::empty(area);
        let v = view_with(vec![
            tree_section("alpha", &["a1", "a2", "a3"], SectionSize::EqualShare),
            tree_section("beta", &["b1", "b2", "b3"], SectionSize::EqualShare),
            tree_section("gamma", &["g1", "g2", "g3"], SectionSize::EqualShare),
            tree_section("delta", &["d1", "d2", "d3"], SectionSize::EqualShare),
        ]);
        let layout = paint_then_layout(&mut buf, area, &v, &Theme::default(), false);

        for s in &layout.sections {
            let body_b = s.body_bounds;
            if body_b.height < 1.0 {
                continue;
            }
            // Find an item row painted inside this section's body bounds.
            // body_bounds is in absolute coords; iterate cell rows that
            // lie strictly inside.
            let body_y_start = body_b.y.round() as u16;
            let body_y_end = (body_b.y + body_b.height).round() as u16;
            let body_x_start = body_b.x.round() as u16;
            let body_x_end = (body_b.x + body_b.width).round() as u16;
            let item_prefix = match v.sections[s.section_idx].id.chars().next() {
                Some(c) => c,
                None => continue,
            };
            let mut painted_item_y: Option<u16> = None;
            for y in body_y_start..body_y_end {
                let row = row_text(&buf, y);
                if row.contains(item_prefix)
                    && !row.contains(&v.sections[s.section_idx].id.to_uppercase())
                {
                    painted_item_y = Some(y);
                    break;
                }
            }
            let painted_y = painted_item_y.unwrap_or_else(|| {
                panic!(
                    "section {} ({}) body bounds y={}..{} contained no painted item row",
                    s.section_idx, v.sections[s.section_idx].id, body_y_start, body_y_end
                )
            });
            // Hit_test at a column inside the body bounds.
            let click_x = (body_x_start + body_x_end) as f32 / 2.0;
            let hit = layout.hit_test(click_x, painted_y as f32);
            match hit {
                MultiSectionViewHit::Body { section } => assert_eq!(
                    section, s.section_idx,
                    "row {} paints item in section {} but hit_test returns Body{{section: {}}}",
                    painted_y, s.section_idx, section
                ),
                other => panic!(
                    "row {} paints item in section {} but hit_test returns {:?}",
                    painted_y, s.section_idx, other
                ),
            }
        }
    }

    #[test]
    fn body_clicks_land_in_painted_section_under_fractional_distribution() {
        body_clicks_land_in_painted_section_under_fractional_distribution_at(0, 0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn body_clicks_land_in_painted_section_under_fractional_distribution_at_nonzero_origin() {
        body_clicks_land_in_painted_section_under_fractional_distribution_at(7, 13);
    }

    /// Sections with overflowing content reserve a 1-cell scrollbar
    /// gutter on the body's trailing edge. Clicks in that column must
    /// hit `Scrollbar`, NOT `Body` — otherwise the click would select
    /// an empty body row instead of scrolling. Body and Scrollbar hit
    /// regions must not overlap, and Body must not extend into the
    /// scrollbar column.
    #[test]
    fn scrollbar_column_hits_scrollbar_not_body_when_section_overflows() {
        let area = TuiRect::new(0, 0, 20, 8);
        let mut buf = Buffer::empty(area);
        // 1 section, 10 items in a 6-row body — overflows.
        let v = view_with(vec![tree_section(
            "lots",
            &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"],
            SectionSize::EqualShare,
        )]);
        let layout = paint_then_layout(&mut buf, area, &v, &Theme::default(), false);
        let s = &layout.sections[0];
        let sb = s
            .scrollbar_bounds
            .expect("overflowing section must reserve a scrollbar gutter (paint↔click contract)");
        // Hit_test the centre cell of the scrollbar column.
        let click_x = sb.x + sb.width / 2.0;
        let click_y = sb.y + sb.height / 2.0;
        match layout.hit_test(click_x, click_y) {
            MultiSectionViewHit::Scrollbar { section, .. } => {
                assert_eq!(section, 0, "expected Scrollbar{{0}}");
            }
            other => panic!(
                "click at ({:.1}, {:.1}) inside scrollbar bounds {:?} returned {:?} — Body has shadowed Scrollbar in hit_regions",
                click_x, click_y, sb, other
            ),
        }
        // Conversely, hit_test the leftmost body cell — that must be Body, not Scrollbar.
        let body_b = s.body_bounds;
        if body_b.height >= 1.0 && body_b.width >= 1.0 {
            let click = layout.hit_test(body_b.x + 0.5, body_b.y + 0.5);
            assert!(
                matches!(click, MultiSectionViewHit::Body { section: 0 }),
                "leftmost body cell at ({:.1}, {:.1}) returned {:?}",
                body_b.x + 0.5,
                body_b.y + 0.5,
                click
            );
        }
    }

    /// Painted scrollbar thumb lands at the y range the layout computed
    /// from `(scroll_offset, content_rows, viewport_rows)` — this is the
    /// *Primitive Authoring Rule #6* test for state-derived paint geometry.
    ///
    /// Setup: one section with 20 rows in a 10-cell body, scrolled by
    /// 2 rows. With the snapping arithmetic in `compute_thumb_bounds`,
    /// thumb lands at `sb.y + 1` with height 5.
    #[test]
    fn scrollbar_thumb_paints_at_layout_position() {
        let area = TuiRect::new(0, 0, 30, 11);
        let mut buf = Buffer::empty(area);
        let mut v = view_with(vec![tree_section(
            "scrolly",
            &[
                "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12",
                "r13", "r14", "r15", "r16", "r17", "r18", "r19",
            ],
            SectionSize::EqualShare,
        )]);
        // Scroll 2 rows down on the only section.
        if let SectionBody::Tree(t) = &mut v.sections[0].body {
            t.scroll_offset = 2;
        }
        let layout = paint_then_layout(&mut buf, area, &v, &Theme::default(), false);
        let thumb = layout.sections[0]
            .thumb_bounds
            .expect("Tree body with overflow should have computed thumb_bounds");
        let sb = layout.sections[0].scrollbar_bounds.unwrap();

        // Part A — layout's thumb_bounds matches the formula.
        // fit_thumb(scroll=2, total=20, visible=10, track=10, min=1) →
        //   raw_len = 10/20 * 10 = 5; thumb_len = 5
        //   range = max(20-10, 1) = 10; travel = 10-5 = 5
        //   thumb_start = (2/10)*5 = 1.0
        // With cell_quantum=1.0: start_q=1, end_q=ceil(6)=6, len=5.
        // Section 0 starts at y=0; header 1 cell; gutter at y=1, height 10.
        // → thumb at sb.y(=1)+1 = 2, height 5. Catches layout-formula
        // mutations that move with paint (paint+layout could agree on
        // a wrong answer if both reference the same broken formula).
        assert!(
            (thumb.y - 2.0).abs() < 0.01,
            "thumb.y = {} (expected 2.0 from formula)",
            thumb.y
        );
        assert!(
            (thumb.height - 5.0).abs() < 0.01,
            "thumb.height = {} (expected 5.0 from formula)",
            thumb.height
        );

        // Part B — painted '█' rows match thumb_bounds. Catches paint
        // mutations that ignore thumb_bounds.
        let gutter_x = sb.x.round() as u16;
        let mut painted_thumb_rows: Vec<u16> = Vec::new();
        for y in (sb.y.round() as u16)..((sb.y + sb.height).round() as u16) {
            if cell_char(&buf, gutter_x, y) == '█' {
                painted_thumb_rows.push(y);
            }
        }
        assert_eq!(
            painted_thumb_rows,
            (thumb.y.round() as u16..(thumb.y + thumb.height).round() as u16).collect::<Vec<_>>(),
            "painted thumb rows should match layout's thumb_bounds y range"
        );

        // Part C — hit_test at a painted thumb cell returns
        // `Scrollbar { Thumb }`. Closes the round-trip: paint position,
        // layout bounds, and hit region all reference the same
        // coordinates.
        let painted_mid = painted_thumb_rows[painted_thumb_rows.len() / 2];
        match layout.hit_test(sb.x + sb.width / 2.0, painted_mid as f32 + 0.5) {
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::Thumb,
            } => assert_eq!(section, 0),
            other => panic!(
                "hit at painted thumb row {} returned {:?}; expected Scrollbar::Thumb",
                painted_mid, other
            ),
        }
    }

    /// Panel-level (`WholePanel`) scrollbar: painted thumb glyphs match
    /// [`MultiSectionViewLayout::panel_scrollbar_thumb`] exactly, and
    /// that thumb's size accounts for divider strips.
    ///
    /// Regression coverage: `paint_panel_scrollbar` must reuse the thumb
    /// geometry from `panel_scrollbar_thumb` (computed once via `fit_thumb`)
    /// rather than recomputing from section sizes alone. With two 11-row tree
    /// sections (each `1 (header) + 11 (rows) = 12`) plus one 1-cell divider
    /// (`allow_resize = true`), the correct total is `25`, not `24`. In a
    /// 10-row viewport that difference changes the painted thumb height: 4.0
    /// vs. ceil(4.1666) = 5 — a visibly taller, wrong thumb.
    #[test]
    fn panel_scrollbar_thumb_paints_at_layout_position_with_dividers() {
        let area = TuiRect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let rows: [&str; 11] = [
            "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10",
        ];
        let v = MultiSectionView {
            id: WidgetId::new("v"),
            sections: vec![
                tree_section("a", &rows, SectionSize::EqualShare),
                tree_section("b", &rows, SectionSize::EqualShare),
            ],
            active_section: None,
            axis: Axis::Vertical,
            allow_resize: true,
            allow_collapse: true,
            scroll_mode: ScrollMode::WholePanel,
            has_focus: false,
            panel_scroll: 0.0,
        };

        let layout = paint_then_layout(&mut buf, area, &v, &Theme::default(), false);
        let panel_sb = layout
            .panel_scrollbar
            .expect("total_content (25) > viewport (10) → panel scrollbar present");
        let thumb = layout
            .panel_scrollbar_thumb
            .expect("panel_scrollbar present implies panel_scrollbar_thumb present");

        // Part A — matches `fit_thumb(scroll=0, total=25, visible=10,
        // track=10, min=1)`: raw_len = 10/25*10 = 4.0, thumb_len = 4.0;
        // scroll=0 → thumb_start = 0.
        assert!(
            (thumb.height - 4.0).abs() < 0.01,
            "thumb.height = {} (expected 4.0 — pre-#820 formula would give 5.0 \
             by dropping the divider strip from its total)",
            thumb.height
        );
        assert!(
            (thumb.y - panel_sb.y).abs() < 0.01,
            "thumb.y = {} (expected to start at the track top: panel_scroll=0)",
            thumb.y
        );

        // Part B — painted '█' rows match `panel_scrollbar_thumb` exactly.
        let gutter_x = panel_sb.x.round() as u16;
        let mut painted_thumb_rows: Vec<u16> = Vec::new();
        for y in (panel_sb.y.round() as u16)..((panel_sb.y + panel_sb.height).round() as u16) {
            if cell_char(&buf, gutter_x, y) == '█' {
                painted_thumb_rows.push(y);
            }
        }
        assert_eq!(
            painted_thumb_rows,
            (thumb.y.round() as u16..(thumb.y + thumb.height).round() as u16).collect::<Vec<_>>(),
            "painted panel-thumb rows should match layout's panel_scrollbar_thumb"
        );

        // Part C — hit_test at a painted thumb cell returns
        // `PanelScrollbar { Thumb }`, closing the paint/layout/hit-test
        // round trip.
        let painted_mid = painted_thumb_rows[painted_thumb_rows.len() / 2];
        match layout.hit_test(panel_sb.x + panel_sb.width / 2.0, painted_mid as f32 + 0.5) {
            MultiSectionViewHit::PanelScrollbar {
                kind: ScrollbarHit::Thumb,
            } => {}
            other => panic!(
                "hit at painted panel-thumb row {} returned {:?}; expected PanelScrollbar::Thumb",
                painted_mid, other
            ),
        }
    }

    /// Hit-test in the gutter column above the painted thumb returns
    /// `TrackBefore`, not `Thumb`. Locks the consumer-page-up
    /// contract: clicking above the thumb is meant to page up by the
    /// viewport size, not start a thumb drag.
    #[test]
    fn track_before_thumb_hits_track_before() {
        let area = TuiRect::new(0, 0, 30, 11);
        let mut v = view_with(vec![tree_section(
            "scrolly",
            &[
                "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12",
                "r13", "r14", "r15", "r16", "r17", "r18", "r19",
            ],
            SectionSize::EqualShare,
        )]);
        if let SectionBody::Tree(t) = &mut v.sections[0].body {
            t.scroll_offset = 2; // thumb lands away from the top
        }
        let layout = chrome_layout(&v, area);
        let sb = layout.sections[0].scrollbar_bounds.unwrap();
        let thumb = layout.sections[0].thumb_bounds.unwrap();

        // Click in the gutter column above the thumb's first y.
        let click_x = sb.x + sb.width / 2.0;
        let click_y = sb.y + 0.5; // top-of-gutter cell, before thumb starts
        assert!(
            click_y < thumb.y,
            "test setup: click_y must be above thumb.y (got {} vs {})",
            click_y,
            thumb.y
        );
        match layout.hit_test(click_x, click_y) {
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackBefore,
            } => assert_eq!(section, 0),
            other => panic!("expected TrackBefore, got {:?}", other),
        }
    }

    /// Symmetric: hit-test below the painted thumb returns
    /// `TrackAfter`. Locks the page-down contract.
    #[test]
    fn track_after_thumb_hits_track_after() {
        let area = TuiRect::new(0, 0, 30, 11);
        let mut v = view_with(vec![tree_section(
            "scrolly",
            &[
                "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12",
                "r13", "r14", "r15", "r16", "r17", "r18", "r19",
            ],
            SectionSize::EqualShare,
        )]);
        if let SectionBody::Tree(t) = &mut v.sections[0].body {
            t.scroll_offset = 2;
        }
        let layout = chrome_layout(&v, area);
        let sb = layout.sections[0].scrollbar_bounds.unwrap();
        let thumb = layout.sections[0].thumb_bounds.unwrap();

        // Click in the gutter below thumb's last y.
        let click_x = sb.x + sb.width / 2.0;
        let click_y = thumb.y + thumb.height + 0.5;
        assert!(
            click_y < sb.y + sb.height,
            "test setup: click_y must be inside the gutter"
        );
        match layout.hit_test(click_x, click_y) {
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackAfter,
            } => assert_eq!(section, 0),
            other => panic!("expected TrackAfter, got {:?}", other),
        }
    }

    /// Each section's body bounds and scrollbar bounds must not
    /// overlap. If body extended into the scrollbar column, paint and
    /// click would each see the column differently. Walk every section
    /// that has a scrollbar and assert the geometric exclusion.
    #[test]
    fn body_and_scrollbar_bounds_never_overlap() {
        let area = TuiRect::new(0, 0, 20, 24);
        let v = view_with(vec![
            tree_section(
                "a",
                &["x", "y", "z", "w", "v", "u", "t", "s"],
                SectionSize::EqualShare,
            ),
            tree_section(
                "b",
                &["x", "y", "z", "w", "v", "u", "t", "s"],
                SectionSize::EqualShare,
            ),
        ]);
        let mut buf = Buffer::empty(area);
        let layout = paint_then_layout(&mut buf, area, &v, &Theme::default(), false);
        for s in &layout.sections {
            if let Some(sb) = s.scrollbar_bounds {
                let body = s.body_bounds;
                let body_right = body.x + body.width;
                let sb_left = sb.x;
                assert!(
                    body_right <= sb_left + 0.001,
                    "section {}: body bounds {:?} extend into scrollbar bounds {:?}",
                    s.section_idx,
                    body,
                    sb
                );
            }
        }
    }

    #[test]
    fn divider_painted_only_when_resize_allowed() {
        let mut buf = Buffer::empty(TuiRect::new(0, 0, 10, 8));
        let mut v = view_with(vec![
            empty_section("a", SectionSize::EqualShare),
            empty_section("b", SectionSize::EqualShare),
        ]);
        // Without allow_resize: no divider strip, sections take 4 rows each.
        paint_view(
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
        paint_view(
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

    // ── Consumer-state round-trip harness ──────────────────────────────────
    //
    // The primitive-level round-trip tests above prove paint and click
    // agree on coordinate geometry. These tests prove the agreement
    // still holds under the *consumer pattern* the Debug sidebar (and
    // any "N stacked TreeView sections" host) wants:
    //
    //   - Per-section `scroll_offset` and `selected_path` live on the
    //     host, not on the primitive.
    //   - Header click activates the section without selecting a row.
    //   - Body row click activates AND selects.
    //   - Empty-body click activates without selecting.
    //   - Scrollbar drag updates ONLY the targeted section's offset.
    //   - After offset changes, paint shows the offset-th row at the
    //     body's top — and clicking that top row hit_tests back to the
    //     same row index.
    //
    // The runnable counterpart is `quadraui/examples/msv_multi_tree.rs`.
    //
    // Per CLAUDE.md "Migration discipline (corollary)": a green
    // primitive test does not prove the consumer integration is
    // correct. These tests are the gate that proves it for this
    // consumer shape.

    use crate::primitives::tree::TreeRow as TR;
    use crate::types::TreePath;

    struct ConsumerSection {
        id: SectionId,
        rows: Vec<TR>,
        scroll_offset: usize,
        selected_path: Option<TreePath>,
    }

    struct ConsumerState {
        sections: Vec<ConsumerSection>,
        active_section: Option<usize>,
    }

    impl ConsumerState {
        fn build_view(&self) -> MultiSectionView {
            let sections = self
                .sections
                .iter()
                .enumerate()
                .map(|(idx, s)| Section {
                    id: s.id.clone(),
                    header: SectionHeader {
                        title: StyledText::plain(s.id.to_uppercase()),
                        show_chevron: false,
                        ..Default::default()
                    },
                    body: SectionBody::Tree(TreeView {
                        id: WidgetId::new(format!("{}-tree", s.id)),
                        rows: s.rows.clone(),
                        selection_mode: crate::types::SelectionMode::Single,
                        selected_path: s.selected_path.clone(),
                        scroll_offset: s.scroll_offset,
                        items_window: None,
                        scroll_px: None,
                        style: Default::default(),
                        has_focus: self.active_section == Some(idx),
                    }),
                    aux: None,
                    size: SectionSize::EqualShare,
                    collapsed: false,
                    min_size: None,
                    max_size: None,
                })
                .collect();
            MultiSectionView {
                id: WidgetId::new("consumer"),
                sections,
                active_section: self.active_section,
                axis: Axis::Vertical,
                allow_resize: false,
                allow_collapse: false,
                scroll_mode: ScrollMode::PerSection,
                has_focus: true,
                panel_scroll: 0.0,
            }
        }

        /// Route a primary click at `(x, y)` inside `area` exactly the
        /// way `examples/msv_multi_tree.rs::DebugSidebar::click` does.
        /// Header → activate w/o select; Body+Row → activate+select;
        /// Body+Empty (empty section) → activate w/o select.
        /// Mirror the example's `DebugSidebar::drag_to` arithmetic:
        /// 1 cell of drag = 1 row of scroll, clamped to
        /// `[0, rows.len() - viewport_rows]`. The natural-max clamp
        /// (vs. the naive `rows.len() - 1`) is the bug class
        /// `consumer_drag_past_natural_max_clamps_to_keep_viewport_full`
        /// catches.
        fn drag_scrollbar(
            &mut self,
            section: usize,
            origin_offset: usize,
            viewport_rows: usize,
            dy: i32,
        ) {
            let row_count = self.sections[section].rows.len();
            let max_offset = row_count.saturating_sub(viewport_rows);
            let new = (origin_offset as i32 + dy).max(0) as usize;
            self.sections[section].scroll_offset = new.min(max_offset);
        }

        fn click(&mut self, x: f32, y: f32, area: TuiRect) {
            let view = self.build_view();
            let layout = chrome_layout(&view, area);
            match layout.hit_test(x, y) {
                MultiSectionViewHit::Header { section, .. } => {
                    self.active_section = Some(section);
                    self.sections[section].selected_path = None;
                }
                MultiSectionViewHit::Body { section } => {
                    self.active_section = Some(section);
                    let body_b = layout.sections[section].body_bounds;
                    let tree = match &view.sections[section].body {
                        SectionBody::Tree(t) => t.clone(),
                        _ => return,
                    };
                    let body_area = TuiRect::new(
                        body_b.x.round() as u16,
                        body_b.y.round() as u16,
                        body_b.width.round() as u16,
                        body_b.height.round() as u16,
                    );
                    let inner = crate::tui::tui_tree_layout(&tree, body_area);
                    match inner.hit_test(x - body_b.x, y - body_b.y) {
                        crate::TreeViewHit::Row(idx) | crate::TreeViewHit::Chevron(idx) => {
                            self.sections[section].selected_path =
                                Some(tree.rows[idx].path.clone());
                        }
                        crate::TreeViewHit::Empty => {
                            // Activated above; nothing to select.
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn make_rows(prefix: &str, n: usize) -> Vec<TR> {
        (0..n)
            .map(|i| TR {
                path: vec![i as u16],
                indent: 0,
                icon: None,
                text: StyledText::plain(format!("{prefix}{i}")),
                badge: None,
                is_expanded: None,
                decoration: Default::default(),
                edit: None,
            })
            .collect()
    }

    fn debug_sidebar_state() -> ConsumerState {
        ConsumerState {
            sections: vec![
                ConsumerSection {
                    id: "vars".into(),
                    rows: make_rows("v", 12),
                    scroll_offset: 0,
                    selected_path: None,
                },
                ConsumerSection {
                    id: "watch".into(),
                    rows: make_rows("w", 8),
                    scroll_offset: 0,
                    selected_path: None,
                },
                ConsumerSection {
                    id: "stack".into(),
                    rows: make_rows("frame", 5),
                    scroll_offset: 0,
                    selected_path: None,
                },
                ConsumerSection {
                    id: "bps".into(),
                    rows: Vec::new(), // empty body
                    scroll_offset: 0,
                    selected_path: None,
                },
            ],
            active_section: None,
        }
    }

    /// Header click activates the section but does NOT select a row.
    /// Important for VSCode-style sidebars: clicking the header focuses
    /// the section; selection is reserved for body interaction.
    #[test]
    fn consumer_header_click_activates_section_without_selecting() {
        let area = TuiRect::new(0, 0, 30, 20);
        let mut state = debug_sidebar_state();
        // Pre-seed a selection in section 1 — header click on section 2
        // must NOT clobber section 1's selection.
        state.sections[1].selected_path = Some(vec![3]);

        // Find the header row of section 2 by querying the layout.
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        let hb = layout.sections[2].header_bounds;
        let click_x = hb.x + hb.width / 2.0;
        let click_y = hb.y + 0.5;

        state.click(click_x, click_y, area);

        assert_eq!(
            state.active_section,
            Some(2),
            "header click should activate section 2"
        );
        assert!(
            state.sections[2].selected_path.is_none(),
            "header click must not select a row in the activated section"
        );
        assert_eq!(
            state.sections[1].selected_path,
            Some(vec![3]),
            "header click on section 2 must not touch section 1's selection"
        );
    }

    /// Body row click activates the section AND selects the clicked
    /// row. Round-trip: paint, find a body row in the buffer, click at
    /// that exact y, assert the selected_path matches that row's path.
    /// Catches the consumer-side off-by-one where paint/click agreed
    /// at the primitive layer but the host's inner-tree hit-test
    /// translation drifted (subtracting wrong origin, etc).
    #[test]
    fn consumer_body_row_click_activates_and_selects_clicked_row() {
        let area = TuiRect::new(0, 0, 30, 21);
        let mut buf = Buffer::empty(area);
        let mut state = debug_sidebar_state();
        let view = state.build_view();
        // Paint, then find the row with text "v2" (vars section, 3rd row).
        paint_view(&mut buf, area, &view, &Theme::default(), false);
        let layout = chrome_layout(&view, area);
        let body_b = layout.sections[0].body_bounds;
        let body_y_start = body_b.y.round() as u16;
        let body_y_end = (body_b.y + body_b.height).round() as u16;
        // Scan body rows for "v2".
        let mut painted_y: Option<u16> = None;
        for y in body_y_start..body_y_end {
            let row: String = (area.x..area.x + area.width)
                .map(|x| cell_char(&buf, x, y))
                .collect();
            if row.contains("v2") {
                painted_y = Some(y);
                break;
            }
        }
        let painted_y = painted_y.expect("v2 should be painted in section 0 body");

        // Click at the centre of the body width on the painted row.
        let click_x = body_b.x + body_b.width / 2.0;
        state.click(click_x, painted_y as f32 + 0.5, area);

        assert_eq!(state.active_section, Some(0), "section 0 should activate");
        assert_eq!(
            state.sections[0].selected_path,
            Some(vec![2]),
            "click on painted v2 should select path [2]"
        );
    }

    /// Body click on an empty section activates the section but does
    /// NOT set a selection (no rows to pick). The `Body { section }`
    /// hit fires; the host's inner-tree hit_test returns
    /// `TreeViewHit::Empty`; the host filters and leaves selection
    /// alone.
    #[test]
    fn consumer_empty_body_click_activates_without_selecting() {
        let area = TuiRect::new(0, 0, 30, 21);
        let mut state = debug_sidebar_state();
        // Section 3 (`bps`) has 0 rows.
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        let body_b = layout.sections[3].body_bounds;
        let click_x = body_b.x + body_b.width / 2.0;
        let click_y = body_b.y + body_b.height / 2.0;

        state.click(click_x, click_y, area);

        assert_eq!(
            state.active_section,
            Some(3),
            "empty-body click should still activate the section"
        );
        assert!(
            state.sections[3].selected_path.is_none(),
            "empty body has no row to select"
        );
    }

    /// Scrollbar drag (consumer-side) updates ONLY the targeted
    /// section's `scroll_offset`. The other sections keep their
    /// state — that's the whole point of host-owned per-section state
    /// vs. a shared bridging cell. Test simulates the same MouseDown +
    /// MouseMoved sequence the example's `DebugSidebar::click` /
    /// `drag_to` apply.
    #[test]
    fn consumer_scrollbar_drag_updates_only_targeted_section_offset() {
        // 30 cols × 24 rows so each EqualShare section gets enough
        // body height for `vars` (12 rows) to overflow and reserve a
        // scrollbar gutter. With 24 cells / 4 sections, each section
        // has 6 cells (1 header + 5 body) — vars has 12 rows so
        // body_overflows fires.
        let area = TuiRect::new(0, 0, 30, 24);
        let mut state = debug_sidebar_state();
        // Pre-seed a selection in another section so we can verify
        // the drag doesn't disturb it.
        state.sections[1].selected_path = Some(vec![4]);

        let view = state.build_view();
        let layout = chrome_layout(&view, area);

        // Section 0 (vars) overflows — it must have a scrollbar.
        let sb = layout.sections[0]
            .scrollbar_bounds
            .expect("vars body overflows; expected scrollbar gutter");

        // Simulate MouseDown on the scrollbar — capture origin and
        // section viewport rows the same way the example's
        // `DebugSidebar::click` does.
        let press_y = sb.y.round() as u16;
        let origin_offset = state.sections[0].scroll_offset;
        let viewport_rows = layout.sections[0].body_bounds.height as usize;

        // Simulate MouseMoved 3 cells down via the consumer's drag
        // helper. 1 cell = 1 row.
        let drag_y = press_y + 3;
        let dy = drag_y as i32 - press_y as i32;
        state.drag_scrollbar(0, origin_offset, viewport_rows, dy);

        // Only section 0's offset moved.
        assert_eq!(state.sections[0].scroll_offset, 3);
        assert_eq!(state.sections[1].scroll_offset, 0);
        assert_eq!(state.sections[2].scroll_offset, 0);
        assert_eq!(state.sections[3].scroll_offset, 0);

        // Other sections' selection / state untouched.
        assert_eq!(state.sections[1].selected_path, Some(vec![4]));
        assert!(state.sections[2].selected_path.is_none());
    }

    /// Smoke-found regression: dragging past the bottom of the gutter
    /// kept advancing the inner tree's `scroll_offset` past the
    /// natural max (`rows.len() - viewport_rows`), ending in states
    /// where only the trailing row was visible while the saturated
    /// thumb sat idle. Per *Primitive Authoring Rule #6* the thumb
    /// position correctly saturates at max scroll (`fit_thumb` clamps
    /// `scroll/range` to `[0, 1]`); the bug is consumer-side — drag
    /// must clamp to the natural max so paint and state stay in sync.
    ///
    /// Catches naive `rows.len() - 1` clamps. Empirically: replacing
    /// the clamp denominator with `1` in `drag_scrollbar` makes this
    /// test fail.
    #[test]
    fn consumer_drag_past_natural_max_clamps_to_keep_viewport_full() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut state = debug_sidebar_state();
        let view = state.build_view();
        let layout = chrome_layout(&view, area);

        let viewport_rows = layout.sections[0].body_bounds.height as usize;
        let row_count = state.sections[0].rows.len();
        let natural_max = row_count.saturating_sub(viewport_rows);
        assert!(
            natural_max > 0 && natural_max < row_count - 1,
            "test setup: vars must overflow with a non-degenerate natural_max (got {natural_max}, row_count {row_count}, viewport {viewport_rows})"
        );

        // Drag WAY past the bottom — dy = +1000 cells. Natural-max
        // clamp must absorb the overshoot.
        state.drag_scrollbar(0, 0, viewport_rows, 1000);

        assert_eq!(
            state.sections[0].scroll_offset, natural_max,
            "drag past bottom should clamp to natural_max ({natural_max}); got {}",
            state.sections[0].scroll_offset
        );
        assert!(
            state.sections[0].scroll_offset < row_count - 1,
            "scroll_offset reached row_count - 1 ({}) — that's the over-scroll \
             state where only the trailing row is visible. The natural-max \
             clamp must prevent this by construction.",
            row_count - 1
        );
    }

    /// After updating a section's `scroll_offset`, paint shows the
    /// offset-th source row at the body's top — and clicking that top
    /// row hit_tests back to the same row index. This is the round-
    /// trip across the scroll boundary: paint and click both consume
    /// `scroll_offset`, so they must agree on which row is at the top
    /// after any drag.
    #[test]
    fn consumer_post_drag_paint_and_click_agree_on_top_row() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut buf = Buffer::empty(area);
        let mut state = debug_sidebar_state();

        // Apply a drag of 3 rows on section 0 (vars).
        state.sections[0].scroll_offset = 3;

        // Paint the new state.
        let view = state.build_view();
        paint_view(&mut buf, area, &view, &Theme::default(), false);
        let layout = chrome_layout(&view, area);

        // The body of section 0 now shows v3, v4, v5, ... at the top.
        // Find "v3" in the buffer; assert it's painted at body's first
        // row, and clicking that row selects path [3].
        let body_b = layout.sections[0].body_bounds;
        let top_y = body_b.y.round() as u16;
        let row: String = (area.x..area.x + area.width)
            .map(|x| cell_char(&buf, x, top_y))
            .collect();
        assert!(
            row.contains("v3"),
            "post-drag top body row should paint v3, got {row:?}"
        );

        // Click that top row — selected_path must be [3], not [0].
        let click_x = body_b.x + body_b.width / 2.0;
        state.click(click_x, top_y as f32 + 0.5, area);
        assert_eq!(
            state.sections[0].selected_path,
            Some(vec![3]),
            "click on post-drag top row should select path [3] (the offset)"
        );
    }

    // ── Consumer-state round-trip harness — SC panel ─────────────────────
    //
    // The SC panel consumer shape adds a
    // `SectionAux::Input` commit-message editor on section 0, plus
    // collapsible Tree sections. The contracts the harness verifies:
    //
    //   - Input keystrokes update aux text + caret; paint shows the
    //     typed text in the aux row.
    //   - Click on aux focuses the input; click outside blurs it.
    //   - Click on chevron toggles `collapsed`; round-trip across paint
    //     of '▾'/'▸' glyphs and click resolution stays consistent.
    //   - Collapsed section paints header only — the inner Tree's
    //     items do not appear in the buffer in that section's vertical
    //     span.
    //   - For every cell in the view, hit_test never returns
    //     `Body { section }` for a collapsed section. Locks the
    //     "click in collapsed body area" semantics (acceptance
    //     criterion 3): there is no body region for a collapsed
    //     section; the next section's chrome occupies that y-range.
    //
    // Runnable counterpart: `quadraui/examples/msv_sc_panel.rs`.

    struct SCState {
        commit_message: String,
        commit_caret: usize,
        commit_input_active: bool,
        sections: Vec<SCStateSection>,
        active_section: Option<usize>,
        previous_active: Option<usize>,
    }

    struct SCStateSection {
        id: SectionId,
        rows: Vec<TR>,
        collapsed: bool,
        scroll_offset: usize,
        selected_path: Option<TreePath>,
    }

    impl SCState {
        fn new() -> Self {
            Self {
                commit_message: String::new(),
                commit_caret: 0,
                commit_input_active: false,
                sections: vec![
                    SCStateSection {
                        id: "changes".into(),
                        rows: make_rows("c", 6),
                        collapsed: false,
                        scroll_offset: 0,
                        selected_path: None,
                    },
                    SCStateSection {
                        id: "staged".into(),
                        rows: make_rows("s", 4),
                        collapsed: false,
                        scroll_offset: 0,
                        selected_path: None,
                    },
                    SCStateSection {
                        id: "worktrees".into(),
                        rows: make_rows("w", 2),
                        collapsed: false,
                        scroll_offset: 0,
                        selected_path: None,
                    },
                ],
                active_section: None,
                previous_active: None,
            }
        }

        fn build_view(&self) -> MultiSectionView {
            let sections = self
                .sections
                .iter()
                .enumerate()
                .map(|(idx, s)| {
                    let aux = if idx == 0 {
                        Some(SectionAux::Input(InlineInput {
                            id: WidgetId::new("commit"),
                            text: self.commit_message.clone(),
                            caret: self.commit_caret,
                            placeholder: Some("Commit message".to_string()),
                            has_focus: self.commit_input_active,
                        }))
                    } else {
                        None
                    };
                    let size = if idx == 0 {
                        SectionSize::Fixed(8)
                    } else {
                        SectionSize::EqualShare
                    };
                    Section {
                        id: s.id.clone(),
                        header: SectionHeader {
                            title: StyledText::plain(s.id.to_uppercase()),
                            show_chevron: true,
                            ..Default::default()
                        },
                        body: SectionBody::Tree(TreeView {
                            id: WidgetId::new(format!("{}-tree", s.id)),
                            rows: s.rows.clone(),
                            selection_mode: crate::types::SelectionMode::Single,
                            selected_path: s.selected_path.clone(),
                            scroll_offset: s.scroll_offset,
                            items_window: None,
                            scroll_px: None,
                            style: Default::default(),
                            has_focus: !self.commit_input_active
                                && self.active_section == Some(idx),
                        }),
                        aux,
                        size,
                        collapsed: s.collapsed,
                        min_size: None,
                        max_size: None,
                    }
                })
                .collect();
            MultiSectionView {
                id: WidgetId::new("sc"),
                sections,
                active_section: self.active_section,
                axis: Axis::Vertical,
                allow_resize: false,
                allow_collapse: true,
                scroll_mode: ScrollMode::PerSection,
                has_focus: true,
                panel_scroll: 0.0,
            }
        }

        /// Click router mirroring `examples/msv_sc_panel.rs::SCSidebar::click`.
        /// Aux→focus input; Chevron→toggle collapsed + activate;
        /// TitleArea→activate w/o select; outside→blur input.
        fn click(&mut self, x: f32, y: f32, area: TuiRect) {
            let view = self.build_view();
            let layout = chrome_layout(&view, area);
            match layout.hit_test(x, y) {
                MultiSectionViewHit::Aux {
                    kind: AuxHit::Input,
                    ..
                } => {
                    self.previous_active = self.active_section;
                    self.commit_input_active = true;
                    self.active_section = None;
                }
                MultiSectionViewHit::Header {
                    section,
                    kind: HeaderHit::Chevron,
                } => {
                    self.commit_input_active = false;
                    self.active_section = Some(section);
                    self.sections[section].collapsed = !self.sections[section].collapsed;
                }
                MultiSectionViewHit::Header {
                    section,
                    kind: HeaderHit::TitleArea,
                } => {
                    self.commit_input_active = false;
                    self.active_section = Some(section);
                }
                MultiSectionViewHit::Body { section } => {
                    self.commit_input_active = false;
                    self.active_section = Some(section);
                }
                _ => {
                    self.commit_input_active = false;
                }
            }
        }

        fn type_char(&mut self, c: char) {
            if !self.commit_input_active {
                return;
            }
            let mut chars: Vec<char> = self.commit_message.chars().collect();
            chars.insert(self.commit_caret.min(chars.len()), c);
            self.commit_message = chars.into_iter().collect();
            self.commit_caret += 1;
        }

        fn backspace(&mut self) {
            if !self.commit_input_active || self.commit_caret == 0 {
                return;
            }
            let mut chars: Vec<char> = self.commit_message.chars().collect();
            chars.remove(self.commit_caret - 1);
            self.commit_message = chars.into_iter().collect();
            self.commit_caret -= 1;
        }

        fn blur_input(&mut self) {
            self.commit_input_active = false;
            self.active_section = self.previous_active.or(Some(0));
        }
    }

    /// Helper: read a buffer row left-to-right.
    fn buf_row(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        (area.x..area.x + area.width)
            .map(|x| cell_char(buf, x, y))
            .collect()
    }

    // ── 1. Aux input keystroke routing ───────────────────────────────────
    //
    // After focus + typing, the host's commit_message + caret reflect
    // the chars entered, AND the painted aux row shows the typed text.
    // Catches both state-mutation drift and paint-doesn't-read-state
    // bugs.

    /// Aux-row click focuses the commit input. Then typed chars
    /// update host state AND appear in the painted aux row.
    #[test]
    fn consumer_sc_input_keystroke_updates_aux_text_and_caret() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut buf = Buffer::empty(area);
        let mut state = SCState::new();

        // Click on the aux row to focus the input.
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        let aux_b = layout.sections[0]
            .aux_bounds
            .expect("section 0 has aux=Input");
        state.click(aux_b.x + 1.0, aux_b.y + 0.5, area);
        assert!(
            state.commit_input_active,
            "click on aux should focus the commit input"
        );

        // Type "hi".
        state.type_char('h');
        state.type_char('i');
        assert_eq!(state.commit_message, "hi");
        assert_eq!(state.commit_caret, 2);

        // Paint and verify aux row shows the typed text.
        let view = state.build_view();
        paint_view(&mut buf, area, &view, &Theme::default(), false);
        let aux_y = aux_b.y.round() as u16;
        let painted = buf_row(&buf, aux_y);
        assert!(
            painted.starts_with("hi"),
            "aux row {} should start with typed 'hi', got {painted:?}",
            aux_y
        );

        // Backspace — host state and paint both update.
        state.backspace();
        assert_eq!(state.commit_message, "h");
        assert_eq!(state.commit_caret, 1);
        let mut buf2 = Buffer::empty(area);
        paint_view(
            &mut buf2,
            area,
            &state.build_view(),
            &Theme::default(),
            false,
        );
        let painted = buf_row(&buf2, aux_y);
        assert!(
            painted.starts_with("h ") || painted.starts_with("h"),
            "aux row {} after backspace should start with 'h', got {painted:?}",
            aux_y
        );
    }

    /// Click outside the aux row blurs the input. Locks the focus
    /// transition contract: a click on a header (or anywhere else)
    /// drops input focus.
    #[test]
    fn consumer_sc_click_outside_aux_blurs_input() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut state = SCState::new();
        // Focus the input.
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        let aux_b = layout.sections[0].aux_bounds.unwrap();
        state.click(aux_b.x + 1.0, aux_b.y + 0.5, area);
        assert!(state.commit_input_active);

        // Click on section 1's header.
        let header1 = layout.sections[1].header_bounds;
        state.click(header1.x + 5.0, header1.y + 0.5, area);
        assert!(
            !state.commit_input_active,
            "click outside aux should blur the input"
        );
    }

    /// Esc (blur) restores the active section that was captured when the
    /// input gained focus. If no section was active before, defaults to 0.
    #[test]
    fn consumer_sc_blur_restores_previous_active_section() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut state = SCState::new();

        // Activate section 1 via header click.
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        let header1 = layout.sections[1].header_bounds;
        state.click(header1.x + 5.0, header1.y + 0.5, area);
        assert_eq!(state.active_section, Some(1));

        // Click aux to focus input.
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        let aux_b = layout.sections[0].aux_bounds.unwrap();
        state.click(aux_b.x + 1.0, aux_b.y + 0.5, area);
        assert!(state.commit_input_active);
        assert_eq!(state.active_section, None);

        // Blur — should restore section 1.
        state.blur_input();
        assert!(!state.commit_input_active);
        assert_eq!(
            state.active_section,
            Some(1),
            "blur should restore the previously-active section"
        );
    }

    /// When no section was active before focusing the input, blur
    /// defaults to section 0 (matches Tab-from-none behaviour).
    #[test]
    fn consumer_sc_blur_defaults_to_section_0_when_no_previous() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut state = SCState::new();
        assert_eq!(state.active_section, None);

        // Click aux directly (no prior section activation).
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        let aux_b = layout.sections[0].aux_bounds.unwrap();
        state.click(aux_b.x + 1.0, aux_b.y + 0.5, area);
        assert!(state.commit_input_active);

        // Blur — should default to section 0.
        state.blur_input();
        assert_eq!(
            state.active_section,
            Some(0),
            "blur with no previous active should default to section 0"
        );
    }

    // ── 2. Chevron toggle round-trip ─────────────────────────────────────
    //
    // Section starts expanded ('▾'). Click the chevron at the painted
    // glyph position; assert collapsed=true and paint flips to '▸'.
    // Click again; assert expanded again. Catches any drift between
    // the chevron's painted position and the chevron hit region.

    #[test]
    fn consumer_sc_chevron_click_toggles_collapsed_round_trip() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut buf = Buffer::empty(area);
        let mut state = SCState::new();

        // Section 1 starts expanded.
        assert!(!state.sections[1].collapsed);

        // Paint and find the chevron in section 1's header row.
        // Default TreeStyle shows '▾' for expanded, '▸' for collapsed.
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        paint_view(&mut buf, area, &view, &Theme::default(), false);
        let header_y = layout.sections[1].header_bounds.y.round() as u16;
        let painted = buf_row(&buf, header_y);
        assert!(
            painted.starts_with('▾'),
            "section 1 header should paint expanded chevron '▾' at start, got {painted:?}"
        );

        // Click the chevron at painted column 0.
        state.click(0.5, header_y as f32 + 0.5, area);
        assert!(
            state.sections[1].collapsed,
            "chevron click should toggle collapsed=true"
        );

        // Re-paint; chevron flips to '▸'.
        let mut buf = Buffer::empty(area);
        let view = state.build_view();
        let layout = chrome_layout(&view, area);
        paint_view(&mut buf, area, &view, &Theme::default(), false);
        let header_y = layout.sections[1].header_bounds.y.round() as u16;
        let painted = buf_row(&buf, header_y);
        assert!(
            painted.starts_with('▸'),
            "section 1 header should paint collapsed chevron '▸' after toggle, got {painted:?}"
        );

        // Second click flips back.
        state.click(0.5, header_y as f32 + 0.5, area);
        assert!(
            !state.sections[1].collapsed,
            "second chevron click should toggle collapsed=false"
        );
    }

    // ── 3. Collapsed section paints header only ──────────────────────────
    //
    // When section 1 is collapsed, its inner Tree row prefixes
    // (e.g. "s0", "s1") must not appear anywhere in the buffer.
    // Catches "collapsed section still paints body" bugs in the
    // rasteriser.

    #[test]
    fn consumer_sc_collapsed_section_paints_header_only() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut buf = Buffer::empty(area);
        let mut state = SCState::new();
        state.sections[1].collapsed = true;

        let view = state.build_view();
        paint_view(&mut buf, area, &view, &Theme::default(), false);

        // Scan every row: no row should contain any of section 1's
        // item text ("s0".."s3"). The header text "STAGED" is fine.
        for y in 0..area.height {
            let row = buf_row(&buf, y);
            for r in 0..state.sections[1].rows.len() {
                let needle = format!("s{}", r);
                assert!(
                    !row.contains(&needle),
                    "collapsed section's body item '{}' painted at row {}: {row:?}",
                    needle,
                    y
                );
            }
        }
    }

    // ── 4. No click anywhere returns Body for a collapsed section ────────
    //
    // Locks the acceptance-criterion-3 semantics: a collapsed section
    // has no body hit region. The y range that *would* be its body
    // (if expanded) is occupied by the next section. Walk every cell
    // in the view; assert hit_test never returns Body{collapsed_idx}.

    #[test]
    fn consumer_sc_collapsed_body_never_hit_testable() {
        let area = TuiRect::new(0, 0, 30, 24);
        let mut state = SCState::new();
        state.sections[1].collapsed = true;
        let view = state.build_view();
        let layout = chrome_layout(&view, area);

        for y in 0..area.height {
            for x in 0..area.width {
                let hit = layout.hit_test(x as f32, y as f32);
                assert!(
                    !matches!(hit, MultiSectionViewHit::Body { section: 1 }),
                    "click at ({}, {}) returned Body{{1}} despite section 1 collapsed",
                    x,
                    y
                );
            }
        }
    }

    // ── 5. Dynamic section count: layout still snaps integer cells ───────
    //
    // Acceptance criterion 5: layout works across a range of section
    // counts. Sweep 1..=8 sections; assert (a) every bound is
    // integer-aligned (cell_quantum=1.0), (b) total resolved size
    // never exceeds bounds.height. Mixes Fixed(8) + EqualShare to
    // exercise the same shape the SC panel uses.

    #[test]
    fn consumer_sc_dynamic_section_count_layout_snaps_integer_cells() {
        for n in 1..=8 {
            let area = TuiRect::new(0, 0, 30, 30);
            let mut state = SCState::new();
            state.sections.clear();
            for i in 0..n {
                state.sections.push(SCStateSection {
                    id: format!("s{i}"),
                    rows: make_rows("r", 4),
                    collapsed: false,
                    scroll_offset: 0,
                    selected_path: None,
                });
            }
            let view = state.build_view();
            let layout = chrome_layout(&view, area);

            for s in &layout.sections {
                assert_eq!(
                    s.header_bounds.y.round(),
                    s.header_bounds.y,
                    "n={}: section {} header y not integer-aligned: {}",
                    n,
                    s.section_idx,
                    s.header_bounds.y
                );
                assert_eq!(
                    s.body_bounds.height.round(),
                    s.body_bounds.height,
                    "n={}: section {} body height not integer-aligned: {}",
                    n,
                    s.section_idx,
                    s.body_bounds.height
                );
            }

            let total: f32 = layout.sections.iter().map(|s| s.resolved_size).sum();
            assert!(
                total <= area.height as f32 + 0.001,
                "n={n}: total resolved size {total} exceeds bounds height {}",
                area.height
            );
        }
    }

    // ── 6. Mixed Form + Tree section round-trip ────────────────────────────
    //
    // Paints a Form section above a Tree section, finds the painted form
    // field labels and tree row text, hit-tests the coordinates, and
    // asserts each hit lands in the correct section with the correct body
    // type. Validates the Sidebar → MSV → TUI pipeline end-to-end.

    use crate::primitives::form::{FieldKind, Form, FormField};

    fn form_section(id: &str, fields: Vec<FormField>, size: SectionSize) -> Section {
        Section {
            id: id.into(),
            header: SectionHeader {
                title: StyledText::plain(id.to_uppercase()),
                show_chevron: false,
                ..Default::default()
            },
            body: SectionBody::Form(Form {
                id: WidgetId::new(format!("{}-form", id)),
                fields,
                focused_field: None,
                scroll_offset: 0,
                has_focus: false,
                label_position: Default::default(),
                label_align: Default::default(),
                label_width: None,
                control_width: Default::default(),
                min_control_width: None,
                spacing: Default::default(),
            }),
            aux: None,
            size,
            collapsed: false,
            min_size: None,
            max_size: None,
        }
    }

    #[test]
    fn mixed_form_tree_paint_click_round_trip() {
        let area = TuiRect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);

        let fields = vec![
            FormField {
                id: WidgetId::new("query"),
                label: StyledText::plain("Query"),
                kind: FieldKind::TextInput {
                    value: "hello".into(),
                    placeholder: String::new(),
                    cursor: None,
                    selection_anchor: None,
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            },
            FormField {
                id: WidgetId::new("case"),
                label: StyledText::plain("Case"),
                kind: FieldKind::Toggle { value: true },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            },
        ];

        let v = view_with(vec![
            form_section("search", fields, SectionSize::Content),
            tree_section(
                "results",
                &["file1.rs", "file2.rs", "file3.rs"],
                SectionSize::EqualShare,
            ),
        ]);

        let layout = paint_then_layout(&mut buf, area, &v, &Theme::default(), false);

        // Verify both sections painted their headers.
        let search_header_y = find_row_with(&buf, "SEARCH").expect("SEARCH header not painted");
        let results_header_y = find_row_with(&buf, "RESULTS").expect("RESULTS header not painted");
        assert!(
            results_header_y > search_header_y,
            "RESULTS header should be below SEARCH header"
        );

        // Verify form field labels are painted.
        let query_y = find_row_with(&buf, "Query").expect("Query field label not painted");
        let case_y = find_row_with(&buf, "Case").expect("Case field label not painted");
        assert!(
            query_y > search_header_y && query_y < results_header_y,
            "Query field should be in SEARCH section body"
        );
        assert!(
            case_y > search_header_y && case_y < results_header_y,
            "Case field should be in SEARCH section body"
        );

        // Verify tree rows are painted.
        let file1_y = find_row_with(&buf, "file1.rs").expect("file1.rs tree row not painted");
        assert!(
            file1_y > results_header_y,
            "file1.rs should be in RESULTS section body"
        );

        // Round-trip: hit-test at form field row → Body { section: 0 }.
        let hit = layout.hit_test(5.0, query_y as f32);
        match hit {
            MultiSectionViewHit::Body { section, .. } => assert_eq!(
                section, 0,
                "click at Query row hit section {section}, expected 0"
            ),
            other => panic!(
                "click at Query row returned {:?}, expected Body{{0}}",
                other
            ),
        }

        // Round-trip: hit-test at tree row → Body { section: 1 }.
        let hit = layout.hit_test(5.0, file1_y as f32);
        match hit {
            MultiSectionViewHit::Body { section, .. } => assert_eq!(
                section, 1,
                "click at file1.rs row hit section {section}, expected 1"
            ),
            other => panic!(
                "click at file1.rs row returned {:?}, expected Body{{1}}",
                other
            ),
        }

        // Round-trip: hit-test at headers → Header { section: N }.
        let hit = layout.hit_test(5.0, search_header_y as f32);
        assert!(
            matches!(hit, MultiSectionViewHit::Header { section: 0, .. }),
            "click at SEARCH header returned {:?}",
            hit
        );
        let hit = layout.hit_test(5.0, results_header_y as f32);
        assert!(
            matches!(hit, MultiSectionViewHit::Header { section: 1, .. }),
            "click at RESULTS header returned {:?}",
            hit
        );
    }

    #[test]
    fn form_toggle_group_items_individually_clickable_in_msv() {
        use crate::primitives::form::{FormHit, ToggleGroupItem};

        let area = TuiRect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);

        let fields = vec![FormField {
            id: WidgetId::new("flags"),
            label: StyledText::plain(""),
            kind: FieldKind::ToggleGroup {
                toggles: vec![
                    ToggleGroupItem {
                        id: WidgetId::new("case"),
                        label: "Aa".into(),
                        value: false,
                    },
                    ToggleGroupItem {
                        id: WidgetId::new("regex"),
                        label: ".*".into(),
                        value: true,
                    },
                ],
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }];

        let v = view_with(vec![form_section("opts", fields, SectionSize::EqualShare)]);
        let layout = paint_then_layout(&mut buf, area, &v, &Theme::default(), false);

        let s = &layout.sections[0];
        let body_b = s.body_bounds;

        // Find the row with the toggles — scan for "Aa" painted text.
        let toggle_y = find_row_with(&buf, "Aa").expect("ToggleGroup items not painted");

        // The click should land in the body.
        let hit = layout.hit_test(5.0, toggle_y as f32);
        assert!(
            matches!(hit, MultiSectionViewHit::Body { section: 0 }),
            "click at toggle row returned {:?}, expected Body{{0}}",
            hit
        );

        // Now hit-test the Form layout inside the body to verify
        // individual items are reachable.
        if let SectionBody::Form(f) = &v.sections[0].body {
            let form_layout = crate::primitives::form::cell_layout::tui_form_layout(
                f,
                TuiRect::new(0, 0, body_b.width as u16, body_b.height as u16),
            );
            // First item should be individually hit-testable.
            let vis = &form_layout.visible_fields[0];
            assert!(
                !vis.item_bounds.is_empty(),
                "ToggleGroup should have per-item bounds in form layout"
            );
            let (first_id, first_rect) = &vis.item_bounds[0];
            assert_eq!(first_id, &WidgetId::new("case"));
            let hit = form_layout.hit_test(first_rect.x + 0.5, first_rect.y + 0.5);
            assert_eq!(
                hit,
                FormHit::Field(WidgetId::new("case")),
                "hit-test at first toggle item should return its individual ID"
            );
        } else {
            panic!("expected Form body");
        }
    }

    #[test]
    fn form_toggle_value_paints_correctly() {
        let area = TuiRect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);

        let fields = vec![FormField {
            id: WidgetId::new("toggle"),
            label: StyledText::plain("Enabled"),
            kind: FieldKind::Toggle { value: true },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }];

        let v = view_with(vec![form_section("opts", fields, SectionSize::EqualShare)]);
        paint_view(&mut buf, area, &v, &Theme::default(), false);

        let toggle_y = find_row_with(&buf, "Enabled").expect("Toggle field not painted");
        let row = row_text(&buf, toggle_y);
        assert!(
            row.contains("[x]"),
            "toggle value=true should paint [x], got row: {row:?}"
        );
    }

    // `paint_empty_body` centers the empty-state message using
    // `line.visible_width()` (display-width-aware). Pin that the painted
    // extent of a CJK message agrees with `visible_width()` exactly, so it
    // lands with equal left/right margins.
    #[test]
    fn empty_body_centers_cjk_message_by_display_width_not_char_count() {
        let area = TuiRect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let empty = EmptyBody {
            text: StyledText::plain("日本語"),
            ..Default::default()
        };
        assert_eq!(empty.text.visible_width(), 6);

        paint_empty_body(
            &mut BufferCellSurface::new(&mut buf),
            crate::event::Rect::new(0.0, 0.0, 20.0, 3.0),
            &empty,
            &Theme::default(),
        );

        // total=1 line, start_y = 0 + (3 - 1) / 2 = 1.
        let y = 1;
        // x_start = 0 + (20 - 6) / 2 = 7.
        assert_eq!(cell_char(&buf, 7, y), '日');
        assert_eq!(buf[(8u16, y)].symbol(), "");
        assert_eq!(cell_char(&buf, 9, y), '本');
        assert_eq!(buf[(10u16, y)].symbol(), "");
        assert_eq!(cell_char(&buf, 11, y), '語');
        assert_eq!(buf[(12u16, y)].symbol(), "");

        // Equal margins on both sides: nothing painted before col 7 or at/
        // after col 13 (7 + visible_width) on the message row.
        for x in 0..7 {
            assert_eq!(
                cell_char(&buf, x, y),
                ' ',
                "unexpected content left of center at col {x}"
            );
        }
        for x in 13..20 {
            assert_eq!(
                cell_char(&buf, x, y),
                ' ',
                "unexpected content right of the CJK message's measured width at col {x}"
            );
        }
    }
}
