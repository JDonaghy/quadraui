//! [`route_palette_host`] — the keys a widget hosting a modal
//! [`crate::Palette`] claims before the palette's own input sees them.
//!
//! The filesystem pickers ([`crate::compose::PathPicker`]) and
//! [`crate::compose::DualModePalette`] all compose a palette and resolve
//! the same prelude first: Escape dismisses, Enter confirms, and — only
//! when the host opts into vim keys — an unmodified `k`/`j` moves the
//! highlight instead of typing. Everything the prelude does not claim is
//! the palette's own query editing and list navigation.

use crate::interactive::RoutedEvent;
use crate::{Key, NamedKey};

/// A keystroke [`route_palette_host`] claimed for the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaletteHostKey {
    /// Escape: dismiss the palette.
    Cancel,
    /// Enter: confirm what the palette shows.
    Confirm,
    /// `k` with vim keys: move the highlight up one row.
    Up,
    /// `j` with vim keys: move the highlight down one row.
    Down,
}

/// The prelude key `event` names, or `None` when the palette's own input
/// should handle it.
///
/// Escape and Enter are claimed whatever modifiers are held. `k`/`j` are
/// claimed only with `vim_keys` and no Ctrl/Cmd/Alt, so with vim keys off
/// both letters type into the query like any other character.
pub(crate) fn route_palette_host(event: &RoutedEvent, vim_keys: bool) -> Option<PaletteHostKey> {
    let RoutedEvent::Key(key) = event else {
        return None;
    };
    let modified = key.modifiers.ctrl || key.modifiers.cmd || key.modifiers.alt;
    match key.key {
        Key::Named(NamedKey::Escape) => Some(PaletteHostKey::Cancel),
        Key::Named(NamedKey::Enter) => Some(PaletteHostKey::Confirm),
        Key::Char('k') if vim_keys && !modified => Some(PaletteHostKey::Up),
        Key::Char('j') if vim_keys && !modified => Some(PaletteHostKey::Down),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::KeyEvent;
    use crate::Modifiers;

    fn key(key: Key, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key,
            modifiers,
            repeat: false,
        })
    }

    #[test]
    fn escape_and_enter_are_claimed_with_or_without_vim_keys() {
        for vim in [false, true] {
            let esc = key(Key::Named(NamedKey::Escape), Modifiers::default());
            let enter = key(Key::Named(NamedKey::Enter), Modifiers::default());
            assert_eq!(route_palette_host(&esc, vim), Some(PaletteHostKey::Cancel));
            assert_eq!(
                route_palette_host(&enter, vim),
                Some(PaletteHostKey::Confirm)
            );
        }
    }

    #[test]
    fn j_and_k_move_only_with_vim_keys_and_no_modifier() {
        let j = key(Key::Char('j'), Modifiers::default());
        let k = key(Key::Char('k'), Modifiers::default());
        assert_eq!(route_palette_host(&j, true), Some(PaletteHostKey::Down));
        assert_eq!(route_palette_host(&k, true), Some(PaletteHostKey::Up));
        assert_eq!(route_palette_host(&j, false), None);
        assert_eq!(route_palette_host(&k, false), None);
        let ctrl_j = key(
            Key::Char('j'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        assert_eq!(route_palette_host(&ctrl_j, true), None);
    }

    #[test]
    fn other_keys_and_non_key_events_are_left_to_the_palette() {
        let a = key(Key::Char('a'), Modifiers::default());
        assert_eq!(route_palette_host(&a, true), None);
        assert_eq!(
            route_palette_host(&RoutedEvent::Text("x".into()), true),
            None
        );
    }
}
