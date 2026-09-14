//! Guards the `objc2-app-kit` feature list in `Cargo.toml` against the
//! imports `src/macos/` actually makes (#498 CI fix).
//!
//! ## Why this test exists
//!
//! `objc2-app-kit` gates **every generated class, enum and constant
//! behind a Cargo feature named after the header it came from**. Importing
//! `objc2_app_kit::NSCursor` without `features = ["NSCursor"]` is not a
//! warning or a runtime surprise — the item does not exist, and the crate
//! fails to compile with a bare `unresolved import`.
//!
//! That failure is invisible on every machine in this fleet. `lib.rs`
//! gates `mod macos` on `all(feature = "macos", target_os = "macos")`, so
//! `cargo check -p quadraui --features macos` on Linux compiles *none* of
//! `src/macos/` and exits 0 (see `.github/workflows/macos.yml`'s header,
//! which measures exactly that). The only place a missing feature shows up
//! is the `macOS` workflow's `macos-latest` runner — which, thanks to that
//! workflow's `paths:` filter, is a slow, once-per-macOS-touching-PR
//! signal. #498 burned a full merge-gate round trip on precisely this:
//! `set_cursor` landed with `use objc2_app_kit::NSCursor` and no
//! `"NSCursor"` feature, and nothing anywhere caught it until CI.
//!
//! This test closes that hole with a plain text check that runs on every
//! leg, on any OS, in milliseconds: every `objc2_app_kit::` symbol
//! `src/macos/` imports must be registered in [`REQUIRED_FEATURE`] below
//! with the feature that supplies it, and that feature must be enabled in
//! `Cargo.toml`.
//!
//! ## Why the map is explicit rather than derived
//!
//! A symbol's feature is **not** reliably its own name, nor even its own
//! prefix — the mapping is per-header, and objc2-app-kit's headers do not
//! line up with class names:
//!
//! - `NSApplicationActivationPolicy` lives in the **`NSRunningApplication`**
//!   feature, not `NSApplication`.
//! - `NSBackingStoreType` lives in **`NSGraphics`**, not `NSWindow`.
//! - `NSControlStateValueOn` / `Off` live in **`NSCell`**.
//!
//! A prefix heuristic would get all three wrong. So the map is written out
//! by hand, and adding an import to `src/macos/` fails this test until the
//! author looks the symbol up (`objc2-app-kit`'s `src/generated/mod.rs`
//! spells the `#[cfg(feature = "…")]` out next to every re-export) and
//! registers it — which is exactly the checklist step #498 skipped.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Every `objc2_app_kit::` symbol `src/macos/` may import, mapped to the
/// `objc2-app-kit` Cargo feature that supplies it. Verified against
/// `objc2-app-kit-0.3.2`'s `src/generated/mod.rs` re-export cfgs (the
/// version `Cargo.lock` resolves the manifest's `"0.3"` requirement to).
const REQUIRED_FEATURE: &[(&str, &str)] = &[
    // Issue #936 (`NSAlert`-backed `show_message_dialog`): all three live
    // in the `NSAlert` header, so unlike most of this list the feature
    // *is* the symbol prefix here. Calling `addButtonWithTitle:` on the
    // alert additionally needs "NSButton" + "NSControl" (on top of the
    // "NSResponder"/"NSView" this manifest already enabled) — that is a
    // cfg on the *method*, not on any imported symbol, so it can't be
    // expressed as a row here; quadraui/Cargo.toml comments it instead.
    ("NSAlert", "NSAlert"),
    // `NSAlertFirstButtonReturn`'s re-export cfg is
    // `all(feature = "NSAlert", feature = "NSApplication")` — the only
    // compound one in this list, and this map holds a single feature per
    // symbol. "NSApplication" is not lost: `src/macos/backend.rs` imports
    // `NSApplication` itself, so the row below guards it independently.
    ("NSAlertFirstButtonReturn", "NSAlert"),
    ("NSAlertStyle", "NSAlert"),
    // Issue #952 (`system_theme`) deliberately has **no row** for the
    // `NSAppearance` feature, even though `Cargo.toml` enables it and
    // `system_theme` depends on it. `NSApplication::effectiveAppearance`
    // and `NSAppearance::name` are generated as *inherent* methods gated
    // on `#[cfg(feature = "NSAppearance")]`, reached through the value
    // `effectiveAppearance` returns — so `src/macos/` never names an
    // `objc2_app_kit::NSAppearance*` symbol, and a row here would be
    // stale by `required_feature_map_has_no_unused_entries`'s definition.
    // Same shape as the "NSButton"/"NSControl" cfg-on-the-method case the
    // `NSAlert` comment above describes: inexpressible as a row, so
    // `quadraui/Cargo.toml` carries the justification instead. Do not
    // "tidy" `"NSAppearance"` out of the manifest — it is an
    // `unresolved method` build failure on macos-latest only.
    ("NSApplication", "NSApplication"),
    // Not `NSApplication` — see the module doc.
    ("NSApplicationActivationPolicy", "NSRunningApplication"),
    ("NSApplicationDelegate", "NSApplication"),
    // Issue #951 (`applicationShouldTerminate:` so Cmd-Q / the app-menu
    // Quit item run the app's `UiEvent::WindowClose` veto): lives in the
    // `NSApplication` header, so here the feature *is* the symbol prefix.
    ("NSApplicationTerminateReply", "NSApplication"),
    // Not `NSWindow` — see the module doc.
    ("NSBackingStoreType", "NSGraphics"),
    // Issue #956 (`shell.beep`): the bare `NSBeep()` C function is
    // declared in the `NSGraphics` header alongside `NSBackingStoreType`
    // above — there is no "NSBeep" feature. A prefix heuristic would get
    // this wrong in the same way it gets `NSBackingStoreType` wrong.
    ("NSBeep", "NSGraphics"),
    // Issue #952 (`system_theme`): `NSColor::controlAccentColor` — the
    // feature *is* the symbol's own name here, same shape as `NSAlert`.
    ("NSColor", "NSColor"),
    // #952 review follow-up (`mac_accent_color`): `colorUsingColorSpace:`'s
    // `NSColorSpace` argument type, used to defensively convert
    // `controlAccentColor` to sRGB before reading its RGB components —
    // the feature *is* the symbol's own name here, same shape as
    // `NSColor`/`NSAlert`.
    ("NSColorSpace", "NSColorSpace"),
    ("NSControlStateValueOff", "NSCell"),
    ("NSControlStateValueOn", "NSCell"),
    ("NSCursor", "NSCursor"),
    // Issue #834 (OS file drop): both live in the `NSDragging` header,
    // not `NSDraggingInfo`'s/`NSDragOperation`'s own names.
    ("NSDragOperation", "NSDragging"),
    ("NSDraggingInfo", "NSDragging"),
    ("NSEvent", "NSEvent"),
    // Issue #953 (tray click button detection, `NSEventMask` widening a
    // status-item button's `sendActionOn:` to include right-clicks):
    // both live in the `NSEvent` header, not features of their own.
    ("NSEventMask", "NSEvent"),
    ("NSEventModifierFlags", "NSEvent"),
    ("NSEventType", "NSEvent"),
    ("NSGraphicsContext", "NSGraphicsContext"),
    // Issue #953 (tray icon): `TrayService::set_icon` decodes into this
    // directly (`NSImage::initWithData`/`initWithContentsOfFile`) rather
    // than going through `super::image`'s `CGImage` path — the feature
    // is the symbol's own name, same shape as `NSColor`/`NSAlert`.
    ("NSImage", "NSImage"),
    ("NSMenu", "NSMenu"),
    ("NSMenuItem", "NSMenuItem"),
    ("NSOpenPanel", "NSOpenPanel"),
    // Issue #834 (OS file drop): lives in the `NSPasteboard` header.
    ("NSPasteboardTypeFileURL", "NSPasteboard"),
    ("NSSavePanel", "NSSavePanel"),
    // Issue #953 (tray icon): both live in their own same-named headers,
    // same shape as `NSImage`/`NSColor`/`NSAlert` above.
    // `NSVariableStatusItemLength` (the sentinel `statusItemWithLength:`
    // takes) lives in the `NSStatusBar` header, not a feature of its own
    // — same shape as `NSFloatingWindowLevel`/`NSNormalWindowLevel`
    // living in `NSWindow` above.
    ("NSStatusBar", "NSStatusBar"),
    ("NSStatusItem", "NSStatusItem"),
    ("NSVariableStatusItemLength", "NSStatusBar"),
    ("NSView", "NSView"),
    ("NSViewFrameDidChangeNotification", "NSView"),
    ("NSWindow", "NSWindow"),
    // Issue #947 (client-side titlebar): the traffic-light identifiers
    // handed to `NSWindow::standardWindowButton:` when measuring the
    // inset AppKit reserves at the leading edge of a
    // `FullSizeContentView` window. Lives in the `NSWindow` header, not a
    // "NSWindowButton" feature of its own — there is no such feature.
    // `standardWindowButton:` itself is additionally cfg'd
    // `all(feature = "NSButton", feature = "NSControl", feature = "NSView")`
    // because it hands back an `NSButton`; like `NSAlert`'s
    // `addButtonWithTitle:` above that is a cfg on the *method*, which
    // this per-symbol map can't express, so quadraui/Cargo.toml comments
    // it instead. All three were already enabled for #936.
    ("NSWindowButton", "NSWindow"),
    // Issue #951 (`windowShouldClose:` so the red traffic light / Cmd-W
    // run the app's `UiEvent::WindowClose` veto): the delegate protocol
    // is declared in the `NSWindow` header, not a "NSWindowDelegate"
    // feature of its own — same shape as `NSWindowButton` above.
    ("NSWindowDelegate", "NSWindow"),
    // Issue #834 (HiDPI runtime change): lives in the `NSWindow` header,
    // same as `NSWindowStyleMask` above.
    ("NSWindowDidChangeBackingPropertiesNotification", "NSWindow"),
    ("NSWindowStyleMask", "NSWindow"),
    // Issue #950 (window control, `set_always_on_top`): both level
    // constants live in the `NSWindow` header, not a feature of their
    // own — same shape as `NSWindowButton`/`NSWindowStyleMask` above.
    ("NSFloatingWindowLevel", "NSWindow"),
    ("NSNormalWindowLevel", "NSWindow"),
    // Issue #947 (client-side titlebar): `setTitleVisibility:` hides
    // AppKit's own title string so the app-drawn band doesn't
    // double-draw it. `NSWindow` header again, same as the style mask.
    ("NSWindowTitleVisibility", "NSWindow"),
    // Issue #952 (`system_theme`): `NSWorkspace::accessibilityDisplayShouldIncreaseContrast`
    // — the feature *is* the symbol's own name here, same shape as `NSColor`/`NSAlert`.
    ("NSWorkspace", "NSWorkspace"),
];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `src/macos/`, recursively.
fn macos_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {dir:?}: {e}"));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&manifest_dir().join("src/macos"), &mut out);
    out.sort();
    assert!(
        !out.is_empty(),
        "expected .rs files under quadraui/src/macos — did the module move?"
    );
    out
}

/// `objc2_app_kit::Foo` / `objc2_app_kit::{Foo, Bar}` symbols referenced by
/// `src`. Comment lines are skipped so the module docs that *name* these
/// types (there are many) don't register as imports.
fn referenced_symbols(src: &str) -> BTreeSet<String> {
    const PATH: &str = "objc2_app_kit::";
    let mut found = BTreeSet::new();
    // Track each line's byte offset in `src` so a `use objc2_app_kit::{`
    // brace group that spans lines can be read past the line end (matching
    // on the line alone would truncate the group).
    let mut line_start = 0usize;
    for line in src.lines() {
        let this_line_start = line_start;
        line_start += line.len() + 1; // +1 for the '\n' `lines()` stripped
        if line.trim_start().starts_with("//") {
            continue;
        }
        let mut cursor = 0usize;
        while let Some(offset) = line[cursor..].find(PATH) {
            let after = cursor + offset + PATH.len();
            cursor = after;
            let rest = &src[this_line_start + after..];
            if let Some(group) = rest.strip_prefix('{') {
                let end = group
                    .find('}')
                    .expect("unterminated objc2_app_kit use-group");
                for item in group[..end].split(',') {
                    let item = item.trim();
                    if !item.is_empty() {
                        found.insert(item.to_string());
                    }
                }
            } else {
                let ident: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !ident.is_empty() {
                    found.insert(ident);
                }
            }
        }
    }
    found
}

/// Feature strings enabled on the `objc2-app-kit` dependency in
/// `Cargo.toml`. `#` comments are stripped first — the manifest's comments
/// contain double-quoted text (e.g. `target_os = "macos"`) that would
/// otherwise read as feature names.
fn enabled_app_kit_features() -> BTreeSet<String> {
    let manifest = fs::read_to_string(manifest_dir().join("Cargo.toml")).expect("read Cargo.toml");
    let start = manifest
        .find("objc2-app-kit = {")
        .expect("objc2-app-kit dependency line");
    let rest = &manifest[start..];
    let end = rest
        .find("] }")
        .expect("objc2-app-kit features array close")
        + 1;
    let decl = &rest[..end];

    let mut features = BTreeSet::new();
    for line in decl.lines() {
        let code = line.split('#').next().unwrap_or("");
        for chunk in code.split('"').skip(1).step_by(2) {
            features.insert(chunk.to_string());
        }
    }
    // The `features = [` array is the only quoted content left after the
    // comment strip, apart from the version requirement — drop that by
    // shape (leading digit) rather than by literal, so bumping the
    // dependency doesn't silently leave it in the set. It used to be
    // spelled `remove("0.2")`, which stopped matching when the manifest
    // moved to `"0.3"`; harmless, but only because no feature is ever
    // named after a version.
    features.retain(|f| !f.starts_with(|c: char| c.is_ascii_digit()));
    features
}

#[test]
fn every_app_kit_import_has_its_cargo_feature_enabled() {
    let enabled = enabled_app_kit_features();
    let mut failures: Vec<String> = Vec::new();

    for path in macos_sources() {
        let src = fs::read_to_string(&path).expect("read source");
        let rel = path
            .strip_prefix(manifest_dir())
            .unwrap_or(&path)
            .display()
            .to_string();
        for symbol in referenced_symbols(&src) {
            let Some((_, feature)) = REQUIRED_FEATURE.iter().find(|(s, _)| *s == symbol) else {
                failures.push(format!(
                    "{rel}: `objc2_app_kit::{symbol}` is not registered in \
                     tests/macos_appkit_features.rs::REQUIRED_FEATURE — look up its \
                     `#[cfg(feature = \"…\")]` in objc2-app-kit's src/generated/mod.rs \
                     and add it (the feature is NOT reliably the symbol's own name)"
                ));
                continue;
            };
            if !enabled.contains(*feature) {
                failures.push(format!(
                    "{rel}: `objc2_app_kit::{symbol}` needs `features = [\"{feature}\"]` on \
                     the objc2-app-kit dependency in quadraui/Cargo.toml, which is not \
                     enabled — this compiles nowhere but macos-latest, where it is an \
                     `unresolved import` build failure"
                ));
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The map must not rot in the other direction either: a stale entry for a
/// symbol nothing imports any more is a claim about the manifest nobody is
/// checking. (Not fatal to the build — hence a separate test from the one
/// above, which is the actual CI-failure guard.)
#[test]
fn required_feature_map_has_no_unused_entries() {
    let mut referenced = BTreeSet::new();
    for path in macos_sources() {
        let src = fs::read_to_string(&path).expect("read source");
        referenced.extend(referenced_symbols(&src));
    }

    let stale: Vec<&str> = REQUIRED_FEATURE
        .iter()
        .map(|(s, _)| *s)
        .filter(|s| !referenced.contains(*s))
        .collect();

    assert!(
        stale.is_empty(),
        "REQUIRED_FEATURE lists symbols src/macos no longer imports: {stale:?} — \
         drop them (and re-check whether Cargo.toml still needs their features)"
    );
}
