//! `cargo run --example win_demo --features win` (Windows only)
//!
//! Smoke-test for the Win32 + Direct2D window bootstrap (issue #19).
//! Opens a native window with a cleared Direct2D surface, responds to
//! resize (recreating the render target) and DPI changes, and closes
//! cleanly via the title bar's close button.
//!
//! Draws nothing yet — every `WinBackend::draw_*` rasteriser is still a
//! `todo!()` stub (later issues implement each one, mirroring how the
//! GTK backend shipped its rasterisers one primitive at a time). This
//! example only exercises the bootstrap itself: window creation, the
//! message loop, and the Direct2D render-target lifecycle. Once a later
//! issue wires real widgets into `WinBackend`, this can be rewritten to
//! share `examples/common`'s `AppState` the way `tui_demo`/`gtk_demo`
//! already do.
//!
//! `quadraui::win::run` only exists when compiled for `target_os =
//! "windows"` (see `src/win/mod.rs`/`Cargo.toml`'s `win` feature
//! comment) — this example is Windows-only, verified by `cargo check
//! --target x86_64-pc-windows-msvc --features win` and the
//! `windows-latest` CI leg, not by ordinary Linux CI.

// `cfg`-gated alongside `main()`'s Windows arm below — on any other host
// nothing ever constructs this, and an unconditional definition would
// just be a `dead_code` warning for no benefit.
#[cfg(target_os = "windows")]
struct BootstrapDemo;

#[cfg(target_os = "windows")]
impl quadraui::AppLogic for BootstrapDemo {
    type AreaId = ();

    fn render(&self, _backend: &mut dyn quadraui::Backend, _area: ()) {
        // Nothing to paint yet (see module docs above) — `begin_frame`'s
        // `Clear` is the entire visible content of this bootstrap demo.
    }

    fn handle(
        &mut self,
        event: quadraui::UiEvent,
        _backend: &mut dyn quadraui::Backend,
    ) -> quadraui::Reaction {
        match event {
            // Mouse/keyboard translation lands in #20 — the only input
            // this bootstrap needs to honour is the window chrome's own
            // close button, which arrives as `WindowClose` regardless.
            quadraui::UiEvent::WindowClose => quadraui::Reaction::Exit,
            _ => quadraui::Reaction::Continue,
        }
    }
}

#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    quadraui::win::run(BootstrapDemo)
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("win_demo only runs on Windows — see this file's module docs.");
}
