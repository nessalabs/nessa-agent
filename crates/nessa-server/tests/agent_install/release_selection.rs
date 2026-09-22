//! Preferred release selection is a domain decision independent of pin order.

use super::*;
use crate::agent_install::domain::{
    ArchiveDigest, ArchivePath, ArchiveUrl, Libc, ReleasePlatform, ReleaseRequirements,
    ReleaseVersion,
};

fn host(avx2: bool, libc: Libc) -> HostPlatform {
    HostPlatform::new(
        ReleasePlatform::new("linux", "x86_64").unwrap(),
        Some(libc),
        avx2,
    )
}

fn release(libc: Libc, avx2: bool, digest: char) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse("1.18.31").unwrap(),
        ReleasePlatform::new("linux", "x86_64").unwrap(),
        ReleaseRequirements::new(Some(libc), avx2),
        ArchiveUrl::parse(&format!("https://example.test/opencode-{digest}.tar.gz")).unwrap(),
        ArchiveDigest::parse(&digest.to_string().repeat(64)).unwrap(),
        ArchivePath::parse("package/bin/opencode").unwrap(),
    )
    .unwrap()
}

#[test]
fn the_most_demanding_eligible_pin_wins_in_either_order() {
    let demanding = release(Libc::Gnu, true, 'a');
    let baseline = release(Libc::Gnu, false, 'b');
    for releases in [
        vec![demanding.clone(), baseline.clone()],
        vec![baseline, demanding.clone()],
    ] {
        assert_eq!(
            preferred_release(releases, &host(true, Libc::Gnu))
                .unwrap()
                .archive_digest(),
            demanding.archive_digest()
        );
    }
}

#[test]
fn requirements_filter_before_preference_and_none_remains_distinct() {
    let demanding = release(Libc::Gnu, true, 'a');
    let baseline = release(Libc::Gnu, false, 'b');
    assert_eq!(
        preferred_release(
            vec![demanding, baseline.clone()],
            &host(false, Libc::Gnu),
        )
        .unwrap()
        .archive_digest(),
        baseline.archive_digest()
    );
    assert_eq!(
        preferred_release(vec![baseline], &host(true, Libc::Musl)),
        None
    );
}
