//! Credential-free worker image, pinned to the lease protocol it serves.
//! A source build with another protocol must publish its matching image before
//! enabling token-only setup; it cannot silently clone an incompatible worker.
// Saved logins (#1260) add `takesLogin` to a grant's terms and a login to a
// harness's start. A saved Codex key keeps one durable home for this host,
// so a later harness can resume its threads, with its credential store set
// by TOML key in that home's config, and an empty login variable is not the
// host's own sign-in. This build is not e753740e1590e4f6. The image id stays the published worker until a rebuilt
// image is published for this hash; a worker dialing in on the older
// protocol takes this gateway's build.
const PROTOCOL: &str = "239c696fd0e0b3f4";
const IMAGE: &str = "bsh_KC74wkwmxzdt57Y9MxwBdSJlBa75Lfjz0BvFC-6SuS4";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
