//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "b698f20053f8bb7c";
const IMAGE: &str = "bsh_G4ziMuXnUzHK8dY91afTTrloPBNLhQh7zHrOyjMQfLw";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
