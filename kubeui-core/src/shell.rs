//! Backend-agnostic bootstrap shared by every `quadraui::AppLogic`
//! consumer of this crate ([`crate::KubeApp`]).
//!
//! Click resolution and event handling live on `KubeApp` itself now
//! (both backends run through the same `quadraui::tui::run` /
//! `quadraui::gtk::run` loop, so there's no second copy to keep in
//! sync) — this module is left with the bits that are genuinely just
//! "do this once before the event loop starts".

use quadraui::{Color, Theme};

use crate::state::AppState;

/// Unified kubeui theme. Spread `..kubeui_core::theme()` to override
/// individual fields (lighter background for OLED screens, etc.), or
/// use `Theme::light_default()` / `Theme::from_vscode_json` directly —
/// every colour [`crate::view`] draws with comes from whichever
/// `Theme` [`crate::KubeApp`] is constructed with.
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
/// "found N namespaces" message, and return. [`crate::KubeApp::new`]
/// calls this after [`crate::install_crypto_provider`] and before
/// handing off to the runner's event loop.
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
