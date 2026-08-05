//! Ordered, scope-aware key binding table (#473).
//!
//! [`Backend::register_accelerator`][crate::Backend::register_accelerator]
//! only resolves `AcceleratorScope::Global` bindings — the backend doesn't
//! know which widget has focus or which app-defined mode is active, so
//! [`TuiBackend::apply_accelerators`](crate::tui::TuiBackend) explicitly
//! skips `Widget`/`Mode`-scoped entries and leaves them as raw
//! `KeyPressed` events. Consumers that want those scopes have
//! historically hand-rolled the guard themselves — repeating a predicate
//! like `!pty_active && !any_blocking_modal_active() && …` across every
//! `Key::Char` match arm instead of declaring the scope once.
//!
//! [`KeyMap`] is that missing resolver. Apps declare an ordered table of
//! `(scope, binding, action id)` once; at each `KeyPressed` event they
//! call [`KeyMap::resolve`] with a [`KeyContext`] describing *this
//! frame's* focus/mode/blocked state, and get back the one
//! [`AcceleratorId`] (if any) that should fire — no per-arm guard
//! clauses, no scope logic duplicated at every call site.
//!
//! # Ordering = priority
//!
//! Entries are tried in registration order; the first entry whose key +
//! modifiers match **and** whose scope is currently active wins. This is
//! the "fallthrough" the table provides: register the narrow, specific
//! binding first (e.g. `Widget(palette_id)` → `"palette.close"` for
//! `Escape`) and the broad fallback after (e.g. `Global` → `"app.blur"`
//! for the same `Escape`). When the palette has focus the first entry's
//! scope is active and wins; otherwise resolution falls through to the
//! next entry that matches the key.
//!
//! # Relationship to `Accelerator` / `Backend::register_accelerator`
//!
//! `KeyMap` is backend-agnostic and doesn't require registering anything
//! with a `Backend` — it resolves directly against raw `Key` +
//! `Modifiers`, so it works identically whether the native `KeyPressed`
//! event came through unmodified or already had `Global` accelerators
//! peeled off upstream by the backend. [`KeyMap::bind_accelerator`] lets
//! apps build the table straight from the same [`Accelerator`] values they
//! already declare for `register_accelerator`, so the two paths share one
//! source of truth instead of two.

use crate::accelerator::{key_to_binding_name, parse_binding};
use crate::event::Key;
use crate::types::{Modifiers, WidgetId};
use crate::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding, ParsedBinding};

/// One resolved row of the table: a scope, its parsed key binding, and
/// the action it fires. Kept private — apps interact with the table
/// through [`KeyMap::bind`] / [`KeyMap::resolve`], never this struct
/// directly.
#[derive(Debug, Clone)]
struct KeyMapEntry {
    scope: AcceleratorScope,
    binding: ParsedBinding,
    action: AcceleratorId,
}

/// An ordered `(AcceleratorScope, ParsedBinding, AcceleratorId)` table.
/// See the module docs for the resolution model.
#[derive(Debug, Clone, Default)]
pub struct KeyMap {
    entries: Vec<KeyMapEntry>,
}

impl KeyMap {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `binding` at `scope` for `action`. Later calls are lower
    /// priority — see the module docs' "Ordering = priority" section.
    ///
    /// Silently skips unparseable `KeyBinding::Literal` strings, mirroring
    /// [`Backend::register_accelerator`][crate::Backend::register_accelerator]'s
    /// contract (an unparseable binding never matches rather than
    /// panicking or erroring).
    pub fn bind(
        &mut self,
        scope: AcceleratorScope,
        binding: KeyBinding,
        action: impl Into<AcceleratorId>,
    ) -> &mut Self {
        if let Some(parsed) = parse_binding(&binding) {
            self.entries.push(KeyMapEntry {
                scope,
                binding: parsed,
                action: action.into(),
            });
        }
        self
    }

    /// Register an already-declared [`Accelerator`] — the same value an
    /// app hands to `Backend::register_accelerator`, so a `Global`-scoped
    /// accelerator and its `KeyMap` entry never drift apart. See the
    /// module docs' "Relationship to `Accelerator`" section.
    pub fn bind_accelerator(&mut self, acc: &Accelerator) -> &mut Self {
        if let Some(parsed) = parse_binding(&acc.binding) {
            self.entries.push(KeyMapEntry {
                scope: acc.scope.clone(),
                binding: parsed,
                action: acc.id.clone(),
            });
        }
        self
    }

    /// Remove every entry bound to `action`, regardless of scope/binding.
    pub fn unbind(&mut self, action: &AcceleratorId) {
        self.entries.retain(|e| &e.action != action);
    }

    /// Number of registered entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve a native key press to the highest-priority matching action
    /// whose scope is active in `ctx`, or `None` if nothing matches.
    ///
    /// Apps typically call this from their `UiEvent::KeyPressed` arm
    /// before falling back to widget-local handling — a `Some` result
    /// means "dispatch this one action id"; a `None` result means "no
    /// declared binding claims this key, handle it (or ignore it) as
    /// before".
    pub fn resolve(
        &self,
        key: &Key,
        modifiers: Modifiers,
        ctx: &KeyContext,
    ) -> Option<AcceleratorId> {
        let key_name = key_to_binding_name(key);
        self.entries
            .iter()
            .find(|e| {
                e.binding.modifiers == modifiers
                    && e.binding.key == key_name
                    && ctx.scope_active(&e.scope)
            })
            .map(|e| e.action.clone())
    }
}

/// Per-frame state [`KeyMap::resolve`] evaluates a candidate entry's
/// [`AcceleratorScope`] against.
///
/// Cheap to construct fresh on every `KeyPressed` event — it borrows the
/// caller's own focus/mode bookkeeping rather than owning a copy.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyContext<'a> {
    /// Every [`WidgetId`] that currently "has focus" for the purpose of
    /// `AcceleratorScope::Widget` matching — the focused widget itself
    /// plus any ancestors that should also claim its widget-scoped
    /// bindings (e.g. a modal's own id, so its `Escape` binding fires
    /// no matter which child control inside it is focused). An
    /// `AcceleratorScope::Widget(id)` entry is active whenever `id`
    /// appears anywhere in this slice — order doesn't matter.
    pub focus_chain: &'a [WidgetId],
    /// The app-defined mode string (vim-like apps: `"n"`, `"i"`, `"v"`,
    /// …). `AcceleratorScope::Mode(m)` is active when this equals
    /// `Some(m)`.
    pub mode: Option<&'a str>,
    /// When `true`, `AcceleratorScope::Global` entries never match —
    /// the single flag that replaces the hand-repeated
    /// `!pty_active && !any_blocking_modal_active() && …` guard chain
    /// this type was filed to eliminate. Set it whenever *anything*
    /// should suppress global shortcuts (a blocking modal is open, a
    /// PTY has raw input focus, …); `Widget`/`Mode`-scoped entries are
    /// unaffected since their own scope already gates them precisely.
    pub global_blocked: bool,
}

impl<'a> KeyContext<'a> {
    /// A context with no focus chain, no mode, and `Global` unblocked.
    pub fn new() -> Self {
        Self {
            focus_chain: &[],
            mode: None,
            global_blocked: false,
        }
    }

    /// Set the focus chain (see [`Self::focus_chain`]).
    pub fn with_focus_chain(mut self, chain: &'a [WidgetId]) -> Self {
        self.focus_chain = chain;
        self
    }

    /// Set the active mode string (see [`Self::mode`]).
    pub fn with_mode(mut self, mode: &'a str) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Mark `Global`-scope entries as blocked for this resolution (see
    /// [`Self::global_blocked`]).
    pub fn blocked(mut self) -> Self {
        self.global_blocked = true;
        self
    }

    fn scope_active(&self, scope: &AcceleratorScope) -> bool {
        match scope {
            AcceleratorScope::Global => !self.global_blocked,
            AcceleratorScope::Widget(id) => self.focus_chain.contains(id),
            AcceleratorScope::Mode(m) => self.mode == Some(m.as_str()),
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mods_none() -> Modifiers {
        Modifiers::default()
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn empty_map_resolves_nothing() {
        let map = KeyMap::new();
        assert!(map.is_empty());
        assert_eq!(
            map.resolve(&Key::Char('p'), ctrl(), &KeyContext::new()),
            None
        );
    }

    #[test]
    fn global_scope_matches_by_default() {
        let mut map = KeyMap::new();
        map.bind(
            AcceleratorScope::Global,
            KeyBinding::Literal("Ctrl+P".into()),
            "palette.open",
        );
        assert_eq!(map.len(), 1);
        let id = map
            .resolve(&Key::Char('p'), ctrl(), &KeyContext::new())
            .unwrap();
        assert_eq!(id.as_str(), "palette.open");
    }

    #[test]
    fn global_scope_blocked_does_not_match() {
        let mut map = KeyMap::new();
        map.bind(
            AcceleratorScope::Global,
            KeyBinding::Literal("Ctrl+P".into()),
            "palette.open",
        );
        let ctx = KeyContext::new().blocked();
        assert_eq!(map.resolve(&Key::Char('p'), ctrl(), &ctx), None);
    }

    #[test]
    fn widget_scope_matches_when_in_focus_chain() {
        let mut map = KeyMap::new();
        let modal_id = WidgetId::new("modal:confirm");
        map.bind(
            AcceleratorScope::Widget(modal_id.clone()),
            KeyBinding::Literal("Escape".into()),
            "modal.close",
        );
        let chain = [WidgetId::new("modal:confirm:input"), modal_id.clone()];
        let ctx = KeyContext::new().with_focus_chain(&chain);
        let id = map
            .resolve(&Key::Named(crate::NamedKey::Escape), mods_none(), &ctx)
            .unwrap();
        assert_eq!(id.as_str(), "modal.close");
    }

    #[test]
    fn widget_scope_does_not_match_when_absent_from_focus_chain() {
        let mut map = KeyMap::new();
        map.bind(
            AcceleratorScope::Widget(WidgetId::new("modal:confirm")),
            KeyBinding::Literal("Escape".into()),
            "modal.close",
        );
        let chain = [WidgetId::new("editor:main")];
        let ctx = KeyContext::new().with_focus_chain(&chain);
        assert_eq!(
            map.resolve(&Key::Named(crate::NamedKey::Escape), mods_none(), &ctx),
            None
        );
    }

    #[test]
    fn mode_scope_matches_active_mode_only() {
        let mut map = KeyMap::new();
        map.bind(
            AcceleratorScope::Mode("n".into()),
            KeyBinding::Literal("d".into()),
            "vim.delete_line",
        );
        let ctx_normal = KeyContext::new().with_mode("n");
        assert_eq!(
            map.resolve(&Key::Char('d'), mods_none(), &ctx_normal)
                .as_ref()
                .map(AcceleratorId::as_str),
            Some("vim.delete_line")
        );

        let ctx_insert = KeyContext::new().with_mode("i");
        assert_eq!(map.resolve(&Key::Char('d'), mods_none(), &ctx_insert), None);
    }

    #[test]
    fn earlier_binding_wins_when_both_scopes_active() {
        let mut map = KeyMap::new();
        let palette_id = WidgetId::new("palette");
        map.bind(
            AcceleratorScope::Widget(palette_id.clone()),
            KeyBinding::Literal("Escape".into()),
            "palette.close",
        );
        map.bind(
            AcceleratorScope::Global,
            KeyBinding::Literal("Escape".into()),
            "app.blur",
        );

        let chain = [palette_id.clone()];
        let ctx_focused = KeyContext::new().with_focus_chain(&chain);
        assert_eq!(
            map.resolve(
                &Key::Named(crate::NamedKey::Escape),
                mods_none(),
                &ctx_focused
            )
            .as_ref()
            .map(AcceleratorId::as_str),
            Some("palette.close")
        );
    }

    #[test]
    fn falls_through_to_next_entry_when_first_scope_inactive() {
        let mut map = KeyMap::new();
        let palette_id = WidgetId::new("palette");
        map.bind(
            AcceleratorScope::Widget(palette_id),
            KeyBinding::Literal("Escape".into()),
            "palette.close",
        );
        map.bind(
            AcceleratorScope::Global,
            KeyBinding::Literal("Escape".into()),
            "app.blur",
        );

        // Palette not focused — falls through to the Global fallback.
        let ctx = KeyContext::new();
        assert_eq!(
            map.resolve(&Key::Named(crate::NamedKey::Escape), mods_none(), &ctx)
                .as_ref()
                .map(AcceleratorId::as_str),
            Some("app.blur")
        );
    }

    #[test]
    fn bind_accelerator_shares_scope_and_id() {
        let mut map = KeyMap::new();
        let acc = Accelerator {
            id: AcceleratorId::new("editor.save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        };
        map.bind_accelerator(&acc);
        let id = map
            .resolve(&Key::Char('s'), ctrl(), &KeyContext::new())
            .unwrap();
        assert_eq!(id, acc.id);
    }

    #[test]
    fn unparseable_literal_is_skipped_silently() {
        let mut map = KeyMap::new();
        map.bind(
            AcceleratorScope::Global,
            KeyBinding::Literal("".into()),
            "noop",
        );
        assert!(map.is_empty());
    }

    #[test]
    fn unbind_removes_all_entries_for_action() {
        let mut map = KeyMap::new();
        map.bind(
            AcceleratorScope::Global,
            KeyBinding::Literal("Ctrl+P".into()),
            "palette.open",
        );
        map.bind(
            AcceleratorScope::Mode("n".into()),
            KeyBinding::Literal("Ctrl+P".into()),
            "palette.open",
        );
        assert_eq!(map.len(), 2);
        map.unbind(&AcceleratorId::new("palette.open"));
        assert!(map.is_empty());
    }

    #[test]
    fn modifier_mismatch_does_not_match() {
        let mut map = KeyMap::new();
        map.bind(
            AcceleratorScope::Global,
            KeyBinding::Literal("Ctrl+P".into()),
            "palette.open",
        );
        assert_eq!(
            map.resolve(&Key::Char('p'), mods_none(), &KeyContext::new()),
            None
        );
    }
}
