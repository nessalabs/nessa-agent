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
// The shared names and bounds moved out of the SDK into nessa-types (now in
// the fingerprint), and the env serve sources read time through
// crate::timer::clock, so this build is not main's 239c696fd0e0b3f4. The
// frames and their meaning are unchanged; the image id stays the published
// worker until a rebuilt image is published for this hash.
// Device pairing moved into its own crate (#1311), so env serve's dial
// imports its TLS stream from there, and this build is not main's
// 68607d55e45aebbc. Again the frames and their meaning are unchanged.
// Env serve then moved into its own crate (#1311): its sources are listed
// from there and import each other by crate path, so this build is not
// 9ebfca20fd69fe0f either, with the frames still unchanged.
const PROTOCOL: &str = "67da1d599b21a89f";
const IMAGE: &str = "bsh_KC74wkwmxzdt57Y9MxwBdSJlBa75Lfjz0BvFC-6SuS4";
pub(crate) fn worker_image() -> Option<&'static str> {
    (crate::env::LEASE_PROTOCOL == PROTOCOL).then_some(IMAGE)
}
