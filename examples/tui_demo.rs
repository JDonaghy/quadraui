//! `cargo run --example tui_demo`
//!
//! A self-contained demonstration of two quadraui primitives running
//! against a tiny ratatui-based backend. Shows the patterns described
//! in `BACKEND.md`:
//!
//! - **`TabBar` contract**: pre-measure each tab in cell-units, call
//!   [`TabBar::fit_active_scroll_offset`] to find the correct offset
//!   for the current width, write back to app state, repaint if it
//!   changed (the "two-pass paint" — even in TUI, where the loop
//!   eventually re-renders, the inline second paint eliminates the
//!   one-frame visual artifact).
//! - **`StatusBar` contract**: call [`StatusBar::fit_right_start`] with
//!   a cell-count measurer + a 2-cell minimum gap, render only the
//!   visible slice. Click handling skips the dropped segments via
//!   [`StatusBar::resolve_click_fit_chars`].
//! - **Event flow**: keys/clicks become enum events → mutate app state
//!   → next paint reflects the change. No closures cross the
//!   primitive boundary.
//!
//! Controls:
//! - `←` / `→`         — switch tab
//! - `n`               — open a new tab
//! - `x`               — close the active tab
//! - `Tab` (Shift-Tab) — focus next/previous status segment
//! - `Enter`           — activate the focused status segment
//! - `q` / `Esc`       — quit
//!
//! Resize the terminal narrow + wide while many tabs are open to see
//! the auto-scroll-to-active behaviour and the right-segment drop.

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RatColor, Modifier, Style};
use ratatui::Terminal;

use quadraui::{Color, StatusBar, StatusBarSegment, TabBar, TabBarSegment, TabItem, WidgetId};

// ─── App state ───────────────────────────────────────────────────────────────

/// All persistent state lives here. Each frame:
/// 1. We build quadraui primitives FROM this state.
/// 2. We render the primitives.
/// 3. Events MUTATE this state.
struct AppState {
    tabs: Vec<String>,
    active_tab: usize,
    /// Authoritative tab scroll offset. The `TabBar` primitive's
    /// `scroll_offset` field is the *input* per frame — this field is
    /// where we store the value the backend computed and wrote back via
    /// the `TabBar` contract.
    tab_scroll_offset: usize,
    /// Which clickable status-bar segment currently has keyboard focus
    /// (highlighted by underline). Tab cycles through the visible right
    /// segments.
    focused_status_idx: usize,
    /// Last action triggered, shown in the status message.
    last_message: String,
}

impl AppState {
    fn new() -> Self {
        Self {
            tabs: vec![
                "main.rs".into(),
                "lib.rs".into(),
                "Cargo.toml".into(),
                "README.md".into(),
                "tests.rs".into(),
            ],
            active_tab: 0,
            tab_scroll_offset: 0,
            focused_status_idx: 0,
            last_message: "ready".into(),
        }
    }

    fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
        }
    }
    fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
        }
    }
    fn open_tab(&mut self) {
        let n = self.tabs.len() + 1;
        self.tabs.push(format!("scratch-{n}.txt"));
        self.active_tab = self.tabs.len() - 1;
    }
    fn close_active(&mut self) {
        if self.tabs.len() > 1 {
            self.tabs.remove(self.active_tab);
            if self.active_tab >= self.tabs.len() {
                self.active_tab = self.tabs.len() - 1;
            }
        }
    }
}

// ─── Primitive builders (state → primitive) ──────────────────────────────────

fn build_tab_bar(state: &AppState) -> TabBar {
    let tabs = state
        .tabs
        .iter()
        .enumerate()
        .map(|(i, name)| TabItem {
            label: format!(" {}: {} ", i + 1, name),
            is_active: i == state.active_tab,
            is_dirty: false,
            is_preview: false,
        })
        .collect();
    TabBar {
        id: WidgetId::new("tabs:editor"),
        tabs,
        scroll_offset: state.tab_scroll_offset,
        right_segments: vec![TabBarSegment {
            text: " + ".into(),
            width_cells: 3,
            id: Some(WidgetId::new("tab:new")),
            is_active: false,
        }],
        active_accent: Some(Color::rgb(80, 160, 240)),
    }
}

fn build_status_bar(state: &AppState, focused_id: Option<&str>) -> StatusBar {
    let bar_fg = Color::rgb(220, 220, 220);
    let bar_bg = Color::rgb(40, 40, 60);
    let mode_fg = Color::rgb(80, 200, 120);

    let make = |text: String, action: Option<&str>| StatusBarSegment {
        text,
        fg: bar_fg,
        bg: bar_bg,
        bold: false,
        action_id: action.map(WidgetId::new),
    };

    let left = vec![
        StatusBarSegment {
            text: " DEMO ".into(),
            fg: mode_fg,
            bg: bar_bg,
            bold: true,
            action_id: None,
        },
        make(
            format!(
                " {} ",
                state
                    .tabs
                    .get(state.active_tab)
                    .cloned()
                    .unwrap_or_default()
            ),
            None,
        ),
    ];

    // Right segments — built least-important first, most-important last.
    // `fit_right_start` drops from the front when the bar is narrow,
    // so cursor position (the rightmost segment) always stays visible.
    // See StatusBar's "Backend contract" rustdoc.
    let mut right = vec![
        make(format!(" {} ", state.last_message), Some("status:dismiss")),
        make(" UTF-8 ".into(), Some("status:encoding")),
        make(" LF ".into(), Some("status:line_ending")),
        make(
            format!(" Tab {} ", state.active_tab + 1),
            Some("status:goto"),
        ),
        make(" rust ".into(), Some("status:language")),
    ];

    // Mark the focused segment by toggling bold (a backend's per-frame
    // interaction state — passed alongside the primitive, not inside it).
    if let Some(fid) = focused_id {
        for seg in &mut right {
            if let Some(ref id) = seg.action_id {
                if id.as_str() == fid {
                    seg.bold = true;
                }
            }
        }
    }

    StatusBar {
        id: WidgetId::new("status:editor"),
        left_segments: left,
        right_segments: right,
    }
}

// ─── Backend (primitive → ratatui buffer) ────────────────────────────────────

fn rat_color(c: Color) -> RatColor {
    RatColor::Rgb(c.r, c.g, c.b)
}

/// Fill `area` with bg, then write `text` left-aligned.
fn put_segment(buf: &mut Buffer, x: u16, y: u16, text: &str, fg: Color, bg: Color, bold: bool) {
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

/// Draw a `TabBar` and return the offset that would actually keep the
/// active tab visible — caller compares to `bar.scroll_offset` and
/// triggers a repaint if they differ. This is the TabBar contract.
fn draw_tab_bar(buf: &mut Buffer, area: Rect, bar: &TabBar) -> usize {
    // Clear the row.
    let bar_bg = RatColor::Rgb(20, 20, 30);
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, area.y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().bg(bar_bg));
        }
    }

    // Reserve the right-segment area first.
    let right_w: u16 = bar.right_segments.iter().map(|s| s.width_cells).sum();
    let tab_area_w = area.width.saturating_sub(right_w);

    // Pre-measure each tab in cell counts (the unit we'll pass to
    // `fit_active_scroll_offset`). Same widths get reused for the paint
    // loop so there's no double-measure.
    let tab_widths: Vec<usize> = bar.tabs.iter().map(|t| t.label.chars().count()).collect();

    // Compute the correct scroll offset for the *current* width using
    // actual measured tab widths in cells.
    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let correct_offset = if let Some(active) = active_idx {
        TabBar::fit_active_scroll_offset(active, bar.tabs.len(), tab_area_w as usize, |i| {
            tab_widths[i]
        })
    } else {
        bar.scroll_offset
    };

    // Paint visible tabs starting from the primitive's *input* offset
    // (the corrected offset only matters for the next paint).
    let mut x = area.x;
    for (i, tab) in bar.tabs.iter().enumerate().skip(bar.scroll_offset) {
        let w = tab_widths[i] as u16;
        if x + w > area.x + tab_area_w {
            break;
        }
        let fg = if tab.is_active {
            RatColor::Rgb(255, 255, 255)
        } else {
            RatColor::Rgb(160, 160, 180)
        };
        let bg = if tab.is_active {
            RatColor::Rgb(50, 80, 130)
        } else {
            bar_bg
        };
        let style = if tab.is_active {
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg).bg(bg)
        };
        for (offset, ch) in tab.label.chars().enumerate() {
            if let Some(cell) =
                buf.cell_mut(ratatui::layout::Position::new(x + offset as u16, area.y))
            {
                cell.set_char(ch);
                cell.set_style(style);
            }
        }
        x += w;
    }

    // Right segments — paint at the right edge.
    let mut rx = area.x + tab_area_w;
    for seg in &bar.right_segments {
        for (i, ch) in seg.text.chars().enumerate() {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(rx + i as u16, area.y))
            {
                cell.set_char(ch);
                cell.set_style(
                    Style::default()
                        .fg(RatColor::Rgb(200, 200, 200))
                        .bg(RatColor::Rgb(60, 60, 80)),
                );
            }
        }
        rx += seg.width_cells;
    }

    correct_offset
}

/// Draw a `StatusBar` honouring the narrow-width contract: drop low-
/// priority right segments from the front so the rightmost (highest
/// priority) ones stay visible at the right edge with a 2-cell gap.
fn draw_status_bar(buf: &mut Buffer, area: Rect, bar: &StatusBar) {
    // Background fill.
    let fill = bar
        .left_segments
        .first()
        .map(|s| rat_color(s.bg))
        .unwrap_or(RatColor::Rgb(40, 40, 60));
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, area.y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().bg(fill));
        }
    }

    // Left segments — accumulate from area.x.
    let mut cx = area.x;
    for seg in &bar.left_segments {
        let w = seg.text.chars().count() as u16;
        put_segment(buf, cx, area.y, &seg.text, seg.fg, seg.bg, seg.bold);
        cx = cx.saturating_add(w);
    }

    // Right segments — drop from the front via fit_right_start.
    // The 2-cell gap is the StatusBar contract's recommended minimum
    // separation between left and right halves.
    const MIN_GAP_CELLS: usize = 2;
    let start = bar.fit_right_start_chars(area.width as usize, MIN_GAP_CELLS);
    let visible = &bar.right_segments[start..];
    let right_width: u16 = visible.iter().map(|s| s.text.chars().count() as u16).sum();
    let mut rx = (area.x + area.width).saturating_sub(right_width);
    for seg in visible {
        let w = seg.text.chars().count() as u16;
        put_segment(buf, rx, area.y, &seg.text, seg.fg, seg.bg, seg.bold);
        rx = rx.saturating_add(w);
    }
}

// ─── Event handling (event → state mutation) ─────────────────────────────────

fn focused_segment_id(state: &AppState) -> Option<String> {
    let bar = build_status_bar(state, None);
    let interactive: Vec<&StatusBarSegment> = bar
        .right_segments
        .iter()
        .filter(|s| s.action_id.is_some())
        .collect();
    interactive
        .get(state.focused_status_idx)
        .and_then(|s| s.action_id.as_ref())
        .map(|id| id.as_str().to_string())
}

fn handle_status_action(state: &mut AppState, id: &str) {
    state.last_message = match id {
        "status:dismiss" => "dismissed".into(),
        "status:encoding" => "encoding picker (mock)".into(),
        "status:line_ending" => "line-ending picker (mock)".into(),
        "status:goto" => format!("go-to-tab picker (active = {})", state.active_tab + 1),
        "status:language" => "language picker (mock)".into(),
        _ => format!("unknown action: {}", id),
    };
}

// ─── Main loop ───────────────────────────────────────────────────────────────

fn main() -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut state = AppState::new();
    let result = run(&mut terminal, &mut state);
    teardown_terminal(&mut terminal)?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, state: &mut AppState) -> io::Result<()> {
    loop {
        // ── Two-pass paint (TabBar contract step 4) ──────────────────────
        let painted_offset = paint_once(terminal, state)?;
        if painted_offset != state.tab_scroll_offset {
            // The backend's measurement says the engine's offset would
            // hide the active tab. Update state and repaint immediately
            // so the user never sees the stale frame.
            state.tab_scroll_offset = painted_offset;
            paint_once(terminal, state)?;
        }

        // ── Event handling ───────────────────────────────────────────────
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) => match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Right => state.next_tab(),
                    KeyCode::Left => state.prev_tab(),
                    KeyCode::Char('n') => state.open_tab(),
                    KeyCode::Char('x') => state.close_active(),
                    KeyCode::Tab => {
                        // Cycle through visible interactive status segments.
                        let count = build_status_bar(state, None)
                            .right_segments
                            .iter()
                            .filter(|s| s.action_id.is_some())
                            .count();
                        if count > 0 {
                            state.focused_status_idx = (state.focused_status_idx + 1) % count;
                        }
                    }
                    KeyCode::BackTab => {
                        let count = build_status_bar(state, None)
                            .right_segments
                            .iter()
                            .filter(|s| s.action_id.is_some())
                            .count();
                        if count > 0 {
                            state.focused_status_idx =
                                (state.focused_status_idx + count - 1) % count;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(id) = focused_segment_id(state) {
                            handle_status_action(state, &id);
                        }
                    }
                    _ => {}
                },
                Event::Mouse(_) => { /* ignored in this demo */ }
                Event::Resize(_, _) => { /* re-paint on next loop */ }
                _ => {}
            }
        }
    }
}

/// Paint one frame. Returns the `correct_scroll_offset` the TabBar
/// computed for this width — caller compares to `state.tab_scroll_offset`
/// to decide whether a second pass is needed.
fn paint_once(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &AppState,
) -> io::Result<usize> {
    let mut painted_offset = state.tab_scroll_offset;
    terminal.draw(|f| {
        let area = f.area();
        if area.height < 4 {
            return;
        }
        let buf = f.buffer_mut();

        // Layout: [tab bar][body...][status bar][hint row]
        let tab_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        let status_area = Rect {
            x: area.x,
            y: area.y + area.height - 2,
            width: area.width,
            height: 1,
        };
        let hint_area = Rect {
            x: area.x,
            y: area.y + area.height - 1,
            width: area.width,
            height: 1,
        };
        let body_area = Rect {
            x: area.x,
            y: tab_area.y + 1,
            width: area.width,
            height: status_area.y.saturating_sub(tab_area.y + 1),
        };

        // Build + draw tab bar; capture the corrected scroll offset.
        let tab_bar = build_tab_bar(state);
        painted_offset = draw_tab_bar(buf, tab_area, &tab_bar);

        // Body — show which tab is active.
        let body_msg = format!(
            " You are viewing tab {} of {} — \"{}\"",
            state.active_tab + 1,
            state.tabs.len(),
            state
                .tabs
                .get(state.active_tab)
                .cloned()
                .unwrap_or_default()
        );
        put_segment(
            buf,
            body_area.x,
            body_area.y + 1,
            &body_msg,
            Color::rgb(220, 220, 220),
            Color::rgb(20, 20, 30),
            false,
        );

        // Build + draw status bar.
        let focused = focused_segment_id(state);
        let status_bar = build_status_bar(state, focused.as_deref());
        draw_status_bar(buf, status_area, &status_bar);

        // Hint row.
        put_segment(
            buf,
            hint_area.x,
            hint_area.y,
            " ←/→ tab • n new • x close • Tab cycle status • Enter activate • q quit ",
            Color::rgb(160, 160, 180),
            Color::rgb(20, 20, 30),
            false,
        );
    })?;
    Ok(painted_offset)
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn teardown_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}
