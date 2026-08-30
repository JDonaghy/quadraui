//! Pure decoding of Win32 message payloads — `WPARAM`/`LPARAM` word
//! unpacking and the DPI ratio, with no WinAPI calls in sight.
//!
//! # Why this is its own module
//!
//! Everything else under `src/win/` either calls into Direct2D/Win32 or is
//! a `todo!()` stub, so it is only *type*-checked on Linux (`ci.yml`'s
//! "Compile check (win feature)" step) and only really compiled on the
//! `windows-latest` leg. That leaves the one part of the bootstrap that is
//! plain arithmetic — "which half of `lparam` is the width", "what does
//! `wparam` mean in DPI terms" — with no coverage at all, on either leg.
//!
//! Those are exactly the bits that get silently transposed. `WM_SIZE` packs
//! *width* in the low word and *height* in the high word; swapping them
//! produces a window that renders at its own transposed size and looks
//! plausible until it isn't square. `WM_DPICHANGED` packs a DPI (a number
//! like 144), not a scale (a number like 1.5), and forgetting the divide
//! yields a 144x viewport scale. Neither mistake is a compile error on any
//! platform, and neither is visible to `cargo check`.
//!
//! So the arithmetic lives here, host-independent and unit-tested, and the
//! `cfg(target_os = "windows")` code in [`super::run`] / [`super::backend`]
//! calls it rather than open-coding the shifts inline. The tests below run
//! anywhere `--features win` is enabled, including this repo's Linux CI.
//!
//! Everything here is `#[cfg_attr(not(target_os = "windows"), allow(dead_code))]`:
//! the callers are all Windows-gated, so off Windows these functions are
//! genuinely unreachable outside the test module, and `-D warnings` would
//! otherwise reject the file it is here to make testable.

/// The 96-DPI baseline every Win32 DPI API measures against
/// (`GetDpiForWindow`, `WM_DPICHANGED`'s `wparam`). 96 DPI is 100% scale.
pub(crate) const USER_DEFAULT_SCREEN_DPI: f32 = 96.0;

/// Decode `WM_SIZE`'s `lparam` into the new client area's
/// `(width, height)` in device pixels.
///
/// Win32 packs both into one `LPARAM`: width in the low word, height in
/// the high word (`LOWORD`/`HIWORD`). The cast to `u32` before masking is
/// load-bearing on 64-bit — `LPARAM` is a signed `isize`, so masking the
/// raw value would sign-extend rather than truncate.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn size_from_lparam(lparam: isize) -> (u32, u32) {
    let packed = lparam as u32;
    (packed & 0xFFFF, (packed >> 16) & 0xFFFF)
}

/// Decode `WM_DPICHANGED`'s `wparam` into the ratio
/// [`Viewport::scale`][crate::event::Viewport] carries.
///
/// The low word is the new DPI; the high word is the same value (Windows
/// documents X and Y DPI as always equal), so only the low word is read.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn dpi_scale_from_wparam(wparam: usize) -> f32 {
    dpi_ratio((wparam & 0xFFFF) as u32)
}

/// `dpi / 96.0` — the ratio `Viewport::scale` carries for this backend
/// (issue #19's "DPI scale factor plumbed to `Viewport::scale`"
/// acceptance criterion).
///
/// `dpi == 0` maps to 100% rather than dividing by zero. In practice that
/// only happens for an invalid `HWND` passed to `GetDpiForWindow`, which
/// the callers here can't produce (they always pass a `HWND` they just
/// created or just received a message for) — but "the whole UI renders at
/// scale `inf`" is a bad failure mode to leave one bad handle away.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn dpi_ratio(dpi: u32) -> f32 {
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / USER_DEFAULT_SCREEN_DPI
    }
}

/// Decode a mouse message's `lparam` (`WM_MOUSEMOVE`, `WM_LBUTTONDOWN/UP`,
/// `WM_RBUTTONDOWN/UP`, …) into client-area `(x, y)` device pixels.
///
/// Win32 packs both into one `LPARAM` the same way `WM_SIZE` does — `x` in
/// the low word, `y` in the high word — but unlike `WM_SIZE`'s always-
/// non-negative client size, mouse coordinates go negative during a
/// mouse-captured drag that tracks outside the client rect. Each word is
/// therefore read as a **signed** 16-bit value (`i16`, matching the
/// `GET_X_LPARAM`/`GET_Y_LPARAM` macros' `(short)` casts), not masked and
/// widened the way [`size_from_lparam`] treats its always-unsigned words.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn point_from_lparam(lparam: isize) -> (i16, i16) {
    let packed = lparam as u32;
    ((packed & 0xFFFF) as i16, ((packed >> 16) & 0xFFFF) as i16)
}

/// Decode `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL`'s `wparam` into the signed wheel
/// delta (the `GET_WHEEL_DELTA_WPARAM` macro's `(short)HIWORD(wParam)`).
/// Positive is forward/right, negative is backward/left — one full
/// detent is `WHEEL_DELTA` (120); high-resolution wheels/trackpads may
/// report smaller fractional steps.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn wheel_delta_from_wparam(wparam: usize) -> i16 {
    ((wparam as u32 >> 16) & 0xFFFF) as i16
}

/// `WHEEL_DELTA` — one full wheel-notch's worth of [`wheel_delta_from_wparam`].
/// Dividing by this normalises a raw delta to "notches" (quadraui's
/// `ScrollDelta` unit), matching the TUI backend's crossterm translator
/// (one `ScrollUp`/`ScrollDown` event = 1.0) and the GTK/macOS backends'
/// small-magnitude deltas.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) const WHEEL_DELTA: f32 = 120.0;

/// Decode `WM_KEYDOWN`/`WM_KEYUP`'s `lparam` bit 30 (the "previous key
/// state" flag) into whether this is an OS-generated auto-repeat rather
/// than the key's first press. Per `WM_KEYDOWN`'s documented layout, bit
/// 30 is `1` if the key was already down before this message.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn is_repeat_from_lparam(lparam: isize) -> bool {
    (lparam as u32) & (1 << 30) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `LOWORD` is width, `HIWORD` is height — not the other way round.
    /// Deliberately non-square so a transposition can't pass.
    #[test]
    fn size_from_lparam_reads_width_from_the_low_word() {
        // 1280 x 720: 0x02D0 wide, 0x02D0 high would be square, so use the
        // real 16:9 pair — 1280 = 0x0500, 720 = 0x02D0.
        let lparam = (720isize << 16) | 1280isize;
        assert_eq!(size_from_lparam(lparam), (1280, 720));
    }

    /// A maximised window on a 4K monitor still fits in 16 bits, but the
    /// high word's top bit being set makes `LPARAM`'s signedness visible:
    /// height 34000 sets bit 31 of the packed value, so a `& 0xFFFF` on the
    /// raw `isize` (without the `as u32` truncation first) would sign-extend
    /// and read back garbage.
    #[test]
    fn size_from_lparam_truncates_rather_than_sign_extending() {
        let packed = ((34_000u32 << 16) | 3840u32) as i32 as isize;
        assert!(packed < 0, "test setup: expected the sign bit to be set");
        assert_eq!(size_from_lparam(packed), (3840, 34_000));
    }

    /// The synchronous `WM_SIZE` Windows fires from inside `CreateWindowExW`
    /// for a minimised/zero-size window carries 0x0 — decoding must report
    /// it verbatim rather than panicking, and `resize_surface` is what
    /// clamps it to the 1x1 Direct2D requires.
    #[test]
    fn size_from_lparam_handles_a_zero_client_rect() {
        assert_eq!(size_from_lparam(0), (0, 0));
    }

    /// `wparam` carries a DPI (144), not a scale (1.5) — the divide by 96
    /// is the whole point.
    #[test]
    fn dpi_scale_from_wparam_converts_dpi_to_a_scale_ratio() {
        // 150% scaling: both words are 144, per WM_DPICHANGED's contract.
        let wparam = (144usize << 16) | 144usize;
        assert_eq!(dpi_scale_from_wparam(wparam), 1.5);
    }

    #[test]
    fn dpi_scale_from_wparam_maps_the_96_dpi_baseline_to_100_percent() {
        assert_eq!(dpi_scale_from_wparam((96usize << 16) | 96usize), 1.0);
    }

    /// The common HiDPI ladder Windows exposes in Display Settings.
    #[test]
    fn dpi_ratio_matches_the_standard_windows_scaling_steps() {
        assert_eq!(dpi_ratio(96), 1.0);
        assert_eq!(dpi_ratio(120), 1.25);
        assert_eq!(dpi_ratio(144), 1.5);
        assert_eq!(dpi_ratio(192), 2.0);
    }

    /// Never `inf`: a zero DPI degrades to unscaled rather than poisoning
    /// every subsequent layout computation with a non-finite scale.
    #[test]
    fn dpi_ratio_falls_back_to_unscaled_instead_of_dividing_by_zero() {
        let scale = dpi_ratio(0);
        assert!(scale.is_finite(), "scale must never be inf/NaN");
        assert_eq!(scale, 1.0);
    }

    /// LOWORD is x, HIWORD is y — same word order as `WM_SIZE`, but read
    /// as signed 16-bit values this time.
    #[test]
    fn point_from_lparam_reads_x_from_the_low_word() {
        let lparam = (200isize << 16) | 100isize;
        assert_eq!(point_from_lparam(lparam), (100, 200));
    }

    /// A drag that tracks outside the client rect (mouse capture) reports
    /// negative coordinates — `point_from_lparam` must sign-extend each
    /// 16-bit word individually rather than treating the whole `LPARAM`
    /// as one signed value the way [`super::size_from_lparam`] does *not*
    /// need to (client sizes are never negative).
    #[test]
    fn point_from_lparam_sign_extends_negative_coordinates() {
        // x = -5, y = -10, packed as their 16-bit two's-complement forms.
        let x = (-5i16) as u16 as isize;
        let y = (-10i16) as u16 as isize;
        let lparam = (y << 16) | x;
        assert_eq!(point_from_lparam(lparam), (-5, -10));
    }

    #[test]
    fn point_from_lparam_handles_the_origin() {
        assert_eq!(point_from_lparam(0), (0, 0));
    }

    /// `GET_WHEEL_DELTA_WPARAM` reads the *signed* high word — a forward
    /// wheel notch (120) and a backward one (-120) must round-trip.
    #[test]
    fn wheel_delta_from_wparam_reads_forward_and_backward_notches() {
        let forward = (120i16 as u16 as usize) << 16;
        assert_eq!(wheel_delta_from_wparam(forward), 120);

        let backward = ((-120i16) as u16 as usize) << 16;
        assert_eq!(wheel_delta_from_wparam(backward), -120);
    }

    /// The low word (key-state flags: `MK_CONTROL`, `MK_SHIFT`, …) must
    /// not leak into the decoded delta.
    #[test]
    fn wheel_delta_from_wparam_ignores_the_low_word_key_state_flags() {
        let wparam = (120usize << 16) | 0x0009; // MK_CONTROL | MK_SHIFT
        assert_eq!(wheel_delta_from_wparam(wparam), 120);
    }

    #[test]
    fn is_repeat_from_lparam_reads_bit_30() {
        assert!(!is_repeat_from_lparam(0));
        assert!(is_repeat_from_lparam(1 << 30));
        // Unrelated bits (e.g. bit 31 = transition state) don't affect it.
        assert!(is_repeat_from_lparam((1 << 30) | (1 << 31)));
        assert!(!is_repeat_from_lparam(1 << 29));
    }
}
