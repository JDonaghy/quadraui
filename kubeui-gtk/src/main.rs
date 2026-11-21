//! kubeui-gtk — GTK4 Kubernetes dashboard.
//!
//! Thin binary around [`kubeui_core::KubeApp`], which implements
//! `quadraui::AppLogic`. All state, rendering, event handling, k8s
//! client, and theme live in `kubeui-core` and are shared with the
//! `kubeui` TUI binary; this binary only picks the GTK runner.
//! `quadraui::gtk::run` owns the `gtk4::Application`, window, drawing
//! area, and event controllers, so there's no bespoke GTK wiring here.

fn main() -> std::process::ExitCode {
    let app = match kubeui_core::KubeApp::new() {
        Ok(app) => app,
        Err(e) => {
            eprintln!("kubeui-gtk: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match quadraui::gtk::run(app) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => std::process::Termination::report(err),
    }
}
