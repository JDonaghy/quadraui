#![cfg(feature = "tui")]
//! `Backend::push_clip`/`pop_clip` on `TuiBackend` — the backend-neutral
//! clip-rect seam a compositing container (a scroll view clipping a
//! partly-visible child) reaches for before painting past its own
//! viewport.
//!
//! TUI has no per-cell scissor to filter paints against, so
//! `TuiBackend` instead snapshots the region a push narrows away from
//! and restores whatever part of it still sits outside the clip once
//! the matching `pop_clip` runs — see `tui::canvas::TuiClipFrame`'s doc.
//! The first two tests drive the real `Backend` trait methods directly
//! against a `ratatui::Terminal<TestBackend>`, the same low-level seam a
//! future `ScrollContainer` rasteriser would call, rather than through
//! `TuiDriver`'s `AppLogic` event loop — no primitive wires this seam up
//! yet (that is the follow-up issue), so there is no app-level event
//! path for most of it. The last test *does* go through `TuiDriver`: it
//! calls `push_clip`/`draw_solid_fill`/`pop_clip` from inside an
//! `AppLogic::render`, which is enough to exercise the frame-boundary
//! invariant (`TuiBackend::begin_frame` clearing `clip_stack`) through
//! the real `event -> handle -> render` path.

use quadraui::tui::testing::TuiDriver;
use quadraui::tui::TuiBackend;
use quadraui::{AppLogic, Backend, Color, Reaction, Rect as QRect, UiEvent};
use ratatui::backend::TestBackend;
use ratatui::style::Color as RColor;
use ratatui::Terminal;

fn blank_terminal(w: u16, h: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(w, h)).expect("TestBackend terminal")
}

#[test]
fn push_clip_confines_an_overflowing_fill_to_the_clipped_cells() {
    let mut backend = TuiBackend::new();
    let mut terminal = blank_terminal(10, 10);
    let red = Color::rgb(200, 0, 0);

    terminal
        .draw(|frame| {
            backend.enter_frame_scope(frame, |b| {
                b.push_clip(QRect::new(0.0, 0.0, 2.0, 2.0));
                b.draw_solid_fill(QRect::new(0.0, 0.0, 8.0, 8.0), red);
                b.pop_clip();
            });
        })
        .expect("draw");

    let buf = terminal.backend().buffer();
    let expected = ratatui::style::Color::Rgb(200, 0, 0);
    assert_eq!(
        buf[(0, 0)].bg,
        expected,
        "a cell inside the pushed clip must keep the fill"
    );
    assert_eq!(
        buf[(1, 1)].bg,
        expected,
        "the pushed clip's far corner (still inside its 2x2 cells) must keep the fill"
    );
    assert_eq!(
        buf[(5, 5)].bg,
        RColor::Reset,
        "the fill reaches cell (5, 5), but the popped 2x2-cell clip must have rolled it back"
    );
    assert_eq!(
        buf[(2, 2)].bg,
        RColor::Reset,
        "cell (2, 2) sits one cell past the clip's far edge and must not have been painted"
    );
}

#[test]
fn nested_pop_restores_the_outer_clips_own_paint_not_the_pre_push_blank() {
    let mut backend = TuiBackend::new();
    let mut terminal = blank_terminal(10, 10);
    let blue = Color::rgb(0, 0, 200);
    let red = Color::rgb(200, 0, 0);

    terminal
        .draw(|frame| {
            backend.enter_frame_scope(frame, |b| {
                // Outer clip: cells (0,0)..(6,6).
                b.push_clip(QRect::new(0.0, 0.0, 6.0, 6.0));
                b.draw_solid_fill(QRect::new(0.0, 0.0, 10.0, 10.0), blue);
                // Inner clip, nested inside the outer one: cells (1,1)..(3,3).
                b.push_clip(QRect::new(1.0, 1.0, 2.0, 2.0));
                b.draw_solid_fill(QRect::new(0.0, 0.0, 10.0, 10.0), red);
                // Popping the inner clip must roll its donut back to the
                // *outer* clip's blue, not all the way to blank.
                b.pop_clip();
                b.pop_clip();
            });
        })
        .expect("draw");

    let buf = terminal.backend().buffer();
    let blue_rgb = ratatui::style::Color::Rgb(0, 0, 200);
    let red_rgb = ratatui::style::Color::Rgb(200, 0, 0);

    assert_eq!(
        buf[(1, 1)].bg,
        red_rgb,
        "inside the inner clip, the red fill survives both pops"
    );
    assert_eq!(
        buf[(4, 4)].bg,
        blue_rgb,
        "inside the outer clip but outside the inner one, the inner pop must restore blue \
         (the outer clip's own paint), not discard it"
    );
    assert_eq!(
        buf[(8, 8)].bg,
        RColor::Reset,
        "outside the outer clip entirely, the outer pop must roll the red fill all the way \
         back to blank"
    );
}

/// `AppLogic` whose `render` pushes an unbalanced clip on its first call
/// (no matching `pop_clip`) and, on every later call, pushes a clip
/// sized to the *whole* buffer and pops it properly. Driven through
/// `TuiDriver`, not a raw `Terminal`, so `begin_frame`'s `clip_stack.clear()`
/// runs via the real frame-paint path.
struct ClipLeakApp {
    frame: std::cell::Cell<u32>,
}

impl AppLogic for ClipLeakApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let frame = self.frame.get();
        self.frame.set(frame + 1);
        if frame == 0 {
            backend.push_clip(QRect::new(0.0, 0.0, 2.0, 2.0));
            backend.draw_solid_fill(QRect::new(0.0, 0.0, 10.0, 10.0), Color::rgb(200, 0, 0));
            // Deliberately no `pop_clip()` — the bug this test guards against.
        } else {
            // If `begin_frame` had not cleared frame 0's leaked entry,
            // this push would intersect against its stale 2x2 clip
            // instead of the full 10x10 buffer, narrowing this fill to
            // 2x2 cells instead of the 10x10 this frame actually asks for.
            backend.push_clip(QRect::new(0.0, 0.0, 10.0, 10.0));
            backend.draw_solid_fill(QRect::new(0.0, 0.0, 10.0, 10.0), Color::rgb(0, 0, 200));
            backend.pop_clip();
        }
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

#[test]
fn begin_frame_clears_an_unbalanced_push_before_the_next_frame() {
    // `TuiDriver::new` paints frame 0 (the unbalanced push + red fill).
    let mut driver = TuiDriver::new(
        ClipLeakApp {
            frame: std::cell::Cell::new(0),
        },
        10,
        10,
    );
    // Frame 1: a fresh push against what must be a clean `clip_stack`.
    driver.render();

    let blue = RColor::Rgb(0, 0, 200);
    assert_eq!(
        driver.style_at(5, 5).expect("in-bounds").bg,
        blue,
        "frame 1's full-buffer push must not be narrowed by frame 0's leaked, \
         unbalanced push — begin_frame must clear clip_stack between frames"
    );
    assert_eq!(
        driver.style_at(9, 9).expect("in-bounds").bg,
        blue,
        "frame 1's fill reaches every corner of the 10x10 buffer, unclipped by \
         any stale frame-0 state"
    );
}
