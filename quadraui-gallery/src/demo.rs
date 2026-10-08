//! The [`Demo`] trait — the one seam every ported example implements.
//!
//! A `Demo` is a self-contained, backend-agnostic exhibit: it renders
//! into a main-pane rect the same way any [`quadraui::AppLogic`] renders
//! into a viewport, handles the events the gallery shell forwards to it,
//! and additionally knows how to describe itself for the Code and Data
//! tabs (`CLAUDE.md`'s "Demos are mandatory" rule, applied to a gallery
//! of demos rather than a single example binary).

use quadraui::{Backend, BackendCaps, Rect, UiEvent};
use serde_json::Value;

/// One exhibit in the gallery: a name, a group, an optional list of
/// named variants, and four read/write seams the shell drives.
///
/// Implementors own their own state (the way `ToastApp` in
/// `examples/common/toast_app.rs` owns `toasts: Vec<Toast>`) and are
/// constructed once by the [`crate::registry::registry`] entry that
/// lists them.
pub trait Demo {
    /// Short display name, e.g. `"Toast"`. Shown in the sidebar tree and
    /// as the demo's row in `data()`'s JSON.
    fn name(&self) -> &'static str;

    /// Group name — one of the five activity-bar groups in
    /// [`crate::app::GROUPS`]. A demo with a group not in that list is
    /// simply never shown (caught by the registry table-driven test).
    fn group(&self) -> &'static str;

    /// Named sub-configurations this demo exposes. The seed `ToastDemo`
    /// has exactly one; a future demo (e.g. a `Button` demo with
    /// default/disabled/focused states) lists more here. Every `Demo`
    /// method below takes the variant index so callers always know which
    /// configuration they're addressing; `render`/`handle`/`data` are
    /// free to ignore it when there is only one.
    fn variants(&self) -> &'static [&'static str] {
        &["default"]
    }

    /// Render `variant` into `area` (the gallery's Demo-tab content
    /// rect). Mirrors [`quadraui::AppLogic::render`]'s contract: plain
    /// data in, backend rasterises.
    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect);

    /// Handle one event already routed to the Demo tab's content area.
    /// The gallery shell logs the event (and its target `WidgetId`, if
    /// any) *before* calling this — see
    /// [`crate::app::GalleryApp::handle`] — so a demo only needs to
    /// react, not also log.
    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> quadraui::Reaction;

    /// This module's own source, trimmed to the `// gallery:begin` /
    /// `// gallery:end` region (see [`extract_region`]). Implementors
    /// store `const SOURCE: &str = include_str!("this_module.rs");` at
    /// the top of the demo's module, then return
    /// `extract_region(SOURCE)` here.
    fn source(&self) -> &'static str;

    /// The primitive(s) `variant` currently renders, as a
    /// `serde_json::Value` for the Data tab.
    fn data(&self, variant: usize) -> Value;

    /// A one-line note for the Demo tab when `caps` reports something
    /// this demo/variant relies on as unsupported by the running
    /// backend. `None` (the default) means every capability this demo
    /// uses is available.
    fn caps_note(&self, variant: usize, caps: &BackendCaps) -> Option<String> {
        let _ = (variant, caps);
        None
    }
}

/// Trim `source` to the text between a `// gallery:begin` line and the
/// next `// gallery:end` line (exclusive of both marker lines).
///
/// Falls back to the whole (trimmed) source when either marker is
/// missing — the registry's table-driven test asserts every demo's
/// region is non-empty, so a demo module that forgets the markers still
/// shows *something* on the Code tab rather than silently nothing, but
/// fails that test's "has real markers" expectation loudly instead.
pub fn extract_region(source: &str) -> &str {
    let Some(begin_at) = source.find("gallery:begin") else {
        return source.trim();
    };
    let Some(end_at) = source.find("gallery:end") else {
        return source.trim();
    };
    if end_at <= begin_at {
        return source.trim();
    }
    let body_start = source[begin_at..]
        .find('\n')
        .map(|n| begin_at + n + 1)
        .unwrap_or(source.len());
    let body_end = source[..end_at]
        .rfind('\n')
        .map(|n| n + 1)
        .unwrap_or(begin_at);
    if body_start >= body_end {
        return "";
    }
    source[body_start..body_end].trim_end_matches('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_region_trims_to_markers() {
        let src = "use foo;\n\n// gallery:begin\nstruct Thing;\nimpl Thing {}\n// gallery:end\n\nfn main() {}\n";
        let region = extract_region(src);
        assert!(region.contains("struct Thing;"));
        assert!(region.contains("impl Thing {}"));
        assert!(!region.contains("use foo;"));
        assert!(!region.contains("fn main()"));
    }

    #[test]
    fn extract_region_falls_back_without_markers() {
        let src = "struct NoMarkers;\n";
        assert_eq!(extract_region(src), "struct NoMarkers;");
    }
}
