//! Auto-update seam — [`UpdateService`] and the feed/info/staged types it
//! trades in.
//!
//! This module defines the backend-agnostic *contract* for checking a
//! feed, downloading a new build, and applying it once staged. It ships
//! no real implementation: every method on the default
//! [`UpdateService`] (the one [`crate::PlatformServices::update_service`]
//! vends when a backend doesn't override it) returns
//! [`BackendError::Unsupported`]. A Sparkle (macOS), WinSparkle/MSIX
//! (Windows), or AppImage (Linux) implementation is a separate,
//! backend-specific follow-on — this module only has to give each of
//! those a trait to implement against.
//!
//! TUI cannot stage or apply a native update (there is no package format
//! a terminal app controls), but it still degrades honestly rather than
//! going fully silent: [`crate::tui::services::TuiPlatformServices`]
//! overrides [`UpdateService::download_and_stage`] to surface an
//! "update available" diagnostic — see that module for the mechanism —
//! before returning the same `Unsupported` every other backend's default
//! does, since no bytes are actually staged.

use crate::backend::{BackendError, ServiceResult};

/// Where to look for updates — currently just a feed URL, kept as its
/// own named struct (rather than a bare `&str` parameter) so a future
/// backend can grow it (a public key for signature verification, a
/// channel name) without changing [`UpdateService::check_for_update`]'s
/// signature.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UpdateFeed {
    /// URL of the update feed/appcast (format is implementation-defined —
    /// Sparkle's XML appcast, a WinSparkle feed, a JSON manifest, etc.).
    pub url: String,
}

impl UpdateFeed {
    /// Build a feed pointing at `url`.
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

/// A single available update, as reported by
/// [`UpdateService::check_for_update`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UpdateInfo {
    /// The update's version string, in whatever format the feed uses
    /// (semver, a build number, a date — this trait makes no assumption).
    pub version: String,
    /// Human-readable release notes for this version.
    pub notes: String,
    /// URL of the downloadable update artifact.
    pub url: String,
    /// Signature of the artifact at `url`, for verification before
    /// staging. Opaque to this trait — its format (a detached Ed25519
    /// signature, a DSA signature, a SHA-256 digest) is whatever the
    /// implementing backend's update mechanism expects.
    pub signature: String,
}

impl UpdateInfo {
    /// Build an [`UpdateInfo`] from its four fields, in `version, notes,
    /// url, signature` order.
    ///
    /// All four parameters share the same type, so a transposed pair of
    /// arguments still compiles — [`UpdateInfoBuilder`] (via
    /// [`Self::builder`]) names each field at the call site instead, for a
    /// caller that would rather not rely on getting the order right.
    pub fn new(
        version: impl Into<String>,
        notes: impl Into<String>,
        url: impl Into<String>,
        signature: impl Into<String>,
    ) -> Self {
        Self {
            version: version.into(),
            notes: notes.into(),
            url: url.into(),
            signature: signature.into(),
        }
    }

    /// Start building an [`UpdateInfo`] one named field at a time. See
    /// [`UpdateInfoBuilder`].
    pub fn builder(version: impl Into<String>) -> UpdateInfoBuilder {
        UpdateInfoBuilder::new(version)
    }
}

/// Named-field alternative to [`UpdateInfo::new`]'s four same-typed
/// positional arguments, reached via [`UpdateInfo::builder`]. `version` is
/// required up front — an `UpdateInfo` with no version string is never
/// meaningful — while `notes`, `url` and `signature` default to empty and
/// can be set in any order:
///
/// ```
/// use quadraui::UpdateInfo;
///
/// let info = UpdateInfo::builder("1.2.3")
///     .url("https://example.invalid/update.bin")
///     .signature("deadbeef")
///     .notes("bug fixes")
///     .build();
/// assert_eq!(info.version, "1.2.3");
/// ```
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct UpdateInfoBuilder {
    version: String,
    notes: String,
    url: String,
    signature: String,
}

impl UpdateInfoBuilder {
    /// Start a builder with `version` set; every other field starts empty.
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            ..Self::default()
        }
    }

    /// Set the release notes.
    pub fn notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = notes.into();
        self
    }

    /// Set the update artifact's URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Set the artifact's signature.
    pub fn signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = signature.into();
        self
    }

    /// Consume the builder, producing the finished [`UpdateInfo`].
    pub fn build(self) -> UpdateInfo {
        UpdateInfo {
            version: self.version,
            notes: self.notes,
            url: self.url,
            signature: self.signature,
        }
    }
}

/// An update that has been downloaded and verified, ready for
/// [`UpdateService::apply_on_quit`] to install it. Carries the
/// [`UpdateInfo`] it was staged from so a caller can still show "about
/// to install version X" without having kept its own copy around.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StagedUpdate {
    /// The update this staged artifact was downloaded from.
    pub info: UpdateInfo,
}

impl StagedUpdate {
    /// Wrap `info` as a staged update.
    pub fn new(info: UpdateInfo) -> Self {
        Self { info }
    }
}

/// The auto-update contract — check a feed, download and stage a new
/// build, apply it on the next quit. Backend-neutral by design: see the
/// module doc for which backends are expected to override which methods.
///
/// Every method defaults to [`BackendError::Unsupported`] so a backend
/// that implements none of them (every backend as of this trait landing)
/// still compiles and behaves honestly — a caller sees "no updater here"
/// rather than a silent no-op.
pub trait UpdateService {
    /// Query `feed` for a newer version than the one currently running.
    /// `Ok(None)` means the feed was reachable and reports no update
    /// available (not an error); `Ok(Some(info))` means one is. `Err`
    /// covers both "this backend has no updater" and a real failure to
    /// reach/parse the feed.
    fn check_for_update(&self, feed: &UpdateFeed) -> ServiceResult<Option<UpdateInfo>> {
        let _ = feed;
        Err(BackendError::Unsupported)
    }

    /// Download the artifact `info` points at, verify its signature, and
    /// stage it for installation.
    fn download_and_stage(&self, info: &UpdateInfo) -> ServiceResult<StagedUpdate> {
        let _ = info;
        Err(BackendError::Unsupported)
    }

    /// Arrange for `staged` to be installed the next time the app quits
    /// (or immediately, if the backend's update mechanism relaunches on
    /// its own — the exact timing is implementation-defined).
    ///
    /// **Invariant:** `staged` must be a [`StagedUpdate`] this same
    /// service instance produced from its own [`Self::download_and_stage`]
    /// — the artifact it points at has already been downloaded and
    /// signature-verified by that call. An implementation is free to
    /// install whatever `staged.info` describes without re-verifying it;
    /// a caller that hand-builds a `StagedUpdate` (via its public
    /// constructor) and passes it here is bypassing that verification.
    fn apply_on_quit(&self, staged: StagedUpdate) -> ServiceResult<()> {
        let _ = staged;
        Err(BackendError::Unsupported)
    }
}

/// [`UpdateService`] whose every method returns
/// [`BackendError::Unsupported`] — the default
/// [`crate::PlatformServices::update_service`] vends. A zero-sized
/// marker, not a handle: it inherits every default method body and
/// overrides none, so it carries no state of its own.
pub(crate) struct NoUpdateService;

impl UpdateService for NoUpdateService {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_feed_new_stores_the_url() {
        let feed = UpdateFeed::new("https://example.com/appcast.xml");
        assert_eq!(feed.url, "https://example.com/appcast.xml");
    }

    #[test]
    fn update_info_new_stores_all_four_fields() {
        let info = UpdateInfo::new("1.2.3", "bug fixes", "https://example.com/v1.2.3", "sig");
        assert_eq!(info.version, "1.2.3");
        assert_eq!(info.notes, "bug fixes");
        assert_eq!(info.url, "https://example.com/v1.2.3");
        assert_eq!(info.signature, "sig");
    }

    #[test]
    fn update_info_builder_fills_in_each_named_field() {
        let info = UpdateInfo::builder("1.2.3")
            .url("https://example.com/v1.2.3")
            .signature("sig")
            .notes("bug fixes")
            .build();
        assert_eq!(
            info,
            UpdateInfo::new("1.2.3", "bug fixes", "https://example.com/v1.2.3", "sig")
        );
    }

    #[test]
    fn update_info_builder_defaults_unset_fields_to_empty() {
        let info = UpdateInfo::builder("1.2.3").build();
        assert_eq!(info, UpdateInfo::new("1.2.3", "", "", ""));
    }

    #[test]
    fn staged_update_new_carries_its_info() {
        let info = UpdateInfo::new("1.2.3", "", "https://example.com/v1.2.3", "sig");
        let staged = StagedUpdate::new(info.clone());
        assert_eq!(staged.info, info);
    }

    #[test]
    fn no_update_service_reports_unsupported_everywhere() {
        let service = NoUpdateService;
        let feed = UpdateFeed::new("https://example.com/appcast.xml");
        assert_eq!(
            service.check_for_update(&feed),
            Err(BackendError::Unsupported)
        );

        let info = UpdateInfo::new("1.2.3", "notes", "https://example.com/v1.2.3", "sig");
        assert_eq!(
            service.download_and_stage(&info),
            Err(BackendError::Unsupported)
        );

        let staged = StagedUpdate::new(info);
        assert_eq!(
            service.apply_on_quit(staged),
            Err(BackendError::Unsupported)
        );
    }
}
