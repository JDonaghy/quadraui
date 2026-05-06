//! Win-GUI runner: Win32 message loop driving an [`AppLogic`] impl.
//!
//! The runner creates a Win32 window (RegisterClassEx + CreateWindowEx),
//! initialises a Direct2D render target, enters the message loop, and
//! translates WM_* messages → [`UiEvent`] → `app.handle()` →
//! `app.render(&mut backend)`.
//!
//! Mirrors `quadraui::gtk::run` and `quadraui::tui::run` — consumers
//! call `quadraui::win::run(MyApp::new())` and the same `AppLogic`
//! impl drives every backend.

use crate::runner::AppLogic;

pub fn run<A: AppLogic + 'static>(_app: A) -> std::process::ExitCode {
    todo!(
        "Win32 message loop: RegisterClassEx, CreateWindowEx, \
         Direct2D render target, translate WM_* → UiEvent, \
         dispatch to app.handle(), redraw via app.render()"
    )
}
