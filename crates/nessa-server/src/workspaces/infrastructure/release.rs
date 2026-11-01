//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "dad5afbc1bc5755f";
const IMAGE: &str = "bsh_-6mHzeuFu3FWNbJWOe0IbXLXxhBcJPNOjq76Bk_nMGI";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
