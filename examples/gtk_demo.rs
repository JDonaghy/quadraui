//! `cargo run --example gtk_demo --features gtk-example`
//!
//! Same demo as `tui_demo.rs`, rendered with `gtk4` + Cairo + Pango.
//! Demonstrates the GTK-flavoured backend patterns from `BACKEND.md`:
//!
//! - **`TabBar` contract with pixel measurements**: each tab's full
//!   visual width is the Pango pixel width of its label PLUS the
//!   per-tab padding the backend adds (left pad, close button, right
//!   pad, gap). Pre-measure into a `Vec<f64>`, pass to
//!   [`TabBar::fit_active_scroll_offset`] with a closure that returns
//!   `tab_widths[i] as usize`. The unit is pixels — the same unit
//!   `available_width` (the bar's pixel width minus reserved buttons)
//!   uses.
//! - **`StatusBar` contract with pixel measurements**: Pango pixel
//!   widths for each segment, 16-px minimum gap (vs 2-cell gap in
//!   the TUI version), `fit_right_start` returns the index where
//!   visible right segments start.
//! - **Two-pass paint inline within `set_draw_func`**: pass 1 paints
//!   with the current state, post-paint applies the corrected
//!   `tab_scroll_offset`, pass 2 overdraws the same Cairo context.
//!   No `idle_add` — the deferred-callback approach is unreliable
//!   during continuous resize-drag (see `BACKEND.md` §3).
//! - **State outside the primitive**: per-frame interaction state
//!   (focused status segment, hovered tab, drag) lives beside the
//!   primitive — passed as parameters to draw functions, not stored
//!   on the primitive struct.
//!
//! Controls:
//! - `←` / `→`         — switch tab
//! - `n`               — open a new tab
//! - `x`               — close the active tab
//! - `Tab` / `Shift-Tab` — focus next/previous status segment
//! - `Return`          — activate the focused status segment
//! - `q`               — quit
//!
//! Resize the window narrow + wide while many tabs are open. Active
//! tab stays visible (TabBar contract); right status segments drop
//! from the front when the bar gets narrow (StatusBar contract).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::cairo::Context as CairoContext;
use gtk4::gdk;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, DrawingArea, EventControllerKey};

use quadraui::{Color, StatusBar, StatusBarSegment, TabBar};

// Shared backend-agnostic app code (AppState, build_tab_bar,
// build_status_bar, focused_segment_id, handle_status_action).
// Compare this file's body to `tui_demo.rs` — what's left in each
// IS the backend code. Anything common lives in
// `examples/common/mod.rs`.
#[path = "common/mod.rs"]
mod common;
use common::{build_status_bar, build_tab_bar, focused_segment_id, handle_status_action, AppState};

// ─── Backend (primitive → Cairo + Pango) ─────────────────────────────────────
//
// Per-tab pixel breakdown (compare with the contract in `TabBar`'s rustdoc):
//
//     +───────────────────────────────────────+   ← 1 px outer gap
//     | TAB_PAD | label pixels | TAB_PAD | × |
//     +───────────────────────────────────────+
//          ^14         ^pango       ^14    ^close_w
//
// The tab's "full visual width" is everything between the outer-gap
// borders. That's what `fit_active_scroll_offset` needs.

const TAB_PAD: f64 = 14.0;
const TAB_INNER_GAP: f64 = 0.0; // demo simplification — vimcode adds 10px between label and close
const TAB_OUTER_GAP: f64 = 1.0;
const TAB_ROW_HEIGHT: f64 = 28.0;
const STATUS_ROW_HEIGHT: f64 = 22.0;
const STATUS_MIN_GAP_PX: usize = 16;

fn set_color(cr: &CairoContext, c: Color) {
    cr.set_source_rgb(c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0);
}

/// Pango-measure the pixel width of `text` using `layout`. Layout is
/// pre-configured with the UI font.
fn measure_px(layout: &pango::Layout, text: &str) -> f64 {
    layout.set_text(text);
    layout.pixel_size().0 as f64
}

/// Per-tab full slot width in pixels (label + padding + close button +
/// outer gap). Used both for the paint loop and for the
/// `fit_active_scroll_offset` measurer.
fn measure_tab_slot(layout: &pango::Layout, label: &str, close_w: f64) -> f64 {
    let label_w = measure_px(layout, label);
    TAB_PAD + label_w + TAB_INNER_GAP + close_w + TAB_PAD + TAB_OUTER_GAP
}

/// Draw a [`TabBar`] honouring the contract. Returns the
/// `correct_scroll_offset` the caller should write back to app state.
fn draw_tab_bar(
    cr: &CairoContext,
    layout: &pango::Layout,
    bar_x: f64,
    bar_y: f64,
    bar_w: f64,
    bar: &TabBar,
) -> usize {
    // Background fill.
    set_color(cr, Color::rgb(20, 20, 30));
    cr.rectangle(bar_x, bar_y, bar_w, TAB_ROW_HEIGHT);
    let _ = cr.fill();

    // Reserve the right-segment area first. For the demo we render the
    // right segments at a fixed 28px each (matching TAB_ROW_HEIGHT for
    // square buttons). A real backend would Pango-measure each segment.
    let right_w: f64 = bar.right_segments.iter().map(|_| 28.0).sum();
    let tab_area_w = (bar_w - right_w).max(0.0);

    // Pre-measure each tab's full slot width in pixels — we'll reuse
    // these for both `fit_active_scroll_offset` and the paint loop, so
    // we don't double-pay Pango.
    let close_w = measure_px(layout, "×");
    let tab_widths: Vec<f64> = bar
        .tabs
        .iter()
        .map(|t| measure_tab_slot(layout, &t.label, close_w))
        .collect();

    // Compute the correct scroll offset for the *current* width using
    // pixel-based measurements. This is the TabBar contract step 2.
    let active_idx = bar.tabs.iter().position(|t| t.is_active);
    let correct_offset = if let Some(active) = active_idx {
        TabBar::fit_active_scroll_offset(active, bar.tabs.len(), tab_area_w as usize, |i| {
            tab_widths[i] as usize
        })
    } else {
        bar.scroll_offset
    };

    // Paint visible tabs starting from the primitive's *input*
    // scroll_offset (the corrected value applies to the next paint).
    let mut x = bar_x;
    let active_accent = bar.active_accent;
    for (i, tab) in bar.tabs.iter().enumerate().skip(bar.scroll_offset) {
        let slot_w = tab_widths[i];
        if x + slot_w > bar_x + tab_area_w {
            break;
        }

        // Tab background.
        if tab.is_active {
            set_color(cr, Color::rgb(50, 80, 130));
        } else {
            set_color(cr, Color::rgb(20, 20, 30));
        }
        cr.rectangle(x, bar_y, slot_w - TAB_OUTER_GAP, TAB_ROW_HEIGHT);
        let _ = cr.fill();

        // Active accent — 2px bar at the top.
        if tab.is_active {
            if let Some(accent) = active_accent {
                set_color(cr, accent);
                cr.rectangle(x, bar_y, slot_w - TAB_OUTER_GAP, 2.0);
                let _ = cr.fill();
            }
        }

        // Label text.
        layout.set_text(&tab.label);
        let (label_w, label_h) = layout.pixel_size();
        let text_y = bar_y + (TAB_ROW_HEIGHT - label_h as f64) / 2.0;
        if tab.is_active {
            set_color(cr, Color::rgb(255, 255, 255));
        } else {
            set_color(cr, Color::rgb(160, 160, 180));
        }
        cr.move_to(x + TAB_PAD, text_y);
        pangocairo::functions::show_layout(cr, layout);

        // Close button.
        layout.set_text("×");
        let close_x = x + TAB_PAD + label_w as f64 + TAB_INNER_GAP;
        cr.move_to(close_x, text_y);
        pangocairo::functions::show_layout(cr, layout);

        x += slot_w;
    }

    // Right segments (just the "+" new-tab button in this demo).
    let mut rx = bar_x + tab_area_w;
    for seg in &bar.right_segments {
        set_color(cr, Color::rgb(60, 60, 80));
        cr.rectangle(rx, bar_y, 28.0, TAB_ROW_HEIGHT);
        let _ = cr.fill();
        layout.set_text(&seg.text);
        let (sw, sh) = layout.pixel_size();
        set_color(cr, Color::rgb(200, 200, 200));
        cr.move_to(
            rx + (28.0 - sw as f64) / 2.0,
            bar_y + (TAB_ROW_HEIGHT - sh as f64) / 2.0,
        );
        pangocairo::functions::show_layout(cr, layout);
        rx += 28.0;
    }

    correct_offset
}

/// Draw a [`StatusBar`] honouring the narrow-width contract. Drops
/// low-priority right segments via `fit_right_start` with a 16-px gap.
fn draw_status_bar(
    cr: &CairoContext,
    layout: &pango::Layout,
    bar_x: f64,
    bar_y: f64,
    bar_w: f64,
    bar: &StatusBar,
) {
    // Background fill (use first segment's bg).
    let fill = bar
        .left_segments
        .first()
        .map(|s| s.bg)
        .unwrap_or(Color::rgb(40, 40, 60));
    set_color(cr, fill);
    cr.rectangle(bar_x, bar_y, bar_w, STATUS_ROW_HEIGHT);
    let _ = cr.fill();

    // Helper: render a single segment at (x, y), return its pixel width.
    let render_segment =
        |cr: &CairoContext, layout: &pango::Layout, x: f64, seg: &StatusBarSegment| -> f64 {
            layout.set_text(&seg.text);
            let attrs = pango::AttrList::new();
            if seg.bold {
                attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
            }
            layout.set_attributes(Some(&attrs));
            let (w, h) = layout.pixel_size();
            let w_f = w as f64;
            // Per-segment background fill (in case it differs from bar bg).
            set_color(cr, seg.bg);
            cr.rectangle(x, bar_y, w_f, STATUS_ROW_HEIGHT);
            let _ = cr.fill();
            // Text.
            set_color(cr, seg.fg);
            cr.move_to(x, bar_y + (STATUS_ROW_HEIGHT - h as f64) / 2.0);
            pangocairo::functions::show_layout(cr, layout);
            layout.set_attributes(None);
            w_f
        };

    // Left segments: accumulate from bar_x.
    let mut cx = bar_x;
    for seg in &bar.left_segments {
        cx += render_segment(cr, layout, cx, seg);
    }

    // Right segments: drop from the front via fit_right_start with a
    // pixel-unit measurer. This is the StatusBar contract.
    let measure_seg = |seg: &StatusBarSegment| -> usize {
        layout.set_text(&seg.text);
        layout.pixel_size().0.max(0) as usize
    };
    let start = bar.fit_right_start(bar_w as usize, STATUS_MIN_GAP_PX, measure_seg);
    let visible = &bar.right_segments[start..];

    // Sum widths of visible right segments to find the right-aligned start x.
    let visible_widths: Vec<f64> = visible
        .iter()
        .map(|s| {
            layout.set_text(&s.text);
            layout.pixel_size().0 as f64
        })
        .collect();
    let total_right: f64 = visible_widths.iter().sum();
    let mut rx = (bar_x + bar_w - total_right).max(cx);
    for seg in visible {
        rx += render_segment(cr, layout, rx, seg);
    }
}

// ─── Main: GTK setup + event loop ────────────────────────────────────────────

fn main() -> glib::ExitCode {
    let app = Application::builder()
        .application_id("io.quadraui.demo")
        .build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let state = Rc::new(RefCell::new(AppState::new()));
    let drawing_area = DrawingArea::new();
    drawing_area.set_can_focus(true);
    drawing_area.set_focusable(true);

    // Pango layout setup. Reused across frames via interior mutability.
    let pango_ctx = drawing_area.create_pango_context();
    let layout_for_draw = pango::Layout::new(&pango_ctx);
    layout_for_draw.set_font_description(Some(&pango::FontDescription::from_string("Sans 11")));
    let layout_cell: Rc<RefCell<pango::Layout>> = Rc::new(RefCell::new(layout_for_draw));

    // Stash whether pass 2 was triggered last frame, so we can show it
    // in the demo's hint row (purely cosmetic — not part of the contract).
    let last_repaint_corrected = Rc::new(Cell::new(false));

    // ── set_draw_func: the two-pass paint pattern ─────────────────────────
    {
        let state = state.clone();
        let layout_cell = layout_cell.clone();
        let last_repaint_corrected = last_repaint_corrected.clone();
        drawing_area.set_draw_func(move |_da, cr, w, h| {
            let layout = layout_cell.borrow();
            let w_f = w as f64;
            let h_f = h as f64;

            // Background.
            set_color(cr, Color::rgb(30, 30, 40));
            cr.rectangle(0.0, 0.0, w_f, h_f);
            let _ = cr.fill();

            // Layout: tab bar at top, status bar at bottom, hint row above status.
            let tab_y = 0.0;
            let status_y = h_f - STATUS_ROW_HEIGHT;
            let hint_y = status_y - STATUS_ROW_HEIGHT;
            let body_y = tab_y + TAB_ROW_HEIGHT;
            let body_h = (hint_y - body_y).max(0.0);

            // ── Pass 1: paint with current state, capture the correct offset.
            let correct_offset = {
                let s = state.borrow();
                let bar = build_tab_bar(&s);
                draw_tab_bar(cr, &layout, 0.0, tab_y, w_f, &bar)
            };

            // ── Apply correction to app state.
            let changed = {
                let mut s = state.borrow_mut();
                if s.tab_scroll_offset != correct_offset {
                    s.tab_scroll_offset = correct_offset;
                    true
                } else {
                    false
                }
            };

            // ── Pass 2: if state changed, repaint inline. Overdraws pass 1
            // in the same Cairo context. NO idle_add — see BACKEND.md §3.
            if changed {
                let s = state.borrow();
                let bar = build_tab_bar(&s);
                draw_tab_bar(cr, &layout, 0.0, tab_y, w_f, &bar);
            }
            last_repaint_corrected.set(changed);

            // Body content.
            {
                let s = state.borrow();
                let body_msg = format!(
                    "Tab {} of {} — \"{}\"",
                    s.active_tab + 1,
                    s.tabs.len(),
                    s.tabs.get(s.active_tab).cloned().unwrap_or_default()
                );
                layout.set_text(&body_msg);
                let (_, lh) = layout.pixel_size();
                set_color(cr, Color::rgb(220, 220, 220));
                cr.move_to(20.0, body_y + (body_h - lh as f64) / 2.0);
                pangocairo::functions::show_layout(cr, &layout);
            }

            // Hint row.
            {
                let hint = if last_repaint_corrected.get() {
                    " ←/→ tab • n new • x close • Tab cycle status • Enter activate • q quit  [pass-2 fired]"
                } else {
                    " ←/→ tab • n new • x close • Tab cycle status • Enter activate • q quit"
                };
                set_color(cr, Color::rgb(40, 40, 50));
                cr.rectangle(0.0, hint_y, w_f, STATUS_ROW_HEIGHT);
                let _ = cr.fill();
                layout.set_text(hint);
                let (_, lh) = layout.pixel_size();
                set_color(cr, Color::rgb(140, 140, 160));
                cr.move_to(8.0, hint_y + (STATUS_ROW_HEIGHT - lh as f64) / 2.0);
                pangocairo::functions::show_layout(cr, &layout);
            }

            // Status bar.
            {
                let s = state.borrow();
                let focused = focused_segment_id(&s);
                let bar = build_status_bar(&s, focused.as_deref());
                draw_status_bar(cr, &layout, 0.0, status_y, w_f, &bar);
            }
        });
    }

    // ── Key controller ─────────────────────────────────────────────────────
    let key = EventControllerKey::new();
    {
        let state = state.clone();
        let da = drawing_area.clone();
        key.connect_key_pressed(move |_ctrl, key, _code, _modifier| {
            let mut s = state.borrow_mut();
            let mut handled = true;
            match key {
                gdk::Key::Left => s.prev_tab(),
                gdk::Key::Right => s.next_tab(),
                gdk::Key::n => s.open_tab(),
                gdk::Key::x => s.close_active(),
                gdk::Key::Tab => s.cycle_status_focus(1),
                gdk::Key::ISO_Left_Tab => s.cycle_status_focus(-1),
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    let id = focused_segment_id(&s);
                    if let Some(id) = id {
                        handle_status_action(&mut s, &id);
                    }
                }
                gdk::Key::q | gdk::Key::Escape => {
                    if let Some(window) = da.root().and_downcast::<gtk4::Window>() {
                        window.close();
                    }
                }
                _ => handled = false,
            }
            drop(s);
            if handled {
                da.queue_draw();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }

    let window = ApplicationWindow::builder()
        .application(app)
        .title("quadraui — GTK demo")
        .default_width(800)
        .default_height(220)
        .child(&drawing_area)
        .build();
    window.add_controller(key);
    drawing_area.grab_focus();
    window.present();
}
