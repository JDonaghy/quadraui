//! Fixtures shared by more than one `wire` submodule's own `#[cfg(test)]`
//! block — compiled only under `#[cfg(test)]` itself (see [`super`]'s own
//! `mod test_support;` declaration), never part of this crate's release
//! build.

use serde_json::Value;

use crate::layout::LayoutNode;
use crate::DecodeError;

use super::decode_layout_node;

pub(super) fn decode(value: &Value) -> Result<LayoutNode, DecodeError> {
    decode_layout_node("test", value)
}

/// Unwraps a `DecodeError`, panicking on an `Ok` -- the `#[cfg(test)]`
/// counterpart of `Result::expect_err`, which the `Ok` types below
/// (`LayoutNode`, `RenderOutput`, `Rc<dyn ExtraPaint>`) don't derive
/// `Debug` for.
pub(super) fn err<T>(result: Result<T, DecodeError>) -> DecodeError {
    match result {
        Ok(_) => panic!("expected an error, got Ok"),
        Err(e) => e,
    }
}
