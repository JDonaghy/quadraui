#![cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
//! Black-box tests for `DualModePalette` on the widget contract: the
//! palette reaches the frame as a `Surface::Widget`, paints through the
//! composed `Palette`'s painters, and routes keys and clicks through the
//! shared engine into the palette's own `Interactive`. Every change and
//! confirmation arrives as `UiEvent::DualModePalette`; the app applies
//! the query, mode and highlight onto its descriptor and refilters.
//! Driven on the TUI backend and on the macOS pixel backend.

use quadraui::{
    AppLogic, Backend, DualModePalette, DualModePaletteAction, FrameTree, PaletteItem, Reaction,
    Rect, ScreenLayout, StyledText, Surface, UiEvent, WidgetId,
};

const BRANCHES: &[&str] = &["main", "develop", "feature/menus", "feature/pickers"];

fn items(query: &str) -> Vec<PaletteItem> {
    BRANCHES
        .iter()
        .filter(|b| b.contains(query))
        .map(|b| PaletteItem {
            text: StyledText::plain(*b),
            detail: None,
            icon: None,
            match_positions: Vec::new(),
            depth: 0,
            expandable: false,
            expanded: false,
        })
        .collect()
}

/// One palette routed through the engine; the app owns its state.
struct BranchApp {
    palette: DualModePalette,
    area: Rect,
    confirmed: Vec<DualModePaletteAction>,
}

impl BranchApp {
    fn new(area: Rect) -> Self {
        Self {
            palette: DualModePalette::new(
                WidgetId::new("branches"),
                "Branches",
                Some("New branch:".into()),
                items(""),
            )
            .with_vim_keys(true),
            area,
            confirmed: Vec::new(),
        }
    }

    fn selected_label(&self) -> String {
        self.palette.items[self.palette.selected]
            .text
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect()
    }
}

impl AppLogic for BranchApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: self.area,
            widget: Box::new(self.palette.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.palette.id, self.area, Box::new(self.palette.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::DualModePalette(id, action) = event else {
            return Reaction::Continue;
        };
        if id != self.palette.id {
            return Reaction::Continue;
        }
        match action {
            DualModePaletteAction::QueryChanged { value } => {
                self.palette.items = items(&value);
                self.palette.query_cursor = value.len();
                self.palette.query = value;
                self.palette.selected = 0;
            }
            DualModePaletteAction::SelectionChanged { idx } => self.palette.selected = idx,
            DualModePaletteAction::ModeToggled { new_mode } => self.palette.mode = new_mode,
            other => self.confirmed.push(other),
        }
        Reaction::Redraw
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{NamedKey, PaletteMode};

    const COLS: u16 = 60;
    const ROWS: u16 = 14;
    const AREA: Rect = Rect::new(0.0, 0.0, COLS as f32, ROWS as f32);

    fn driver() -> TuiDriver<BranchApp> {
        let mut driver = TuiDriver::new(BranchApp::new(AREA), COLS, ROWS);
        driver.set_double_click_folding(false);
        let (x, y) = driver.find("Branches").expect("title on screen");
        driver.click(x, y);
        driver
    }

    #[test]
    fn list_mode_paints_the_items_and_typing_refilters_them() {
        let mut driver = driver();
        for b in BRANCHES {
            assert!(driver.screen_contains(b), "{b}: {}", driver.screen());
        }
        for c in "feat".chars() {
            driver.type_char(c);
        }
        assert!(!driver.screen_contains("develop"), "{}", driver.screen());
        assert!(
            driver.screen_contains("feature/menus"),
            "{}",
            driver.screen()
        );
    }

    #[test]
    fn down_then_enter_confirms_the_highlighted_item() {
        let mut driver = driver();
        driver.press_named(NamedKey::Down);
        assert_eq!(driver.app().selected_label(), "develop");
        driver.press_named(NamedKey::Enter);
        assert_eq!(
            driver.app().confirmed,
            vec![DualModePaletteAction::ItemConfirmed { idx: 1 }]
        );
    }

    #[test]
    fn j_and_k_move_the_highlight_in_list_mode() {
        let mut driver = driver();
        driver.type_char('j');
        driver.type_char('j');
        driver.type_char('k');
        assert_eq!(driver.app().selected_label(), "develop");
        assert_eq!(driver.app().palette.query, "");
    }

    #[test]
    fn tab_switches_to_input_mode_where_enter_confirms_the_text() {
        let mut driver = driver();
        driver.press_named(NamedKey::Tab);
        assert_eq!(driver.app().palette.mode, PaletteMode::Input);
        assert!(driver.screen_contains("New branch:"), "{}", driver.screen());
        assert!(!driver.screen_contains("develop"), "{}", driver.screen());
        for c in "hotfix".chars() {
            driver.type_char(c);
        }
        driver.press_named(NamedKey::Enter);
        assert_eq!(
            driver.app().confirmed,
            vec![DualModePaletteAction::TextConfirmed {
                value: "hotfix".into()
            }]
        );
    }

    #[test]
    fn clicking_a_row_highlights_it_and_escape_cancels() {
        let mut driver = driver();
        let (x, y) = driver.find("feature/pickers").expect("row on screen");
        driver.click(x, y);
        assert_eq!(driver.app().selected_label(), "feature/pickers");
        driver.press_named(NamedKey::Escape);
        assert_eq!(
            driver.app().confirmed,
            vec![DualModePaletteAction::Cancelled]
        );
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod mac {
    use super::*;
    use quadraui::macos::testing::MacDriver;
    use quadraui::NamedKey;

    const W: u32 = 640;
    const H: u32 = 360;
    const AREA: Rect = Rect::new(0.0, 0.0, W as f32, H as f32);

    #[test]
    fn click_selects_a_row_and_enter_confirms_it() {
        let mut driver = MacDriver::new(BranchApp::new(AREA), W, H);
        assert!(
            driver.screen_contains("develop"),
            "{:?}",
            driver.painted_texts()
        );
        let (x, y) = driver.find("feature/menus").expect("row painted");
        driver.click(x, y);
        assert_eq!(driver.app().selected_label(), "feature/menus");
        driver.press_named(NamedKey::Enter);
        assert_eq!(
            driver.app().confirmed,
            vec![DualModePaletteAction::ItemConfirmed { idx: 2 }]
        );
    }
}
