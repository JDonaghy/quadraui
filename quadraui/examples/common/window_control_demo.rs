//! Window-control demo (issue #950) — exercises `Backend::window()` /
//! `WindowControl` from app code and shows the `ServiceResult` each call
//! returns as on-screen status text.
//!
//! There is no paintable, headless-observable effect for the *real*
//! window-state change itself (title text, fullscreen, always-on-top,
//! minimize/restore all act on a real OS toplevel — see
//! `tests/conformance.rs`'s `UNGATED_CAPS` `window_control` entry, and
//! `CAP_CONTRACTS`'s `window_chrome` entry for the same "acts on a real
//! toplevel" shape this backend surface shares). The *outcome* —
//! `Ok(())` vs. `Err(BackendError::Unsupported)` vs. a `PlatformFailure`
//! — is paintable status text, though, and that's what
//! `tests/tui_example_driver.rs` asserts against: TUI genuinely supports
//! `set_title` (OSC 0/2) and genuinely does not support the rest, so the
//! two outcomes are deterministic and driver-testable even without a
//! live window.
//!
//! Keys: `t` set_title, `f` toggle fullscreen, `a` toggle always-on-top,
//! `m` minimize, `r` restore, `c` center. Esc quits.

use quadraui::{
    AppLogic, Backend, BackendError, Color, InteractionState, Key, NamedKey, Reaction, Rect,
    ServiceResult, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct WindowControlDemo {
    status: String,
    fullscreen: bool,
    always_on_top: bool,
}

impl WindowControlDemo {
    pub fn new() -> Self {
        Self {
            status: "t=title f=fullscreen a=always-on-top m=minimize r=restore c=center"
                .to_string(),
            fullscreen: false,
            always_on_top: false,
        }
    }

    /// Renders a `ServiceResult<()>` as status text — `Backend::window()`
    /// returning `None` (no window yet) is folded into the same
    /// `Unsupported` shape a caller already has to handle, since both
    /// mean "this backend can't do it right now" from the app's point of
    /// view.
    fn describe(action: &str, result: ServiceResult<()>) -> String {
        match result {
            Ok(()) => format!("{action}: ok"),
            Err(e) => format!("{action}: {e:?}"),
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("window-control-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: " Window control demo (#950) ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for WindowControlDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for WindowControlDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('t'),
                ..
            } => {
                let result = match backend.window() {
                    Some(w) => w.set_title("quadraui window-control demo"),
                    None => Err(BackendError::Unsupported),
                };
                self.status = Self::describe("set_title", result);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('f'),
                ..
            } => {
                self.fullscreen = !self.fullscreen;
                let want = self.fullscreen;
                let result = match backend.window() {
                    Some(w) => w.set_fullscreen(want),
                    None => Err(BackendError::Unsupported),
                };
                self.status = Self::describe("set_fullscreen", result);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('a'),
                ..
            } => {
                self.always_on_top = !self.always_on_top;
                let want = self.always_on_top;
                let result = match backend.window() {
                    Some(w) => w.set_always_on_top(want),
                    None => Err(BackendError::Unsupported),
                };
                self.status = Self::describe("set_always_on_top", result);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                let result = match backend.window() {
                    Some(w) => w.minimize(),
                    None => Err(BackendError::Unsupported),
                };
                self.status = Self::describe("minimize", result);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('r'),
                ..
            } => {
                let result = match backend.window() {
                    Some(w) => w.restore(),
                    None => Err(BackendError::Unsupported),
                };
                self.status = Self::describe("restore", result);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('c'),
                ..
            } => {
                let result = match backend.window() {
                    Some(w) => w.center(),
                    None => Err(BackendError::Unsupported),
                };
                self.status = Self::describe("center", result);
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
