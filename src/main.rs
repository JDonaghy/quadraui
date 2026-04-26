//! kubeui — TUI Kubernetes dashboard.
//!
//! Backend-specific shell around [`kubeui_core`]. Owns terminal
//! setup/teardown, the crossterm event loop, and ratatui rasterisers
//! for each `quadraui` primitive the core builds. Everything else —
//! state, k8s client, view-builders, the action reducer — lives in
//! `kubeui-core` and is shared with `kubeui-gtk`.

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RatColor, Modifier, Style};
use ratatui::Terminal;

use kubeui_core::{
    apply_action, build_list, build_picker, build_status_bar, picker_bounds, picker_current_index,
    Action, AppState, Focus,
};
use quadraui::{Color, ListView, StatusBar, StyledText};

// ─── Backend (primitive → ratatui buffer) ───────────────────────────────────

fn rat_color(c: Color) -> RatColor {
    RatColor::Rgb(c.r, c.g, c.b)
}

fn put_text(buf: &mut Buffer, x: u16, y: u16, text: &str, fg: Color, bg: Color, bold: bool) {
    let mut style = Style::default().fg(rat_color(fg)).bg(rat_color(bg));
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    let mut cx = x;
    for ch in text.chars() {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
        cx = cx.saturating_add(1);
    }
}

fn put_styled(buf: &mut Buffer, x: u16, y: u16, st: &StyledText, default_fg: Color, bg: Color) {
    let mut cx = x;
    for span in &st.spans {
        let fg = span.fg.unwrap_or(default_fg);
        put_text(buf, cx, y, &span.text, fg, bg, span.bold);
        cx = cx.saturating_add(span.text.chars().count() as u16);
    }
}

fn draw_list(buf: &mut Buffer, area: Rect, list: &ListView) {
    use quadraui::ListItemMeasure;

    let bg = Color::rgb(20, 22, 30);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(rat_color(bg)));
            }
        }
    }

    let title_height: f32 = if list.title.is_some() { 1.0 } else { 0.0 };
    let layout = list.layout(
        area.width as f32,
        area.height as f32,
        title_height,
        |_| ListItemMeasure::new(1.0),
    );

    if let (Some(title), Some(tb)) = (list.title.as_ref(), layout.title_bounds) {
        put_styled(
            buf,
            area.x + tb.x as u16,
            area.y + tb.y as u16,
            title,
            Color::rgb(200, 200, 200),
            bg,
        );
    }

    for vis in &layout.visible_items {
        let item = &list.items[vis.item_idx];
        let is_sel = vis.item_idx == list.selected_idx;
        let row_bg = if is_sel {
            Color::rgb(50, 60, 90)
        } else {
            bg
        };
        let row_y = area.y + vis.bounds.y as u16;
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, row_y)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(rat_color(row_bg)));
            }
        }
        put_styled(
            buf,
            area.x + vis.bounds.x as u16,
            row_y,
            &item.text,
            Color::rgb(220, 220, 220),
            row_bg,
        );
        if let Some(detail) = item.detail.as_ref() {
            let detail_w: usize = detail.spans.iter().map(|s| s.text.chars().count()).sum();
            let dx = (area.x + area.width).saturating_sub(detail_w as u16 + 1);
            put_styled(buf, dx, row_y, detail, Color::rgb(180, 180, 180), row_bg);
        }
    }
}

fn draw_yaml(buf: &mut Buffer, area: Rect, yaml: &str, scroll: usize, has_focus: bool) {
    let bg = Color::rgb(16, 18, 24);
    let fg = Color::rgb(200, 200, 200);
    let key_fg = Color::rgb(140, 200, 240);
    let title_fg = if has_focus {
        Color::rgb(255, 220, 140)
    } else {
        key_fg
    };

    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(rat_color(bg)));
            }
        }
    }
    if area.height == 0 || area.width == 0 {
        return;
    }
    let header = if has_focus { " YAML  ◀ j/k" } else { " YAML" };
    put_text(buf, area.x, area.y, header, title_fg, bg, true);

    let inner = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    for (row_off, line) in yaml
        .lines()
        .skip(scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        let row_y = inner.y + row_off as u16;
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(colon) = trimmed.find(':') {
            let key = &line[..indent + colon];
            put_text(buf, inner.x, row_y, key, key_fg, bg, false);
            let value_x = inner.x.saturating_add(key.chars().count() as u16);
            let value = &line[indent + colon..];
            let max_w = (inner.width as usize)
                .saturating_sub(value_x.saturating_sub(inner.x) as usize);
            let value_clip: String = value.chars().take(max_w).collect();
            put_text(buf, value_x, row_y, &value_clip, fg, bg, false);
        } else {
            let max_w = inner.width as usize;
            let line_clip: String = line.chars().take(max_w).collect();
            put_text(buf, inner.x, row_y, &line_clip, fg, bg, false);
        }
    }
}

fn draw_status_bar(buf: &mut Buffer, area: Rect, bar: &StatusBar) {
    let bar_bg = Color::rgb(40, 40, 60);
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, area.y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().bg(rat_color(bar_bg)));
        }
    }
    let mut x = area.x;
    for seg in &bar.left_segments {
        let w = seg.text.chars().count() as u16;
        put_text(buf, x, area.y, &seg.text, seg.fg, seg.bg, seg.bold);
        x = x.saturating_add(w);
    }
    let right_w: u16 = bar
        .right_segments
        .iter()
        .map(|s| s.text.chars().count() as u16)
        .sum();
    let mut rx = area.x + area.width.saturating_sub(right_w);
    for seg in &bar.right_segments {
        let w = seg.text.chars().count() as u16;
        put_text(buf, rx, area.y, &seg.text, seg.fg, seg.bg, seg.bold);
        rx = rx.saturating_add(w);
    }
}

fn draw_picker(buf: &mut Buffer, area: Rect, list: &ListView) {
    let bg = Color::rgb(28, 32, 44);
    let border = Color::rgb(120, 160, 200);

    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(rat_color(bg)));
            }
        }
    }
    let h = area.height;
    let w = area.width;
    if h < 2 || w < 2 {
        return;
    }
    let style_b = Style::default().fg(rat_color(border)).bg(rat_color(bg));
    for x in 0..w {
        let ch_top = if x == 0 {
            '╭'
        } else if x == w - 1 {
            '╮'
        } else {
            '─'
        };
        let ch_bot = if x == 0 {
            '╰'
        } else if x == w - 1 {
            '╯'
        } else {
            '─'
        };
        if let Some(c) = buf.cell_mut(ratatui::layout::Position::new(area.x + x, area.y)) {
            c.set_char(ch_top);
            c.set_style(style_b);
        }
        if let Some(c) =
            buf.cell_mut(ratatui::layout::Position::new(area.x + x, area.y + h - 1))
        {
            c.set_char(ch_bot);
            c.set_style(style_b);
        }
    }
    for y in 1..h - 1 {
        if let Some(c) = buf.cell_mut(ratatui::layout::Position::new(area.x, area.y + y)) {
            c.set_char('│');
            c.set_style(style_b);
        }
        if let Some(c) =
            buf.cell_mut(ratatui::layout::Position::new(area.x + w - 1, area.y + y))
        {
            c.set_char('│');
            c.set_style(style_b);
        }
    }
    if let Some(title) = list.title.as_ref() {
        if let Some(span) = title.spans.first() {
            let tx = area.x + 2;
            put_text(buf, tx, area.y, &span.text, border, bg, true);
        }
    }
    for (i, item) in list.items.iter().enumerate() {
        let row_y = area.y + 1 + i as u16;
        if row_y >= area.y + h - 1 {
            break;
        }
        let is_sel = i == list.selected_idx;
        let row_bg = if is_sel { Color::rgb(60, 80, 120) } else { bg };
        for x in (area.x + 1)..(area.x + w - 1) {
            if let Some(c) = buf.cell_mut(ratatui::layout::Position::new(x, row_y)) {
                c.set_char(' ');
                c.set_style(Style::default().bg(rat_color(row_bg)));
            }
        }
        if let Some(span) = item.text.spans.first() {
            put_text(
                buf,
                area.x + 2,
                row_y,
                &span.text,
                Color::rgb(220, 220, 220),
                row_bg,
                span.bold,
            );
        }
    }
}

// ─── Event translation (crossterm → kubeui_core::Action) ─────────────────────

/// Translate a single crossterm key event into one or more `Action`s.
/// Picker-mode key handling is here in the backend (rather than in
/// the reducer) because what counts as "type a character" depends on
/// raw key info the reducer shouldn't see.
fn key_to_actions(state: &AppState, key: KeyCode) -> Vec<Action> {
    if state.picker.is_some() {
        return match key {
            KeyCode::Esc => vec![Action::PickerCancel],
            KeyCode::Enter => vec![Action::PickerCommit],
            KeyCode::Down => vec![Action::PickerMoveDown],
            KeyCode::Up => vec![Action::PickerMoveUp],
            KeyCode::Backspace => vec![Action::PickerBackspace],
            KeyCode::Char(ch) => vec![Action::PickerInput(ch)],
            _ => vec![],
        };
    }
    match key {
        KeyCode::Char('q') | KeyCode::Esc => vec![Action::Quit],
        KeyCode::Char('r') => vec![Action::Refresh],
        KeyCode::Char('n') => vec![Action::OpenNamespacePicker],
        KeyCode::Char('K') => vec![Action::OpenKindPicker],
        KeyCode::Tab | KeyCode::BackTab => vec![Action::ToggleFocus],
        KeyCode::Char('j') | KeyCode::Down => vec![Action::MoveDown],
        KeyCode::Char('k') | KeyCode::Up => vec![Action::MoveUp],
        KeyCode::PageDown => vec![Action::YamlPageDown],
        KeyCode::PageUp => vec![Action::YamlPageUp],
        _ => vec![],
    }
}

/// Resolve a left-click into one or more `Action`s. Walks the same
/// `quadraui` primitives the renderer drew so paint and click stay
/// in sync.
fn click_to_actions(
    state: &AppState,
    terminal_size: (u16, u16),
    col: u16,
    row: u16,
) -> Vec<Action> {
    let (term_w, term_h) = terminal_size;
    let viewport = quadraui::Rect::new(0.0, 0.0, term_w as f32, term_h as f32);

    // Picker (modal): click on row → select + commit; click outside → dismiss.
    if let Some(picker) = state.picker.as_ref() {
        let pb = picker_bounds(picker, viewport, 1.0, 1.0);
        let inside = (col as f32) >= pb.x
            && (col as f32) < pb.x + pb.width
            && (row as f32) >= pb.y
            && (row as f32) < pb.y + pb.height;
        if !inside {
            return vec![Action::PickerCancel];
        }
        let inner_row = row.saturating_sub(pb.y as u16);
        if inner_row > 0 && inner_row < pb.height as u16 - 1 {
            let visible_idx = (inner_row - 1) as usize;
            return vec![Action::PickerSelectVisible(visible_idx)];
        }
        return vec![];
    }

    // Status bar: bottom row only.
    if row + 1 == term_h {
        let bar = build_status_bar(state);
        if let Some(id) = bar.resolve_click(col, term_w as usize) {
            return vec![Action::StatusBarSegmentClicked(id)];
        }
    }
    vec![]
}

// ─── Main loop ──────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    kubeui_core::install_crypto_provider()?;
    let rt = tokio::runtime::Runtime::new()?;

    let context = rt
        .block_on(kubeui_core::current_context_name())
        .unwrap_or_else(|_| "<unknown>".to_string());

    let (namespaces, ns_status) = match rt.block_on(kubeui_core::list_namespaces()) {
        Ok(ns) => (ns, String::new()),
        Err(e) => (Vec::new(), format!("Namespace list failed: {e}")),
    };
    let ns_count = namespaces.len();

    let mut terminal = setup_terminal()?;
    let mut state = AppState::new(context, namespaces);
    state.status = if ns_count > 0 {
        format!("Found {ns_count} namespaces. Press r to load.")
    } else {
        ns_status
    };

    let result = run(&mut terminal, &mut state, &rt);
    teardown_terminal(&mut terminal)?;
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    rt: &tokio::runtime::Runtime,
) -> Result<()> {
    while !state.should_quit {
        terminal.draw(|frame| {
            let area = frame.area();
            let body_h = area.height.saturating_sub(1);
            let list_w = (area.width * 4 / 10).max(20).min(area.width);
            let list_area = Rect {
                x: area.x,
                y: area.y,
                width: list_w,
                height: body_h,
            };
            let yaml_area = Rect {
                x: area.x + list_w,
                y: area.y,
                width: area.width.saturating_sub(list_w),
                height: body_h,
            };
            let status_area = Rect {
                x: area.x,
                y: area.y + body_h,
                width: area.width,
                height: 1,
            };
            let list = build_list(state);
            draw_list(frame.buffer_mut(), list_area, &list);
            draw_yaml(
                frame.buffer_mut(),
                yaml_area,
                state.yaml_for_selected(),
                state.yaml_scroll,
                state.focus == Focus::Yaml,
            );
            let bar = build_status_bar(state);
            draw_status_bar(frame.buffer_mut(), status_area, &bar);
            if let Some(picker) = state.picker.as_ref() {
                let viewport =
                    quadraui::Rect::new(0.0, 0.0, area.width as f32, area.height as f32);
                let pb = picker_bounds(picker, viewport, 1.0, 1.0);
                let pb_rect = Rect {
                    x: pb.x as u16,
                    y: pb.y as u16,
                    width: pb.width as u16,
                    height: pb.height as u16,
                };
                let current = picker_current_index(state, picker.purpose);
                let view = build_picker(picker, current);
                draw_picker(frame.buffer_mut(), pb_rect, &view);
            }
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let term_size = terminal.size().map(|s| (s.width, s.height)).unwrap_or((80, 24));
        match event::read()? {
            Event::Key(key) => {
                for action in key_to_actions(state, key.code) {
                    apply_action(state, action, rt);
                }
            }
            Event::Mouse(me) => {
                if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                    for action in click_to_actions(state, term_size, me.column, me.row) {
                        apply_action(state, action, rt);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

// ─── Terminal setup / teardown ──────────────────────────────────────────────

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn teardown_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
