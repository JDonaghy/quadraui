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
//! The temp file is deliberately *not* deleted once any backend has
//! actually registered against it: Fontconfig/FreeType may re-open it
//! lazily the first time a glyph from this font is actually shaped,
//! which can be well after this call returns, and on macOS Core Text
//! holds a registration against the same path for the rest of the
//! process's life once `CTFontManagerRegisterFontsForURL` itself
//! succeeds — even on the rarer path where reading a family name back
//! out afterwards fails. Deleting it early in either case would surface
//! as tofu glyphs at paint time instead of a loud error here.
//! [`register_font_from_memory`] only removes the file when neither
//! registration ever actually took — nothing holds a reference to it in
//! that case, so there's nothing to protect. Anything left in place is
//! cleaned up the same way any other stray temp file is — the OS's own
//! temp-directory housekeeping, or process exit on platforms that scope
//! `/tmp` per boot.
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Register `bytes` (raw TTF/OTF/TTC data) as an application font for the
/// lifetime of this process — with Fontconfig (`FcConfigAppFontAddFile`)
/// and, on macOS, also with Core Text directly — and nudge Pango's
/// default font map to notice immediately. Matches
/// [`crate::Backend::register_font_from_memory`]'s contract.
///
/// Returns every family name either registration resolved the font to —
/// Fontconfig's own FreeType scan can report more than one, e.g. for a
/// TTC or a variable font exposing named instances; Core Text's own
/// family-name read is folded in alongside them, deduplicated
/// case-insensitively (ASCII). On macOS the Core Text name is ordered
/// *first*: Pango's default font map there is a `PangoCoreTextFontMap`
/// (see [`notify_fontmap_config_changed`]'s doc), which never consults
/// Fontconfig, so a caller taking `names.first()` — as
/// [`crate::Backend::register_font_from_memory`]'s own doc recommends —
/// must get the name Pango on that platform can actually resolve.
///
/// Returns `None` if `bytes` isn't a font either backend's parser can
/// make sense of, or if every registration attempt itself fails. On
/// macOS this also means: if Core Text's own registration never
/// produced a usable family name, the whole call reports failure even
/// when Fontconfig's half succeeded — a Fontconfig-only family is one
/// `PangoCoreTextFontMap` will never resolve on that platform, so
/// reporting success there would reproduce the exact silent-tofu state
/// this function exists to prevent.
pub(crate) fn register_font_from_memory(bytes: &[u8]) -> Option<Vec<String>> {
    let path = create_temp_font_file(bytes)?;

    let names = register_with_fontconfig(&path);

    // Rebinding `names` (rather than mutating a shared `let mut`) keeps
    // this platform-neutral: on every other target `names` is exactly
    // what `register_with_fontconfig` produced and is never mutated, so
    // it has no business being `mut` there.
    #[cfg(target_os = "macos")]
    let names = {
        let registration = macos_core_text::register(&path);
        match registration.family {
            Some(name) => fold_in_core_text_name(names, name),
            None => {
                // Pango's default font map on macOS never consults
                // Fontconfig — a Fontconfig-only success here would hand
                // the caller a family that font map can never resolve.
                // `registration.registered` tells us whether Core Text
                // itself still holds a live registration against `path`
                // even though no family name could be read back; if so,
                // the file must stay in place for the same reason the
                // module doc gives for a successful registration.
                if !registration.registered {
                    let _ = std::fs::remove_file(&path);
                }
                return None;
            }
        }
    };

    if names.is_empty() {
        // Neither registration produced a family this function could
        // hand back — treat it the same as a parse failure rather than
        // fabricating one the caller could pass straight to
        // `set_nerd_font_fallback` and never resolve. Nothing registered
        // against `path`, so removing it is safe.
        let _ = std::fs::remove_file(&path);
        return None;
    }

    notify_fontmap_config_changed();
    Some(names)
}

/// Register [`crate::codicon::FONT_BYTES`] exactly once per process via
/// [`register_font_from_memory`] — every GUI backend's
/// built-in chrome glyphs (tree chevrons, tab dirty/close, the
/// context-menu submenu arrow) need this done unconditionally, with no
/// app opt-in, unlike an app's own Nerd-Font registration. `GtkBackend::new`
/// calls this once per instance; a `OnceLock` collapses repeated calls
/// (e.g. several `GtkBackend`s built across a test binary) to a single
/// real registration rather than leaking one Fontconfig app-font entry
/// and one temp file per call — see [`register_font_from_memory`]'s own
/// doc for why each call writes a fresh temp file.
///
/// Returns whether the font is available for painting in the current
/// process —
/// `false` only if registration itself failed (a corrupt bundled asset,
/// which `codicon::tests::font_bytes_is_a_real_sfnt_font` already guards
/// against, or an environment where neither Fontconfig nor Core Text
/// registration could write the temp file at all).
pub(crate) fn ensure_codicon_registered() -> bool {
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REGISTERED.get_or_init(|| register_font_from_memory(crate::codicon::FONT_BYTES).is_some())
}

/// Moves `name` to the front of `names`, removing any case-insensitive
/// match already present — see [`register_font_from_memory`]'s own doc
/// for why the Core Text name must lead on macOS.
#[cfg(target_os = "macos")]
fn fold_in_core_text_name(mut names: Vec<String>, name: String) -> Vec<String> {
    if let Some(pos) = names
        .iter()
        .position(|existing| existing.eq_ignore_ascii_case(&name))
    {
        names.remove(pos);
    }
    names.insert(0, name);
    names
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

/// Write `bytes` to a fresh [`unique_temp_font_path`] and return that
/// path — but refuse to write through anything already sitting at that
/// path, symlink included.
///
/// `std::fs::write` alone would happily follow a pre-existing symlink at
/// a predictable `$TMPDIR` path and overwrite whatever it points at —
/// this font-registration path previously only ran when an app
/// explicitly opted into `register_font_from_memory`, but every GUI
/// backend's own `::new()` now calls it unconditionally for the bundled
/// codicon font, so a local attacker on a shared `/tmp` no longer needs
/// any app cooperation to plant that symlink first. `OpenOptions::
/// create_new` is `O_EXCL` under the hood: it fails outright if anything
/// — file, symlink, or otherwise — already exists at the path, rather
/// than writing through it. The `pid`+per-process-counter name is
/// already unique in practice, so a second attempt on collision would
/// only ever retry against an adversarial pre-plant, not a legitimate
/// race with another `quadraui` process; this makes exactly one attempt
/// and reports failure rather than silently falling back to an
/// overwrite.
fn create_temp_font_file(bytes: &[u8]) -> Option<PathBuf> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let path = unique_temp_font_path();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .ok()?;
    file.write_all(bytes).ok()?;
    Some(path)
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
/// This function always calls `pango_font_map_changed` — a function on
/// Pango's own GIR-introspected `PangoFontMap` base-class surface that
/// never downcasts its argument, so it is sound regardless of which
/// concrete font-map class the default happens to be, and it is the
/// only one of the two notifiers here that reaches a
/// `PangoCoreTextFontMap` at all.
///
/// When [`pango_cairo_font_map_get_font_type`] additionally confirms the
/// default font map is genuinely Fc-backed (`CAIRO_FONT_TYPE_FT`), this
/// function also calls the Fc-specific `pango_fc_font_map_config_changed`
/// (hand-declared below — see that `extern "C"` block's own doc for why
/// it isn't part of the `pango`/`pangocairo` crates' generated bindings).
/// `pango_font_map_changed` only bumps the font map's serial number;
/// `pango_fc_font_map_config_changed` additionally clears
/// `PangoFcFontMap`'s own cached family list and pattern caches
/// (`pango_fc_font_map_cache_clear`), which the generic call does not
/// reach on that subclass. Calling both is strictly additive — the
/// `FontTypeFt` check below is what keeps the `PANGO_FC_FONT_MAP` cast
/// inside the Fc-specific call sound, since it only runs when the
/// concrete class really is `PangoFcFontMap`.
///
/// [`pango_cairo_font_map_get_font_type`]: pangocairo::ffi::pango_cairo_font_map_get_font_type
fn notify_fontmap_config_changed() {
    // SAFETY: `pango_cairo_font_map_get_default()` returns a borrowed,
    // non-owned `PangoFontMap*` that GTK keeps alive for the process —
    // this function never frees it, only reads through it and passes it
    // to the two Pango notifier functions below.
    let fontmap = unsafe { pangocairo::ffi::pango_cairo_font_map_get_default() };
    if fontmap.is_null() {
        return;
    }

    // SAFETY: `pango_font_map_changed` is part of Pango's own
    // GIR-introspected `PangoFontMap` base-class surface — it never
    // downcasts its argument, so it is sound against any concrete
    // subclass, including the `PangoCoreTextFontMap` GTK4-on-macOS
    // actually returns.
    unsafe { pangocairo::pango::ffi::pango_font_map_changed(fontmap) };

    // SAFETY: `fontmap` is the same live, non-null pointer validated
    // above; `pango_cairo_font_map_get_font_type` takes it retyped to
    // `PangoCairoFontMap*` (the interface every
    // `pango_cairo_font_map_get_default()` result implements by
    // construction) — a same-address pointer-marker reinterpret, not a
    // GObject instance cast, so it is safe regardless of the map's
    // concrete backend class.
    let font_type: pangocairo::cairo::FontType =
        unsafe { pangocairo::ffi::pango_cairo_font_map_get_font_type(fontmap.cast()) }.into();
    if font_type != pangocairo::cairo::FontType::FontTypeFt {
        return;
    }
    // SAFETY: `font_type == FontTypeFt` just confirmed `fontmap` is
    // genuinely backed by `CAIRO_FONT_TYPE_FT`, which on every GTK4
    // build quadraui has observed (Linux X11/Wayland, the real target of
    // this module's `ci.yml` leg) means the concrete class really is
    // `PangoFcFontMap` — `pango_fc_font_map_config_changed`'s own
    // `PANGO_FC_FONT_MAP` cast is therefore sound, not just hoped-for.
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

/// Core Text registration for the GTK path on macOS.
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

    /// The outcome of [`register`] — `registered` and `family` are
    /// tracked separately because they can disagree: Core Text can
    /// accept the registration itself while the follow-up family-name
    /// read-back still comes back empty, and a caller deciding whether
    /// the temp file backing this registration is still safe to delete
    /// needs to know which one actually happened.
    pub(super) struct Registration {
        /// Whether `CTFontManagerRegisterFontsForURL` itself succeeded.
        /// Once true, Core Text holds a live registration against the
        /// path passed to [`register`] for the rest of the process's
        /// life, regardless of whether `family` also came back `Some`.
        pub(super) registered: bool,
        /// The family name Core Text resolves the font to, read back
        /// from the registered URL — `None` if registration failed
        /// outright, or if it succeeded but no descriptor could be read
        /// back from it.
        pub(super) family: Option<String>,
    }

    /// Register the font at `path` with Core Text for the lifetime of
    /// this process via `CTFontManagerRegisterFontsForURL`, reading back
    /// the family name Core Text resolves the font to via
    /// `CTFontManagerCreateFontDescriptorsFromURL` — never the caller's
    /// guess. Registration itself can fail (e.g. a duplicate PostScript
    /// name already registered in this process — including, in
    /// practice, by `crate::macos::text::register_font_from_memory`
    /// itself, if a consumer somehow has both the `gtk` and `macos`
    /// features on and registers the same bytes through both paths).
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
    pub(super) fn register(path: &Path) -> Registration {
        let Some(url) = CFURL::from_path(path, false) else {
            return Registration {
                registered: false,
                family: None,
            };
        };

        // SAFETY: `url.as_concrete_TypeRef()` is a valid, live `CFURLRef`
        // for the duration of this call. Passing `null` for the error
        // out-param tells Core Text this caller has no use for the
        // `CFErrorRef` it would otherwise hand back — `registered`
        // already carries the only thing this function reports on
        // failure, and a non-null out-param would hand back a
        // `+1`-retained `CFErrorRef` this function would then have to
        // release itself.
        let registered = unsafe {
            CTFontManagerRegisterFontsForURL(
                url.as_concrete_TypeRef(),
                K_CT_FONT_MANAGER_SCOPE_PROCESS,
                std::ptr::null_mut(),
            )
        };
        if !registered {
            return Registration {
                registered: false,
                family: None,
            };
        }

        // SAFETY: `url.as_concrete_TypeRef()` is the same live `CFURLRef`
        // just registered above; `CTFontManagerCreateFontDescriptorsFromURL`
        // is a real `core-text` binding (unlike
        // `CTFontManagerRegisterFontsForURL` above), so no hand-rolled
        // signature risk here.
        let descriptors_ref =
            unsafe { CTFontManagerCreateFontDescriptorsFromURL(url.as_concrete_TypeRef()) };
        if descriptors_ref.is_null() {
            return Registration {
                registered: true,
                family: None,
            };
        }
        // SAFETY: a non-null return is a `+1`-retained `CFArrayRef` per
        // Core Foundation's create-rule, which `wrap_under_create_rule`
        // takes ownership of.
        let descriptors: CFArray<CTFontDescriptor> =
            unsafe { TCFType::wrap_under_create_rule(descriptors_ref) };
        let family = descriptors
            .get(0)
            .map(|descriptor| descriptor.family_name());
        Registration {
            registered: true,
            family,
        }
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
            error: *mut core_foundation::error::CFErrorRef,
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

    // ── GTK-on-macOS Core Text registration ─────────────────────────────

    /// The real icon font vimcode registers at startup — same fixture
    /// `crate::macos::text`'s own tests embed, not a CI-only stand-in.
    /// Loaded from `tests/fixtures/vimcode-icons.ttf`: vimcode's own
    /// bundled Nerd Font icon subset, used here purely as a real,
    /// parseable font with a real PUA codepoint — see that directory for
    /// licensing.
    const ICON_FONT_BYTES: &[u8] = include_bytes!("../../tests/fixtures/vimcode-icons.ttf");

    /// `register_font_from_memory(ICON_FONT_BYTES)`, cached: Core Text
    /// registration is process-global and rejects a second registration
    /// of the same PostScript name (see [`macos_core_text::register`]'s
    /// doc), and the default test runner executes every `#[test]` in
    /// this file concurrently, in one process. Every test below that
    /// needs the fixture registered calls this instead of
    /// `register_font_from_memory` directly, so the fixture is
    /// registered exactly once no matter how many tests want it, and
    /// none of them can observe the other's in-flight registration.
    fn registered_icon_font_names() -> &'static [String] {
        static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        NAMES.get_or_init(|| {
            register_font_from_memory(ICON_FONT_BYTES).expect(
                "vimcode-icons.ttf is a well-formed font every registration backend here \
                 should parse",
            )
        })
    }

    /// A real, parseable font must always register on every platform.
    /// On Linux this exercises the Fontconfig half of
    /// [`register_font_from_memory`] alone; on macOS it exercises both
    /// halves, proven separately by the test below.
    #[test]
    fn register_font_from_memory_registers_the_real_icon_font_fixture() {
        let names = registered_icon_font_names();
        assert!(
            !names.is_empty(),
            "a real font must resolve at least one family name"
        );
    }

    /// [`has_font_family`] must recognise a family this process just
    /// registered via [`register_font_from_memory`] — otherwise it would
    /// be blind to exactly the app-bundled case that function already
    /// covers, undermining the "check register_font_from_memory first,
    /// then has_font_family" ordering `Backend::register_font_from_memory`'s
    /// own doc recommends. On macOS this exercises
    /// [`macos_core_text::has_font_family`] specifically, since
    /// [`registered_icon_font_names`] orders the Core Text name first.
    #[test]
    fn has_font_family_sees_the_real_icon_font_fixture() {
        let names = registered_icon_font_names();
        let family = names.first().expect("at least one family name");
        assert!(
            has_font_family(family),
            "just-registered family {family:?} should be reported as installed"
        );
    }

    /// On a Homebrew/macOS GTK4 build, Pango's default font map is a
    /// `PangoCoreTextFontMap`, which never consults Fontconfig.
    /// Registering a font with `FcConfigAppFontAddFile` alone therefore
    /// leaves it invisible to Pango there, and every PUA glyph from it
    /// renders as tofu: Pango's font map has no knowledge of the family
    /// at all, so `font_map.load_font` either returns `None` or
    /// substitutes some unrelated installed family — exactly the
    /// "doesn't fail, just substitutes" trap
    /// `macos_core_text::has_font_family`'s doc describes for
    /// `CTFontCreateWithName`. This test asserts Pango's default font
    /// map genuinely resolves the registered family, which only holds if
    /// the Core Text half of [`register_font_from_memory`] actually ran.
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
    /// a Core Text/Pango font-matching property orthogonal to whether
    /// the font reaches Pango's font map at all — asking Core Text
    /// directly for the exact family this function just resolved, rather
    /// than asking Pango to re-resolve the name a second time, sidesteps
    /// it.
    #[cfg(target_os = "macos")]
    #[test]
    fn gtk_register_font_from_memory_registers_with_core_text_so_pango_resolves_pua_glyph() {
        use pangocairo::pango::prelude::*;

        let names = registered_icon_font_names();
        let family = names.first().expect("at least one family name").to_string();
        assert!(
            macos_core_text::has_font_family(&family),
            "the family this function returns first on macOS, {family:?}, must be the one \
             Core Text itself resolves — otherwise the assertions below would just be \
             re-testing Fontconfig's own name"
        );

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
        let all_mapped = unsafe {
            ctfont.get_glyphs_for_characters(characters.as_ptr(), glyphs.as_mut_ptr(), 1)
        };
        assert!(
            all_mapped && glyphs[0] != 0,
            "Core Text resolved family {family:?} but reports glyph 0 (.notdef) for U+EAF0 \
             (the Explorer icon) — the font registration didn't actually reach Core Text"
        );
    }
}
