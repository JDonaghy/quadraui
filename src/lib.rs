//! Backend-agnostic core for kubeui.
//!
//! Two backends consume this crate today (`kubeui` for TUI,
//! `kubeui-gtk` for GTK). Both follow the same shape:
//!
//! ```ignore
//! kubeui_core::install_crypto_provider();
//! let rt = tokio::runtime::Runtime::new()?;
//! let context = rt.block_on(kubeui_core::current_context_name())?;
//! let namespaces = rt.block_on(kubeui_core::list_namespaces()).unwrap_or_default();
//! let mut state = kubeui_core::AppState::new(context, namespaces);
//! loop {
//!     // Per-backend rasterise:
//!     //   build_list(&state)     → ListView
//!     //   build_status_bar(...)  → StatusBar
//!     //   build_picker(...)      → ListView
//!
//!     // Per-backend translate:
//!     //   raw event → Vec<Action>
//!     for action in actions {
//!         kubeui_core::apply_action(&mut state, action, &rt);
//!     }
//!     if state.should_quit { break; }
//! }
//! ```

pub mod action;
pub mod k8s;
pub mod state;
pub mod view;

// Re-exports — everything backends commonly touch is one path away.
pub use action::{apply_action, refresh_resources, Action};
pub use k8s::{current_context_name, list_namespaces, list_resources, ResourceItem};
pub use state::{AppState, Focus, Picker, PickerPurpose, ResourceKind};
pub use view::{
    build_list, build_picker, build_status_bar, picker_bounds, picker_current_index, status_color,
};

/// Install the rustls crypto provider — must be called once per
/// process before any TLS handshake. Backends call this in `main`
/// before constructing the Tokio runtime.
pub fn install_crypto_provider() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install rustls crypto provider"))
}
