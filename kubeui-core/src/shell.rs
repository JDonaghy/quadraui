//! Backend-agnostic helpers shared between `kubeui` (TUI) and
//! `kubeui-gtk`. Each backend wires its native event source, surface,
//! and runtime; everything else — bootstrap, theme, click resolution —
//! lives here so both shells stay nearly identical and a third backend
//! only re-implements the truly platform-specific bits.

use quadraui::{Color, ContextMenuHit, ContextMenuItemMeasure, Rect as QRect, StatusBarHit, Theme};

use crate::state::AppState;
use crate::{
    build_picker_menu, build_status_bar, decode_picker_hit_id, picker_anchor, picker_current_index,
    picker_menu_width, Action,
};

/// Unified kubeui theme. Both backends consume this; a backend that
/// wants to override (lighter background for OLED screens, etc.) can
/// spread on top with `Theme { background: ..., ..kubeui_core::theme() }`.
///
/// The values match the TUI palette before the lift; GTK's pre-lift
/// background was 2 RGB units lighter and is now unified to TUI's
/// (visually a hair darker on GTK, indistinguishable in practice).
pub fn theme() -> Theme {
    Theme {
        background: Color::rgb(20, 22, 30),
        foreground: Color::rgb(220, 220, 220),
        selected_bg: Color::rgb(50, 60, 90),
        muted_fg: Color::rgb(180, 180, 180),
        surface_bg: Color::rgb(28, 32, 44),
        surface_fg: Color::rgb(220, 220, 220),
        border_fg: Color::rgb(120, 160, 200),
        title_fg: Color::rgb(120, 160, 200),
        header_fg: Color::rgb(160, 200, 240),
        ..Theme::default()
    }
}

/// Build the initial [`AppState`]: fetch the current context name and
/// the namespace list from the cluster, populate `state.status` with a
/// "found N namespaces" message, and return. Both backends call this
/// after [`crate::install_crypto_provider`] and before entering their
/// event loop.
pub fn bootstrap_state(rt: &tokio::runtime::Runtime) -> AppState {
    let context = rt
        .block_on(crate::current_context_name())
        .unwrap_or_else(|_| "<unknown>".to_string());
    let (namespaces, ns_status) = match rt.block_on(crate::list_namespaces()) {
        Ok(ns) => (ns, String::new()),
        Err(e) => (Vec::new(), format!("Namespace list failed: {e}")),
    };
    let ns_count = namespaces.len();
    let mut state = AppState::new(context, namespaces);
    state.status = if ns_count > 0 {
        format!("Found {ns_count} namespaces. Press r to load.")
    } else {
        ns_status
    };
    state
}

/// Resolve a left-click at viewport-relative coordinates into one or
/// more [`Action`]s. Walks the same `quadraui` primitives the renderer
/// drew so paint and click stay in sync.
///
/// `cell_w` / `cell_h` are the unit step sizes for the viewport (1.0
/// each for TUI cells, character-width / line-height pixels for GTK).
/// The status bar is assumed to live on the bottom row of `viewport`,
/// at height `cell_h`.
pub fn resolve_click(
    state: &AppState,
    viewport: QRect,
    x: f32,
    y: f32,
    cell_w: f32,
    cell_h: f32,
) -> Vec<Action> {
    // Picker (dropdown over status bar): hit-test the live menu layout
    // so click resolution stays in lock-step with paint. Click outside
    // → dismiss; click on row → decode the row id back to a visible idx.
    if let Some(picker) = state.picker.as_ref() {
        let Some(anchor) = picker_anchor(state, viewport, cell_w, cell_h) else {
            return vec![Action::PickerCancel];
        };
        let menu = build_picker_menu(picker, picker_current_index(state, picker.purpose));
        let menu_w = picker_menu_width(picker, viewport, cell_w);
        let menu_layout = menu.layout_at(anchor, viewport, menu_w, |_| {
            ContextMenuItemMeasure::new(cell_h)
        });
        return match menu_layout.hit_test(x, y) {
            ContextMenuHit::Item(id) => {
                if let Some(orig) = decode_picker_hit_id(id.as_str()) {
                    let visible = picker.visible_indices();
                    if let Some(visible_idx) = visible.iter().position(|&o| o == orig) {
                        return vec![Action::PickerSelectVisible(visible_idx)];
                    }
                }
                vec![]
            }
            ContextMenuHit::Inert => vec![],
            ContextMenuHit::Empty => vec![Action::PickerCancel],
        };
    }

    // Status bar: bottom `cell_h` of the viewport.
    let status_top = viewport.y + viewport.height - cell_h;
    if y >= status_top {
        let bar = build_status_bar(state);
        let layout = bar.layout(viewport.width, cell_h, 2.0 * cell_w, |seg| {
            quadraui::StatusSegmentMeasure::new(seg.text.chars().count() as f32 * cell_w)
        });
        if let StatusBarHit::Segment(id) = layout.hit_test(x, y - status_top) {
            return vec![Action::StatusBarSegmentClicked(id)];
        }
    }
    vec![]
}
