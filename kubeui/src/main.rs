//! kubeui — TUI Kubernetes dashboard.
//!
//! Thin binary around [`kubeui_core::KubeApp`], which implements
//! `quadraui::AppLogic`. All state, rendering, event handling, k8s
//! client, and theme live in `kubeui-core` and are shared with
//! `kubeui-gtk`; this binary only picks the TUI runner.
//! `quadraui::tui::run` owns terminal setup/teardown (raw mode, the
//! alternate screen, mouse capture) and tears it down even if the app
//! panics, so there's no bespoke terminal-handling code here.

fn main() -> anyhow::Result<()> {
    let app = kubeui_core::KubeApp::new()?;
    quadraui::tui::run(app)?;
    Ok(())
}
