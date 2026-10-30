//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "a8e6fff6643172a9";
const IMAGE: &str = "bsh_tu8vxzXBBQhMDD8tkFO1pUy9pY4lKRXxGrl1mp-PAu4";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
