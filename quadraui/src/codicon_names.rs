//! Name → codepoint table backing [`crate::Icon::codicon`].
//!
//! Lets an app request one of the bundled codicon font's icons by its
//! codicon.css name (`"folder"`, `"file"`, …) instead of a raw PUA
//! codepoint. Deliberately **not** gated behind any backend feature —
//! unlike `codicon.rs` (the font bytes themselves, plus the chrome-glyph
//! constants every GUI rasteriser paints), this table is plain data
//! [`crate::Icon::codicon`] needs on every build, including a TUI-only or
//! even a zero-backend-feature one: the resulting `Icon::glyph` is just a
//! `char` resolved from this table, painted through the same
//! `Icon::resolve` path any other app-supplied glyph already goes
//! through (see [`crate::Icon::codicon`]'s doc).
//!
//! A curated subset of codicon 0.0.46's ~500-icon set — file-explorer,
//! version-control and common chrome icons, the ones an app building a
//! VS-Code-like sidebar or activity bar actually reaches for. Every
//! codepoint here is independently verified against the bundled font's
//! own `cmap` table by this module's tests, and against the upstream
//! `name → codepoint` mapping published at
//! <https://github.com/microsoft/vscode-codicons/blob/main/src/template/mapping.json>
//! (codicon 0.0.46-24, the version `codicon.rs`'s `FONT_BYTES` embeds).
//! Browse the full icon set (names and glyphs together) at
//! <https://microsoft.github.io/vscode-codicons/dist/codicon.html> — most
//! of those names resolve in the bundled font too, but only the ones
//! below are reachable through [`crate::Icon::codicon`]; extend
//! [`NAMES`] (with a matching `cmap` entry, checked by this module's
//! tests) to add more.

pub(crate) const NAMES: &[(&str, char)] = &[
    ("folder", '\u{ea83}'),
    ("folder-opened", '\u{eaf7}'),
    ("root-folder", '\u{eb46}'),
    ("root-folder-opened", '\u{eb45}'),
    ("new-folder", '\u{ea80}'),
    ("symbol-folder", '\u{ea83}'),
    ("file", '\u{ea7b}'),
    ("files", '\u{eaf0}'),
    ("new-file", '\u{ea7f}'),
    ("file-code", '\u{eae9}'),
    ("file-media", '\u{eaea}'),
    ("file-pdf", '\u{eaeb}'),
    ("file-submodule", '\u{eaec}'),
    ("file-zip", '\u{eaef}'),
    ("symbol-file", '\u{eb60}'),
    ("json", '\u{eb0f}'),
    ("markdown", '\u{eb1d}'),
    ("gear", '\u{eaf8}'),
    ("settings-gear", '\u{eb51}'),
    ("search", '\u{ea6d}'),
    ("trash", '\u{ea81}'),
    ("edit", '\u{ea73}'),
    ("save", '\u{eb4b}'),
    ("git-commit", '\u{eafc}'),
    // Upstream codicon.css's own `git-branch` codepoint (`U+EC6F`) falls
    // outside the `U+EA60`-`U+EC1E` range Nerd Fonts patches codicon
    // into, so a Nerd-Font-patched terminal paints tofu at that
    // codepoint. `U+EA68` ("source-control") maps to the identical
    // glyph in the bundled font (same outline, glyph id 121), so using
    // it here keeps the GUI pixel-identical while staying inside the
    // patched range on TUI.
    ("git-branch", '\u{ea68}'),
    ("source-control", '\u{ea68}'),
    ("terminal", '\u{ea85}'),
    ("package", '\u{eb29}'),
    ("extensions", '\u{eae6}'),
    ("account", '\u{eb99}'),
    ("bug", '\u{eaaf}'),
    ("debug-alt", '\u{eb91}'),
    ("circle-filled", '\u{ea71}'),
    ("warning", '\u{ea6c}'),
    ("error", '\u{ea87}'),
    ("info", '\u{ea74}'),
    ("pass-filled", '\u{ebb3}'),
];

/// Resolve a codicon.css name (e.g. `"folder"`) to its codepoint in the
/// bundled font — the lookup behind [`crate::Icon::codicon`]. `None` if
/// `name` isn't in [`NAMES`].
pub(crate) fn codepoint(name: &str) -> Option<char> {
    NAMES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`NAMES`] entries are distinct names, each mapping to a codepoint
    /// inside the bundled font's PUA coverage. Codepoints themselves may
    /// repeat — upstream codicon.css aliases several names to the same
    /// glyph (`"folder"` and `"symbol-folder"` both resolve to
    /// `codicon-folder`) — so only name uniqueness is asserted here.
    #[test]
    fn names_are_distinct_and_map_to_in_range_codepoints() {
        let mut seen_names = std::collections::HashSet::new();
        for (name, c) in NAMES {
            assert!(seen_names.insert(*name), "duplicate codicon name: {name:?}");
            assert!(
                ('\u{ea60}'..='\u{ece7}').contains(c),
                "{name:?} (U+{:04X}) is outside the bundled font's PUA coverage",
                *c as u32
            );
        }
    }

    /// Every [`NAMES`] codepoint resolves to a real (non-`.notdef`) glyph
    /// in the bundled font's own `cmap` table. Run unconditionally (not
    /// gated behind a GUI feature, unlike `codicon.rs`'s own cmap test)
    /// by reading `codicon.ttf` directly here — see this module's doc for
    /// why [`NAMES`] has no feature gate to piggy-back on.
    #[test]
    fn names_resolve_to_a_real_glyph_in_the_bundled_fonts_cmap() {
        let font_bytes = include_bytes!("../assets/codicon.ttf");
        for (name, c) in NAMES {
            let glyph_id = cmap_glyph_id(font_bytes, *c as u32)
                .unwrap_or_else(|| panic!("codicon.ttf's cmap table could not be parsed at all"));
            assert_ne!(
                glyph_id, 0,
                "{name:?} (U+{:04X}) has no glyph mapped in codicon.ttf's own cmap table",
                *c as u32
            );
        }
    }

    /// [`NAMES`] entries resolved codepoint matches the glyph the bundled
    /// font's own `post` table (format 2.0) names that codepoint —
    /// not just *some* glyph, the *named* one. A handful of
    /// [`NAMES`] entries deliberately point at a different upstream
    /// codicon.css name's codepoint because the two share one glyph
    /// outline (an upstream alias dedup, or — for `"git-branch"` — a
    /// deliberate swap onto the Nerd-Font-patched range; see [`NAMES`]'s
    /// `"git-branch"` entry comment), so those are listed in
    /// `ALIASES` rather than failing this check.
    #[test]
    fn names_resolve_to_the_matching_glyph_name_in_the_bundled_fonts_post_table() {
        /// `(our_name, the_post_table_name_its_codepoint_actually_resolves_to)`
        /// — every deliberate alias dedup upstream codicon.css itself makes,
        /// plus `"git-branch"`'s deliberate swap onto `"source-control"`'s
        /// codepoint (see [`NAMES`]).
        const ALIASES: &[(&str, &str)] = &[
            ("symbol-folder", "folder"),
            ("file", "symbol-file"),
            ("json", "symbol-namespace"),
            ("git-branch", "source-control"),
        ];

        let font_bytes = include_bytes!("../assets/codicon.ttf");
        let post_names = post_table_glyph_names(font_bytes)
            .unwrap_or_else(|| panic!("codicon.ttf's post table could not be parsed at all"));
        for (name, c) in NAMES {
            let glyph_id = cmap_glyph_id(font_bytes, *c as u32).unwrap_or(0);
            let post_name = post_names
                .get(glyph_id as usize)
                .and_then(|n| n.as_deref())
                .unwrap_or_else(|| {
                    panic!("{name:?} (U+{:04X}, glyph id {glyph_id}) has no post-table name — font's post table does not cover standard Macintosh glyph ids", *c as u32)
                });
            let expected = ALIASES
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, expected)| *expected)
                .unwrap_or(name);
            assert_eq!(
                post_name, expected,
                "{name:?} (U+{:04X}) resolves to glyph {post_name:?} in codicon.ttf's post table, expected {expected:?}",
                *c as u32
            );
        }
    }

    /// [`codepoint`] is the lookup [`NAMES`] backs: a known name resolves
    /// to its table entry, an unknown one is `None`.
    #[test]
    fn codepoint_looks_up_known_names_and_rejects_unknown_ones() {
        assert_eq!(codepoint("folder"), Some('\u{ea83}'));
        assert_eq!(codepoint("file"), Some('\u{ea7b}'));
        assert_eq!(codepoint("not-a-real-codicon-name"), None);
    }

    /// Look up a Unicode codepoint's glyph ID in an sfnt's `cmap` table —
    /// just enough of
    /// <https://learn.microsoft.com/en-us/typography/opentype/spec/cmap>
    /// to answer [`names_resolve_to_a_real_glyph_in_the_bundled_fonts_cmap`]
    /// without a font-parsing dependency. A copy of `codicon.rs`'s own
    /// test-only helper of the same name — duplicated rather than shared
    /// because `codicon.rs` is feature-gated and this module deliberately
    /// isn't (see this module's doc).
    fn cmap_glyph_id(bytes: &[u8], codepoint: u32) -> Option<u32> {
        let num_tables = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
        let records_start = 12;
        let mut cmap_offset = None;
        for i in 0..num_tables {
            let rec = records_start + i * 16;
            let tag = bytes.get(rec..rec + 4)?;
            if tag == b"cmap" {
                let offset =
                    u32::from_be_bytes(bytes.get(rec + 8..rec + 12)?.try_into().ok()?) as usize;
                cmap_offset = Some(offset);
                break;
            }
        }
        let cmap = cmap_offset?;
        let num_subtables =
            u16::from_be_bytes(bytes.get(cmap + 2..cmap + 4)?.try_into().ok()?) as usize;
        let mut best_subtable_offset = None;
        for i in 0..num_subtables {
            let rec = cmap + 4 + i * 8;
            let platform_id = u16::from_be_bytes(bytes.get(rec..rec + 2)?.try_into().ok()?);
            let encoding_id = u16::from_be_bytes(bytes.get(rec + 2..rec + 4)?.try_into().ok()?);
            let offset = u32::from_be_bytes(bytes.get(rec + 4..rec + 8)?.try_into().ok()?) as usize;
            let is_better = match (platform_id, encoding_id) {
                (3, 10) => true,
                (3, 1) if best_subtable_offset.is_none() => true,
                _ => false,
            };
            if is_better {
                best_subtable_offset = Some(cmap + offset);
            }
        }
        let sub = best_subtable_offset?;
        let format = u16::from_be_bytes(bytes.get(sub..sub + 2)?.try_into().ok()?);
        match format {
            4 => cmap_format4_glyph_id(bytes, sub, codepoint),
            12 => cmap_format12_glyph_id(bytes, sub, codepoint),
            _ => None,
        }
    }

    /// Format-4 (segment mapping to delta values) `cmap` subtable lookup.
    fn cmap_format4_glyph_id(bytes: &[u8], sub: usize, codepoint: u32) -> Option<u32> {
        if codepoint > 0xFFFF {
            return Some(0);
        }
        let c = codepoint as u16;
        let seg_count_x2 =
            u16::from_be_bytes(bytes.get(sub + 6..sub + 8)?.try_into().ok()?) as usize;
        let seg_count = seg_count_x2 / 2;
        let end_codes_start = sub + 14;
        let start_codes_start = end_codes_start + seg_count_x2 + 2; // +2 reservedPad
        let id_deltas_start = start_codes_start + seg_count_x2;
        let id_range_offsets_start = id_deltas_start + seg_count_x2;
        for i in 0..seg_count {
            let end_code = u16::from_be_bytes(
                bytes
                    .get(end_codes_start + i * 2..end_codes_start + i * 2 + 2)?
                    .try_into()
                    .ok()?,
            );
            if c > end_code {
                continue;
            }
            let start_code = u16::from_be_bytes(
                bytes
                    .get(start_codes_start + i * 2..start_codes_start + i * 2 + 2)?
                    .try_into()
                    .ok()?,
            );
            if c < start_code {
                return Some(0);
            }
            let id_delta = i16::from_be_bytes(
                bytes
                    .get(id_deltas_start + i * 2..id_deltas_start + i * 2 + 2)?
                    .try_into()
                    .ok()?,
            );
            let id_range_offset_pos = id_range_offsets_start + i * 2;
            let id_range_offset = u16::from_be_bytes(
                bytes
                    .get(id_range_offset_pos..id_range_offset_pos + 2)?
                    .try_into()
                    .ok()?,
            );
            if id_range_offset == 0 {
                return Some(((c as i32 + id_delta as i32) as u16) as u32);
            }
            let glyph_index_addr =
                id_range_offset_pos + id_range_offset as usize + 2 * (c - start_code) as usize;
            let raw_glyph_id = u16::from_be_bytes(
                bytes
                    .get(glyph_index_addr..glyph_index_addr + 2)?
                    .try_into()
                    .ok()?,
            );
            if raw_glyph_id == 0 {
                return Some(0);
            }
            return Some(((raw_glyph_id as i32 + id_delta as i32) as u16) as u32);
        }
        Some(0)
    }

    /// Glyph id → name, read from an sfnt's `post` table. `None` whole-table
    /// if the table isn't present or isn't format 2.0 (the only format
    /// carrying per-glyph names); `None` per-entry for a glyph id below
    /// 258 — this font's names are all custom (PUA icon glyphs, never one
    /// of the 258 standard Macintosh glyph names), which
    /// [`names_resolve_to_the_matching_glyph_name_in_the_bundled_fonts_post_table`]
    /// relies on rather than embedding that 258-name table here.
    fn post_table_glyph_names(bytes: &[u8]) -> Option<Vec<Option<String>>> {
        let num_tables = u16::from_be_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
        let records_start = 12;
        let mut post_offset = None;
        for i in 0..num_tables {
            let rec = records_start + i * 16;
            let tag = bytes.get(rec..rec + 4)?;
            if tag == b"post" {
                let offset =
                    u32::from_be_bytes(bytes.get(rec + 8..rec + 12)?.try_into().ok()?) as usize;
                post_offset = Some(offset);
                break;
            }
        }
        let post = post_offset?;
        let version = u32::from_be_bytes(bytes.get(post..post + 4)?.try_into().ok()?);
        if version != 0x0002_0000 {
            return None;
        }
        let num_glyphs = u16::from_be_bytes(bytes.get(post + 32..post + 34)?.try_into().ok()?);
        let index_start = post + 34;
        let mut indices = Vec::with_capacity(num_glyphs as usize);
        for i in 0..num_glyphs as usize {
            let idx = u16::from_be_bytes(
                bytes
                    .get(index_start + i * 2..index_start + i * 2 + 2)?
                    .try_into()
                    .ok()?,
            );
            indices.push(idx);
        }

        // Pascal strings (one length byte + that many bytes) immediately
        // follow the index array, one per custom-name slot — slot count
        // is `max(glyphNameIndex) - 258 + 1`, not the number of glyphs
        // referencing a custom name, since two glyph ids may point at the
        // same slot. Stop there rather than scanning to EOF, since
        // nothing delimits the end of this region from whatever table
        // follows it in the sfnt.
        let needed = indices
            .iter()
            .filter(|idx| **idx >= 258)
            .map(|idx| *idx as usize - 258 + 1)
            .max()
            .unwrap_or(0);
        let mut custom_names = Vec::new();
        let mut pos = index_start + num_glyphs as usize * 2;
        while custom_names.len() < needed {
            let len = *bytes.get(pos)? as usize;
            pos += 1;
            let name = std::str::from_utf8(bytes.get(pos..pos + len)?)
                .ok()?
                .to_string();
            pos += len;
            custom_names.push(name);
        }

        Some(
            indices
                .into_iter()
                .map(|idx| {
                    if idx >= 258 {
                        custom_names.get(idx as usize - 258).cloned()
                    } else {
                        None
                    }
                })
                .collect(),
        )
    }

    /// Format-12 (segmented coverage) `cmap` subtable lookup.
    fn cmap_format12_glyph_id(bytes: &[u8], sub: usize, codepoint: u32) -> Option<u32> {
        let num_groups =
            u32::from_be_bytes(bytes.get(sub + 12..sub + 16)?.try_into().ok()?) as usize;
        let groups_start = sub + 16;
        for i in 0..num_groups {
            let rec = groups_start + i * 12;
            let start_char = u32::from_be_bytes(bytes.get(rec..rec + 4)?.try_into().ok()?);
            let end_char = u32::from_be_bytes(bytes.get(rec + 4..rec + 8)?.try_into().ok()?);
            if codepoint < start_char || codepoint > end_char {
                continue;
            }
            let start_glyph = u32::from_be_bytes(bytes.get(rec + 8..rec + 12)?.try_into().ok()?);
            return Some(start_glyph + (codepoint - start_char));
        }
        Some(0)
    }
}
