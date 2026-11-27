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
//! `IconGrid` — every other built-in primitive, and any kind a future
//! `quadraui-binding-core` wire layer needs, grows this same table
//! rather than hand-writing another decode path.
//!
//! Only entries whose type implements [`crate::primitive::DynWidget`] —
//! which every [`crate::interactive::Interactive`] widget gets for free
//! via that trait's blanket impl — type-check here, so a `widget_kinds!`
//! entry can never produce a value the rest of the widget-contract
//! routing engine doesn't already know how to layout/paint/route events
//! to.
//!
//! That bound is per-feature-set, not absolute: a primitive whose
//! `Primitive`/`Interactive` impls are themselves gated on a backend
//! feature is only decodable on a build that has one. An entry carries
//! its type's own `#[cfg(…)]` for exactly that reason, and a `kind`
//! whose entry is cfg'd out decodes as
//! [`KindError::Unknown`] rather than failing to compile — a
//! zero-backend build of this crate has no rasteriser to paint such a
//! widget with anyway.

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
    /// [`register_widget_kind`] was asked to register a kind string that
    /// the static [`widget_kinds!`] table or the runtime registry already
    /// names.
    Duplicate(String),
}

impl std::fmt::Display for KindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KindError::Unknown(kind) => write!(f, "unknown widget kind {kind:?}"),
            KindError::Decode { kind, source } => {
                write!(f, "failed to decode widget kind {kind:?}: {source}")
            }
            KindError::Duplicate(kind) => write!(f, "widget kind {kind:?} is already registered"),
        }
    }
}

impl std::error::Error for KindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            KindError::Unknown(_) | KindError::Duplicate(_) => None,
            KindError::Decode { source, .. } => Some(source),
        }
    }
}

/// Decodes one runtime-registered widget kind's JSON payload into a boxed,
/// type-erased widget. Registered through [`register_widget_kind`].
pub type DecodeFn =
    fn(serde_json::Value) -> Result<Box<dyn crate::primitive::DynWidget>, serde_json::Error>;

fn runtime_registry(
) -> &'static std::sync::RwLock<std::collections::HashMap<&'static str, DecodeFn>> {
    static REGISTRY: std::sync::OnceLock<
        std::sync::RwLock<std::collections::HashMap<&'static str, DecodeFn>>,
    > = std::sync::OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// Registers a widget kind at runtime, so [`decode_widget`] and
/// [`decode_widget_from`] decode `kind` through `decode` after finding no
/// static [`widget_kinds!`] entry for it. This is how a widget defined
/// outside this crate (a binding's script-defined widget, a third-party
/// crate's `Interactive` type) joins the same decode path the built-in
/// kinds use.
///
/// Fails with [`KindError::Duplicate`] when `kind` is already a static
/// entry or already registered; [`unregister_widget_kind`] frees a
/// runtime name for reuse.
pub fn register_widget_kind(kind: &'static str, decode: DecodeFn) -> Result<(), KindError> {
    if is_static_widget_kind(kind) {
        return Err(KindError::Duplicate(kind.to_string()));
    }
    let mut registry = runtime_registry()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if registry.contains_key(kind) {
        return Err(KindError::Duplicate(kind.to_string()));
    }
    registry.insert(kind, decode);
    Ok(())
}

/// Removes a kind [`register_widget_kind`] added. A static
/// [`widget_kinds!`] entry or an unknown name is left untouched.
pub fn unregister_widget_kind(kind: &str) {
    runtime_registry()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(kind);
}

/// Every kind currently registered through [`register_widget_kind`],
/// sorted.
pub fn runtime_widget_kinds() -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> = runtime_registry()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .keys()
        .copied()
        .collect();
    kinds.sort_unstable();
    kinds
}

/// Every kind [`decode_widget`] accepts on this build: the static
/// [`widget_kinds!`] entries in table order, then the runtime-registered
/// kinds sorted. A binding reads this to name the accepted kinds in an
/// unknown-kind error.
pub fn widget_kind_names() -> Vec<&'static str> {
    let mut kinds = static_widget_kinds();
    kinds.extend(runtime_widget_kinds());
    kinds
}

/// The fallback arm of every [`widget_kinds!`]-generated decode: looks
/// `kind` up in the runtime registry, reporting [`KindError::Unknown`]
/// when nothing registered it.
#[doc(hidden)]
pub fn decode_registered_widget(
    kind: &str,
    value: serde_json::Value,
) -> Result<Box<dyn crate::primitive::DynWidget>, KindError> {
    let decode = runtime_registry()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(kind)
        .copied();
    match decode {
        Some(decode) => decode(value).map_err(|source| KindError::Decode {
            kind: kind.to_string(),
            source,
        }),
        None => Err(KindError::Unknown(kind.to_string())),
    }
}

/// Declare the `kind` string → descriptor type table [`decode_widget`]
/// matches against, generating `decode_widget` and its
/// deserializer-taking twin `decode_widget_from` in the invoking module.
///
/// Each `$ty` must implement [`serde::de::DeserializeOwned`] and
/// [`crate::primitive::DynWidget`] — every built-in `Primitive` that also
/// implements [`crate::interactive::Interactive`] gets the latter for
/// free via that trait's blanket impl (see [`crate::primitive::DynWidget`]'s
/// own doc).
///
/// An entry may carry attributes, which land on the generated `match`
/// arm. That is how a descriptor whose `Primitive`/`Interactive` impls
/// are gated on a backend feature joins the table: give the entry the
/// same `#[cfg(…)]` those impls have, and on a feature set that compiles
/// them out its `kind` simply decodes as [`KindError::Unknown`].
///
/// ```
/// use quadraui::widget_kinds;
///
/// widget_kinds! {
///     "cell_grid" => quadraui::CellGrid,
///     #[cfg(feature = "tui")]
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
/// let widget = decode_widget("cell_grid", payload.clone()).unwrap();
/// assert_eq!(widget.id().map(|id| id.as_str()), Some("sheet"));
///
/// // The same decode against a borrowed `Value` as a `Deserializer`,
/// // which is where a wire layer substitutes its own wrapper.
/// let widget = decode_widget_from("cell_grid", &payload).unwrap();
/// assert_eq!(widget.id().map(|id| id.as_str()), Some("sheet"));
/// ```
#[macro_export]
macro_rules! widget_kinds {
    ( $( $( #[$meta:meta] )* $kind:literal => $ty:ty ),* $(,)? ) => {
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
                    $( #[$meta] )*
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
                other => $crate::widget_kinds::decode_registered_widget(other, value),
            }
        }

        /// [`decode_widget`] against an arbitrary [`serde::Deserializer`]
        /// rather than an owned `serde_json::Value` — the seam a wire
        /// layer with its own unknown-key / diagnostics policy decodes
        /// through, since its wrapping deserializer sees each descriptor
        /// type's own field list via `deserialize_struct` the way a
        /// statically-typed `T::deserialize` call does.
        ///
        /// The error is the deserializer's own, so an unknown `kind` is
        /// reported as a [`serde::de::Error::custom`] message rather than
        /// a [`KindError`]; a caller holding a kind string it has already
        /// matched against its own table never sees that arm.
        pub fn decode_widget_from<'de, D>(
            kind: &str,
            deserializer: D,
        ) -> ::std::result::Result<::std::boxed::Box<dyn $crate::primitive::DynWidget>, D::Error>
        where
            D: ::serde::Deserializer<'de>,
        {
            use ::serde::de::Error as _;
            match kind {
                $(
                    $( #[$meta] )*
                    $kind => {
                        let widget = <$ty as ::serde::Deserialize>::deserialize(deserializer)?;
                        ::std::result::Result::Ok(::std::boxed::Box::new(widget))
                    }
                )*
                other => {
                    let value = <::serde_json::Value as ::serde::Deserialize>::deserialize(
                        deserializer,
                    )?;
                    $crate::widget_kinds::decode_registered_widget(other, value)
                        .map_err(|err| D::Error::custom(err))
                }
            }
        }

        /// Whether `kind` is one of this table's own static entries on
        /// this build (runtime-registered kinds excluded).
        pub fn is_static_widget_kind(kind: &str) -> bool {
            match kind {
                $(
                    $( #[$meta] )*
                    $kind => true,
                )*
                _ => false,
            }
        }

        /// This table's static kind strings on this build, in table order.
        pub fn static_widget_kinds() -> ::std::vec::Vec<&'static str> {
            #[allow(unused_mut)]
            let mut kinds = ::std::vec::Vec::new();
            $(
                $( #[$meta] )*
                kinds.push($kind);
            )*
            kinds
        }
    };
}

widget_kinds! {
    "cell_grid" => crate::CellGrid,
    // Gated the same as `impl Primitive for IconGrid` (and the
    // `non_interactive!(IconGrid)` that rides on it), so this entry
    // exists exactly on the feature sets where `IconGrid` is a
    // `DynWidget`.
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    "icon_grid" => crate::IconGrid,
    // Gated the same as `impl Primitive for ListView`/`impl Interactive
    // for ListView`, so this entry exists exactly on the feature sets
    // where `ListView` is a `DynWidget`.
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    "list" => crate::ListView,
    // Gated the same as `impl Primitive for TreeView`/`TreeTable`, so
    // these entries exist exactly on the feature sets where each is a
    // `DynWidget`. `"tree_table_controller"` is the wire name the py/node
    // hierarchical-model kind publishes; it decodes to the same flattened
    // `TreeTable` descriptor as `"tree_table"`.
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    "tree" => crate::TreeView,
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    "tree_table" => crate::TreeTable,
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    "tree_table_controller" => crate::TreeTable,
    // Ungated, like `impl Primitive for TabBar`/`impl Interactive for
    // TabBar` — `TabBar` is a `DynWidget` on every feature set.
    "tab_bar" => crate::TabBar,
    // Group A's small widgets, all three ungated for the same reason
    // `TabBar` is: their `Primitive`/`Interactive` impls carry no
    // feature gate, so each is a `DynWidget` on every feature set. The
    // kind strings match the ones `quadraui-binding-core`'s wire schema
    // already publishes for them.
    "progress" => crate::ProgressBar,
    "spinner" => crate::Spinner,
    "scrollbar" => crate::Scrollbar,
    // Slice 1 of the Switch/RadioGroup/Button port, same ungated shape
    // as `TabBar`/Group A above: their `Primitive`/`Interactive` impls
    // carry no feature gate, so each is a `DynWidget` on every feature
    // set. The kind strings match the ones `quadraui-binding-core`'s
    // wire schema already publishes for them.
    "switch" => crate::Switch,
    "radio_group" => crate::RadioGroup,
    "button" => crate::Button,
    // Ungated like `TabBar`: `SidebarPanel`'s `Primitive`/`Interactive`
    // impls carry no feature gate.
    "sidebar_panel" => crate::SidebarPanel,
    // Ungated like `Button`: `CommandCenter`'s `Primitive`/`Interactive`
    // impls carry no feature gate. The kind string matches the one
    // `quadraui-binding-core`'s wire schema already publishes for it.
    "command_center" => crate::CommandCenter,
    // Ungated like `Button`: `Terminal`'s `Primitive`/`Interactive` impls
    // carry no feature gate. The kind string is the one the wire schema
    // already publishes for it.
    "terminal" => crate::Terminal,
    "status_bar" => crate::StatusBar,
    "activity_bar" => crate::ActivityBar,
    // Ungated like `TabBar`: the menu widgets' `Primitive`/`Interactive`
    // impls carry no feature gate. The kind strings are the ones the wire
    // schema already publishes for them.
    "toolbar" => crate::Toolbar,
    "menu_bar" => crate::MenuBar,
    "context_menu" => crate::ContextMenu,
    // Ungated: `Palette`'s and `Form`'s `Primitive`/`Interactive` impls
    // carry no feature gate. The kind strings are the ones the wire
    // schema already publishes for them.
    "palette" => crate::Palette,
    "form" => crate::Form,
    // Ungated: `Editor`'s and `Minimap`'s `Primitive`/`Interactive` impls
    // and `Image`'s `Primitive` impl carry no feature gate. The kind
    // strings are the ones the wire schema already publishes for them.
    "editor" => crate::Editor,
    "minimap" => crate::Minimap,
    "image" => crate::Image,
    // Ungated: the `Primitive`/`Interactive` impls of `Chart`, `Canvas`,
    // `ScrollView`, `BoardModel`, `PipelineView` and `DiffView` carry no
    // feature gate. The kind strings are the ones the wire schema already
    // publishes for them.
    "chart" => crate::Chart,
    "canvas" => crate::Canvas,
    "scroll_view" => crate::ScrollView,
    "board" => crate::BoardModel,
    "pipeline_view" => crate::PipelineView,
    "diff_view" => crate::DiffView,
    "checkbox" => crate::Checkbox,
    "slider" => crate::Slider,
    "select" => crate::Select,
    "markdown" => crate::Markdown,
    "message_list" => crate::MessageList,
    "password" => crate::PasswordInput,
    "textarea" => crate::TextArea,
    // Ungated like `MessageList`: `FindReplacePanel`'s
    // `Primitive`/`Interactive` impls carry no feature gate.
    "find_replace" => crate::FindReplacePanel,
    "command_line" => crate::CommandLine,
    "number_input" => crate::NumberInput,
    "combo_box" => crate::ComboBox,
    "date_picker" => crate::DatePicker,
    // `DataTable` and `MultiSectionView` implement the contract only on a
    // build with a backend feature.
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    "data_table" => crate::DataTable,
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    "multi_section_view" => crate::MultiSectionView,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_test_progress(
        value: serde_json::Value,
    ) -> Result<Box<dyn crate::primitive::DynWidget>, serde_json::Error> {
        let bar: crate::ProgressBar = serde_json::from_value(value)?;
        Ok(Box::new(bar))
    }

    #[test]
    fn a_runtime_registered_kind_decodes_and_unregisters() {
        let payload = serde_json::json!({"id": "rt-bar", "value": 0.5});
        assert!(matches!(
            decode_widget("test_runtime_bar", payload.clone()),
            Err(KindError::Unknown(_))
        ));
        register_widget_kind("test_runtime_bar", decode_test_progress).expect("registers");
        assert!(widget_kind_names().contains(&"test_runtime_bar"));
        let widget = decode_widget("test_runtime_bar", payload.clone()).expect("decodes");
        assert_eq!(widget.id().map(|id| id.as_str()), Some("rt-bar"));
        let widget = decode_widget_from("test_runtime_bar", &payload).expect("decodes");
        assert_eq!(widget.id().map(|id| id.as_str()), Some("rt-bar"));
        assert!(matches!(
            register_widget_kind("test_runtime_bar", decode_test_progress),
            Err(KindError::Duplicate(_))
        ));
        unregister_widget_kind("test_runtime_bar");
        assert!(matches!(
            decode_widget("test_runtime_bar", payload),
            Err(KindError::Unknown(_))
        ));
    }

    #[test]
    fn registering_a_static_kind_is_a_duplicate() {
        assert!(matches!(
            register_widget_kind("cell_grid", decode_test_progress),
            Err(KindError::Duplicate(_))
        ));
        assert!(static_widget_kinds().contains(&"cell_grid"));
    }

    #[test]
    fn a_runtime_kind_reports_its_decode_error() {
        register_widget_kind("test_runtime_bad", decode_test_progress).expect("registers");
        let err = decode_widget("test_runtime_bad", serde_json::json!({"value": "x"}))
            .err()
            .expect("malformed payload fails");
        unregister_widget_kind("test_runtime_bad");
        assert!(matches!(err, KindError::Decode { .. }));
    }

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

    /// Coverage for the `"icon_grid"` entry, gated exactly as that entry
    /// is: `IconGrid` is only a `DynWidget` — and so only decodable — on
    /// a feature set that compiles its `Primitive`/`Interactive` impls.
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    mod gated_kind {
        use super::*;

        #[test]
        fn decodes_the_other_known_kind() {
            // `IconGrid::id` is a plain field, so `DynWidget::id` recovers
            // it generically, the same way it does for every other
            // built-in primitive with its own id field.
            let payload = serde_json::json!({
                "id": "icons",
                "items": [],
            });
            let widget = decode_widget("icon_grid", payload).expect("icon_grid decodes");
            assert_eq!(widget.id().map(|id| id.as_str()), Some("icons"));
        }

        /// `data_table` and `multi_section_view` decode on a build with a
        /// backend feature, the only builds their contract exists on.
        #[test]
        fn decodes_the_backend_gated_group_f_kinds() {
            let table = crate::DataTable {
                id: "table".into(),
                columns: Vec::new(),
                rows: Vec::new(),
                selected_idx: None,
                scroll_offset: 0,
                sort: None,
                has_focus: false,
                show_scrollbar: false,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: Vec::new(),
                footer: None,
                bordered: false,
                striped: false,
                glyphs: None,
            };
            let payload = serde_json::to_value(table).expect("data_table serializes");
            let widget = decode_widget("data_table", payload).expect("data_table decodes");
            assert_eq!(widget.id().map(|id| id.as_str()), Some("table"));

            let view = crate::MultiSectionView {
                id: "panel".into(),
                sections: Vec::new(),
                active_section: None,
                axis: crate::MsvAxis::Vertical,
                allow_resize: false,
                allow_collapse: true,
                scroll_mode: crate::ScrollMode::PerSection,
                has_focus: false,
                panel_scroll: 0.0,
            };
            let payload = serde_json::to_value(view).expect("multi_section_view serializes");
            let widget =
                decode_widget("multi_section_view", payload).expect("multi_section_view decodes");
            assert_eq!(widget.id().map(|id| id.as_str()), Some("panel"));
        }

        #[test]
        fn rejects_a_malformed_payload() {
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
    }

    /// The group-F kinds a binding host sends round-trip through
    /// `decode_widget` from the shape their descriptors serialize to.
    #[test]
    fn group_f_kinds_decode_from_their_serialized_descriptors() {
        let cases: Vec<(&str, serde_json::Value, &str)> = vec![
            (
                "message_list",
                serde_json::to_value(crate::MessageList {
                    id: "chat".into(),
                    rows: Vec::new(),
                    scroll_top: 0,
                })
                .expect("message_list serializes"),
                "chat",
            ),
            (
                "password",
                serde_json::to_value(crate::PasswordInput::new("pw".into()))
                    .expect("password serializes"),
                "pw",
            ),
            (
                "textarea",
                serde_json::to_value(crate::TextArea::new("notes".into()))
                    .expect("textarea serializes"),
                "notes",
            ),
        ];
        for (kind, payload, id) in cases {
            let widget = decode_widget(kind, payload).unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert_eq!(widget.id().map(|w| w.as_str()), Some(id), "{kind}");
        }
    }

    /// A `find_replace` payload with no `id` decodes under the overlay's
    /// default id, the shape a binding host sends for its one live panel.
    #[test]
    fn find_replace_decodes_with_its_default_id() {
        let widget = decode_widget(
            "find_replace",
            serde_json::json!({
                "query": "needle",
                "group_bounds": {"x": 0.0, "y": 0.0, "width": 40.0, "height": 20.0},
            }),
        )
        .expect("find_replace decodes");
        assert_eq!(widget.id().map(|w| w.as_str()), Some("find_replace"));
    }

    /// Group A's three kinds decode from the same wire shape their
    /// descriptors serialize to, with every optional field defaulted —
    /// what a binding host sends when it only sets the fields it cares
    /// about.
    #[test]
    fn decodes_the_group_a_kinds() {
        let progress = decode_widget(
            "progress",
            serde_json::json!({"id": "dl", "label": "Fetching", "value": 0.5}),
        )
        .expect("progress decodes");
        assert_eq!(progress.id().map(|id| id.as_str()), Some("dl"));

        let spinner =
            decode_widget("spinner", serde_json::json!({"id": "boot"})).expect("spinner decodes");
        assert_eq!(spinner.id().map(|id| id.as_str()), Some("boot"));

        let scrollbar = decode_widget(
            "scrollbar",
            serde_json::json!({
                "id": "gutter",
                "axis": "Vertical",
                "track": {"x": 79.0, "y": 0.0, "width": 1.0, "height": 24.0},
                "thumb_start": 0.0,
                "thumb_len": 6.0,
            }),
        )
        .expect("scrollbar decodes");
        assert_eq!(scrollbar.id().map(|id| id.as_str()), Some("gutter"));
    }

    /// The menu widgets decode from the wire shape their descriptors
    /// serialize to, with every optional field defaulted.
    #[test]
    fn decodes_the_menu_kinds() {
        let toolbar = decode_widget(
            "toolbar",
            serde_json::json!({"id": "tools", "buttons": [{"Separator": null}]}),
        )
        .expect("toolbar decodes");
        assert_eq!(toolbar.id().map(|id| id.as_str()), Some("tools"));

        let menu_bar = decode_widget(
            "menu_bar",
            serde_json::json!({
                "id": "menus",
                "items": [{"id": "menu:file", "label": "&File", "disabled": false, "submenu": null}],
                "open_item": null,
                "focused_item": null,
            }),
        )
        .expect("menu_bar decodes");
        assert_eq!(menu_bar.id().map(|id| id.as_str()), Some("menus"));

        let context_menu = decode_widget(
            "context_menu",
            serde_json::json!({
                "id": "popup",
                "items": [],
                "selected_idx": 0,
                "bg": null,
            }),
        )
        .expect("context_menu decodes");
        assert_eq!(context_menu.id().map(|id| id.as_str()), Some("popup"));
    }

    #[test]
    fn rejects_a_malformed_group_a_payload() {
        // `value` is a fraction — a string has no sensible decode.
        let payload = serde_json::json!({"id": "dl", "value": "half"});
        match decode_widget("progress", payload) {
            Err(KindError::Decode { kind, .. }) => assert_eq!(kind, "progress"),
            other => panic!("expected Err(KindError::Decode), got {}", describe(other)),
        }
    }

    /// A build with no backend feature compiles `IconGrid`'s
    /// `Primitive`/`Interactive` impls out, so its `widget_kinds!` entry
    /// goes with them and the kind reads as unknown rather than failing
    /// to build.
    #[cfg(not(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )))]
    #[test]
    fn gated_kind_is_unknown_without_a_backend() {
        let payload = serde_json::json!({"id": "icons", "items": []});
        match decode_widget("icon_grid", payload) {
            Err(KindError::Unknown(kind)) => assert_eq!(kind, "icon_grid"),
            other => panic!("expected Err(KindError::Unknown), got {}", describe(other)),
        }
    }

    /// Coverage for the `"list"` entry, gated exactly as that entry is:
    /// `ListView` is only a `DynWidget` — and so only decodable — on a
    /// feature set that compiles its `Primitive`/`Interactive` impls.
    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    mod list_kind {
        use super::*;

        #[test]
        fn decodes_the_list_kind() {
            let payload = serde_json::json!({
                "id": "quickfix",
                "items": [],
                "selected_idx": 0,
            });
            let widget = decode_widget("list", payload).expect("list decodes");
            assert_eq!(widget.id().map(|id| id.as_str()), Some("quickfix"));
        }

        #[test]
        fn rejects_a_malformed_list_payload() {
            // `selected_idx` must be an integer — a string value has no
            // sensible decode and should surface as `KindError::Decode`,
            // not panic.
            let payload = serde_json::json!({
                "id": "quickfix",
                "items": [],
                "selected_idx": "first",
            });
            match decode_widget("list", payload) {
                Err(KindError::Decode { kind, .. }) => assert_eq!(kind, "list"),
                other => panic!("expected Err(KindError::Decode), got {}", describe(other)),
            }
        }
    }

    /// A build with no backend feature compiles `ListView`'s
    /// `Primitive`/`Interactive` impls out, so its `widget_kinds!` entry
    /// goes with them and the kind reads as unknown rather than failing
    /// to build.
    #[cfg(not(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )))]
    #[test]
    fn list_kind_is_unknown_without_a_backend() {
        let payload = serde_json::json!({"id": "quickfix", "items": [], "selected_idx": 0});
        match decode_widget("list", payload) {
            Err(KindError::Unknown(kind)) => assert_eq!(kind, "list"),
            other => panic!("expected Err(KindError::Unknown), got {}", describe(other)),
        }
    }

    /// Coverage for the `"tab_bar"` entry, ungated exactly as that entry
    /// is: `TabBar` is a `DynWidget` — and so decodable — on every
    /// feature set, including one with no rasteriser at all.
    mod tab_bar_kind {
        use super::*;

        #[test]
        fn decodes_the_tab_bar_kind() {
            let payload = serde_json::json!({
                "id": "tabs:editor",
                "tabs": [{"label": "main.rs", "is_active": true}],
            });
            let widget = decode_widget("tab_bar", payload).expect("tab_bar decodes");
            assert_eq!(widget.id().map(|id| id.as_str()), Some("tabs:editor"));
        }

        #[test]
        fn rejects_a_malformed_tab_bar_payload() {
            // `tabs` must be an array — a string value has no sensible
            // decode and should surface as `KindError::Decode`, not panic.
            let payload = serde_json::json!({
                "id": "tabs:editor",
                "tabs": "not-an-array",
            });
            match decode_widget("tab_bar", payload) {
                Err(KindError::Decode { kind, .. }) => assert_eq!(kind, "tab_bar"),
                other => panic!("expected Err(KindError::Decode), got {}", describe(other)),
            }
        }
    }

    /// Group A slice-1's three kinds (`Switch`/`RadioGroup`/`Button`)
    /// decode from the same wire shape their descriptors serialize to,
    /// with every optional field defaulted.
    #[test]
    fn decodes_the_switch_radio_group_and_button_kinds() {
        let switch = decode_widget("switch", serde_json::json!({"id": "settings:wrap"}))
            .expect("switch decodes");
        assert_eq!(switch.id().map(|id| id.as_str()), Some("settings:wrap"));

        let radio_group = decode_widget(
            "radio_group",
            serde_json::json!({
                "id": "settings:theme",
                "options": [{"id": "settings:theme:light", "label": "Light"}],
            }),
        )
        .expect("radio_group decodes");
        assert_eq!(
            radio_group.id().map(|id| id.as_str()),
            Some("settings:theme")
        );

        let button = decode_widget(
            "button",
            serde_json::json!({"id": "dialog:ok", "label": "OK"}),
        )
        .expect("button decodes");
        assert_eq!(button.id().map(|id| id.as_str()), Some("dialog:ok"));
    }

    #[test]
    fn decodes_the_command_center_kind() {
        let widget = decode_widget(
            "command_center",
            serde_json::json!({
                "id": "command_center:main",
                "back_enabled": true,
                "forward_enabled": false,
                "search_label": "my-project",
            }),
        )
        .expect("command_center decodes");
        assert_eq!(
            widget.id().map(|id| id.as_str()),
            Some("command_center:main")
        );
        assert_eq!(
            widget
                .downcast_ref::<crate::CommandCenter>()
                .map(|cc| cc.search_label.as_str()),
            Some("my-project")
        );
    }

    #[test]
    fn rejects_a_malformed_switch_payload() {
        // `value` must be a bool — a string has no sensible decode.
        let payload = serde_json::json!({"id": "settings:wrap", "value": "on"});
        match decode_widget("switch", payload) {
            Err(KindError::Decode { kind, .. }) => assert_eq!(kind, "switch"),
            other => panic!("expected Err(KindError::Decode), got {}", describe(other)),
        }
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
        // `row_count` must be an integer — a string value has no sensible
        // decode and should surface as `KindError::Decode`, not panic.
        let payload = serde_json::json!({
            "id": "sheet",
            "row_count": "lots",
            "col_count": 5,
            "default_row_height": 1.0,
            "default_col_width": 8.0,
        });
        match decode_widget("cell_grid", payload) {
            Err(KindError::Decode { kind, .. }) => assert_eq!(kind, "cell_grid"),
            other => panic!("expected Err(KindError::Decode), got {}", describe(other)),
        }
    }

    #[test]
    fn kind_error_display_names_the_kind() {
        let unknown = KindError::Unknown("mystery".to_string());
        assert!(unknown.to_string().contains("mystery"));

        let payload = serde_json::json!({"id": "sheet", "row_count": "lots"});
        let decode_err = match decode_widget("cell_grid", payload) {
            Err(err) => err,
            Ok(_) => panic!("expected a decode error"),
        };
        assert!(decode_err.to_string().contains("cell_grid"));
        assert!(std::error::Error::source(&decode_err).is_some());
    }

    /// `decode_widget_from` decodes the same `kind` → type table out of a
    /// borrowed `Value` acting as its own `Deserializer`, which is the
    /// shape a wire layer substitutes its unknown-key-reporting wrapper
    /// into.
    #[test]
    fn decodes_a_known_kind_from_a_deserializer() {
        let payload = serde_json::json!({"id": "btn:ok", "label": "OK"});
        match decode_widget_from("button", &payload) {
            Ok(widget) => assert_eq!(widget.id().map(|id| id.as_str()), Some("btn:ok")),
            Err(err) => panic!("expected a decoded button, got Err({err})"),
        }
    }

    /// An unknown kind has no [`KindError`] to report through, since the
    /// error type is the deserializer's own — it surfaces as a custom
    /// serde message naming the kind instead.
    #[test]
    fn rejects_an_unknown_kind_from_a_deserializer() {
        let payload = serde_json::json!({});
        match decode_widget_from("not_a_real_kind", &payload) {
            Err(err) => assert!(
                err.to_string().contains("not_a_real_kind"),
                "{}",
                err.to_string()
            ),
            Ok(widget) => panic!(
                "expected an unknown-kind error, got Ok(widget id = {:?})",
                widget.id().map(|id| id.as_str())
            ),
        }
    }

    /// A malformed payload for a known kind surfaces the deserializer's
    /// own error rather than being swallowed.
    #[test]
    fn rejects_a_malformed_payload_from_a_deserializer() {
        let payload = serde_json::json!({"id": "btn:ok", "label": 7});
        match decode_widget_from("button", &payload) {
            Err(err) => assert!(!err.to_string().is_empty()),
            Ok(widget) => panic!(
                "expected a decode error, got Ok(widget id = {:?})",
                widget.id().map(|id| id.as_str())
            ),
        }
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

    #[test]
    fn decodes_the_chart_canvas_and_scroll_view_kinds() {
        let chart = decode_widget(
            "chart",
            serde_json::json!({"id": "chart", "kind": "Line", "series": []}),
        )
        .expect("chart decodes");
        assert_eq!(chart.id().map(|id| id.as_str()), Some("chart"));
        let canvas = decode_widget("canvas", serde_json::json!({"id": "canvas", "ops": []}))
            .expect("canvas decodes");
        assert_eq!(canvas.id().map(|id| id.as_str()), Some("canvas"));
        let view = decode_widget(
            "scroll_view",
            serde_json::json!({
                "id": "scroll",
                "viewport": {"x": 0.0, "y": 0.0, "width": 10.0, "height": 4.0},
                "content_len": 20.0,
            }),
        )
        .expect("scroll_view decodes");
        assert_eq!(view.id().map(|id| id.as_str()), Some("scroll"));
    }

    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    mod tree_kinds {
        use super::*;
        use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow, DataTable};
        use crate::types::{Decoration, SelectionMode, StyledText, TreeStyle};
        use crate::{TreeRow, TreeTable, TreeTableNode, TreeView, WidgetId};

        fn sample_tree() -> TreeView {
            TreeView {
                id: WidgetId::new("explorer"),
                rows: vec![TreeRow {
                    path: vec![0],
                    indent: 0,
                    icon: None,
                    text: StyledText::plain("src"),
                    badge: None,
                    is_expanded: Some(false),
                    decoration: Decoration::Normal,
                    edit: None,
                }],
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: TreeStyle::default(),
                has_focus: false,
            }
        }

        fn sample_tree_table() -> TreeTable {
            let table = DataTable {
                id: WidgetId::new("files"),
                columns: vec![Column {
                    title: "Name".to_string(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                }],
                rows: vec![DataRow {
                    cells: vec![StyledText::plain("src")],
                    decoration: Decoration::Normal,
                }],
                selected_idx: None,
                scroll_offset: 0,
                sort: None,
                has_focus: false,
                show_scrollbar: false,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: vec![],
                footer: None,
                bordered: false,
                striped: false,
                glyphs: None,
            };
            TreeTable::new(table, vec![TreeTableNode::new(vec![0], 0)])
        }

        #[test]
        fn decodes_a_tree() {
            let payload = serde_json::to_value(sample_tree()).expect("tree serializes");
            let widget = decode_widget("tree", payload).expect("tree decodes");
            assert_eq!(widget.id().map(|id| id.as_str()), Some("explorer"));
        }

        #[test]
        fn decodes_a_tree_table_under_both_kind_names() {
            for kind in ["tree_table", "tree_table_controller"] {
                let payload = serde_json::to_value(sample_tree_table()).expect("table serializes");
                let widget = decode_widget(kind, payload)
                    .unwrap_or_else(|err| panic!("{kind} decodes: {err}"));
                assert_eq!(widget.id().map(|id| id.as_str()), Some("files"), "{kind}");
            }
        }
    }
}
