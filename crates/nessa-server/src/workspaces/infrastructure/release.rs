//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "9fb87f4768979b13";
const IMAGE: &str = "bsh_Q6hsvSC_2BkqDfLK1-jNkGxs5kGzLniNbNqdvQlkJhg";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
