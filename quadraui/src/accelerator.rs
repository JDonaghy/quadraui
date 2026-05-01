//! Declarative cross-platform keybindings.
//!
//! Apps declare `Accelerator`s and register them with the active
//! [`Backend`][crate::Backend]. The backend watches native key events, matches
//! them to the registered accelerators, and emits
//! [`UiEvent::Accelerator`][crate::UiEvent::Accelerator] when one fires.
//! Apps dispatch on [`AcceleratorId`] without parsing key strings themselves.
//!
//! ## Platform idiom parity
//!
//! Universal bindings ([`KeyBinding::Save`], [`KeyBinding::Copy`], etc.)
//! render as `⌘S` on macOS and `Ctrl+S` elsewhere — the app doesn't care.
//! App-specific bindings use [`KeyBinding::Literal`], which accepts both
//! vim-style (`<C-S-t>`) and plus-style (`Ctrl+Shift+T`) input.
//!
//! See `quadraui/docs/BACKEND_TRAIT_PROPOSAL.md` §3 for the full rationale.

use serde::{Deserialize, Serialize};

use crate::types::{Modifiers, WidgetId};

/// Stable identifier for a registered accelerator. Apps match on this in the
/// [`UiEvent::Accelerator`][crate::UiEvent::Accelerator] arm of their
/// dispatcher.
///
/// Conventionally namespaced (`"editor.save"`, `"plugin:my-ext:send"`) so
/// plugin accelerators don't collide with app accelerators.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AcceleratorId(pub String);

impl AcceleratorId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for AcceleratorId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for AcceleratorId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// When an [`Accelerator`] should fire relative to app state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcceleratorScope {
    /// Fires regardless of what's focused. E.g. "Ctrl+P" for the palette.
    Global,
    /// Fires only when the given widget (or its descendants) have focus.
    /// Used for widget-local shortcuts like Esc inside a picker.
    Widget(WidgetId),
    /// Fires only when the given mode is active. For vim-like apps: `"n"`
    /// = Normal mode, `"i"` = Insert, `"v"` = Visual. Apps define the
    /// mode strings.
    Mode(String),
}

/// The key combination to bind.
///
/// Universal variants render platform-appropriately (e.g. `⌘S` on macOS);
/// [`KeyBinding::Literal`] is for app-specific bindings that don't have a
/// universal name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyBinding {
    // ── Universal accelerators (render platform-appropriately) ─────────
    Save,
    Open,
    New,
    Close,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    SelectAll,
    Find,
    Replace,
    Quit,

    /// App-specific binding. Parser accepts **both** vim-style
    /// (`<C-S-t>`) and plus-style (`Ctrl+Shift+T`) input; first character
    /// determines which parser runs. See
    /// [`parse_key_binding`] for the grammar.
    Literal(String),
}

/// A declared accelerator: its stable ID, the keys that trigger it, the
/// scope in which it's active, and an optional display label for menus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accelerator {
    pub id: AcceleratorId,
    pub binding: KeyBinding,
    pub scope: AcceleratorScope,
    /// Human-readable label for menus, tooltips, help. `None` means the
    /// backend derives one from the binding (e.g. "Ctrl+Shift+T").
    pub label: Option<String>,
}

// ─── Parsing ────────────────────────────────────────────────────────────────

/// Parsed form of a [`KeyBinding::Literal`] string. Backends match native
/// key events against this struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParsedBinding {
    pub modifiers: Modifiers,
    /// Lowercased character for letters; uppercase named key (`Enter`,
    /// `F1`, `Escape`) for non-printables.
    pub key: String,
}

/// Parse a `KeyBinding::Literal` string. Accepts two formats:
///
/// - **Vim-style**: `<C-S-t>`, `<C-A-Left>`, `<F5>`. Starts with `<`.
///   Modifier letters: `C` = Ctrl, `S` = Shift, `A` = Alt, `D`/`M` =
///   Cmd/Super.
/// - **Plus-style**: `Ctrl+Shift+T`, `Cmd+S`, `Alt+F4`. Case-insensitive
///   modifiers.
///
/// Format is detected from the first character — `<` means vim-style,
/// anything else means plus-style. Single-character bindings without
/// separators (e.g. `"a"`, `"Escape"`) parse as plus-style with no
/// modifiers.
///
/// Returns `None` on unparseable input.
pub fn parse_key_binding(s: &str) -> Option<ParsedBinding> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with('<') {
        parse_vim_style(s)
    } else {
        parse_plus_style(s)
    }
}

/// Vim-style: `<C-S-t>`, `<C-A-Left>`, `<F5>`, `<Escape>`.
fn parse_vim_style(s: &str) -> Option<ParsedBinding> {
    let inner = s.strip_prefix('<')?.strip_suffix('>')?;
    if inner.is_empty() {
        return None;
    }
    let parts: Vec<&str> = inner.split('-').collect();
    // Last part is the key; earlier parts are modifier letters.
    let (key_part, mod_parts) = parts.split_last()?;
    let mut modifiers = Modifiers::default();
    for m in mod_parts {
        match *m {
            "C" => modifiers.ctrl = true,
            "S" => modifiers.shift = true,
            "A" => modifiers.alt = true,
            "D" | "M" => modifiers.cmd = true,
            _ => return None,
        }
    }
    Some(ParsedBinding {
        modifiers,
        key: normalise_key_name(key_part),
    })
}

/// Plus-style: `Ctrl+Shift+T`, `Cmd+S`, `Alt+F4`, `Escape`.
fn parse_plus_style(s: &str) -> Option<ParsedBinding> {
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let mut modifiers = Modifiers::default();
    let mut key: Option<String> = None;
    for p in &parts {
        let lower = p.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" | "option" | "opt" => modifiers.alt = true,
            "cmd" | "command" | "super" | "win" | "meta" => modifiers.cmd = true,
            _ => {
                if key.is_some() {
                    // More than one non-modifier token → malformed.
                    return None;
                }
                key = Some(normalise_key_name(p));
            }
        }
    }
    Some(ParsedBinding {
        modifiers,
        key: key?,
    })
}

/// Normalise a key name to the canonical form used by backends.
///
/// - Single letters → lowercase (`T` → `t`).
/// - Named keys → TitleCase preserved (`Escape`, `F5`, `Left`).
fn normalise_key_name(s: &str) -> String {
    if s.chars().count() == 1 {
        s.to_ascii_lowercase()
    } else {
        s.to_string()
    }
}

// ─── Rendering ──────────────────────────────────────────────────────────────

/// Render an [`Accelerator`] for display in a menu, tooltip, or help overlay.
///
/// Always platform-appropriate: `⌘⇧T` on macOS, `Ctrl+Shift+T` on
/// Win/Linux/TUI — regardless of which input format the app used.
pub fn render_accelerator(acc: &Accelerator, platform: Platform) -> String {
    if let Some(ref label) = acc.label {
        return label.clone();
    }
    render_binding(&acc.binding, platform)
}

/// Render just the [`KeyBinding`] portion.
pub fn render_binding(b: &KeyBinding, platform: Platform) -> String {
    match b {
        KeyBinding::Save => platform.fmt_mod_letter("Ctrl", "S"),
        KeyBinding::Open => platform.fmt_mod_letter("Ctrl", "O"),
        KeyBinding::New => platform.fmt_mod_letter("Ctrl", "N"),
        KeyBinding::Close => platform.fmt_mod_letter("Ctrl", "W"),
        KeyBinding::Copy => platform.fmt_mod_letter("Ctrl", "C"),
        KeyBinding::Cut => platform.fmt_mod_letter("Ctrl", "X"),
        KeyBinding::Paste => platform.fmt_mod_letter("Ctrl", "V"),
        KeyBinding::Undo => platform.fmt_mod_letter("Ctrl", "Z"),
        KeyBinding::Redo => platform.fmt_mod_shift_letter("Ctrl", "Z"),
        KeyBinding::SelectAll => platform.fmt_mod_letter("Ctrl", "A"),
        KeyBinding::Find => platform.fmt_mod_letter("Ctrl", "F"),
        KeyBinding::Replace => platform.fmt_mod_letter("Ctrl", "H"),
        KeyBinding::Quit => platform.fmt_mod_letter("Ctrl", "Q"),
        KeyBinding::Literal(s) => render_literal(s, platform),
    }
}

fn render_literal(s: &str, platform: Platform) -> String {
    match parse_key_binding(s) {
        Some(p) => platform.fmt_parsed(&p),
        // Unparseable literal — show the raw string rather than hide it.
        None => s.to_string(),
    }
}

/// Platform identity for rendering. Backends return their own variant from
/// [`PlatformServices::platform_name`][crate::PlatformServices::platform_name].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Macos,
    Windows,
    Linux,
    Tui,
}

impl Platform {
    fn fmt_mod_letter(self, modifier: &str, letter: &str) -> String {
        match self {
            Platform::Macos => format!("{}{}", self.cmd_glyph(), letter),
            _ => format!("{}+{}", modifier, letter),
        }
    }

    fn fmt_mod_shift_letter(self, modifier: &str, letter: &str) -> String {
        match self {
            Platform::Macos => format!("{}⇧{}", self.cmd_glyph(), letter),
            _ => format!("{}+Shift+{}", modifier, letter),
        }
    }

    fn fmt_parsed(self, p: &ParsedBinding) -> String {
        match self {
            Platform::Macos => {
                let mut s = String::new();
                if p.modifiers.ctrl {
                    s.push('⌃');
                }
                if p.modifiers.alt {
                    s.push('⌥');
                }
                if p.modifiers.shift {
                    s.push('⇧');
                }
                if p.modifiers.cmd {
                    s.push('⌘');
                }
                s.push_str(&display_key(&p.key));
                s
            }
            _ => {
                let mut parts: Vec<&str> = Vec::new();
                if p.modifiers.ctrl {
                    parts.push("Ctrl");
                }
                if p.modifiers.alt {
                    parts.push("Alt");
                }
                if p.modifiers.shift {
                    parts.push("Shift");
                }
                if p.modifiers.cmd {
                    parts.push("Super");
                }
                let key_disp = display_key(&p.key);
                parts.push(&key_disp);
                parts.join("+")
            }
        }
    }

    fn cmd_glyph(self) -> &'static str {
        match self {
            Platform::Macos => "⌘",
            _ => "",
        }
    }
}

fn display_key(k: &str) -> String {
    if k.chars().count() == 1 {
        k.to_ascii_uppercase()
    } else {
        k.to_string()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, shift: bool, alt: bool, cmd: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            cmd,
        }
    }

    #[test]
    fn parse_vim_style_basic() {
        let p = parse_key_binding("<C-S-t>").unwrap();
        assert_eq!(p.modifiers, mods(true, true, false, false));
        assert_eq!(p.key, "t");
    }

    #[test]
    fn parse_vim_style_named_key() {
        let p = parse_key_binding("<C-A-Left>").unwrap();
        assert_eq!(p.modifiers, mods(true, false, true, false));
        assert_eq!(p.key, "Left");
    }

    #[test]
    fn parse_vim_style_no_modifiers() {
        let p = parse_key_binding("<F5>").unwrap();
        assert_eq!(p.modifiers, Modifiers::default());
        assert_eq!(p.key, "F5");
    }

    #[test]
    fn parse_vim_style_cmd_via_d_modifier() {
        let p = parse_key_binding("<D-s>").unwrap();
        assert_eq!(p.modifiers, mods(false, false, false, true));
    }

    #[test]
    fn parse_vim_style_rejects_unknown_modifier() {
        assert!(parse_key_binding("<X-t>").is_none());
    }

    #[test]
    fn parse_vim_style_rejects_unclosed() {
        assert!(parse_key_binding("<C-t").is_none());
    }

    #[test]
    fn parse_plus_style_basic() {
        let p = parse_key_binding("Ctrl+Shift+T").unwrap();
        assert_eq!(p.modifiers, mods(true, true, false, false));
        assert_eq!(p.key, "t");
    }

    #[test]
    fn parse_plus_style_case_insensitive_modifiers() {
        let p = parse_key_binding("ctrl+SHIFT+t").unwrap();
        assert_eq!(p.modifiers, mods(true, true, false, false));
        assert_eq!(p.key, "t");
    }

    #[test]
    fn parse_plus_style_cmd_aliases() {
        for input in ["Cmd+S", "Command+S", "Super+S", "Win+S", "Meta+S"] {
            let p = parse_key_binding(input).unwrap_or_else(|| panic!("failed to parse {input}"));
            assert_eq!(p.modifiers, mods(false, false, false, true));
            assert_eq!(p.key, "s", "input was {input}");
        }
    }

    #[test]
    fn parse_plus_style_alt_aliases() {
        for input in ["Alt+F4", "Option+F4", "Opt+F4"] {
            let p = parse_key_binding(input).unwrap();
            assert_eq!(p.modifiers, mods(false, false, true, false));
            assert_eq!(p.key, "F4");
        }
    }

    #[test]
    fn parse_plus_style_bare_key() {
        let p = parse_key_binding("Escape").unwrap();
        assert_eq!(p.modifiers, Modifiers::default());
        assert_eq!(p.key, "Escape");

        let p = parse_key_binding("a").unwrap();
        assert_eq!(p.key, "a");
    }

    #[test]
    fn parse_plus_style_rejects_multiple_keys() {
        assert!(parse_key_binding("Ctrl+A+B").is_none());
    }

    #[test]
    fn parse_plus_style_rejects_empty_parts() {
        assert!(parse_key_binding("Ctrl++T").is_none());
        assert!(parse_key_binding("+T").is_none());
    }

    #[test]
    fn parse_empty_or_whitespace() {
        assert!(parse_key_binding("").is_none());
        assert!(parse_key_binding("   ").is_none());
    }

    #[test]
    fn parse_round_trip_vim_and_plus_agree() {
        let vim = parse_key_binding("<C-S-t>").unwrap();
        let plus = parse_key_binding("Ctrl+Shift+T").unwrap();
        assert_eq!(vim, plus);
    }

    #[test]
    fn render_save_macos_vs_other() {
        let acc = Accelerator {
            id: AcceleratorId::new("app.save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        };
        assert_eq!(render_accelerator(&acc, Platform::Macos), "⌘S");
        assert_eq!(render_accelerator(&acc, Platform::Windows), "Ctrl+S");
        assert_eq!(render_accelerator(&acc, Platform::Linux), "Ctrl+S");
        assert_eq!(render_accelerator(&acc, Platform::Tui), "Ctrl+S");
    }

    #[test]
    fn render_redo_uses_shift_on_every_platform() {
        let b = KeyBinding::Redo;
        assert_eq!(render_binding(&b, Platform::Macos), "⌘⇧Z");
        assert_eq!(render_binding(&b, Platform::Linux), "Ctrl+Shift+Z");
    }

    #[test]
    fn render_literal_parses_and_formats() {
        let b = KeyBinding::Literal("<C-S-t>".into());
        assert_eq!(render_binding(&b, Platform::Linux), "Ctrl+Shift+T");
        assert_eq!(render_binding(&b, Platform::Macos), "⌃⇧T");
    }

    #[test]
    fn render_literal_plus_style_round_trips() {
        let b = KeyBinding::Literal("Ctrl+Shift+T".into());
        assert_eq!(render_binding(&b, Platform::Linux), "Ctrl+Shift+T");
    }

    #[test]
    fn render_literal_unparseable_falls_back_to_raw() {
        let b = KeyBinding::Literal("gibberish".into());
        assert_eq!(render_binding(&b, Platform::Linux), "gibberish");
    }

    #[test]
    fn render_uses_explicit_label_when_set() {
        let acc = Accelerator {
            id: AcceleratorId::new("custom"),
            binding: KeyBinding::Literal("<C-S-t>".into()),
            scope: AcceleratorScope::Global,
            label: Some("Secret handshake".into()),
        };
        assert_eq!(
            render_accelerator(&acc, Platform::Linux),
            "Secret handshake"
        );
    }

    #[test]
    fn accelerator_serde_round_trip() {
        let acc = Accelerator {
            id: AcceleratorId::new("editor.save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        };
        let json = serde_json::to_string(&acc).unwrap();
        let back: Accelerator = serde_json::from_str(&json).unwrap();
        assert_eq!(acc, back);

        let acc = Accelerator {
            id: AcceleratorId::new("terminal.maximize"),
            binding: KeyBinding::Literal("<C-S-t>".into()),
            scope: AcceleratorScope::Mode("normal".into()),
            label: Some("Maximize terminal".into()),
        };
        let json = serde_json::to_string(&acc).unwrap();
        let back: Accelerator = serde_json::from_str(&json).unwrap();
        assert_eq!(acc, back);
    }
}
