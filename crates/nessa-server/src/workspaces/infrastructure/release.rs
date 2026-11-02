//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "c516606214ce8bc2";
const IMAGE: &str = "bsh_MF9Ppg9XWHyVD8qYw7Wv6mku_yJpX2lS-Dlj4-LZ27E";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
