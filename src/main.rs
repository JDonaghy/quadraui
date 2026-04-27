//! kubeui — TUI Kubernetes dashboard.
//!
//! Backend-specific shell around [`kubeui_core`]. Owns terminal
//! setup/teardown, the crossterm event loop, and ratatui rasterisers
//! for each `quadraui` primitive the core builds. Everything else —
//! state, k8s client, view-builders, theme, click resolution, the
//! action reducer — lives in `kubeui-core` and is shared with
//! `kubeui-gtk`.

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
use ratatui::layout::Rect;
use ratatui::Terminal;

use kubeui_core::{
    apply_action, bootstrap_state, build_list, build_picker_menu, build_status_bar,
    build_yaml_view, picker_anchor, picker_current_index, picker_menu_width, resolve_click, theme,
    Action, AppState,
};
use quadraui::{Color, ContextMenuItemMeasure};

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

// ─── Main loop ──────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    kubeui_core::install_crypto_provider()?;
    let rt = tokio::runtime::Runtime::new()?;
    let mut state = bootstrap_state(&rt);
    let mut terminal = setup_terminal()?;
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
            quadraui::tui::draw_list(frame.buffer_mut(), list_area, &list, &theme(), false);
            let yaml = build_yaml_view(state);
            let yaml_theme = quadraui::Theme {
                background: Color::rgb(16, 18, 24),
                ..theme()
            };
            quadraui::tui::draw_text_display(frame.buffer_mut(), yaml_area, &yaml, &yaml_theme);
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
                    let viewport =
                        quadraui::Rect::new(0.0, 0.0, term_size.0 as f32, term_size.1 as f32);
                    for action in
                        resolve_click(state, viewport, me.column as f32, me.row as f32, 1.0, 1.0)
                    {
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
