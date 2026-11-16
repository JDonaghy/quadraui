//! Reusable context-sensitive help layer for [`ShellApp`](crate::ShellApp)
//! consumers.
//!
//! Three pieces, composed from existing primitives rather than a new one:
//!
//! 1. **[`HelpRegistry`]** — apps register a [`ViewHelp`] (reference notes +
//!    actions) per view/panel id. "What this column/chip means" lives in
//!    [`HelpNote`]; "the available actions" (label + description +
//!    accelerator) live in [`HelpAction`].
//! 2. **[`HelpOverlayController`]** — a `?`-triggered cheatsheet overlay
//!    rendering the active view's [`ViewHelp`]. Composed from
//!    [`crate::Dialog`] (card chrome: padding, border, `Style::corner_radius`,
//!    rendered in the chrome/UI font on every backend —
//!    `Backend::draw_dialog` is implemented on every backend) with the
//!    reference notes in [`Dialog::body`] and the registered actions in a
//!    [`crate::DialogTable`] (two columns — "Shortcut", "Description" —
//!    whose widths [`Dialog`]'s shared paint auto-sizes from each cell's
//!    *real measured text width*, not a fixed character count, so columns
//!    stay aligned under a proportional font). Views with no registered
//!    help still get a visible (if minimal) cheatsheet rather than a
//!    silent no-op — see [`HelpOverlayController::render`].
//! 3. **[`help_actions_to_palette_items`] / [`filter_help_actions`]** — feed
//!    registered actions into the existing [`crate::Palette`] /
//!    [`crate::DualModePaletteController`] so they're searchable by label
//!    *and* description.
//!
//! # Cross-backend portability
//!
//! [`HelpOverlayController::render`] sizes the popup from
//! `backend.measure()` — never hardcoded cells/pixels — so the same code
//! paints correctly on every backend. [`Dialog`] classifies as a chrome
//! primitive (see `primitives::dialog`'s module doc), so the cheatsheet
//! renders in the UI font everywhere, including macOS/Win, where the
//! editor font is typically monospace and this content isn't editor
//! content.

use std::collections::BTreeMap;

use crate::compose::Handled;
use crate::event::Rect;
use crate::primitives::toolbar::ToolbarItemMeasure;
use crate::types::{StyledSpan, StyledText, WidgetId};
use crate::{Backend, Dialog, DialogMeasure, DialogTable, Key, NamedKey, PaletteItem, UiEvent};

// ── Data model ───────────────────────────────────────────────────────────

/// A descriptive help entry with no associated action — "what this
/// column/chip/badge means". Rendered in the cheatsheet's reference
/// section; never fed into the command palette (it isn't a command).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpNote {
    /// Short label (e.g. `"●"`, `"Modified"`, the thing being explained).
    pub label: String,
    /// One-line explanation.
    pub description: String,
}

impl HelpNote {
    pub fn new(label: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: description.into(),
        }
    }
}

/// A registered action: label + one-line description + optional
/// accelerator display string (e.g. `"Ctrl+S"` — see
/// [`crate::accelerator::render_binding`] to derive one platform-
/// appropriately). Rendered in the cheatsheet's actions section *and*
/// convertible to a [`PaletteItem`] via [`help_actions_to_palette_items`]
/// so the same registration drives both surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpAction {
    /// Stable id for the app to match on after a palette selection
    /// (mirrors [`crate::AcceleratorId`]'s role — apps `match` on this
    /// rather than the display label).
    pub id: WidgetId,
    pub label: String,
    pub description: String,
    pub accelerator: Option<String>,
}

impl HelpAction {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: WidgetId::new(id.into()),
            label: label.into(),
            description: description.into(),
            accelerator: None,
        }
    }

    pub fn with_accelerator(mut self, accelerator: impl Into<String>) -> Self {
        self.accelerator = Some(accelerator.into());
        self
    }
}

/// Everything a single view/panel wants to say about itself: a title for
/// the cheatsheet header, reference notes, and registered actions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewHelp {
    pub title: String,
    pub notes: Vec<HelpNote>,
    pub actions: Vec<HelpAction>,
}

impl ViewHelp {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            notes: Vec::new(),
            actions: Vec::new(),
        }
    }

    pub fn with_notes(mut self, notes: Vec<HelpNote>) -> Self {
        self.notes = notes;
        self
    }

    pub fn with_actions(mut self, actions: Vec<HelpAction>) -> Self {
        self.actions = actions;
        self
    }
}

/// Registry of per-view help content. Apps register once (typically at
/// startup) and look up by whatever view/panel id string they already
/// track (e.g. a [`WidgetId::as_str()`] of the active `ShellApp` panel).
#[derive(Debug, Clone, Default)]
pub struct HelpRegistry {
    views: BTreeMap<String, ViewHelp>,
}

impl HelpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the help content for a view id.
    pub fn register(&mut self, view_id: impl Into<String>, help: ViewHelp) {
        self.views.insert(view_id.into(), help);
    }

    /// Look up a view's help content.
    pub fn get(&self, view_id: &str) -> Option<&ViewHelp> {
        self.views.get(view_id)
    }

    /// Registered actions for a view, or an empty slice if the view has
    /// no registration (or no actions). Convenience for palette wiring —
    /// callers that need the notes too should use [`Self::get`] directly.
    pub fn actions_for(&self, view_id: &str) -> &[HelpAction] {
        self.views
            .get(view_id)
            .map(|v| v.actions.as_slice())
            .unwrap_or(&[])
    }
}

// ── Cheatsheet overlay ──────────────────────────────────────────────────

/// Semantic payload of [`Handled::Event`] returned by
/// [`HelpOverlayController::handle_event`] / `open_action` / `close_action` /
/// `toggle_action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpOverlayAction {
    /// The overlay just opened.
    Opened,
    /// The overlay just closed.
    Closed,
}

/// Result of [`HelpOverlayController::handle`] / `open` / `close` /
/// `toggle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[deprecated(
    since = "0.9.0",
    note = "use `HelpOverlayController::handle_event`'s `Handled<HelpOverlayAction>` instead — `Opened`/`Closed` move to `HelpOverlayAction` under `Handled::Event`, `Consumed`/`Ignored` are `Handled::Consumed`/`Handled::Ignored`"
)]
pub enum HelpOverlayEvent {
    /// The overlay just opened.
    Opened,
    /// The overlay just closed.
    Closed,
    /// A key was swallowed while the overlay was open (it's modal — keys
    /// don't fall through to the app underneath).
    Consumed,
    /// The event wasn't relevant (overlay closed and the key wasn't the
    /// open trigger).
    Ignored,
}

/// Cross-backend compose controller for a `?`-triggered cheatsheet
/// overlay. Wraps a [`Dialog`] — see the module docs for the card chrome
/// + aligned-table rendering this gets for free.
///
/// Purely a display + open/close state machine: it does not own a
/// [`HelpRegistry`] itself. Callers pass the current view's [`ViewHelp`]
/// to [`Self::render`] each frame (typically
/// `registry.get(current_view_id)`), so the same controller instance
/// works across every panel a `ShellApp` has — "context-sensitive"
/// falls out of the caller picking which `ViewHelp` to hand it.
#[derive(Debug, Clone)]
pub struct HelpOverlayController {
    id: WidgetId,
    open: bool,
}

impl HelpOverlayController {
    /// Create a closed overlay controller.
    pub fn new() -> Self {
        Self {
            id: WidgetId::new("help-cheatsheet"),
            open: false,
        }
    }

    /// Override the `WidgetId` used for the rendered [`Dialog`].
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = WidgetId::new(id.into());
        self
    }

    /// Whether the cheatsheet is currently showing.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the overlay unconditionally.
    pub fn open_action(&mut self) -> HelpOverlayAction {
        self.open = true;
        HelpOverlayAction::Opened
    }

    /// Open the overlay unconditionally.
    #[deprecated(
        since = "0.9.0",
        note = "use `open_action`, which returns `HelpOverlayAction` instead of `HelpOverlayEvent`"
    )]
    #[allow(deprecated)] // `HelpOverlayEvent` is `#[deprecated]`; this method is its own shim.
    pub fn open(&mut self) -> HelpOverlayEvent {
        match self.open_action() {
            HelpOverlayAction::Opened => HelpOverlayEvent::Opened,
            HelpOverlayAction::Closed => HelpOverlayEvent::Closed,
        }
    }

    /// Close the overlay unconditionally.
    pub fn close_action(&mut self) -> HelpOverlayAction {
        self.open = false;
        HelpOverlayAction::Closed
    }

    /// Close the overlay unconditionally.
    #[deprecated(
        since = "0.9.0",
        note = "use `close_action`, which returns `HelpOverlayAction` instead of `HelpOverlayEvent`"
    )]
    #[allow(deprecated)] // `HelpOverlayEvent` is `#[deprecated]`; this method is its own shim.
    pub fn close(&mut self) -> HelpOverlayEvent {
        match self.close_action() {
            HelpOverlayAction::Opened => HelpOverlayEvent::Opened,
            HelpOverlayAction::Closed => HelpOverlayEvent::Closed,
        }
    }

    /// Flip open/closed.
    pub fn toggle_action(&mut self) -> HelpOverlayAction {
        if self.open {
            self.close_action()
        } else {
            self.open_action()
        }
    }

    /// Flip open/closed.
    #[deprecated(
        since = "0.9.0",
        note = "use `toggle_action`, which returns `HelpOverlayAction` instead of `HelpOverlayEvent`"
    )]
    #[allow(deprecated)] // `HelpOverlayEvent` is `#[deprecated]`; this method is its own shim.
    pub fn toggle(&mut self) -> HelpOverlayEvent {
        match self.toggle_action() {
            HelpOverlayAction::Opened => HelpOverlayEvent::Opened,
            HelpOverlayAction::Closed => HelpOverlayEvent::Closed,
        }
    }

    /// Drive the open/close state machine with a backend-neutral
    /// [`UiEvent`].
    ///
    /// - Closed: `?` opens it (any other event is [`Handled::Ignored`] so
    ///   callers can fall through to their own handling).
    /// - Open: `Escape` or `?` closes it; every other key is
    ///   [`Handled::Consumed`] (modal — nothing leaks through to the view
    ///   underneath, matching [`crate::Palette`]'s "click intercept is
    ///   mandatory" contract for keyboard input).
    pub fn handle_event(&mut self, event: &UiEvent) -> Handled<HelpOverlayAction> {
        let UiEvent::KeyPressed { key, modifiers, .. } = event else {
            return if self.open {
                Handled::Consumed
            } else {
                Handled::Ignored
            };
        };
        // Ignore Ctrl/Alt/Cmd-held combos so this doesn't steal e.g. a
        // hypothetical Ctrl+? or Cmd+? binding from the app.
        let plain = !modifiers.ctrl && !modifiers.alt && !modifiers.cmd;

        if self.open {
            match key {
                Key::Named(NamedKey::Escape) => Handled::Event(self.close_action()),
                Key::Char('?') if plain => Handled::Event(self.close_action()),
                _ => Handled::Consumed,
            }
        } else if plain && matches!(key, Key::Char('?')) {
            Handled::Event(self.open_action())
        } else {
            Handled::Ignored
        }
    }

    /// Drive the open/close state machine with a backend-neutral
    /// [`UiEvent`].
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_event`, which returns `Handled<HelpOverlayAction>` instead of `HelpOverlayEvent`"
    )]
    #[allow(deprecated)] // `HelpOverlayEvent` is `#[deprecated]`; this method is its own shim.
    pub fn handle(&mut self, event: &UiEvent) -> HelpOverlayEvent {
        match self.handle_event(event) {
            Handled::Event(HelpOverlayAction::Opened) => HelpOverlayEvent::Opened,
            Handled::Event(HelpOverlayAction::Closed) => HelpOverlayEvent::Closed,
            Handled::Consumed => HelpOverlayEvent::Consumed,
            Handled::Ignored => HelpOverlayEvent::Ignored,
        }
    }

    /// Paint the cheatsheet centered over `container` if open; a no-op
    /// when closed.
    ///
    /// `help` is `None` when the caller's `HelpRegistry` has no
    /// [`ViewHelp`] registered for the currently active view. This
    /// controller doesn't own the registry (see the type docs), so it
    /// can't refuse to open in that case — instead it paints a minimal
    /// "no help available" panel rather than nothing at all. That
    /// matters because `is_open()` is already `true` by the time
    /// `render` runs (set by `open`/`toggle`/`handle`), and `handle`
    /// unconditionally swallows every key while open (see its docs); if
    /// `render` drew nothing here, the overlay would look "stuck" —
    /// keys vanish with no on-screen indication anything is open — until
    /// the user happens to guess `Escape` or `?` again. Painting
    /// *something* keeps "overlay open" and "overlay visible" the same
    /// fact.
    pub fn render(&self, container: Rect, backend: &mut dyn Backend, help: Option<&ViewHelp>) {
        if !self.open {
            return;
        }
        let title = match help {
            Some(h) => format!("Help — {}  (Esc to close)", h.title),
            None => "Help  (Esc to close)".to_string(),
        };
        let (body, table) = build_help_body_and_table(help);

        let m = backend.measure();
        let line_height = m.line_height;
        let char_width = m.char_width;
        let table_height = table
            .as_ref()
            .map(|t| t.tui_total_height() as f32 * line_height)
            .unwrap_or(0.0);

        let dialog = Dialog {
            id: self.id,
            title: StyledText::plain(title),
            body,
            buttons: Vec::new(),
            severity: None,
            vertical_buttons: false,
            table,
            input: None,
        };

        // 70% of `container`, floored to a readable minimum derived from
        // the backend's own char width (never a hardcoded pixel constant
        // — see `docs/LESSONS.md`), clamped so a narrow `container` never
        // produces an inverted `.max(..).min(..)` range.
        let min_w = (char_width * 44.0).min(container.width.max(1.0));
        let width = (container.width * 0.7)
            .max(min_w)
            .min(container.width.max(1.0));

        let measure = DialogMeasure {
            width,
            title_height: line_height,
            body_height: dialog.body.len() as f32 * line_height,
            table_height,
            input_height: 0.0,
            // No buttons — a cheatsheet is dismiss-only via `Esc`/`?`,
            // handled by `handle_event`, not a button click — so the
            // row reserves no space.
            button_row_height: 0.0,
            button_width: 0.0,
            button_gap: 0.0,
            padding: line_height,
        };
        let layout = dialog.layout(container, measure, |_| ToolbarItemMeasure::new(0.0));
        backend.draw_dialog(&dialog, &layout);
    }
}

impl Default for HelpOverlayController {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the cheatsheet's [`Dialog::body`] (a "Reference" header +
/// `help.notes`, when non-empty) and its [`DialogTable`] (an "Actions"
/// header line followed by a `"Shortcut" | "Description"` table built
/// from `help.actions`, when non-empty).
///
/// The table is the "aligned two-column shortcut table" this cheatsheet
/// renders: `Dialog`'s shared paint auto-sizes each column from the
/// *real measured width* of its widest cell (`PaintSurface::surface_measure_text`),
/// not a fixed character count, so the columns stay aligned under a
/// proportional font — unlike hand-padding a `format!` width, which only
/// lines up on a monospace grid.
///
/// Returns a single-line "no help available" body with no table when
/// `help` is `None` — see [`HelpOverlayController::render`]'s doc for why
/// a missing registration still paints something.
fn build_help_body_and_table(help: Option<&ViewHelp>) -> (Vec<StyledText>, Option<DialogTable>) {
    let Some(help) = help else {
        return (
            vec![StyledText::plain(
                "No help available for this view. (Esc to close)",
            )],
            None,
        );
    };

    let mut body = Vec::new();
    if !help.notes.is_empty() {
        body.push(StyledText::plain("Reference"));
        for note in &help.notes {
            body.push(StyledText::plain(format!(
                "{}  —  {}",
                note.label, note.description
            )));
        }
    }

    let table = if help.actions.is_empty() {
        None
    } else {
        if !body.is_empty() {
            body.push(StyledText::plain(""));
        }
        body.push(StyledText::plain("Actions"));
        Some(DialogTable {
            headers: Some(vec!["Shortcut".to_string(), "Description".to_string()]),
            rows: help
                .actions
                .iter()
                .map(|action| {
                    let shortcut = match &action.accelerator {
                        Some(acc) => format!("{}  ({})", action.label, acc),
                        None => action.label.clone(),
                    };
                    vec![shortcut, action.description.clone()]
                })
                .collect(),
            column_widths: None,
        })
    };

    (body, table)
}

// ── Command-palette integration ─────────────────────────────────────────

/// Convert registered actions into [`PaletteItem`]s for
/// [`crate::Palette`] / [`crate::DualModePaletteController`].
///
/// Each item's primary text is `"{label}  — {description}"` (bold label
/// span + dim description span) so the description is visibly searchable
/// in the list, not just filterable — and `detail` carries the
/// accelerator, matching the convention documented on
/// [`PaletteItem::detail`] ("shortcut" is a named example there).
///
/// `query` is the current filter text (typically the same query passed
/// to [`filter_help_actions`] to produce `actions`); every case-insensitive
/// occurrence of it in the concatenated label+description text is
/// recorded in [`PaletteItem::match_positions`] so backends highlight
/// *why* a row matched, not just that it did. Pass `""` for an
/// unfiltered/initial list — `match_positions` comes back empty, per
/// [`PaletteItem::match_positions`]'s "empty means no highlighting"
/// convention.
///
/// Apps typically call this after filtering with
/// [`filter_help_actions`], mirroring the existing branch-picker demo
/// pattern of recomputing the filtered slice from the query each time
/// (see `examples/common/palette_dual_mode_app.rs`) so a selected index
/// maps back to the same `HelpAction`.
pub fn help_actions_to_palette_items<'a, I>(actions: I, query: &str) -> Vec<PaletteItem>
where
    I: IntoIterator<Item = &'a HelpAction>,
{
    let query_lower = query.to_lowercase();
    actions
        .into_iter()
        .map(|action| {
            let mut spans = vec![StyledSpan {
                bold: true,
                ..StyledSpan::plain(action.label.clone())
            }];
            if !action.description.is_empty() {
                spans.push(StyledSpan::plain(format!("  — {}", action.description)));
            }
            let match_positions = if query_lower.is_empty() {
                Vec::new()
            } else {
                let concatenated: String = spans.iter().map(|s| s.text.as_str()).collect();
                match_byte_positions(&concatenated, &query_lower)
            };
            PaletteItem {
                text: StyledText { spans },
                detail: action
                    .accelerator
                    .as_ref()
                    .map(|a| StyledText::plain(a.clone())),
                icon: None,
                match_positions,
                depth: 0,
                expandable: false,
                expanded: false,
            }
        })
        .collect()
}

/// Byte offsets of every case-insensitive occurrence of `needle_lower`
/// (already lowercased) inside `haystack` — highlights *every* match, not
/// just the first, so a query appearing in both the label and the
/// description span highlights both.
fn match_byte_positions(haystack: &str, needle_lower: &str) -> Vec<usize> {
    if needle_lower.is_empty() {
        return Vec::new();
    }
    let haystack_lower = haystack.to_lowercase();
    let mut positions = Vec::new();
    let mut search_start = 0;
    while search_start <= haystack_lower.len() {
        let Some(found) = haystack_lower[search_start..].find(needle_lower) else {
            break;
        };
        let match_start = search_start + found;
        positions.extend(match_start..match_start + needle_lower.len());
        search_start = match_start + needle_lower.len().max(1);
    }
    positions
}

/// Case-insensitive subsequence fuzzy match against **both** `label` and
/// `description`. An empty `query` matches everything (preserving order),
/// matching [`Palette`]'s "empty query shows everything" convention.
///
/// Uses [`crate::text_util::fuzzy_score`] rather than a plain substring
/// check — a query like `"sv"` matches a label like `"Save"` (s...v as a
/// subsequence) even though `"sv"` never appears contiguously.
pub fn filter_help_actions<'a>(actions: &'a [HelpAction], query: &str) -> Vec<&'a HelpAction> {
    if query.is_empty() {
        return actions.iter().collect();
    }
    let q = query.to_lowercase();
    actions
        .iter()
        .filter(|a| {
            let haystack = format!("{} {}", a.label, a.description).to_lowercase();
            crate::text_util::fuzzy_score(&haystack, &q).is_some()
        })
        .collect()
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modifiers;

    fn key_ev(k: Key) -> UiEvent {
        UiEvent::KeyPressed {
            key: k,
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    fn ctrl_ev(k: Key) -> UiEvent {
        UiEvent::KeyPressed {
            key: k,
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        }
    }

    fn cmd_ev(k: Key) -> UiEvent {
        UiEvent::KeyPressed {
            key: k,
            modifiers: Modifiers {
                cmd: true,
                ..Modifiers::default()
            },
            repeat: false,
        }
    }

    // ── HelpRegistry ─────────────────────────────────────────────────

    #[test]
    fn registry_get_returns_none_for_unknown_view() {
        let reg = HelpRegistry::new();
        assert!(reg.get("nope").is_none());
        assert!(reg.actions_for("nope").is_empty());
    }

    #[test]
    fn registry_register_then_get_roundtrips() {
        let mut reg = HelpRegistry::new();
        let help = ViewHelp::new("Explorer").with_notes(vec![HelpNote::new("●", "modified")]);
        reg.register("panel:explorer", help.clone());
        assert_eq!(reg.get("panel:explorer"), Some(&help));
    }

    #[test]
    fn registry_register_replaces_existing() {
        let mut reg = HelpRegistry::new();
        reg.register("v", ViewHelp::new("First"));
        reg.register("v", ViewHelp::new("Second"));
        assert_eq!(reg.get("v").unwrap().title, "Second");
    }

    #[test]
    fn registry_actions_for_returns_registered_actions() {
        let mut reg = HelpRegistry::new();
        let action = HelpAction::new("a.save", "Save", "Write to disk").with_accelerator("Ctrl+S");
        reg.register("v", ViewHelp::new("V").with_actions(vec![action.clone()]));
        assert_eq!(reg.actions_for("v"), &[action]);
    }

    // ── HelpOverlayController ────────────────────────────────────────

    #[test]
    fn starts_closed() {
        let ctrl = HelpOverlayController::new();
        assert!(!ctrl.is_open());
    }

    #[test]
    fn question_mark_opens_when_closed() {
        let mut ctrl = HelpOverlayController::new();
        let ev = ctrl.handle_event(&key_ev(Key::Char('?')));
        assert_eq!(ev, Handled::Event(HelpOverlayAction::Opened));
        assert!(ctrl.is_open());
    }

    #[test]
    fn other_keys_ignored_when_closed() {
        let mut ctrl = HelpOverlayController::new();
        let ev = ctrl.handle_event(&key_ev(Key::Char('x')));
        assert_eq!(ev, Handled::Ignored);
        assert!(!ctrl.is_open());
    }

    #[test]
    fn escape_closes_when_open() {
        let mut ctrl = HelpOverlayController::new();
        ctrl.open_action();
        let ev = ctrl.handle_event(&key_ev(Key::Named(NamedKey::Escape)));
        assert_eq!(ev, Handled::Event(HelpOverlayAction::Closed));
        assert!(!ctrl.is_open());
    }

    #[test]
    fn question_mark_closes_when_open() {
        let mut ctrl = HelpOverlayController::new();
        ctrl.open_action();
        let ev = ctrl.handle_event(&key_ev(Key::Char('?')));
        assert_eq!(ev, Handled::Event(HelpOverlayAction::Closed));
    }

    #[test]
    fn other_keys_consumed_when_open() {
        let mut ctrl = HelpOverlayController::new();
        ctrl.open_action();
        let ev = ctrl.handle_event(&key_ev(Key::Char('x')));
        assert_eq!(ev, Handled::Consumed);
        assert!(ctrl.is_open(), "unrelated keys must not close the overlay");
    }

    #[test]
    fn ctrl_question_mark_does_not_open() {
        let mut ctrl = HelpOverlayController::new();
        let ev = ctrl.handle_event(&ctrl_ev(Key::Char('?')));
        assert_eq!(ev, Handled::Ignored);
        assert!(!ctrl.is_open());
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend — so it must be excluded the same way, or a hypothetical
    // app-level Cmd+? binding would be stolen by the overlay.
    #[test]
    fn cmd_question_mark_does_not_open() {
        let mut ctrl = HelpOverlayController::new();
        let ev = ctrl.handle_event(&cmd_ev(Key::Char('?')));
        assert_eq!(ev, Handled::Ignored);
        assert!(!ctrl.is_open());
    }

    #[test]
    fn toggle_flips_state() {
        let mut ctrl = HelpOverlayController::new();
        assert_eq!(ctrl.toggle_action(), HelpOverlayAction::Opened);
        assert_eq!(ctrl.toggle_action(), HelpOverlayAction::Closed);
    }

    #[test]
    fn non_key_event_consumed_when_open_ignored_when_closed() {
        let mut ctrl = HelpOverlayController::new();
        let mouse = UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: crate::Point::new(0.0, 0.0),
            modifiers: Modifiers::default(),
        };
        assert_eq!(ctrl.handle_event(&mouse), Handled::Ignored);
        ctrl.open_action();
        assert_eq!(ctrl.handle_event(&mouse), Handled::Consumed);
    }

    #[test]
    #[allow(deprecated)] // covers the `handle`/`open`/`close`/`toggle` shims kept for pre-`Handled` callers.
    fn deprecated_methods_still_adapt_to_help_overlay_event() {
        let mut ctrl = HelpOverlayController::new();
        assert_eq!(ctrl.open(), HelpOverlayEvent::Opened);
        assert_eq!(
            ctrl.handle(&key_ev(Key::Char('x'))),
            HelpOverlayEvent::Consumed
        );
        assert_eq!(ctrl.close(), HelpOverlayEvent::Closed);
        assert_eq!(ctrl.toggle(), HelpOverlayEvent::Opened);
        assert_eq!(
            ctrl.handle(&key_ev(Key::Named(NamedKey::Escape))),
            HelpOverlayEvent::Closed
        );
    }

    // ── build_help_body_and_table ──────────────────────────────────────

    #[test]
    fn help_body_and_table_include_notes_and_actions() {
        let help = ViewHelp::new("Explorer")
            .with_notes(vec![HelpNote::new("●", "modified file")])
            .with_actions(vec![
                HelpAction::new("a.open", "Open", "Open the file").with_accelerator("Enter")
            ]);
        let (body, table) = build_help_body_and_table(Some(&help));
        let joined: String = body
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Reference"));
        assert!(joined.contains("modified file"));
        assert!(joined.contains("Actions"));

        let table = table.expect("actions should produce a shortcut table");
        assert_eq!(
            table.headers,
            Some(vec!["Shortcut".to_string(), "Description".to_string()])
        );
        assert_eq!(table.rows.len(), 1);
        assert!(table.rows[0][0].contains("Open"));
        assert!(table.rows[0][0].contains("Enter"));
        assert!(table.rows[0][1].contains("Open the file"));
    }

    #[test]
    fn help_body_and_table_empty_for_empty_help() {
        let help = ViewHelp::new("Empty");
        let (body, table) = build_help_body_and_table(Some(&help));
        assert!(body.is_empty());
        assert!(table.is_none());
    }

    #[test]
    fn help_body_and_table_none_help_shows_fallback() {
        let (body, table) = build_help_body_and_table(None);
        assert_eq!(body.len(), 1);
        assert!(body[0].spans[0].text.contains("No help available"));
        assert!(table.is_none());
    }

    // ── help_actions_to_palette_items ─────────────────────────────────

    #[test]
    fn palette_items_include_label_description_and_accelerator() {
        let actions = vec![
            HelpAction::new("a.save", "Save", "Write current file to disk")
                .with_accelerator("Ctrl+S"),
        ];
        let items = help_actions_to_palette_items(&actions, "");
        assert_eq!(items.len(), 1);
        let text: String = items[0]
            .text
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert!(text.contains("Save"));
        assert!(text.contains("Write current file to disk"));
        assert_eq!(items[0].detail.as_ref().unwrap().spans[0].text, "Ctrl+S");
    }

    #[test]
    fn palette_items_omit_detail_when_no_accelerator() {
        let actions = vec![HelpAction::new("a.x", "X", "does x")];
        let items = help_actions_to_palette_items(&actions, "");
        assert!(items[0].detail.is_none());
    }

    #[test]
    fn palette_items_match_positions_empty_for_empty_query() {
        let actions = vec![HelpAction::new("a.save", "Save", "Write to disk")];
        let items = help_actions_to_palette_items(&actions, "");
        assert!(items[0].match_positions.is_empty());
    }

    #[test]
    fn palette_items_match_positions_highlight_label_match() {
        let actions = vec![HelpAction::new("a.save", "Save", "Write to disk")];
        let items = help_actions_to_palette_items(&actions, "sav");
        // "Save" is the first span, so the match starts at byte 0.
        assert_eq!(items[0].match_positions, vec![0, 1, 2]);
    }

    #[test]
    fn palette_items_match_positions_highlight_description_match() {
        let actions = vec![HelpAction::new("a.save", "Save", "Write to disk")];
        let items = help_actions_to_palette_items(&actions, "disk");
        let concatenated = "Save  — Write to disk";
        let start = concatenated.find("disk").unwrap();
        assert_eq!(
            items[0].match_positions,
            (start..start + "disk".len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn palette_items_match_positions_case_insensitive() {
        let actions = vec![HelpAction::new("a.save", "Save", "write to disk")];
        let items = help_actions_to_palette_items(&actions, "SAVE");
        assert_eq!(items[0].match_positions, vec![0, 1, 2, 3]);
    }

    #[test]
    fn palette_items_match_positions_multiple_occurrences() {
        let actions = vec![HelpAction::new("a.x", "Cat", "concatenate files")];
        let items = help_actions_to_palette_items(&actions, "cat");
        // "Cat" matches the label, and "concatenate" contains "cat"
        // again later in the description — both should highlight.
        let concatenated_lower = "cat  — concatenate files";
        let expected: Vec<usize> = concatenated_lower
            .match_indices("cat")
            .flat_map(|(i, m)| i..i + m.len())
            .collect();
        assert!(
            expected.len() > 3,
            "sanity check: query should match in two places"
        );
        assert_eq!(items[0].match_positions, expected);
    }

    // ── filter_help_actions ────────────────────────────────────────────

    #[test]
    fn filter_empty_query_returns_all() {
        let actions = vec![
            HelpAction::new("a", "Alpha", "first"),
            HelpAction::new("b", "Beta", "second"),
        ];
        assert_eq!(filter_help_actions(&actions, "").len(), 2);
    }

    #[test]
    fn filter_matches_label() {
        let actions = vec![
            HelpAction::new("a", "Alpha", "first"),
            HelpAction::new("b", "Beta", "second"),
        ];
        let matched = filter_help_actions(&actions, "alp");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].label, "Alpha");
    }

    #[test]
    fn filter_matches_description_when_label_does_not_match() {
        let actions = vec![
            HelpAction::new("a", "Save", "write current file to disk"),
            HelpAction::new("b", "Quit", "exit the application"),
        ];
        let matched = filter_help_actions(&actions, "disk");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].label, "Save");
    }

    #[test]
    fn filter_no_match_returns_empty() {
        let actions = vec![HelpAction::new("a", "Save", "write to disk")];
        assert!(filter_help_actions(&actions, "zzz").is_empty());
    }

    #[test]
    fn filter_matches_non_contiguous_subsequence() {
        // Filter_help_actions moved from substring to subsequence
        // matching. "sv" never appears contiguously in "Save", but s...v
        // is a subsequence of it.
        let actions = vec![
            HelpAction::new("a", "Save", "write current file to disk"),
            HelpAction::new("b", "Quit", "exit the application"),
        ];
        let matched = filter_help_actions(&actions, "sv");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].label, "Save");
    }
}
