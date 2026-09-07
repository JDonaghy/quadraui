//! Shared `app`/`backend`/`exited` core for headless [`AppLogic`] test
//! drivers (quadraui#814, finishing quadraui#708's "Problem 2" —
//! `testing.rs`'s own doc called `TuiDriver`/`GtkDriver`/`MacDriver` "a
//! 3-way copy" before #708 gave them the shared [`super::DriverInput`] /
//! [`super::PixelClickConformance`] trait defaults; #814 is the second
//! half, for the state those trait defaults are methods *on*).
//!
//! Before this, `TuiDriver`, `GtkDriver`, `MacDriver`, and `WinDriver`
//! each declared their own `app: A`, `backend: B`, `exited: bool` fields
//! and their own one-line `app`/`app_mut`/`backend`/`exited` accessor
//! methods — four copies of the same three fields and the same four
//! methods. `MacDriver` and `WinDriver` additionally duplicated the
//! `EventOutcome -> Reaction` match inside `dispatch` byte-for-byte, and
//! (once #721 gave both backends an identically-shaped `text_runs()`
//! accessor) `painted_texts`/`screen_contains`/`find_bounds`/`find` too —
//! the 2026-09-04 duplication audit that opened #708 measured the
//! gtk↔macos overlap at 26 lines and the mac↔win `text_runs()`-backed
//! query methods plus the `dispatch` match at roughly 80 lines combined.
//!
//! [`DriverCore<B, A>`] is the one implementation every backend driver
//! now wraps instead of duplicating: the `app`/`backend`/`exited` trio,
//! its accessors, and the [`EventOutcome`] bookkeeping every backend's
//! production `dispatch_event` return value goes through
//! ([`Self::apply_outcome`]). [`TextRunSource`] plus the
//! [`DriverCore`]-impl-block gated on it is the second half: the shared
//! `painted_texts`/`screen_contains`/`find_bounds`/`find` bodies for the
//! two backends (`MacBackend`, `WinBackend`) that record paint-time text
//! runs in the same `&[TextRun]` shape.
//!
//! Each concrete driver still owns its own render surface: a `TestBackend`
//! paired with a `Terminal` for TUI, an `ImageSurface` for GTK, a
//! `BitmapSurface` for macOS, a `HeadlessSurface` for Win-GUI. `DriverCore`
//! has no opinion on how a frame gets painted, which is why
//! [`Self::apply_outcome`] takes the render step as a caller-supplied
//! closure rather than owning it. `TuiDriver::dispatch` stays a
//! hand-written loop rather than calling `apply_outcome`: it dispatches
//! every [`crate::UiEvent`] that `TuiBackend::translate_injected` expands
//! one injected event into (a `MouseDown` inside a registered text region
//! can become a drag-start plus a selection event) and combines their
//! `Reaction`s, a shape `apply_outcome`'s one-`EventOutcome`-in,
//! one-`Reaction`-out signature doesn't fit — see that method's own doc.

// Both only pulled in by `apply_outcome` below, which itself is
// cfg-gated to the three backends that call it (`gtk`/`macos`/`win` —
// TUI's own `dispatch` matches `EventOutcome` directly instead and
// returns its own `Reaction`s, see the module doc) — gate these imports
// in lock-step or a `tui`-only build (no pixel backend) flags them
// unused under `-D warnings`.
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
use crate::runner::Reaction;
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
use crate::runtime::EventOutcome;

/// The `app`/`backend`/`exited` trio every backend driver owns, plus the
/// bookkeeping built directly on top of it. See the module doc for what
/// this replaced.
pub(crate) struct DriverCore<B, A> {
    app: A,
    backend: B,
    exited: bool,
}

impl<B, A> DriverCore<B, A> {
    /// Wrap an already-configured `app`/`backend` pair. Each concrete
    /// driver's own `new` still does its backend-specific setup dance
    /// (font install, painted-text recording, seeding the viewport,
    /// calling `app.setup`) before constructing this — `DriverCore`
    /// itself has no opinion on any of that, and starts `exited` at
    /// `false` unconditionally.
    pub(crate) fn new(app: A, backend: B) -> Self {
        Self {
            app,
            backend,
            exited: false,
        }
    }

    /// Access the app state for test assertions.
    pub(crate) fn app(&self) -> &A {
        &self.app
    }

    /// Mutable access to the app state for tests that need to poke state
    /// directly rather than through a scripted [`crate::UiEvent`].
    pub(crate) fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// Access the backend for test assertions (e.g. active selection
    /// state, drag state).
    pub(crate) fn backend(&self) -> &B {
        &self.backend
    }

    /// Mutable access to the backend — needed for `&mut self` trait
    /// methods a test wants to poll directly, and for feeding the
    /// backend into a production `dispatch_event`/`render_frame` call.
    /// `MacDriver` never needs a standalone `&mut B` (its `dispatch`/
    /// `render` both reach for `backend`+`app` together via
    /// [`Self::parts_mut`] instead), so it's the one driver that doesn't
    /// call this — gated so a `macos`-only build (no `tui`/`gtk`/`win`)
    /// doesn't trip `dead_code`.
    #[cfg(any(
        feature = "tui",
        feature = "gtk",
        all(feature = "win", target_os = "windows")
    ))]
    pub(crate) fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Whether the app has returned [`Reaction::Exit`].
    pub(crate) fn exited(&self) -> bool {
        self.exited
    }

    /// Latch `exited` directly, for a driver whose own `dispatch` doesn't
    /// go through [`Self::apply_outcome`] (`TuiDriver`'s hand-written
    /// multi-event loop — see the module doc) but still needs to record
    /// an `EventOutcome::Exit` it matched on itself. Only `TuiDriver`
    /// calls this — gated so a `gtk`/`macos`/`win`-only build doesn't
    /// trip `dead_code`.
    #[cfg(feature = "tui")]
    pub(crate) fn mark_exited(&mut self) {
        self.exited = true;
    }

    /// Split mutable access to the backend and the app at once.
    ///
    /// A production `dispatch_event(event, &mut backend, &mut app)` call
    /// needs both simultaneously; two separate [`Self::backend_mut`]/
    /// [`Self::app_mut`] calls can't provide that; each one borrows
    /// `&mut self` for as long as its returned reference lives, so using
    /// both in the same call expression is a double mutable borrow of
    /// `self`. Splitting the two fields up front, the way this method
    /// does, borrows each disjointly instead.
    pub(crate) fn parts_mut(&mut self) -> (&mut B, &mut A) {
        (&mut self.backend, &mut self.app)
    }
}

// `apply_outcome` only has a caller on the three backends whose own
// `dispatch` is a single `EventOutcome` in, one `Reaction` out shape
// (`GtkDriver`, `MacDriver`, `WinDriver`) — `TuiDriver`'s hand-written
// multi-event loop uses `mark_exited` directly instead (see the module
// doc), so under a `tui`-only build (no `gtk`/`macos`/`win`) this method
// would have zero callers and trip `dead_code` under this crate's
// workflow-wide `-D warnings`. Same cfg predicate as the driver structs
// themselves compile under (`macos`/`win` additionally need their
// `target_os` — see each module's own `mod testing;` gate).
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
impl<B, A> DriverCore<B, A> {
    /// Apply one [`EventOutcome`] the way every production
    /// `dispatch_event` caller does: no-op on `Continue`, repaint (via
    /// caller-supplied `render`) on `Redraw`, latch `exited` on `Exit` so
    /// a later dispatch short-circuits to `Reaction::Exit` without
    /// re-entering the app. `render` takes `&mut B`/`&A` rather than
    /// closing over the concrete driver's `self` because the driver's
    /// real render call also needs its own surface (`ImageSurface`,
    /// `BitmapSurface`, …), which this type deliberately doesn't own —
    /// see the module doc.
    pub(crate) fn apply_outcome(
        &mut self,
        outcome: EventOutcome,
        render: impl FnOnce(&mut B, &A),
    ) -> Reaction {
        match outcome {
            EventOutcome::Continue => Reaction::Continue,
            EventOutcome::Redraw => {
                render(&mut self.backend, &self.app);
                Reaction::Redraw
            }
            // quadraui#832: no render (the whole point of `RedrawAfter`
            // is deferring one), and no driver-side timer to arm either
            // — a headless driver has no event loop for a scheduled
            // wake to interrupt. Pass the deadline through so a test
            // that wants to assert on it can.
            EventOutcome::RedrawAfter(d) => Reaction::RedrawAfter(d),
            EventOutcome::Exit => {
                self.exited = true;
                Reaction::Exit
            }
        }
    }
}

// This whole section — the trait, both impls, and the generic
// `DriverCore` methods gated on it — only has a consumer when at least
// one of `MacBackend`/`WinBackend` is actually in the build: with
// neither feature on (e.g. this crate's `tui`/`gtk` CI legs), nothing
// implements `TextRunSource` and nothing calls the methods below, which
// trips `dead_code` under this crate's workflow-wide `-D warnings`. Mirrors
// the same two-backend cfg predicate the paint-time text-run sink at the
// top of `testing/mod.rs` uses, minus TUI/GTK (neither is ever a
// `TextRunSource`, so their features don't belong in this predicate).
#[cfg(any(
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
mod text_run_query {
    use super::DriverCore;
    use crate::testing::TextRun;
    use crate::Rect;

    /// A backend that records paint-time text runs as a flat `&[TextRun]`
    /// — implemented for [`crate::macos::backend::MacBackend`] and
    /// [`crate::win::backend::WinBackend`] below (quadraui#721 gave both
    /// an identically-shaped `text_runs()` accessor). `GtkBackend` is
    /// deliberately not a `TextRunSource`: its `painted_text` field is a
    /// private `Vec<PaintedText>`, a different shape recorded at a
    /// different choke point (`gtk::painted_text::show_layout`), so
    /// `GtkDriver` keeps its own
    /// `painted_texts`/`screen_contains`/`find_bounds`/`find` bodies
    /// rather than adopting this trait. TUI has no backend-side
    /// text-run sink at all — `TuiDriver::inventory` scans the cell grid
    /// directly.
    pub(crate) trait TextRunSource {
        fn text_runs(&self) -> &[TextRun];
    }

    #[cfg(all(feature = "macos", target_os = "macos"))]
    impl TextRunSource for crate::macos::backend::MacBackend {
        fn text_runs(&self) -> &[TextRun] {
            // Resolves to the inherent `MacBackend::text_runs`, not this
            // trait method — inherent methods take priority.
            self.text_runs()
        }
    }

    #[cfg(all(feature = "win", target_os = "windows"))]
    impl TextRunSource for crate::win::backend::WinBackend {
        fn text_runs(&self) -> &[TextRun] {
            // Resolves to the inherent `WinBackend::text_runs`, not this
            // trait method — inherent methods take priority.
            self.text_runs()
        }
    }

    impl<B: TextRunSource, A> DriverCore<B, A> {
        /// All text painted during the last render, as recorded by this
        /// backend's paint-time sink — the shared body behind
        /// `MacDriver::painted_texts`/`WinDriver::painted_texts`.
        pub(crate) fn painted_texts(&self) -> Vec<&str> {
            self.backend()
                .text_runs()
                .iter()
                .map(|r| r.text.as_str())
                .collect()
        }

        /// True if any painted text run contains `needle` — the shared
        /// body behind
        /// `MacDriver::screen_contains`/`WinDriver::screen_contains`.
        pub(crate) fn screen_contains(&self, needle: &str) -> bool {
            self.backend()
                .text_runs()
                .iter()
                .any(|r| r.text.contains(needle))
        }

        /// Bounds of the first painted text run containing `needle` —
        /// the shared body behind
        /// `MacDriver::find_bounds`/`WinDriver::find_bounds`.
        pub(crate) fn find_bounds(&self, needle: &str) -> Option<Rect> {
            self.backend()
                .text_runs()
                .iter()
                .find(|r| r.text.contains(needle))
                .map(|r| r.bounds)
        }

        /// Center coordinates of the first painted text run containing
        /// `needle` — the shared body behind
        /// `MacDriver::find`/`WinDriver::find`.
        pub(crate) fn find(&self, needle: &str) -> Option<(f32, f32)> {
            self.find_bounds(needle)
                .map(|b| (b.x + b.width / 2.0, b.y + b.height / 2.0))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        struct FakeTextBackend {
            runs: Vec<TextRun>,
        }

        impl TextRunSource for FakeTextBackend {
            fn text_runs(&self) -> &[TextRun] {
                &self.runs
            }
        }

        fn fake_text_core() -> DriverCore<FakeTextBackend, ()> {
            DriverCore::new(
                (),
                FakeTextBackend {
                    runs: vec![TextRun {
                        text: "hello world".into(),
                        bounds: Rect::new(10.0, 20.0, 40.0, 8.0),
                    }],
                },
            )
        }

        #[test]
        fn painted_texts_lists_every_run() {
            let core = fake_text_core();
            assert_eq!(core.painted_texts(), vec!["hello world"]);
        }

        #[test]
        fn screen_contains_matches_a_substring() {
            let core = fake_text_core();
            assert!(core.screen_contains("world"));
            assert!(!core.screen_contains("nope"));
        }

        #[test]
        fn find_bounds_and_find_locate_the_matching_run() {
            let core = fake_text_core();
            assert_eq!(
                core.find_bounds("hello"),
                Some(Rect::new(10.0, 20.0, 40.0, 8.0))
            );
            assert_eq!(core.find("hello"), Some((30.0, 24.0)));
            assert_eq!(core.find_bounds("nope"), None);
            assert_eq!(core.find("nope"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeApp(u32);
    struct FakeBackend;

    #[test]
    fn accessors_read_and_write_through() {
        let mut core: DriverCore<FakeBackend, FakeApp> = DriverCore::new(FakeApp(1), FakeBackend);
        assert_eq!(core.app().0, 1);
        core.app_mut().0 = 2;
        assert_eq!(core.app().0, 2);
        let _: &FakeBackend = core.backend();
        assert!(
            !core.exited(),
            "a freshly constructed core must not report exited"
        );
    }

    #[test]
    #[cfg(any(
        feature = "tui",
        feature = "gtk",
        all(feature = "win", target_os = "windows")
    ))]
    fn backend_mut_gives_mutable_access() {
        let mut core: DriverCore<FakeBackend, FakeApp> = DriverCore::new(FakeApp(1), FakeBackend);
        let _: &mut FakeBackend = core.backend_mut();
    }

    #[test]
    #[cfg(feature = "tui")]
    fn mark_exited_latches_without_going_through_apply_outcome() {
        let mut core: DriverCore<FakeBackend, FakeApp> = DriverCore::new(FakeApp(0), FakeBackend);
        assert!(!core.exited());
        core.mark_exited();
        assert!(core.exited());
    }

    #[test]
    fn parts_mut_gives_disjoint_mutable_access_to_both_fields() {
        let mut core: DriverCore<FakeApp, FakeApp> = DriverCore::new(FakeApp(1), FakeApp(2));
        let (backend, app) = core.parts_mut();
        backend.0 = 10;
        app.0 = 20;
        assert_eq!(core.backend().0, 10);
        assert_eq!(core.app().0, 20);
    }

    #[test]
    #[cfg(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    ))]
    fn apply_outcome_continue_is_a_no_op() {
        let mut core: DriverCore<FakeBackend, FakeApp> = DriverCore::new(FakeApp(0), FakeBackend);
        let mut rendered = 0;
        let reaction = core.apply_outcome(EventOutcome::Continue, |_, _| rendered += 1);
        assert_eq!(reaction, Reaction::Continue);
        assert_eq!(rendered, 0, "Continue must never invoke the render closure");
        assert!(!core.exited());
    }

    #[test]
    #[cfg(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    ))]
    fn apply_outcome_redraw_renders_and_reports_redraw() {
        let mut core: DriverCore<FakeBackend, FakeApp> = DriverCore::new(FakeApp(0), FakeBackend);
        let mut rendered = 0;
        let reaction = core.apply_outcome(EventOutcome::Redraw, |_, _| rendered += 1);
        assert_eq!(reaction, Reaction::Redraw);
        assert_eq!(
            rendered, 1,
            "Redraw must invoke the render closure exactly once"
        );
        assert!(!core.exited());
    }

    #[test]
    #[cfg(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    ))]
    fn apply_outcome_exit_latches_exited_without_rendering() {
        let mut core: DriverCore<FakeBackend, FakeApp> = DriverCore::new(FakeApp(0), FakeBackend);
        let mut rendered = 0;
        let reaction = core.apply_outcome(EventOutcome::Exit, |_, _| rendered += 1);
        assert_eq!(reaction, Reaction::Exit);
        assert_eq!(rendered, 0, "Exit must never invoke the render closure");
        assert!(
            core.exited(),
            "Exit must latch `exited` so a later dispatch short-circuits"
        );
    }
}
