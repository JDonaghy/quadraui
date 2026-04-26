//! Pure functions that turn [`crate::AppState`] into `quadraui`
//! primitives. Backends call these once per frame, then rasterise
//! the returned primitives.
//!
//! All geometry is in abstract f32 units (cells for TUI, pixels for
//! GTK). Backends supply the viewport size in their native unit and
//! convert as needed when drawing.

use quadraui::{
    Color, ContextMenu, ContextMenuItem, ContextMenuPlacement, Decoration, ListItem, ListView,
    Rect, StatusBar, StatusBarSegment, StatusSegmentMeasure, StatusSegmentSide, StyledSpan,
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

/// Build the [`ContextMenu`] primitive for an open picker. Items are
/// filtered by the picker's current query, with a `* ` prefix marking
/// the row that's "current" in the app (the namespace or kind the app
/// is on now). Placement is `Above` so the menu pops up over its
/// status-bar trigger like a real dropdown — the rasteriser auto-flips
/// to `Below` if the viewport is too short above.
///
/// Item ids are `picker:{orig_idx}` so the click handler can decode
/// which original index the user activated, even after query filtering
/// reorders the visible rows.
pub fn build_picker_menu(picker: &Picker, current_orig_idx: Option<usize>) -> ContextMenu {
    let visible = picker.visible_indices();
    let items: Vec<ContextMenuItem> = visible
        .iter()
        .map(|&orig| {
            let name = &picker.items[orig];
            let is_current = current_orig_idx == Some(orig);
            let prefix = if is_current { "* " } else { "  " };
            ContextMenuItem {
                id: Some(WidgetId::new(format!("picker:{orig}"))),
                label: StyledText {
                    spans: vec![StyledSpan {
                        text: format!("{prefix}{name}"),
                        fg: None,
                        bg: None,
                        bold: is_current,
                        italic: false,
                        underline: false,
                    }],
                },
                detail: None,
                disabled: false,
            }
        })
        .collect();
    ContextMenu {
        id: WidgetId::new("picker"),
        items,
        selected_idx: picker.selected,
        bg: None,
        placement: ContextMenuPlacement::Above,
    }
}

/// Decode a picker `ContextMenuHit::Item` id (e.g. `"picker:7"`) back
/// to the original item index. Returns `None` for ids that aren't
/// picker rows.
pub fn decode_picker_hit_id(id: &str) -> Option<usize> {
    id.strip_prefix("picker:").and_then(|s| s.parse().ok())
}

/// Width of the picker dropdown menu in the viewport's unit (cells for
/// TUI, pixels for GTK). Sized to fit the longest label + 4 cells of
/// breathing room (matching the old `picker_bounds` heuristic).
pub fn picker_menu_width(picker: &Picker, viewport: Rect, cell_w: f32) -> f32 {
    let max_label = picker
        .items
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(20)
        .max(20);
    // 4 cells: 2 borders + 2 padding.
    let w_cells = (max_label + 4) as f32;
    (w_cells * cell_w).min(viewport.width - 4.0 * cell_w)
}

/// Anchor rect for the open picker — the status-bar segment that
/// triggered it. Returns coordinates in the viewport's unit (cells for
/// TUI, pixels for GTK), assuming the status bar sits at the bottom
/// row of `viewport` with height `cell_h`.
///
/// Returns `None` if no picker is open or the status-bar segment
/// couldn't be located (shouldn't happen — `build_status_bar` always
/// emits the namespace and kind segments).
pub fn picker_anchor(state: &AppState, viewport: Rect, cell_w: f32, cell_h: f32) -> Option<Rect> {
    let purpose = state.picker.as_ref()?.purpose;
    let target = match purpose {
        PickerPurpose::Namespace => "status:namespace",
        PickerPurpose::ResourceKind => "status:kind",
    };
    let bar = build_status_bar(state);
    let bar_y = viewport.y + viewport.height - cell_h;
    let layout = bar.layout(viewport.width, cell_h, 2.0 * cell_w, |seg| {
        StatusSegmentMeasure::new(seg.text.chars().count() as f32 * cell_w)
    });
    for vis in &layout.visible_segments {
        let seg = match vis.side {
            StatusSegmentSide::Left => &bar.left_segments[vis.segment_idx],
            StatusSegmentSide::Right => &bar.right_segments[vis.segment_idx],
        };
        if seg
            .action_id
            .as_ref()
            .map(|id| id.as_str() == target)
            .unwrap_or(false)
        {
            return Some(Rect::new(
                vis.bounds.x + viewport.x,
                bar_y,
                vis.bounds.width,
                cell_h,
            ));
        }
    }
    None
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
