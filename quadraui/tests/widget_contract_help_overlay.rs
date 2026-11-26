#![cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
//! Black-box tests for `HelpOverlay` on the widget contract: the
//! cheatsheet card reaches the frame as a `Surface::Widget`, paints
//! through the composed `Dialog`'s painters while the app's `open` flag
//! is set, and routes its keys through the shared engine back to the app
//! as `UiEvent::HelpOverlay`, which the app applies onto `open`. Driven
//! on the TUI backend and on the macOS pixel backend.

use quadraui::{
    AppLogic, Backend, FrameTree, HelpAction, HelpNote, HelpOverlay, HelpOverlayAction, Reaction,
    Rect, ScreenLayout, Surface, UiEvent, ViewHelp, WidgetId,
};

fn help() -> ViewHelp {
    ViewHelp::new("Explorer")
        .with_notes(vec![HelpNote::new("●", "Unsaved changes")])
        .with_actions(vec![
            HelpAction::new("open", "Open", "Open the selected file").with_accelerator("Enter"),
            HelpAction::new("rename", "Rename", "Rename the selected file"),
        ])
}

/// One overlay routed through the engine; the app owns `open`.
struct HelpApp {
    overlay: HelpOverlay,
    area: Rect,
    actions: Vec<HelpOverlayAction>,
}

impl HelpApp {
    fn new(area: Rect) -> Self {
        let mut overlay = HelpOverlay::new(WidgetId::new("help"));
        overlay.open = true;
        overlay.help = Some(help());
        Self {
            overlay,
            area,
            actions: Vec::new(),
        }
    }
}

impl AppLogic for HelpApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.area,
            widget: Box::new(self.overlay.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.overlay.id, self.area, Box::new(self.overlay.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::HelpOverlay(id, action) = event {
            if id == self.overlay.id {
                self.overlay.open = action == HelpOverlayAction::Opened;
                self.actions.push(action);
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::NamedKey;

    const COLS: u16 = 70;
    const ROWS: u16 = 20;
    const AREA: Rect = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);

    fn focus(driver: &mut TuiDriver<HelpApp>) {
        let (x, y) = driver.find("Unsaved changes").expect("note on screen");
        driver.click(x, y);
    }

    #[test]
    fn an_open_overlay_paints_the_views_notes_and_action_table() {
        let driver = TuiDriver::new(HelpApp::new(AREA), COLS, ROWS);
        for text in [
            "Help — Explorer",
            "Unsaved changes",
            "Shortcut",
            "Open  (Enter)",
            "Rename the selected file",
        ] {
            assert!(driver.screen_contains(text), "{text}: {}", driver.screen());
        }
    }

    #[test]
    fn escape_closes_it_and_the_next_frame_paints_nothing() {
        let mut driver = TuiDriver::new(HelpApp::new(AREA), COLS, ROWS);
        focus(&mut driver);
        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().actions, vec![HelpOverlayAction::Closed]);
        assert!(!driver.screen_contains("Explorer"), "{}", driver.screen());
    }

    #[test]
    fn question_mark_closes_it_and_other_keys_are_swallowed() {
        let mut driver = TuiDriver::new(HelpApp::new(AREA), COLS, ROWS);
        focus(&mut driver);
        driver.type_char('x');
        assert!(driver.app().actions.is_empty());
        assert!(driver.screen_contains("Explorer"), "{}", driver.screen());
        driver.type_char('?');
        assert_eq!(driver.app().actions, vec![HelpOverlayAction::Closed]);
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod mac {
    use super::*;
    use quadraui::macos::testing::MacDriver;
    use quadraui::NamedKey;

    const W: u32 = 720;
    const H: u32 = 480;
    const AREA: Rect = Rect::new(0.0, 0.0, W as f32, H as f32);

    #[test]
    fn paints_the_card_and_escape_closes_it() {
        let mut driver = MacDriver::new(HelpApp::new(AREA), W, H);
        for text in ["Unsaved changes", "Shortcut", "Rename the selected file"] {
            assert!(
                driver.screen_contains(text),
                "{text}: {:?}",
                driver.painted_texts()
            );
        }
        let (x, y) = driver.find("Unsaved changes").expect("note painted");
        driver.click(x, y);
        driver.press_named(NamedKey::Escape);
        assert_eq!(driver.app().actions, vec![HelpOverlayAction::Closed]);
        assert!(!driver.screen_contains("Unsaved changes"));
    }
}
