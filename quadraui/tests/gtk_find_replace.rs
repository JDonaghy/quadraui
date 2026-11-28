#![cfg(feature = "gtk")]
//! Headless `GtkDriver` paint->click round trip for `FindReplacePanel` on
//! the widget contract, against a real, proportional Pango font.
//!
//! `quadraui::gtk::testing::GtkDriver::new` always seeds a real `"Sans
//! 11"` Pango context before the first frame paints (see that module's
//! doc), so a TUI-style `char_width == 1.0` assumption anywhere between
//! paint and hit-test would mis-route every click here: the real glyph
//! pitch is not an integer number of device px per cell.
//!
//! The panel is painted as a `Surface::Widget` in the box
//! `FindReplacePanel::anchored_rect` places and registered in the app's
//! `FrameTree`, so every click and key runs through the shared routing
//! engine into the panel's own `Interactive::handle`, and comes back as
//! the `UiEvent::FindReplace` this host writes back with
//! `FindReplacePanel::apply_event`. Glyphs are read back off the painted
//! screen rather than off internal paint-call recordings.

use std::cell::Cell;

use quadraui::{
    AppLogic, Backend, FindReplaceEvent, FindReplacePanel, FrameTree, Key, Modifiers, MouseButton,
    NamedKey, Point, Reaction, Rect, ScreenLayout, Surface, UiEvent, WidgetId,
};

use quadraui::gtk::testing::GtkDriver;

const CANVAS_W: i32 = 700;
const CANVAS_H: i32 = 200;

// `crate::codicon::{CASE_SENSITIVE, WHOLE_WORD, REGEX, PRESERVE_CASE}`
// are `pub(crate)`, unreachable from this external test crate, so these
// mirror those exact codepoints (see `quadraui/src/codicon.rs`) for use
// as `GtkDriver::find`/`screen_contains` needles. Each is a single
// Private Use Area codepoint, so a substring match against painted text
// cannot collide with anything else on screen.
const CASE_SENSITIVE_GLYPH: char = '\u{eab1}';
const WHOLE_WORD_GLYPH: char = '\u{eb7e}';
const REGEX_GLYPH: char = '\u{eb38}';
const PRESERVE_CASE_GLYPH: char = '\u{eb2e}';

/// A `FindReplacePanel` with the replace row visible (so all four
/// toggles, including preserve case, have a hit region), routed through
/// the app's frame tree, plus every event it reported.
struct FindReplaceDriverApp {
    panel: FindReplacePanel,
    events: Vec<FindReplaceEvent>,
    rect: Cell<Rect>,
}

impl FindReplaceDriverApp {
    fn new() -> Self {
        let (hit_regions, _input_width) =
            quadraui::compute_find_replace_hit_regions(50, true, "1 of 3", 2, 2);
        let panel = FindReplacePanel {
            id: WidgetId::new("find_replace"),
            query: "needle".into(),
            replacement: "pin".into(),
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
            events: Vec::new(),
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
                self.events.push(event);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}

fn find_glyph(driver: &GtkDriver<FindReplaceDriverApp>, glyph: char) -> (f32, f32) {
    let needle = glyph.to_string();
    driver
        .find(&needle)
        .unwrap_or_else(|| panic!("expected codicon glyph {glyph:?} painted on screen"))
}

#[test]
fn toggles_paint_as_codicon_glyphs_with_a_proportional_font() {
    let driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(driver.screen_contains(&CASE_SENSITIVE_GLYPH.to_string()));
    assert!(driver.screen_contains(&WHOLE_WORD_GLYPH.to_string()));
    assert!(driver.screen_contains(&REGEX_GLYPH.to_string()));
    assert!(driver.screen_contains(&PRESERVE_CASE_GLYPH.to_string()));
    // The TUI literal glyph pairs must not leak into the GTK rasteriser.
    assert!(!driver.screen_contains("Aa"));
    assert!(!driver.screen_contains(".*"));
}

#[test]
fn clicking_the_match_case_toggle_flips_state_at_its_codicon_glyph() {
    let mut driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(!driver.app().panel.case_sensitive);

    let (x, y) = find_glyph(&driver, CASE_SENSITIVE_GLYPH);
    driver.click(x, y);

    assert!(driver.app().panel.case_sensitive);
    assert!(driver.screen_contains(&CASE_SENSITIVE_GLYPH.to_string()));
}

#[test]
fn clicking_the_whole_word_toggle_flips_state_at_its_codicon_glyph() {
    let mut driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(!driver.app().panel.whole_word);

    let (x, y) = find_glyph(&driver, WHOLE_WORD_GLYPH);
    driver.click(x, y);

    assert!(driver.app().panel.whole_word);
    assert!(driver.screen_contains(&WHOLE_WORD_GLYPH.to_string()));
}

#[test]
fn clicking_the_regex_toggle_flips_state_at_its_codicon_glyph() {
    let mut driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    assert!(!driver.app().panel.use_regex);

    let (x, y) = find_glyph(&driver, REGEX_GLYPH);
    driver.click(x, y);

    assert!(driver.app().panel.use_regex);
    assert!(driver.screen_contains(&REGEX_GLYPH.to_string()));
}

#[test]
fn clicking_the_preserve_case_toggle_flips_state_at_its_codicon_glyph() {
    let mut driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
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
    let mut driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
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

/// Click the start of the query text, focusing the panel and parking the
/// caret before its first character.
fn focus_query_start(driver: &mut GtkDriver<FindReplaceDriverApp>) {
    let b = driver.find_bounds("needle").expect("query painted");
    driver.click(b.x + 1.0, b.y + b.height / 2.0);
    assert_eq!(driver.app().panel.focus, 0);
    assert_eq!(driver.app().panel.cursor, 0, "{:?}", driver.app().events);
}

#[test]
fn typing_edits_the_query_through_the_routing_engine() {
    let mut driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    focus_query_start(&mut driver);

    driver.type_char('x');
    assert_eq!(driver.app().panel.query, "xneedle");
    assert!(
        driver.screen_contains("xneedle"),
        "{:?}",
        driver.painted_texts()
    );

    driver.press_named(NamedKey::End);
    driver.press_named(NamedKey::Backspace);
    assert_eq!(driver.app().panel.query, "xneedl");
}

#[test]
fn enter_and_the_arrow_buttons_step_through_matches() {
    let mut driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    focus_query_start(&mut driver);
    driver.app_mut().events.clear();

    driver.press_named(NamedKey::Enter);
    driver.dispatch(UiEvent::KeyPressed {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    let (x, y) = driver.find("\u{2193}").expect("next button painted");
    driver.click(x, y);
    let (x, y) = driver.find("\u{2191}").expect("prev button painted");
    driver.click(x, y);

    assert_eq!(
        driver.app().events,
        vec![
            FindReplaceEvent::NextMatch,
            FindReplaceEvent::PrevMatch,
            FindReplaceEvent::NextMatch,
            FindReplaceEvent::PrevMatch,
        ]
    );
}

#[test]
fn replace_field_typing_and_the_replace_buttons_report() {
    let mut driver = GtkDriver::new(FindReplaceDriverApp::new(), CANVAS_W, CANVAS_H);
    focus_query_start(&mut driver);
    driver.press_named(NamedKey::Tab);
    assert_eq!(driver.app().panel.focus, 1);
    driver.type_char('e');
    assert_eq!(driver.app().panel.replacement, "pine");
    assert!(
        driver.screen_contains("pine"),
        "{:?}",
        driver.painted_texts()
    );

    driver.app_mut().events.clear();
    driver.press_named(NamedKey::Enter);
    let (x, y) = driver.find("R1").expect("replace-one painted");
    driver.click(x, y);
    let (x, y) = driver.find("R*").expect("replace-all painted");
    driver.click(x, y);
    assert_eq!(
        driver.app().events,
        vec![
            FindReplaceEvent::ReplaceCurrent,
            FindReplaceEvent::ReplaceCurrent,
            FindReplaceEvent::ReplaceAll,
        ]
    );
}
