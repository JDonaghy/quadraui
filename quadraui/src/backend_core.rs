//! `BackendCore` — the accelerator registry + text-selection bookkeeping
//! every pixel/cell backend embeds (#1090).
//!
//! # What this consolidates
//!
//! Before this module, `TuiBackend`, `GtkBackend`, `MacBackend` and
//! `WinBackend` each carried their own copy of:
//!
//! 1. The accelerator registry: `accelerators: HashMap<AcceleratorId,
//!    Accelerator>` + `parsed_accelerators: Vec<(ParsedBinding,
//!    AcceleratorId)>`, plus `register_accelerator`/`unregister_accelerator`/
//!    `match_keypress`/`apply_accelerators` — identical logic, sometimes
//!    (`GtkBackend`) re-deriving helpers (`parse_binding`,
//!    `named_key_to_binding_name`) that [`crate::accelerator`] already
//!    exposes as `pub fn`s, instead of calling them.
//! 2. A `text_selection: crate::text_selection::TextSelectionState` field
//!    plus ~8 thin `pub(crate)` wrapper methods
//!    (`active_text_selection`/`set_active_text_selection`/
//!    `clear_selection_display`/`clear_text_selection`/
//!    `cancel_text_selection_drag`/`track_focused_text_region`/
//!    `select_all_text_region`/`register_text_region`), each with its own
//!    (near-verbatim) doc comment, repeated four times.
//!
//! `BackendCore` is the one place both now live. Each backend embeds it
//! as a single field (replacing the two/three fields above) and calls
//! its methods directly from its `Backend`/`PreprocessBackend` trait
//! impls — no per-backend wrapper layer left to duplicate.
//!
//! # What stays backend-specific
//!
//! - **TUI's selection highlight** reads the live `ratatui::buffer::Buffer`
//!   cell-by-cell (only reachable inside `terminal.draw`'s closure) and
//!   caches the extracted text for Ctrl-C — see
//!   `crate::tui::backend::TuiBackend::apply_selection_highlight`. That
//!   stays a TUI-only method; it reads `self.core.text_selection`
//!   directly (the field is `pub(crate)`) rather than going through a
//!   `BackendCore` wrapper, since nothing else needs a TUI-flavoured cut
//!   of this state.
//! - **The real paint call** for GTK/macOS/Win-GUI (Cairo `fill()`,
//!   CoreGraphics `CGContextFillRect`, Direct2D `FillRectangle`) is real
//!   toolkit code with no portable shape, so it stays in each backend's
//!   own `apply_selection_highlight`. [`BackendCore::selection_highlight_rects`]
//!   is the one piece of that method's logic (this crate) — resolving
//!   the active selection + `TextRegion` into row-relative, then
//!   pixel-space, rectangles — that *was* duplicated (byte-identical
//!   modulo variable names) across all three; it now runs once. The
//!   `#[cfg(...)]` gate mirrors [`crate::native_surface`]'s: only the
//!   pixel-based backends call it, so a `tui`-only build doesn't carry
//!   dead code under `-D warnings`.
//! - **macOS's universal-binding modifier translation**
//!   (`macos_universal_binding_modifiers` — Cmd instead of Ctrl for
//!   `KeyBinding::Save`/`Copy`/etc.) is the one place accelerator
//!   registration genuinely differs per backend.
//!   [`BackendCore::register_accelerator_with`] takes a
//!   `FnOnce(ParsedBinding) -> ParsedBinding` transform so `MacBackend`
//!   can still apply it while every other backend passes the identity
//!   closure via [`BackendCore::register_accelerator`].

use std::collections::HashMap;

use crate::accelerator::{key_to_binding_name, parse_binding};
use crate::dispatch::{DragState, TextRegion};
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::event::Rect;
use crate::event::{Key, Point};
use crate::text_selection::{ActiveTextSelection, TextSelectionState};
use crate::types::{Modifiers, WidgetId};
use crate::{Accelerator, AcceleratorId, AcceleratorScope, ParsedBinding, UiEvent};

/// Shared accelerator registry + text-selection bookkeeping. See the
/// module doc for what this replaces and what deliberately stays
/// per-backend.
#[derive(Default)]
pub(crate) struct BackendCore {
    accelerators: HashMap<AcceleratorId, Accelerator>,
    /// Pre-parsed bindings, kept in lock-step with `accelerators`.
    /// First-match-wins iteration order matches insertion order (`Vec`,
    /// not `HashMap`).
    parsed_accelerators: Vec<(ParsedBinding, AcceleratorId)>,
    /// Region registry + active-selection state. `pub(crate)`: TUI's
    /// live-buffer selection highlight/extraction reads this directly
    /// (see the module doc) rather than through a wrapper method, and
    /// [`crate::dispatch::route_pointer`] needs `&mut TextSelectionState`
    /// at a couple of call sites that also touch other backend fields in
    /// the same statement.
    pub(crate) text_selection: TextSelectionState,
}

impl BackendCore {
    // ─── Accelerators ───────────────────────────────────────────────────

    /// Register `acc`, replacing any prior entry with the same id (both
    /// in the map and the parsed list — otherwise a stale binding would
    /// shadow the new one in [`Self::match_keypress`]).
    ///
    /// `#[allow(dead_code)]`: `TuiBackend`/`GtkBackend`/`WinBackend` call
    /// this directly; `MacBackend` calls
    /// [`Self::register_accelerator_with`] instead (its one adopter of
    /// the transform hook — see that method's doc), so a `--features
    /// macos`-only build (`macos.yml`'s cross-target compile check, see
    /// `CLAUDE.md`) has no caller for this exact method and would
    /// otherwise fail its `-D warnings` `dead_code` gate.
    #[allow(dead_code)]
    pub(crate) fn register_accelerator(&mut self, acc: &Accelerator) {
        self.register_accelerator_with(acc, |parsed| parsed);
    }

    /// [`Self::register_accelerator`], but `transform` gets a chance to
    /// rewrite the freshly-parsed binding before it's stored — the one
    /// adopter is `MacBackend`, which maps universal bindings onto Cmd
    /// instead of Ctrl via `macos_universal_binding_modifiers`. Every
    /// other backend calls [`Self::register_accelerator`] (the identity
    /// transform).
    pub(crate) fn register_accelerator_with(
        &mut self,
        acc: &Accelerator,
        transform: impl FnOnce(ParsedBinding) -> ParsedBinding,
    ) {
        self.accelerators.insert(acc.id.clone(), acc.clone());
        self.parsed_accelerators.retain(|(_, id)| id != &acc.id);
        if let Some(parsed) = parse_binding(&acc.binding) {
            self.parsed_accelerators
                .push((transform(parsed), acc.id.clone()));
        }
    }

    /// Remove a registered accelerator (both the display entry and the
    /// parsed matcher entry). A no-op if `id` isn't registered.
    pub(crate) fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.accelerators.remove(id);
        self.parsed_accelerators.retain(|(_, eid)| eid != id);
    }

    /// Look up a registered `Global`-scope accelerator for a `(key,
    /// modifiers)` pair. Non-`Global`-scope entries are skipped — the
    /// backend doesn't own focus/mode context the way a scoped resolver
    /// does.
    pub(crate) fn match_keypress(&self, key: &Key, modifiers: Modifiers) -> Option<AcceleratorId> {
        let key_name = key_to_binding_name(key);
        for (parsed, id) in &self.parsed_accelerators {
            if parsed.modifiers == modifiers && parsed.key == key_name {
                if let Some(acc) = self.accelerators.get(id) {
                    if matches!(acc.scope, AcceleratorScope::Global) {
                        return Some(id.clone());
                    }
                }
            }
        }
        None
    }

    /// Rewrite matching `UiEvent::KeyPressed` events in place as
    /// `UiEvent::Accelerator(id, modifiers)`. A no-op (skips the whole
    /// slice without iterating) when nothing is registered.
    ///
    /// `#[allow(dead_code)]`: `TuiBackend`/`GtkBackend`/`WinBackend` batch
    /// events and rewrite them via this method; `MacBackend` dispatches
    /// synchronously per `NSEvent` and calls [`Self::match_keypress`]
    /// directly instead (see that backend's `match_keypress` doc), so a
    /// `--features macos`-only build has no caller — same rationale as
    /// [`Self::register_accelerator`]'s attribute above.
    #[allow(dead_code)]
    pub(crate) fn apply_accelerators(&self, events: &mut [UiEvent]) {
        if self.parsed_accelerators.is_empty() {
            return;
        }
        for ev in events.iter_mut() {
            if let UiEvent::KeyPressed { key, modifiers, .. } = ev {
                if let Some(id) = self.match_keypress(key, *modifiers) {
                    *ev = UiEvent::Accelerator(id, *modifiers);
                }
            }
        }
    }

    // ─── Text selection ─────────────────────────────────────────────────
    //
    // Thin forwards to `TextSelectionState` — see that type's doc for the
    // actual behaviour. Kept here (rather than requiring every call site
    // to spell `self.core.text_selection.foo()`) so a backend's
    // `Backend`/`PreprocessBackend` impl reads as a one-line delegation
    // to `self.core.foo()`, matching the accelerator methods above.

    /// Clear the per-frame region registry. Call at the start of every
    /// frame (same lifecycle as a backend's other per-frame caches).
    pub(crate) fn begin_frame(&mut self) {
        self.text_selection.begin_frame();
    }

    /// Register `region` for the current frame.
    pub(crate) fn register_text_region(&mut self, region: TextRegion) {
        self.text_selection.register_text_region(region);
    }

    /// Return the current active text selection, if any.
    pub(crate) fn active_text_selection(&self) -> Option<&ActiveTextSelection> {
        self.text_selection.active_text_selection()
    }

    /// Update (or start) the active text selection.
    pub(crate) fn set_active_text_selection(
        &mut self,
        region: WidgetId,
        anchor: Point,
        focus: Point,
    ) {
        self.text_selection
            .set_active_text_selection(region, anchor, focus);
    }

    /// Clear the active text selection highlight only (does NOT end an
    /// in-progress `TextSelection` drag).
    pub(crate) fn clear_selection_display(&mut self) {
        self.text_selection.clear_selection_display();
    }

    /// Clear the active text selection and end any in-progress
    /// `TextSelection` drag.
    pub(crate) fn clear_text_selection(&mut self, drag_state: &mut DragState) {
        self.text_selection.clear_text_selection(drag_state);
    }

    /// End any in-progress `TextSelection` drag without clearing the
    /// displayed `active_selection`.
    pub(crate) fn cancel_text_selection_drag(&mut self, drag_state: &mut DragState) {
        self.text_selection.cancel_text_selection_drag(drag_state);
    }

    /// Set the active selection to cover the entire visible content of
    /// the most-recently focused `TextRegion` (the Ctrl-A target).
    pub(crate) fn select_all_text_region(&mut self) -> bool {
        self.text_selection.select_all_text_region()
    }

    // ─── Pixel-based selection highlight (GTK / Win-GUI / macOS) ───────

    /// Resolve the active selection into pixel-space rectangles ready to
    /// hand to each backend's real fill call, or `None` when there is
    /// nothing to paint (no active selection, the region isn't
    /// registered this frame, metrics aren't known yet, or every
    /// resolved range has zero-or-negative width).
    ///
    /// This is [`crate::text_selection::pixel_selection_ranges`] (cell
    /// math) plus [`crate::paint_geometry::text_selection_highlight_rects`]
    /// (range → pixel rect) chained together — the exact two-step
    /// pipeline `GtkBackend::apply_selection_highlight`,
    /// `MacBackend::apply_selection_highlight`, and
    /// `WinBackend::apply_selection_highlight` each ran inline before
    /// #1090, byte-identical modulo local variable names.
    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    pub(crate) fn selection_highlight_rects(
        &self,
        line_height: f32,
        char_width: f32,
    ) -> Option<Vec<Rect>> {
        let sel = self.text_selection.active_text_selection()?;
        let region = self.text_selection.find_region(&sel.region)?;
        let ranges = crate::text_selection::pixel_selection_ranges(
            region.bounds,
            sel.anchor,
            sel.focus,
            line_height,
            char_width,
        )?;
        if ranges.is_empty() {
            return None;
        }
        let rects = crate::paint_geometry::text_selection_highlight_rects(
            region.bounds,
            &ranges,
            char_width as f64,
            line_height as f64,
        );
        if rects.is_empty() {
            None
        } else {
            Some(rects)
        }
    }

    /// Extract the selected text from the active selection's `TextRegion`
    /// using its stored `lines` — the pixel-based twin of TUI's
    /// live-buffer extraction (see the module doc). Empty when there is
    /// no active selection, the region isn't registered this frame, or
    /// it has no `lines` content.
    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    pub(crate) fn extract_selection_text_pixel(&self, line_height: f32, char_width: f32) -> String {
        let Some(sel) = self.text_selection.active_text_selection() else {
            return String::new();
        };
        let Some(region) = self.text_selection.find_region(&sel.region) else {
            return String::new();
        };
        crate::text_selection::extract_lines_pixel(
            region,
            sel.anchor,
            sel.focus,
            line_height,
            char_width,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accelerator::KeyBinding;
    use crate::event::NamedKey;

    fn save_accelerator() -> Accelerator {
        Accelerator {
            id: AcceleratorId::new("app.save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        }
    }

    #[test]
    fn register_then_match_finds_the_binding() {
        let mut core = BackendCore::default();
        core.register_accelerator(&save_accelerator());
        let id = core.match_keypress(
            &Key::Char('s'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert_eq!(id, Some(AcceleratorId::new("app.save")));
    }

    #[test]
    fn unregister_removes_the_match() {
        let mut core = BackendCore::default();
        core.register_accelerator(&save_accelerator());
        core.unregister_accelerator(&AcceleratorId::new("app.save"));
        assert_eq!(
            core.match_keypress(
                &Key::Char('s'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                }
            ),
            None
        );
    }

    #[test]
    fn register_accelerator_with_applies_the_transform() {
        let mut core = BackendCore::default();
        core.register_accelerator_with(&save_accelerator(), |mut parsed| {
            parsed.modifiers.cmd = true;
            parsed.modifiers.ctrl = false;
            parsed
        });
        assert_eq!(
            core.match_keypress(
                &Key::Char('s'),
                Modifiers {
                    cmd: true,
                    ..Default::default()
                }
            ),
            Some(AcceleratorId::new("app.save"))
        );
        assert_eq!(
            core.match_keypress(
                &Key::Char('s'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                }
            ),
            None
        );
    }

    #[test]
    fn apply_accelerators_rewrites_matching_key_presses() {
        let mut core = BackendCore::default();
        core.register_accelerator(&save_accelerator());
        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Char('s'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }];
        core.apply_accelerators(&mut events);
        assert_eq!(
            events[0],
            UiEvent::Accelerator(
                AcceleratorId::new("app.save"),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                }
            )
        );
    }

    #[test]
    fn apply_accelerators_ignores_non_global_scope() {
        let mut core = BackendCore::default();
        core.register_accelerator(&Accelerator {
            id: AcceleratorId::new("widget.esc"),
            binding: KeyBinding::Literal("<Escape>".into()),
            scope: AcceleratorScope::Widget(WidgetId::new("picker")),
            label: None,
        });
        let mut events = vec![UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        }];
        core.apply_accelerators(&mut events);
        assert!(matches!(events[0], UiEvent::KeyPressed { .. }));
    }

    #[test]
    fn text_selection_round_trip_through_the_registry() {
        let mut core = BackendCore::default();
        core.register_text_region(TextRegion {
            id: WidgetId::new("body"),
            bounds: crate::event::Rect::new(0.0, 0.0, 100.0, 50.0),
            lines: vec![],
        });
        assert!(core.select_all_text_region());
        assert!(core.active_text_selection().is_some());
        core.clear_selection_display();
        assert!(core.active_text_selection().is_none());
        core.begin_frame();
        assert!(core.text_selection.text_regions.is_empty());
    }

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[test]
    fn selection_highlight_rects_none_with_no_active_selection() {
        let core = BackendCore::default();
        assert!(core.selection_highlight_rects(20.0, 10.0).is_none());
    }

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[test]
    fn selection_highlight_rects_resolves_a_single_row() {
        let mut core = BackendCore::default();
        core.register_text_region(TextRegion {
            id: WidgetId::new("body"),
            bounds: crate::event::Rect::new(0.0, 0.0, 200.0, 100.0),
            lines: vec!["hello world".into()],
        });
        core.set_active_text_selection(
            WidgetId::new("body"),
            Point::new(0.0, 0.0),
            Point::new(50.0, 5.0),
        );
        let rects = core
            .selection_highlight_rects(20.0, 10.0)
            .expect("a single-row selection resolves to at least one rect");
        assert_eq!(rects.len(), 1);
        // `text_selection_line_range` is half-open on the focus column
        // (+1), so focus x=50 (col 5) resolves to col_end=6 → 60px wide.
        assert_eq!(rects[0], Rect::new(0.0, 0.0, 60.0, 20.0));
    }

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[test]
    fn extract_selection_text_pixel_reads_the_region_lines() {
        let mut core = BackendCore::default();
        core.register_text_region(TextRegion {
            id: WidgetId::new("body"),
            bounds: crate::event::Rect::new(0.0, 0.0, 200.0, 100.0),
            lines: vec!["hello world".into()],
        });
        core.set_active_text_selection(
            WidgetId::new("body"),
            Point::new(0.0, 0.0),
            Point::new(50.0, 5.0),
        );
        assert_eq!(core.extract_selection_text_pixel(20.0, 10.0), "hello");
    }
}
