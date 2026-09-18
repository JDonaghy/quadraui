//! CSS-style generic font family tokens (issue #1023).
//!
//! [`Backend::set_editor_font`]/[`Backend::set_ui_font`] take a family
//! string that used to go straight to the native font system unresolved:
//! a consumer had no portable way to ask for "the monospace font" without
//! guessing which concrete family exists on which OS (`Menlo` on macOS,
//! `Monospace` on fontconfig, `Consolas` on Windows, …). vimcode's
//! `src/app_support.rs` ended up branching on `cfg!(target_os = "macos")`
//! in otherwise-shared code to work around exactly this gap — the thing
//! its own platform-neutrality rule forbids.
//!
//! [`GenericFamily`] is the fix: the three tokens CSS's `font-family`
//! property already reserves for this (plus Pango's own
//! `Monospace`/`Sans` spellings for the first two, since GTK already
//! resolves those natively via fontconfig) — parsed once here and mapped
//! to a concrete native font by each pixel backend's own
//! `set_editor_font`/`set_ui_font`:
//!
//! | Backend | mapping |
//! |---|---|
//! | GTK (`gtk::backend`) | fontconfig aliases (`Monospace`/`Sans` — already generic; `system-ui` is rewritten to `Sans`, fontconfig's own closest concept) |
//! | macOS (`macos::backend`, `macos::text`) | `kCTFontSystemFontType` (sans-serif/system-ui) / `kCTFontUserFixedPitchFontType` (monospace) |
//! | Win-GUI (`win::backend`) | `DEFAULT_UI_FONT_FAMILY`/`DEFAULT_EDITOR_FONT_FAMILY` DirectWrite defaults |
//!
//! TUI is exempt for the same structural reason it takes both methods'
//! no-op [`Backend`] default — a terminal cell grid has no font concept
//! to resolve into (see `crate::backend::BackendCaps::generic_font_families`'s
//! doc for how that shows up in the cross-backend capability sweep).
//!
//! Not under `primitives/`, for the same reason `crate::font_role` isn't
//! (see that module's doc): this has no `Layout`, no `hit_test`, no
//! `Backend::draw_*` method of its own, so it must not inflate the
//! primitive count `readme_truth.rs` derives from `primitives/mod.rs`'s
//! `pub mod` list.
//!
//! [`Backend::set_editor_font`]: crate::Backend::set_editor_font
//! [`Backend::set_ui_font`]: crate::Backend::set_ui_font
//! [`Backend`]: crate::Backend

/// A generic font family request — "give me a monospace font", not a
/// specific one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericFamily {
    /// CSS `monospace` / Pango `Monospace` — fixed-width text (editor
    /// content, terminals). Every glyph shares one advance width, the
    /// property [`crate::Backend::set_editor_font`]'s doc requires of
    /// whatever family resolves this.
    Monospace,
    /// CSS `sans-serif` / Pango `Sans` — the platform's default
    /// proportional face.
    SansSerif,
    /// CSS `system-ui` — the platform's native chrome/UI font. Distinct
    /// from `sans-serif` in the CSS spec (a platform may reserve a
    /// different face for its own chrome than for generic prose), but
    /// every backend here maps it to the same concrete family
    /// `sans-serif` maps to: that family already *is* each platform's UI
    /// font (CoreText's `kCTFontSystemFontType`, DirectWrite's
    /// `Segoe UI`, fontconfig's `sans-serif` alias), so there is no
    /// second, more-chrome-specific default to point `system-ui` at
    /// instead.
    SystemUi,
}

impl GenericFamily {
    /// Parse `token` as one of the three generic family keywords,
    /// case-insensitively and trimmed of surrounding whitespace — `None`
    /// for anything else, including a concrete family name (`"Menlo"`)
    /// or an empty string.
    ///
    /// Recognizes both CSS's own spellings (`monospace`, `sans-serif`,
    /// `system-ui`) and Pango's historical aliases for the first two
    /// (`Monospace`, `Sans`) — GTK/fontconfig already treat those as
    /// synonyms, so a caller who writes Pango-style font descriptions
    /// elsewhere doesn't have to switch vocabulary just to reach this
    /// path. Pango has no alias for `system-ui`; only the CSS spelling
    /// reaches it.
    pub fn parse(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "monospace" => Some(Self::Monospace),
            "sans-serif" | "sans" => Some(Self::SansSerif),
            "system-ui" => Some(Self::SystemUi),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_css_tokens() {
        assert_eq!(
            GenericFamily::parse("monospace"),
            Some(GenericFamily::Monospace)
        );
        assert_eq!(
            GenericFamily::parse("sans-serif"),
            Some(GenericFamily::SansSerif)
        );
        assert_eq!(
            GenericFamily::parse("system-ui"),
            Some(GenericFamily::SystemUi)
        );
    }

    #[test]
    fn parses_pango_aliases_for_the_first_two() {
        assert_eq!(
            GenericFamily::parse("Monospace"),
            Some(GenericFamily::Monospace)
        );
        assert_eq!(GenericFamily::parse("Sans"), Some(GenericFamily::SansSerif));
    }

    #[test]
    fn pango_has_no_system_ui_alias() {
        assert_eq!(GenericFamily::parse("System"), None);
        assert_eq!(GenericFamily::parse("SystemUi"), None);
    }

    #[test]
    fn case_and_whitespace_insensitive() {
        assert_eq!(
            GenericFamily::parse("  MONOSPACE  "),
            Some(GenericFamily::Monospace)
        );
        assert_eq!(
            GenericFamily::parse("Sans-Serif"),
            Some(GenericFamily::SansSerif)
        );
        assert_eq!(
            GenericFamily::parse("System-UI"),
            Some(GenericFamily::SystemUi)
        );
    }

    #[test]
    fn concrete_family_names_do_not_parse() {
        assert_eq!(GenericFamily::parse("Menlo"), None);
        assert_eq!(GenericFamily::parse(""), None);
        assert_eq!(GenericFamily::parse("   "), None);
        assert_eq!(GenericFamily::parse("Segoe UI"), None);
        assert_eq!(GenericFamily::parse("SF Pro Text"), None);
    }
}
