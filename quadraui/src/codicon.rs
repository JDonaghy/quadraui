//! Bundled codicon icon font.
//!
//! Every GUI backend paints its built-in chrome glyphs — tree
//! expand/collapse chevrons, tab dirty/close marks, the context-menu
//! submenu arrow, the data-table sort arrow — as codepoints from
//! Microsoft's codicon font, the vector icons VS Code's own chrome uses.
//! TUI paints plain Unicode characters (`▾`/`▸`/`×`/`●`/`▶`/`▼`) in the
//! same places. An app's own [`crate::Icon`] glyphs are a separate path:
//! they need a Nerd Font, which is not this crate's to bundle (it is
//! enormous and GPL-licensed), so the host supplies that font itself.
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
//! Nerd Fonts patches codicon's own glyphs into the *same* PUA range
//! (`U+EA60`–`U+EC1E`) rather than a disjoint one, so an app-supplied
//! Nerd Font can legitimately define a codepoint this module also
//! uses. What keeps that from shadowing the app's own glyph is
//! ordering, not code-space separation: every backend's codicon
//! registration call site (`GtkBackend::new`, `MacBackend::new`,
//! `WinBackend::new`) appends `FONT_FAMILY` *last* in its fallback
//! chain — GTK's family list, macOS's cascade list, Windows'
//! `AddMapping` order — so an app-supplied font earlier in that same
//! chain always wins any overlapping codepoint, and codicon only
//! resolves a codepoint nothing else in the chain claimed.

// `--features win` type-checks `win::*` on every host, but the WinAPI
// call sites that register the font and paint the tab close/dirty glyphs
// are `target_os = "windows"`-only, so a non-Windows `win`-only build
// compiles these items with no reader. The `allow(dead_code)` below is
// scoped to exactly that build; every real GUI build still lints them.

/// Raw TTF bytes of the bundled codicon font (codicon 0.0.46-24,
/// upstream: <https://github.com/microsoft/vscode-codicons>). Registered
/// once per process by each GUI backend via
/// [`crate::Backend::register_font_from_memory`]'s own implementation —
/// see this module's doc for why that happens unconditionally.
#[cfg_attr(
    not(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    )),
    allow(dead_code)
)]
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
#[cfg_attr(
    not(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    )),
    allow(dead_code)
)]
pub(crate) const FONT_FAMILY: &str = "codicon";

/// Branch row expanded — `codicon-chevron-down`, the GUI counterpart of
/// the plain `▾` [`crate::TreeStyle::chevron_expanded`] default paints on
/// TUI.
pub(crate) const CHEVRON_DOWN: char = '\u{eab4}';

/// Branch row collapsed — `codicon-chevron-right`, the GUI counterpart
/// of the plain `▸` [`crate::TreeStyle::chevron_collapsed`] default paints
/// on TUI.
/// Also used for the context-menu submenu pull-right affordance
/// (`\u{25b6}` on TUI) — codicon has no separate "submenu" glyph, and
/// VS Code's own context menu reuses this exact chevron for both.
pub(crate) const CHEVRON_RIGHT: char = '\u{eab6}';

/// Checked context-menu item indicator — `codicon-check` (TUI paints a
/// plain `✓`).
#[cfg_attr(
    not(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    )),
    allow(dead_code)
)]
pub(crate) const CHECK: char = '\u{eab2}';

/// Tab close button — `codicon-close` (TUI paints a plain `×`).
#[cfg_attr(
    not(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    )),
    allow(dead_code)
)]
pub(crate) const CLOSE: char = '\u{ea76}';

/// Tab dirty indicator (shown instead of the close glyph until
/// hovered) — `codicon-circle-filled` (TUI paints a plain `●`).
#[cfg_attr(
    not(any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    )),
    allow(dead_code)
)]
pub(crate) const DIRTY: char = '\u{ea71}';

/// Sort-ascending column-header indicator — `codicon-triangle-up`
/// (TUI paints a plain `▲`). Painted by every GUI data table: GTK's
/// own rasteriser (`gtk::data_table`) and the shared
/// `primitives::data_table::native_surface_paint` path macOS and
/// Win-GUI both go through.
pub(crate) const SORT_ASCENDING: char = '\u{eb71}';

/// Sort-descending column-header indicator — `codicon-triangle-down`
/// (TUI paints a plain `▼`). Painted by every GUI data table: GTK's
/// own rasteriser (`gtk::data_table`) and the shared
/// `primitives::data_table::native_surface_paint` path macOS and
/// Win-GUI both go through.
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

    /// Every glyph constant is a distinct codepoint inside the bundled
    /// font's own PUA coverage (`U+EA60`–`U+ECE7`, the actual `cmap`
    /// range `codicon.ttf` ships — wider than the `codicon.css` name →
    /// codepoint mapping this module's constants are drawn from, since
    /// the font includes glyphs no released `codicon.css` version names
    /// yet) — catches a transposed digit copied from `codicon.css`
    /// resolving to some other PUA glyph (or no glyph at all) instead
    /// of the intended one.
    #[test]
    fn glyph_constants_are_distinct_and_in_codicons_pua_block() {
        let glyphs = [
            CHEVRON_DOWN,
            CHEVRON_RIGHT,
            CHECK,
            CLOSE,
            DIRTY,
            SORT_ASCENDING,
            SORT_DESCENDING,
        ];
        for g in glyphs {
            assert!(
                ('\u{ea60}'..='\u{ece7}').contains(&g),
                "{g:?} (U+{:04X}) is outside the bundled font's PUA coverage",
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
        // resolving. No font-parsing dependency is pulled in for this —
        // `sfnt_family_name` below reads just enough of the `name`
        // table (Windows platform, nameID 1) to answer this one
        // question, rather than asserting the constant against its own
        // literal.
        let parsed = sfnt_family_name(FONT_BYTES)
            .expect("codicon.ttf must carry a Windows-platform Font Family (nameID 1) record");
        assert_eq!(
            parsed, FONT_FAMILY,
            "FONT_FAMILY must match the bundled font's own name table"
        );
    }

    /// Read the Windows-platform (platformID 3) Font Family name
    /// (nameID 1) out of an sfnt's `name` table — just enough of
    /// <https://learn.microsoft.com/en-us/typography/opentype/spec/name>
    /// to answer [`font_family_matches_the_bundled_fonts_own_name_table`]
    /// without a font-parsing dependency. Returns `None` if the font has
    /// no `name` table, no matching record, or either is malformed —
    /// every `None` path is deliberately a plain fallback rather than a
    /// panic, since this is test-only parsing of a known-good asset, not
    /// a path any real paint call goes through.
    fn sfnt_family_name(bytes: &[u8]) -> Option<String> {
        let num_tables = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
        let records_start = 12;
        let mut name_table_offset = None;
        for i in 0..num_tables {
            let rec = records_start + i * 16;
            let tag = bytes.get(rec..rec + 4)?;
            if tag == b"name" {
                let offset =
                    u32::from_be_bytes(bytes.get(rec + 8..rec + 12)?.try_into().ok()?) as usize;
                name_table_offset = Some(offset);
                break;
            }
        }
        let name_table = name_table_offset?;
        let count = u16::from_be_bytes(bytes.get(name_table + 2..name_table + 4)?.try_into().ok()?)
            as usize;
        let storage_offset =
            u16::from_be_bytes(bytes.get(name_table + 4..name_table + 6)?.try_into().ok()?)
                as usize;
        let records_start = name_table + 6;
        for i in 0..count {
            let rec = records_start + i * 12;
            let platform_id = u16::from_be_bytes(bytes.get(rec..rec + 2)?.try_into().ok()?);
            let name_id = u16::from_be_bytes(bytes.get(rec + 6..rec + 8)?.try_into().ok()?);
            if platform_id != 3 || name_id != 1 {
                continue;
            }
            let length =
                u16::from_be_bytes(bytes.get(rec + 8..rec + 10)?.try_into().ok()?) as usize;
            let str_offset =
                u16::from_be_bytes(bytes.get(rec + 10..rec + 12)?.try_into().ok()?) as usize;
            let start = name_table + storage_offset + str_offset;
            let raw = bytes.get(start..start + length)?;
            // Windows-platform name records are UTF-16BE.
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            return String::from_utf16(&units).ok();
        }
        None
    }
}
