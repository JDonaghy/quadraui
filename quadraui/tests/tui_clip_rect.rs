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
//! These tests drive the real `Backend` trait methods directly against
//! a `ratatui::Terminal<TestBackend>`, the same low-level seam a future
//! `ScrollContainer` rasteriser would call, rather than through
//! `TuiDriver`'s `AppLogic` event loop — no primitive wires this seam
//! up yet (that is the follow-up issue), so there is no app-level event
//! path to drive here.

use quadraui::tui::TuiBackend;
use quadraui::{Backend, Color, Rect as QRect};
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
