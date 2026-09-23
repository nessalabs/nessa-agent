//! Select the best pinned release that a host can run.
//!
//! Pin order is presentation, not policy. Selection first filters by the
//! host's validated capabilities, then prefers the eligible artifact with the
//! greatest declared requirements. This applies only to releases that are
//! actually pinned; it does not imply that every vendor label or baseline
//! archive has been accepted.

use super::{HostPlatform, PinnedRelease};

/// Choose the most demanding eligible pinned release for `host`.
///
/// Returns `None` when no supplied release runs on the host. Consuming the
/// releases makes the selected pin an owned value that callers can carry to
/// store and installer ports without borrowing a parsed pin document.
pub fn preferred_release(
    releases: impl IntoIterator<Item = PinnedRelease>,
    host: &HostPlatform,
) -> Option<PinnedRelease> {
    releases
        .into_iter()
        .filter(|release| release.runs_on(host))
        .max_by_key(|release| release.requirements().demand())
}

#[cfg(test)]
#[path = "../../../tests/agent_install/release_selection.rs"]
mod tests;
