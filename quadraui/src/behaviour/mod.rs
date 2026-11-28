//! Shared keyboard/scroll/typeahead behaviour every widget's `Interactive`
//! impl builds from, instead of re-deriving the same arithmetic.
//!
//! [`crate::interactive::Interactive`] defines the *contract* a widget's
//! behaviour implements; this module holds the actual reusable pieces that
//! contract is built out of:
//!
//! - [`scroll`] — the "keep the selected/focused item inside the viewport"
//!   formula, in row-quantised (`usize`) and pixel-precise (`f32`) forms.
//! - [`nav`] — [`nav::FlatNav`] for a flat, index-addressed list (clamped or
//!   wrapping, with a disabled-item skip predicate) and
//!   [`nav::TreeNavCore`]/[`nav::NavRow`] for a flattened tree-shaped row
//!   list addressed by path, operating on a `&[impl NavRow]` view the
//!   caller passes in rather than owning any row content itself.
//!   [`nav::NavKey`] is the one place a raw keystroke becomes a
//!   navigation intent.
//! - [`typeahead`] — buffered, debounced "type letters to jump to a
//!   matching item" behaviour.
//! - [`row_window`] — a windowed row collection's band, its rows-requested
//!   check and the [`row_window::HostLatch`] uncontrolled-state rule.
//! - [`text_edit`] — the one text-editing core (caret, selection, word
//!   motion, buffer mutation) every editable text widget shares.
//!
//! Every function here is pure arithmetic over caller-supplied state (an
//! offset, a `len`, a `current` index, a row/path list passed by
//! reference) — nothing in this module owns widget content or holds a
//! native dependency, so it compiles and runs identically on every
//! backend.

pub mod nav;
pub mod row_window;
pub mod scroll;
pub mod text_edit;
pub mod typeahead;
