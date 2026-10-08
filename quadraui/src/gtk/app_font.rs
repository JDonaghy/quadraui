//! `Backend::register_font_from_memory` for GTK — process-local
//! application-font registration via Fontconfig (issue #1013), plus Core
//! Text directly on macOS (issue #1367).
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
//! ## GTK4-on-macOS: Fontconfig alone is not enough (issue #1367)
//!
//! Fontconfig registration above is necessary but not sufficient on
//! macOS: a Homebrew GTK4 build's default Pango-Cairo font map is a
//! `PangoCoreTextFontMap` (see [`notify_fontmap_config_changed`]'s own
//! doc for how that's detected), which resolves every family name
//! straight through Core Text and never consults Fontconfig at all. A
//! font registered *only* via `FcConfigAppFontAddFile` is therefore
//! invisible to Pango on that build — every PUA glyph from it falls back
//! to tofu, which is exactly what real-screen capture on macOS showed.
//!
//! [`register_font_from_memory`] closes that gap by additionally
//! registering the same temp file with Core Text directly, via
//! [`macos_core_text::register`]. It deliberately does not call
//! `crate::macos::text::register_font_from_memory` — that module only
//! compiles when the separate `macos` feature is also on (AppKit's own
//! native backend), but vimcode's macOS GUI download ships `gtk` alone,
//! so this path must work with `macos` off. [`macos_core_text`] is this
//! module's own, much smaller copy of just the registration half; GTK's
//! own Pango font map does its own name-based fallback resolution, so
//! there is no need for `crate::macos::text`'s cascade-descriptor-pinning
//! machinery here.
//!
//! It also deliberately calls `CTFontManagerRegisterFontsForURL`, not
//! the `CTFontManagerRegisterGraphicsFont` call
//! `crate::macos::text::register_font_from_memory` uses — confirmed
//! against a real Homebrew GTK4 build, the two are not interchangeable
//! here. `CTFontManagerRegisterGraphicsFont` makes a font available for
//! direct construction by name (`CTFontCreateWithName`/Core Text's own
//! `make_font_exact`-style lookups), but Apple's own docs say it is
//! *not* added to the font collection enumeration APIs
//! (`CTFontCollectionCreateFromAvailableFonts` and friends) — and
//! `PangoCoreTextFontMap`'s own family matching, which is what
//! `pango_font_map_load_font` goes through on this build, resolves
//! through exactly that enumeration. A font registered only via
//! `CTFontManagerRegisterGraphicsFont` therefore resolves correctly by
//! name but is invisible to `PangoCoreTextFontMap`'s own family listing —
//! confirmed directly against a real Homebrew GTK4 build, not just
//! inferred from Apple's docs. `CTFontManagerRegisterFontsForURL`
//! genuinely adds the font to that collection, which is why this module
//! needs a filesystem path
//! (reusing the one [`register_with_fontconfig`] already wrote) rather
//! than registering from an in-memory buffer the way
//! `crate::macos::text::register_font_from_memory` does.
//!
//! ## Why a temp file, unlike macOS/Win-GUI
//!
//! Fontconfig's public API has no in-memory registration entry point —
//! `FcConfigAppFontAddFile` only ever takes a path, scanning the file
//! itself (via FreeType) to build the `FcPattern`s it registers. This
//! module writes `bytes` to a fresh, process-private path under
//! [`std::env::temp_dir`] and hands *that* to Fontconfig (and, on macOS,
//! to Core Text too) — still nothing like the system-wide
//! `~/.local/share/fonts` + `fc-cache` installer this issue's own
//! "Consequence" section describes vimcode shipping: no permanent write,
//! no font-directory scan, no daemon, and Fontconfig's side registers
//! against `FcSetApplication` (an in-memory set scoped to this process's
//! `FcConfig`), never fontconfig's on-disk cache.
//!
//! The temp file is deliberately *not* deleted once registration
//! returns: Fontconfig/FreeType may re-open it lazily the first time a
//! glyph from this font is actually shaped, which can be well after this
//! call returns, and on macOS Core Text holds a registration against the
//! same path for the rest of the process's life. Deleting it early would
//! surface as tofu glyphs at paint time instead of a loud error here. It
//! is cleaned up the same way any other stray temp file is — the OS's
//! own temp-directory housekeeping, or process exit on platforms that
//! scope `/tmp` per boot.
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Register `bytes` (raw TTF/OTF/TTC data) as an application font for the
/// lifetime of this process — with Fontconfig (`FcConfigAppFontAddFile`)
/// and, on macOS, also with Core Text directly (issue #1367) — and nudge
/// Pango's default font map to notice immediately. Matches
/// [`crate::Backend::register_font_from_memory`]'s contract.
///
/// Returns every family name either registration resolved the font to —
/// Fontconfig's own FreeType scan can report more than one, e.g. for a
/// TTC or a variable font exposing named instances; Core Text's own
/// family-name read is folded in alongside them, deduplicated
/// case-insensitively. Returns `None` if `bytes` isn't a font either
/// backend's parser can make sense of, or if every registration attempt
/// itself fails (e.g. Fontconfig has no current config, which in
/// practice never happens once GTK has initialised).
pub(crate) fn register_font_from_memory(bytes: &[u8]) -> Option<Vec<String>> {
    let path = unique_temp_font_path();
    if std::fs::write(&path, bytes).is_err() {
        // Neither registration below can proceed without this file on
        // disk — Core Text's half needs it just as much as Fontconfig's.
        return None;
    }

    let mut names = register_with_fontconfig(&path);

    #[cfg(target_os = "macos")]
    let core_text_name = macos_core_text::register(&path);
    #[cfg(not(target_os = "macos"))]
    let core_text_name: Option<String> = None;

    if let Some(name) = core_text_name {
        if !names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&name))
        {
            names.push(name);
        }
    }

    if names.is_empty() {
        // Neither registration produced a family this function could
        // hand back — treat it the same as a parse failure rather than
        // fabricating one the caller could pass straight to
        // `set_nerd_font_fallback` and never resolve. The temp file is
        // left in place regardless (see the module doc) — it's never
        // been registered against this process's `FcConfig` or Core
        // Text, so there's no lazy re-open to protect, but deleting it
        // here would gain nothing a failed registration doesn't already
        // make obvious.
        return None;
    }

    notify_fontmap_config_changed();
    Some(names)
}

/// The Fontconfig half of [`register_font_from_memory`] (issue #1013):
/// hands `path` to `FcConfigAppFontAddFile` — see the module doc for why
/// a temp file rather than any in-memory Fontconfig API. Returns every
/// family name Fontconfig's own FreeType scan resolved the file to, read
/// back from the `FcPattern`s Fontconfig itself produced, never the
/// caller's guess.
///
/// Returns an empty `Vec` (never bails the whole registration) on any
/// failure along the way — on macOS, [`register_font_from_memory`]'s
/// Core Text half can still succeed and resolve a real family even when
/// this one doesn't.
fn register_with_fontconfig(path: &Path) -> Vec<String> {
    let Some(c_path) = path.to_str().and_then(|s| CString::new(s).ok()) else {
        return Vec::new();
    };

    // SAFETY: `FcConfigGetCurrent` takes no arguments and cannot fail —
    // it lazily creates and returns Fontconfig's default `FcConfig` on
    // first call, a pointer Fontconfig owns for the process's lifetime.
    // This module never frees it.
    let config = unsafe { fontconfig_sys::FcConfigGetCurrent() };
    if config.is_null() {
        return Vec::new();
    }

    let before = app_font_count(config);
    // SAFETY: `config` is non-null (checked above); `c_path` is a valid,
    // NUL-terminated C string that outlives this call.
    let added = unsafe { fontconfig_sys::FcConfigAppFontAddFile(config, c_path.as_ptr().cast()) };
    if added == 0 {
        return Vec::new();
    }

    new_app_font_family_names(config, before)
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

/// Answer whether `family` is genuinely installed —
/// [`crate::Backend::has_font_family`]'s GTK implementation (issue
/// #1024), consulting Fontconfig via [`fontconfig_has_font_family`] and,
/// on macOS, also Core Text directly via
/// [`macos_core_text::has_font_family`] (issue #1367).
///
/// Fontconfig alone is not a complete answer on a Homebrew GTK4 build:
/// Pango's default font map there resolves names through Core Text, not
/// Fontconfig (see [`notify_fontmap_config_changed`]'s doc), so a family
/// Core Text would resolve but Fontconfig wouldn't — or vice versa, e.g.
/// immediately after [`register_font_from_memory`]'s Fontconfig half
/// failed but its Core Text half succeeded — must still answer `true`
/// here. Checking both and returning as soon as either says yes is a
/// superset, never a subset, of either backend's own answer, so this
/// cannot turn a real "installed" into a false `false`.
pub(crate) fn has_font_family(family: &str) -> bool {
    let requested = family.trim();
    if requested.is_empty() {
        return false;
    }

    if fontconfig_has_font_family(requested) {
        return true;
    }

    #[cfg(target_os = "macos")]
    if macos_core_text::has_font_family(requested) {
        return true;
    }

    false
}

/// The Fontconfig half of [`has_font_family`] (issue #1024), via
/// Fontconfig's own match/substitution algorithm.
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
///
/// `requested` is assumed already trimmed and non-empty — [`has_font_family`]
/// checks that once for both backends.
fn fontconfig_has_font_family(requested: &str) -> bool {
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
/// config.
///
/// `pango_cairo_font_map_get_default()` does not always return the same
/// concrete class: on a Homebrew/macOS GTK4 build (this crate's own dev
/// boxes included), Cairo's own preferred font backend is Quartz, not
/// FreeType, so the default Pango-Cairo font map is a
/// `PangoCoreTextFontMap`. GTK4-on-Linux (the platform this module's
/// `ci.yml` leg actually runs on) is the opposite: X11/Wayland Cairo is
/// always FreeType/Fontconfig-backed, so the default font map there
/// genuinely is a `PangoFcFontMap`.
///
/// This issue #1367 fix calls [`pango::prelude::FontMapExt::changed`]
/// (`pango_font_map_changed`) — a method on the base `PangoFontMap`
/// GObject class, not the Fontconfig-specific `PangoFcFontMap` subclass
/// — rather than the `PangoFcFontMap`-only `pango_fc_font_map_config_changed`
/// this function used before. `changed` is part of Pango's own
/// GIR-introspected surface (unlike `pango_fc_font_map_config_changed`,
/// which isn't), so the `pango` crate already binds it safely with no
/// GObject instance cast at all — meaning it is sound to call
/// unconditionally, regardless of which concrete font-map class the
/// default happens to be on this platform, and correctly notifies a
/// `PangoCoreTextFontMap` on macOS where the old Fc-only call was both
/// wrong (a silent no-op, since the `FontTypeFt` guard it carried never
/// matched there) and, had that guard not been present, unsound (a
/// mismatched `PANGO_FC_FONT_MAP` cast is UB whenever a release GLib
/// build — e.g. Homebrew's — disables its own cast-check assertions).
fn notify_fontmap_config_changed() {
    use pangocairo::pango::prelude::FontMapExt as _;
    pangocairo::FontMap::default().changed();
}

/// Core Text registration for the GTK path on macOS (issue #1367).
///
/// Deliberately *not* a call into `crate::macos::text`: that module only
/// compiles under the separate `macos` feature (AppKit's own native
/// backend), and the `gtk` feature's own macOS story — vimcode's macOS
/// GUI download — must work with `macos` off. This is this module's own,
/// much smaller copy of just the registration and family-lookup halves
/// of `crate::macos::text::register_font_from_memory`/`make_font_exact`;
/// it skips that module's cascade-descriptor-pinning machinery, which
/// exists only to disambiguate quadraui's own `CTFont` fallback cascade
/// — GTK never builds one of those, since Pango does its own name-based
/// font-family fallback resolution independently.
#[cfg(target_os = "macos")]
mod macos_core_text {
    use core_foundation::array::CFArray;
    use core_foundation::base::TCFType;
    use core_foundation::error::CFErrorRef;
    use core_foundation::url::{CFURLRef, CFURL};
    use core_text::font;
    use core_text::font_descriptor::CTFontDescriptor;
    use core_text::font_manager::CTFontManagerCreateFontDescriptorsFromURL;
    use std::path::Path;

    /// `kCTFontManagerScopeProcess` (`CoreText/CTFontManager.h`) — not
    /// bound by `core-text` 20.1 at all (its own `font_manager.rs` has no
    /// `CTFontManagerScope` type, since every function that would take
    /// one is commented out there too). Register for the lifetime of
    /// this process only, matching
    /// [`crate::Backend::register_font_from_memory`]'s contract — the
    /// same scope [`register`]'s sibling
    /// `crate::macos::text::register_font_from_memory` gets for free,
    /// since its own `CTFontManagerRegisterGraphicsFont` call has no
    /// scope parameter at all (every font it registers is already
    /// process-local).
    const K_CT_FONT_MANAGER_SCOPE_PROCESS: u32 = 1;

    /// Register the font at `path` with Core Text for the lifetime of
    /// this process via `CTFontManagerRegisterFontsForURL`, returning
    /// the family name Core Text resolves the font to — read back from
    /// the registered URL via `CTFontManagerCreateFontDescriptorsFromURL`,
    /// never the caller's guess. Returns `None` if `path` isn't a font
    /// Core Text's parser can make sense of, or registration itself
    /// fails (e.g. a duplicate PostScript name already registered in
    /// this process — including, in practice, by
    /// `crate::macos::text::register_font_from_memory` itself, if a
    /// consumer somehow has both the `gtk` and `macos` features on and
    /// registers the same bytes through both paths).
    ///
    /// Takes a filesystem path, not raw bytes — unlike
    /// `crate::macos::text::register_font_from_memory`'s
    /// `CTFontManagerRegisterGraphicsFont`. See this module's parent
    /// doc for why the two calls are not interchangeable here: only the
    /// URL-based call adds the font to the collection enumeration
    /// `PangoCoreTextFontMap`'s own family matching goes through. `path`
    /// is the same temp file [`super::register_with_fontconfig`] already
    /// wrote — see the module doc for why it's safe to register with two
    /// backends.
    pub(super) fn register(path: &Path) -> Option<String> {
        let url = CFURL::from_path(path, false)?;

        let mut error: CFErrorRef = std::ptr::null_mut();
        // SAFETY: `url.as_concrete_TypeRef()` is a valid, live `CFURLRef`
        // for the duration of this call; `error` is a valid out-param
        // Core Text only ever writes through, and its value is discarded
        // on failure — this function's `None` already tells the caller
        // registration failed.
        let registered = unsafe {
            CTFontManagerRegisterFontsForURL(
                url.as_concrete_TypeRef(),
                K_CT_FONT_MANAGER_SCOPE_PROCESS,
                &mut error,
            )
        };
        if !registered {
            return None;
        }

        // SAFETY: `url.as_concrete_TypeRef()` is the same live `CFURLRef`
        // just registered above; `CTFontManagerCreateFontDescriptorsFromURL`
        // is a real `core-text` binding (unlike
        // `CTFontManagerRegisterFontsForURL` above), so no hand-rolled
        // signature risk here.
        let descriptors_ref =
            unsafe { CTFontManagerCreateFontDescriptorsFromURL(url.as_concrete_TypeRef()) };
        if descriptors_ref.is_null() {
            return None;
        }
        // SAFETY: a non-null return is a `+1`-retained `CFArrayRef` per
        // Core Foundation's create-rule, which `wrap_under_create_rule`
        // takes ownership of.
        let descriptors: CFArray<CTFontDescriptor> =
            unsafe { TCFType::wrap_under_create_rule(descriptors_ref) };
        descriptors
            .get(0)
            .map(|descriptor| descriptor.family_name())
    }

    /// Answer whether `family` is genuinely installed, via Core Text's
    /// own name resolution — the Core Text half of [`super::has_font_family`]
    /// (issue #1367), mirroring [`crate::macos::text::make_font_exact`]'s
    /// "the resolved font's own name must equal the request" check:
    /// `CTFontCreateWithName` substitutes a default face rather than
    /// failing for an unknown family, so a bare success can't mean
    /// "installed" on its own.
    ///
    /// `family` is assumed already trimmed and non-empty — [`super::has_font_family`]
    /// checks that once for both backends.
    pub(super) fn has_font_family(family: &str) -> bool {
        let Ok(ctfont) = font::new_from_name(family, 12.0) else {
            return false;
        };
        ctfont.family_name().eq_ignore_ascii_case(family)
            || ctfont.postscript_name().eq_ignore_ascii_case(family)
    }

    #[link(name = "CoreText", kind = "framework")]
    extern "C" {
        // Not bound by `core-text` 20.1 at all — its own
        // `font_manager.rs` comments this declaration out
        // (`//pub fn CTFontManagerRegisterFontsForURL`).
        fn CTFontManagerRegisterFontsForURL(
            font_url: CFURLRef,
            scope: u32,
            error: *mut CFErrorRef,
        ) -> bool;
    }
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

    // ── issue #1367: GTK-on-macOS Core Text registration ────────────────

    /// The real icon font vimcode registers at startup — same fixture
    /// `crate::macos::text`'s own tests embed, not a CI-only stand-in.
    /// Provenance details live on that module's `ICON_FONT_BYTES`
    /// constant.
    const ICON_FONT_BYTES: &[u8] = include_bytes!("../../tests/fixtures/vimcode-icons.ttf");

    /// A real, parseable font must always register on every platform —
    /// runs everywhere (not `#[ignore]`d, unlike the manual smoke tests
    /// above which need an externally supplied path) because this fixture
    /// ships in the repo. On Linux this exercises the Fontconfig half of
    /// [`register_font_from_memory`] alone; on macOS it exercises both
    /// halves, proven separately by the test below.
    #[test]
    fn register_font_from_memory_registers_the_real_icon_font_fixture() {
        let names = register_font_from_memory(ICON_FONT_BYTES).expect(
            "vimcode-icons.ttf is a well-formed font every registration backend here should parse",
        );
        assert!(
            !names.is_empty(),
            "a real font must resolve at least one family name"
        );
    }

    /// Issue #1367's actual regression: on a Homebrew/macOS GTK4 build,
    /// Pango's default font map is a `PangoCoreTextFontMap`, which never
    /// consults Fontconfig — so registering the icon font with
    /// `FcConfigAppFontAddFile` alone (what this module did before this
    /// fix) leaves it invisible to Pango, and every PUA glyph from it
    /// renders as tofu. This fails on that old code: Pango's font map has
    /// no knowledge of the family at all, so `font_map.load_font` either
    /// returns `None` or substitutes some unrelated installed family —
    /// exactly the "doesn't fail, just substitutes" trap
    /// `macos_core_text::has_font_family`'s doc describes for
    /// `CTFontCreateWithName`.
    ///
    /// The glyph-coverage half of this check goes through Core Text
    /// directly (`get_glyphs_for_characters`) rather than Pango's own
    /// `pango_font_has_char`/layout shaping: confirmed against a real
    /// Homebrew GTK4 build that the latter answers unreliably whenever
    /// more than one *installed* font shares the exact same family
    /// string — e.g. a leftover real vimcode font install under
    /// `~/Library/Fonts` or `~/.local/share/fonts` on a dev box that has
    /// actually run vimcode before, racing this test's own ephemeral
    /// process-scoped registration for the same name. That ambiguity is
    /// a pre-existing Core Text/Pango font-matching property, orthogonal
    /// to what this issue is about (whether the font reaches Pango's font
    /// map at all) — asking Core Text directly for the exact family this
    /// function just resolved, rather than asking Pango to re-resolve the
    /// name a second time, sidesteps it.
    #[cfg(target_os = "macos")]
    #[test]
    fn gtk_register_font_from_memory_registers_with_core_text_so_pango_resolves_pua_glyph() {
        use pangocairo::pango::prelude::*;

        let names = register_font_from_memory(ICON_FONT_BYTES)
            .expect("vimcode-icons.ttf is a well-formed font Core Text can parse");
        let family = names.first().expect("at least one family name").to_string();

        let font_map = pangocairo::FontMap::default();
        let context = pangocairo::pango::Context::new();
        context.set_font_map(Some(&font_map));

        let mut desc = pangocairo::pango::FontDescription::new();
        desc.set_family(&family);
        desc.set_size(12 * pangocairo::pango::SCALE);

        let font = font_map.load_font(&context, &desc).unwrap_or_else(|| {
            panic!(
                "Pango's default font map returned no font at all for the just-registered \
                 family {family:?}"
            )
        });

        let resolved_family = font.describe().family().map(|f| f.to_string());
        assert_eq!(
            resolved_family.as_deref(),
            Some(family.as_str()),
            "Pango resolved family {resolved_family:?} instead of the just-registered \
             {family:?} — on macOS this means the font only reached Fontconfig, which \
             PangoCoreTextFontMap never consults"
        );

        let ctfont = core_text::font::new_from_name(&family, 12.0)
            .expect("Core Text must resolve the family it just registered");
        let characters: [u16; 1] = [0xEAF0];
        let mut glyphs: [u16; 1] = [0];
        // SAFETY: `characters`/`glyphs` are both live, 1-element arrays;
        // `1` is their shared length.
        unsafe {
            ctfont.get_glyphs_for_characters(characters.as_ptr(), glyphs.as_mut_ptr(), 1);
        }
        assert_ne!(
            glyphs[0], 0,
            "Core Text resolved family {family:?} but reports glyph 0 (.notdef) for U+EAF0 \
             (the Explorer icon) — the font registration didn't actually reach Core Text"
        );
    }
}
