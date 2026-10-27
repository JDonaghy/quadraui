//! Power monitor + sleep-blocker seam — the types
//! [`crate::PlatformServices::on_battery`],
//! [`crate::PlatformServices::thermal_state`],
//! [`crate::PlatformServices::begin_power_save_block`]/
//! [`crate::PlatformServices::end_power_save_block`], and
//! [`crate::UiEvent::PowerStateChanged`] trade in.
//!
//! This module defines the backend-neutral *contract* only — querying
//! whether the machine is on battery, reading the OS's thermal-pressure
//! classification, and starting/stopping a sleep/suspend inhibitor. It
//! ships no real implementation: every [`PlatformServices`][crate::PlatformServices]
//! query here defaults to [`BackendError::Unsupported`][crate::BackendError::Unsupported],
//! and no backend emits [`crate::UiEvent::PowerStateChanged`] yet. Wiring a
//! real producer — IOKit's `IOPSCopyPowerSourcesInfo` and
//! `IOPMAssertionCreateWithName` on macOS, `GetSystemPowerStatus` and
//! `SetThreadExecutionState` on Win-GUI, `org.freedesktop.UPower` and
//! `org.freedesktop.login1`'s `Inhibit` call over D-Bus on GTK/Linux — is a
//! separate, backend-specific follow-on; this module only has to give each
//! of those a contract to implement against.
//!
//! TUI cannot query battery/thermal state and has no sleep-inhibitor API of
//! its own to call into, but [`crate::tui::services::TuiPlatformServices`]
//! still overrides [`PlatformServices::begin_power_save_block`][crate::PlatformServices::begin_power_save_block]/
//! [`PlatformServices::end_power_save_block`][crate::PlatformServices::end_power_save_block]
//! with an honest no-op blocker (bookkeeping-only — it mints and tracks a
//! real [`BlockerId`], it just never asks the OS to stay awake) rather than
//! leaving them `Unsupported`: a TUI app that starts a blocker and later
//! ends it should see a consistent, truthful lifecycle, not an error on the
//! very first call. TUI never emits [`crate::UiEvent::PowerStateChanged`] —
//! a terminal has no OS suspend/resume/AC notification to forward.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The kind of sleep/suspend inhibitor
/// [`crate::PlatformServices::begin_power_save_block`] starts, mirroring
/// Electron's `powerSaveBlocker` kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PowerSaveKind {
    /// Keep the display from sleeping/dimming (but allow the system to
    /// otherwise idle/suspend).
    PreventDisplaySleep,
    /// Keep the whole system from suspending (a superset of
    /// [`Self::PreventDisplaySleep`] on every platform that distinguishes
    /// the two).
    PreventAppSuspension,
}

/// The OS's thermal-pressure classification, as reported by
/// [`crate::PlatformServices::thermal_state`]. Ordered from coolest to
/// most severe; mirrors macOS's `ProcessInfo.ThermalState` the most
/// directly, since that is the only in-tree target platform with a
/// first-class four-level signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ThermalState {
    /// No thermal pressure — normal operation.
    Nominal,
    /// Mild thermal pressure; no user-visible impact expected yet.
    Fair,
    /// Significant thermal pressure; performance may already be reduced.
    Serious,
    /// Severe thermal pressure; the system is actively protecting itself
    /// (aggressive throttling, possible imminent shutdown).
    Critical,
}

/// Opaque handle to an in-flight sleep/suspend inhibitor, returned by
/// [`crate::PlatformServices::begin_power_save_block`] and consumed by
/// [`crate::PlatformServices::end_power_save_block`].
///
/// Allocator-issued, not caller-constructed — mirrors the monotonic-id
/// pattern in `descriptor_store.rs`/`flex.rs`/`desktop.rs`: a blocker id has
/// no meaningful string or numeric form a caller would ever want to pick
/// itself, only identity, so [`Self::next`] is the only way to mint one and
/// stays `pub(crate)` for backends' own `begin_power_save_block` overrides
/// to call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockerId(u64);

impl BlockerId {
    /// Mint the next globally unique [`BlockerId`]. Backend-internal: an
    /// app never constructs one of these itself, only receives one back
    /// from [`crate::PlatformServices::begin_power_save_block`].
    ///
    /// `#[cfg_attr(not(feature = "tui"), allow(dead_code))]` for the same
    /// reason `desktop.rs`'s `windows_explorer_select_parameters` carries
    /// one: this module is platform-neutral and compiles under every
    /// feature set, but today only `tui::services::TuiPlatformServices`'s
    /// `begin_power_save_block` override calls it in production code — a
    /// `win`/`gtk` override added later picks up the same call. The
    /// `#[cfg(test)]` module below exercises it regardless of feature set.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The payload of [`crate::UiEvent::PowerStateChanged`] — which power
/// transition occurred. `Serialize`/`Deserialize` (unlike [`PowerSaveKind`]/
/// [`ThermalState`] above) because it travels inside a `UiEvent`, and every
/// `UiEvent` payload satisfies that bound — see `event.rs`'s module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PowerEventKind {
    /// The system is about to suspend/sleep.
    Suspend,
    /// The system resumed from suspend/sleep.
    Resume,
    /// The system switched to running on battery power.
    OnBattery,
    /// The system switched to running on mains/AC power.
    OnAc,
    /// The screen locked.
    LockScreen,
    /// The screen unlocked.
    UnlockScreen,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocker_id_next_is_monotonically_unique() {
        let a = BlockerId::next();
        let b = BlockerId::next();
        let c = BlockerId::next();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn thermal_state_orders_from_coolest_to_most_severe() {
        assert!(ThermalState::Nominal < ThermalState::Fair);
        assert!(ThermalState::Fair < ThermalState::Serious);
        assert!(ThermalState::Serious < ThermalState::Critical);
    }

    #[test]
    fn power_save_kind_variants_are_distinct() {
        assert_ne!(
            PowerSaveKind::PreventDisplaySleep,
            PowerSaveKind::PreventAppSuspension
        );
    }

    #[test]
    fn power_event_kind_round_trips_through_json() {
        for kind in [
            PowerEventKind::Suspend,
            PowerEventKind::Resume,
            PowerEventKind::OnBattery,
            PowerEventKind::OnAc,
            PowerEventKind::LockScreen,
            PowerEventKind::UnlockScreen,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: PowerEventKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }
}
