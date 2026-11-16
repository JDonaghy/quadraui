//! Kitty terminal graphics protocol: pure byte/escape-sequence encoding.
//!
//! This module turns already-decoded pixels (see
//! [`crate::tui::image_decode`], behind the separate `tui-images`
//! feature) into the raw APC escape sequences the protocol defines:
//! transmit (`a=t`), virtual placement (`a=p,U=1`), delete (`a=d`), and
//! the Unicode-placeholder cell content a host paints those placements
//! through. No terminal capability detection and no dependency on an
//! image decoder lives here — see `quadraui::tui::caps` for the former
//! and [`crate::tui::image`]'s `draw_image_kitty` for the paint-path
//! caller that turns a decoded [`crate::tui::image_decode::DecodedImage`]
//! into calls against this module's encoders.
//!
//! Every reference below (`<ESC>_G...`, the `a`/`f`/`s`/`v`/`i`/`m`/`q`/
//! `d`/`U`/`c`/`r` keys, the row/column diacritic table) is the Kitty
//! graphics protocol spec: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
//! and its `rowcolumn-diacritics.txt` data file.
//!
//! # Chunking
//!
//! A payload over [`CHUNK_SIZE`] base64 bytes is split across multiple
//! APC sequences: the first chunk carries the full control data plus
//! `m=1`; every chunk but the last carries only `m=1`; the last chunk
//! carries only `m=0`. `q=2` (suppress both OK and error terminal
//! responses) is set once, on the first chunk — the spec requires
//! subsequent chunks carry no key but `m` (and optionally `q`, which
//! this module does not repeat).
//!
//! # tmux passthrough
//!
//! [`tmux_passthrough_wrap`] and [`base64_encode`] are generic terminal
//! escape-sequence plumbing, not graphics-specific — `tui::services`'
//! OSC 52 clipboard-write sequence reuses both rather than carrying its
//! own copy.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::primitives::image::ImageSource;
use crate::types::Color;

/// Maximum base64 payload bytes per APC chunk (the protocol's own
/// limit — see the module doc's "Chunking" section).
pub(crate) const CHUNK_SIZE: usize = 4096;

/// Base64-encode `data` using the standard alphabet (no line wrapping).
///
/// Hand-rolled rather than pulling in a `base64` crate dependency: this
/// crate's dependency footprint already avoids adding a new crate for a
/// few dozen lines of well-tested arithmetic — see `tui::services`'s own
/// prior copy of this, now unified here.
pub(crate) fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() {
            data[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < data.len() {
            data[i + 2] as u32
        } else {
            0
        };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 0x3f) as usize] as char);
        out.push(CHARS[((n >> 12) & 0x3f) as usize] as char);
        if i + 1 < data.len() {
            out.push(CHARS[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(CHARS[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

/// Wrap a terminal escape sequence in tmux's DCS passthrough so tmux
/// forwards it verbatim to the outer terminal: `ESC P tmux ; <seq> ESC \`,
/// with every inner `ESC` doubled (tmux's escaping rule). Requires
/// `allow-passthrough on` in the tmux config.
pub(crate) fn tmux_passthrough_wrap(seq: &str) -> String {
    let escaped = seq.replace('\x1b', "\x1b\x1b");
    format!("\x1bPtmux;{escaped}\x1b\\")
}

/// Build one raw APC escape sequence: `ESC _G <control-data>[;<payload>]
/// ESC \`. `payload` is omitted (and the `;` separator with it) when
/// `None` — the protocol treats a payload-less command (e.g. `a=p`,
/// `a=d`) differently from one with an explicit empty payload.
fn apc(control_data: &str, payload: Option<&str>) -> String {
    match payload {
        Some(p) => format!("\x1b_G{control_data};{p}\x1b\\"),
        None => format!("\x1b_G{control_data}\x1b\\"),
    }
}

/// Split `payload_b64` into [`CHUNK_SIZE`]-byte APC sequences, with
/// `control_data` (the `a=t,f=...,i=...` keys) on the first chunk only,
/// `m=1`/`m=0` on every chunk, and `q=2` added once, on the first chunk.
/// `base64_encode`'s output is pure ASCII, so byte and `char` boundaries
/// coincide — slicing by byte offset never splits a multi-byte char.
fn chunk_transmit(control_data: &str, payload_b64: &str) -> Vec<String> {
    let total = payload_b64.len();
    if total == 0 {
        return vec![apc(&format!("{control_data},q=2,m=0"), Some(""))];
    }
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < total {
        let end = (offset + CHUNK_SIZE).min(total);
        let is_last = end == total;
        let flag = if is_last { 0 } else { 1 };
        let chunk = &payload_b64[offset..end];
        let control = if offset == 0 {
            format!("{control_data},q=2,m={flag}")
        } else {
            format!("m={flag}")
        };
        out.push(apc(&control, Some(chunk)));
        offset = end;
    }
    out
}

/// Transmit raw RGBA8 pixels (`f=32`) for image `id`, chunked per
/// [`chunk_transmit`]. `rgba` must be exactly `width * height * 4` bytes
/// — callers get that shape for free from
/// [`crate::tui::image_decode::DecodedImage`].
///
/// Only called from `tui::image::draw_image_kitty`, which is itself
/// behind the `tui-images` Cargo feature — unused (and `allow`ed) on a
/// bare `tui` build with no image decoder to feed it.
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) fn transmit_rgba(id: u32, width: u32, height: u32, rgba: &[u8]) -> Vec<String> {
    let payload = base64_encode(rgba);
    chunk_transmit(&format!("a=t,f=32,i={id},s={width},v={height}"), &payload)
}

/// Transmit already-encoded PNG bytes directly (`f=100` passthrough —
/// no re-encode through [`crate::tui::image_decode`]'s RGBA path), for a
/// source that is already a small-enough PNG. Chunked per
/// [`chunk_transmit`]; width/height are read from the PNG data itself by
/// the terminal, so no `s`/`v` keys are needed.
///
/// No paint-path caller yet — `tui::image::draw_image_kitty` always
/// decodes through [`crate::tui::image_decode`]'s RGBA path (one shape
/// to cache and transmit, matching every other primitive's decode-to-
/// RGBA backends), so this PNG-passthrough fast path stays unused until
/// a caller wants to skip that re-encode for a source already small
/// enough to send as-is.
#[allow(dead_code)]
pub(crate) fn transmit_png(id: u32, png_bytes: &[u8]) -> Vec<String> {
    let payload = base64_encode(png_bytes);
    chunk_transmit(&format!("a=t,f=100,i={id}"), &payload)
}

/// Create a virtual placement for image `id`, shown through
/// `cols`×`rows` Unicode placeholder cells (`a=p,U=1`) — see the module
/// doc's spec link, "Unicode placeholders" section. No payload.
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) fn place_virtual(id: u32, cols: u32, rows: u32) -> String {
    apc(&format!("a=p,U=1,i={id},c={cols},r={rows},q=2"), None)
}

/// Delete image `id` and free its stored data (`a=d,d=I` — the
/// uppercase `I` variant frees data; see the spec's delete-value table).
///
/// Same `tui-images`-only reachability as [`transmit_rgba`] —
/// `TuiBackend::end_frame`'s only caller is behind that feature; the
/// session-teardown `delete_all` sibling below has no such gate.
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) fn delete_image(id: u32) -> String {
    apc(&format!("a=d,d=I,i={id},q=2"), None)
}

/// Delete every image and free all stored data (`a=d,d=A`) — teardown,
/// e.g. on shutdown or a full-screen redraw that invalidates every id.
pub(crate) fn delete_all() -> String {
    apc("a=d,d=A,q=2", None)
}

/// Base Unicode placeholder character (`U+10EEEE`) every placeholder
/// cell starts with, before its row/column diacritics.
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) const PLACEHOLDER_BASE: char = '\u{10EEEE}';

/// Row/column diacritics table, in index order (index 0 encodes row/col
/// `0`, index 1 encodes `1`, …), copied verbatim from the protocol's
/// `rowcolumn-diacritics.txt` (combining marks of class 230 with no
/// decomposition mapping, Unicode 6.0.0). 297 entries — a placeholder
/// grid wider or taller than that has no diacritic and falls back to
/// the "inherit from the cell to the left" rule the spec describes,
/// which this module does not implement — `tui::image::draw_image_kitty`
/// simply skips a cell past this range (see [`placeholder_cell_text`]'s
/// `None` case).
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
#[rustfmt::skip]
const ROWCOL_DIACRITICS: [char; 297] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}',
    '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}',
    '\u{05A8}', '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
    '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}', '\u{06D7}', '\u{06D8}',
    '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}',
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}',
    '\u{0735}', '\u{0736}', '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}',
    '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}', '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}',
    '\u{07EF}', '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}', '\u{082B}', '\u{082C}',
    '\u{082D}', '\u{0951}', '\u{0953}', '\u{0954}', '\u{0F82}', '\u{0F83}', '\u{0F86}', '\u{0F87}',
    '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}',
    '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', '\u{1B6B}', '\u{1B6D}',
    '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}',
    '\u{1DC5}', '\u{1DC6}', '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}',
    '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}',
    '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', '\u{1DE0}', '\u{1DE1}',
    '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}',
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}',
    '\u{20E9}', '\u{20F0}', '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}',
    '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}',
    '\u{2DEB}', '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}', '\u{2DF2}',
    '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A67C}', '\u{A67D}',
    '\u{A6F0}', '\u{A6F1}', '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}',
    '\u{A8E6}', '\u{A8E7}', '\u{A8E8}', '\u{A8E9}', '\u{A8EA}', '\u{A8EB}', '\u{A8EC}', '\u{A8ED}',
    '\u{A8EE}', '\u{A8EF}', '\u{A8F0}', '\u{A8F1}', '\u{AAB0}', '\u{AAB2}', '\u{AAB3}', '\u{AAB7}',
    '\u{AAB8}', '\u{AABE}', '\u{AABF}', '\u{AAC1}', '\u{FE20}', '\u{FE21}', '\u{FE22}', '\u{FE23}',
    '\u{FE24}', '\u{FE25}', '\u{FE26}', '\u{10A0F}', '\u{10A38}', '\u{1D185}', '\u{1D186}', '\u{1D187}',
    '\u{1D188}', '\u{1D189}', '\u{1D1AA}', '\u{1D1AB}', '\u{1D1AC}', '\u{1D1AD}', '\u{1D242}', '\u{1D243}',
    '\u{1D244}',
];

/// Look up the diacritic for row/column index `n` — `None` past the
/// table's 297-entry range (see [`ROWCOL_DIACRITICS`]'s doc).
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) fn placeholder_diacritic(n: u32) -> Option<char> {
    ROWCOL_DIACRITICS.get(n as usize).copied()
}

/// Build one placeholder cell's text content: `U+10EEEE` followed by
/// the row diacritic then the column diacritic — `None` if either index
/// is out of the table's range.
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) fn placeholder_cell_text(row: u32, col: u32) -> Option<String> {
    let row_d = placeholder_diacritic(row)?;
    let col_d = placeholder_diacritic(col)?;
    let mut s =
        String::with_capacity(PLACEHOLDER_BASE.len_utf8() + row_d.len_utf8() + col_d.len_utf8());
    s.push(PLACEHOLDER_BASE);
    s.push(row_d);
    s.push(col_d);
    Some(s)
}

/// Foreground colour a placeholder cell must be painted with to carry
/// image `id` (low 24 bits — see the module doc's spec link for the
/// most-significant-byte third diacritic this module doesn't need since
/// [`crate::tui::image_decode`] never allocates an id above 24 bits).
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) fn placeholder_fg_color(id: u32) -> Color {
    Color::rgb(
        ((id >> 16) & 0xff) as u8,
        ((id >> 8) & 0xff) as u8,
        (id & 0xff) as u8,
    )
}

/// Identifies one placed image for [`IdRegistry`]: the decoded source
/// plus the terminal cell rect it occupies. A different cell size needs
/// its own transmit (and thus its own kitty image id) — same
/// granularity as `crate::image_cache`'s own cache key.
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) struct ImageKey {
    source_hash: u64,
    cols: u32,
    rows: u32,
}

impl ImageKey {
    #[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
    pub(crate) fn new(source: &ImageSource, cols: u32, rows: u32) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        Self {
            source_hash: hasher.finish(),
            cols,
            rows,
        }
    }
}

/// One entry [`IdRegistry`] tracks: the allocated kitty image id, and how
/// many frames have passed since it was last touched.
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
struct LiveEntry {
    id: u32,
    idle_frames: u32,
}

/// Whether [`IdRegistry::touch`] allocated a fresh id (needs transmitting
/// this frame) or found one already live (already transmitted, just
/// needs placing).
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) enum IdLookup {
    New(u32),
    Live(u32),
}

/// Tracks which kitty image ids are currently live across frames, so a
/// caller transmits each source/size combination exactly once and knows
/// when to free an id that stopped appearing. Owned per-session by
/// `tui::image::KittyImageState`, driven once per frame from
/// `TuiBackend::begin_frame`/`end_frame`.
///
/// Same `tui-images`-only reachability as [`transmit_rgba`].
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
pub(crate) struct IdRegistry {
    next_id: u32,
    entries: HashMap<ImageKey, LiveEntry>,
}

// Every method here shares `ImageKey`/`IdRegistry`'s own `tui-images`-
// only reachability (see those types' docs) — one blanket allow for
// the whole `impl` instead of repeating it per method.
#[cfg_attr(not(feature = "tui-images"), allow(dead_code))]
impl IdRegistry {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            entries: HashMap::new(),
        }
    }

    /// Age every tracked entry by one frame — call once per frame,
    /// before [`Self::touch`]ing every image painted that frame.
    pub(crate) fn begin_frame(&mut self) {
        for entry in self.entries.values_mut() {
            entry.idle_frames += 1;
        }
    }

    /// Look up or allocate the id for `key`, resetting its idle counter —
    /// call once per frame for every image actually painted that frame.
    pub(crate) fn touch(&mut self, key: ImageKey) -> IdLookup {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.idle_frames = 0;
            return IdLookup::Live(entry.id);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.entries.insert(key, LiveEntry { id, idle_frames: 0 });
        IdLookup::New(id)
    }

    /// Remove and return the ids of every entry idle for at least
    /// `max_idle_frames` frames (not [`Self::touch`]ed since) — the
    /// caller emits [`delete_image`] for each.
    pub(crate) fn expire(&mut self, max_idle_frames: u32) -> Vec<u32> {
        let expired: Vec<ImageKey> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.idle_frames >= max_idle_frames)
            .map(|(key, _)| key.clone())
            .collect();
        expired
            .into_iter()
            .filter_map(|key| self.entries.remove(&key).map(|entry| entry.id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64_hello() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn tmux_passthrough_doubles_every_esc() {
        let wrapped = tmux_passthrough_wrap("\x1bA\x1bB");
        assert_eq!(wrapped, "\x1bPtmux;\x1b\x1bA\x1b\x1bB\x1b\\");
    }

    #[test]
    fn transmit_rgba_single_chunk_exact_bytes() {
        // 1x1 opaque red pixel: 4 bytes, well under CHUNK_SIZE.
        let rgba = [0xffu8, 0x00, 0x00, 0xff];
        let seqs = transmit_rgba(42, 1, 1, &rgba);
        assert_eq!(seqs.len(), 1, "data fits in a single chunk");
        let payload = base64_encode(&rgba);
        assert_eq!(
            seqs[0],
            format!("\x1b_Ga=t,f=32,i=42,s=1,v=1,q=2,m=0;{payload}\x1b\\")
        );
    }

    #[test]
    fn transmit_png_single_chunk_exact_bytes() {
        let png = b"not a real png but transmit_png doesn't care";
        let seqs = transmit_png(7, png);
        assert_eq!(seqs.len(), 1);
        let payload = base64_encode(png);
        assert_eq!(
            seqs[0],
            format!("\x1b_Ga=t,f=100,i=7,q=2,m=0;{payload}\x1b\\")
        );
    }

    #[test]
    fn transmit_rgba_multi_chunk_splits_at_chunk_size_with_m_flags() {
        // CHUNK_SIZE + 1 bytes of base64 payload forces exactly 2 chunks.
        let rgba = vec![0xabu8; (CHUNK_SIZE + 1) * 3 / 4 + 1];
        let seqs = transmit_rgba(1, 10, 10, &rgba);
        assert_eq!(seqs.len(), 2, "payload over CHUNK_SIZE must split");

        let full_payload = base64_encode(&rgba);
        let (first_b64, second_b64) = full_payload.split_at(CHUNK_SIZE);

        assert_eq!(
            seqs[0],
            format!("\x1b_Ga=t,f=32,i=1,s=10,v=10,q=2,m=1;{first_b64}\x1b\\"),
            "first chunk carries full control data plus m=1"
        );
        assert_eq!(
            seqs[1],
            format!("\x1b_Gm=0;{second_b64}\x1b\\"),
            "last chunk carries only m=0"
        );
    }

    #[test]
    fn place_virtual_exact_bytes() {
        assert_eq!(
            place_virtual(42, 8, 4),
            "\x1b_Ga=p,U=1,i=42,c=8,r=4,q=2\x1b\\"
        );
    }

    #[test]
    fn delete_image_exact_bytes() {
        assert_eq!(delete_image(10), "\x1b_Ga=d,d=I,i=10,q=2\x1b\\");
    }

    #[test]
    fn delete_all_exact_bytes() {
        assert_eq!(delete_all(), "\x1b_Ga=d,d=A,q=2\x1b\\");
    }

    #[test]
    fn placeholder_diacritics_match_spec_worked_example() {
        // The spec's own worked example: row 0 -> U+0305, row 1 -> U+030D.
        assert_eq!(placeholder_diacritic(0), Some('\u{0305}'));
        assert_eq!(placeholder_diacritic(1), Some('\u{030D}'));
    }

    #[test]
    fn placeholder_diacritic_out_of_range_is_none() {
        assert_eq!(placeholder_diacritic(297), None);
    }

    #[test]
    fn placeholder_cell_text_matches_spec_worked_2x2_grid() {
        // `printf "\e[38;5;42m\U10EEEE\U0305\U0305\U10EEEE\U0305\U030D..."`
        // from the spec: row 0 covers (0,0) and (0,1); row 1 covers
        // (1,0) and (1,1).
        assert_eq!(
            placeholder_cell_text(0, 0),
            Some("\u{10EEEE}\u{0305}\u{0305}".to_string())
        );
        assert_eq!(
            placeholder_cell_text(0, 1),
            Some("\u{10EEEE}\u{0305}\u{030D}".to_string())
        );
        assert_eq!(
            placeholder_cell_text(1, 0),
            Some("\u{10EEEE}\u{030D}\u{0305}".to_string())
        );
        assert_eq!(
            placeholder_cell_text(1, 1),
            Some("\u{10EEEE}\u{030D}\u{030D}".to_string())
        );
    }

    #[test]
    fn placeholder_cell_text_out_of_range_row_is_none() {
        assert_eq!(placeholder_cell_text(297, 0), None);
    }

    #[test]
    fn placeholder_fg_color_round_trips_24_bit_id() {
        let id = 0x01_ab_cd_u32;
        let color = placeholder_fg_color(id);
        assert_eq!(color, Color::rgb(0x01, 0xab, 0xcd));
    }

    fn src(bytes: &[u8]) -> ImageSource {
        ImageSource::Bytes(bytes.to_vec())
    }

    #[test]
    fn id_registry_allocates_new_id_once_then_reports_live() {
        let mut registry = IdRegistry::new();
        let key = ImageKey::new(&src(b"a"), 4, 2);

        let first = registry.touch(key.clone());
        let IdLookup::New(first_id) = first else {
            panic!("first touch for a fresh key must report New");
        };

        registry.begin_frame();
        let second = registry.touch(key);
        let IdLookup::Live(second_id) = second else {
            panic!("second touch for the same key must report Live");
        };
        assert_eq!(
            first_id, second_id,
            "the same key must keep the same id across frames"
        );
    }

    #[test]
    fn id_registry_different_keys_get_different_ids() {
        let mut registry = IdRegistry::new();
        let a = registry.touch(ImageKey::new(&src(b"a"), 4, 2));
        let b = registry.touch(ImageKey::new(&src(b"b"), 4, 2));
        let id = |lookup: IdLookup| match lookup {
            IdLookup::New(id) | IdLookup::Live(id) => id,
        };
        assert_ne!(id(a), id(b));
    }

    #[test]
    fn id_registry_expires_entries_idle_past_the_threshold() {
        let mut registry = IdRegistry::new();
        let key = ImageKey::new(&src(b"a"), 4, 2);
        let IdLookup::New(id) = registry.touch(key) else {
            panic!("first touch must allocate a new id");
        };

        // Three frames pass with nothing touching this key.
        registry.begin_frame();
        registry.begin_frame();
        registry.begin_frame();

        let expired = registry.expire(3);
        assert_eq!(expired, vec![id]);

        // Expired entries are gone — expiring again finds nothing.
        registry.begin_frame();
        assert_eq!(registry.expire(3), Vec::<u32>::new());
    }

    #[test]
    fn id_registry_touch_resets_idle_counter() {
        let mut registry = IdRegistry::new();
        let key = ImageKey::new(&src(b"a"), 4, 2);
        registry.touch(key.clone());

        registry.begin_frame();
        registry.begin_frame();
        registry.touch(key); // touched again before expiry threshold

        // Idle count was reset to 0 by the touch above, so a
        // threshold of 2 should not expire it yet.
        assert_eq!(registry.expire(2), Vec::<u32>::new());
    }
}
