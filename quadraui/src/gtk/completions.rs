//! GTK entry point for [`crate::Completions`] — a thin delegate onto the
//! widget's own shared pixel painter,
//! [`crate::primitives::completions::native_surface_paint::paint`], which
//! documents the popup chrome and row content it paints. This module only
//! adapts a Cairo context plus its Pango layout into the
//! [`crate::PaintSurface`] that painter takes.
//!
//! ## Border vs TUI
//!
//! The pixel painter strokes a full 4-side border around the popup. The
//! cell painter draws side `│` borders only — its row geometry has no
//! top/bottom rows for the chrome. That divergence is intrinsic to the
//! surfaces (TUI cells coalesce; Cairo paints arbitrary geometry); the
//! data primitive is shared, the chrome paint approach is not.

use gtk4::cairo::Context;
use gtk4::pango;

use crate::primitives::completions::{native_surface_paint, Completions, CompletionsLayout};
use crate::theme::Theme;

/// Paint a [`Completions`] popup at its resolved [`CompletionsLayout`].
///
/// Background uses [`Theme::completion_bg`], the selected row uses
/// [`Theme::completion_selected_bg`], item text uses
/// [`Theme::completion_fg`], right-aligned `detail` uses
/// [`Theme::muted_fg`], and the popup border uses
/// [`Theme::completion_border`].
///
/// Kind icons paint their [`crate::Icon::fallback`] text: this entry
/// point carries no Nerd-Font toggle of its own, so it passes
/// `nerd_fonts_enabled: false` and renders the same glyph the cell
/// painter does. A caller that wants the Nerd-Font glyph instead paints
/// the popup through
/// [`crate::DrawsPrimitives::draw_primitive`],
/// which resolves the toggle from the live backend.
pub fn draw_completions(
    cr: &Context,
    pango_layout: &pango::Layout,
    completions: &Completions,
    layout: &CompletionsLayout,
    theme: &Theme,
) {
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(pango_layout),
        translucent_fill: false,
    };
    native_surface_paint::paint(
        completions,
        layout,
        &mut surface,
        theme,
        &crate::style::Style::default(),
        /* nerd_fonts_enabled */ false,
    );
}
