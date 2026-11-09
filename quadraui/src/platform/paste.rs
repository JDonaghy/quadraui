// ─────────────────────────────────────────────────────────────────────
// Paste-keypress predicate
// ─────────────────────────────────────────────────────────────────────

/// Which single platform modifier a backend's clipboard-paste chord
/// treats as native — Ctrl on Linux/GTK and Windows, Cmd (⌘) on macOS.
/// [`crate::Modifiers::cmd`] is the crate-wide abstraction for "this
/// platform's OS-level modifier key" — but that shared abstraction means
/// `modifiers.cmd` can be held on *any* backend (Super on Linux, the
/// Windows key on Win32), not only on macOS. [`is_paste_keypress`] takes
/// this enum so each backend tells the predicate which one of `ctrl`/`cmd`
/// is *its* paste modifier, instead of the predicate accepting either
/// interchangeably — see D-011 in `docs/decisions/DECISIONS.md` for the
/// regression this fixes (plain Super+V used to trigger paste on GTK, and
/// plain Ctrl+V used to trigger paste on macOS, neither of which is that
/// platform's actual convention).
///
/// `#[allow(dead_code)]`: each variant is constructed in production code
/// by exactly one backend (`Ctrl` by TUI/GTK/Windows via
/// [`crate::runtime::preprocess_event`], `Cmd` by macOS via the same
/// function's `PreprocessBackend::paste_modifier` override), and this
/// module compiles under `any(tui, gtk, macos, win)` — the same
/// single-backend feature-gate shape `win::run::dispatch_event`'s own
/// `#[allow(dead_code)]` documents. Building with only one of those
/// features active (e.g. the `ubuntu-latest` `--features win` leg, which
/// has no `tui`/`gtk`/`macos`) so only one variant is ever constructed
/// outside `#[cfg(test)]` would otherwise trip `-D warnings`' dead-code
/// lint on the other variant — despite both being genuinely constructed
/// on the CI legs where their respective backend feature is active, and
/// both being exercised unconditionally by `is_paste_keypress_tests`
/// below, which is what actually proves out D-011 §4's contract for
/// every backend on any host.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum PasteModifier {
    /// Ctrl-V / Ctrl-Shift-V — TUI, GTK (Linux), and Win32.
    Ctrl,
    /// Cmd-V / Cmd-Shift-V (⌘V / ⌘⇧V) — macOS.
    Cmd,
}

/// Is `key`+`modifiers` this platform's clipboard-paste shortcut?
///
/// One predicate for every desktop backend; `native` names which
/// platform modifier the *calling* backend treats as "paste" — GTK and
/// `win` both pass [`PasteModifier::Ctrl`], macOS passes
/// [`PasteModifier::Cmd`]. Only that modifier is accepted: the *other*
/// one must be explicitly absent, not merely ignored, so e.g. plain
/// Super+V (GTK, where `modifiers.cmd` is Super) and plain Ctrl+V
/// (macOS) are both rejected exactly as they were before this predicate
/// was lifted out of `gtk::run`/inlined in `macos::run` — see D-011 §4
/// in `docs/decisions/DECISIONS.md` for why "either modifier alone" was wrong.
/// `alt` is never a valid paste chord on any backend.
///
/// `shift` is deliberately **not** checked either way — Ctrl-Shift-V and
/// (by the same reasoning, extended here) Cmd-Shift-V both trigger paste
/// too. This extends the shift-tolerant contract to every adopter —
/// including macOS, which used to require `shift: false` in its inline
/// Cmd-V match guard before this predicate replaced it. See D-011 in
/// `docs/decisions/DECISIONS.md` for why this settles the contract once
/// instead of letting each new adopter silently pick its own.
///
/// Case-insensitive on the letter (`'v'` or `'V'`) on every backend.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
pub(crate) fn is_paste_keypress(
    key: &crate::Key,
    modifiers: &crate::Modifiers,
    native: PasteModifier,
) -> bool {
    let (native_held, other_held) = match native {
        PasteModifier::Ctrl => (modifiers.ctrl, modifiers.cmd),
        PasteModifier::Cmd => (modifiers.cmd, modifiers.ctrl),
    };
    matches!(key, crate::Key::Char('v') | crate::Key::Char('V'))
        && native_held
        && !other_held
        && !modifiers.alt
}

#[cfg(all(
    test,
    any(
        feature = "tui",
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
mod is_paste_keypress_tests {
    //! Coverage for [`is_paste_keypress`] — the single predicate
    //! `tui::run`, `gtk::run`, `macos::run`, and `win::run`'s
    //! `dispatch_event`s all now call (via
    //! `crate::runtime::preprocess_event`), each backend supplying its
    //! own [`PasteModifier`]. Pure/display-
    //! free: no live backend needed to exercise every branch of the
    //! contract D-011 (`docs/decisions/DECISIONS.md`) records.
    use super::*;
    use crate::{Key, Modifiers};

    fn mods(ctrl: bool, shift: bool, alt: bool, cmd: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            cmd,
        }
    }

    #[test]
    fn plain_ctrl_v_is_a_paste_keypress_for_ctrl_native_backends() {
        // GTK (Linux) and Win32.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, false),
            PasteModifier::Ctrl
        ));
        assert!(is_paste_keypress(
            &Key::Char('V'),
            &mods(true, false, false, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn plain_cmd_v_is_a_paste_keypress_for_cmd_native_backends() {
        // macOS — `cmd` is the crate-wide abstraction for the platform
        // OS-modifier key (⌘ on macOS).
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(false, false, false, true),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn plain_cmd_v_is_not_a_paste_keypress_for_ctrl_native_backends() {
        // D-011 §4 regression check: on GTK, `modifiers.cmd` is Super
        // (SUPER_MASK/META_MASK — see `gtk::events`), and on `win` it's
        // the Windows key. Plain Super+V / Win+V must NOT trigger paste
        // just because *some* backend's native modifier is held — GTK's
        // pre-lift `is_paste_keypress` never accepted it, and adopting
        // the shared predicate must not silently start accepting it.
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(false, false, false, true),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn plain_ctrl_v_is_not_a_paste_keypress_for_cmd_native_backends() {
        // D-011 §4 regression check: macOS's pre-lift inline Cmd-V guard
        // required `ctrl: false` exactly, so plain Ctrl-V never triggered
        // paste on macOS. The shared predicate must preserve that.
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, false),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn ctrl_shift_v_is_a_paste_keypress_for_ctrl_native_backends() {
        // Some terminal emulators reserve plain Ctrl-V for
        // a control byte and use Ctrl-Shift-V for paste instead.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(true, true, false, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn cmd_shift_v_is_a_paste_keypress_for_cmd_native_backends() {
        // D-011: shift tolerance: Cmd-Shift-V also triggers paste on macOS.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(false, true, false, true),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn ctrl_alt_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, true, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn cmd_alt_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(false, false, true, true),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn both_ctrl_and_cmd_is_not_a_paste_keypress() {
        // The other platform modifier must be explicitly absent, not
        // merely not-required — holding both is rejected regardless of
        // which one is native to the backend.
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, true),
            PasteModifier::Ctrl
        ));
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, true),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn shift_v_alone_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(false, true, false, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn plain_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &Modifiers::default(),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn ctrl_c_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('c'),
            &mods(true, false, false, false),
            PasteModifier::Ctrl
        ));
    }
}
