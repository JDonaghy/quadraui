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
//! `super::mod`'s `mod text;` declaration and `backend.rs`'s module docs
//! for why the rest of this repo's `--features win` compile gate stays
//! meaningful without a Windows host.

use windows::core::{Error as WinError, Result as WinResult, BOOL, HSTRING};
use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{ID2D1RenderTarget, D2D1_DRAW_TEXT_OPTIONS_CLIP};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
};

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
pub(crate) struct DWrite {
    factory: IDWriteFactory,
    text_format: IDWriteTextFormat,
    /// Bold variant of `text_format`, same family/size — built alongside
    /// it so chrome rasterisers (`StatusBar` segment `bold`, #25) can
    /// request bold-weight measurement/painting without constructing a
    /// throwaway `IDWriteTextFormat` per call.
    bold_text_format: IDWriteTextFormat,
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
    /// Returns the constructed handles plus `(line_height, char_width)`
    /// resolved from the format's real font metrics, so the caller
    /// ([`super::backend::WinBackend::attach_surface`]) can feed them
    /// straight into `set_current_line_height`/`set_current_char_width`
    /// without a second round-trip through this module.
    pub(crate) fn new(family: &str, size_pt: f32) -> WinResult<(Self, f32, f32)> {
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let size_dip = pt_to_dip(size_pt);
        let text_format =
            create_text_format(&factory, family, size_dip, DWRITE_FONT_WEIGHT_NORMAL)?;
        let bold_text_format =
            create_text_format(&factory, family, size_dip, DWRITE_FONT_WEIGHT_BOLD)?;

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
            },
            line_height,
            char_width,
        ))
    }

    /// `(width, height)` DIPs of `text` laid out against this format —
    /// the `measure_text(text) -> (width_dips, height_dips)` helper
    /// issue #21 asks for, scoped to the current editor font.
    pub(crate) fn measure_text(&self, text: &str) -> WinResult<(f32, f32)> {
        measure_text(&self.factory, &self.text_format, text)
    }

    /// Like [`Self::measure_text`], but against the bold variant when
    /// `bold` is `true` — chrome rasterisers (e.g. [`crate::StatusBar`]'s
    /// per-segment `bold` flag, #25) use this so the fit/paint widths
    /// agree regardless of weight.
    pub(crate) fn measure_text_styled(&self, text: &str, bold: bool) -> WinResult<(f32, f32)> {
        measure_text(&self.factory, self.format_for(bold), text)
    }

    /// Paint `text` inside `rect` (DIPs, target-relative) in `color`,
    /// clipped to the rect, using this format against `target`.
    pub(crate) fn draw_text(
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
    pub(crate) fn draw_text_styled(
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

/// Resolve `family`'s `DWRITE_FONT_METRICS` (design-unit ascent/descent/
/// line-gap + units-per-em) from the system font collection. Falls back
/// to the collection's first family if `family` isn't installed — same
/// "don't fail, degrade" posture as every other backend falling back to
/// a system default font.
fn font_face_metrics(factory: &IDWriteFactory, family: &str) -> WinResult<DWRITE_FONT_METRICS> {
    let mut collection: Option<IDWriteFontCollection> = None;
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
    unsafe { collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)? };
    let index = if exists.as_bool() { index } else { 0 };

    let font_family = unsafe { collection.GetFontFamily(index)? };
    let font = unsafe {
        font_family.GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )?
    };
    let face = unsafe { font.CreateFontFace()? };
    let mut metrics = DWRITE_FONT_METRICS::default();
    unsafe { face.GetMetrics(&mut metrics) };
    Ok(metrics)
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
    let layout = unsafe { factory.CreateTextLayout(&wide, format, f32::MAX, f32::MAX)? };
    let mut metrics = Default::default();
    unsafe { layout.GetMetrics(&mut metrics)? };
    Ok((metrics.width, metrics.height))
}

/// Paint `text` inside `rect` (DIPs, target-relative) in `color`, clipped
/// to the rect. Creates a throwaway `ID2D1SolidColorBrush` per call —
/// fine for this issue's infrastructure role; a rasteriser painting many
/// runs per frame should hoist brush creation once real callers land
/// (same "seam, not yet wired" posture as every other `todo!()` in
/// `backend.rs`).
fn draw_text(
    target: &ID2D1RenderTarget,
    format: &IDWriteTextFormat,
    text: &str,
    rect: Rect,
    color: Color,
) -> WinResult<()> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let brush = unsafe { target.CreateSolidColorBrush(&color_to_d2d(color), None)? };
    let layout_rect = D2D_RECT_F {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.width,
        bottom: rect.y + rect.height,
    };
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
    let brush = unsafe { target.CreateSolidColorBrush(&color_to_d2d(color), None)? };
    let rect_f = D2D_RECT_F {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.width,
        bottom: rect.y + rect.height,
    };
    unsafe { target.FillRectangle(&rect_f, &brush) };
    Ok(())
}
