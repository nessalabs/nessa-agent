//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
// timer/mod.rs is in the fingerprint. The shared timer removed timer/runtime.rs
// from that list, and the Unix-only timeout export changed the file again, so
// this build is not main's f7cb3ca564647c78. The image id stays the published
// worker until a rebuilt image is published for this hash.
const PROTOCOL: &str = "c1804c100ef10b8e";
const IMAGE: &str = "bsh_KC74wkwmxzdt57Y9MxwBdSJlBa75Lfjz0BvFC-6SuS4";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
