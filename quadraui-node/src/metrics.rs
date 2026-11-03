//! Frame inspection & backend metrics -- thin napi-rs leaves straight over
//! [`Backend`]'s own measurement methods, the napi-rs analog of
//! `quadraui-py`'s own metrics methods on `QuiApp`.
//!
//! Every method resolves against `self.driver.backend()`, this app's one
//! headless `TuiBackend` -- there is no `run_native`/`run_tui` analog on
//! this crate's `QuiApp` to resolve against instead, so a caller here always
//! gets numbers that agree with what `screen()`/`screenContains()` actually
//! show.

use super::*;

/// `[[fg_r, fg_g, fg_b], [bg_r, bg_g, bg_b]]` -- [`QuiApp::cell_colors`]'s
/// return shape, named so the signature doesn't trip clippy's
/// `type_complexity` lint -- mirrors `quadraui-py`'s own `CellColors`.
type CellColors = ((u8, u8, u8), (u8, u8, u8));

#[napi]
impl QuiApp {
    /// This app's current viewport as `[width, height, scale]`, in cells --
    /// [`Backend::viewport`]. `scale` is always `1.0` on the headless
    /// `TuiBackend`. Matches the `width`/`height` this app was constructed
    /// (or last `resize`d) with.
    #[napi]
    pub fn viewport(&mut self, env: Env) -> Result<(f64, f64, f64)> {
        self.guarded(env, |app| {
            let vp = app.driver.backend().viewport();
            Ok((vp.width as f64, vp.height as f64, vp.scale as f64))
        })
    }

    /// Height of one standard text row, in the headless backend's native
    /// units -- [`Backend::line_height`]. Always `1.0` (one terminal cell).
    #[napi(js_name = "lineHeight")]
    pub fn line_height(&mut self, env: Env) -> Result<f64> {
        self.guarded(env, |app| Ok(app.driver.backend().line_height() as f64))
    }

    /// Approximate monospace character width, in the headless backend's
    /// native units -- [`Backend::char_width`]. Always `1.0` (one terminal
    /// cell).
    #[napi(js_name = "charWidth")]
    pub fn char_width(&mut self, env: Env) -> Result<f64> {
        self.guarded(env, |app| Ok(app.driver.backend().char_width() as f64))
    }

    /// The advance `draw_list` actually paints `ListView` row text with --
    /// [`Backend::list_char_width`]. Not always the same as `charWidth` on
    /// a pixel backend, whose chrome font can differ from its editor font.
    #[napi(js_name = "listCharWidth")]
    pub fn list_char_width(&mut self, env: Env) -> Result<f64> {
        self.guarded(env, |app| Ok(app.driver.backend().list_char_width() as f64))
    }

    /// Line height of the chrome / UI font --
    /// [`Backend::chrome_line_height`]. Equal to `lineHeight` on the
    /// headless backend; use this instead of `lineHeight` when sizing
    /// chrome rows on a backend with a separate chrome font.
    #[napi(js_name = "chromeLineHeight")]
    pub fn chrome_line_height(&mut self, env: Env) -> Result<f64> {
        self.guarded(env, |app| {
            Ok(app.driver.backend().chrome_line_height() as f64)
        })
    }

    /// Width this backend reserves for its own native scrollbar overlay
    /// alongside scrollable content -- [`Backend::scrollbar_reserve`].
    /// Always `0.0` on the headless backend.
    #[napi(js_name = "scrollbarReserve")]
    pub fn scrollbar_reserve(&mut self, env: Env) -> Result<f64> {
        self.guarded(env, |app| {
            Ok(app.driver.backend().scrollbar_reserve() as f64)
        })
    }

    /// Whether this backend can render individual text rows at a larger
    /// font size -- [`Backend::scales_text_rows`]. Always `false` on the
    /// headless backend, a fixed-cell grid where a terminal cell can't
    /// grow.
    #[napi(js_name = "scalesTextRows")]
    pub fn scales_text_rows(&mut self, env: Env) -> Result<bool> {
        self.guarded(env, |app| Ok(app.driver.backend().scales_text_rows()))
    }

    /// Snaps a proposed height, in the same units as `lineHeight`, to what
    /// this backend will actually paint -- [`Backend::snap_height`].
    /// Identity on the headless backend, whose cell grid has no sub-row
    /// resolution to snap away.
    #[napi(js_name = "snapHeight")]
    pub fn snap_height(&mut self, env: Env, h: f64) -> Result<f64> {
        self.guarded(env, |app| {
            Ok(app.driver.backend().snap_height(h as f32) as f64)
        })
    }

    /// Real `[width, height]`, in the headless backend's native units, of
    /// `text` rendered in `role`'s font -- [`Backend::measure_text`]. `role`
    /// is `"chrome"` (the default) or `"editor"`; the headless backend
    /// returns the same cell-count width (height always `1.0`) for either.
    #[napi(js_name = "measureText")]
    pub fn measure_text(
        &mut self,
        env: Env,
        text: String,
        role: Option<String>,
    ) -> Result<(f64, f64)> {
        self.check_not_dispatching()?;
        let role = font_role(role.as_deref().unwrap_or("chrome"))
            .map_err(|err| invalid_font_role_error(format!("measureText: {err}")))?;
        self.guarded(env, |app| {
            let (w, h) = app.driver.backend().measure_text(&text, role);
            Ok((w as f64, h as f64))
        })
    }

    /// This backend's declared capabilities -- [`Backend::backend_caps`] --
    /// as a plain object mapping capability name to whether it's declared.
    /// Covers every name in [`BackendCaps::vocabulary`] plus three fields
    /// that vocabulary excludes because they aren't plain
    /// capability-presence booleans: `kitty_keyboard`/`sgr_pixel_mouse`
    /// (bools, terminal input-protocol support) and `color_depth` (one of
    /// `"ansi16"`, `"indexed256"`, `"true_color"`).
    #[napi(js_name = "backendCaps")]
    pub fn backend_caps(&mut self, env: Env) -> Result<Object<'_>> {
        self.guarded(env, |app| {
            let caps = app.driver.backend().backend_caps();
            let mut obj = Object::new(&env)?;
            for name in BackendCaps::vocabulary() {
                obj.set(name, caps.has(name))?;
            }
            obj.set("kitty_keyboard", caps.kitty_keyboard)?;
            obj.set("sgr_pixel_mouse", caps.sgr_pixel_mouse)?;
            obj.set("color_depth", color_depth_name(caps.color_depth))?;
            Ok(obj)
        })
    }

    /// RGB colour painted at one headless-backend cell, read back as
    /// `[[fg_r, fg_g, fg_b], [bg_r, bg_g, bg_b]]` --
    /// [`quadraui::tui::testing::TuiDriver::style_at`]. `None` if `(x, y)`
    /// is outside the current viewport. Exists so a test can prove
    /// `setTheme`/`setStyle` actually changed what gets painted, not just
    /// what `theme()`/`style()` read back.
    #[napi(js_name = "cellColors")]
    pub fn cell_colors(&mut self, env: Env, x: u16, y: u16) -> Result<Option<CellColors>> {
        self.guarded(env, |app| {
            app.driver.render();
            take_pending_error(app.driver.app_mut())?;
            let Some(style) = app.driver.style_at(x, y) else {
                return Ok(None);
            };
            Ok(Some((ratatui_rgb(style.fg), ratatui_rgb(style.bg))))
        })
    }
}

/// Extracts `(r, g, b)` from a [`quadraui::tui::testing::Color`] -- `(0, 0,
/// 0)` for any non-`Rgb` variant, which no in-tree rasteriser ever paints
/// (every one goes through `quadraui::tui::ratatui_color`, which only ever
/// constructs `Rgb`) -- mirrors `quadraui-py`'s own `ratatui_rgb`.
fn ratatui_rgb(color: quadraui::tui::testing::Color) -> (u8, u8, u8) {
    match color {
        quadraui::tui::testing::Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}
