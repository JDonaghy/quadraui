#![cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
//! Black-box tests for `FolderPicker` on the widget contract: the picker
//! reaches the frame as a `Surface::Widget`, paints through the composed
//! `Palette`'s painters, and routes keys and clicks through the shared
//! engine into the palette's own `Interactive`. Browsing changes and the
//! final choice arrive as `UiEvent::PathPicker`; the app applies the
//! browsing changes with `FolderPicker::apply`. Driven on the TUI backend
//! and on the macOS pixel backend.

use std::path::{Path, PathBuf};

use quadraui::{
    AppLogic, Backend, FolderPicker, FrameTree, PathPickerAction, Reaction, Rect, ScreenLayout,
    Surface, UiEvent,
};

/// `root/project/{src,tests}`, so the picker can browse up and down.
fn scratch() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("project").join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("project").join("tests")).unwrap();
    dir
}

/// One picker routed through the engine; the app applies its browsing
/// changes and records what it confirmed or whether it was dismissed.
struct PickerApp {
    picker: FolderPicker,
    area: Rect,
    confirmed: Option<PathBuf>,
    cancelled: bool,
}

impl PickerApp {
    fn new(root: &Path, area: Rect) -> Self {
        Self {
            picker: FolderPicker::rooted_at(root, Vec::new()),
            area,
            confirmed: None,
            cancelled: false,
        }
    }
}

impl AppLogic for PickerApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.area,
            widget: Box::new(self.picker.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.picker.id, self.area, Box::new(self.picker.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::PathPicker(id, action) = event else {
            return Reaction::Continue;
        };
        if id != self.picker.id {
            return Reaction::Continue;
        }
        match action {
            PathPickerAction::Confirmed { path } => self.confirmed = Some(path),
            PathPickerAction::Cancelled => self.cancelled = true,
            other => self.picker.apply(&other, 10),
        }
        Reaction::Redraw
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::NamedKey;

    const COLS: u16 = 70;
    const ROWS: u16 = 16;
    const AREA: Rect = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);

    fn driver(root: &Path) -> TuiDriver<PickerApp> {
        let mut driver = TuiDriver::new(PickerApp::new(root, AREA), COLS, ROWS);
        driver.set_double_click_folding(false);
        let (x, y) = driver.find("Open Folder").expect("title on screen");
        driver.click(x, y);
        driver
    }

    fn select(driver: &mut TuiDriver<PickerApp>, row: &str) {
        let idx = driver
            .app()
            .picker
            .paths
            .filtered
            .iter()
            .position(|p| p == Path::new(row))
            .unwrap_or_else(|| panic!("{row} listed"));
        while driver.app().picker.paths.selected < idx {
            driver.press_named(NamedKey::Down);
        }
    }

    #[test]
    fn paints_the_title_and_the_walked_directories() {
        let dir = scratch();
        let driver = driver(&dir.path().join("project"));
        for text in ["Open Folder", "src", "tests", ".."] {
            assert!(driver.screen_contains(text), "{text}: {}", driver.screen());
        }
    }

    #[test]
    fn enter_on_a_directory_confirms_it() {
        let dir = scratch();
        let root = dir.path().join("project");
        let mut driver = driver(&root);
        select(&mut driver, "tests");
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().confirmed, Some(root.join("tests")));
    }

    #[test]
    fn dash_and_dotdot_browse_up_and_the_list_follows() {
        let dir = scratch();
        let root = dir.path().join("project");
        let mut driver = driver(&root);
        driver.type_char('-');
        assert_eq!(driver.app().picker.paths.root, dir.path());
        assert!(
            driver.screen_contains("project"),
            "the parent's walk lists the child: {}",
            driver.screen()
        );

        select(&mut driver, "project");
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().confirmed, Some(root.clone()));
    }

    #[test]
    fn j_and_k_move_the_highlight_instead_of_typing() {
        let dir = scratch();
        let mut driver = driver(&dir.path().join("project"));
        driver.type_char('j');
        assert_eq!(driver.app().picker.paths.selected, 1);
        driver.type_char('k');
        assert_eq!(driver.app().picker.paths.selected, 0);
        assert_eq!(driver.app().picker.paths.query, "");
    }

    #[test]
    fn typing_filters_and_escape_dismisses() {
        let dir = scratch();
        let mut driver = driver(&dir.path().join("project"));
        for c in "src".chars() {
            driver.type_char(c);
        }
        assert_eq!(driver.app().picker.paths.query, "src");
        assert!(!driver.screen_contains("tests"), "{}", driver.screen());
        driver.press_named(NamedKey::Escape);
        assert!(driver.app().cancelled);
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod mac {
    use super::*;
    use quadraui::macos::testing::MacDriver;
    use quadraui::NamedKey;

    const W: u32 = 720;
    const H: u32 = 420;
    const AREA: Rect = Rect::new(0.0, 0.0, W as f32, H as f32);

    #[test]
    fn browses_up_and_confirms_a_folder() {
        let dir = scratch();
        let root = dir.path().join("project");
        let mut driver = MacDriver::new(PickerApp::new(&root, AREA), W, H);
        assert!(
            driver.screen_contains("tests"),
            "{:?}",
            driver.painted_texts()
        );
        let (x, y) = driver.find("tests").expect("row painted");
        driver.click(x, y);
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().confirmed, Some(root.join("tests")));
    }
}
