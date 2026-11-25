//! The images this build makes cloud VMs from. A Cloud Box runs a gateway
//! of its own (ADR 1417), so these are the images its bootstrap helper
//! ships in; #1427 publishes the gateway image and pins it here, and Create
//! Cloud Box stays hidden until then.
const IMAGE: &str = "bsh_KC74wkwmxzdt57Y9MxwBdSJlBa75Lfjz0BvFC-6SuS4";
/// The Boat image new VMs are made from.
pub fn boat_image() -> Option<&'static str> {
    Some(IMAGE)
}

/// The Agent37 template release new VMs are made from. A template lives in
/// one Agent37 workspace, so this alpha's VMs are made in the workspace it
/// was published to.
const AGENT37_TEMPLATE: &str = "nessa-runtime@1";
pub fn agent37_template() -> Option<&'static str> {
    Some(AGENT37_TEMPLATE)
}
