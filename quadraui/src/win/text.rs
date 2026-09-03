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

use windows::core::{Error as WinError, Result as WinResult, BOOL, HSTRING};
use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1RenderTarget, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
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
    pub fn new(family: &str, size_pt: f32) -> WinResult<(Self, f32, f32)> {
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
    let brush = unsafe { target.CreateSolidColorBrush(&color_to_d2d(color), None)? };
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
    unsafe { target.DrawRectangle(&rect_f, &brush, stroke_width, None) };
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
    unsafe { target.PushAxisAlignedClip(&rect_f, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
}

/// Blend `over` on top of `base` at `alpha` (`0.0` = all `base`, `1.0`
/// = all `over`) — the CPU-side stand-in for Cairo's `set_source_rgba`
/// alpha-blended fills (`gtk::data_table`'s selection/hover tint,
/// `gtk::editor`'s cursor/selection overlays), since [`fill_rect`] only
/// takes an opaque colour: the render target here is created with
/// `D2D1_ALPHA_MODE_IGNORE`/`UNKNOWN` and every rasteriser in this
/// module paints with plain solid-colour fills, so pre-mixing the
/// colour on the CPU is simpler than adding a second, alpha-aware fill
/// path solely for a handful of tint overlays.
pub(crate) fn blend(base: Color, over: Color, alpha: f32) -> Color {
    let alpha = alpha.clamp(0.0, 1.0);
    let mix = |b: u8, o: u8| -> u8 { (b as f32 * (1.0 - alpha) + o as f32 * alpha).round() as u8 };
    Color::rgb(
        mix(base.r, over.r),
        mix(base.g, over.g),
        mix(base.b, over.b),
    )
}

/// Pop the clip most recently pushed by [`push_clip`].
pub(crate) fn pop_clip(target: &ID2D1RenderTarget) {
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
    let brush = unsafe { target.CreateSolidColorBrush(&color_to_d2d(color), None)? };
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

/// Fill a circle centred at `(cx, cy)` (DIPs, target-relative) with
/// radius `r` in `color` — [`crate::win::chart`]'s data-point hover
/// marker.
pub(crate) fn fill_circle(
    target: &ID2D1RenderTarget,
    cx: f32,
    cy: f32,
    r: f32,
    color: Color,
) -> WinResult<()> {
    let brush = unsafe { target.CreateSolidColorBrush(&color_to_d2d(color), None)? };
    let ellipse = D2D1_ELLIPSE {
        point: Vector2 { X: cx, Y: cy },
        radiusX: r,
        radiusY: r,
    };
    unsafe { target.FillEllipse(&ellipse, &brush) };
    Ok(())
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
}
