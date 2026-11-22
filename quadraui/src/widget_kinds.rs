//! Decode a wire-shaped `(kind, value)` pair — a widget kind string plus
//! its JSON-shaped descriptor payload — into a boxed, type-erased
//! [`crate::primitive::DynWidget`], without the caller needing to know
//! which concrete `Primitive` type that kind names.
//!
//! This is the seam a binding layer (`quadraui-binding-core`'s
//! `quadraui-py`/`quadraui-node` render callbacks, or any other
//! out-of-process producer of widget descriptors as JSON) decodes
//! against: it has a string naming the widget kind and a
//! `serde_json::Value` holding that widget's own fields, and needs a
//! `Box<dyn DynWidget>` it can push onto a [`crate::frame_tree::FrameTree`]
//! or [`crate::frame::Surface::Widget`] — the same shape every built-in
//! `Primitive` descriptor already produces by value.
//!
//! [`widget_kinds!`] declares the `kind` string → descriptor type table
//! and generates [`decode_widget`] from it: one `match` arm per entry,
//! each deserializing `value` into that entry's own type via
//! [`serde::de::DeserializeOwned`] and boxing the result. This module
//! invokes it once, seeded with the two kinds [`crate::CellGrid`] and
//! [`crate::IconGrid`] — every other built-in primitive, and any kind a
//! future `quadraui-binding-core` wire layer needs, grows this same
//! table rather than hand-writing another decode path.
//!
//! Only entries whose type implements [`crate::primitive::DynWidget`] —
//! which every [`crate::interactive::Interactive`] widget gets for free
//! via that trait's blanket impl — type-check here, so a `widget_kinds!`
//! entry can never produce a value the rest of the widget-contract
//! routing engine doesn't already know how to layout/paint/route events
//! to.

/// Error produced by [`decode_widget`]: either `kind` has no matching
/// [`widget_kinds!`] entry, or it does but `value` failed to deserialize
/// into that entry's own type.
#[derive(Debug)]
pub enum KindError {
    /// No [`widget_kinds!`] entry matches this kind string.
    Unknown(String),
    /// `kind` matched a [`widget_kinds!`] entry, but `value` failed to
    /// deserialize into that entry's own type — the wrapped
    /// [`serde_json::Error`] carries the field/shape mismatch.
    Decode {
        kind: String,
        source: serde_json::Error,
    },
}

impl std::fmt::Display for KindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KindError::Unknown(kind) => write!(f, "unknown widget kind {kind:?}"),
            KindError::Decode { kind, source } => {
                write!(f, "failed to decode widget kind {kind:?}: {source}")
            }
        }
    }
}

impl std::error::Error for KindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KindError::Unknown(_) => None,
            KindError::Decode { source, .. } => Some(source),
        }
    }
}

/// Declare the `kind` string → descriptor type table [`decode_widget`]
/// matches against, generating `decode_widget` itself in the invoking
/// module.
///
/// Each `$ty` must implement [`serde::de::DeserializeOwned`] and
/// [`crate::primitive::DynWidget`] — every built-in `Primitive` that also
/// implements [`crate::interactive::Interactive`] gets the latter for
/// free via that trait's blanket impl (see [`crate::primitive::DynWidget`]'s
/// own doc).
///
/// ```
/// use quadraui::widget_kinds;
///
/// widget_kinds! {
///     "cell_grid" => quadraui::CellGrid,
///     "icon_grid" => quadraui::IconGrid,
/// }
///
/// let payload = serde_json::json!({
///     "id": "sheet",
///     "row_count": 1,
///     "col_count": 1,
///     "default_row_height": 1.0,
///     "default_col_width": 8.0,
/// });
/// let widget = decode_widget("cell_grid", payload).unwrap();
/// assert_eq!(widget.id().map(|id| id.as_str()), Some("sheet"));
/// ```
#[macro_export]
macro_rules! widget_kinds {
    ( $( $kind:literal => $ty:ty ),* $(,)? ) => {
        /// Decode `value` into the boxed, type-erased widget `kind`
        /// names — see the `widget_kinds` module doc and the
        /// `widget_kinds!` macro invocation that generated this
        /// function for the full `kind` → type table.
        pub fn decode_widget(
            kind: &str,
            value: ::serde_json::Value,
        ) -> ::std::result::Result<
            ::std::boxed::Box<dyn $crate::primitive::DynWidget>,
            $crate::widget_kinds::KindError,
        > {
            match kind {
                $(
                    $kind => {
                        let widget: $ty = ::serde_json::from_value(value).map_err(|source| {
                            $crate::widget_kinds::KindError::Decode {
                                kind: kind.to_string(),
                                source,
                            }
                        })?;
                        ::std::result::Result::Ok(::std::boxed::Box::new(widget))
                    }
                )*
                other => ::std::result::Result::Err(
                    $crate::widget_kinds::KindError::Unknown(other.to_string())
                ),
            }
        }
    };
}

widget_kinds! {
    "cell_grid" => crate::CellGrid,
    "icon_grid" => crate::IconGrid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_known_kind() {
        let payload = serde_json::json!({
            "id": "sheet",
            "row_count": 10,
            "col_count": 5,
            "default_row_height": 1.0,
            "default_col_width": 8.0,
        });
        let widget = decode_widget("cell_grid", payload).expect("cell_grid decodes");
        assert_eq!(widget.id().map(|id| id.as_str()), Some("sheet"));
    }

    #[test]
    fn decodes_the_other_known_kind() {
        // `IconGrid` doesn't override `Primitive::id` (its id lives on
        // each `IconGridItem`, not the grid itself), so `DynWidget::id`
        // falls back to the trait's own default of `None` — decoding
        // without error is this kind's own round-trip signal.
        let payload = serde_json::json!({
            "id": "icons",
            "items": [],
        });
        let widget = decode_widget("icon_grid", payload).expect("icon_grid decodes");
        assert_eq!(widget.id(), None);
    }

    #[test]
    fn rejects_an_unknown_kind() {
        // `Box<dyn DynWidget>` isn't `Debug`, so `Result::unwrap_err`
        // doesn't type-check here — match the `Result` directly instead.
        let payload = serde_json::Value::Null;
        match decode_widget("not_a_real_kind", payload) {
            Err(KindError::Unknown(kind)) => assert_eq!(kind, "not_a_real_kind"),
            other => panic!("expected Err(KindError::Unknown), got {}", describe(other)),
        }
    }

    #[test]
    fn rejects_a_malformed_payload_for_a_known_kind() {
        // `items` must be an array — a string value has no sensible
        // decode and should surface as `KindError::Decode`, not panic.
        let payload = serde_json::json!({
            "id": "icons",
            "items": "not-an-array",
        });
        match decode_widget("icon_grid", payload) {
            Err(KindError::Decode { kind, .. }) => assert_eq!(kind, "icon_grid"),
            other => panic!("expected Err(KindError::Decode), got {}", describe(other)),
        }
    }

    #[test]
    fn kind_error_display_names_the_kind() {
        let unknown = KindError::Unknown("mystery".to_string());
        assert!(unknown.to_string().contains("mystery"));

        let payload = serde_json::json!({"id": "icons", "items": "nope"});
        let decode_err = match decode_widget("icon_grid", payload) {
            Err(err) => err,
            Ok(_) => panic!("expected a decode error"),
        };
        assert!(decode_err.to_string().contains("icon_grid"));
        assert!(std::error::Error::source(&decode_err).is_some());
    }

    /// `Box<dyn DynWidget>` isn't `Debug`, so a `panic!("{other:?}")` on
    /// the `Ok` arm of a `decode_widget` result doesn't type-check —
    /// this renders just enough of it (or the real `KindError`) for a
    /// test failure message.
    fn describe(result: Result<Box<dyn crate::primitive::DynWidget>, KindError>) -> String {
        match result {
            Ok(widget) => format!("Ok(widget id = {:?})", widget.id().map(|id| id.as_str())),
            Err(err) => format!("Err({err})"),
        }
    }
}
