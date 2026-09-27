//! macOS layout helper for [`crate::TextInput`].
//!
//! Painting lives in the shared [`crate::primitives::text_input::paint`]
//! (issue #1093) — `MacBackend::draw_text_input` builds the layout via
//! [`mac_text_input_layout`] (a thin wrapper over the portable
//! [`crate::primitives::text_input::TextInput::layout`], the same shape
//! `win::text_input::win_text_input_layout` / `tui::text_input::
//! tui_text_input_layout` already use) and hands both to `paint` through
//! `MacBackend`'s own [`crate::native_surface::NativeSurface`]
//! implementation.
//!
//! Before #1093, `MacBackend::draw_text_input` was a bare stub that only
//! ever returned a layout — no background, no text, no cursor ever
//! reached the screen, and nothing in [`crate::BackendCaps`] said so. See
//! `primitives::text_input`'s own `native_surface_paint` module doc for
//! why this reaches for the shared `NativeSurface` painter instead of a
//! fourth bespoke per-backend rasteriser (GTK/Windows/TUI each already
//! have a working, already-tested one of their own).

use crate::event::Rect;
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};

/// Compute [`TextInputLayout`] for `ti` painted at `rect`, using the
/// backend's tracked `line_height`/`char_width` — what
/// [`crate::macos::backend::MacBackend::text_input_layout`] and
/// [`crate::macos::backend::MacBackend::draw_text_input`] both call.
/// Delegates entirely to [`TextInput::layout`] — no geometry re-derived
/// here, matching every other backend's `*_text_input_layout` wrapper.
pub fn mac_text_input_layout(
    ti: &TextInput,
    rect: Rect,
    line_height: f32,
    char_width: f32,
) -> TextInputLayout {
    ti.layout(rect, TextInputMeasure::new(line_height, char_width))
}
