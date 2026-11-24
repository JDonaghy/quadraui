#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `Spinner` on the widget contract: the
//! spinner reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine.
//!
//! The animation is a pure function of the descriptor's `frame_idx`: the
//! app advances it on its own ticker and the painter indexes the shared
//! frame table, so no frame state lives in a backend.
//!
//! A spinner is purely decorative (`non_interactive!`), so a click on it
//! must take no focus, emit no widget event, and still reach the app as
//! the plain mouse event.

use quadraui::primitive::{LayoutCtx, Primitive, Size};
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, Reaction, Rect, ScreenLayout, Spinner, SpinnerHit, Surface,
    UiEvent, WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 6;
const SPIN: Rect = Rect::new(3.0, 1.0, 20.0, 1.0);

/// The frames the shared table cycles through, in order — the spinner
/// paints `FRAMES[frame_idx % FRAMES.len()]`. Spelled here rather than
/// imported because the table itself is crate-internal.
const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// One spinner, drawn and routed as a `Surface::Widget`, plus a log of
/// every event the runtime hands to `handle`.
struct SpinnerApp {
    spinner: Spinner,
    events: Vec<UiEvent>,
}

impl SpinnerApp {
    fn new(label: &str) -> Self {
        Self {
            spinner: Spinner {
                id: WidgetId::new("spinner:lsp"),
                label: label.to_string(),
                frame_idx: 0,
                accent: None,
            },
            events: Vec::new(),
        }
    }
}

impl AppLogic for SpinnerApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: SPIN,
            widget: Box::new(self.spinner.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.spinner.id, SPIN, Box::new(self.spinner.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

fn glyph_at_origin(driver: &TuiDriver<SpinnerApp>) -> char {
    driver
        .screen()
        .lines()
        .nth(SPIN.y as usize)
        .and_then(|line| line.chars().nth(SPIN.x as usize))
        .unwrap_or(' ')
}

#[test]
fn surface_widget_spinner_paints_its_glyph_then_its_label() {
    let driver = TuiDriver::new(SpinnerApp::new("Indexing"), COLS, ROWS);
    assert_eq!(glyph_at_origin(&driver), FRAMES[0], "{}", driver.screen());
    let label = driver.find_bounds("Indexing").expect("label on screen");
    assert_eq!(label.y, SPIN.y);
    assert_eq!(
        label.x,
        SPIN.x + 2.0,
        "one separator column after the glyph"
    );
}

#[test]
fn surface_widget_spinner_advances_its_glyph_with_frame_idx() {
    let mut driver = TuiDriver::new(SpinnerApp::new(""), COLS, ROWS);
    for (frame_idx, expected) in FRAMES.iter().enumerate() {
        driver.app_mut().spinner.frame_idx = frame_idx;
        driver.render();
        assert_eq!(glyph_at_origin(&driver), *expected, "frame {frame_idx}");
    }
    // The table wraps rather than running off its end.
    driver.app_mut().spinner.frame_idx = FRAMES.len() + 3;
    driver.render();
    assert_eq!(glyph_at_origin(&driver), FRAMES[3]);
}

#[test]
fn clicking_a_spinner_is_routed_but_never_consumed() {
    let mut driver = TuiDriver::new(SpinnerApp::new("Indexing"), COLS, ROWS);
    driver.click(SPIN.x, SPIN.y);

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
        "a decorative spinner must not take focus: {events:?}"
    );
}

/// `Primitive::measure` reports the glyph-plus-label run's own size on
/// both axes — a spinner is an inline indicator, not a stretcher.
#[test]
fn spinner_measures_its_own_inline_run() {
    let labelled = SpinnerApp::new("Indexing").spinner;
    let cells = LayoutCtx::new(1.0, 1.0, false);
    let natural = labelled.measure(&cells, Size::new(COLS as f32, ROWS as f32));
    // Glyph + separator + 8 label columns.
    assert_eq!(natural.width, Some(10.0));
    assert_eq!(natural.height, Some(1.0));

    let bare = SpinnerApp::new("").spinner;
    let natural = bare.measure(&cells, Size::new(COLS as f32, ROWS as f32));
    assert_eq!(natural.width, Some(1.0), "glyph only");
}

/// The cell layout is clipped to the rect it was given, and its bounds
/// hit-test in the absolute frame they were laid out in.
#[test]
fn spinner_layout_clips_to_its_rect_and_hit_tests_absolutely() {
    let spinner = SpinnerApp::new("Indexing").spinner;
    let ctx = LayoutCtx::new(1.0, 1.0, false);

    // Spelled through the trait: `Spinner` also has an inherent
    // `layout(origin_x, origin_y, measure)` that would shadow it.
    let layout = Primitive::layout(&spinner, &ctx, SPIN);
    assert_eq!(layout.bounds.x, SPIN.x);
    assert_eq!(layout.bounds.y, SPIN.y);
    assert_eq!(layout.bounds.width, 10.0);
    assert_eq!(
        layout.hit_test(SPIN.x + 0.5, SPIN.y + 0.5, &spinner.id),
        SpinnerHit::Body(spinner.id)
    );
    assert_eq!(
        layout.hit_test(SPIN.x + 12.0, SPIN.y + 0.5, &spinner.id),
        SpinnerHit::Empty
    );

    let narrow = Rect::new(SPIN.x, SPIN.y, 4.0, 1.0);
    let layout = Primitive::layout(&spinner, &ctx, narrow);
    assert_eq!(layout.bounds.width, 4.0, "never wider than its rect");
}
