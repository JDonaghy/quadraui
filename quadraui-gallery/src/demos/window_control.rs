//! `WindowControl` demo — adapted from
//! `quadraui/examples/common/window_control_demo.rs`.
//!
//! Exercises `Backend::window()` / `WindowControl` from app code and
//! shows the `ServiceResult` each call returns as status text. There is
//! no paintable, headless-observable effect for the *real* window-state
//! change (a toplevel title, fullscreen, minimize, …), but the outcome —
//! `Ok(())` vs. `Err(BackendError::Unsupported)` — is: TUI genuinely
//! supports `set_title` (OSC 0/2) and genuinely does not support the
//! rest, so both outcomes are deterministic here.

use quadraui::{
    Backend, BackendCaps, BackendError, Color, InteractionState, Reaction, Rect, ServiceResult,
    StatusBar, StatusBarSegment, Toolbar, ToolbarButton, ToolbarHit, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("window_control.rs");

// gallery:begin
pub struct WindowControlDemo {
    status: String,
    fullscreen: bool,
    always_on_top: bool,
}

impl WindowControlDemo {
    pub fn new() -> Self {
        Self {
            status: "click a button to try it".into(),
            fullscreen: false,
            always_on_top: false,
        }
    }

    fn describe(action: &str, result: ServiceResult<()>) -> String {
        match result {
            Ok(()) => format!("{action}: ok"),
            Err(e) => format!("{action}: {e:?}"),
        }
    }

    fn toolbar(&self) -> Toolbar {
        let btn = |id: &str, label: &str| ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.into(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: label.into(),
        };
        Toolbar::new(WidgetId::new("gallery:window-control")).with_buttons(vec![
            btn("gallery:wc:title", "Set Title"),
            btn("gallery:wc:fullscreen", "Fullscreen"),
            btn("gallery:wc:always-on-top", "Always-on-top"),
            btn("gallery:wc:minimize", "Minimize"),
            btn("gallery:wc:restore", "Restore"),
            btn("gallery:wc:center", "Center"),
        ])
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:window-control:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn dispatch(&mut self, id: &WidgetId, backend: &mut dyn Backend) {
        let result;
        let action;
        match id.as_str() {
            "gallery:wc:title" => {
                action = "set_title";
                result = match backend.window() {
                    Some(w) => w.set_title("quadraui gallery"),
                    None => Err(BackendError::Unsupported),
                };
            }
            "gallery:wc:fullscreen" => {
                action = "set_fullscreen";
                self.fullscreen = !self.fullscreen;
                let want = self.fullscreen;
                result = match backend.window() {
                    Some(w) => w.set_fullscreen(want),
                    None => Err(BackendError::Unsupported),
                };
            }
            "gallery:wc:always-on-top" => {
                action = "set_always_on_top";
                self.always_on_top = !self.always_on_top;
                let want = self.always_on_top;
                result = match backend.window() {
                    Some(w) => w.set_always_on_top(want),
                    None => Err(BackendError::Unsupported),
                };
            }
            "gallery:wc:minimize" => {
                action = "minimize";
                result = match backend.window() {
                    Some(w) => w.minimize(),
                    None => Err(BackendError::Unsupported),
                };
            }
            "gallery:wc:restore" => {
                action = "restore";
                result = match backend.window() {
                    Some(w) => w.restore(),
                    None => Err(BackendError::Unsupported),
                };
            }
            "gallery:wc:center" => {
                action = "center";
                result = match backend.window() {
                    Some(w) => w.center(),
                    None => Err(BackendError::Unsupported),
                };
            }
            _ => return,
        }
        self.status = Self::describe(action, result);
    }
}

impl Default for WindowControlDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for WindowControlDemo {
    fn name(&self) -> &'static str {
        "Window Control"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let bar_rect = Rect::new(area.x, area.y, area.width, lh);
        let _ =
            backend.draw_toolbar_interactive(bar_rect, &self.toolbar(), &InteractionState::new());

        let status_rect = Rect::new(area.x, area.y + lh, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        if let UiEvent::MouseDown { position, .. } = event {
            let lh = backend.line_height();
            let bar_rect = Rect::new(area.x, area.y, area.width, lh);
            if position.y >= bar_rect.y && position.y < bar_rect.y + bar_rect.height {
                let bar = self.toolbar();
                let layout = backend.toolbar_layout(bar_rect, &bar);
                if let ToolbarHit::Button(id) = layout.hit_test(position.x, position.y) {
                    self.dispatch(&id, backend);
                    return Reaction::Redraw;
                }
            }
        }
        Reaction::Continue
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "status": self.status,
            "fullscreen": self.fullscreen,
            "always_on_top": self.always_on_top,
        })
    }

    fn caps_note(&self, _variant: usize, caps: &BackendCaps) -> Option<String> {
        // `window_control` acts on a real OS toplevel and has no dedicated
        // `BackendCaps` flag (it's in `tests/conformance.rs`'s ungated
        // caps, not the gated matrix) — the honest signal is each
        // button's own `ServiceResult`, shown live in the status line
        // below rather than summarised here. `window_chrome` is the
        // closest *gated* relative (CSD drag/resize/maximize), so note
        // it when absent since the two tend to travel together.
        if caps.window_chrome {
            None
        } else {
            Some(
                "This backend has no client-side window chrome \
                 (BackendCaps::window_chrome is false) — expect most of \
                 these buttons to report Unsupported too."
                    .into(),
            )
        }
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_formats_ok_and_err() {
        assert_eq!(WindowControlDemo::describe("x", Ok(())), "x: ok");
        assert!(WindowControlDemo::describe("x", Err(BackendError::Unsupported)).contains("x:"));
    }
}
