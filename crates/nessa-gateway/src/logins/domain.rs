//! What a saved login is, where each is kept, and which paired machines
//! their owner switched on to receive them.
use nessa_protocol::agents::AgentId;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use nessa_vault::domain::EntryName;
use std::collections::BTreeSet;

/// The vault entry an agent's saved login is kept in: one per agent, so no
/// agent is ever handed another's.
#[must_use]
pub fn login_entry(agent: AgentId) -> EntryName {
    EntryName::new(match agent {
        AgentId::Claude => "login.claude",
        AgentId::Codex => "login.codex",
        AgentId::Opencode => "login.opencode",
    })
    .expect("static vault entry")
}

/// The vault entry the owner's per-device switches are kept in. Kept beside
/// the logins, so a locked vault hands no login and changes no switch.
#[must_use]
pub fn devices_entry() -> EntryName {
    EntryName::new("login.devices").expect("static vault entry")
}

/// The most paired machines a switch list holds: more than any Devices
/// list shows. `LoginsStatus.devicesOn.maxItems` in the product schema
/// publishes it, and protocol generation fails if the two differ.
pub const MAX_SWITCHED_ON: usize = 256;

/// The paired machines whose owner switched on "Use my saved logins here".
/// Every other paired machine is off. SSH hosts and Boat workspaces have no
/// switch: this gateway logs in as their account and can already set
/// anything a harness there runs with (the policy says so).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSwitches(BTreeSet<PeerEnvironment>);

/// A switch list that cannot be: too many machines, or one named twice or
/// not a paired machine's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSwitches;

impl DeviceSwitches {
    /// Whether `peer` is switched on.
    #[must_use]
    pub fn is_on(&self, peer: &PeerEnvironment) -> bool {
        self.0.contains(peer)
    }

    /// The machines switched on, in their names' order.
    pub fn on(&self) -> impl Iterator<Item = &PeerEnvironment> {
        self.0.iter()
    }

    /// These switches with `peer` turned `on` or off.
    ///
    /// # Errors
    /// [`InvalidSwitches`] when turning one more on passes
    /// [`MAX_SWITCHED_ON`].
    pub fn switched(&self, peer: PeerEnvironment, on: bool) -> Result<Self, InvalidSwitches> {
        let mut next = self.0.clone();
        if on {
            next.insert(peer);
        } else {
            next.remove(&peer);
        }
        if next.len() > MAX_SWITCHED_ON {
            return Err(InvalidSwitches);
        }
        Ok(Self(next))
    }

    /// Switches with exactly `peers` on.
    ///
    /// # Errors
    /// [`InvalidSwitches`] when a machine is named twice or there are more
    /// than [`MAX_SWITCHED_ON`].
    pub fn from_peers(
        peers: impl IntoIterator<Item = PeerEnvironment>,
    ) -> Result<Self, InvalidSwitches> {
        let mut on = BTreeSet::new();
        for peer in peers {
            if !on.insert(peer) || on.len() > MAX_SWITCHED_ON {
                return Err(InvalidSwitches);
            }
        }
        Ok(Self(on))
    }
}
