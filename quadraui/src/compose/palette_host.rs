//! The key prelude every widget that hosts a [`Palette`] routes through:
//! [`DualModePalette`](crate::compose::DualModePalette) and the path
//! pickers ([`FilePicker`](crate::compose::FilePicker),
//! [`FolderPicker`](crate::compose::FolderPicker)).
//!
//! A palette host decides for itself what Escape and Enter mean, and may
//! opt into vim-style `j`/`k` highlight moves; everything else is the
//! palette's own input — query editing and list navigation. This module
//! resolves one routed event into the [`PaletteHostInput`] it names, so
//! every host interprets the same keys the same way and only maps the
//! result onto its own action type.

use crate::behaviour::nav::{FlatNav, NavKey};
use crate::compose::Handled;
use crate::interactive::{EventCx, Interactive, RoutedEvent};
use crate::primitives::palette::{Palette, PaletteEvent, PalettePrimitiveLayout, PaletteState};
use crate::{Key, NamedKey};

/// What one routed event means to a palette host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PaletteHostInput {
    /// Escape, or the palette dismissed itself.
    Cancel,
    /// Enter (`None`), or the palette confirmed row `idx` itself
    /// (`Some(idx)`, e.g. a double-click).
    Confirm(Option<usize>),
    /// Move the highlight to row `idx`.
    Select(usize),
    /// The query text is now `value`.
    QueryChanged(String),
}

/// The row a vim-style `k` (`up`) or `j` moves the highlight to in a
/// `len`-row list highlighting `selected`, clamped at either end.
fn vim_row(len: usize, selected: usize, up: bool) -> usize {
    let key = if up { NavKey::Up } else { NavKey::Down };
    FlatNav::mv(len, Some(selected), key, false, |_| true).unwrap_or(selected)
}

/// Resolve one routed event for a host painting `palette`, whose list has
/// `len` rows with row `selected` highlighted.
///
/// Escape and Enter are the host's ([`PaletteHostInput::Cancel`],
/// [`PaletteHostInput::Confirm`]`(None)`). With `vim_keys`, an unmodified
/// `k`/`j` moves the highlight instead of typing. Every other event goes
/// to the palette's own [`Interactive`], and the change it reports is
/// handed to `answer`; an event the palette consumes without a change is
/// [`Handled::Consumed`], one it ignores [`Handled::Ignored`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn route_palette_host<A>(
    palette: &Palette,
    state: &mut PaletteState,
    event: &RoutedEvent,
    layout: &PalettePrimitiveLayout,
    cx: &mut EventCx,
    (len, selected): (usize, usize),
    vim_keys: bool,
    answer: impl FnOnce(PaletteHostInput) -> Handled<A>,
) -> Handled<A> {
    if let RoutedEvent::Key(key) = event {
        let modified = key.modifiers.ctrl || key.modifiers.cmd || key.modifiers.alt;
        match key.key {
            Key::Named(NamedKey::Escape) => return answer(PaletteHostInput::Cancel),
            Key::Named(NamedKey::Enter) => return answer(PaletteHostInput::Confirm(None)),
            Key::Char(c @ ('k' | 'j')) if vim_keys && !modified => {
                return answer(PaletteHostInput::Select(vim_row(len, selected, c == 'k')));
            }
            _ => {}
        }
    }
    match Interactive::handle(palette, state, event, layout, cx) {
        Handled::Event(PaletteEvent::QueryChanged { value }) => {
            answer(PaletteHostInput::QueryChanged(value))
        }
        Handled::Event(PaletteEvent::SelectionChanged { idx }) => {
            answer(PaletteHostInput::Select(idx))
        }
        Handled::Event(PaletteEvent::ItemConfirmed { idx }) => {
            answer(PaletteHostInput::Confirm(Some(idx)))
        }
        Handled::Event(PaletteEvent::Closed) => answer(PaletteHostInput::Cancel),
        Handled::Event(_) | Handled::Consumed => Handled::Consumed,
        Handled::Ignored => Handled::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vim_rows_clamp_at_both_ends() {
        assert_eq!(vim_row(3, 0, true), 0);
        assert_eq!(vim_row(3, 1, true), 0);
        assert_eq!(vim_row(3, 1, false), 2);
        assert_eq!(vim_row(3, 2, false), 2);
        assert_eq!(vim_row(0, 0, false), 0);
    }
}
