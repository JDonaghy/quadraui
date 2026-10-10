//! DirectWrite text infrastructure — factory + text-format creation,
//! font-metrics measurement, and Direct2D text painting (issue #21).
//!
//! Mirrors the GTK backend's Pango-based measurement
//! (`gtk::run::render_frame`'s `pango_ctx.metrics()` for line height,
//! `layout.pixel_size()` for char width — see also `gtk::status_bar`'s
//! per-segment Pango measurer) with the DirectWrite equivalents:
//! `IDWriteFontFace::GetMetrics` (`DWRITE_FONT_METRICS`, design units
//! scaled by font size) for line height, and
//! `IDWriteTextLayout::GetMetrics` on a laid-out `"0"` for an
//! approximate char width.
//!
//! This whole module only exists on `target_os = "windows"` — see
//! `super::mod`'s `pub mod text;` declaration and `backend.rs`'s module docs
//! for why the rest of this repo's `--features win` compile gate stays
//! meaningful without a Windows host.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use windows::core::{
    Error as WinError, IUnknown, Interface, Result as WinResult, BOOL, HSTRING, PCWSTR,
};
use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFactory2, IDWriteFactory5, IDWriteFontCollection,
    IDWriteFontCollection1, IDWriteFontFallback, IDWriteFontFile, IDWriteLocalizedStrings,
    IDWriteTextFormat, IDWriteTextFormat1, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_UNICODE_RANGE, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows_numerics::Vector2;

use crate::event::Rect;
use crate::win::msg::pt_to_dip;
use crate::Color;

/// Live DirectWrite handles for the configured editor font: the shared
/// factory plus an `IDWriteTextFormat` for the current family + size.
///
/// Owned by [`super::backend::Surface`] and recreated alongside it (see
/// that struct's docs) — like the sibling `ID2D1Factory`, this is more
/// state than strictly needs the render target's lifetime (DirectWrite
/// resources aren't GPU-device-bound), but keeping every per-window
/// resource on one struct with one recreate-on-device-loss path avoids a
/// second "is it attached yet" check.
///
/// `pub` (with a `pub` constructor and measure/draw methods) rather than
/// `pub(crate)`: it appears by reference in the signatures of the chrome
/// rasterisers `super::mod` re-exports (`draw_status_bar`, `draw_tab_bar`,
/// `draw_activity_bar`, `draw_menu_bar`, and their `*_layout` twins), so a
/// crate-private type here is a `private_interfaces` warning — i.e. a build
/// failure under CI's `-D warnings`. Same posture as
/// [`crate::macos::text`]'s `pub fn make_font` / `pub fn measure_text`,
/// which the macOS rasterisers take the same way.
pub struct DWrite {
    factory: IDWriteFactory,
    text_format: IDWriteTextFormat,
    /// Bold variant of `text_format`, same family/size — built alongside
    /// it so chrome rasterisers (`StatusBar` segment `bold`, #25) can
    /// request bold-weight measurement/painting without constructing a
    /// throwaway `IDWriteTextFormat` per call.
    bold_text_format: IDWriteTextFormat,
    /// Family name this handle was constructed with — stashed so
    /// [`Self::with_size`] can build a same-family clone at a different
    /// size without an `IDWriteTextFormat::GetFontFamilyName` round-trip
    /// (issue #1157, the activity-bar icon glyph's fixed-size knob).
    family: String,
    /// Nerd-Font fallback this handle was constructed with, if any —
    /// same reuse rationale as `family`.
    fallback: Option<IDWriteFontFallback>,
    /// Same-family [`IDWriteTextFormat`]s at minimap row-pitch sizes,
    /// keyed by the rounded DIP size
    /// [`crate::primitives::minimap::minimap_font_px`] resolved — not by
    /// the raw `f64` it returns, since that's a continuous function of
    /// row pitch and a bare-`f64` `HashMap` key would almost never hit on
    /// a second lookup even when two rows share a pitch (float equality).
    /// `win::minimap::paint_row_glyphs` is the only reader
    /// ([`Self::minimap_text_format`]); in practice a surface's rows all
    /// share one pitch, so this holds at most a couple of entries per
    /// `DWrite`, not one per row.
    minimap_formats: RefCell<HashMap<i32, IDWriteTextFormat>>,
}

impl DWrite {
    /// Create the shared `IDWriteFactory` and an `IDWriteTextFormat` for
    /// `family` at `size_pt` **points** — the same convention
    /// [`WinBackend::editor_font_size_pt`][crate::win::backend::WinBackend]
    /// and GTK's `editor_font_size_pt` (fed through Pango, which is points)
    /// use. DirectWrite's `fontSize` parameter is DIPs, not points (1 DIP =
    /// 1/96in, 1 point = 1/72in), so `size_pt` is converted via
    /// [`pt_to_dip`] before it reaches DirectWrite — see that function's
    /// docs for why a straight passthrough is wrong.
    ///
    /// `fallback` — built by [`build_nerd_font_fallback`] from whatever
    /// [`Backend::set_nerd_font_fallback`][crate::Backend::set_nerd_font_fallback]
    /// last set — is applied once here, to both text formats, via
    /// [`IDWriteTextFormat1::SetFontFallback`] (issue #929). Every
    /// `IDWriteTextLayout` created against a format afterward — whether
    /// built explicitly by [`Self::measure_text`]/[`Self::measure_text_styled`]
    /// or internally by [`ID2D1RenderTarget::DrawText`] inside
    /// [`Self::draw_text`]/[`Self::draw_text_styled`] — inherits it from
    /// the format it was created from, the same way it inherits
    /// alignment/wrapping; `IDWriteTextLayout2::SetFontFallback` (the
    /// per-layout override the same interface exposes) is for a caller
    /// that wants to *diverge* from the format's fallback for one
    /// layout, which no rasteriser here needs. `None` leaves
    /// DirectWrite's own system fallback chain untouched, same as before
    /// this parameter existed. If `SetFontFallback` itself errors (see
    /// [`apply_fallback_to_format`]'s doc), this emits a
    /// [`crate::diagnostics`] message and continues without the
    /// fallback rather than returning `Err` — a missing fallback
    /// degrades to tofu on uncovered characters, which is no worse than
    /// the pre-#929 baseline every other font/layout error here does not
    /// need to tolerate.
    ///
    /// Returns the constructed handles plus `(line_height, char_width)`
    /// resolved from the format's real font metrics, so the caller
    /// ([`super::backend::WinBackend::attach_surface`]) can feed them
    /// straight into `set_current_line_height`/`set_current_char_width`
    /// without a second round-trip through this module.
    pub fn new(
        family: &str,
        size_pt: f32,
        fallback: Option<&IDWriteFontFallback>,
    ) -> WinResult<(Self, f32, f32)> {
        // SAFETY: `DWriteCreateFactory` takes no pointers here beyond the
        // factory-type enum; the returned `IDWriteFactory` is a COM
        // interface `Self` owns for its whole lifetime (released via
        // `windows-rs`'s `Drop`).
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let size_dip = pt_to_dip(size_pt);
        let text_format =
            create_text_format(&factory, family, size_dip, DWRITE_FONT_WEIGHT_NORMAL)?;
        let bold_text_format =
            create_text_format(&factory, family, size_dip, DWRITE_FONT_WEIGHT_BOLD)?;
        // #1077: every rasteriser that paints multi-line content already
        // does its own line-splitting before calling `draw_text`/
        // `DrawText` (see `primitives::text_display::paint` and
        // `primitives::dialog::paint`'s per-line `row_rect`s, both of
        // which draw one already-split line per call) — none of them
        // rely on DirectWrite's own word-wrap. Left at the default
        // `DWRITE_WORD_WRAPPING_WRAP`, `draw_text`'s `layout_rect` (whose
        // `right`/`bottom` are computed as `x + width`/`y + height` in
        // `f32`, a fraction of a DIP narrower than the width
        // `measure_text` returned for the very same string — float
        // rounding, not antialiasing) can wrap a label exactly as wide as
        // its measured box onto a second line, which
        // `D2D1_DRAW_TEXT_OPTIONS_CLIP` then crops entirely, painting
        // zero pixels for it. `NO_WRAP` makes `draw_text`'s rect-sized
        // labels immune to that off-by-a-float-epsilon: overflow simply
        // clips at the right edge instead of reflowing.
        // SAFETY: `text_format`/`bold_text_format` are the live interfaces
        // just created above; `SetWordWrapping` takes a plain enum value,
        // no pointers.
        unsafe { text_format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)? };
        // SAFETY: same as the `text_format` call immediately above.
        unsafe { bold_text_format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)? };
        if let Some(fallback) = fallback {
            // Font fallback is a "nice to have that beats tofu," not a
            // hard requirement (issue #929 review) — degrade to no
            // fallback rather than failing the whole surface/headless
            // attach (`WinBackend::attach_surface`/`attach_headless`
            // `?`-propagate whatever `DWrite::new` returns) if
            // `SetFontFallback` ever errors on some future Windows
            // quirk. Both macOS and GTK's equivalent paths already
            // silently continue without a fallback on failure; this
            // matches that posture instead of taking down painting
            // entirely over a cosmetic feature.
            if let Err(err) = apply_fallback_to_format(&text_format, fallback) {
                crate::diagnostics::emit(format!(
                    "quadraui: IDWriteTextFormat1::SetFontFallback failed for the regular text \
                     format ({err:?}); continuing without a Nerd-Font fallback"
                ));
            }
            if let Err(err) = apply_fallback_to_format(&bold_text_format, fallback) {
                crate::diagnostics::emit(format!(
                    "quadraui: IDWriteTextFormat1::SetFontFallback failed for the bold text \
                     format ({err:?}); continuing without a Nerd-Font fallback"
                ));
            }
        }

        let font_metrics = font_face_metrics(&factory, family)?;
        let units_per_em = (font_metrics.designUnitsPerEm as f32).max(1.0);
        let line_gap = (font_metrics.lineGap as f32).max(0.0);
        let line_height = (font_metrics.ascent as f32 + font_metrics.descent as f32 + line_gap)
            / units_per_em
            * size_dip;
        let (char_width, _) = measure_text(&factory, &text_format, "0")?;

        Ok((
            Self {
                factory,
                text_format,
                bold_text_format,
                family: family.to_string(),
                fallback: fallback.cloned(),
                minimap_formats: RefCell::new(HashMap::new()),
            },
            line_height,
            char_width,
        ))
    }

    /// Build a new `DWrite` for the same family and Nerd-Font fallback as
    /// `self`, but at `size_pt` — used by `win::activity_bar` (issue
    /// #1157) to paint the icon glyph at a fixed VS-Code-parity size
    /// independent of whichever editor/chrome font size `self` itself
    /// was constructed at, mirroring
    /// [`crate::macos::backend::MacBackend`]'s
    /// `font.clone_with_font_size(..)` and
    /// `gtk::activity_bar::activity_bar_icon_font`'s fresh
    /// `FontDescription` — both of which also derive a same-family,
    /// differently-sized text handle from whatever the caller passed in,
    /// rather than trusting its own size.
    pub(crate) fn with_size(&self, size_pt: f32) -> WinResult<DWrite> {
        DWrite::new(&self.family, size_pt, self.fallback.as_ref()).map(|(dw, _, _)| dw)
    }

    /// `(width, height)` DIPs of `text` laid out against this format —
    /// the `measure_text(text) -> (width_dips, height_dips)` helper
    /// issue #21 asks for, scoped to the current editor font.
    pub fn measure_text(&self, text: &str) -> WinResult<(f32, f32)> {
        measure_text(&self.factory, &self.text_format, text)
    }

    /// Like [`Self::measure_text`], but against the bold variant when
    /// `bold` is `true` — chrome rasterisers (e.g. [`crate::StatusBar`]'s
    /// per-segment `bold` flag, #25) use this so the fit/paint widths
    /// agree regardless of weight.
    pub fn measure_text_styled(&self, text: &str, bold: bool) -> WinResult<(f32, f32)> {
        measure_text(&self.factory, self.format_for(bold), text)
    }

    /// Paint `text` inside `rect` (DIPs, target-relative) in `color`,
    /// clipped to the rect, using this format against `target`.
    pub fn draw_text(
        &self,
        target: &ID2D1RenderTarget,
        text: &str,
        rect: Rect,
        color: Color,
    ) -> WinResult<()> {
        draw_text(target, &self.text_format, text, rect, color)
    }

    /// Like [`Self::draw_text`], but against the bold variant when `bold`
    /// is `true`. See [`Self::measure_text_styled`].
    pub fn draw_text_styled(
        &self,
        target: &ID2D1RenderTarget,
        text: &str,
        rect: Rect,
        color: Color,
        bold: bool,
    ) -> WinResult<()> {
        draw_text(target, self.format_for(bold), text, rect, color)
    }

    fn format_for(&self, bold: bool) -> &IDWriteTextFormat {
        if bold {
            &self.bold_text_format
        } else {
            &self.text_format
        }
    }

    /// Same-family `IDWriteTextFormat` at `size_px` DIPs, cached by
    /// rounded size — what
    /// [`crate::win::minimap::paint_row_glyphs`]'s `Characters` branch
    /// shapes through, so a minimap row paints at
    /// [`crate::primitives::minimap::minimap_font_px`]'s resolved size
    /// rather than this handle's own editor-size [`Self::text_format`].
    /// `size_px` is rounded to the nearest DIP before the cache lookup —
    /// row pitch is already a small, mostly-fixed set of values in
    /// practice (one per `MinimapScale`), so this keeps the cache at a
    /// couple of entries rather than growing one per float-distinct
    /// `vline.bounds.height`, and side-steps `f64`-as-a-map-key float
    /// equality entirely.
    ///
    /// No word-wrapping/fallback divergence from [`Self::text_format`]:
    /// built the same way (`DWRITE_WORD_WRAPPING_NO_WRAP`, this handle's
    /// own Nerd-Font fallback if any) via [`create_text_format`], just at
    /// a different size.
    pub(crate) fn minimap_text_format(&self, size_px: f64) -> WinResult<IDWriteTextFormat> {
        let key = (size_px.round() as i32).max(1);
        if let Some(format) = self.minimap_formats.borrow().get(&key) {
            return Ok(format.clone());
        }

        let format = create_text_format(
            &self.factory,
            &self.family,
            key as f32,
            DWRITE_FONT_WEIGHT_NORMAL,
        )?;
        // SAFETY: `format` is the live interface just created above;
        // `SetWordWrapping` takes a plain enum value, no pointers — same
        // call `DWrite::new` makes on `text_format`/`bold_text_format`,
        // for the same reason (see that call's own doc comment).
        unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)? };
        if let Some(fallback) = &self.fallback {
            // Same "degrade, don't fail the paint" posture `DWrite::new`
            // takes for the editor-size formats —
            // see that call's doc comment.
            if let Err(err) = apply_fallback_to_format(&format, fallback) {
                crate::diagnostics::emit(format!(
                    "quadraui: IDWriteTextFormat1::SetFontFallback failed for a minimap text \
                     format at {key}px ({err:?}); continuing without a Nerd-Font fallback"
                ));
            }
        }

        self.minimap_formats
            .borrow_mut()
            .insert(key, format.clone());
        Ok(format)
    }

    /// Paint `text` inside `rect` (DIPs, target-relative) in `color` at
    /// `size_px` DIPs — [`Self::draw_text`]'s minimap-sized twin,
    /// via [`Self::minimap_text_format`] rather than this
    /// handle's editor-size format.
    pub(crate) fn draw_text_minimap(
        &self,
        target: &ID2D1RenderTarget,
        text: &str,
        rect: Rect,
        color: Color,
        size_px: f64,
    ) -> WinResult<()> {
        let format = self.minimap_text_format(size_px)?;
        draw_text(target, &format, text, rect, color)
    }

    /// Number of distinct sizes cached by [`Self::minimap_text_format`] so
    /// far — `#[cfg(test)]` acceptance hook proving a
    /// `Characters`-mode minimap paint actually requests a minimap-sized
    /// format (and, together with a same-size/different-size pair of
    /// calls, that the cache hits rather than reallocating per paint)
    /// rather than silently falling back to the editor-size
    /// [`Self::text_format`].
    #[cfg(test)]
    pub(crate) fn minimap_format_cache_len(&self) -> usize {
        self.minimap_formats.borrow().len()
    }

    /// Is `size_px` (rounded the same way [`Self::minimap_text_format`]
    /// rounds its cache key) present in the minimap-format cache?
    /// `#[cfg(test)]` hook that lets a caller pin *which* size got
    /// cached, not just how many — [`Self::minimap_format_cache_len`]
    /// alone can't distinguish a paint that cached the resolved minimap
    /// size from one that accidentally cached the (much larger)
    /// editor-format size.
    #[cfg(test)]
    pub(crate) fn has_cached_minimap_px(&self, size_px: f64) -> bool {
        let key = (size_px.round() as i32).max(1);
        self.minimap_formats.borrow().contains_key(&key)
    }
}

/// Issue #1078: a direct impl rather than a per-module wrapper struct —
/// before this, `win::form::DWriteMeasure` and `win::toolbar::DWriteMeasure`
/// were byte-identical one-line adapters (`.measure_text(text).0`) over
/// this exact type, and `win::sidebar_panel` imported the `toolbar` one
/// only because there had to be a canonical copy *somewhere*. Implementing
/// [`TextMeasure`] on `DWrite` itself means every layout fn that needs a
/// `&dyn TextMeasure` can pass a live `&DWrite` straight through — no
/// wrapper, no duplicate to drift.
impl crate::primitives::layout_metrics::TextMeasure for DWrite {
    fn width_of(&self, text: &str) -> f32 {
        self.measure_text(text).map(|(w, _)| w).unwrap_or(0.0)
    }
}

/// `IDWriteFactory::CreateTextFormat` for `family` at `size_dip` — already
/// converted from points via [`pt_to_dip`] by the caller, since
/// `CreateTextFormat`'s `fontSize` parameter is DIPs, not points. Uses the
/// system font collection (`None`) and the `"en-us"` locale — DirectWrite
/// requires a non-null locale name; empty/garbage locales silently fall
/// back to whatever the font supports, so a real BCP-47 tag is used
/// rather than `""`.
fn create_text_format(
    factory: &IDWriteFactory,
    family: &str,
    size_dip: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> WinResult<IDWriteTextFormat> {
    // SAFETY: `factory` is the caller's live interface; `HSTRING::from`
    // builds owned, self-contained strings passed by value (not raw
    // pointers with a borrowed lifetime), and `None` for the font
    // collection asks for the system collection.
    unsafe {
        factory.CreateTextFormat(
            &HSTRING::from(family),
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size_dip,
            &HSTRING::from("en-us"),
        )
    }
}

/// Apply `fallback` to `format` via `IDWriteTextFormat1::SetFontFallback`
/// — see [`DWrite::new`]'s doc for why setting it here (rather than per
/// layout) is enough to cover every layout DirectWrite later builds from
/// `format`, including `ID2D1RenderTarget::DrawText`'s internal one.
///
/// `IDWriteTextFormat1` is a Windows-8-and-later interface; the `cast`
/// only fails on a Windows 7 host DirectWrite 1.0, which this crate does
/// not otherwise support (every other rasteriser already assumes
/// `IDWriteFactory5`-era APIs — see [`register_font_from_memory`]).
/// [`DWrite::new`]'s caller logs and continues without a fallback rather
/// than propagating this error (issue #929 review): font fallback is a
/// "nice to have that beats tofu," and a failure here should degrade,
/// not take down the whole surface/headless attach the way an `Err`
/// return from `DWrite::new` would.
fn apply_fallback_to_format(
    format: &IDWriteTextFormat,
    fallback: &IDWriteFontFallback,
) -> WinResult<()> {
    let format1: IDWriteTextFormat1 = format.cast()?;
    // SAFETY: `format1` is the just-cast live interface (`cast` is a
    // `QueryInterface` on the same underlying COM object, per this
    // function's own doc comment); `fallback` is the caller's live
    // interface, only borrowed for this call.
    unsafe { format1.SetFontFallback(fallback) }
}

/// Register `bytes` (raw TTF/OTF font data) with DirectWrite for the
/// lifetime of this process via an `IDWriteInMemoryFontFileLoader` — no
/// filesystem write, matching [`crate::Backend::register_font_from_memory`]'s
/// contract (issue #929).
///
/// Returns a private `IDWriteFontCollection1` containing just this font,
/// plus every family name it exposes (read back from the font's own name
/// table via `IDWriteFontFamily::GetFamilyNames`, not the caller's
/// guess) — [`build_nerd_font_fallback`] takes the collection so a
/// fallback built from one of these names resolves against the font
/// that was actually registered, not whatever the system happens to
/// have installed under the same family name.
///
/// Requires `IDWriteFactory5` (Windows 10 1809+); every in-tree Win-GUI
/// rasteriser already assumes DirectWrite APIs at least this new (see
/// `apply_fallback_to_format`'s doc), so this is not a new floor.
pub fn register_font_from_memory(bytes: &[u8]) -> WinResult<(IDWriteFontCollection1, Vec<String>)> {
    // SAFETY: `DWriteCreateFactory` takes no pointers here beyond the
    // factory-type enum; the returned `IDWriteFactory5` is a live COM
    // interface owned by this scope for the rest of the function.
    let factory: IDWriteFactory5 = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
    // SAFETY: `factory` is the live interface from above.
    let loader = unsafe { factory.CreateInMemoryFontFileLoader()? };
    // The loader must be registered on the factory before a file
    // reference it creates can be resolved — see
    // `IDWriteFactory::RegisterFontFileLoader`'s docs. Left registered
    // for the process lifetime, matching this method's own contract.
    // SAFETY: `factory`/`loader` are both live interfaces from above.
    unsafe { factory.RegisterFontFileLoader(&loader)? };
    let factory_base: IDWriteFactory = factory.cast()?;
    // SAFETY: `factory_base` is the just-cast live interface;
    // `loader` is the registered loader from above; `bytes` is the
    // caller's slice, and `bytes.as_ptr()`/`bytes.len()` are read
    // together from the same still-live `&[u8]` borrow, so the pointer
    // and length agree. `CreateInMemoryFontFileReference` copies the
    // font data into its own storage (the whole point of the "in
    // memory" loader) rather than retaining `bytes`' pointer past this
    // call, per `IDWriteInMemoryFontFileLoader`'s documented contract.
    let file: IDWriteFontFile = unsafe {
        loader.CreateInMemoryFontFileReference(
            &factory_base,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            None::<&IUnknown>,
        )?
    };

    // SAFETY: `factory` is still the live interface from above.
    let builder = unsafe { factory.CreateFontSetBuilder()? };
    // SAFETY: `builder` is the interface just created; `file` is the
    // live `IDWriteFontFile` from above, only borrowed for this call.
    unsafe { builder.AddFontFile(&file)? };
    // SAFETY: `builder` is still live, now with `file` added above.
    let font_set = unsafe { builder.CreateFontSet()? };
    // SAFETY: `factory` is still live; `font_set` is the live set just
    // built.
    let collection = unsafe { factory.CreateFontCollectionFromFontSet(&font_set)? };

    // SAFETY: `collection` is the live interface from above.
    let count = unsafe { collection.GetFontFamilyCount() };
    let mut names = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: `collection` is still live; `i` is in `0..count`, the
        // exact range `GetFontFamilyCount` just reported as valid.
        let family = unsafe { collection.GetFontFamily(i)? };
        // SAFETY: `family` is the live interface just returned.
        let localized = unsafe { family.GetFamilyNames()? };
        names.push(read_localized_string(&localized, 0)?);
    }
    Ok((collection, names))
}

/// Read the string at `index` out of an `IDWriteLocalizedStrings` (the
/// two-call length-then-fill pattern every DirectWrite string-table
/// accessor uses) as a Rust `String`.
fn read_localized_string(strings: &IDWriteLocalizedStrings, index: u32) -> WinResult<String> {
    // SAFETY: `strings` is the caller's live interface; `index` is
    // whatever the caller passed through — out-of-range is a documented
    // `Err`, not UB, per `IDWriteLocalizedStrings::GetStringLength`.
    let len = unsafe { strings.GetStringLength(index)? };
    // +1 for the NUL terminator `GetString` writes into the buffer.
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY: `buf` is sized to `len + 1` immediately above — exactly
    // what `GetString`'s own docs require (the string plus its NUL
    // terminator) for the same `index`/`strings` just queried.
    unsafe { strings.GetString(index, &mut buf)? };
    Ok(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Build an [`IDWriteFontFallback`] that resolves `family` for
/// characters the primary font can't cover, falling back to it only
/// *after* every mapping the system's own default fallback chain
/// already provides (issue #929) — mirrors the "later family in the
/// list, consulted only for uncovered characters" contract
/// `crate::gtk::with_nerd_font_fallback` already documents for Pango's
/// cascade, so an app moving from GTK's implicit behaviour to this
/// explicit API sees the same shape.
///
/// `collection` should be the value [`register_font_from_memory`]
/// returned when `family` came from an app-registered font, so `family`
/// resolves against that private collection rather than a
/// same-named system font; pass `None` to resolve `family` against the
/// system collection instead (a system-installed Nerd Font, for
/// instance).
///
/// The mapped range is the full Unicode codepoint space
/// (`0x0..=0x10FFFF`) rather than just the Private-Use-Area block Nerd
/// Font glyphs live in: `family` is only ever *consulted* for a
/// character none of the higher-priority system mappings already
/// resolved (that's what `AddMappings(system_fallback)` before our own
/// `AddMapping` call buys), so a wider range costs nothing and doesn't
/// need updating if a future icon set uses codepoints outside PUA-A.
pub fn build_nerd_font_fallback(
    family: &str,
    collection: Option<&IDWriteFontCollection1>,
) -> WinResult<IDWriteFontFallback> {
    build_nerd_font_fallback_multi(&[(family, collection)])
}

/// [`build_nerd_font_fallback`]'s multi-family generalisation: one
/// `IDWriteFontFallback` carrying an `AddMapping` entry for every
/// `(family, collection)` pair in `mappings`, each consulted in
/// order only for characters none of the higher-priority entries before
/// it (including the system fallback added first) already resolved —
/// `build_nerd_font_fallback` itself is a thin single-mapping call
/// into this fn, so the two can never drift on the shared factory/
/// system-fallback/builder setup. [`crate::win::backend::WinBackend`]
/// uses this directly to combine the app's own `set_nerd_font_fallback`
/// family with the bundled codicon family in one `IDWriteFontFallback` —
/// DirectWrite text formats carry exactly one fallback object each, so
/// two separate single-family calls cannot be composed after the fact
/// the way GTK's family-list string or macOS's cascade-list array can.
pub fn build_nerd_font_fallback_multi(
    mappings: &[(&str, Option<&IDWriteFontCollection1>)],
) -> WinResult<IDWriteFontFallback> {
    // SAFETY: `DWriteCreateFactory` takes no pointers here beyond the
    // factory-type enum; the returned `IDWriteFactory2` is a live COM
    // interface owned by this scope for the rest of the function.
    let factory: IDWriteFactory2 = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
    // SAFETY: `factory` is the live interface from above.
    let system_fallback = unsafe { factory.GetSystemFontFallback()? };
    // SAFETY: `factory` is still live.
    let builder = unsafe { factory.CreateFontFallbackBuilder()? };
    // SAFETY: `builder` is the interface just created; `system_fallback`
    // is the live fallback from above, only borrowed for this call.
    unsafe { builder.AddMappings(&system_fallback)? };

    let ranges = [DWRITE_UNICODE_RANGE {
        first: 0x0000_0000,
        last: 0x0010_FFFF,
    }];
    for &(family, collection) in mappings {
        let family_wide = HSTRING::from(family);
        let family_ptrs = [family_wide.as_ptr()];
        // `AddMapping` wants an `Option<&IDWriteFontCollection>` (the base
        // interface), not the `IDWriteFontCollection1` this module otherwise
        // deals in — `windows-core`'s `Param` blanket impl for `Option<&T>`
        // requires the exact interface type, so the derived-to-base upcast
        // has to happen explicitly via `cast` (a `QueryInterface` on the same
        // underlying COM object, not a new one) rather than relying on the
        // interface hierarchy to coerce it implicitly.
        let base_collection: Option<IDWriteFontCollection> = match collection {
            Some(c) => Some(c.cast()?),
            None => None,
        };
        // SAFETY: `builder` is the live interface built above; `ranges` and
        // `family_ptrs` are local arrays (`family_ptrs`'s pointer comes from
        // `family_wide`, a local `HSTRING` that outlives this call);
        // `base_collection` is either `None` or a live, just-cast interface;
        // `PCWSTR::null()` for the optional locale-fallback name needs no
        // backing buffer.
        unsafe {
            builder.AddMapping(
                &ranges,
                &family_ptrs,
                base_collection.as_ref(),
                &HSTRING::from("en-us"),
                PCWSTR::null(),
                1.0,
            )?
        };
    }
    // SAFETY: `builder` is still the live interface, now with every
    // mapping added above.
    unsafe { builder.CreateFontFallback() }
}

/// Resolve `family`'s `DWRITE_FONT_METRICS` (design-unit ascent/descent/
/// line-gap + units-per-em) from the system font collection. Falls back
/// to the collection's first family if `family` isn't installed — same
/// "don't fail, degrade" posture as every other backend falling back to
/// a system default font.
fn font_face_metrics(factory: &IDWriteFactory, family: &str) -> WinResult<DWRITE_FONT_METRICS> {
    let mut collection: Option<IDWriteFontCollection> = None;
    // SAFETY: `factory` is the caller's live interface; `collection` is
    // a plain stack `Option` the call writes through; `false` (don't
    // check for updates) needs no pointer.
    unsafe { factory.GetSystemFontCollection(&mut collection, false)? };
    // `GetSystemFontCollection` is documented to populate `collection`
    // whenever it returns `Ok(())`, but that's an assumption about the FFI
    // binding's out-param contract, not something the type system proves —
    // propagate rather than `expect`/panic if it's ever violated, matching
    // every other fallible call in this module.
    let collection = collection.ok_or_else(|| {
        WinError::new(
            E_UNEXPECTED,
            "GetSystemFontCollection returned Ok(()) but left the collection unpopulated",
        )
    })?;

    let mut index = 0u32;
    let mut exists = BOOL(0);
    // SAFETY: `collection` is the live interface confirmed populated
    // above; `HSTRING::from` builds an owned string passed by value;
    // `index`/`exists` are plain stack out-params the call writes
    // through.
    unsafe { collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)? };
    let index = if exists.as_bool() { index } else { 0 };

    // SAFETY: `collection` is still live; `index` is either the index
    // `FindFamilyName` just confirmed exists, or `0` — always in range
    // for a non-empty system collection (this function's own doc: "falls
    // back to the collection's first family").
    let font_family = unsafe { collection.GetFontFamily(index)? };
    // SAFETY: `font_family` is the live interface just returned; the
    // three enum values need no pointers.
    let font = unsafe {
        font_family.GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )?
    };
    // SAFETY: `font` is the live interface from above.
    let face = unsafe { font.CreateFontFace()? };
    let mut metrics = DWRITE_FONT_METRICS::default();
    // SAFETY: `face` is the live interface from above; `metrics` is a
    // plain stack struct the call writes through.
    unsafe { face.GetMetrics(&mut metrics) };
    Ok(metrics)
}

/// Answer whether `family` is installed in the system font collection —
/// [`crate::Backend::has_font_family`]'s Win-GUI implementation (issue
/// #1024). Distinct from [`register_font_from_memory`]'s private,
/// app-registered collection: this only ever consults
/// `GetSystemFontCollection`, the same collection
/// [`font_face_metrics`] resolves an editor/UI font family against, so
/// this answers "did the user install this themselves" rather than
/// "did the app bundle it".
pub fn has_font_family(family: &str) -> WinResult<bool> {
    // SAFETY: `DWriteCreateFactory` takes no pointers here beyond the
    // factory-type enum; the returned `IDWriteFactory` is a live COM
    // interface owned by this scope for the rest of the function.
    let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
    let mut collection: Option<IDWriteFontCollection> = None;
    // SAFETY: `factory` is the live interface from above; `collection`
    // is a plain stack `Option` the call writes through; `false` needs
    // no pointer.
    unsafe { factory.GetSystemFontCollection(&mut collection, false)? };
    // Same "documented to populate on Ok(())" caveat as
    // `font_face_metrics` above — propagate rather than assume.
    let collection = collection.ok_or_else(|| {
        WinError::new(
            E_UNEXPECTED,
            "GetSystemFontCollection returned Ok(()) but left the collection unpopulated",
        )
    })?;

    let mut index = 0u32;
    let mut exists = BOOL(0);
    // SAFETY: `collection` is the live interface confirmed populated
    // above; `HSTRING::from` builds an owned string passed by value;
    // `index`/`exists` are plain stack out-params the call writes
    // through.
    unsafe { collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)? };
    Ok(exists.as_bool())
}

/// `IDWriteTextLayout::GetMetrics` for `text` laid out against `format`
/// with an effectively unbounded box — `(width, height)` DIPs of the
/// tightest box the text actually occupies. Used both for the
/// approximate char width in [`DWrite::new`] and exposed via
/// [`DWrite::measure_text`] for arbitrary strings.
fn measure_text(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
) -> WinResult<(f32, f32)> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    // SAFETY: `factory` is the caller's live interface; `wide` is a
    // local `Vec<u16>` that outlives this synchronous call; `format` is
    // the caller's live interface, only borrowed for this call.
    let layout = unsafe { factory.CreateTextLayout(&wide, format, f32::MAX, f32::MAX)? };
    let mut metrics = Default::default();
    // SAFETY: `layout` is the live interface from above; `metrics` is a
    // plain stack struct the call writes through.
    unsafe { layout.GetMetrics(&mut metrics)? };
    Ok((metrics.width, metrics.height))
}

/// Paint `text` inside `rect` (DIPs, target-relative) in `color`, clipped
/// to the rect. Brush creation goes through [`get_or_create_brush`], so
/// repeated calls with the same `(target, color)` reuse one cached
/// `ID2D1SolidColorBrush` instead of creating a fresh one per call.
///
/// This is the single choke point every Win-GUI chrome rasteriser paints
/// text through (`win::status_bar`, `win::tab_bar`, `win::activity_bar`,
/// …, via [`DWrite::draw_text`]/[`DWrite::draw_text_styled`]), so it's
/// also where paint-time text-run recording hooks in for
/// [`super::testing::WinDriver::find`]/`find_bounds`/`inventory`
/// (quadraui#721) — the Win-GUI counterpart of `gtk::painted_text::show_layout`
/// / `macos::text::draw_text`'s recording, sharing the same thread-local
/// sink (`crate::testing::record_text_run`). Unlike those two, Win-GUI
/// doesn't need the `text_run_sink_active()` pre-check to skip expensive
/// measurement work — `rect` is already the caller's own layout box (e.g.
/// a `StatusBar` segment's hit-testable bounds), nothing left to measure —
/// but it's checked anyway so this function's reachability (and therefore
/// its `cfg`) matches `record_text_run`'s exactly; see that function's doc
/// in `crate::testing` for why the two must move together.
fn draw_text(
    target: &ID2D1RenderTarget,
    format: &IDWriteTextFormat,
    text: &str,
    rect: Rect,
    color: Color,
) -> WinResult<()> {
    if crate::testing::text_run_sink_active() {
        crate::testing::record_text_run(text, rect);
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let brush = get_or_create_brush(target, color)?;
    let layout_rect = D2D_RECT_F {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.width,
        bottom: rect.y + rect.height,
    };
    // SAFETY: `target` is still the live render target; `wide` is a
    // local `Vec<u16>` that outlives this synchronous call; `format` is
    // the caller's live interface; `layout_rect` is a local built
    // immediately above and `brush` a live interface (just created or
    // fetched from `get_or_create_brush`'s cache), both borrowed only for
    // this call.
    unsafe {
        target.DrawText(
            &wide,
            format,
            &layout_rect,
            &brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
    Ok(())
}

pub(crate) fn color_to_d2d(color: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: color.r as f32 / 255.0,
        g: color.g as f32 / 255.0,
        b: color.b as f32 / 255.0,
        a: color.a as f32 / 255.0,
    }
}

// ─── Brush cache ─────────────────────────────────────────────────────────
//
// Without it, every one of `fill_rect`/`fill_rounded_rect`/`stroke_rect`/
// `draw_line`/`draw_text` created a fresh `ID2D1SolidColorBrush` and let it
// drop at the end of the call — one `CreateSolidColorBrush` per primitive,
// per frame. Measured on a real 1639x1079 window that's 647 brushes/frame
// for a palette of only ~20 distinct colors.
//
// Ownership: the cache ([`BrushCache`]) is a field of the backend's
// `Surface` — it lives exactly as long as the render target its brushes
// were created against, and is dropped (releasing every brush) through an
// ordinary Rust drop when that surface is replaced or torn down. A
// re-attach / `ensure_surface` recreate therefore starts from an empty
// cache with no explicit "clear" call to forget.
//
// Lookup: every paint helper in this module takes a bare
// `&ID2D1RenderTarget`, not the backend, so `WinBackend::begin_frame`
// *activates* its surface's cache for the duration of the frame via
// [`activate_brush_cache`] and `end_frame` deactivates it. The
// thread-local only ever holds a `Weak` reference plus the owning
// target's raw pointer:
//
// - **No COM release in a TLS destructor.** An earlier revision kept the
//   brushes themselves in a `thread_local!`, so every thread that painted
//   released D2D brushes (and, through them, the last reference to their
//   `ID2D1Factory`) from its TLS destructor at thread exit. On Windows
//   those destructors run under the loader lock, and factory teardown
//   there deadlocked the whole `cargo test --features win` run on real
//   Windows. Dropping a `Weak` never drops the value it points at.
// - **No cross-target reuse.** A brush is tied to the device that created
//   it. Helpers called against any target other than the active one (or
//   outside a backend frame, e.g. a test painting straight onto a
//   `HeadlessSurface`) bypass the cache and create a one-off brush, the
//   pre-cache behaviour. The pointer comparison can't be fooled by address
//   reuse: the `Weak` only upgrades while the `Surface` — and so the
//   target it holds a reference to — is still alive.

/// Per-render-target brush memo, owned by the backend's `Surface`. See the
/// `Brush cache` section comment above.
pub(crate) type BrushCache = Rc<BrushMap>;

/// The map behind a [`BrushCache`].
type BrushMap = RefCell<HashMap<Color, ID2D1SolidColorBrush>>;

/// A fresh, empty [`BrushCache`] for a newly (re)created surface.
pub(crate) fn new_brush_cache() -> BrushCache {
    Rc::new(RefCell::new(HashMap::new()))
}

thread_local! {
    /// The cache [`get_or_create_brush`] consults, plus the raw pointer
    /// of the render target it belongs to. Set by
    /// [`activate_brush_cache`] for the duration of a backend frame.
    static ACTIVE_BRUSH_CACHE: RefCell<Option<(usize, Weak<BrushMap>)>> = const { RefCell::new(None) };
    /// Count of brushes actually created via `CreateSolidColorBrush`
    /// inside [`get_or_create_brush`] — written unconditionally, only read
    /// through the `#[cfg(test)]` accessors below (this issue's
    /// acceptance-test hook).
    static BRUSH_CREATIONS: Cell<u32> = const { Cell::new(0) };
}

/// Make `cache` (owned by the surface wrapping `target`) the one
/// [`get_or_create_brush`] uses on this thread until
/// [`deactivate_brush_cache`].
pub(crate) fn activate_brush_cache(target: &ID2D1RenderTarget, cache: &BrushCache) {
    let entry = (target.as_raw() as usize, Rc::downgrade(cache));
    ACTIVE_BRUSH_CACHE.with(|active| *active.borrow_mut() = Some(entry));
}

/// Stop routing brush lookups through any cache on this thread.
pub(crate) fn deactivate_brush_cache() {
    ACTIVE_BRUSH_CACHE.with(|active| *active.borrow_mut() = None);
}

/// Get the cached `ID2D1SolidColorBrush` for `color` when `target` is the
/// active cache's target, creating (and caching) one on a miss; otherwise
/// create a one-off brush.
fn get_or_create_brush(
    target: &ID2D1RenderTarget,
    color: Color,
) -> WinResult<ID2D1SolidColorBrush> {
    let raw = target.as_raw() as usize;
    let cache = ACTIVE_BRUSH_CACHE.with(|active| match &*active.borrow() {
        Some((owner, weak)) if *owner == raw => weak.upgrade(),
        _ => None,
    });
    if let Some(brush) = cache
        .as_ref()
        .and_then(|cache| cache.borrow().get(&color).cloned())
    {
        return Ok(brush);
    }
    // SAFETY: `target` is the caller's live render target; `color_to_d2d`
    // builds a plain stack `D2D1_COLOR_F` borrowed only for this call;
    // `None` for brush properties asks for the default (opaque) brush.
    let brush = unsafe { target.CreateSolidColorBrush(&color_to_d2d(color), None)? };
    BRUSH_CREATIONS.with(|count| count.set(count.get() + 1));
    if let Some(cache) = cache {
        cache.borrow_mut().insert(color, brush.clone());
    }
    Ok(brush)
}

/// Count of brushes created since the thread started or the last
/// [`reset_brush_creation_count`] call. `#[cfg(test)]` so a non-test
/// build doesn't carry it as dead code.
#[cfg(test)]
pub(crate) fn brush_creation_count() -> u32 {
    BRUSH_CREATIONS.with(|count| count.get())
}

/// Reset [`brush_creation_count`]'s counter to `0`.
#[cfg(test)]
pub(crate) fn reset_brush_creation_count() {
    BRUSH_CREATIONS.with(|count| count.set(0));
}

/// Fill `rect` (DIPs, target-relative) with a solid `color` on `target`.
///
/// Shared by every chrome rasteriser in `crate::win` (status bar, tab bar,
/// activity bar, menu bar — #25) for background fills, active/hover tints,
/// and accent lines, so each one doesn't hand-roll its own
/// `CreateSolidColorBrush` + `FillRectangle` pair. Mirrors
/// [`super::testing::HeadlessSurface::fill_rect`], which exists
/// separately because it also owns the `BeginDraw`/`EndDraw` bracket for
/// the headless test surface; this version assumes the caller is already
/// inside a frame (or a `HeadlessSurface::paint` closure).
pub(crate) fn fill_rect(target: &ID2D1RenderTarget, rect: Rect, color: Color) -> WinResult<()> {
    let brush = get_or_create_brush(target, color)?;
    let rect_f = D2D_RECT_F {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.width,
        bottom: rect.y + rect.height,
    };
    // SAFETY: `target` is still the live render target; `rect_f` is a
    // local built immediately above and `brush` a live interface (just
    // created or fetched from the cache), both borrowed only for this
    // call.
    unsafe { target.FillRectangle(&rect_f, &brush) };
    Ok(())
}

/// [`fill_rect`]'s rounded-corner twin (issue #1073) —
/// `ID2D1RenderTarget::FillRoundedRectangle` with both radii set to
/// `radius`, clamped to half of `rect`'s shorter side (and floored at
/// `0.0`, so a negative radius degrades to "no rounding" rather than
/// flowing through unclamped) so a radius wider than the box it
/// outlines can't produce Direct2D's own degenerate "radius bigger than
/// the rect" shape (the same clamp
/// [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]'s
/// doc requires of every implementor).
pub(crate) fn fill_rounded_rect(
    target: &ID2D1RenderTarget,
    rect: Rect,
    radius: f32,
    color: Color,
) -> WinResult<()> {
    let brush = get_or_create_brush(target, color)?;
    let r = radius.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
    let rounded = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: rect.x,
            top: rect.y,
            right: rect.x + rect.width,
            bottom: rect.y + rect.height,
        },
        radiusX: r,
        radiusY: r,
    };
    // SAFETY: `target` is still the live render target; `rounded` is a
    // local built immediately above and `brush` a live interface (just
    // created or fetched from the cache), both borrowed only for this
    // call.
    unsafe { target.FillRoundedRectangle(&rounded, &brush) };
    Ok(())
}

/// Stroke the outline of `rect` (DIPs, target-relative) in `color` at
/// `stroke_width` — the Direct2D twin of `fill_rect` for the overlay
/// rasterisers (#28: tooltip / context menu / dialog / palette /
/// completions / find-replace / rich-text-popup) that all draw a
/// bordered box, mirroring GTK/Cairo's `cr.rectangle(..).stroke()`
/// idiom used throughout `crate::gtk`.
///
/// # The stroke lands *inside* `rect`
///
/// Direct2D centres a stroke on the geometry it is handed, so passing
/// `rect` through verbatim would put half of `stroke_width` outside the
/// caller's own bounds — over whatever the host painted beside the
/// popup, which for an overlay is somebody else's pixels — and leave
/// the border straddling two device-pixel rows, each antialiased to
/// roughly half coverage instead of one crisp line. This helper insets
/// the geometry by `stroke_width / 2` so the whole border sits within
/// `rect`: at scale 1, a 1-DIP stroke on integer bounds then covers
/// exactly the boundary pixel row/column, and `rect`'s neighbours are
/// left untouched. Cairo has the same centred-stroke rule, which is why
/// `crate::gtk`'s rasterisers offset their 1 px rules by half a pixel
/// for the same reason.
///
/// The inset is clamped to half the rect's own extent (and to `>= 0`)
/// so a stroke wider than the box it outlines degenerates to a filled
/// sliver rather than an inverted rectangle.
pub(crate) fn stroke_rect(
    target: &ID2D1RenderTarget,
    rect: Rect,
    color: Color,
    stroke_width: f32,
) -> WinResult<()> {
    let brush = get_or_create_brush(target, color)?;
    let inset = (stroke_width / 2.0)
        .min(rect.width / 2.0)
        .min(rect.height / 2.0)
        .max(0.0);
    let rect_f = D2D_RECT_F {
        left: rect.x + inset,
        top: rect.y + inset,
        right: rect.x + rect.width - inset,
        bottom: rect.y + rect.height - inset,
    };
    // SAFETY: `target` is still the live render target; `rect_f` is a
    // local built above and `brush` a live interface (just created or
    // fetched from the cache), both borrowed only for this call; `None`
    // for stroke style asks for Direct2D's default solid stroke.
    unsafe { target.DrawRectangle(&rect_f, &brush, stroke_width, None) };
    Ok(())
}

/// [`stroke_rect`]'s rounded-corner twin —
/// `ID2D1RenderTarget::DrawRoundedRectangle` with both radii set to
/// `radius`, clamped the same way [`fill_rounded_rect`]'s is. Same
/// inside-the-bounds inset as [`stroke_rect`], for the same reason (see
/// that function's doc).
pub(crate) fn stroke_rounded_rect(
    target: &ID2D1RenderTarget,
    rect: Rect,
    radius: f32,
    color: Color,
    stroke_width: f32,
) -> WinResult<()> {
    let brush = get_or_create_brush(target, color)?;
    let inset = (stroke_width / 2.0)
        .min(rect.width / 2.0)
        .min(rect.height / 2.0)
        .max(0.0);
    let r = (radius - inset)
        .min(rect.width / 2.0 - inset)
        .min(rect.height / 2.0 - inset)
        .max(0.0);
    let rounded = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F {
            left: rect.x + inset,
            top: rect.y + inset,
            right: rect.x + rect.width - inset,
            bottom: rect.y + rect.height - inset,
        },
        radiusX: r,
        radiusY: r,
    };
    // SAFETY: `target` is still the live render target; `rounded` is a
    // local built immediately above and `brush` a live interface (just
    // created or fetched from the cache), both borrowed only for this
    // call; `None` for stroke style asks for Direct2D's default solid
    // stroke.
    unsafe { target.DrawRoundedRectangle(&rounded, &brush, stroke_width, None) };
    Ok(())
}

/// Push an axis-aligned clip rect (DIPs, target-relative) onto `target`.
/// Every push must be balanced by a [`pop_clip`] — content rasterisers
/// that paint per-row / per-cell text wider than their own bounds (e.g.
/// a horizontally-scrolled `ListView`/`DataTable` row) use this pair to
/// keep scrolled-off glyphs from bleeding into neighbouring rows or
/// columns, mirroring `cr.save()` / `cr.rectangle(..).clip()` /
/// `cr.restore()` on the GTK/Cairo backend. Infallible on
/// `ID2D1RenderTarget` (same posture as `BeginDraw`/`Clear` — see
/// `WinBackend::begin_frame`'s doc), so this and [`pop_clip`] return
/// nothing to propagate.
pub(crate) fn push_clip(target: &ID2D1RenderTarget, rect: Rect) {
    let rect_f = D2D_RECT_F {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.width,
        bottom: rect.y + rect.height,
    };
    // SAFETY: `target` is the caller's live render target; `rect_f` is a
    // plain stack struct built immediately above, borrowed only for this
    // call — infallible on `ID2D1RenderTarget` per this function's own
    // doc comment.
    unsafe { target.PushAxisAlignedClip(&rect_f, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
}

/// Pop the clip most recently pushed by [`push_clip`].
pub(crate) fn pop_clip(target: &ID2D1RenderTarget) {
    // SAFETY: `target` is the caller's live render target; the caller
    // owes the "every push balanced by a pop" invariant this function's
    // own doc comment (and `push_clip`'s) describes.
    unsafe { target.PopAxisAlignedClip() };
}

/// Stroke a line from `(x0, y0)` to `(x1, y1)` (DIPs, target-relative)
/// in `color` at `stroke_width` — [`crate::win::chart`]'s line paths /
/// axis rules / crosshair, the one shape none of this module's other
/// helpers cover (they're all rectangle-based). `strokestyle: None`
/// gives Direct2D's default (solid) stroke.
pub(crate) fn draw_line(
    target: &ID2D1RenderTarget,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: Color,
    stroke_width: f32,
) -> WinResult<()> {
    let brush = get_or_create_brush(target, color)?;
    // SAFETY: `target` is still the live render target; `brush` is the
    // live interface just created or fetched from the cache; `Vector2`s
    // are plain stack structs;
    // `None` for stroke style asks for Direct2D's default solid stroke,
    // per this function's own doc comment.
    unsafe {
        target.DrawLine(
            Vector2 { X: x0, Y: y0 },
            Vector2 { X: x1, Y: y1 },
            &brush,
            stroke_width,
            None,
        );
    }
    Ok(())
}

/// Run `f` with `target`'s transform temporarily set to a translation by
/// `(dx, dy)` DIPs, then restore the identity transform. Returns `f`'s
/// own return value.
///
/// The Direct2D twin of Cairo's `cr.translate(dx, dy)` /
/// `CGContextTranslateCTM` (see [`crate::macos::activity_bar`]'s module
/// doc) — used by callers that paint a bar-relative rasteriser (e.g.
/// [`crate::win::activity_bar::draw_activity_bar`]) at a non-zero
/// on-screen origin without threading that origin through every
/// coordinate the rasteriser itself computes.
///
/// Restoring identity unconditionally (rather than the transform that was
/// active before this call) matches [`with_horizontal_scale`] and every
/// other rasteriser in this crate, which never leaves a non-identity
/// transform set on the target between draw calls. **`f` must not itself
/// leave a non-identity transform active, and callers must not nest this
/// inside another still-open `with_translation`/`with_horizontal_scale`
/// call** — doing so would have the inner call's unconditional identity
/// restore clobber the outer transform rather than composing with it.
pub(crate) fn with_translation<F: FnOnce() -> R, R>(
    target: &ID2D1RenderTarget,
    dx: f32,
    dy: f32,
    f: F,
) -> R {
    let translated = windows_numerics::Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: dx,
        M32: dy,
    };
    // SAFETY: `target` is the caller's live render target; `translated`
    // is a plain stack struct borrowed only for this call — infallible
    // on `ID2D1RenderTarget`, per this function's own doc comment.
    unsafe { target.SetTransform(&translated) };
    let result = f();
    let identity = windows_numerics::Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: 0.0,
        M32: 0.0,
    };
    // SAFETY: same as the `translated` call above — `target` is still
    // the live render target, `identity` a plain stack struct borrowed
    // only for this call, restoring the bracket this function's doc
    // comment guarantees.
    unsafe { target.SetTransform(&identity) };
    result
}

/// Run `f` with `target`'s transform temporarily set to a horizontal
/// scale of `scale_x`, anchored at `anchor_x` (DIPs) so content at that
/// x-coordinate doesn't shift — only stretches/shrinks to either side of
/// it — then restore the identity transform.
///
/// Direct2D has no per-draw-call scale parameter (unlike Cairo's
/// `cr.scale` or Core Text's text-matrix `a` component, see
/// [`crate::macos::text::draw_text_scaled_x`]) — only a render-target-wide
/// transform via `SetTransform`. [`crate::win::terminal::draw_terminal_cells`]
/// uses this to stretch or shrink a double-width glyph (CJK / emoji) so it
/// fills its two-column cell box exactly, using the scale factor from
/// [`crate::terminal_style::wide_glyph_x_scale`] — the same decision GTK
/// and macOS apply (#500, #703).
///
/// Restoring identity unconditionally (rather than the transform that was
/// active before this call) matches every other rasteriser in this crate,
/// which never leaves a non-identity transform set on the target between
/// draw calls.
pub(crate) fn with_horizontal_scale<F: FnOnce()>(
    target: &ID2D1RenderTarget,
    scale_x: f32,
    anchor_x: f32,
    f: F,
) {
    let scaled = windows_numerics::Matrix3x2 {
        M11: scale_x,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: anchor_x * (1.0 - scale_x),
        M32: 0.0,
    };
    // SAFETY: `target` is the caller's live render target; `scaled` is a
    // plain stack struct borrowed only for this call — infallible on
    // `ID2D1RenderTarget`, per this function's own doc comment.
    unsafe { target.SetTransform(&scaled) };
    f();
    let identity = windows_numerics::Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: 0.0,
        M32: 0.0,
    };
    // SAFETY: same as the `scaled` call above — `target` is still the
    // live render target, `identity` a plain stack struct borrowed only
    // for this call, restoring the bracket this function's doc comment
    // guarantees.
    unsafe { target.SetTransform(&identity) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::testing::HeadlessSurface;

    /// [`stroke_rect`] must keep the whole stroke inside the rect it is
    /// given: the boundary pixel row/column carries the border colour at
    /// full strength, and neither the pixel just outside the bounds nor
    /// the one just inside is touched. Without the half-stroke inset,
    /// Direct2D centres the stroke on the geometry and *both* of those
    /// probes come back as a half-coverage blend instead.
    #[test]
    fn stroke_rect_paints_a_crisp_border_inside_the_bounds() {
        const BG: Color = Color::rgb(10, 20, 30);
        const BORDER: Color = Color::rgb(200, 100, 50);

        let surface = HeadlessSurface::new(32, 32).expect("create surface");
        surface
            .paint(|target| {
                let _ = fill_rect(target, Rect::new(0.0, 0.0, 32.0, 32.0), BG);
                let _ = stroke_rect(target, Rect::new(8.0, 8.0, 16.0, 16.0), BORDER, 1.0);
            })
            .expect("paint");

        let rgb = |x: u32, y: u32| {
            let c = surface.pixel_at(x, y);
            (c.r, c.g, c.b)
        };
        let border = (BORDER.r, BORDER.g, BORDER.b);
        let bg = (BG.r, BG.g, BG.b);

        // Each of the four boundary lines is fully covered.
        assert_eq!(rgb(16, 8), border, "top edge");
        assert_eq!(rgb(16, 23), border, "bottom edge");
        assert_eq!(rgb(8, 16), border, "left edge");
        assert_eq!(rgb(23, 16), border, "right edge");

        // Nothing bleeds outside the bounds…
        assert_eq!(rgb(16, 7), bg, "one row above the top edge");
        assert_eq!(rgb(7, 16), bg, "one column left of the left edge");
        assert_eq!(rgb(24, 16), bg, "one column right of the right edge");
        assert_eq!(rgb(16, 24), bg, "one row below the bottom edge");

        // …and the interior is left to whatever was painted underneath.
        assert_eq!(rgb(16, 9), bg, "one row inside the top edge");
        assert_eq!(rgb(9, 16), bg, "one column inside the left edge");
    }

    /// [`with_horizontal_scale`] must restore the identity transform once
    /// its closure returns — every other rasteriser in this crate assumes
    /// the target's transform is always identity when it starts drawing,
    /// so a leaked scale would silently distort every draw call after it
    /// for the rest of the frame.
    #[test]
    fn with_horizontal_scale_restores_identity_transform_after() {
        let surface = HeadlessSurface::new(50, 50).expect("create surface");
        surface
            .paint(|target| {
                with_horizontal_scale(target, 1.5, 10.0, || {
                    // Closure body intentionally does nothing — this test
                    // only checks the transform bracket, not a paint
                    // result.
                });
                let mut m = windows_numerics::Matrix3x2 {
                    M11: 0.0,
                    M12: 0.0,
                    M21: 0.0,
                    M22: 0.0,
                    M31: 0.0,
                    M32: 0.0,
                };
                // SAFETY: `target` is the live render target
                // `Self::paint` passes in; `m` is a plain stack struct
                // the call writes through.
                unsafe { target.GetTransform(&mut m) };
                assert_eq!(
                    (m.M11, m.M12, m.M21, m.M22, m.M31, m.M32),
                    (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
                    "transform must be identity after with_horizontal_scale returns"
                );
            })
            .expect("paint");
    }

    /// The scale is anchored at `anchor_x`: a point exactly at the anchor
    /// must map to itself, while a point one DIP to the right of it moves
    /// by `scale_x` DIPs — confirms the `M31` offset term, not just that
    /// `M11` carries the scale factor.
    #[test]
    fn with_horizontal_scale_anchors_at_the_given_x() {
        let surface = HeadlessSurface::new(50, 50).expect("create surface");
        surface
            .paint(|target| {
                with_horizontal_scale(target, 2.0, 10.0, || {
                    let mut m = windows_numerics::Matrix3x2 {
                        M11: 0.0,
                        M12: 0.0,
                        M21: 0.0,
                        M22: 0.0,
                        M31: 0.0,
                        M32: 0.0,
                    };
                    // SAFETY: `target` is the live render target
                    // `Self::paint` passes in; `m` is a plain stack
                    // struct the call writes through.
                    unsafe { target.GetTransform(&mut m) };
                    // x' = x * M11 + M31
                    let map_x = |x: f32| x * m.M11 + m.M31;
                    assert!(
                        (map_x(10.0) - 10.0).abs() < 1e-6,
                        "anchor point must map to itself"
                    );
                    assert!(
                        (map_x(11.0) - 12.0).abs() < 1e-6,
                        "one DIP right of the anchor must move by scale_x DIPs"
                    );
                });
            })
            .expect("paint");
    }

    /// #1077 regression: `draw_text` must paint a label into a box sized
    /// to exactly its own measured width — the shared
    /// `primitives::dialog`/`primitives::text_display` paints do exactly
    /// this (`row_rect`/`text_rect` widths come straight from
    /// `surface_measure_text`), so a caller-visible failure here reads as
    /// "the button label silently doesn't paint."
    ///
    /// This reproduces the exact measurements from the first shared-paint
    /// caller to hit it (a "  OK  " dialog button label at `x =
    /// 129.45442`): `layout_rect.right = x + width` loses a fraction of a
    /// DIP to `f32` rounding versus the `width` `measure_text` returned
    /// for the same string (`25.091144562` vs `25.091140747` in `f64`),
    /// so the box is a hair narrower than the text it's sized to. Under
    /// the default `DWRITE_WORD_WRAPPING_WRAP`, DirectWrite reflows the
    /// trailing `"  "` onto a second line, which
    /// `D2D1_DRAW_TEXT_OPTIONS_CLIP` then crops away entirely — zero
    /// pixels painted, even though the box is (up to float noise) exactly
    /// the text's own width. `DWrite::new` sets
    /// `DWRITE_WORD_WRAPPING_NO_WRAP` precisely so this can't happen: an
    /// exact-width box always paints on one line, overflow (if any)
    /// clips at the right edge instead of reflowing.
    #[test]
    fn draw_text_paints_a_label_sized_to_its_own_measured_width() {
        const BG: Color = Color::rgb(10, 20, 30);
        const FG: Color = Color::rgb(220, 40, 40);

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let text = "  OK  ";
        let (width, height) = dwrite.measure_text(text).expect("measure_text");

        let x = 129.454_42_f32;
        let rect = Rect::new(x, 4.0, width, height.max(1.0));

        let surface = HeadlessSurface::new(220, 40).expect("create surface");
        surface
            .paint(|target| {
                let _ = fill_rect(target, Rect::new(0.0, 0.0, 220.0, 40.0), BG);
                dwrite.draw_text(target, text, rect, FG).expect("draw_text");
            })
            .expect("paint");

        let bg = (BG.r, BG.g, BG.b);
        let found = (rect.y as u32..(rect.y + rect.height) as u32)
            .flat_map(|y| (rect.x as u32..(rect.x + rect.width) as u32).map(move |x| (x, y)))
            .map(|(x, y)| surface.pixel_at(x, y))
            .any(|px| (px.r, px.g, px.b) != bg);
        assert!(
            found,
            "a label box sized to exactly its own measured width must still paint at \
             least one non-background pixel"
        );
    }

    /// End-to-end proof that [`register_font_from_memory`] +
    /// [`build_nerd_font_fallback_multi`] actually resolve
    /// [`crate::codicon::CLOSE`] through the bundled font, mirroring the
    /// exact pipeline `WinBackend::new`/`set_nerd_font_fallback` run:
    /// register the real [`crate::codicon::FONT_BYTES`], build a
    /// fallback mapping just that one family, bake it into a
    /// `DWrite` via `fallback: Some(&fallback)`, and paint the glyph.
    /// If registration or the fallback mapping were ever silently
    /// broken (wrong family name, dropped mapping), DirectWrite would
    /// have nothing to resolve that Private-Use-Area codepoint against
    /// and this would paint zero ink, the same failure mode
    /// `draw_text_paints_a_label_sized_to_its_own_measured_width` above
    /// guards for ordinary text.
    #[test]
    fn codicon_close_glyph_resolves_through_the_registered_fallback_and_paints_ink() {
        const BG: Color = Color::rgb(10, 20, 30);
        const FG: Color = Color::rgb(220, 40, 40);

        let (collection, names) = register_font_from_memory(crate::codicon::FONT_BYTES)
            .expect("the bundled codicon.ttf must register with DirectWrite");
        assert!(
            names.iter().any(|n| n == crate::codicon::FONT_FAMILY),
            "registered family names {names:?} should include {:?}",
            crate::codicon::FONT_FAMILY
        );
        let fallback =
            build_nerd_font_fallback_multi(&[(crate::codicon::FONT_FAMILY, Some(&collection))])
                .expect("build a fallback mapping just the registered codicon family");

        let (dwrite, _, _) = DWrite::new("Segoe UI", 16.0, Some(&fallback)).expect("create DWrite");
        let glyph = crate::codicon::CLOSE.to_string();
        let (width, height) = dwrite.measure_text(&glyph).expect("measure_text");

        let rect = Rect::new(4.0, 4.0, width.max(1.0), height.max(1.0));
        let surface = HeadlessSurface::new(40, 40).expect("create surface");
        surface
            .paint(|target| {
                let _ = fill_rect(target, Rect::new(0.0, 0.0, 40.0, 40.0), BG);
                dwrite
                    .draw_text(target, &glyph, rect, FG)
                    .expect("draw_text");
            })
            .expect("paint");

        let bg = (BG.r, BG.g, BG.b);
        let found = (rect.y as u32..(rect.y + rect.height) as u32)
            .flat_map(|y| (rect.x as u32..(rect.x + rect.width) as u32).map(move |x| (x, y)))
            .map(|(x, y)| surface.pixel_at(x, y))
            .any(|px| (px.r, px.g, px.b) != bg);
        assert!(
            found,
            "the codicon close glyph, painted through the registered-font fallback, must \
             paint at least one non-background pixel"
        );
    }

    /// #1077 review follow-up: the fix above (`DWRITE_WORD_WRAPPING_NO_WRAP`)
    /// is justified by "no caller relies on DirectWrite's own wrapping" —
    /// every rasteriser pre-splits multi-line content into one `draw_text`
    /// call per already-measured line (see this module's doc). This pins
    /// the other half of that claim for a box that's *genuinely* too
    /// narrow for its text (not just a hair short by float rounding, like
    /// the test above): a long run given a box only wide enough for its
    /// first character or two must still paint visible ink right at the
    /// box's left edge — i.e. clip at the right edge — rather than reflow
    /// its first word onto a second line that a caller's single-line-tall
    /// box then crops away entirely (the exact silent-vanishing failure
    /// mode `DWRITE_WORD_WRAPPING_WRAP` produced pre-fix, reproduced
    /// above for the narrower "hair short by rounding" case).
    #[test]
    fn draw_text_clips_an_overlong_run_instead_of_wrapping_it_away() {
        const BG: Color = Color::rgb(10, 20, 30);
        const FG: Color = Color::rgb(220, 40, 40);

        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let text = "This label is far too long to fit in the narrow box below it";
        let (_, natural_height) = dwrite.measure_text(text).expect("measure_text");

        // A box far narrower than even the text's first word, but
        // exactly one line tall — what a caller passes when it has
        // already decided this is single-line content (e.g. a
        // status-bar segment or table cell) and expects overflow to
        // clip, not grow the box downward or hide the start of the run.
        let rect = Rect::new(4.0, 4.0, 12.0, natural_height.max(1.0));

        let surface = HeadlessSurface::new(220, 40).expect("create surface");
        surface
            .paint(|target| {
                let _ = fill_rect(target, Rect::new(0.0, 0.0, 220.0, 40.0), BG);
                dwrite.draw_text(target, text, rect, FG).expect("draw_text");
            })
            .expect("paint");

        let bg = (BG.r, BG.g, BG.b);
        let ink_in_box = (rect.y as u32..(rect.y + rect.height) as u32)
            .flat_map(|y| (rect.x as u32..(rect.x + rect.width) as u32).map(move |x| (x, y)))
            .map(|(x, y)| surface.pixel_at(x, y))
            .any(|px| (px.r, px.g, px.b) != bg);
        assert!(
            ink_in_box,
            "a box far narrower than the text's first word must still paint the visible \
             prefix that fits (clipped at the right edge), not vanish entirely because \
             the first word reflowed onto a line this single-line-tall box can't show"
        );
    }

    /// [`DWrite::minimap_text_format`] must cache by rounded
    /// px — a second call at the exact same size reuses the cached
    /// `IDWriteTextFormat` (same underlying COM object, proven via
    /// `Interface::as_raw` identity) rather than allocating a fresh one
    /// every row/frame, and a different size still gets its own, distinct
    /// format.
    #[test]
    fn minimap_text_format_caches_by_rounded_px() {
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");

        let a = dwrite.minimap_text_format(6.0).expect("format at 6px");
        let b = dwrite
            .minimap_text_format(6.0)
            .expect("format at 6px again");
        assert_eq!(
            a.as_raw(),
            b.as_raw(),
            "the same rounded px must reuse the cached format"
        );
        assert_eq!(dwrite.minimap_format_cache_len(), 1);

        let c = dwrite.minimap_text_format(12.0).expect("format at 12px");
        assert_ne!(
            a.as_raw(),
            c.as_raw(),
            "a different rounded px must create a new format"
        );
        assert_eq!(dwrite.minimap_format_cache_len(), 2);
    }
}
