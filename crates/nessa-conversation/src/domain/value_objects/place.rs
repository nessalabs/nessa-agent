//! Where a conversation runs, as one value: here, a configured SSH host, one
//! paired computer, any paired computer (the pool), or a cloud VM of its own.
//!
//! A place is kept and asked for as a name: in a placement file and in a
//! creation request's `environment`. [`Place::named`] reads such a name and
//! [`Place::name`] prints one; a channel's columns use its own words
//! ([`super::ChannelExecution::parse`]). Code that decides where a
//! conversation runs matches on the value.
use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, PeerEnvironment, SshDestination};

/// Where a conversation runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// This gateway's own machine.
    Here,
    /// The machine this configured OpenSSH destination reaches.
    Host(SshDestination),
    /// The paired computer that proves this key when it dials in.
    Peer(PeerEnvironment),
    /// Any paired computer this gateway's pool may use; which one holds the
    /// conversation now is its worker binding, a separate fact.
    Pool,
    /// A cloud VM of the conversation's own, on the owner's Boat account.
    CloudVm,
}

/// A name that names no place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownPlace;

impl Place {
    /// The pool's name. An SSH destination holds no `:`, and no key is
    /// `any`, so it names nothing else.
    const POOL: &'static str = "peer:any";

    /// The place `name` names: `here`, `peer:any`, `boat:thread`, a paired
    /// computer's `peer:<key>`, or else an SSH destination.
    ///
    /// # Errors
    /// [`UnknownPlace`] for a name that is none of these.
    pub fn named(name: &str) -> Result<Self, UnknownPlace> {
        if name == Self::POOL {
            return Ok(Self::Pool);
        }
        EnvironmentRef::named(name)
            .map(Self::from)
            .map_err(|_| UnknownPlace)
    }

    /// Its name, as [`Self::named`] reads it.
    pub fn name(&self) -> String {
        match self {
            Self::Here => EnvironmentRef::HERE.to_owned(),
            Self::Host(host) => EnvironmentRef::Ssh(host.clone()).name(),
            Self::Peer(peer) => peer.name(),
            Self::Pool => Self::POOL.to_owned(),
            Self::CloudVm => EnvironmentRef::Boat.name(),
        }
    }

    /// Its name when it is away from here; `None` for here, which a
    /// placement file says by its absence.
    pub fn remote_name(&self) -> Option<String> {
        self.is_remote().then(|| self.name())
    }

    /// Whether it runs away from this gateway's machine.
    pub fn is_remote(&self) -> bool {
        *self != Self::Here
    }
}

impl From<EnvironmentRef> for Place {
    /// The place one lease's environment is in. A lease is always in one
    /// environment, so it is never the pool.
    fn from(environment: EnvironmentRef) -> Self {
        match environment {
            EnvironmentRef::Here => Self::Here,
            EnvironmentRef::Boat => Self::CloudVm,
            EnvironmentRef::Ssh(host) => Self::Host(host),
            EnvironmentRef::Peer(peer) => Self::Peer(peer),
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/place.rs"]
mod tests;
