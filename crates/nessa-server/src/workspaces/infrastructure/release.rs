//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
const PROTOCOL: &str = "b6e4f48aa694ce08";
const IMAGE: &str = "bsh_1WtCS3oK4SWwNbdpOnBXzFPq_PwIT1aGCwnJSQHWakU";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
