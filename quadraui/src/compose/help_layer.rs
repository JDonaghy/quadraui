//! Reusable context-sensitive help layer for [`ShellApp`](crate::ShellApp)
//! consumers (#431).
//!
//! Three pieces, composed from existing primitives rather than a new one:
//!
//! 1. **[`HelpRegistry`]** — apps register a [`ViewHelp`] (reference notes +
//!    actions) per view/panel id. "What this column/chip means" lives in
//!    [`HelpNote`]; "the available actions" (label + description +
//!    accelerator) live in [`HelpAction`].
//! 2. **[`HelpOverlayController`]** — a `?`-triggered cheatsheet overlay
//!    rendering the active view's [`ViewHelp`]. Composed from
//!    [`crate::Panel`] (chrome: title bar + border — `Backend::draw_panel`
//!    is implemented on every backend today) framing a
//!    [`crate::TextDisplay`] (content: the note/action list). Views with
//!    no registered help still get a visible (if minimal) cheatsheet
//!    rather than a silent no-op — see [`HelpOverlayController::render`].
//! 3. **[`help_actions_to_palette_items`] / [`filter_help_actions`]** — feed
//!    registered actions into the existing [`crate::Palette`] /
//!    [`crate::DualModePaletteController`] so they're searchable by label
//!    *and* description.
//!
//! # Why not the old `Modal` primitive?
//!
//! quadraui used to also ship a `Modal` primitive describing backdrop +
//! centered-content geometry — but it had no `Backend::draw_modal`
//! method and no backend ever painted one, so this overlay was built on
//! [`crate::Panel`] instead, which already ships a working chrome
//! rasteriser on every backend: same "bordered popup framing app-drawn
//! content" shape, backed by a primitive that actually renders. `Modal`
//! itself was deleted as dead API in #509 (zero consumers in-repo or in
//! either downstream consumer, and no rasteriser ever arrived to give
//! it one) — see `docs/DECISIONS.md`.
//!
//! # Cross-backend portability
//!
//! [`HelpOverlayController::render`] sizes the popup from
//! `backend.line_height()` / `backend.char_width()` — never hardcoded
//! cells/pixels — so the same code paints correctly on TUI and GTK (see
//! `quadraui/docs/LESSONS.md`).

use std::collections::BTreeMap;

use crate::event::Rect;
use crate::types::{Decoration, StyledSpan, StyledText, WidgetId};
use crate::{Backend, Key, NamedKey, PaletteItem, Panel, TextDisplay, TextDisplayLine, UiEvent};

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

/// Result of [`HelpOverlayController::handle`] / `open` / `close` /
/// `toggle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
/// overlay. Wraps a [`Panel`] (chrome) + [`TextDisplay`] (content) —
/// see the module docs for why not the old `Modal` primitive.
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

    /// Override the `WidgetId` used for the rendered [`Panel`].
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = WidgetId::new(id.into());
        self
    }

    /// Whether the cheatsheet is currently showing.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the overlay unconditionally.
    pub fn open(&mut self) -> HelpOverlayEvent {
        self.open = true;
        HelpOverlayEvent::Opened
    }

    /// Close the overlay unconditionally.
    pub fn close(&mut self) -> HelpOverlayEvent {
        self.open = false;
        HelpOverlayEvent::Closed
    }

    /// Flip open/closed.
    pub fn toggle(&mut self) -> HelpOverlayEvent {
        if self.open {
            self.close()
        } else {
            self.open()
        }
    }

    /// Drive the open/close state machine with a backend-neutral
    /// [`UiEvent`].
    ///
    /// - Closed: `?` opens it (any other event is [`HelpOverlayEvent::Ignored`]
    ///   so callers can fall through to their own handling).
    /// - Open: `Escape` or `?` closes it; every other key is
    ///   [`HelpOverlayEvent::Consumed`] (modal — nothing leaks through to
    ///   the view underneath, matching [`crate::Palette`]'s "click
    ///   intercept is mandatory" contract for keyboard input).
    pub fn handle(&mut self, event: &UiEvent) -> HelpOverlayEvent {
        let UiEvent::KeyPressed { key, modifiers, .. } = event else {
            return if self.open {
                HelpOverlayEvent::Consumed
            } else {
                HelpOverlayEvent::Ignored
            };
        };
        // Ignore Ctrl/Alt-held combos so this doesn't steal e.g. a
        // hypothetical Ctrl+? binding from the app.
        let plain = !modifiers.ctrl && !modifiers.alt;

        if self.open {
            match key {
                Key::Named(NamedKey::Escape) => self.close(),
                Key::Char('?') if plain => self.close(),
                _ => HelpOverlayEvent::Consumed,
            }
        } else if plain && matches!(key, Key::Char('?')) {
            self.open()
        } else {
            HelpOverlayEvent::Ignored
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
        let rect = Self::popup_rect(container, backend);
        let title = match help {
            Some(h) => format!("Help — {}  (Esc to close)", h.title),
            None => "Help  (Esc to close)".to_string(),
        };
        let panel = Panel {
            id: self.id.clone(),
            title: Some(StyledText::plain(title)),
            actions: vec![],
            accent: None,
            collapsed: false,
        };
        let layout = backend.draw_panel(rect, &panel);
        let lines = match help {
            Some(h) => build_cheatsheet_lines(h),
            None => vec![TextDisplayLine {
                spans: vec![StyledSpan::plain(
                    "No help available for this view. (Esc to close)",
                )],
                decoration: Decoration::Normal,
                timestamp: None,
            }],
        };
        let td = TextDisplay {
            id: WidgetId::new(format!("{}-content", self.id.as_str())),
            lines,
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        backend.draw_text_display(layout.content_bounds, &td);
    }

    /// Popup geometry: 70% of `container`, floored to a readable minimum
    /// derived from the backend's own line height / char width (never a
    /// hardcoded cell/pixel constant — see `docs/LESSONS.md`).
    fn popup_rect(container: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        let cw = backend.char_width();
        let w = (container.width * 0.7).max(cw * 44.0).min(container.width);
        let h = (container.height * 0.7)
            .max(lh * 10.0)
            .min(container.height);
        let x = container.x + (container.width - w) * 0.5;
        let y = container.y + (container.height - h) * 0.5;
        Rect::new(x, y, w, h)
    }
}

impl Default for HelpOverlayController {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the cheatsheet body: a "Reference" section from `help.notes`
/// (when non-empty) followed by an "Actions" section from `help.actions`
/// (when non-empty), each entry hand-aligned into columns via padded
/// `format!` widths.
///
/// That padding is fixed-*character-count*, so it lines up exactly on a
/// monospace TUI grid — but `quadraui::gtk::text_display` renders
/// `TextDisplay` spans through Pango with the proportional UI font
/// (`"Sans 11"` by default; see `gtk/backend.rs`), where two
/// equal-character-count strings can render to different pixel widths.
/// The accelerator/description columns are therefore ragged on GTK
/// today. Accepted for now — it's visual-only and GTK example coverage
/// waits on `GtkDriver` (#301) — but a real fix would render each column
/// as its own `StyledSpan` sized from a backend-measured text width
/// instead of character count.
fn build_cheatsheet_lines(help: &ViewHelp) -> Vec<TextDisplayLine> {
    let mut lines = Vec::new();

    if !help.notes.is_empty() {
        lines.push(header_line("Reference"));
        for note in &help.notes {
            lines.push(entry_line(&note.label, None, &note.description));
        }
    }

    if !help.actions.is_empty() {
        if !lines.is_empty() {
            lines.push(blank_line());
        }
        lines.push(header_line("Actions"));
        for action in &help.actions {
            lines.push(entry_line(
                &action.label,
                action.accelerator.as_deref(),
                &action.description,
            ));
        }
    }

    lines
}

fn header_line(text: &str) -> TextDisplayLine {
    TextDisplayLine {
        spans: vec![StyledSpan {
            bold: true,
            ..StyledSpan::plain(text)
        }],
        decoration: Decoration::Header,
        timestamp: None,
    }
}

fn blank_line() -> TextDisplayLine {
    TextDisplayLine {
        spans: vec![StyledSpan::plain("")],
        decoration: Decoration::Normal,
        timestamp: None,
    }
}

fn entry_line(label: &str, accelerator: Option<&str>, description: &str) -> TextDisplayLine {
    let text = format!(
        "  {:<20}{:<14}{}",
        label,
        accelerator.unwrap_or(""),
        description
    );
    TextDisplayLine {
        spans: vec![StyledSpan::plain(text)],
        decoration: Decoration::Normal,
        timestamp: None,
    }
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
/// `description` — the "searchable with descriptions" half of #431's
/// acceptance bar. An empty `query` matches everything (preserving
/// order), matching [`Palette`]'s "empty query shows everything"
/// convention.
///
/// Uses [`crate::text_util::fuzzy_score`] (#474) rather than a plain
/// substring check — a query like `"sv"` now matches a label like
/// `"Save"` (s...v as a subsequence) even though `"sv"` never appears
/// contiguously. Matching order among non-empty-query results is
/// unchanged (original registration order, not re-ranked by score) —
/// only *which* actions pass the filter changes.
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
        let ev = ctrl.handle(&key_ev(Key::Char('?')));
        assert_eq!(ev, HelpOverlayEvent::Opened);
        assert!(ctrl.is_open());
    }

    #[test]
    fn other_keys_ignored_when_closed() {
        let mut ctrl = HelpOverlayController::new();
        let ev = ctrl.handle(&key_ev(Key::Char('x')));
        assert_eq!(ev, HelpOverlayEvent::Ignored);
        assert!(!ctrl.is_open());
    }

    #[test]
    fn escape_closes_when_open() {
        let mut ctrl = HelpOverlayController::new();
        ctrl.open();
        let ev = ctrl.handle(&key_ev(Key::Named(NamedKey::Escape)));
        assert_eq!(ev, HelpOverlayEvent::Closed);
        assert!(!ctrl.is_open());
    }

    #[test]
    fn question_mark_closes_when_open() {
        let mut ctrl = HelpOverlayController::new();
        ctrl.open();
        let ev = ctrl.handle(&key_ev(Key::Char('?')));
        assert_eq!(ev, HelpOverlayEvent::Closed);
    }

    #[test]
    fn other_keys_consumed_when_open() {
        let mut ctrl = HelpOverlayController::new();
        ctrl.open();
        let ev = ctrl.handle(&key_ev(Key::Char('x')));
        assert_eq!(ev, HelpOverlayEvent::Consumed);
        assert!(ctrl.is_open(), "unrelated keys must not close the overlay");
    }

    #[test]
    fn ctrl_question_mark_does_not_open() {
        let mut ctrl = HelpOverlayController::new();
        let ev = ctrl.handle(&ctrl_ev(Key::Char('?')));
        assert_eq!(ev, HelpOverlayEvent::Ignored);
        assert!(!ctrl.is_open());
    }

    #[test]
    fn toggle_flips_state() {
        let mut ctrl = HelpOverlayController::new();
        assert_eq!(ctrl.toggle(), HelpOverlayEvent::Opened);
        assert_eq!(ctrl.toggle(), HelpOverlayEvent::Closed);
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
        assert_eq!(ctrl.handle(&mouse), HelpOverlayEvent::Ignored);
        ctrl.open();
        assert_eq!(ctrl.handle(&mouse), HelpOverlayEvent::Consumed);
    }

    // ── build_cheatsheet_lines ────────────────────────────────────────

    #[test]
    fn cheatsheet_lines_include_notes_and_actions() {
        let help = ViewHelp::new("Explorer")
            .with_notes(vec![HelpNote::new("●", "modified file")])
            .with_actions(vec![
                HelpAction::new("a.open", "Open", "Open the file").with_accelerator("Enter")
            ]);
        let lines = build_cheatsheet_lines(&help);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Reference"));
        assert!(joined.contains("modified file"));
        assert!(joined.contains("Actions"));
        assert!(joined.contains("Open"));
        assert!(joined.contains("Open the file"));
        assert!(joined.contains("Enter"));
    }

    #[test]
    fn cheatsheet_lines_empty_for_empty_help() {
        let help = ViewHelp::new("Empty");
        assert!(build_cheatsheet_lines(&help).is_empty());
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
        // #474: filter_help_actions moved from substring to subsequence
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
