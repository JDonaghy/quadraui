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
}
