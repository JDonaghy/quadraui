//! Pure functions that turn [`crate::AppState`] into `quadraui`
//! primitives. [`crate::KubeApp::render`] calls these once per frame,
//! then rasterises the returned primitives through the generic
//! `quadraui::Backend` trait.
//!
//! All geometry is in abstract f32 units (cells for TUI, pixels for
//! GTK) — `Backend` converts as needed per backend.
//!
//! Colours are read from a `quadraui::Theme` rather than hardcoded, so
//! swapping in `Theme::light_default()` (or a VS Code theme loaded via
//! `Theme::from_vscode_json`) re-colours every pane here without
//! touching this file.

use quadraui::primitives::board::badge_fg_color;
use quadraui::{
    BadgeStatus, Color, Decoration, ListItem, ListView, StatusBar, StatusBarSegment, StyledSpan,
    StyledText, TextDisplay, TextDisplayLine, Theme, WidgetId,
};

use crate::state::{AppState, Focus};

/// Build the `ListView` primitive for the resource list pane.
pub fn build_list(state: &AppState, theme: &Theme) -> ListView {
    let items: Vec<ListItem> = state
        .resources
        .iter()
        .map(|r| ListItem {
            text: StyledText {
                spans: vec![StyledSpan::plain(r.name.clone())],
            },
            icon: None,
            detail: Some(StyledText {
                spans: vec![
                    StyledSpan {
                        text: r.status.clone(),
                        fg: Some(status_color(&r.status, theme)),
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
                fg: Some(theme.title_fg),
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
        h_scroll: 0,
        max_content_width: None,
        show_v_scrollbar: false,
    }
}

/// Build the `StatusBar` primitive. Segments carry `action_id`s so
/// the click handler can route via `StatusBar::layout`/`hit_test`
/// without duplicating segment math.
pub fn build_status_bar(state: &AppState, theme: &Theme) -> StatusBar {
    let bar_bg = theme.header_bg;
    let left = vec![
        StatusBarSegment {
            text: format!(" {} ", state.context),
            fg: theme.muted_fg,
            bg: bar_bg,
            bold: true,
            action_id: Some(WidgetId::new("status:context")),
        },
        StatusBarSegment {
            text: format!(" {} ", state.current_namespace()),
            fg: theme.accent_fg,
            bg: bar_bg,
            bold: false,
            action_id: Some(WidgetId::new("status:namespace")),
        },
        StatusBarSegment {
            text: format!(" {} ", state.kind.label()),
            fg: theme.header_fg,
            bg: bar_bg,
            bold: false,
            action_id: Some(WidgetId::new("status:kind")),
        },
    ];
    let right = vec![StatusBarSegment {
        text: format!(" {} ", state.status),
        fg: theme.foreground,
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
/// the `key:` prefix in `theme.accent_fg` and the trailing value in
/// the default `fg`. Lines without a `:` render plainly in the
/// default `fg`. `scroll_offset` is forwarded so backends can clamp
/// it via the primitive's layout.
///
/// The pane title (e.g. `" YAML"` / `" YAML  ◀ j/k"`) stays bespoke
/// here because it's a single-row decoration that depends on app
/// focus state and shouldn't scroll with the body.
pub fn build_yaml_view(state: &AppState, theme: &Theme) -> TextDisplay {
    let key_fg = theme.accent_fg;
    let has_focus = matches!(state.focus, Focus::Yaml);
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
    let title_text = if has_focus { " YAML  ◀ j/k" } else { " YAML" };
    // Focused pane gets the fuzzy-match highlight colour — already
    // the theme's "draw attention to this" tone — so focus reads the
    // same way regardless of which `Theme` the app is running.
    let title_fg = if has_focus { theme.match_fg } else { key_fg };
    TextDisplay {
        id: WidgetId::new("yaml"),
        lines,
        scroll_offset: state.yaml_scroll,
        // App owns scroll explicitly; auto-scroll is for log tails.
        auto_scroll: false,
        max_lines: 0,
        has_focus,
        title: Some(StyledText {
            spans: vec![StyledSpan {
                text: title_text.to_string(),
                fg: Some(title_fg),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            }],
        }),
        show_scrollbar: false,
    }
}

/// Heuristic colour for a status string, read off `theme`'s badge
/// tokens rather than hardcoded RGB — covers the strings each
/// resource kind emits today plus a sensible default. Mirrors
/// `BadgeStatus`'s pass/running/warning/blocked vocabulary:
/// - `Running`/`Succeeded`/`Active` → `Passed` (green)
/// - `Pending`/`ContainerCreating` → `Running` (in progress)
/// - `Failed`/`CrashLoopBackOff`/`Error` → `Blocked` (red)
/// - a `ready/total` ratio → `Passed` when equal, `Warning` otherwise
/// - anything else (e.g. a Service's `ClusterIP`/`NodePort`/…) is
///   informational, not a pass/fail state, so it reads from
///   `theme.link_fg` directly rather than forcing it through
///   `BadgeStatus`.
pub fn status_color(s: &str, theme: &Theme) -> Color {
    match s {
        "Running" | "Succeeded" | "Active" => badge_fg_color(BadgeStatus::Passed, theme),
        "Pending" | "ContainerCreating" => badge_fg_color(BadgeStatus::Running, theme),
        "Failed" | "CrashLoopBackOff" | "Error" => badge_fg_color(BadgeStatus::Blocked, theme),
        "ClusterIP" | "NodePort" | "LoadBalancer" | "ExternalName" => theme.link_fg,
        s if s.contains('/') => match s.split_once('/') {
            Some((a, b)) if a == b => badge_fg_color(BadgeStatus::Passed, theme),
            _ => badge_fg_color(BadgeStatus::Warning, theme),
        },
        _ => theme.muted_fg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_color_tracks_theme_badge_tokens() {
        let theme = Theme::default();
        assert_eq!(
            status_color("Running", &theme),
            badge_fg_color(BadgeStatus::Passed, &theme)
        );
        assert_eq!(
            status_color("CrashLoopBackOff", &theme),
            badge_fg_color(BadgeStatus::Blocked, &theme)
        );
    }

    #[test]
    fn status_color_differs_between_dark_and_light_themes() {
        let dark = Theme::default();
        let light = Theme::light_default();
        assert_ne!(
            status_color("Running", &dark),
            status_color("Running", &light)
        );
    }
}
