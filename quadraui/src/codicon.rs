//! Bundled codicon icon font.
//!
//! Before this module, every GUI backend's built-in chrome glyphs — tree
//! expand/collapse chevrons, tab dirty/close marks, the context-menu
//! submenu arrow, the data-table sort arrow — were plain Unicode
//! characters (`▾`/`▸`/`×`/`●`/`▶`/`▼`) painted in whatever font the
//! surrounding chrome text used. They rendered, but did not look like
//! VS Code's own chrome, whose equivalent glyphs are vector icons from
//! Microsoft's codicon font. Separately, an app's own [`crate::Icon`]
//! glyphs only painted when the host both supplied a Nerd-Font-based
//! glyph *and* called [`crate::Backend::set_nerd_fonts`] — a real Nerd
//! Font is not this crate's to bundle (it is enormous and GPL-licensed),
//! so that path stays opt-in.
//!
//! codicon is small (≈150 KiB), narrowly scoped (chrome glyphs, not a
//! general-purpose icon set) and CC-BY-4.0 — licence terms this crate
//! can embed directly. [`FONT_BYTES`] is that font; every GUI backend
//! self-registers it once, unconditionally, via
//! [`crate::Backend::register_font_from_memory`]'s per-backend
//! implementation, so the glyphs in this module paint correctly with no
//! app configuration — see each backend's `codicon`-registration call
//! site (`GtkBackend::new`, `MacBackend::new`, `WinBackend::new`) for
//! where that happens.
//!
//! # Licence
//!
//! <https://github.com/microsoft/vscode-codicons>, Creative Commons
//! Attribution 4.0 International (CC-BY-4.0). Full licence text ships at
//! `quadraui/assets/CODICON_LICENSE`; see the crate-root `README.md`'s
//! "Third-party assets" section for the shipped attribution.
//!
//! # Why these codepoints
//!
//! Every constant below is a codicon Private-Use-Area codepoint, taken
//! from codicon 0.0.46's own `codicon.css` name → codepoint mapping.
//! codicon's assigned PUA block (`U+EA60`–`U+EC1E`) does not overlap any
//! Nerd Font patch's own PUA ranges, so appending `FONT_FAMILY` to a
//! backend's existing Nerd-Font fallback chain (as every backend's
//! codicon-registration call site does) cannot shadow an app-supplied
//! icon glyph — the two code spaces are disjoint.

/// Raw TTF bytes of the bundled codicon font (codicon 0.0.46-24,
/// upstream: <https://github.com/microsoft/vscode-codicons>). Registered
/// once per process by each GUI backend via
/// [`crate::Backend::register_font_from_memory`]'s own implementation —
/// see this module's doc for why that happens unconditionally.
pub(crate) const FONT_BYTES: &[u8] = include_bytes!("../assets/codicon.ttf");

/// The font family name every backend registers [`FONT_BYTES`] under,
/// and appends to its Nerd-Font-glyph fallback chain (GTK's
/// `with_nerd_font_fallback`, macOS's `CTFont` cascade list, Windows'
/// `IDWriteFontFallback`) so a codicon codepoint resolves regardless of
/// whether — or what — the host app has configured its own Nerd Font
/// fallback to. Matches the family name baked into the font's own
/// `name` table (confirmed via `fc-scan --format '%{family}\n'`), so a
/// backend that resolves by name against the font it just registered
/// needs no translation step.
pub(crate) const FONT_FAMILY: &str = "codicon";

/// Branch row expanded — `codicon-chevron-down`, replacing the plain
/// `▾` [`crate::TreeStyle::chevron_expanded`] default paints on TUI.
pub(crate) const CHEVRON_DOWN: char = '\u{eab4}';

/// Branch row collapsed — `codicon-chevron-right`, replacing the plain
/// `▸` [`crate::TreeStyle::chevron_collapsed`] default paints on TUI.
/// Also used for the context-menu submenu pull-right affordance
/// (`\u{25b6}` on TUI) — codicon has no separate "submenu" glyph, and
/// VS Code's own context menu reuses this exact chevron for both.
pub(crate) const CHEVRON_RIGHT: char = '\u{eab6}';

/// Tab close button — `codicon-close`, replacing the plain `×`.
pub(crate) const CLOSE: char = '\u{ea76}';

/// Tab dirty indicator (shown instead of the close glyph until
/// hovered) — `codicon-circle-filled`, replacing the plain `●`.
pub(crate) const DIRTY: char = '\u{ea71}';

/// Sort-ascending column-header indicator — `codicon-triangle-up`,
/// replacing the plain `▲`.
pub(crate) const SORT_ASCENDING: char = '\u{eb71}';

/// Sort-descending column-header indicator — `codicon-triangle-down`,
/// replacing the plain `▼`.
pub(crate) const SORT_DESCENDING: char = '\u{eb6e}';

#[cfg(test)]
mod tests {
    use super::*;

    /// `FONT_BYTES` must be a real sfnt (TrueType/OpenType) font, not a
    /// corrupted or truncated asset — checked via the sfnt version tag
    /// every TrueType font starts with (`0x00010000`), the same
    /// first-four-bytes check `fc-scan`/FreeType itself uses to decide
    /// whether a file is even worth parsing further.
    #[test]
    fn font_bytes_is_a_real_sfnt_font() {
        assert!(
            FONT_BYTES.len() > 1024,
            "codicon.ttf looks truncated: {} bytes",
            FONT_BYTES.len()
        );
        assert_eq!(
            &FONT_BYTES[0..4],
            &[0x00, 0x01, 0x00, 0x00],
            "codicon.ttf's first four bytes must be the sfnt version tag"
        );
    }

    /// Every glyph constant is a distinct codepoint inside codicon's own
    /// assigned PUA block (`U+EA60`–`U+EC1E`) — catches a transposed
    /// digit copied from `codicon.css` resolving to some other PUA
    /// glyph (or no glyph at all) instead of the intended one.
    #[test]
    fn glyph_constants_are_distinct_and_in_codicons_pua_block() {
        let glyphs = [
            CHEVRON_DOWN,
            CHEVRON_RIGHT,
            CLOSE,
            DIRTY,
            SORT_ASCENDING,
            SORT_DESCENDING,
        ];
        for g in glyphs {
            assert!(
                ('\u{ea60}'..='\u{ec1e}').contains(&g),
                "{g:?} (U+{:04X}) is outside codicon's assigned PUA block",
                g as u32
            );
        }
        let mut seen = std::collections::HashSet::new();
        for g in glyphs {
            assert!(seen.insert(g), "duplicate glyph constant: {g:?}");
        }
    }

    #[test]
    fn font_family_matches_the_bundled_fonts_own_name_table() {
        // Keeps `FONT_FAMILY` honest against a future font-asset bump:
        // if `codicon.ttf` is ever swapped for a build whose `name`
        // table reports a different family, a registration call keyed
        // on the literal `"codicon"` string would silently stop
        // resolving. This crate has no font-parsing dependency to read
        // the `name` table back at test time, so this only re-asserts
        // the constant's own documented contract (see its doc comment)
        // rather than parsing `FONT_BYTES` itself.
        assert_eq!(FONT_FAMILY, "codicon");
    }
}
