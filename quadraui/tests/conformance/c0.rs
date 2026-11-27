//! Tier C0 — smoke test that every primitive draws observable output.
//!
//! `Backend` gives several `draw_*`-adjacent methods a no-op (or `false`)
//! default so a new backend compiles before every optional surface is
//! wired up. That silence is the bug: a backend can implement every
//! `draw_*` method and still discard the caller's content, and nothing
//! in a green build says so. C0 is the boot tier that closes that gap:
//! for every primitive in [`CASES`], `begin_frame` → draw → `end_frame`
//! must not panic, and the resulting frame must be **observable** — a
//! non-empty `FrameInventory::text_runs()` or a non-empty `zones()` —
//! on every registered backend.
//!
//! [`CASES`] *is* the generator: [`run`] is generic over
//! [`super::runner::DriverFactory`], so adding a backend costs nothing
//! here (same "one factory, no per-backend code" shape as
//! `fixtures::build`), and adding a primitive is one more entry in the
//! table rather than one more hand-written test function per backend.
//!
//! Deliberately weak: this is "the primitive draws *something*
//! attributable to it", not a rendering assertion — layout and
//! interaction get checked elsewhere.
//!
//! [`CASES`] covers every `draw_*` method [`quadraui::Backend`] declares.
//! That claim is not just asserted in prose:
//! `tests::cases_cover_every_draw_method_on_the_trait` below parses the
//! `Backend` trait body in `backend.rs`'s own source for every `fn draw_*`
//! trait item and fails if `CASES` and the trait ever disagree, in either
//! direction. A `fn draw_*` elsewhere in that file — a generic entry point
//! that cannot sit on dyn-compatible `Backend`, so no backend overrides it
//! — is exempt only by being named, with its reason, in
//! `tests::NON_TRAIT_DRAW_FNS`.

use quadraui::{
    AppLogic, Backend, Decoration, ListItem, ListView, ProgressBar, Reaction, Rect, ScrollAxis,
    Scrollbar, Spinner, StyledText, UiEvent, WidgetId,
};

use super::runner::{DriverFactory, DynDriver};

/// Backend-neutral viewport, generous enough that no descriptor below
/// clips for want of room — a C0 smoke that failed on fixture size would
/// be reporting the fixture, not the backend.
const VIEWPORT: quadraui::testing::LogicalViewport =
    quadraui::testing::LogicalViewport::new(80, 24);

fn id(suffix: &str) -> WidgetId {
    WidgetId::new(format!("c0:{suffix}"))
}

/// One primitive's C0 case.
pub struct Case {
    /// The `Backend` method this case exercises — named in failure text
    /// so a red row points straight at the trait method to fix.
    pub method: &'static str,
    /// The text this descriptor hands the backend, or `None` for a
    /// chrome-only primitive that paints no text at all.
    pub needle: Option<&'static str>,
    /// Paints the descriptor into the given viewport rect, in the
    /// backend's own units (never literal cells or pixels).
    pub paint: fn(&mut dyn Backend, Rect),
}

/// One exemplar per primitive, built from public quadraui API only.
///
/// **Exhaustive, and mechanically held so**: every `fn draw_*` on
/// [`quadraui::Backend`] has exactly one entry here, and
/// [`tests::cases_cover_every_draw_method_on_the_trait`] fails if that
/// ever stops being true in either direction. Adding a primitive to the
/// trait is therefore one more entry here, not an optional one.
pub const CASES: &[Case] = &[
    Case {
        method: "draw_list",
        needle: Some("c0list"),
        paint: |b, area| {
            let list = ListView {
                id: id("list"),
                title: None,
                items: vec![ListItem {
                    text: StyledText::plain("c0list"),
                    icon: None,
                    detail: None,
                    decoration: Decoration::Normal,
                }],
                selected_idx: 0,
                scroll_offset: 0,
                has_focus: false,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
                striped: false,
            };
            #[allow(deprecated)]
            b.draw_list(area, &list);
        },
    },
    // `draw_list_with_state` is an additive trait default that forwards to
    // `draw_list`, ignoring both indices, until a backend overrides it to
    // tint hover/pressed. This row proves the default reaches a rasteriser
    // and still paints observable content on every backend.
    Case {
        method: "draw_list_with_state",
        needle: Some("c0listhv"),
        paint: |b, area| {
            let list = ListView {
                id: id("list-hover"),
                title: None,
                items: vec![ListItem {
                    text: StyledText::plain("c0listhv"),
                    icon: None,
                    detail: None,
                    decoration: Decoration::Normal,
                }],
                selected_idx: usize::MAX,
                scroll_offset: 0,
                has_focus: false,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
                striped: false,
            };
            b.draw_list_with_state(
                area,
                &list,
                &quadraui::ListPaintState::default().with_hovered_idx(Some(0)),
                false,
            );
        },
    },
    Case {
        method: "draw_spinner",
        needle: Some("c0spin"),
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let spinner = Spinner {
                id: id("spinner"),
                label: "c0spin".to_string(),
                frame_idx: 0,
                accent: None,
            };
            let _ = b.draw_spinner(Rect::new(0.0, 0.0, cw * 20.0, lh), &spinner);
            let _ = area;
        },
    },
    Case {
        method: "draw_progress",
        needle: Some("c0prog"),
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            let bar = ProgressBar {
                id: id("progress"),
                label: "c0prog".to_string(),
                value: Some(0.5),
                frame_idx: 0,
                cancellable: false,
                accent: None,
            };
            let _ = b.draw_progress(Rect::new(0.0, 0.0, cw * 30.0, lh * 2.0), &bar);
            let _ = area;
        },
    },
    // ── Chrome-only primitives — no text, so the only way the frame can
    // report them at all is a registered zone (contract §5b's sharp end).
    Case {
        method: "draw_scrollbar",
        needle: None,
        paint: |b, area| {
            let cw = b.char_width();
            let track = Rect::new(area.width - cw, 0.0, cw, area.height);
            let mut sb = Scrollbar::default();
            sb.id = id("scrollbar");
            sb.axis = ScrollAxis::Vertical;
            sb.track = track;
            sb.thumb_len = area.height / 2.0;
            b.draw_scrollbar(track, &sb);
        },
    },
    Case {
        method: "draw_focus_ring",
        needle: None,
        paint: |b, area| {
            let cw = b.char_width();
            let lh = b.line_height();
            b.draw_focus_ring(Rect::new(cw * 2.0, lh * 2.0, cw * 20.0, lh * 3.0));
            let _ = area;
        },
    },
];

/// An `AppLogic` whose whole render pass is one primitive's canned
/// descriptor.
struct Fixture {
    paint: fn(&mut dyn Backend, Rect),
}

impl AppLogic for Fixture {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        (self.paint)(backend, Rect::new(0.0, 0.0, vp.width, vp.height));
    }

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}

/// One case's outcome on one backend. Positional: `run()` returns exactly
/// one `CaseOutcome` per [`CASES`] entry, in the same order, so callers
/// zip the two rather than this type repeating `Case::method`.
pub struct CaseOutcome {
    /// `false` means the primitive panicked mid-paint — a worse failure
    /// than painting nothing, and the one C0's boot half exists to catch.
    pub survived: bool,
    /// `true` when the descriptor had no needle, or the needle it handed
    /// the backend came back in `text_runs()`.
    pub text_ok: bool,
    /// `true` when the frame reports a non-empty `text_runs()` or a
    /// non-empty `zones()` — contract §5b's observability floor.
    pub observable: bool,
    /// A compact dump of what the frame reported, for failure text.
    pub reported: String,
}

/// Run every [`CASES`] entry against one backend, built via `F`.
///
/// Generic over [`DriverFactory`] rather than duplicated per backend, so
/// a shared body can't accidentally assert something on one backend it
/// forgot to check on another.
pub fn run<F: DriverFactory>() -> Vec<CaseOutcome> {
    CASES
        .iter()
        .map(|case| {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let driver: Box<dyn DynDriver> = F::make(Fixture { paint: case.paint }, VIEWPORT);
                let inv = driver.inventory();
                let text_ok = match case.needle {
                    Some(needle) => driver.screen_has(needle),
                    None => true,
                };
                let observable = !inv.text_runs().is_empty() || !inv.zones().is_empty();
                let runs: Vec<String> = inv
                    .text_runs()
                    .iter()
                    .map(|r| r.text.clone())
                    .filter(|t| !t.trim().is_empty())
                    .take(10)
                    .collect();
                let zones: Vec<String> = inv
                    .zones()
                    .iter()
                    .map(|z| z.id.as_str().to_string())
                    .collect();
                (
                    text_ok,
                    observable,
                    format!("text_runs: {runs:?}; zones: {zones:?}"),
                )
            }));
            match outcome {
                Ok((text_ok, observable, reported)) => CaseOutcome {
                    survived: true,
                    text_ok,
                    observable,
                    reported,
                },
                Err(_) => CaseOutcome {
                    survived: false,
                    text_ok: false,
                    observable: false,
                    reported: "panicked mid-paint".to_string(),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::CASES;
    use std::collections::BTreeSet;

    const BACKEND_SRC: &str = include_str!("../../src/backend.rs");

    /// `fn draw_*` declarations in `backend.rs` that live *outside* the
    /// `Backend` trait body, each exempt from `CASES` for a reason stated
    /// here. C0 smoke-tests per-backend paint output, so a method only
    /// belongs in `CASES` when a backend can override it and when every
    /// registered backend can produce an observable frame from it.
    ///
    /// `DrawsPrimitives::draw_primitive` is generic over
    /// `quadraui::Primitive`, so it cannot sit on `Backend` at all —
    /// `Backend` is used as `&mut dyn Backend` crate-wide. It is declared
    /// once on a sibling trait blanket-implemented for every `Backend`,
    /// which no backend can override, and its body paints only through
    /// whichever of `Backend::paint_surface` (GTK/macOS/Win) or
    /// `Backend::cell_surface` (TUI) the receiving backend answers `Some`
    /// from — and even then, only for a `Primitive` whose own `paint`/
    /// `paint_cells` impl does something observable, which `CASES`' fixed
    /// set of built-in primitives has no entry for. So there is no
    /// observable-on-every-backend frame for C0 to assert.
    /// `tests/primitive_public_api.rs` covers the method's own contract
    /// instead, and `src/primitives/list.rs`'s parity tests cover the
    /// pilot `Primitive` impl's pixel output.
    ///
    /// `DrawsPrimitives::draw_dyn` is the `&dyn DynWidget` counterpart of
    /// the same method on the same blanket-implemented sibling trait, so it
    /// is exempt for the same reasons: no backend can override it, and what
    /// it paints comes entirely from the erased widget handed to it rather
    /// than from the receiving backend, so `CASES`' fixed set of built-in
    /// primitives gives C0 nothing observable to assert on every backend.
    /// Its own contract — layout resolution, the shared `ctx`, and the
    /// single `PaintTarget` paint call per resolved target — is covered by
    /// the `DynWidget` tests in `src/primitive.rs` and `src/frame.rs`.
    const NON_TRAIT_DRAW_FNS: &[&str] = &["draw_dyn", "draw_primitive"];

    /// The `Backend` trait body's own source lines, from the `pub trait
    /// Backend:` declaration to its closing brace in column 0.
    fn backend_trait_body() -> Vec<&'static str> {
        let mut lines = BACKEND_SRC
            .lines()
            .skip_while(|line| !line.starts_with("pub trait Backend:"));
        lines
            .next()
            .expect("backend.rs must declare `pub trait Backend:` at column 0");
        lines.take_while(|line| *line != "}").collect()
    }

    /// Method names of every `fn draw_*` declaration in `lines`, skipping
    /// doc-comment prose (the example inside the "Implementations are thin
    /// wrappers" blurb is indented behind `///` rather than being a real
    /// item).
    fn draw_fn_names<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
        lines
            .into_iter()
            .filter(|line| line.trim_start().starts_with("fn draw_"))
            .map(|line| {
                line.trim_start()
                    .trim_start_matches("fn ")
                    .split(['(', '<'])
                    .next()
                    .unwrap()
                    .trim()
            })
            .collect()
    }

    /// `CASES` must name *every* `draw_*` method on `Backend`, not a
    /// hand-picked sample. Parse the `Backend` trait body in `backend.rs`'s
    /// own source for every `fn draw_*` trait item and assert the two lists
    /// agree exactly.
    #[test]
    fn cases_cover_every_draw_method_on_the_trait() {
        let trait_methods: Vec<&str> = draw_fn_names(backend_trait_body());
        assert!(
            trait_methods.len() >= 5,
            "sanity check failed: expected many `fn draw_*` trait methods in backend.rs, \
             found only {} — the line-based parser above likely broke against a reformatted \
             file rather than the trait actually shrinking: {trait_methods:?}",
            trait_methods.len()
        );

        let case_methods: BTreeSet<&str> = CASES.iter().map(|c| c.method).collect();

        let missing: Vec<&str> = trait_methods
            .iter()
            .copied()
            .filter(|m| !case_methods.contains(m))
            .collect();
        assert!(
            missing.is_empty(),
            "{} `Backend::draw_*` method(s) have no `CASES` entry, so C0 says nothing about \
             them (quadraui#492): {missing:?}",
            missing.len()
        );

        let stale: Vec<&str> = case_methods
            .iter()
            .copied()
            .filter(|m| !trait_methods.contains(m))
            .collect();
        assert!(
            stale.is_empty(),
            "CASES names method(s) not found on the `Backend` trait today — likely a typo, or \
             a renamed/removed method CASES was not updated for: {stale:?}"
        );
    }

    /// Reading only the `Backend` trait body above must not become a way to
    /// smuggle an unsmoked `draw_*` method into `backend.rs`: every
    /// `fn draw_*` declared outside that body has to be named in
    /// [`NON_TRAIT_DRAW_FNS`], with its reason, and every name there has to
    /// still exist.
    #[test]
    fn draw_fns_outside_the_backend_trait_are_declared_exempt() {
        let in_trait: BTreeSet<&str> = draw_fn_names(backend_trait_body()).into_iter().collect();
        let outside: BTreeSet<&str> = draw_fn_names(BACKEND_SRC.lines())
            .into_iter()
            .filter(|method| !in_trait.contains(method))
            .collect();

        let undeclared: Vec<&str> = outside
            .iter()
            .copied()
            .filter(|method| !NON_TRAIT_DRAW_FNS.contains(method))
            .collect();
        assert!(
            undeclared.is_empty(),
            "{} `fn draw_*` declaration(s) in backend.rs sit outside the `Backend` trait body \
             and are not listed in NON_TRAIT_DRAW_FNS, so neither C0 nor the coverage check \
             above says anything about them: {undeclared:?} — add a `CASES` entry if a backend \
             can override it, or list it with its reason",
            undeclared.len()
        );

        let stale: Vec<&str> = NON_TRAIT_DRAW_FNS
            .iter()
            .copied()
            .filter(|method| !outside.contains(method))
            .collect();
        assert!(
            stale.is_empty(),
            "NON_TRAIT_DRAW_FNS names method(s) backend.rs no longer declares outside the \
             `Backend` trait body — renamed, removed, or promoted onto the trait (where it \
             needs a `CASES` entry instead): {stale:?}"
        );
    }
}
