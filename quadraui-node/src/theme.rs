//! Theme, style & fonts -- thin napi-rs leaves straight over [`Backend`]'s
//! own theme/style/font surface, the napi-rs analog of `quadraui-py`'s own
//! theme/style/font methods on `QuiApp`.
//!
//! `setTheme`/`setStyle`/`setNerdFonts`/`setEditorFont`/`setUiFont`/
//! `registerFontFromMemory`/`setNerdFontFallback` each takes effect
//! immediately on this app's own live backend *and* records the change on
//! [`BindingHost`], so [`BindingHost::setup`] replays it if a future
//! `AppLogic::setup` call constructs a fresh backend -- mirrors
//! `quadraui-py`'s own replay contract for the same methods.
//!
//! Every method here resolves against `self.driver.backend()`, this app's
//! own headless `TuiBackend`, and so calls [`QuiApp::check_not_live`]
//! first: once `startTui`/`startNative` is running, that backend is the
//! dormant pre-live one, not the live session's own, so a change (or a
//! read) here would otherwise silently miss the terminal/window actually
//! on screen.
//!
//! `setTheme`/`setStyle` above are whole-app-wide -- every `draw_*` call
//! after them paints with the new values, everywhere. A render callback
//! that wants to vary the theme/style for just one subtree (e.g. a
//! destructive panel tinted red) instead sets an optional `"style"` key on
//! any layout-tree node it returns -- see `docs/bindings-wire-format.md`'s
//! own doc for that wire shape. That path is handled entirely inside
//! `quadraui-binding-core` (`layout::StyleOverride`, `render::paint_layout`'s
//! `Styled` arm), with no napi-rs method of its own: it rides the same
//! render callback a tree already returns.
//!
//! `defineStyle` is the one method here that is neither a `Backend` leaf
//! nor a replayed host field: it fills [`BindingHost::style_sets`], the
//! registry a node's `"class"` key resolves against during each frame's own
//! render-callback decode (`decode_render_output_with_styles`). Nothing is
//! ever pushed onto a `Backend` for it, so there is nothing for
//! [`BindingHost::setup`] to replay.

use super::*;

#[napi]
impl QuiApp {
    /// This app's active [`quadraui::Theme`] -- [`Backend::theme`] -- as a
    /// plain object with one entry per `Theme` field, each colour itself a
    /// `{r, g, b, a}` object.
    #[napi]
    pub fn theme(&mut self, env: Env) -> Result<Unknown<'static>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let theme = app.driver.backend().theme();
            encode_via_json(env, "theme", &theme)
        })
    }

    /// Overrides one or more [`quadraui::Theme`] fields on this app's
    /// backend -- [`Backend::set_theme`] -- merged onto whatever the
    /// backend's current theme already has via [`merge_overrides`], so a
    /// caller only needs to name the fields it actually wants to change.
    /// `overrides` keys must be real `Theme` field names. Each value is a
    /// `"#rrggbb"`/`"#rrggbbaa"` hex string, an `[r, g, b[, a]]` array, or a
    /// `{r, g, b[, a]}` object.
    #[napi(js_name = "setTheme")]
    pub fn set_theme(&mut self, env: Env, overrides: Object) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        let keys = Object::keys(&overrides)?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let theme = merge_overrides(backend.theme(), keys, |key| {
                let json = match overrides.get::<Unknown>(key).map_err(|err| err.reason)? {
                    Some(val) => to_descriptor_value(val)?,
                    // `Object::keys` can report an own key whose value is
                    // JS `undefined` (napi's `Object::get` surfaces that as
                    // `None`, not `Some(<undefined>)`) -- mirrors
                    // `to_descriptor_value`'s own `Undefined | Null =>
                    // JsonValue::Null` rather than panicking on an
                    // assumption the JS object model doesn't actually hold.
                    None => JsonValue::Null,
                };
                theme_color_value(key, json)
            })
            .map_err(|err| override_error_to_js("setTheme", err))?;
            backend.set_theme(theme);
            app_logic.host.theme = Some(theme);
            Ok(())
        })
    }

    /// This app's active [`quadraui::Style`] -- [`Backend::style`] -- as a
    /// plain object with one entry per token.
    #[napi]
    pub fn style(&mut self, env: Env) -> Result<Unknown<'static>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let style = app.driver.backend().style();
            encode_via_json(env, "style", &style)
        })
    }

    /// Overrides one or more [`quadraui::Style`] tokens --
    /// [`Backend::set_style`] -- merged onto the backend's current style
    /// the same way [`Self::set_theme`] merges `Theme` fields.
    #[napi(js_name = "setStyle")]
    pub fn set_style(&mut self, env: Env, overrides: Object) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        let keys = Object::keys(&overrides)?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let style = merge_overrides(backend.style(), keys, |key| {
                match overrides.get::<Unknown>(key).map_err(|err| err.reason)? {
                    Some(val) => to_descriptor_value(val),
                    // See `setTheme`'s matching arm for why `Object::get`
                    // can report `None` for one of `Object::keys`' own keys.
                    None => Ok(JsonValue::Null),
                }
            })
            .map_err(|err| override_error_to_js("setStyle", err))?;
            backend.set_style(style);
            app_logic.host.style = Some(style);
            Ok(())
        })
    }

    /// Registers a named style set under `name` -- the CSS-class analogue
    /// of [`Self::set_theme`]'s whole-app override, and the napi-rs
    /// counterpart of `quadraui-py`'s own `define_style`. `overrides`
    /// takes the same keys a layout node's own `"style"` object does
    /// (`bg`/`fg`/`border`/`accent`/`surface_bg`/`accent_bg`/`padding`/
    /// `corner_radius`/`border_width`; an unknown key throws
    /// `InvalidDescriptorError`). Any node in any later frame's tree then
    /// names the set through its own `"class"` key -- a single name, a
    /// whitespace-separated string of them, or an array -- instead of
    /// restating the same tokens inline on every node that wants them.
    ///
    /// Several classes merge left to right (a later name wins a token an
    /// earlier one also named), and the node's own inline `"style"`
    /// object beats all of them. A class name no set was ever defined
    /// under is ignored rather than throwing, the same unknown-key policy
    /// an unrecognised sibling key on a node object gets.
    ///
    /// Four sets exist before any call here -- `primary`, `secondary`,
    /// `danger`, `subtle` -- each derived from the app's live `Theme` (so
    /// they follow a [`Self::set_theme`] call) rather than from fixed
    /// colours. Defining one of those names replaces it.
    ///
    /// Redefining a set applies from the next frame on: nothing caches a
    /// resolved override, and this requests that frame immediately
    /// ([`Backend::request_frame_in`]) so a running `startTui`/
    /// `startNative` loop repaints rather than waiting for the next
    /// unrelated event.
    #[napi(js_name = "defineStyle")]
    pub fn define_style(&mut self, env: Env, name: String, overrides: Object) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        if name.is_empty() {
            return Err(invalid_descriptor_error(
                "defineStyle: \"name\" must not be empty",
            ));
        }
        let json = to_descriptor_value(overrides.to_unknown())
            .map_err(|detail| invalid_descriptor_error(format!("defineStyle: {detail}")))?;
        let parsed = decode_style_overrides("defineStyle", &json)
            .map_err(invalid_descriptor_error)?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            app_logic.host.style_sets.define(name, parsed);
            backend.request_frame_in(std::time::Duration::ZERO);
            Ok(())
        })
    }

    /// Every style-set name currently registered, sorted -- the four
    /// built-ins plus whatever [`Self::define_style`] has added.
    #[napi(js_name = "styleSetNames")]
    pub fn style_set_names(&mut self, env: Env) -> Result<Vec<String>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            Ok(app
                .driver
                .app()
                .host
                .style_sets
                .names()
                .into_iter()
                .map(str::to_owned)
                .collect())
        })
    }

    /// Syncs the nerd-fonts flag -- [`Backend::set_nerd_fonts`] -- so
    /// icon-bearing surfaces render `Icon::glyph` when `enabled` rather
    /// than `Icon::fallback`.
    #[napi(js_name = "setNerdFonts")]
    pub fn set_nerd_fonts(&mut self, env: Env, enabled: bool) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.set_nerd_fonts(enabled);
            app_logic.host.nerd_fonts = Some(enabled);
            Ok(())
        })
    }

    /// Whatever was last handed to [`Self::set_nerd_fonts`] --
    /// [`Backend::nerd_fonts_enabled`].
    #[napi(js_name = "nerdFontsEnabled")]
    pub fn nerd_fonts_enabled(&mut self, env: Env) -> Result<bool> {
        self.check_not_live()?;
        self.guarded(env, |app| Ok(app.driver.backend().nerd_fonts_enabled()))
    }

    /// Overrides the font that paints editor content --
    /// [`Backend::set_editor_font`]. No-op on the headless `TuiBackend` (a
    /// fixed-cell backend has no font concept), but still recorded and
    /// replayed by [`BindingHost::setup`].
    #[napi(js_name = "setEditorFont")]
    pub fn set_editor_font(&mut self, env: Env, family: String, size_pt: f64) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.set_editor_font(&family, size_pt as f32);
            app_logic.host.editor_font = Some((family, size_pt as f32));
            Ok(())
        })
    }

    /// Overrides the font that paints chrome -- [`Backend::set_ui_font`].
    /// Same no-op-on-TUI-but-still-replayed contract as
    /// [`Self::set_editor_font`].
    #[napi(js_name = "setUiFont")]
    pub fn set_ui_font(&mut self, env: Env, font_desc: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.set_ui_font(&font_desc);
            app_logic.host.ui_font = Some(font_desc);
            Ok(())
        })
    }

    /// Registers `bytes` (raw TTF/OTF data) with the platform font manager
    /// -- [`Backend::register_font_from_memory`]. Returns the family
    /// name(s) it registered under, or `None` on the headless `TuiBackend`
    /// or if `bytes` isn't a font this backend's platform font manager can
    /// parse. Re-registered by [`BindingHost::setup`].
    #[napi(js_name = "registerFontFromMemory")]
    pub fn register_font_from_memory(
        &mut self,
        env: Env,
        bytes: Vec<u8>,
    ) -> Result<Option<Vec<String>>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            let families = backend.register_font_from_memory(&bytes);
            app_logic.host.registered_fonts.push(bytes);
            Ok(families)
        })
    }

    /// Sets the fallback font family consulted when the primary font can't
    /// cover a character -- [`Backend::set_nerd_font_fallback`]. Same
    /// no-op-on-TUI-but-still-replayed contract as
    /// [`Self::set_editor_font`].
    #[napi(js_name = "setNerdFontFallback")]
    pub fn set_nerd_font_fallback(&mut self, env: Env, family: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.set_nerd_font_fallback(&family);
            app_logic.host.nerd_font_fallback = Some(family);
            Ok(())
        })
    }

    /// Whether `family` is already installed on this platform as a system
    /// font -- [`Backend::has_font_family`]. Always `None` on the headless
    /// `TuiBackend` -- see that method's own doc for why `None` means
    /// "don't know", never "false".
    #[napi(js_name = "hasFontFamily")]
    pub fn has_font_family(&mut self, env: Env, family: String) -> Result<Option<bool>> {
        self.check_not_live()?;
        self.guarded(env, |app| Ok(app.driver.backend().has_font_family(&family)))
    }

    /// Steers the hardware caret to `shape` (`"block"`, `"bar"`, or
    /// `"underline"`) -- [`Backend::set_caret_shape`]. On the headless
    /// `TuiBackend` this always writes a DECSCUSR escape sequence to real
    /// stdout regardless of whether a real terminal is attached -- harmless
    /// but not something a driver test can observe landing anywhere.
    #[napi(js_name = "setCaretShape")]
    pub fn set_caret_shape(&mut self, env: Env, shape: String) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        let shape = caret_shape(&shape)
            .map_err(|err| invalid_descriptor_error(format!("setCaretShape: {err}")))?;
        self.guarded(env, |app| {
            app.driver.parts_mut().0.set_caret_shape(shape);
            Ok(())
        })
    }
}

/// Maps a [`merge_overrides`] failure to this binding's own tagged-message
/// exception taxonomy, prefixed with `what` (the calling method's own
/// name) exactly once -- the napi-rs analog of `quadraui-py`'s own
/// `override_error_to_py`.
fn override_error_to_js(what: &str, err: OverrideError) -> Error {
    match err {
        OverrideError::Internal(_) => internal_error(format!("{what}: {err}")),
        OverrideError::UnknownField(_)
        | OverrideError::InvalidValue(_)
        | OverrideError::Invalid(_) => invalid_descriptor_error(format!("{what}: {err}")),
    }
}
