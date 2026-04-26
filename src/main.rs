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
    apply_action, build_list, build_picker_menu, build_status_bar, decode_picker_hit_id,
    picker_anchor, picker_current_index, picker_menu_width, Action, AppState, Focus,
};
use quadraui::{Color, ContextMenuHit, ContextMenuItemMeasure, ListView};

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

fn draw_list(buf: &mut Buffer, area: Rect, list: &ListView) {
    quadraui::tui::draw_list(buf, area, list, &theme(), false);
}

/// Draw the YAML pane: bespoke title row + delegated `TextDisplay`
/// body. Title stays in the binary because it depends on focus state
/// and shouldn't scroll with the body.
fn draw_yaml(buf: &mut Buffer, area: Rect, state: &AppState) {
    let bg = Color::rgb(16, 18, 24);
    let key_fg = Color::rgb(140, 200, 240);
    let has_focus = state.focus == Focus::Yaml;
    let title_fg = if has_focus {
        Color::rgb(255, 220, 140)
    } else {
        key_fg
    };

    if area.height == 0 || area.width == 0 {
        return;
    }

    // Title row: bespoke, doesn't scroll.
    let header = if has_focus { " YAML  ◀ j/k" } else { " YAML" };
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, area.y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().bg(rat_color(bg)));
        }
    }
    put_text(buf, area.x, area.y, header, title_fg, bg, true);

    // Body: delegated to `quadraui::tui::draw_text_display`. Use a
    // YAML-pane-specific theme so the body bg is the slightly-darker
    // (16, 18, 24) instead of the unified theme bg.
    let body = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    let display = kubeui_core::build_yaml_view(state);
    let yaml_theme = quadraui::Theme {
        background: bg,
        ..theme()
    };
    quadraui::tui::draw_text_display(buf, body, &display, &yaml_theme);
}

/// Theme used for the public quadraui rasterisers. kubeui's palette is
/// hardcoded; this maps the relevant subset to `quadraui::Theme` fields
/// so the public `draw_*` rasterisers paint with kubeui's colours.
fn theme() -> quadraui::Theme {
    quadraui::Theme {
        // List background — also fallback for empty StatusBar (rare).
        background: Color::rgb(20, 22, 30),
        foreground: Color::rgb(220, 220, 220),
        // Selected row in the resource list.
        selected_bg: Color::rgb(50, 60, 90),
        // Detail-column dim text.
        muted_fg: Color::rgb(180, 180, 180),
        // Bordered surfaces (picker modal).
        surface_bg: Color::rgb(28, 32, 44),
        surface_fg: Color::rgb(220, 220, 220),
        border_fg: Color::rgb(120, 160, 200),
        title_fg: Color::rgb(120, 160, 200),
        ..quadraui::Theme::default()
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

    // Picker (dropdown over status bar): click on row → select + commit;
    // click outside → dismiss. The menu hit-test handles bounds + per-row
    // resolution; we decode the row id back to its visible index.
    if let Some(picker) = state.picker.as_ref() {
        let Some(anchor) = picker_anchor(state, viewport, 1.0, 1.0) else {
            return vec![Action::PickerCancel];
        };
        let menu = build_picker_menu(picker, picker_current_index(state, picker.purpose));
        let menu_w = picker_menu_width(picker, viewport, 1.0);
        let menu_layout = menu.layout_at(anchor, viewport, menu_w, |_| {
            ContextMenuItemMeasure::new(1.0)
        });
        match menu_layout.hit_test(col as f32, row as f32) {
            ContextMenuHit::Item(id) => {
                if let Some(orig) = decode_picker_hit_id(id.as_str()) {
                    let visible = picker.visible_indices();
                    if let Some(visible_idx) = visible.iter().position(|&o| o == orig) {
                        return vec![Action::PickerSelectVisible(visible_idx)];
                    }
                }
                return vec![];
            }
            ContextMenuHit::Inert => return vec![],
            ContextMenuHit::Empty => return vec![Action::PickerCancel],
        }
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
            draw_yaml(frame.buffer_mut(), yaml_area, state);
            let bar = build_status_bar(state);
            let bar_layout = bar.layout(status_area.width as f32, 1.0, 2.0, |seg| {
                quadraui::StatusSegmentMeasure::new(seg.text.chars().count() as f32)
            });
            quadraui::tui::draw_status_bar(
                frame.buffer_mut(),
                status_area,
                &bar,
                &bar_layout,
                &theme(),
            );
            if let Some(picker) = state.picker.as_ref() {
                let viewport = quadraui::Rect::new(0.0, 0.0, area.width as f32, area.height as f32);
                if let Some(anchor) = picker_anchor(state, viewport, 1.0, 1.0) {
                    let current = picker_current_index(state, picker.purpose);
                    let menu = build_picker_menu(picker, current);
                    let menu_w = picker_menu_width(picker, viewport, 1.0);
                    let menu_layout = menu.layout_at(anchor, viewport, menu_w, |_| {
                        ContextMenuItemMeasure::new(1.0)
                    });
                    quadraui::tui::draw_context_menu(
                        frame.buffer_mut(),
                        &menu,
                        &menu_layout,
                        &theme(),
                    );
                }
            }
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let term_size = terminal
            .size()
            .map(|s| (s.width, s.height))
            .unwrap_or((80, 24));
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
