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
//! exercises the exact bug class #1432's px-space hit-region model
//! exists to fix: a TUI-style `char_width == 1.0` assumption would
//! silently mis-hit-test every click here, because the real DirectWrite
//! glyph pitch is not an integer number of device px per cell.
//!
//! This drives the real `event -> handle -> render` path
//! (`WinBackend::draw_find_replace` for paint, `FindReplacePanel::hit_test`
//! fed with `Backend::char_width()` / `Backend::line_height()` for click)
//! the same way a production click router would, and reads the glyph
//! back off the painted screen rather than off internal paint-call
//! recordings.

use quadraui::{
    AppLogic, Backend, FindReplaceClickTarget, FindReplaceHit, FindReplacePanel, Modifiers,
    MouseButton, Point, Reaction, Rect, UiEvent,
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

/// Minimal `AppLogic` wrapping a `FindReplacePanel` with the replace row
/// visible (so all four toggles, including `TogglePreserveCase`, have a
/// hit region) and routing clicks through the exact popup-geometry
/// formula `WinBackend::draw_find_replace` -> `native_surface_paint::paint`
/// uses, fed with the backend's real (proportional) `char_width()` /
/// `line_height()` -- never a hardcoded `1.0, 1.0` TUI-style scale.
struct FindReplaceDriverApp {
    panel: FindReplacePanel,
}

impl FindReplaceDriverApp {
    fn new() -> Self {
        let (hit_regions, _input_width) =
            quadraui::compute_find_replace_hit_regions(50, true, "1 of 3", 2, 2);
        let panel = FindReplacePanel {
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
        };
        Self { panel }
    }

    /// The panel's content-corner origin, derived the same way
    /// `WinBackend::draw_find_replace` (via
    /// `primitives::find_replace::native_surface_paint::paint`) derives
    /// it at paint time -- see that function's `popup_w`/`popup_h`/
    /// `popup_x`/`popup_y` for the formula this mirrors.
    fn content_origin(&self, cw: f32, lh: f32) -> (f32, f32) {
        let popup_w = self.panel.panel_width as f32 * cw;
        let row_count = if self.panel.show_replace { 2.0 } else { 1.0 };
        let popup_h = (row_count + 2.0) * lh;
        let _ = popup_h;
        let gb = self.panel.group_bounds;
        let popup_x = ((gb.x + gb.width) - popup_w - 10.0).max(gb.x);
        let popup_y = gb.y + 2.0;
        (popup_x + cw, popup_y + lh)
    }
}

impl AppLogic for FindReplaceDriverApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_find_replace(self.panel.group_bounds, &self.panel);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::MouseDown { position, .. } => {
                let cw = backend.char_width().max(1.0);
                let lh = backend.line_height().max(1.0);
                let origin = self.content_origin(cw, lh);
                match self.panel.hit_test(position.x, position.y, origin, cw, lh) {
                    FindReplaceHit::Target(FindReplaceClickTarget::ToggleCase) => {
                        self.panel.case_sensitive = !self.panel.case_sensitive;
                    }
                    FindReplaceHit::Target(FindReplaceClickTarget::ToggleWholeWord) => {
                        self.panel.whole_word = !self.panel.whole_word;
                    }
                    FindReplaceHit::Target(FindReplaceClickTarget::ToggleRegex) => {
                        self.panel.use_regex = !self.panel.use_regex;
                    }
                    FindReplaceHit::Target(FindReplaceClickTarget::TogglePreserveCase) => {
                        self.panel.preserve_case = !self.panel.preserve_case;
                    }
                    _ => {}
                }
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
