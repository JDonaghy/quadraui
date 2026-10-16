//! Shared primitive types used across widgets: colours, icons, styled text,
//! widget identifiers, modifier flags, and tree paths.
//!
//! Design invariants (from `docs/UI_CRATE_DESIGN.md` §10, plugin-driven UI):
//! - Every type is owned (no `&'static` or borrowed data in public API).
//! - Every type is `Serialize + Deserialize` so Lua plugins can produce them
//!   via JSON → Rust struct conversion.
//! - No Rust closures cross the Rust/plugin boundary; events are plain data.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{OnceLock, RwLock};

/// RGBA colour, 0-255 each channel.
///
/// Matches the shape of `vimcode::Color` conceptually. Adapter code in apps
/// (e.g. vimcode) maps app-specific colour types to this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parse `"#rrggbb"` or `"#rrggbbaa"`. Returns `None` on malformed input.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        // Every byte must be an ASCII hex digit before we index into `s` by
        // byte offset below — this also guarantees `s` is all-ASCII, so
        // `s.len()` (bytes) matches char count and every `&s[i..j]` slice
        // below lands on a char boundary. A non-ASCII (or otherwise
        // non-hex) input like "#\u{20ac}abc" is rejected here instead of
        // panicking on the slice.
        if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let (r, g, b, a) = match s.len() {
            6 => (
                u8::from_str_radix(&s[0..2], 16).ok()?,
                u8::from_str_radix(&s[2..4], 16).ok()?,
                u8::from_str_radix(&s[4..6], 16).ok()?,
                255,
            ),
            8 => (
                u8::from_str_radix(&s[0..2], 16).ok()?,
                u8::from_str_radix(&s[2..4], 16).ok()?,
                u8::from_str_radix(&s[4..6], 16).ok()?,
                u8::from_str_radix(&s[6..8], 16).ok()?,
            ),
            _ => return None,
        };
        Some(Self { r, g, b, a })
    }

    /// Parse `"#rrggbbaa"` and alpha-composite it over `bg`, discarding the
    /// alpha channel in the result (`a` is always `255`). Falls back to
    /// [`Self::from_hex`] — including its 6-digit-only, no-alpha shape —
    /// when `s` carries no alpha channel, so this is a strict superset of
    /// `from_hex` for callers that always have a background to composite
    /// against. Returns `None` on malformed input, same rules as
    /// `from_hex`.
    ///
    /// Lifted from vimcode's `render::Color::try_from_hex_over`,
    /// where it exists because VS Code theme JSON commonly expresses
    /// translucent overlay colours (`editor.lineHighlightBackground`,
    /// diff backgrounds, bracket-match highlights) as `#rrggbbaa` meant
    /// to be composited over the editor background — a rasteriser that
    /// just took the raw (translucent) colour at face value would ignore
    /// the intended blend.
    pub fn try_from_hex_over(s: &str, bg: Self) -> Option<Self> {
        let stripped = s.strip_prefix('#')?;
        if stripped.len() != 8 {
            return Self::from_hex(s);
        }
        // Same ASCII-hex-digit guard as `from_hex`, for the same reason:
        // guarantees byte offsets below land on char boundaries.
        if !stripped.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let r = u8::from_str_radix(&stripped[0..2], 16).ok()?;
        let g = u8::from_str_radix(&stripped[2..4], 16).ok()?;
        let b = u8::from_str_radix(&stripped[4..6], 16).ok()?;
        let a = u8::from_str_radix(&stripped[6..8], 16).ok()?;
        let alpha = a as f64 / 255.0;
        let blend = |fg: u8, bg: u8| -> u8 {
            (fg as f64 * alpha + bg as f64 * (1.0 - alpha)).round() as u8
        };
        Some(Self {
            r: blend(r, bg.r),
            g: blend(g, bg.g),
            b: blend(b, bg.b),
            a: 255,
        })
    }

    /// Linearly interpolate each channel toward white by `amount`
    /// (clamped to `[0.0, 1.0]`). Used by primitives that derive a
    /// hover/highlight tint from a base background colour.
    pub fn lighten(self, amount: f64) -> Self {
        let f = amount.clamp(0.0, 1.0);
        Self {
            r: (self.r as f64 + (255.0 - self.r as f64) * f) as u8,
            g: (self.g as f64 + (255.0 - self.g as f64) * f) as u8,
            b: (self.b as f64 + (255.0 - self.b as f64) * f) as u8,
            a: self.a,
        }
    }

    /// Linearly interpolate each channel toward black by `amount`
    /// (clamped to `[0.0, 1.0]`). Symmetric counterpart of [`Self::lighten`].
    pub fn darken(self, amount: f64) -> Self {
        let f = amount.clamp(0.0, 1.0);
        Self {
            r: (self.r as f64 * (1.0 - f)) as u8,
            g: (self.g as f64 * (1.0 - f)) as u8,
            b: (self.b as f64 * (1.0 - f)) as u8,
            a: self.a,
        }
    }

    /// Return this colour with the alpha channel scaled to `alpha`
    /// (clamped to `[0.0, 1.0]`) on the `0..=255` byte scale, leaving
    /// `r`/`g`/`b` untouched.
    pub fn with_alpha(self, alpha: f64) -> Self {
        Self {
            a: (255.0 * alpha.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }

    /// Alpha-blend `over` on top of `self` at `alpha` (clamped to
    /// `[0.0, 1.0]`), discarding both inputs' own alpha channels — the
    /// result is always fully opaque (`a: 255`).
    ///
    /// Used by rasterisers on backends whose fill/line primitives don't
    /// honour `Color::a`, as a CPU-side alpha pre-mix. Consolidates the
    /// byte-identical `win::text::blend` and
    /// `primitives::chart::native_surface_paint::blend` free functions
    pub fn blend(self, over: Self, alpha: f64) -> Self {
        let alpha = alpha.clamp(0.0, 1.0);
        let mix =
            |b: u8, o: u8| -> u8 { (b as f64 * (1.0 - alpha) + o as f64 * alpha).round() as u8 };
        Self::rgb(
            mix(self.r, over.r),
            mix(self.g, over.g),
            mix(self.b, over.b),
        )
    }
}

/// A contiguous run of text sharing a single style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledSpan {
    pub text: String,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
}

impl StyledSpan {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }

    pub fn with_fg(text: impl Into<String>, fg: Color) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

/// A sequence of styled spans rendered inline on one line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledText {
    pub spans: Vec<StyledSpan>,
}

impl StyledText {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            spans: vec![StyledSpan::plain(text)],
        }
    }

    pub fn colored(text: impl Into<String>, fg: Color) -> Self {
        Self {
            spans: vec![StyledSpan::with_fg(text, fg)],
        }
    }

    /// Terminal display width of the concatenated span text, in cells.
    ///
    /// Sums each span's [`crate::text_util::display_width`] — CJK and
    /// wide-emoji glyphs count as 2, combining marks count as 0, so this
    /// is not the same as `chars().count()`.
    pub fn visible_width(&self) -> usize {
        self.spans
            .iter()
            .map(|s| crate::text_util::display_width(&s.text))
            .sum()
    }
}

/// Icon reference. Each app supplies both the glyph (for icon fonts) and
/// an ASCII/Unicode fallback (for TUI or when the icon font is unavailable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Icon {
    /// Nerd Font / icon-font glyph string.
    pub glyph: String,
    /// ASCII or basic-Unicode fallback used when `glyph` cannot render.
    pub fallback: String,
    /// Glyph colour. `None` paints the icon in the surrounding row's
    /// default foreground. `Some` paints both the Nerd Font glyph and
    /// its ASCII fallback in this colour.
    #[serde(default)]
    pub color: Option<Color>,
}

impl Icon {
    pub fn new(glyph: impl Into<String>, fallback: impl Into<String>) -> Self {
        Self {
            glyph: glyph.into(),
            fallback: fallback.into(),
            color: None,
        }
    }

    /// Attach a glyph colour. Painted by tree renderers in place of
    /// the row's default foreground; see the `color` field docs.
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

/// Collapse a bare string into an [`Icon`] with `glyph == fallback` —
/// the common case when the glyph and plain-text fallback are the same.
impl From<&str> for Icon {
    fn from(s: &str) -> Self {
        Self::new(s, s)
    }
}

/// See [`impl From<&str> for Icon`] — same collapse, owned-`String` input.
impl From<String> for Icon {
    fn from(s: String) -> Self {
        Self::new(s.clone(), s)
    }
}

/// Process-global string interner backing [`WidgetId`].
///
/// A widget's label is looked up once per distinct string and then shared:
/// every `WidgetId::new("same-label")` call after the first returns a
/// `Copy` handle into this table instead of allocating again. Interned
/// strings are leaked (never freed) for the process lifetime, which is the
/// standard trade-off for this pattern — apps generate widget IDs from a
/// bounded set of labels (one per widget, maybe with a handful of runtime
/// suffixes), not an unbounded one per frame.
struct WidgetIdTable {
    strings: Vec<&'static str>,
    index: HashMap<&'static str, u32>,
}

impl WidgetIdTable {
    fn new() -> Self {
        Self {
            strings: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl Default for WidgetIdTable {
    fn default() -> Self {
        Self::new()
    }
}

fn widget_id_table() -> &'static RwLock<WidgetIdTable> {
    static TABLE: OnceLock<RwLock<WidgetIdTable>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(WidgetIdTable::new()))
}

fn intern_widget_id(label: impl Into<String>) -> u32 {
    let label = label.into();
    {
        let table = widget_id_table()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(&idx) = table.index.get(label.as_str()) {
            return idx;
        }
    }
    let mut table = widget_id_table()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Re-check: another thread may have interned the same label while we
    // were waiting for the write lock.
    if let Some(&idx) = table.index.get(label.as_str()) {
        return idx;
    }
    let leaked: &'static str = Box::leak(label.into_boxed_str());
    let idx = table.strings.len() as u32;
    table.strings.push(leaked);
    table.index.insert(leaked, idx);
    idx
}

fn resolve_widget_id(idx: u32) -> &'static str {
    widget_id_table()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .strings[idx as usize]
}

/// Stable identifier for a widget across frames.
///
/// `Copy` and cheap to pass around: the label lives once in a process-global
/// interning table (see [`WidgetIdTable`]) and a `WidgetId` is just an index
/// into it. Construct one from any string — `WidgetId::new("my-id")` or
/// `"my-id".into()` — and read the label back with [`WidgetId::as_str`].
/// Apps should namespace labels (e.g. `"plugin:my-ext:main-form"`) to avoid
/// collisions with core widget IDs.
///
/// Serializes and deserializes as its plain string label — the wire form is
/// unchanged by interning, so JSON/plugin boundaries that produce or consume
/// a `WidgetId` keep working unmodified.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WidgetId(u32);

impl WidgetId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(intern_widget_id(id.into()))
    }

    /// The label this handle was interned from.
    pub fn as_str(&self) -> &'static str {
        resolve_widget_id(self.0)
    }
}

impl fmt::Debug for WidgetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WidgetId").field(&self.as_str()).finish()
    }
}

impl fmt::Display for WidgetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for WidgetId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WidgetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(WidgetId::new)
    }
}

impl From<&str> for WidgetId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for WidgetId {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// Keyboard modifier state carried alongside input events.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub cmd: bool,
}

/// Path to a row in a tree-structured widget.
///
/// `vec![]` is the root; `vec![3, 1]` is the second child of the fourth
/// top-level row. Indices are `u16` to fit comfortably in small fixed-size
/// arrays while still accommodating trees with thousands of siblings.
pub type TreePath = Vec<u16>;

/// Selection behaviour for row-oriented widgets (`TreeView`, `ListView`,
/// `DataTable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SelectionMode {
    /// Rows cannot be selected.
    None,
    /// At most one row is selected at a time.
    #[default]
    Single,
    /// Multiple rows may be selected (shift/ctrl click).
    Multi,
}

/// Optional visual decoration applied to a row beyond its text colour.
///
/// Backends map each variant to an appropriate visual treatment. For row
/// widgets like `TreeView`, `Header` typically gets a distinct background
/// (section-header styling); `Muted` dims the foreground; `Error` /
/// `Warning` override the foreground with theme error/warning colours;
/// `Modified` implies italic but otherwise normal colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Decoration {
    /// Rendered with default foreground and background.
    #[default]
    Normal,
    /// Section-header styling: distinct (typically darker or accent) background,
    /// often with bold text. Use for rows that group others rather than
    /// represent a leaf entity. Row tree-hierarchy status (`is_expanded`
    /// Some vs. None) is orthogonal: not all branches are headers, and a
    /// leaf row can be a header.
    Header,
    /// Dimmed (e.g. gitignored file, stale git log entry).
    Muted,
    /// Red / error-toned (e.g. lint error, merge conflict).
    Error,
    /// Amber / warning-toned (e.g. unsaved changes badge).
    Warning,
    /// Italicised, typically unchanged foreground colour.
    Modified,
}

/// Right-aligned status indicator on a row: short text plus optional
/// foreground/background colouring. Used by `TreeView` / `ListView` /
/// `DataTable` for git-status letters, item counts, conflict markers, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Badge {
    pub text: String,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

impl Badge {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: None,
            bg: None,
        }
    }

    pub fn colored(text: impl Into<String>, fg: Color) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bg: None,
        }
    }
}

/// Visual style configuration for a `TreeView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeStyle {
    /// Cells / pixels per visual indent level.
    pub indent: u16,
    /// Draw expand/collapse chevrons on branches.
    pub show_chevrons: bool,
    /// Chevron drawn for an expanded branch. Painted verbatim on TUI.
    /// On GTK / macOS / Win-GUI, the *default* value (`"▾"`) is instead
    /// painted as a sharper codicon glyph
    /// (`crate::codicon::CHEVRON_DOWN`); any non-default override an
    /// app sets here is still honoured and painted verbatim on every
    /// backend, GUI included.
    pub chevron_expanded: String,
    /// Chevron drawn for a collapsed branch. Painted verbatim on TUI.
    /// On GTK / macOS / Win-GUI, the *default* value (`"▸"`) is instead
    /// painted as a sharper codicon glyph
    /// (`crate::codicon::CHEVRON_RIGHT`); any non-default override an
    /// app sets here is still honoured and painted verbatim on every
    /// backend, GUI included.
    pub chevron_collapsed: String,
    /// Host override for non-header row height, in the backend's native
    /// unit (pixels for GTK / macOS / Win-GUI). `Some(22)` by default,
    /// matching VS Code's tree row pitch, independent of editor font size.
    /// Header rows (`Decoration::Header`) are unaffected; they stay pitched
    /// off `line_height`. Set to `None` to fall back to the old
    /// line-height-derived pitch (`line_height * 1.4` for leaves and
    /// branches) instead.
    ///
    /// **TUI ignores this field** — a cell grid has no literal pixels, so
    /// rows there stay a fixed 1 cell regardless of this override.
    ///
    /// Integral because both GUI rasterisers `.round()` the row pitch
    /// to whole native units before painting.
    ///
    /// Both `gtk_tree_layout` / `mac_tree_layout` (hit-test) and
    /// `draw_tree` (paint) read this same field, so paint and hit-test
    /// stay in sync by construction.
    #[serde(default = "TreeStyle::default_row_height")]
    pub row_height: Option<u16>,
}

impl TreeStyle {
    fn default_row_height() -> Option<u16> {
        Some(22)
    }
}

impl Default for TreeStyle {
    fn default() -> Self {
        Self {
            indent: 2,
            show_chevrons: true,
            chevron_expanded: "▾".into(),
            chevron_collapsed: "▸".into(),
            row_height: Self::default_row_height(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_alpha_scales_a_leaves_rgb_untouched() {
        let c = Color::rgb(10, 20, 30).with_alpha(0.5);
        assert_eq!((c.r, c.g, c.b, c.a), (10, 20, 30, 128));
    }

    #[test]
    fn with_alpha_clamps_out_of_range_input() {
        assert_eq!(Color::rgb(1, 2, 3).with_alpha(-1.0).a, 0);
        assert_eq!(Color::rgb(1, 2, 3).with_alpha(2.0).a, 255);
    }

    #[test]
    fn blend_zero_alpha_is_base_one_alpha_is_over() {
        let base = Color::rgb(0, 0, 0);
        let over = Color::rgb(200, 100, 50);
        assert_eq!(base.blend(over, 0.0), Color::rgb(0, 0, 0));
        assert_eq!(base.blend(over, 1.0), Color::rgb(200, 100, 50));
    }

    #[test]
    fn blend_midpoint_averages_channels_and_is_opaque() {
        let base = Color::rgba(0, 0, 0, 10);
        let over = Color::rgba(200, 100, 50, 20);
        let mixed = base.blend(over, 0.5);
        assert_eq!((mixed.r, mixed.g, mixed.b, mixed.a), (100, 50, 25, 255));
    }

    #[test]
    fn from_hex_parses_rgb() {
        assert_eq!(
            Color::from_hex("#ff0080"),
            Some(Color::rgb(0xff, 0x00, 0x80))
        );
    }

    #[test]
    fn from_hex_parses_rgba() {
        assert_eq!(
            Color::from_hex("#ff008040"),
            Some(Color::rgba(0xff, 0x00, 0x80, 0x40))
        );
    }

    #[test]
    fn from_hex_rejects_missing_prefix() {
        assert_eq!(Color::from_hex("ff0080"), None);
    }

    #[test]
    fn from_hex_rejects_wrong_length() {
        assert_eq!(Color::from_hex("#ff008"), None);
    }

    #[test]
    fn from_hex_rejects_non_hex_ascii() {
        assert_eq!(Color::from_hex("#zzzzzz"), None);
    }

    /// A multibyte character inside the hex digits used to make `s.len()`
    /// (byte length) satisfy the `6`/`8` length check while slicing
    /// mid-character, panicking instead of returning `None`.
    #[test]
    fn from_hex_multibyte_char_returns_none_instead_of_panicking() {
        // "\u{20ac}abc" ('€' + "abc") is 3 + 3 = 6 bytes, matching the RGB
        // length branch by byte count alone.
        assert_eq!(Color::from_hex("#\u{20ac}abc"), None);
    }

    #[test]
    fn visible_width_ascii_matches_char_count() {
        assert_eq!(StyledText::plain("hello").visible_width(), 5);
        assert_eq!(StyledText::plain("").visible_width(), 0);
    }

    /// CJK/emoji glyphs occupy two terminal cells each, so `visible_width`
    /// must account for double-width characters, not just count `chars()`.
    #[test]
    fn visible_width_cjk_counts_double_width() {
        let text = StyledText::plain("日本語");
        assert_eq!(text.visible_width(), 6);
        assert_ne!(text.visible_width(), text.spans[0].text.chars().count());
    }

    #[test]
    fn visible_width_sums_across_spans() {
        let text = StyledText {
            spans: vec![
                StyledSpan::plain("中"),
                StyledSpan::plain("a"),
                StyledSpan::plain("文"),
            ],
        };
        // "中" (2) + "a" (1) + "文" (2) = 5, vs. a char count of 3.
        assert_eq!(text.visible_width(), 5);
    }

    #[test]
    fn visible_width_combining_mark_counts_zero() {
        // "e" + U+0301 COMBINING ACUTE ACCENT renders as one visual
        // column even though it is two `char`s.
        let text = StyledText::plain("e\u{0301}");
        assert_eq!(text.visible_width(), 1);
    }

    #[test]
    fn color_from_hex_rgb() {
        let c = Color::from_hex("#1a2b3c").unwrap();
        assert_eq!(c, Color::rgb(0x1a, 0x2b, 0x3c));
    }

    #[test]
    fn color_from_hex_rgba() {
        let c = Color::from_hex("#1a2b3c80").unwrap();
        assert_eq!(c, Color::rgba(0x1a, 0x2b, 0x3c, 0x80));
    }

    #[test]
    fn color_from_hex_invalid() {
        assert!(Color::from_hex("not a hex").is_none());
        assert!(Color::from_hex("#xyz").is_none());
        assert!(Color::from_hex("#12345").is_none()); // wrong length
    }

    #[test]
    fn try_from_hex_over_blends_alpha_against_background() {
        let bg = Color::rgb(0, 0, 0);
        // Full alpha (`ff`) over black should equal the opaque colour.
        let c = Color::try_from_hex_over("#ff008040", bg).unwrap();
        // alpha byte 0x40 = 64/255 ≈ 0.251
        let alpha = 0x40 as f64 / 255.0;
        let blend = |fg: u8| -> u8 { (fg as f64 * alpha).round() as u8 };
        assert_eq!(c, Color::rgba(blend(0xff), blend(0x00), blend(0x80), 255));
    }

    #[test]
    fn try_from_hex_over_full_alpha_ignores_background() {
        let bg = Color::rgb(10, 20, 30);
        let c = Color::try_from_hex_over("#ff0080ff", bg).unwrap();
        assert_eq!(c, Color::rgba(0xff, 0x00, 0x80, 255));
    }

    #[test]
    fn try_from_hex_over_zero_alpha_equals_background() {
        let bg = Color::rgb(10, 20, 30);
        let c = Color::try_from_hex_over("#ff008000", bg).unwrap();
        assert_eq!(c, Color::rgba(10, 20, 30, 255));
    }

    #[test]
    fn try_from_hex_over_falls_back_to_from_hex_without_alpha() {
        let bg = Color::rgb(10, 20, 30);
        assert_eq!(
            Color::try_from_hex_over("#1a2b3c", bg),
            Color::from_hex("#1a2b3c")
        );
    }

    #[test]
    fn try_from_hex_over_rejects_malformed_input() {
        let bg = Color::rgb(0, 0, 0);
        assert_eq!(Color::try_from_hex_over("not a hex", bg), None);
        assert_eq!(Color::try_from_hex_over("#zzzzzzzz", bg), None);
        assert_eq!(Color::try_from_hex_over("#12345", bg), None);
    }

    #[test]
    fn widget_id_roundtrip() {
        let id = WidgetId::new("sc-tree");
        assert_eq!(id.as_str(), "sc-tree");
        let id2: WidgetId = "sc-tree".into();
        assert_eq!(id, id2);
    }

    #[test]
    fn widget_id_is_copy_not_just_clone() {
        // `WidgetId` derives `Copy`; this compiles only because passing
        // `id` by value to the first `as_str()` call doesn't move it out
        // from under the second.
        let id = WidgetId::new("copy-check");
        assert_eq!(id.as_str(), "copy-check");
        assert_eq!(id.as_str(), "copy-check");
    }

    #[test]
    fn widget_id_serializes_as_plain_string() {
        let id = WidgetId::new("wire-format-check");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"wire-format-check\"");
        let back: WidgetId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn widget_id_interns_equal_labels_to_equal_handles() {
        // Two handles built from the same label are `==` and share the
        // same resolved label, whether or not they came from the same
        // `intern_widget_id` call — the interning table is process-global.
        let a = WidgetId::new("shared-label");
        let b = WidgetId::new("shared-label".to_string());
        assert_eq!(a, b);
        assert_eq!(a.as_str(), b.as_str());
    }

    #[test]
    fn styled_text_visible_width() {
        let text = StyledText {
            spans: vec![
                StyledSpan::plain("hello "),
                StyledSpan::with_fg("world", Color::rgb(255, 0, 0)),
            ],
        };
        assert_eq!(text.visible_width(), 11);
    }
}
