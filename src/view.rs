//! Pure functions that turn [`crate::AppState`] into `quadraui`
//! primitives. Backends call these once per frame, then rasterise
//! the returned primitives.
//!
//! All geometry is in abstract f32 units (cells for TUI, pixels for
//! GTK). Backends supply the viewport size in their native unit and
//! convert as needed when drawing.

use quadraui::{
    Color, Decoration, ListItem, ListView, Rect, StatusBar, StatusBarSegment, StyledSpan,
    StyledText, TextDisplay, TextDisplayLine, WidgetId,
};

use crate::state::{AppState, Focus, Picker, PickerPurpose, ResourceKind};

/// Build the `ListView` primitive for the resource list pane.
pub fn build_list(state: &AppState) -> ListView {
    let items: Vec<ListItem> = state
        .resources
        .iter()
        .map(|r| ListItem {
            text: StyledText {
                spans: vec![StyledSpan {
                    text: r.name.clone(),
                    fg: None,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                }],
            },
            icon: None,
            detail: Some(StyledText {
                spans: vec![
                    StyledSpan {
                        text: r.status.clone(),
                        fg: Some(status_color(&r.status)),
                        bg: None,
                        bold: false,
                        italic: false,
                        underline: false,
                    },
                    StyledSpan {
                        text: format!("  {}", r.age),
                        fg: None,
                        bg: None,
                        bold: false,
                        italic: false,
                        underline: false,
                    },
                ],
            }),
            decoration: Decoration::Normal,
        })
        .collect();

    let title_text = format!(
        " {} in {} ({})",
        state.kind.label(),
        state.current_namespace(),
        state.resources.len()
    );
    ListView {
        id: WidgetId::new("resources"),
        title: Some(StyledText {
            spans: vec![StyledSpan {
                text: title_text,
                fg: Some(Color::rgb(160, 200, 240)),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            }],
        }),
        items,
        selected_idx: state.selected,
        scroll_offset: 0,
        has_focus: state.focus == Focus::Resources,
        bordered: false,
    }
}

/// Build the `StatusBar` primitive. Segments carry `action_id`s so
/// each backend's click handler can route via
/// [`StatusBar::resolve_click`] without duplicating segment math.
pub fn build_status_bar(state: &AppState) -> StatusBar {
    let bar_bg = Color::rgb(40, 40, 60);
    let left = vec![
        StatusBarSegment {
            text: format!(" {} ", state.context),
            fg: Color::rgb(180, 180, 200),
            bg: bar_bg,
            bold: true,
            action_id: Some(WidgetId::new("status:context")),
        },
        StatusBarSegment {
            text: format!(" {} ", state.current_namespace()),
            fg: Color::rgb(140, 220, 180),
            bg: bar_bg,
            bold: false,
            action_id: Some(WidgetId::new("status:namespace")),
        },
        StatusBarSegment {
            text: format!(" {} ", state.kind.label()),
            fg: Color::rgb(220, 200, 140),
            bg: bar_bg,
            bold: false,
            action_id: Some(WidgetId::new("status:kind")),
        },
    ];
    let right = vec![StatusBarSegment {
        text: format!(" {} ", state.status),
        fg: Color::rgb(200, 200, 200),
        bg: bar_bg,
        bold: false,
        action_id: None,
    }];
    StatusBar {
        id: WidgetId::new("status"),
        left_segments: left,
        right_segments: right,
    }
}

/// Build the `TextDisplay` primitive for the YAML pane.
///
/// Each YAML line becomes a `TextDisplayLine` with two styled spans:
/// the `key:` prefix in `key_fg` and the trailing value in the default
/// `fg`. Lines without a `:` render plainly in the default `fg`.
/// `scroll_offset` is forwarded so backends can clamp it via the
/// primitive's layout.
///
/// The pane title (e.g. `" YAML"` / `" YAML  ◀ j/k"`) stays bespoke
/// in each binary because it's a single-row decoration that depends
/// on app focus state and shouldn't scroll with the body.
pub fn build_yaml_view(state: &AppState) -> TextDisplay {
    let key_fg = Color::rgb(140, 200, 240);
    let lines: Vec<TextDisplayLine> = state
        .yaml_for_selected()
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            let spans = if let Some(colon) = trimmed.find(':') {
                let key = &line[..indent + colon];
                let value = &line[indent + colon..];
                vec![
                    StyledSpan {
                        text: key.to_string(),
                        fg: Some(key_fg),
                        bg: None,
                        bold: false,
                        italic: false,
                        underline: false,
                    },
                    StyledSpan::plain(value),
                ]
            } else {
                vec![StyledSpan::plain(line)]
            };
            TextDisplayLine {
                spans,
                decoration: Decoration::Normal,
                timestamp: None,
            }
        })
        .collect();
    TextDisplay {
        id: WidgetId::new("yaml"),
        lines,
        scroll_offset: state.yaml_scroll,
        // App owns scroll explicitly; auto-scroll is for log tails.
        auto_scroll: false,
        max_lines: 0,
        has_focus: matches!(state.focus, Focus::Yaml),
    }
}

/// Build the bordered `ListView` primitive for an open picker. Items
/// are filtered by the picker's current query before building, so
/// the visible rows match what the user typed in. The `* ` prefix
/// marks the row that's "current" in the app (the namespace or kind
/// the app is on now).
pub fn build_picker(picker: &Picker, current_orig_idx: Option<usize>) -> ListView {
    let visible = picker.visible_indices();
    let items: Vec<ListItem> = visible
        .iter()
        .map(|&orig| {
            let name = &picker.items[orig];
            let is_current = current_orig_idx == Some(orig);
            let prefix = if is_current { "* " } else { "  " };
            ListItem {
                text: StyledText {
                    spans: vec![StyledSpan {
                        text: format!("{prefix}{name}"),
                        fg: None,
                        bg: None,
                        bold: is_current,
                        italic: false,
                        underline: false,
                    }],
                },
                icon: None,
                detail: None,
                decoration: Decoration::Normal,
            }
        })
        .collect();
    let title_text = if picker.query.is_empty() {
        picker.title.clone()
    } else {
        format!("{}— {}", picker.title, picker.query)
    };
    ListView {
        id: WidgetId::new("picker"),
        title: Some(StyledText {
            spans: vec![StyledSpan {
                text: title_text,
                fg: Some(Color::rgb(160, 200, 240)),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            }],
        }),
        items,
        selected_idx: picker.selected,
        scroll_offset: 0,
        has_focus: true,
        bordered: true,
    }
}

/// Compute the centered bounds of an open picker in the given viewport.
/// Result is in the same f32 unit as the viewport (cells for TUI,
/// pixels for GTK). Both backends call this so paint and click
/// hit-test agree on geometry.
///
/// For TUI, `cell_unit` should be 1.0; for GTK, callers pass a
/// `(char_width, line_height)` pair so the picker still displays at a
/// readable size in pixels.
pub fn picker_bounds(picker: &Picker, viewport: Rect, cell_w: f32, cell_h: f32) -> Rect {
    let max_label = picker
        .items
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(20)
        .max(20);
    // 4 cells of breathing room: 2 borders + 2 padding.
    let w_cells = (max_label + 4) as f32;
    let w_px = (w_cells * cell_w).min(viewport.width - 4.0 * cell_w);
    // +2 rows for top/bottom border.
    let h_rows = (picker.items.len() as f32 + 2.0)
        .min((viewport.height - 4.0 * cell_h) / cell_h)
        .max(3.0);
    let h_px = h_rows * cell_h;
    let x = viewport.x + (viewport.width - w_px) / 2.0;
    let y = viewport.y + (viewport.height - h_px) / 2.0;
    Rect::new(x, y, w_px, h_px)
}

/// Pre-selected row index when opening a picker — so the `* ` marker
/// in [`build_picker`] lands on the right row. Returned as
/// `Option<usize>` because `ResourceKind::ALL.position(…)` could
/// theoretically return `None` (won't in practice).
pub fn picker_current_index(state: &AppState, purpose: PickerPurpose) -> Option<usize> {
    match purpose {
        PickerPurpose::Namespace => Some(state.current_ns),
        PickerPurpose::ResourceKind => ResourceKind::ALL.iter().position(|k| *k == state.kind),
    }
}

/// Heuristic colour for a status string. Covers the strings each
/// kind emits today plus a sensible default.
pub fn status_color(s: &str) -> Color {
    match s {
        "Running" | "Succeeded" | "Active" => Color::rgb(140, 220, 140),
        "Pending" | "ContainerCreating" => Color::rgb(220, 200, 120),
        "Failed" | "CrashLoopBackOff" | "Error" => Color::rgb(220, 120, 120),
        "ClusterIP" | "NodePort" | "LoadBalancer" | "ExternalName" => Color::rgb(160, 200, 240),
        s if s.contains('/') => match s.split_once('/') {
            Some((a, b)) if a == b => Color::rgb(140, 220, 140),
            _ => Color::rgb(220, 200, 120),
        },
        _ => Color::rgb(180, 180, 180),
    }
}
