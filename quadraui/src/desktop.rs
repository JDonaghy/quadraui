//! Backward-compatible facade over [`crate::platform`].
//!
//! [`crate::platform`] holds the actual backend-neutral desktop/OS-service
//! plumbing (window-drag arm/threshold/commit, modal-pump re-entrancy
//! guard, trash, the default-handler opener, notifications, clipboard
//! error mapping, headless smoke-mode predicates, the paste-keypress
//! predicate, the `PointerShape` enum-walk scaffold, caught-panic
//! reporting), split by concern into one file per submodule. This module
//! re-exports every one of those `pub(crate)` items at its historical
//! `crate::desktop::X` path, so every existing call site across
//! `gtk`/`macos`/`win`/`tui` keeps compiling unchanged. See
//! [`crate::platform`]'s own module doc for the implementation and the
//! rationale behind each piece.
pub(crate) use crate::platform::*;
