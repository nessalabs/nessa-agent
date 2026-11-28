//! The images this build makes cloud VMs from: the workspace image a
//! channel's VMs run, and the Cloud Box image, which also carries a gateway
//! of its own (ADR 1417): the runtime, the bootstrap helper and cloudflared
//! that `docs/guides/headless-gateway/install-cloud-box` installs.
const IMAGE: &str = "bsh_KC74wkwmxzdt57Y9MxwBdSJlBa75Lfjz0BvFC-6SuS4";
/// The Boat image new VMs are made from.
pub fn boat_image() -> Option<&'static str> {
    Some(IMAGE)
}

/// The Boat image a new Cloud Box is made from: the workspace image above
/// with `install-cloud-box` run in it at #1508's 682acb0 (runtime
/// fingerprint ecca94b4…, cloudflared 77e26d8d…).
const CLOUD_BOX_IMAGE: &str = "bsh_l16HvHN76Ci1eNzOYDKEef5CtvfmyPeapGIlgnodKm7";
pub fn boat_cloud_box_image() -> Option<&'static str> {
    Some(CLOUD_BOX_IMAGE)
}

/// The Agent37 template release new VMs are made from. A template lives in
/// one Agent37 workspace, so this alpha's VMs are made in the workspace it
/// was published to.
const AGENT37_TEMPLATE: &str = "nessa-runtime@1";
pub fn agent37_template() -> Option<&'static str> {
    Some(AGENT37_TEMPLATE)
}
