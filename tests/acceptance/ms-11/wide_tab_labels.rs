// Sealed acceptance slice — ms-11 / issue #554 ("TUI tab bar measures and
// paints labels in chars, not columns").
//
// Authored independently from the implementation, against
// `tests/acceptance/ms-11/contract.md` §3 (and §2 on assertion polarity).
// `include!`d from `quadraui/tests/acceptance.rs` below its SEALED marker.
//
// ── Assertion polarity (contract §2) — read before editing ────────────────
//
// `screen_contains` is INVERTED for this issue: it is `true` on the broken
// tree and `false` on the fixed one, because `screen()` concatenates every
// stored cell while a real terminal (and `TuiDriver::row_cells`) strides two
// columns past a double-width glyph. Every load-bearing assertion below
// therefore goes through `find_bounds` / `click_text`, never
// `screen_contains` and never a golden-grid diff of
// `mocks/tabbar-wide-labels.screen` (that mock is illustrative — contract §8).
//
// ── Fixture (contract §3b, §6) ────────────────────────────────────────────
//
// Defined inline: `common::tab_group_demo::TabGroupDemo`'s `group` field is
// private and its constructor hardcodes ASCII labels, so tab labels cannot be
// set through it. Only quadraui's public API is used here.

#[cfg(feature = "tui")]
mod ms11_554_wide_tab_labels {
    use std::cell::RefCell;

    use quadraui::testing::ConformanceDriver;
    use quadraui::tui::testing::TuiDriver;
    use quadraui::{AppLogic, Backend, Color, Reaction, Rect, TabBar, TabItem, UiEvent, WidgetId};

    /// Tab 0 — 11 `char`s, **14 display columns** (contract §3b).
    const WIDE_LABEL: &str = " 1: 日本語.rs ";
    /// Tab 1 — ASCII control, 12 chars / 12 columns.
    const ASCII_LABEL: &str = " 2: main.rs ";
    /// Contract §3h — secondary, never asserted with a pinned width.
    const EMOJI_LABEL: &str = " 3: 🚀ship.rs ";

    /// Sub-span under test in the wide label: 3 wide (6 cols) + 3 narrow
    /// (3 cols) = **9 columns** once #554 is fixed.
    const WIDE_NEEDLE: &str = "日本語.rs";
    const WIDE_NEEDLE_COLS: f32 = 9.0;
    /// Sub-span under test in the ASCII control: **7 columns**, today and
    /// after the fix alike.
    const ASCII_NEEDLE: &str = "main.rs";
    const ASCII_NEEDLE_COLS: f32 = 7.0;
    const EMOJI_NEEDLE: &str = "🚀ship.rs";

    // Contract §3b fixes the viewport at 80×6.
    const COLS: u16 = 80;
    const ROWS: u16 = 6;

    /// A minimal `AppLogic` that draws one `TabBar` and records, from the
    /// `TabBarHits` the backend hands back, which tab a click landed on.
    ///
    /// This mirrors how a real consumer (vimcode, coord-tui) wires the bar
    /// up: the *painted* columns are located by the driver, and the click is
    /// resolved through the backend's own *measured* slot positions. A tree
    /// where measure and paint disagree therefore fails §3g, which is the
    /// whole downstream point of this issue.
    struct WideTabFixture {
        labels: Vec<String>,
        active: usize,
        /// `(start_x, end_x)` per tab, as reported by the last
        /// `draw_tab_bar`. `render` takes `&self`, hence the `RefCell`.
        slots: RefCell<Vec<(f64, f64)>>,
    }

    impl WideTabFixture {
        fn new(labels: &[&str], active: usize) -> Self {
            Self {
                labels: labels.iter().map(|s| (*s).to_string()).collect(),
                active,
                slots: RefCell::new(Vec::new()),
            }
        }

        /// The recorded active-tab id (contract §3g observes this).
        fn active_label(&self) -> &str {
            &self.labels[self.active]
        }

        fn bar(&self) -> TabBar {
            TabBar {
                id: WidgetId::new("ms11:554:tabs"),
                tabs: self
                    .labels
                    .iter()
                    .enumerate()
                    .map(|(i, label)| TabItem {
                        label: label.clone(),
                        is_active: i == self.active,
                        is_dirty: false,
                        is_preview: false,
                        is_closable: true,
                    })
                    .collect(),
                scroll_offset: 0,
                right_segments: vec![],
                active_accent: Some(Color::rgb(80, 160, 240)),
                // Mirrors `mocks/tabbar-wide-labels.screen`, which paints a
                // `×` per tab. Every width assertion below is on a *label
                // sub-span*, never on a whole tab cell, so close-button
                // presence cannot move any asserted number.
                show_tab_close: true,
                compact: false,
            }
        }
    }

    impl AppLogic for WideTabFixture {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let bar = self.bar();
            let hits = backend.draw_tab_bar(Rect::new(0.0, 0.0, COLS as f32, 1.0), &bar, None);
            *self.slots.borrow_mut() = hits.slot_positions;
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            if let UiEvent::MouseDown { position, .. } = event {
                let x = position.x as f64;
                let hit = self
                    .slots
                    .borrow()
                    .iter()
                    // Tabs before `scroll_offset` carry zero-width
                    // `(0.0, 0.0)` sentinels; `end > start` skips them.
                    .position(|&(start, end)| end > start && x >= start && x < end);
                if let Some(idx) = hit {
                    if idx != self.active {
                        self.active = idx;
                        return Reaction::Redraw;
                    }
                }
            }
            Reaction::Continue
        }
    }

    /// The two-tab fixture of contract §3b: wide tab active, ASCII control
    /// second — the render `mocks/tabbar-wide-labels.screen` illustrates.
    fn wide_then_ascii() -> TuiDriver<WideTabFixture> {
        TuiDriver::new(WideTabFixture::new(&[WIDE_LABEL, ASCII_LABEL], 0), COLS, ROWS)
    }

    // ── §3c — the load-bearing clause ─────────────────────────────────────
    //
    // Red today: the painter writes each char into ONE cell, so `row_cells`
    // reconstructs the row as `日 語 r s` — `本` and `.` are stepped over and
    // the needle matches no window at all.

    #[test]
    fn wide_label_paints_every_glyph_in_its_own_columns() {
        let driver = wide_then_ascii();

        let bounds = driver.find_bounds(WIDE_NEEDLE).unwrap_or_else(|| {
            panic!(
                "contract §3c: find_bounds({WIDE_NEEDLE:?}) returned None — a glyph of the \
                 wide label was dropped (the row reads as if `本` and `.` were never \
                 painted). Screen:\n{}",
                driver.screen()
            )
        });

        assert_eq!(
            bounds.width, WIDE_NEEDLE_COLS,
            "contract §3c: {WIDE_NEEDLE:?} must occupy {WIDE_NEEDLE_COLS} display columns \
             (3 wide + 3 narrow), got {}. Screen:\n{}",
            bounds.width,
            driver.screen()
        );
    }

    // ── §3d — ASCII regression half (green today, must stay green) ────────

    #[test]
    fn ascii_label_is_unchanged() {
        let driver = wide_then_ascii();

        let bounds = driver.find_bounds(ASCII_NEEDLE).unwrap_or_else(|| {
            panic!(
                "contract §3d: find_bounds({ASCII_NEEDLE:?}) returned None — the ASCII \
                 control tab must be unaffected by #554. Screen:\n{}",
                driver.screen()
            )
        });

        assert_eq!(
            bounds.width, ASCII_NEEDLE_COLS,
            "contract §3d: {ASCII_NEEDLE:?} must occupy {ASCII_NEEDLE_COLS} columns, got {}. \
             Screen:\n{}",
            bounds.width,
            driver.screen()
        );
    }

    // ── §3e — measured budget agrees with painted width ───────────────────
    //
    // Relational, never a hardcoded column (epic #480 pillar 3). Once tab 0
    // is measured at its true 14 columns, tab 1's label must begin beyond
    // the end of the wide sub-span.

    #[test]
    fn measured_tab_budget_matches_the_painted_width() {
        let driver = wide_then_ascii();

        let wide = driver.find_bounds(WIDE_NEEDLE).unwrap_or_else(|| {
            panic!(
                "contract §3e precondition (§3c): {WIDE_NEEDLE:?} is not painted as a \
                 contiguous run. Screen:\n{}",
                driver.screen()
            )
        });
        let ascii = driver.find_bounds(ASCII_NEEDLE).unwrap_or_else(|| {
            panic!(
                "contract §3e precondition: {ASCII_NEEDLE:?} is not painted. Screen:\n{}",
                driver.screen()
            )
        });

        assert!(
            ascii.x > wide.x + WIDE_NEEDLE_COLS,
            "contract §3e: the ASCII tab's label starts at column {} but the wide tab's \
             label ends at column {} — the tab was measured narrower than it paints, so \
             the second tab overlaps the first. Screen:\n{}",
            ascii.x,
            wide.x + WIDE_NEEDLE_COLS,
            driver.screen()
        );
    }

    // ── §3g — hit regions agree with what was painted ─────────────────────
    //
    // The downstream contract: vimcode's `tab_hit_width` is deliberately
    // pinned on today's `.chars().count()` behaviour and is the single
    // vimcode-side edit once this lands; coord-tui shares this tab bar and
    // should be checked for the same assumption.
    //
    // Seeded with tab 1 active (contract §3g) so the click is a real state
    // change and not a no-op.

    #[test]
    fn click_on_wide_label_activates_the_painted_tab() {
        let mut driver = TuiDriver::new(
            WideTabFixture::new(&[WIDE_LABEL, ASCII_LABEL], 1),
            COLS,
            ROWS,
        );

        assert_eq!(
            driver.app().active_label(),
            ASCII_LABEL,
            "fixture precondition: tab 1 starts active so the click is a real state change"
        );
        assert!(
            driver.find_bounds(WIDE_NEEDLE).is_some(),
            "contract §3g precondition (§3c): {WIDE_NEEDLE:?} is not painted as a \
             contiguous run, so there is nothing to click. Screen:\n{}",
            driver.screen()
        );

        // `click_text` locates via `find_bounds` and clicks the painted
        // span's centre, so this exercises paint→hit agreement rather than
        // restating either side.
        driver.click_text(WIDE_NEEDLE);

        assert_eq!(
            driver.app().active_label(),
            WIDE_LABEL,
            "contract §3g: clicking the painted centre of {WIDE_NEEDLE:?} must activate \
             tab 0 — a hit box computed from a char-count measure lands on the wrong tab. \
             Screen:\n{}",
            driver.screen()
        );
    }

    // ── §3h — emoji, secondary ────────────────────────────────────────────
    //
    // Presence only, never a pinned width: emoji width depends on the exact
    // `char_cell_width` table, which #545 has already had to correct once
    // for PUA ranges. A width disagreement here is a #545-family issue, not
    // a #554 regression — file it separately rather than widening this slice.

    #[test]
    fn emoji_label_paints_every_glyph() {
        let driver = TuiDriver::new(
            WideTabFixture::new(&[WIDE_LABEL, ASCII_LABEL, EMOJI_LABEL], 0),
            COLS,
            ROWS,
        );

        assert!(
            driver.find_bounds(EMOJI_NEEDLE).is_some(),
            "contract §3h: find_bounds({EMOJI_NEEDLE:?}) returned None — a glyph of the \
             emoji label was dropped. Screen:\n{}",
            driver.screen()
        );
    }

    // TODO(test-author): the contract does not specify whether a tab whose
    // wide label is *clipped* by an overflowing bar must break before or
    // after the double-width glyph (i.e. whether a half-painted wide glyph
    // is ever legal). No clause is asserted for that case; the fixture keeps
    // all tabs comfortably inside the 80-column viewport so the question
    // never arises here.
}
