// ─────────────────────────────────────────────────────────────────────
// map_arboard_error
// ─────────────────────────────────────────────────────────────────────

/// Map an `arboard::Error` from a named native call into a
/// [`crate::backend::BackendError::PlatformFailure`]. `arboard::Error`
/// has no "unsupported" variant of its own — every arm this maps
/// (including `ContentNotAvailable`, e.g. "clipboard has no image right
/// now") is a real outcome of a call the calling backend *does*
/// implement, so `PlatformFailure` — not
/// [`crate::backend::BackendError::Unsupported`] — is the honest
/// mapping; see `BackendError::Unsupported`'s own doc for why that
/// variant is reserved for "this backend has no implementation" instead.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
pub(crate) fn map_arboard_error(call: &str, err: arboard::Error) -> crate::backend::BackendError {
    crate::backend::BackendError::PlatformFailure {
        context: format!("{call}: {err}"),
    }
}

#[cfg(all(
    test,
    any(feature = "gtk", all(feature = "macos", target_os = "macos"))
))]
mod map_arboard_error_tests {
    use super::*;

    #[test]
    fn wraps_the_call_name_and_error_into_a_platform_failure() {
        let err = map_arboard_error("arboard::get_image", arboard::Error::ContentNotAvailable);
        match err {
            crate::backend::BackendError::PlatformFailure { context } => {
                assert!(context.contains("arboard::get_image"));
                assert!(context.contains(&arboard::Error::ContentNotAvailable.to_string()));
            }
            other => panic!("expected PlatformFailure, got {other:?}"),
        }
    }
}
