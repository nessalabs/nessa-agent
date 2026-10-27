//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "066217f7701b98b9";
const IMAGE: &str = "bsh_cQ5cYJX64ZJth3NRQsovUo0R8WODUOV_3PXw3shYLuU";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
