//! Reusable context-sensitive help layer for [`ShellApp`](crate::ShellApp)
//! consumers.
//!
//! Three pieces, composed from existing primitives rather than a new one:
//!
//! 1. **[`HelpRegistry`]** — apps register a [`ViewHelp`] (reference notes +
//!    actions) per view/panel id. "What this column/chip means" lives in
//!    [`HelpNote`]; "the available actions" (label + description +
//!    accelerator) live in [`HelpAction`].
//! 2. **[`HelpOverlay`]** — a `?`-triggered cheatsheet overlay
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
//!    silent no-op — see [`HelpOverlay::help`].
//! 3. **[`help_actions_to_palette_items`] / [`filter_help_actions`]** — feed
//!    registered actions into the existing [`crate::Palette`] /
//!    [`crate::DualModePalette`] so they're searchable by label
//!    *and* description.
//!
//! # Cross-backend portability
//!
//! [`HelpOverlay`] sizes the popup from the backend's real
//! measurement — never hardcoded cells/pixels — so the same code
//! paints correctly on every backend. [`Dialog`] classifies as a chrome
//! primitive (see `primitives::dialog`'s module doc), so the cheatsheet
//! renders in the UI font everywhere, including macOS/Win, where the
//! editor font is typically monospace and this content isn't editor
//! content.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::compose::Handled;
use crate::event::Rect;
use crate::interactive::{EventCx, FocusSpec, Interactive, PointerKind, RoutedEvent, WidgetAction};
use crate::primitive::{LayoutCtx, Primitive, Size};
use crate::primitives::dialog::DialogPrimitiveLayout;
use crate::types::{StyledSpan, StyledText, WidgetId};
use crate::{Dialog, DialogTable, Key, NamedKey, PaletteItem, UiEvent};

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

/// The [`HelpOverlay`] widget's action: what [`HelpOverlay::handle_event`]
/// and its `Interactive` report, and what [`HelpOverlay::apply`] writes
/// back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum HelpOverlayAction {
    /// The overlay just opened.
    Opened,
    /// The overlay just closed.
    Closed,
}

impl WidgetAction for HelpOverlayAction {
    /// Rides [`UiEvent::HelpOverlay`] under the overlay's own id; the host
    /// applies it onto [`HelpOverlay::open`].
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::HelpOverlay(id, self)
    }
}

/// The `?`-triggered cheatsheet as a widget: a [`Dialog`] card showing
/// one view's [`ViewHelp`] while [`Self::open`] is set, and nothing while
/// it is not.
///
/// Composed from [`Dialog`] — see the module docs for the card chrome +
/// aligned-table rendering this gets for free. `open` is the host's: the
/// overlay reports [`HelpOverlayAction`] and the host writes it back.
#[derive(Debug, Clone, PartialEq)]
pub struct HelpOverlay {
    /// The overlay's id, also the painted [`Dialog`]'s.
    pub id: WidgetId,
    /// Whether the cheatsheet is showing.
    pub open: bool,
    /// The active view's help, or `None` when the view registered none —
    /// which still paints a minimal "no help available" card, so an open
    /// overlay is always a visible one.
    pub help: Option<ViewHelp>,
}

/// [`Interactive::State`] for [`HelpOverlay`]. The overlay has no
/// interaction state of its own: whether it is open is
/// [`HelpOverlay::open`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HelpOverlayState {}

/// [`HelpOverlay`]'s resolved layout: the rect it was laid out in, and
/// the card's own layout while it is open.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct HelpOverlayLayout {
    /// The rect [`Primitive::layout`] was called with — the container the
    /// card centres itself in.
    pub area: Rect,
    /// The card's layout, `None` while the overlay is closed.
    pub dialog: Option<DialogPrimitiveLayout>,
}

impl HelpOverlay {
    /// A closed overlay with no help content, under `id`.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            open: false,
            help: None,
        }
    }

    /// The card this overlay paints: the help's title, notes and action
    /// table, with no buttons — the cheatsheet is dismiss-only, through
    /// `Esc`/`?`.
    pub fn dialog(&self) -> Dialog {
        let title = match &self.help {
            Some(h) => format!("Help — {}  (Esc to close)", h.title),
            None => "Help  (Esc to close)".to_string(),
        };
        let (body, table) = build_help_body_and_table(self.help.as_ref());
        Dialog {
            id: self.id,
            title: StyledText::plain(title),
            body,
            buttons: Vec::new(),
            severity: None,
            vertical_buttons: false,
            table,
            input: None,
        }
    }

    /// Resolve one backend-neutral event against the overlay: keys
    /// through [`Self::handle_key`]; any other event is
    /// [`Handled::Consumed`] while the overlay is open (it is modal, so
    /// nothing leaks through to the view underneath) and
    /// [`Handled::Ignored`] while it is closed. The host writes a
    /// reported action back with [`Self::apply`].
    pub fn handle_event(&self, event: &UiEvent) -> Handled<HelpOverlayAction> {
        match event {
            UiEvent::KeyPressed { key, modifiers, .. } => self.handle_key(key, modifiers),
            _ if self.open => Handled::Consumed,
            _ => Handled::Ignored,
        }
    }

    /// Write `action` back onto [`Self::open`].
    pub fn apply(&mut self, action: HelpOverlayAction) {
        self.open = action == HelpOverlayAction::Opened;
    }

    /// The action that flips the overlay open or closed from its current
    /// state; the host applies it like any other.
    pub fn toggle_action(&self) -> HelpOverlayAction {
        if self.open {
            HelpOverlayAction::Closed
        } else {
            HelpOverlayAction::Opened
        }
    }

    /// Resolve one keystroke: `?` opens a closed overlay; `Escape` or `?`
    /// closes an open one, and every other key is swallowed while open
    /// (modal — nothing leaks through to the view underneath). Ctrl/Alt/
    /// Cmd-held combos never open or close it, so a `Ctrl+?` binding
    /// stays the app's.
    pub fn handle_key(
        &self,
        key: &Key,
        modifiers: &crate::Modifiers,
    ) -> Handled<HelpOverlayAction> {
        let plain = !modifiers.ctrl && !modifiers.alt && !modifiers.cmd;
        if self.open {
            match key {
                Key::Named(NamedKey::Escape) => Handled::Event(HelpOverlayAction::Closed),
                Key::Char('?') if plain => Handled::Event(HelpOverlayAction::Closed),
                _ => Handled::Consumed,
            }
        } else if plain && matches!(key, Key::Char('?')) {
            Handled::Event(HelpOverlayAction::Opened)
        } else {
            Handled::Ignored
        }
    }
}

impl A11yPrimitive for HelpOverlay {
    fn a11y_info(&self) -> A11yInfo {
        if self.open {
            self.dialog().a11y_info()
        } else {
            A11yInfo::new()
        }
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        if self.open {
            self.dialog().a11y_children()
        } else {
            Vec::new()
        }
    }
}

impl Primitive for HelpOverlay {
    type Layout = HelpOverlayLayout;
    type Event = HelpOverlayAction;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn paint_bleed(&self, style: &crate::Style) -> f32 {
        Primitive::paint_bleed(&self.dialog(), style)
    }

    /// The open card's own size; nothing while closed.
    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        if self.open {
            Primitive::measure(&self.dialog(), ctx, avail)
        } else {
            crate::NaturalSize::NONE
        }
    }

    /// The card centres itself in `rect`, through [`Dialog`]'s own
    /// layout, sized from the backend's real metrics — never hardcoded
    /// cells/pixels.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> HelpOverlayLayout {
        HelpOverlayLayout {
            area: rect,
            dialog: self
                .open
                .then(|| Primitive::layout(&self.dialog(), ctx, rect)),
        }
    }

    fn paint(
        &self,
        layout: &HelpOverlayLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        if let Some(dialog_layout) = &layout.dialog {
            Primitive::paint(&self.dialog(), dialog_layout, surface, env);
        }
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &HelpOverlayLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        if let Some(dialog_layout) = &layout.dialog {
            Primitive::paint_cells(&self.dialog(), dialog_layout, cells, env);
        }
    }
}

impl Interactive for HelpOverlay {
    type State = HelpOverlayState;
    type Action = HelpOverlayAction;

    /// One stop while open, so keys reach the card; none while closed.
    fn focus(&self) -> FocusSpec {
        if self.open {
            FocusSpec::Stop
        } else {
            FocusSpec::None
        }
    }

    /// Keys through [`HelpOverlay::handle_key`]; a press on the open card
    /// is swallowed, since the card is modal.
    fn handle(
        &self,
        _state: &mut HelpOverlayState,
        event: &RoutedEvent,
        layout: &HelpOverlayLayout,
        cx: &mut EventCx,
    ) -> Handled<HelpOverlayAction> {
        match event {
            RoutedEvent::Key(key) => self.handle_key(&key.key, &key.modifiers),
            RoutedEvent::Pointer {
                kind: PointerKind::Down(_),
                local,
                ..
            } => match &layout.dialog {
                Some(dialog) => {
                    let (x, y) = (layout.area.x + local.x, layout.area.y + local.y);
                    if dialog
                        .layout
                        .bounds
                        .contains(crate::event::Point::new(x, y))
                    {
                        cx.request_focus(None);
                        Handled::Consumed
                    } else {
                        Handled::Ignored
                    }
                }
                None => Handled::Ignored,
            },
            _ => Handled::Ignored,
        }
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
/// `help` is `None`: an open overlay is always a visible one, so a
/// missing registration still paints something.
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
/// [`crate::Palette`] / [`crate::DualModePalette`].
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

    // ── HelpOverlay open/close ───────────────────────────────────────

    /// Feed `event` to `overlay` and write any reported action back, the
    /// way a host drives the overlay.
    fn drive(overlay: &mut HelpOverlay, event: &UiEvent) -> Handled<HelpOverlayAction> {
        let handled = overlay.handle_event(event);
        if let Handled::Event(action) = handled {
            overlay.apply(action);
        }
        handled
    }

    fn overlay() -> HelpOverlay {
        HelpOverlay::new(WidgetId::new("help-cheatsheet"))
    }

    fn opened() -> HelpOverlay {
        HelpOverlay {
            open: true,
            ..overlay()
        }
    }

    #[test]
    fn starts_closed() {
        assert!(!overlay().open);
    }

    #[test]
    fn question_mark_opens_when_closed() {
        let mut o = overlay();
        let ev = drive(&mut o, &key_ev(Key::Char('?')));
        assert_eq!(ev, Handled::Event(HelpOverlayAction::Opened));
        assert!(o.open);
    }

    #[test]
    fn other_keys_ignored_when_closed() {
        let mut o = overlay();
        let ev = drive(&mut o, &key_ev(Key::Char('x')));
        assert_eq!(ev, Handled::Ignored);
        assert!(!o.open);
    }

    #[test]
    fn escape_closes_when_open() {
        let mut o = opened();
        let ev = drive(&mut o, &key_ev(Key::Named(NamedKey::Escape)));
        assert_eq!(ev, Handled::Event(HelpOverlayAction::Closed));
        assert!(!o.open);
    }

    #[test]
    fn question_mark_closes_when_open() {
        let mut o = opened();
        let ev = drive(&mut o, &key_ev(Key::Char('?')));
        assert_eq!(ev, Handled::Event(HelpOverlayAction::Closed));
    }

    #[test]
    fn other_keys_consumed_when_open() {
        let mut o = opened();
        let ev = drive(&mut o, &key_ev(Key::Char('x')));
        assert_eq!(ev, Handled::Consumed);
        assert!(o.open, "unrelated keys must not close the overlay");
    }

    #[test]
    fn ctrl_question_mark_does_not_open() {
        let mut o = overlay();
        let ev = drive(&mut o, &ctrl_ev(Key::Char('?')));
        assert_eq!(ev, Handled::Ignored);
        assert!(!o.open);
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend — so it must be excluded the same way, or a hypothetical
    // app-level Cmd+? binding would be stolen by the overlay.
    #[test]
    fn cmd_question_mark_does_not_open() {
        let mut o = overlay();
        let ev = drive(&mut o, &cmd_ev(Key::Char('?')));
        assert_eq!(ev, Handled::Ignored);
        assert!(!o.open);
    }

    #[test]
    fn toggle_action_flips_state() {
        let mut o = overlay();
        assert_eq!(o.toggle_action(), HelpOverlayAction::Opened);
        o.apply(o.toggle_action());
        assert_eq!(o.toggle_action(), HelpOverlayAction::Closed);
    }

    #[test]
    fn non_key_event_consumed_when_open_ignored_when_closed() {
        let mouse = UiEvent::MouseDown {
            widget: None,
            button: crate::MouseButton::Left,
            position: crate::Point::new(0.0, 0.0),
            modifiers: Modifiers::default(),
        };
        assert_eq!(overlay().handle_event(&mouse), Handled::Ignored);
        assert_eq!(opened().handle_event(&mouse), Handled::Consumed);
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
