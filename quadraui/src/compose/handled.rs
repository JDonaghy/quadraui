//! Shared "did you handle it" contract for compose controllers.
//!
//! [`Handled<E>`] is the one shape a `handle_*` method returns: a semantic
//! payload to route further ([`Handled::Event`]), a state-only consumption
//! that just needs a redraw ([`Handled::Consumed`]), or "not mine"
//! ([`Handled::Ignored`]).
//!
//! [`crate::compose::ContextMenuOutcome`] has the same three-way shape
//! under its own name, since it isn't deprecated.

/// Outcome of a compose controller's `handle_*` method.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handled<E> {
    /// A semantic event the caller must route further (e.g. activate a
    /// tab, close a panel). Always the terminal, most-specific outcome —
    /// never paired with a redraw hint, since routing the event already
    /// implies the caller redraws if it decides to.
    Event(E),
    /// The event changed internal state (hover, selection, a press
    /// started) with nothing further to route. The caller should redraw.
    Consumed,
    /// The event wasn't relevant to this controller at all.
    Ignored,
}

impl<E> Handled<E> {
    /// `true` for [`Handled::Ignored`].
    pub fn is_ignored(&self) -> bool {
        matches!(self, Handled::Ignored)
    }

    /// `true` for [`Handled::Consumed`].
    pub fn is_consumed(&self) -> bool {
        matches!(self, Handled::Consumed)
    }

    /// `true` for [`Handled::Event`].
    pub fn is_event(&self) -> bool {
        matches!(self, Handled::Event(_))
    }

    /// Extract the payload, if this is [`Handled::Event`].
    pub fn into_event(self) -> Option<E> {
        match self {
            Handled::Event(e) => Some(e),
            Handled::Consumed | Handled::Ignored => None,
        }
    }

    /// Map the payload of [`Handled::Event`], leaving `Consumed`/`Ignored`
    /// unchanged.
    pub fn map<F, T>(self, f: F) -> Handled<T>
    where
        F: FnOnce(E) -> T,
    {
        match self {
            Handled::Event(e) => Handled::Event(f(e)),
            Handled::Consumed => Handled::Consumed,
            Handled::Ignored => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_event_roundtrips() {
        assert_eq!(Handled::Event(42).into_event(), Some(42));
        assert_eq!(Handled::<i32>::Consumed.into_event(), None);
        assert_eq!(Handled::<i32>::Ignored.into_event(), None);
    }

    #[test]
    fn map_only_touches_event() {
        assert_eq!(Handled::Event(1).map(|n| n + 1), Handled::Event(2));
        assert_eq!(Handled::<i32>::Consumed.map(|n| n + 1), Handled::Consumed);
        assert_eq!(Handled::<i32>::Ignored.map(|n| n + 1), Handled::Ignored);
    }
}
