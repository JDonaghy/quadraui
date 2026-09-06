//! Direct2D / DirectWrite layout helper for
//! [`crate::primitives::chart::Chart`] (issue #26).
//!
//! Painting moved to the shared [`crate::primitives::chart::paint`]
//! (#810, NativeSurface Phase 2c) — see that fn's doc for the
//! divergences resolved while unifying `gtk::chart::draw_chart`,
//! `macos::chart::draw_chart` and `win::chart::draw_chart` into one
//! implementation, including the quadraui#791 clip this backend never
//! had before. This module now only carries [`win_chart_layout`], the
//! DIP-unit layout query used for both hit-testing
//! (`WinBackend::chart_layout`) and painting (`WinBackend::draw_chart`).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod chart;` and `backend.rs`'s module
//! docs.

use crate::event::Rect;
use crate::primitives::chart::{Chart, ChartLayout, ChartMeasure};

/// Compute a [`Chart`]'s layout without painting — the DirectWrite twin
/// of `gtk_chart_layout`/`mac_chart_layout`.
pub fn win_chart_layout(
    chart: &Chart,
    rect: Rect,
    char_width: f32,
    line_height: f32,
) -> ChartLayout {
    chart.layout(
        rect.x,
        rect.y,
        ChartMeasure {
            width: rect.width,
            height: rect.height,
            char_width,
            line_height,
        },
    )
}
