//! Paint-time text recording for the GTK backend (quadraui#489).
//!
//! # Why a sink instead of per-primitive geometry
//!
//! [`super::testing::GtkDriver::find`] / `screen_contains` are backed by
//! [`super::backend::GtkBackend::painted_text`] — a `(text, bounds)` map
//! that has to be populated at paint time, because GTK has no character
//! grid to scan the way `TuiDriver::screen` does. Before this module only
//! three trait methods (`draw_status_bar`, `draw_data_table`,
//! `draw_pipeline_view`) recorded anything: each *re-derived* its labels'
//! rects from the primitive's returned `*Layout` after the rasteriser had
//! already painted them. That works for a handful of primitives with
//! flat, layout-addressable labels, but it does not scale to all 37
//! rasterisers:
//!
//! * The rasterisers are free functions (`quadraui::gtk::draw_tree` etc.)
//!   with no `&mut GtkBackend` to record into, and they are **`pub`** — a
//!   sink parameter or a changed return type would be a breaking change
//!   for every path-dep consumer (see `CLAUDE.md` → *Downstream
//!   consumers*).
//! * Several (palette, editor, form, diff view) resolve their text
//!   positions with intermediate state that never leaves the paint
//!   function — a per-primitive re-derivation would be a second,
//!   silently-drifting copy of that geometry, exactly the failure
//!   `docs/LESSONS.md` calls "cache at paint, hit-test at click".
//!
//! So instead of re-deriving anything, this module records at the **one
//! choke point every GTK rasteriser already funnels its text through**:
//! `pangocairo`'s `show_layout`. [`show_layout`] is a drop-in replacement
//! that reads the text and extents straight off the `pango::Layout` about
//! to be painted, converts the Cairo current point to device (surface)
//! coordinates, and appends the run to a thread-local sink. No rasteriser
//! signature changes, and the recorded bounds cannot drift from the
//! painted glyphs because they *are* the painted glyphs' bounds.
//!
//! # Lifecycle
//!
//! The sink is `None` (inactive, zero cost) unless
//! [`super::backend::GtkBackend::set_painted_text_recording`] has been
//! enabled — [`super::testing::GtkDriver`] turns it on, production
//! runners leave it off so a live app never allocates a `String` per
//! painted text run. When enabled, `GtkBackend::enter_frame_scope`
//! installs a fresh sink for the duration of the frame's paint pass and
//! drains it into `GtkBackend::painted_text` afterwards. Draining *after*
//! the frame keeps the pre-existing logical records (whole status-bar
//! segments, whole data-table cells — recorded by the trait methods
//! during the frame) ahead of the per-glyph-run records in the map, so
//! `find_bounds` still resolves those primitives to their hit-testable
//! label rect rather than to a vertically-centred glyph run inside it.

use std::cell::RefCell;

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use crate::Rect;

/// One recorded text run: the exact string handed to Pango and its
/// on-surface bounds in device (pixel) coordinates.
pub(crate) type TextRun = (String, Rect);

thread_local! {
    /// Active sink, or `None` when recording is off. Thread-local rather
    /// than backend-owned because the rasterisers are free functions with
    /// no backend handle; GTK painting is single-threaded, and one sink
    /// per thread is exactly right for `cargo test`'s parallel test
    /// threads (each `GtkDriver` lives on its own thread).
    static SINK: RefCell<Option<Vec<TextRun>>> = const { RefCell::new(None) };
}

/// Install a fresh recording sink, returning the previous one so a
/// (theoretically) nested paint scope can restore it. Pair with
/// [`take_sink`].
pub(crate) fn install_sink() -> Option<Vec<TextRun>> {
    SINK.with(|s| s.borrow_mut().replace(Vec::new()))
}

/// Take everything recorded since [`install_sink`] and restore
/// `previous` as the active sink (`None` = recording off again).
pub(crate) fn take_sink(previous: Option<Vec<TextRun>>) -> Vec<TextRun> {
    SINK.with(|s| {
        let mut slot = s.borrow_mut();
        let recorded = slot.take().unwrap_or_default();
        *slot = previous;
        recorded
    })
}

/// Whether a sink is currently installed. Checked before doing any of
/// the (otherwise wasted) measurement work in [`show_layout`].
fn is_active() -> bool {
    SINK.with(|s| s.borrow().is_some())
}

/// Append one run to the active sink. No-op when recording is off.
fn record(text: &str, bounds: Rect) {
    SINK.with(|s| {
        if let Some(sink) = s.borrow_mut().as_mut() {
            sink.push((text.to_string(), bounds));
        }
    });
}

/// Paint `layout` at the Cairo current point, recording the text run
/// into the active sink first.
///
/// Drop-in replacement for `pangocairo::functions::show_layout` — every
/// GTK rasteriser calls this instead, which is what makes
/// `GtkDriver::find` work for *all* text-bearing primitives rather than
/// the three that hand-rolled their own recording (quadraui#489).
///
/// Bounds are converted with `Context::user_to_device`, so a rasteriser
/// painting under an active `cr.translate(...)` / `cr.scale(...)` (e.g.
/// `Backend::draw_activity_bar`, which translates by the bar's origin
/// before delegating) still records absolute surface coordinates — the
/// space `GtkDriver::click` takes.
pub(crate) fn show_layout(cr: &Context, layout: &pango::Layout) {
    if is_active() {
        if let Ok((ux, uy)) = cr.current_point() {
            let text = layout.text();
            // Whitespace-only runs (selection prefixes, alignment pads,
            // blank rows) can never be a useful `find` needle and would
            // bury the real labels in `painted_texts()` output.
            if !text.trim().is_empty() {
                let (w_px, h_px) = layout.pixel_size();
                let (dx, dy) = cr.user_to_device(ux, uy);
                // `user_to_device_distance` only fails on a Cairo context
                // in an error state — one that could not have painted
                // anything anyway. Fall back to the unscaled extents so a
                // run is still recorded (findable) if that ever happens.
                let (dw, dh) = cr
                    .user_to_device_distance(w_px as f64, h_px as f64)
                    .unwrap_or((w_px as f64, h_px as f64));
                record(
                    text.as_str(),
                    Rect::new(dx as f32, dy as f32, dw as f32, dh as f32),
                );
            }
        }
    }
    pcfn::show_layout(cr, layout);
}

#[cfg(test)]
mod tests {
    use crate::backend::Backend;
    use crate::gtk::backend::GtkBackend;
    use crate::gtk::testing::GtkDriver;
    use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
    use crate::runner::{AppLogic, Reaction};
    use crate::types::{Color, WidgetId};
    use crate::{Rect, UiEvent};
    use pangocairo::cairo::{Context, Format, ImageSurface};

    const W: i32 = 200;
    const H: i32 = 60;
    const ROW_H: f32 = 20.0;

    /// Two stacked `StatusBar`s — the primitive that records *both* a
    /// logical label (from `GtkBackend::draw_status_bar`, aligned with
    /// `status_bar_layout`'s hit regions) and a per-glyph-run record
    /// (from this module). Used to pin the ordering contract.
    struct StackedBarsApp;

    impl AppLogic for StackedBarsApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            for (i, label) in ["row zero", "row one"].into_iter().enumerate() {
                backend.draw_status_bar(
                    Rect::new(0.0, i as f32 * ROW_H, W as f32, ROW_H),
                    &StatusBar {
                        id: WidgetId::new(format!("row-{i}")),
                        left_segments: vec![StatusBarSegment {
                            text: label.to_string(),
                            fg: Color::rgb(255, 255, 255),
                            bg: Color::rgb(20, 20, 20),
                            bold: false,
                            action_id: None,
                        }],
                        right_segments: vec![],
                    },
                    None,
                    None,
                );
            }
        }

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    /// Recording is opt-in: a backend nobody enabled it on must not pay a
    /// `String` per painted text run. This is the production runner path
    /// (`gtk::run` never enables it), so a regression here would be a
    /// silent per-frame allocation cost in every live GTK app.
    #[test]
    fn recording_is_off_unless_explicitly_enabled() {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("surface");
        let cr = Context::new(&surface).expect("context");
        let mut backend = GtkBackend::new();

        crate::gtk::run::render_frame(&mut backend, &StackedBarsApp, &cr, W, H);
        let without = backend.painted_text_for_test().len();

        backend.set_painted_text_recording(true);
        crate::gtk::run::render_frame(&mut backend, &StackedBarsApp, &cr, W, H);
        let with = backend.painted_text_for_test().len();

        // Only the two logical `draw_status_bar` records survive with the
        // sink off; enabling it adds the frame's glyph runs on top.
        assert_eq!(
            without, 2,
            "with recording off only the trait method's own records should land"
        );
        assert!(
            with > without,
            "enabling recording should add the frame's painted glyph runs \
             ({with} vs {without})"
        );
    }

    /// Ordering contract: a primitive that records a *logical* label must
    /// keep winning `find_bounds`, because that rect is the one aligned
    /// with the primitive's hit regions. The glyph run inside it is
    /// vertically centred, so resolving to it instead would silently
    /// change the y a `find`-driven click lands on (the quadraui#488
    /// bug's shape).
    #[test]
    fn logical_records_precede_glyph_runs_in_the_map() {
        let driver = GtkDriver::new(StackedBarsApp, W, H);

        let row0 = driver.find_bounds("row zero").expect("row zero painted");
        let row1 = driver.find_bounds("row one").expect("row one painted");

        assert_eq!(
            row0.y, 0.0,
            "row 0 should resolve to `draw_status_bar`'s own record at the bar's y"
        );
        assert_eq!(
            row1.y, ROW_H,
            "row 1 should resolve to its own bar's y, not row 0's"
        );
        // Both rows still contribute a glyph run — the same text appears
        // twice in the map, the logical record first.
        assert_eq!(
            driver
                .painted_texts()
                .iter()
                .filter(|t| t.contains("row zero"))
                .count(),
            2,
            "expected one logical record plus one glyph run: {:?}",
            driver.painted_texts()
        );
    }

    /// Bounds are recorded in device (surface) space, so a rasteriser
    /// painting under an active transform still reports coordinates
    /// `GtkDriver::click` can use verbatim. `Backend::draw_activity_bar`
    /// is the real caller that does this.
    #[test]
    fn translated_paints_record_absolute_coordinates() {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("surface");
        let cr = Context::new(&surface).expect("context");
        let layout = pangocairo::functions::create_layout(&cr);
        layout.set_text("shifted");

        let prev = super::install_sink();
        cr.save().ok();
        cr.translate(30.0, 12.0);
        cr.move_to(5.0, 3.0);
        super::show_layout(&cr, &layout);
        cr.restore().ok();
        let runs = super::take_sink(prev);

        assert_eq!(runs.len(), 1, "one run should have been recorded");
        let (text, bounds) = &runs[0];
        assert_eq!(text, "shifted");
        assert_eq!(
            (bounds.x, bounds.y),
            (35.0, 15.0),
            "translate must be folded into the recorded origin"
        );
        assert!(bounds.width > 0.0 && bounds.height > 0.0);
    }

    /// Whitespace-only runs (alignment pads, blank rows, selection
    /// prefixes) are dropped: they can never be a useful needle and would
    /// bury the real labels in `painted_texts()`.
    #[test]
    fn whitespace_only_runs_are_not_recorded() {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("surface");
        let cr = Context::new(&surface).expect("context");
        let layout = pangocairo::functions::create_layout(&cr);

        let prev = super::install_sink();
        for text in ["", "   ", "real"] {
            layout.set_text(text);
            cr.move_to(0.0, 0.0);
            super::show_layout(&cr, &layout);
        }
        let runs = super::take_sink(prev);

        assert_eq!(runs.len(), 1, "only the non-blank run should record");
        assert_eq!(runs[0].0, "real");
    }

    /// Painting outside an installed sink must not record — and must
    /// still paint.
    #[test]
    fn painting_with_no_sink_installed_is_a_no_op_for_recording() {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("surface");
        let cr = Context::new(&surface).expect("context");
        let layout = pangocairo::functions::create_layout(&cr);
        layout.set_text("unrecorded");
        cr.move_to(0.0, 0.0);
        super::show_layout(&cr, &layout);

        let prev = super::install_sink();
        let runs = super::take_sink(prev);
        assert!(
            runs.is_empty(),
            "a run painted before the sink existed must not leak into it: {runs:?}"
        );
    }
}
