#![cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
//! Black-box tests for `FilePicker` on the widget contract: the picker
//! reaches the frame as a `Surface::Widget`, paints through the composed
//! `Palette`'s painters, and routes keys and clicks through the shared
//! engine into the palette's own `Interactive`. Browsing changes and the
//! final choice arrive as `UiEvent::PathPicker`; the app applies the
//! browsing changes with `FilePicker::apply`, so the next frame paints
//! the filtered list. Driven on the TUI backend and on the macOS pixel
//! backend.

use std::path::{Path, PathBuf};

use quadraui::{
    AppLogic, Backend, FilePicker, FilePickerMode, FrameTree, PathPickerAction, Reaction, Rect,
    ScreenLayout, Surface, UiEvent,
};

/// A scratch tree with two files and one directory.
fn scratch() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("alpha.rs"), "").unwrap();
    std::fs::write(dir.path().join("beta.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs").join("guide.md"), "").unwrap();
    dir
}

/// One picker routed through the engine; the app applies its browsing
/// changes and records what it confirmed or whether it was dismissed.
struct PickerApp {
    picker: FilePicker,
    area: Rect,
    confirmed: Option<PathBuf>,
    cancelled: bool,
}

impl PickerApp {
    fn new(mode: FilePickerMode, root: &Path, area: Rect) -> Self {
        Self {
            picker: FilePicker::new(mode, root, Vec::new()),
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

    fn driver(mode: FilePickerMode, root: &Path) -> TuiDriver<PickerApp> {
        let mut driver = TuiDriver::new(PickerApp::new(mode, root, AREA), COLS, ROWS);
        driver.set_double_click_folding(false);
        // A press on the palette gives it keyboard focus.
        let (x, y) = driver
            .find("Open File")
            .or_else(|| driver.find("Save File"))
            .unwrap();
        driver.click(x, y);
        driver
    }

    #[test]
    fn paints_the_title_and_the_walked_entries() {
        let dir = scratch();
        let driver = driver(FilePickerMode::Open, dir.path());
        for text in ["Open File", "alpha.rs", "beta.txt", "docs"] {
            assert!(driver.screen_contains(text), "{text}: {}", driver.screen());
        }
    }

    #[test]
    fn typing_filters_the_list_and_enter_confirms_the_match() {
        let dir = scratch();
        let mut driver = driver(FilePickerMode::Open, dir.path());
        for c in "beta".chars() {
            driver.type_char(c);
        }
        assert_eq!(driver.app().picker.paths.query, "beta");
        assert!(!driver.screen_contains("alpha.rs"), "{}", driver.screen());
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().confirmed, Some(dir.path().join("beta.txt")));
    }

    #[test]
    fn enter_on_a_directory_browses_into_it() {
        let dir = scratch();
        let mut driver = driver(FilePickerMode::Open, dir.path());
        for c in "docs".chars() {
            driver.type_char(c);
        }
        let docs = driver
            .app()
            .picker
            .paths
            .filtered
            .iter()
            .position(|p| p == Path::new("docs"))
            .expect("docs listed");
        for _ in 0..docs {
            driver.press_named(NamedKey::Down);
        }
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().picker.paths.root, dir.path().join("docs"));
        assert!(driver.screen_contains("guide.md"), "{}", driver.screen());
        assert_eq!(driver.app().confirmed, None);
    }

    #[test]
    fn save_mode_types_vim_letters_and_confirms_the_typed_name() {
        let dir = scratch();
        let mut driver = driver(FilePickerMode::Save, dir.path());
        for c in "jk-notes".chars() {
            driver.type_char(c);
        }
        assert_eq!(driver.app().picker.paths.query, "jk-notes");
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().confirmed, Some(dir.path().join("jk-notes")));
    }

    #[test]
    fn escape_dismisses_the_picker() {
        let dir = scratch();
        let mut driver = driver(FilePickerMode::Open, dir.path());
        driver.press_named(NamedKey::Escape);
        assert!(driver.app().cancelled);
    }

    #[test]
    fn clicking_a_row_highlights_it() {
        let dir = scratch();
        let mut driver = driver(FilePickerMode::Open, dir.path());
        let (x, y) = driver.find("beta.txt").expect("row on screen");
        driver.click(x, y);
        let picker = &driver.app().picker;
        assert_eq!(
            picker.paths.filtered[picker.paths.selected],
            PathBuf::from("beta.txt")
        );
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
    fn filters_and_confirms_a_file() {
        let dir = scratch();
        let mut driver =
            MacDriver::new(PickerApp::new(FilePickerMode::Open, dir.path(), AREA), W, H);
        assert!(
            driver.screen_contains("alpha.rs"),
            "{:?}",
            driver.painted_texts()
        );
        let (x, y) = driver.find("alpha.rs").expect("row painted");
        driver.click(x, y);
        for c in "beta".chars() {
            driver.type_char(c);
        }
        assert!(!driver.screen_contains("alpha.rs"));
        driver.press_named(NamedKey::Enter);
        assert_eq!(driver.app().confirmed, Some(dir.path().join("beta.txt")));
    }
}
