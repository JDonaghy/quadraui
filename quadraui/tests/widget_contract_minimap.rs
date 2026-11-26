#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Minimap` on the widget contract: the
//! minimap reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells` as
//! braille, and is registered in the app's `FrameTree` so a click or drag
//! routes through `Interactive::handle` and comes back out as the
//! `UiEvent::Minimap(_, MinimapEvent::Seek { .. })` this app records.

use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, Minimap, MinimapEvent, MinimapLine, MinimapScale, MinimapSizing,
    Reaction, Rect, ScreenLayout, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 20;
const ROWS: u16 = 12;
/// A four-cell-wide, eight-row strip: 32 buffer lines at four per row.
const AREA: Rect = Rect::new(2.0, 1.0, 4.0, 8.0);

fn minimap_id() -> WidgetId {
    WidgetId::new("minimap:editor")
}

fn minimap(sizing: Option<MinimapSizing>) -> Minimap {
    Minimap {
        id: minimap_id(),
        lines: (0..32)
            .map(|i| MinimapLine {
                text: if i % 4 == 0 {
                    "fn main() {}".to_string()
                } else {
                    String::new()
                },
                line_idx: i,
            })
            .collect(),
        syntax_spans: vec![],
        visible_row_start: 0,
        visible_row_count: 8,
        total_buffer_lines: 32,
        scale: MinimapScale::One,
        sizing,
    }
}

struct MinimapApp {
    minimap: Minimap,
    seeks: Vec<f32>,
}

impl AppLogic for MinimapApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.minimap.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.minimap.id, AREA, Box::new(self.minimap.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        if let UiEvent::Minimap(id, MinimapEvent::Seek { fraction }) = event {
            if id == minimap_id() {
                self.seeks.push(fraction);
            }
        }
        Reaction::Continue
    }
}

fn driver(sizing: Option<MinimapSizing>) -> TuiDriver<MinimapApp> {
    TuiDriver::new(
        MinimapApp {
            minimap: minimap(sizing),
            seeks: Vec::new(),
        },
        COLS,
        ROWS,
    )
}

fn cell(driver: &TuiDriver<MinimapApp>, x: u16, y: u16) -> char {
    driver.styled_row(y)[x as usize].0
}

#[test]
fn surface_widget_minimap_paints_braille_rows() {
    let driver = driver(None);
    let (x0, y0) = (AREA.x as u16, AREA.y as u16);
    // Each row packs four lines; only the first carries text, so only the
    // top dot row of each cell is set.
    for y in y0..y0 + AREA.height as u16 {
        for x in x0..x0 + AREA.width as u16 {
            let ch = cell(&driver, x, y) as u32;
            assert!(
                (0x2800..=0x28FF).contains(&ch),
                "({x},{y}) is not braille: {}",
                driver.screen()
            );
        }
    }
    // "fn main() {}" starts non-blank: the first cell's top-left dot is set.
    assert_ne!(cell(&driver, x0, y0), '\u{2800}', "{}", driver.screen());
    // Nothing outside the strip.
    assert_eq!(cell(&driver, x0 + AREA.width as u16, y0), ' ');
}

#[test]
fn repeat_to_fill_sizing_on_the_descriptor_stretches_a_short_file() {
    let mut driver = driver(Some(MinimapSizing::RepeatToFill(1.0)));
    driver.app_mut().minimap.lines.truncate(8);
    driver.app_mut().minimap.total_buffer_lines = 8;
    driver.render();
    let (x0, y0) = (AREA.x as u16, AREA.y as u16);
    // Two rows of content, each repeated across four cell rows.
    for dy in 0..4 {
        assert_eq!(cell(&driver, x0, y0 + dy), cell(&driver, x0, y0));
        assert_eq!(cell(&driver, x0, y0 + 4 + dy), cell(&driver, x0, y0 + 4));
    }
    assert_ne!(cell(&driver, x0, y0), '\u{2800}', "{}", driver.screen());
}

#[test]
fn clicking_the_strip_seeks_to_the_clicked_fraction() {
    let mut driver = driver(None);
    driver.click(AREA.x + 1.0, AREA.y + 4.0);
    assert_eq!(driver.app().seeks, vec![0.5]);
}

#[test]
fn a_drag_keeps_seeking_and_clamps_past_the_strip() {
    let mut driver = driver(None);
    driver.mouse_down(AREA.x + 1.0, AREA.y);
    driver.mouse_move(AREA.x + 1.0, AREA.y + 2.0);
    driver.mouse_move(AREA.x + 9.0, AREA.y + 40.0);
    driver.mouse_up(AREA.x + 9.0, AREA.y + 40.0);
    // Released: a later move seeks nothing.
    driver.mouse_move(AREA.x + 1.0, AREA.y + 3.0);
    assert_eq!(driver.app().seeks, vec![0.0, 0.25, 1.0]);
}

#[test]
fn a_move_without_a_drag_seeks_nothing() {
    let mut driver = driver(None);
    driver.mouse_move(AREA.x + 1.0, AREA.y + 2.0);
    assert!(driver.app().seeks.is_empty());
}

#[test]
fn scale_and_sizing_round_trip_through_serde_and_default_when_absent() {
    let mut mm = minimap(Some(MinimapSizing::VsCodeParity {
        target_cols: 120.0,
        fraction: 0.15,
        min: 10.0,
        max: 160.0,
    }));
    mm.scale = MinimapScale::Two;
    let json = serde_json::to_value(&mm).expect("serialize");
    let back: Minimap = serde_json::from_value(json.clone()).expect("deserialize");
    assert_eq!(back, mm);

    let mut bare = json;
    let obj = bare.as_object_mut().expect("object");
    obj.remove("scale");
    obj.remove("sizing");
    let back: Minimap = serde_json::from_value(bare).expect("deserialize without the new keys");
    assert_eq!(back.scale, MinimapScale::One);
    assert_eq!(back.sizing, None);
}
