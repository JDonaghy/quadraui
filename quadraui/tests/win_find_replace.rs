#![cfg(all(feature = "win", target_os = "windows"))]
//! Headless `WinDriver` paint->click round trip for `FindReplacePanel`'s
//! four toggle buttons (match case, whole word, regex, preserve case)
//! against a real, proportional DirectWrite font.
//!
//! Mirrors `tests/gtk_find_replace.rs` one-for-one for the Win-GUI
//! backend: `WinBackend::new` self-registers the bundled codicon font and
//! seeds a codicon-only `nerd_font_fallback` before the first frame
//! paints (see `src/win/backend.rs`'s
//! `new_self_registers_codicon_and_seeds_a_fallback` test), so this
//! exercises the px-space hit-region model's failure mode: a TUI-style
//! `char_width == 1.0` assumption would silently mis-hit-test every click
//! here, because the real DirectWrite glyph pitch is not an integer
//! number of device px per cell.
//!
//! The panel paints through its widget contract and every click runs
//! through the shared routing engine into the panel's own
//! `Interactive::handle`; glyphs are read back off the painted screen
//! rather than off internal paint-call recordings.

use std::cell::Cell;

use quadraui::{
    AppLogic, Backend, FindReplacePanel, FrameTree, Modifiers, MouseButton, Point, Reaction, Rect,
    ScreenLayout, Surface, UiEvent, WidgetId,
};

use quadraui::win::testing::WinDriver;

const CANVAS_W: u32 = 700;
const CANVAS_H: u32 = 200;

// `crate::codicon::{CASE_SENSITIVE, WHOLE_WORD, REGEX, PRESERVE_CASE}`
// are `pub(crate)`, unreachable from this external test crate, so these
// mirror those exact codepoints (see `quadraui/src/codicon.rs`) for use
// as `WinDriver::find`/`screen_contains` needles. Each is a single
// Private Use Area codepoint, so a substring match against painted text
// cannot collide with anything else on screen.
const CASE_SENSITIVE_GLYPH: char = '\u{eab1}';
const WHOLE_WORD_GLYPH: char = '\u{eb7e}';
const REGEX_GLYPH: char = '\u{eb38}';
const PRESERVE_CASE_GLYPH: char = '\u{eb2e}';

/// A `FindReplacePanel` with the replace row visible (so all four
/// toggles, including preserve case, have a hit region), painted as a
/// `Surface::Widget` in the box `FindReplacePanel::anchored_rect` places
/// and routed through the app's frame tree into the panel's own
/// `Interactive::handle`; every reported event is written back with
/// `FindReplacePanel::apply_event`.
struct FindReplaceDriverApp {
    panel: FindReplacePanel,
    rect: Cell<Rect>,
}

impl FindReplaceDriverApp {
    fn new() -> Self {
        let (hit_regions, _input_width) =
            quadraui::compute_find_replace_hit_regions(50, true, "1 of 3", 2, 2);
        let panel = FindReplacePanel {
            id: WidgetId::new("find_replace"),
            query: "needle".into(),
            replacement: String::new(),
            show_replace: true,
            focus: 0,
            cursor: 6,
            sel_anchor: None,
            match_info: "1 of 3".into(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            preserve_case: false,
            in_selection: false,
            group_bounds: Rect::new(0.0, 0.0, CANVAS_W as f32, CANVAS_H as f32),
            panel_width: 50,
            replace_one_glyph: "R1".into(),
            replace_all_glyph: "R*".into(),
            hit_regions,
            hidden_toggles: Vec::new(),
        };
        Self {
            panel,
            rect: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
        }
    }
}

impl AppLogic for FindReplaceDriverApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let rect = self.panel.anchored_rect(backend, self.panel.group_bounds);
        self.rect.set(rect);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect,
            widget: Box::new(self.panel.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.panel.id, self.rect.get(), Box::new(self.panel.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::FindReplace(id, event) if id == self.panel.id => {
                self.panel.apply_event(&event);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn find_glyph(driver: &WinDriver<FindReplaceDriverApp>, glyph: char) -> (f32, f32) {
    let needle = glyph.to_string();
    driver
        .find(&needle)
        .unwrap_or_else(|| panic!("expected codicon glyph {glyph:?} painted on screen"))
}

#[test]
fn toggles_paint_as_codicon_glyphs_with_a_proportional_font() {
    let driver = WinDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(driver.screen_contains(&CASE_SENSITIVE_GLYPH.to_string()));
    assert!(driver.screen_contains(&WHOLE_WORD_GLYPH.to_string()));
    assert!(driver.screen_contains(&REGEX_GLYPH.to_string()));
    assert!(driver.screen_contains(&PRESERVE_CASE_GLYPH.to_string()));
    // The TUI literal glyph pairs must not leak into the Win-GUI rasteriser.
    assert!(!driver.screen_contains("Aa"));
    assert!(!driver.screen_contains(".*"));
}

#[test]
fn clicking_the_match_case_toggle_flips_state_at_its_codicon_glyph() {
    let mut driver = WinDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(!driver.app().panel.case_sensitive);

    let (x, y) = find_glyph(&driver, CASE_SENSITIVE_GLYPH);
    driver.click(x, y);

    assert!(driver.app().panel.case_sensitive);
    assert!(driver.screen_contains(&CASE_SENSITIVE_GLYPH.to_string()));
}

#[test]
fn clicking_the_whole_word_toggle_flips_state_at_its_codicon_glyph() {
    let mut driver = WinDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(!driver.app().panel.whole_word);

    let (x, y) = find_glyph(&driver, WHOLE_WORD_GLYPH);
    driver.click(x, y);

    assert!(driver.app().panel.whole_word);
    assert!(driver.screen_contains(&WHOLE_WORD_GLYPH.to_string()));
}

#[test]
fn clicking_the_regex_toggle_flips_state_at_its_codicon_glyph() {
    let mut driver = WinDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(!driver.app().panel.use_regex);

    let (x, y) = find_glyph(&driver, REGEX_GLYPH);
    driver.click(x, y);

    assert!(driver.app().panel.use_regex);
    assert!(driver.screen_contains(&REGEX_GLYPH.to_string()));
}

#[test]
fn clicking_the_preserve_case_toggle_flips_state_at_its_codicon_glyph() {
    let mut driver = WinDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(!driver.app().panel.preserve_case);

    let (x, y) = find_glyph(&driver, PRESERVE_CASE_GLYPH);
    driver.click(x, y);

    assert!(driver.app().panel.preserve_case);
    assert!(driver.screen_contains(&PRESERVE_CASE_GLYPH.to_string()));
}

/// A click landing just outside a toggle's region (one device px short of
/// its left edge) must miss -- this is the regression #1432's px-space
/// model guards against: with a proportional font, a stale `char_width
/// == 1.0` assumption would make hit-testing drift from the painted
/// glyph position by many pixels, not sub-pixel amounts, so this would
/// fail loudly rather than flakily if the px projection ever broke.
#[test]
fn click_outside_a_toggles_region_does_not_flip_it() {
    let mut driver = WinDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    let bounds = driver
        .find_bounds(&CASE_SENSITIVE_GLYPH.to_string())
        .expect("case-sensitive glyph painted");

    driver.dispatch(UiEvent::MouseDown {
        widget: None,
        button: MouseButton::Left,
        position: Point::new(bounds.x - 5.0, bounds.y + bounds.height / 2.0),
        modifiers: Modifiers::default(),
    });

    assert!(!driver.app().panel.case_sensitive);
}
