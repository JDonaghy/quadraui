//! Minimap `AppLogic` + `quadraui::{tui,gtk}::run` example ([`tui_minimap`]
//! / [`gtk_minimap`]).
//!
//! A static ~80-line "buffer" rendered as a code-overview minimap, plus a
//! status bar showing the current scroll position. Demonstrates
//! `sample_lines` (row down-sampling) and `aggregate_spans` (colour
//! down-sampling) feeding one `Minimap` descriptor that both backends
//! paint with zero backend-specific app code — GTK via font scaling, TUI
//! via braille (#382).
//!
//! Controls:
//! - Up/Down       scroll the viewport (moves the highlighted band)
//! - Click minimap seek the viewport to that fraction of the file
//! - q / Esc       quit

use quadraui::{
    aggregate_spans, sample_lines, AppLogic, Backend, Color, Key, Minimap, MinimapGrid, MinimapHit,
    MouseButton, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, SyntaxSpan, UiEvent,
    WidgetId,
};

/// Rows of the buffer visible in the (non-minimap) editor viewport —
/// stands in for a real editor's own scroll window.
const VIEWPORT_ROWS: usize = 10;

/// TUI packs 4 buffer lines into one braille row; GTK paints 1 buffer
/// line per row. This example doesn't know (or need to know) which
/// backend it's running under, so it samples generously at the denser
/// factor — both backends handle receiving more `lines` than they have
/// rows for by grouping/tiling, per `Minimap::layout`.
const LINES_PER_ROW: usize = 4;

pub struct MinimapApp {
    buffer: Vec<String>,
    scroll_offset: usize,
}

impl MinimapApp {
    pub fn new() -> Self {
        let buffer = (0..80)
            .map(|i| match i % 6 {
                0 => format!("fn function_{i}() {{"),
                1 => "    let value = compute();".to_string(),
                2 => "    // a short comment explaining it".to_string(),
                3 => String::new(),
                4 => "    value.process()".to_string(),
                _ => "}".to_string(),
            })
            .collect();
        Self {
            buffer,
            scroll_offset: 0,
        }
    }

    fn buffer_refs(&self) -> Vec<&str> {
        self.buffer.iter().map(String::as_str).collect()
    }

    /// Build the `Minimap` descriptor for the current scroll position.
    fn minimap(&self, target_rows: usize) -> Minimap {
        let refs = self.buffer_refs();
        let target = target_rows.saturating_mul(LINES_PER_ROW).max(1);
        let lines = sample_lines(&refs, target);

        // A couple of illustrative syntax spans — "fn" in one colour,
        // comments in another — aggregated down to whatever cell size
        // this backend actually paints.
        let raw_spans: Vec<SyntaxSpan> = lines
            .iter()
            .enumerate()
            .filter_map(|(idx, l)| {
                let trimmed = l.text.trim_start();
                if trimmed.starts_with("fn") {
                    Some(SyntaxSpan {
                        line_idx: idx,
                        start_col: 0,
                        end_col: 2,
                        color: Color::rgb(80, 160, 255),
                    })
                } else if trimmed.starts_with("//") {
                    Some(SyntaxSpan {
                        line_idx: idx,
                        start_col: 4,
                        end_col: l.text.len(),
                        color: Color::rgb(100, 180, 100),
                    })
                } else {
                    None
                }
            })
            .collect();
        let grid = MinimapGrid {
            rows: lines.len().div_ceil(LINES_PER_ROW).max(1),
            cols: 200,
            lines_per_row: LINES_PER_ROW,
            cols_per_cell: 2,
        };
        let syntax_spans = aggregate_spans(&raw_spans, grid);

        let visible_row_start = lines
            .iter()
            .position(|l| l.line_idx >= self.scroll_offset)
            .unwrap_or(0);
        let visible_row_end = lines
            .iter()
            .position(|l| l.line_idx >= self.scroll_offset + VIEWPORT_ROWS)
            .unwrap_or(lines.len());

        Minimap {
            id: WidgetId::new("minimap"),
            lines,
            syntax_spans,
            visible_row_start,
            visible_row_count: visible_row_end.saturating_sub(visible_row_start).max(1),
            total_buffer_lines: self.buffer.len(),
        }
    }

    /// The minimap's rect, in the same units [`Self::render`] and
    /// [`Self::handle`] both derive it from — recomputed rather than
    /// cached (`render` only has `&self`), mirroring `ChartApp`'s
    /// `MouseMoved` handler in `examples/common/chart_app.rs`.
    fn minimap_rect(&self, backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let minimap_w = backend.char_width() * 14.0;
        Rect::new(
            viewport.width - minimap_w,
            lh,
            minimap_w,
            viewport.height - lh * 2.0,
        )
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" Minimap demo — line {} ", self.scroll_offset),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " up/down=scroll click=seek q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn seek(&mut self, fraction: f32) {
        let max_start = self.buffer.len().saturating_sub(VIEWPORT_ROWS);
        let target = (fraction as f64 * max_start as f64).round() as usize;
        self.scroll_offset = target.min(max_start);
    }
}

impl Default for MinimapApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for MinimapApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let minimap_rect = self.minimap_rect(backend);
        let approx_rows = (minimap_rect.height / lh).max(1.0) as usize;
        let minimap = self.minimap(approx_rows);
        let _ = backend.draw_minimap(minimap_rect, &minimap);

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                let max_start = self.buffer.len().saturating_sub(VIEWPORT_ROWS);
                self.scroll_offset = (self.scroll_offset + 1).min(max_start);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                Reaction::Redraw
            }
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let minimap_rect = self.minimap_rect(backend);
                let lh = backend.line_height();
                let approx_rows = (minimap_rect.height / lh).max(1.0) as usize;
                let minimap = self.minimap(approx_rows);
                let layout = backend.minimap_layout(minimap_rect, &minimap);
                if let MinimapHit::Seek { fraction } = layout.hit_test(position.x, position.y) {
                    self.seek(fraction);
                }
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
