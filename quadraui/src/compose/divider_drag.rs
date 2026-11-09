//! Shared "is a divider currently being dragged" state, generic over the
//! payload a drag site needs to remember between its start and its
//! subsequent move/end events.
//!
//! Pattern of [`crate::primitives::scrollbar::ThumbDrag`], generalised to a
//! bare optional payload: unlike a scrollbar thumb, the dividers composed by
//! [`crate::compose::AppShell`] (sidebar resize offset, bottom-panel resize
//! offset) and [`crate::compose::TabGroupController`] (active split index)
//! need no shared offset-to-value math of their own — only the
//! `begin`/`get`/`end` sequencing those three drags each reimplemented
//! independently as a bare `Option<T>` field.

/// `T` is the per-site payload remembered while a divider drag is in
/// progress: an `f32` pixel offset between the press and the divider's own
/// position for [`crate::compose::AppShell`]'s two line-dragged chrome
/// dividers, a `usize` split index for
/// [`crate::compose::TabGroupController`]'s N-way `Split` dividers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DividerDrag<T>(Option<T>);

impl<T: Copy> DividerDrag<T> {
    /// No drag in progress.
    pub(crate) const fn none() -> Self {
        Self(None)
    }

    /// Arm a drag with `state`, overwriting any previous (uncommitted) one.
    pub(crate) fn begin(&mut self, state: T) {
        self.0 = Some(state);
    }

    /// The remembered payload, if a drag is currently in progress.
    pub(crate) fn get(&self) -> Option<T> {
        self.0
    }

    /// Whether a drag is currently in progress. Test-only today: no
    /// production caller needs it directly (every site either reads
    /// [`Self::get`] for the payload or calls [`Self::end`] to both check
    /// and clear it at once), but it's handy for asserting a drag site's
    /// drag-in-progress state in a test without reaching into the
    /// `Option` the type wraps.
    #[cfg(test)]
    pub(crate) fn is_dragging(&self) -> bool {
        self.0.is_some()
    }

    /// End the drag if one was in progress, discarding the remembered
    /// payload. Returns whether a drag was actually in progress.
    pub(crate) fn end(&mut self) -> bool {
        self.0.take().is_some()
    }
}

impl<T> Default for DividerDrag<T> {
    fn default() -> Self {
        Self(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_no_drag() {
        let d: DividerDrag<f32> = DividerDrag::none();
        assert!(!d.is_dragging());
        assert_eq!(d.get(), None);
    }

    #[test]
    fn begin_then_get_round_trips_the_payload() {
        let mut d = DividerDrag::none();
        d.begin(3.5_f32);
        assert!(d.is_dragging());
        assert_eq!(d.get(), Some(3.5));
    }

    #[test]
    fn begin_overwrites_a_previous_uncommitted_drag() {
        let mut d = DividerDrag::none();
        d.begin(1_usize);
        d.begin(2_usize);
        assert_eq!(d.get(), Some(2));
    }

    #[test]
    fn end_clears_and_reports_whether_a_drag_was_in_progress() {
        let mut d = DividerDrag::none();
        assert!(!d.end());
        d.begin(7_i32);
        assert!(d.end());
        assert!(!d.is_dragging());
    }

    #[test]
    fn default_is_no_drag() {
        let d: DividerDrag<f32> = DividerDrag::default();
        assert!(!d.is_dragging());
    }
}
