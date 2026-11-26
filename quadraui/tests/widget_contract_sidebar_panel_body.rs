#![cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos")
))]
//! Black-box tests for `SidebarPanelBody` on the widget contract: the
//! panel paints its background and header/search chrome through its own
//! `Primitive` (reached via `DrawsPrimitives::draw_dyn`), and its body
//! widget paints into the rect left over. Asserted on the rendered cells
//! (TuiDriver) and on the painted text runs of a pixel surface (GTK,
//! macOS).

use quadraui::compose::sidebar_panel_body::{SidebarPanelBody, SidebarPanelChrome};
use quadraui::{
    AppLogic, Backend, Color, DrawsPrimitives, FrameTree, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

const BG: Color = Color::rgb(12, 34, 56);

/// One sidebar panel painted every frame with a fixed body label.
struct PanelApp {
    rect: Rect,
    panel: SidebarPanelBody,
}

impl AppLogic for PanelApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        self.panel.render_with(backend, self.rect, |b, body| {
            b.draw_dyn(
                body,
                &StatusBar {
                    id: WidgetId::new("panel:body"),
                    left_segments: vec![StatusBarSegment {
                        text: "body-row".into(),
                        fg: Color::rgb(255, 255, 255),
                        bg: Color::rgb(0, 0, 0),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: Vec::new(),
                },
                &(),
            );
        });
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        FrameTree::new()
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

fn header_and_search(active: bool) -> SidebarPanelBody {
    SidebarPanelBody {
        background: Some(BG),
        chrome: SidebarPanelChrome::HeaderAndSearch {
            header: "EXTENSIONS".into(),
            query: String::new(),
            placeholder: "Search extensions".into(),
            active,
        },
        scrollbar_gutter: Some(1.0),
    }
}

fn status_bars() -> SidebarPanelBody {
    let bar = |id: &str, text: &str| StatusBar {
        id: WidgetId::new(id),
        left_segments: vec![StatusBarSegment {
            text: text.into(),
            fg: Color::rgb(255, 255, 255),
            bg: Color::rgb(40, 40, 40),
            bold: false,
            action_id: None,
        }],
        right_segments: Vec::new(),
    };
    SidebarPanelBody {
        background: None,
        chrome: SidebarPanelChrome::StatusBars(vec![bar("t", "RUN AND DEBUG"), bar("a", "Start")]),
        scrollbar_gutter: None,
    }
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::TuiDriver;

    const RECT: Rect = Rect::new(0.0, 0.0, 30.0, 8.0);

    #[test]
    fn header_search_and_body_paint_in_their_own_rows() {
        let driver = TuiDriver::new(
            PanelApp {
                rect: RECT,
                panel: header_and_search(false),
            },
            40,
            10,
        );
        let screen: Vec<String> = driver.screen().lines().map(str::to_string).collect();
        assert!(screen[0].starts_with("EXTENSIONS"), "{screen:?}");
        assert!(
            screen[1].contains("/ Search extensions"),
            "search row: {screen:?}"
        );
        assert!(screen[2].starts_with("body-row"), "body row: {screen:?}");
    }

    #[test]
    fn background_fills_the_gutter_the_body_leaves() {
        let driver = TuiDriver::new(
            PanelApp {
                rect: RECT,
                panel: header_and_search(false),
            },
            40,
            10,
        );
        let gutter = driver
            .style_at(RECT.width as u16 - 1, 5)
            .expect("gutter cell");
        assert_eq!(
            gutter.bg,
            quadraui::tui::testing::Color::Rgb(BG.r, BG.g, BG.b),
            "the panel background shows in the gutter"
        );
    }

    #[test]
    fn active_search_row_paints_a_caret() {
        let driver = TuiDriver::new(
            PanelApp {
                rect: RECT,
                panel: header_and_search(true),
            },
            40,
            10,
        );
        let row: String = driver.styled_row(1).into_iter().map(|(c, _)| c).collect();
        assert!(row.contains('█'), "caret on the active search row: {row:?}");
        assert!(
            !row.contains("Search extensions"),
            "no placeholder while active"
        );
    }

    #[test]
    fn status_bar_chrome_paints_one_row_per_bar() {
        let driver = TuiDriver::new(
            PanelApp {
                rect: RECT,
                panel: status_bars(),
            },
            40,
            10,
        );
        let screen: Vec<String> = driver.screen().lines().map(str::to_string).collect();
        assert!(screen[0].contains("RUN AND DEBUG"), "{screen:?}");
        assert!(screen[1].contains("Start"), "{screen:?}");
        assert!(screen[2].starts_with("body-row"), "{screen:?}");
    }
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;

    #[test]
    fn header_search_and_body_paint_top_to_bottom_on_gtk() {
        let rect = Rect::new(0.0, 0.0, 260.0, 200.0);
        let driver = GtkDriver::new(
            PanelApp {
                rect,
                panel: header_and_search(false),
            },
            280,
            220,
        );
        let header = driver.find_bounds("EXTENSIONS").expect("header painted");
        let search = driver
            .find_bounds("Search extensions")
            .expect("placeholder painted");
        let body = driver.find_bounds("body-row").expect("body painted");
        assert!(header.y < search.y && search.y < body.y);
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod macos {
    use super::*;
    use quadraui::macos::testing::MacDriver;

    #[test]
    fn header_search_and_body_paint_top_to_bottom_on_macos() {
        let rect = Rect::new(0.0, 0.0, 260.0, 200.0);
        let driver = MacDriver::new(
            PanelApp {
                rect,
                panel: header_and_search(false),
            },
            280,
            220,
        );
        let header = driver.find_bounds("EXTENSIONS").expect("header painted");
        let search = driver
            .find_bounds("Search extensions")
            .expect("placeholder painted");
        let body = driver.find_bounds("body-row").expect("body painted");
        assert!(header.y < search.y && search.y < body.y);
    }

    #[test]
    fn status_bar_chrome_paints_each_bar_on_macos() {
        let rect = Rect::new(0.0, 0.0, 260.0, 200.0);
        let driver = MacDriver::new(
            PanelApp {
                rect,
                panel: status_bars(),
            },
            280,
            220,
        );
        let title = driver
            .find_bounds("RUN AND DEBUG")
            .expect("title bar painted");
        let action = driver.find_bounds("Start").expect("action bar painted");
        assert!(title.y < action.y);
    }
}
