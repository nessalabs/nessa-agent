//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "a8cbfd3cb04e54cc";
const IMAGE: &str = "bsh_KC74wkwmxzdt57Y9MxwBdSJlBa75Lfjz0BvFC-6SuS4";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
