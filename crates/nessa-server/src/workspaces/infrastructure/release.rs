//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "f59d8af8e03ae1f9";
const IMAGE: &str = "bsh_SuwMO3FoyEUsQUOyr6Hm_hvHw6iyY85H-s6jRWjXszM";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
