#![cfg(feature = "tui")]
//! Black-box `TuiDriver` test for `ProgressBar` on the widget contract:
//! the bar reaches the frame as a type-erased `Surface::Widget`, is
//! painted through `DynWidget::paint` → `Primitive::paint_cells`, and is
//! registered in the app's `FrameTree` so input routes through the shared
//! widget-contract engine.
//!
//! A bar is `non_interactive!` — even its cancel affordance is the host's,
//! classified with `ProgressBarLayout::hit_test` — so a click on it must
//! take no focus, emit no widget event, and still reach the app as the
//! plain mouse event.

use quadraui::primitive::{LayoutCtx, Primitive, Size};
use quadraui::tui::testing::TuiDriver;
use quadraui::{
    AppLogic, Backend, FrameTree, ProgressBar, Reaction, Rect, ScreenLayout, Surface, UiEvent,
    WidgetId,
};

const COLS: u16 = 40;
const ROWS: u16 = 6;
const BAR: Rect = Rect::new(2.0, 2.0, 20.0, 1.0);

/// One progress bar, drawn and routed as a `Surface::Widget`, plus a log
/// of every event the runtime hands to `handle`.
struct ProgressApp {
    bar: ProgressBar,
    events: Vec<UiEvent>,
}

impl ProgressApp {
    fn new(value: Option<f32>, label: &str, cancellable: bool) -> Self {
        Self {
            bar: ProgressBar {
                id: WidgetId::new("progress:download"),
                label: label.to_string(),
                value,
                frame_idx: 0,
                cancellable,
                accent: None,
            },
            events: Vec::new(),
        }
    }
}

impl AppLogic for ProgressApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: BAR,
            widget: Box::new(self.bar.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.bar.id, BAR, Box::new(self.bar.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        self.events.push(event);
        Reaction::Continue
    }
}

fn row(driver: &TuiDriver<ProgressApp>, y: usize) -> Vec<char> {
    driver
        .screen()
        .lines()
        .nth(y)
        .map(|line| line.chars().collect())
        .unwrap_or_default()
}

#[test]
fn surface_widget_progress_fills_half_its_track_at_half_value() {
    let driver = TuiDriver::new(ProgressApp::new(Some(0.5), "", false), COLS, ROWS);
    let cells = row(&driver, BAR.y as usize);
    let x = BAR.x as usize;
    assert_eq!(cells[x], '█', "{}", driver.screen());
    assert_eq!(cells[x + 9], '█', "{}", driver.screen());
    assert_eq!(cells[x + 10], '░', "{}", driver.screen());
    assert_eq!(cells[x + 19], '░', "{}", driver.screen());
}

#[test]
fn surface_widget_progress_overlays_its_label_on_the_track() {
    let driver = TuiDriver::new(ProgressApp::new(Some(0.5), "Fetching", false), COLS, ROWS);
    let bounds = driver.find_bounds("Fetching").expect("label on screen");
    assert_eq!(bounds.y, BAR.y);
    assert_eq!(bounds.x, BAR.x + 1.0, "one pad column inside the track");
}

#[test]
fn surface_widget_progress_paints_a_cancel_glyph_when_cancellable() {
    let driver = TuiDriver::new(ProgressApp::new(Some(0.5), "", true), COLS, ROWS);
    assert!(driver.screen_contains("×"), "{}", driver.screen());
}

#[test]
fn surface_widget_progress_slides_its_pulse_when_indeterminate() {
    let first = TuiDriver::new(ProgressApp::new(None, "", false), COLS, ROWS);
    let before = row(&first, BAR.y as usize);

    let mut later = TuiDriver::new(ProgressApp::new(None, "", false), COLS, ROWS);
    later.app_mut().bar.frame_idx = 5;
    later.render();
    let after = row(&later, BAR.y as usize);

    assert_ne!(before, after, "the pulse must move with frame_idx");
}

#[test]
fn clicking_a_progress_bar_is_routed_but_never_consumed() {
    let mut driver = TuiDriver::new(ProgressApp::new(Some(0.5), "", true), COLS, ROWS);
    driver.click(BAR.x + 5.0, BAR.y);

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
        "a non-interactive bar must not take focus: {events:?}"
    );
}

/// `Primitive::measure` reports one row and no intrinsic width — a bar's
/// length is the measurement it communicates, so a container stretches
/// it.
#[test]
fn progress_measures_one_row_tall_and_stretches_wide() {
    let bar = ProgressApp::new(Some(0.5), "Fetching", false).bar;
    let cells = LayoutCtx::new(1.0, 1.0, false);
    let natural = bar.measure(&cells, Size::new(COLS as f32, ROWS as f32));
    assert_eq!(natural.width, None);
    assert_eq!(natural.height, Some(1.0));

    // The same answer in the pixel regime, in that regime's own units.
    let pixels = LayoutCtx::new(18.0, 8.0, false);
    let natural = bar.measure(&pixels, Size::new(400.0, 100.0));
    assert_eq!(natural.width, None);
    assert_eq!(natural.height, Some(18.0));
}

/// `Primitive::layout` reports bounds in the absolute frame it was given,
/// and the cancel gutter it reserves is hit-testable at the coordinates
/// the painter drew the glyph at.
#[test]
fn progress_layout_and_hit_test_agree_in_the_absolute_frame() {
    use quadraui::ProgressBarHit;

    let bar = ProgressApp::new(Some(0.5), "", true).bar;
    let ctx = LayoutCtx::new(1.0, 1.0, false);
    // Spelled through the trait: `ProgressBar` also has an inherent
    // `layout(origin_x, origin_y, measure)` that would shadow it.
    let layout = Primitive::layout(&bar, &ctx, BAR);
    assert_eq!(layout.bounds, BAR);

    let cancel = layout.cancel_bounds.expect("cancellable bar has a gutter");
    assert_eq!(cancel.x, BAR.x + BAR.width - 3.0);
    assert_eq!(
        layout.hit_test(cancel.x + 1.5, BAR.y + 0.5),
        ProgressBarHit::Cancel(bar.id)
    );
    assert_eq!(
        layout.hit_test(BAR.x + 1.0, BAR.y + 0.5),
        ProgressBarHit::Body(bar.id)
    );
    assert_eq!(layout.hit_test(0.0, 0.0), ProgressBarHit::Empty);
}
