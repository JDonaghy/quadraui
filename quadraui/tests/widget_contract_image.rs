#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Image` on the widget contract: the
//! image reaches the frame as a type-erased `Surface::Widget`, is painted
//! through `DynWidget::paint` → `Primitive::paint_cells`, which asks the
//! cell surface's graphics channel first and paints the fallback text
//! itself when the channel paints nothing.
//!
//! An image is `non_interactive!`, so a click on it must take no focus,
//! emit no widget event, and still reach the app as the plain mouse event.

use quadraui::primitive::{LayoutCtx, Primitive, Size};
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, ColorDepth, FrameTree, Image, ImageFit, ImageSource, Reaction, Rect,
    ScreenLayout, Surface, UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 8;
const AREA: Rect = Rect::new(4.0, 2.0, 12.0, 4.0);

fn logo_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets/quadra_logo.png")
}

fn logo(fallback_text: &str) -> Image {
    Image {
        id: WidgetId::new("image:logo"),
        source: ImageSource::Path(logo_path()),
        intrinsic_size: Some((64, 64)),
        fit: ImageFit::Fill,
        fallback_text: fallback_text.to_string(),
    }
}

/// One image, drawn and routed as a `Surface::Widget`, plus a log of
/// every event the runtime hands to `handle`.
struct ImageApp {
    image: Image,
    events: Vec<UiEvent>,
}

impl AppLogic for ImageApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: AREA,
            widget: Box::new(self.image.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.image.id, AREA, Box::new(self.image.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

/// A driver whose terminal reports `depth` and no Kitty graphics, with
/// the frame re-rendered under that capability.
fn driver(image: Image, depth: ColorDepth) -> TuiDriver<ImageApp> {
    let mut driver = TuiDriver::new(
        ImageApp {
            image,
            events: Vec::new(),
        },
        COLS,
        ROWS,
    );
    let (backend, _) = driver.parts_mut();
    backend.set_color_depth(depth);
    backend.set_kitty_graphics_supported(false);
    driver.render();
    driver
}

fn row(driver: &TuiDriver<ImageApp>, y: usize) -> String {
    driver
        .screen()
        .lines()
        .nth(y)
        .unwrap_or_default()
        .to_string()
}

#[test]
fn surface_widget_image_paints_centered_fallback_text_without_a_graphics_channel() {
    let driver = driver(logo("[LOGO]"), ColorDepth::Indexed256);
    let bounds = driver
        .find_bounds("[LOGO]")
        .unwrap_or_else(|| panic!("fallback text on screen:\n{}", driver.screen()));
    // Centered: (12 - 6) / 2 = 3 columns in, on the middle of 4 rows.
    assert_eq!(bounds.x, AREA.x + 3.0, "{}", driver.screen());
    assert_eq!(bounds.y, AREA.y + 2.0, "{}", driver.screen());
}

#[test]
fn surface_widget_image_with_empty_fallback_leaves_its_rect_blank() {
    let driver = driver(logo(""), ColorDepth::Indexed256);
    for y in AREA.y as usize..(AREA.y + AREA.height) as usize {
        let line: String = row(&driver, y)
            .chars()
            .skip(AREA.x as usize)
            .take(AREA.width as usize)
            .collect();
        assert!(line.trim().is_empty(), "row {y}: {line:?}");
    }
}

#[test]
fn surface_widget_image_fallback_is_clipped_to_its_rect() {
    let driver = driver(
        logo("a fallback far wider than the rect"),
        ColorDepth::Indexed256,
    );
    let line = row(&driver, (AREA.y + 2.0) as usize);
    let inside: String = line
        .chars()
        .skip(AREA.x as usize)
        .take(AREA.width as usize)
        .collect();
    assert_eq!(inside, "a fallback f", "{}", driver.screen());
    let past: String = line.chars().skip((AREA.x + AREA.width) as usize).collect();
    assert!(past.trim().is_empty(), "{}", driver.screen());
}

#[cfg(feature = "tui-images")]
#[test]
fn surface_widget_image_paints_a_halfblock_mosaic_on_a_truecolor_terminal() {
    let driver = driver(logo("[LOGO]"), ColorDepth::TrueColor);
    assert!(!driver.screen_contains("[LOGO]"), "{}", driver.screen());
    for y in AREA.y as usize..(AREA.y + AREA.height) as usize {
        let line: String = row(&driver, y)
            .chars()
            .skip(AREA.x as usize)
            .take(AREA.width as usize)
            .collect();
        assert_eq!(line, "▀".repeat(AREA.width as usize), "row {y}");
    }
}

#[cfg(feature = "tui-images")]
#[test]
fn surface_widget_image_paints_kitty_placeholders_when_kitty_graphics_are_confirmed() {
    let mut driver = driver(logo("[LOGO]"), ColorDepth::TrueColor);
    let (backend, _) = driver.parts_mut();
    backend.set_kitty_graphics_supported(true);
    backend.set_kitty_graphics_cell_px(Some((8, 16)));
    driver.render();
    let line = row(&driver, AREA.y as usize);
    let first = line.chars().nth(AREA.x as usize);
    assert_eq!(first, Some('\u{10EEEE}'), "{}", driver.screen());
    assert!(!driver.screen_contains("[LOGO]"), "{}", driver.screen());
}

#[test]
fn clicking_an_image_is_routed_but_never_consumed() {
    let mut driver = driver(logo("[LOGO]"), ColorDepth::Indexed256);
    driver.click(AREA.x + 5.0, AREA.y + 2.0);

    let events = &driver.app().events;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, UiEvent::MouseDown { .. })),
        "the plain mouse-down must reach the app: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, UiEvent::FocusChanged(Some(_)))),
        "a non-interactive image must not take focus: {events:?}"
    );
}

/// On a cell grid the natural size is the fallback text's width by one
/// row — what paints wherever the terminal cannot rasterise — and nothing
/// when there is no fallback; on a pixel surface it is `intrinsic_size`.
#[test]
fn image_measures_its_fallback_on_cells_and_its_intrinsic_size_on_pixels() {
    let cells = LayoutCtx::new(1.0, 1.0, false);
    let avail = Size::new(COLS as f32, ROWS as f32);
    let natural = logo("[LOGO]").measure(&cells, avail);
    assert_eq!(natural.width, Some(6.0));
    assert_eq!(natural.height, Some(1.0));
    let natural = logo("").measure(&cells, avail);
    assert_eq!((natural.width, natural.height), (None, None));

    let pixels = LayoutCtx::new(18.0, 8.0, false);
    let avail = Size::new(400.0, 300.0);
    let natural = logo("[LOGO]").measure(&pixels, avail);
    assert_eq!(natural.width, Some(64.0));
    assert_eq!(natural.height, Some(64.0));
    let mut unsized_logo = logo("[LOGO]");
    unsized_logo.intrinsic_size = None;
    let natural = unsized_logo.measure(&pixels, avail);
    assert_eq!((natural.width, natural.height), (None, None));
}

/// `Primitive::layout` keeps the rect it was given as the paint area and
/// fits the image within it.
#[test]
fn image_layout_keeps_its_area_and_fits_within_it() {
    let mut image = logo("[LOGO]");
    image.fit = ImageFit::Contain;
    let ctx = LayoutCtx::new(18.0, 8.0, false);
    let rect = Rect::new(10.0, 10.0, 200.0, 64.0);
    // Spelled through the trait: `Image` also has an inherent
    // `layout(bounds)` that would shadow it.
    let layout = Primitive::layout(&image, &ctx, rect);
    assert_eq!(layout.area, rect);
    assert_eq!(layout.bounds, Rect::new(78.0, 10.0, 64.0, 64.0));
}
