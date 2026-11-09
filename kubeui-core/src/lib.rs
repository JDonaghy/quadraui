//! Backend-agnostic core for kubeui.
//!
//! [`KubeApp`] implements `quadraui::AppLogic`; both backends
//! (`kubeui` for TUI, `kubeui-gtk` for GTK) are a ~10-line `main` that
//! builds one and hands it to the matching runner:
//!
//! ```ignore
//! let app = kubeui_core::KubeApp::new()?;
//! quadraui::tui::run(app)?; // or quadraui::gtk::run(app) for the GTK binary
//! ```
//!
//! Everything else — state, k8s client, view-builders, theme, the
//! action reducer, the namespace/kind picker — lives in this crate so
//! a new feature is written once and both backends pick it up the
//! moment `KubeApp` does.

pub mod action;
pub mod app;
pub mod k8s;
pub mod shell;
pub mod state;
pub mod view;

// Re-exports — everything a backend binary or a test touches is one
// path away.
pub use action::{apply_action, refresh_resources, Action};
pub use app::KubeApp;
pub use k8s::{current_context_name, list_namespaces, list_resources, ResourceItem};
pub use shell::{bootstrap_state, theme};
pub use state::{AppState, Focus, PickerOverlay, PickerPurpose, ResourceKind};
pub use view::{build_list, build_status_bar, build_yaml_view, status_color};

/// Install the rustls crypto provider — must be called once per
/// process before any TLS handshake. [`KubeApp::new`] calls this
/// before constructing the Tokio runtime.
pub fn install_crypto_provider() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install rustls crypto provider"))
}
