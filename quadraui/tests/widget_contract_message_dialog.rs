#![cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
//! Black-box tests for `MessageDialog` on the widget contract: the alert
//! reaches the frame as a type-erased `Surface::Widget`, paints through
//! the composed `Dialog`'s own painters, and is registered in the app's
//! `FrameTree`, so input routes through the shared engine — hit test,
//! click-to-focus, `Interactive::handle` — and back out as the
//! `UiEvent::MessageDialogResolved` the app records. Driven on the TUI
//! backend and on the macOS pixel backend.

use quadraui::{
    AppLogic, Backend, FrameTree, MessageDialog, MessageDialogButton, MessageDialogOptions,
    Reaction, Rect, ScreenLayout, Surface, UiEvent, WidgetId,
};

const TITLE: &str = "Unsaved Changes";
const BODY: &str = "Save before closing?";

fn save_id() -> WidgetId {
    WidgetId::new("alert:save")
}

fn cancel_id() -> WidgetId {
    WidgetId::new("alert:cancel")
}

#[cfg(feature = "tui")]
fn ok_id() -> WidgetId {
    WidgetId::new("alert:ok")
}

fn button(id: WidgetId, label: &str, is_default: bool, is_cancel: bool) -> MessageDialogButton {
    MessageDialogButton {
        id,
        label: label.into(),
        is_default,
        is_cancel,
        spec: None,
    }
}

/// One alert routed through the engine, plus the choices the app saw.
struct AlertApp {
    alert: MessageDialog,
    area: Rect,
    resolved: Vec<WidgetId>,
}

impl AlertApp {
    fn save_cancel(area: Rect) -> Self {
        Self::with_buttons(
            area,
            vec![
                button(save_id(), "Save", true, false),
                button(cancel_id(), "Cancel", false, true),
            ],
        )
    }

    #[cfg(feature = "tui")]
    fn ok_only(area: Rect) -> Self {
        Self::with_buttons(area, vec![button(ok_id(), "OK", true, false)])
    }

    fn with_buttons(area: Rect, buttons: Vec<MessageDialogButton>) -> Self {
        Self {
            alert: MessageDialog::new(MessageDialogOptions {
                title: TITLE.into(),
                body: BODY.into(),
                buttons,
                severity: None,
            })
            .with_id("alert"),
            area,
            resolved: Vec::new(),
        }
    }
}

impl AppLogic for AlertApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.area,
            widget: Box::new(self.alert.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(
            self.alert.dialog.id,
            self.area,
            Box::new(self.alert.clone()),
        );
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::MessageDialogResolved(id) = event {
            self.resolved.push(id);
            return Reaction::Redraw;
        }
        Reaction::Continue
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{Key, NamedKey};

    const COLS: u16 = 60;
    const ROWS: u16 = 20;
    const AREA: Rect = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);

    /// Click inside the body text: focuses the alert without answering it.
    fn focus(driver: &mut TuiDriver<AlertApp>) {
        let (x, y) = driver.find(BODY).expect("body on screen");
        driver.click(x, y);
        assert!(driver.app().resolved.is_empty());
    }

    #[test]
    fn paints_title_body_and_buttons_through_the_composed_dialog() {
        let driver = TuiDriver::new(AlertApp::save_cancel(AREA), COLS, ROWS);
        for text in [TITLE, BODY, "Save", "Cancel"] {
            assert!(driver.screen_contains(text), "{}", driver.screen());
        }
        assert!(driver.screen_contains("╭"), "{}", driver.screen());
    }

    #[test]
    fn clicking_a_button_resolves_it() {
        let mut driver = TuiDriver::new(AlertApp::save_cancel(AREA), COLS, ROWS);
        let b = driver.find_bounds("Cancel").expect("Cancel on screen");
        driver.click(b.x + 0.5, b.y);
        assert_eq!(driver.app().resolved, vec![cancel_id()]);
    }

    #[test]
    fn enter_resolves_the_default_button_and_arrows_move_focus() {
        let mut driver = TuiDriver::new(AlertApp::save_cancel(AREA), COLS, ROWS);
        focus(&mut driver);
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().resolved, vec![save_id()]);

        driver.press_named(NamedKey::Right);
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().resolved, vec![save_id(), cancel_id()]);

        // Left wraps back past the first button.
        driver.press_named(NamedKey::Left);
        driver.press_named(NamedKey::Left);
        driver.press_named(NamedKey::Enter);
        assert_eq!(
            driver.app().resolved,
            vec![save_id(), cancel_id(), cancel_id()]
        );
    }

    #[test]
    fn shift_tab_moves_focus_backwards() {
        let mut driver = TuiDriver::new(AlertApp::save_cancel(AREA), COLS, ROWS);
        focus(&mut driver);
        driver.press(Key::Named(NamedKey::BackTab));
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().resolved, vec![cancel_id()]);
    }

    #[test]
    fn escape_resolves_the_cancel_button() {
        let mut driver = TuiDriver::new(AlertApp::save_cancel(AREA), COLS, ROWS);
        focus(&mut driver);
        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().resolved, vec![cancel_id()]);
    }

    #[test]
    fn escape_on_an_ok_only_alert_resolves_the_default_button() {
        let mut driver = TuiDriver::new(AlertApp::ok_only(AREA), COLS, ROWS);
        focus(&mut driver);
        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().resolved, vec![ok_id()]);
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod mac {
    use super::*;
    use quadraui::macos::testing::MacDriver;
    use quadraui::NamedKey;

    const W: u32 = 640;
    const H: u32 = 400;
    const AREA: Rect = Rect::new(0.0, 0.0, W as f32, H as f32);

    #[test]
    fn paints_the_alert_and_a_click_resolves_a_button() {
        let mut driver = MacDriver::new(AlertApp::save_cancel(AREA), W, H);
        for text in [TITLE, BODY, "Save", "Cancel"] {
            assert!(
                driver.screen_contains(text),
                "{text}: {:?}",
                driver.painted_texts()
            );
        }
        let (x, y) = driver.find("Cancel").expect("Cancel painted");
        driver.click(x, y);
        assert_eq!(driver.app().resolved, vec![cancel_id()]);
    }

    #[test]
    fn keyboard_focus_moves_along_the_button_row() {
        let mut driver = MacDriver::new(AlertApp::save_cancel(AREA), W, H);
        let (x, y) = driver.find(BODY).expect("body painted");
        driver.click(x, y);
        driver.press_named(NamedKey::Right);
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().resolved, vec![cancel_id()]);
    }
}
