//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
// Saved logins (#1260) add `takesLogin` to a grant's terms and a login to a
// harness's start, so this build is not main's c1804c100ef10b8e. The image id
// stays the published worker until a rebuilt image is published for this
// hash; a worker dialing in on the older protocol takes this gateway's build.
const PROTOCOL: &str = "d4045bc69e119d13";
const IMAGE: &str = "bsh_KC74wkwmxzdt57Y9MxwBdSJlBa75Lfjz0BvFC-6SuS4";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
