//! Materializing a binding's wire input into one [`serde_json::Value`]
//! tree ([`materialize`]), the shared unknown-key diagnostics session
//! ([`DecodeDiagnostics`]) every per-kind decoder in [`crate::wire`]
//! routes an unrecognized optional key through, and the small
//! `obj_get`/`required_*`/`opt_*`/`dict_*` accessor vocabulary every
//! other `wire` submodule reads a decoded `Value` dict through.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Deserializer;
use serde_json::Value;

use quadraui::{Backend, BackendWidget, Rect, WidgetId};

use crate::host::text_display_from_lines;
use crate::DecodeError;

/// Minimal [`BackendWidget`] painted into a `run_shell` bottom-panel tab's
/// body: a single line naming the tab's own id. Bottom-panel tabs built
/// from `run_shell`'s `"bottom_panel"` config have no binding-registered
/// per-tab content callback (out of scope for this first cut), but
/// [`quadraui::BottomPanelTab::content`] still requires *some* [`BackendWidget`], and
/// painting the active tab's id makes "which tab is active" visible on the
/// rendered screen — useful for a driver test to assert on, instead of an
/// invisible no-op.
pub struct StaticTabContent(pub String);

impl BackendWidget for StaticTabContent {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let td = text_display_from_lines(
            WidgetId::new(format!("shell:bottom_panel:{}", self.0)),
            vec![self.0.clone()],
        );
        backend.draw_text_display(rect, &td);
    }
}

// ─── Materializing the input into a `Value` tree ───────────────────────────

/// Materializes `deserializer`'s whole input into one [`serde_json::Value`]
/// tree — almost `Value::deserialize`, except for the two deltas that keep
/// a decoder below byte-identical to one that decoded straight off the
/// original `Deserializer`, with no `Value` hop in between:
/// - a byte string (Python `bytes`/`bytearray`, which `pythonize` routes to
///   `deserialize_bytes`/`visit_bytes`) becomes a [`Value::Array`] of `u8`
///   numbers — the same shape `Vec<u8>`'s own default `Deserialize` impl
///   produces from a byte string via `deserialize_seq`, so
///   `quadraui::ImageSource::Bytes(Vec<u8>)` keeps decoding through
///   `{"Bytes": <bytes>}` rather than being rejected as "invalid
///   descriptor" by `serde_json::Value`'s own visitor (which has no
///   `visit_bytes` override).
/// - a non-finite float (`NaN`/`inf`/`-inf`) is rejected outright instead
///   of silently becoming [`Value::Null`] the way `serde_json`'s own
///   visitor does (`Number::from_f64` returns `None` for a non-finite
///   value, falling back to `Value::Null`) — [`obj_get`]'s "a `null` entry
///   means the key is absent" convention would otherwise alias a
///   non-finite value with a field the caller never set.
///
/// One accepted narrowing remains versus decoding straight off the
/// original `Deserializer`: every object key becomes a `String` (matching
/// `serde_json::Value::Object`'s own `Map<String, Value>` shape), so a
/// non-string dict key is rejected and a string-spelled-integer key (e.g.
/// `"3"`) is indistinguishable from the integer `3` once both land in the
/// same `Value::Object`. The in-tree callers of this module key every
/// dynamically-typed dict by string already (`"id"`, `"split"`, ...), and
/// `serde_json::Value` has no representation for a non-string key, so
/// fully preserving the original key's type would need abandoning
/// `Value` as the dispatch medium.
pub(super) fn materialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    deserializer.deserialize_any(ValueVisitor)
}

/// [`DeserializeSeed`] wrapper so [`ValueVisitor::visit_seq`]/`visit_map`
/// can recurse into [`materialize`]'s own visitor for each element/value —
/// `SeqAccess::next_element_seed`/`MapAccess::next_value_seed` take a seed,
/// not a bare `Visitor`.
struct ValueSeed;

impl<'de> DeserializeSeed<'de> for ValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        materialize(deserializer)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a value decodable into a descriptor wire tree")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        if !v.is_finite() {
            return Err(E::custom(format!("non-finite number {v} is not supported")));
        }
        Ok(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        materialize(deserializer)
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Value, E> {
        Ok(Value::Array(
            v.iter().map(|byte| Value::Number((*byte).into())).collect(),
        ))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut vec = Vec::new();
        while let Some(v) = seq.next_element_seed(ValueSeed)? {
            vec.push(v);
        }
        Ok(Value::Array(vec))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut obj = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(ValueSeed)?;
            obj.insert(key, value);
        }
        Ok(Value::Object(obj))
    }
}

// ─── Unknown optional-key diagnostics ───────────────────────────────────────

/// Sibling keys every layout-tree node dict may carry regardless of its own
/// kind: `"style"`/`"class"` (resolved by [`layout_node_from_value`]'s own
/// wrapper, once around whichever kind-specific decode below returns),
/// `"size"`/`"min"`/`"max"` (consumed by [`layout_children_from_value`] when
/// this dict is a row/column child — a harmless no-op on a node reached any
/// other way, since a child dict has no way to know which container, if
/// any, it ends up inside), and `"wire_version"` (checked once, up front, by
/// [`check_wire_version`], against the whole render output rather than any
/// one node — see that function's own doc). [`check_node_sibling_keys`]
/// folds these into every per-kind known-key list below, so none of them
/// has to repeat them.
pub(super) const UNIVERSAL_NODE_SIBLING_KEYS: [&str; 6] =
    ["style", "class", "size", "min", "max", "wire_version"];

/// The one wire key [`decode_value_checked`] accepts inside a `"list"`/
/// `"tree"`/`"data_table"` descriptor body despite none of `ListView`/
/// `TreeView`/`DataTable` having a field for it — [`crate::scroll_px_from_value`]
/// reads it straight out of the same `Value` dict, so it must stay out of the
/// unknown-key check's reject set for exactly those three kinds. Every other
/// descriptor-body kind passes `&[]` instead.
pub(super) const SCROLL_PX_EXTRA_KEYS: [&str; 1] = ["scroll_px"];

/// The two leaf-only drag-role wire keys [`drag_role_from_leaf_value`]
/// reads: `"draggable"` (this leaf can be picked up and dragged) and
/// `"drop_target"` (this leaf accepts a drop). Both are optional and
/// independent — a leaf may set either, both, or neither. Valid only on an
/// id-addressable leaf node dict (folded into [`super::leaf`]'s own
/// known-key list alongside `"id"`); a container dict carrying either is a
/// hard [`DecodeError`], checked by [`super::layout`] unconditionally
/// rather than through the warn/strict [`DecodeDiagnostics`] policy.
pub(super) const DRAG_ROLE_KEYS: [&str; 2] = ["draggable", "drop_target"];

/// One leaf's decoded drag-role flags — see [`DRAG_ROLE_KEYS`]. Recorded
/// into [`DecodeDiagnostics::drag_roles`]'s id-keyed side table rather than
/// onto `LayoutLeaf` itself; no paint or dispatch path reads it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DragRole {
    pub draggable: bool,
    pub drop_target: bool,
}

/// Reads the leaf-only [`DRAG_ROLE_KEYS`] off a leaf dict `value`, returning
/// `Ok(None)` when neither key is present (the overwhelmingly common case —
/// no leaf opts into either drag role) rather than a [`DragRole`] that's
/// `Default`-equal to "neither". Each key present must be a JSON bool; a
/// non-bool value is a [`DecodeError::type_mismatch`].
pub(super) fn drag_role_from_leaf_value(
    context: &str,
    value: &Value,
) -> Result<Option<DragRole>, DecodeError> {
    let draggable = opt_bool(value, "draggable", context)?;
    let drop_target = opt_bool(value, "drop_target", context)?;
    if draggable.is_none() && drop_target.is_none() {
        return Ok(None);
    }
    Ok(Some(DragRole {
        draggable: draggable.unwrap_or(false),
        drop_target: drop_target.unwrap_or(false),
    }))
}

/// Session-scoped sink for a wire decode's unknown-*optional*-key findings
/// — a dict key none of this crate's decoders recognize there, outside the
/// small number of closed-vocabulary sub-dicts (`"padding"`'s object shape,
/// a `"style"` override) that already hard-reject a typo unconditionally;
/// see each of their own docs for why those stay stricter. A required field
/// or a wrongly-typed value is always a [`DecodeError`] regardless of this
/// policy — this only covers an otherwise-harmless unrecognised sibling key
/// a decoder would otherwise silently drop.
///
/// [`Self::warn`] (and [`Self::default`]) keep decoding on a finding:
/// each distinct key name reaches the registered hook (if any) at most once
/// for this `DecodeDiagnostics`'s whole lifetime — a binding holds one
/// alongside its other session-scoped state (`crate::host::BindingHost`),
/// not fresh per decode call, so a render callback re-decoded every frame
/// doesn't re-warn on the same typo each time. [`Self::strict`] instead
/// turns the very first finding into a hard [`DecodeError::UnknownKey`],
/// the `strict=True`/`strict: true` app option a binding surfaces.
pub struct DecodeDiagnostics {
    strict: Cell<bool>,
    warned: RefCell<HashSet<String>>,
    hook: RefCell<Option<UnknownKeyHook>>,
    drag_roles: RefCell<HashMap<String, DragRole>>,
}

/// [`DecodeDiagnostics::set_hook`]'s registered callback, named so the
/// field holding it stays under clippy's `type_complexity` threshold.
type UnknownKeyHook = Rc<dyn Fn(&str, &str)>;

impl DecodeDiagnostics {
    /// A fresh warn-mode session with no hook registered yet — equivalent
    /// to [`Self::default`].
    pub fn warn() -> Self {
        Self {
            strict: Cell::new(false),
            warned: RefCell::new(HashSet::new()),
            hook: RefCell::new(None),
            drag_roles: RefCell::new(HashMap::new()),
        }
    }

    /// A fresh strict-mode session: every unknown optional key found from
    /// here on is a hard [`DecodeError::UnknownKey`] rather than a warning.
    pub fn strict() -> Self {
        let this = Self::warn();
        this.set_strict(true);
        this
    }

    /// Switches this session's policy — `true` for strict (reject), `false`
    /// for warn — without touching the already-warned set or hook. A
    /// binding calls this from its own `strict` setter/constructor
    /// argument, rather than needing to replace the whole
    /// [`BindingHost`](crate::host::BindingHost)-held instance.
    pub fn set_strict(&self, strict: bool) {
        self.strict.set(strict);
    }

    /// True once [`Self::set_strict`]`(true)`/[`Self::strict`] has set this
    /// session to reject rather than warn.
    pub fn is_strict(&self) -> bool {
        self.strict.get()
    }

    /// Registers (replacing any previous registration) the callback a
    /// warn-mode finding invokes — `hook(path, key)`, called at most once
    /// per distinct `key` for this session's lifetime. A binding wires this
    /// to whatever callable its own `on_unknown_key`-style surface
    /// registers; `None` (the default) makes a warn-mode finding a no-op
    /// beyond being recorded in [`Self::warned_keys`].
    pub fn set_hook(&self, hook: impl Fn(&str, &str) + 'static) {
        *self.hook.borrow_mut() = Some(Rc::new(hook));
    }

    /// Every key name that has already reached the hook (or would have, had
    /// one been registered) this session — exposed so a headless test can
    /// assert "once per key" without needing to install a hook itself.
    pub fn warned_keys(&self) -> HashSet<String> {
        self.warned.borrow().clone()
    }

    /// Records leaf `id`'s decoded [`DragRole`] — called by
    /// [`super::leaf::layout_leaf_from_value`] once per leaf dict that sets
    /// at least one of [`DRAG_ROLE_KEYS`]. A later call for the same `id`
    /// overwrites the earlier entry, matching how a re-decoded frame's
    /// other diagnostics state (the warned-key set) is also carried
    /// forward rather than reset.
    pub(super) fn record_drag_role(&self, id: &str, role: DragRole) {
        self.drag_roles.borrow_mut().insert(id.to_string(), role);
    }

    /// The id-keyed side table [`Self::record_drag_role`] has built up over
    /// this session's decode(s) so far — every leaf id whose dict set at
    /// least one of [`DRAG_ROLE_KEYS`], mapped to its resolved [`DragRole`].
    /// A leaf that set neither key has no entry here at all.
    pub fn drag_roles(&self) -> HashMap<String, DragRole> {
        self.drag_roles.borrow().clone()
    }

    /// Applies this session's policy to one finding: `key`, found on the
    /// dict `field` names at `path`, outside `expected`. Strict mode
    /// returns the same [`DecodeError::UnknownKey`] a required-field
    /// mismatch already would; warn mode calls the registered hook (at most
    /// once for this `key`) and returns `Ok`, so decoding continues
    /// unaffected either way.
    fn report_unknown_key(
        &self,
        path: &str,
        field: &str,
        key: &str,
        expected: &[&str],
    ) -> Result<(), DecodeError> {
        if self.strict.get() {
            return Err(DecodeError::unknown_key(path, field, key, expected));
        }
        if self.warned.borrow_mut().insert(key.to_string()) {
            // Clone the `Rc` out of the `RefCell` before invoking it, rather
            // than holding the borrow across the call: a hook that re-enters
            // and calls `set_hook` on this same session would otherwise hit
            // a double-borrow panic.
            let hook = self.hook.borrow().clone();
            if let Some(hook) = hook {
                hook(path, key);
            }
        }
        Ok(())
    }
}

impl Default for DecodeDiagnostics {
    fn default() -> Self {
        Self::warn()
    }
}

/// Diffs `value`'s own keys (when it is an object; a no-op otherwise —
/// every caller below only reaches this once its own shape check already
/// confirmed an object) against `known`, reporting every key outside it
/// through `diagnostics` — the one choke point every per-kind decoder
/// below routes an unrecognised *optional* sibling key through, rather than
/// each rolling its own loop (the pre-existing `"padding"`/`"style"` loops
/// stay separate — those hard-reject unconditionally, a deliberately
/// stricter policy; see their own docs for why).
pub(super) fn check_unknown_keys(
    diagnostics: &DecodeDiagnostics,
    path: &str,
    field: &str,
    value: &Value,
    known: &[&str],
) -> Result<(), DecodeError> {
    let Some(obj) = value.as_object() else {
        return Ok(());
    };
    for key in obj.keys() {
        if !known.contains(&key.as_str()) {
            diagnostics.report_unknown_key(path, field, key, known)?;
        }
    }
    Ok(())
}

/// [`check_unknown_keys`] against a layout-tree node dict specifically:
/// `known` is `shape_keys` plus [`UNIVERSAL_NODE_SIBLING_KEYS`], folded in
/// so every per-kind call site below only has to name the keys unique to
/// its own kind.
pub(super) fn check_node_sibling_keys(
    diagnostics: &DecodeDiagnostics,
    path: &str,
    field: &str,
    value: &Value,
    shape_keys: &[&str],
) -> Result<(), DecodeError> {
    let Some(obj) = value.as_object() else {
        return Ok(());
    };
    // Reported as the finding's `expected` list too, not just checked
    // against -- a strict-mode error naming only `shape_keys` would
    // under-report what this node actually accepts.
    let known: Vec<&str> = shape_keys
        .iter()
        .copied()
        .chain(UNIVERSAL_NODE_SIBLING_KEYS)
        .collect();
    for key in obj.keys() {
        if !known.contains(&key.as_str()) {
            diagnostics.report_unknown_key(path, field, key, &known)?;
        }
    }
    Ok(())
}

/// A one-shot [`serde::Deserializer`] wrapping a `&Value` object, used only
/// by [`decode_value_checked`]: every method but [`Self::deserialize_struct`]
/// forwards straight through to `value`'s own `Deserializer` impl, and that
/// one method diffs `value`'s own keys against the field-name list `T`'s
/// derived `Deserialize` impl hands it, unioned with `extra_keys` (a
/// call-site-supplied allow-list for wire keys `T` deliberately has no
/// field for, e.g. `"scroll_px"` on `list`/`tree`/`data_table` — see
/// [`crate::scroll_px_from_value`]), reporting anything outside that union
/// through `diagnostics` before continuing the real decode. A `T` whose
/// derive reaches this instead through `deserialize_map` (a `#[serde(flatten)]`
/// field, a hand-written `Deserialize` impl) skips the check silently rather
/// than tripping it falsely — for every key in `fields` union `extra_keys`,
/// this adds a finding only for keys outside both, never for a key either
/// one already accepts.
struct DescriptorBodyDeserializer<'de, 'b> {
    value: &'de Value,
    diagnostics: &'b DecodeDiagnostics,
    path: &'b str,
    field: &'b str,
    extra_keys: &'b [&'b str],
    unknown_key: &'b Cell<Option<DecodeError>>,
}

impl<'de, 'b> serde::Deserializer<'de> for DescriptorBodyDeserializer<'de, 'b> {
    type Error = serde_json::Error;

    fn deserialize_struct<V>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if let Some(obj) = self.value.as_object() {
            let known: Vec<&str> = fields
                .iter()
                .copied()
                .chain(self.extra_keys.iter().copied())
                .collect();
            for key in obj.keys() {
                if !known.contains(&key.as_str()) {
                    if let Err(err) = self
                        .diagnostics
                        .report_unknown_key(self.path, self.field, key, &known)
                    {
                        self.unknown_key.replace(Some(err));
                        return Err(serde::de::Error::custom(
                            "unknown key rejected by DecodeDiagnostics",
                        ));
                    }
                }
            }
        }
        self.value.deserialize_struct(name, fields, visitor)
    }

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.value.deserialize_any(visitor)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

/// [`decode_value`] for a descriptor *body* dict -- a leaf's `"button"`/
/// `"slider"`/... value, decoded straight into its fixed or additive-primitive
/// type -- except an unrecognized key inside that dict goes through
/// `diagnostics` exactly like a layout-tree node's own sibling keys already
/// do via [`check_unknown_keys`], rather than decoding silently either way.
/// `field` is the descriptor-body's own kind name (`"button"`, `"slider"`,
/// ...), reported alongside `path` the same way [`check_unknown_keys`]'s
/// callers already name their own dict. `extra_keys` names wire keys this
/// kind's dict accepts outside of `T`'s own derived fields -- e.g.
/// `"scroll_px"` for `"list"`/`"tree"`/`"data_table"`, which
/// [`crate::scroll_px_from_value`] reads out of this same dict rather than
/// `T` ever having a field for it; `&[]` for every kind with no such key.
pub(crate) fn decode_value_checked<'de, T>(
    diagnostics: &DecodeDiagnostics,
    path: &str,
    field: &str,
    value: &'de Value,
    extra_keys: &[&str],
) -> Result<T, DecodeError>
where
    T: serde::Deserialize<'de>,
{
    let unknown_key: Cell<Option<DecodeError>> = Cell::new(None);
    let guard = DescriptorBodyDeserializer {
        value,
        diagnostics,
        path,
        field,
        extra_keys,
        unknown_key: &unknown_key,
    };
    T::deserialize(guard).map_err(|err| {
        unknown_key
            .replace(None)
            .unwrap_or_else(|| DecodeError::borrow(path, format!("invalid descriptor: {err}")))
    })
}

// ─── Value helpers ──────────────────────────────────────────────────────────

/// Reads `value[key]`, treating a `null` entry the same as a missing one —
/// every decoder below has its own binding's "`None`-vs-missing both mean
/// empty" quirk collapsed by this one shared accessor.
pub(super) fn obj_get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object()?.get(key).filter(|v| !v.is_null())
}

/// Checks that `value` is a dict, raising a [`DecodeError`] that names
/// `suffix` (already carrying whatever label/id prefix the caller has in
/// hand) otherwise.
pub(super) fn must_be_dict<'a>(
    what: &str,
    value: &'a Value,
    suffix: &str,
) -> Result<&'a Value, DecodeError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(DecodeError::type_mismatch(what, suffix, "an object"))
    }
}

/// Reads `value[key]` as a required sub-dict, raising a [`DecodeError`]
/// at `context` (a caller-supplied path so far) if it is missing or not a
/// dict.
pub(super) fn required_dict<'a>(
    value: &'a Value,
    key: &str,
    context: &str,
) -> Result<&'a Value, DecodeError> {
    match obj_get(value, key) {
        Some(v) if v.is_object() => Ok(v),
        Some(_) => Err(DecodeError::type_mismatch(context, key, "an object")),
        None => Err(DecodeError::missing_field(context, key)),
    }
}

/// Reads `value[key]` as a required `list`, raising a [`DecodeError`] at
/// `context` if it is missing or not a list.
pub(super) fn required_list<'a>(
    value: &'a Value,
    key: &str,
    context: &str,
) -> Result<&'a Vec<Value>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_array()
            .ok_or_else(|| DecodeError::type_mismatch(context, key, "an array")),
        None => Err(DecodeError::missing_field(context, key)),
    }
}

/// Reads `value[key]` as a required string, raising a [`DecodeError`] at
/// `context` if it is missing or not a string.
pub(super) fn required_str(value: &Value, key: &str, context: &str) -> Result<String, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| DecodeError::type_mismatch(context, key, "a string")),
        None => Err(DecodeError::missing_field(context, key)),
    }
}

/// Reads `value[key]` as a required `f32`, raising a [`DecodeError`] at
/// `context` if it is missing or not a number.
pub(super) fn required_f32(value: &Value, key: &str, context: &str) -> Result<f32, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_f64()
            .map(|f| f as f32)
            .ok_or_else(|| DecodeError::type_mismatch(context, key, "a number")),
        None => Err(DecodeError::missing_field(context, key)),
    }
}

/// Reads `value[key]` as an optional `f32`, raising a [`DecodeError`] at
/// `context` if present but not a number. `None` when absent.
pub(super) fn opt_f32(value: &Value, key: &str, context: &str) -> Result<Option<f32>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| DecodeError::type_mismatch(context, key, "a number")),
        None => Ok(None),
    }
}

/// Reads `value[key]` as an optional `bool`, raising a [`DecodeError`] at
/// `context` if present but not a bool. `None` when absent.
pub(super) fn opt_bool(
    value: &Value,
    key: &str,
    context: &str,
) -> Result<Option<bool>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| DecodeError::type_mismatch(context, key, "a bool")),
        None => Ok(None),
    }
}

/// Reads `value[key]` as an optional non-negative integer, raising a
/// [`DecodeError`] at `context` if present but not one. `None` when
/// absent.
pub(super) fn opt_usize(
    value: &Value,
    key: &str,
    context: &str,
) -> Result<Option<usize>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_u64()
            .map(|n| Some(n as usize))
            .ok_or_else(|| DecodeError::type_mismatch(context, key, "an integer")),
        None => Ok(None),
    }
}

/// [`opt_usize`], bounded to `u8` — raises a [`DecodeError`] when present
/// but not an integer in `0..=255` (e.g. [`quadraui::Style::shadow_elevation`]'s
/// own clamped range). `None` when absent.
pub(super) fn opt_u8(value: &Value, key: &str, context: &str) -> Result<Option<u8>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| DecodeError::type_mismatch(context, key, "an integer in 0..=255")),
        None => Ok(None),
    }
}

/// [`opt_usize`], widened to `u64` — raises a [`DecodeError`] at `context`
/// if present but not a non-negative integer. `None` when absent.
pub(super) fn opt_u64(value: &Value, key: &str, context: &str) -> Result<Option<u64>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| DecodeError::type_mismatch(context, key, "an integer")),
        None => Ok(None),
    }
}

/// [`dict_str`], but raises a [`DecodeError`] when `key` is absent or
/// `null` instead of returning `Ok(None)`.
pub(super) fn dict_required_str(
    what: &str,
    value: &Value,
    key: &str,
) -> Result<String, DecodeError> {
    dict_str(what, value, key)?.ok_or_else(|| DecodeError::missing_field(what, key))
}

/// Reads `value[key]` as a string, raising a [`DecodeError`] if present
/// but not a string. `None` when `key` is absent or `null`.
pub(super) fn dict_str(
    what: &str,
    value: &Value,
    key: &str,
) -> Result<Option<String>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| DecodeError::type_mismatch(what, key, "a string")),
        None => Ok(None),
    }
}

/// Reads `value[key]` as an `f32`, raising a [`DecodeError`] if present but
/// not a number. `None` when `key` is absent or `null`.
pub(super) fn dict_f32(what: &str, value: &Value, key: &str) -> Result<Option<f32>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .ok_or_else(|| DecodeError::type_mismatch(what, key, "a number")),
        None => Ok(None),
    }
}

/// Reads `value[key]` as a `bool`, raising a [`DecodeError`] if present but
/// not a bool. `None` when `key` is absent or `null`.
pub(super) fn dict_bool(what: &str, value: &Value, key: &str) -> Result<Option<bool>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| DecodeError::type_mismatch(what, key, "a bool")),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A minimal `Deserializer` whose `deserialize_any` calls exactly one
    /// visitor method -- standing in for a slice of `pythonize`'s own
    /// `Depythonizer` dispatch (a Python `bytes`/`bytearray` always
    /// routes to `deserialize_bytes`/`visit_bytes`) without needing a
    /// live Python interpreter to drive it.
    struct BytesDeserializer<'a>(&'a [u8]);

    impl<'de> Deserializer<'de> for BytesDeserializer<'_> {
        type Error = serde::de::value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            visitor.visit_bytes(self.0)
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map struct enum identifier ignored_any
        }
    }

    /// A minimal `Deserializer` whose `deserialize_any` calls
    /// `visit_f64` directly -- standing in for `pythonize`'s own
    /// `deserialize_f64`/`visit_f64` routing for a Python `float`.
    struct F64Deserializer(f64);

    impl<'de> Deserializer<'de> for F64Deserializer {
        type Error = serde::de::value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            visitor.visit_f64(self.0)
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map struct enum identifier ignored_any
        }
    }

    /// A Python `bytes`/`bytearray` value -- which `pythonize` routes
    /// through `deserialize_bytes`/`visit_bytes`, not `deserialize_seq`
    /// -- materializes into the same array-of-numbers shape `Vec<u8>`'s
    /// own `Deserialize` impl expects from a `deserialize_seq` call,
    /// rather than being rejected (the regression this module's own
    /// `materialize`, not `serde_json::Value`'s `Deserialize` impl, is
    /// written to avoid: `serde_json::Value`'s visitor has no
    /// `visit_bytes` override at all).
    #[test]
    fn materializes_bytes_as_a_number_array() {
        let value = materialize(BytesDeserializer(&[1, 2, 3])).expect("bytes materialize");
        assert_eq!(value, json!([1, 2, 3]));
    }

    /// A non-finite float (`NaN`/`inf`/`-inf`) is rejected outright during
    /// materialization rather than silently becoming `Value::Null` --
    /// which `obj_get`'s "a `null` entry means the key is absent"
    /// convention would otherwise alias with a field the caller never
    /// set.
    #[test]
    fn rejects_a_non_finite_float() {
        let result = materialize(F64Deserializer(f64::NAN));
        assert!(result.is_err());
    }

    /// An image leaf's `"Bytes"` source decodes from the same
    /// array-of-numbers shape `materialize` produces for a Python `bytes`
    /// value (see `materializes_bytes_as_a_number_array`) -- the full
    /// decode path the fix in this module protects end to end.
    #[test]
    fn decodes_an_image_leaf_with_byte_source() {
        let value = json!({
            "id": "img1",
            "source": {"Bytes": [1, 2, 3]},
            "intrinsic_size": null,
            "fit": "Contain",
            "fallback_text": "",
        });
        crate::extra_paint::decode_extra_primitive("test", "image", &value)
            .expect("valid image with byte source");
    }
}
