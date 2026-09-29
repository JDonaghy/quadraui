//! `Backend::register_font_from_memory` for GTK — process-local
//! application-font registration via Fontconfig (issue #1013).
//!
//! `GtkBackend` used to declare `app_font_registration: true` while
//! taking the trait's `register_font_from_memory` no-op default — a
//! lying capability: a consumer that checked the cap and called the
//! method got a silent no-op, with no signal that it needed to fall
//! back to something else. macOS (`super::super::macos::text`) and
//! Win-GUI (`super::super::win::text`) both already honour the real
//! contract via `CTFontManagerRegisterGraphicsFont`/
//! `IDWriteInMemoryFontFileLoader`; this module is GTK's counterpart,
//! built on Fontconfig's `FcConfigAppFontAddFile` instead.
//!
//! ## Why a temp file, unlike macOS/Win-GUI
//!
//! Fontconfig's public API has no in-memory registration entry point —
//! `FcConfigAppFontAddFile` only ever takes a path, scanning the file
//! itself (via FreeType) to build the `FcPattern`s it registers. This
//! module writes `bytes` to a fresh, process-private path under
//! [`std::env::temp_dir`] and hands *that* to Fontconfig — still nothing
//! like the system-wide `~/.local/share/fonts` + `fc-cache` installer
//! this issue's own "Consequence" section describes vimcode shipping:
//! no permanent write, no font-directory scan, no daemon, and the file
//! is registered against `FcSetApplication` (an in-memory set scoped to
//! this process's `FcConfig`), never fontconfig's on-disk cache.
//!
//! The temp file is deliberately *not* deleted once registration
//! returns: Fontconfig/FreeType may re-open it lazily the first time a
//! glyph from this font is actually shaped, which can be well after this
//! call returns. Deleting it early would surface as tofu glyphs at paint
//! time instead of a loud error here. It is cleaned up the same way any
//! other stray temp file is — the OS's own temp-directory housekeeping,
//! or process exit on platforms that scope `/tmp` per boot.
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Register `bytes` (raw TTF/OTF/TTC data) as a Fontconfig application
/// font for the lifetime of this process, and nudge Pango's Cairo/Fc font
/// map to notice immediately — matching
/// [`crate::Backend::register_font_from_memory`]'s contract.
///
/// Returns every family name Fontconfig's own FreeType scan resolved the
/// file to — there can be more than one, e.g. a TTC or a variable font
/// exposing named instances — read back from the `FcPattern`s Fontconfig
/// itself produced, never the caller's guess. Returns `None` if `bytes`
/// isn't a font Fontconfig's FreeType backend can parse, or if
/// registration itself fails (e.g. Fontconfig has no current config,
/// which in practice never happens once GTK has initialised).
pub(crate) fn register_font_from_memory(bytes: &[u8]) -> Option<Vec<String>> {
    let path = unique_temp_font_path();
    std::fs::write(&path, bytes).ok()?;
    let c_path = match path.to_str().and_then(|s| CString::new(s).ok()) {
        Some(c) => c,
        None => {
            let _ = std::fs::remove_file(&path);
            return None;
        }
    };

    // SAFETY: `FcConfigGetCurrent` takes no arguments and cannot fail —
    // it lazily creates and returns Fontconfig's default `FcConfig` on
    // first call, a pointer Fontconfig owns for the process's lifetime.
    // This module never frees it.
    let config = unsafe { fontconfig_sys::FcConfigGetCurrent() };
    if config.is_null() {
        let _ = std::fs::remove_file(&path);
        return None;
    }

    let before = app_font_count(config);
    // SAFETY: `config` is non-null (checked above); `c_path` is a valid,
    // NUL-terminated C string that outlives this call.
    let added = unsafe { fontconfig_sys::FcConfigAppFontAddFile(config, c_path.as_ptr().cast()) };
    if added == 0 {
        let _ = std::fs::remove_file(&path);
        return None;
    }

    let names = new_app_font_family_names(config, before);
    if names.is_empty() {
        // Fontconfig accepted the file (`added != 0`) but produced no
        // pattern this function could read a family back from — treat
        // it the same as a parse failure rather than handing the caller
        // a fabricated family it could pass straight to
        // `set_nerd_font_fallback` and never resolve.
        return None;
    }

    notify_fontmap_config_changed();
    Some(names)
}

/// A fresh path under [`std::env::temp_dir`], unique across every call in
/// this process (an `AtomicU64` counter) and across processes (the pid) —
/// same naming convention as this crate's other ad hoc temp-file tests
/// (see `src/desktop.rs`'s `move_to_trash` tests).
fn unique_temp_font_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "quadraui-1013-appfont-{}-{n}.font",
        std::process::id()
    ))
}

/// `config`'s current `FcSetApplication` font count, or `0` if Fontconfig
/// hasn't built that set yet (no application font has ever been added).
fn app_font_count(config: *mut fontconfig_sys::FcConfig) -> usize {
    // SAFETY: `config` is a live, non-null `FcConfig` (checked by every
    // caller before this is reached).
    let set = unsafe { fontconfig_sys::FcConfigGetFonts(config, fontconfig_sys::FcSetApplication) };
    if set.is_null() {
        return 0;
    }
    // SAFETY: `set` is a live `FcFontSet` Fontconfig owns for the
    // config's lifetime; `nfont` is a plain, always-initialised field.
    unsafe { (*set).nfont.max(0) as usize }
}

/// The family name(s) of every `FcPattern` Fontconfig appended to
/// `config`'s application font set since it held `before` entries —
/// i.e. exactly the pattern(s) the just-returned `FcConfigAppFontAddFile`
/// call added, assuming (true within this module — GTK's glib main loop
/// is single-threaded) no concurrent registration raced it.
fn new_app_font_family_names(config: *mut fontconfig_sys::FcConfig, before: usize) -> Vec<String> {
    // SAFETY: same live, non-null `FcConfig` guarantee as `app_font_count`.
    let set = unsafe { fontconfig_sys::FcConfigGetFonts(config, fontconfig_sys::FcSetApplication) };
    if set.is_null() {
        return Vec::new();
    }
    // SAFETY: `set` is a live `FcFontSet` Fontconfig owns for the
    // config's lifetime; `nfont`/`fonts` are plain, always-initialised
    // fields, and `fonts` is a valid array of `nfont` pattern pointers
    // per Fontconfig's own `FcFontSet` contract.
    let (nfont, fonts) = unsafe { ((*set).nfont.max(0) as usize, (*set).fonts) };
    if fonts.is_null() || nfont <= before {
        return Vec::new();
    }

    let mut names = Vec::with_capacity(nfont - before);
    for i in before..nfont {
        // SAFETY: `i < nfont`, within the array `fonts` points to.
        let pattern = unsafe { *fonts.add(i) };
        if !pattern.is_null() {
            if let Some(name) = pattern_family_name(pattern) {
                names.push(name);
            }
        }
    }
    names
}

/// Read `pattern`'s `FC_FAMILY` property (index 0 — the font's primary,
/// unlocalised family name), or `None` if the pattern carries none.
fn pattern_family_name(pattern: *mut fontconfig_sys::FcPattern) -> Option<String> {
    let mut out: *mut fontconfig_sys::FcChar8 = std::ptr::null_mut();
    // SAFETY: `pattern` is a live, non-null `FcPattern` Fontconfig itself
    // just produced (see `new_app_font_family_names`); `FC_FAMILY` is a
    // `'static` NUL-terminated constant; `&mut out` is a valid out-param
    // `FcPatternGetString` only ever writes through when it returns
    // `FcResultMatch`.
    let result = unsafe {
        fontconfig_sys::FcPatternGetString(
            pattern,
            fontconfig_sys::constants::FC_FAMILY.as_ptr(),
            0,
            &mut out,
        )
    };
    if result != fontconfig_sys::FcResultMatch || out.is_null() {
        return None;
    }
    // SAFETY: on `FcResultMatch`, Fontconfig guarantees `out` points at a
    // NUL-terminated string it owns for the pattern's lifetime (not this
    // function's) — copied out into an owned `String` before returning,
    // so nothing here outlives that ownership.
    let c = unsafe { CStr::from_ptr(out as *const c_char) };
    Some(c.to_string_lossy().into_owned())
}

/// Answer whether `family` is genuinely installed, via Fontconfig's own
/// match/substitution algorithm — [`crate::Backend::has_font_family`]'s
/// GTK implementation (issue #1024).
///
/// `FcFontMatch` never fails to produce *a* result: hand it an unknown
/// family and Fontconfig's default-substitution chain still resolves to
/// *something* (typically the configured default sans-serif), the same
/// "always substitutes, never fails" behaviour
/// `crate::macos::text::make_font_exact`'s doc describes for
/// `CTFontCreateWithName`. So a bare match can't distinguish "installed"
/// from "substituted" — this builds a pattern requesting exactly
/// `family`, runs it through `FcConfigSubstitute`/`FcDefaultSubstitute`/
/// `FcFontMatch` the same way any real Fontconfig client resolves a
/// font, and then compares the *matched* pattern's own `FC_FAMILY`
/// against what was asked (ASCII-case-insensitively — Fontconfig's own
/// family matching is case-insensitive), mirroring `make_font_exact`'s
/// "the resolved font's own name must equal the request" check.
pub(crate) fn has_font_family(family: &str) -> bool {
    let requested = family.trim();
    if requested.is_empty() {
        return false;
    }
    let Ok(c_family) = CString::new(requested) else {
        return false;
    };

    // SAFETY: same lazily-created, process-owned `FcConfig` as
    // `register_font_from_memory` above — never freed by this module.
    let config = unsafe { fontconfig_sys::FcConfigGetCurrent() };
    if config.is_null() {
        return false;
    }

    // SAFETY: `FcPatternCreate` returns either a freshly allocated,
    // owned pattern or null on allocation failure (checked below).
    let pattern = unsafe { fontconfig_sys::FcPatternCreate() };
    if pattern.is_null() {
        return false;
    }
    // SAFETY: `pattern` is the live, non-null, owned pattern just
    // created above; `FC_FAMILY` is a `'static` NUL-terminated constant;
    // `c_family` outlives this call. `FcConfigSubstitute`/
    // `FcDefaultSubstitute` both take the same live `pattern` and
    // `config` this function already validated non-null.
    unsafe {
        fontconfig_sys::FcPatternAddString(
            pattern,
            fontconfig_sys::constants::FC_FAMILY.as_ptr(),
            c_family.as_ptr().cast(),
        );
        fontconfig_sys::FcConfigSubstitute(config, pattern, fontconfig_sys::FcMatchPattern);
        fontconfig_sys::FcDefaultSubstitute(pattern);
    }

    let mut result: fontconfig_sys::FcResult = fontconfig_sys::FcResultNoMatch;
    // SAFETY: `config`/`pattern` are both live and non-null; `&mut
    // result` is a valid out-param `FcFontMatch` always writes through
    // before returning.
    let matched = unsafe { fontconfig_sys::FcFontMatch(config, pattern, &mut result) };
    // SAFETY: `pattern` was allocated by `FcPatternCreate` above;
    // `FcFontMatch` does not take ownership of its pattern argument, so
    // this function (which created it) is responsible for freeing it.
    unsafe { fontconfig_sys::FcPatternDestroy(pattern) };

    if matched.is_null() {
        return false;
    }
    let resolved = pattern_family_name(matched);
    // SAFETY: `matched` is the pattern `FcFontMatch` returned ownership
    // of to this caller (per Fontconfig's own `FcFontMatch` contract).
    unsafe { fontconfig_sys::FcPatternDestroy(matched) };

    resolved.is_some_and(|name| name.eq_ignore_ascii_case(requested))
}

/// Nudge Pango's default Cairo font map to notice the config change
/// `register_font_from_memory` just made, so a [`gtk4::pango::Layout`]
/// built after this call resolves the new family immediately rather
/// than whatever the font map already cached from the pre-registration
/// config — but only when that font map is actually Fc-backed.
///
/// `pango_cairo_font_map_get_default()` does not always return a
/// `PangoFcFontMap`: on a Homebrew/macOS GTK4 build (this crate's own
/// dev boxes included — this function was written against a real
/// segfault caught there), Cairo's own preferred font backend is Quartz,
/// not FreeType, so the default Pango-Cairo font map is a
/// `PangoCoreTextFontMap` instead. GTK4-on-Linux (the platform this
/// module's `ci.yml` leg actually runs on) is the opposite: X11/Wayland
/// Cairo is always FreeType/Fontconfig-backed, so the default font map
/// there genuinely is a `PangoFcFontMap`. Calling a `PangoFcFontMap`-only
/// function against a `PangoCoreTextFontMap` instance is exactly the
/// undefined behaviour a GObject cast is supposed to catch — but
/// `PANGO_FC_FONT_MAP`'s `G_TYPE_CHECK_INSTANCE_CAST` is a no-op in a
/// release GLib build (`G_DISABLE_CAST_CHECKS`), which is what Homebrew
/// ships, so the mismatched cast silently reinterprets a
/// `PangoCoreTextFontMap*` as a `PangoFcFontMap*` and reads garbage
/// memory as if it were Fc-specific fields — the segfault this function
/// exists to avoid. [`pango_cairo_font_map_get_font_type`] (a real
/// `pangocairo-sys` binding, unlike [`pango_fc_font_map_config_changed`]
/// below) answers "is it actually Fc-backed" safely, with no cast at
/// all, so this function only calls the Fc-specific notifier when the
/// answer is yes. On a non-Fc font map (this dev box; theoretically
/// GTK4-on-Windows too, depending on which font backend that build
/// prefers) the font *is* still registered with Fontconfig by the time
/// this is called — there's simply no Fc-specific fontmap cache to
/// invalidate on that platform, since nothing there consults Fontconfig
/// for glyph resolution in the first place.
///
/// `pango_fc_font_map_config_changed` has no binding in the `pango`/
/// `pangocairo` crates — it's declared on `PangoFcFontMap`
/// (`pango/pangofc-fontmap.h`), which — unlike `PangoFontMap`/
/// `PangoCairoFontMap` — isn't part of Pango's GObject-introspected
/// surface, so `pango-sys`/`pangocairo-sys` never generate a binding for
/// it. Declared by hand below, the same "the wrapper crate doesn't
/// expose this one function" rationale `src/macos/text.rs`'s own
/// hand-declared `extern "C"` block gives for `CTFontManagerRegisterGraphicsFont`.
fn notify_fontmap_config_changed() {
    // SAFETY: `pango_cairo_font_map_get_default()` returns a borrowed,
    // non-owned `PangoFontMap*` that GTK keeps alive for the process —
    // this function never frees it, only reads through it.
    // `pango_cairo_font_map_get_font_type` takes the same pointer,
    // retyped to `PangoCairoFontMap*` (the interface every
    // `pango_cairo_font_map_get_default()` result implements by
    // construction), which is a same-address pointer-marker reinterpret,
    // not a GObject instance cast — safe regardless of the map's
    // concrete backend class, unlike `pango_fc_font_map_config_changed`
    // below.
    let fontmap = unsafe { pangocairo::ffi::pango_cairo_font_map_get_default() };
    if fontmap.is_null() {
        return;
    }
    let font_type: pangocairo::cairo::FontType =
        unsafe { pangocairo::ffi::pango_cairo_font_map_get_font_type(fontmap.cast()) }.into();
    if font_type != pangocairo::cairo::FontType::FontTypeFt {
        return;
    }
    // SAFETY: `font_type == FontTypeFt` just confirmed `fontmap` is
    // genuinely backed by `CAIRO_FONT_TYPE_FT`, which on every GTK4
    // build quadraui has observed (Linux X11/Wayland, the real target of
    // this issue) means the concrete class really is `PangoFcFontMap` —
    // `pango_fc_font_map_config_changed`'s own `PANGO_FC_FONT_MAP` cast
    // is therefore sound, not just hoped-for.
    unsafe { pango_fc_font_map_config_changed(fontmap) };
}

extern "C" {
    // No `#[link(name = "...")]` needed here: unlike `libpango-1.0`
    // itself (already on the link line via `pango-sys`'s own build
    // script), this specific symbol lives in `libpangoft2-1.0` — a
    // separate shared library `pango-sys`/`pangocairo-sys` never probe
    // for, since `PangoFcFontMap` isn't part of either's
    // GIR-introspected surface. `../../build.rs`'s `pangoft2` probe
    // (`gtk` feature only) is what actually gets `-lpangoft2-1.0` and
    // its `-L` search path onto this crate's final link line; confirmed
    // against a real Homebrew build via `nm -gU` that the symbol is
    // absent from `libpango-1.0`/`libpangocairo-1.0` and present only in
    // `libpangoft2-1.0`.
    fn pango_fc_font_map_config_changed(fcfontmap: *mut pangocairo::pango::ffi::PangoFontMap);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes that are not a font at all — `register_font_from_memory`
    /// must report that as `None`, not a fabricated family, matching
    /// `MacBackend`/`WinBackend`'s own rejection tests for the same
    /// input shape (issue #1013).
    #[test]
    fn register_font_from_memory_rejects_bytes_that_are_not_a_font() {
        let garbage = [0u8; 64];
        assert!(
            register_font_from_memory(&garbage).is_none(),
            "64 zero bytes are not a parseable font — must report None, not a fabricated family"
        );
    }

    /// Empty input is a degenerate case of the same rejection — nothing
    /// for FreeType to scan, so nothing should come back.
    #[test]
    fn register_font_from_memory_rejects_empty_bytes() {
        assert!(
            register_font_from_memory(&[]).is_none(),
            "empty bytes are not a parseable font — must report None"
        );
    }

    /// Two temp paths generated back to back must differ — a collision
    /// would mean the second call's `std::fs::write` silently clobbers
    /// the first call's still-in-use registered file.
    #[test]
    fn unique_temp_font_path_does_not_collide_within_a_process() {
        let a = unique_temp_font_path();
        let b = unique_temp_font_path();
        assert_ne!(a, b, "consecutive calls must not reuse the same temp path");
    }

    /// A family name guaranteed not to exist should never resolve as
    /// "installed" — the whole point of comparing the matched pattern's
    /// own family back against the request, not just trusting that
    /// `FcFontMatch` returned something (issue #1024).
    #[test]
    fn has_font_family_is_false_for_a_name_nothing_is_installed_under() {
        assert!(!has_font_family(
            "Definitely Not A Real Font Family Quadraui 1024"
        ));
    }

    /// Empty/whitespace-only input is a degenerate case of the same
    /// rejection — there is no family to look up.
    #[test]
    fn has_font_family_is_false_for_empty_or_blank_input() {
        assert!(!has_font_family(""));
        assert!(!has_font_family("   "));
    }

    /// A family this same process just registered via
    /// `register_font_from_memory` must be reported as installed —
    /// otherwise `has_font_family` would be blind to exactly the
    /// app-bundled case `Backend::register_font_from_memory` already
    /// covers, undermining the "check register_font_from_memory first,
    /// then has_font_family" ordering this issue's `Backend` doc
    /// recommends. Manual/`#[ignore]`d like `manual_smoke_real_font`
    /// above: no font file ships in this repo.
    #[test]
    #[ignore]
    fn manual_smoke_has_font_family_sees_a_just_registered_app_font() {
        let path = std::env::var("QUADRAUI_SMOKE_FONT").expect("set QUADRAUI_SMOKE_FONT");
        let bytes = std::fs::read(&path).expect("read font");
        let names = register_font_from_memory(&bytes).expect("register_font_from_memory");
        let family = names.first().expect("at least one family name");
        assert!(
            has_font_family(family),
            "just-registered family {family:?} should be reported as installed"
        );
    }

    /// Manual, `#[ignore]`d-test-only round trip against a *real* font
    /// file — same pattern `src/macos/headless.rs` uses for its own
    /// visual-confirmation tools. The garbage-byte tests above only
    /// prove the rejection path; this is what actually proved the fix
    /// during development, including catching a real segfault (this
    /// module's `notify_fontmap_config_changed` doc explains the fix) on
    /// a Homebrew/macOS GTK4 build. No font file ships in this repo, so
    /// point `QUADRAUI_SMOKE_FONT` at any real `.ttf`/`.otf` on the
    /// host, e.g.:
    ///
    /// ```text
    /// QUADRAUI_SMOKE_FONT=/path/to/some/font.ttf \
    ///   cargo test --features gtk --lib gtk::app_font::tests::manual_smoke_real_font \
    ///   -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn manual_smoke_real_font() {
        let path = std::env::var("QUADRAUI_SMOKE_FONT").expect("set QUADRAUI_SMOKE_FONT");
        let bytes = std::fs::read(&path).expect("read font");
        let names = register_font_from_memory(&bytes).expect("register_font_from_memory");
        eprintln!("registered families: {names:?}");
        assert!(!names.is_empty());
    }
}
